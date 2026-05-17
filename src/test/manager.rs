//! `TestManager` — owns the active test-run session plus the user-
//! facing state that survives across runs (output buffer, last run).
//! Mirrors the shape of `DapManager`: process spawn happens here; a
//! reader thread parses adapter stdout line-by-line and pushes events
//! into an mpsc channel; the main loop calls `drain()` once per tick
//! to pull events off and let `app/test_glue.rs` react.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use super::specs::{LineParseState, TestAdapterSpec};
use super::types::{
    OutputStream, ResolvedCommand, TestEvent, TestOutputRow, TestRunRequest, TestSummary,
};

const OUTPUT_LOG_CAP: usize = 5000;

#[derive(Default)]
pub struct TestManager {
    /// Currently-running adapter, if any. `None` between runs.
    pub session: Option<TestSession>,
    /// Streaming overlay rows for the current (or most recent) run.
    /// Cleared at the start of every new run so the user always sees
    /// the latest output without scrolling past stale entries.
    pub output_buffer: Vec<TestOutputRow>,
    /// Rolling tally for the in-progress run — updated as `Finished`
    /// events arrive. Cleared at the start of every new run.
    pub summary: TestSummary,
    /// Failure cases captured from the most recent run, suitable for
    /// loading into the quickfix list. Replaced on every run start.
    pub failures: Vec<super::types::TestFailure>,
    /// The most recent run's request, replayed by `:testlast`. `None`
    /// before the first run.
    pub last_run: Option<TestRunRequest>,
    /// Cached adapter pick for the active workspace, threaded into the
    /// last run so `:testlast` doesn't re-walk the marker tree.
    pub last_adapter_key: Option<String>,
}

pub struct TestSession {
    /// Adapter key the session was spawned with — kept so a future
    /// status line / `:health` panel can identify which runner is
    /// active without re-walking the workspace.
    #[allow(dead_code)]
    pub adapter_key: String,
    /// Full command line rendered for the overlay header. Held here
    /// so a `:health` snapshot can surface the in-flight invocation.
    #[allow(dead_code)]
    pub display_command: String,
    /// Wall-clock start time for the run. Used by the overlay's
    /// "running for N.Ns" footer (future polish).
    #[allow(dead_code)]
    pub started_at: Instant,
    pub child: Arc<Mutex<Child>>,
    pub events_rx: Receiver<TestEvent>,
}

