//! Minimal LSP client. First cut handles diagnostics only:
//! spawn server, send initialize/initialized/didOpen/didChange, push diagnostics
//! notifications to a channel that the main loop drains.

use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

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
}

#[derive(Debug, Clone)]
pub enum LspEvent {
    GotoDef { path: PathBuf, line: usize, col: usize },
    Hover { text: String },
    Completion { items: Vec<CompletionItem> },
    DiagnosticsUpdated,
    NotFound(&'static str),
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

#[derive(Debug, Clone, Copy)]
pub enum PendingRequest {
    GotoDef,
    Hover,
    Completion,
}

/// State of a client's outgoing pipe. Until the server has answered the
/// `initialize` request we buffer frames; the reader thread flushes them in
/// order once it sees the response.
enum InitState {
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

#[derive(Debug, Clone)]
pub struct ServerSpec {
    /// Stable key — one client per (key) per workspace root.
    pub key: String,
    /// LSP languageId sent on textDocument/didOpen.
    pub language_id: String,
    /// Candidate command paths in priority order. First one that resolves wins.
    pub cmd_candidates: Vec<String>,
    pub args: Vec<String>,
    /// Filenames whose presence marks a project root, in priority order.
    pub root_markers: Vec<String>,
    /// initializationOptions field on the initialize request.
    pub initialization_options: Value,
}

/// Pick the LSP server config for a path's extension. `None` if we don't know the extension.
///
/// Command candidates are bare names — `resolve_command` then walks `$PATH` to find them.
/// We only special-case `~/.cargo/bin` for rust-analyzer because that's the Rust toolchain
/// convention (and not tied to any other tool's package manager).
pub fn spec_for_path(path: &Path) -> Option<ServerSpec> {
    let ext = path.extension().and_then(|s| s.to_str())?;
    let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/"));
    let cargo_bin = |bin: &str| format!("{}/.cargo/bin/{}", home, bin);
    let go_bin = |bin: &str| format!("{}/go/bin/{}", home, bin);
    let local_bin = |sub: &str, bin: &str| format!("{}/.local/bin/{}/{}", home, sub, bin);
    let stdio = || vec!["--stdio".to_string()];

    let ts_markers = || {
        vec![
            "package-lock.json".into(),
            "yarn.lock".into(),
            "pnpm-lock.yaml".into(),
            "bun.lockb".into(),
            "bun.lock".into(),
            "tsconfig.json".into(),
            "jsconfig.json".into(),
            "package.json".into(),
            ".git".into(),
        ]
    };
    let ts_init = || json!({ "hostInfo": "binvim", "preferences": {} });

    match ext {
        "rs" => Some(ServerSpec {
            key: "rust".into(),
            language_id: "rust".into(),
            cmd_candidates: vec!["rust-analyzer".into(), cargo_bin("rust-analyzer")],
            args: vec![],
            root_markers: vec!["Cargo.toml".into(), "rust-project.json".into(), ".git".into()],
            initialization_options: Value::Null,
        }),
        "ts" => Some(ServerSpec {
            key: "ts".into(),
            language_id: "typescript".into(),
            cmd_candidates: vec!["typescript-language-server".into()],
            args: stdio(),
            root_markers: ts_markers(),
            initialization_options: ts_init(),
        }),
        "tsx" => Some(ServerSpec {
            key: "ts".into(),
            language_id: "typescriptreact".into(),
            cmd_candidates: vec!["typescript-language-server".into()],
            args: stdio(),
            root_markers: ts_markers(),
            initialization_options: ts_init(),
        }),
        "jsx" => Some(ServerSpec {
            key: "ts".into(),
            language_id: "javascriptreact".into(),
            cmd_candidates: vec!["typescript-language-server".into()],
            args: stdio(),
            root_markers: ts_markers(),
            initialization_options: ts_init(),
        }),
        "js" | "mjs" | "cjs" => Some(ServerSpec {
            key: "ts".into(),
            language_id: "javascript".into(),
            cmd_candidates: vec!["typescript-language-server".into()],
            args: stdio(),
            root_markers: ts_markers(),
            initialization_options: ts_init(),
        }),
        "json" | "jsonc" => {
            // Biome doesn't support global installs — it lives in node_modules.
            // Walk up from the file until we find a node_modules/.bin/biome; if
            // we don't find one, no JSON LSP attaches.
            let start = path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let biome = find_node_modules_bin(&start, "biome")?;
            Some(ServerSpec {
                key: "biome".into(),
                language_id: "json".into(),
                cmd_candidates: vec![biome],
                args: vec!["lsp-proxy".into()],
                root_markers: vec!["biome.json".into(), "biome.jsonc".into(), "package.json".into(), ".git".into()],
                initialization_options: Value::Null,
            })
        }
        "go" => Some(ServerSpec {
            key: "go".into(),
            language_id: "go".into(),
            cmd_candidates: vec!["gopls".into(), go_bin("gopls")],
            args: vec![],
            root_markers: vec!["go.mod".into(), "go.work".into(), ".git".into()],
            initialization_options: Value::Null,
        }),
        "html" | "htm" => Some(ServerSpec {
            key: "html".into(),
            language_id: "html".into(),
            cmd_candidates: vec!["vscode-html-language-server".into()],
            args: stdio(),
            root_markers: vec!["package.json".into(), "*.csproj".into(), ".git".into()],
            initialization_options: Value::Null,
        }),
        "cshtml" | "razor" => {
            // Razor IntelliSense is best with rzls (Razor Language Server) but
            // it isn't packaged in mason or as a NuGet/dotnet tool today. Try
            // it anyway in case the user installed it manually — and otherwise
            // fall back to OmniSharp, which handles .cshtml as a C# document
            // and gives real IntelliSense for the embedded code blocks (@{},
            // @Model.X, etc.). Better than html-LSP-only.
            let rzls = ServerSpec {
                key: "rzls".into(),
                language_id: "razor".into(),
                cmd_candidates: vec!["rzls".into(), local_bin("rzls", "rzls")],
                args: vec![],
                root_markers: vec!["*.csproj".into(), "*.sln".into(), ".git".into()],
                initialization_options: Value::Null,
            };
            if resolve_command(&rzls.cmd_candidates).is_some() {
                return Some(rzls);
            }
            let omnisharp = ServerSpec {
                key: "omnisharp".into(),
                language_id: "razor".into(),
                cmd_candidates: vec![
                    "OmniSharp".into(),
                    "omnisharp".into(),
                    local_bin("omnisharp", "OmniSharp"),
                ],
                args: vec![
                    "-z".into(),
                    "--hostPID".into(),
                    std::process::id().to_string(),
                    "DotNet:enablePackageRestore=false".into(),
                    "--encoding".into(),
                    "utf-8".into(),
                    "--languageserver".into(),
                ],
                root_markers: vec![
                    "*.sln".into(),
                    "*.csproj".into(),
                    "*.fsproj".into(),
                    "*.vbproj".into(),
                    ".git".into(),
                ],
                initialization_options: Value::Null,
            };
            if resolve_command(&omnisharp.cmd_candidates).is_some() {
                return Some(omnisharp);
            }
            // Last resort — at least give markup IntelliSense.
            Some(ServerSpec {
                key: "html".into(),
                language_id: "html".into(),
                cmd_candidates: vec!["vscode-html-language-server".into()],
                args: stdio(),
                root_markers: vec!["package.json".into(), "*.csproj".into(), ".git".into()],
                initialization_options: Value::Null,
            })
        }
        "css" | "scss" | "less" => Some(ServerSpec {
            key: "css".into(),
            language_id: ext.into(),
            cmd_candidates: vec!["vscode-css-language-server".into()],
            args: stdio(),
            root_markers: vec!["package.json".into(), ".git".into()],
            initialization_options: Value::Null,
        }),
        "astro" => Some(ServerSpec {
            key: "astro".into(),
            language_id: "astro".into(),
            cmd_candidates: vec!["astro-ls".into()],
            args: stdio(),
            root_markers: vec!["astro.config.mjs".into(), "astro.config.ts".into(), "package.json".into(), ".git".into()],
            initialization_options: Value::Null,
        }),
        "cs" | "vb" => Some(ServerSpec {
            key: "omnisharp".into(),
            language_id: if ext == "cs" { "csharp".into() } else { "vb".into() },
            cmd_candidates: vec![
                "OmniSharp".into(),
                "omnisharp".into(),
                local_bin("omnisharp", "OmniSharp"),
            ],
            args: vec![
                "-z".into(),
                "--hostPID".into(),
                std::process::id().to_string(),
                "DotNet:enablePackageRestore=false".into(),
                "--encoding".into(),
                "utf-8".into(),
                "--languageserver".into(),
            ],
            root_markers: vec![
                "*.sln".into(),
                "*.csproj".into(),
                "*.fsproj".into(),
                "*.vbproj".into(),
                ".git".into(),
            ],
            initialization_options: Value::Null,
        }),
        _ => None,
    }
}

/// Walk up from `start` looking for any of the marker filenames. Markers
/// starting with `*.` match any directory entry with that extension (used for
/// `.sln` / `.csproj` etc. where the actual filename varies). Falls back to
/// `start` if no marker matches.
pub fn find_workspace_root(start: &Path, markers: &[String]) -> PathBuf {
    let canon = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut dir: &Path = canon.as_path();
    loop {
        for marker in markers {
            if let Some(ext) = marker.strip_prefix("*.") {
                if dir_contains_extension(dir, ext) {
                    return dir.to_path_buf();
                }
            } else if dir.join(marker).exists() {
                return dir.to_path_buf();
            }
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p,
            _ => break,
        }
    }
    canon
}

/// Walk up from `start` looking for `node_modules/.bin/<name>`. Returns the
/// first match (the closest one to the file). Used for tools like biome that
/// don't support global installs.
fn find_node_modules_bin(start: &Path, name: &str) -> Option<String> {
    let canon = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut dir: &Path = canon.as_path();
    loop {
        let candidate = dir.join("node_modules").join(".bin").join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p,
            _ => return None,
        }
    }
}

fn dir_contains_extension(dir: &Path, ext: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(file_ext) = path.extension().and_then(|e| e.to_str()) {
            if file_ext.eq_ignore_ascii_case(ext) {
                return true;
            }
        }
    }
    false
}

fn resolve_command(candidates: &[String]) -> Option<(String, Vec<String>)> {
    for c in candidates {
        let path = if c.starts_with("~/") {
            let home = std::env::var("HOME").ok()?;
            format!("{}/{}", home, &c[2..])
        } else {
            c.clone()
        };
        if path.contains('/') {
            if std::path::Path::new(&path).is_file() {
                return Some((path, vec![]));
            }
            continue;
        }
        if let Some(found) = which_in_path(&path) {
            return Some((found, vec![]));
        }
    }
    None
}

fn which_in_path(name: &str) -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(':') {
        let full = std::path::Path::new(dir).join(name);
        if full.is_file() {
            return Some(full.to_string_lossy().to_string());
        }
    }
    None
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
                                "snippetSupport": false,
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
                        "signatureHelp": { "dynamicRegistration": false },
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

fn reader_loop(
    stdout: impl Read + Send + 'static,
    stdin: Arc<Mutex<ChildStdin>>,
    init_state: Arc<Mutex<InitState>>,
    tx: Sender<LspIncoming>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() || line.is_empty() {
                return;
            }
            let trimmed = line.trim_end_matches(&['\r', '\n'][..]);
            if trimmed.is_empty() {
                break;
            }
            if let Some(rest) = trimmed.to_lowercase().strip_prefix("content-length:") {
                content_length = rest.trim().parse().ok();
            }
        }
        let Some(len) = content_length else { return; };
        let mut body = vec![0u8; len];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&body) else { continue };
        dispatch(value, &stdin, &init_state, &tx);
    }
}

