//! Task-runner glue. Picker open / accept / re-run flows + the actual
//! spawn into a labelled bottom-terminal tab.
//!
//! Architectural choice: tasks reuse the existing `:terminal` pane
//! infrastructure rather than getting a dedicated overlay. Each picked
//! task spawns a fresh tab whose label is the task name, so a user can
//! kick off `dev` + `lint` + `build` and tell them apart in the strip.
//! Output stays in the terminal grid (vte-parsed, scrollback, mouse
//! forwarding all work for free); the user closes a finished task with
//! `<leader>tq` the same way they would any other terminal tab. The
//! test runner has structured per-event parsing because tests have
//! per-case verdicts worth surfacing — tasks are just commands.

use std::path::PathBuf;

use crate::mode::Mode;
use crate::picker::{PickerKind, PickerPayload, PickerState};
use crate::task::{self, Task};
use crate::terminal::Terminal;

impl super::App {
    /// `:task` / `<leader>mm` — discover workspace tasks and open a
    /// picker over them. Empty workspace → status-line hint, no
    /// picker. Discovery walks up from the active buffer's parent
    /// (or cwd) once per source; the union is presented in source
    /// order so similar tasks cluster.
    pub(super) fn cmd_task_picker(&mut self) {
        let start = self.task_start_dir();
        let tasks = task::discover_all(&start);
        if tasks.is_empty() {
            self.status_msg = "no tasks discovered (npm / just / cargo / make / dotnet)".into();
            return;
        }
        // Stash the task list and build picker rows that point into it
        // via TaskIdx. Display line: `<tag>  <label>  · <description>`.
        // The tag column is left-padded to a fixed width so the labels
        // align visually across sources without manual eyeballing.
        let tag_width = tasks.iter().map(|t| t.source.tag().len()).max().unwrap_or(4);
        let items: Vec<(String, PickerPayload)> = tasks
            .iter()
            .enumerate()
            .map(|(idx, t)| {
                let desc = t.description.as_deref().unwrap_or("");
                // [bg] marks "dev / watch / serve" tasks so the user
                // can tell what would re-spawn on `:tasklast`. See
                // `Task::is_long_running` for the heuristic.
                let bg = if t.is_long_running() { "  [bg]" } else { "" };
                let display = if desc.is_empty() {
                    format!("{:width$}  {}{bg}", t.source.tag(), t.label, width = tag_width)
                } else {
                    format!(
                        "{:width$}  {}{bg}  · {}",
                        t.source.tag(),
                        t.label,
                        desc,
                        width = tag_width
                    )
                };
                (display, PickerPayload::TaskIdx(idx))
            })
            .collect();
        self.pending_tasks = tasks;
        self.picker = Some(PickerState::new(
            PickerKind::Task,
            "Tasks".into(),
            items,
        ));
        self.mode = Mode::Picker;
    }

    /// Picker-accept handler — pull the staged `Task` by index, kick
    /// it off in a new terminal tab, and remember it for `:tasklast`.
    pub(super) fn task_run_picked(&mut self, idx: usize) {
        // Drain the pending list — even on a bad index, the staging
        // area shouldn't survive into the next picker session.
        let pending = std::mem::take(&mut self.pending_tasks);
        let Some(task) = pending.into_iter().nth(idx) else {
            self.status_msg = "task pick lost context".into();
            return;
        };
        self.task_kickoff(task);
    }

    /// `:tasklast` / `<leader>ml` — re-run the most recent task this
    /// session. Spawns a fresh tab rather than re-using the previous
    /// one so the user can compare runs side-by-side; the previous
    /// tab keeps draining (it's just one of N in the strip now).
    pub(super) fn cmd_task_last(&mut self) {
        let Some(task) = self.last_task.clone() else {
            self.status_msg = "no recent task — run `:task` first".into();
            return;
        };
        // Cautionary hint for long-running tasks — the previous
        // dev server is probably still on a sibling tab, and a second
        // copy will fight it for the same port. The kickoff still
        // proceeds (the user might genuinely want a second instance).
        let bg = task.is_long_running();
        self.task_kickoff(task);
        if bg {
            self.status_msg = format!("{} (long-running — previous tab may still be alive)", self.status_msg);
        }
    }

