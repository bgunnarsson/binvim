//! Self-contained terminal model. Owns a PTY-backed child process
//! (typically a shell), a vte-parsed grid of cells, a cursor, and a
//! scrollback ring. Bytes from the PTY land in a channel filled by
//! a reader thread; `drain()` pulls them on demand and feeds them to
//! the vte parser, which calls callbacks on `VteHandler` that mutate
//! the grid + cursor. Input is the opposite direction: callers
//! invoke `write_bytes` and the bytes go straight to the PTY master.
//!
//! Intentionally not integrated into `App` / `Window` / the renderer
//! yet — that's a follow-up session. This module exists so the
//! terminal model can be developed and tested in isolation:
//!
//!   - PTY spawn + reader thread + write path
//!   - VT100 / xterm escape-sequence handling via the `vte` crate
//!     (CUP / CUU / CUD / CUF / CUB, ED / EL, SGR colour + attrs,
//!     IND / RI, simple SU / SD, line wrap, scrollback)
//!   - Grid + cursor model the renderer will eventually paint
//!
//! Currently `#[allow(dead_code)]` at the module level because the
//! integration layer doesn't exist yet. Tests at the bottom exercise
//! the public API end-to-end (`echo hello`, ANSI colour, cursor
//! moves, clear, scrollback) so the model is verified independently.

#![allow(dead_code)]

use anyhow::{Context, Result};
use crossterm::style::Color;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use vte::{Params, Parser, Perform};

/// One cell in the terminal grid. `ch` is the glyph; `fg`/`bg` are
/// the foreground / background colours (LSP-style — None means
/// "default", which the renderer translates to its palette
/// defaults). Boolean attrs cover the SGR flags terminals actually
/// use day-to-day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// "Reverse video" SGR (`\x1b[7m`). Many shells use this for
    /// selection / paging chrome; the renderer should swap fg/bg
    /// for cells with this set.
    pub reverse: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self::blank()
    }
}

impl Cell {
    pub const fn blank() -> Self {
        Self {
            ch: ' ',
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
            reverse: false,
        }
    }
}

/// The visible grid + the scrolled-off scrollback ring. The grid
/// is exactly `rows × cols` cells; the scrollback grows up to
/// `SCROLLBACK_CAP` lines and then drops the oldest from the front
/// as new lines are scrolled in.
pub struct Grid {
    pub rows: usize,
    pub cols: usize,
    pub cells: Vec<Vec<Cell>>,
    /// Lines that have scrolled off the top of the visible grid.
    /// Newest at the tail. Each entry is a full row of cells, sized
    /// `cols` wide at the moment it scrolled.
    pub scrollback: Vec<Vec<Cell>>,
}

/// Default scrollback size — `:terminal` panes don't need infinite
/// history, but a session's worth of commands fits comfortably in
/// 10k rows.
pub const SCROLLBACK_CAP: usize = 10_000;

impl Grid {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            cells: vec![vec![Cell::blank(); cols]; rows],
            scrollback: Vec::new(),
        }
    }

    fn clear(&mut self) {
        for row in &mut self.cells {
            for cell in row.iter_mut() {
                *cell = Cell::blank();
            }
        }
    }

    /// Scroll the visible grid up by one line. The top row is
    /// pushed into scrollback; the bottom row becomes blank.
    fn scroll_up(&mut self) {
        if self.cells.is_empty() {
            return;
        }
        let evicted = self.cells.remove(0);
        self.scrollback.push(evicted);
        if self.scrollback.len() > SCROLLBACK_CAP {
            let drop_n = self.scrollback.len() - SCROLLBACK_CAP;
            self.scrollback.drain(0..drop_n);
        }
        self.cells.push(vec![Cell::blank(); self.cols]);
    }

    fn resize(&mut self, rows: usize, cols: usize) {
        self.rows = rows;
        self.cols = cols;
        for row in &mut self.cells {
            row.resize(cols, Cell::blank());
        }
        self.cells.resize_with(rows, || vec![Cell::blank(); cols]);
    }
}

