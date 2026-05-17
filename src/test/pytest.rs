//! `pytest` adapter — discovery, run-command construction, and the
//! line-by-line parser that turns pytest's verbose output into
//! `TestEvent`s. Structurally parallel to `cargo.rs` and `vitest.rs`.
//!
//! Pytest invocation flags worth knowing:
//!
//! * `-v` puts one line per test on stdout in the form
//!   `path/to/test_foo.py::test_bar PASSED [ 50%]`.
//! * `--tb=line` collapses each failing test's traceback onto a single
//!   line: `/abs/path/test_foo.py:14: AssertionError: expected …`.
//!   That's how the parser pulls failure locations.
//! * `--no-header` drops pytest's banner, `--color=no` drops ANSI
//!   sequences so the streaming parser sees plain text.
//! * `-k <expr>` is a substring filter against test names — used for
//!   `:testnearest` style runs. File-scoped runs pass the path
//!   positionally instead, which is more selective.

use std::path::{Path, PathBuf};

use super::types::{
    ResolvedCommand, TestEvent, TestLocation, TestRunRequest, TestStatus, TestSummary,
};

/// State threaded across `parse_event_line` invocations. Pytest
/// emits the case verdict and the failure detail on different lines,
/// so the parser holds the most-recent failing case name plus a
/// name→location / name→message map populated when the FAILURES /
/// short-summary blocks stream through later.
#[derive(Debug, Default)]
pub struct PytestParseState {
    /// `name → location`. Filled from `--tb=line` rows that look like
    /// `<path>:<line>: ExceptionName: msg`. Keyed by the most-recent
    /// `path::test_name` seen.
    pub failure_locations: std::collections::HashMap<String, TestLocation>,
    /// `name → first error message`. Filled from the short summary
    /// info block (`FAILED path::test - ExceptionName: msg`).
    pub failure_messages: std::collections::HashMap<String, String>,
    /// Most recent test that emitted a `FAILED` verdict — used so the
    /// indented `--tb=line` row below it attaches to the right case.
    pub current_failure_name: Option<String>,
    /// Final summary parsed from a `===== N passed, M failed in T s =====`
    /// line. Re-emitted by `flush_parser` so the contract matches the
    /// cargo / vitest adapters.
    pub rolling: TestSummary,
}

/// `pytest --collect-only -q` writes one `path::test_name` per line
/// (plus a trailing tally line we filter out). No build / compile
/// step required.
pub fn build_list_command(root: &Path) -> Option<ResolvedCommand> {
    let args: Vec<String> = vec![
        "--collect-only".into(),
        "-q".into(),
        "--no-header".into(),
        "--color=no".into(),
    ];
    let display = format!("pytest {}", args.join(" "));
    Some(ResolvedCommand {
        program: "pytest".into(),
        args,
        cwd: root.to_path_buf(),
        display,
    })
}

pub fn parse_list_output(stdout: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Pytest's `-q` collect output looks like `path/to/test.py::test_name`
        // — the only reliable signal. The tail lines ("N tests collected
        // in 0.01s", "no tests ran in 0.01s") get filtered by the same
        // `::` check.
        if !trimmed.contains("::") {
            continue;
        }
        out.push(trimmed.to_string());
    }
    out.sort();
    out.dedup();
    out
}

pub fn build_run_command(req: &TestRunRequest) -> Result<ResolvedCommand, String> {
    let mut args: Vec<String> = vec![
        "-v".into(),
        "--no-header".into(),
        "--tb=line".into(),
        "--color=no".into(),
    ];
    if let Some(filter) = req.filter.as_ref() {
        if !filter.is_empty() {
            // Heuristic: anything that looks like a path / nodeid goes
            // positionally (pytest treats those as collection roots);
            // a bare word goes through `-k <expr>` for substring
            // matching against test names.
            if looks_like_path_filter(filter) {
                args.push(filter.clone());
            } else {
                args.push("-k".into());
                args.push(filter.clone());
            }
        }
    }
    let display = format!("pytest {}", args.join(" "));
    Ok(ResolvedCommand {
        program: "pytest".into(),
        args,
        cwd: req.workspace_root.clone(),
        display,
    })
}

fn looks_like_path_filter(s: &str) -> bool {
    s.contains('/') || s.ends_with(".py") || s.contains("::")
}

