//! `DapManager` owns at most one debug session plus the user's breakpoint
//! table. Drives the adapter's lifecycle (initialize → launch → set
//! breakpoints → configurationDone → run/stop) in response to messages
//! arriving on the reader-thread channel; the main loop calls `drain` to
//! pull the resulting `DapEvent`s off and react.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::client::DapClient;
use super::specs::DapAdapterSpec;
use super::types::{
    DapEvent, DapIncoming, OutputLine, Scope, SessionState, SourceBreakpoint, StackFrame, Variable,
};

#[derive(Default)]
#[allow(dead_code)]
pub struct DapManager {
    /// Breakpoints the user has toggled in the editor, keyed by absolute
    /// path. Persisted across sessions in memory so re-launching reuses
    /// them. The map outlives any session.
    pub breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>,
    /// Active session, if any. `None` between launches.
    pub session: Option<DapSession>,
    /// Rolling debug-console log. Newest at the tail. Bounded so a chatty
    /// program doesn't grow it without limit.
    pub output_buffer: Vec<OutputLine>,
}

const OUTPUT_LOG_CAP: usize = 2000;

#[allow(dead_code)]
pub struct DapSession {
    pub adapter_key: String,
    pub workspace_root: PathBuf,
    pub state: SessionState,
    pub frames: Vec<StackFrame>,
    pub current_thread: Option<u64>,
    /// Variable scopes reported for the top frame (typically just "Locals").
    /// Refreshed on each `stopped` event; cleared on `continued`.
    pub scopes: Vec<Scope>,
    /// Resolved locals for the first non-expensive scope. The pane shows
    /// these directly. Expanding nested values is Phase 3 territory.
    pub locals: Vec<Variable>,
    /// Spawned adapter process + reader channel. Dropping the session
    /// drops the child, which closes the reader thread on its own.
    pub client: DapClient,
    /// Launch-request arguments. Built before spawn and held so they can
    /// be sent the moment the `initialize` response arrives, without
    /// re-running adapter-specific resolution.
    pub launch_args: Value,
    /// Last status message emitted by the adapter (or our own status
    /// machine) — surfaced verbatim in the bottom pane header.
    pub status_line: String,
}

/// What `step()` should ask the adapter to do.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum StepKind {
    Continue,
    Next,
    StepIn,
    StepOut,
}

