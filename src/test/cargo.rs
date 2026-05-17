//! `cargo test` adapter — discovery, run-command construction, and the
//! line-by-line parser that turns libtest's terse text output into
//! `TestEvent`s.
//!
//! libtest's stable output looks like:
//!
//! ```text
//! running 3 tests
//! test motion::tests::word_forward_basic ... ok
//! test motion::tests::word_forward_long_word ... FAILED
//! test motion::tests::word_forward_punct ... ignored
//!
//! failures:
//!
//! ---- motion::tests::word_forward_long_word stdout ----
//! thread 'motion::tests::word_forward_long_word' panicked at src/motion.rs:123:5:
//! assertion `left == right` failed
//!   left: 5
//!  right: 6
//!
//! failures:
//!     motion::tests::word_forward_long_word
//!
//! test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
//! ```
//!
//! `cargo test` may invoke multiple test binaries in the same run
//! (unit tests + each integration target + doctests), each producing
//! its own `running N tests` / `test result:` block. We accumulate
//! per-block summaries into a single rolling total.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::types::{
    OutputStream, ResolvedCommand, TestEvent, TestLocation, TestRunRequest, TestStatus,
    TestSummary,
};

/// State threaded across `parse_event_line` invocations. Holds the
/// in-progress panic block keyed by test name so when the panic line
/// arrives the matching `Case` event gets enriched with a location.
#[derive(Debug, Default)]
pub struct CargoParseState {
    /// Cases emitted as `Failed` before their panic message arrived.
    /// libtest prints `test X ... FAILED` immediately; the panic detail
    /// lands later in the `failures:` block. We keep `name → location`
    /// here so the final flush can decorate the run's failures with
    /// locations for the quickfix list.
    pub failure_locations: HashMap<String, TestLocation>,
    /// Same as `failure_locations` but for the panic message body —
    /// the first non-empty line after the `panicked at` header.
    pub failure_messages: HashMap<String, String>,
    /// Are we currently inside a `failures:` block? Set by the
    /// `failures:` header, cleared by an empty line or a subsequent
    /// `test result:` summary.
    pub in_failures_block: bool,
    /// Name of the test whose panic body we're currently reading. Set
    /// by `---- <name> stdout ----`, cleared on the next blank line.
    pub current_failure: Option<String>,
    /// Pending `parsed-from-panic` location lookups — we may see the
    /// `panicked at FILE:LINE:COL:` line BEFORE the panic message body
    /// line, so we hold the location until the next non-empty line
    /// shows up.
    pub pending_panic_message: bool,
    /// Rolling tally across multiple per-binary `test result:` lines.
    pub rolling: TestSummary,
}

/// `cargo test -- --list --format=terse` lists every test the workspace
/// would run, one per line, in the form `motion::tests::foo: test`.
pub fn build_list_command(root: &Path) -> Option<ResolvedCommand> {
    let args: Vec<String> = vec![
        "test".into(),
        "--quiet".into(),
        "--".into(),
        "--list".into(),
        "--format=terse".into(),
    ];
    let display = format!("cargo {}", args.join(" "));
    Some(ResolvedCommand {
        program: "cargo".into(),
        args,
        cwd: root.to_path_buf(),
        display,
    })
}

