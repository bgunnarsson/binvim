//! Window-split actions: `<C-w>` v/s/h/j/k/l/q/c/o/=.
//!
//! The active window's view state lives on `App.window` (live), every
//! other window's state is stashed in `App.windows` keyed by
//! `WindowId`. Layout owns the split tree; we just hand it the focused
//! id and let it mutate the tree, then move state between live and
//! stashed slots to match the new active window.

use crate::layout::{FocusDir, SplitDir};

impl super::App {
    /// `<C-w>v` / `<C-w>s` — split the active window. The new window
    /// inherits the focused one's buffer + cursor + viewport so both
    /// panes start in sync, and focus moves to the new pane (matching
    /// Vim's `:vsplit` / `:split` default).
    pub(super) fn window_split(&mut self, dir: SplitDir) {
        let new_id = self.layout.alloc_id();
        if !self.layout.split(self.active_window, dir, new_id) {
            self.status_msg = "split failed: active window not in layout".into();
            return;
        }
        // The active live state stays on App.window; we stash a clone
        // for the *new* pane so its independent cursor / view survive
        // when focus eventually moves there.
        self.windows.insert(new_id, self.window.clone());
        // Vim moves the cursor to the freshly-opened pane.
        self.focus_window(new_id);
    }

    /// `<C-w>h/j/k/l` — focus the spatially-nearest neighbouring
    /// window in `dir`. No-op when the active window is on the
    /// requested edge.
    pub(super) fn window_focus(&mut self, dir: FocusDir) {
        let editor_rect = self.editor_rect();
        let Some(target) = self
            .layout
            .focus_neighbor(self.active_window, dir, editor_rect)
        else {
            return;
        };
        self.focus_window(target);
    }

    /// Swap the live `App.window` with the stash for `target`. Updates
    /// `active_window` so the next render places the cursor in the
    /// right pane. If the target window points at a different buffer
    /// than the currently-live one, also stash/load the buffer-level
    /// state via `switch_to` so App's live fields (buffer, history,
    /// folds, highlight cache, git hunks, …) match the new focus.
    fn focus_window(&mut self, target: crate::layout::WindowId) {
        if target == self.active_window {
            return;
        }
        let old_id = self.active_window;
        // Stash current live window state into the slot for the outgoing window.
        let outgoing = std::mem::take(&mut self.window);
        self.windows.insert(old_id, outgoing);
        // Pull in the incoming window's stash and adopt it as the live one.
        let incoming = self
            .windows
            .remove(&target)
            .expect("focus target has no stashed window");
        let target_buffer = incoming.buffer_idx;
        self.window = incoming;
        self.active_window = target;
        // If the new focus shows a different buffer, swap live buffer
        // state. `switch_to` handles the snapshot/load dance and resets
        // window.buffer_idx — but it would overwrite the freshly-pulled
        // cursor/viewport too, so first cache them and reapply after.
        if target_buffer != self.active {
            let cursor = self.window.cursor;
            let view_top = self.window.view_top;
            let view_left = self.window.view_left;
            let visual_anchor = self.window.visual_anchor;
            if let Err(e) = self.switch_to(target_buffer) {
                self.status_msg = format!("error: {e}");
                return;
            }
            self.window.cursor = cursor;
            self.window.view_top = view_top;
            self.window.view_left = view_left;
            self.window.visual_anchor = visual_anchor;
        }
    }

    /// `<C-w>q` / `<C-w>c` — close the active window. Refuses if it's
    /// the last one (use `:q` to quit the editor in that case). The
    /// sibling that absorbed the closed pane's space becomes the new
    /// focus, matching Vim's behaviour. If that sibling shows a
    /// different buffer than the active one, buffer-level state on
    /// `App` is swapped to match.
    pub(super) fn window_close(&mut self) {
        let target = self.active_window;
        let Some(new_focus) = self.layout.close(target) else {
            self.status_msg = "E444: cannot close last window".into();
            return;
        };
        // Stash slot for the closed window is no longer reachable.
        self.windows.remove(&target);
        // The new-focus window's stash holds its view state — swap it
        // onto App.window, then run a buffer swap if its buffer_idx
        // differs from the one we're leaving behind.
        let incoming = self
            .windows
            .remove(&new_focus)
            .expect("post-close focus has no stashed window");
        let target_buffer = incoming.buffer_idx;
        self.window = incoming;
        self.active_window = new_focus;
        if target_buffer != self.active {
            let cursor = self.window.cursor;
            let view_top = self.window.view_top;
            let view_left = self.window.view_left;
            let visual_anchor = self.window.visual_anchor;
            if let Err(e) = self.switch_to(target_buffer) {
                self.status_msg = format!("error: {e}");
                return;
            }
            self.window.cursor = cursor;
            self.window.view_top = view_top;
            self.window.view_left = view_left;
            self.window.visual_anchor = visual_anchor;
        }
    }

    /// `<C-w>o` — close every window except the active one.
    pub(super) fn window_only(&mut self) {
        let keep = self.active_window;
        let dropped = self.layout.only(keep);
        for id in dropped {
            self.windows.remove(&id);
        }
    }

    /// Called by `delete_buffer` after a `buffers.remove(removed)` and
    /// any `self.active` adjustment: re-points every Window's
    /// `buffer_idx` so it stays consistent with the now-shifted buffer
    /// list. Live `App.window` snaps to `self.active`; stashed windows
    /// that referenced the removed slot fall back to `self.active`,
    /// and any with an index past the removed one are decremented.
    pub(super) fn remap_windows_after_remove(&mut self, removed: usize) {
        self.window.buffer_idx = self.active;
        let new_active = self.active;
        for w in self.windows.values_mut() {
            if w.buffer_idx == removed {
                w.buffer_idx = new_active;
            } else if w.buffer_idx > removed {
                w.buffer_idx -= 1;
            }
        }
    }

    /// Called by `delete_all_buffers` / `buffer_only` / similar
    /// "collapse the buffer list" operations. Every Window — live and
    /// stashed — is pointed at `target` (typically 0 for "fresh empty
    /// seed" or `self.active` for "kept the active one").
    pub(super) fn remap_windows_to_single(&mut self, target: usize) {
        self.window.buffer_idx = target;
        for w in self.windows.values_mut() {
            w.buffer_idx = target;
        }
    }
}
