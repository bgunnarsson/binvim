//! Primitive edits invoked from `apply_action` — insert mode entry,
//! replace/delete/put, surround manipulation, indent/outdent, undo/redo,
//! number adjustments, and case toggling. Plus the line-number parser
//! helpers that `Ctrl-A`/`Ctrl-X` rely on.

use std::time::Instant;

use crate::editorconfig::IndentStyle;
use crate::mode::Mode;
use crate::parser::InsertWhere;

use super::pair::{is_paired_bracket, surround_open_close};
use super::state::{YankHighlight, YANK_FLASH_DURATION};

impl super::App {
    /// Set up a yank flash over the given char-index range. The renderer
    /// paints the range in a Peach background until the deadline passes.
    pub(super) fn flash_yank(&mut self, start: usize, end: usize) {
        if end <= start {
            return;
        }
        self.yank_highlight = Some(YankHighlight {
            start,
            end,
            expires_at: Instant::now() + YANK_FLASH_DURATION,
        });
    }

    /// Insert one indent unit (per .editorconfig) at the start of every line
    /// in `[l1, l2]`. Skips empty lines.
    pub(super) fn indent_lines(&mut self, l1: usize, l2: usize) {
        let last = self.buffer.line_count().saturating_sub(1);
        let l2 = l2.min(last);
        let unit = self.editorconfig.indent_string();
        for line in l1..=l2 {
            let line_len = self.buffer.line_len(line);
            if line_len == 0 {
                continue;
            }
            let line_start = self.buffer.line_start_idx(line);
            self.buffer.insert_at_idx(line_start, &unit);
        }
        self.cursor.line = l1;
        let col = self.first_non_blank_col(l1);
        self.cursor.col = col;
        self.cursor.want_col = col;
    }

    /// Remove up to one indent unit's worth of leading whitespace from every
    /// line in `[l1, l2]`. For tab indent style we strip one tab if present;
    /// for spaces we strip up to `indent_size` whitespace chars.
    pub(super) fn outdent_lines(&mut self, l1: usize, l2: usize) {
        let last = self.buffer.line_count().saturating_sub(1);
        let l2 = l2.min(last);
        let style = self.editorconfig.indent_style;
        let max_chars = self.editorconfig.indent_size.max(1);
        for line in l1..=l2 {
            let line_len = self.buffer.line_len(line);
            if line_len == 0 {
                continue;
            }
            let line_start = self.buffer.line_start_idx(line);
            let take = match style {
                IndentStyle::Tabs => {
                    if matches!(self.buffer.char_at(line, 0), Some('\t')) { 1 } else { 0 }
                }
                IndentStyle::Spaces => {
                    let mut t = 0usize;
                    while t < max_chars && t < line_len {
                        match self.buffer.char_at(line, t) {
                            Some(c) if c.is_whitespace() => t += 1,
                            _ => break,
                        }
                    }
                    t
                }
            };
            if take > 0 {
                self.buffer.delete_range(line_start, line_start + take);
            }
        }
        self.cursor.line = l1;
        let col = self.first_non_blank_col(l1);
        self.cursor.col = col;
        self.cursor.want_col = col;
    }

    pub(super) fn first_non_blank_col(&self, line: usize) -> usize {
        let line_len = self.buffer.line_len(line);
        let mut col = 0;
        while col < line_len {
            match self.buffer.char_at(line, col) {
                Some(c) if c.is_whitespace() => col += 1,
                _ => break,
            }
        }
        col
    }

