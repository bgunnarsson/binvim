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
    NotFound(&'static str),
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
}

#[derive(Debug, Clone)]
pub struct LspHealth {
    pub key: String,
    pub language_id: String,
    pub root_uri: String,
    pub pending_requests: usize,
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
