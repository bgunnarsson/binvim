//! Orchestrates the integrated test runner. Pure UI glue — every
//! adapter-specific decision lives in `crate::test::specs` /
//! `crate::test::cargo`. This module owns the `:test*` ex-commands,
//! the per-tick `TestEvent` drain, and the bridge to quickfix + the
//! results overlay.
//!
//! Flow:
//!  - `:test` opens a picker of discovered tests (cargo: parsed from
//!    `cargo test -- --list`).
//!  - `:testnearest` / `:testfile` / `:testlast` build a
//!    `TestRunRequest` and hand it to `test.start_run`.
//!  - Main loop drains `test.drain()` → `handle_test_events`.
//!  - On `Finished` with failures, the overlay opens and the captured
//!    failures populate the quickfix list (replacing the previous one)
//!    so `]q` / `[q` walk them.

use std::path::PathBuf;

use crate::app::state::{QuickfixEntry, QuickfixState};
use crate::picker::{PickerKind, PickerPayload, PickerState};
use crate::test::{
    adapter_for_workspace, TestAdapterSpec, TestEvent, TestRunRequest, TestStatus, TestSummary,
};

impl super::App {
    /// `:test` — discover test items in the active buffer's workspace
    /// and open a picker. Selecting a row runs that single test by
    /// passing its name as the filter.
    pub(super) fn cmd_test_picker(&mut self) {
        let Some((spec, root)) = self.test_resolve_adapter() else {
            self.status_msg = "test: no adapter for this workspace".into();
            return;
        };
        let Some(list_cmd) = (spec.build_list_command)(&root) else {
            self.status_msg = format!("test: {} adapter has no discovery", spec.key);
            return;
        };
        // Discovery shells out synchronously — it's only ever a list
        // command, but on a cold workspace `cargo test -- --list` will
        // actually build first. Surface a status message so the user
        // doesn't think the editor froze.
        self.status_msg = format!("Discovering tests with `{}`…", list_cmd.display);
        let output = std::process::Command::new(&list_cmd.program)
            .args(&list_cmd.args)
            .current_dir(&list_cmd.cwd)
            .output();
        let Ok(out) = output else {
            self.status_msg = format!("test: failed to spawn {}", list_cmd.program);
            return;
        };
        if !out.status.success() {
            let tail = String::from_utf8_lossy(&out.stderr);
            let snippet: String = tail
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .chars()
                .take(120)
                .collect();
            self.status_msg = format!("test: discovery failed — {snippet}");
            return;
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let names = (spec.parse_list_output)(&stdout);
        if names.is_empty() {
            self.status_msg = "test: no tests discovered".into();
            return;
        }
        let items: Vec<(String, PickerPayload)> = names
            .into_iter()
            .map(|n| {
                let payload = PickerPayload::TestTarget {
                    adapter_key: spec.key.to_string(),
                    name: n.clone(),
                };
                (n, payload)
            })
            .collect();
        let title = format!("Tests ({} adapter)", spec.key);
        self.picker = Some(PickerState::new(PickerKind::TestTarget, title, items));
        self.mode = crate::mode::Mode::Picker;
    }

    /// `:testnearest` — find the test function enclosing the cursor and
    /// run it. Falls back to a status message if no `#[test]` (or
    /// equivalent attribute) is found above the cursor.
    pub(super) fn cmd_test_nearest(&mut self) {
        let Some((spec, root)) = self.test_resolve_adapter() else {
            self.status_msg = "test: no adapter for this workspace".into();
            return;
        };
        let cursor_line = self.window.cursor.line;
        let text = self.buffer.rope.to_string();
        let Some(name) = (spec.filter_for_nearest)(&text, cursor_line) else {
            self.status_msg = "test: no test under cursor".into();
            return;
        };
        let label = format!("nearest: {name}");
        let req = TestRunRequest {
            filter: Some(name),
            workspace_root: root,
            label,
        };
        self.test_kickoff(&spec, req);
    }

    /// `:testfile` — run every test in the active buffer's file. Uses
    /// the adapter's `filter_for_file` heuristic to build the libtest
    /// substring filter.
    pub(super) fn cmd_test_file(&mut self) {
        let Some((spec, root)) = self.test_resolve_adapter() else {
            self.status_msg = "test: no adapter for this workspace".into();
            return;
        };
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "test: save the buffer first".into();
            return;
        };
        let filter = (spec.filter_for_file)(&path, &root);
        let label = match filter.as_deref() {
            Some(f) => format!("file: {f}"),
            None => "all".into(),
        };
        let req = TestRunRequest {
            filter,
            workspace_root: root,
            label,
        };
        self.test_kickoff(&spec, req);
    }

