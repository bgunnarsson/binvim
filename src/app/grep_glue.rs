//! `<leader>fg` grep picker — debounced, backgrounded ripgrep.
//!
//! The picker re-searches on every keystroke, which is why none of this runs
//! inline in the key handler. A two-character query over a large workspace
//! emits hundreds of megabytes of matches; searching it synchronously on the
//! main thread froze the editor for as long as ripgrep took, with no way to
//! cancel and no repaint in between. Three guards keep it bounded now: a
//! debounce so a typing burst spawns one search rather than one per character,
//! a background thread so the UI keeps painting, and a kill on the in-flight
//! child so a superseded query stops scanning immediately.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::picker::{self, PickerKind};

use super::state::GrepEvent;

/// How long the query must sit still before a search fires. Matches the
/// package-search debounce — long enough that ordinary typing produces one
/// search, short enough not to feel laggy.
const GREP_DEBOUNCE: Duration = Duration::from_millis(350);

/// Shortest query we'll search for. One or two characters match in essentially
/// every file, so the result set is noise and the scan is at its most
/// expensive — the two cases coincide.
const GREP_MIN_LEN: usize = 3;

/// Rows kept from one run. Hitting this kills the child mid-scan, so the cost
/// of a broad query is bounded by this number rather than by its match count.
const GREP_MAX_RESULTS: usize = 500;

impl super::App {
    /// Record that the grep picker's query changed; `grep_tick` fires the
    /// debounced search from the main loop.
    pub(super) fn grep_mark_dirty(&mut self) {
        self.grep.dirty_at = Some(Instant::now());
        // Any in-flight scan is for a query the user has already typed past.
        self.grep_cancel_inflight();
        // Drop rows from the previous query immediately rather than leaving
        // them under the new input until the debounce elapses — they belong to
        // a search the user has moved on from, and backspacing below the
        // minimum length would otherwise strand them on screen for good.
        let short = self
            .picker
            .as_ref()
            .is_some_and(|p| p.input.chars().count() < GREP_MIN_LEN);
        if let Some(p) = self.picker.as_mut() {
            picker::replace_items(p, Vec::new());
            p.title = if short {
                Self::grep_prompt_title()
            } else {
                "Grep".into()
            };
        }
    }

    /// Title for a grep picker with nothing to show yet.
    pub(super) fn grep_empty_title(&self) -> String {
        Self::grep_prompt_title()
    }

    fn grep_prompt_title() -> String {
        format!("Grep (type ≥{GREP_MIN_LEN} chars)")
    }

    /// Fire the debounced search once the grep picker's query has settled.
    /// Returns `true` if a search was kicked off.
    pub(super) fn grep_tick(&mut self) -> bool {
        let due = matches!(self.picker.as_ref().map(|p| p.kind), Some(PickerKind::Grep))
            && self
                .grep
                .dirty_at
                .is_some_and(|t| Instant::now() >= t + GREP_DEBOUNCE);
        if !due {
            return false;
        }
        self.grep.dirty_at = None;
        let query = self
            .picker
            .as_ref()
            .map(|p| p.input.clone())
            .unwrap_or_default();
        if query.chars().count() < GREP_MIN_LEN {
            return false;
        }
        self.grep_spawn(query);
        true
    }

