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
    /// right pane.
    fn focus_window(&mut self, target: crate::layout::WindowId) {
        if target == self.active_window {
            return;
        }
        let old_id = self.active_window;
        // Stash current live state into the slot for the outgoing window.
        let outgoing = std::mem::take(&mut self.window);
        self.windows.insert(old_id, outgoing);
        // Pull in the incoming window's stash.
        let incoming = self
            .windows
            .remove(&target)
            .expect("focus target has no stashed window");
        self.window = incoming;
        self.active_window = target;
    }

    /// `<C-w>q` / `<C-w>c` — close the active window. Refuses if it's
    /// the last one (use `:q` to quit the editor in that case). The
    /// sibling that absorbed the closed pane's space becomes the new
    /// focus, matching Vim's behaviour.
    pub(super) fn window_close(&mut self) {
        let target = self.active_window;
        let Some(new_focus) = self.layout.close(target) else {
            self.status_msg = "E444: cannot close last window".into();
            return;
        };
        // Stash slot for the closed window is no longer reachable.
        self.windows.remove(&target);
        // The new-focus window's stash holds its view state — swap it in
        // to become the live one, replacing the closed window's old
        // live state on App.window.
        let incoming = self
            .windows
            .remove(&new_focus)
            .expect("post-close focus has no stashed window");
        self.window = incoming;
        self.active_window = new_focus;
    }

    /// `<C-w>o` — close every window except the active one.
    pub(super) fn window_only(&mut self) {
        let keep = self.active_window;
        let dropped = self.layout.only(keep);
        for id in dropped {
            self.windows.remove(&id);
        }
    }
}
