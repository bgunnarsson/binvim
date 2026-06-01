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

use crate::ansi::{ansi_256, ansi_basic_colour, ansi_bright_colour};
use anyhow::{Context, Result};
use crossterm::style::Color;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use unicode_width::UnicodeWidthChar;
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
    /// How far the renderer's view is shifted up from the live tail,
    /// in lines. `0` follows live output; `K` shows the slice of
    /// scrollback + grid ending `K` lines above the live cursor row.
    /// Bumped automatically by `scroll_up()` so the user stays
    /// anchored to the same content as new output streams in
    /// (tmux/screen behaviour). Clamped to `scrollback.len()`.
    pub view_scroll: usize,
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
            view_scroll: 0,
        }
    }

    /// Fetch the visible row at index `row` (0-based from the top of
    /// the visible window), accounting for the current `view_scroll`.
    /// Stitches the tail of scrollback to the head of `cells` so the
    /// renderer can stay agnostic of where each line lives. Returns
    /// `None` if `row` overshoots the combined buffer (shouldn't
    /// happen when the renderer respects `rows`, but defensive).
    pub fn visible_row(&self, row: usize) -> Option<&Vec<Cell>> {
        let sb_len = self.scrollback.len();
        let combined_top = sb_len.saturating_sub(self.view_scroll);
        let idx = combined_top + row;
        if idx < sb_len {
            self.scrollback.get(idx)
        } else {
            self.cells.get(idx - sb_len)
        }
    }

    /// Adjust `view_scroll` by `delta` lines (positive = scroll up
    /// into history, negative = scroll down toward live), clamping to
    /// `[0, scrollback.len()]`. Returns the new offset.
    pub fn scroll_view_by(&mut self, delta: isize) -> usize {
        let max = self.scrollback.len() as isize;
        let next = (self.view_scroll as isize + delta).clamp(0, max);
        self.view_scroll = next as usize;
        self.view_scroll
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
        // Keep the user anchored to the same content while output
        // streams in (tmux/screen behaviour). Without this, new
        // lines arriving in the live grid would slide the user's
        // scrolled-back view upward through their own history.
        if self.view_scroll > 0 {
            self.view_scroll += 1;
        }
        if self.scrollback.len() > SCROLLBACK_CAP {
            let drop_n = self.scrollback.len() - SCROLLBACK_CAP;
            self.scrollback.drain(0..drop_n);
            // After eviction the view can no longer reach as far
            // back — clamp before the renderer next reads it.
            if self.view_scroll > self.scrollback.len() {
                self.view_scroll = self.scrollback.len();
            }
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

    /// Flatten scrollback + visible grid into one `Vec<String>`.
    /// Trailing whitespace per row is trimmed so the result reads
    /// like the user would see it in a regular log file. Used by the
    /// task runner's quickfix scrape — colour info on each `Cell` is
    /// dropped (the scraper only cares about `path:line:col:` shapes).
    pub fn text_lines(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.scrollback.len() + self.cells.len());
        for row in self.scrollback.iter().chain(self.cells.iter()) {
            let s: String = row.iter().map(|c| c.ch).collect();
            out.push(s.trim_end().to_string());
        }
        out
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
    /// xterm DECAWM "deferred wrap" — writing a glyph that lands at
    /// the last column leaves the cursor *at* that column with this
    /// flag set; the wrap only fires when the next glyph is written.
    /// CR / explicit cursor motion / LF / SGR-style erases all clear
    /// the flag. Without this, zsh's `PROMPT_SP` trick (print
    /// `width` spaces + `\r` whether or not the prev output ended
    /// with `\n`) wraps one row too eagerly and a stray blank row
    /// shows up between every consecutive prompt.
    pending_wrap: bool,
    /// xterm DEC private modes 47 / 1047 / 1049 — when a TUI like
    /// vim, htop, claude, opencode enters alt-screen we stash the
    /// current main-screen state here and clear the live grid so
    /// the TUI gets a fresh canvas. On exit we swap it back. The
    /// `Option` is `Some` exactly when alt-screen is active.
    saved_main: Option<SavedScreen>,
    /// DEC private mode 25 (DECTCEM) — cursor visibility. TUIs that
    /// render their own caret (claude / opencode / codex are all
    /// built on frameworks that hide the hardware cursor on startup
    /// with `\x1b[?25l` and draw their own) rely on the terminal
    /// respecting this. Defaults to `true` (visible) so a plain
    /// shell, which never touches DECTCEM, keeps its cursor.
    cursor_visible: bool,
    /// DEC private mode 2004 — bracketed paste. TUIs that distinguish
    /// typed input from pasted input (claude / codex / opencode, and
    /// most shells with a modern readline) request it; when set, a
    /// paste must be wrapped in `\x1b[200~ … \x1b[201~` so the program
    /// treats the whole blob as one paste instead of a stream of
    /// keystrokes — without this, interior newlines read as Enter and
    /// the AI panes fire one message per line.
    pub(crate) bracketed_paste: bool,
}

/// Snapshot of the main screen taken when a TUI enters alt-screen
/// mode. Restored verbatim on exit so the shell prompt the user
/// was looking at before reappears intact.
pub(crate) struct SavedScreen {
    pub cells: Vec<Vec<Cell>>,
    pub cur_row: usize,
    pub cur_col: usize,
    pub pen: Cell,
    pub pending_wrap: bool,
    pub saved: Option<(usize, usize)>,
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
            pending_wrap: false,
            saved_main: None,
            cursor_visible: true,
            bracketed_paste: false,
        }
    }

    /// Enter alt-screen: snapshot the current grid + cursor + pen,
    /// then wipe the live grid so the TUI starts on a clean canvas.
    /// No-op if alt-screen is already active (a second 1049h shouldn't
    /// stack snapshots — the inner program would never recover the
    /// outer shell's prompt).
    fn enter_alt_screen(&mut self) {
        if self.saved_main.is_some() {
            return;
        }
        self.saved_main = Some(SavedScreen {
            cells: self.grid.cells.clone(),
            cur_row: self.cur_row,
            cur_col: self.cur_col,
            pen: self.pen,
            pending_wrap: self.pending_wrap,
            saved: self.saved,
        });
        self.grid.clear();
        self.cur_row = 0;
        self.cur_col = 0;
        self.pen = Cell::blank();
        self.pending_wrap = false;
    }

    /// Exit alt-screen: restore the snapshot taken on entry. No-op if
    /// alt-screen isn't active (a stray 1049l before an h would
    /// otherwise wipe live shell output).
    fn exit_alt_screen(&mut self) {
        let Some(saved) = self.saved_main.take() else {
            return;
        };
        // Restored cells might be shorter / narrower than the current
        // grid (resize while inside alt-screen). Pad rows + columns
        // back out to the live grid dimensions with blanks so the
        // renderer doesn't read past the end of any row.
        let mut cells = saved.cells;
        for row in cells.iter_mut() {
            if row.len() < self.grid.cols {
                row.resize(self.grid.cols, Cell::blank());
            } else if row.len() > self.grid.cols {
                row.truncate(self.grid.cols);
            }
        }
        while cells.len() < self.grid.rows {
            cells.push(vec![Cell::blank(); self.grid.cols]);
        }
        if cells.len() > self.grid.rows {
            cells.truncate(self.grid.rows);
        }
        self.grid.cells = cells;
        self.cur_row = saved.cur_row.min(self.grid.rows.saturating_sub(1));
        self.cur_col = saved.cur_col.min(self.grid.cols.saturating_sub(1));
        self.pen = saved.pen;
        self.pending_wrap = saved.pending_wrap;
        self.saved = saved.saved;
        // A TUI that hid the cursor (`?25l`) is expected to restore it
        // on exit, but not all do. The shell prompt we're swapping back
        // to assumes a visible cursor, so force it back on rather than
        // leave the user with no caret if the TUI was sloppy.
        self.cursor_visible = true;
    }

    /// True when a TUI has switched us into alt-screen mode. Used by
    /// the side-terminal pane's loading-screen heuristic — once a TUI
    /// is rendering into the alt buffer we know it's settled enough
    /// to drop the loading splash.
    pub fn alt_screen_active(&self) -> bool {
        self.saved_main.is_some()
    }

    fn move_to(&mut self, row: usize, col: usize) {
        self.cur_row = row.min(self.grid.rows.saturating_sub(1));
        self.cur_col = col.min(self.grid.cols.saturating_sub(1));
        self.pending_wrap = false;
    }

    /// Move down one row; if we'd fall off the bottom, scroll the
    /// grid up. This is what LF / IND do. Also clears any deferred
    /// wrap — once we've explicitly moved down, the "still on last
    /// col" state is gone.
    fn line_feed(&mut self) {
        if self.cur_row + 1 >= self.grid.rows {
            self.grid.scroll_up();
        } else {
            self.cur_row += 1;
        }
        self.pending_wrap = false;
    }

    fn carriage_return(&mut self) {
        self.cur_col = 0;
        self.pending_wrap = false;
    }

    fn write_glyph(&mut self, c: char) {
        // East Asian Wide glyphs and most emoji occupy two display
        // cells; combining marks zero. We honour the unicode-width
        // standard so the host terminal's painted width matches our
        // grid's column accounting — without this, a wide char like
        // ⚡ in a zsh prompt shifts the cursor visually by one cell
        // every time it appears, and `place_cursor` ends up
        // pointing at the previous character instead of the
        // next-write position.
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if w == 0 {
            // Combining mark — merge into the previous cell's glyph
            // rather than write to its own column. Simple model:
            // append to the most recently written cell's char. If
            // there's no previous cell on this row, drop it (rare).
            if self.cur_col > 0 {
                let prev_col = self.cur_col - 1;
                if self.cur_row < self.grid.rows {
                    // Skip the merge for now — keeping the base char
                    // alone is visually correct for the common ASCII
                    // case. A future pass could store grapheme
                    // clusters per cell.
                    let _ = prev_col;
                }
            }
            return;
        }
        // Resolve a deferred wrap from the previous glyph BEFORE
        // writing this one. xterm's DECAWM rule: the wrap fires on
        // the next glyph, not when the cursor reaches the margin.
        if self.pending_wrap {
            self.pending_wrap = false;
            self.cur_col = 0;
            self.line_feed();
        }
        if self.cur_row >= self.grid.rows {
            return;
        }
        if self.cur_col >= self.grid.cols {
            // Defensive — shouldn't happen given pending_wrap above,
            // but guards against drift if cols shrunk under us.
            self.cur_col = 0;
            self.line_feed();
            if self.cur_row >= self.grid.rows {
                return;
            }
        }
        let mut cell = self.pen;
        cell.ch = c;
        self.grid.cells[self.cur_row][self.cur_col] = cell;
        // Wide char — reserve the next column as a continuation
        // marker so the renderer knows to skip it (otherwise it'd
        // emit the cell's default ' ' and steal a column the host
        // terminal already painted with the wide glyph's right
        // half). `\0` is the convention.
        if w == 2 && self.cur_col + 1 < self.grid.cols {
            let mut cont = self.pen;
            cont.ch = '\0';
            self.grid.cells[self.cur_row][self.cur_col + 1] = cont;
        }
        // Advance — but defer the wrap if this glyph filled the row.
        // The cursor visually stays at the last column with
        // pending_wrap set; the NEXT glyph will resolve the wrap.
        // CR / cursor motion clears it without ever wrapping, which
        // is what makes zsh's PROMPT_SP trick (fill row + CR) leave
        // the cursor on the same row.
        let next_col = self.cur_col + w;
        if next_col >= self.grid.cols {
            self.cur_col = self.grid.cols.saturating_sub(1);
            self.pending_wrap = true;
        } else {
            self.cur_col = next_col;
        }
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
                4 => {
                    // Kitty / vte extended underline-style: `\e[4m` (bare)
                    // is single underline, but `\e[4:0m` means "no
                    // underline" (sub-params 1..5 = single/double/curly/
                    // dotted/dashed). Claude Code wraps hyperlinks with
                    // `\e[4:3m ... \e[4:0m`; reading the bare primary
                    // would leave underline stuck on after the link ends
                    // and smear it across every cell that followed.
                    let style = param.get(1).copied().unwrap_or(1);
                    self.pen.underline = style != 0;
                }
                7 => self.pen.reverse = true,
                22 => self.pen.bold = false,
                23 => self.pen.italic = false,
                24 => self.pen.underline = false,
                27 => self.pen.reverse = false,
                30..=37 => self.pen.fg = Some(ansi_basic_colour(n - 30)),
                38 => {
                    if let Some(c) = parse_extended_colour(param, &mut iter) {
                        self.pen.fg = Some(c);
                    }
                }
                39 => self.pen.fg = None,
                40..=47 => self.pen.bg = Some(ansi_basic_colour(n - 40)),
                48 => {
                    if let Some(c) = parse_extended_colour(param, &mut iter) {
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

/// Parse the `38 ⟨…⟩` / `48 ⟨…⟩` extended-colour tail and return
/// the resolved `Color`, or `None` if the form wasn't recognised.
///
/// Two wire forms appear in the wild for the same colour:
///
///   - Semicolon form: `\e[38;2;R;G;Bm`. Each value is its own
///     `Param`; we walk the outer `iter` to collect `kind` (`2` or
///     `5`) and the channel values, consuming three / four `next()`
///     calls. Without this branch, opencode's heavy use of
///     truecolour fg/bg via `38;…` / `48;…` would never colourise.
///
///   - Colon sub-param form: `\e[38:2:R:G:Bm`. Every value lands
///     inside ONE `Param` (the same one that carries the leading
///     `38`), so we read from that param's tail and DO NOT touch
///     `iter`. Touching `iter` here is what caused the
///     "underline stuck on" rendering opencode triggers: it emits
///     `\e[38:2:R:G:B;24m` to set an RGB fg and then turn off
///     underline, and the old impl swallowed the `24` looking for
///     a non-existent colour-spec word.
fn parse_extended_colour(param: &[u16], iter: &mut vte::ParamsIter<'_>) -> Option<Color> {
    // Colon form: the sub-params are sub-values of `param`.
    // param[0] is the leading 38/48; param[1] is `kind`; the rest
    // is channel data. Detect this when the leading word came with
    // sub-values attached.
    if param.len() > 1 {
        let kind = *param.get(1)?;
        return match kind {
            5 => {
                let idx = *param.get(2)? as u8;
                Some(ansi_256(idx))
            }
            2 => {
                // The CSI spec also tolerates a colour-space-id
                // sentinel between `2` and the RGB triple: the long
                // form is `38:2::R:G:B`. When the second sub-value
                // is absent / zero / 1, prefer the trailing three
                // positions; when only three values follow `2`,
                // they ARE the R/G/B. Pick whichever shape matches
                // the param length so both wire forms map to the
                // same colour.
                let len = param.len();
                let (r, g, b) = if len >= 6 {
                    (
                        *param.get(3)? as u8,
                        *param.get(4)? as u8,
                        *param.get(5)? as u8,
                    )
                } else {
                    (
                        *param.get(2)? as u8,
                        *param.get(3)? as u8,
                        *param.get(4)? as u8,
                    )
                };
                Some(Color::Rgb { r, g, b })
            }
            _ => None,
        };
    }
    // Semicolon form: walk the outer iter for kind + channels.
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
                self.pending_wrap = false;
            }
            b'\t' => {
                // Tab — advance to the next 8-column stop.
                let next = (self.cur_col / 8 + 1) * 8;
                let cap = self.grid.cols.saturating_sub(1);
                self.cur_col = next.min(cap);
                self.pending_wrap = false;
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
                self.pending_wrap = false;
            }
            'B' => {
                let n = (first as usize).max(1);
                self.cur_row = (self.cur_row + n).min(self.grid.rows.saturating_sub(1));
                self.pending_wrap = false;
            }
            'C' => {
                let n = (first as usize).max(1);
                self.cur_col = (self.cur_col + n).min(self.grid.cols.saturating_sub(1));
                self.pending_wrap = false;
            }
            'D' => {
                let n = (first as usize).max(1);
                self.cur_col = self.cur_col.saturating_sub(n);
                self.pending_wrap = false;
            }
            'E' => {
                // CNL — Cursor Next Line: down N rows, snap to col 0.
                let n = (first as usize).max(1);
                self.cur_row = (self.cur_row + n).min(self.grid.rows.saturating_sub(1));
                self.cur_col = 0;
                self.pending_wrap = false;
            }
            'F' => {
                // CPL — Cursor Previous Line: up N rows, snap to col 0.
                // The .NET MSBuild terminal logger redraws its progress
                // block each frame by emitting `\e[nF` to rewind to the
                // top of the block, then erasing below and rewriting.
                // Without this arm the cursor never moves up, so every
                // timer tick (`(0.1s)`, `(0.2s)`, …) lands on a fresh
                // line instead of overwriting the previous one.
                let n = (first as usize).max(1);
                self.cur_row = self.cur_row.saturating_sub(n);
                self.cur_col = 0;
                self.pending_wrap = false;
            }
            'G' => {
                // CHA — Cursor Horizontal Absolute (1-based).
                let col = (first as usize).saturating_sub(1);
                self.cur_col = col.min(self.grid.cols.saturating_sub(1));
                self.pending_wrap = false;
            }
            'd' => {
                // VPA — Vertical Position Absolute (1-based row),
                // column unchanged. The counterpart to CHA on the
                // vertical axis; progress renderers that jump to an
                // absolute row use this.
                let row = (first as usize).saturating_sub(1);
                self.cur_row = row.min(self.grid.rows.saturating_sub(1));
                self.pending_wrap = false;
            }
            '@' => {
                // ICH — Insert N blank cells at the cursor, shifting
                // the rest of the row right (cells pushed past the
                // right margin fall off). zsh's zle uses this to make
                // room when you type a character in the middle of a
                // line instead of redrawing the whole tail.
                let n = (first as usize).max(1);
                self.insert_chars(n);
                self.pending_wrap = false;
            }
            'P' => {
                // DCH — Delete N cells at the cursor, shifting the
                // rest of the row left and blank-filling the right.
                // This is what zle emits when you backspace / delete a
                // character in the MIDDLE of a line: it removes the
                // char at the cursor and pulls the tail in, rather
                // than rewriting every cell after it. Without this arm
                // the deletion never lands in the grid, so a mid-line
                // backspace looked like it erased from the end of the
                // line instead of at the cursor.
                let n = (first as usize).max(1);
                self.delete_chars(n);
                self.pending_wrap = false;
            }
            'X' => {
                // ECH — Erase N cells from the cursor (overwrite with
                // blanks, no shift). Some line editors blank the cell
                // a char used to occupy this way instead of via DCH.
                let n = (first as usize).max(1);
                self.erase_chars(n);
                self.pending_wrap = false;
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
                        25 => self.cursor_visible = enable,
                        2004 => self.bracketed_paste = enable,
                        1000 => self.mouse_button_mode = enable,
                        1002 => self.mouse_drag_mode = enable,
                        1003 => self.mouse_motion_mode = enable,
                        1006 => self.mouse_sgr = enable,
                        // 47 / 1047 / 1049 — alt-screen variants.
                        // 1049 is the modern form (save cursor + swap
                        // + clear on enter, swap + restore cursor on
                        // exit); 47 / 1047 are the older shapes most
                        // editors and pagers still emit. They all map
                        // to the same enter_/exit_alt_screen pair —
                        // the difference between the variants
                        // historically was "do you also save the
                        // cursor?", which our snapshot does
                        // unconditionally, and "do you clear the alt
                        // screen on entry?", which we also do
                        // unconditionally so vim / htop / claude /
                        // opencode all start on a clean canvas.
                        47 | 1047 | 1049 => {
                            if enable {
                                self.enter_alt_screen();
                            } else {
                                self.exit_alt_screen();
                            }
                        }
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
            b'D' => self.line_feed(), // IND
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
                self.pending_wrap = false;
                self.bracketed_paste = false;
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
    /// DCH — delete `n` cells at the cursor, sliding the remainder of
    /// the row left and padding the freed right end with blanks. The
    /// deleted cells inherit no colour; new right-edge cells are plain
    /// blanks (xterm fills with the default background, not the pen).
    fn delete_chars(&mut self, n: usize) {
        let row = self.cur_row;
        if row >= self.grid.rows {
            return;
        }
        let cols = self.grid.cols;
        let from = self.cur_col.min(cols);
        let n = n.min(cols - from);
        if n == 0 {
            return;
        }
        let line = &mut self.grid.cells[row];
        line.drain(from..from + n);
        for _ in 0..n {
            line.push(Cell::blank());
        }
    }

    /// ICH — insert `n` blank cells at the cursor, sliding the
    /// remainder of the row right. Cells pushed past the right margin
    /// are discarded so the row stays exactly `cols` wide.
    fn insert_chars(&mut self, n: usize) {
        let row = self.cur_row;
        if row >= self.grid.rows {
            return;
        }
        let cols = self.grid.cols;
        let at = self.cur_col.min(cols);
        let n = n.min(cols - at);
        if n == 0 {
            return;
        }
        let line = &mut self.grid.cells[row];
        for _ in 0..n {
            line.insert(at, Cell::blank());
        }
        line.truncate(cols);
    }

    /// ECH — blank `n` cells starting at the cursor without shifting
    /// the rest of the row.
    fn erase_chars(&mut self, n: usize) {
        let row = self.cur_row;
        if row >= self.grid.rows {
            return;
        }
        let cols = self.grid.cols;
        let from = self.cur_col.min(cols);
        let to = (from + n).min(cols);
        for col in from..to {
            self.grid.cells[row][col] = Cell::blank();
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
    /// Handle to the spawned PTY child process. Kept so `Drop` can
    /// `wait()` on it and reap the zombie — without this the child
    /// process slot lingers in the kernel until binvim itself exits,
    /// which during a long session leaves the user's `jobs -p` /
    /// shell prompt counting every closed terminal as a "hanging
    /// shell". `Option` because Drop takes the handle out to move it
    /// into the reaper thread.
    child: Mutex<Option<Box<dyn Child + Send + Sync>>>,
    /// Optional display label shown in the tab strip — set when the
    /// terminal was spawned for a specific purpose (e.g. a task run
    /// labelled "build" / "dev"). When `None` the renderer falls back
    /// to the positional tab number. Held as a `Mutex` so non-`mut`
    /// call sites (the renderer holds `&App`) can read it without
    /// fighting borrow rules; writes are one-shot at spawn time.
    label: Mutex<Option<String>>,
}

pub struct TerminalInner {
    pub(crate) parser: Parser,
    pub(crate) handler: VteHandler,
    /// Set once `is_alive()` discovers the child has died, so we
    /// don't keep polling its exit status.
    pub(crate) exited: bool,
}

impl Terminal {
    /// Spawn `shell` (default `$SHELL`, falling back to the platform
    /// default) in a `rows × cols` PTY. The reader thread starts
    /// immediately; callers should drive `drain()` on each frame so
    /// the grid reflects the latest output.
    pub fn spawn(rows: u16, cols: u16, shell: Option<&str>) -> Result<Self> {
        let shell_cmd = match shell {
            Some(s) => s.to_string(),
            None => default_shell(),
        };
        Self::spawn_program(rows, cols, &shell_cmd, &[])
    }

    /// Spawn `program` with `args` in a `rows × cols` PTY. Same env
    /// + cwd treatment as `spawn`. Used by the side-pane AI launcher
    /// to start the user's shell with `-l -i -c "exec <tool>"` so
    /// the shell sources its rc files (login + interactive), picks
    /// up nvm / asdf / direnv shims, and then `exec`s into the tool
    /// — replacing the shell process so the tool owns the PTY.
    pub fn spawn_program(rows: u16, cols: u16, program: &str, args: &[&str]) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty failed")?;

        let mut cmd = CommandBuilder::new(program);
        for a in args {
            cmd.arg(a);
        }
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        // Pass through binvim's env so the spawned shell sees the
        // same `HOME`, `PATH`, `NVM_DIR`, `XDG_*`, etc. that we
        // were launched with. `portable_pty`'s `CommandBuilder`
        // ships with an empty env by default — without this loop
        // the child gets only the vars we set explicitly below,
        // and any rc-file logic that branches on `NVM_DIR` /
        // `HOMEBREW_PREFIX` / etc. silently falls through (which
        // is why `:claude` worked when launched via `zsh -i -c
        // "exec claude"` but `:codex` couldn't find its node
        // shebang interpreter — nvm's `.zshrc` block read an
        // unset `NVM_DIR`).
        for (k, v) in std::env::vars() {
            cmd.env(k, v);
        }
        // TERM=xterm-256color gives shells the modern colour
        // expectations without lying about features we don't have
        // (alternate screen, mouse modes will quietly noop on our
        // side — they're handled by our outer terminal anyway).
        cmd.env("TERM", "xterm-256color");
        // Suppress zsh's PROMPT_EOL_MARK (`%`) — zsh prints an
        // inverse-video `%` + newline at startup if it can't
        // verify the previous output ended with a newline, which
        // looks like a stray glyph in our empty pane. Other shells
        // ignore this env entirely.
        cmd.env("PROMPT_EOL_MARK", "");

        let child = pair
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
        let writer = pair.master.take_writer().context("take_writer failed")?;

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
            child: Mutex::new(Some(child)),
            label: Mutex::new(None),
        })
    }

    /// Attach (or replace) the display label shown in the tab strip.
    /// Callers set this right after spawn for terminals that run a
    /// specific named purpose — the task runner does so with the task
    /// name. `None` (or an unset label) leaves the renderer on its
    /// positional-number fallback.
    pub fn set_label(&self, label: Option<String>) {
        if let Ok(mut slot) = self.label.lock() {
            *slot = label;
        }
    }

    /// Snapshot of the current label, if any. Cheap clone — the
    /// renderer reads this once per frame.
    pub fn label(&self) -> Option<String> {
        self.label.lock().ok().and_then(|l| l.clone())
    }

    /// Returns `Some(status_code)` the *first* time the child process
    /// is observed to have exited (status defaults to 0 if the
    /// platform doesn't expose one). Subsequent calls return `None`
    /// even though the child is dead — `inner.exited` is the
    /// idempotency latch. Called once per frame by the task-runner
    /// quickfix scrape; cheap (`try_wait` is non-blocking).
    pub fn poll_exit(&self) -> Option<i32> {
        // Already reported on a prior frame.
        if let Ok(inner) = self.inner.lock() {
            if inner.exited {
                return None;
            }
        }
        let mut child_guard = match self.child.lock() {
            Ok(g) => g,
            Err(_) => return None,
        };
        let status = match child_guard.as_mut() {
            Some(c) => match c.try_wait() {
                // exit_code() returns u32; downcast to i32 for the
                // standard "negative if abnormal" convention. None
                // means the platform couldn't report — call it 0.
                Ok(Some(s)) => s.exit_code() as i32,
                Ok(None) | Err(_) => return None,
            },
            // Drop has already taken the child. Treat as silently
            // exited so the caller's "already reported" gate trips on
            // the next call.
            None => 0,
        };
        if let Ok(mut inner) = self.inner.lock() {
            inner.exited = true;
        }
        Some(status)
    }

    /// True once `poll_exit` has observed the child exit. Lets the
    /// renderer tag dead tabs without polling the OS itself.
    pub fn has_exited(&self) -> bool {
        self.inner.lock().map(|i| i.exited).unwrap_or(false)
    }
}

impl Drop for Terminal {
    /// Reap the PTY's child process so it doesn't linger as a zombie
    /// after the user closes the terminal. The actual SIGHUP arrives
    /// when the `master` Arc drops (its FD close hangs up the PTY);
    /// here we move the child handle into a detached thread that
    /// blocks on `wait()`, which the kernel resolves the instant the
    /// shell exits. Without this the child sits as `<defunct>` in
    /// `ps` until binvim itself dies — visible to the user as a
    /// growing count in their shell prompt's job indicator.
    fn drop(&mut self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

impl Terminal {
    /// Forward `bytes` to the PTY master — i.e. into the child's
    /// stdin. Used to wire user keystrokes through to the shell.
    pub fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes)?;
        w.flush()?;
        Ok(())
    }

    /// Forward pasted `text` to the PTY. When the embedded program has
    /// enabled bracketed-paste mode (DECSET 2004) the blob is wrapped in
    /// `\x1b[200~ … \x1b[201~` so the program sees one atomic paste —
    /// interior newlines stay literal instead of each firing an Enter.
    /// When the program hasn't asked for it (a bare shell, a pager) the
    /// markers would arrive as literal `[200~` junk, so we send the raw
    /// text and let the child interpret newlines itself.
    pub fn write_paste(&self, text: &str) -> Result<()> {
        let bracketed = self
            .inner
            .lock()
            .map(|i| i.handler.bracketed_paste)
            .unwrap_or(false);
        let mut w = self.writer.lock().unwrap();
        if bracketed {
            w.write_all(b"\x1b[200~")?;
            w.write_all(text.as_bytes())?;
            w.write_all(b"\x1b[201~")?;
        } else {
            w.write_all(text.as_bytes())?;
        }
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

    /// Whether the embedded program currently wants the hardware
    /// cursor shown (DECTCEM, `\x1b[?25h` / `\x1b[?25l`). TUIs that
    /// draw their own caret hide it; the renderer honours this so we
    /// don't paint a stray system cursor at the program's parked
    /// position while its real caret lives elsewhere.
    pub fn cursor_visible(&self) -> bool {
        self.inner
            .lock()
            .map(|i| i.handler.cursor_visible)
            .unwrap_or(true)
    }

    /// True when the embedded program has switched into alt-screen
    /// mode (vim, htop, claude, opencode, codex). Side-pane loading
    /// uses this as the "we're a real TUI now" signal.
    pub fn alt_screen_active(&self) -> bool {
        self.inner.lock().unwrap().handler.alt_screen_active()
    }

    /// Current scrollback view offset in lines (`0` = following live
    /// output). Cheap — used by the status row in the pane header
    /// and the mouse / key handlers when computing relative scrolls.
    pub fn view_scroll(&self) -> usize {
        self.inner
            .lock()
            .map(|i| i.handler.grid.view_scroll)
            .unwrap_or(0)
    }

    /// True when the user has scrolled back into history. Renderer
    /// uses this to surface a `↑Nb` marker in the pane header so the
    /// user notices their view isn't live.
    pub fn is_scrolled_back(&self) -> bool {
        self.view_scroll() > 0
    }

    /// Adjust the scrollback view by `delta` lines (positive = scroll
    /// up into history, negative = scroll down toward live). Clamped
    /// to the available scrollback. Returns the new offset.
    pub fn scroll_view_by(&self, delta: isize) -> usize {
        match self.inner.lock() {
            Ok(mut i) => i.handler.grid.scroll_view_by(delta),
            Err(_) => 0,
        }
    }

    /// Snap the view back to live output (offset 0). Called from the
    /// key handler when the user types — they expect to see the prompt
    /// they're typing into, not a stale scrollback position.
    pub fn snap_view_to_live(&self) {
        if let Ok(mut i) = self.inner.lock() {
            i.handler.grid.view_scroll = 0;
        }
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

/// The shell to spawn when no override is provided. Honours `$SHELL`
/// first; falls back to the platform's canonical login shell —
/// `/bin/sh` on Unix, `$COMSPEC` (or `cmd.exe`) on Windows. Callers
/// that need POSIX-shell semantics (`-l -i -c`) should be aware that
/// Windows' `cmd.exe` uses a different flag dialect (`/C`); that
/// translation is out of scope for v1 of the Windows port.
pub fn default_shell() -> String {
    if let Ok(s) = std::env::var("SHELL") {
        if !s.is_empty() {
            return s;
        }
    }
    if cfg!(windows) {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    } else {
        "/bin/sh".into()
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
    fn cpl_moves_up_to_line_start() {
        // CPL (`\e[nF`) — up N rows AND to column 0. This is what the
        // .NET MSBuild terminal logger uses to rewind to the top of
        // its progress block before redrawing; without it the per-tick
        // timer accumulated on fresh lines instead of overwriting.
        // Start at (2,3), CPL 2 → (0,0), then write 'X'.
        let h = parse_bytes(b"\x1b[3;4H\x1b[2FX", 4, 10);
        assert_eq!(h.grid.cells[0][0].ch, 'X');
        assert_eq!((h.cur_row, h.cur_col), (0, 1));
    }

    #[test]
    fn cnl_moves_down_to_line_start() {
        // CNL (`\e[nE`) — down N rows AND to column 0.
        let h = parse_bytes(b"\x1b[1;5H\x1b[2EX", 4, 10);
        assert_eq!(h.grid.cells[2][0].ch, 'X');
        assert_eq!((h.cur_row, h.cur_col), (2, 1));
    }

    #[test]
    fn vpa_sets_absolute_row_keeps_column() {
        // VPA (`\e[nd`) — jump to 1-based row N, column unchanged.
        // From (0,3) a VPA 3 lands on row 2, still at col 3.
        let h = parse_bytes(b"\x1b[1;4H\x1b[3dX", 4, 10);
        assert_eq!(h.grid.cells[2][3].ch, 'X');
        assert_eq!((h.cur_row, h.cur_col), (2, 4));
    }

    #[test]
    fn dch_deletes_at_cursor_and_shifts_tail_left() {
        // Regression for a mid-line backspace looking like it erased
        // from the end of the line. zle deletes the char at the cursor
        // with DCH (`\e[P`), pulling the rest of the row in. Type
        // "hello", move the cursor back onto the second 'l' (col 2),
        // then DCH 1 — the row must become "helo" with a blank tail.
        let h = parse_bytes(b"hello\x1b[1;3H\x1b[P", 2, 10);
        assert_eq!(line_text(&h.grid, 0), "helo");
        assert_eq!(h.grid.cells[0][9].ch, ' ');
    }

    #[test]
    fn ich_inserts_blanks_and_shifts_tail_right() {
        // ICH (`\e[@`) — zle makes room for a typed char mid-line.
        // From "helo", cursor at col 3 (the 'o'), insert 1 blank then
        // overwrite it: "hel o" before the new glyph lands.
        let h = parse_bytes(b"helo\x1b[1;4H\x1b[@", 2, 10);
        assert_eq!(h.grid.cells[0][3].ch, ' ');
        assert_eq!(h.grid.cells[0][4].ch, 'o');
    }

    #[test]
    fn ech_blanks_in_place_without_shifting() {
        // ECH (`\e[X`) erases cells under the cursor but leaves the
        // tail where it is — no left shift, unlike DCH.
        let h = parse_bytes(b"hello\x1b[1;2H\x1b[2X", 2, 10);
        assert_eq!(h.grid.cells[0][0].ch, 'h');
        assert_eq!(h.grid.cells[0][1].ch, ' ');
        assert_eq!(h.grid.cells[0][2].ch, ' ');
        assert_eq!(h.grid.cells[0][3].ch, 'l');
    }

    #[test]
    fn dotnet_progress_timer_overwrites_in_place() {
        // Regression for the `dotnet build` timer rendering on a new
        // line per tick. The logger draws one progress row, then each
        // frame rewinds with CPL (`\e[1F`), erases below (`\e[0J`), and
        // rewrites. All ticks must collapse onto the same row.
        let h = parse_bytes(
            b"proj (0.0s)\x1b[1F\x1b[0Jproj (0.1s)\x1b[1F\x1b[0Jproj (0.2s)",
            3,
            20,
        );
        assert_eq!(line_text(&h.grid, 0), "proj (0.2s)");
        assert_eq!(line_text(&h.grid, 1), "");
        assert_eq!(h.cur_row, 0);
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
    fn visible_row_follows_scrollback_when_view_scrolled_back() {
        // Print enough lines to push three into scrollback (lines
        // "a","b","c"), leaving the live grid showing "d","e","f","g".
        let mut h = parse_bytes(b"a\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng", 4, 5);
        assert_eq!(h.grid.scrollback.len(), 3);
        // view_scroll = 0 → window is the live grid as before.
        assert_eq!(visible_text(&h.grid, 0), "d");
        assert_eq!(visible_text(&h.grid, 3), "g");
        // Pull the view up by one line — top row should now be the
        // last scrollback line ("c") and the live grid slides down
        // by one (we lose visibility of "g" off the bottom).
        h.grid.scroll_view_by(1);
        assert_eq!(visible_text(&h.grid, 0), "c");
        assert_eq!(visible_text(&h.grid, 1), "d");
        assert_eq!(visible_text(&h.grid, 3), "f");
        // Pull all the way back — top of window is the oldest
        // scrollback line ("a"). Asking for a row past the combined
        // length returns None (defensive bound).
        h.grid.scroll_view_by(2);
        assert_eq!(h.grid.view_scroll, 3);
        assert_eq!(visible_text(&h.grid, 0), "a");
        assert_eq!(visible_text(&h.grid, 3), "d");
    }

    #[test]
    fn scroll_view_clamps_to_scrollback_bounds() {
        // 3 lines pushed into scrollback. The view can scroll up by
        // at most that many lines (any further is a no-op).
        let mut h = parse_bytes(b"a\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng", 4, 5);
        assert_eq!(h.grid.scrollback.len(), 3);
        // Try to scroll way past the end — clamps to 3.
        assert_eq!(h.grid.scroll_view_by(100), 3);
        // Try to scroll down past 0 — clamps to 0.
        assert_eq!(h.grid.scroll_view_by(-100), 0);
    }

    #[test]
    fn scroll_up_anchors_user_to_content_when_view_scrolled() {
        // Set up: 3 lines in scrollback, view scrolled all the way
        // back so the user sees the oldest line at the top.
        let mut h = parse_bytes(b"a\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng", 4, 5);
        h.grid.scroll_view_by(3);
        assert_eq!(visible_text(&h.grid, 0), "a");
        // Streaming a new line scrolls the live grid up — without
        // the auto-bump, the user's view would slide forward through
        // their own history. With the bump, they stay anchored to
        // "a" at the top.
        parse_into(&mut h, b"\r\nh");
        assert_eq!(h.grid.scrollback.len(), 4);
        assert_eq!(h.grid.view_scroll, 4);
        assert_eq!(visible_text(&h.grid, 0), "a");
    }

    fn visible_text(grid: &Grid, row: usize) -> String {
        grid.visible_row(row)
            .map(|r| r.iter().map(|c| c.ch).collect::<String>().trim_end().into())
            .unwrap_or_default()
    }

    fn parse_into(h: &mut VteHandler, bytes: &[u8]) {
        let mut parser = Parser::new();
        parser.advance(h, bytes);
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
    fn writing_to_last_column_defers_wrap_until_next_glyph() {
        // xterm DECAWM behaviour: writing the 5th glyph on a 5-col
        // terminal leaves the cursor at col 4 with a wrap pending,
        // NOT at (row 1, col 0). The wrap fires only on the next
        // glyph. Without this, zsh's PROMPT_SP trick (fill row + CR
        // to land at col 0 of the same row) takes us one row too far.
        let h = parse_bytes(b"hello", 3, 5);
        assert_eq!(h.cur_row, 0);
        assert_eq!(h.cur_col, 4);
        // Next glyph triggers the wrap.
        let h = parse_bytes(b"hello!", 3, 5);
        assert_eq!(h.cur_row, 1);
        assert_eq!(h.cur_col, 1);
    }

    #[test]
    fn cr_after_row_fill_stays_on_same_row() {
        // The actual zsh PROMPT_SP shape: fill the row with spaces,
        // then CR. The CR should clear the pending wrap and land us
        // at (row 0, col 0), NOT (row 1, col 0). This is what
        // previously cost us a blank row between consecutive prompts.
        let h = parse_bytes(b"     \r", 3, 5);
        assert_eq!(h.cur_row, 0);
        assert_eq!(h.cur_col, 0);
    }

    #[test]
    fn alt_screen_enter_clears_and_exit_restores_main() {
        // Write "hello" on the main screen, enter alt-screen, write
        // "alt", exit, and the main screen content should be back
        // exactly as it was on entry. This is the contract vim,
        // htop, claude, etc. depend on — without it, every TUI
        // close would smear its last frame over the user's shell.
        let h = parse_bytes(b"hello\x1b[?1049h\x1b[2J\x1b[Halt\x1b[?1049l", 3, 10);
        assert_eq!(line_text(&h.grid, 0), "hello");
        assert!(!h.alt_screen_active());
    }

    #[test]
    fn alt_screen_active_flag_tracks_mode() {
        let h = parse_bytes(b"\x1b[?1049h", 3, 10);
        assert!(h.alt_screen_active());
        let h = parse_bytes(b"\x1b[?1049h\x1b[?1049l", 3, 10);
        assert!(!h.alt_screen_active());
    }

    #[test]
    fn bracketed_paste_flag_tracks_decset_2004() {
        // Off by default; ?2004h enables, ?2004l disables. This gates
        // whether write_paste wraps a paste in \x1b[200~ … \x1b[201~.
        assert!(!parse_bytes(b"", 3, 10).bracketed_paste);
        assert!(parse_bytes(b"\x1b[?2004h", 3, 10).bracketed_paste);
        assert!(!parse_bytes(b"\x1b[?2004h\x1b[?2004l", 3, 10).bracketed_paste);
        // RIS (full reset) clears it.
        assert!(!parse_bytes(b"\x1b[?2004h\x1bc", 3, 10).bracketed_paste);
    }

    #[test]
    fn alt_screen_legacy_47_and_1047_also_swap() {
        // Older shapes — vim and friends still emit these on terms
        // without 1049 in terminfo. They must hit the same code path.
        let h = parse_bytes(b"main\x1b[?47halt", 2, 10);
        assert!(h.alt_screen_active());
        let h = parse_bytes(b"main\x1b[?47halt\x1b[?47l", 2, 10);
        assert!(!h.alt_screen_active());
        assert_eq!(line_text(&h.grid, 0), "main");
    }

    #[test]
    fn sgr_extended_underline_style_zero_disables_underline() {
        // \x1b[4:3m turns on a curly underline; \x1b[4:0m turns it
        // off — Claude Code's hyperlink wrap convention. Reading
        // the bare primary `4` as "on" used to leave underline stuck
        // across every cell that followed the link, smearing a line
        // under the rest of the welcome screen.
        let h = parse_bytes(b"\x1b[4:3mAB\x1b[4:0mC", 1, 4);
        assert!(h.grid.cells[0][0].underline);
        assert!(h.grid.cells[0][1].underline);
        assert!(!h.grid.cells[0][2].underline);
    }

    #[test]
    fn sgr_bare_4_enables_underline() {
        // Sanity: the unadorned `\x1b[4m` (no sub-param) still
        // enables single underline.
        let h = parse_bytes(b"\x1b[4mAB\x1b[24mC", 1, 4);
        assert!(h.grid.cells[0][0].underline);
        assert!(!h.grid.cells[0][2].underline);
    }

    #[test]
    fn sgr_truecolour_24bit() {
        // \x1b[38;2;255;128;0m — orange foreground.
        let h = parse_bytes(b"\x1b[38;2;255;128;0mX", 1, 3);
        assert_eq!(
            h.grid.cells[0][0].fg,
            Some(Color::Rgb {
                r: 255,
                g: 128,
                b: 0
            })
        );
    }

    #[test]
    fn sgr_truecolour_24bit_colon_form() {
        // \x1b[38:2:255:128:0m — same colour, colon-separated
        // sub-params. opencode emits this shape; we must read the
        // R/G/B out of the same param rather than walking the outer
        // iter (which would eat unrelated trailing params).
        let h = parse_bytes(b"\x1b[38:2:255:128:0mX", 1, 3);
        assert_eq!(
            h.grid.cells[0][0].fg,
            Some(Color::Rgb {
                r: 255,
                g: 128,
                b: 0
            })
        );
    }

    #[test]
    fn sgr_colon_truecolour_then_underline_off_clears_underline() {
        // Regression for the opencode rendering bug: setting RGB fg
        // via the colon form and then turning underline off in the
        // SAME CSI used to leave underline stuck because parsing
        // `38:2:R:G:B` walked the outer iter and ate the trailing
        // `24` looking for a colour kind. After the fix, the `24`
        // arrives at the SGR loop and underline gets cleared.
        let h = parse_bytes(b"\x1b[4m\x1b[38:2:100:150:200;24mX", 1, 3);
        assert_eq!(
            h.grid.cells[0][0].fg,
            Some(Color::Rgb {
                r: 100,
                g: 150,
                b: 200
            })
        );
        assert!(!h.grid.cells[0][0].underline);
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
    fn wide_glyph_advances_two_columns() {
        // ⚡ (U+26A1) is East Asian Wide / emoji presentation by
        // default in most fonts. The host terminal paints it across
        // two cells; our grid models the same so the cursor lands
        // at the actual next-write position.
        let h = parse_bytes("a⚡b".as_bytes(), 2, 10);
        assert_eq!(h.grid.cells[0][0].ch, 'a');
        assert_eq!(h.grid.cells[0][1].ch, '⚡');
        assert_eq!(
            h.grid.cells[0][2].ch, '\0',
            "wide-char continuation cell should be marked"
        );
        assert_eq!(h.grid.cells[0][3].ch, 'b');
        assert_eq!((h.cur_row, h.cur_col), (0, 4));
    }

    fn wide_glyph_at_end_defers_wrap_until_next_glyph() {
        // Cols = 3, so `ab⚡` lands the wide ⚡ at col 2 with no
        // continuation cell (col 3 is out of bounds). Under xterm
        // DECAWM the cursor stays at col 2 with a wrap pending; the
        // wrap only fires on the next glyph write. This is what
        // makes zsh's right-prompt + PROMPT_SP idioms work without
        // eating an extra row.
        let h = parse_bytes("ab⚡".as_bytes(), 2, 3);
        assert_eq!(h.grid.cells[0][0].ch, 'a');
        assert_eq!(h.grid.cells[0][1].ch, 'b');
        assert_eq!(h.grid.cells[0][2].ch, '⚡');
        assert_eq!((h.cur_row, h.cur_col), (0, 2));
        // One more glyph triggers the deferred wrap.
        let h = parse_bytes("ab⚡x".as_bytes(), 2, 3);
        assert_eq!((h.cur_row, h.cur_col), (1, 1));
    }

    #[test]
    fn wide_glyph_at_end_wraps_one_cell_early() {
        // Retained name for tooling pinned to it. Kept as a thin
        // wrapper around the deferred-wrap variant above.
        let h = parse_bytes("ab⚡".as_bytes(), 2, 3);
        assert_eq!(h.grid.cells[0][2].ch, '⚡');
    }

    #[test]
    fn decset_mouse_modes_toggle() {
        // `CSI ? 1006 h` then `CSI ? 1000 h` enable two mouse
        // modes. `CSI ? 1000 l` disables one. Bare `CSI ? 25 h`
        // (cursor visibility) is unrelated and shouldn't perturb
        // mouse state.
        let h = parse_bytes(b"\x1b[?1006h\x1b[?1000h\x1b[?25h\x1b[?1000l", 4, 10);
        assert!(h.mouse_sgr, "SGR mode should still be on");
        assert!(!h.mouse_button_mode, "1000 should have been disabled");
        assert!(!h.mouse_drag_mode);
        assert!(!h.mouse_motion_mode);
    }

    #[test]
    fn dectcem_toggles_cursor_visibility() {
        // Default visible; `?25l` hides, `?25h` re-shows. TUIs that
        // draw their own caret rely on this so binvim doesn't paint a
        // stray system cursor over them.
        let h = parse_bytes(b"x", 4, 10);
        assert!(h.cursor_visible, "fresh terminal should show the cursor");
        let h = parse_bytes(b"\x1b[?25l", 4, 10);
        assert!(!h.cursor_visible, "?25l should hide the cursor");
        let h = parse_bytes(b"\x1b[?25l\x1b[?25h", 4, 10);
        assert!(h.cursor_visible, "?25h should re-show the cursor");
    }

    #[test]
    fn alt_screen_exit_restores_cursor_visibility() {
        // A TUI that hid the cursor and left alt-screen without
        // restoring it shouldn't leave the shell prompt caret-less.
        let h = parse_bytes(b"\x1b[?1049h\x1b[?25l\x1b[?1049l", 4, 10);
        assert!(h.cursor_visible);
    }

    #[test]
    fn decset_alt_screen_isolates_writes_from_main_buffer() {
        // 1049h → write "hello" into the alt buffer → 1049l. The
        // main buffer was empty at the swap-in point, so after the
        // round-trip it should still be empty — "hello" lived only
        // in alt-screen and got discarded on swap-back. Previously
        // this test pinned the "we ignore 1049 entirely" behaviour
        // and asserted "hello" ended up on the main grid; the new
        // alt-screen plumbing means that would be wrong (vim's
        // last frame would smear onto the user's shell).
        let h = parse_bytes(b"\x1b[?1049hhello\x1b[?1049l", 4, 10);
        assert_eq!(line_text(&h.grid, 0), "");
        assert!(!h.alt_screen_active());
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
