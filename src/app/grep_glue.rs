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