/// vte `Perform` impl. Owns the grid + cursor + current pen state.
/// Each escape-sequence callback the parser fires mutates one or
/// more of these. The parser keeps no state of its own beyond what
/// it needs to recognise sequences — so when we resize / reset /
/// re-init, the handler is what we touch.
pub struct VteHandler {
    pub(crate) grid: Grid,
    /// 0-based cursor row.
    pub(crate) cur_row: usize,
    /// 0-based cursor column.
    pub(crate) cur_col: usize,
    /// Saved cursor (DECSC / DECRC, `\x1b 7` / `\x1b 8`).
    saved: Option<(usize, usize)>,
    /// Pen state — applied to every printed cell until SGR resets it.
    pen: Cell,
    /// DEC private mode 1000 — X10 / VT200 mouse: report button
    /// press and release only.
    pub(crate) mouse_button_mode: bool,
    /// DEC private mode 1002 — button-event tracking: same as 1000
    /// plus drag motion events while a button is held.
    pub(crate) mouse_drag_mode: bool,
    /// DEC private mode 1003 — any-event tracking: motion regardless
    /// of button state. Almost no real program uses this — kept
    /// for completeness so the gating logic is correct.
    pub(crate) mouse_motion_mode: bool,
    /// DEC private mode 1006 — SGR mouse encoding. Modern xterm,
    /// works correctly past col 95 (the legacy 1000 encoding
    /// breaks above that because it stuffs coords into a single
    /// byte each). When set, mouse events go out as
    /// `\x1b[<{btn};{x};{y}{M|m}` instead of the legacy form.
    pub(crate) mouse_sgr: bool,
}

/// Snapshot of which mouse-tracking modes the terminal currently
/// has enabled. Used by the App's mouse-event handler to decide
/// whether to forward events to the PTY or pass them through to
/// the editor.
#[derive(Debug, Clone, Copy, Default)]
pub struct MouseModeState {
    pub any: bool,
    pub drag: bool,
    pub motion: bool,
    pub sgr: bool,
}

impl VteHandler {
    fn new(rows: usize, cols: usize) -> Self {
        Self {
            grid: Grid::new(rows, cols),
            cur_row: 0,
            cur_col: 0,
            saved: None,
            pen: Cell::blank(),
            mouse_button_mode: false,
            mouse_drag_mode: false,
            mouse_motion_mode: false,
            mouse_sgr: false,
        }
    }

    fn move_to(&mut self, row: usize, col: usize) {
        self.cur_row = row.min(self.grid.rows.saturating_sub(1));
        self.cur_col = col.min(self.grid.cols.saturating_sub(1));
    }

    /// Advance the cursor by one column. If it would step past the
    /// right edge, wrap onto the next row (scrolling if necessary).
    fn advance(&mut self) {
        self.cur_col += 1;
        if self.cur_col >= self.grid.cols {
            self.cur_col = 0;
            self.line_feed();
        }
    }

    /// Move down one row; if we'd fall off the bottom, scroll the
    /// grid up. This is what LF / IND do.
    fn line_feed(&mut self) {
        if self.cur_row + 1 >= self.grid.rows {
            self.grid.scroll_up();
        } else {
            self.cur_row += 1;
        }
    }

    fn carriage_return(&mut self) {
        self.cur_col = 0;
    }

    fn write_glyph(&mut self, c: char) {
        if self.cur_row < self.grid.rows && self.cur_col < self.grid.cols {
            let mut cell = self.pen;
            cell.ch = c;
            self.grid.cells[self.cur_row][self.cur_col] = cell;
        }
        self.advance();
    }

    fn apply_sgr(&mut self, params: &Params) {
        // SGR (`\x1b[...m`) is the colour / attr verb. We get a list
        // of integers; each one (or sometimes a 38;5;N / 38;2;R;G;B
        // sub-sequence) mutates the pen. Empty params = reset (same
        // as `0`).
        if params.is_empty() {
            self.pen = Cell::blank();
            return;
        }
        let mut iter = params.iter();
        while let Some(param) = iter.next() {
            let n = param.first().copied().unwrap_or(0);
            match n {
                0 => self.pen = Cell::blank(),
                1 => self.pen.bold = true,
                3 => self.pen.italic = true,
                4 => self.pen.underline = true,
                7 => self.pen.reverse = true,
                22 => self.pen.bold = false,
                23 => self.pen.italic = false,
                24 => self.pen.underline = false,
                27 => self.pen.reverse = false,
                30..=37 => self.pen.fg = Some(ansi_basic_colour(n - 30)),
                38 => {
                    if let Some(c) = parse_extended_colour(&mut iter) {
                        self.pen.fg = Some(c);
                    }
                }
                39 => self.pen.fg = None,
                40..=47 => self.pen.bg = Some(ansi_basic_colour(n - 40)),
                48 => {
                    if let Some(c) = parse_extended_colour(&mut iter) {
                        self.pen.bg = Some(c);
                    }
                }
                49 => self.pen.bg = None,
                90..=97 => self.pen.fg = Some(ansi_bright_colour(n - 90)),
                100..=107 => self.pen.bg = Some(ansi_bright_colour(n - 100)),
                _ => {} // unknown — silently drop
            }
        }
    }
}

