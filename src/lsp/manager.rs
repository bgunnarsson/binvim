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
    /// Copilot `checkStatus` — response says whether the user is signed
    /// in and surfaces their handle.
    CopilotCheckStatus,
    /// Copilot `signIn` — kicks off the device-flow auth. Response
    /// carries a verification URI + user code to display.
    CopilotSignIn,
    /// Copilot `textDocument/inlineCompletion`. Carries the anchor
    /// position + buffer version so a late reply can be dropped if the
    /// user has since edited or moved.
    CopilotInline {
        path: PathBuf,
        line: usize,
        col: usize,
        buffer_version: u64,
    },
}

/// Container for per-language LSP clients keyed by `ServerSpec.key`.
pub struct LspManager {
    clients: HashMap<String, LspClient>,
    pub diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
    /// Each client allocates IDs from its own counter, so the global key is
    /// `(client_key, id)` rather than just `id` to avoid cross-server clashes.
    pending: HashMap<(String, u64), PendingRequest>,
    /// Mirror of `Config.copilot.enabled` — when true, the Copilot
    /// language server is attached as an aux LSP to every buffer.
    pub copilot_enabled: bool,
    /// Sign-in state for the Copilot LSP. `NotStarted` until the server
    /// responds to `checkStatus`; `SignedIn` once the user's account is
    /// usable; `Pending { url, code }` while a device-flow auth is in
    /// progress and the user needs to visit GitHub.
    #[allow(dead_code)]
    pub copilot_status: CopilotStatus,
}

/// Sign-in state for the Copilot LSP. Surfaced in the status line +
/// `:health` so the user knows whether suggestions are actually
/// going to come back. Variants beyond `NotStarted` are populated by
/// the sign-in / inline-completion follow-up pass; the foundation
/// commit just adds the type + plumbing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(dead_code)]
pub enum CopilotStatus {
    #[default]
    NotStarted,
    /// `checkStatus` in flight, waiting for response.
    Checking,
    SignedIn {
        user: String,
    },
    SignedOut,
    /// Device-flow auth pending: the user must visit `verification_uri`
    /// and enter `user_code`.
    PendingAuth {
        verification_uri: String,
        user_code: String,
    },
    /// Hard error from the server (not installed, crashed, etc.).
    Error(String),
}