    /// Spawn a task in a new bottom-terminal tab. Labels the tab with
    /// the task name so the strip stays readable when several tasks
    /// are running simultaneously (the common case: `dev` + `lint` +
    /// `build`).
    fn task_kickoff(&mut self, task: Task) {
        let command_line = task.command_line();
        let label = task.label.clone();
        // Compute pane geometry. Mirrors `cmd_open_terminal` — we flip
        // the open flag first so `terminal_pane_rows()` returns the
        // post-open value when we size the PTY.
        let was_empty = self.terminals.is_empty();
        self.terminal_pane_open = true;
        let rows = self.terminal_pane_rows().saturating_sub(1).max(4) as u16;
        let cols = (self.width as usize).max(8) as u16;
        // Spawn via `$SHELL -l -i -c "<command>"` so the user's rc
        // files load (PATH shims from nvm / asdf / direnv all
        // necessary for many tasks — `pnpm` is usually a nvm-managed
        // Node script) and the resulting shell `exec`s into the task
        // program. The shell wrapper inherits cwd via env::set_current_dir
        // before the spawn, so the task runs from `task.cwd`.
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        // Push the task's working directory into the spawn via a `cd`
        // prefix on the shell command. Cleaner than mutating
        // std::env::current_dir() (which is process-global) — the
        // child shell does the cd, the parent stays put.
        let cwd_arg = shell_quote(&task.cwd);
        let launcher = format!("cd {cwd_arg} && exec {command_line}");
        match Terminal::spawn_program(rows, cols, &shell, &["-l", "-i", "-c", &launcher]) {
            Ok(term) => {
                term.set_label(Some(label.clone()));
                self.terminals.push(term);
                self.active_terminal_idx = self.terminals.len() - 1;
                self.mode = Mode::Terminal;
                self.terminal_focus = crate::app::TerminalFocus::Bottom;
                self.adjust_viewport();
                self.status_msg = format!("task: {} ({})", task.label, command_line);
                self.last_task = Some(task);
                self.resize_all_side_terminals();
            }
            Err(e) => {
                if was_empty {
                    self.terminal_pane_open = false;
                }
                self.status_msg = format!("task: spawn failed: {e}");
            }
        }
    }