/// Parse the `38;5;N` (256-colour) or `38;2;R;G;B` (truecolor) tail
/// after the leading 38 / 48. Advances the iterator past whatever
/// it consumed. Returns the resolved Color, or None if the form
/// wasn't recognised (in which case the iterator's position is
/// undefined — the caller treats it as a noop).
fn parse_extended_colour(
    iter: &mut vte::ParamsIter<'_>,
) -> Option<Color> {
    let kind = iter.next()?.first().copied()?;
    match kind {
        5 => {
            let idx = iter.next()?.first().copied()? as u8;
            Some(ansi_256(idx))
        }
        2 => {
            let r = iter.next()?.first().copied()? as u8;
            let g = iter.next()?.first().copied()? as u8;
            let b = iter.next()?.first().copied()? as u8;
            Some(Color::Rgb { r, g, b })
        }
        _ => None,
    }
}

fn ansi_basic_colour(n: u16) -> Color {
    match n {
        0 => Color::Black,
        1 => Color::DarkRed,
        2 => Color::DarkGreen,
        3 => Color::DarkYellow,
        4 => Color::DarkBlue,
        5 => Color::DarkMagenta,
        6 => Color::DarkCyan,
        7 => Color::Grey,
        _ => Color::Reset,
    }
}

fn ansi_bright_colour(n: u16) -> Color {
    match n {
        0 => Color::DarkGrey,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::White,
        _ => Color::Reset,
    }
}

fn ansi_256(idx: u8) -> Color {
    // Standard xterm 256-colour cube: 0-15 are the basic + bright
    // palette, 16-231 are a 6×6×6 RGB cube, 232-255 is a 24-step
    // grayscale ramp. Translate to truecolor so the renderer can
    // paint without consulting a separate lookup.
    if idx < 8 {
        return ansi_basic_colour(idx as u16);
    }
    if idx < 16 {
        return ansi_bright_colour((idx - 8) as u16);
    }
    if idx < 232 {
        let n = idx - 16;
        let r = n / 36;
        let g = (n % 36) / 6;
        let b = n % 6;
        let scale = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
        return Color::Rgb {
            r: scale(r),
            g: scale(g),
            b: scale(b),
        };
    }
    let v = 8 + (idx - 232) * 10;
    Color::Rgb { r: v, g: v, b: v }
}