fn dispatch(
    msg: Value,
    stdin: &Arc<Mutex<ChildStdin>>,
    init_state: &Arc<Mutex<InitState>>,
    tx: &Sender<LspIncoming>,
) {
    // Server-to-client request: has both `id` and `method`. Auto-reply so the server
    // doesn't stall waiting for a response we won't otherwise produce.
    let id = msg.get("id").and_then(|v| v.as_u64());
    let method = msg.get("method").and_then(|v| v.as_str()).map(|s| s.to_string());
    if let (Some(id), Some(method)) = (id, method.clone()) {
        auto_respond(stdin, id, &method, msg.get("params"));
        return;
    }

    // Response: has `id` and either `result` or `error`.
    if let Some(id) = id {
        if let Some(result) = msg.get("result").cloned() {
            // First response while still buffering = answer to `initialize`.
            // Promote the queue to Ready, send "initialized", then flush queued
            // frames in order. We hold the lock for the whole flush so any
            // main-thread sends wait until we're done — preserving order.
            let mut g = init_state.lock().unwrap();
            if matches!(*g, InitState::Buffering(_)) {
                let frames = match std::mem::replace(&mut *g, InitState::Ready) {
                    InitState::Buffering(f) => f,
                    InitState::Ready => Vec::new(),
                };
                let init_notif = json!({
                    "jsonrpc": "2.0",
                    "method": "initialized",
                    "params": {},
                });
                if let Ok(body) = serde_json::to_string(&init_notif) {
                    let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
                    if let Ok(mut s) = stdin.lock() {
                        let _ = s.write_all(frame.as_bytes());
                        let _ = s.flush();
                    }
                }
                for frame in frames {
                    if let Ok(mut s) = stdin.lock() {
                        let _ = s.write_all(&frame);
                        let _ = s.flush();
                    }
                }
            }
            drop(g);
            let _ = tx.send(LspIncoming::Response { id, result });
            return;
        }
        if let Some(err) = msg.get("error") {
            let message = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let _ = tx.send(LspIncoming::ErrorReply { id, message });
            return;
        }
    }

    // Plain notification (no `id`).
    let Some(method) = msg.get("method").and_then(|v| v.as_str()) else { return; };
    if method == "textDocument/publishDiagnostics" {
        if let Some(params) = msg.get("params") {
            if let Some(d) = parse_publish_diagnostics(params) {
                let _ = tx.send(LspIncoming::Diagnostics(d));
            }
        }
    }
}

