//! Multi-cursor fan-out for operators in Normal mode.
//!
//! When the user has `additional_cursors` populated (Ctrl-click in
//! Insert mode + Esc, or `c` out of a multi-selection Visual block),
//! the high-traffic operators — d/c/y on word motions / text objects /
//! whole lines, plus `x` — should fire at every cursor, not just the
//! primary. Insert-mode mirroring (typing, Backspace, Enter) already
//! works; this module fills the Normal-mode gap.
//!
//! The flow is uniform across entry points:
//!   1. Build one `(start, end)` range per cursor.
//!   2. Pre-compute each range's landing position in the post-delete
//!      buffer — `original_start - chars_deleted_before_it`.
//!   3. Delete ranges in descending order so each `delete_range` call
//!      sees the buffer in its original coordinate system.
//!   4. Re-seat the primary at landings[0] and the additional cursors
//!      at landings[1..]. For Change, enter Insert mode — the existing
//!      Insert-mode multi-cursor mirroring then takes over.
//!
//! Overlapping ranges are deduplicated (the leftmost-starting one wins)
//! before the deletion loop so a cursor at the end of one word and the
//! start of the next doesn't get the gap clobbered twice.

use crate::buffer::Buffer;
use crate::cursor::Cursor;
use crate::mode::{Mode, Operator};
use crate::motion::{self, MotionKind, MotionResult};
use crate::parser::MotionVerb;
use crate::text_object::{self, TextObjectVerb};

impl super::App {
    /// Try the multi-cursor path for `Action::Operate { op, motion, … }`.
    /// Returns `true` if it handled the operator (caller skips the
    /// single-cursor fallback). Falls through (returns `false`) when no
    /// additional cursors are active, the operator isn't D/C/Y, or the
    /// motion isn't one the multi-cursor path supports.
    pub(super) fn try_multi_op_motion(
        &mut self,
        op: Operator,
        motion: MotionVerb,
        count: usize,
        register: Option<char>,
    ) -> bool {
        if self.additional_cursors.is_empty() {
            return false;
        }
        if !matches!(op, Operator::Delete | Operator::Change | Operator::Yank) {
            return false;
        }
        let mut ranges = Vec::with_capacity(self.additional_cursors.len() + 1);
        for cur in self.all_cursors() {
            let Some(m) = run_motion_stateless(&self.buffer, cur, motion, count) else {
                return false; // unsupported motion — bail to single-cursor path
            };
            ranges.push(range_from_motion_at(&self.buffer, cur, m));
        }
        self.apply_multi_op(op, ranges, register, false);
        true
    }

    pub(super) fn try_multi_op_textobj(
        &mut self,
        op: Operator,
        obj: TextObjectVerb,
        register: Option<char>,
    ) -> bool {
        if self.additional_cursors.is_empty() {
            return false;
        }
        if !matches!(op, Operator::Delete | Operator::Change | Operator::Yank) {
            return false;
        }
        let mut ranges = Vec::with_capacity(self.additional_cursors.len() + 1);
        for cur in self.all_cursors() {
            // Text objects return `TextRange { start, end, linewise }`.
            // For our purposes only start/end matter — linewise text
            // objects aren't in the supported set (we only handle iw/aw/
            // similar word-like ones at the multi-cursor level).
            let Some(r) = text_object::compute(&self.buffer, cur, obj) else {
                continue;
            };
            ranges.push((r.start, r.end));
        }
        if ranges.is_empty() {
            return false;
        }
        self.apply_multi_op(op, ranges, register, false);
        true
    }

    pub(super) fn try_multi_op_linewise(
        &mut self,
        op: Operator,
        count: usize,
        register: Option<char>,
    ) -> bool {
        if self.additional_cursors.is_empty() {
            return false;
        }
        if !matches!(op, Operator::Delete | Operator::Change | Operator::Yank) {
            return false;
        }
        let last_line = self.buffer.line_count().saturating_sub(1);
        let mut ranges = Vec::with_capacity(self.additional_cursors.len() + 1);
        for cur in self.all_cursors() {
            let l1 = cur.line;
            let l2 = (l1 + count - 1).min(last_line);
            let start = self.buffer.line_start_idx(l1);
            let end = self.buffer.line_start_idx(l2 + 1);
            ranges.push((start, end));
        }
        self.apply_multi_op(op, ranges, register, true);
        true
    }