impl Perform for VteHandler {
    fn print(&mut self, c: char) {
        self.write_glyph(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.line_feed(),
            b'\r' => self.carriage_return(),
            0x08 => {
                // Backspace — move cursor left, don't erase.
                if self.cur_col > 0 {
                    self.cur_col -= 1;
                }
            }
            b'\t' => {
                // Tab — advance to the next 8-column stop.
                let next = (self.cur_col / 8 + 1) * 8;
                let cap = self.grid.cols.saturating_sub(1);
                self.cur_col = next.min(cap);
            }
            // BEL, NUL, and the rest fall through silently.
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, c: char) {
        let first = params
            .iter()
            .next()
            .and_then(|p| p.first().copied())
            .unwrap_or(0);
        let second = params
            .iter()
            .nth(1)
            .and_then(|p| p.first().copied())
            .unwrap_or(0);
        match c {
            // CUP — Cursor Position. Both args 1-based per spec, 0 == 1.
            'H' | 'f' => {
                let row = (first as usize).saturating_sub(1).max(0);
                let col = (second as usize).saturating_sub(1).max(0);
                self.move_to(row, col);
            }
            'A' => {
                let n = (first as usize).max(1);
                self.cur_row = self.cur_row.saturating_sub(n);
            }
            'B' => {
                let n = (first as usize).max(1);
                self.cur_row = (self.cur_row + n).min(self.grid.rows.saturating_sub(1));
            }
            'C' => {
                let n = (first as usize).max(1);
                self.cur_col = (self.cur_col + n).min(self.grid.cols.saturating_sub(1));
            }
            'D' => {
                let n = (first as usize).max(1);
                self.cur_col = self.cur_col.saturating_sub(n);
            }
            'G' => {
                // CHA — Cursor Horizontal Absolute (1-based).
                let col = (first as usize).saturating_sub(1);
                self.cur_col = col.min(self.grid.cols.saturating_sub(1));
            }
            'J' => match first {
                0 => self.erase_below_cursor(),
                1 => self.erase_above_cursor(),
                2 | 3 => self.grid.clear(),
                _ => {}
            },
            'K' => match first {
                0 => self.erase_to_eol(),
                1 => self.erase_to_bol(),
                2 => self.erase_line(),
                _ => {}
            },
            'S' => {
                // SU — Scroll Up.
                let n = (first as usize).max(1);
                for _ in 0..n {
                    self.grid.scroll_up();
                }
            }
            'm' => self.apply_sgr(params),
            // Mode set / reset (`?...h` / `?...l`) — DEC private
            // mode bits. We honour the mouse-tracking modes (1000,
            // 1002, 1003, 1006) so click + scroll forwarding can
            // gate on what the program asked for. Other DEC modes
            // (cursor visibility, alt screen, bracketed paste, …)
            // are silently accepted so shells / TUIs don't think
            // we're broken.
            'h' | 'l' if intermediates.contains(&b'?') => {
                let enable = c == 'h';
                for param in params.iter() {
                    let n = param.first().copied().unwrap_or(0);
                    match n {
                        1000 => self.mouse_button_mode = enable,
                        1002 => self.mouse_drag_mode = enable,
                        1003 => self.mouse_motion_mode = enable,
                        1006 => self.mouse_sgr = enable,
                        _ => {}
                    }
                }
            }
            'h' | 'l' => {}
            _ => {} // unrecognised — drop silently
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'D' => self.line_feed(),    // IND
            b'M' => {
                // RI — Reverse Index. Move cursor up; if at top,
                // scroll down (we don't implement scroll-down so
                // just stay put — rare in real shells).
                if self.cur_row > 0 {
                    self.cur_row -= 1;
                }
            }
            b'7' => self.saved = Some((self.cur_row, self.cur_col)),
            b'8' => {
                if let Some((r, c)) = self.saved {
                    self.move_to(r, c);
                }
            }
            b'c' => {
                // RIS — full terminal reset.
                self.grid.clear();
                self.grid.scrollback.clear();
                self.cur_row = 0;
                self.cur_col = 0;
                self.saved = None;
                self.pen = Cell::blank();
            }
            _ => {}
        }
    }

    // OSC, DCS, and the hook/put/unhook trio are unused for our
    // purposes — window titles, terminfo strings, sixel graphics.
    // Default impls drop silently.
    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}
    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _c: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
}

impl VteHandler {
    fn erase_to_eol(&mut self) {
        let row = self.cur_row;
        let from = self.cur_col;
        if row >= self.grid.rows {
            return;
        }
        for col in from..self.grid.cols {
            self.grid.cells[row][col] = Cell::blank();
        }
    }
    fn erase_to_bol(&mut self) {
        let row = self.cur_row;
        if row >= self.grid.rows {
            return;
        }
        let to = self.cur_col.min(self.grid.cols.saturating_sub(1));
        for col in 0..=to {
            self.grid.cells[row][col] = Cell::blank();
        }
    }
    fn erase_line(&mut self) {
        let row = self.cur_row;
        if row >= self.grid.rows {
            return;
        }
        for cell in &mut self.grid.cells[row] {
            *cell = Cell::blank();
        }
    }
    fn erase_below_cursor(&mut self) {
        self.erase_to_eol();
        for row in (self.cur_row + 1)..self.grid.rows {
            for cell in &mut self.grid.cells[row] {
                *cell = Cell::blank();
            }
        }
    }
    fn erase_above_cursor(&mut self) {
        self.erase_to_bol();
        for row in 0..self.cur_row {
            for cell in &mut self.grid.cells[row] {
                *cell = Cell::blank();
            }
        }
    }
}