/// Reply to server-to-client requests with reasonable defaults so the server's
/// initialization (and ongoing operation) isn't blocked waiting for us.
fn auto_respond(
    stdin: &Arc<Mutex<ChildStdin>>,
    id: u64,
    method: &str,
    params: Option<&Value>,
) {
    let result = match method {
        // workspace/configuration → array of nulls, sized to params.items.len().
        "workspace/configuration" => {
            let n = params
                .and_then(|p| p.get("items"))
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            json!(vec![Value::Null; n])
        }
        // workspace/applyEdit → claim we applied (we don't yet).
        "workspace/applyEdit" => json!({ "applied": false }),
        // Various capability registrations / progress windows → null is fine.
        _ => Value::Null,
    };
    let resp = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    let body = match serde_json::to_string(&resp) {
        Ok(s) => s,
        Err(_) => return,
    };
    let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
    if let Ok(mut s) = stdin.lock() {
        let _ = s.write_all(frame.as_bytes());
        let _ = s.flush();
    }
}

fn parse_publish_diagnostics(params: &Value) -> Option<DiagnosticsMessage> {
    let uri = params.get("uri")?.as_str()?.to_string();
    let arr = params.get("diagnostics")?.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for d in arr {
        let range = d.get("range")?;
        let start = range.get("start")?;
        let end = range.get("end")?;
        let line = start.get("line")?.as_u64()? as usize;
        let col = start.get("character")?.as_u64()? as usize;
        let end_line = end.get("line")?.as_u64()? as usize;
        let end_col = end.get("character")?.as_u64()? as usize;
        let severity = match d.get("severity").and_then(|v| v.as_u64()) {
            Some(1) => Severity::Error,
            Some(2) => Severity::Warning,
            Some(3) => Severity::Info,
            Some(4) => Severity::Hint,
            _ => Severity::Info,
        };
        let message = d
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(Diagnostic {
            line,
            col,
            end_line,
            end_col,
            severity,
            message,
        });
    }
    Some(DiagnosticsMessage {
        uri,
        diagnostics: out,
    })
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

/// Container for per-language LSP clients keyed by `ServerSpec.key`.
pub struct LspManager {
    clients: HashMap<String, LspClient>,
    pub diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
    pending: HashMap<u64, PendingRequest>,
}

impl LspManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            diagnostics: HashMap::new(),
            pending: HashMap::new(),
        }
    }

    pub fn ensure_for_path(&mut self, path: &Path, fallback_root: &Path) -> Option<&LspClient> {
        let spec = spec_for_path(path)?;
        if !self.clients.contains_key(&spec.key) {
            // Walk up from the buffer's parent dir for the actual project root.
            let start = path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| fallback_root.to_path_buf());
            let root = find_workspace_root(&start, &spec.root_markers);
            let client = LspClient::spawn_spec(&spec, &root)?;
            self.clients.insert(spec.key.clone(), client);
        }
        self.clients.get(&spec.key)
    }

    pub fn client_for_path(&self, path: &Path) -> Option<&LspClient> {
        let spec = spec_for_path(path)?;
        self.clients.get(&spec.key)
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
        for client in self.clients.values() {
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
                        if let Some(req) = self.pending.remove(&id) {
                            if let Some(ev) = handle_response(req, &result) {
                                events.push(ev);
                            }
                        }
                    }
                    LspIncoming::ErrorReply { id, .. } => {
                        self.pending.remove(&id);
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
        self.pending.insert(id, PendingRequest::GotoDef);
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
        self.pending.insert(id, PendingRequest::Hover);
        true
    }

    pub fn request_completion(
        &mut self,
        path: &Path,
        line: usize,
        col: usize,
        trigger_char: Option<char>,
    ) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        // LSP CompletionTriggerKind: 1=Invoked, 2=TriggerCharacter.
        // Servers use this to decide whether to return member-access
        // completions (after `.`, `:`, etc.) versus general scope items.
        let context = match trigger_char {
            Some(c) => json!({ "triggerKind": 2, "triggerCharacter": c.to_string() }),
            None => json!({ "triggerKind": 1 }),
        };
        let _ = client.send_request(
            id,
            "textDocument/completion",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": col },
                "context": context,
            }),
        );
        self.pending.insert(id, PendingRequest::Completion);
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
    }
}

