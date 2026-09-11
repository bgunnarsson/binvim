//! Buffer search (`/`, `?`, `n`, `N`, `*`, `#`), the jumplist, and the
//! per-line range queries the renderer needs (search-match highlights,
//! matched-pair brackets, yank flash, visual selection projection,
//! click-inside-notification hit testing).

use crossterm::event::{KeyCode, KeyEvent};
use std::time::Instant;

use crate::cursor::Cursor;
use crate::keymap::MapMode;
use crate::mode::{Mode, VisualKind};
use crate::motion::{MotionKind, MotionResult};

use super::pair::{
    bracket_pair, find_match_close, find_match_open, html_tag_pair_at, is_bracket,
    is_html_like_buffer,
};

impl super::App {
    pub(super) fn search_word_under_cursor(&mut self, backward: bool, whole_word: bool) {
        let Some(word) = self.word_under_cursor() else {
            self.status_msg = "No word under cursor".into();
            return;
        };
        if !self.set_search(&word_pattern(&word, whole_word), backward) {
            return;
        }
        let cur_idx = self
            .buffer
            .pos_to_char(self.window.cursor.line, self.window.cursor.col);
        let from = if backward { cur_idx } else { cur_idx + 1 };
        let mut hit = self.find_match(from, !backward, true);
        // `#` from inside a word starts from that word, not its own match.
        if backward
            && let Some((start, _)) = hit.filter(|&(s, len)| s <= cur_idx && cur_idx < s + len)
        {
            hit = self.find_match(start, false, true);
        }
        match hit {
            Some((idx, _)) => {
                self.push_jump();
                self.cursor_to_idx(idx);
                self.clamp_cursor_normal();
            }
            None => self.status_msg = format!("Pattern not found: {word}"),
        }
    }

