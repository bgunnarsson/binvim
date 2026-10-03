//! Jupyter kernels for notebook buffers.
//!
//! binvim doesn't speak the kernel's ZeroMQ protocol itself. It runs
//! `kernel_bridge.py` under the interpreter the kernel should be — any
//! Python with ipykernel has jupyter_client, which does speak it — and
//! trades one JSON object per line with it over stdio, the way the LSP and
//! DAP clients talk to their servers.
//!
//! Finding the interpreter and starting the bridge happen on a thread, so
//! the UI never waits on a Python starting up; cells run before the kernel
//! is ready queue in the command channel and go once it is.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Which cells a run sends, counted from the one the cursor is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunScope {
    Cell,
    /// The cell, then the cursor to the next one (a new one at the end).
    Advance,
    All,
    /// Every cell above, not this one.
    Above,
    /// This cell and every one below.
    Below,
}

/// `:cell run|clear|output`, `:kernel …` and their `<leader>n` keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelCmd {
    Run(RunScope),
    Start,
    Restart,
    Interrupt,
    Stop,
    Clear { all: bool },
    Output,
}

const BRIDGE: &str = include_str!("kernel_bridge.py");

#[derive(Debug, Clone, PartialEq)]
pub enum KernelEvent {
    /// The kernel answered and takes cells now.
    Ready {
        python: PathBuf,
        version: String,
    },
    /// No interpreter with ipykernel. `Some` is an environment the notebook
    /// belongs to (a project venv, `$VIRTUAL_ENV`) that lacks it — installing
    /// it elsewhere wouldn't help, since the notebook's imports live there.
    Missing(Option<PathBuf>),
    Busy(String),
    Count(String, u64),
    /// An output, with the display id it can be updated by.
    Output(String, Value, Option<String>),
    /// `update_display`: the new output for every display with this id.
    Update(String, Value),
    Clear(String, bool),
    Done(String),
    /// The kernel or the bridge is gone; nothing more comes from it.
    Dead(String),
}

/// Dropping it closes the command channel; the writer thread then closes
/// the bridge's stdin, and the bridge shuts the kernel down. Closing binvim
/// any other way closes that stdin too.
pub struct Kernel {
    cmds: Sender<Value>,
    events: Receiver<KernelEvent>,
    pub ready: bool,
    /// Cells sent and not finished, so the run loop polls quickly only
    /// while something is coming.
    pub outstanding: usize,
}

impl Kernel {
    /// A kernel for a notebook in `dir`, started in `dir` as Jupyter starts
    /// it, so relative paths in the notebook resolve the same.
    pub fn start(dir: &Path) -> Kernel {
        let (cmd_tx, cmd_rx) = channel::<Value>();
        let (ev_tx, ev_rx) = channel();
        let dir = dir.to_path_buf();
        std::thread::spawn(move || {
            let python = match find_interpreter(&dir) {
                Ok(p) => p,
                Err(env) => {
                    let _ = ev_tx.send(KernelEvent::Missing(env));
                    return;
                }
            };
            run_bridge(&python, &dir, cmd_rx, ev_tx);
        });
        Kernel {
            cmds: cmd_tx,
            events: ev_rx,
            ready: false,
            outstanding: 0,
        }
    }

    pub fn execute(&mut self, cell: &str, code: &str) {
        self.outstanding += 1;
        let _ = self
            .cmds
            .send(json!({"op": "execute", "cell": cell, "code": code}));
    }

    pub fn interrupt(&self) {
        let _ = self.cmds.send(json!({"op": "interrupt"}));
    }

    pub fn try_recv(&self) -> Option<KernelEvent> {
        self.events.try_recv().ok()
    }
}

fn run_bridge(python: &Path, dir: &Path, cmds: Receiver<Value>, events: Sender<KernelEvent>) {
    let mut command = Command::new(python);
    command
        .arg("-c")
        .arg(BRIDGE)
        .arg(dir)
        // Not the notebook's directory: `-c` puts the current directory on
        // `sys.path`, and the bridge imports json and queue before it gets
        // to start the kernel there.
        .current_dir(neutral_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Its own process group: a Ctrl-C typed while the editor is suspended
    // for lazygit or the installer must not interrupt the kernel.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = events.send(KernelEvent::Dead(format!(
                "could not run {}: {e}",
                python.display()
            )));
            return;
        }
    };
    let (Some(mut stdin), Some(stdout), Some(mut stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        let _ = child.kill();
        let _ = child.wait();
        return;
    };
    let err_thread = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    std::thread::spawn(move || read_events(stdout, err_thread, events));
    for cmd in cmds {
        if writeln!(stdin, "{cmd}")
            .and_then(|_| stdin.flush())
            .is_err()
        {
            break;
        }
    }
    drop(stdin);
    reap(child);
}

/// Wait for the bridge to shut its kernel down, and kill it if it hangs.
fn reap(mut child: Child) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
}