impl DapManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self) -> bool {
        self.session
            .as_ref()
            .map(|s| !matches!(s.state, SessionState::Terminated))
            .unwrap_or(false)
    }

    pub fn toggle_breakpoint(&mut self, path: &Path, line: usize) -> bool {
        let entry = self.breakpoints.entry(path.to_path_buf()).or_default();
        let added = if let Some(idx) = entry.iter().position(|b| b.line == line) {
            entry.remove(idx);
            if entry.is_empty() {
                self.breakpoints.remove(path);
            }
            false
        } else {
            entry.push(SourceBreakpoint {
                line,
                condition: None,
            });
            true
        };
        // If a session is alive, push the updated source-level list right
        // away so the dot the user just toggled in the gutter takes effect.
        self.resend_breakpoints_for(path);
        added
    }

    /// Drop every breakpoint we know about for `path` and push the empty
    /// list to the adapter if a session is alive. Returns the number of
    /// breakpoints that were cleared so the caller can surface it.
    pub fn clear_breakpoints_in_file(&mut self, path: &Path) -> usize {
        let removed = self
            .breakpoints
            .remove(path)
            .map(|v| v.len())
            .unwrap_or(0);
        if removed > 0 {
            self.resend_breakpoints_for(path);
        }
        removed
    }

    pub fn has_breakpoint(&self, path: &Path, line: usize) -> bool {
        self.breakpoints
            .get(path)
            .map(|v| v.iter().any(|b| b.line == line))
            .unwrap_or(false)
    }

    /// Spawn the adapter, run its prelaunch hook (synchronous — typically
    /// 1-2s for `dotnet build`), and send the initial `initialize` request.
    /// The rest of the handshake is driven by responses + events arriving
    /// on the channel, which `drain` processes.
    pub fn start_session(
        &mut self,
        adapter: DapAdapterSpec,
        root: PathBuf,
    ) -> Result<(), String> {
        if self.is_active() {
            return Err("debug session already active — :dapstop first".into());
        }

        if let Some(pre) = adapter.prelaunch {
            let output = std::process::Command::new(pre.program)
                .args(pre.args)
                .current_dir(&root)
                .output()
                .map_err(|e| format!("{} failed to start: {}", pre.program, e))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                let detail = stderr
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .or_else(|| stdout.lines().rev().find(|l| !l.trim().is_empty()))
                    .unwrap_or("(no output)");
                return Err(format!("{}: {}", pre.label, detail));
            }
        }

        // Resolve launch args before spawning the adapter — if the dll
        // can't be found we avoid leaking an orphan netcoredbg process.
        let launch_args = (adapter.build_launch_args)(&root)?;

        let client = DapClient::spawn_spec(&adapter)
            .ok_or_else(|| format!("could not spawn adapter `{}`", adapter.key))?;

        let init_seq = client.alloc_seq();
        client
            .send_request(
                init_seq,
                "initialize",
                json!({
                    "clientID": "binvim",
                    "clientName": "binvim",
                    // VSCode's well-known type id for .NET — netcoredbg
                    // (and other adapters) gate behaviour on it. Our
                    // internal `adapter.key` ("dotnet") wouldn't match.
                    "adapterID": "coreclr",
                    "pathFormat": "path",
                    "linesStartAt1": true,
                    "columnsStartAt1": true,
                    "supportsVariableType": true,
                    "supportsVariablePaging": false,
                    "supportsRunInTerminalRequest": false,
                }),
            )
            .map_err(|e| format!("initialize send failed: {e}"))?;

        self.session = Some(DapSession {
            adapter_key: adapter.key.to_string(),
            workspace_root: root,
            state: SessionState::Initializing,
            frames: Vec::new(),
            current_thread: None,
            scopes: Vec::new(),
            locals: Vec::new(),
            client,
            launch_args,
            status_line: "initialising adapter…".into(),
        });
        Ok(())
    }

    /// Politely ask the adapter to disconnect; clear local session state.
    /// Best-effort — we drop the client immediately so any in-flight reply
    /// gets discarded.
    pub fn stop_session(&mut self) {
        if let Some(session) = self.session.as_ref() {
            let seq = session.client.alloc_seq();
            let _ = session.client.send_request(
                seq,
                "disconnect",
                json!({
                    "restart": false,
                    "terminateDebuggee": true,
                }),
            );
        }
        self.session = None;
    }

    /// One step / continue command targeted at the currently-stopped
    /// thread. Silently does nothing if the session isn't in a stopped
    /// state — the calling key/command handler decides whether to warn.
    pub fn step(&self, kind: StepKind) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let SessionState::Stopped { thread_id, .. } = session.state else {
            return;
        };
        let (command, arguments) = match kind {
            StepKind::Continue => ("continue", json!({ "threadId": thread_id })),
            StepKind::Next => ("next", json!({ "threadId": thread_id })),
            StepKind::StepIn => ("stepIn", json!({ "threadId": thread_id })),
            StepKind::StepOut => ("stepOut", json!({ "threadId": thread_id })),
        };
        let seq = session.client.alloc_seq();
        let _ = session.client.send_request(seq, command, arguments);
    }

    /// Pull all available messages off the reader-thread channel, run them
    /// through the protocol state machine, and return the editor-facing
    /// `DapEvent`s the main loop should react to.
    pub fn drain(&mut self) -> Vec<DapEvent> {
        let mut events = Vec::new();
        let mut msgs = Vec::new();
        let mut stderr_lines: Vec<OutputLine> = Vec::new();
        let mut exit_code: Option<i32> = None;
        if let Some(session) = self.session.as_ref() {
            while let Ok(msg) = session.client.incoming_rx.try_recv() {
                msgs.push(msg);
            }
            while let Ok(line) = session.client.stderr_rx.try_recv() {
                stderr_lines.push(line);
            }
            // If the adapter exited without going through `terminated`/
            // `exited`, the reader thread will block forever — surface
            // the crash so the user sees something instead of a hang.
            exit_code = session.client.try_exit_status();
        }
        for line in stderr_lines {
            // Stream into the output buffer so the pane shows whatever the
            // adapter printed before dying.
            let trimmed = line.output.clone();
            self.output_buffer.push(line.clone());
            if self.output_buffer.len() > OUTPUT_LOG_CAP {
                let excess = self.output_buffer.len() - OUTPUT_LOG_CAP;
                self.output_buffer.drain(0..excess);
            }
            events.push(DapEvent::Output(line));
            // Mirror the freshest stderr line into the pane status so the
            // user spots the crash without having to scroll the output.
            if let Some(s) = self.session.as_mut() {
                s.status_line = trimmed;
            }
        }
        for msg in msgs {
            self.process_incoming(msg, &mut events);
        }
        if let Some(code) = exit_code {
            // Don't double-report if the protocol path already saw
            // `terminated` and cleared the session.
            if self.session.is_some() {
                let msg = format!("adapter exited unexpectedly (code {})", code);
                if let Some(s) = self.session.as_mut() {
                    s.state = SessionState::Terminated;
                    s.status_line = msg.clone();
                }
                events.push(DapEvent::AdapterError(msg));
                events.push(DapEvent::Terminated);
            }
        }
        events
    }

    fn process_incoming(&mut self, msg: DapIncoming, events: &mut Vec<DapEvent>) {
        match msg {
            DapIncoming::Response {
                command,
                success,
                body,
                message,
                ..
            } => self.handle_response(command, success, body, message, events),
            DapIncoming::Event { event, body } => self.handle_event(event, body, events),
            DapIncoming::Request {
                seq, command, ..
            } => {
                // We don't support any server-to-client requests yet; reply
                // unsuccessfully so the adapter doesn't sit waiting.
                if let Some(session) = self.session.as_ref() {
                    let _ = session
                        .client
                        .send_response(seq, &command, false, Value::Null);
                }
            }
        }
    }

    fn handle_response(
        &mut self,
        command: String,
        success: bool,
        body: Value,
        message: Option<String>,
        events: &mut Vec<DapEvent>,
    ) {
        if !success {
            let detail = message.unwrap_or_else(|| "(no message)".into());
            let err = format!("{} failed: {}", command, detail);
            if let Some(s) = self.session.as_mut() {
                s.status_line = err.clone();
            }
            events.push(DapEvent::AdapterError(err));
            return;
        }
        match command.as_str() {
            "initialize" => {
                events.push(DapEvent::Initialized);
                if let Some(session) = self.session.as_mut() {
                    session.status_line = "launching debuggee…".into();
                }
                if let Some(session) = self.session.as_ref() {
                    let seq = session.client.alloc_seq();
                    let _ = session.client.send_request(
                        seq,
                        "launch",
                        session.launch_args.clone(),
                    );
                }
            }
            "launch" => {
                if let Some(s) = self.session.as_mut() {
                    s.state = SessionState::Configuring;
                }
            }
            "configurationDone" => {
                if let Some(s) = self.session.as_mut() {
                    s.state = SessionState::Running;
                    s.status_line = "running".into();
                }
            }
            "stackTrace" => {
                let frames = parse_stack_frames(&body);
                let top_id = frames.first().map(|f| f.id);
                if let Some(s) = self.session.as_mut() {
                    s.frames = frames;
                }
                // Auto-chain into scopes for the top frame so the pane can
                // show locals without an extra command from the user.
                if let (Some(id), Some(session)) = (top_id, self.session.as_ref()) {
                    let seq = session.client.alloc_seq();
                    let _ = session.client.send_request(
                        seq,
                        "scopes",
                        json!({ "frameId": id }),
                    );
                }
            }
            "scopes" => {
                let scopes = parse_scopes(&body);
                // Pick the first non-expensive scope (typically "Locals").
                let target = scopes
                    .iter()
                    .find(|s| !s.expensive)
                    .map(|s| s.variables_reference);
                if let Some(s) = self.session.as_mut() {
                    s.scopes = scopes;
                    s.locals.clear();
                }
                if let (Some(vref), Some(session)) = (target, self.session.as_ref()) {
                    let seq = session.client.alloc_seq();
                    let _ = session.client.send_request(
                        seq,
                        "variables",
                        json!({ "variablesReference": vref }),
                    );
                }
            }
            "variables" => {
                let vars = parse_variables(&body);
                if let Some(s) = self.session.as_mut() {
                    s.locals = vars;
                }
            }
            _ => {}
        }
    }

    fn handle_event(&mut self, event: String, body: Value, events: &mut Vec<DapEvent>) {
        match event.as_str() {
            "initialized" => {
                self.send_breakpoints_and_configdone();
            }
            "stopped" => {
                let thread_id = body
                    .get("threadId")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let reason = body
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let description = body
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let hit_breakpoint_ids: Vec<u64> = body
                    .get("hitBreakpointIds")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_u64()).collect())
                    .unwrap_or_default();
                if let Some(s) = self.session.as_mut() {
                    s.state = SessionState::Stopped {
                        thread_id,
                        reason: reason.clone(),
                    };
                    s.current_thread = Some(thread_id);
                    s.status_line = format!("stopped — {}", reason);
                }
                // Kick off a stackTrace request so the pane has frames to
                // show as soon as the user reads the "stopped" status.
                if let Some(session) = self.session.as_ref() {
                    let seq = session.client.alloc_seq();
                    let _ = session.client.send_request(
                        seq,
                        "stackTrace",
                        json!({
                            "threadId": thread_id,
                            "startFrame": 0,
                            "levels": 20,
                        }),
                    );
                }
                events.push(DapEvent::Stopped {
                    thread_id,
                    reason,
                    description,
                    hit_breakpoint_ids,
                });
            }
            "continued" => {
                let thread_id = body.get("threadId").and_then(|v| v.as_u64());
                let all_threads = body
                    .get("allThreadsContinued")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if let Some(s) = self.session.as_mut() {
                    s.frames.clear();
                    s.scopes.clear();
                    s.locals.clear();
                    s.current_thread = None;
                    if !matches!(s.state, SessionState::Terminated) {
                        s.state = SessionState::Running;
                        s.status_line = "running".into();
                    }
                }
                events.push(DapEvent::Continued {
                    thread_id,
                    all_threads,
                });
            }
            "output" => {
                let category = body
                    .get("category")
                    .and_then(|v| v.as_str())
                    .unwrap_or("console")
                    .to_string();
                let output = body
                    .get("output")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let line = OutputLine { category, output };
                self.output_buffer.push(line.clone());
                if self.output_buffer.len() > OUTPUT_LOG_CAP {
                    let excess = self.output_buffer.len() - OUTPUT_LOG_CAP;
                    self.output_buffer.drain(0..excess);
                }
                events.push(DapEvent::Output(line));
            }
            "terminated" => {
                if let Some(s) = self.session.as_mut() {
                    s.state = SessionState::Terminated;
                    s.status_line = "terminated".into();
                }
                events.push(DapEvent::Terminated);
            }
            "exited" => {
                let code = body.get("exitCode").and_then(|v| v.as_i64()).unwrap_or(0);
                if let Some(s) = self.session.as_mut() {
                    s.status_line = format!("exited ({})", code);
                }
                events.push(DapEvent::Exited { exit_code: code });
            }
            "thread" => {
                let thread_id = body
                    .get("threadId")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let reason = body
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                events.push(DapEvent::Thread { reason, thread_id });
            }
            _ => {}
        }
    }

    fn send_breakpoints_and_configdone(&self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        // setBreakpoints replaces the adapter's per-source list — one
        // request per file. Empty list is fine: a no-breakpoints source
        // doesn't need a request, but sending it is harmless.
        for (path, list) in &self.breakpoints {
            let seq = session.client.alloc_seq();
            let bps_json: Vec<Value> = list
                .iter()
                .map(|b| {
                    let mut o = serde_json::Map::new();
                    o.insert("line".into(), json!(b.line));
                    if let Some(c) = &b.condition {
                        o.insert("condition".into(), json!(c));
                    }
                    Value::Object(o)
                })
                .collect();
            let _ = session.client.send_request(
                seq,
                "setBreakpoints",
                json!({
                    "source": { "path": path.display().to_string() },
                    "breakpoints": bps_json,
                }),
            );
        }
        let seq = session.client.alloc_seq();
        let _ = session
            .client
            .send_request(seq, "configurationDone", json!({}));
    }

    fn resend_breakpoints_for(&self, path: &Path) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        if matches!(session.state, SessionState::Terminated) {
            return;
        }
        let list = self.breakpoints.get(path).cloned().unwrap_or_default();
        let bps_json: Vec<Value> = list
            .iter()
            .map(|b| json!({ "line": b.line }))
            .collect();
        let seq = session.client.alloc_seq();
        let _ = session.client.send_request(
            seq,
            "setBreakpoints",
            json!({
                "source": { "path": path.display().to_string() },
                "breakpoints": bps_json,
            }),
        );
    }
}

