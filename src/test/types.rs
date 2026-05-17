//! Wire-side types for the test runner. Adapter-agnostic.
//!
//! A run is a single invocation of a test-adapter command (e.g. `cargo
//! test foo -- --nocapture`). The reader thread parses adapter stdout
//! line-by-line into `TestEvent`s and pushes them onto a channel; the
//! manager drains the channel on every main-loop tick and forwards the
//! events to `app/test_glue.rs` for UI mutation.
//!
//! Per-adapter parsing logic lives in `test/cargo.rs` etc. — those are
//! the only places that know about the underlying tool's output shape.

use std::path::PathBuf;
use std::time::Instant;

/// Pass/fail/ignored verdict for one test case. Kept simple — adapters
/// with more states (e.g. "flaky", "retried") flatten down to one of
/// these three plus a free-form `message`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestStatus {
    Passed,
    Failed,
    Ignored,
}

impl TestStatus {
    /// Three-letter overlay glyph for this status. Currently used only
    /// for tests / future status-line summaries — the renderer inlines
    /// its own five-letter variant ("PASS "/"FAIL "/"SKIP ") because
    /// it pads to the same width.
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            TestStatus::Passed => "PASS",
            TestStatus::Failed => "FAIL",
            TestStatus::Ignored => "SKIP",
        }
    }
}

/// One streamed event from a running adapter. Renders into the overlay
/// and (on `Finished` with failures) populates the quickfix list.
#[derive(Debug, Clone)]
pub enum TestEvent {
    /// Adapter has started — the command line and human-friendly label
    /// to display in the overlay header. Fires once per run.
    Started {
        #[allow(dead_code)]
        adapter_key: String,
        command_line: String,
    },
    /// A single test case completed with the given verdict. `name` is
    /// the adapter's canonical test name (e.g. `motion::tests::foo`);
    /// `message` carries any tool-emitted detail (panic text, ignored
    /// reason, …).
    Case {
        name: String,
        status: TestStatus,
        /// Optional file/line/col extracted by the adapter parser from
        /// panic output (e.g. `panicked at src/foo.rs:123:45`). Used to
        /// populate quickfix entries when the run finishes with
        /// failures.
        location: Option<TestLocation>,
        message: Option<String>,
    },
    /// Raw output line from the adapter (stdout or stderr) — surfaced
    /// in the overlay so the user can see `println!` debug prints and
    /// adapter chatter (`Compiling …`, `Finished …`).
    Output {
        stream: OutputStream,
        text: String,
    },
    /// Adapter has finished. `summary` is best-effort — adapters that
    /// don't emit a tally just leave the counts at zero.
    Finished { summary: TestSummary },
    /// Adapter exited with a non-zero status before producing a
    /// `Finished` event (e.g. compilation failed for `cargo test`).
    /// `message` is the captured stderr tail when available.
    Aborted { message: String },
}

/// stdout vs stderr — drives colour in the overlay rendering. Most
/// adapters print pass/fail tallies on stdout and compile errors on
/// stderr, so the distinction is worth preserving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// Per-case failure location extracted from adapter output. Coords are
/// 1-indexed (what the underlying tool prints, and what quickfix
/// expects).
#[derive(Debug, Clone)]
pub struct TestLocation {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
}

/// End-of-run tally. All counts default to 0 so a partial parse still
/// renders cleanly. `total` may be greater than `passed + failed +
/// ignored` when the adapter filtered out tests (which counts as
/// neither pass nor fail).
#[derive(Debug, Clone, Default)]
pub struct TestSummary {
    pub passed: usize,
    pub failed: usize,
    pub ignored: usize,
    pub filtered_out: usize,
}

impl TestSummary {
    pub fn add(&mut self, other: &TestSummary) {
        self.passed += other.passed;
        self.failed += other.failed;
        self.ignored += other.ignored;
        self.filtered_out += other.filtered_out;
    }
}

/// One row in the streaming output overlay. Built from the `TestEvent`
/// stream as it arrives, retained in `TestManager.output_buffer` so the
/// user can scroll back through the run after it finishes.
#[derive(Debug, Clone)]
pub enum TestOutputRow {
    /// Header — bold title at the top of the overlay.
    Header {
        command_line: String,
        #[allow(dead_code)]
        started_at: Instant,
    },
    /// A single completed test case.
    Case {
        name: String,
        status: TestStatus,
        message: Option<String>,
    },
    /// Raw adapter output.
    Output {
        stream: OutputStream,
        text: String,
    },
    /// End-of-run tally line.
    Summary(TestSummary),
    /// Adapter failed to run — compile error, command not found, etc.
    Aborted(String),
}

/// One captured failure case — enough to construct a quickfix entry. We
/// hold these on the manager rather than constructing the quickfix list
/// directly so the orchestration layer (which owns App) gets to decide
/// when (and whether) to replace the user's current quickfix list.
#[derive(Debug, Clone)]
pub struct TestFailure {
    pub name: String,
    pub location: Option<TestLocation>,
    pub message: Option<String>,
}

/// Resolved invocation — the precise command the adapter wants to run.
/// Built by the adapter's `build_run_command` from a `TestRunRequest`.
#[derive(Debug, Clone)]
pub struct ResolvedCommand {
    /// Program to invoke (e.g. `cargo`).
    pub program: String,
    pub args: Vec<String>,
    /// Working directory for the spawned process.
    pub cwd: PathBuf,
    /// Human-readable command line for the overlay header. Includes
    /// program + args joined by spaces.
    pub display: String,
}

/// What the orchestration layer asked the adapter to run. Adapter-
/// agnostic: each adapter's `build_run_command` translates this into a
/// concrete `ResolvedCommand`.
#[derive(Debug, Clone)]
pub struct TestRunRequest {
    /// Substring filter on the test name. `None` means "run all
    /// reachable tests."
    pub filter: Option<String>,
    /// Workspace root the run should execute against. Resolved by the
    /// orchestration layer via the adapter's `root_markers`.
    pub workspace_root: PathBuf,
    /// Human label for status messages — typically "nearest test" /
    /// "file" / "all" / a picked test name.
    pub label: String,
}
