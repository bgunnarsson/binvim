//! Wire-side types shared across the LSP submodules and the URI helpers.
//! Anything app.rs needs is re-exported from `crate::lsp`.

use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Diagnostic {
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Debug, Clone)]
pub struct DiagnosticsMessage {
    pub uri: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub enum LspIncoming {
    Diagnostics(DiagnosticsMessage),
    Response { id: u64, result: Value },
    /// Request that the editor needs to react to (e.g. goto-def jump, hover popup).
    #[allow(dead_code)]
    ErrorReply { id: u64, message: String },
    /// Server-to-client `workspace/applyEdit` — the main thread applies the
    /// edit and replies with `{ applied: true }` (or false on failure).
    ApplyEditRequest { id: u64, edit: Value },
    /// `window/showMessage` (popup-style notification) or
    /// `window/logMessage` (debug log entry). Both share the same shape
    /// — a severity + a string — and only differ in how the editor
    /// surfaces them. `is_show=true` flags showMessage (loud), `false`
    /// flags logMessage (quiet log entry).
    ServerMessage {
        severity: MessageSeverity,
        text: String,
        is_show: bool,
    },
}

/// LSP `MessageType` enum values: 1 = Error, 2 = Warning, 3 = Info,
/// 4 = Log. Anything else is normalised to `Info` on parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageSeverity {
    Error,
    Warning,
    Info,
    Log,
}

/// One decoded semantic token. Coordinates are 0-based; `length` is
/// the LSP `length` field in UTF-16 code units — same encoding the
/// server emitted, kept opaque here. The renderer translates against
/// the buffer's char count by treating each unit as one char (which
/// matches the spec for ASCII / most identifiers — multi-codeunit
/// emoji in identifiers is a rare edge we accept misalignment on).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SemanticToken {
    pub line: usize,
    pub start_col: usize,
    pub length: usize,
    /// Resolved name from the server's `legend.tokenTypes` — `function`,
    /// `keyword`, `variable`, … Used as a tree-sitter-style capture
    /// when looking up a colour in the config palette.
    pub token_type: String,
    /// Resolved modifier names from `legend.tokenModifiers`. Appended
    /// to `token_type` as dotted suffixes (`function.async`,
    /// `variable.readonly`) when resolving colours.
    pub modifiers: Vec<String>,
}

/// One `textDocument/documentHighlight` range. `kind` is the LSP
/// `DocumentHighlightKind` enum: 1 = Text (plain match), 2 = Read,
/// 3 = Write. The renderer applies a subtle bg colour the same across
/// all kinds today — the field is kept so a future pass can colour
/// reads vs writes differently.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DocumentHighlightRange {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub kind: u8,
}

/// One inlay hint. `line`/`col` are 0-indexed buffer coordinates where
/// the hint should appear; `label` is the displayable text. `kind` is
/// the LSP `InlayHintKind` enum (1 = Type, 2 = Parameter).
#[derive(Debug, Clone)]
pub struct InlayHint {
    pub line: usize,
    pub col: usize,
    pub label: String,
    /// LSP `InlayHintKind`: 1 = Type, 2 = Parameter. Parameter hints
    /// render in a slightly warmer tone than type hints so the two
    /// categories scan apart on a line that mixes both.
    pub kind: u8,
}

