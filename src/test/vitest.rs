//! `vitest` adapter — discovery, run-command construction, and the
//! line-by-line parser that turns vitest's verbose-reporter output
//! into `TestEvent`s. Mirrors `cargo.rs` in structure; the framework
//! (overlay, quickfix, status line, ex-commands) is fully shared.
//!
//! Vitest's verbose reporter (non-TTY) streams one line per test:
//!
//! ```text
//!  ✓ utils.test.ts > slugify > lowercases and dashes spaces
//!  × utils.test.ts > slugify > strips punctuation
//!    → expected 'its-a-test!' to be 'its-a-test'
//!
//! ⎯⎯⎯⎯⎯⎯⎯ Failed Tests 1 ⎯⎯⎯⎯⎯⎯⎯
//!
//!  FAIL  utils.test.ts > slugify > strips punctuation
//! AssertionError: expected 'its-a-test!' to be 'its-a-test'
//!
//!  ❯ utils.test.ts:14:5
//!
//!  Test Files  1 failed (1)
//!       Tests  1 failed | 15 passed (16)
//! ```
//!
//! Per-test verdict comes from the `✓` / `×` / `↓` lines that stream
//! as tests complete; failure locations come from the `❯ file:line:col`
//! lines inside the `Failed Tests` block (the streamed `×` line
//! doesn't carry a location). The final `Tests` summary line gives
//! the rolling tally — vitest doesn't print per-file blocks the way
//! cargo does, so there's only ever one summary per run.
//!
//! `:testnearest` walks upward from the cursor for a `describe(...)`,
//! `it(...)`, or `test(...)` invocation and extracts the string
//! literal it was called with. Returned verbatim — vitest's `-t`
//! flag treats the filter as a substring match against the full test
//! name (`describe` + `it` concatenated with spaces), so the name
//! alone matches as long as it isn't a substring of unrelated tests.

use std::path::{Path, PathBuf};

use super::types::{
    ResolvedCommand, TestEvent, TestLocation, TestRunRequest, TestStatus, TestSummary,
};

/// State threaded across `parse_event_line` invocations. Holds the
/// last `×`-lined case name so the `→` line on the row below can be
/// stitched on as the failure message, and the file/test → location
/// map built from the `Failed Tests` block so failures emitted as
/// `Case { location: None }` during streaming can be re-emitted on
/// flush with locations attached for quickfix.
#[derive(Debug, Default)]
pub struct VitestParseState {
    /// Name of the most-recent failing test, used to associate the
    /// indented `→ ...` line that immediately follows with it. Set
    /// on `×` lines, cleared after the `→` is consumed.
    pub current_failure_name: Option<String>,
    /// `test name → first error line`. Captures the streamed `→ ...`
    /// detail line. Used by `flush_parser` to decorate the failing
    /// `Case` events with a message for the quickfix overlay.
    pub failure_messages: std::collections::HashMap<String, String>,
    /// `test name → location`. Captures `❯ file:line:col` lines from
    /// the `Failed Tests` block.
    pub failure_locations: std::collections::HashMap<String, TestLocation>,
    /// Current test name being read inside a `FAIL  ...` block —
    /// used so the next `❯ file:line:col` line attaches to the
    /// right case. Cleared on the next blank line.
    pub current_fail_block: Option<String>,
    /// Accumulated summary from the final `Tests` line. Vitest only
    /// emits one summary per run, but we hold it on `flush_parser`
    /// regardless so the contract matches the cargo adapter.
    pub rolling: TestSummary,
}