    pub(super) fn enter_insert(&mut self, w: InsertWhere) {
        self.history.record(&self.buffer.rope, self.cursor);
        match w {
            InsertWhere::Cursor => {}
            InsertWhere::AfterCursor => {
                let len = self.buffer.line_len(self.cursor.line);
                if self.cursor.col < len {
                    self.cursor.col += 1;
                    self.cursor.want_col = self.cursor.col;
                }
            }
            InsertWhere::LineBelow => {
                let len = self.buffer.line_len(self.cursor.line);
                let idx = self.buffer.pos_to_char(self.cursor.line, len);
                self.buffer.insert_at_idx(idx, "\n");
                self.cursor.line += 1;
                self.cursor.col = 0;
                self.cursor.want_col = 0;
            }
            InsertWhere::LineAbove => {
                let idx = self.buffer.line_start_idx(self.cursor.line);
                self.buffer.insert_at_idx(idx, "\n");
                self.cursor.col = 0;
                self.cursor.want_col = 0;
            }
            InsertWhere::LineFirstNonBlank => {
                let line_len = self.buffer.line_len(self.cursor.line);
                let mut col = 0;
                while col < line_len {
                    match self.buffer.char_at(self.cursor.line, col) {
                        Some(c) if c.is_whitespace() => col += 1,
                        _ => break,
                    }
                }
                self.cursor.col = col;
                self.cursor.want_col = col;
            }
            InsertWhere::LineEnd => {
                let len = self.buffer.line_len(self.cursor.line);
                self.cursor.col = len;
                self.cursor.want_col = len;
            }
        }
        self.mode = Mode::Insert;
    }

    pub(super) fn replace_char(&mut self, ch: char, count: usize) {
        let line = self.cursor.line;
        let line_len = self.buffer.line_len(line);
        if line_len == 0 {
            return;
        }
        let start = self.buffer.pos_to_char(line, self.cursor.col);
        let max_end = self.buffer.pos_to_char(line, line_len);
        let end = (start + count.max(1)).min(max_end);
        let actual = end - start;
        if actual == 0 {
            return;
        }
        self.buffer.delete_range(start, end);
        let mut buf = String::new();
        for _ in 0..actual {
            buf.push(ch);
        }
        self.buffer.insert_at_idx(start, &buf);
        self.cursor.col = self.cursor.col + actual.saturating_sub(1);
        self.cursor.want_col = self.cursor.col;
        self.clamp_cursor_normal();
    }

    pub(super) fn join_lines(&mut self, count: usize) {
        let times = count.max(1);
        for _ in 0..times {
            let cur_line = self.cursor.line;
            if cur_line + 1 >= self.buffer.line_count() {
                break;
            }
            let line_len = self.buffer.line_len(cur_line);
            let nl_idx = self.buffer.pos_to_char(cur_line, line_len);
            // Skip leading whitespace on the next line.
            let next_len = self.buffer.line_len(cur_line + 1);
            let mut skip = 0usize;
            while skip < next_len {
                match self.buffer.char_at(cur_line + 1, skip) {
                    Some(c) if c.is_whitespace() => skip += 1,
                    _ => break,
                }
            }
            self.buffer.delete_range(nl_idx, nl_idx + 1 + skip);
            // Insert a single space unless the cur line is empty or already ends in whitespace,
            // or the next line started with `)`.
            let cur_ends_ws = line_len > 0
                && self
                    .buffer
                    .char_at(cur_line, line_len - 1)
                    .map(|c| c.is_whitespace())
                    .unwrap_or(false);
            let next_starts_close = self
                .buffer
                .char_at(cur_line, line_len)
                .map(|c| c == ')')
                .unwrap_or(false);
            let insert_space = line_len > 0 && !cur_ends_ws && !next_starts_close;
            if insert_space {
                self.buffer.insert_at_idx(nl_idx, " ");
            }
            self.cursor.col = line_len;
            self.cursor.want_col = self.cursor.col;
        }
        self.clamp_cursor_normal();
    }

