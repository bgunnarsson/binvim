//! Reader-thread loop for one DAP adapter. Consumes Content-Length framed
//! JSON off the adapter's stdout, classifies each message as event /
//! response / request, and forwards it on the channel. The manager (main
//! thread) does the protocol bookkeeping.
//!
//! The framing is identical to LSP — same `Content-Length: N\r\n\r\n`
//! header, same JSON body — so this file's framing code mirrors
//! `lsp/io.rs` line-for-line. Only the dispatch differs.

use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::sync::mpsc::Sender;

use super::types::DapIncoming;

pub(super) fn reader_loop(stdout: impl Read + Send + 'static, tx: Sender<DapIncoming>) {
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
        let Some(len) = content_length else { return };
        let mut body = vec![0u8; len];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&body) else { continue };
        dispatch(value, &tx);
    }
}

fn dispatch(msg: Value, tx: &Sender<DapIncoming>) {
    let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match msg_type {
        "event" => {
            let event = msg
                .get("event")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let body = msg.get("body").cloned().unwrap_or(Value::Null);
            let _ = tx.send(DapIncoming::Event { event, body });
        }
        "response" => {
            let request_seq = msg
                .get("request_seq")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let command = msg
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let success = msg
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let body = msg.get("body").cloned().unwrap_or(Value::Null);
            let message = msg
                .get("message")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let _ = tx.send(DapIncoming::Response {
                request_seq,
                command,
                success,
                body,
                message,
            });
        }
        "request" => {
            let seq = msg.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
            let command = msg
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = msg.get("arguments").cloned().unwrap_or(Value::Null);
            let _ = tx.send(DapIncoming::Request {
                seq,
                command,
                arguments,
            });
        }
        _ => {}
    }
}