    /// Working directory to anchor task discovery against. Prefer the
    /// active buffer's parent directory (so editing a file inside a
    /// nested workspace yields that workspace's tasks); fall back to
    /// the editor's cwd.
    fn task_start_dir(&self) -> PathBuf {
        self.buffer
            .path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

impl super::App {
    /// Walk every terminal tab; for each labelled (task-spawned) tab
    /// that has just exited, scrape the visible grid + scrollback
    /// for `path:line:col` error lines and merge them into the
    /// quickfix list. Returns `true` if anything was added (caller
    /// re-renders to reflect the new qf state on the status line).
    ///
    /// Only labelled tabs are polled — the user might intentionally
    /// `exit` a plain shell tab; we don't want that to flush
    /// random text into quickfix. The label-presence test is a clean
    /// proxy for "spawned by the task runner."
    pub(super) fn task_poll_exits_and_scrape(&mut self) -> bool {
        let mut new_entries: Vec<crate::app::state::QuickfixEntry> = Vec::new();
        let mut status_hint: Option<String> = None;
        for term in &self.terminals {
            let Some(label) = term.label() else { continue };
            let Some(_status) = term.poll_exit() else { continue };
            let inner = term.grid();
            let text = inner.handler.grid.text_lines().join("\n");
            drop(inner);
            let scraped = scrape_task_errors(&text);
            let n = scraped.len();
            if n > 0 {
                new_entries.extend(scraped);
                status_hint = Some(format!("'{label}': {n} error{} → quickfix", if n == 1 { "" } else { "s" }));
            } else if status_hint.is_none() {
                status_hint = Some(format!("'{label}' exited (no errors detected)"));
            }
        }
        if new_entries.is_empty() {
            if let Some(msg) = status_hint {
                self.status_msg = msg;
                return true;
            }
            return false;
        }
        // Replace rather than append — `]q` semantics are nicer when
        // the list reflects the most recent build. Users who want the
        // old list before kicking off a new task can step through it
        // first.
        let n = new_entries.len();
        self.quickfix = Some(crate::app::state::QuickfixState {
            entries: new_entries,
            current: 0,
        });
        self.status_msg = status_hint
            .unwrap_or_else(|| format!("quickfix: {n} task error{}", if n == 1 { "" } else { "s" }));
        true
    }
}

/// Parse compiler / linter error lines out of raw task output. Handles
/// two forms:
///
///   - `path:line:col[:end_col]: <anything>` — gcc / clang / rustc /
///     ruff / biome / eslint / generic POSIX
///   - `path(line,col): <anything>` — tsc legacy formatter
///
/// Skips lines starting with `--> ` even though rustc emits them
/// (those are decorations beneath an `error[E0XXX]:` header that
/// already has the location on a separate line — collecting both
/// would double-count). For ANSI-coloured output: the vte parser
/// strips escapes into `Cell` attributes before this scraper sees
/// the text, so we don't need to handle escapes here.
pub(super) fn scrape_task_errors(text: &str) -> Vec<crate::app::state::QuickfixEntry> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut out: Vec<crate::app::state::QuickfixEntry> = Vec::new();
    let mut seen: std::collections::HashSet<(std::path::PathBuf, usize, usize)> =
        std::collections::HashSet::new();
    for raw in text.lines() {
        let line = raw.trim_start();
        // Strip `--> ` (rustc decoration) so the substring math
        // doesn't trip on the leading arrow.
        let line = line.strip_prefix("--> ").unwrap_or(line);
        if let Some(qf) = parse_posix_form(line, &cwd) {
            if seen.insert((qf.path.clone(), qf.line, qf.col)) {
                out.push(qf);
            }
            continue;
        }
        if let Some(qf) = parse_tsc_form(line, &cwd) {
            if seen.insert((qf.path.clone(), qf.line, qf.col)) {
                out.push(qf);
            }
        }
    }
    out
}

/// `path:line:col[:end_col]: <message>`. Path can contain `/`, `.`,
/// `-`, `_`, alphanumerics; the first `:` after the first space
/// terminates path lookup. We require both `line` and `col` to parse
/// as digits to keep false positives down on log lines like
/// `12:34:56 INFO: …`.
fn parse_posix_form(
    line: &str,
    cwd: &std::path::Path,
) -> Option<crate::app::state::QuickfixEntry> {
    // First `:` separates path from line; second separates line from
    // col; third (optional) separates col from end_col; the remainder
    // is the message.
    let bytes = line.as_bytes();
    // Find the path: stops at the first `:` that's followed by a
    // digit (so `C:\foo.rs:10:5: …` and Windows paths don't blow up,
    // though we don't claim full Windows support).
    let mut path_end = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b':' && i > 0 && bytes.get(i + 1).map(|c| c.is_ascii_digit()).unwrap_or(false) {
            path_end = Some(i);
            break;
        }
    }
    let path_end = path_end?;
    let path_part = &line[..path_end];
    let rest = &line[path_end + 1..];
    let (line_s, after_line) = take_digits(rest)?;
    let after_line = after_line.strip_prefix(':')?;
    let (col_s, after_col) = take_digits(after_line)?;
    // Must be followed by `:` (with or without an end_col) and then
    // a message — bare `path:N:M` rows without a trailing colon are
    // probably greps or BIND-style log timestamps.
    let after_col = if let Some(stripped) = after_col.strip_prefix(':') {
        // Could be `:end_col:` or just the message.
        if let Some((_, rest)) = take_digits(stripped) {
            rest.strip_prefix(':').unwrap_or(stripped)
        } else {
            stripped
        }
    } else {
        return None;
    };
    let message = after_col.trim().to_string();
    if message.is_empty() {
        return None;
    }
    let line_n: usize = line_s.parse().ok()?;
    let col_n: usize = col_s.parse().ok()?;
    if line_n == 0 || col_n == 0 || path_part.is_empty() {
        return None;
    }
    Some(crate::app::state::QuickfixEntry {
        path: resolve_path(path_part, cwd),
        line: line_n,
        col: col_n,
        text: message,
    })
}

/// `path(line,col): <message>` — tsc's default formatter.
fn parse_tsc_form(
    line: &str,
    cwd: &std::path::Path,
) -> Option<crate::app::state::QuickfixEntry> {
    let open = line.find('(')?;
    let close = line[open..].find(')').map(|i| open + i)?;
    let after = line.get(close + 1..)?.strip_prefix(':')?.trim();
    if after.is_empty() {
        return None;
    }
    let path_part = &line[..open];
    let inside = &line[open + 1..close];
    let (line_s, rest) = inside.split_once(',')?;
    let col_s = rest.trim();
    let line_n: usize = line_s.trim().parse().ok()?;
    let col_n: usize = col_s.parse().ok()?;
    if line_n == 0 || col_n == 0 || path_part.is_empty() {
        return None;
    }
    Some(crate::app::state::QuickfixEntry {
        path: resolve_path(path_part, cwd),
        line: line_n,
        col: col_n,
        text: after.to_string(),
    })
}

fn take_digits(s: &str) -> Option<(&str, &str)> {
    let end = s.bytes().position(|b| !b.is_ascii_digit()).unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    Some((&s[..end], &s[end..]))
}

fn resolve_path(s: &str, cwd: &std::path::Path) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(s);
    if p.is_absolute() { p } else { cwd.join(p) }
}

