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
        }
    }

    pub(super) fn apply_visual_operate(&mut self, op: Operator, target: Option<char>) {
        let kind = match self.mode {
            Mode::Visual(k) => k,
            _ => return,
        };
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
