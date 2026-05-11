//! `LspManager` — owns one client per `ServerSpec.key`, fans messages out
//! across the clients attached to a path, and routes responses back to the
//! main thread as `LspEvent`s.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::client::LspClient;
use super::parse::{
    parse_code_actions_response, parse_completion_response, parse_def_response,
    parse_hover_response, parse_locations_response, parse_signature_help_response,
    parse_symbols_response,
};
use super::specs::{find_workspace_root, resolve_command, specs_for_path};
use super::types::{
    path_to_uri, uri_to_path, ActiveBufferLspStatus, Diagnostic, LspEvent, LspHealth, LspIncoming,
};

#[derive(Debug, Clone)]
pub(super) enum PendingRequest {
    GotoDef,
    Hover,
    Completion,
    SignatureHelp,
    References,
    DocumentSymbols,
    WorkspaceSymbols,
    CodeActions,
    Rename,
    /// Carries the requesting path so the response — which the LSP spec
    /// returns without echoing the URI — can be routed back to the right
    /// buffer in the editor.
    InlayHints { path: PathBuf },
}

/// Container for per-language LSP clients keyed by `ServerSpec.key`.
pub struct LspManager {
    clients: HashMap<String, LspClient>,
    pub diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
    /// Each client allocates IDs from its own counter, so the global key is
    /// `(client_key, id)` rather than just `id` to avoid cross-server clashes.
    pending: HashMap<(String, u64), PendingRequest>,
}

