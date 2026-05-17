//! `:terminal` pane glue. Wires the standalone `crate::terminal`
//! model into the App: spawns the PTY on `:terminal`, forwards
//! keystrokes from `Mode::Terminal` to the PTY's stdin, drains
//! output on every frame, and tracks the pane-open flag the
//! renderer keys off.
//!
//! Design choice: the terminal is *just a terminal*. There's no
//! Vim sub-mode for navigating the grid or visual-selecting it
//! for yank — selection works through the host terminal app's
//! native Shift+drag → Cmd-C path. `<C-w>` is the one escape
//! hatch: it drops focus back to `Mode::Normal` and primes the
//! window-leader parser so `<C-w>k` / `<C-w>q` / `<C-w>>` etc.
//! continue to work for the editor windows above. To re-focus
//! the terminal, `<leader>tf` (or `:term`).

use crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::mode::Mode;
use crate::terminal::Terminal;

impl super::App {
    /// Active terminal handle, if any. Centralised because every
    /// call site that used to read `app.terminal.as_ref()` now has
    /// to index into the `Vec<Terminal>` + active-idx pair.
    pub fn active_terminal(&self) -> Option<&crate::terminal::Terminal> {
        self.terminals.get(self.active_terminal_idx)
    }

    /// `:terminal [cmd]` / `<leader>tt` — spawn a new terminal tab.
    /// Always appends, never reuses. With multiple tabs the
    /// rendered pane sprouts a tab strip in the header (active =
    /// blue bg + white text); with one tab the strip stays hidden
    /// and the header keeps showing the hint line. The new tab
    /// becomes active and focus drops into `Mode::Terminal` so the
    /// user can immediately type into the fresh shell.
    pub(super) fn cmd_open_terminal(&mut self, cmd: Option<String>) {
        // Flip the open flag first so terminal_pane_rows() returns
        // the right value when we ask for dimensions. Toggle back
        // off if the spawn fails AND the pane was previously empty.
        let was_empty = self.terminals.is_empty();
        self.terminal_pane_open = true;
        let rows = self.terminal_pane_rows().saturating_sub(1).max(4) as u16;
        let cols = (self.width as usize).max(8) as u16;
        match Terminal::spawn(rows, cols, cmd.as_deref()) {
            Ok(t) => {
                self.terminals.push(t);
                self.active_terminal_idx = self.terminals.len() - 1;
                self.mode = Mode::Terminal;
                self.adjust_viewport();
                self.status_msg.clear();
                // Deliberately NOT calling resize_all_terminals
                // here. The new PTY was opened at the exact size we
                // want (`rows`/`cols` above match the pane body),
                // and existing tabs haven't lost any rows because
                // pane height doesn't depend on tab count. Firing a
                // SIGWINCH on the freshly spawned shell while it
                // hasn't even printed its first prompt yet causes
                // zsh + starship to emit extra clearing sequences,
                // which renders as a blank line between prompts.
            }
            Err(e) => {
                if was_empty {
                    self.terminal_pane_open = false;
                }
                self.status_msg = format!("terminal: spawn failed: {e}");
            }
        }
    }

    /// `<leader>tq` / `:q` while focused on the pane — drop the
    /// active terminal. If it was the last one, hide the pane.
    pub(super) fn close_terminal(&mut self) {
        if self.terminals.is_empty() {
            return;
        }
        let idx = self.active_terminal_idx.min(self.terminals.len() - 1);
        self.terminals.remove(idx);
        if self.terminals.is_empty() {
            self.active_terminal_idx = 0;
            self.terminal_pane_open = false;
            if matches!(self.mode, Mode::Terminal) {
                self.mode = Mode::Normal;
            }
        } else if self.active_terminal_idx >= self.terminals.len() {
            self.active_terminal_idx = self.terminals.len() - 1;
        }
        self.adjust_viewport();
    }

    /// Switch the active tab — bounds-checked. Called from the
    /// mouse handler when the user clicks a tab label.
    pub(super) fn set_active_terminal(&mut self, idx: usize) {
        if idx < self.terminals.len() && idx != self.active_terminal_idx {
            self.active_terminal_idx = idx;
        }
    }

    /// Push the current pane body geometry to every terminal's
    /// PTY (SIGWINCH). Called when the pane gains rows (un-hide,
    /// debug pane closes, host resize) or loses rows. Background
    /// tabs need the resize too — when the user switches to a tab
    /// that's been hidden behind another for a while, its shell
    /// should already have the current winsize so we don't see a
    /// reflow flash on tab switch.
    pub(super) fn resize_all_terminals(&self) {
        if self.terminals.is_empty() {
            return;
        }
        let rows = self.terminal_pane_rows().saturating_sub(1).max(4) as u16;
        let cols = (self.width as usize).max(8) as u16;
        for t in &self.terminals {
            let _ = t.resize(rows, cols);
        }
    }

