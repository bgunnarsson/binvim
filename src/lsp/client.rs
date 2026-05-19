//! `LspClient` — one spawned language server. Handles the spawn, the
//! `initialize` handshake (asynchronously, via the reader thread), and the
//! frame-writing primitives. Doesn't know about specific request semantics —
//! that lives in `manager.rs`.

use anyhow::Result;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use super::io::reader_loop;
use super::specs::{ServerSpec, resolve_command};
use super::types::{LspIncoming, MessageSeverity, path_to_uri};

/// State of a client's outgoing pipe. Until the server has answered the
/// `initialize` request we buffer frames; the reader thread flushes them in
/// order once it sees the response.
pub(super) enum InitState {
    Buffering(Vec<Vec<u8>>),
    Ready,
}

pub struct LspClient {
    #[allow(dead_code)]
    pub name: String,
    _child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pub incoming_rx: Receiver<LspIncoming>,
    next_id: Arc<Mutex<u64>>,
    init_state: Arc<Mutex<InitState>>,
    #[allow(dead_code)]
    pub root_uri: String,
    pub language_id: String,
    /// Server's semantic-tokens `legend`, captured from the
    /// `initialize` response. `None` until the reader thread has seen
    /// the response (or if the server doesn't advertise the
    /// capability). Shared with the reader thread via Arc<Mutex<…>>;
    /// the manager reads it lazily before firing
    /// `textDocument/semanticTokens/full`.
    pub semantic_tokens_legend: Arc<Mutex<Option<SemanticTokensLegend>>>,
    /// `serverCapabilities.codeLensProvider` flag, captured from the
    /// `initialize` response. The manager gates
    /// `textDocument/codeLens` on this so we don't burn a request
    /// against a server that doesn't advertise the capability.
    pub code_lens_provider: Arc<Mutex<bool>>,
    /// `serverCapabilities.codeLensProvider.resolveProvider` flag.
    /// True when the server expects the client to follow up the
    /// initial `textDocument/codeLens` reply with a
    /// `codeLens/resolve` per item to fill in `command.title`.
    /// csharp-ls / OmniSharp work this way; rust-analyzer inlines
    /// titles in the first reply and leaves this `false`.
    pub code_lens_resolve_provider: Arc<Mutex<bool>>,
    /// Workspace folders this client currently has attached. Starts
    /// with the root used for `initialize`; subsequent files opened
    /// from a sibling project root append (and the manager fires
    /// `workspace/didChangeWorkspaceFolders` if the server supports
    /// it). Single-root use leaves this at one entry, unchanged from
    /// pre-multi-root behaviour.
    pub workspace_folders: Arc<Mutex<Vec<PathBuf>>>,
    /// `serverCapabilities.workspace.workspaceFolders.supported`,
    /// captured from the `initialize` response. When `false` the
    /// manager skips the didChangeWorkspaceFolders dance — there's
    /// no point telling a server about a folder it can't model.
    /// rust-analyzer / tsserver / gopls all support this; some
    /// niche servers don't.
    pub workspace_folders_supported: Arc<Mutex<bool>>,
}

/// Decoded `semanticTokensProvider.legend` from the server's
/// initialize response. `token_types[idx]` and `token_modifiers[idx]`
/// map the integers in the response stream back to capability names
/// the editor can colour with.
#[derive(Debug, Clone, Default)]
pub struct SemanticTokensLegend {
    pub token_types: Vec<String>,
    pub token_modifiers: Vec<String>,
}

