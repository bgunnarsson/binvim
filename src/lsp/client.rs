//! `LspClient` — one spawned language server. Handles the spawn, the
//! `initialize` handshake (asynchronously, via the reader thread), and the
//! frame-writing primitives. Doesn't know about specific request semantics —
//! that lives in `manager.rs`.

use anyhow::Result;
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

use super::io::reader_loop;
use super::specs::{resolve_command, ServerSpec};
use super::types::{path_to_uri, LspIncoming};

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
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdin = Arc::new(Mutex::new(child.stdin.take()?));
        let stdout = child.stdout.take()?;

        let (in_tx, in_rx) = channel();
        let init_state = Arc::new(Mutex::new(InitState::Buffering(Vec::new())));
        let init_state_for_reader = init_state.clone();
        let stdin_for_reader = stdin.clone();
        thread::spawn(move || {
            reader_loop(stdout, stdin_for_reader, init_state_for_reader, in_tx);
        });

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
                        "formatting": { "dynamicRegistration": false }
                    },
                    "workspace": {
                        "applyEdit": true,
                        "workspaceEdit": { "documentChanges": false },
                        "configuration": true,
                        "didChangeConfiguration": { "dynamicRegistration": false },
                        "workspaceFolders": true
                    }
                }
            }),
        );
        // No blocking wait — reader thread handles "initialized" + queue flush when
        // the response comes back. The user can keep editing in the meantime.
        Some(client)
    }

    pub fn alloc_id(&self) -> u64 {
        let mut g = self.next_id.lock().unwrap();
        let id = *g;
        *g += 1;
        id
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

    pub fn did_open(&self, path: &Path, text: &str) -> Result<()> {
        self.send_notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": path_to_uri(path),
                    "languageId": self.language_id,
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