fn parse_completion_response(result: &Value) -> Vec<CompletionItem> {
    let arr = if result.is_array() {
        result.as_array().cloned().unwrap_or_default()
    } else if let Some(items) = result.get("items").and_then(|v| v.as_array()) {
        items.clone()
    } else {
        return Vec::new();
    };
    // Don't cap here — the client filters by typed prefix afterwards, and
    // capping at the wire would silently drop relevant items past the cap
    // (typescript-language-server can return several thousand for a top-level
    // identifier position).
    let mut out = Vec::with_capacity(arr.len());
    for item in arr.iter() {
        let label = item
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if label.is_empty() {
            continue;
        }
        let insert_text = item
            .get("insertText")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                item.get("textEdit")
                    .and_then(|t| t.get("newText"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| label.clone());
        let kind = item.get("kind").and_then(|v| v.as_u64()).map(kind_label);
        let detail = item
            .get("detail")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let filter_text = item
            .get("filterText")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| label.clone());
        let sort_text = item
            .get("sortText")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| label.clone());
        out.push(CompletionItem {
            label,
            insert_text,
            kind,
            detail,
            filter_text,
            sort_text,
        });
    }
    out
}

fn kind_label(k: u64) -> String {
    // Mapping per LSP spec.
    match k {
        1 => "text",
        2 => "method",
        3 => "function",
        4 => "constructor",
        5 => "field",
        6 => "variable",
        7 => "class",
        8 => "interface",
        9 => "module",
        10 => "property",
        11 => "unit",
        12 => "value",
        13 => "enum",
        14 => "keyword",
        15 => "snippet",
        16 => "color",
        17 => "file",
        18 => "reference",
        19 => "folder",
        20 => "enum-member",
        21 => "constant",
        22 => "struct",
        23 => "event",
        24 => "operator",
        25 => "type-param",
        _ => "?",
    }
    .into()
}

