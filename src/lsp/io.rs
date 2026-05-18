//! Reader thread loop and the JSON-RPC dispatcher. Runs on its own thread
//! per spawned client; consumes framed messages off the server's stdout and
//! either sends them up the channel or auto-replies on the spot.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::ChildStdin;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use super::client::{InitState, SemanticTokensLegend};
use super::types::{
    Diagnostic, DiagnosticsMessage, LspIncoming, MessageSeverity, Severity,
};

pub(super) fn reader_loop(
    stdout: impl Read + Send + 'static,
    stdin: Arc<Mutex<ChildStdin>>,
    init_state: Arc<Mutex<InitState>>,
    legend: Arc<Mutex<Option<SemanticTokensLegend>>>,
    code_lens_provider: Arc<Mutex<bool>>,
    code_lens_resolve_provider: Arc<Mutex<bool>>,
    workspace_folders_supported: Arc<Mutex<bool>>,
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
        dispatch(
            value,
            &stdin,
            &init_state,
            &legend,
            &code_lens_provider,
            &code_lens_resolve_provider,
            &workspace_folders_supported,
            &tx,
        );
    }
}

fn dispatch(
    msg: Value,
    stdin: &Arc<Mutex<ChildStdin>>,
    init_state: &Arc<Mutex<InitState>>,
    legend: &Arc<Mutex<Option<SemanticTokensLegend>>>,
    code_lens_provider: &Arc<Mutex<bool>>,
    code_lens_resolve_provider: &Arc<Mutex<bool>>,
    workspace_folders_supported: &Arc<Mutex<bool>>,
    tx: &Sender<LspIncoming>,
) {
    // Server-to-client request: has both `id` and `method`. Auto-reply so the server
    // doesn't stall waiting for a response we won't otherwise produce.
    let id = msg.get("id").and_then(|v| v.as_u64());
    let method = msg.get("method").and_then(|v| v.as_str()).map(|s| s.to_string());
    if let (Some(id), Some(method)) = (id, method.clone()) {
        // workspace/applyEdit needs the main thread to actually mutate
        // buffers — bounce it through the channel and have the main loop
        // reply via `LspManager::send_response`.
        if method == "workspace/applyEdit" {
            let edit = msg
                .get("params")
                .and_then(|p| p.get("edit"))
                .cloned()
                .unwrap_or(Value::Null);
            let _ = tx.send(LspIncoming::ApplyEditRequest { id, edit });
            return;
        }
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
                // First response while Buffering = the initialize reply.
                // Mine its `capabilities.semanticTokensProvider.legend`
                // before promoting state so the manager can decode
                // semantic-token responses against it later.
                if let Some(l) = extract_semantic_tokens_legend(&result) {
                    *legend.lock().unwrap() = Some(l);
                }
                let (has_lens, has_resolve) = extract_code_lens_caps(&result);
                if has_lens {
                    *code_lens_provider.lock().unwrap() = true;
                }
                if has_resolve {
                    *code_lens_resolve_provider.lock().unwrap() = true;
                }
                if extract_workspace_folders_supported(&result) {
                    *workspace_folders_supported.lock().unwrap() = true;
                }
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
    match method {
        "textDocument/publishDiagnostics" => {
            if let Some(params) = msg.get("params") {
                if let Some(d) = parse_publish_diagnostics(params) {
                    let _ = tx.send(LspIncoming::Diagnostics(d));
                }
            }
        }
        // showMessage is popup-style — the server wants the user to
        // notice. logMessage is debug-log noise — server-side stack
        // traces, warm-up progress, etc. We carry both as the same
        // wire variant and let the app decide how loud to be.
        "window/showMessage" | "window/logMessage" => {
            if let Some(params) = msg.get("params") {
                let severity = match params.get("type").and_then(|v| v.as_u64()) {
                    Some(1) => MessageSeverity::Error,
                    Some(2) => MessageSeverity::Warning,
                    Some(3) => MessageSeverity::Info,
                    Some(4) => MessageSeverity::Log,
                    _ => MessageSeverity::Info,
                };
                let text = params
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !text.is_empty() {
                    let _ = tx.send(LspIncoming::ServerMessage {
                        severity,
                        text,
                        is_show: method == "window/showMessage",
                    });
                }
            }
        }
        _ => {}
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
        // workspace/applyEdit is handled out-of-band by the main thread —
        // see `dispatch`. Default arm here just to keep this match
        // exhaustive on future adds.
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

/// Extract the semantic-tokens legend from an `initialize` response.
/// Returns `None` when the server doesn't advertise the capability —
/// in which case `request_semantic_tokens_full` short-circuits.
fn extract_semantic_tokens_legend(init_result: &Value) -> Option<SemanticTokensLegend> {
    let legend = init_result
        .get("capabilities")?
        .get("semanticTokensProvider")?
        .get("legend")?;
    let token_types: Vec<String> = legend
        .get("tokenTypes")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    let token_modifiers: Vec<String> = legend
        .get("tokenModifiers")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    if token_types.is_empty() {
        return None;
    }
    Some(SemanticTokensLegend {
        token_types,
        token_modifiers,
    })
}

/// Inspect the initialize response for code-lens capabilities. The
/// first bool is `codeLensProvider` (boolean shorthand or object
/// form); the second is `codeLensProvider.resolveProvider`, which
/// gates the `codeLens/resolve` round-trip. Servers like csharp-ls
/// return lens items with empty `command` and rely on resolve for
/// the title.
fn extract_code_lens_caps(init_result: &Value) -> (bool, bool) {
    let Some(caps) = init_result.get("capabilities") else { return (false, false); };
    match caps.get("codeLensProvider") {
        Some(v) if v.is_object() => {
            let resolve = v
                .get("resolveProvider")
                .and_then(|r| r.as_bool())
                .unwrap_or(false);
            (true, resolve)
        }
        Some(v) => (v.as_bool().unwrap_or(false), false),
        None => (false, false),
    }
}

/// Pull the `workspace.workspaceFolders.supported` flag out of an
/// `initialize` response. Servers that advertise this can grow their
/// indexed folder set at runtime via
/// `workspace/didChangeWorkspaceFolders`; servers that don't get the
/// single-root treatment forever. The flag is also implicitly true
/// when the server reports `workspace.workspaceFolders` as a bare
/// `true` (older spec shape) — we honour both.
fn extract_workspace_folders_supported(init_result: &Value) -> bool {
    let Some(caps) = init_result.get("capabilities") else { return false; };
    let Some(ws) = caps.get("workspace") else { return false; };
    let Some(folders) = ws.get("workspaceFolders") else { return false; };
    match folders {
        Value::Bool(b) => *b,
        Value::Object(_) => folders
            .get("supported")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        _ => false,
    }
}

#[cfg(test)]
mod cap_tests {
    use super::*;

    #[test]
    fn workspace_folders_supported_reads_object_form() {
        let v: Value = serde_json::from_str(
            r#"{"capabilities":{"workspace":{"workspaceFolders":{"supported":true}}}}"#,
        )
        .unwrap();
        assert!(extract_workspace_folders_supported(&v));
    }

    #[test]
    fn workspace_folders_supported_reads_bool_shorthand() {
        let v: Value = serde_json::from_str(
            r#"{"capabilities":{"workspace":{"workspaceFolders":true}}}"#,
        )
        .unwrap();
        assert!(extract_workspace_folders_supported(&v));
    }

    #[test]
    fn workspace_folders_supported_returns_false_when_object_says_no() {
        let v: Value = serde_json::from_str(
            r#"{"capabilities":{"workspace":{"workspaceFolders":{"supported":false}}}}"#,
        )
        .unwrap();
        assert!(!extract_workspace_folders_supported(&v));
    }

    #[test]
    fn workspace_folders_supported_returns_false_when_field_absent() {
        let v: Value = serde_json::from_str(r#"{"capabilities":{}}"#).unwrap();
        assert!(!extract_workspace_folders_supported(&v));
        let v: Value = serde_json::from_str(r#"{}"#).unwrap();
        assert!(!extract_workspace_folders_supported(&v));
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