    /// `<leader>tp` — show/hide the terminal pane WITHOUT killing
    /// the PTY. The point is to be able to start a long-running
    /// process (`pnpm dev`, `cargo watch`, …), tuck it out of the
    /// way while editing, and bring it back later to check on it.
    ///
    ///   - Pane visible       → hide (clear `terminal_pane_open`,
    ///                          drop focus back to Normal if we were
    ///                          typing into it). PTY stays alive
    ///                          and keeps draining bytes into the
    ///                          grid on every frame.
    ///   - Pane hidden + PTY  → show (re-flip the open flag,
    ///                          re-focus into `Mode::Terminal`,
    ///                          resize the PTY to the current pane
    ///                          dimensions in case the host
    ///                          terminal was resized while hidden).
    ///   - No PTY             → spawn a new one (delegate to
    ///                          `cmd_open_terminal`, same as
    ///                          `<leader>tt`).
    pub(super) fn toggle_terminal_pane(&mut self) {
        if self.terminal_pane_open {
            self.terminal_pane_open = false;
            if matches!(self.mode, Mode::Terminal) {
                self.mode = Mode::Normal;
            }
            self.adjust_viewport();
            return;
        }
        if !self.terminals.is_empty() {
            self.terminal_pane_open = true;
            self.adjust_viewport();
            self.resize_all_terminals();
            self.mode = Mode::Terminal;
            return;
        }
        self.cmd_open_terminal(None);
    }

    /// Drain pending PTY output into every terminal's grid. Called
    /// once per render loop. Returns `true` if any bytes were
    /// processed so the caller can mark the frame dirty. Background
    /// tabs drain too — that's the whole point of multi-tab
    /// terminals (`pnpm dev`'s output keeps accumulating while
    /// focus is on a sibling tab).
    pub(super) fn terminal_drain_if_open(&self) -> bool {
        let mut any = false;
        for t in &self.terminals {
            if t.drain() > 0 {
                any = true;
            }
        }
        any
    }

    /// `Mode::Terminal` key dispatch. Two escape hatches:
    ///
    ///   - `Esc` — drops focus to `Mode::Normal`. Simple + matches
    ///     the user's expectation that Esc always means "leave."
    ///   - `<C-w>` — same drop, plus primes the window-leader
    ///     parser so `<C-w>k` / `<C-w>q` / `<C-w>>` continue to
    ///     work for the editor windows above.
    ///
    /// Every other keystroke forwards to the PTY. To send a literal
    /// Esc to the shell (e.g. for a vi-mode escape inside the
    /// embedded program), use `Ctrl-[` — most shells / TUIs accept
    /// it as the canonical Esc control code.
    pub(super) fn handle_terminal_key(&mut self, key: KeyEvent) {
        if key.modifiers.is_empty() && key.code == KeyCode::Esc {
            self.mode = Mode::Normal;
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('w') {
            self.mode = Mode::Normal;
            self.pending.awaiting_window_leader = true;
            return;
        }
        let bytes = match keyevent_to_bytes(key) {
            Some(b) => b,
            None => return,
        };
        if let Some(t) = self.active_terminal() {
            let _ = t.write_bytes(&bytes);
        }
    }

    /// Mouse event dispatch for the terminal pane. Returns `true`
    /// when the event was consumed (no further editor handling).
    /// Click / scroll outside the pane bounds → not consumed.
    /// Inside the pane: if the embedded program has enabled DECSET
    /// mouse tracking, the event is formatted as the appropriate
    /// xterm escape and forwarded to the PTY. Otherwise a click
    /// pulls focus into the terminal (Mode::Terminal); other mouse
    /// events outside tracking mode are dropped on the floor (the
    /// host terminal app's Shift+drag → Cmd-C still works because
    /// that path bypasses crossterm's capture entirely).
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
        // The first pane row is the header. When there are 2+ tabs
        // it's a clickable tab strip; otherwise it's just the
        // [TERMINAL] chip + hint and a click anywhere on it just
        // focuses the pane.
        let body_top = pane_top + 1;
        if row < body_top {
            if matches!(
                ev.kind,
                MouseEventKind::Down(MouseButton::Left | MouseButton::Middle)
            ) {
                if self.terminals.len() > 1 {
                    let hits = self.terminal_tab_hitboxes.take();
                    let mut clicked: Option<usize> = None;
                    for (idx, x_start, x_end) in &hits {
                        if (col as u16) >= *x_start && (col as u16) < *x_end {
                            clicked = Some(*idx);
                            break;
                        }
                    }
                    self.terminal_tab_hitboxes.set(hits);
                    if let Some(idx) = clicked {
                        self.set_active_terminal(idx);
                    }
                }
                self.mode = Mode::Terminal;
            }
            return true;
        }
        // Coords relative to the grid body (1-based — xterm
        // mouse protocol convention).
        let pane_row = row - body_top + 1;
        let pane_col = col + 1;

        let term = match self.active_terminal() {
            Some(t) => t,
            None => return true,
        };
        let mouse = term.mouse_state();
        if !mouse.any {
            if matches!(
                ev.kind,
                MouseEventKind::Down(MouseButton::Left | MouseButton::Middle)
            ) {
                if !matches!(self.mode, Mode::Terminal) {
                    self.mode = Mode::Terminal;
                }
            }
            return true;
        }

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
            MouseEventKind::ScrollUp => (64, false, false),
            MouseEventKind::ScrollDown => (65, false, false),
            _ => return true,
        };
        let mut cb = button;
        if is_drag {
            cb |= 32;
        }
        let bytes = if mouse.sgr {
            let trail = if is_release { 'm' } else { 'M' };
            format!("\x1b[<{cb};{pane_col};{pane_row}{trail}").into_bytes()
        } else {
            let cb_byte = if is_release { 3u32 } else { cb };
            let mut out = Vec::with_capacity(6);
            out.extend_from_slice(b"\x1b[M");
            out.push((cb_byte + 32) as u8);
            out.push((pane_col as u32 + 32).min(255) as u8);
            out.push((pane_row as u32 + 32).min(255) as u8);
            out
        };
        let _ = term.write_bytes(&bytes);
        if matches!(ev.kind, MouseEventKind::Down(_))
            && !matches!(self.mode, Mode::Terminal)
        {
            self.mode = Mode::Terminal;
        }
        true
    }
}