    /// `:testlast` — re-run the last invocation. Falls back to a
    /// status message if nothing has run yet this session.
    pub(super) fn cmd_test_last(&mut self) {
        let Some(req) = self.test.last_run.clone() else {
            self.status_msg = "test: no previous run".into();
            return;
        };
        let Some((spec, _)) = self.test_resolve_adapter() else {
            self.status_msg = "test: no adapter for this workspace".into();
            return;
        };
        self.test_kickoff(&spec, req);
    }

    /// `:testcancel` — kill the running adapter, if any.
    pub(super) fn cmd_test_cancel(&mut self) {
        if self.test.cancel() {
            self.status_msg = "test: cancelling…".into();
        } else {
            self.status_msg = "test: no run in progress".into();
        }
    }

    /// `:testresults` — toggle the streaming overlay. Useful for
    /// scrolling back through a finished run without re-running.
    pub(super) fn cmd_test_results(&mut self) {
        if self.test.output_buffer.is_empty() {
            self.status_msg = "test: no results yet".into();
            return;
        }
        self.show_test_results_page = true;
        self.show_health_page = false;
        self.show_messages_page = false;
        self.show_start_page = false;
        // Land at the bottom — opening manually after a run finished
        // is almost always "show me the latest", and tail mode also
        // keeps streaming events visible if the run is still going.
        self.test_results_at_tail = true;
        self.test_results_scroll = 0;
        self.completion = None;
        self.hover = None;
        self.signature_help = None;
        self.whichkey = None;
    }

    /// Adapter pick scoped to the active buffer's path. Falls back to
    /// cwd when the buffer is unsaved so `:test*` still works in a
    /// brand-new editor session.
    pub(super) fn test_resolve_adapter(&self) -> Option<(TestAdapterSpec, PathBuf)> {
        let start = self
            .buffer
            .path
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        adapter_for_workspace(&start)
    }

    /// Common kickoff path used by every `:test*` variant. Replaces
    /// the previous run's overlay buffer, spawns the adapter, surfaces
    /// the spawn outcome through the status line, and auto-opens the
    /// overlay so the user sees the run streaming live.
    pub(super) fn test_kickoff(&mut self, spec: &TestAdapterSpec, req: TestRunRequest) {
        let label = req.label.clone();
        match self.test.start_run(spec, req) {
            Ok(_started) => {
                self.show_test_results_page = true;
                self.show_health_page = false;
                self.show_messages_page = false;
                self.show_start_page = false;
                // Fresh run — always start in tail-follow mode so the
                // user sees pass/fail lines stream in at the bottom
                // without having to G.
                self.test_results_at_tail = true;
                self.test_results_scroll = 0;
                self.completion = None;
                self.hover = None;
                self.signature_help = None;
                self.whichkey = None;
                self.status_msg = format!("test: running {label}…");
            }
            Err(e) => {
                self.status_msg = format!("test: {e}");
            }
        }
    }

    /// Picker accept handler for `PickerKind::TestTarget`. Builds a
    /// run request with the picked test's name as the libtest filter
    /// and routes through `test_kickoff`.
    pub(super) fn test_run_picked_target(&mut self, adapter_key: String, name: String) {
        let Some((spec, root)) = self.test_resolve_adapter() else {
            self.status_msg = "test: no adapter for this workspace".into();
            return;
        };
        if spec.key != adapter_key {
            self.status_msg = format!(
                "test: workspace adapter changed ({} → {}), retry",
                adapter_key, spec.key
            );
            return;
        }
        let label = format!("picked: {name}");
        let req = TestRunRequest {
            filter: Some(name),
            workspace_root: root,
            label,
        };
        self.test_kickoff(&spec, req);
    }

    /// Main-loop event-handler counterpart. Folds streamed events
    /// into the status line, opens the overlay on first event, and
    /// loads quickfix on completion.
    pub(super) fn handle_test_events(&mut self, events: Vec<TestEvent>) {
        let mut finished_with_summary: Option<TestSummary> = None;
        let mut saw_aborted: Option<String> = None;
        for ev in events {
            match ev {
                TestEvent::Started { command_line, .. } => {
                    self.status_msg = format!("test: $ {command_line}");
                }
                TestEvent::Case { status, .. } => {
                    // Per-case status updates are noisy on the status
                    // line — the overlay already shows them. We only
                    // surface the live summary count on Finished.
                    let _ = status;
                }
                TestEvent::Output { .. } => {}
                TestEvent::Finished { summary } => {
                    finished_with_summary = Some(summary);
                }
                TestEvent::Aborted { message } => {
                    saw_aborted = Some(message);
                }
            }
        }
        if let Some(msg) = saw_aborted {
            self.status_msg = format!("test: aborted — {msg}");
        } else if let Some(summary) = finished_with_summary {
            self.status_msg = format!(
                "test: {} passed, {} failed, {} ignored{}",
                summary.passed,
                summary.failed,
                summary.ignored,
                if summary.filtered_out > 0 {
                    format!(" ({} filtered)", summary.filtered_out)
                } else {
                    String::new()
                },
            );
            if summary.failed > 0 {
                self.qf_load_from_test_failures();
            }
        }
    }