pub fn parse_event_line(line: &str, state: &mut super::specs::LineParseState) -> Vec<TestEvent> {
    let s = &mut state.pytest;
    let mut events = Vec::new();

    let trimmed = line.trim_end();

    // 1. Per-test verdict — `path::test_name PASSED [ 50%]`.
    //    Strip the trailing `[ NN%]` progress indicator (which pytest
    //    only emits in non-TTY mode if a terminal width is detected,
    //    but we play it safe).
    let (head, status) = parse_verdict(trimmed);
    if let (Some(name), Some(status)) = (head, status) {
        if status == TestStatus::Failed {
            s.current_failure_name = Some(name.clone());
        } else {
            s.current_failure_name = None;
        }
        events.push(TestEvent::Case {
            name,
            status,
            location: None,
            message: None,
        });
        return events;
    }

    // 2. `--tb=line` failure row: `path:line: ExceptionName: msg`. We
    //    attach it to the most-recent failing case (set in step 1).
    if let Some(loc) = parse_tb_line(trimmed) {
        if let Some(name) = s.current_failure_name.clone() {
            // Pull the message off the same line — anything after the
            // `path:line: ` prefix is the exception text.
            let msg = extract_tb_message(trimmed);
            s.failure_locations.insert(name.clone(), loc);
            if let Some(m) = msg {
                s.failure_messages.entry(name).or_insert(m);
            }
        }
        return events;
    }

    // 3. Short test summary block — `FAILED path::name - ExceptionName: msg`.
    //    Captures a fallback message when `--tb=line` failed to attach
    //    one (e.g. multi-line ExceptionGroup output).
    if let Some(rest) = trimmed.strip_prefix("FAILED ") {
        if let Some((name, msg)) = rest.split_once(" - ") {
            let name = name.trim().to_string();
            s.failure_messages
                .entry(name)
                .or_insert_with(|| msg.trim().to_string());
        }
        return events;
    }

    // 4. Final summary — surrounded by `=` rune. Examples:
    //    `==== 1 failed, 15 passed in 0.42s ====`
    //    `==== 16 passed in 0.42s ====`
    //    `==== 1 failed, 1 passed, 1 skipped in 0.42s ====`
    //    `==== 1 passed, 2 warnings in 0.42s ====`
    if let Some(summary) = parse_summary_line(trimmed) {
        s.rolling = summary;
        return events;
    }

    events
}

pub fn flush_parser(state: &mut super::specs::LineParseState) -> Vec<TestEvent> {
    let s = &mut state.pytest;
    let mut events = Vec::new();
    // Re-emit each failing test with its location + message attached
    // so the orchestration layer can populate quickfix. Mirrors the
    // vitest flush — duplicating the case events is harmless because
    // the orchestration layer reads failure data off
    // `TestManager.failures`, not the live event stream.
    let names: Vec<String> = s.failure_locations.keys().cloned().collect();
    for name in names {
        let location = s.failure_locations.remove(&name);
        let message = s.failure_messages.remove(&name);
        events.push(TestEvent::Case {
            name,
            status: TestStatus::Failed,
            location,
            message,
        });
    }
    let summary = std::mem::take(&mut s.rolling);
    events.push(TestEvent::Finished { summary });
    events
}

/// `:testfile` — pass the file path verbatim. Pytest accepts any
/// collection root positionally, so a single file is fine.
pub fn filter_for_file(file: &Path, root: &Path) -> Option<String> {
    let name = file.file_name().and_then(|s| s.to_str())?;
    if !is_test_filename(name) {
        return None;
    }
    let abs = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let root_abs = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let rel = abs.strip_prefix(&root_abs).unwrap_or(&abs);
    Some(rel.to_string_lossy().to_string())
}

/// `:testnearest` — walk upward for the closest `def test_<name>` (or
/// `async def test_<name>`). Class-based tests are flat enough that
/// `-k <method_name>` picks them up too; we don't try to combine
/// class + method.
pub fn filter_for_nearest(buffer_text: &str, cursor_line: usize) -> Option<String> {
    let lines: Vec<&str> = buffer_text.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let cursor = cursor_line.min(lines.len() - 1);
    for i in (0..=cursor).rev() {
        if let Some(name) = parse_test_def(lines[i]) {
            return Some(name);
        }
    }
    None
}

fn parse_test_def(line: &str) -> Option<String> {
    let stripped = line.trim_start();
    let rest = stripped
        .strip_prefix("async def ")
        .or_else(|| stripped.strip_prefix("def "))?;
    let name_end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    let name = &rest[..name_end];
    if !name.starts_with("test") {
        return None;
    }
    Some(name.to_string())
}

fn is_test_filename(name: &str) -> bool {
    name.starts_with("test_") && name.ends_with(".py")
        || name.ends_with("_test.py")
        || name == "conftest.py"
}