    /// `ds{char}` — strip the surrounding pair around the cursor. Reuses
    /// the text-object pair walker so nested pairs balance correctly.
    pub(super) fn surround_delete(&mut self, ch: char) {
        let Some((open_idx, close_idx, _open_str, _close_str)) =
            self.find_surround_around_cursor(ch)
        else {
            self.status_msg = format!("no surrounding {ch}");
            return;
        };
        // Delete the close first so the open's index doesn't shift.
        self.buffer.delete_range(close_idx, close_idx + 1);
        self.buffer.delete_range(open_idx, open_idx + 1);
        // Cursor lands where the opening delimiter was, biased to the
        // first content char if any remains.
        let total = self.buffer.total_chars();
        let new_pos = open_idx.min(total);
        self.cursor_to_idx(new_pos);
        self.clamp_cursor_normal();
    }

    /// `cs{old}{new}` — swap the surrounding pair.
    pub(super) fn surround_change(&mut self, from: char, to: char) {
        let Some((open_idx, close_idx, _, _)) = self.find_surround_around_cursor(from) else {
            self.status_msg = format!("no surrounding {from}");
            return;
        };
        let (new_open, new_close) = surround_open_close(to);
        // Replace close first to keep open's index stable.
        self.buffer.delete_range(close_idx, close_idx + 1);
        self.buffer.insert_at_idx(close_idx, new_close);
        self.buffer.delete_range(open_idx, open_idx + 1);
        self.buffer.insert_at_idx(open_idx, new_open);
        self.clamp_cursor_normal();
    }

    /// Visual `S{char}` — wrap the visual selection in the pair for `ch`.
    pub(super) fn surround_visual(&mut self, ch: char) {
        let kind = match self.mode {
            Mode::Visual(k) => k,
            _ => return,
        };
        let (start, end, _linewise) = self.visual_range_chars(kind);
        if end <= start {
            self.exit_visual();
            return;
        }
        let (open, close) = surround_open_close(ch);
        // Insert close at end first so start doesn't shift.
        self.buffer.insert_at_idx(end, close);
        self.buffer.insert_at_idx(start, open);
        self.cursor_to_idx(start);
        self.clamp_cursor_normal();
        self.exit_visual();
    }

    /// Walk back / forward to find the pair surrounding the cursor for the
    /// given pair-id char. Returns `(open_idx, close_idx, open_str, close_str)`.
    fn find_surround_around_cursor(
        &self,
        ch: char,
    ) -> Option<(usize, usize, &'static str, &'static str)> {
        let (open, close) = surround_open_close(ch);
        // For brackets we use balanced walking; for quotes / backticks we
        // can't balance, so just find the nearest enclosing pair on the
        // line by scanning out from the cursor.
        if is_paired_bracket(ch) {
            let here = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
            let open_c = open.chars().next().unwrap();
            let close_c = close.chars().next().unwrap();
            let mut depth = 1usize;
            let mut i = here;
            let mut o_idx = None;
            // If the cursor is on the opener itself, that's our left edge.
            if self.buffer.rope.get_char(here) == Some(open_c) {
                o_idx = Some(here);
            }
            while o_idx.is_none() && i > 0 {
                i -= 1;
                let c = self.buffer.rope.char(i);
                if c == close_c {
                    depth += 1;
                } else if c == open_c {
                    depth -= 1;
                    if depth == 0 {
                        o_idx = Some(i);
                        break;
                    }
                }
            }
            let o_idx = o_idx?;
            let mut depth = 1usize;
            let mut j = o_idx + 1;
            let total = self.buffer.total_chars();
            while j < total {
                let c = self.buffer.rope.char(j);
                if c == open_c {
                    depth += 1;
                } else if c == close_c {
                    depth -= 1;
                    if depth == 0 {
                        return Some((o_idx, j, open, close));
                    }
                }
                j += 1;
            }
            return None;
        }
        // Quote-style: nearest enclosing same-char on the same line.
        let line = self.cursor.line;
        let line_len = self.buffer.line_len(line);
        if line_len == 0 {
            return None;
        }
        let line_start = self.buffer.line_start_idx(line);
        let here_col = self.cursor.col.min(line_len);
        let chars: Vec<char> = self
            .buffer
            .rope
            .slice(line_start..line_start + line_len)
            .to_string()
            .chars()
            .collect();
        let target = ch;
        let mut left = None;
        let mut right = None;
        for i in (0..here_col).rev() {
            if chars[i] == target {
                left = Some(i);
                break;
            }
        }
        for i in here_col..chars.len() {
            if chars[i] == target {
                right = Some(i);
                break;
            }
        }
        let (l, r) = (left?, right?);
        Some((line_start + l, line_start + r, open, close))
    }

