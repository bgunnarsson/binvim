//! Buffer search (`/`, `?`, `n`, `N`, `*`, `#`), the jumplist, and the
//! per-line range queries the renderer needs (search-match highlights,
//! matched-pair brackets, yank flash, visual selection projection,
//! click-inside-notification hit testing).

use crossterm::event::{KeyCode, KeyEvent};
use std::time::Instant;

use crate::cursor::Cursor;
use crate::mode::{Mode, VisualKind};
use crate::motion::{MotionKind, MotionResult};

use super::pair::{
    bracket_pair, find_match_close, find_match_open, html_tag_pair_at, is_bracket,
    is_html_like_buffer,
};

impl super::App {
    pub(super) fn search_word_under_cursor(&mut self, backward: bool) {
        let Some(word) = self.word_under_cursor() else {
            self.status_msg = "No word under cursor".into();
            return;
        };
        self.last_search = Some((word.clone(), backward));
        self.search_hl_off = false;
        let cur_idx = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
        let total = self.buffer.total_chars();
        let from = if backward {
            cur_idx.saturating_sub(1)
        } else {
            (cur_idx + 1).min(total)
        };
        match self.search(&word, from, !backward, true) {
            Some(idx) => {
                self.push_jump();
                self.cursor_to_idx(idx);
                self.clamp_cursor_normal();
            }
            None => self.status_msg = format!("Pattern not found: {word}"),
        }
    }

    pub(super) fn push_jump(&mut self) {
        let pos = (self.cursor.line, self.cursor.col);
        // If we've stepped back via Ctrl-O, drop the forward history before pushing.
        self.jumplist.truncate(self.jump_idx);
        // Avoid duplicate consecutive entries.
        if self.jumplist.last() != Some(&pos) {
            self.jumplist.push(pos);
        }
        self.jump_idx = self.jumplist.len();
    }

    pub(super) fn jump_back(&mut self) {
        if self.jump_idx == 0 {
            self.status_msg = "Already at oldest jump".into();
            return;
        }
        // If we're at the head, save current position so Ctrl-I can return to it.
        if self.jump_idx == self.jumplist.len() {
            let pos = (self.cursor.line, self.cursor.col);
            if self.jumplist.last() != Some(&pos) {
                self.jumplist.push(pos);
            }
        }
        self.jump_idx -= 1;
        let (l, c) = self.jumplist[self.jump_idx];
        self.cursor.line = l;
        self.cursor.col = c;
        self.cursor.want_col = c;
        self.clamp_cursor_normal();
    }

    pub(super) fn jump_forward(&mut self) {
        if self.jump_idx + 1 >= self.jumplist.len() {
            self.status_msg = "Already at newest jump".into();
            return;
        }
        self.jump_idx += 1;
        let (l, c) = self.jumplist[self.jump_idx];
        self.cursor.line = l;
        self.cursor.col = c;
        self.cursor.want_col = c;
        self.clamp_cursor_normal();
    }

    pub(super) fn word_under_cursor(&self) -> Option<String> {
        let line_len = self.buffer.line_len(self.cursor.line);
        if line_len == 0 {
            return None;
        }
        let cls = |c: char| -> u8 {
            if c.is_whitespace() {
                0
            } else if c.is_alphanumeric() || c == '_' {
                1
            } else {
                2
            }
        };
        let here = self.buffer.char_at(self.cursor.line, self.cursor.col)?;
        let here_class = cls(here);
        if here_class == 0 {
            return None;
        }
        let mut start = self.cursor.col;
        while start > 0 {
            let c = self.buffer.char_at(self.cursor.line, start - 1)?;
            if cls(c) == here_class {
                start -= 1;
            } else {
                break;
            }
        }
        let mut end = self.cursor.col + 1;
        while end < line_len {
            let c = self.buffer.char_at(self.cursor.line, end)?;
            if cls(c) == here_class {
                end += 1;
            } else {
                break;
            }
        }
        let line_start = self.buffer.line_start_idx(self.cursor.line);
        Some(
            self.buffer
                .rope
                .slice((line_start + start)..(line_start + end))
                .to_string(),
        )
    }

    pub(super) fn run_search_next(&self, reverse: bool, _count: usize) -> MotionResult {
        let Some((query, was_backward)) = self.last_search.clone() else {
            return MotionResult { target: self.cursor, kind: MotionKind::CharExclusive };
        };
        // n continues original direction; N reverses it.
        let forward = if reverse { was_backward } else { !was_backward };
        let total = self.buffer.total_chars();
        let cur_idx = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
        let from = if forward { (cur_idx + 1).min(total) } else { cur_idx.saturating_sub(1) };
        match self.search(&query, from, forward, true) {
            Some(idx) => {
                let line = self.buffer.rope.char_to_line(idx);
                let col = idx - self.buffer.rope.line_to_char(line);
                MotionResult {
                    target: Cursor { line, col, want_col: col },
                    kind: MotionKind::CharExclusive,
                }
            }
            None => MotionResult { target: self.cursor, kind: MotionKind::CharExclusive },
        }
    }

