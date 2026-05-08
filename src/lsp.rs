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

pub struct LspClient {
    #[allow(dead_code)]
    pub name: &'static str,
    _child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pub diagnostics_rx: Receiver<DiagnosticsMessage>,
    next_id: Arc<Mutex<u64>>,
    /// Tracks whether the server has answered the initialize request — used by
    /// follow-up chunks (hover, go-to-def) once we issue requests that block on it.
    #[allow(dead_code)]
    pub initialized: Arc<Mutex<bool>>,
    #[allow(dead_code)]
    pub root_uri: String,
}

impl LspClient {
    /// Spawn an LSP server. Returns `None` if the binary isn't on PATH.
    pub fn spawn(name: &'static str, cmd: &str, root: &Path) -> Option<Self> {
        let mut child = Command::new(cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdin = Arc::new(Mutex::new(child.stdin.take()?));
        let stdout = child.stdout.take()?;

        let (diag_tx, diag_rx) = channel();
        let initialized = Arc::new(Mutex::new(false));
        let init_clone = initialized.clone();
        thread::spawn(move || {
            reader_loop(stdout, diag_tx, init_clone);
        });

        let root_uri = path_to_uri(root);
        let client = Self {
            name,
            _child: child,
            stdin,
            diagnostics_rx: diag_rx,
            next_id: Arc::new(Mutex::new(1)),
            initialized,
            root_uri: root_uri.clone(),
        };

        // Fire initialize + initialized; the server publishes diagnostics later.
        let init_id = client.alloc_id();
        let _ = client.send_request(
            init_id,
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": {
                    "textDocument": {
                        "synchronization": {
                            "didOpen": true,
                            "didChange": true,
                            "didClose": true
                        },
                        "publishDiagnostics": { "relatedInformation": false }
                    }
                },
                "workspaceFolders": [{ "uri": root_uri, "name": "root" }]
            }),
        );
        let _ = client.send_notification(
            "initialized",
            json!({}),
        );
        Some(client)
    }

    fn alloc_id(&self) -> u64 {
        let mut g = self.next_id.lock().unwrap();
        let id = *g;
        *g += 1;
        id
    }

    fn send_raw(&self, msg: &Value) -> Result<()> {
        let body = serde_json::to_string(msg)?;
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut stdin = self.stdin.lock().unwrap();
        stdin.write_all(frame.as_bytes())?;
        stdin.flush()?;
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

    pub fn did_open(&self, path: &Path, language_id: &str, text: &str) -> Result<()> {
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

fn reader_loop(
    stdout: impl Read + Send + 'static,
    diag_tx: Sender<DiagnosticsMessage>,
    initialized: Arc<Mutex<bool>>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut content_length: Option<usize> = None;
        // Headers
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
        dispatch(&value, &diag_tx, &initialized);
    }
}

fn dispatch(msg: &Value, diag_tx: &Sender<DiagnosticsMessage>, initialized: &Arc<Mutex<bool>>) {
    // Response (has "id") — only initialize matters for now.
    if msg.get("id").is_some() && msg.get("result").is_some() {
        if let Ok(mut guard) = initialized.lock() {
            *guard = true;
        }
        return;
    }
    let Some(method) = msg.get("method").and_then(|v| v.as_str()) else { return; };
    if method == "textDocument/publishDiagnostics" {
        if let Some(params) = msg.get("params") {
            if let Some(d) = parse_publish_diagnostics(params) {
                let _ = diag_tx.send(d);
            }
        }
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

/// Container for per-language LSP clients. Phase-5 v1 just speaks rust-analyzer.
pub struct LspManager {
    clients: HashMap<&'static str, LspClient>,
    pub diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
}

impl LspManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            diagnostics: HashMap::new(),
        }
    }

    pub fn ensure_for_path(&mut self, path: &Path, root: &Path) -> Option<&LspClient> {
        let key: &'static str = if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            "rust"
        } else {
            return None;
        };
        if !self.clients.contains_key(key) {
            let client = LspClient::spawn(key, "rust-analyzer", root)?;
            self.clients.insert(key, client);
        }
        self.clients.get(key)
    }

    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        for client in self.clients.values() {
            while let Ok(msg) = client.diagnostics_rx.try_recv() {
                if let Some(path) = uri_to_path(&msg.uri) {
                    self.diagnostics.insert(path, msg.diagnostics);
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn diagnostics_for(&self, path: &Path) -> Option<&Vec<Diagnostic>> {
        // Try canonical and as-given.
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
}