    /// Vim-style `Ctrl-A` / `Ctrl-X`. Walks the current line from the
    /// cursor forward to the next parsable number (decimal, `0x…`, `0b…`,
    /// `0o…`), parses it (with optional leading `-`), adds `delta * count`,
    /// and re-renders it preserving the original prefix and minimum width
    /// (so `007` + 1 stays `008`). Cursor lands on the last char of the
    /// new number, matching Vim's behaviour.
    pub(super) fn adjust_number(&mut self, delta: i64, count: usize) {
        let count = count.max(1) as i64;
        let line = self.cursor.line;
        let line_len = self.buffer.line_len(line);
        if line_len == 0 {
            self.status_msg = "no numbers found".into();
            return;
        }
        let line_start = self.buffer.line_start_idx(line);
        let line_text: String = self
            .buffer
            .rope
            .slice(line_start..line_start + line_len)
            .to_string();
        let chars: Vec<char> = line_text.chars().collect();
        let from_col = self.cursor.col.min(chars.len());
        let Some(num) = find_number_on_line(&chars, from_col) else {
            self.status_msg = "no numbers found".into();
            return;
        };
        let new_value = num.value.saturating_add(delta.saturating_mul(count));
        let formatted = format_number(&num, new_value);
        let abs_start = line_start + num.start_col;
        let abs_end = line_start + num.end_col;
        self.buffer.delete_range(abs_start, abs_end);
        self.buffer.insert_at_idx(abs_start, &formatted);
        // Cursor on the last char of the new number.
        let new_end_col = num.start_col + formatted.chars().count().saturating_sub(1);
        self.cursor.col = new_end_col;
        self.cursor.want_col = new_end_col;
    }

