//! Visual-mode helpers — entering/exiting visual, computing the selected
//! char range, and applying operators on a visual selection.

use crate::cursor::Cursor;
use crate::mode::{Mode, Operator, VisualKind};
use crate::text_object::{self, TextObjectVerb};

impl super::App {
    pub(super) fn exit_visual(&mut self) {
        self.mode = Mode::Normal;
        self.visual_anchor = None;
    }

    pub(super) fn visual_range_chars(&self, kind: VisualKind) -> (usize, usize, bool) {
        let anchor = self.visual_anchor.unwrap_or(self.cursor);
        match kind {
            VisualKind::Char => {
                let a = self.buffer.pos_to_char(anchor.line, anchor.col);
                let c = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
                let (lo, hi) = if a <= c { (a, c) } else { (c, a) };
                let total = self.buffer.total_chars();
                (lo, (hi + 1).min(total), false)
            }
            VisualKind::Line => {
                let l1 = anchor.line.min(self.cursor.line);
                let l2 = anchor.line.max(self.cursor.line);
                let s = self.buffer.line_start_idx(l1);
                let e = self.buffer.line_start_idx(l2 + 1);
                let total = self.buffer.total_chars();
                let extend = e == total && l1 > 0;
                let s_eff = if extend { s - 1 } else { s };
                (s_eff, e, true)
            }
            VisualKind::Block => {
                // Block ranges are non-contiguous; the d/c/y path bypasses
                // this fn entirely (see `apply_block_operate`). Surround on
                // a block selection falls back here — give it the coarse
                // anchor-to-cursor char range so it does *something* rather
                // than crashing.
                let a = self.buffer.pos_to_char(anchor.line, anchor.col);
                let c = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
                let (lo, hi) = if a <= c { (a, c) } else { (c, a) };
                let total = self.buffer.total_chars();
                (lo, (hi + 1).min(total), false)
            }
        }
    }

    pub(super) fn apply_visual_operate(&mut self, op: Operator, target: Option<char>) {
        let kind = match self.mode {
            Mode::Visual(k) => k,
            _ => return,
        };
        // Block selection is non-contiguous — handle it on its own track.
        if matches!(kind, VisualKind::Block) {
            self.apply_block_operate(op, target);
            return;
        }
        // Indent / outdent take only the line span and ignore column boundaries.
        // Crucially, keep the selection alive afterwards so the user can keep
        // hammering > / < to indent further without re-selecting.
        if matches!(op, Operator::Indent | Operator::Outdent) {
            let anchor = self.visual_anchor.unwrap_or(self.cursor);
            let saved_anchor_line = anchor.line;
            let saved_anchor_col = anchor.col;
            let saved_cursor_line = self.cursor.line;
            let saved_cursor_col = self.cursor.col;
            let l1 = saved_anchor_line.min(saved_cursor_line);
            let l2 = saved_anchor_line.max(saved_cursor_line);
            if matches!(op, Operator::Indent) {
                self.indent_lines(l1, l2);
            } else {
                self.outdent_lines(l1, l2);
            }
            // Restore the selection: same lines, columns clamped to whatever the
            // shift left of them. The line range is what matters for indent.
            let anchor_max = self.buffer.line_len(saved_anchor_line).saturating_sub(1);
            let cursor_max = self.buffer.line_len(saved_cursor_line).saturating_sub(1);
            self.visual_anchor = Some(Cursor {
                line: saved_anchor_line,
                col: saved_anchor_col.min(anchor_max),
                want_col: saved_anchor_col.min(anchor_max),
            });
            self.cursor.line = saved_cursor_line;
            self.cursor.col = saved_cursor_col.min(cursor_max);
            self.cursor.want_col = self.cursor.col;
            return;
        }
        let (start, end, linewise) = self.visual_range_chars(kind);
        if end <= start {
            self.exit_visual();
            return;
        }
        let removed = self.buffer.rope.slice(start..end).to_string();
        match op {
            Operator::Yank => {
                self.write_yank_register(target, removed, linewise);
                self.flash_yank(start, end);
                self.cursor_to_idx(start);
                self.clamp_cursor_normal();
                self.exit_visual();
            }
            Operator::Delete => {
                self.write_register(target, removed, linewise);
                self.buffer.delete_range(start, end);
                self.cursor_to_idx(start);
                self.clamp_cursor_normal();
                self.exit_visual();
            }
            Operator::Change => {
                self.write_register(target, removed, linewise);
                self.buffer.delete_range(start, end);
                if linewise {
                    self.buffer.insert_at_idx(start, "\n");
                }
                self.cursor_to_idx(start);
                self.mode = Mode::Insert;
                self.visual_anchor = None;
            }
            Operator::Indent | Operator::Outdent => unreachable!(),
        }
    }