    /// Mirror Normal-mode `x` across primary + additional cursors —
    /// delete the character under each cursor. Cursors at end-of-line
    /// (no char under them) are skipped.
    pub(super) fn try_multi_delete_char(
        &mut self,
        count: usize,
        register: Option<char>,
    ) -> bool {
        if self.additional_cursors.is_empty() {
            return false;
        }
        let total = self.buffer.total_chars();
        let mut ranges = Vec::with_capacity(self.additional_cursors.len() + 1);
        for cur in self.all_cursors() {
            let line_len = self.buffer.line_len(cur.line);
            if cur.col >= line_len {
                continue;
            }
            let take = count.min(line_len - cur.col);
            if take == 0 {
                continue;
            }
            let start = self.buffer.pos_to_char(cur.line, cur.col);
            let end = (start + take).min(total);
            ranges.push((start, end));
        }
        if ranges.is_empty() {
            return false;
        }
        self.apply_multi_op(Operator::Delete, ranges, register, false);
        true
    }

    /// Primary + each additional cursor as a `Cursor` value, in no
    /// particular order — callers sort what they need.
    fn all_cursors(&self) -> Vec<Cursor> {
        let mut out = Vec::with_capacity(self.additional_cursors.len() + 1);
        out.push(self.window.cursor);
        for &idx in &self.additional_cursors {
            out.push(cursor_at_idx(&self.buffer, idx));
        }
        out
    }

    fn apply_multi_op(
        &mut self,
        op: Operator,
        mut ranges: Vec<(usize, usize)>,
        register: Option<char>,
        linewise: bool,
    ) {
        // Drop empty / inverted ranges (e.g. `dw` at EOL).
        ranges.retain(|r| r.1 > r.0);
        if ranges.is_empty() {
            return;
        }
        ranges.sort_by_key(|r| r.0);
        // Drop overlaps — keep the leftmost-starting one.
        let mut keep: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
        let mut last_end = 0usize;
        for r in &ranges {
            if r.0 >= last_end {
                keep.push(*r);
                last_end = r.1;
            }
        }
        let ranges = keep;

        // Each range's landing in the post-delete buffer is its original
        // start minus the total chars deleted by ranges before it. Same
        // formula whether op is Delete or Change (Yank leaves the buffer
        // intact, but `landings` is unused there so we just don't read it).
        let mut landings: Vec<usize> = Vec::with_capacity(ranges.len());
        let mut cum = 0usize;
        for r in &ranges {
            landings.push(r.0 - cum);
            cum += r.1 - r.0;
        }

        let combined: String = ranges
            .iter()
            .map(|&(s, e)| self.buffer.rope.slice(s..e).to_string())
            .collect();

        match op {
            Operator::Yank => {
                self.write_yank_register(register, combined, linewise);
                let first = ranges.first().map(|r| r.0).unwrap_or(0);
                let last = ranges.last().map(|r| r.1).unwrap_or(first);
                self.flash_yank(first, last);
            }
            Operator::Delete | Operator::Change => {
                self.write_register(register, combined, linewise);
                // Delete in DESCENDING order so each delete is in the
                // original coordinate system.
                for r in ranges.iter().rev() {
                    self.buffer.delete_range(r.0, r.1);
                }
                let primary = landings[0];
                self.cursor_to_idx(primary);
                self.additional_cursors = landings.into_iter().skip(1).collect();
                self.additional_cursors.sort();
                self.additional_cursors.dedup();
                self.clamp_cursor_normal();
                if matches!(op, Operator::Change) {
                    self.mode = Mode::Insert;
                }
            }
            Operator::Indent | Operator::Outdent => {}
        }
    }
}

/// Build a `Cursor` from an absolute char index. Used to drive
/// stateless motion / text-object queries at each additional cursor.
fn cursor_at_idx(buffer: &Buffer, idx: usize) -> Cursor {
    let total = buffer.total_chars();
    let idx = idx.min(total);
    let line = buffer.rope.char_to_line(idx);
    let line_start = buffer.rope.line_to_char(line);
    let col = idx - line_start;
    Cursor { line, col, want_col: col }
}