/// One running PTY-backed terminal session. Wraps the master
/// half of the PTY pair, a reader thread that funnels child output
/// into a channel, and the vte parser + grid that consume it.
pub struct Terminal {
    /// Master half of the PTY pair. Held inside a Mutex because
    /// both the reader thread (via the reader handle it cloned)
    /// and the main thread (via `write_bytes` → `writer`) need
    /// async access. `Box<dyn …>` because `portable_pty` returns
    /// trait objects.
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    /// Cached writer handle for the PTY master — taken once at
    /// spawn time so we don't lock the master mutex on every keystroke.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Channel filled by the reader thread with raw output bytes.
    rx: Receiver<Vec<u8>>,
    /// vte parser + the handler the parser drives. Lives behind a
    /// Mutex so a future renderer-side read can lock for a snapshot
    /// without racing with `drain`.
    inner: Mutex<TerminalInner>,
}

pub struct TerminalInner {
    pub(crate) parser: Parser,
    pub(crate) handler: VteHandler,
    /// Set once `is_alive()` discovers the child has died, so we
    /// don't keep polling its exit status.
    pub(crate) exited: bool,
}

impl Terminal {
    /// Spawn `shell` (default `$SHELL`, falling back to `/bin/sh`)
    /// in a `rows × cols` PTY. The reader thread starts immediately;
    /// callers should drive `drain()` on each frame so the grid
    /// reflects the latest output.
    pub fn spawn(rows: u16, cols: u16, shell: Option<&str>) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty failed")?;

        let shell_cmd = match shell {
            Some(s) => s.to_string(),
            None => std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
        };
        let mut cmd = CommandBuilder::new(&shell_cmd);
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        // TERM=xterm-256color gives shells the modern colour
        // expectations without lying about features we don't have
        // (alternate screen, mouse modes will quietly noop on our
        // side — they're handled by our outer terminal anyway).
        cmd.env("TERM", "xterm-256color");

        let _child = pair
            .slave
            .spawn_command(cmd)
            .context("spawn_command failed")?;
        // Drop the slave fd so the child becomes the sole owner.
        // Without this, the kernel never notices the child has
        // closed its side and `read` blocks forever after exit.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .context("try_clone_reader failed")?;
        let writer = pair
            .master
            .take_writer()
            .context("take_writer failed")?;

        let (tx, rx) = channel::<Vec<u8>>();
        spawn_reader(reader, tx);

        Ok(Self {
            master: Arc::new(Mutex::new(pair.master)),
            writer: Arc::new(Mutex::new(writer)),
            rx,
            inner: Mutex::new(TerminalInner {
                parser: Parser::new(),
                handler: VteHandler::new(rows as usize, cols as usize),
                exited: false,
            }),
        })
    }

    /// Forward `bytes` to the PTY master — i.e. into the child's
    /// stdin. Used to wire user keystrokes through to the shell.
    pub fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes)?;
        w.flush()?;
        Ok(())
    }

    /// Pull any pending PTY output off the channel and feed it to
    /// the vte parser. Cheap when nothing's queued. Call once per
    /// frame (or whenever `try_recv` would have something to give).
    /// Returns the number of bytes processed — main loop uses this
    /// to decide whether to re-render.
    pub fn drain(&self) -> usize {
        let mut total = 0;
        let mut inner = self.inner.lock().unwrap();
        while let Ok(chunk) = self.rx.try_recv() {
            // vte 0.15's Parser::advance takes a byte slice and
            // streams through it — pass the whole chunk at once
            // rather than looping byte-by-byte. The borrow-split
            // is a quirk: parser and handler are sibling fields
            // so we need separate &mut.
            let TerminalInner {
                parser, handler, ..
            } = &mut *inner;
            parser.advance(handler, &chunk);
            total += chunk.len();
        }
        total
    }

    pub fn grid(&self) -> std::sync::MutexGuard<'_, TerminalInner> {
        self.inner.lock().unwrap()
    }

    pub fn cursor(&self) -> (usize, usize) {
        let inner = self.inner.lock().unwrap();
        (inner.handler.cur_row, inner.handler.cur_col)
    }

    /// Snapshot of which DEC private mouse modes the inner program
    /// has enabled. Callers compare against this to decide whether
    /// to forward mouse events into the PTY.
    pub fn mouse_state(&self) -> MouseModeState {
        let inner = self.inner.lock().unwrap();
        MouseModeState {
            any: inner.handler.mouse_button_mode
                || inner.handler.mouse_drag_mode
                || inner.handler.mouse_motion_mode,
            drag: inner.handler.mouse_drag_mode,
            motion: inner.handler.mouse_motion_mode,
            sgr: inner.handler.mouse_sgr,
        }
    }

    /// Resize the PTY and the grid. The child gets a SIGWINCH so
    /// it can redraw at the new dimensions.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .lock()
            .unwrap()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("pty resize failed")?;
        let mut inner = self.inner.lock().unwrap();
        inner.handler.grid.resize(rows as usize, cols as usize);
        // Clamp the cursor in case the resize shrank under it.
        let max_row = inner.handler.grid.rows.saturating_sub(1);
        let max_col = inner.handler.grid.cols.saturating_sub(1);
        inner.handler.cur_row = inner.handler.cur_row.min(max_row);
        inner.handler.cur_col = inner.handler.cur_col.min(max_col);
        Ok(())
    }
}