/// `npx vitest list` writes `<file> > <suite> > <test>` per line for
/// every test in the workspace. No build / compile step required —
/// vitest does the discovery itself.
pub fn build_list_command(root: &Path) -> Option<ResolvedCommand> {
    let args: Vec<String> = vec!["vitest".into(), "list".into()];
    let display = format!("npx {}", args.join(" "));
    Some(ResolvedCommand {
        program: "npx".into(),
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
        // Skip any chatter vitest might emit (it shouldn't, but defend
        // anyway). A real test line has at least one ` > ` separator.
        if !trimmed.contains(" > ") {
            continue;
        }
        out.push(trimmed.to_string());
    }
    out.sort();
    out.dedup();
    out
}

pub fn build_run_command(req: &TestRunRequest) -> Result<ResolvedCommand, String> {
    let mut args: Vec<String> = vec!["vitest".into(), "run".into(), "--reporter=verbose".into()];
    if let Some(filter) = req.filter.as_ref() {
        if !filter.is_empty() {
            // Heuristic: if the filter looks like a path / file glob,
            // pass it positionally (vitest treats positional args as
            // file filters). Otherwise treat it as a test-name
            // substring via `-t`.
            if looks_like_path_filter(filter) {
                args.push(filter.clone());
            } else {
                args.push("-t".into());
                args.push(filter.clone());
            }
        }
    }
    let display = format!("npx {}", args.join(" "));
    Ok(ResolvedCommand {
        program: "npx".into(),
        args,
        cwd: req.workspace_root.clone(),
        display,
    })
}

fn looks_like_path_filter(s: &str) -> bool {
    // Vitest accepts both real paths and glob patterns positionally.
    // We treat anything containing `/` or a test-file suffix as a
    // path filter; everything else is a name pattern.
    s.contains('/')
        || s.ends_with(".ts")
        || s.ends_with(".tsx")
        || s.ends_with(".js")
        || s.ends_with(".jsx")
        || s.ends_with(".mjs")
        || s.ends_with(".cjs")
}

pub fn parse_event_line(line: &str, state: &mut super::specs::LineParseState) -> Vec<TestEvent> {
    let s = &mut state.vitest;
    let mut events = Vec::new();

    // 1. Per-test streaming lines. Vitest indents with a leading
    //    space; the verdict glyph is the first non-whitespace char.
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("✓ ") {
        s.current_failure_name = None;
        events.push(TestEvent::Case {
            name: strip_trailing_duration(rest).to_string(),
            status: TestStatus::Passed,
            location: None,
            message: None,
        });
        return events;
    }
    if let Some(rest) = trimmed.strip_prefix("× ") {
        let name = strip_trailing_duration(rest).to_string();
        s.current_failure_name = Some(name.clone());
        events.push(TestEvent::Case {
            name,
            status: TestStatus::Failed,
            location: None,
            message: None,
        });
        return events;
    }
    if let Some(rest) = trimmed.strip_prefix("↓ ") {
        // Skipped (`it.skip`, `describe.skip`, `.todo`). Vitest's
        // verbose reporter sometimes omits these lines; when it does
        // emit them the glyph is `↓`.
        s.current_failure_name = None;
        events.push(TestEvent::Case {
            name: strip_trailing_duration(rest).to_string(),
            status: TestStatus::Ignored,
            location: None,
            message: None,
        });
        return events;
    }

    // 2. The indented `→ <message>` line immediately after a `×` carries
    //    the first error message. Capture it against the most recent
    //    failing name so the flush pass can decorate the case event.
    if let Some(rest) = trimmed.strip_prefix("→ ") {
        if let Some(name) = s.current_failure_name.take() {
            s.failure_messages.insert(name, rest.trim().to_string());
        }
        return events;
    }

    // 3. The `FAIL  <name>` header inside the `Failed Tests` block —
    //    arms the parser to attach the next `❯ file:line:col` line.
    //    A blank line typically sits between this and the location
    //    (assertion diff goes in between), so we deliberately don't
    //    clear `current_fail_block` on blank lines — the next FAIL
    //    or ❯ naturally cycles state.
    if let Some(rest) = trimmed.strip_prefix("FAIL  ") {
        s.current_fail_block = Some(rest.trim().to_string());
        return events;
    }

    // 4. Location line inside a fail block — `❯ <file>:<line>:<col>`.
    //    Consume the active fail-block name so a later stray `❯`
    //    (e.g. from a stack frame deeper in the same panic) doesn't
    //    overwrite the location we already captured.
    if let Some(rest) = trimmed.strip_prefix("❯ ") {
        if let (Some(name), Some(loc)) = (s.current_fail_block.take(), parse_location(rest)) {
            s.failure_locations.insert(name, loc);
        }
        return events;
    }

    // 5. Summary line — `      Tests  N failed | M passed | K skipped (T)`
    //    or `      Tests  N passed (T)`.
    if let Some(rest) = trimmed.strip_prefix("Tests  ") {
        if let Some(summary) = parse_summary(rest) {
            s.rolling = summary;
            return events;
        }
    }

    events
}

pub fn flush_parser(state: &mut super::specs::LineParseState) -> Vec<TestEvent> {
    let s = &mut state.vitest;
    let mut events = Vec::new();
    // Re-emit each failing test as a decorated `Case` so the
    // orchestration layer's quickfix builder picks up location +
    // message. We don't try to dedupe against the earlier
    // `Failed/None`-location case events — `qf_load_from_test_failures`
    // reads from `test.failures` which `manager.rs` builds from the
    // decorated copies anyway.
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

/// `:testfile` — pass the file path verbatim to vitest. Vitest accepts
/// positional file/glob args and runs only the tests in the matched
/// files. Returns `None` for files that aren't recognisable test
/// files so an accidental `:testfile` on `utils.ts` doesn't try to
/// run the whole library through the test runner.
pub fn filter_for_file(file: &Path, root: &Path) -> Option<String> {
    let name = file.file_name().and_then(|s| s.to_str())?;
    if !is_test_filename(name) {
        return None;
    }
    let abs = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let root_abs = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let rel = abs.strip_prefix(&root_abs).unwrap_or(&abs);
    // Normalise to `/` on Windows — vitest accepts either, and the
    // forward-slash form matches how the user would type the path
    // themselves and reads cleanly in the status line.
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// `:testnearest` — walk upward from `cursor_line` for the closest
/// `it(...)` / `test(...)` / `describe(...)` invocation, and return
/// the string-literal name it was called with. Vitest's `-t` flag
/// matches against the full hierarchical test name; the leaf name
/// alone is generally specific enough.
pub fn filter_for_nearest(buffer_text: &str, cursor_line: usize) -> Option<String> {
    let lines: Vec<&str> = buffer_text.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let cursor = cursor_line.min(lines.len() - 1);
    // First-pass: an `it`/`test` enclosing the cursor (the test case
    // itself). Second-pass: fall back to a `describe` block name —
    // useful when the cursor sits in the suite-level setup.
    for prefix in &["it", "test"] {
        if let Some(name) = scan_upward_for_call(&lines, cursor, prefix) {
            return Some(name);
        }
    }
    scan_upward_for_call(&lines, cursor, "describe")
}

fn scan_upward_for_call(lines: &[&str], cursor: usize, fn_name: &str) -> Option<String> {
    for i in (0..=cursor).rev() {
        let t = lines[i].trim_start();
        if let Some(name) = extract_call_string_literal(t, fn_name) {
            return Some(name);
        }
    }
    None
}

/// Match `fn_name( "...", ...)` or `fn_name.skip( "...", ...)` /
/// `fn_name.only(...)` / `fn_name.each(...)` etc. and return the
/// string literal in the first arg slot. Returns `None` for
/// non-matches or for template literals (backticks) — the latter
/// could be a `describe.each` row and we don't try to interpolate.
fn extract_call_string_literal(line: &str, fn_name: &str) -> Option<String> {
    let rest = line.strip_prefix(fn_name)?;
    // Allow chained members (`.skip`, `.only`, `.each`, …) — we don't
    // care which variant it is, just that the call signature is
    // `(name, ...)`.
    let rest = if let Some(after_dot) = rest.strip_prefix('.') {
        let chain_end = after_dot
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(after_dot.len());
        &after_dot[chain_end..]
    } else {
        rest
    };
    let rest = rest.strip_prefix('(')?.trim_start();
    let (quote, body) = match rest.chars().next()? {
        '"' => ('"', &rest[1..]),
        '\'' => ('\'', &rest[1..]),
        _ => return None,
    };
    // Match up to the closing quote of the same kind, honouring
    // backslash escapes. Anything more exotic (concatenation,
    // template strings) we punt on — the user can pass a manual
    // filter via the picker.
    let mut out = String::new();
    let mut escaped = false;
    for c in body.chars() {
        if escaped {
            out.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == quote {
            return Some(out);
        }
        out.push(c);
    }
    None
}

fn is_test_filename(name: &str) -> bool {
    // Common patterns: `foo.test.ts`, `foo.spec.tsx`, `foo.test-d.ts`
    // (type-check tests), `foo.bench.ts`.
    for ext in [".test.", ".spec.", ".test-d.", ".bench."] {
        if name.contains(ext) {
            return true;
        }
    }
    false
}

fn strip_trailing_duration(s: &str) -> &str {
    // Vitest sometimes appends ` Nms` / ` N.Nms` after the test name
    // (only for slow tests). Strip it so `name` is stable across
    // fast/slow runs and the substring filter survives a re-run.
    let trimmed = s.trim_end();
    if let Some(idx) = trimmed.rfind(' ') {
        let tail = &trimmed[idx + 1..];
        if tail.ends_with("ms") || tail.ends_with('s') {
            let stem = &tail[..tail
                .len()
                .saturating_sub(if tail.ends_with("ms") { 2 } else { 1 })];
            if !stem.is_empty() && stem.chars().all(|c| c.is_ascii_digit() || c == '.') {
                return trimmed[..idx].trim_end();
            }
        }
    }
    trimmed
}

fn parse_location(s: &str) -> Option<TestLocation> {
    // `❯ ` already stripped — input is `<path>:<line>:<col>` (the
    // path may contain colons on Windows, but binvim is Unix-only
    // for now).
    let trimmed = s.trim();
    let mut iter = trimmed.rsplitn(3, ':');
    let col_s = iter.next()?;
    let line_s = iter.next()?;
    let path_s = iter.next()?;
    let col: usize = col_s.parse().ok()?;
    let line_no: usize = line_s.parse().ok()?;
    Some(TestLocation {
        path: PathBuf::from(path_s.trim()),
        line: line_no,
        col,
    })
}

fn parse_summary(s: &str) -> Option<TestSummary> {
    // Inputs we accept:
    //   `1 failed | 15 passed (16)`
    //   `16 passed (16)`
    //   `1 failed | 1 passed | 1 skipped (3)`
    //   `1 passed | 1 todo (2)`
    let head = s.split('(').next().unwrap_or(s).trim();
    if head.is_empty() {
        return None;
    }
    let mut summary = TestSummary::default();
    let mut saw_any = false;
    for chunk in head.split('|') {
        let chunk = chunk.trim();
        let (num_s, label) = chunk.split_once(' ')?;
        let n: usize = num_s.parse().ok()?;
        if label.starts_with("passed") {
            summary.passed += n;
            saw_any = true;
        } else if label.starts_with("failed") {
            summary.failed += n;
            saw_any = true;
        } else if label.starts_with("skipped") || label.starts_with("todo") {
            summary.ignored += n;
            saw_any = true;
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
        let events = parse_lines(&[" ✓ utils.test.ts > slugify > lowercases and dashes spaces"]);
        let cases: Vec<&TestEvent> = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    TestEvent::Case {
                        status: TestStatus::Passed,
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(cases.len(), 1);
        if let TestEvent::Case { name, .. } = cases[0] {
            assert_eq!(
                name,
                "utils.test.ts > slugify > lowercases and dashes spaces"
            );
        }
    }

    #[test]
    fn fail_line_with_decorated_location_on_flush() {
        let events = parse_lines(&[
            " × utils.test.ts > slugify > strips punctuation",
            "   → expected 'its-a-test!' to be 'its-a-test'",
            "",
            "⎯⎯⎯⎯⎯⎯⎯ Failed Tests 1 ⎯⎯⎯⎯⎯⎯⎯",
            "",
            " FAIL  utils.test.ts > slugify > strips punctuation",
            "AssertionError: expected 'its-a-test!' to be 'its-a-test'",
            "",
            " ❯ utils.test.ts:14:5",
        ]);
        let decorated: Vec<&TestEvent> = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    TestEvent::Case {
                        status: TestStatus::Failed,
                        location: Some(_),
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(decorated.len(), 1);
        if let TestEvent::Case {
            location: Some(loc),
            message,
            ..
        } = decorated[0]
        {
            assert_eq!(loc.path, PathBuf::from("utils.test.ts"));
            assert_eq!(loc.line, 14);
            assert_eq!(loc.col, 5);
            assert_eq!(
                message.as_deref(),
                Some("expected 'its-a-test!' to be 'its-a-test'"),
            );
        } else {
            panic!("expected decorated failure with location");
        }
    }

    #[test]
    fn summary_line_aggregated_into_finished() {
        let events = parse_lines(&[
            " ✓ utils.test.ts > slugify > lowercases and dashes spaces",
            "      Tests  1 failed | 15 passed | 1 skipped (17)",
        ]);
        let last = events.last().expect("Finished is always emitted");
        match last {
            TestEvent::Finished { summary } => {
                assert_eq!(summary.failed, 1);
                assert_eq!(summary.passed, 15);
                assert_eq!(summary.ignored, 1);
            }
            _ => panic!("last event should be Finished"),
        }
    }

    #[test]
    fn summary_line_handles_passed_only() {
        let events = parse_lines(&["      Tests  16 passed (16)"]);
        let last = events.last().unwrap();
        match last {
            TestEvent::Finished { summary } => {
                assert_eq!(summary.passed, 16);
                assert_eq!(summary.failed, 0);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn list_parse_keeps_full_hierarchical_name() {
        let raw = "\
utils.test.ts > slugify > lowercases and dashes spaces
utils.test.ts > slugify > strips punctuation
";
        let mut names = parse_list_output(raw);
        names.sort();
        assert_eq!(
            names,
            vec![
                "utils.test.ts > slugify > lowercases and dashes spaces",
                "utils.test.ts > slugify > strips punctuation",
            ],
        );
    }

    #[test]
    fn filter_for_nearest_finds_it_call() {
        let src = "describe(\"slugify\", () => {\n  it(\"lowercases and dashes spaces\", () => {\n    expect(true).toBe(true);\n  });\n});\n";
        let got = filter_for_nearest(src, 2);
        assert_eq!(got.as_deref(), Some("lowercases and dashes spaces"));
    }

    #[test]
    fn filter_for_nearest_falls_back_to_describe() {
        let src = "describe(\"outer suite\", () => {\n  const setup = 1;\n});\n";
        let got = filter_for_nearest(src, 1);
        assert_eq!(got.as_deref(), Some("outer suite"));
    }

    #[test]
    fn filter_for_nearest_handles_skip_and_only_chains() {
        let src = "it.skip(\"a thing\", () => {});\n";
        assert_eq!(filter_for_nearest(src, 0).as_deref(), Some("a thing"));
        let src = "test.only(\"another\", () => {});\n";
        assert_eq!(filter_for_nearest(src, 0).as_deref(), Some("another"));
    }

    #[test]
    fn filter_for_nearest_handles_single_quotes() {
        let src = "it('quoted', () => {});\n";
        assert_eq!(filter_for_nearest(src, 0).as_deref(), Some("quoted"));
    }

    #[test]
    fn filter_for_file_only_matches_test_files() {
        let tmp = std::env::temp_dir().join("binvim-vitest-filter-for-file");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("sub")).unwrap();
        std::fs::write(tmp.join("foo.test.ts"), "").unwrap();
        std::fs::write(tmp.join("bar.ts"), "").unwrap();
        std::fs::write(tmp.join("sub/baz.spec.ts"), "").unwrap();

        assert_eq!(
            filter_for_file(&tmp.join("foo.test.ts"), &tmp).as_deref(),
            Some("foo.test.ts"),
        );
        assert_eq!(filter_for_file(&tmp.join("bar.ts"), &tmp), None);
        assert_eq!(
            filter_for_file(&tmp.join("sub/baz.spec.ts"), &tmp).as_deref(),
            Some("sub/baz.spec.ts"),
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn build_run_command_routes_path_filter_positionally() {
        let req = TestRunRequest {
            filter: Some("foo.test.ts".into()),
            workspace_root: std::env::temp_dir(),
            label: "file".into(),
        };
        let cmd = build_run_command(&req).unwrap();
        assert!(cmd.args.iter().any(|a| a == "foo.test.ts"));
        assert!(!cmd.args.iter().any(|a| a == "-t"));
    }

    #[test]
    fn build_run_command_routes_name_filter_via_dash_t() {
        let req = TestRunRequest {
            filter: Some("lowercases and dashes spaces".into()),
            workspace_root: std::env::temp_dir(),
            label: "nearest".into(),
        };
        let cmd = build_run_command(&req).unwrap();
        let t_idx = cmd
            .args
            .iter()
            .position(|a| a == "-t")
            .expect("expected -t flag");
        assert_eq!(cmd.args[t_idx + 1], "lowercases and dashes spaces");
    }

    #[test]
    fn strip_trailing_duration_removes_slow_test_tag() {
        assert_eq!(
            strip_trailing_duration("utils.test.ts > slow > thing 142ms"),
            "utils.test.ts > slow > thing",
        );
        assert_eq!(
            strip_trailing_duration("utils.test.ts > fast > thing"),
            "utils.test.ts > fast > thing",
        );
    }
}