/// Stateless variant of `App::run_motion` covering the motions the
/// multi-cursor path supports. Returns `None` for motions that depend
/// on app-level state (last-find, search) — the caller then falls back
/// to the single-cursor path rather than fan out incorrectly.
fn run_motion_stateless(
    buffer: &Buffer,
    from: Cursor,
    m: MotionVerb,
    count: usize,
) -> Option<MotionResult> {
    match m {
        MotionVerb::WordForward => Some(motion::word_forward(buffer, from, count)),
        MotionVerb::WordBackward => Some(motion::word_backward(buffer, from, count)),
        MotionVerb::BigWordForward => Some(motion::big_word_forward(buffer, from, count)),
        MotionVerb::BigWordBackward => Some(motion::big_word_backward(buffer, from, count)),
        MotionVerb::EndWord => Some(motion::end_word(buffer, from, count)),
        MotionVerb::BigEndWord => Some(motion::big_end_word(buffer, from, count)),
        MotionVerb::EndWordBackward => Some(motion::end_word_backward(buffer, from, count)),
        MotionVerb::BigEndWordBackward => Some(motion::big_end_word_backward(buffer, from, count)),
        MotionVerb::Left => Some(motion::left(buffer, from, count)),
        MotionVerb::Right => Some(motion::right(buffer, from, count)),
        MotionVerb::LineStart => Some(motion::line_start(buffer, from)),
        MotionVerb::LineEnd => Some(motion::line_end(buffer, from)),
        MotionVerb::FirstNonBlank => Some(motion::first_non_blank(buffer, from)),
        MotionVerb::LastNonBlank => Some(motion::last_non_blank(buffer, from)),
        // Vertical motions intentionally excluded: a multi-cursor `dj`
        // would delete N (often overlapping) line spans, which is rarely
        // what the user wants and falls under the linewise dd/yy/cc path.
        // Mark / find / search motions excluded because they read App
        // state, not just buffer + cursor.
        _ => None,
    }
}

/// Stateless mirror of `App::range_from_motion`. The Vim
/// "exclusive-becomes-inclusive on newline boundary" rule is the same.
fn range_from_motion_at(buffer: &Buffer, from: Cursor, m: MotionResult) -> (usize, usize) {
    let mut to = m.target;
    let mut kind = m.kind;
    if matches!(kind, MotionKind::CharExclusive) && to.col == 0 && to.line > from.line {
        let prev = to.line - 1;
        let len = buffer.line_len(prev);
        let col = if len == 0 { 0 } else { len - 1 };
        to = Cursor { line: prev, col, want_col: col };
        kind = MotionKind::CharInclusive;
    }
    match kind {
        MotionKind::CharExclusive => {
            let f = buffer.pos_to_char(from.line, from.col);
            let t = buffer.pos_to_char(to.line, to.col);
            if f <= t { (f, t) } else { (t, f) }
        }
        MotionKind::CharInclusive => {
            let f = buffer.pos_to_char(from.line, from.col);
            let t = buffer.pos_to_char(to.line, to.col);
            let total = buffer.total_chars();
            if f <= t {
                (f, (t + 1).min(total))
            } else {
                (t, (f + 1).min(total))
            }
        }
        MotionKind::Linewise => {
            let l1 = from.line.min(to.line);
            let l2 = from.line.max(to.line);
            let start = buffer.line_start_idx(l1);
            let end = buffer.line_start_idx(l2 + 1);
            (start, end)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_buffer(text: &str) -> Buffer {
        Buffer {
            rope: ropey::Rope::from_str(text),
            path: None,
            dirty: false,
            version: 0,
            disk_mtime: None,
            display_name: None,
        }
    }

    #[test]
    fn cursor_at_idx_resolves_line_col() {
        let buf = make_buffer("abc\ndef\nghi");
        let c = cursor_at_idx(&buf, 5); // 'e' on line 1
        assert_eq!(c.line, 1);
        assert_eq!(c.col, 1);
    }

    #[test]
    fn range_from_word_forward_at_offset() {
        let buf = make_buffer("foo bar baz");
        let from = cursor_at_idx(&buf, 4); // start of "bar"
        let m = run_motion_stateless(&buf, from, MotionVerb::WordForward, 1).unwrap();
        let (s, e) = range_from_motion_at(&buf, from, m);
        // `w` from "bar" should land at start of "baz"; range is "bar ".
        assert_eq!(s, 4);
        assert_eq!(e, 8);
    }
}
