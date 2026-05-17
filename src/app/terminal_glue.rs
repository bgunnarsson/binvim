//! `:terminal` overlay glue. Wires the standalone `crate::terminal`
//! model into the App: spawns the PTY on `:terminal`, forwards
//! keystrokes from `Mode::Terminal` to the PTY's stdin, drains
//! output on every frame, and tracks the show/hide flag the
//! renderer keys off.

use crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::mode::Mode;
use crate::terminal::Terminal;

impl super::App {
    /// `:terminal [cmd]` — open the embedded terminal overlay. The
    /// argument, if any, is the command to run instead of `$SHELL`.
    /// Re-opening while a terminal is already alive just re-focuses
    /// the existing pane (cheaper than killing + respawning).
    pub(super) fn cmd_open_terminal(&mut self, cmd: Option<String>) {
        if self.terminal.is_some() {
            self.terminal_pane_open = true;
            self.terminal_scroll = 0;
            self.mode = Mode::Terminal;
            self.adjust_viewport();
            return;
        }
        // Flip the open flag first so terminal_pane_rows() returns
        // the right value when we ask for dimensions. Toggle back
        // off if the spawn fails.
        self.terminal_pane_open = true;
        let rows = self.terminal_pane_rows().max(4) as u16;
        let cols = (self.width as usize).max(8) as u16;
        match Terminal::spawn(rows, cols, cmd.as_deref()) {
            Ok(t) => {
                self.terminal = Some(t);
                self.terminal_scroll = 0;
                self.terminal_visual_anchor = None;
                self.terminal_cursor = (0, 0);
                self.mode = Mode::Terminal;
                self.adjust_viewport();
                self.status_msg.clear();
            }
            Err(e) => {
                self.terminal_pane_open = false;
                self.status_msg = format!("terminal: spawn failed: {e}");
            }
        }
    }

    /// Close the embedded terminal — drops the `Terminal` so its
    /// PTY child + reader thread + writer fd all clean up. Called
    /// on `:q` while the overlay is active and on `<C-w>q` from
    /// `Mode::TerminalNormal`.
    pub(super) fn close_terminal(&mut self) {
        self.terminal = None;
        self.terminal_pane_open = false;
        self.terminal_scroll = 0;
        self.terminal_visual_anchor = None;
        self.terminal_cursor = (0, 0);
        if matches!(self.mode, Mode::Terminal | Mode::TerminalNormal) {
            self.mode = Mode::Normal;
        }
        self.adjust_viewport();
    }

    /// Drain pending PTY output into the grid. Called once per
    /// render loop. Returns `true` if any bytes were processed so
    /// the caller can mark the frame dirty.
    pub(super) fn terminal_drain_if_open(&self) -> bool {
        match self.terminal.as_ref() {
            Some(t) => t.drain() > 0,
            None => false,
        }
    }