/// Translate a `crossterm::KeyEvent` into the byte sequence the
/// PTY's child process should see. Returns `None` for events we
/// can't represent (function keys past F12, etc.). The encoding
/// follows xterm conventions because that's what `TERM=xterm-
/// 256color` says we should produce.
fn keyevent_to_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let _shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let mut out: Vec<u8> = Vec::new();
    if alt {
        out.push(0x1b);
    }
    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                // Standard C0 control-code mapping for Ctrl-letter
                // and the symbol punctuation that ASCII assigned a
                // control code to:
                //   Ctrl-@ → 0x00, Ctrl-A..Z → 0x01..0x1A,
                //   Ctrl-[ → 0x1B (Esc), Ctrl-\ → 0x1C,
                //   Ctrl-] → 0x1D, Ctrl-^ → 0x1E, Ctrl-_ → 0x1F.
                // Anything else with Ctrl falls back to the raw char.
                let upper = c.to_ascii_uppercase();
                let byte = match upper {
                    'A'..='Z' => Some((upper as u8) - b'A' + 1),
                    '@' => Some(0x00),
                    '[' => Some(0x1b),
                    '\\' => Some(0x1c),
                    ']' => Some(0x1d),
                    '^' => Some(0x1e),
                    '_' => Some(0x1f),
                    _ => None,
                };
                match byte {
                    Some(b) => out.push(b),
                    None => {
                        let mut buf = [0u8; 4];
                        let s = c.encode_utf8(&mut buf);
                        out.extend_from_slice(s.as_bytes());
                    }
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
    fn ctrl_left_bracket_emits_esc_byte() {
        // Ctrl-[ is the canonical control code for Esc (0x1b). The
        // handler reserves bare Esc for "leave terminal mode," so
        // users who need to send Esc into the embedded program
        // (vi-mode shells, vim, less, etc.) press Ctrl-[ — this
        // ensures the byte arrives unchanged at the PTY.
        let b = keyevent_to_bytes(k(KeyCode::Char('['), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(b, vec![0x1b]);
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
        assert_eq!(b, vec![0x1b, b'b']);
    }

    #[test]
    fn enter_emits_carriage_return_not_lf() {
        let b = keyevent_to_bytes(k(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
        assert_eq!(b, b"\r");
    }

    #[test]
    fn backspace_emits_del_not_bs() {
        let b = keyevent_to_bytes(k(KeyCode::Backspace, KeyModifiers::NONE)).unwrap();
        assert_eq!(b, vec![0x7f]);
    }

    #[test]
    fn esc_encoder_still_emits_esc_byte() {
        // `keyevent_to_bytes` doesn't know about modes — it just
        // encodes a keypress. `handle_terminal_key` is what
        // intercepts Esc and switches mode to Normal instead of
        // forwarding. Users who genuinely want to send Esc to the
        // shell use Ctrl-[, which Ctrl-letter encoding routes
        // through this path as 0x1b.
        let b = keyevent_to_bytes(k(KeyCode::Esc, KeyModifiers::NONE)).unwrap();
        assert_eq!(b, vec![0x1b]);
    }
}