/// Single-quote a path for safe embedding in a shell command line.
/// Replaces any embedded `'` with `'\''` (close-quote, escaped-quote,
/// reopen-quote) — the standard POSIX trick. Used to build the `cd ...`
/// prefix without inviting injection from a weird project path.
fn shell_quote(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_wraps_in_single_quotes() {
        let q = shell_quote(std::path::Path::new("/tmp/x y"));
        assert_eq!(q, "'/tmp/x y'");
    }

    #[test]
    fn shell_quote_escapes_embedded_single_quote() {
        let q = shell_quote(std::path::Path::new("/tmp/it's"));
        assert_eq!(q, "'/tmp/it'\\''s'");
    }

    #[test]
    fn scrape_posix_form_basic() {
        let txt = "src/foo.rs:10:5: error[E0277]: the trait bound is not satisfied";
        let qf = scrape_task_errors(txt);
        assert_eq!(qf.len(), 1);
        assert_eq!(qf[0].line, 10);
        assert_eq!(qf[0].col, 5);
        assert!(qf[0].path.ends_with("src/foo.rs"));
        assert!(qf[0].text.starts_with("error[E0277]"));
    }

    #[test]
    fn scrape_posix_form_with_end_col() {
        let txt = "lib/parser.ts:42:7:42:18: error TS2304: Cannot find name 'foo'.";
        let qf = scrape_task_errors(txt);
        assert_eq!(qf.len(), 1);
        assert_eq!(qf[0].line, 42);
        assert_eq!(qf[0].col, 7);
        assert!(qf[0].text.contains("Cannot find name"));
    }

    #[test]
    fn scrape_tsc_form() {
        let txt = "src/index.ts(7,3): error TS2552: Cannot find name 'foo'.";
        let qf = scrape_task_errors(txt);
        assert_eq!(qf.len(), 1);
        assert_eq!(qf[0].line, 7);
        assert_eq!(qf[0].col, 3);
        assert!(qf[0].path.ends_with("src/index.ts"));
        assert!(qf[0].text.contains("Cannot find name"));
    }

    #[test]
    fn scrape_rustc_arrow_decoration() {
        // `-->` prefix is stripped so the underlying path:line:col parses.
        let txt = "   --> src/app.rs:120:9: expected `;`";
        let qf = scrape_task_errors(txt);
        assert_eq!(qf.len(), 1);
        assert_eq!(qf[0].line, 120);
        assert_eq!(qf[0].col, 9);
    }

    #[test]
    fn scrape_skips_log_timestamps() {
        // Avoid false positives on log lines like `12:34:56 INFO`.
        let txt = "12:34:56 INFO: starting up";
        let qf = scrape_task_errors(txt);
        assert!(qf.is_empty(), "log timestamp wrongly scraped as error: {:?}", qf);
    }

    #[test]
    fn scrape_dedupes_repeated_locations() {
        let txt = "\
src/foo.rs:10:5: error: one
src/foo.rs:10:5: error: dup
src/foo.rs:11:5: error: distinct";
        let qf = scrape_task_errors(txt);
        assert_eq!(qf.len(), 2);
    }

    #[test]
    fn scrape_multiple_distinct_errors() {
        let txt = "\
src/foo.rs:10:5: error: a
src/bar.rs(3,1): error: b
src/baz.rs:1:1: error: c";
        let qf = scrape_task_errors(txt);
        assert_eq!(qf.len(), 3);
    }

    #[test]
    fn is_long_running_dev() {
        let t = Task {
            label: "dev".into(),
            source: crate::task::TaskSource::NpmScripts,
            cwd: std::path::PathBuf::from("."),
            program: "pnpm".into(),
            args: vec!["dev".into()],
            description: None,
        };
        assert!(t.is_long_running());
    }

    #[test]
    fn is_long_running_build() {
        let t = Task {
            label: "build".into(),
            source: crate::task::TaskSource::NpmScripts,
            cwd: std::path::PathBuf::from("."),
            program: "pnpm".into(),
            args: vec!["build".into()],
            description: None,
        };
        assert!(!t.is_long_running());
    }

    #[test]
    fn is_long_running_word_boundary() {
        // "developer" contains "dev" as a substring but not as a word
        // token — heuristic shouldn't fire.
        let t = Task {
            label: "developer-mode".into(),
            source: crate::task::TaskSource::NpmScripts,
            cwd: std::path::PathBuf::from("."),
            program: "pnpm".into(),
            args: vec![],
            description: None,
        };
        assert!(!t.is_long_running(), "'developer-mode' shouldn't trip the dev heuristic");
    }
}
