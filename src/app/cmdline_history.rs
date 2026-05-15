//! Cmdline (`:`) and search (`/`) history — Up/Down recall within the
//! active prompt, persisted to the session file across launches.
//!
//! State lives on `App`:
//! - `cmd_history` / `search_history`: oldest first, most recent at the
//!   end. Cap enforced at `record` time so the file stays bounded.
//! - `history_cursor`: index into the active history while cycling;
//!   `None` means "below the most recent entry — showing the draft."
//! - `history_draft`: snapshot of `cmdline` taken on the first Up press
//!   so that walking off the bottom of history restores what the user
//!   had typed before they reached for recall.
//!
//! Edits (typing, Backspace) while cycling do NOT reset the cursor —
//! they just mutate `cmdline`. A subsequent Up walks one step further
//! back from the current cursor, discarding the edit. This matches
//! bash/readline's default Up/Down behaviour and avoids the surprise
//! of "I edited and then lost both my edit AND my history position."

/// Maximum entries we keep per history. Matches Vim's `'history'` default.
pub(super) const CMDLINE_HISTORY_CAP: usize = 100;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum HistoryKind {
    Command,
    Search,
}

impl super::App {
    fn history_vec_mut(&mut self, kind: HistoryKind) -> &mut Vec<String> {
        match kind {
            HistoryKind::Command => &mut self.cmd_history,
            HistoryKind::Search => &mut self.search_history,
        }
    }

    fn history_vec(&self, kind: HistoryKind) -> &Vec<String> {
        match kind {
            HistoryKind::Command => &self.cmd_history,
            HistoryKind::Search => &self.search_history,
        }
    }

    /// Append `entry` to the relevant history, dedup against the most
    /// recent entry, and trim to `CMDLINE_HISTORY_CAP`. Empty entries
    /// are ignored (the user typed `:` then Esc — nothing to remember).
    pub(super) fn history_record(&mut self, kind: HistoryKind, entry: &str) {
        if entry.is_empty() {
            return;
        }
        let hist = self.history_vec_mut(kind);
        if hist.last().map(String::as_str) == Some(entry) {
            return;
        }
        hist.push(entry.to_string());
        if hist.len() > CMDLINE_HISTORY_CAP {
            let drop = hist.len() - CMDLINE_HISTORY_CAP;
            hist.drain(..drop);
        }
    }

    /// `<Up>` inside cmdline / search — walk one entry older. First press
    /// snapshots the in-progress draft so a later walk off the bottom
    /// can restore it.
    pub(super) fn history_walk_back(&mut self, kind: HistoryKind) {
        let len = self.history_vec(kind).len();
        if len == 0 {
            return;
        }
        let new_cursor = match self.history_cursor {
            None => {
                self.history_draft = Some(self.cmdline.clone());
                len - 1
            }
            Some(0) => return,
            Some(n) => n - 1,
        };
        self.cmdline = self.history_vec(kind)[new_cursor].clone();
        self.history_cursor = Some(new_cursor);
    }

    /// `<Down>` inside cmdline / search — walk one entry newer. Past the
    /// most-recent entry, fall back to the draft snapshot (or empty if
    /// the user pressed Down without ever pressing Up).
    pub(super) fn history_walk_forward(&mut self, kind: HistoryKind) {
        let Some(cur) = self.history_cursor else {
            return;
        };
        let len = self.history_vec(kind).len();
        if cur + 1 < len {
            let n = cur + 1;
            self.cmdline = self.history_vec(kind)[n].clone();
            self.history_cursor = Some(n);
        } else {
            self.cmdline = self.history_draft.take().unwrap_or_default();
            self.history_cursor = None;
        }
    }

    /// Drop cycling state — call when leaving Command / Search mode by
    /// any path (Enter, Esc, Backspace-out, mode swap).
    pub(super) fn history_reset(&mut self) {
        self.history_cursor = None;
        self.history_draft = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn empty_app() -> App {
        App::new(None).expect("App::new should succeed without a path arg")
    }

    #[test]
    fn record_dedups_consecutive_duplicates() {
        let mut app = empty_app();
        app.history_record(HistoryKind::Command, "w");
        app.history_record(HistoryKind::Command, "w");
        app.history_record(HistoryKind::Command, "q");
        app.history_record(HistoryKind::Command, "q");
        assert_eq!(app.cmd_history, vec!["w", "q"]);
    }

    #[test]
    fn record_skips_empty() {
        let mut app = empty_app();
        app.history_record(HistoryKind::Command, "");
        app.history_record(HistoryKind::Search, "");
        assert!(app.cmd_history.is_empty());
        assert!(app.search_history.is_empty());
    }

    #[test]
    fn record_trims_to_cap() {
        let mut app = empty_app();
        for i in 0..CMDLINE_HISTORY_CAP + 25 {
            app.history_record(HistoryKind::Search, &format!("q{i}"));
        }
        assert_eq!(app.search_history.len(), CMDLINE_HISTORY_CAP);
        // Oldest 25 dropped — first remaining should be q25.
        assert_eq!(app.search_history[0], "q25");
        assert_eq!(
            app.search_history.last().map(String::as_str),
            Some(format!("q{}", CMDLINE_HISTORY_CAP + 24).as_str()),
        );
    }

    #[test]
    fn walk_back_then_forward_round_trips() {
        let mut app = empty_app();
        app.cmd_history = vec!["a".into(), "b".into(), "c".into()];
        app.cmdline = "draft".into();

        app.history_walk_back(HistoryKind::Command);
        assert_eq!(app.cmdline, "c");
        assert_eq!(app.history_cursor, Some(2));
        assert_eq!(app.history_draft.as_deref(), Some("draft"));

        app.history_walk_back(HistoryKind::Command);
        assert_eq!(app.cmdline, "b");

        app.history_walk_back(HistoryKind::Command);
        assert_eq!(app.cmdline, "a");
        assert_eq!(app.history_cursor, Some(0));

        // Past the top — no-op.
        app.history_walk_back(HistoryKind::Command);
        assert_eq!(app.cmdline, "a");

        app.history_walk_forward(HistoryKind::Command);
        assert_eq!(app.cmdline, "b");
        app.history_walk_forward(HistoryKind::Command);
        assert_eq!(app.cmdline, "c");
        // Past the bottom — restore draft, clear cursor.
        app.history_walk_forward(HistoryKind::Command);
        assert_eq!(app.cmdline, "draft");
        assert_eq!(app.history_cursor, None);
        assert_eq!(app.history_draft, None);
    }

    #[test]
    fn walk_forward_without_prior_walk_back_noops() {
        let mut app = empty_app();
        app.cmd_history = vec!["a".into()];
        app.cmdline = "fresh".into();
        app.history_walk_forward(HistoryKind::Command);
        assert_eq!(app.cmdline, "fresh");
        assert_eq!(app.history_cursor, None);
    }

    #[test]
    fn walk_back_on_empty_history_noops() {
        let mut app = empty_app();
        app.cmdline = "x".into();
        app.history_walk_back(HistoryKind::Command);
        assert_eq!(app.cmdline, "x");
        assert_eq!(app.history_cursor, None);
        assert_eq!(app.history_draft, None);
    }

    #[test]
    fn command_and_search_histories_are_independent() {
        let mut app = empty_app();
        app.history_record(HistoryKind::Command, "wq");
        app.history_record(HistoryKind::Search, "foo");
        assert_eq!(app.cmd_history, vec!["wq"]);
        assert_eq!(app.search_history, vec!["foo"]);
    }
}
