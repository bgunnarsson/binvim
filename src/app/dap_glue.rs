//! DAP event handling and `:debug` / `:dap*` ex-command dispatch.
//!
//! Mirrors `app/lsp_glue.rs`: the main loop drains events off the manager
//! and we react here. Editor-side concerns (opening a buffer at the
//! stopped frame, surfacing status messages, opening the bottom pane on
//! session start) live here, not in `dap/manager.rs`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::command::DebugSubCmd;
use crate::dap::{adapter_for_workspace, flat_locals_view, DapEvent, SessionState, StepKind};
use crate::mode::Mode;

impl super::App {
    pub(super) fn dispatch_debug(&mut self, sub: DebugSubCmd) {
        match sub {
            DebugSubCmd::Start => self.dap_start_session(),
            DebugSubCmd::Stop => self.dap_stop_session(),
            DebugSubCmd::Break => self.dap_toggle_breakpoint(),
            DebugSubCmd::ClearBreakpointsInFile => self.dap_clear_breakpoints_in_file(),
            DebugSubCmd::Continue => self.dap_step(StepKind::Continue),
            DebugSubCmd::Next => self.dap_step(StepKind::Next),
            DebugSubCmd::StepIn => self.dap_step(StepKind::StepIn),
            DebugSubCmd::StepOut => self.dap_step(StepKind::StepOut),
            DebugSubCmd::PaneToggle => {
                self.debug_pane_open = !self.debug_pane_open;
                self.adjust_viewport();
                self.status_msg = if self.debug_pane_open {
                    "debug pane: open".into()
                } else {
                    "debug pane: closed".into()
                };
            }
            DebugSubCmd::FocusPane => self.dap_enter_pane_focus(),
        }
    }

    fn dap_enter_pane_focus(&mut self) {
        if !self.dap.is_active() {
            self.status_msg = "debug: no active session".into();
            return;
        }
        if !self.debug_pane_open {
            self.debug_pane_open = true;
            self.adjust_viewport();
        }
        self.dap_pane_cursor = 0;
        self.dap_right_scroll = 0;
        // Park the left column on the last couple of frames so the
        // separator and first locals are visible by default — without
        // hiding the user-relevant frame context above.
        let frames_len = self
            .dap
            .session
            .as_ref()
            .map(|s| s.frames.len())
            .unwrap_or(0);
        self.dap_left_scroll = frames_len.saturating_sub(2);
        self.mode = Mode::DebugPane;
        self.status_msg =
            "pane: j/k navigate · ^Y/^E scroll · J/K log · Enter expand · Esc exits".into();
    }

    pub(super) fn dap_exit_pane_focus(&mut self) {
        if self.mode == Mode::DebugPane {
            self.mode = Mode::Normal;
            self.status_msg.clear();
        }
    }

