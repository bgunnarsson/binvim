//! `dotnet test` adapter — discovery, run-command construction, and
//! the line-by-line parser. Structurally parallel to the cargo /
//! vitest / pytest / gotest adapters; the framework is shared.
//!
//! `dotnet test` (with the default xUnit / NUnit / MSTest loggers and
//! `--logger:"console;verbosity=normal"`) emits per-test verdict lines
//! like:
//!
//! ```text
//! Passed Namespace.TestClass.TestMethod1 [12 ms]
//! Failed Namespace.TestClass.TestMethod2 [13 ms]
//!   Error Message:
//!    Assert.Equal() Failure: Values differ
//!    Expected: 1
//!    Actual:   2
//!   Stack Trace:
//!      at Namespace.TestClass.TestMethod2() in /path/foo.cs:line 14
//! ```
//!
//! Stack-trace `in <path>:line <N>` lines feed `TestLocation`. The
//! `Error Message:` block (one or more indented lines) feeds the
//! per-failure message.

use std::path::{Path, PathBuf};

use super::types::{
    ResolvedCommand, TestEvent, TestLocation, TestRunRequest, TestStatus, TestSummary,
};

/// State threaded across `parse_event_line` invocations. Tracks the
/// most-recent failing test (so stack frames + Error Message rows
/// attach to it) and stashes per-test locations + messages until
/// `flush_parser` re-emits each failing case decorated.
#[derive(Debug, Default)]
pub struct DotnetParseState {
    /// Name of the most-recent `Failed` verdict — used so indented
    /// stack / error rows below it land on the right case. Cleared
    /// when a subsequent `Passed`/`Failed`/`Skipped` line cycles in.
    pub current_failure_name: Option<String>,
    /// `true` while parser is inside the `Error Message:` block
    /// following a Failed verdict. Indented lines accumulate into
    /// `failure_messages`. Reset on the next non-indented line.
    pub in_error_block: bool,
    /// `name → first location` from `in <path>:line N` stack lines.
    pub failure_locations: std::collections::HashMap<String, TestLocation>,
    /// `name → joined error message` from `Error Message:` blocks.
    pub failure_messages: std::collections::HashMap<String, String>,
    /// End-of-run tally from the `Passed:` / `Failed:` / `Skipped:` /
    /// `Total tests:` summary lines.
    pub rolling: TestSummary,
}

/// `dotnet test --list-tests` prints `    FullyQualifiedName` indented
/// rows after a `The following Tests are available:` header. The
/// parser strips the header chatter.
pub fn build_list_command(root: &Path) -> Option<ResolvedCommand> {
    let args: Vec<String> = vec![
        "test".into(),
        "--list-tests".into(),
        "--nologo".into(),
        "--verbosity".into(),
        "quiet".into(),
    ];
    let display = format!("dotnet {}", args.join(" "));
    Some(ResolvedCommand {
        program: "dotnet".into(),
        args,
        cwd: root.to_path_buf(),
        display,
    })
}

