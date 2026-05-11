//! `DapClient` — one spawned debug adapter. Owns the child process, the
//! stdin handle, and a receiver for the reader-thread channel. Doesn't
//! understand DAP semantics — those live in `manager.rs`. Mirrors
//! `lsp/client.rs` but the message shapes use DAP's `seq` / `type` /
//! `command` fields instead of JSON-RPC's `id` / `method`.

use anyhow::Result;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStderr, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use super::io::reader_loop;
use super::specs::{resolve_command, DapAdapterSpec};
use super::types::{DapIncoming, OutputLine};

pub struct DapClient {
    #[allow(dead_code)]
    pub adapter_key: String,
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    pub incoming_rx: Receiver<DapIncoming>,
    /// Adapter stderr — populated by a side thread that pushes each line
    /// into the channel as a synthetic `output` event. Lets the user see
    /// adapter crashes that would otherwise look like a silent hang.
    pub stderr_rx: Receiver<OutputLine>,
    next_seq: Arc<Mutex<u64>>,
}

impl DapClient {
    /// Spawn the adapter described by `spec`. Two reader threads start
    /// immediately: one parses framed DAP messages off stdout, the other
    /// drains stderr into a synthetic output channel so adapter crashes
    /// surface to the pane instead of disappearing.
    pub fn spawn_spec(spec: &DapAdapterSpec) -> Option<Self> {
        let cmd_path = resolve_command(spec.cmd_candidates)?;
        let mut command = Command::new(&cmd_path);
        for arg in spec.args {
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
        let stderr = child.stderr.take()?;

        let (tx, rx) = channel();
        thread::spawn(move || {
            reader_loop(stdout, tx);
        });

        let (stderr_tx, stderr_rx) = channel();
        thread::spawn(move || {
            stderr_loop(stderr, stderr_tx);
        });

        Some(Self {
            adapter_key: spec.key.to_string(),
            child: Arc::new(Mutex::new(child)),
            stdin,
            incoming_rx: rx,
            stderr_rx,
            next_seq: Arc::new(Mutex::new(1)),
        })
    }

    /// Has the adapter process exited? Non-blocking — `Some(code)` once
    /// the child reaps, `None` while it's still alive. The manager polls
    /// this on `drain` so a silent crash becomes an `AdapterError` event.
    pub fn try_exit_status(&self) -> Option<i32> {
        let mut child = self.child.lock().ok()?;
        match child.try_wait() {
            Ok(Some(status)) => Some(status.code().unwrap_or(-1)),
            _ => None,
        }
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

/// Forward each line of the adapter's stderr to the editor as a synthetic
/// "stderr"-category output line. Without this, crashes that print a stack
/// trace and exit immediately look identical to "adapter is alive but not
/// answering" — i.e. an indistinguishable hang.
fn stderr_loop(stderr: ChildStderr, tx: Sender<OutputLine>) {
    let reader = BufReader::new(stderr);
    for line in reader.lines() {
        let Ok(text) = line else { break };
        if text.is_empty() {
            continue;
        }
        let _ = tx.send(OutputLine {
            category: "stderr".into(),
            output: text,
        });
    }
}