impl LspManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            diagnostics: HashMap::new(),
            pending: HashMap::new(),
            copilot_enabled: false,
            copilot_status: CopilotStatus::NotStarted,
        }
    }

    /// Wraps the path-only `specs_for_path` and conditionally adds the
    /// Copilot spec when the user has opted in. Every call site that
    /// used to call `specs_for_path` directly should route through this
    /// instead so the Copilot toggle is honoured consistently.
    fn specs_for(&self, path: &Path) -> Vec<super::specs::ServerSpec> {
        let mut specs = specs_for_path(path);
        if self.copilot_enabled {
            if let Some(spec) = super::specs::copilot_spec_for_path(path) {
                specs.push(spec);
            }
        }
        specs
    }

    /// Spawn every spec that applies to `path` (primary + auxiliary) and
    /// return whether at least one client ended up running. Previously
    /// returned the primary client specifically — that produced a None
    /// result whenever the primary's binary wasn't installed, which
    /// then short-circuited `lsp_sync_active` and prevented `didOpen`
    /// from going out to the auxiliaries either. Now reports success if
    /// *any* matching client is alive, so emmet-ls / Tailwind / etc.
    /// still get the document even when the primary binary is missing.
    pub fn ensure_for_path(&mut self, path: &Path, fallback_root: &Path) -> bool {
        let specs = self.specs_for(path);
        if specs.is_empty() {
            return false;
        }
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
                let key = spec.key.clone();
                self.clients.insert(key.clone(), client);
                // First time Copilot's client comes up, kick a checkStatus
                // request so we know whether to show "signed in" / "not
                // signed in" in the status surface. The response routes
                // back through `handle_response` and emits LspEvent::
                // CopilotStatus.
                if key == "copilot" {
                    self.request_copilot_check_status();
                    self.copilot_status = CopilotStatus::Checking;
                }
            }
        }
        specs.iter().any(|s| self.clients.contains_key(&s.key))
    }

    /// Send Copilot's custom `checkStatus` request. Optional `options`
    /// payload is omitted (server defaults are fine for us).
    pub fn request_copilot_check_status(&mut self) -> bool {
        let Some(client) = self.clients.get("copilot") else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(id, "checkStatus", json!({}));
        self.pending
            .insert(("copilot".into(), id), PendingRequest::CopilotCheckStatus);
        true
    }

    /// Send Copilot's custom `signIn` request to kick off the device-flow
    /// auth. The response carries `verificationUri` + `userCode` which we
    /// display in the status line.
    pub fn request_copilot_sign_in(&mut self) -> bool {
        let Some(client) = self.clients.get("copilot") else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(id, "signIn", json!({}));
        self.pending
            .insert(("copilot".into(), id), PendingRequest::CopilotSignIn);
        true
    }

    /// Send `textDocument/inlineCompletion` (LSP 3.18) to the Copilot
    /// client. Returns `false` when Copilot isn't attached / signed in —
    /// the caller (idle-pause path in `app/lsp_glue.rs`) uses this to
    /// avoid wasted bookkeeping.
    pub fn request_copilot_inline(
        &mut self,
        path: &Path,
        line: usize,
        col: usize,
        buffer_version: u64,
    ) -> bool {
        if !matches!(self.copilot_status, CopilotStatus::SignedIn { .. }) {
            return false;
        }
        let Some(client) = self.clients.get("copilot") else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/inlineCompletion",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": col },
                "context": { "triggerKind": 2 },
            }),
        );
        self.pending.insert(
            ("copilot".into(), id),
            PendingRequest::CopilotInline {
                path: path.to_path_buf(),
                line,
                col,
                buffer_version,
            },
        );
        true
    }

    /// What `:health` should say about the active buffer's LSP attachments.
    /// Walks every spec that *would* apply to the path and reports whether
    /// the binary resolves on PATH and whether the client is currently
    /// running. Lets the user see "Tailwind matched but binary missing"
    /// without having to grep their PATH manually.
    pub fn active_buffer_status(&self, path: &Path) -> Vec<ActiveBufferLspStatus> {
        self.specs_for(path)
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
        self.specs_for(path)
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
                            // OmniSharp's classic Razor mode doesn't run the
                            // Razor source generator, so every `.cshtml` /
                            // `.razor` file parses as raw C# and emits a
                            // cascade of bogus errors ("name does not exist",
                            // "Unexpected character '@'", …). Drop its
                            // diagnostics for those extensions; completions
                            // and hover still flow through.
                            if !suppress_diagnostics_from(client_key, &path) {
                                self.diagnostics.insert(path, d.diagnostics);
                                diagnostics_changed = true;
                            }
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
        for spec in self.specs_for(path) {
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

/// Diagnostics from specific (server, file-ext) pairs that we know are
/// unreliable enough to be net-negative. Currently just OmniSharp + Razor
/// — its legacy `OmniSharp.Razor` extension is abandoned and the bundled
/// `--languageserver` build typically lacks the Razor source generator,
/// so every directive becomes a spurious error.
fn suppress_diagnostics_from(client_key: &str, path: &Path) -> bool {
    if client_key != "omnisharp" {
        return false;
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    matches!(ext.as_deref(), Some("cshtml") | Some("razor"))
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
        PendingRequest::CopilotCheckStatus => {
            // Copilot returns `{ status: "OK" | "NotSignedIn" | ...,
            // user: "..." }`. Older protocol versions used `"signedIn":
            // bool` instead — we accept both shapes.
            let kind = result
                .get("status")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    result
                        .get("signedIn")
                        .and_then(|v| v.as_bool())
                        .map(|b| if b { "OK".to_string() } else { "NotSignedIn".to_string() })
                })
                .unwrap_or_else(|| "Unknown".to_string());
            let user = result
                .get("user")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            Some(LspEvent::CopilotStatus { kind, user })
        }
        PendingRequest::CopilotSignIn => {
            // signIn reply is `{ verificationUri, userCode, status }` for
            // device flow, or `{ status: "AlreadySignedIn" }` if the
            // user is already authenticated.
            let kind = result
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("PromptUserDeviceFlow")
                .to_string();
            let user = result
                .get("user")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            // We piggyback the device-flow fields onto LspEvent::CopilotStatus
            // by embedding them in `kind` — the App parses them out. This
            // keeps the LSP event surface tight.
            let event_kind = if let (Some(uri), Some(code)) = (
                result.get("verificationUri").and_then(|v| v.as_str()),
                result.get("userCode").and_then(|v| v.as_str()),
            ) {
                format!("PendingAuth:{uri}:{code}")
            } else {
                kind
            };
            Some(LspEvent::CopilotStatus { kind: event_kind, user })
        }
        PendingRequest::CopilotInline {
            path,
            line,
            col,
            buffer_version,
        } => {
            // Response shape: { items: [{ insertText, range, command? }] }
            // — we surface the first item's text.
            let text = result
                .get("items")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|item| item.get("insertText"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default();
            if text.is_empty() {
                return Some(LspEvent::NotFound("copilot inline"));
            }
            Some(LspEvent::CopilotInline {
                path,
                line,
                col,
                text,
                buffer_version,
            })
        }
    }
}