    /// Mouse event dispatch for the terminal pane. Returns `true`
    /// when the event was consumed (no further editor handling).
    /// Click / scroll outside the pane bounds → not consumed.
    /// Inside the pane: if the embedded program has enabled DECSET
    /// mouse tracking, the event is formatted as the appropriate
    /// xterm escape and forwarded to the PTY. Otherwise a click
    /// pulls focus into the terminal (Mode::Terminal); scroll
    /// outside-mouse-mode is dropped on the floor.
    pub(super) fn handle_terminal_mouse_event(
        &mut self,
        ev: &MouseEvent,
        row: usize,
        col: usize,
    ) -> bool {
        let pane_rows = self.terminal_pane_rows();
        if pane_rows == 0 {
            return false;
        }
        let pane_top = self.terminal_pane_top();
        let pane_bottom = pane_top + pane_rows;
        if row < pane_top || row >= pane_bottom {
            return false;
        }
        // Coords relative to the pane (1-based — that's what xterm
        // mouse protocols use).
        let pane_row = row - pane_top + 1;
        let pane_col = col + 1;

        let term = match self.terminal.as_ref() {
            Some(t) => t,
            None => return true,
        };
        let mouse = term.mouse_state();
        if !mouse.any {
            // No program-driven mouse tracking. Treat clicks as
            // "focus the terminal" so the user can click the pane
            // to start typing into it. Scroll outside of tracked
            // mode is dropped (a future scrollback-scroll could
            // route Ctrl-Y / Ctrl-E equivalents here).
            if matches!(
                ev.kind,
                MouseEventKind::Down(MouseButton::Left | MouseButton::Middle)
            ) {
                if !matches!(self.mode, Mode::Terminal) {
                    self.terminal_visual_anchor = None;
                    self.mode = Mode::Terminal;
                }
            }
            return true;
        }

        // Translate `ev.kind` to the xterm button code. Drag flag
        // (+32) goes on drag events; the actual button stays
        // encoded in the low bits.
        let (button, is_release, is_drag) = match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => (0u32, false, false),
            MouseEventKind::Down(MouseButton::Middle) => (1, false, false),
            MouseEventKind::Down(MouseButton::Right) => (2, false, false),
            MouseEventKind::Up(MouseButton::Left) => (0, true, false),
            MouseEventKind::Up(MouseButton::Middle) => (1, true, false),
            MouseEventKind::Up(MouseButton::Right) => (2, true, false),
            MouseEventKind::Drag(MouseButton::Left) if mouse.drag => (0, false, true),
            MouseEventKind::Drag(MouseButton::Middle) if mouse.drag => (1, false, true),
            MouseEventKind::Drag(MouseButton::Right) if mouse.drag => (2, false, true),
            MouseEventKind::Moved if mouse.motion => (3, false, false),
            // xterm wheel encoding: button 64 = up, 65 = down,
            // never a release event (wheels are press-only).
            MouseEventKind::ScrollUp => (64, false, false),
            MouseEventKind::ScrollDown => (65, false, false),
            _ => return true, // dropped — but consumed (don't fall through)
        };
        let mut cb = button;
        if is_drag {
            cb |= 32;
        }
        let bytes = if mouse.sgr {
            // SGR encoding (DECSET 1006): `\x1b[<{btn};{x};{y}{M|m}`.
            // Press = M, release = m. Works for arbitrary screen
            // sizes; the legacy form below caps at col 95.
            let trail = if is_release { 'm' } else { 'M' };
            format!("\x1b[<{cb};{pane_col};{pane_row}{trail}").into_bytes()
        } else {
            // Legacy X10/1000 encoding. Coords + button-state are
            // bytes offset by 32. Caps at col 223 (because 32+223
            // = 255 fits one byte). Modern programs prefer SGR,
            // but a few old TUIs still ask for 1000 without 1006.
            // Release is reported with button 3 in this scheme.
            let cb_byte = if is_release { 3u32 } else { cb };
            let mut out = Vec::with_capacity(6);
            out.extend_from_slice(b"\x1b[M");
            out.push((cb_byte + 32) as u8);
            out.push((pane_col as u32 + 32).min(255) as u8);
            out.push((pane_row as u32 + 32).min(255) as u8);
            out
        };
        let _ = term.write_bytes(&bytes);
        // Clicking the pane while the editor was focused also
        // pulls focus in. Without this the user has to click *then*
        // press `i` to start interacting, which surprises everyone.
        if matches!(ev.kind, MouseEventKind::Down(_))
            && !matches!(self.mode, Mode::Terminal)
        {
            self.mode = Mode::Terminal;
        }
        true
    }

    /// `Mode::Terminal` key dispatch — every keystroke that isn't
    /// the mode-switch (`Esc`) gets translated into bytes and sent
    /// through the PTY's stdin. Most keys go through `Char(c)` and
    /// modifier flags; the trick is the named keys (arrows, Home,
    /// PgUp, …) which need ECMA-48 control sequences the shell
    /// recognises. We emit the standard xterm/vt100 forms; modern
    /// shells (bash, zsh, fish) read terminfo from `$TERM` and
    /// recognise these without further config.
    pub(super) fn handle_terminal_key(&mut self, key: KeyEvent) {
        // Esc exits Terminal mode but keeps the overlay alive so
        // the user can scroll / inspect / `:q`. To send a literal
        // Esc to the shell, the user can chord `Ctrl-[` (which is
        // the canonical control-code anyway).
        if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
            self.mode = Mode::TerminalNormal;
            return;
        }
        let bytes = match keyevent_to_bytes(key) {
            Some(b) => b,
            None => return,
        };
        if let Some(t) = self.terminal.as_ref() {
            let _ = t.write_bytes(&bytes);
        }
    }

    /// `Mode::TerminalNormal` key dispatch — the editor's bindings
    /// would be back, but a few Vim-style motions inside the
    /// terminal grid are handled here first so the user can move a
    /// reading-cursor with `h/j/k/l`, enter Visual with `v`, yank
    /// the selection with `y`, and `:q`-close the pane. Anything
    /// not matched here falls through to the normal key handler.
    pub(super) fn handle_terminal_normal_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let no_mods = key.modifiers.is_empty();
        // <C-w>q is the Vim convention for closing a terminal pane.
        // We model it directly here rather than through the
        // window-leader parser because the host buffer's window
        // bindings aren't meaningful while focus is on the pane.
        if ctrl && key.code == KeyCode::Char('q') {
            self.close_terminal();
            return true;
        }
        if !no_mods {
            return false;
        }
        let (rows, cols) = match self.terminal_grid_dims() {
            Some(d) => d,
            None => return false,
        };
        let max_row = rows.saturating_sub(1);
        let max_col = cols.saturating_sub(1);
        match key.code {
            KeyCode::Char('i') | KeyCode::Char('a') => {
                // Re-enter terminal-input mode. Any active selection
                // is dropped (Vim convention: selection lives only
                // while you're in Visual).
                self.terminal_visual_anchor = None;
                self.mode = Mode::Terminal;
                true
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.terminal_cursor.1 = self.terminal_cursor.1.saturating_sub(1);
                true
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.terminal_cursor.1 = (self.terminal_cursor.1 + 1).min(max_col);
                true
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.terminal_cursor.0 = self.terminal_cursor.0.saturating_sub(1);
                true
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.terminal_cursor.0 = (self.terminal_cursor.0 + 1).min(max_row);
                true
            }
            KeyCode::Char('0') | KeyCode::Home => {
                self.terminal_cursor.1 = 0;
                true
            }
            KeyCode::Char('$') | KeyCode::End => {
                self.terminal_cursor.1 = max_col;
                true
            }
            KeyCode::Char('g') => {
                // gg-style — just `g` for v1 (no operator-pending
                // prefix machinery in TerminalNormal yet).
                self.terminal_cursor = (0, 0);
                true
            }
            KeyCode::Char('G') => {
                self.terminal_cursor = (max_row, 0);
                true
            }
            KeyCode::Char('v') => {
                if self.terminal_visual_anchor.is_some() {
                    self.terminal_visual_anchor = None;
                } else {
                    self.terminal_visual_anchor = Some(self.terminal_cursor);
                }
                true
            }
            KeyCode::Char('y') => {
                self.terminal_yank();
                true
            }
            KeyCode::Char('Y') => {
                // Vim Y yanks the current line. Anchor the start at
                // col 0 and run to the end of the visible row.
                let row = self.terminal_cursor.0;
                let anchor = self.terminal_visual_anchor;
                self.terminal_visual_anchor = Some((row, 0));
                self.terminal_cursor = (row, max_col);
                self.terminal_yank();
                self.terminal_visual_anchor = anchor;
                true
            }
            KeyCode::Esc => {
                // Esc from Visual collapses the selection; from
                // plain TerminalNormal it leaves focus on the
                // editor (mode = Normal) but keeps the pane.
                if self.terminal_visual_anchor.is_some() {
                    self.terminal_visual_anchor = None;
                } else {
                    self.mode = Mode::Normal;
                }
                true
            }
            _ => false,
        }
    }

    /// Dimensions of the live terminal grid — `Some((rows, cols))`
    /// when a terminal is alive, `None` otherwise. Read under the
    /// inner mutex but released before this returns.
    fn terminal_grid_dims(&self) -> Option<(usize, usize)> {
        let t = self.terminal.as_ref()?;
        let inner = t.grid();
        Some((inner.handler.grid.rows, inner.handler.grid.cols))
    }

    /// Pull the text between the visual anchor and the live cursor
    /// out of the grid and stash it into the unnamed register +
    /// the OS clipboard. Multi-line ranges get a `\n` per row;
    /// trailing spaces on each row are trimmed so a wide selection
    /// over short text doesn't carry a column of blanks.
    fn terminal_yank(&mut self) {
        let Some(anchor) = self.terminal_visual_anchor else {
            // No selection — `y` is a no-op rather than an error so
            // a stray keypress doesn't clobber the register.
            return;
        };
        let cur = self.terminal_cursor;
        let (start, end) = if anchor <= cur { (anchor, cur) } else { (cur, anchor) };
        let text = match self.terminal.as_ref() {
            Some(t) => collect_grid_range(t, start, end),
            None => return,
        };
        let line_count = text.lines().count();
        // Linewise yank when the selection spans entire rows
        // (anchor at col 0, cursor at end-of-line) — but for v1
        // we always treat terminal yank as charwise, matching what
        // the user sees on screen.
        let linewise = false;
        self.write_register(None, text.clone(), linewise);
        if let Ok(mut cb) = arboard::Clipboard::new() {
            let _ = cb.set_text(text);
        }
        self.terminal_visual_anchor = None;
        self.status_msg = match line_count {
            0 | 1 => "yanked terminal selection".into(),
            n => format!("{n} lines yanked from terminal"),
        };
    }
}

