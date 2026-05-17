//! Wire-side and editor-side data types for the DAP layer.
//!
//! `DapIncoming` is what the reader thread emits — the raw shape of one
//! parsed adapter message. `DapEvent` is what the manager turns those into
//! after the protocol semantics are resolved (response ↔ original request
//! matched, lifecycle events promoted, etc.). The main loop only sees
//! `DapEvent`s.

use serde_json::Value;
use std::path::PathBuf;

/// One message just lifted off the adapter's stdout. Differs from LSP's
/// shape — DAP's request/response/event split is explicit in the `type`
/// field, and responses carry a `success` bool plus the original command
/// name.
#[derive(Debug)]
#[allow(dead_code)]
pub enum DapIncoming {
    /// Server-emitted event (e.g. `stopped`, `output`, `terminated`).
    Event { event: String, body: Value },
    /// Reply to a request we sent.
    Response {
        request_seq: u64,
        command: String,
        success: bool,
        body: Value,
        /// Adapter-supplied error message — only meaningful when `!success`.
        message: Option<String>,
    },
    /// Server-to-client request — DAP allows these for `runInTerminal` and
    /// similar. The main thread handles + replies, mirroring the LSP
    /// `workspace/applyEdit` pattern.
    Request {
        seq: u64,
        command: String,
        arguments: Value,
    },
}

/// Editor-facing debug events. The manager translates raw `DapIncoming`s
/// into these after binding responses to their originating requests.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum DapEvent {
    /// Adapter answered `initialize` — capabilities are now known.
    Initialized,
    /// Program paused. `reason` is the DAP reason string ("breakpoint",
    /// "step", "exception", "pause", "entry", …).
    Stopped {
        thread_id: u64,
        reason: String,
        description: Option<String>,
        hit_breakpoint_ids: Vec<u64>,
    },
    /// Program resumed.
    Continued {
        thread_id: Option<u64>,
        all_threads: bool,
    },
    /// Console output emitted by the debuggee (or the adapter).
    Output(OutputLine),
    /// Thread lifecycle. `reason` is "started" or "exited".
    Thread { reason: String, thread_id: u64 },
    /// One breakpoint's verified state changed (set / cleared / moved
    /// after symbol load).
    Breakpoint { reason: String, breakpoint: Breakpoint },
    /// Debuggee exited normally.
    Exited { exit_code: i64 },
    /// Session torn down (adapter exit, user-initiated, or error).
    Terminated,
    /// Adapter spoke an error we can't recover from — surface to the user.
    AdapterError(String),
}

/// One line in the debug console pane. Categories come straight from DAP:
/// "console", "stdout", "stderr", "telemetry", "important". The pane
/// renders different categories with different colours.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct OutputLine {
    pub category: String,
    pub output: String,
}

/// A source-line breakpoint as the user set it in the editor. Independent
/// of any active session — the manager keeps a per-path table and resends
/// `setBreakpoints` to the adapter whenever a session starts or this list
/// changes.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SourceBreakpoint {
    pub line: usize,
    /// User-supplied conditional expression. Phase-2 territory — none of
    /// the keybindings expose this yet.
    pub condition: Option<String>,
}

/// Breakpoint state as reported back by the adapter — verified flag,
/// resolved location after symbol load, optional adapter id.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Breakpoint {
    /// Adapter-assigned id, present once the adapter has acknowledged it.
    pub id: Option<u64>,
    pub verified: bool,
    /// Resolved path — may differ from where the user set it if the
    /// adapter mapped through a symbol search.
    pub source: Option<PathBuf>,
    /// Resolved line, 1-based as DAP defines it.
    pub line: Option<usize>,
    /// Adapter explanation for unverified breakpoints ("no executable code
    /// at this line", "module not loaded yet", …).
    pub message: Option<String>,
}

/// One frame of the stopped thread's call stack.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct StackFrame {
    pub id: u64,
    pub name: String,
    pub source: Option<PathBuf>,
    /// 1-based per DAP convention.
    pub line: usize,
    pub column: usize,
}

/// A variable scope ("Locals", "Globals", "Arguments", …) within a frame.
/// The `variables_reference` is the DAP handle for the lazy fetch.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Scope {
    pub name: String,
    pub variables_reference: u64,
    pub expensive: bool,
}

/// A single named value inside a scope or another structured variable.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub type_name: Option<String>,
    /// Non-zero when the value has children that can be expanded.
    pub variables_reference: u64,
}

/// A user-managed watch expression — typed once via `:dapwatch
/// <expr>`, evaluated against the top stack frame on every `stopped`
/// event, displayed above locals in the debug pane. Survives across
/// debug sessions (the manager keeps the list; only the evaluated
/// `result` clears between sessions). Adapter-agnostic — DAP's
/// `evaluate` request is supported by every adapter we ship.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DapWatch {
    pub expr: String,
    /// Most recent evaluation. None while a request is in flight or
    /// before the first evaluation has fired. Cleared on session
    /// start and on every `stopped` (the manager re-fires).
    pub result: Option<DapWatchResult>,
}

/// One evaluation of a `DapWatch.expr` against a stack frame.
/// Mirrors the shape of the DAP `evaluate` response.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DapWatchResult {
    pub value: String,
    pub type_name: Option<String>,
    /// Non-zero when the result is structured and could be expanded.
    /// Not yet driven by the pane — present so a future
    /// "press Enter on a watch to expand its children" can land
    /// without re-plumbing the wire layer.
    pub variables_reference: u64,
    /// True when the server returned an error (`response.success ==
    /// false`) or refused the expression (typed wrong, name not in
    /// scope at the current frame, …). `value` holds the error
    /// message in that case.
    pub error: bool,
}

/// The high-level state machine for an active session. Transitions are
/// driven by responses + events from the adapter; the renderer reads this
/// to decide which placeholder to paint.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum SessionState {
    /// `initialize` sent, awaiting capabilities response.
    Initializing,
    /// Capabilities back; `launch` sent (or about to be), awaiting the
    /// `initialized` event before pushing breakpoints.
    Configuring,
    /// `configurationDone` sent and acknowledged — debuggee is or will
    /// shortly be running.
    Running,
    /// Adapter reported a `stopped` event. The frame + scope state on the
    /// manager applies to `thread_id`.
    Stopped { thread_id: u64, reason: String },
    /// Adapter or debuggee terminated. Session can be cleared.
    Terminated,
}