fn parse_scopes(body: &Value) -> Vec<Scope> {
    let mut out = Vec::new();
    let Some(arr) = body.get("scopes").and_then(|v| v.as_array()) else {
        return out;
    };
    for s in arr {
        let name = s
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let variables_reference = s
            .get("variablesReference")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let expensive = s.get("expensive").and_then(|v| v.as_bool()).unwrap_or(false);
        out.push(Scope {
            name,
            variables_reference,
            expensive,
        });
    }
    out
}

fn parse_variables(body: &Value) -> Vec<Variable> {
    let mut out = Vec::new();
    let Some(arr) = body.get("variables").and_then(|v| v.as_array()) else {
        return out;
    };
    for v in arr {
        let name = v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let value = v
            .get("value")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let type_name = v.get("type").and_then(|x| x.as_str()).map(|s| s.to_string());
        let variables_reference = v
            .get("variablesReference")
            .and_then(|x| x.as_u64())
            .unwrap_or(0);
        out.push(Variable {
            name,
            value,
            type_name,
            variables_reference,
        });
    }
    out
}

fn parse_stack_frames(body: &Value) -> Vec<StackFrame> {
    let mut out = Vec::new();
    let Some(arr) = body.get("stackFrames").and_then(|v| v.as_array()) else {
        return out;
    };
    for f in arr {
        let id = f.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
        let name = f
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let line = f.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let column = f.get("column").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let source = f
            .get("source")
            .and_then(|s| s.get("path"))
            .and_then(|p| p.as_str())
            .map(PathBuf::from);
        out.push(StackFrame {
            id,
            name,
            source,
            line,
            column,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_breakpoint_adds_and_removes() {
        let mut m = DapManager::new();
        let p = PathBuf::from("/tmp/x.cs");
        assert!(!m.has_breakpoint(&p, 10));
        assert!(m.toggle_breakpoint(&p, 10));
        assert!(m.has_breakpoint(&p, 10));
        assert!(!m.toggle_breakpoint(&p, 10));
        assert!(!m.has_breakpoint(&p, 10));
        assert!(m.breakpoints.is_empty());
    }

    #[test]
    fn breakpoint_table_is_per_path() {
        let mut m = DapManager::new();
        let a = PathBuf::from("/tmp/a.cs");
        let b = PathBuf::from("/tmp/b.cs");
        m.toggle_breakpoint(&a, 5);
        m.toggle_breakpoint(&b, 5);
        assert!(m.has_breakpoint(&a, 5));
        assert!(m.has_breakpoint(&b, 5));
        assert_eq!(m.breakpoints.len(), 2);
    }

    #[test]
    fn idle_manager_is_inactive_and_drains_empty() {
        let mut m = DapManager::new();
        assert!(!m.is_active());
        assert!(m.drain().is_empty());
    }

    #[test]
    fn step_on_idle_manager_is_noop() {
        // Without a session, step() should not panic — it just early-returns.
        let m = DapManager::new();
        m.step(StepKind::Continue);
        m.step(StepKind::Next);
        m.step(StepKind::StepIn);
        m.step(StepKind::StepOut);
    }
}