impl LspManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            diagnostics: HashMap::new(),
            pending: HashMap::new(),
        }
    }

    /// Spawn every spec that applies to `path` (primary + auxiliary) and
    /// return the primary client. The primary is the first entry from
    /// `specs_for_path`; auxiliaries (like Tailwind) are kept inside the
    /// manager so they receive didOpen/didChange and contribute to
    /// completions, but they don't take over hover/goto-def.
    pub fn ensure_for_path(&mut self, path: &Path, fallback_root: &Path) -> Option<&LspClient> {
        let specs = specs_for_path(path);
        let primary_key = specs.first().map(|s| s.key.clone())?;
        for spec in &specs {
            if self.clients.contains_key(&spec.key) {
                continue;
            }
            let start = path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| fallback_root.to_path_buf());
            let root = find_workspace_root(&start, &spec.root_markers);
            if let Some(client) = LspClient::spawn_spec(spec, &root) {
                self.clients.insert(spec.key.clone(), client);
            }
        }
        self.clients.get(&primary_key)
    }

    /// What `:health` should say about the active buffer's LSP attachments.
    /// Walks every spec that *would* apply to the path and reports whether
    /// the binary resolves on PATH and whether the client is currently
    /// running. Lets the user see "Tailwind matched but binary missing"
    /// without having to grep their PATH manually.
    pub fn active_buffer_status(&self, path: &Path) -> Vec<ActiveBufferLspStatus> {
        specs_for_path(path)
            .into_iter()
            .map(|spec| ActiveBufferLspStatus {
                resolved_binary: resolve_command(&spec.cmd_candidates).map(|(p, _)| p),
                running: self.clients.contains_key(&spec.key),
                key: spec.key,
                language_id: spec.language_id,
            })
            .collect()
    }

    /// Snapshot of every running LSP client for the `:health` view. Sorted by
    /// key so the report order is stable across calls.
    pub fn health_summary(&self) -> Vec<LspHealth> {
        let mut out: Vec<LspHealth> = self
            .clients
            .iter()
            .map(|(key, client)| {
                let pending = self
                    .pending
                    .keys()
                    .filter(|(k, _)| k == key)
                    .count();
                LspHealth {
                    key: key.clone(),
                    language_id: client.language_id.clone(),
                    root_uri: client.root_uri.clone(),
                    pending_requests: pending,
                }
            })
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
    }

    /// All running clients that match the path's spec list, primary first.
    /// Used to fan out didOpen/didChange and completion requests across the
    /// primary server and any attached auxiliaries.
    pub fn clients_for_path(&self, path: &Path) -> Vec<&LspClient> {
        specs_for_path(path)
            .into_iter()
            .filter_map(|spec| self.clients.get(&spec.key))
            .collect()
    }

    pub fn client_for_path(&self, path: &Path) -> Option<&LspClient> {
        self.clients_for_path(path).into_iter().next()
    }

    /// Drain pending LSP messages, bounded per call. Returns `(events, more)`
    /// where `more` is true if any client still has unread messages — the main
    /// loop uses this to know whether to keep polling for input or come back
    /// for another drain pass. Without the bound, OmniSharp's initial
    /// diagnostics flood (hundreds of files re-published in a burst) starves
    /// the event poll for tens of seconds — fine for slow keyboard input but
    /// painfully visible for mouse clicks.
    pub fn drain(&mut self) -> (Vec<LspEvent>, bool) {
        const MAX_PER_CALL: usize = 64;
        let mut events = Vec::new();
        let mut diagnostics_changed = false;
        let mut processed = 0usize;
        let mut more = false;
        for (client_key, client) in self.clients.iter() {
            while processed < MAX_PER_CALL {
                let Ok(msg) = client.incoming_rx.try_recv() else {
                    break;
                };
                processed += 1;
                match msg {
                    LspIncoming::Diagnostics(d) => {
                        if let Some(path) = uri_to_path(&d.uri) {
                            self.diagnostics.insert(path, d.diagnostics);
                            diagnostics_changed = true;
                        }
                    }
                    LspIncoming::Response { id, result } => {
                        if let Some(req) = self.pending.remove(&(client_key.clone(), id)) {
                            if let Some(ev) = handle_response(req, &result) {
                                events.push(ev);
                            }
                        }
                    }
                    LspIncoming::ErrorReply { id, .. } => {
                        self.pending.remove(&(client_key.clone(), id));
                    }
                    LspIncoming::ApplyEditRequest { id, edit } => {
                        events.push(LspEvent::ApplyEditRequest {
                            client_key: client_key.clone(),
                            id,
                            edit,
                        });
                    }
                }
            }
            // If we hit the per-call cap, peek the rest of the clients to know
            // whether to flag `more` without actually processing them this turn.
            if processed >= MAX_PER_CALL {
                more = true;
                break;
            }
        }
        if diagnostics_changed {
            events.push(LspEvent::DiagnosticsUpdated);
        }
        (events, more)
    }

    pub fn diagnostics_for(&self, path: &Path) -> Option<&Vec<Diagnostic>> {
        if let Some(d) = self.diagnostics.get(path) {
            return Some(d);
        }
        let canon = path.canonicalize().ok()?;
        self.diagnostics.get(&canon)
    }

    pub fn did_change_all(&self, path: &Path, version: u64, text: &str) {
        for client in self.clients.values() {
            let _ = client.did_change(path, version, text);
        }
    }

    /// Send `textDocument/didOpen` to every running client that should
    /// attach to `path`, using each spec's languageId. Looking up the
    /// spec per call (instead of the client's stored `language_id`) is
    /// what lets one shared `typescript-language-server` instance serve
    /// both `.ts` (`typescript`) and `.tsx` (`typescriptreact`)
    /// correctly.
    pub fn did_open_all(&self, path: &Path, text: &str) {
        for spec in specs_for_path(path) {
            if let Some(client) = self.clients.get(&spec.key) {
                let _ = client.did_open(path, text, &spec.language_id);
            }
        }
    }

    /// Reply to a server's `workspace/applyEdit` request after the main
    /// thread has applied (or failed to apply) the edit.
    pub fn send_apply_edit_response(&self, client_key: &str, id: u64, applied: bool) {
        if let Some(client) = self.clients.get(client_key) {
            let _ = client.send_response(id, json!({ "applied": applied }));
        }
    }

    /// Fire `workspace/executeCommand` against the path's primary server.
    /// `command_obj` must be the LSP `Command` shape (`{ title, command,
    /// arguments? }`). The server's response is fire-and-forget — most
    /// servers respond null and instead push their effect through a
    /// follow-up `workspace/applyEdit` request.
    pub fn execute_command(&mut self, path: &Path, command_obj: &Value) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let cmd = command_obj
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if cmd.is_empty() {
            return false;
        }
        let mut params = json!({ "command": cmd });
        if let Some(args) = command_obj.get("arguments").cloned() {
            params["arguments"] = args;
        }
        let _ = client.send_request(id, "workspace/executeCommand", params);
        // No PendingRequest variant — we don't surface the response, the
        // server delivers the effect via follow-up applyEdit requests.
        true
    }

    pub fn request_definition(&mut self, path: &Path, line: usize, col: usize) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/definition",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": col }
            }),
        );
        self.pending.insert((client.name.clone(), id), PendingRequest::GotoDef);
        true
    }

    pub fn request_hover(&mut self, path: &Path, line: usize, col: usize) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/hover",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": col }
            }),
        );
        self.pending.insert((client.name.clone(), id), PendingRequest::Hover);
        true
    }

    /// Fan out a completion request to every server attached to this path.
    /// Each server's reply arrives as its own `LspEvent::Completion`; the
    /// caller is responsible for merging them into the in-flight popup.
    pub fn request_completion(
        &mut self,
        path: &Path,
        line: usize,
        col: usize,
        trigger_char: Option<char>,
    ) -> bool {
        // LSP CompletionTriggerKind: 1=Invoked, 2=TriggerCharacter.
        // Servers use this to decide whether to return member-access
        // completions (after `.`, `:`, etc.) versus general scope items.
        let context = match trigger_char {
            Some(c) => json!({ "triggerKind": 2, "triggerCharacter": c.to_string() }),
            None => json!({ "triggerKind": 1 }),
        };
        let mut sent = Vec::new();
        for client in self.clients_for_path(path) {
            let id = client.alloc_id();
            let _ = client.send_request(
                id,
                "textDocument/completion",
                json!({
                    "textDocument": { "uri": path_to_uri(path) },
                    "position": { "line": line, "character": col },
                    "context": context,
                }),
            );
            sent.push((client.name.clone(), id));
        }
        let any = !sent.is_empty();
        for k in sent {
            self.pending.insert(k, PendingRequest::Completion);
        }
        any
    }

    /// Request `textDocument/rename` with the user's chosen new name.
    pub fn request_rename(
        &mut self,
        path: &Path,
        line: usize,
        col: usize,
        new_name: &str,
    ) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/rename",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": col },
                "newName": new_name,
            }),
        );
        self.pending
            .insert((client.name.clone(), id), PendingRequest::Rename);
        true
    }

    /// Request `textDocument/codeAction` for the cursor position. The
    /// caller passes the diagnostics overlapping that position (only those
    /// — passing the full file's worth made tsserver hang on big projects).
    pub fn request_code_actions(
        &mut self,
        path: &Path,
        line: usize,
        col: usize,
        diagnostics: Vec<Value>,
    ) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "range": {
                    "start": { "line": line, "character": col },
                    "end":   { "line": line, "character": col },
                },
                "context": {
                    "diagnostics": diagnostics,
                    "triggerKind": 1,
                },
            }),
        );
        self.pending
            .insert((client.name.clone(), id), PendingRequest::CodeActions);
        true
    }

    /// Request `textDocument/documentSymbol` to populate the outline picker.
    pub fn request_document_symbols(&mut self, path: &Path) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": path_to_uri(path) } }),
        );
        self.pending
            .insert((client.name.clone(), id), PendingRequest::DocumentSymbols);
        true
    }

    /// Request `workspace/symbol`. The server-side fuzzy matcher does the
    /// ranking; we just relay results to the picker. `query` may be empty.
    pub fn request_workspace_symbols(&mut self, path: &Path, query: &str) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(id, "workspace/symbol", json!({ "query": query }));
        self.pending
            .insert((client.name.clone(), id), PendingRequest::WorkspaceSymbols);
        true
    }

    /// Request `textDocument/references` from the primary server with
    /// `includeDeclaration: true` so the user sees the definition site too.
    pub fn request_references(&mut self, path: &Path, line: usize, col: usize) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/references",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": col },
                "context": { "includeDeclaration": true },
            }),
        );
        self.pending
            .insert((client.name.clone(), id), PendingRequest::References);
        true
    }

    /// Request `textDocument/inlayHint` for a line range. `end_line` is
    /// exclusive (LSP `Range.end`). Skipped silently when the primary
    /// server is missing.
    pub fn request_inlay_hints(&mut self, path: &Path, end_line: usize) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/inlayHint",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end":   { "line": end_line, "character": 0 },
                }
            }),
        );
        self.pending.insert(
            (client.name.clone(), id),
            PendingRequest::InlayHints { path: path.to_path_buf() },
        );
        true
    }

    /// Request `textDocument/signatureHelp` from the primary server. Goes
    /// to one server only — multi-server fan-out wouldn't help here, the
    /// primary is the source of truth for the language's call syntax.
    pub fn request_signature_help(&mut self, path: &Path, line: usize, col: usize) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/signatureHelp",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": col }
            }),
        );
        self.pending
            .insert((client.name.clone(), id), PendingRequest::SignatureHelp);
        true
    }
}

