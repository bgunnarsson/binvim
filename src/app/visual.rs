//! Visual-mode helpers — entering/exiting visual, computing the selected
//! char range, and applying operators on a visual selection.

use crate::cursor::Cursor;
use crate::mode::{Mode, Operator, VisualKind};
use crate::text_object::{self, TextObjectVerb};

impl super::App {
    pub(super) fn exit_visual(&mut self) {
        self.mode = Mode::Normal;
        self.visual_anchor = None;
        self.additional_selections.clear();
    }

    /// `Ctrl-N` in Visual-char mode — find the next literal-text match of
    /// the current primary selection and add it as an additional
    /// selection. The primary cursor jumps to the new occurrence (so a
    /// rapid sequence of `Ctrl-N` adds one occurrence per press, walking
    /// the buffer); the previous primary selection is stored in
    /// `additional_selections`. No-ops outside Visual-char.
    pub(super) fn add_next_occurrence_selection(&mut self) {
        let kind = match self.mode {
            Mode::Visual(k) => k,
            _ => return,
        };
        if !matches!(kind, VisualKind::Char) {
            self.status_msg = "Ctrl-N: only in Visual-char".into();
            return;
        }
        let (start, end, _linewise) = self.visual_range_chars(kind);
        if end <= start {
            return;
        }
        let needle = self.buffer.rope.slice(start..end).to_string();
        if needle.is_empty() {
            return;
        }
        // Search forward from one past the end of the current selection,
        // wrapping back to the start so the user can cycle through all
        // occurrences. Wrap is bounded: if the next hit is the same
        // range we already have (only one match in the buffer) we bail.
        let total = self.buffer.total_chars();
        let from = end.min(total);
        let needle_chars = needle.chars().count();
        let hit = self.search(&needle, from, true, true);
        let Some(hit_start) = hit else {
            self.status_msg = format!("No more occurrences of \"{needle}\"");
            return;
        };
        let hit_end = (hit_start + needle_chars).min(total);
        // Don't add the same range twice — e.g. when there's only one
        // match in the buffer and search wraps back to it.
        if hit_start == start && hit_end == end {
            self.status_msg = format!("Only one occurrence of \"{needle}\"");
            return;
        }
        // Save the current primary as an extra selection, then move the
        // primary to the new occurrence.
        let primary_range = (start, end);
        if !self.additional_selections.contains(&primary_range) {
            self.additional_selections.push(primary_range);
        }
        self.additional_selections.sort();
        self.additional_selections.dedup();
        // Anchor at hit_start, cursor at hit_end - 1 (inclusive end for visual).
        self.cursor_to_idx(hit_start);
        let anchor = self.cursor;
        self.cursor_to_idx(hit_end.saturating_sub(1).max(hit_start));
        self.visual_anchor = Some(anchor);
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
        // Multi-selection (Ctrl-N): operator applies to every range —
        // primary plus each stored `additional_selections`. Indent /
        // outdent fall through to the single-selection line path below.
        if !self.additional_selections.is_empty()
            && !matches!(op, Operator::Indent | Operator::Outdent)
        {
            self.apply_multi_selection_operate(op, target);
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

    /// Apply d/c/y to every selection — primary + every entry in
    /// `additional_selections`. Bottom-up edit order keeps the lower
    /// ranges' indices stable across the loop. For `Change`, the
    /// resulting Insert mode picks up `additional_cursors` populated
    /// with each former selection's start position so subsequent typing
    /// mirrors at every site.
    fn apply_multi_selection_operate(&mut self, op: Operator, target: Option<char>) {
        let kind = match self.mode {
            Mode::Visual(k) => k,
            _ => return,
        };
        // Gather all ranges. The primary is computed from the current
        // anchor + cursor; additional ones are stored as (start, end).
        let (p_start, p_end, _linewise) = self.visual_range_chars(kind);
        let mut ranges: Vec<(usize, usize)> = vec![(p_start, p_end)];
        for r in &self.additional_selections {
            if r.0 != p_start || r.1 != p_end {
                ranges.push(*r);
            }
        }
        ranges.sort_by_key(|r| r.0);
        ranges.dedup();
        // Concatenated removed-text for the register, in document order.
        let mut texts: Vec<String> = Vec::with_capacity(ranges.len());
        for &(s, e) in &ranges {
            if e > s {
                texts.push(self.buffer.rope.slice(s..e).to_string());
            } else {
                texts.push(String::new());
            }
        }
        let removed_joined = texts.join("\n");

        match op {
            Operator::Yank => {
                self.write_yank_register(target, removed_joined, false);
                // Flash the primary range only — multi-range flash would
                // need a renderer change.
                self.flash_yank(p_start, p_end);
                self.additional_selections.clear();
                self.cursor_to_idx(p_start);
                self.clamp_cursor_normal();
                self.exit_visual();
            }
            Operator::Delete | Operator::Change => {
                self.write_register(target, removed_joined, false);
                // Delete bottom-up so earlier (lower-indexed) ranges stay valid.
                for &(s, e) in ranges.iter().rev() {
                    if e > s {
                        self.buffer.delete_range(s, e);
                    }
                }
                // The lowest range's start is now where the primary cursor
                // lands; every other range's start (after the bottom-up
                // delete sequence) becomes an additional cursor anchored
                // at its own former start. Bottom-up deletes don't shift
                // indices below the cut, so each range's `s` is still
                // valid as the post-delete cursor location.
                let mut starts: Vec<usize> = ranges.iter().map(|r| r.0).collect();
                starts.sort();
                starts.dedup();
                let primary = starts[0];
                let extras: Vec<usize> = starts.into_iter().skip(1).collect();
                self.cursor_to_idx(primary);
                self.additional_cursors = extras;
                self.additional_selections.clear();
                self.clamp_cursor_normal();
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

    /// Per-line char-column ranges from `additional_selections` that
    /// intersect `line`. Used by the renderer to paint multi-selection
    /// highlights alongside the primary selection.
    pub fn line_extra_selections(&self, line: usize) -> Vec<(usize, usize)> {
        if self.additional_selections.is_empty() {
            return Vec::new();
        }
        let line_start = self.buffer.line_start_idx(line);
        let line_end = line_start + self.buffer.line_len(line);
        let mut out = Vec::new();
        for &(s, e) in &self.additional_selections {
            if e <= line_start || s >= line_end {
                continue;
            }
            let cs = s.saturating_sub(line_start);
            let ce = e.min(line_end).saturating_sub(line_start);
            if ce > cs {
                out.push((cs, ce));
            }
        }
        out
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