fn read_events(
    stdout: impl Read,
    err_thread: std::thread::JoinHandle<String>,
    events: Sender<KernelEvent>,
) {
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let Some(ev) = parse_event(&line) else { continue };
        let dead = matches!(ev, KernelEvent::Dead(_));
        if events.send(ev).is_err() || dead {
            return;
        }
    }
    // The bridge went without saying why: it crashed before it could, and
    // what Python printed is the reason.
    let err = err_thread.join().unwrap_or_default();
    let last = err.lines().rev().find(|l| !l.trim().is_empty());
    let msg = match last {
        Some(l) => format!("the kernel bridge exited: {}", l.trim()),
        None => "the kernel bridge exited".to_string(),
    };
    let _ = events.send(KernelEvent::Dead(msg));
}

fn parse_event(line: &str) -> Option<KernelEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    let cell = || v.get("cell").and_then(Value::as_str).map(str::to_string);
    Some(match v.get("ev")?.as_str()? {
        "ready" => KernelEvent::Ready {
            python: PathBuf::from(v.get("python").and_then(Value::as_str).unwrap_or("")),
            version: v
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        "busy" => KernelEvent::Busy(cell()?),
        "count" => KernelEvent::Count(cell()?, v.get("count")?.as_u64()?),
        "output" => KernelEvent::Output(
            cell()?,
            v.get("output")?.clone(),
            v.get("display_id")
                .and_then(Value::as_str)
                .map(str::to_string),
        ),
        "update" => KernelEvent::Update(
            v.get("display_id")?.as_str()?.to_string(),
            v.get("output")?.clone(),
        ),
        "clear" => KernelEvent::Clear(
            cell()?,
            v.get("wait").and_then(Value::as_bool).unwrap_or(false),
        ),
        "done" => KernelEvent::Done(cell()?),
        "dead" => KernelEvent::Dead(
            v.get("msg")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        ),
        _ => return None,
    })
}

fn neutral_dir() -> PathBuf {
    if cfg!(unix) {
        PathBuf::from("/")
    } else {
        std::env::temp_dir()
    }
}

/// The interpreter a notebook in `dir` runs under. An environment the
/// notebook belongs to wins (`paths::python_env`), because that is where its
/// imports are installed, and if it lacks ipykernel the answer is to install
/// it there (`Err(Some(python))`). Without one, the first Python on `$PATH`
/// that has ipykernel, then the venv `:install` puts ipykernel in.
pub fn find_interpreter(dir: &Path) -> Result<PathBuf, Option<PathBuf>> {
    find_interpreter_with(dir, |py| {
        binvim::install::python_has_module(py, "ipykernel")
    })
}

fn find_interpreter_with(
    dir: &Path,
    has_ipykernel: impl Fn(&Path) -> bool,
) -> Result<PathBuf, Option<PathBuf>> {
    if let Some(py) = crate::paths::python_env(dir) {
        return if has_ipykernel(&py) {
            Ok(py)
        } else {
            Err(Some(py))
        };
    }
    for name in ["python3", "python"] {
        if let Some(py) = crate::paths::find_on_path(name) {
            if has_ipykernel(&py) {
                return Ok(py);
            }
        }
    }
    binvim::install::module_python("ipykernel").ok_or(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_parse_from_the_bridge_lines() {
        assert_eq!(
            parse_event(r#"{"ev":"count","cell":"a","count":3}"#),
            Some(KernelEvent::Count("a".into(), 3))
        );
        assert_eq!(
            parse_event(r#"{"ev":"clear","cell":"a","wait":true}"#),
            Some(KernelEvent::Clear("a".into(), true))
        );
        assert_eq!(
            parse_event(r#"{"ev":"done","cell":"a","count":null}"#),
            Some(KernelEvent::Done("a".into()))
        );
        assert_eq!(
            parse_event(r#"{"ev":"dead","msg":"gone"}"#),
            Some(KernelEvent::Dead("gone".into()))
        );
        assert_eq!(parse_event("not json"), None);
        assert_eq!(
            parse_event(r#"{"ev":"output","cell":"a","output":{},"display_id":"d"}"#),
            Some(KernelEvent::Output(
                "a".into(),
                serde_json::json!({}),
                Some("d".into())
            ))
        );
        assert_eq!(
            parse_event(r#"{"ev":"update","display_id":"d","output":{}}"#),
            Some(KernelEvent::Update("d".into(), serde_json::json!({})))
        );
        assert_eq!(parse_event(r#"{"ev":"busy"}"#), None);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("binvim-kernel-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn a_project_venv_without_ipykernel_is_reported_not_skipped() {
        let dir = scratch("venv");
        let py = dir.join(".venv/bin/python");
        std::fs::create_dir_all(py.parent().unwrap()).unwrap();
        std::fs::write(&py, "").unwrap();
        let sub = dir.join("lessons");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(
            find_interpreter_with(&sub, |_| false),
            Err(Some(py.clone()))
        );
        assert_eq!(find_interpreter_with(&sub, |_| true), Ok(py));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stdout_eof_without_a_reason_reports_the_last_stderr_line() {
        let (tx, rx) = channel();
        let err = std::thread::spawn(|| "Traceback\n  oops\nSyntaxError: bad\n\n".to_string());
        read_events(&b"{\"ev\":\"busy\",\"cell\":\"a\"}\n"[..], err, tx);
        assert_eq!(rx.recv().unwrap(), KernelEvent::Busy("a".into()));
        assert_eq!(
            rx.recv().unwrap(),
            KernelEvent::Dead("the kernel bridge exited: SyntaxError: bad".into())
        );
    }
}