fn parse_verdict(line: &str) -> (Option<String>, Option<TestStatus>) {
    // Strip a trailing ` [NN%]` progress tag if present.
    let cleaned = strip_progress_tag(line);
    // The verdict is the last whitespace-separated token on the line.
    let cleaned = cleaned.trim_end();
    for (verdict, status) in [
        ("PASSED", TestStatus::Passed),
        ("FAILED", TestStatus::Failed),
        ("ERROR", TestStatus::Failed),
        ("SKIPPED", TestStatus::Ignored),
        ("XFAIL", TestStatus::Ignored),
        ("XPASS", TestStatus::Passed),
    ] {
        if let Some(head) = cleaned.strip_suffix(verdict) {
            let head = head.trim_end();
            if head.contains("::") {
                return (Some(head.to_string()), Some(status));
            }
        }
    }
    (None, None)
}

fn strip_progress_tag(line: &str) -> &str {
    let trimmed = line.trim_end();
    if !trimmed.ends_with(']') {
        return trimmed;
    }
    // Find the `[NN%]` block start. Bail on any malformed sequence.
    let Some(open) = trimmed.rfind('[') else { return trimmed };
    let between = &trimmed[open + 1..trimmed.len() - 1];
    if between.ends_with('%')
        && between[..between.len() - 1].trim().chars().all(|c| c.is_ascii_digit() || c == ' ')
    {
        trimmed[..open].trim_end()
    } else {
        trimmed
    }
}

fn parse_tb_line(line: &str) -> Option<TestLocation> {
    // `--tb=line` format: `<path>:<line>: ExceptionName: msg`.
    // Reject anything that doesn't have at least two colons in the
    // first 200 chars to keep accidental matches against random log
    // output cheap.
    let first_colon = line.find(':')?;
    let after_path = &line[first_colon + 1..];
    let second_colon = after_path.find(':')?;
    let line_no_s = &after_path[..second_colon];
    let line_no: usize = line_no_s.parse().ok()?;
    let path = line[..first_colon].trim();
    if path.is_empty() || !looks_like_python_path(path) {
        return None;
    }
    Some(TestLocation {
        path: PathBuf::from(path),
        line: line_no,
        col: 1,
    })
}

fn looks_like_python_path(s: &str) -> bool {
    s.ends_with(".py") && !s.contains(' ')
}

fn extract_tb_message(line: &str) -> Option<String> {
    // After the second colon comes ` ExceptionName: msg` (or sometimes
    // just ` msg` for raw assertions). Return the trimmed remainder.
    let first_colon = line.find(':')?;
    let rest = &line[first_colon + 1..];
    let second_colon = rest.find(':')?;
    let tail = rest[second_colon + 1..].trim();
    if tail.is_empty() {
        None
    } else {
        Some(tail.to_string())
    }
}

