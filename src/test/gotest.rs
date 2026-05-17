//! `go test` adapter — discovery, run-command construction, and the
//! line-by-line parser that turns `go test -v` output into
//! `TestEvent`s. Structurally parallel to `cargo.rs` / `vitest.rs` /
//! `pytest.rs`; the framework is shared.
//!
//! go test's verbose output looks like:
//!
//! ```text
//! === RUN   TestSlugify
//! --- PASS: TestSlugify (0.00s)
//! === RUN   TestStripsPunctuation
//!     foo_test.go:14: expected its-a-test, got its-a-test!
//! --- FAIL: TestStripsPunctuation (0.00s)
//! FAIL
//! exit status 1
//! FAIL    example.com/foo  0.123s
//! ```
//!
//! Subtests (`t.Run("case", …)`) print as `TestParent/case`. The
//! parser keeps them whole so the user's `-run` filter (regex) can
//! match the nested name verbatim.

use std::path::{Path, PathBuf};

use super::types::{
    ResolvedCommand, TestEvent, TestLocation, TestRunRequest, TestStatus, TestSummary,
};

/// State threaded across `parse_event_line` invocations. Holds the
/// name of the most-recent `=== RUN` so the indented diagnostic
/// lines below it (`    file_test.go:14: msg`) can be attributed to
/// the right case before the `--- FAIL` row closes it out.
#[derive(Debug, Default)]
pub struct GoTestParseState {
    /// Name of the test currently running. Updated on `=== RUN` and
    /// cleared when its `--- PASS/FAIL/SKIP` row arrives.
    pub current_test: Option<String>,
    /// `name → location` from indented `    file:line: msg` lines.
    pub failure_locations: std::collections::HashMap<String, TestLocation>,
    /// `name → first message` from the same indented lines.
    pub failure_messages: std::collections::HashMap<String, String>,
    /// Accumulated counts; go test doesn't print a global tally so
    /// we tick this on each case verdict.
    pub rolling: TestSummary,
}

