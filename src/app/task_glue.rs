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
                let display = if desc.is_empty() {
                    format!("{:width$}  {}", t.source.tag(), t.label, width = tag_width)
                } else {
                    format!(
                        "{:width$}  {}  · {}",
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
        self.task_kickoff(task);
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
}