/// Collect the chars from the live grid between `start` and `end`
/// inclusive (both `(row, col)`, 0-based). Multi-row selections
/// emit one line per row with a `\n` separator; trailing spaces
/// per row are trimmed so a wide selection over short content
/// doesn't carry a column of blanks. Coordinates are clamped to
/// the grid's current dimensions.
fn collect_grid_range(
    term: &crate::terminal::Terminal,
    start: (usize, usize),
    end: (usize, usize),
) -> String {
    let inner = term.grid();
    let grid = &inner.handler.grid;
    if grid.rows == 0 || grid.cols == 0 {
        return String::new();
    }
    let (s_row, s_col) = (
        start.0.min(grid.rows.saturating_sub(1)),
        start.1.min(grid.cols.saturating_sub(1)),
    );
    let (e_row, e_col) = (
        end.0.min(grid.rows.saturating_sub(1)),
        end.1.min(grid.cols.saturating_sub(1)),
    );
    let mut out = String::new();
    for row in s_row..=e_row {
        let from = if row == s_row { s_col } else { 0 };
        let to = if row == e_row { e_col } else { grid.cols.saturating_sub(1) };
        let line: String = grid.cells[row][from..=to]
            .iter()
            .map(|c| c.ch)
            .collect();
        out.push_str(line.trim_end_matches(' '));
        if row < e_row {
            out.push('\n');
        }
    }
    out
}

