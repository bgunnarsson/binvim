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
    NotFound(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub enum PendingRequest {
    GotoDef,
    Hover,
}

pub struct LspClient {
    #[allow(dead_code)]
    pub name: &'static str,
    _child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pub incoming_rx: Receiver<LspIncoming>,
    next_id: Arc<Mutex<u64>>,
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

        let (in_tx, in_rx) = channel();
        let initialized = Arc::new(Mutex::new(false));
        let init_clone = initialized.clone();
        thread::spawn(move || {
            reader_loop(stdout, in_tx, init_clone);
        });

        let root_uri = path_to_uri(root);
        let client = Self {
            name,
            _child: child,
            stdin,
            incoming_rx: in_rx,
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

    pub fn alloc_id(&self) -> u64 {
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
    tx: Sender<LspIncoming>,
    initialized: Arc<Mutex<bool>>,
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
        dispatch(value, &tx, &initialized);
    }
}

fn dispatch(msg: Value, tx: &Sender<LspIncoming>, initialized: &Arc<Mutex<bool>>) {
    // Response: has "id" and either "result" or "error".
    if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
        if let Some(result) = msg.get("result").cloned() {
            if let Ok(mut g) = initialized.lock() {
                *g = true;
            }
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
    // Notification.
    let Some(method) = msg.get("method").and_then(|v| v.as_str()) else { return; };
    if method == "textDocument/publishDiagnostics" {
        if let Some(params) = msg.get("params") {
            if let Some(d) = parse_publish_diagnostics(params) {
                let _ = tx.send(LspIncoming::Diagnostics(d));
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
    /// Outstanding requests we issued; matched against incoming responses by id.
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

    fn lang_key_for_path(path: &Path) -> Option<&'static str> {
        match path.extension().and_then(|s| s.to_str()) {
            Some("rs") => Some("rust"),
            _ => None,
        }
    }

    pub fn ensure_for_path(&mut self, path: &Path, root: &Path) -> Option<&LspClient> {
        let key = Self::lang_key_for_path(path)?;
        if !self.clients.contains_key(key) {
            let client = LspClient::spawn(key, "rust-analyzer", root)?;
            self.clients.insert(key, client);
        }
        self.clients.get(key)
    }

    pub fn client_for_path(&self, path: &Path) -> Option<&LspClient> {
        let key = Self::lang_key_for_path(path)?;
        self.clients.get(key)
    }

    pub fn drain(&mut self) -> Vec<LspEvent> {
        let mut events = Vec::new();
        let mut diagnostics_changed = false;
        for client in self.clients.values() {
            while let Ok(msg) = client.incoming_rx.try_recv() {
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
        }
        // Caller can re-render whenever diagnostics arrive even without a code event.
        let _ = diagnostics_changed;
        events
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
    }
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