    pub(super) fn push_jump(&mut self) {
        let pos = (self.window.cursor.line, self.window.cursor.col);
        self.buffer.set_mark('\'', pos.0, pos.1);
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
            let pos = (self.window.cursor.line, self.window.cursor.col);
            if self.jumplist.last() != Some(&pos) {
                self.jumplist.push(pos);
            }
        }
        self.jump_idx -= 1;
        let (l, c) = self.jumplist[self.jump_idx];
        self.window.cursor.line = l;
        self.window.cursor.col = c;
        self.window.cursor.want_col = c;
        self.clamp_cursor_normal();
    }

    /// `g;` (`older`) / `g,`: `count` places through the change list. A count
    /// past either end stops at the last entry; only a move from there errors.
    pub(super) fn goto_change(&mut self, older: bool, count: usize) {
        let len = self.buffer.changes.len();
        if len == 0 {
            self.status_msg = "E664: changelist is empty".into();
            return;
        }
        let idx = self.buffer.change_idx.min(len);
        let target = if older {
            if idx == 0 {
                self.status_msg = "E662: At start of changelist".into();
                return;
            }
            idx.saturating_sub(count.max(1))
        } else {
            if idx + 1 >= len {
                self.status_msg = "E663: At end of changelist".into();
                return;
            }
            (idx + count.max(1)).min(len - 1)
        };
        self.buffer.change_idx = target;
        let (line, col) = self.buffer.pos_of(self.buffer.changes[target]);
        self.window.cursor.line = line.min(self.buffer.line_count().saturating_sub(1));
        self.window.cursor.col = col;
        self.window.cursor.want_col = col;
        self.clamp_cursor_normal();
    }

    /// `:changes` — the change list in the list overlay, oldest first, each
    /// row numbered by its distance from where `g;` / `g,` are, as in Vim.
    pub(super) fn cmd_changes(&mut self) {
        let idx = self.buffer.change_idx.min(self.buffer.changes.len());
        let rows = self
            .buffer
            .changes
            .iter()
            .enumerate()
            .map(|(i, &at)| {
                let (line, col) = self.buffer.pos_of(at);
                let here = if i == idx { '>' } else { ' ' };
                let label = format!("{here}{:>5} {:>5} {:>4}", i.abs_diff(idx), line + 1, col);
                (label, self.line_preview(line))
            })
            .collect();
        self.show_listing(super::state::Listing {
            title: "change  line   col  text".into(),
            rows,
            empty: "(no changes yet)".into(),
        });
    }

    /// `:marks` — this buffer's marks in the list overlay, in Vim's order:
    /// `'`, the letters, then the ones Vim keeps itself.
    pub(super) fn cmd_marks(&mut self) {
        let mut names: Vec<char> = self.buffer.marks.keys().copied().collect();
        names.sort_by_key(|&name| match name {
            '\'' => (0, 0),
            'a'..='z' => (1, name as usize),
            'A'..='Z' => (2, name as usize),
            '0'..='9' => (3, name as usize),
            _ => (4, "\"[]^.<>".find(name).unwrap_or(usize::MAX)),
        });
        let rows = names
            .into_iter()
            .map(|name| {
                let (line, col) = self.buffer.pos_of(self.buffer.marks[&name]);
                let label = format!("{name} {:>6} {:>4}", line + 1, col);
                (label, self.line_preview(line))
            })
            .collect();
        self.show_listing(super::state::Listing {
            title: "mark   line   col  text".into(),
            rows,
            empty: "(no marks set)".into(),
        });
    }

    /// `:jumps` — the jump list in the list overlay, oldest first, each row
    /// numbered by its distance from where `Ctrl-O` / `Ctrl-I` are, as in Vim.
    pub(super) fn cmd_jumps(&mut self) {
        let idx = self.jump_idx.min(self.jumplist.len());
        let rows = self
            .jumplist
            .iter()
            .enumerate()
            .map(|(i, &(line, col))| {
                let here = if i == idx { '>' } else { ' ' };
                let label = format!("{here}{:>4} {:>5} {:>4}", i.abs_diff(idx), line + 1, col);
                (label, self.line_preview(line))
            })
            .collect();
        self.show_listing(super::state::Listing {
            title: "jump  line   col  text".into(),
            rows,
            empty: "(no jumps yet)".into(),
        });
    }

    /// A line's text, trimmed, for a list-overlay row. Clamped: the jump list
    /// doesn't move with edits, so it can name a line that's gone.
    fn line_preview(&self, line: usize) -> String {
        let line = line.min(self.buffer.rope.len_lines().saturating_sub(1));
        self.buffer.rope.line(line).to_string().trim().to_string()
    }

    fn show_listing(&mut self, listing: super::state::Listing) {
        self.listing = Some(listing);
        self.show_list_page = true;
        self.list_scroll = 0;
    }

    pub(super) fn jump_forward(&mut self) {
        if self.jump_idx + 1 >= self.jumplist.len() {
            self.status_msg = "Already at newest jump".into();
            return;
        }
        self.jump_idx += 1;
        let (l, c) = self.jumplist[self.jump_idx];
        self.window.cursor.line = l;
        self.window.cursor.col = c;
        self.window.cursor.want_col = c;
        self.clamp_cursor_normal();
    }

    pub(super) fn word_under_cursor(&self) -> Option<String> {
        let line_len = self.buffer.line_len(self.window.cursor.line);
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
        let here = self
            .buffer
            .char_at(self.window.cursor.line, self.window.cursor.col)?;
        let here_class = cls(here);
        if here_class == 0 {
            return None;
        }
        let mut start = self.window.cursor.col;
        while start > 0 {
            let c = self.buffer.char_at(self.window.cursor.line, start - 1)?;
            if cls(c) == here_class {
                start -= 1;
            } else {
                break;
            }
        }
        let mut end = self.window.cursor.col + 1;
        while end < line_len {
            let c = self.buffer.char_at(self.window.cursor.line, end)?;
            if cls(c) == here_class {
                end += 1;
            } else {
                break;
            }
        }
        let line_start = self.buffer.line_start_idx(self.window.cursor.line);
        Some(
            self.buffer
                .rope
                .slice((line_start + start)..(line_start + end))
                .to_string(),
        )
    }

    /// Word at an explicit `(line, col)` — same identifier-class scan
    /// as `word_under_cursor` but restricted to `[A-Za-z0-9_]` runs so
    /// it can be used to derive a grep needle from an LSP-provided
    /// position (code lens click, etc.) without picking up surrounding
    /// punctuation.
    pub(super) fn identifier_at(&self, line: usize, col: usize) -> Option<String> {
        let line_len = self.buffer.line_len(line);
        if line_len == 0 {
            return None;
        }
        let is_ident = |c: char| c.is_alphanumeric() || c == '_';
        let here = self.buffer.char_at(line, col)?;
        if !is_ident(here) {
            return None;
        }
        let mut start = col;
        while start > 0 {
            let c = self.buffer.char_at(line, start - 1)?;
            if is_ident(c) {
                start -= 1;
            } else {
                break;
            }
        }
        let mut end = col + 1;
        while end < line_len {
            let c = self.buffer.char_at(line, end)?;
            if is_ident(c) {
                end += 1;
            } else {
                break;
            }
        }
        let line_start = self.buffer.line_start_idx(line);
        Some(
            self.buffer
                .rope
                .slice((line_start + start)..(line_start + end))
                .to_string(),
        )
    }

    pub(super) fn run_search_next(&self, reverse: bool, count: usize) -> MotionResult {
        let stay = MotionResult {
            target: self.window.cursor,
            kind: MotionKind::CharExclusive,
        };
        let Some((_, was_backward)) = self.last_search.as_ref() else {
            return stay;
        };
        // n continues original direction; N reverses it.
        let forward = if reverse {
            *was_backward
        } else {
            !*was_backward
        };
        let mut at = self
            .buffer
            .pos_to_char(self.window.cursor.line, self.window.cursor.col);
        for _ in 0..count.max(1) {
            let from = if forward { at + 1 } else { at };
            let Some((start, _)) = self.find_match(from, forward, true) else {
                return stay;
            };
            at = start;
        }
        let line = self.buffer.rope.char_to_line(at);
        let col = at - self.buffer.rope.line_to_char(line);
        MotionResult {
            target: Cursor {
                line,
                col,
                want_col: col,
            },
            kind: MotionKind::CharExclusive,
        }
    }

    /// `pattern` becomes the search `n` / `N` repeat and the highlight shows,
    /// or the status line says why it can't.
    pub(super) fn set_search(&mut self, pattern: &str, backward: bool) -> bool {
        match compile_search(pattern) {
            Ok(re) => {
                self.search_pattern = Some(re);
                self.last_search = Some((pattern.to_string(), backward));
                self.search_hl_off = false;
                true
            }
            Err(e) => {
                self.status_msg = e;
                false
            }
        }
    }

    /// The next match of the search pattern: starting at or after
    /// `from_char` going forward, before it going back, round the buffer's
    /// end when `wrap` is set. `(start, length)` in chars.
    pub(super) fn find_match(
        &self,
        from_char: usize,
        forward: bool,
        wrap: bool,
    ) -> Option<(usize, usize)> {
        let re = self.search_pattern.as_ref()?;
        let rope = &self.buffer.rope;
        let text = rope.to_string();
        let from = rope.char_to_byte(from_char.min(rope.len_chars()));
        let all = hits(re, &text);
        let pick = if forward {
            all.iter()
                .find(|h| h.0 >= from)
                .or(all.first().filter(|_| wrap))
        } else {
            all.iter()
                .rev()
                .find(|h| h.0 < from)
                .or(all.last().filter(|_| wrap))
        };
        let &(s, e) = pick?;
        let start = rope.byte_to_char(s);
        Some((start, rope.byte_to_char(e) - start))
    }

    /// The literal, case-insensitive text search a Visual selection's
    /// `Ctrl-N` uses — no pattern syntax, and no effect on `n`.
    pub(super) fn find_literal(
        &self,
        query: &str,
        from_char: usize,
        forward: bool,
        wrap: bool,
    ) -> Option<usize> {
        if query.is_empty() {
            return None;
        }
        let rope = &self.buffer.rope;
        // Case-insensitive comparison: lowercase both haystack and needle.
        // `to_ascii_lowercase` only touches A-Z, so byte lengths are
        // preserved and byte offsets in `text` map 1:1 back to rope chars.
        let text = rope.to_string().to_ascii_lowercase();
        let needle = query.to_ascii_lowercase();
        let total = rope.len_chars();
        let from_byte = rope.char_to_byte(from_char.min(total));
        if forward {
            if let Some(b) = text.get(from_byte..).and_then(|s| s.find(needle.as_str())) {
                return Some(rope.byte_to_char(from_byte + b));
            }
            if wrap {
                if let Some(b) = text.get(..from_byte).and_then(|s| s.find(needle.as_str())) {
                    return Some(rope.byte_to_char(b));
                }
            }
        } else {
            if let Some(b) = text.get(..from_byte).and_then(|s| s.rfind(needle.as_str())) {
                return Some(rope.byte_to_char(b));
            }
            if wrap {
                if let Some(b) = text.get(from_byte..).and_then(|s| s.rfind(needle.as_str())) {
                    return Some(rope.byte_to_char(from_byte + b));
                }
            }
        }
        None
    }

    pub(super) fn handle_search_key(&mut self, key: KeyEvent) {
        use super::cmdline_history::HistoryKind;
        if self.keymap_take(key, MapMode::Command) {
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.cmdline.clear();
                self.cmdline_cursor = 0;
                self.history_reset();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let query = std::mem::take(&mut self.cmdline);
                self.cmdline_cursor = 0;
                self.history_record(HistoryKind::Search, &query);
                self.history_reset();
                let backward = match self.mode {
                    Mode::Search { backward } => backward,
                    _ => return,
                };
                self.mode = Mode::Normal;
                self.execute_search(&query, backward);
            }
            KeyCode::Backspace => {
                if self.cmdline.is_empty() {
                    self.history_reset();
                    self.mode = Mode::Normal;
                } else {
                    self.cmdline_backspace_at_cursor();
                }
            }
            KeyCode::Delete => self.cmdline_delete_forward_at_cursor(),
            KeyCode::Left => self.cmdline_cursor_move_back(),
            KeyCode::Right => self.cmdline_cursor_move_forward(),
            KeyCode::Home => self.cmdline_cursor = 0,
            KeyCode::End => self.cmdline_cursor = self.cmdline.len(),
            KeyCode::Up => self.history_walk_back(HistoryKind::Search),
            KeyCode::Down => self.history_walk_forward(HistoryKind::Search),
            KeyCode::Char(c) => {
                self.cmdline_insert_char_at_cursor(c);
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
        if !self.set_search(&q, backward) {
            return;
        }
        let cur_idx = self
            .buffer
            .pos_to_char(self.window.cursor.line, self.window.cursor.col);
        // From the char after the cursor, as Vim does, so a match the cursor
        // is already on isn't found again.
        let from = if backward { cur_idx } else { cur_idx + 1 };
        match self.find_match(from, !backward, true) {
            Some((idx, _)) => {
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
        let line = self.window.cursor.line;
        let col = self.window.cursor.col;
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

    /// Char-column ranges of search-highlight matches on `line` of an
    /// arbitrary buffer — used by the renderer when drawing inactive
    /// panes so each pane's own buffer is searched against the global
    /// `last_search` term.
    pub fn line_search_matches_in(
        &self,
        buffer: &crate::buffer::Buffer,
        line: usize,
    ) -> Vec<(usize, usize)> {
        if self.search_hl_off {
            return Vec::new();
        }
        let Some(re) = self.search_pattern.as_ref() else {
            return Vec::new();
        };
        let line_len = buffer.line_len(line);
        if line_len == 0 {
            return Vec::new();
        }
        let line_start = buffer.line_start_idx(line);
        let text = buffer
            .rope
            .slice(line_start..(line_start + line_len))
            .to_string();
        hits(re, &text)
            .into_iter()
            .filter(|(s, e)| e > s)
            .map(|(s, e)| (text[..s].chars().count(), text[..e].chars().count()))
            .collect()
    }

    /// For visual mode rendering: return the half-open `[start_col, end_col)` of selected
    /// chars on this line, or `None` if none. For V-line, returns full line range; for
    /// V-block, returns the column-rectangle slice for this row.
    pub fn line_selection(&self, line: usize) -> Option<(usize, usize)> {
        let kind = match self.mode {
            Mode::Visual(k) => k,
            _ => return None,
        };
        let anchor = self.window.visual_anchor?;
        let cursor = self.window.cursor;
        let line_len = self.buffer.line_len(line);
        match kind {
            VisualKind::Line => {
                let l1 = anchor.line.min(cursor.line);
                let l2 = anchor.line.max(cursor.line);
                if line < l1 || line > l2 {
                    return None;
                }
                let end = if line_len == 0 { 1 } else { line_len };
                Some((0, end))
            }
            VisualKind::Char => {
                let (lo, hi) = if (anchor.line, anchor.col) <= (cursor.line, cursor.col) {
                    (anchor, cursor)
                } else {
                    (cursor, anchor)
                };
                if line < lo.line || line > hi.line {
                    return None;
                }
                let start_col = if line == lo.line { lo.col } else { 0 };
                let end_col = if line == hi.line {
                    (hi.col + 1).min(line_len.max(1))
                } else {
                    line_len.max(1)
                };
                Some((start_col, end_col))
            }
            VisualKind::Block => {
                let l1 = anchor.line.min(cursor.line);
                let l2 = anchor.line.max(cursor.line);
                if line < l1 || line > l2 {
                    return None;
                }
                let c1 = anchor.col.min(cursor.col);
                let c2 = anchor.col.max(cursor.col);
                // Skip rows that don't reach the block's left edge.
                if line_len <= c1 {
                    return None;
                }
                let end = (c2 + 1).min(line_len);
                Some((c1, end))
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
            let segs = len.div_ceil(max_inner);
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

/// `*`'s pattern for `word`: the word as literal text, inside `\<` / `\>`
/// when `whole` and it starts / ends on a keyword char, and without regard
/// to case — Vim's `*` ignores 'smartcase'.
fn word_pattern(word: &str, whole: bool) -> String {
    let keyword = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::new();
    if whole && word.chars().next().is_some_and(keyword) {
        out.push_str("\\<");
    }
    for c in word.chars() {
        if matches!(c, '\\' | '.' | '*' | '$' | '^' | '~' | '[') {
            out.push('\\');
        }
        out.push(c);
    }
    if whole && word.chars().last().is_some_and(keyword) {
        out.push_str("\\>");
    }
    out.push_str("\\c");
    out
}

/// A search pattern in Vim's syntax, compiled: `^` and `$` match at every
/// line, and case follows `translate`.
pub(super) fn compile_search(pattern: &str) -> Result<regex::Regex, String> {
    let (source, ignore_case) = translate(pattern).map_err(|e| format!("Invalid pattern: {e}"))?;
    regex::RegexBuilder::new(&source)
        .case_insensitive(ignore_case)
        .multi_line(true)
        .build()
        .map_err(|_| format!("Invalid pattern: {pattern}"))
}

/// Where `re` matches in `text`, as byte ranges — narrowed to the `zs`
/// group when the pattern has `\zs` / `\ze`.
pub(super) fn hits(re: &regex::Regex, text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    while from <= text.len() {
        let Some(caps) = re.captures_at(text, from) else {
            break;
        };
        let Some(whole) = caps.get(0) else {
            break;
        };
        let part = caps.name("zs").unwrap_or(whole);
        out.push((part.start(), part.end()));
        // Past an empty match by one char, or the loop would stay on it.
        let step = text[whole.end()..].chars().next().map_or(1, char::len_utf8);
        from = if whole.end() > whole.start() {
            whole.end()
        } else {
            whole.end() + step
        };
    }
    out
}

/// How much of Vim's pattern syntax is special without a backslash: `\v`
/// (very magic), the default `\m`, `\M` (nomagic) and `\V` (very nomagic).
#[derive(Clone, Copy)]
enum Magic {
    Very,
    On,
    Off,
    VeryOff,
}

/// The characters that can be special at all; which of them are special
/// bare, and which only after a backslash, depends on the `Magic`.
const SPECIALS: &str = ".*$^~[()|+?={<>%@";

fn special_bare(magic: Magic, c: char) -> bool {
    match magic {
        Magic::Very => true,
        Magic::On => matches!(c, '.' | '*' | '$' | '^' | '~' | '['),
        Magic::Off => matches!(c, '$' | '^'),
        Magic::VeryOff => false,
    }
}

/// The pattern char at `i`: itself, or the one a backslash escapes — with
/// whether it was escaped and how many chars it took.
fn token(chars: &[char], i: usize) -> (char, bool, usize) {
    match (chars[i], chars.get(i + 1)) {
        ('\\', Some(&n)) => (n, true, 2),
        (c, _) => (c, false, 1),
    }
}

/// A backslashed letter's meaning, when it's a class or an escape.
fn escape_class(c: char) -> Option<&'static str> {
    let class = match c {
        's' => "[ \\t]",
        'S' => "[^ \\t]",
        'd' => "[0-9]",
        'D' => "[^0-9]",
        'w' => "[0-9A-Za-z_]",
        'W' => "[^0-9A-Za-z_]",
        'a' => "[A-Za-z]",
        'A' => "[^A-Za-z]",
        // Case-exact even when the pattern ignores case.
        'l' => "(?-i:[a-z])",
        'L' => "(?-i:[^a-z])",
        'u' => "(?-i:[A-Z])",
        'U' => "(?-i:[^A-Z])",
        'x' => "[0-9A-Fa-f]",
        'X' => "[^0-9A-Fa-f]",
        'h' => "[A-Za-z_]",
        'H' => "[^A-Za-z_]",
        'o' => "[0-7]",
        'O' => "[^0-7]",
        'n' => "\\n",
        't' => "\\t",
        'r' => "\\r",
        'e' => "\\x1B",
        _ => return None,
    };
    Some(class)
}

/// Vim's pattern syntax as a Rust regex, and whether it ignores case.
///
/// In the default `magic` mode `.`, `*`, `[…]`, `^` and `$` are special
/// bare, while `\(…\)`, `\%(…\)`, `\|`, `\+`, `\?` / `\=`, `\{n,m}` /
/// `\{-n,m}` and `\<` / `\>` need their backslash — a bare `+ ? = ( ) | {`
/// is literal. `\v` makes them all special bare, `\M` only `^` and `$`,
/// `\V` none. Classes: `\s \d \w \a \l \u \x \h \o` and capitals, `\_s`,
/// `\_.`; escapes `\n \t \r \e`. `\zs` / `\ze` mark the part of a hit that
/// counts as the match. Case is smart: an upper-case letter anywhere but
/// after a backslash makes the pattern case-sensitive, and `\c` / `\C`
/// decide outright. Backreferences, `\@` lookaround and the other `\%`
/// items are refused rather than half-done.
fn translate(pattern: &str) -> Result<(String, bool), String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut magic = Magic::On;
    let mut case: Option<bool> = None;
    let mut has_upper = false;
    // At the start of a branch `^` anchors and `*` is a plain star.
    let mut branch_start = true;
    let mut zs_open = false;
    let mut zs_used = false;
    let mut i = 0;
    while i < chars.len() {
        let (c, escaped, width) = token(&chars, i);
        i += width;
        if escaped && (c.is_ascii_alphanumeric() || c == '_') {
            match c {
                'c' => case = Some(true),
                'C' => case = Some(false),
                'v' => magic = Magic::Very,
                'm' => magic = Magic::On,
                'M' => magic = Magic::Off,
                'V' => magic = Magic::VeryOff,
                'z' => {
                    match chars.get(i) {
                        Some('s') if !zs_used => {
                            out.push_str("(?P<zs>");
                            zs_open = true;
                            zs_used = true;
                        }
                        Some('e') if zs_open => {
                            out.push(')');
                            zs_open = false;
                        }
                        Some('e') if !zs_used => {
                            out = format!("(?P<zs>{out})");
                            zs_used = true;
                        }
                        _ => return Err("one \\zs and one \\ze at most".into()),
                    }
                    i += 1;
                }
                '_' => {
                    let piece = match chars.get(i) {
                        Some('s') => "[ \\t\\n]",
                        Some('.') => "(?s:.)",
                        _ => return Err("only \\_s and \\_. of the \\_ items".into()),
                    };
                    out.push_str(piece);
                    branch_start = false;
                    i += 1;
                }
                '0'..='9' => return Err("backreferences aren't supported".into()),
                _ => match escape_class(c) {
                    Some(class) => {
                        out.push_str(class);
                        branch_start = false;
                    }
                    None => return Err(format!("\\{c} isn't supported")),
                },
            }
            continue;
        }
        let special = SPECIALS.contains(c) && escaped != special_bare(magic, c);
        if !special {
            if !escaped && c.is_uppercase() {
                has_upper = true;
            }
            out.push_str(&regex::escape(&c.to_string()));
            branch_start = false;
            continue;
        }
        match c {
            '^' if branch_start => out.push('^'),
            '$' if ends_branch(&chars, i, magic) => out.push('$'),
            '*' if branch_start => {
                out.push_str("\\*");
                branch_start = false;
            }
            '^' | '$' | '~' => {
                out.push_str(&regex::escape(&c.to_string()));
                branch_start = false;
            }
            '.' | '*' | '+' | ')' => {
                out.push(c);
                branch_start = false;
            }
            '?' | '=' => out.push('?'),
            '(' | '|' => {
                out.push(c);
                branch_start = true;
            }
            '%' if chars.get(i) == Some(&'(') => {
                out.push_str("(?:");
                branch_start = true;
                i += 1;
            }
            '[' => {
                match bracket(&chars, i, &mut has_upper) {
                    Some((class, end)) => {
                        out.push_str(&class);
                        i = end;
                    }
                    None => out.push_str("\\["),
                }
                branch_start = false;
            }
            '{' => {
                let (quantifier, end) = brace(&chars, i)?;
                out.push_str(&quantifier);
                i = end;
            }
            '<' => {
                out.push_str("\\b{start}");
                branch_start = false;
            }
            '>' => {
                out.push_str("\\b{end}");
                branch_start = false;
            }
            _ => return Err(format!("{c} items (\\{c}) aren't supported")),
        }
    }
    if zs_open {
        out.push(')');
    }
    Ok((out, case.unwrap_or(!has_upper)))
}

/// Whether the pattern ends at `i`, or a branch does — so a `$` just before
/// is an anchor and not a dollar sign.
fn ends_branch(chars: &[char], i: usize, magic: Magic) -> bool {
    if i >= chars.len() {
        return true;
    }
    let (c, escaped, _) = token(chars, i);
    matches!(c, '|' | ')') && escaped != special_bare(magic, c)
}

/// A `[…]` class, from `i` just after the `[`: the Rust class and where the
/// pattern goes on, or `None` when there's no closing `]` and the `[` is a
/// plain bracket.
fn bracket(chars: &[char], mut i: usize, has_upper: &mut bool) -> Option<(String, usize)> {
    let mut out = String::from("[");
    if chars.get(i) == Some(&'^') {
        out.push('^');
        i += 1;
    }
    if chars.get(i) == Some(&']') {
        out.push_str("\\]");
        i += 1;
    }
    while let Some(&c) = chars.get(i) {
        match c {
            ']' => {
                out.push(']');
                return Some((out, i + 1));
            }
            '\\' => {
                let n = *chars.get(i + 1)?;
                let piece = match n {
                    'n' => "\\n".to_string(),
                    't' => "\\t".to_string(),
                    'r' => "\\r".to_string(),
                    'e' => "\\x1B".to_string(),
                    _ => regex::escape(&n.to_string()),
                };
                out.push_str(&piece);
                i += 2;
            }
            // `[:alpha:]` and its kind pass through as they are.
            '[' if chars.get(i + 1) == Some(&':') => {
                let end = (i + 2..chars.len().saturating_sub(1))
                    .find(|&j| chars[j] == ':' && chars[j + 1] == ']')?;
                out.extend(chars[i..end + 2].iter());
                i = end + 2;
            }
            // Nested classes, `&&`, `~~` and `--` mean something in a Rust
            // class and nothing in Vim's.
            '[' | '&' | '~' => {
                out.push('\\');
                out.push(c);
                i += 1;
            }
            '-' if out.ends_with('-') => {
                out.push_str("\\-");
                i += 1;
            }
            _ => {
                if c.is_uppercase() {
                    *has_upper = true;
                }
                out.push(c);
                i += 1;
            }
        }
    }
    None
}

/// A `\{…}` count, from `i` just after the `{`: the Rust quantifier and
/// where the pattern goes on. `\{-…}` is lazy, and the closing `}` may carry
/// a backslash of its own.
fn brace(chars: &[char], i: usize) -> Result<(String, usize), String> {
    let close = (i..chars.len())
        .find(|&j| chars[j] == '}')
        .ok_or("\\{ without its }")?;
    let mut body: String = chars[i..close].iter().collect();
    if body.ends_with('\\') {
        body.pop();
    }
    let lazy = body.starts_with('-');
    let body = body.trim_start_matches('-');
    let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    let quantifier = match body.split_once(',') {
        None if body.is_empty() => "*".to_string(),
        None if digits(body) => format!("{{{body}}}"),
        Some((lo, hi)) if digits(lo) && digits(hi) => {
            let lo = if lo.is_empty() { "0" } else { lo };
            format!("{{{lo},{hi}}}")
        }
        _ => return Err(format!("\\{{{body}}} isn't a count")),
    };
    let quantifier = if lazy { quantifier + "?" } else { quantifier };
    Ok((quantifier, close + 1))
}

#[cfg(test)]
mod tests {
    use super::{compile_search, hits, word_pattern};

    /// (pattern, text, where the first match starts and what it covers).
    const TABLE: &[(&str, &str, Option<(usize, &str)>)] = &[
        ("foo", "a foo", Some((2, "foo"))),
        ("foo", "a FOO", Some((2, "FOO"))),
        ("Foo", "a foo Foo", Some((6, "Foo"))),
        ("\\cFoo", "a foo", Some((2, "foo"))),
        ("\\Cfoo", "a FOO foo", Some((6, "foo"))),
        ("a\\+", "caaat", Some((1, "aaa"))),
        ("a+", "aa a+", Some((3, "a+"))),
        ("\\v(ab)+", "x abab", Some((2, "abab"))),
        ("\\(ab\\)\\+", "x abab", Some((2, "abab"))),
        ("\\%(ab\\)\\+", "x abab", Some((2, "abab"))),
        ("a\\{2}", "a aaa", Some((2, "aa"))),
        ("a\\{-1,}", "aaa", Some((0, "a"))),
        ("a\\{,2}b", "aaab", Some((1, "aab"))),
        ("\\<is\\>", "this is", Some((5, "is"))),
        ("foo\\|bar", "x bar", Some((2, "bar"))),
        ("x\\zsy", "xy", Some((1, "y"))),
        ("x\\zey", "xz xy", Some((3, "x"))),
        ("\\Va.c", "abc a.c", Some((4, "a.c"))),
        ("\\Mx.y", "xzy x.y", Some((4, "x.y"))),
        ("a.c", "abc", Some((0, "abc"))),
        ("a\\.c", "abc a.c", Some((4, "a.c"))),
        ("[0-9]\\+", "ab12c", Some((2, "12"))),
        ("[]x]", "a]", Some((1, "]"))),
        ("\\d\\d", "a12", Some((1, "12"))),
        ("\\s", "a b", Some((1, " "))),
        ("\\u\\l", "aB Cd", Some((3, "Cd"))),
        ("^b", "ab\nbc", Some((3, "b"))),
        ("c$", "cab\nbc", Some((5, "c"))),
        ("a^b", "a^b", Some((0, "a^b"))),
        ("*a", "x*a", Some((1, "*a"))),
        ("q", "abc", None),
    ];

    #[test]
    fn the_translator_matches_the_way_vim_does() {
        for &(pattern, text, want) in TABLE {
            let re = compile_search(pattern).unwrap_or_else(|e| panic!("{pattern}: {e}"));
            let got = hits(&re, text).first().map(|&(s, e)| (s, &text[s..e]));
            assert_eq!(got, want, "{pattern} on {text:?}");
        }
    }

    #[test]
    fn what_the_translator_cannot_do_is_an_error() {
        for pattern in ["\\(a\\)\\1", "a\\@=", "\\%d123", "\\(a", "a\\{x}"] {
            assert!(compile_search(pattern).is_err(), "{pattern}");
        }
    }

    #[test]
    fn star_patterns_take_the_word_literally() {
        assert_eq!(word_pattern("foo", true), "\\<foo\\>\\c");
        assert_eq!(word_pattern("foo", false), "foo\\c");
        assert_eq!(word_pattern("a.b", true), "\\<a\\.b\\>\\c");
    }
}