    pub(super) fn search(&self, query: &str, from_char: usize, forward: bool, wrap: bool) -> Option<usize> {
        if query.is_empty() {
            return None;
        }
        let rope = &self.buffer.rope;
        let text = rope.to_string();
        let total = rope.len_chars();
        let from_byte = rope.char_to_byte(from_char.min(total));
        if forward {
            if let Some(b) = text.get(from_byte..).and_then(|s| s.find(query)) {
                return Some(rope.byte_to_char(from_byte + b));
            }
            if wrap {
                if let Some(b) = text.get(..from_byte).and_then(|s| s.find(query)) {
                    return Some(rope.byte_to_char(b));
                }
            }
        } else {
            if let Some(b) = text.get(..from_byte).and_then(|s| s.rfind(query)) {
                return Some(rope.byte_to_char(b));
            }
            if wrap {
                if let Some(b) = text.get(from_byte..).and_then(|s| s.rfind(query)) {
                    return Some(rope.byte_to_char(from_byte + b));
                }
            }
        }
        None
    }

    pub(super) fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.cmdline.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let query = std::mem::take(&mut self.cmdline);
                let backward = match self.mode {
                    Mode::Search { backward } => backward,
                    _ => return,
                };
                self.mode = Mode::Normal;
                self.execute_search(&query, backward);
            }
            KeyCode::Backspace => {
                if self.cmdline.is_empty() {
                    self.mode = Mode::Normal;
                } else {
                    self.cmdline.pop();
                }
            }
            KeyCode::Char(c) => {
                self.cmdline.push(c);
            }
            _ => {}
        }
    }

    fn execute_search(&mut self, query: &str, backward: bool) {
        let q = if query.is_empty() {
            match self.last_search.as_ref() {
                Some((q, _)) => q.clone(),
                None => return,
            }
        } else {
            query.to_string()
        };
        self.last_search = Some((q.clone(), backward));
        self.search_hl_off = false;
        let cur_idx = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
        let forward = !backward;
        match self.search(&q, cur_idx, forward, true) {
            Some(idx) => {
                self.push_jump();
                self.cursor_to_idx(idx);
                self.clamp_cursor_normal();
            }
            None => {
                self.status_msg = format!("Pattern not found: {q}");
            }
        }
    }

    /// Char-index ranges of the current matched bracket pair / HTML tag pair
    /// based on cursor position. Empty when the cursor isn't on a recognised
    /// pair or no match exists. Each returned range is `(start, end)` in
    /// global char indices, half-open. For brackets each range is one char;
    /// for HTML tags it spans the entire `<…>` of the open and close tag.
    pub fn matched_pair_ranges(&self) -> Vec<(usize, usize)> {
        let line = self.cursor.line;
        let col = self.cursor.col;
        // HTML tag matching takes precedence so cursor on `<` of `<div>`
        // shows the whole-tag highlight rather than a single `<` char match.
        if is_html_like_buffer(&self.buffer) {
            if let Some(pair) = html_tag_pair_at(&self.buffer, line, col) {
                return vec![pair.0, pair.1];
            }
        }
        // Brackets — check char under cursor (Normal mode), then char before
        // (Insert mode just past an opener).
        let here = self.buffer.char_at(line, col);
        let prev = if col > 0 {
            self.buffer.char_at(line, col - 1)
        } else {
            None
        };
        let (bracket_idx, bracket_char) = match (here, prev) {
            (Some(c), _) if is_bracket(c) => (self.buffer.pos_to_char(line, col), c),
            (_, Some(c)) if is_bracket(c) => {
                (self.buffer.pos_to_char(line, col).saturating_sub(1), c)
            }
            _ => return Vec::new(),
        };
        let (open, close, forward) = bracket_pair(bracket_char);
        let other = if forward {
            find_match_close(&self.buffer, bracket_idx, open, close)
        } else {
            find_match_open(&self.buffer, bracket_idx, open, close)
        };
        let Some(other) = other else { return Vec::new() };
        vec![(bracket_idx, bracket_idx + 1), (other, other + 1)]
    }

    /// Char-column ranges on `line` covered by the matched-pair highlight.
    /// Multiple ranges are possible when both halves of an HTML tag pair
    /// land on the same row.
    pub fn line_match_pair(&self, line: usize) -> Vec<(usize, usize)> {
        let ranges = self.matched_pair_ranges();
        if ranges.is_empty() {
            return Vec::new();
        }
        let line_start = self.buffer.line_start_idx(line);
        let line_len = self.buffer.line_len(line);
        let line_end = line_start + line_len;
        let mut out = Vec::new();
        for (s, e) in ranges {
            if e <= line_start || s >= line_end {
                continue;
            }
            let cs = s.saturating_sub(line_start);
            let ce_global = e.min(line_end);
            let ce = ce_global.saturating_sub(line_start);
            if ce > cs {
                out.push((cs, ce));
            }
        }
        out
    }

    /// Per-line view of the active yank flash, returned as a char-column
    /// range on `line`. Returns `None` when the line is outside the range
    /// or the flash has expired.
    pub fn line_yank_highlight(&self, line: usize) -> Option<(usize, usize)> {
        let h = self.yank_highlight.as_ref()?;
        if Instant::now() >= h.expires_at {
            return None;
        }
        let line_start = self.buffer.line_start_idx(line);
        let line_len = self.buffer.line_len(line);
        let line_content_end = line_start + line_len;
        let s = h.start.saturating_sub(line_start);
        let e_global = h.end.min(line_content_end);
        let e = e_global.saturating_sub(line_start);
        if e <= s {
            return None;
        }
        Some((s, e.min(line_len)))
    }

    /// Char-column ranges of search-highlight matches on `line`.
    pub fn line_search_matches(&self, line: usize) -> Vec<(usize, usize)> {
        if self.search_hl_off {
            return Vec::new();
        }
        let Some((q, _)) = &self.last_search else {
            return Vec::new();
        };
        if q.is_empty() {
            return Vec::new();
        }
        let line_len = self.buffer.line_len(line);
        if line_len == 0 {
            return Vec::new();
        }
        let line_start = self.buffer.line_start_idx(line);
        let text: String = self
            .buffer
            .rope
            .slice(line_start..(line_start + line_len))
            .to_string();
        let qlen = q.chars().count();
        let mut out = Vec::new();
        let mut byte = 0usize;
        while byte <= text.len() {
            let Some(rel) = text[byte..].find(q.as_str()) else {
                break;
            };
            let abs_byte = byte + rel;
            let char_start = text[..abs_byte].chars().count();
            out.push((char_start, char_start + qlen));
            byte = abs_byte + q.len().max(1);
        }
        out
    }

    /// For visual mode rendering: return the half-open `[start_col, end_col)` of selected
    /// chars on this line, or `None` if none. For V-line, returns full line range.
    pub fn line_selection(&self, line: usize) -> Option<(usize, usize)> {
        let kind = match self.mode {
            Mode::Visual(k) => k,
            _ => return None,
        };
        let anchor = self.visual_anchor?;
        let cursor = self.cursor;
        let (lo, hi) = if (anchor.line, anchor.col) <= (cursor.line, cursor.col) {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };
        if line < lo.line || line > hi.line {
            return None;
        }
        let line_len = self.buffer.line_len(line);
        match kind {
            VisualKind::Line => {
                let end = if line_len == 0 { 1 } else { line_len };
                Some((0, end))
            }
            VisualKind::Char => {
                let start_col = if line == lo.line { lo.col } else { 0 };
                let end_col = if line == hi.line {
                    (hi.col + 1).min(line_len.max(1))
                } else {
                    line_len.max(1)
                };
                Some((start_col, end_col))
            }
        }
    }

    /// Bounds-check a mouse position against the rendered top-right notification box.
    /// Mirrors the layout in `render::draw_notification` (height = 3 rows).
    pub(super) fn click_inside_notification(&self, row: usize, col: usize) -> bool {
        if self.status_msg.is_empty() {
            return false;
        }
        if matches!(self.mode, Mode::Command | Mode::Search { .. }) {
            return false;
        }
        // Mirror `draw_notification`'s wrap so the click hit-test matches
        // what's actually painted (max half the terminal width, multiple
        // rows when the message wraps).
        const MAX_ROWS: usize = 6;
        let total_w = self.width as usize;
        let half_inner = (total_w / 2).saturating_sub(4);
        let term_inner = total_w.saturating_sub(8);
        let max_inner = half_inner.min(term_inner).max(20);
        let mut rows = 0usize;
        let mut widest = 0usize;
        for raw in self.status_msg.lines() {
            if raw.is_empty() {
                rows += 1;
                continue;
            }
            let len = raw.chars().count();
            let segs = (len + max_inner - 1) / max_inner;
            rows += segs;
            widest = widest.max(len.min(max_inner));
        }
        if rows == 0 {
            return false;
        }
        let visible_rows = rows.min(MAX_ROWS);
        let inner_w = widest + 2;
        let box_w = inner_w + 2;
        let left = total_w.saturating_sub(box_w + 1);
        row < visible_rows + 2 && col >= left && col < left + box_w
    }
}