impl TestManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// True while a run is in flight. Drives the status-line activity
    /// indicator and prevents overlapping runs.
    pub fn is_running(&self) -> bool {
        self.session.is_some()
    }

    /// Spawn the adapter described by `spec` against `req`. Returns
    /// `Err` if the resolver rejected the request or the process
    /// couldn't be spawned. On success the session takes over until
    /// the reader thread closes the channel.
    pub fn start_run(
        &mut self,
        spec: &TestAdapterSpec,
        req: TestRunRequest,
    ) -> Result<TestEvent, String> {
        if self.session.is_some() {
            return Err("test run already in progress".into());
        }
        let resolved = (spec.build_run_command)(&req)?;
        let (events_tx, events_rx) = channel();
        let child = spawn_runner(&resolved, spec.clone(), events_tx)?;
        let started = TestEvent::Started {
            adapter_key: spec.key.to_string(),
            command_line: resolved.display.clone(),
        };
        self.output_buffer.clear();
        self.summary = TestSummary::default();
        self.failures.clear();
        self.output_buffer.push(TestOutputRow::Header {
            command_line: resolved.display.clone(),
            started_at: Instant::now(),
        });
        self.last_run = Some(req);
        self.last_adapter_key = Some(spec.key.to_string());
        self.session = Some(TestSession {
            adapter_key: spec.key.to_string(),
            display_command: resolved.display,
            started_at: Instant::now(),
            child,
            events_rx,
        });
        Ok(started)
    }

    /// Pull all available events off the reader-thread channel, fold
    /// them into the manager's rolling state, and return them for the
    /// orchestration layer to react to. Returns `(events, progress)`
    /// — `progress=true` whenever any byte / event was processed so
    /// the main loop knows to schedule a redraw.
    pub fn drain(&mut self) -> (Vec<TestEvent>, bool) {
        let mut events = Vec::new();
        let mut session_dead = false;
        if let Some(session) = self.session.as_ref() {
            while let Ok(ev) = session.events_rx.try_recv() {
                events.push(ev);
            }
            // Detect a silent reader-thread death (e.g. the child was
            // killed externally). If the process has exited AND the
            // channel is drained, the session is over.
            if let Ok(mut child) = session.child.lock() {
                if let Ok(Some(status)) = child.try_wait() {
                    // Drain any final events the reader thread queued
                    // before noticing EOF.
                    while let Ok(ev) = session.events_rx.try_recv() {
                        events.push(ev);
                    }
                    let has_finished = events
                        .iter()
                        .any(|e| matches!(e, TestEvent::Finished { .. }));
                    let has_aborted = events
                        .iter()
                        .any(|e| matches!(e, TestEvent::Aborted { .. }));
                    if !has_finished && !has_aborted && !status.success() {
                        events.push(TestEvent::Aborted {
                            message: format!("adapter exited with code {}", status.code().unwrap_or(-1)),
                        });
                    }
                    session_dead = true;
                }
            }
        }
        let progress = !events.is_empty() || session_dead;
        for ev in &events {
            self.fold_event(ev);
        }
        if session_dead {
            self.session = None;
        }
        (events, progress)
    }

    /// Abort the active run by killing the child. The reader thread
    /// will notice EOF and close the channel; the next `drain()` then
    /// reaps the session.
    pub fn cancel(&mut self) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        let _ = session.child.lock().map(|mut c| c.kill());
        true
    }

    fn fold_event(&mut self, ev: &TestEvent) {
        match ev {
            TestEvent::Started { .. } => {}
            TestEvent::Case {
                name,
                status,
                location,
                message,
            } => {
                self.output_buffer.push(TestOutputRow::Case {
                    name: name.clone(),
                    status: *status,
                    message: message.clone(),
                });
                if matches!(status, super::types::TestStatus::Failed) {
                    // Replace any prior entry for this test — the
                    // post-flush decorated event (with location +
                    // message) supersedes the bare in-stream one.
                    self.failures.retain(|f| f.name != *name);
                    self.failures.push(super::types::TestFailure {
                        name: name.clone(),
                        location: location.clone(),
                        message: message.clone(),
                    });
                }
            }
            TestEvent::Output { stream, text } => {
                self.output_buffer.push(TestOutputRow::Output {
                    stream: *stream,
                    text: text.clone(),
                });
            }
            TestEvent::Finished { summary } => {
                self.summary.add(summary);
                self.output_buffer
                    .push(TestOutputRow::Summary(self.summary.clone()));
            }
            TestEvent::Aborted { message } => {
                self.output_buffer
                    .push(TestOutputRow::Aborted(message.clone()));
            }
        }
        if self.output_buffer.len() > OUTPUT_LOG_CAP {
            let excess = self.output_buffer.len() - OUTPUT_LOG_CAP;
            self.output_buffer.drain(0..excess);
        }
    }
}

/// Spawn the resolved command and start the reader thread that parses
/// its stdout into `TestEvent`s. Stderr is forwarded as `Output {
/// stream: Stderr, … }` so compile errors and other tool chatter
/// reach the overlay.
fn spawn_runner(
    resolved: &ResolvedCommand,
    spec: TestAdapterSpec,
    events_tx: Sender<TestEvent>,
) -> Result<Arc<Mutex<Child>>, String> {
    let mut cmd = Command::new(&resolved.program);
    cmd.args(&resolved.args)
        .current_dir(&resolved.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn {}: {}", resolved.program, e))?;
    let stdout = child.stdout.take().ok_or_else(|| "no stdout".to_string())?;
    let stderr = child.stderr.take().ok_or_else(|| "no stderr".to_string())?;
    let tx_stdout = events_tx.clone();
    let tx_stderr = events_tx;
    let spec_stdout = spec;
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        let mut state = LineParseState::default();
        for line in reader.lines() {
            let Ok(text) = line else { break };
            // Forward the raw line as Output so it lands in the overlay
            // even if the parser doesn't recognise it (compile output,
            // doctest section headers, etc).
            let _ = tx_stdout.send(TestEvent::Output {
                stream: OutputStream::Stdout,
                text: text.clone(),
            });
            let events = (spec_stdout.parse_event_line)(&text, &mut state);
            for ev in events {
                let _ = tx_stdout.send(ev);
            }
        }
        let final_events = (spec_stdout.flush_parser)(&mut state);
        for ev in final_events {
            let _ = tx_stdout.send(ev);
        }
    });
    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let Ok(text) = line else { break };
            let _ = tx_stderr.send(TestEvent::Output {
                stream: OutputStream::Stderr,
                text,
            });
        }
    });
    Ok(Arc::new(Mutex::new(child)))
}

