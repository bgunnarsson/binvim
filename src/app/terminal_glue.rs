//! `:terminal` overlay glue. Wires the standalone `crate::terminal`
//! model into the App: spawns the PTY on `:terminal`, forwards
//! keystrokes from `Mode::Terminal` to the PTY's stdin, drains
//! output on every frame, and tracks the show/hide flag the
//! renderer keys off.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::mode::Mode;
use crate::terminal::Terminal;

impl super::App {
    /// `:terminal [cmd]` — open the embedded terminal overlay. The
    /// argument, if any, is the command to run instead of `$SHELL`.
    /// Re-opening while a terminal is already alive just re-focuses
    /// the existing pane (cheaper than killing + respawning).
    pub(super) fn cmd_open_terminal(&mut self, cmd: Option<String>) {
        if self.terminal.is_some() {
            self.show_terminal_page = true;
            self.terminal_scroll = 0;
            self.mode = Mode::Terminal;
            return;
        }
        // Spawn at the editor's editable size — minus 1 row reserved
        // for the status line. Min dims keep tiny terminals from
        // refusing a 0×0 PTY.
        let rows = (self.height as usize).saturating_sub(1).max(4) as u16;
        let cols = (self.width as usize).max(8) as u16;
        match Terminal::spawn(rows, cols, cmd.as_deref()) {
            Ok(t) => {
                self.terminal = Some(t);
                self.show_terminal_page = true;
                self.terminal_scroll = 0;
                self.mode = Mode::Terminal;
                self.status_msg.clear();
            }
            Err(e) => {
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
        self.show_terminal_page = false;
        self.terminal_scroll = 0;
        if matches!(self.mode, Mode::Terminal | Mode::TerminalNormal) {
            self.mode = Mode::Normal;
        }
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
    /// are back (so `:`, `<C-w>`, etc. work) but the overlay is
    /// still painted. `i` / `a` re-enters `Mode::Terminal`.
    /// Currently only the i/a transition + scroll bindings are
    /// wired here; everything else falls through to the normal
    /// key handler.
    pub(super) fn handle_terminal_normal_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.is_empty() {
            match key.code {
                KeyCode::Char('i') | KeyCode::Char('a') => {
                    self.mode = Mode::Terminal;
                    return true;
                }
                _ => {}
            }
        }
        false
    }
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