fn parse_summary_line(line: &str) -> Option<TestSummary> {
    // Pytest wraps the summary in `=` runes; strip leading / trailing
    // sequences and the trailing duration before counting tokens.
    let stripped = line.trim_matches(|c: char| c == '=' || c.is_whitespace());
    // The body must mention `in <duration>s` — keeps random `===` lines
    // out of the summary parser.
    let (body, _duration) = stripped.rsplit_once(" in ")?;
    let mut summary = TestSummary::default();
    let mut saw_any = false;
    for chunk in body.split(',') {
        let chunk = chunk.trim();
        let (num_s, label) = chunk.split_once(' ')?;
        let n: usize = num_s.parse().ok()?;
        match label.trim() {
            l if l.starts_with("passed") => {
                summary.passed += n;
                saw_any = true;
            }
            l if l.starts_with("failed") || l.starts_with("error") => {
                summary.failed += n;
                saw_any = true;
            }
            l if l.starts_with("skipped") || l.starts_with("xfailed") => {
                summary.ignored += n;
                saw_any = true;
            }
            _ => {}
        }
    }
    if saw_any { Some(summary) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::specs::LineParseState;

    fn parse_lines(lines: &[&str]) -> Vec<TestEvent> {
        let mut state = LineParseState::default();
        let mut events = Vec::new();
        for l in lines {
            events.extend(parse_event_line(l, &mut state));
        }
        events.extend(flush_parser(&mut state));
        events
    }

    #[test]
    fn pass_line_emits_case_event() {
        let events = parse_lines(&["tests/test_foo.py::test_bar PASSED                                    [ 50%]"]);
        let cases: Vec<&TestEvent> = events
            .iter()
            .filter(|e| matches!(e, TestEvent::Case { status: TestStatus::Passed, .. }))
            .collect();
        assert_eq!(cases.len(), 1);
        if let TestEvent::Case { name, .. } = cases[0] {
            assert_eq!(name, "tests/test_foo.py::test_bar");
        }
    }

    #[test]
    fn fail_line_with_decorated_location_on_flush() {
        let events = parse_lines(&[
            "tests/test_foo.py::test_bar FAILED                                    [100%]",
            "tests/test_foo.py:14: AssertionError: assert 1 == 2",
            "=========================== short test summary info ===========================",
            "FAILED tests/test_foo.py::test_bar - AssertionError: assert 1 == 2",
            "=========================== 1 failed, 15 passed in 0.42s ===========================",
        ]);
        let decorated: Vec<&TestEvent> = events
            .iter()
            .filter(|e| matches!(e, TestEvent::Case { status: TestStatus::Failed, location: Some(_), .. }))
            .collect();
        assert_eq!(decorated.len(), 1);
        if let TestEvent::Case { location: Some(loc), message, .. } = decorated[0] {
            assert_eq!(loc.path, PathBuf::from("tests/test_foo.py"));
            assert_eq!(loc.line, 14);
            assert!(message.as_deref().unwrap_or("").contains("AssertionError"));
        } else {
            panic!("expected decorated failure with location");
        }
    }

    #[test]
    fn summary_line_aggregated_into_finished() {
        let events = parse_lines(&[
            "==== 1 failed, 15 passed, 1 skipped in 0.42s ====",
        ]);
        match events.last().unwrap() {
            TestEvent::Finished { summary } => {
                assert_eq!(summary.failed, 1);
                assert_eq!(summary.passed, 15);
                assert_eq!(summary.ignored, 1);
            }
            _ => panic!("last event should be Finished"),
        }
    }

    #[test]
    fn list_parse_keeps_node_ids() {
        let raw = "\
tests/test_a.py::test_one
tests/test_a.py::test_two
tests/test_b.py::TestClass::test_method

2 tests collected in 0.01s
";
        let mut names = parse_list_output(raw);
        names.sort();
        assert_eq!(
            names,
            vec![
                "tests/test_a.py::test_one",
                "tests/test_a.py::test_two",
                "tests/test_b.py::TestClass::test_method",
            ],
        );
    }

    #[test]
    fn filter_for_nearest_walks_to_test_def() {
        let src = "import pytest\n\ndef test_slugify():\n    assert True\n    # cursor here\n";
        let got = filter_for_nearest(src, 4);
        assert_eq!(got.as_deref(), Some("test_slugify"));
    }

    #[test]
    fn filter_for_nearest_handles_async_def() {
        let src = "async def test_async_path():\n    pass\n";
        let got = filter_for_nearest(src, 1);
        assert_eq!(got.as_deref(), Some("test_async_path"));
    }

    #[test]
    fn filter_for_nearest_skips_non_test_def() {
        let src = "def helper():\n    return 1\n\ndef test_real():\n    pass\n";
        // Cursor inside the helper (line 1) — no test_ function above.
        assert_eq!(filter_for_nearest(src, 1), None);
    }

    #[test]
    fn filter_for_file_only_matches_test_files() {
        let tmp = std::env::temp_dir().join("binvim-pytest-filter-for-file");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("sub")).unwrap();
        std::fs::write(tmp.join("test_foo.py"), "").unwrap();
        std::fs::write(tmp.join("bar.py"), "").unwrap();
        std::fs::write(tmp.join("sub/baz_test.py"), "").unwrap();

        assert_eq!(
            filter_for_file(&tmp.join("test_foo.py"), &tmp).as_deref(),
            Some("test_foo.py"),
        );
        assert_eq!(filter_for_file(&tmp.join("bar.py"), &tmp), None);
        assert_eq!(
            filter_for_file(&tmp.join("sub/baz_test.py"), &tmp).as_deref(),
            Some("sub/baz_test.py"),
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn build_run_command_routes_path_filter_positionally() {
        let req = TestRunRequest {
            filter: Some("tests/test_foo.py".into()),
            workspace_root: PathBuf::from("/tmp"),
            label: "file".into(),
        };
        let cmd = build_run_command(&req).unwrap();
        assert!(cmd.args.iter().any(|a| a == "tests/test_foo.py"));
        assert!(!cmd.args.iter().any(|a| a == "-k"));
    }

    #[test]
    fn build_run_command_routes_name_filter_via_dash_k() {
        let req = TestRunRequest {
            filter: Some("test_slugify".into()),
            workspace_root: PathBuf::from("/tmp"),
            label: "nearest".into(),
        };
        let cmd = build_run_command(&req).unwrap();
        let k_idx = cmd.args.iter().position(|a| a == "-k").expect("expected -k flag");
        assert_eq!(cmd.args[k_idx + 1], "test_slugify");
    }

    #[test]
    fn parse_verdict_recognises_xpass_xfail() {
        let (n, s) = parse_verdict("tests/test_foo.py::test_bar XPASS");
        assert_eq!(n.as_deref(), Some("tests/test_foo.py::test_bar"));
        assert_eq!(s, Some(TestStatus::Passed));
        let (n, s) = parse_verdict("tests/test_foo.py::test_bar XFAIL");
        assert_eq!(n.as_deref(), Some("tests/test_foo.py::test_bar"));
        assert_eq!(s, Some(TestStatus::Ignored));
    }
}