    pub(super) fn toggle_case(&mut self, count: usize) {
        let line = self.cursor.line;
        let line_len = self.buffer.line_len(line);
        if line_len == 0 {
            return;
        }
        for _ in 0..count.max(1) {
            if self.cursor.col >= self.buffer.line_len(self.cursor.line) {
                break;
            }
            let c = match self.buffer.char_at(self.cursor.line, self.cursor.col) {
                Some(c) => c,
                None => break,
            };
            let new_c = if c.is_lowercase() {
                c.to_uppercase().next().unwrap_or(c)
            } else if c.is_uppercase() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                c
            };
            let idx = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
            self.buffer.delete_range(idx, idx + 1);
            self.buffer.insert_char(self.cursor.line, self.cursor.col, new_c);
            // Advance unless we're at end of line.
            let len_now = self.buffer.line_len(self.cursor.line);
            if self.cursor.col + 1 < len_now {
                self.cursor.col += 1;
            }
        }
        self.cursor.want_col = self.cursor.col;
        self.clamp_cursor_normal();
    }

    pub(super) fn delete_char_forward(&mut self, count: usize, target: Option<char>) {
        let line_len = self.buffer.line_len(self.cursor.line);
        if line_len == 0 {
            return;
        }
        let start = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
        let max_end = self.buffer.pos_to_char(self.cursor.line, line_len);
        let end = (start + count).min(max_end);
        let removed = self.buffer.delete_range(start, end);
        if !removed.is_empty() {
            self.write_register(target, removed, false);
        }
        self.clamp_cursor_normal();
    }

    pub(super) fn put(&mut self, before: bool, count: usize, target: Option<char>) {
        let Some(reg) = self.read_register(target) else {
            return;
        };
        if reg.text.is_empty() {
            return;
        }
        if reg.linewise {
            let target_line = if before {
                self.cursor.line
            } else {
                self.cursor.line + 1
            };
            let mut text = String::new();
            for _ in 0..count {
                text.push_str(&reg.text);
            }
            if !text.ends_with('\n') {
                text.push('\n');
            }
            let total = self.buffer.total_chars();
            let idx = self.buffer.line_start_idx(target_line);
            // If pasting "below" past the end of a file with no trailing newline,
            // we need to lead with a newline rather than trailing one.
            let has_trailing_nl = total == 0
                || self
                    .buffer
                    .rope
                    .get_char(total - 1)
                    .map(|c| c == '\n')
                    .unwrap_or(false);
            if idx >= total && !has_trailing_nl {
                let to_insert = format!("\n{}", text.trim_end_matches('\n'));
                self.buffer.insert_at_idx(idx, &to_insert);
            } else {
                self.buffer.insert_at_idx(idx, &text);
            }
            self.cursor.line = target_line;
            self.cursor.col = 0;
            self.cursor.want_col = 0;
        } else {
            let target_idx = if before {
                self.buffer.pos_to_char(self.cursor.line, self.cursor.col)
            } else {
                let line_len = self.buffer.line_len(self.cursor.line);
                if line_len == 0 {
                    self.buffer.line_start_idx(self.cursor.line)
                } else {
                    self.buffer
                        .pos_to_char(self.cursor.line, self.cursor.col + 1)
                }
            };
            let mut text = String::new();
            for _ in 0..count {
                text.push_str(&reg.text);
            }
            let inserted_chars = text.chars().count();
            self.buffer.insert_at_idx(target_idx, &text);
            if inserted_chars > 0 {
                let new_idx = target_idx + inserted_chars - 1;
                self.cursor_to_idx(new_idx);
            }
            self.clamp_cursor_normal();
        }
    }

    pub(super) fn undo(&mut self) {
        if let Some(snap) = self.history.undo(&self.buffer.rope, self.cursor) {
            self.buffer.rope = snap.rope;
            self.cursor = snap.cursor;
            self.buffer.dirty = true;
            // Bump version so the highlight cache and LSP didChange know
            // to recompute — replacing the rope wholesale is still a
            // mutation, even if it goes through `buffer.rope = …` rather
            // than the per-edit helpers.
            self.buffer.version = self.buffer.version.wrapping_add(1);
            self.clamp_cursor_normal();
        } else {
            self.status_msg = "Already at oldest change".into();
        }
    }

    pub(super) fn redo(&mut self) {
        if let Some(snap) = self.history.redo(&self.buffer.rope, self.cursor) {
            self.buffer.rope = snap.rope;
            self.cursor = snap.cursor;
            self.buffer.dirty = true;
            self.buffer.version = self.buffer.version.wrapping_add(1);
            self.clamp_cursor_normal();
        } else {
            self.status_msg = "Already at newest change".into();
        }
    }
}