fn handle_response(req: PendingRequest, result: &Value) -> Option<LspEvent> {
    match req {
        PendingRequest::GotoDef => match parse_def_response(result) {
            Some((path, line, col)) => Some(LspEvent::GotoDef { path, line, col }),
            None => Some(LspEvent::NotFound("definition")),
        },
        PendingRequest::Hover => match parse_hover_response(result) {
            Some(text) => Some(LspEvent::Hover { text }),
            None => Some(LspEvent::NotFound("hover")),
        },
        PendingRequest::Completion => {
            let items = parse_completion_response(result);
            if items.is_empty() {
                Some(LspEvent::NotFound("completions"))
            } else {
                Some(LspEvent::Completion { items })
            }
        }
        PendingRequest::SignatureHelp => match parse_signature_help_response(result) {
            Some(sig) => Some(LspEvent::SignatureHelp(sig)),
            None => Some(LspEvent::NotFound("signature")),
        },
        PendingRequest::References => {
            let items = parse_locations_response(result);
            if items.is_empty() {
                Some(LspEvent::NotFound("references"))
            } else {
                Some(LspEvent::References { items })
            }
        }
        PendingRequest::DocumentSymbols => {
            let items = parse_symbols_response(result);
            if items.is_empty() {
                Some(LspEvent::NotFound("symbols"))
            } else {
                Some(LspEvent::Symbols { items, workspace: false })
            }
        }
        PendingRequest::WorkspaceSymbols => {
            let items = parse_symbols_response(result);
            // Empty results during live filtering shouldn't toast — the
            // caller distinguishes by the `workspace: true` flag.
            Some(LspEvent::Symbols { items, workspace: true })
        }
        PendingRequest::CodeActions => {
            let items = parse_code_actions_response(result);
            if items.is_empty() {
                Some(LspEvent::NotFound("code actions"))
            } else {
                Some(LspEvent::CodeActions { items })
            }
        }
        PendingRequest::Rename => {
            if result.is_null() {
                Some(LspEvent::NotFound("rename target"))
            } else {
                Some(LspEvent::Rename { edit: result.clone() })
            }
        }
        PendingRequest::InlayHints { path } => {
            let hints = super::parse::parse_inlay_hints_response(result);
            Some(LspEvent::InlayHints { path, hints })
        }
    }
}