    /// Key dispatch for `Mode::DebugPane`. Returns `true` if the key was
    /// consumed (the caller skips the normal-mode dispatch in that case).
    pub(super) fn handle_debug_pane_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let locals_len = self
            .dap
            .session
            .as_ref()
            .map(|s| flat_locals_view(s).len())
            .unwrap_or(0);
        match key.code {
            KeyCode::Esc => {
                self.dap_exit_pane_focus();
                true
            }
            // Ctrl-Y / Ctrl-E: scroll the left column without moving the
            // selection — Vim-convention free scroll for peeking at
            // frames above the locals.
            KeyCode::Char('y') if ctrl => {
                self.dap_left_scroll = self.dap_left_scroll.saturating_sub(1);
                true
            }
            KeyCode::Char('e') if ctrl => {
                self.dap_left_scroll = self
                    .dap_left_scroll
                    .saturating_add(1)
                    .min(self.dap_left_scroll_max());
                true
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if locals_len > 0 {
                    self.dap_pane_cursor = (self.dap_pane_cursor + 1).min(locals_len - 1);
                    self.dap_follow_selection_in_left_column();
                }
                true
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.dap_pane_cursor = self.dap_pane_cursor.saturating_sub(1);
                self.dap_follow_selection_in_left_column();
                true
            }
            KeyCode::Char('g') => {
                self.dap_pane_cursor = 0;
                self.dap_follow_selection_in_left_column();
                true
            }
            KeyCode::Char('G') => {
                self.dap_pane_cursor = locals_len.saturating_sub(1);
                self.dap_follow_selection_in_left_column();
                true
            }
            // Right column scrolling — capital J/K so lowercase stays
            // bound to locals navigation. J pages toward the latest log
            // line; K pages back into older history.
            KeyCode::Char('J') => {
                self.dap_right_scroll = self.dap_right_scroll.saturating_sub(1);
                true
            }
            KeyCode::Char('K') => {
                self.dap_right_scroll = self
                    .dap_right_scroll
                    .saturating_add(1)
                    .min(self.dap_right_scroll_max());
                true
            }
            KeyCode::Enter | KeyCode::Tab | KeyCode::Char(' ') => {
                self.dap_pane_toggle_at_cursor();
                self.dap_follow_selection_in_left_column();
                true
            }
            // Stepping bindings while focus is in the pane — saves an
            // Esc + <leader>d{c,n,i,O} round-trip during inspection.
            KeyCode::Char('c') => {
                self.dap.step(StepKind::Continue);
                true
            }
            KeyCode::Char('n') => {
                self.dap.step(StepKind::Next);
                true
            }
            KeyCode::Char('i') => {
                self.dap.step(StepKind::StepIn);
                true
            }
            KeyCode::Char('O') => {
                self.dap.step(StepKind::StepOut);
                true
            }
            // Ex-command escape hatch so the user can `:dapstop`/etc from
            // inside the pane without having to bounce through Normal.
            KeyCode::Char(':') => {
                self.mode = Mode::Command;
                self.cmdline.clear();
                true
            }
            _ => false,
        }
    }

    /// Maximum valid value for `dap_left_scroll`. The total left-column
    /// row count (frames + optional separator + locals tree) minus the
    /// number of body rows the pane currently has.
    pub(super) fn dap_left_scroll_max(&self) -> usize {
        let Some(session) = self.dap.session.as_ref() else {
            return 0;
        };
        let flat = flat_locals_view(session);
        let total = if flat.is_empty() {
            session.frames.len()
        } else {
            session.frames.len() + 1 + flat.len()
        };
        let body_rows = self.debug_pane_rows().saturating_sub(1);
        total.saturating_sub(body_rows)
    }

    /// Maximum valid value for `dap_right_scroll`. Counts every output
    /// line currently in the buffer; the buffer is bounded by
    /// `OUTPUT_LOG_CAP` so this is cheap.
    pub(super) fn dap_right_scroll_max(&self) -> usize {
        let total_lines: usize = self
            .dap
            .output_buffer
            .iter()
            .map(|o| o.output.lines().count().max(1))
            .sum();
        let body_rows = self.debug_pane_rows().saturating_sub(1);
        total_lines.saturating_sub(body_rows)
    }

    /// Adjust `dap_left_scroll` so the currently-selected local is in
    /// the visible viewport. Called after every selection-moving key.
    fn dap_follow_selection_in_left_column(&mut self) {
        let Some(session) = self.dap.session.as_ref() else {
            return;
        };
        let frames_len = session.frames.len();
        let flat_len = flat_locals_view(session).len();
        if flat_len == 0 {
            return;
        }
        let cursor = self.dap_pane_cursor.min(flat_len - 1);
        // Locals occupy rows `[frames_len + 1, frames_len + 1 + flat_len)`
        // — the `+1` accounts for the "── Locals ──" separator row.
        let selected_abs = frames_len + 1 + cursor;
        let body_rows = self.debug_pane_rows().saturating_sub(1);
        if body_rows == 0 {
            return;
        }
        if selected_abs < self.dap_left_scroll {
            self.dap_left_scroll = selected_abs;
        }
        let last_visible = self.dap_left_scroll + body_rows;
        if selected_abs >= last_visible {
            self.dap_left_scroll = selected_abs + 1 - body_rows;
        }
        let max = self.dap_left_scroll_max();
        if self.dap_left_scroll > max {
            self.dap_left_scroll = max;
        }
    }

    /// Visual Studio / Rider-style debug shortcut keys, dispatched
    /// regardless of editor mode so the muscle memory works during
    /// editing too. Returns `true` if the key was consumed.
    ///
    /// - `F5`        — start session (or continue if already paused)
    /// - `Shift+F5`  — stop session
    /// - `F9`        — toggle breakpoint at cursor
    /// - `F10`       — step over (next)
    /// - `F11`       — step into
    /// - `Shift+F11` — step out
    pub(super) fn try_handle_debug_function_key(&mut self, k: &KeyEvent) -> bool {
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let modless = k
            .modifiers
            .difference(KeyModifiers::SHIFT)
            .is_empty();
        if !modless {
            return false;
        }
        match k.code {
            KeyCode::F(5) if shift => {
                self.dispatch_debug(DebugSubCmd::Stop);
                true
            }
            KeyCode::F(5) => {
                // F5 doubles as "start if there's no session yet" so the
                // user doesn't have to remember a separate Start binding.
                if self.dap.is_active() {
                    self.dispatch_debug(DebugSubCmd::Continue);
                } else {
                    self.dispatch_debug(DebugSubCmd::Start);
                }
                true
            }
            KeyCode::F(9) => {
                self.dispatch_debug(DebugSubCmd::Break);
                true
            }
            KeyCode::F(10) => {
                self.dispatch_debug(DebugSubCmd::Next);
                true
            }
            KeyCode::F(11) if shift => {
                self.dispatch_debug(DebugSubCmd::StepOut);
                true
            }
            KeyCode::F(11) => {
                self.dispatch_debug(DebugSubCmd::StepIn);
                true
            }
            _ => false,
        }
    }

    fn dap_pane_toggle_at_cursor(&mut self) {
        // Pull out the vref to toggle in a tight `&self` scope so the
        // ensuing `toggle_expanded` call can take `&mut self.dap`.
        let vref = {
            let Some(session) = self.dap.session.as_ref() else {
                return;
            };
            let flat = flat_locals_view(session);
            if flat.is_empty() {
                return;
            }
            let idx = self.dap_pane_cursor.min(flat.len() - 1);
            let row = &flat[idx];
            if !row.expandable {
                return;
            }
            row.var.variables_reference
        };
        self.dap.toggle_expanded(vref);
    }

    fn dap_start_session(&mut self) {
        let start_dir = self
            .buffer
            .path
            .as_ref()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
        let Some((adapter, root)) = adapter_for_workspace(&start_dir) else {
            self.status_msg =
                "debug: no adapter found for this workspace (need a *.csproj/*.sln)".into();
            return;
        };
        // Surface the build progress before the blocking prelaunch step.
        // We can't redraw mid-call (the prelaunch runs synchronously) but
        // setting the status here means it's at least visible after the
        // build finishes if there was no error.
        self.status_msg = format!("debug: {} ({})", adapter.key, root.display());
        // Force the pane open so the user can see status as the handshake
        // unfolds.
        self.debug_pane_open = true;
        self.adjust_viewport();
        match self.dap.start_session(adapter, root) {
            Ok(()) => {
                self.status_msg = "debug: session starting".into();
            }
            Err(e) => {
                self.status_msg = format!("debug: {e}");
            }
        }
    }

    fn dap_stop_session(&mut self) {
        if !self.dap.is_active() {
            self.status_msg = "debug: no active session".into();
            return;
        }
        self.dap.stop_session();
        self.status_msg = "debug: session terminated".into();
        if self.mode == Mode::DebugPane {
            self.mode = Mode::Normal;
        }
        // Close the bottom pane on stop — the session is gone, there's
        // nothing useful to look at, and reclaiming the rows snaps the
        // editor back to its usual height.
        if self.debug_pane_open {
            self.debug_pane_open = false;
            self.adjust_viewport();
        }
    }

    fn dap_toggle_breakpoint(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "debug: buffer has no path".into();
            return;
        };
        let abs = path.canonicalize().unwrap_or(path);
        // Cursor.line is 0-based; DAP / the user-visible line number is 1-based.
        let line = self.cursor.line + 1;
        // Toggle the breakpoint silently — the gutter dot is the
        // user-visible confirmation, so a status-line notification is
        // redundant noise on every press.
        let _ = self.dap.toggle_breakpoint(&abs, line);
    }

    fn dap_clear_breakpoints_in_file(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "debug: buffer has no path".into();
            return;
        };
        let abs = path.canonicalize().unwrap_or(path);
        let n = self.dap.clear_breakpoints_in_file(&abs);
        self.status_msg = match n {
            0 => "debug: no breakpoints in this file".into(),
            1 => "debug: cleared 1 breakpoint".into(),
            n => format!("debug: cleared {n} breakpoints"),
        };
    }

    fn dap_step(&mut self, kind: StepKind) {
        if !self.dap.is_active() {
            self.status_msg = "debug: no active session".into();
            return;
        }
        let stopped = self
            .dap
            .session
            .as_ref()
            .map(|s| matches!(s.state, SessionState::Stopped { .. }))
            .unwrap_or(false);
        if !stopped {
            self.status_msg = "debug: program is not paused".into();
            return;
        }
        self.dap.step(kind);
    }

    pub(super) fn handle_dap_events(&mut self, events: Vec<DapEvent>) {
        for ev in events {
            match ev {
                DapEvent::Initialized => {}
                DapEvent::Stopped {
                    thread_id, reason, ..
                } => {
                    self.status_msg = format!("debug: stopped — {} (thread {})", reason, thread_id);
                    // Reset the pane scroll positions so the user sees
                    // the new stop's frame + locals from a sensible
                    // starting position rather than wherever the previous
                    // stop's viewport happened to land.
                    self.dap_pane_cursor = 0;
                    self.dap_right_scroll = 0;
                    self.dap_left_scroll = 0;
                    self.dap_jump_to_top_frame();
                }
                DapEvent::Continued { .. } => {
                    self.status_msg = "debug: running".into();
                }
                DapEvent::Output(_) => {}
                DapEvent::Thread { reason, thread_id } => {
                    if reason == "exited" {
                        self.status_msg = format!("debug: thread {} exited", thread_id);
                    }
                }
                DapEvent::Breakpoint { .. } => {}
                DapEvent::Exited { exit_code } => {
                    self.status_msg = format!("debug: debuggee exited ({})", exit_code);
                }
                DapEvent::Terminated => {
                    self.status_msg = "debug: session ended".into();
                    self.dap.session = None;
                    if self.mode == Mode::DebugPane {
                        self.mode = Mode::Normal;
                    }
                    if self.debug_pane_open {
                        self.debug_pane_open = false;
                        self.adjust_viewport();
                    }
                }
                DapEvent::AdapterError(msg) => {
                    self.status_msg = format!("debug error: {msg}");
                }
            }
        }
    }

    /// On a `stopped` event the manager has already requested the stack
    /// trace; by the time the renderer's next frame runs the top frame
    /// has typically arrived. Open the source file (if needed) and put
    /// the cursor on the frame's line so the user immediately sees where
    /// execution paused.
    fn dap_jump_to_top_frame(&mut self) {
        let Some(session) = self.dap.session.as_ref() else {
            return;
        };
        let Some(top) = session.frames.first() else {
            return;
        };
        let Some(path) = top.source.clone() else {
            return;
        };
        // DAP frames carry 1-based line numbers; binvim's cursor is 0-based.
        let line = top.line.saturating_sub(1);
        let col = top.column.saturating_sub(1);
        let already_open = self
            .buffer
            .path
            .as_ref()
            .map(|p| p == &path || p.canonicalize().ok() == path.canonicalize().ok())
            .unwrap_or(false);
        if !already_open {
            self.push_jump();
            if let Err(e) = self.open_buffer(path) {
                self.status_msg = format!("debug: cannot open frame source: {e}");
                return;
            }
        }
        self.cursor.line = line;
        self.cursor.col = col;
        self.cursor.want_col = col;
        self.clamp_cursor_normal();
        self.adjust_viewport();
    }
}
