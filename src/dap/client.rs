//! `DapClient` — one spawned debug adapter. Owns the child process, the
//! stdin handle, and a receiver for the reader-thread channel. Doesn't
//! understand DAP semantics — those live in `manager.rs`. Mirrors
//! `lsp/client.rs` but the message shapes use DAP's `seq` / `type` /
//! `command` fields instead of JSON-RPC's `id` / `method`.

use anyhow::Result;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

use super::io::reader_loop;
use super::specs::{resolve_command, DapAdapterSpec};
use super::types::DapIncoming;

pub struct DapClient {
    #[allow(dead_code)]
    pub adapter_key: String,
    _child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pub incoming_rx: Receiver<DapIncoming>,
    next_seq: Arc<Mutex<u64>>,
}

impl DapClient {
    /// Spawn the adapter described by `spec`. The reader thread starts
    /// immediately and pushes parsed messages onto the returned receiver.
    /// Returns `None` if the adapter command can't be resolved on `$PATH`
    /// or the spawn itself fails.
    pub fn spawn_spec(spec: &DapAdapterSpec) -> Option<Self> {
        let cmd_path = resolve_command(spec.cmd_candidates)?;
        let mut command = Command::new(&cmd_path);
        for arg in spec.args {
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

        let (tx, rx) = channel();
        thread::spawn(move || {
            reader_loop(stdout, tx);
        });

        Some(Self {
            adapter_key: spec.key.to_string(),
            _child: child,
            stdin,
            incoming_rx: rx,
            next_seq: Arc::new(Mutex::new(1)),
        })
    }

    pub fn alloc_seq(&self) -> u64 {
        let mut g = self.next_seq.lock().unwrap();
        let id = *g;
        *g += 1;
        id
    }

    /// Send a `request` message. `seq` should come from `alloc_seq` so
    /// responses can be matched against it.
    pub fn send_request(&self, seq: u64, command: &str, arguments: Value) -> Result<()> {
        let msg = json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments,
        });
        self.write_frame(&msg)
    }

    /// Reply to a server-initiated request. The manager bounces
    /// `runInTerminal`-style requests through the main thread (for parity
    /// with how the LSP layer treats `workspace/applyEdit`).
    #[allow(dead_code)]
    pub fn send_response(
        &self,
        request_seq: u64,
        command: &str,
        success: bool,
        body: Value,
    ) -> Result<()> {
        let msg = json!({
            "seq": self.alloc_seq(),
            "type": "response",
            "request_seq": request_seq,
            "success": success,
            "command": command,
            "body": body,
        });
        self.write_frame(&msg)
    }

    fn write_frame(&self, msg: &Value) -> Result<()> {
        let body = serde_json::to_string(msg)?;
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut stdin = self.stdin.lock().unwrap();
        stdin.write_all(frame.as_bytes())?;
        stdin.flush()?;
        Ok(())
    }
}
