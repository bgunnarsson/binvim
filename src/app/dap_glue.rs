//! DAP event handling and `:debug` / `:dap*` ex-command dispatch.
//!
//! Mirrors `app/lsp_glue.rs`: the main loop drains events off the manager
//! and we react here. Editor-side concerns (opening a buffer at the
//! stopped frame, surfacing status messages, opening the bottom pane on
//! session start) live here, not in `dap/manager.rs`.

use crate::command::DebugSubCmd;
use crate::dap::{adapter_for_workspace, DapEvent, SessionState, StepKind};

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
        }
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
    }

    fn dap_toggle_breakpoint(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "debug: buffer has no path".into();
            return;
        };
        let abs = path.canonicalize().unwrap_or(path);
        // Cursor.line is 0-based; DAP / the user-visible line number is 1-based.
        let line = self.cursor.line + 1;
        let added = self.dap.toggle_breakpoint(&abs, line);
        self.status_msg = if added {
            format!("breakpoint set at {}:{}", abs.display(), line)
        } else {
            format!("breakpoint cleared at {}:{}", abs.display(), line)
        };
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