    /// Apply an operator to the current visual-block selection. Block
    /// operations delete / yank / change the rectangular column range on
    /// each row of the line span. Indent / outdent on a block fall back to
    /// the line range it covers — matching what users typically want from
    /// `>` on a `Ctrl-V` selection.
    fn apply_block_operate(&mut self, op: Operator, target: Option<char>) {
        let anchor = self.visual_anchor.unwrap_or(self.cursor);
        let l1 = anchor.line.min(self.cursor.line);
        let l2 = anchor.line.max(self.cursor.line);
        let c1 = anchor.col.min(self.cursor.col);
        let c2 = anchor.col.max(self.cursor.col);

        if matches!(op, Operator::Indent) {
            self.indent_lines(l1, l2);
            return;
        }
        if matches!(op, Operator::Outdent) {
            self.outdent_lines(l1, l2);
            return;
        }

        // Build the yanked text by snapping the column slice on every line.
        // Lines shorter than `c1` contribute an empty row, matching Vim.
        let mut chunks: Vec<String> = Vec::with_capacity(l2 - l1 + 1);
        for line in l1..=l2 {
            let line_len = self.buffer.line_len(line);
            let start = c1.min(line_len);
            let end = (c2 + 1).min(line_len);
            if end <= start {
                chunks.push(String::new());
                continue;
            }
            let line_start = self.buffer.line_start_idx(line);
            let slice: String = self
                .buffer
                .rope
                .slice((line_start + start)..(line_start + end))
                .to_string();
            chunks.push(slice);
        }
        let removed = chunks.join("\n");

        match op {
            Operator::Yank => {
                self.write_yank_register(target, removed, false);
                // Visual flash only covers the first line — better than no
                // feedback, and a true block flash would need a renderer
                // change we haven't done.
                let line_start = self.buffer.line_start_idx(l1);
                let first_line_len = self.buffer.line_len(l1);
                let start = (line_start + c1).min(line_start + first_line_len);
                let end = (line_start + c2 + 1).min(line_start + first_line_len);
                self.flash_yank(start, end);
                self.cursor.line = l1;
                self.cursor.col = c1.min(first_line_len.saturating_sub(1));
                self.cursor.want_col = self.cursor.col;
                self.exit_visual();
            }
            Operator::Delete | Operator::Change => {
                self.write_register(target, removed, false);
                // Iterate bottom-up so a per-line delete doesn't shift the
                // start index of higher lines.
                for line in (l1..=l2).rev() {
                    let line_len = self.buffer.line_len(line);
                    let start = c1.min(line_len);
                    let end = (c2 + 1).min(line_len);
                    if end > start {
                        let line_start = self.buffer.line_start_idx(line);
                        self.buffer
                            .delete_range(line_start + start, line_start + end);
                    }
                }
                self.cursor.line = l1;
                let new_len = self.buffer.line_len(l1);
                self.cursor.col = c1.min(new_len.saturating_sub(1));
                self.cursor.want_col = self.cursor.col;
                if matches!(op, Operator::Change) {
                    self.mode = Mode::Insert;
                    self.visual_anchor = None;
                } else {
                    self.exit_visual();
                }
            }
            Operator::Indent | Operator::Outdent => unreachable!(),
        }
    }

    pub(super) fn apply_visual_select_textobj(&mut self, obj: TextObjectVerb) {
        let range = match text_object::compute(&self.buffer, self.cursor, obj) {
            Some(r) => r,
            None => return,
        };
        // Anchor → start, cursor → end-1 (inclusive endpoint for visual).
        self.cursor_to_idx(range.start);
        let anchor = self.cursor;
        let end_idx = range.end.saturating_sub(1).max(range.start);
        self.cursor_to_idx(end_idx);
        self.visual_anchor = Some(anchor);
    }
}