    fn grep_spawn(&mut self, query: String) {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.grep.epoch += 1;
        self.grep.busy = true;
        let epoch = self.grep.epoch;
        let tx = self.grep.tx.clone();
        // Both sides hold the same slot: the thread publishes its child into
        // it and reaps from it, the main thread reaches in only to kill.
        let child: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));
        self.grep.child = Some(Arc::clone(&child));
        if let Some(p) = self.picker.as_mut() {
            p.title = format!("Grep: {query} (searching…)");
        }
        thread::spawn(move || {
            let items = picker::run_ripgrep(&query, &cwd, GREP_MAX_RESULTS, &child);
            let _ = tx.send(GrepEvent {
                epoch,
                query,
                items,
            });
        });
    }

    /// Kill the running ripgrep, if any, and bump the epoch so its result is
    /// discarded when it lands. The child is left in the slot for its own
    /// thread to reap — `wait` never runs on the main thread.
    fn grep_cancel_inflight(&mut self) {
        let Some(slot) = self.grep.child.take() else {
            return;
        };
        self.grep.epoch += 1;
        self.grep.busy = false;
        if let Ok(mut guard) = slot.lock()
            && let Some(child) = guard.as_mut()
        {
            let _ = child.kill();
        }
    }

    /// Tear down all grep state — called when the picker closes so a search
    /// started by a query the user abandoned doesn't keep scanning.
    pub(super) fn grep_cancel(&mut self) {
        self.grep.dirty_at = None;
        self.grep_cancel_inflight();
    }

    /// Drain finished searches into the picker. Returns `true` if anything
    /// changed and the frame needs a repaint.
    /// `:grep[!] args` — `rg --vimgrep args` through the shell, as Vim runs
    /// 'grepprg', so quoting and globs read the same; the matches become the
    /// quickfix list and, without `!`, the first is jumped to.
    pub(super) fn grep_command(&mut self, args: &str, jump: bool) {
        let args = match self.expand_file_names(args) {
            Ok(args) => args,
            Err(e) => {
                self.status_msg = e;
                return;
            }
        };
        let output = match crate::format::run_shell(&format!("rg --vimgrep {args}"), "") {
            Ok(output) => output,
            Err(e) => {
                self.status_msg = e;
                return;
            }
        };
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let entries = super::quickfix::vimgrep_entries(&output.text, &cwd);
        if entries.is_empty() {
            self.quickfix = None;
            let first = output.text.lines().next().unwrap_or("");
            // ripgrep exits 1 when nothing matched; anything else failed.
            self.status_msg = match output.failed {
                Some(code) if code != 1 => format!("grep: shell returned {code}: {first}"),
                _ => format!("E480: No match: {args}"),
            };
            return;
        }
        self.qf_replace(entries, jump);
    }

    /// `:vimgrep /pat/[g][j] files` — a Vim pattern, translated for ripgrep
    /// as `:S` does, over `files`. A file with `*`, `?` or `[` in it is a glob
    /// over the working directory rather than a path. Without `g` only a
    /// line's first match counts, as in Vim.
    pub(super) fn vimgrep_command(&mut self, pattern: &str, files: &str, all: bool, jump: bool) {
        let source = match self
            .pattern_or_last(pattern)
            .and_then(|pattern| super::search::search_source(&pattern))
        {
            Ok(source) => source,
            Err(e) => {
                self.status_msg = e;
                return;
            }
        };
        let files = match self.expand_file_names(files) {
            Ok(files) => files,
            Err(e) => {
                self.status_msg = e;
                return;
            }
        };
        let (globs, mut paths): (Vec<&str>, Vec<&str>) = files
            .split_whitespace()
            .partition(|file| file.contains(['*', '?', '[']));
        if globs.is_empty() && paths.is_empty() {
            self.status_msg = "E683: File name missing or invalid pattern".into();
            return;
        }
        if paths.is_empty() {
            paths.push(".");
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut rg = std::process::Command::new("rg");
        rg.args(["--vimgrep", "--color=never", "--no-messages"]);
        for glob in &globs {
            rg.arg("--glob").arg(glob);
        }
        let output = rg
            .arg("-e")
            .arg(&source)
            .arg("--")
            .args(&paths)
            .current_dir(&cwd)
            .output();
        let Ok(output) = output else {
            self.status_msg = "vimgrep: ripgrep not on PATH".into();
            return;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let mut entries = super::quickfix::vimgrep_entries(&text, &cwd);
        if !all {
            entries.dedup_by(|b, a| a.path == b.path && a.line == b.line);
        }
        if entries.is_empty() {
            self.quickfix = None;
            self.status_msg = format!("E480: No match: {pattern}");
            return;
        }
        self.qf_replace(entries, jump);
    }

    pub(super) fn handle_grep_events(&mut self) -> bool {
        let mut progress = false;
        while let Ok(ev) = self.grep.rx.try_recv() {
            // Drop results for a superseded query — the user has typed on.
            if ev.epoch != self.grep.epoch {
                continue;
            }
            progress = true;
            self.grep.busy = false;
            self.grep.child = None;
            // The picker may have closed or switched kind while the search ran.
            let Some(p) = self.picker.as_mut() else {
                continue;
            };
            if p.kind != PickerKind::Grep {
                continue;
            }
            let n = ev.items.len();
            p.title = if n == 0 {
                format!("Grep: {} (no matches)", ev.query)
            } else if n >= GREP_MAX_RESULTS {
                // Hit the cap, so the scan was cut short — say so rather than
                // implying these are all the matches in the workspace.
                format!("Grep: {} (first {n})", ev.query)
            } else {
                format!("Grep: {} ({n})", ev.query)
            };
            picker::replace_items(p, ev.items);
        }
        progress
    }
}
