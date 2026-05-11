//! Glue between the parsed git hunks (`App.git_hunks`) and user-facing
//! actions: `]h` / `[h` navigation, `<leader>hp` preview. Stage 3 will
//! grow this into stage/unstage/reset.

use crate::app::state::HoverState;

impl super::App {
    /// Jump the cursor to the next (or previous) git hunk in the active
    /// buffer. Wraps around the buffer if no hunk lies in the requested
    /// direction. No-op when the buffer has no hunks.
    pub(super) fn hunk_jump(&mut self, forward: bool) {
        if self.git_hunks.is_empty() {
            self.status_msg = "no git hunks in this buffer".into();
            return;
        }
        let here = self.cursor.line;
        let target = if forward {
            self.git_hunks.iter().find(|h| h.start_line > here)
        } else {
            self.git_hunks.iter().rev().find(|h| h.end_line < here)
        };
        match target {
            Some(h) => {
                let line = h.start_line.min(self.buffer.line_count().saturating_sub(1));
                self.push_jump();
                self.cursor.line = line;
                self.cursor.col = 0;
                self.cursor.want_col = 0;
            }
            None => {
                self.status_msg = if forward {
                    "no more hunks below".into()
                } else {
                    "no more hunks above".into()
                };
            }
        }
    }

    /// Show the unified-diff hunk under the cursor in a hover popup.
    /// Re-runs `git diff -U3` so the popup carries three lines of
    /// surrounding context, then slices out the hunk whose new-side
    /// range covers the cursor.
    pub(super) fn hunk_preview(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "no path: open a file first".into();
            return;
        };
        let line_one_based = self.cursor.line + 1;
        let in_hunk = self
            .git_hunks
            .iter()
            .any(|h| self.cursor.line >= h.start_line && self.cursor.line <= h.end_line);
        if !in_hunk {
            self.status_msg = "no hunk under cursor".into();
            return;
        }
        match crate::git::hunk_text_for_line(&path, line_one_based) {
            Some(text) if !text.trim().is_empty() => {
                // Wrap as a fenced markdown code block so the existing
                // hover renderer treats it as syntax-coloured code.
                let wrapped = format!("```diff\n{}\n```", text.trim_end());
                self.hover = HoverState::from_lsp_text(&wrapped, self.width as usize, true);
                if self.hover.is_none() {
                    self.status_msg = "git: preview empty".into();
                }
            }
            _ => {
                self.status_msg = "git: no preview available".into();
            }
        }
    }
}