    /// Build a quickfix list from the most recent run's captured
    /// failures. Replaces any existing quickfix list — fresh test
    /// failures supersede stale grep / references / diagnostics
    /// entries (matches Vim's `:make` flow). When a failure lacks a
    /// parsed location, the entry points at the active buffer line 1
    /// so the user can still walk the list without it being broken.
    fn qf_load_from_test_failures(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let fallback_path = self.buffer.path.clone().unwrap_or_else(|| cwd.clone());
        let entries: Vec<QuickfixEntry> = self
            .test
            .failures
            .iter()
            .map(|f| {
                let (path, line, col) = match &f.location {
                    Some(loc) => {
                        // libtest may print a path relative to the
                        // workspace root; resolve to an absolute path
                        // so `qf_jump_current` can open it from any
                        // cwd.
                        let abs = if loc.path.is_absolute() {
                            loc.path.clone()
                        } else {
                            let root = self
                                .test
                                .last_run
                                .as_ref()
                                .map(|r| r.workspace_root.clone())
                                .unwrap_or_else(|| cwd.clone());
                            root.join(&loc.path)
                        };
                        (abs, loc.line, loc.col)
                    }
                    None => (fallback_path.clone(), 1, 1),
                };
                let text = match &f.message {
                    Some(m) => format!("{}: {}", f.name, m),
                    None => f.name.clone(),
                };
                QuickfixEntry {
                    path,
                    line,
                    col,
                    text,
                }
            })
            .collect();
        if entries.is_empty() {
            return;
        }
        let n = entries.len();
        self.quickfix = Some(QuickfixState { entries, current: 0 });
        // Don't auto-jump — the user has the overlay open, and yanking
        // them out of it on completion is jarring. They can hit `]q`
        // when ready.
        self.status_msg = format!(
            "{} test failure{} loaded into quickfix",
            n,
            if n == 1 { "" } else { "s" }
        );
    }

    /// Scrollback bound for the results overlay — matches the shape
    /// of `health_max_scroll` / `messages_max_scroll`.
    pub(super) fn test_results_max_scroll(&self) -> usize {
        let rows = self.buffer_rows();
        let viewport = rows.saturating_sub(1);
        self.test_results_content_height
            .get()
            .saturating_sub(viewport)
    }

    /// User-initiated scroll. Tail-follow mode is interactive: any
    /// upward scroll drops us out of it (so newly-arriving events
    /// don't yank the viewport away while the user's reading
    /// scrollback); any downward scroll that reaches the bottom
    /// re-engages it (so the user gets back to live-tail without
    /// having to press `G`).
    pub(super) fn test_results_scroll_by(&mut self, delta: isize) {
        let max = self.test_results_max_scroll();
        if delta < 0 {
            // Up. If we were tailing, seed scroll from the live
            // bottom so the user keeps reading from where they were.
            if self.test_results_at_tail {
                self.test_results_at_tail = false;
                self.test_results_scroll = max;
            }
            let cur = self.test_results_scroll as isize;
            let next = (cur + delta).max(0) as usize;
            self.test_results_scroll = next.min(max);
        } else if delta > 0 {
            if self.test_results_at_tail {
                // Already pinned at the bottom — nothing to do.
                return;
            }
            let cur = self.test_results_scroll as isize;
            let next = ((cur + delta).max(0) as usize).min(max);
            self.test_results_scroll = next;
            // Reached the bottom — re-engage tail follow so future
            // streamed events keep the viewport live.
            if next >= max {
                self.test_results_at_tail = true;
            }
        }
    }

    /// Convenience predicate matching `TestStatus::Failed` — used by
    /// the renderer (which can't depend on the test types module
    /// directly because of orphan rules around foreign trait impls).
    #[allow(dead_code)]
    pub(super) fn test_status_is_failed(status: TestStatus) -> bool {
        matches!(status, TestStatus::Failed)
    }
}