/// A number parsed out of a buffer line. `start_col` and `end_col` are
/// char-column positions on the line (half-open). `negative` is true when
/// the parsed digits had a leading `-`. `min_width` is the digit count
/// (excluding prefix and sign) so leading zeros are preserved on re-render.
#[derive(Debug, Clone)]
struct ParsedNumber {
    start_col: usize,
    end_col: usize,
    value: i64,
    base: NumberBase,
    negative: bool,
    min_width: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum NumberBase {
    Dec,
    Hex,
    Oct,
    Bin,
}

/// Vim-compatible number scan: walk from `from_col` to the end of `chars`
/// and return the first number we can parse. Recognises `0x…`, `0b…`,
/// `0o…`, plain decimals, and a leading `-` that's not the right operand
/// of an identifier (so `x-1` stays positive).
fn find_number_on_line(chars: &[char], from_col: usize) -> Option<ParsedNumber> {
    let n = chars.len();
    let mut i = from_col.min(n);
    while i < n {
        // Try to start a number at i.
        let (digits_start, base, prefix_chars) = if chars[i] == '0' && i + 1 < n {
            match chars[i + 1].to_ascii_lowercase() {
                'x' => (i + 2, NumberBase::Hex, 2),
                'b' => (i + 2, NumberBase::Bin, 2),
                'o' => (i + 2, NumberBase::Oct, 2),
                _ if chars[i].is_ascii_digit() => (i, NumberBase::Dec, 0),
                _ => (i, NumberBase::Dec, 0),
            }
        } else if chars[i].is_ascii_digit() {
            (i, NumberBase::Dec, 0)
        } else {
            i += 1;
            continue;
        };
        // Read digits.
        let valid = |c: char| match base {
            NumberBase::Dec => c.is_ascii_digit(),
            NumberBase::Hex => c.is_ascii_hexdigit(),
            NumberBase::Oct => ('0'..='7').contains(&c),
            NumberBase::Bin => c == '0' || c == '1',
        };
        let mut end = digits_start;
        while end < n && valid(chars[end]) {
            end += 1;
        }
        if end == digits_start {
            i += 1;
            continue;
        }
        // Optional leading `-` only when it's standalone (start of line or
        // following whitespace / opening punctuation) so identifiers like
        // `x-1` don't get re-interpreted.
        let mut start = i;
        let mut negative = false;
        if prefix_chars == 0 && start > 0 && chars[start - 1] == '-' {
            let two_back = if start >= 2 { Some(chars[start - 2]) } else { None };
            let standalone = match two_back {
                None => true,
                Some(c) => !(c.is_alphanumeric() || c == '_' || c == ')' || c == ']'),
            };
            if standalone {
                start -= 1;
                negative = true;
            }
        }
        let digits: String = chars[digits_start..end].iter().collect();
        let parsed = match base {
            NumberBase::Dec => i64::from_str_radix(&digits, 10).ok(),
            NumberBase::Hex => i64::from_str_radix(&digits, 16).ok(),
            NumberBase::Oct => i64::from_str_radix(&digits, 8).ok(),
            NumberBase::Bin => i64::from_str_radix(&digits, 2).ok(),
        }?;
        let value = if negative { -parsed } else { parsed };
        return Some(ParsedNumber {
            start_col: start,
            end_col: end,
            value,
            base,
            negative,
            min_width: digits.len(),
        });
    }
    None
}

/// Render `new_value` in the same shape as the original number — same
/// base, same prefix, same minimum digit width (so `007` + 1 stays `008`).
fn format_number(orig: &ParsedNumber, new_value: i64) -> String {
    let abs = new_value.unsigned_abs();
    let body = match orig.base {
        NumberBase::Dec => format!("{}", abs),
        NumberBase::Hex => format!("{:x}", abs),
        NumberBase::Oct => format!("{:o}", abs),
        NumberBase::Bin => format!("{:b}", abs),
    };
    // Pad with leading zeros up to the original width if the original
    // explicitly used leading zeros (i.e. it was wider than the natural
    // representation of its value).
    let padded = if body.len() < orig.min_width && orig.min_width > 1 {
        let pad = orig.min_width - body.len();
        format!("{}{}", "0".repeat(pad), body)
    } else {
        body
    };
    let prefix = match orig.base {
        NumberBase::Hex => "0x",
        NumberBase::Oct => "0o",
        NumberBase::Bin => "0b",
        NumberBase::Dec => "",
    };
    let sign = if new_value < 0 {
        "-"
    } else if orig.negative && new_value == 0 {
        // Was negative, now zero — drop the sign.
        ""
    } else {
        ""
    };
    format!("{sign}{prefix}{padded}")
}