fn parse_def_response(result: &Value) -> Option<(PathBuf, usize, usize)> {
    if result.is_null() {
        return None;
    }
    let loc = if result.is_array() {
        result.as_array()?.first()?
    } else {
        result
    };
    // Location | LocationLink — try .uri first, then .targetUri.
    let uri = loc
        .get("uri")
        .and_then(|v| v.as_str())
        .or_else(|| loc.get("targetUri").and_then(|v| v.as_str()))?;
    let path = uri_to_path(uri)?;
    let range = loc
        .get("range")
        .or_else(|| loc.get("targetSelectionRange"))
        .or_else(|| loc.get("targetRange"))?;
    let start = range.get("start")?;
    let line = start.get("line")?.as_u64()? as usize;
    let col = start.get("character")?.as_u64()? as usize;
    Some((path, line, col))
}

fn parse_hover_response(result: &Value) -> Option<String> {
    if result.is_null() {
        return None;
    }
    let contents = result.get("contents")?;
    if let Some(s) = contents.as_str() {
        return Some(s.to_string());
    }
    if let Some(obj) = contents.as_object() {
        if let Some(v) = obj.get("value").and_then(|v| v.as_str()) {
            return Some(v.to_string());
        }
    }
    if let Some(arr) = contents.as_array() {
        let mut out = String::new();
        for item in arr {
            let s = item
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| item.get("value").and_then(|v| v.as_str()).map(|s| s.to_string()));
            if let Some(s) = s {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&s);
            }
        }
        if !out.is_empty() {
            return Some(out);
        }
    }
    None
}