/// `go test -list .* ./...` enumerates every test name reachable from
/// the workspace root. Output is per-package, with `ok <pkg>` /
/// `FAIL <pkg>` summary rows interleaved; the parser filters those.
pub fn build_list_command(root: &Path) -> Option<ResolvedCommand> {
    let args: Vec<String> = vec![
        "test".into(),
        "-list".into(),
        ".*".into(),
        "./...".into(),
    ];
    let display = format!("go {}", args.join(" "));
    Some(ResolvedCommand {
        program: "go".into(),
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
        // Skip non-test rows: per-package `ok ...` / `FAIL ...` /
        // `?  package [no test files]` chatter.
        if trimmed.starts_with("ok ")
            || trimmed.starts_with("FAIL\t")
            || trimmed.starts_with("FAIL ")
            || trimmed.starts_with("?\t")
            || trimmed.starts_with("? ")
            || trimmed.starts_with("# ")
        {
            continue;
        }
        // Real test names start with `Test`, `Benchmark`, `Example`,
        // or `Fuzz` (the four go test recognises). Subtests would be
        // reported separately when actually run, not in the list.
        if trimmed.starts_with("Test")
            || trimmed.starts_with("Benchmark")
            || trimmed.starts_with("Example")
            || trimmed.starts_with("Fuzz")
        {
            out.push(trimmed.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

pub fn build_run_command(req: &TestRunRequest) -> Result<ResolvedCommand, String> {
    let mut args: Vec<String> = vec!["test".into(), "-v".into()];
    if let Some(filter) = req.filter.as_ref() {
        if !filter.is_empty() {
            // Decide between `-run <pattern>` (name filter) and a
            // positional package selector (file / directory filter).
            // `-run` accepts a regex; we anchor the filter so a name
            // that matches a prefix doesn't accidentally fire other
            // tests too.
            if looks_like_package_filter(filter) {
                args.push(filter.clone());
            } else {
                args.push("-run".into());
                args.push(format!("^{}$", regex_escape(filter)));
            }
        }
    }
    // No filter → run everything reachable from the workspace.
    if !args.iter().any(|a| a.starts_with("./") || a.contains('/')) {
        args.push("./...".into());
    }
    let display = format!("go {}", args.join(" "));
    Ok(ResolvedCommand {
        program: "go".into(),
        args,
        cwd: req.workspace_root.clone(),
        display,
    })
}

fn looks_like_package_filter(s: &str) -> bool {
    s.starts_with("./") || s.starts_with('/') || s.contains("...")
}

/// Minimal regex-escape for the `-run` flag. We only escape the
/// metacharacters that show up in real go test names — `/` for
/// subtests is the common one, plus the ASCII punctuation that's
/// rarely but legally part of a name.
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        match c {
            '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$'
            | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

pub fn parse_event_line(line: &str, state: &mut super::specs::LineParseState) -> Vec<TestEvent> {
    let s = &mut state.gotest;
    let mut events = Vec::new();
    let trimmed = line.trim_end();

    // 1. `=== RUN   <name>` — arm the parser for indented diagnostic
    //    lines below.
    if let Some(rest) = trimmed.strip_prefix("=== RUN") {
        let name = rest.trim();
        if !name.is_empty() {
            s.current_test = Some(name.to_string());
        }
        return events;
    }
    // `=== PAUSE` / `=== CONT` interleave when -parallel runs the same
    // test concurrently; we ignore them (verdict comes from --- ...).
    if trimmed.starts_with("=== ") {
        return events;
    }

    // 2. `--- PASS: <name> (Ns)` / `--- FAIL: <name>` / `--- SKIP: <name>`.
    if let Some(rest) = trimmed.strip_prefix("--- PASS:") {
        if let Some(name) = parse_verdict_name(rest) {
            s.current_test = None;
            s.rolling.passed += 1;
            events.push(TestEvent::Case {
                name,
                status: TestStatus::Passed,
                location: None,
                message: None,
            });
        }
        return events;
    }
    if let Some(rest) = trimmed.strip_prefix("--- FAIL:") {
        if let Some(name) = parse_verdict_name(rest) {
            // The location + message were captured on the indented
            // lines above; emit a plain Case here and rely on flush
            // to re-emit the decorated copy.
            s.current_test = None;
            s.rolling.failed += 1;
            events.push(TestEvent::Case {
                name,
                status: TestStatus::Failed,
                location: None,
                message: None,
            });
        }
        return events;
    }
    if let Some(rest) = trimmed.strip_prefix("--- SKIP:") {
        if let Some(name) = parse_verdict_name(rest) {
            s.current_test = None;
            s.rolling.ignored += 1;
            events.push(TestEvent::Case {
                name,
                status: TestStatus::Ignored,
                location: None,
                message: None,
            });
        }
        return events;
    }

    // 3. Indented diagnostic line: `    foo_test.go:14: expected …`.
    //    Attribute to the most-recent `=== RUN` test. Only the first
    //    location per test is captured (later lines often print
    //    follow-up assertion detail).
    if let Some(stripped) = trimmed.strip_prefix("    ") {
        if let (Some(name), Some(loc)) =
            (s.current_test.clone(), parse_indented_location(stripped))
        {
            s.failure_locations.entry(name.clone()).or_insert(loc);
            if let Some(msg) = indented_message(stripped) {
                s.failure_messages.entry(name).or_insert(msg);
            }
        }
        return events;
    }

    events
}

pub fn flush_parser(state: &mut super::specs::LineParseState) -> Vec<TestEvent> {
    let s = &mut state.gotest;
    let mut events = Vec::new();
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

/// `:testfile` — run the active file's enclosing package. go test is
/// package-scoped, so the granularity is "the directory the file
/// lives in." Returns the relative directory path under the
/// workspace root, prefixed with `./` so go interprets it as a
/// local-package selector instead of a module path.
pub fn filter_for_file(file: &Path, root: &Path) -> Option<String> {
    let name = file.file_name().and_then(|s| s.to_str())?;
    if !name.ends_with("_test.go") {
        return None;
    }
    let dir = file.parent()?;
    let abs = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let root_abs = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let rel = abs.strip_prefix(&root_abs).unwrap_or(&abs);
    let rel_s = rel.to_string_lossy();
    if rel_s.is_empty() {
        Some("./".into())
    } else {
        Some(format!("./{rel_s}"))
    }
}

/// `:testnearest` — walk upward for the closest `func Test<Name>(…) {`,
/// `func Benchmark<Name>`, `func Example<Name>`, or `func Fuzz<Name>`.
/// Subtests defined via `t.Run("case", …)` are still reachable by
/// running the parent test; we don't try to compose the nested name.
pub fn filter_for_nearest(buffer_text: &str, cursor_line: usize) -> Option<String> {
    let lines: Vec<&str> = buffer_text.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let cursor = cursor_line.min(lines.len() - 1);
    for i in (0..=cursor).rev() {
        if let Some(name) = parse_test_func(lines[i]) {
            return Some(name);
        }
    }
    None
}

fn parse_test_func(line: &str) -> Option<String> {
    let stripped = line.trim_start();
    let rest = stripped.strip_prefix("func ")?;
    let name_end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    let name = &rest[..name_end];
    if !(name.starts_with("Test")
        || name.starts_with("Benchmark")
        || name.starts_with("Example")
        || name.starts_with("Fuzz"))
    {
        return None;
    }
    Some(name.to_string())
}

fn parse_verdict_name(rest: &str) -> Option<String> {
    // Input shapes:
    //   ` TestFoo (0.00s)`
    //   ` TestFoo/sub_case (0.00s)`
    //   ` TestFoo` (no duration tail)
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return None;
    }
    let name = match trimmed.rfind('(') {
        Some(idx) => trimmed[..idx].trim().to_string(),
        None => trimmed.to_string(),
    };
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn parse_indented_location(line: &str) -> Option<TestLocation> {
    // `<file>:<line>: <msg>` — same as pytest's --tb=line, just with
    // a Go-flavoured path. Reject anything missing the `.go` suffix
    // to avoid latching onto random log output.
    let first_colon = line.find(':')?;
    let path = line[..first_colon].trim();
    if !path.ends_with(".go") {
        return None;
    }
    let after_path = &line[first_colon + 1..];
    let second_colon = after_path.find(':')?;
    let line_s = &after_path[..second_colon];
    let line_no: usize = line_s.parse().ok()?;
    Some(TestLocation {
        path: PathBuf::from(path),
        line: line_no,
        col: 1,
    })
}

fn indented_message(line: &str) -> Option<String> {
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
    fn pass_emits_case() {
        let events = parse_lines(&[
            "=== RUN   TestSlugify",
            "--- PASS: TestSlugify (0.00s)",
        ]);
        let cases: Vec<&TestEvent> = events
            .iter()
            .filter(|e| matches!(e, TestEvent::Case { status: TestStatus::Passed, .. }))
            .collect();
        assert_eq!(cases.len(), 1);
        if let TestEvent::Case { name, .. } = cases[0] {
            assert_eq!(name, "TestSlugify");
        }
    }

    #[test]
    fn fail_attaches_location_and_message_on_flush() {
        let events = parse_lines(&[
            "=== RUN   TestStripsPunctuation",
            "    utils_test.go:14: expected its-a-test, got its-a-test!",
            "--- FAIL: TestStripsPunctuation (0.00s)",
            "FAIL",
            "exit status 1",
            "FAIL\texample.com/foo\t0.123s",
        ]);
        let decorated: Vec<&TestEvent> = events
            .iter()
            .filter(|e| matches!(e, TestEvent::Case { status: TestStatus::Failed, location: Some(_), .. }))
            .collect();
        assert_eq!(decorated.len(), 1);
        if let TestEvent::Case { location: Some(loc), message, .. } = decorated[0] {
            assert_eq!(loc.path, PathBuf::from("utils_test.go"));
            assert_eq!(loc.line, 14);
            assert!(message.as_deref().unwrap_or("").contains("expected"));
        } else {
            panic!("expected decorated failure");
        }
    }

    #[test]
    fn summary_counts_accumulated_from_verdicts() {
        let events = parse_lines(&[
            "=== RUN   TestA",
            "--- PASS: TestA (0.00s)",
            "=== RUN   TestB",
            "--- FAIL: TestB (0.00s)",
            "=== RUN   TestC",
            "--- SKIP: TestC (0.00s)",
        ]);
        match events.last().unwrap() {
            TestEvent::Finished { summary } => {
                assert_eq!(summary.passed, 1);
                assert_eq!(summary.failed, 1);
                assert_eq!(summary.ignored, 1);
            }
            _ => panic!("last event should be Finished"),
        }
    }

    #[test]
    fn subtests_keep_full_path() {
        let events = parse_lines(&[
            "=== RUN   TestParent",
            "=== RUN   TestParent/case_one",
            "--- PASS: TestParent/case_one (0.00s)",
            "--- PASS: TestParent (0.00s)",
        ]);
        let names: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                TestEvent::Case { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&"TestParent/case_one".to_string()));
        assert!(names.contains(&"TestParent".to_string()));
    }

    #[test]
    fn list_parse_filters_package_chatter() {
        let raw = "\
TestSlugify
TestStripsPunctuation
ok      example.com/foo  0.123s
?       example.com/empty [no test files]
";
        let mut names = parse_list_output(raw);
        names.sort();
        assert_eq!(names, vec!["TestSlugify", "TestStripsPunctuation"]);
    }

    #[test]
    fn filter_for_nearest_finds_test_func() {
        let src = "package foo\n\nfunc helper() int { return 1 }\n\nfunc TestSlugify(t *testing.T) {\n\tt.Run(\"case\", func(t *testing.T) {\n\t\t// cursor here\n\t})\n}\n";
        let got = filter_for_nearest(src, 6);
        assert_eq!(got.as_deref(), Some("TestSlugify"));
    }

    #[test]
    fn filter_for_nearest_picks_up_benchmark_and_example() {
        let src = "func BenchmarkFoo(b *testing.B) {}\n";
        assert_eq!(filter_for_nearest(src, 0).as_deref(), Some("BenchmarkFoo"));
        let src = "func ExampleHello() {\n\tfmt.Println(\"hi\")\n}\n";
        assert_eq!(filter_for_nearest(src, 1).as_deref(), Some("ExampleHello"));
    }

    #[test]
    fn build_run_command_anchors_name_filter() {
        let req = TestRunRequest {
            filter: Some("TestSlugify".into()),
            workspace_root: PathBuf::from("/tmp"),
            label: "nearest".into(),
        };
        let cmd = build_run_command(&req).unwrap();
        let r_idx = cmd.args.iter().position(|a| a == "-run").expect("expected -run flag");
        assert_eq!(cmd.args[r_idx + 1], "^TestSlugify$");
    }

    #[test]
    fn build_run_command_routes_package_filter_positionally() {
        let req = TestRunRequest {
            filter: Some("./pkg/...".into()),
            workspace_root: PathBuf::from("/tmp"),
            label: "file".into(),
        };
        let cmd = build_run_command(&req).unwrap();
        assert!(cmd.args.iter().any(|a| a == "./pkg/..."));
        assert!(!cmd.args.iter().any(|a| a == "-run"));
    }

    #[test]
    fn regex_escape_quotes_metachars() {
        assert_eq!(regex_escape("TestFoo"), "TestFoo");
        assert_eq!(regex_escape("Test.With.Dots"), "Test\\.With\\.Dots");
    }
}