impl TerminalInner {
    pub fn grid(&self) -> &Grid {
        &self.handler.grid
    }
    pub fn cursor(&self) -> (usize, usize) {
        (self.handler.cur_row, self.handler.cur_col)
    }
}

fn spawn_reader(mut reader: Box<dyn Read + Send>, tx: Sender<Vec<u8>>) {
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Drives a fresh parser+handler over a byte slice without the
    /// PTY round-trip — pure model test, no external processes.
    fn parse_bytes(bytes: &[u8], rows: usize, cols: usize) -> VteHandler {
        let mut parser = Parser::new();
        let mut handler = VteHandler::new(rows, cols);
        parser.advance(&mut handler, bytes);
        handler
    }

    fn line_text(grid: &Grid, row: usize) -> String {
        grid.cells[row]
            .iter()
            .map(|c| c.ch)
            .collect::<String>()
            .trim_end_matches(' ')
            .to_string()
    }

    #[test]
    fn plain_print_advances_cursor() {
        let h = parse_bytes(b"hello", 4, 20);
        assert_eq!(line_text(&h.grid, 0), "hello");
        assert_eq!((h.cur_row, h.cur_col), (0, 5));
    }

    #[test]
    fn lf_then_cr_moves_to_next_row_col_0() {
        let h = parse_bytes(b"ab\r\ncd", 4, 20);
        assert_eq!(line_text(&h.grid, 0), "ab");
        assert_eq!(line_text(&h.grid, 1), "cd");
        assert_eq!((h.cur_row, h.cur_col), (1, 2));
    }

    #[test]
    fn cup_moves_cursor_1_based() {
        // CUP 3;5 = row 3 col 5 (1-based) → (2, 4) 0-based.
        let h = parse_bytes(b"\x1b[3;5Hx", 4, 20);
        assert_eq!(h.grid.cells[2][4].ch, 'x');
        assert_eq!((h.cur_row, h.cur_col), (2, 5));
    }

    #[test]
    fn cursor_wrap_at_eol_moves_to_next_row() {
        let h = parse_bytes(b"abcde", 4, 3);
        // 'a' 'b' 'c' fill row 0; 'd' wraps to row 1 col 0; 'e' col 1.
        assert_eq!(line_text(&h.grid, 0), "abc");
        assert_eq!(line_text(&h.grid, 1), "de");
        assert_eq!((h.cur_row, h.cur_col), (1, 2));
    }

    #[test]
    fn lf_at_bottom_scrolls_and_pushes_into_scrollback() {
        // 4 rows. Print 5 lines — last `\r\n` pushes row 0 into
        // scrollback. LF on its own moves down without resetting
        // the column (terminal raw mode); the tty layer is what
        // turns `\n` into `\r\n` for cooked output. We model the
        // raw behaviour so a shell running on top of us sees
        // exactly what it'd see on a real terminal.
        let h = parse_bytes(b"a\r\nb\r\nc\r\nd\r\ne", 4, 5);
        assert_eq!(line_text(&h.grid, 0), "b");
        assert_eq!(line_text(&h.grid, 3), "e");
        assert_eq!(h.grid.scrollback.len(), 1);
        let scrolled = &h.grid.scrollback[0];
        assert_eq!(scrolled[0].ch, 'a');
    }

    #[test]
    fn sgr_bold_red_then_reset() {
        // \x1b[1;31m sets bold + red fg. Then "AB". Then \x1b[0m
        // resets. Then "C". A/B should be bold+red, C plain.
        let h = parse_bytes(b"\x1b[1;31mAB\x1b[0mC", 2, 10);
        assert!(h.grid.cells[0][0].bold);
        assert_eq!(h.grid.cells[0][0].fg, Some(Color::DarkRed));
        assert!(h.grid.cells[0][1].bold);
        assert!(!h.grid.cells[0][2].bold);
        assert_eq!(h.grid.cells[0][2].fg, None);
        assert_eq!(h.grid.cells[0][2].ch, 'C');
    }

    #[test]
    fn sgr_truecolour_24bit() {
        // \x1b[38;2;255;128;0m — orange foreground.
        let h = parse_bytes(b"\x1b[38;2;255;128;0mX", 1, 3);
        assert_eq!(
            h.grid.cells[0][0].fg,
            Some(Color::Rgb { r: 255, g: 128, b: 0 })
        );
    }

    #[test]
    fn ed_2_clears_whole_grid() {
        let h = parse_bytes(b"line1\nline2\nline3\x1b[2J", 4, 10);
        for row in 0..h.grid.rows {
            assert_eq!(line_text(&h.grid, row), "");
        }
    }

    #[test]
    fn el_0_clears_to_eol_from_cursor() {
        // print "hello world" then CR to col 0 then CUF 5 (move to
        // col 5) then EL 0 (clear right of cursor).
        let h = parse_bytes(b"hello world\r\x1b[5C\x1b[0K", 1, 20);
        assert_eq!(line_text(&h.grid, 0), "hello");
    }

    #[test]
    fn decset_mouse_modes_toggle() {
        // `CSI ? 1006 h` then `CSI ? 1000 h` enable two mouse
        // modes. `CSI ? 1000 l` disables one. Bare `CSI ? 25 h`
        // (cursor visibility) is unrelated and shouldn't perturb
        // mouse state.
        let h = parse_bytes(
            b"\x1b[?1006h\x1b[?1000h\x1b[?25h\x1b[?1000l",
            4, 10,
        );
        assert!(h.mouse_sgr, "SGR mode should still be on");
        assert!(!h.mouse_button_mode, "1000 should have been disabled");
        assert!(!h.mouse_drag_mode);
        assert!(!h.mouse_motion_mode);
    }

    #[test]
    fn decset_alt_screen_is_accepted_silently() {
        // The alt-screen toggle (1049) isn't modelled but must not
        // panic / leak state into other handlers.
        let h = parse_bytes(b"\x1b[?1049hhello\x1b[?1049l", 4, 10);
        assert_eq!(line_text(&h.grid, 0), "hello");
    }

    #[test]
    fn save_restore_cursor_roundtrip() {
        // print at (0,0) → save → move → write → restore → write at
        // saved position.
        let h = parse_bytes(b"\x1b7\x1b[3;4HX\x1b8Y", 5, 10);
        assert_eq!(h.grid.cells[2][3].ch, 'X');
        assert_eq!(h.grid.cells[0][0].ch, 'Y');
    }

    // -----------------------------------------------------------
    // End-to-end PTY tests — actually fork /bin/sh, write a command,
    // read the output back. Slow + non-hermetic (depends on /bin/sh
    // being present), but they catch the integration of the PTY
    // layer + reader thread + parser that the pure-model tests can't.
    // -----------------------------------------------------------

    /// Wait up to `max` for `cond()` to return true. Used so tests
    /// don't hang forever when the PTY misbehaves and don't burn
    /// CPU spinning when it just needs a moment.
    fn wait_for(max: Duration, mut cond: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < max {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn end_to_end_echo_appears_in_grid() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let term = Terminal::spawn(8, 40, Some("/bin/sh")).expect("spawn shell");
        term.write_bytes(b"echo binvim-pty-marker\nexit\n")
            .expect("write to pty");
        let mut found = false;
        wait_for(Duration::from_secs(3), || {
            term.drain();
            let inner = term.inner.lock().unwrap();
            for row in &inner.handler.grid.cells {
                let line: String = row.iter().map(|c| c.ch).collect();
                if line.contains("binvim-pty-marker") {
                    found = true;
                    return true;
                }
            }
            for row in &inner.handler.grid.scrollback {
                let line: String = row.iter().map(|c| c.ch).collect();
                if line.contains("binvim-pty-marker") {
                    found = true;
                    return true;
                }
            }
            false
        });
        assert!(found, "did not see echo output within 3s");
    }

    #[test]
    fn end_to_end_resize_does_not_panic() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let term = Terminal::spawn(8, 40, Some("/bin/sh")).expect("spawn shell");
        term.resize(12, 60).expect("resize ok");
        let inner = term.inner.lock().unwrap();
        assert_eq!(inner.handler.grid.rows, 12);
        assert_eq!(inner.handler.grid.cols, 60);
    }
}
