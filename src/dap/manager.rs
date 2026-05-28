//! `DapManager` owns at most one debug session plus the user's breakpoint
//! table. Drives the adapter's lifecycle (initialize → launch → set
//! breakpoints → configurationDone → run/stop) in response to messages
//! arriving on the reader-thread channel; the main loop calls `drain` to
//! pull the resulting `DapEvent`s off and react.

use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use super::client::DapClient;
use super::specs::DapAdapterSpec;
use super::types::{
    DapEvent, DapIncoming, DapWatch, DapWatchResult, OutputLine, Scope, SessionState,
    SourceBreakpoint, StackFrame, Variable,
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
    /// User-managed watch expressions, evaluated against the top frame on
    /// every `stopped` event. The list survives across sessions — only the
    /// `result` field on each entry clears between sessions.
    pub watches: Vec<DapWatch>,
    /// In-flight prelaunch build (`dotnet build`, `cargo build`, …). Held
    /// here rather than blocking the main thread inside `start_session` so
    /// the editor stays responsive and the build's stdout/stderr can stream
    /// into the Console tab live. `drain` watches for child exit and
    /// transitions into the real `DapSession` on success.
    pub pending_build: Option<PendingBuild>,
}

/// One asynchronously-running adapter prelaunch. Built by `start_session`
/// when the adapter spec declares a `PrelaunchCommand`; consumed by `drain`
/// once the child process exits. The reader threads (one per pipe) push
/// each line into the matching `Receiver`; `drain` promotes those into
/// `output_buffer` entries categorised "stdout"/"stderr" so the Console
/// tab paints them with the same colours as live debuggee output.
#[allow(dead_code)]
pub struct PendingBuild {
    /// Adapter to spawn after the build completes successfully. Moved
    /// into `start_session_post_build` on transition.
    pub adapter: DapAdapterSpec,
    /// Launch context to pass to the adapter post-build.
    pub ctx: super::specs::LaunchContext,
    /// Child process running the prelaunch command.
    pub child: std::process::Child,
    /// Stdout/stderr lines from the reader threads, drained per `drain` tick.
    pub stdout_rx: Receiver<String>,
    pub stderr_rx: Receiver<String>,
    /// Human label for status / completion messages ("Building .NET project").
    pub label: String,
    /// Spawn time — used to render elapsed seconds in the completion message.
    pub started_at: std::time::Instant,
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
    /// `variables_reference` of the scope whose contents are displayed in
    /// the pane as "locals". Picked as the first non-expensive scope on
    /// each stop; expansion further into structured values lives in
    /// `children` and `expanded`.
    pub scope_for_display: Option<u64>,
    /// Cached children per `variables_reference`. The pane's root locals
    /// are `children[scope_for_display]`; expanding a variable populates
    /// `children[var.variables_reference]` lazily.
    pub children: HashMap<u64, Vec<Variable>>,
    /// `variables_reference`s the user has toggled open. Persisted across
    /// pane re-renders; cleared on each stop so stale handles don't leak
    /// between stops (DAP doesn't promise vref stability).
    pub expanded: HashSet<u64>,
    /// In-flight `variables` requests — `request_seq → parent_vref`. Lets
    /// the response handler store children under the right parent when
    /// several fetches are outstanding (e.g. user expands a deeply-nested
    /// branch quickly).
    pub pending_variable_fetches: HashMap<u64, u64>,
    /// In-flight `evaluate` requests for watch expressions —
    /// `request_seq → index into DapManager.watches`. The response
    /// handler uses this to update the right watch entry's `result`.
    pub pending_watch_evals: HashMap<u64, usize>,
    /// Spawned adapter process + reader channel. Dropping the session
    /// drops the child, which closes the reader thread on its own.
    pub client: DapClient,
    /// Launch-request arguments. Built before spawn and held so they can
    /// be sent the moment the `initialize` response arrives, without
    /// re-running adapter-specific resolution. For an attach session this
    /// holds the `attach`-request arguments instead.
    pub launch_args: Value,
    /// The configuration request sent after `initialize` — `"launch"` for a
    /// spawned adapter, `"attach"` for a TCP attach (Android / JDWP).
    pub request_command: &'static str,
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
        if self.pending_build.is_some() {
            return true;
        }
        self.session
            .as_ref()
            .map(|s| !matches!(s.state, SessionState::Terminated))
            .unwrap_or(false)
    }

    /// Human-readable label for the in-flight prelaunch, e.g. "Building
    /// .NET project". Used by the editor to surface "debug: building …"
    /// in the status line so the user knows what the editor is doing
    /// while the build streams output into the Console tab.
    pub fn pending_build_label(&self) -> Option<&str> {
        self.pending_build.as_ref().map(|b| b.label.as_str())
    }

    /// Adapter key (`"dotnet"`, `"lldb"`, …) of the in-flight prelaunch.
    /// Used by the debug pane header to show `[DEBUG | <adapter>]` while
    /// the build is running but before `self.session` exists.
    pub fn pending_build_adapter_key(&self) -> Option<&str> {
        self.pending_build.as_ref().map(|b| b.adapter.key)
    }

    /// Append `line` to `output_buffer`, evicting from the head if the
    /// rolling cap is exceeded. Centralised so the prelaunch streaming
    /// path and the adapter-side push paths share the same eviction.
    fn push_output_line(&mut self, line: OutputLine) {
        self.output_buffer.push(line);
        if self.output_buffer.len() > OUTPUT_LOG_CAP {
            let excess = self.output_buffer.len() - OUTPUT_LOG_CAP;
            self.output_buffer.drain(0..excess);
        }
    }

    /// Kill any in-flight prelaunch build and reap its child. Called
    /// from `stop_session`/`stop_session_blocking` so a pending build
    /// gets cancelled when the user explicitly stops the session, or
    /// when `dap_start_session` restarts over a still-building previous
    /// launch.
    fn cancel_pending_build(&mut self) {
        if let Some(mut b) = self.pending_build.take() {
            let _ = b.child.kill();
            let _ = b.child.wait();
            let msg = format!("✗ {} cancelled", b.label);
            self.push_output_line(OutputLine {
                category: "stderr".into(),
                output: msg,
            });
        }
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
                hit_condition: None,
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
        let removed = self.breakpoints.remove(path).map(|v| v.len()).unwrap_or(0);
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

    /// Read accessor for the breakpoint at `(path, line)`. The gutter
    /// renderer and the breakpoints pane use this to pick a glyph /
    /// surface the condition string. Returns `None` when no breakpoint
    /// exists at that site; cloning is cheap (Strings are short).
    pub fn breakpoint_at(&self, path: &Path, line: usize) -> Option<SourceBreakpoint> {
        self.breakpoints
            .get(path)
            .and_then(|v| v.iter().find(|b| b.line == line))
            .cloned()
    }

    /// Set (or replace) the `condition` on the breakpoint at `(path,
    /// line)`. Creates an unconditional breakpoint first if none
    /// exists. Returns `true` when the call mutated state (so the
    /// caller can surface a confirmation), `false` only if `path`
    /// is somehow not absolutisable — currently always true in
    /// practice. Push to the adapter immediately if a session is
    /// alive.
    pub fn set_breakpoint_condition(
        &mut self,
        path: &Path,
        line: usize,
        condition: Option<String>,
    ) -> bool {
        let entry = self.breakpoints.entry(path.to_path_buf()).or_default();
        if let Some(bp) = entry.iter_mut().find(|b| b.line == line) {
            bp.condition = condition;
        } else {
            entry.push(SourceBreakpoint {
                line,
                condition,
                hit_condition: None,
            });
        }
        self.resend_breakpoints_for(path);
        true
    }

    /// Set (or replace) the `hitCondition` on the breakpoint at
    /// `(path, line)`. Same semantics as `set_breakpoint_condition`.
    pub fn set_breakpoint_hit_condition(
        &mut self,
        path: &Path,
        line: usize,
        hit_condition: Option<String>,
    ) -> bool {
        let entry = self.breakpoints.entry(path.to_path_buf()).or_default();
        if let Some(bp) = entry.iter_mut().find(|b| b.line == line) {
            bp.hit_condition = hit_condition;
        } else {
            entry.push(SourceBreakpoint {
                line,
                condition: None,
                hit_condition,
            });
        }
        self.resend_breakpoints_for(path);
        true
    }

    /// Strip both `condition` and `hitCondition` from the breakpoint
    /// at `(path, line)`. Returns `true` if a breakpoint existed at
    /// that site (whether or not it had conditions to strip), `false`
    /// when the caller asked to "plain-ify" a line without one — that
    /// way the dispatcher can show a sensible status message instead
    /// of silently no-op'ing. Pushes the adapter update.
    pub fn strip_breakpoint_conditions(&mut self, path: &Path, line: usize) -> bool {
        let entry = match self.breakpoints.get_mut(path) {
            Some(e) => e,
            None => return false,
        };
        let bp = match entry.iter_mut().find(|b| b.line == line) {
            Some(b) => b,
            None => return false,
        };
        bp.condition = None;
        bp.hit_condition = None;
        self.resend_breakpoints_for(path);
        true
    }

    /// Begin starting a debug session. If the adapter declares a prelaunch
    /// (`dotnet build`, `cargo build`, …), the command is spawned with
    /// piped stdout/stderr and stashed in `pending_build`; reader threads
    /// stream each line into channels and `drain` promotes them into the
    /// Console tab live. The real adapter spawn + `initialize` only fires
    /// once the build child exits successfully — see
    /// `start_session_post_build`. Adapters without a prelaunch (delve,
    /// debugpy) go straight to that path.
    ///
    /// Returns `Ok(())` as soon as the build child has spawned (or as soon
    /// as the synchronous spawn path completes for no-prelaunch adapters).
    /// Build failures surface asynchronously as a `DapEvent::AdapterError`
    /// after the child exits non-zero.
    pub fn start_session(
        &mut self,
        adapter: DapAdapterSpec,
        ctx: super::specs::LaunchContext,
    ) -> Result<(), String> {
        if self.is_active() {
            return Err("debug session already active — :dapstop first".into());
        }

        if let Some(pre) = (adapter.prelaunch)(&ctx) {
            // Spawn the build asynchronously so the editor stays responsive
            // while it runs. `dotnet build` on a real solution can sit on
            // the CPU for several seconds; the synchronous variant of this
            // froze the editor and gave the user zero feedback.
            use std::io::{BufRead, BufReader};
            use std::process::{Command, Stdio};
            use std::sync::mpsc;
            let mut child = Command::new(&pre.program)
                .args(&pre.args)
                .current_dir(&ctx.root)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("{} failed to start: {}", pre.program, e))?;
            let (stdout_tx, stdout_rx) = mpsc::channel::<String>();
            let (stderr_tx, stderr_rx) = mpsc::channel::<String>();
            if let Some(stdout) = child.stdout.take() {
                std::thread::spawn(move || {
                    let r = BufReader::new(stdout);
                    for line in r.lines().map_while(|r| r.ok()) {
                        if stdout_tx.send(line).is_err() {
                            break;
                        }
                    }
                });
            }
            if let Some(stderr) = child.stderr.take() {
                std::thread::spawn(move || {
                    let r = BufReader::new(stderr);
                    for line in r.lines().map_while(|r| r.ok()) {
                        if stderr_tx.send(line).is_err() {
                            break;
                        }
                    }
                });
            }
            // Header line so the Console tab shows *something* the
            // moment the pane opens, before the build child has even
            // printed its first line. The leading `›` glyph + plain
            // category match the "session-internal status" look used
            // elsewhere (verified-breakpoint summaries etc.) rather
            // than masquerading as stdout from the build itself.
            let header = format!("› {} ({} {})", pre.label, pre.program, pre.args.join(" "));
            self.push_output_line(OutputLine {
                category: "console".into(),
                output: header,
            });
            self.pending_build = Some(PendingBuild {
                adapter,
                ctx,
                child,
                stdout_rx,
                stderr_rx,
                label: pre.label,
                started_at: std::time::Instant::now(),
            });
            return Ok(());
        }

        // No prelaunch — spawn the adapter and send `initialize` directly.
        self.start_session_post_build(adapter, ctx)
    }

    /// Spawn the adapter process and send `initialize`. Called either
    /// directly from `start_session` (no-prelaunch adapters) or from
    /// `drain` once the prelaunch build has exited successfully. Splitting
    /// the launch-args resolution out into this stage matters for .NET:
    /// `dotnet_launch_args` scans `bin/Debug/net*/` for the freshly built
    /// dll, so it can't run until the build has actually produced one.
    fn start_session_post_build(
        &mut self,
        adapter: DapAdapterSpec,
        ctx: super::specs::LaunchContext,
    ) -> Result<(), String> {
        let root = ctx.root.clone();

        // Resolve launch args before spawning the adapter — if the dll
        // can't be found we avoid leaking an orphan netcoredbg process.
        let launch_args = (adapter.build_launch_args)(&ctx)?;

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
                    // Each adapter declares its own well-known id — netcoredbg
                    // keys behaviour off `"coreclr"`, debugpy off `"debugpy"`,
                    // lldb-dap off `"lldb-dap"`, delve off `"go"`.
                    "adapterID": adapter.adapter_id,
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
            scope_for_display: None,
            children: HashMap::new(),
            expanded: HashSet::new(),
            pending_variable_fetches: HashMap::new(),
            pending_watch_evals: HashMap::new(),
            client,
            launch_args,
            request_command: "launch",
            status_line: "initialising adapter…".into(),
        });
        Ok(())
    }

    /// Start a debug session by attaching to an adapter already listening on a
    /// TCP port — the jdtls-hosted java-debug shape for Android. There's no
    /// process to spawn or prelaunch to run (the caller in `app/dap_glue.rs`
    /// has already built + installed the app, forwarded the JDWP port via adb,
    /// and obtained both the DAP port and the attach arguments). The handshake
    /// is otherwise identical: `initialize`, then — on its response —
    /// `attach` rather than `launch`.
    pub fn start_attach_session(
        &mut self,
        adapter_key: &'static str,
        adapter_id: &str,
        dap_port: u16,
        workspace_root: PathBuf,
        attach_args: Value,
    ) -> Result<(), String> {
        if self.is_active() {
            return Err("debug session already active — :dapstop first".into());
        }
        let addr = SocketAddr::from(([127, 0, 0, 1], dap_port));
        let client = DapClient::connect_tcp(adapter_key, addr)
            .ok_or_else(|| format!("could not connect to {adapter_key} adapter on :{dap_port}"))?;

        let init_seq = client.alloc_seq();
        client
            .send_request(
                init_seq,
                "initialize",
                json!({
                    "clientID": "binvim",
                    "clientName": "binvim",
                    "adapterID": adapter_id,
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
            adapter_key: adapter_key.to_string(),
            workspace_root,
            state: SessionState::Initializing,
            frames: Vec::new(),
            current_thread: None,
            scopes: Vec::new(),
            scope_for_display: None,
            children: HashMap::new(),
            expanded: HashSet::new(),
            pending_variable_fetches: HashMap::new(),
            pending_watch_evals: HashMap::new(),
            client,
            launch_args: attach_args,
            request_command: "attach",
            status_line: "attaching to debuggee…".into(),
        });
        Ok(())
    }

    /// Politely ask the adapter to disconnect; clear local session state.
    /// Best-effort — we drop the client immediately so any in-flight reply
    /// gets discarded.
    pub fn stop_session(&mut self) {
        // Kill an in-flight prelaunch first — `is_active()` treats a
        // pending build as an active session, so the caller's "stop
        // before restart" path winds up here even when no DAP session
        // proper has come up yet.
        self.cancel_pending_build();
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

    /// Stop the active session and poll the adapter's child until it
    /// reaps, up to `max_wait`. Used by the auto-restart path in
    /// `<leader>ds` so the previous `dotnet` debuggee has actually
    /// released its listening port before a new netcoredbg spawns and
    /// tries to bind the same port. Returns whether the child exited
    /// within the budget — caller can fall through either way; this is
    /// purely best-effort.
    pub fn stop_session_blocking(&mut self, max_wait: std::time::Duration) -> bool {
        // Cancel any in-flight prelaunch first so the freshly-spawned
        // adapter doesn't race with a still-running build child holding
        // file locks on the project's output dirs.
        self.cancel_pending_build();
        // Pull the client out before clearing the session so we can keep
        // polling it after `self.session = None`. The DAP protocol layer
        // is done with it at this point — only the OS-level child handle
        // is still useful.
        let client = self.session.take().map(|s| s.client);
        if let Some(client) = client {
            let seq = client.alloc_seq();
            let _ = client.send_request(
                seq,
                "disconnect",
                json!({
                    "restart": false,
                    "terminateDebuggee": true,
                }),
            );
            let start = std::time::Instant::now();
            while start.elapsed() < max_wait {
                if client.try_exit_status().is_some() {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            // Didn't exit in time — adapter is hung. Drop the client to
            // close its stdin (some adapters take that as a hint to
            // exit) and let it become a zombie on the user's machine
            // rather than blocking the restart.
            return false;
        }
        true
    }

    /// One step / continue command targeted at the currently-stopped
    /// thread. Silently does nothing if the session isn't in a stopped
    /// state — the calling key/command handler decides whether to warn.
    /// Flip whether `vref` is expanded in the pane's locals tree. Returns
    /// the new state. When expanding for the first time, kicks off a DAP
    /// `variables` request so the children populate asynchronously.
    pub fn toggle_expanded(&mut self, vref: u64) -> bool {
        let now_expanded = match self.session.as_mut() {
            Some(s) if s.expanded.contains(&vref) => {
                s.expanded.remove(&vref);
                false
            }
            Some(s) => {
                s.expanded.insert(vref);
                true
            }
            None => return false,
        };
        if now_expanded {
            // Only fetch if we haven't cached this set of children yet.
            let need_fetch = self
                .session
                .as_ref()
                .map(|s| !s.children.contains_key(&vref))
                .unwrap_or(false);
            if need_fetch {
                self.request_variables(vref);
            }
        }
        now_expanded
    }

    /// Send a `variables` request and record the `seq → parent_vref`
    /// mapping so the response handler stores children under the right
    /// parent. Idempotency of in-flight requests is the caller's problem;
    /// double-firing just produces two responses that both write the same
    /// `children[vref]` entry.
    fn request_variables(&mut self, vref: u64) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let seq = session.client.alloc_seq();
        if session
            .client
            .send_request(seq, "variables", json!({ "variablesReference": vref }))
            .is_err()
        {
            return;
        }
        if let Some(s) = self.session.as_mut() {
            s.pending_variable_fetches.insert(seq, vref);
        }
    }

    /// Append `expr` to the watch list. Re-evaluates on the next
    /// stop. Skips duplicates so repeatedly `:dapwatch foo`'ing
    /// the same expression doesn't multiply the row count.
    pub fn add_watch(&mut self, expr: String) -> bool {
        if expr.trim().is_empty() {
            return false;
        }
        if self.watches.iter().any(|w| w.expr == expr) {
            return false;
        }
        self.watches.push(DapWatch { expr, result: None });
        // If we're currently stopped, fire eval right away so the
        // user sees the result without waiting for the next stop.
        self.evaluate_pending_watches();
        true
    }

    /// Drop the watch at `idx`. Returns the removed expression so
    /// the caller can echo "removed `foo`" — or None if the index
    /// was out of range.
    pub fn remove_watch(&mut self, idx: usize) -> Option<String> {
        if idx >= self.watches.len() {
            return None;
        }
        Some(self.watches.remove(idx).expr)
    }

    /// Fire `evaluate` for every watch whose result is currently
    /// missing. Called automatically when a stop's stackTrace
    /// response lands (frame_id becomes available) and on
    /// `add_watch` if the session is already stopped.
    fn evaluate_pending_watches(&mut self) {
        // Snapshot the frame_id + pending list while only-immutable-
        // borrowing self.session; release that borrow before each
        // mutable update into pending_watch_evals.
        let frame_id = match self.session.as_ref().and_then(|s| s.frames.first()) {
            Some(f) => f.id,
            None => return,
        };
        let pending: Vec<(usize, String)> = self
            .watches
            .iter()
            .enumerate()
            .filter_map(|(i, w)| {
                if w.result.is_none() {
                    Some((i, w.expr.clone()))
                } else {
                    None
                }
            })
            .collect();
        for (idx, expr) in pending {
            // Alloc + send under an immutable borrow (alloc_seq /
            // send_request only need &client). Then drop and take
            // a mutable borrow for the pending-map insert.
            let seq = match self.session.as_ref() {
                Some(session) => {
                    let seq = session.client.alloc_seq();
                    if session
                        .client
                        .send_request(
                            seq,
                            "evaluate",
                            json!({
                                "expression": expr,
                                "frameId": frame_id,
                                "context": "watch",
                            }),
                        )
                        .is_err()
                    {
                        continue;
                    }
                    seq
                }
                None => return,
            };
            if let Some(s) = self.session.as_mut() {
                s.pending_watch_evals.insert(seq, idx);
            }
        }
    }

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

    /// Drain whatever the prelaunch reader threads have queued, then
    /// check whether the build child has exited. On a successful exit
    /// we transition into the real adapter spawn via
    /// `start_session_post_build`; on a failed exit we emit an
    /// `AdapterError` (without `Terminated` — there's no session yet,
    /// and we want the debug pane to stay open so the user can read
    /// the build output that just streamed in). Returns `true` if
    /// anything happened that the main loop should redraw for.
    fn pump_pending_build(&mut self, events: &mut Vec<DapEvent>) -> bool {
        // Snapshot what the reader threads have sent so we can release
        // the `&mut self.pending_build` borrow before mutating
        // `self.output_buffer` through `push_output_line`.
        let mut stdout_batch: Vec<String> = Vec::new();
        let mut stderr_batch: Vec<String> = Vec::new();
        let mut exit_outcome: Option<Result<std::process::ExitStatus, std::io::Error>> = None;
        let mut elapsed_secs = 0.0f32;
        let mut label = String::new();
        if let Some(b) = self.pending_build.as_mut() {
            while let Ok(line) = b.stdout_rx.try_recv() {
                stdout_batch.push(line);
            }
            while let Ok(line) = b.stderr_rx.try_recv() {
                stderr_batch.push(line);
            }
            match b.child.try_wait() {
                Ok(Some(status)) => {
                    exit_outcome = Some(Ok(status));
                    elapsed_secs = b.started_at.elapsed().as_secs_f32();
                    label = b.label.clone();
                }
                Ok(None) => {}
                Err(e) => {
                    exit_outcome = Some(Err(e));
                    elapsed_secs = b.started_at.elapsed().as_secs_f32();
                    label = b.label.clone();
                }
            }
        }
        let mut progress = !stdout_batch.is_empty() || !stderr_batch.is_empty();
        for s in stdout_batch {
            let line = OutputLine {
                category: "stdout".into(),
                output: s,
            };
            self.push_output_line(line.clone());
            events.push(DapEvent::Output(line));
        }
        for s in stderr_batch {
            let line = OutputLine {
                category: "stderr".into(),
                output: s,
            };
            self.push_output_line(line.clone());
            events.push(DapEvent::Output(line));
        }

        let Some(outcome) = exit_outcome else {
            return progress;
        };
        progress = true;
        // Drop the build now that the child has been reaped. After the
        // take, `self.pending_build` is None — so `is_active()` correctly
        // becomes false if `start_session_post_build` fails below and
        // doesn't create a session.
        let build = self.pending_build.take().expect("checked above");
        match outcome {
            Ok(status) if status.success() => {
                let summary = format!("✓ {} succeeded in {:.1}s", label, elapsed_secs);
                self.push_output_line(OutputLine {
                    category: "console".into(),
                    output: summary,
                });
                if let Err(e) = self.start_session_post_build(build.adapter, build.ctx) {
                    let msg = format!("debug: {e}");
                    self.push_output_line(OutputLine {
                        category: "stderr".into(),
                        output: msg.clone(),
                    });
                    events.push(DapEvent::AdapterError(msg));
                }
            }
            Ok(status) => {
                let code = status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "?".into());
                let summary = format!("✗ {} failed (exit {}) in {:.1}s", label, code, elapsed_secs);
                self.push_output_line(OutputLine {
                    category: "stderr".into(),
                    output: summary.clone(),
                });
                events.push(DapEvent::AdapterError(summary));
            }
            Err(e) => {
                let summary = format!("✗ {} wait failed: {}", label, e);
                self.push_output_line(OutputLine {
                    category: "stderr".into(),
                    output: summary.clone(),
                });
                events.push(DapEvent::AdapterError(summary));
            }
        }
        progress
    }

    /// Pull all available messages off the reader-thread channel, run them
    /// through the protocol state machine, and return:
    ///
    /// - the editor-facing `DapEvent`s the main loop should react to, and
    /// - a `progress` bool that's `true` whenever *any* incoming message
    ///   was processed.
    ///
    /// Many protocol replies (stackTrace, scopes, variables) update visible
    /// session state without emitting a user-facing event — the renderer
    /// still needs to know they happened, otherwise frames + locals
    /// appear stale until the next keypress. The `progress` flag lets the
    /// main loop request a redraw on those silent state mutations.
    pub fn drain(&mut self) -> (Vec<DapEvent>, bool) {
        let mut events = Vec::new();
        // ------------------------------------------------------------------
        // Pending-build pump. Done first so the build's final output
        // lines flush into the pane on the same tick as the transition
        // into the real DAP session, rather than getting clobbered by
        // the initialize-launch chatter.
        // ------------------------------------------------------------------
        let build_progress = self.pump_pending_build(&mut events);

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
        let progress =
            build_progress || !msgs.is_empty() || !stderr_lines.is_empty() || exit_code.is_some();
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
        (events, progress)
    }

    fn process_incoming(&mut self, msg: DapIncoming, events: &mut Vec<DapEvent>) {
        match msg {
            DapIncoming::Response {
                request_seq,
                command,
                success,
                body,
                message,
            } => self.handle_response(request_seq, command, success, body, message, events),
            DapIncoming::Event { event, body } => self.handle_event(event, body, events),
            DapIncoming::Request { seq, command, .. } => {
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
        request_seq: u64,
        command: String,
        success: bool,
        body: Value,
        message: Option<String>,
        events: &mut Vec<DapEvent>,
    ) {
        if !success {
            // Evaluate failures are normal — typos, names not in
            // scope at the current frame, side-effects refused.
            // Surface them on the watch row instead of as a top-line
            // AdapterError that would clobber the status line.
            if command == "evaluate" {
                if let Some(s) = self.session.as_mut() {
                    if let Some(idx) = s.pending_watch_evals.remove(&request_seq) {
                        if let Some(w) = self.watches.get_mut(idx) {
                            w.result = Some(DapWatchResult {
                                value: message.unwrap_or_else(|| "(no message)".into()),
                                type_name: None,
                                variables_reference: 0,
                                error: true,
                            });
                        }
                        return;
                    }
                }
            }
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
                    session.status_line = if session.request_command == "attach" {
                        "attaching to debuggee…".into()
                    } else {
                        "launching debuggee…".into()
                    };
                }
                if let Some(session) = self.session.as_ref() {
                    let seq = session.client.alloc_seq();
                    let _ = session.client.send_request(
                        seq,
                        session.request_command,
                        session.launch_args.clone(),
                    );
                }
            }
            // `launch` (spawned adapters) and `attach` (TCP / JDWP) both move
            // the session into the configuration phase on a successful reply.
            "launch" | "attach" => {
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
            // netcoredbg reports per-breakpoint validation in the
            // response — surface unverified ones to the status line and
            // output pane so the user spots a missing-PDB / wrong-line
            // / "bind by pattern" misfire without a silent never-hits.
            "setBreakpoints" => {
                let mut unverified: Vec<(u64, String)> = Vec::new();
                if let Some(arr) = body.get("breakpoints").and_then(|v| v.as_array()) {
                    for bp in arr {
                        let verified = bp
                            .get("verified")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        if !verified {
                            let line = bp.get("line").and_then(|v| v.as_u64()).unwrap_or(0);
                            let reason = bp
                                .get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("not bound (often: line is not an executable statement — try inside the handler body)")
                                .to_string();
                            unverified.push((line, reason));
                        }
                    }
                }
                if !unverified.is_empty() {
                    let lines = unverified
                        .iter()
                        .map(|(l, r)| format!("line {l}: {r}"))
                        .collect::<Vec<_>>()
                        .join("; ");
                    let summary =
                        format!("{} breakpoint(s) unverified — {}", unverified.len(), lines);
                    if let Some(s) = self.session.as_mut() {
                        s.status_line = summary.clone();
                    }
                    let line = OutputLine {
                        category: "console".into(),
                        output: summary,
                    };
                    self.output_buffer.push(line.clone());
                    if self.output_buffer.len() > OUTPUT_LOG_CAP {
                        let excess = self.output_buffer.len() - OUTPUT_LOG_CAP;
                        self.output_buffer.drain(0..excess);
                    }
                    events.push(DapEvent::Output(line));
                }
            }
            "stackTrace" => {
                let frames = parse_stack_frames(&body);
                let top_id = frames.first().map(|f| f.id);
                if frames.is_empty() {
                    // Some adapters require `threads` first before
                    // they'll fill out stackTrace; some return empty
                    // when called with a stale thread id. Surface
                    // either case so the user sees something in the
                    // pane instead of an indefinite "(no frames)".
                    let total = body
                        .get("totalFrames")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let line = OutputLine {
                        category: "console".into(),
                        output: format!(
                            "stackTrace returned 0 frames (totalFrames={}) — adapter may need a threads request first",
                            total
                        ),
                    };
                    self.output_buffer.push(line.clone());
                    if self.output_buffer.len() > OUTPUT_LOG_CAP {
                        let excess = self.output_buffer.len() - OUTPUT_LOG_CAP;
                        self.output_buffer.drain(0..excess);
                    }
                    events.push(DapEvent::Output(line));
                }
                if let Some(s) = self.session.as_mut() {
                    s.frames = frames;
                }
                // Auto-chain into scopes for the top frame so the pane can
                // show locals without an extra command from the user.
                if let (Some(id), Some(session)) = (top_id, self.session.as_ref()) {
                    let seq = session.client.alloc_seq();
                    let _ = session
                        .client
                        .send_request(seq, "scopes", json!({ "frameId": id }));
                }
                // Now that frames are populated and we have a
                // valid top frame_id, fire watch evaluations.
                // Skipped on empty-stackTrace (no frame to anchor
                // against — the user would see stale results).
                if top_id.is_some() {
                    self.evaluate_pending_watches();
                }
            }
            "scopes" => {
                let scopes = parse_scopes(&body);
                // Pick the first non-expensive scope (typically "Locals"
                // for stack-based languages, "Arguments + Locals" for C#).
                let target = scopes
                    .iter()
                    .find(|s| !s.expensive)
                    .map(|s| s.variables_reference);
                if let Some(s) = self.session.as_mut() {
                    s.scopes = scopes;
                    s.scope_for_display = target;
                    s.children.clear();
                    s.expanded.clear();
                    s.pending_variable_fetches.clear();
                }
                if let Some(vref) = target {
                    self.request_variables(vref);
                }
            }
            "variables" => {
                let vars = parse_variables(&body);
                if let Some(s) = self.session.as_mut() {
                    if let Some(parent_vref) = s.pending_variable_fetches.remove(&request_seq) {
                        s.children.insert(parent_vref, vars);
                    } else {
                        // No mapping — most likely a stale response from
                        // before the last stop. Discard quietly.
                    }
                }
            }
            "evaluate" => {
                // Watch expression result. Match the request_seq
                // against pending_watch_evals to find which watch
                // row this answers, then drop the value onto it.
                let idx = match self.session.as_mut() {
                    Some(s) => s.pending_watch_evals.remove(&request_seq),
                    None => None,
                };
                if let Some(idx) = idx {
                    let value = body
                        .get("result")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let type_name = body
                        .get("type")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let variables_reference = body
                        .get("variablesReference")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    if let Some(w) = self.watches.get_mut(idx) {
                        w.result = Some(DapWatchResult {
                            value,
                            type_name,
                            variables_reference,
                            error: false,
                        });
                    }
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
                let thread_id = body.get("threadId").and_then(|v| v.as_u64()).unwrap_or(0);
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
                    // `variables_reference` numbers aren't guaranteed
                    // stable across stops — drop the cached tree state
                    // before we re-fetch scopes for the new top frame.
                    s.scopes.clear();
                    s.scope_for_display = None;
                    s.children.clear();
                    s.expanded.clear();
                    s.pending_variable_fetches.clear();
                    s.pending_watch_evals.clear();
                }
                // Watch results from the previous frame don't
                // apply at the new stop — clear so the pane shows
                // "evaluating…" until the new responses arrive.
                for w in &mut self.watches {
                    w.result = None;
                }
                // Ask for the live thread list and the top frame's stack
                // back-to-back. netcoredbg in particular needs the
                // `threads` round-trip before it'll produce a populated
                // stackTrace for the just-stopped thread; without it,
                // the response comes back with `stackFrames: []` and the
                // pane stays empty even though execution paused.
                if let Some(session) = self.session.as_ref() {
                    let threads_seq = session.client.alloc_seq();
                    let _ = session
                        .client
                        .send_request(threads_seq, "threads", json!({}));
                    let stack_seq = session.client.alloc_seq();
                    let _ = session.client.send_request(
                        stack_seq,
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
                    s.scope_for_display = None;
                    s.children.clear();
                    s.expanded.clear();
                    s.pending_variable_fetches.clear();
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
                let thread_id = body.get("threadId").and_then(|v| v.as_u64()).unwrap_or(0);
                let reason = body
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                events.push(DapEvent::Thread { reason, thread_id });
            }
            // netcoredbg fires this when a previously-pending
            // breakpoint binds after a JIT, or when it rebinds an
            // existing one to a different line (common for lambdas:
            // line N → line N-3 of the enclosing call). Surface the
            // change so the user sees that the breakpoint actually
            // landed somewhere — and where.
            "breakpoint" => {
                let reason = body
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(bp) = body.get("breakpoint") {
                    let verified = bp
                        .get("verified")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let line = bp.get("line").and_then(|v| v.as_u64()).unwrap_or(0);
                    let msg = bp
                        .get("message")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let summary = match (reason.as_str(), verified, msg) {
                        ("changed", true, _) => format!("breakpoint bound at line {line}"),
                        ("changed", false, Some(m)) => format!("breakpoint line {line}: {m}"),
                        ("changed", false, None) => {
                            format!("breakpoint line {line}: still pending")
                        }
                        ("removed", _, _) => {
                            format!("breakpoint at line {line} removed by adapter")
                        }
                        (r, _, _) => format!("breakpoint event ({r}) at line {line}"),
                    };
                    if let Some(s) = self.session.as_mut() {
                        s.status_line = summary.clone();
                    }
                    let line = OutputLine {
                        category: "console".into(),
                        output: summary,
                    };
                    self.output_buffer.push(line.clone());
                    if self.output_buffer.len() > OUTPUT_LOG_CAP {
                        let excess = self.output_buffer.len() - OUTPUT_LOG_CAP;
                        self.output_buffer.drain(0..excess);
                    }
                    events.push(DapEvent::Output(line));
                }
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
            let bps_json: Vec<Value> = list.iter().map(encode_source_breakpoint).collect();
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
        // Use the same encoder as the configurationDone path so
        // condition + hitCondition aren't dropped on the post-toggle
        // resend. (Prior implementation built `{"line":N}` inline and
        // silently lost both fields — any conditional set before a
        // toggle reverted to a plain breakpoint on the next adapter
        // sync.)
        let bps_json: Vec<Value> = list.iter().map(encode_source_breakpoint).collect();
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

/// Build the DAP `SourceBreakpoint` JSON object for one breakpoint.
/// Honours `condition` and `hitCondition` when present so a toggle /
/// resend doesn't strip the user's conditional expression.
fn encode_source_breakpoint(b: &SourceBreakpoint) -> Value {
    let mut o = serde_json::Map::new();
    o.insert("line".into(), json!(b.line));
    if let Some(c) = &b.condition {
        o.insert("condition".into(), json!(c));
    }
    if let Some(h) = &b.hit_condition {
        o.insert("hitCondition".into(), json!(h));
    }
    Value::Object(o)
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
        let expensive = s
            .get("expensive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
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
        let type_name = v
            .get("type")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
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

/// One row in the flattened locals tree. The pane renderer prints these
/// with indentation; the pane-focus key handler indexes into the slice
/// by `App.dap_pane_cursor` to figure out which variable Enter/Tab
/// should toggle.
pub struct FlatLocalRow<'a> {
    pub depth: usize,
    pub var: &'a Variable,
    pub expandable: bool,
    pub expanded: bool,
}

/// Flatten the session's locals tree, honouring the current `expanded`
/// set. Returns an empty `Vec` whenever locals aren't available yet
/// (running state, no scope picked, response in flight, …).
pub fn flat_locals_view(session: &DapSession) -> Vec<FlatLocalRow<'_>> {
    let mut out = Vec::new();
    let Some(root_vref) = session.scope_for_display else {
        return out;
    };
    let Some(roots) = session.children.get(&root_vref) else {
        return out;
    };
    walk_locals(session, roots, 0, &mut out);
    out
}

fn walk_locals<'a>(
    session: &'a DapSession,
    vars: &'a [Variable],
    depth: usize,
    out: &mut Vec<FlatLocalRow<'a>>,
) {
    for v in vars {
        let expandable = v.variables_reference > 0;
        let expanded = expandable && session.expanded.contains(&v.variables_reference);
        out.push(FlatLocalRow {
            depth,
            var: v,
            expandable,
            expanded,
        });
        if expanded {
            if let Some(children) = session.children.get(&v.variables_reference) {
                walk_locals(session, children, depth + 1, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_breakpoint_adds_and_removes() {
        let mut m = DapManager::new();
        let p = std::env::temp_dir().join("x.cs");
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
        let a = std::env::temp_dir().join("a.cs");
        let b = std::env::temp_dir().join("b.cs");
        m.toggle_breakpoint(&a, 5);
        m.toggle_breakpoint(&b, 5);
        assert!(m.has_breakpoint(&a, 5));
        assert!(m.has_breakpoint(&b, 5));
        assert_eq!(m.breakpoints.len(), 2);
    }

    #[test]
    fn set_breakpoint_condition_creates_when_absent() {
        let mut m = DapManager::new();
        let p = std::env::temp_dir().join("x.cs");
        assert!(m.breakpoint_at(&p, 7).is_none());
        m.set_breakpoint_condition(&p, 7, Some("i == 3".into()));
        let bp = m.breakpoint_at(&p, 7).unwrap();
        assert_eq!(bp.condition.as_deref(), Some("i == 3"));
        assert!(bp.hit_condition.is_none());
        assert!(bp.is_conditional());
    }

    #[test]
    fn set_breakpoint_condition_replaces_existing() {
        let mut m = DapManager::new();
        let p = std::env::temp_dir().join("x.cs");
        m.toggle_breakpoint(&p, 12); // plain
        m.set_breakpoint_condition(&p, 12, Some("len > 0".into()));
        let bp = m.breakpoint_at(&p, 12).unwrap();
        assert_eq!(bp.condition.as_deref(), Some("len > 0"));
        // Replace with a different expression.
        m.set_breakpoint_condition(&p, 12, Some("len > 5".into()));
        assert_eq!(
            m.breakpoint_at(&p, 12).unwrap().condition.as_deref(),
            Some("len > 5")
        );
        // None clears the condition (but keeps the breakpoint).
        m.set_breakpoint_condition(&p, 12, None);
        let bp = m.breakpoint_at(&p, 12).unwrap();
        assert!(bp.condition.is_none());
        assert!(!bp.is_conditional());
        assert!(m.has_breakpoint(&p, 12));
    }

    #[test]
    fn hit_condition_independent_of_condition() {
        let mut m = DapManager::new();
        let p = std::env::temp_dir().join("x.cs");
        m.set_breakpoint_condition(&p, 3, Some("x > 0".into()));
        m.set_breakpoint_hit_condition(&p, 3, Some("5".into()));
        let bp = m.breakpoint_at(&p, 3).unwrap();
        assert_eq!(bp.condition.as_deref(), Some("x > 0"));
        assert_eq!(bp.hit_condition.as_deref(), Some("5"));
        assert!(bp.is_conditional());
    }

    #[test]
    fn strip_breakpoint_conditions_keeps_breakpoint() {
        let mut m = DapManager::new();
        let p = std::env::temp_dir().join("x.cs");
        m.set_breakpoint_condition(&p, 9, Some("y == 1".into()));
        m.set_breakpoint_hit_condition(&p, 9, Some("10".into()));
        assert!(m.strip_breakpoint_conditions(&p, 9));
        let bp = m.breakpoint_at(&p, 9).unwrap();
        assert!(bp.condition.is_none());
        assert!(bp.hit_condition.is_none());
        assert!(!bp.is_conditional());
        assert!(m.has_breakpoint(&p, 9));
    }

    #[test]
    fn strip_breakpoint_conditions_returns_false_for_missing() {
        let mut m = DapManager::new();
        let p = std::env::temp_dir().join("x.cs");
        assert!(!m.strip_breakpoint_conditions(&p, 1));
    }

    #[test]
    fn encode_source_breakpoint_includes_both_fields() {
        let bp = SourceBreakpoint {
            line: 42,
            condition: Some("k != 0".into()),
            hit_condition: Some(">= 3".into()),
        };
        let v = encode_source_breakpoint(&bp);
        assert_eq!(v["line"], json!(42));
        assert_eq!(v["condition"], json!("k != 0"));
        assert_eq!(v["hitCondition"], json!(">= 3"));
    }

    #[test]
    fn encode_source_breakpoint_omits_empty_fields() {
        let bp = SourceBreakpoint {
            line: 1,
            condition: None,
            hit_condition: None,
        };
        let v = encode_source_breakpoint(&bp);
        assert!(v.get("condition").is_none());
        assert!(v.get("hitCondition").is_none());
    }

    #[test]
    fn idle_manager_is_inactive_and_drains_empty() {
        let mut m = DapManager::new();
        assert!(!m.is_active());
        let (events, progress) = m.drain();
        assert!(events.is_empty());
        assert!(!progress);
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

    #[test]
    fn add_watch_appends_and_dedups() {
        let mut m = DapManager::new();
        assert!(m.add_watch("foo".into()));
        assert!(m.add_watch("bar".into()));
        // Duplicate expr → refused (returns false, no duplicate row).
        assert!(!m.add_watch("foo".into()));
        assert_eq!(m.watches.len(), 2);
        assert_eq!(m.watches[0].expr, "foo");
        assert_eq!(m.watches[1].expr, "bar");
    }

    #[test]
    fn add_watch_rejects_empty_and_whitespace() {
        let mut m = DapManager::new();
        assert!(!m.add_watch("".into()));
        assert!(!m.add_watch("   ".into()));
        assert!(m.watches.is_empty());
    }

    #[test]
    fn remove_watch_returns_expr_and_shifts_indices() {
        let mut m = DapManager::new();
        m.add_watch("a".into());
        m.add_watch("b".into());
        m.add_watch("c".into());
        assert_eq!(m.remove_watch(1).as_deref(), Some("b"));
        assert_eq!(m.watches.len(), 2);
        assert_eq!(m.watches[0].expr, "a");
        assert_eq!(m.watches[1].expr, "c");
        // Out-of-range → None.
        assert!(m.remove_watch(99).is_none());
    }
}