#[derive(Debug, Clone)]
pub enum LspEvent {
    GotoDef { path: PathBuf, line: usize, col: usize },
    Hover { text: String },
    Completion { items: Vec<CompletionItem> },
    SignatureHelp(SignatureHelp),
    References { items: Vec<LocationItem> },
    Symbols { items: Vec<SymbolItem>, workspace: bool },
    CodeActions { items: Vec<CodeActionItem> },
    /// `WorkspaceEdit` returned from `textDocument/rename`. The applier in
    /// app.rs consumes this directly via `apply_workspace_edit`.
    Rename { edit: Value },
    /// Server asked us to apply a `WorkspaceEdit`. App applies it then
    /// uses `LspManager::send_apply_edit_response` to ack the originating
    /// request.
    ApplyEditRequest {
        client_key: String,
        id: u64,
        edit: Value,
    },
    DiagnosticsUpdated,
    /// `textDocument/inlayHint` results for `path` — the App stores them
    /// per-buffer and the renderer pulls them on draw.
    InlayHints { path: PathBuf, hints: Vec<InlayHint> },
    /// `textDocument/documentHighlight` reply — every range matching
    /// the symbol the cursor was on when the request fired. Anchor
    /// (line/col/version) lets the App drop stale replies that arrived
    /// after the cursor moved off the symbol.
    DocumentHighlights {
        path: PathBuf,
        anchor_line: usize,
        anchor_col: usize,
        anchor_version: u64,
        ranges: Vec<DocumentHighlightRange>,
    },
    /// `textDocument/semanticTokens/full` reply — decoded against the
    /// server's legend into flat per-token records. `buffer_version`
    /// is the version we asked for; stale responses are dropped by
    /// the App when it compares against the live buffer version.
    SemanticTokens {
        path: PathBuf,
        buffer_version: u64,
        tokens: Vec<SemanticToken>,
    },
    /// Copilot `checkStatus` reply — used to drive `App.lsp.copilot_status`
    /// + the status-line indicator. `kind` is the raw protocol string
    /// (`"OK"`, `"NotSignedIn"`, `"NotAuthorized"`, `"NoTelemetryConsent"`,
    /// …); the App normalises it into a `CopilotStatus`.
    CopilotStatus { kind: String, user: Option<String> },
    /// Server emitted a `window/showMessage` or `window/logMessage`.
    /// `client_key` lets the app tag the message with which server it
    /// came from so the log isn't a mystery soup of unattributed lines.
    /// `is_show` distinguishes the loud showMessage (popup-style,
    /// usually surfaced in the status line) from the quiet logMessage
    /// (log-only, viewable via `:messages`).
    ServerMessage {
        client_key: String,
        severity: MessageSeverity,
        text: String,
        is_show: bool,
    },
    /// Copilot `inlineCompletion` reply — at most one suggestion text
    /// is surfaced as a ghost. `line`/`col` is the cursor position the
    /// request was anchored on; the App drops the ghost if the cursor
    /// has since moved off it. `replace_start_line`/`replace_start_col`
    /// is the (line, col) where the suggestion's `range.start` sits;
    /// `text` is the full replacement, which usually includes whatever
    /// the user has already typed between range.start and the cursor.
    /// On accept the buffer span `[replace_start .. cursor]` is wiped
    /// and `text` is inserted at `replace_start`, so the user's
    /// existing prefix isn't duplicated.
    CopilotInline {
        path: PathBuf,
        line: usize,
        col: usize,
        replace_start_line: usize,
        replace_start_col: usize,
        text: String,
        buffer_version: u64,
    },
    NotFound(&'static str),
    /// Server returned an error reply (not a successful result) for a
    /// tracked request — used by the App to free in-flight throttle
    /// slots so we don't deadlock waiting forever for a response that
    /// will never arrive. `kind` is the short label from
    /// `pending_request_kind`; `path` is the buffer the request was
    /// anchored to (None for kinds without a path, e.g. workspace
    /// symbols or Copilot status).
    RequestFailed {
        kind: &'static str,
        path: Option<PathBuf>,
    },
}

/// A code action the user can pick from `<leader>a`. We keep the raw
/// `command` and `edit` JSON values so the applier can match against
/// either shape (LSP returns `Command` or `CodeAction` interchangeably).
#[derive(Debug, Clone)]
pub struct CodeActionItem {
    pub title: String,
    pub kind: Option<String>,
    pub edit: Option<Value>,
    pub command: Option<Value>,
    /// Set when the action is published as `disabled` — we still surface it
    /// so the user can see why the server thinks it doesn't apply, but the
    /// apply path will reject it.
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SymbolItem {
    pub name: String,
    /// Container path for nested symbols, e.g. `App > render > draw`. Empty
    /// for top-level symbols.
    pub container: String,
    pub kind: String,
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
}

/// One result from a `textDocument/references` (or similar) call. `path`
/// is on disk, line/col are 0-indexed.
#[derive(Debug, Clone)]
pub struct LocationItem {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
}

/// Parsed `SignatureHelp` response. We render the active signature only —
/// most servers return one anyway, and overload menus are rarely useful in
/// a TUI.
#[derive(Debug, Clone)]
pub struct SignatureHelp {
    pub label: String,
    /// Char range in `label` covering the active parameter, if known.
    pub active_param: Option<(usize, usize)>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CompletionItem {
    pub label: String,
    pub insert_text: String,
    pub kind: Option<String>,
    pub detail: Option<String>,
    /// Server-supplied filter key. Falls back to `label` when absent.
    pub filter_text: String,
    /// Server-supplied sort key. Falls back to `label` when absent. Lets the
    /// LSP's relevance order survive client-side filtering (e.g. typescript's
    /// "0~document" sorts globals before locals when relevant).
    pub sort_text: String,
    /// LSP `insertTextFormat == 2` — `insert_text` is a TextMate snippet
    /// with `$N` / `${N:default}` placeholders that need to be parsed and
    /// resolved before insertion.
    pub is_snippet: bool,
    /// `textEdit.range` from the server, if any. When present this is the
    /// authoritative span to replace on accept — not the client-side
    /// word-prefix guess. Servers use this to e.g. cover a trailing `.`
    /// when inserting `?.method`, so we get `obj?.method` instead of
    /// `obj.?.method`. Stored as `(start_line, start_col, end_line, end_col)`.
    pub text_edit_range: Option<(usize, usize, usize, usize)>,
}

#[derive(Debug, Clone)]
pub struct LspHealth {
    pub key: String,
    pub language_id: String,
    pub root_uri: String,
    pub pending_requests: usize,
    /// Breakdown of pending requests by kind, sorted descending by
    /// count. Lets `:health` surface "8× SemanticTokens stuck" instead
    /// of the flat "8 pending" — load-bearing for diagnosing slow /
    /// hung servers.
    pub pending_breakdown: Vec<(String, usize)>,
}

#[derive(Debug, Clone)]
pub struct ActiveBufferLspStatus {
    pub key: String,
    pub language_id: String,
    /// Resolved path on disk (from `$PATH` or absolute) — `None` means no
    /// candidate command exists on the system.
    pub resolved_binary: Option<String>,
    pub running: bool,
}

pub fn path_to_uri(path: &Path) -> String {
    let abs = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf());
    let s = abs.to_string_lossy().to_string();
    if s.starts_with('/') {
        format!("file://{}", s)
    } else {
        format!("file:///{}", s.replace('\\', "/"))
    }
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let stripped = uri.strip_prefix("file://")?;
    Some(PathBuf::from(stripped))
}