impl LspClient {
    /// Spawn an LSP server given a [`ServerSpec`] and a workspace root.
    /// Returns `None` if no candidate command resolves or spawning fails.
    pub fn spawn_spec(spec: &ServerSpec, root: &Path) -> Option<Self> {
        let (cmd_path, _) = resolve_command(&spec.cmd_candidates)?;
        let mut command = Command::new(&cmd_path);
        for arg in &spec.args {
            command.arg(arg);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;
        let stdin = Arc::new(Mutex::new(child.stdin.take()?));
        let stdout = child.stdout.take()?;
        let stderr = child.stderr.take();

        let (in_tx, in_rx) = channel();
        let init_state = Arc::new(Mutex::new(InitState::Buffering(Vec::new())));
        let init_state_for_reader = init_state.clone();
        let stdin_for_reader = stdin.clone();
        let semantic_tokens_legend: Arc<Mutex<Option<SemanticTokensLegend>>> =
            Arc::new(Mutex::new(None));
        let legend_for_reader = semantic_tokens_legend.clone();
        let code_lens_provider: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
        let code_lens_for_reader = code_lens_provider.clone();
        let code_lens_resolve_provider: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
        let code_lens_resolve_for_reader = code_lens_resolve_provider.clone();
        let workspace_folders_supported: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
        let folders_supported_for_reader = workspace_folders_supported.clone();
        let in_tx_for_reader = in_tx.clone();
        thread::spawn(move || {
            reader_loop(
                stdout,
                stdin_for_reader,
                init_state_for_reader,
                legend_for_reader,
                code_lens_for_reader,
                code_lens_resolve_for_reader,
                folders_supported_for_reader,
                in_tx_for_reader,
            );
        });
        // Forward the server's stderr into the same channel as a
        // synthetic logMessage so the user can see crash traces /
        // panic backtraces / capability errors via `:messages`
        // instead of having them disappear into the void.
        if let Some(stderr) = stderr {
            let tx = in_tx.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    let Ok(text) = line else { break };
                    if text.trim().is_empty() {
                        continue;
                    }
                    let _ = tx.send(LspIncoming::ServerMessage {
                        severity: MessageSeverity::Log,
                        text,
                        is_show: false,
                    });
                }
            });
        }

        let root_uri = path_to_uri(root);
        let client = Self {
            name: spec.key.clone(),
            _child: child,
            stdin,
            incoming_rx: in_rx,
            next_id: Arc::new(Mutex::new(1)),
            init_state,
            root_uri: root_uri.clone(),
            language_id: spec.language_id.clone(),
            semantic_tokens_legend,
            code_lens_provider,
            code_lens_resolve_provider,
            workspace_folders: Arc::new(Mutex::new(vec![root.to_path_buf()])),
            workspace_folders_supported,
        };

        // Send initialize directly (bypassing the queue gate, which only holds
        // back later messages). Initialized + queued frames are flushed by the
        // reader thread once the response arrives — we don't block here.
        let init_id = client.alloc_id();
        let _ = client.send_request_direct(
            init_id,
            "initialize",
            json!({
                "processId": std::process::id(),
                "clientInfo": { "name": "binvim", "version": env!("CARGO_PKG_VERSION") },
                "rootUri": root_uri,
                "rootPath": root.to_string_lossy(),
                "workspaceFolders": [{ "uri": root_uri, "name": root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "root".into()) }],
                "initializationOptions": spec.initialization_options,
                "capabilities": {
                    "general": {
                        "positionEncodings": ["utf-8", "utf-16"]
                    },
                    "textDocument": {
                        "synchronization": {
                            "dynamicRegistration": false,
                            "didSave": true
                        },
                        "publishDiagnostics": {
                            "relatedInformation": false,
                            "versionSupport": false,
                            "tagSupport": { "valueSet": [1, 2] }
                        },
                        "hover": {
                            "dynamicRegistration": false,
                            "contentFormat": ["markdown", "plaintext"]
                        },
                        "definition": {
                            "dynamicRegistration": false,
                            "linkSupport": true
                        },
                        "references": { "dynamicRegistration": false },
                        "documentHighlight": { "dynamicRegistration": false },
                        "documentSymbol": { "dynamicRegistration": false },
                        "rename": {
                            "dynamicRegistration": false,
                            "prepareSupport": false
                        },
                        "completion": {
                            "dynamicRegistration": false,
                            "completionItem": {
                                "snippetSupport": true,
                                "documentationFormat": ["markdown", "plaintext"],
                                "deprecatedSupport": true,
                                "preselectSupport": false,
                                "insertReplaceSupport": false,
                                "resolveSupport": { "properties": ["documentation", "detail"] }
                            },
                            "completionItemKind": {
                                "valueSet": (1..=25).collect::<Vec<_>>()
                            },
                            "contextSupport": true
                        },
                        "signatureHelp": {
                            "dynamicRegistration": false,
                            "signatureInformation": {
                                "documentationFormat": ["markdown", "plaintext"],
                                "parameterInformation": { "labelOffsetSupport": true },
                                "activeParameterSupport": true
                            },
                            "contextSupport": true
                        },
                        "codeAction": {
                            "dynamicRegistration": false,
                            "codeActionLiteralSupport": {
                                "codeActionKind": {
                                    "valueSet": [
                                        "", "quickfix", "refactor",
                                        "refactor.extract", "refactor.inline", "refactor.rewrite",
                                        "source", "source.organizeImports"
                                    ]
                                }
                            }
                        },
                        "formatting": { "dynamicRegistration": false },
                        "codeLens": { "dynamicRegistration": false },
                        "semanticTokens": {
                            "dynamicRegistration": false,
                            "requests": {
                                "range": false,
                                "full": { "delta": false }
                            },
                            "tokenTypes": [
                                "namespace", "type", "class", "enum", "interface",
                                "struct", "typeParameter", "parameter", "variable",
                                "property", "enumMember", "event", "function",
                                "method", "macro", "keyword", "modifier", "comment",
                                "string", "number", "regexp", "operator", "decorator"
                            ],
                            "tokenModifiers": [
                                "declaration", "definition", "readonly", "static",
                                "deprecated", "abstract", "async", "modification",
                                "documentation", "defaultLibrary"
                            ],
                            "formats": ["relative"],
                            "overlappingTokenSupport": false,
                            "multilineTokenSupport": false
                        }
                    },
                    "workspace": {
                        "applyEdit": true,
                        "workspaceEdit": { "documentChanges": false },
                        "configuration": true,
                        "didChangeConfiguration": { "dynamicRegistration": false },
                        "workspaceFolders": true
                    },
                    // rust-analyzer (and a handful of other servers)
                    // only emit their client-side-command lenses when
                    // the client tells them which commands it can
                    // execute. Without this advertisement the
                    // `textDocument/codeLens` response comes back
                    // empty even though `codeLensProvider` is
                    // advertised — the server has nothing the editor
                    // is willing to invoke. The names here mirror
                    // what rust-analyzer's `client_commands_options`
                    // looks for; we intercept `runSingle` /
                    // `debugSingle` client-side in
                    // `app/lsp_glue.rs::invoke_lens_command`, the
                    // others fall back to `workspace/executeCommand`.
                    "experimental": {
                        "commands": {
                            "commands": [
                                "rust-analyzer.runSingle",
                                "rust-analyzer.debugSingle",
                                "rust-analyzer.showReferences",
                                "rust-analyzer.gotoLocation",
                                "rust-analyzer.triggerParameterHints",
                                "rust-analyzer.rename"
                            ]
                        }
                    }
                }
            }),
        );
        // No blocking wait — reader thread handles "initialized" + queue flush when
        // the response comes back. The user can keep editing in the meantime.
        Some(client)
    }

    /// Attach a new workspace folder to a live client. Returns `true`
    /// if the folder was actually added (the manager surfaces this so
    /// it can fire `workspace/didChangeWorkspaceFolders` only when
    /// something changed), `false` if it was already attached.
    /// Caller is responsible for honoring the supported-cap — this
    /// helper just maintains the local set.
    pub fn add_workspace_folder(&self, folder: &Path) -> bool {
        let folder = folder.to_path_buf();
        let mut g = self.workspace_folders.lock().unwrap();
        if g.iter().any(|f| f == &folder) {
            return false;
        }
        g.push(folder);
        true
    }

    /// Snapshot the attached folders. Cheap clone — typical sessions
    /// stay at 1-2 folders; even an aggressive monorepo session
    /// rarely climbs past 10.
    pub fn workspace_folders_snapshot(&self) -> Vec<PathBuf> {
        self.workspace_folders.lock().unwrap().clone()
    }

    pub fn supports_workspace_folders(&self) -> bool {
        *self.workspace_folders_supported.lock().unwrap()
    }

    pub fn alloc_id(&self) -> u64 {
        let mut g = self.next_id.lock().unwrap();
        let id = *g;
        *g += 1;
        id
    }

    /// True once the server has answered `initialize` and the reader
    /// thread has promoted the queue to Ready. Stays false when the
    /// "server" turns out to be a non-functional wrapper that exits
    /// without sending a response (a common failure mode with the
    /// rustup proxy when the toolchain doesn't have a real
    /// rust-analyzer component installed) — in that case the LSP
    /// looks "running" but requests pile up in the init queue
    /// forever and never go on the wire.
    pub fn is_initialized(&self) -> bool {
        matches!(*self.init_state.lock().unwrap(), InitState::Ready)
    }

    /// Number of frames waiting in the init buffer to be flushed
    /// once `initialize` resolves. Anything > 0 alongside
    /// `is_initialized() == false` after a few seconds means the
    /// server is wedged at startup.
    pub fn queued_init_frames(&self) -> usize {
        match &*self.init_state.lock().unwrap() {
            InitState::Buffering(q) => q.len(),
            InitState::Ready => 0,
        }
    }

    /// Write a frame straight to stdin. Used by `send_request_direct` for the
    /// initialize request and by the reader thread when flushing the queue.
    fn write_frame_unconditional(&self, frame: &[u8]) -> std::io::Result<()> {
        let mut stdin = self.stdin.lock().unwrap();
        stdin.write_all(frame)?;
        stdin.flush()
    }

    /// Public send path — buffers if init isn't done; otherwise writes directly.
    fn send_raw(&self, msg: &Value) -> Result<()> {
        let body = serde_json::to_string(msg)?;
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes();
        let mut g = self.init_state.lock().unwrap();
        match &mut *g {
            InitState::Ready => {
                drop(g);
                self.write_frame_unconditional(&frame)?;
            }
            InitState::Buffering(q) => {
                q.push(frame);
            }
        }
        Ok(())
    }

    /// Send a request without going through the init gate. Reserved for the
    /// initialize request itself (it must be the first thing on the wire).
    fn send_request_direct(&self, id: u64, method: &str, params: Value) -> Result<()> {
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let body = serde_json::to_string(&msg)?;
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        self.write_frame_unconditional(frame.as_bytes())?;
        Ok(())
    }

    pub fn send_request(&self, id: u64, method: &str, params: Value) -> Result<()> {
        self.send_raw(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
    }

    pub fn send_notification(&self, method: &str, params: Value) -> Result<()> {
        self.send_raw(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    /// Reply to a server-initiated request the client received earlier
    /// (e.g. `workspace/applyEdit`).
    pub fn send_response(&self, id: u64, result: Value) -> Result<()> {
        self.send_raw(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
    }

    /// `textDocument/didOpen`. The languageId is passed explicitly per
    /// call rather than read off the client because a single client
    /// instance often hosts multiple file types (the `ts` server keys
    /// `.ts` and `.tsx`, the `omnisharp` server keys `.cs`, `.razor`,
    /// `.cshtml`, …). Using `self.language_id` would lock every
    /// follow-up file to whichever spec the client first spawned for —
    /// which is how `.tsx` files end up parsed as plain TS and the
    /// server complains about every `<…>`.
    pub fn did_open(&self, path: &Path, text: &str, language_id: &str) -> Result<()> {
        self.send_notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": path_to_uri(path),
                    "languageId": language_id,
                    "version": 1,
                    "text": text,
                }
            }),
        )
    }

    pub fn did_change(&self, path: &Path, version: u64, text: &str) -> Result<()> {
        self.send_notification(
            "textDocument/didChange",
            json!({
                "textDocument": {
                    "uri": path_to_uri(path),
                    "version": version,
                },
                "contentChanges": [{ "text": text }],
            }),
        )
    }
}