pub fn parse_list_output(stdout: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_list = false;
    for line in stdout.lines() {
        if line.contains("The following Tests are available") {
            in_list = true;
            continue;
        }
        if !in_list {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // A real test row is the indented FQN; build chatter and
        // intermediate "Determining projects to restore..." lines
        // start at column 0. The list section keeps its rows
        // indented at least once, so any line whose first char isn't
        // whitespace ends the list.
        if !line.starts_with(' ') && !line.starts_with('\t') {
            in_list = false;
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
        "test".into(),
        "--nologo".into(),
        "--logger".into(),
        "console;verbosity=normal".into(),
    ];
    if let Some(filter) = req.filter.as_ref() {
        if !filter.is_empty() {
            args.push("--filter".into());
            // Heuristic: a filter with `=` / `!=` etc. is already a
            // raw `--filter` expression; pass it through verbatim.
            // Otherwise wrap as `FullyQualifiedName~<filter>` for a
            // substring match, which is what `:testnearest` /
            // `:testfile` produce.
            if filter
                .contains(|c: char| c == '=' || c == '!' || c == '~' || c == '|' || c == '&')
            {
                args.push(filter.clone());
            } else {
                args.push(format!("FullyQualifiedName~{filter}"));
            }
        }
    }
    let display = format!("dotnet {}", args.join(" "));
    Ok(ResolvedCommand {
        program: "dotnet".into(),
        args,
        cwd: req.workspace_root.clone(),
        display,
    })
}

pub fn parse_event_line(line: &str, state: &mut super::specs::LineParseState) -> Vec<TestEvent> {
    let s = &mut state.dotnet;
    let mut events = Vec::new();
    let trimmed = line.trim_end();

    // 1. Per-test verdict — `Passed FQN [Nms]` / `Failed FQN [Nms]`
    //    / `Skipped FQN [Nms]`. Status comes from the first token.
    for (prefix, status) in [
        ("Passed ", TestStatus::Passed),
        ("Failed ", TestStatus::Failed),
        ("Skipped ", TestStatus::Ignored),
    ] {
        if let Some(rest) = trimmed.trim_start().strip_prefix(prefix) {
            let name = strip_duration_tag(rest).trim().to_string();
            if name.is_empty() {
                return events;
            }
            s.in_error_block = false;
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
    }

    // 2. `Error Message:` opens a multi-line error block. The next
    //    indented lines are the message body; lines starting at
    //    column 0 (or `Stack Trace:`) close it.
    if trimmed.trim_start().starts_with("Error Message:") {
        s.in_error_block = true;
        return events;
    }
    if trimmed.trim_start().starts_with("Stack Trace:") {
        s.in_error_block = false;
        return events;
    }
    if s.in_error_block {
        if let Some(name) = s.current_failure_name.clone() {
            let body = trimmed.trim();
            if !body.is_empty() {
                s.failure_messages
                    .entry(name)
                    .and_modify(|m| {
                        if !m.is_empty() {
                            m.push(' ');
                        }
                        m.push_str(body);
                    })
                    .or_insert_with(|| body.to_string());
            }
        }
        // Don't `return` — fall through so a stack-trace location
        // line embedded inside the block still parses correctly.
    }

    // 3. Stack-trace location row — `   at FQN(...) in /path/foo.cs:line 14`.
    //    Only the first per failing test is kept (top of the user-
    //    code stack is what the user wants to jump to).
    if let Some(loc) = parse_stack_location(trimmed) {
        if let Some(name) = s.current_failure_name.clone() {
            s.failure_locations.entry(name).or_insert(loc);
        }
        return events;
    }

    // 4. Summary block at the end:
    //    `Passed!  - Failed:     0, Passed:    16, Skipped:     0, Total:    16, Duration: 0.42s - ...`
    //    or `Failed!  - Failed:     1, Passed:    15, Skipped:     0, ...`
    if trimmed.contains("Failed:") && trimmed.contains("Passed:") && trimmed.contains("Total:") {
        if let Some(summary) = parse_summary_line(trimmed) {
            s.rolling = summary;
            return events;
        }
    }

    events
}

pub fn flush_parser(state: &mut super::specs::LineParseState) -> Vec<TestEvent> {
    let s = &mut state.dotnet;
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
    // Also re-emit failures that had a message but no location, so
    // the test-results overlay shows the body even when the stack
    // frame couldn't be parsed.
    let leftover: Vec<String> = s.failure_messages.keys().cloned().collect();
    for name in leftover {
        let message = s.failure_messages.remove(&name);
        events.push(TestEvent::Case {
            name,
            status: TestStatus::Failed,
            location: None,
            message,
        });
    }
    let summary = std::mem::take(&mut s.rolling);
    events.push(TestEvent::Finished { summary });
    events
}

/// `:testfile` — filter on the file's class name. dotnet test doesn't
/// take a file path directly, but xUnit / NUnit / MSTest all encode
/// the file's primary test class into the FQN, so `ClassName~<base>`
/// is the closest thing.
pub fn filter_for_file(file: &Path, _root: &Path) -> Option<String> {
    let stem = file.file_stem().and_then(|s| s.to_str())?;
    if !is_test_filename_stem(stem) {
        return None;
    }
    Some(format!("ClassName~{stem}"))
}

/// `:testnearest` — walk upward for the closest `[Fact]` / `[Theory]`
/// / `[Test]` / `[TestMethod]` attribute, then for the `public ...
/// <Name>(` declaration immediately after.
pub fn filter_for_nearest(buffer_text: &str, cursor_line: usize) -> Option<String> {
    let lines: Vec<&str> = buffer_text.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let cursor = cursor_line.min(lines.len() - 1);
    // Walk back for the nearest attribute line, then scan forward
    // a short way for the method-decl signature it decorates.
    for i in (0..=cursor).rev() {
        if is_test_attribute(lines[i]) {
            let end = (i + 8).min(lines.len());
            for j in i..end {
                if let Some(name) = parse_method_decl(lines[j]) {
                    return Some(name);
                }
            }
        }
    }
    None
}

fn is_test_attribute(line: &str) -> bool {
    let stripped = line.trim_start();
    let attr = stripped.trim_start_matches('[').trim_end();
    let head = attr
        .split(|c: char| c == '(' || c == ']')
        .next()
        .unwrap_or("")
        .trim();
    matches!(head, "Fact" | "Theory" | "Test" | "TestMethod" | "TestCase")
}

fn parse_method_decl(line: &str) -> Option<String> {
    let stripped = line.trim_start();
    // We want the token immediately followed by `(`. Allow whatever
    // modifiers / return type came before — split on whitespace and
    // grab the last word that ends in `(`.
    let open = stripped.find('(')?;
    let head = &stripped[..open];
    let name = head.rsplit(|c: char| c.is_whitespace()).next()?;
    if name.is_empty() {
        return None;
    }
    if !name.chars().next().map(|c| c.is_ascii_alphabetic() || c == '_').unwrap_or(false) {
        return None;
    }
    Some(name.to_string())
}

fn is_test_filename_stem(stem: &str) -> bool {
    // Common conventions: `FooTests.cs`, `FooTest.cs`, `FooSpec.cs`.
    stem.ends_with("Tests") || stem.ends_with("Test") || stem.ends_with("Spec")
}

fn strip_duration_tag(s: &str) -> &str {
    // Strip a trailing `[N ms]` / `[N.NNN s]` block from the tail.
    let trimmed = s.trim_end();
    if !trimmed.ends_with(']') {
        return trimmed;
    }
    let Some(open) = trimmed.rfind('[') else { return trimmed };
    trimmed[..open].trim_end()
}

fn parse_stack_location(line: &str) -> Option<TestLocation> {
    // Look for ` in <path>:line <N>` — present in stack traces from
    // every common .NET test runner. The path can contain colons on
    // Windows; binvim is Unix-only so we don't worry about that.
    let in_idx = line.find(" in ")?;
    let after = &line[in_idx + 4..];
    let line_marker = after.rfind(":line ")?;
    let path = after[..line_marker].trim();
    let line_no_s = after[line_marker + 6..].trim();
    if path.is_empty() {
        return None;
    }
    let line_no: usize = line_no_s.split_whitespace().next()?.parse().ok()?;
    Some(TestLocation {
        path: PathBuf::from(path),
        line: line_no,
        col: 1,
    })
}

fn parse_summary_line(line: &str) -> Option<TestSummary> {
    let mut summary = TestSummary::default();
    let mut saw_any = false;
    // dotnet's summary header is "Failed!  - Failed: 1, Passed: …",
    // so the first chunk after `split(',')` contains TWO `Failed`
    // tokens (the verdict banner plus the count). The actual key
    // is the last word before the `:` in each chunk, so we take
    // that rather than the first split component.
    for chunk in line.split(',') {
        let chunk = chunk.trim();
        let Some(colon) = chunk.rfind(':') else { continue };
        let head = chunk[..colon].trim();
        let val = chunk[colon + 1..].trim();
        let Some(n_s) = val.split_whitespace().next() else { continue };
        let Ok(n) = n_s.parse::<usize>() else { continue };
        let key = head.split_whitespace().last().unwrap_or("");
        match key {
            "Passed" => {
                summary.passed = n;
                saw_any = true;
            }
            "Failed" => {
                summary.failed = n;
                saw_any = true;
            }
            "Skipped" => {
                summary.ignored = n;
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
    fn pass_emits_case() {
        let events = parse_lines(&["Passed Foo.BarTests.Method1 [12 ms]"]);
        let cases: Vec<&TestEvent> = events
            .iter()
            .filter(|e| matches!(e, TestEvent::Case { status: TestStatus::Passed, .. }))
            .collect();
        assert_eq!(cases.len(), 1);
        if let TestEvent::Case { name, .. } = cases[0] {
            assert_eq!(name, "Foo.BarTests.Method1");
        }
    }

    #[test]
    fn fail_attaches_location_from_stack_trace() {
        let events = parse_lines(&[
            "Failed Foo.BarTests.Method2 [13 ms]",
            "  Error Message:",
            "   Assert.Equal() Failure",
            "  Stack Trace:",
            "     at Foo.BarTests.Method2() in /path/foo.cs:line 14",
            "",
            "Failed!  - Failed:     1, Passed:    15, Skipped:     0, Total:    16, Duration: 0.42s",
        ]);
        let decorated: Vec<&TestEvent> = events
            .iter()
            .filter(|e| matches!(e, TestEvent::Case { status: TestStatus::Failed, location: Some(_), .. }))
            .collect();
        assert_eq!(decorated.len(), 1);
        if let TestEvent::Case { name, location: Some(loc), message, .. } = decorated[0] {
            assert_eq!(name, "Foo.BarTests.Method2");
            assert_eq!(loc.path, PathBuf::from("/path/foo.cs"));
            assert_eq!(loc.line, 14);
            assert!(message
                .as_deref()
                .unwrap_or("")
                .contains("Assert.Equal() Failure"));
        } else {
            panic!("expected decorated failure with location");
        }
    }

    #[test]
    fn summary_line_parsed_into_finished() {
        let events = parse_lines(&[
            "Failed!  - Failed:     1, Passed:    15, Skipped:     2, Total:    18, Duration: 0.42s",
        ]);
        match events.last().unwrap() {
            TestEvent::Finished { summary } => {
                assert_eq!(summary.failed, 1);
                assert_eq!(summary.passed, 15);
                assert_eq!(summary.ignored, 2);
            }
            _ => panic!("last event should be Finished"),
        }
    }

    #[test]
    fn list_parse_strips_header_chatter() {
        let raw = "\
Determining projects to restore...
All projects are up-to-date for restore.
The following Tests are available:
    Foo.BarTests.Method1
    Foo.BarTests.Method2
    Foo.OtherTests.MethodA
Test Run Successful.
";
        let mut names = parse_list_output(raw);
        names.sort();
        assert_eq!(
            names,
            vec![
                "Foo.BarTests.Method1",
                "Foo.BarTests.Method2",
                "Foo.OtherTests.MethodA",
            ],
        );
    }

    #[test]
    fn filter_for_nearest_finds_fact() {
        let src = "\
namespace Foo;
public class BarTests {
    [Fact]
    public void MyTest() {
        Assert.True(true);
        // cursor here
    }
}
";
        let got = filter_for_nearest(src, 5);
        assert_eq!(got.as_deref(), Some("MyTest"));
    }

    #[test]
    fn filter_for_nearest_handles_theory() {
        let src = "\
[Theory]
[InlineData(1)]
public void Parametrised(int x) {}
";
        let got = filter_for_nearest(src, 2);
        assert_eq!(got.as_deref(), Some("Parametrised"));
    }

    #[test]
    fn filter_for_file_wraps_classname_filter() {
        let got = filter_for_file(
            &PathBuf::from("/proj/BarTests.cs"),
            &PathBuf::from("/proj"),
        );
        assert_eq!(got.as_deref(), Some("ClassName~BarTests"));
        let got = filter_for_file(
            &PathBuf::from("/proj/RegularClass.cs"),
            &PathBuf::from("/proj"),
        );
        assert!(got.is_none());
    }

    #[test]
    fn build_run_command_wraps_name_filter() {
        let req = TestRunRequest {
            filter: Some("MyTest".into()),
            workspace_root: PathBuf::from("/tmp"),
            label: "nearest".into(),
        };
        let cmd = build_run_command(&req).unwrap();
        let f_idx = cmd.args.iter().position(|a| a == "--filter").expect("expected --filter");
        assert_eq!(cmd.args[f_idx + 1], "FullyQualifiedName~MyTest");
    }

    #[test]
    fn build_run_command_passes_raw_filter_through() {
        let req = TestRunRequest {
            filter: Some("ClassName=BarTests&Category!=Slow".into()),
            workspace_root: PathBuf::from("/tmp"),
            label: "file".into(),
        };
        let cmd = build_run_command(&req).unwrap();
        let f_idx = cmd.args.iter().position(|a| a == "--filter").expect("expected --filter");
        assert_eq!(cmd.args[f_idx + 1], "ClassName=BarTests&Category!=Slow");
    }
}