pub fn parse_list_output(stdout: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        // Lines look like `motion::tests::foo: test` (or `: benchmark`).
        // Skip the trailing `N tests, M benchmarks` tally and any cargo
        // chatter (`Compiling …`, `Finished …`, `Running …`).
        if let Some(name) = line.strip_suffix(": test") {
            out.push(name.trim().to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

pub fn build_run_command(req: &TestRunRequest) -> Result<ResolvedCommand, String> {
    let mut args: Vec<String> = vec!["test".into(), "--color=never".into()];
    // Filter goes before the `--` so cargo treats it as a positional
    // filter passed to libtest. libtest applies it as a substring match
    // on the test name, which is exactly what we want for both
    // `:testnearest` (function name) and `:testfile` (module path).
    if let Some(filter) = req.filter.as_ref() {
        if !filter.is_empty() {
            args.push(filter.clone());
        }
    }
    args.push("--".into());
    args.push("--nocapture".into());
    let display = format!("cargo {}", args.join(" "));
    Ok(ResolvedCommand {
        program: "cargo".into(),
        args,
        cwd: req.workspace_root.clone(),
        display,
    })
}

pub fn parse_event_line(line: &str, state: &mut super::specs::LineParseState) -> Vec<TestEvent> {
    let s = &mut state.cargo;
    let trimmed = line.trim_end();
    let mut events = Vec::new();

    // 1. Case results — `test X ... ok` / `... FAILED` / `... ignored`.
    if let Some(rest) = trimmed.strip_prefix("test ") {
        if let Some((name, verdict)) = rest.rsplit_once(" ... ") {
            let status = match verdict.trim() {
                "ok" => Some(TestStatus::Passed),
                "FAILED" => Some(TestStatus::Failed),
                v if v.starts_with("ignored") => Some(TestStatus::Ignored),
                _ => None,
            };
            if let Some(status) = status {
                // libtest sometimes prefixes the name with `tests::` —
                // that's part of the canonical path and we preserve it.
                events.push(TestEvent::Case {
                    name: name.trim().to_string(),
                    status,
                    location: None,
                    message: None,
                });
                return events;
            }
        }
    }

    // 2. `failures:` header — start of the per-failure detail block.
    if trimmed == "failures:" {
        s.in_failures_block = true;
        s.current_failure = None;
        s.pending_panic_message = false;
        return events;
    }

    // 3. Per-failure section header: `---- <name> stdout ----`.
    if let Some(inner) = trimmed
        .strip_prefix("---- ")
        .and_then(|r| r.strip_suffix(" ----"))
    {
        if let Some(name) = inner.strip_suffix(" stdout") {
            s.current_failure = Some(name.trim().to_string());
            s.pending_panic_message = false;
            return events;
        }
    }

    // 4. Panic header line: `thread 'X' panicked at FILE:LINE:COL:`.
    //    Extract the location for the quickfix list.
    if let Some(loc) = parse_panic_header(trimmed) {
        if let Some(name) = s.current_failure.clone() {
            s.failure_locations.insert(name, loc);
            s.pending_panic_message = true;
        }
        return events;
    }

    // 5. Panic message body — the next non-empty, non-prefixed line
    //    after the panic header. Saved per current failure so a later
    //    quickfix decoration can show it as the entry's `text` field.
    if s.pending_panic_message && !trimmed.is_empty() {
        if let Some(name) = s.current_failure.clone() {
            s.failure_messages.insert(name, trimmed.to_string());
            s.pending_panic_message = false;
        }
        return events;
    }

    // 6. Per-binary `test result:` tally — accumulate into the run total.
    if let Some(summary) = parse_test_result_line(trimmed) {
        s.rolling.add(&summary);
        // Leaving the failures-block on every summary keeps the parser
        // honest if the user runs multiple test binaries back-to-back.
        s.in_failures_block = false;
        s.current_failure = None;
        s.pending_panic_message = false;
        return events;
    }

    // 7. Empty line resets per-failure tracking inside the failures
    //    block — every detail section ends with one.
    if trimmed.is_empty() {
        s.current_failure = None;
        s.pending_panic_message = false;
    }
    events
}

pub fn flush_parser(state: &mut super::specs::LineParseState) -> Vec<TestEvent> {
    let s = &mut state.cargo;
    let mut events = Vec::new();
    // Re-emit Case events for each captured failure decorated with
    // location + message. The orchestration layer keys quickfix
    // entries off these decorated cases (rather than the early
    // `Case { status: Failed, location: None }` events emitted while
    // parsing the per-test summary lines above).
    for (name, loc) in s.failure_locations.iter() {
        events.push(TestEvent::Case {
            name: name.clone(),
            status: TestStatus::Failed,
            location: Some(loc.clone()),
            message: s.failure_messages.get(name).cloned(),
        });
    }
    let summary = std::mem::take(&mut s.rolling);
    events.push(TestEvent::Finished { summary });
    events
}

/// File-stem heuristic for `:testfile`: derive a cargo test substring
/// from the path relative to the workspace `src/` directory. Maps
/// `src/motion.rs` → `motion::`, `src/app/dap_glue.rs` →
/// `app::dap_glue::`, `src/main.rs` / `src/lib.rs` → no filter (run
/// the whole crate). Returns `None` for paths outside `src/` and for
/// non-Rust files.
pub fn filter_for_file(file: &Path, root: &Path) -> Option<String> {
    if file.extension().and_then(|e| e.to_str()) != Some("rs") {
        return None;
    }
    let abs = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let root_abs = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let rel = abs.strip_prefix(&root_abs).ok()?;
    let mut comps: Vec<&str> = rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    // Drop the leading `src/` if it's present — cargo's module path
    // doesn't include it. Other top-level dirs (`tests/`, `benches/`,
    // `examples/`) we leave alone; they're rarely the source of a
    // single-file filter anyway.
    if comps.first().copied() == Some("src") {
        comps.remove(0);
    }
    if comps.is_empty() {
        return None;
    }
    let last = comps.pop()?;
    let stem = Path::new(last).file_stem().and_then(|s| s.to_str())?;
    // `main.rs`/`lib.rs`/`mod.rs` aren't modules of their own; running
    // them as filters would match nothing or everything depending on
    // location, so collapse them to the directory module if any.
    if matches!(stem, "main" | "lib" | "mod") {
        if comps.is_empty() {
            return None;
        }
        return Some(format!("{}::", comps.join("::")));
    }
    let mut path: Vec<String> = comps.iter().map(|c| (*c).to_string()).collect();
    path.push(stem.to_string());
    Some(format!("{}::", path.join("::")))
}

/// Walk backwards from `cursor_line` for a `#[test]` attribute, then
/// scan forward for the next `fn name(` and return `name`. Used by
/// `:testnearest`. The function name alone is a sufficient libtest
/// filter — uniqueness is rarely a problem in practice, and ambiguity
/// just means the user runs slightly more than they asked for.
pub fn filter_for_nearest(buffer_text: &str, cursor_line: usize) -> Option<String> {
    let lines: Vec<&str> = buffer_text.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let cursor = cursor_line.min(lines.len() - 1);
    // Walk upward (and through the current line) for `#[test]` or
    // any of the common attribute variants. We accept anything whose
    // attribute name starts with `test` so `#[tokio::test]`,
    // `#[rstest]`, `#[test_case]`, etc. all qualify. The first match
    // wins — usually the attribute directly above the enclosing fn.
    let mut anchor: Option<usize> = None;
    for i in (0..=cursor).rev() {
        let t = lines[i].trim_start();
        if !t.starts_with("#[") {
            continue;
        }
        let attr = &t[2..];
        if attr.starts_with("test")
            || attr.starts_with("tokio::test")
            || attr.starts_with("async_std::test")
            || attr.starts_with("rstest")
        {
            anchor = Some(i);
            break;
        }
    }
    let start = anchor?;
    for line in lines.iter().skip(start) {
        let t = line.trim_start();
        if t.starts_with("//") || t.starts_with("#[") {
            continue;
        }
        // Match `fn name(`, optionally preceded by visibility / async
        // modifiers. Covers the common cases; expand if real-world
        // tests break it.
        let Some(after_fn) = t
            .strip_prefix("fn ")
            .or_else(|| t.strip_prefix("pub fn "))
            .or_else(|| t.strip_prefix("pub(crate) fn "))
            .or_else(|| t.strip_prefix("pub(super) fn "))
            .or_else(|| t.strip_prefix("async fn "))
            .or_else(|| t.strip_prefix("pub async fn "))
        else {
            continue;
        };
        let name: String = after_fn
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() {
            return None;
        }
        return Some(name);
    }
    None
}

fn parse_panic_header(line: &str) -> Option<TestLocation> {
    // Format: `thread '<name>' panicked at <file>:<line>:<col>:`
    // libtest also emits a shorter form when location is unavailable;
    // we only care about the full form.
    let after_at = line.split(" panicked at ").nth(1)?;
    let trimmed = after_at.trim_end_matches(':');
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

fn parse_test_result_line(line: &str) -> Option<TestSummary> {
    // Format: `test result: ok. N passed; M failed; K ignored; … filtered out; …`
    let rest = line.strip_prefix("test result: ")?;
    // Drop the leading `ok.` / `FAILED.` verdict.
    let rest = rest.split_once('.').map(|(_, r)| r.trim()).unwrap_or(rest);
    let mut summary = TestSummary::default();
    for chunk in rest.split(';') {
        let chunk = chunk.trim().trim_end_matches('.');
        let (num_s, label_s) = chunk.split_once(' ')?;
        let n: usize = num_s.parse().ok()?;
        let label = label_s.trim();
        if label.starts_with("passed") {
            summary.passed += n;
        } else if label.starts_with("failed") {
            summary.failed += n;
        } else if label.starts_with("ignored") {
            summary.ignored += n;
        } else if label.starts_with("filtered out") {
            summary.filtered_out += n;
        }
    }
    let _ = OutputStream::Stdout; // silence unused-warning when only used in tests
    Some(summary)
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
    fn passing_test_emits_case_event() {
        let events = parse_lines(&["test motion::tests::foo ... ok"]);
        let cases: Vec<&TestEvent> = events
            .iter()
            .filter(|e| matches!(e, TestEvent::Case { status: TestStatus::Passed, .. }))
            .collect();
        assert_eq!(cases.len(), 1);
    }

    #[test]
    fn failed_test_decorated_with_panic_location_on_flush() {
        let events = parse_lines(&[
            "test motion::tests::foo ... FAILED",
            "",
            "failures:",
            "",
            "---- motion::tests::foo stdout ----",
            "thread 'motion::tests::foo' panicked at src/motion.rs:42:5:",
            "assertion `left == right` failed",
            "",
            "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out",
        ]);
        let decorated: Vec<&TestEvent> = events
            .iter()
            .filter(|e| matches!(e, TestEvent::Case { status: TestStatus::Failed, location: Some(_), .. }))
            .collect();
        assert_eq!(decorated.len(), 1, "flush should re-emit failed case with location");
        if let TestEvent::Case { location: Some(loc), message, .. } = decorated[0] {
            assert_eq!(loc.path, PathBuf::from("src/motion.rs"));
            assert_eq!(loc.line, 42);
            assert_eq!(loc.col, 5);
            assert_eq!(message.as_deref(), Some("assertion `left == right` failed"));
        } else {
            panic!("expected decorated case with location");
        }
    }

    #[test]
    fn summary_aggregates_across_binaries() {
        let events = parse_lines(&[
            "test a ... ok",
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out",
            "test b ... ok",
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out",
        ]);
        let last = events.last().expect("flush always emits Finished");
        match last {
            TestEvent::Finished { summary } => {
                assert_eq!(summary.passed, 2);
                assert_eq!(summary.failed, 0);
            }
            _ => panic!("last event should be Finished"),
        }
    }

    #[test]
    fn list_parse_pulls_test_names_off_terse_output() {
        let raw = "   Compiling x v0.1.0\n    Finished test target(s)\n\
            motion::tests::foo: test\nmotion::tests::bar: test\n2 tests, 0 benchmarks\n";
        let mut names = parse_list_output(raw);
        names.sort();
        assert_eq!(names, vec!["motion::tests::bar", "motion::tests::foo"]);
    }

    #[test]
    fn filter_for_file_strips_src_and_appends_colon_colon() {
        let tmp = std::env::temp_dir().join("binvim-cargo-filter-for-file");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src/app")).unwrap();
        std::fs::write(tmp.join("src/motion.rs"), "").unwrap();
        std::fs::write(tmp.join("src/app/dap_glue.rs"), "").unwrap();
        std::fs::write(tmp.join("src/main.rs"), "").unwrap();

        assert_eq!(
            filter_for_file(&tmp.join("src/motion.rs"), &tmp),
            Some("motion::".to_string())
        );
        assert_eq!(
            filter_for_file(&tmp.join("src/app/dap_glue.rs"), &tmp),
            Some("app::dap_glue::".to_string())
        );
        // main.rs / lib.rs collapse to no filter (run the whole crate).
        assert_eq!(filter_for_file(&tmp.join("src/main.rs"), &tmp), None);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn filter_for_nearest_walks_upward_for_test_attr() {
        let src = "#[test]\nfn word_forward_basic() {\n    assert!(true);\n}\n";
        let got = filter_for_nearest(src, 2);
        assert_eq!(got.as_deref(), Some("word_forward_basic"));
    }

    #[test]
    fn filter_for_nearest_handles_pub_async_fn() {
        let src = "#[tokio::test]\npub async fn does_thing() {}\n";
        let got = filter_for_nearest(src, 1);
        assert_eq!(got.as_deref(), Some("does_thing"));
    }

    #[test]
    fn filter_for_nearest_returns_none_when_no_test_above() {
        let src = "fn helper() {}\nfn other() {}\n";
        assert_eq!(filter_for_nearest(src, 1), None);
    }
}