/// Translate a `crossterm::KeyEvent` into the byte sequence the
/// PTY's child process should see. Returns `None` for events we
/// can't represent (function keys past F4 if we add them later,
/// etc.). The encoding follows xterm conventions because that's
/// what `TERM=xterm-256color` says we should produce.
fn keyevent_to_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let mut out: Vec<u8> = Vec::new();
    if alt {
        // ESC-prefix for Alt/Meta — universal across xterm-family
        // terminals. Modern shells (zsh, bash with readline) read
        // this as the Meta modifier.
        out.push(0x1b);
    }
    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                // Ctrl-letter → corresponding C0 control code.
                // Ctrl-@ = 0, Ctrl-A = 1, … Ctrl-Z = 26, Ctrl-[ = 27, …
                let upper = c.to_ascii_uppercase();
                if ('A'..='Z').contains(&upper) {
                    out.push((upper as u8) - b'A' + 1);
                } else {
                    // Non-letter with Ctrl — fall through to bare char.
                    let mut buf = [0u8; 4];
                    let s = c.encode_utf8(&mut buf);
                    out.extend_from_slice(s.as_bytes());
                }
            } else {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                out.extend_from_slice(s.as_bytes());
            }
        }
        KeyCode::Enter => out.push(b'\r'),
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        KeyCode::Backspace => out.push(0x7f),
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Left => out.extend_from_slice(b"\x1b[D"),
        KeyCode::Right => out.extend_from_slice(b"\x1b[C"),
        KeyCode::Up => out.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => out.extend_from_slice(b"\x1b[B"),
        KeyCode::Home => out.extend_from_slice(b"\x1b[H"),
        KeyCode::End => out.extend_from_slice(b"\x1b[F"),
        KeyCode::PageUp => out.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => out.extend_from_slice(b"\x1b[6~"),
        KeyCode::Delete => out.extend_from_slice(b"\x1b[3~"),
        KeyCode::Insert => out.extend_from_slice(b"\x1b[2~"),
        KeyCode::F(n) => match n {
            1 => out.extend_from_slice(b"\x1bOP"),
            2 => out.extend_from_slice(b"\x1bOQ"),
            3 => out.extend_from_slice(b"\x1bOR"),
            4 => out.extend_from_slice(b"\x1bOS"),
            // F5+ — xterm-style CSI tildes.
            5 => out.extend_from_slice(b"\x1b[15~"),
            6 => out.extend_from_slice(b"\x1b[17~"),
            7 => out.extend_from_slice(b"\x1b[18~"),
            8 => out.extend_from_slice(b"\x1b[19~"),
            9 => out.extend_from_slice(b"\x1b[20~"),
            10 => out.extend_from_slice(b"\x1b[21~"),
            11 => out.extend_from_slice(b"\x1b[23~"),
            12 => out.extend_from_slice(b"\x1b[24~"),
            _ => return None,
        },
        _ => return None,
    }
    let _ = shift; // Shift isn't independently encoded in the basic
    // xterm forms above — modifier-aware variants exist
    // (`\x1b[1;2A`) but most shells handle the bare forms fine.
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn plain_char_passes_through() {
        let b = keyevent_to_bytes(k(KeyCode::Char('a'), KeyModifiers::NONE)).unwrap();
        assert_eq!(b, b"a");
    }

    #[test]
    fn ctrl_letter_maps_to_c0() {
        let b = keyevent_to_bytes(k(KeyCode::Char('c'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(b, vec![3]); // Ctrl-C
        let b = keyevent_to_bytes(k(KeyCode::Char('d'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(b, vec![4]); // Ctrl-D
    }

    #[test]
    fn arrow_keys_emit_csi_sequences() {
        let b = keyevent_to_bytes(k(KeyCode::Up, KeyModifiers::NONE)).unwrap();
        assert_eq!(b, b"\x1b[A");
        let b = keyevent_to_bytes(k(KeyCode::Right, KeyModifiers::NONE)).unwrap();
        assert_eq!(b, b"\x1b[C");
    }

    #[test]
    fn alt_prefixes_with_escape() {
        let b = keyevent_to_bytes(k(KeyCode::Char('b'), KeyModifiers::ALT)).unwrap();
        // ESC + 'b' — bash readline's Meta-b (back-word).
        assert_eq!(b, vec![0x1b, b'b']);
    }

    #[test]
    fn enter_emits_carriage_return_not_lf() {
        // Most shells expect \r as the line-submission byte; the
        // pty layer turns it into \n for canonical-mode programs.
        let b = keyevent_to_bytes(k(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
        assert_eq!(b, b"\r");
    }

    #[test]
    fn backspace_emits_del_not_bs() {
        // Modern unix terminals send 0x7f (DEL) for backspace; the
        // C0 BS (0x08) is for forward-delete / older terminals.
        let b = keyevent_to_bytes(k(KeyCode::Backspace, KeyModifiers::NONE)).unwrap();
        assert_eq!(b, vec![0x7f]);
    }
}
