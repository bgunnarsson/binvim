//! Minimal LSP client. First cut handles diagnostics only:
//! spawn server, send initialize/initialized/didOpen/didChange, push diagnostics
//! notifications to a channel that the main loop drains.
//!
//! Sub-module map:
//! - [`types`]: wire-side data types and URI helpers
//! - [`specs`]: per-extension server dispatch and workspace discovery
//! - [`client`]: spawn one server, send/receive frames over its stdio
//! - [`io`]: reader-thread loop and JSON-RPC dispatcher
//! - [`manager`]: fan-out across clients, route responses to `LspEvent`s
//! - [`parse`]: pure response parsers

mod client;
mod io;
mod manager;
mod parse;
mod specs;
mod types;

pub use manager::{CopilotStatus, LspManager};
pub use specs::{find_node_modules_bin, find_tailwind_config};
pub use types::{
    ActiveBufferLspStatus, CodeActionItem, CompletionItem, Diagnostic, DocumentHighlightRange,
    InlayHint, LocationItem, LspEvent, LspHealth, MessageSeverity, SemanticToken, Severity,
    SignatureHelp, SymbolItem, uri_to_path,
};
