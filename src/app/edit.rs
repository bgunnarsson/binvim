//! Primitive edits invoked from `apply_action` — insert mode entry,
//! replace/delete/put, surround manipulation, indent/outdent, undo/redo,
//! number adjustments, and case toggling. Plus the line-number parser
//! helpers that `Ctrl-A`/`Ctrl-X` rely on.

use std::time::Instant;

use crate::editorconfig::IndentStyle;
use crate::mode::{Mode, Operator, VisualKind};
use crate::parser::{InsertWhere, PutStyle};

use super::pair::{is_paired_bracket, surround_open_close};
use super::state::{ReplaceSession, ReplaceUndo, YANK_FLASH_DURATION, YankHighlight};

impl super::App {
    /// Mirror a single-char insert at the primary cursor across all
    /// additional cursors. Bottom-up insertion order keeps lower
    /// positions stable during the loop; after all inserts each cursor
    /// (sorted ascending) lands at `original + (rank + 1)` chars.
    /// Caller is responsible for any cursor.col bookkeeping the normal
    /// insert path would have done — this routine handles all
    /// `additional_cursors` and the primary char-index together.
    pub(super) fn mirror_insert_char(&mut self, c: char) {
        let primary_pos = self
            .buffer
            .pos_to_char(self.window.cursor.line, self.window.cursor.col);
        if self.additional_cursors.is_empty() {
            self.buffer
                .insert_char(self.window.cursor.line, self.window.cursor.col, c);
            self.window.cursor.col += 1;
            self.window.cursor.want_col = self.window.cursor.col;
            return;
        }
        let mut positions: Vec<(usize, bool)> = vec![(primary_pos, true)];
        for &p in &self.additional_cursors {
            positions.push((p, false));
        }
        positions.sort_by_key(|x| x.0);
        // Insert from highest to lowest — earlier inserts (in this loop) are at
        // higher positions and don't shift later (lower) ones.
        let s = c.to_string();
        for (p, _) in positions.iter().rev() {
            self.buffer.insert_at_idx(*p, &s);
        }
        // Compute each cursor's final position: original + (rank + 1) where
        // rank is the index in sorted-ascending order. Each insertion shifts
        // every cursor at a strictly higher position by 1.
        let mut new_primary = primary_pos + 1;
        let mut new_additional: Vec<usize> = Vec::with_capacity(self.additional_cursors.len());
        for (rank, (p, is_primary)) in positions.iter().enumerate() {
            let new_pos = p + rank + 1;
            if *is_primary {
                new_primary = new_pos;
            } else {
                new_additional.push(new_pos);
            }
        }
        new_additional.sort();
        new_additional.dedup();
        self.additional_cursors = new_additional;
        self.cursor_to_idx(new_primary);
    }

    /// Mirror a backspace across primary + every additional cursor.
    /// Cursors at column 0 (or char index 0) are skipped — joining lines
    /// across the multi-cursor set is genuinely tricky and the user can
    /// fall back to single-cursor mode for that.
    pub(super) fn mirror_backspace(&mut self) {
        let primary_pos = self
            .buffer
            .pos_to_char(self.window.cursor.line, self.window.cursor.col);
        if self.additional_cursors.is_empty() {
            // Fall through to the caller's normal backspace path.
            return;
        }
        // Filter to cursors that have something to delete.
        let mut positions: Vec<(usize, bool)> = Vec::new();
        if primary_pos > 0 && self.window.cursor.col > 0 {
            positions.push((primary_pos, true));
        }
        for &p in &self.additional_cursors {
            if p > 0 {
                let line = self.buffer.rope.char_to_line(p);
                let line_start = self.buffer.rope.line_to_char(line);
                // Skip cursors that sit at column 0 — see doc comment.
                if p > line_start {
                    positions.push((p, false));
                }
            }
        }
        if positions.is_empty() {
            return;
        }
        positions.sort_by_key(|x| x.0);
        // Delete from highest to lowest so lower positions don't shift.
        for (p, _) in positions.iter().rev() {
            self.buffer.delete_range(*p - 1, *p);
        }
        // After all deletions, the cursor at sorted-asc rank R is at
        // p[R] - 1 (for own delete) - R (for lower-rank deletes that
        // shifted it down).
        let mut new_primary = primary_pos.saturating_sub(1);
        let mut new_additional: Vec<usize> = Vec::with_capacity(self.additional_cursors.len());
        // Keep any cursors we skipped (column 0) intact.
        for &p in &self.additional_cursors {
            if p == 0 {
                new_additional.push(p);
                continue;
            }
            let line = self.buffer.rope.char_to_line(p);
            let line_start = self.buffer.rope.line_to_char(line);
            if p <= line_start {
                new_additional.push(p);
            }
        }
        for (rank, (p, is_primary)) in positions.iter().enumerate() {
            let new_pos = p - 1 - rank;
            if *is_primary {
                new_primary = new_pos;
            } else {
                new_additional.push(new_pos);
            }
        }
        new_additional.sort();
        new_additional.dedup();
        self.additional_cursors = new_additional;
        self.cursor_to_idx(new_primary);
    }

    /// Per-line char-column positions of additional cursors on `line`.
    /// Used by the renderer to paint multi-cursor markers.
    pub fn line_multi_cursor_cols(&self, line: usize) -> Vec<usize> {
        if self.additional_cursors.is_empty() {
            return Vec::new();
        }
        let total = self.buffer.total_chars();
        let mut out = Vec::new();
        for &p in &self.additional_cursors {
            if p > total {
                continue;
            }
            let l = self.buffer.rope.char_to_line(p);
            if l == line {
                let line_start = self.buffer.rope.line_to_char(l);
                out.push(p - line_start);
            }
        }
        out
    }

    /// Set up a yank flash over the given char-index range. The renderer
    /// paints the range in a Peach background until the deadline passes.
    pub(super) fn flash_yank(&mut self, start: usize, end: usize) {
        if end <= start {
            return;
        }
        // Every yank flashes its range, so this is where `'[` / `']` learn it.
        self.buffer.marks.insert('[', start);
        self.buffer.marks.insert(']', end - 1);
        self.yank_highlight = Some(YankHighlight {
            start,
            end,
            expires_at: Instant::now() + YANK_FLASH_DURATION,
        });
    }

    /// `gu` / `gU` / `g~` / `g?` over `[start, end)`: rewritten in place, and
    /// no register — the case operators don't take the text anywhere.
    pub(super) fn recase_range(&mut self, start: usize, end: usize, how: crate::mode::CaseOp) {
        if end <= start {
            return;
        }
        let old = self.buffer.rope.slice(start..end).to_string();
        let new = how.apply(&old);
        if new != old {
            self.buffer.replace_range(start, end, &new);
        }
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
        self.window.cursor.line = l1;
        let col = self.first_non_blank_col(l1);
        self.window.cursor.col = col;
        self.window.cursor.want_col = col;
    }

    /// `=` — re-indents lines `l1..=l2` the way Enter indents a new line (see
    /// `indent_below`), each against the nearest non-blank line above it as
    /// already re-indented. Blank lines lose their whitespace.
    pub(super) fn reindent_range(&mut self, l1: usize, l2: usize) {
        let last = crate::motion::vim_line_count(&self.buffer).saturating_sub(1);
        let l2 = l2.min(last);
        let unit = self.editorconfig.indent_string();
        let text_of = |buffer: &crate::buffer::Buffer, line: usize| {
            let start = buffer.line_start_idx(line);
            buffer
                .rope
                .slice(start..start + buffer.line_len(line))
                .to_string()
        };
        let mut above = (0..l1)
            .rev()
            .map(|line| text_of(&self.buffer, line))
            .find(|text| !text.trim().is_empty());
        for line in l1..=l2 {
            let text = text_of(&self.buffer, line);
            let body = text.trim_start_matches([' ', '\t']);
            let blank = body.trim().is_empty();
            let lead = match above.as_deref() {
                Some(prev) if !blank => indent_below(prev, body, &unit),
                _ => String::new(),
            };
            // The old indent is spaces and tabs only, so bytes count chars.
            let old_len = text.len() - body.len();
            if lead != text[..old_len] {
                let start = self.buffer.line_start_idx(line);
                self.buffer.replace_range(start, start + old_len, &lead);
            }
            if !blank {
                above = Some(format!("{lead}{body}"));
            }
        }
        self.window.cursor.line = l1;
        let col = self.first_non_blank_col(l1);
        self.window.cursor.col = col;
        self.window.cursor.want_col = col;
    }

    /// `gq` / `gw` — re-flows lines `l1..=l2` to `.editorconfig`'s
    /// `max_line_length`, or 79 columns as Vim does without a `textwidth`.
    /// `gq` leaves the cursor on the last line it wrote, `gw` where it was.
    pub(super) fn format_lines(&mut self, l1: usize, l2: usize, keep_cursor: bool) {
        let last = crate::motion::vim_line_count(&self.buffer).saturating_sub(1);
        let l2 = l2.min(last);
        let width = self.editorconfig.max_line_length.unwrap_or(79);
        let marker = self
            .buffer
            .path
            .as_deref()
            .and_then(crate::lang::Lang::detect)
            .and_then(|lang| lang.line_comment_prefix());
        let start = self.buffer.line_start_idx(l1);
        let end = self.buffer.line_start_idx(l2) + self.buffer.line_len(l2);
        let old = self.buffer.rope.slice(start..end).to_string();
        let new = reflow(&old, width, marker, self.editorconfig.tab_width);
        if new != old {
            self.buffer.replace_range(start, end, &new);
        }
        let cursor = self.window.cursor;
        let last = crate::motion::vim_line_count(&self.buffer).saturating_sub(1);
        let (line, col) = if keep_cursor {
            let line = cursor.line.min(last);
            (
                line,
                cursor.col.min(self.buffer.line_len(line).saturating_sub(1)),
            )
        } else {
            let line = l1 + new.matches('\n').count();
            (line, self.first_non_blank_col(line))
        };
        self.window.cursor.line = line;
        self.window.cursor.col = col;
        self.window.cursor.want_col = col;
    }

    /// `!{motion}` / `!!` — opens the `:` line with the lines' range typed
    /// in (`:5,7!`), for the command to filter them through. Vim types
    /// `.,.+2`; binvim's ranges take line numbers only.
    pub(super) fn open_filter_prompt(&mut self, l1: usize, l2: usize) {
        self.cmdline = if l1 == l2 {
            format!("{}!", l1 + 1)
        } else {
            format!("{},{}!", l1 + 1, l2 + 1)
        };
        self.cmdline_cursor = self.cmdline.len();
        self.history_reset();
        self.mode = Mode::Command;
    }

    /// `:{range}!cmd` — lines `l1..=l2` go to `cmd` on stdin and what it
    /// prints takes their place. A command that fails leaves them as they
    /// were and says why.
    pub(super) fn filter_lines(&mut self, l1: usize, l2: usize, cmd: &str) {
        // Enter on the bare `:5,7!` `!` opens would otherwise run nothing
        // and take the lines with it.
        if cmd.trim().is_empty() {
            self.status_msg = "E34: No previous command".into();
            return;
        }
        let start = self.buffer.line_start_idx(l1);
        let end = self.buffer.line_start_idx(l2 + 1);
        let input = self.buffer.rope.slice(start..end).to_string();
        let mut output = match crate::format::filter_through_shell(cmd, &input) {
            Ok(output) => output,
            Err(e) => {
                self.status_msg = e;
                return;
            }
        };
        // The line after the range stays its own, as the input's newline kept it.
        if input.ends_with('\n') && !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        self.history.record(&self.buffer.rope, self.window.cursor);
        self.buffer.replace_range(start, end, &output);
        let last = crate::motion::vim_line_count(&self.buffer).saturating_sub(1);
        let line = l1.min(last);
        let col = self.first_non_blank_col(line);
        self.window.cursor.line = line;
        self.window.cursor.col = col;
        self.window.cursor.want_col = col;
        let lines = l2 - l1 + 1;
        let plural = if lines == 1 { "" } else { "s" };
        self.status_msg = format!("{lines} line{plural} filtered");
    }

    /// `>`, `<`, `=`, `gq` / `gw` and `!` over whole lines.
    pub(super) fn shift_lines(&mut self, op: Operator, l1: usize, l2: usize) {
        match op {
            Operator::Indent => self.indent_lines(l1, l2),
            Operator::Outdent => self.outdent_lines(l1, l2),
            Operator::Reindent => self.reindent_range(l1, l2),
            Operator::Format { keep_cursor } => self.format_lines(l1, l2, keep_cursor),
            Operator::Filter => self.open_filter_prompt(l1, l2),
            Operator::Delete | Operator::Change | Operator::Yank | Operator::Case(_) => {}
        }
    }

    /// `:m` / `:t` / `:co` — lines `l1..=l2` moved, or copied, to start at
    /// line `at`, counted before the move. The line they end on.
    pub(super) fn transfer_lines(
        &mut self,
        l1: usize,
        l2: usize,
        at: usize,
        copy: bool,
    ) -> Result<usize, String> {
        if !copy && at > l1 && at <= l2 {
            return Err("E134: Cannot move a range of lines into itself".into());
        }
        let text_of = |app: &Self, line: usize| {
            let start = app.buffer.line_start_idx(line);
            let len = app.buffer.line_len(line);
            app.buffer.rope.slice(start..start + len).to_string()
        };
        let block: Vec<String> = (l1..=l2).map(|line| text_of(self, line)).collect();
        let n = block.len();
        self.history.record(&self.buffer.rope, self.window.cursor);
        if copy {
            self.replace_lines(at, at, &block);
            return Ok(at + n - 1);
        }
        if at <= l1 {
            let mut lines = block;
            lines.extend((at..l1).map(|line| text_of(self, line)));
            self.replace_lines(at, l2 + 1, &lines);
            return Ok(at + n - 1);
        }
        let mut lines: Vec<String> = (l2 + 1..at).map(|line| text_of(self, line)).collect();
        lines.extend(block);
        self.replace_lines(l1, at, &lines);
        Ok(at - 1)
    }

    /// Lines `lo..hi` replaced by `lines`, one line each — `lo == hi` puts
    /// them in before line `lo`, which may be one past the last. The last
    /// line keeps, or goes without, a line break as the buffer's did.
    pub(super) fn replace_lines(&mut self, lo: usize, hi: usize, lines: &[String]) {
        let count = crate::motion::vim_line_count(&self.buffer);
        let total = self.buffer.total_chars();
        let at = |app: &Self, line: usize| {
            if line < count {
                app.buffer.line_start_idx(line)
            } else {
                total
            }
        };
        let (start, end) = (at(self, lo), at(self, hi));
        let ends_in_break = total > 0 && self.buffer.rope.char(total - 1) == '\n';
        let mut text = lines.join("\n");
        if hi < count || ends_in_break {
            text.push('\n');
        } else if lo == count {
            text.insert(0, '\n');
        }
        self.buffer.replace_range(start, end, &text);
    }

    /// `:le` / `:ri` / `:ce` — lines `l1..=l2` indented to `width` columns
    /// for `Left`, or right-aligned / centred in `width` columns, which
    /// default to `max_line_length`, else 80. Blank lines come out empty.
    pub(super) fn align_lines(
        &mut self,
        l1: usize,
        l2: usize,
        align: crate::command::Align,
        width: Option<usize>,
    ) {
        use crate::command::Align;
        let tab = self.editorconfig.tab_width.max(1);
        let text_width = width.unwrap_or(self.editorconfig.max_line_length.unwrap_or(80));
        for line in l1..=l2 {
            let start = self.buffer.line_start_idx(line);
            let len = self.buffer.line_len(line);
            let old = self.buffer.rope.slice(start..start + len).to_string();
            let body = match align {
                Align::Left => old.trim_start(),
                Align::Right | Align::Center => old.trim(),
            };
            let columns = match align {
                Align::Left => width.unwrap_or(0),
                Align::Right => text_width.saturating_sub(display_width(body, tab)),
                Align::Center => text_width.saturating_sub(display_width(body, tab)) / 2,
            };
            let new = if body.is_empty() {
                String::new()
            } else {
                format!("{}{body}", self.indent_text(columns))
            };
            if new != old {
                self.buffer.replace_range(start, start + len, &new);
            }
        }
    }

    /// `:retab[!] [N]` — lines `l1..=l2` with every run of blanks holding a
    /// tab (with `!`, every run) laid out again for tabstop `N`: in spaces
    /// when the file indents with spaces, in tabs where they fit when it uses
    /// tabs. Each run keeps the columns the old tabstop gave it, and `N`
    /// becomes the tabstop.
    pub(super) fn retab_lines(&mut self, l1: usize, l2: usize, bang: bool, tabstop: Option<usize>) {
        let old = self.editorconfig.tab_width.max(1);
        let new = tabstop.unwrap_or(old).max(1);
        let tabs = matches!(
            self.editorconfig.indent_style,
            crate::editorconfig::IndentStyle::Tabs
        );
        for line in l1..=l2 {
            let start = self.buffer.line_start_idx(line);
            let len = self.buffer.line_len(line);
            let text = self.buffer.rope.slice(start..start + len).to_string();
            let redone = retab(&text, old, new, tabs, bang);
            if redone != text {
                self.buffer.replace_range(start, start + len, &redone);
            }
        }
        self.editorconfig.tab_width = new;
    }

    /// `:sort` — lines `l1..=l2` sorted as `opts` says, `re` picking the part
    /// of each line that counts.
    pub(super) fn sort_range(
        &mut self,
        l1: usize,
        l2: usize,
        opts: &crate::command::SortOpts,
        re: Option<&regex::Regex>,
    ) {
        let lines: Vec<String> = (l1..=l2)
            .map(|line| {
                let start = self.buffer.line_start_idx(line);
                let len = self.buffer.line_len(line);
                self.buffer.rope.slice(start..start + len).to_string()
            })
            .collect();
        let sorted = sort_lines(lines.clone(), opts, re);
        if sorted == lines {
            return;
        }
        self.history.record(&self.buffer.rope, self.window.cursor);
        self.replace_lines(l1, l2 + 1, &sorted);
    }

    /// Leading blanks `columns` wide — tabs and then spaces when the file
    /// indents with tabs.
    fn indent_text(&self, columns: usize) -> String {
        let tab = self.editorconfig.tab_width.max(1);
        match self.editorconfig.indent_style {
            crate::editorconfig::IndentStyle::Tabs => {
                "\t".repeat(columns / tab) + &" ".repeat(columns % tab)
            }
            crate::editorconfig::IndentStyle::Spaces => " ".repeat(columns),
        }
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
                    if matches!(self.buffer.char_at(line, 0), Some('\t')) {
                        1
                    } else {
                        0
                    }
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
        self.window.cursor.line = l1;
        let col = self.first_non_blank_col(l1);
        self.window.cursor.col = col;
        self.window.cursor.want_col = col;
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
        self.history.record(&self.buffer.rope, self.window.cursor);
        // Multi-cursor preservation: `i` and `a` keep the secondary
        // cursors aligned with the primary's transformation. Every other
        // Insert-entry (line-above/below, jump to line ends) does
        // something line-specific that doesn't translate cleanly to N
        // positions — collapse so mirroring isn't surprising.
        match w {
            InsertWhere::Cursor => {}
            InsertWhere::AfterCursor => {
                let total = self.buffer.total_chars();
                for pos in &mut self.additional_cursors {
                    let line = self.buffer.rope.char_to_line(*pos);
                    let line_start = self.buffer.rope.line_to_char(line);
                    let col = *pos - line_start;
                    let len = self.buffer.line_len(line);
                    if col < len && *pos < total {
                        *pos += 1;
                    }
                }
            }
            _ => {
                self.additional_cursors.clear();
            }
        }
        match w {
            InsertWhere::Cursor => {}
            InsertWhere::AfterCursor => {
                let len = self.buffer.line_len(self.window.cursor.line);
                if self.window.cursor.col < len {
                    self.window.cursor.col += 1;
                    self.window.cursor.want_col = self.window.cursor.col;
                }
            }
            InsertWhere::LineBelow => {
                let len = self.buffer.line_len(self.window.cursor.line);
                let idx = self.buffer.pos_to_char(self.window.cursor.line, len);
                self.buffer.insert_at_idx(idx, "\n");
                self.window.cursor.line += 1;
                self.window.cursor.col = 0;
                self.window.cursor.want_col = 0;
            }
            InsertWhere::LineAbove => {
                let idx = self.buffer.line_start_idx(self.window.cursor.line);
                self.buffer.insert_at_idx(idx, "\n");
                self.window.cursor.col = 0;
                self.window.cursor.want_col = 0;
            }
            InsertWhere::LineFirstNonBlank => {
                let line_len = self.buffer.line_len(self.window.cursor.line);
                let mut col = 0;
                while col < line_len {
                    match self.buffer.char_at(self.window.cursor.line, col) {
                        Some(c) if c.is_whitespace() => col += 1,
                        _ => break,
                    }
                }
                self.window.cursor.col = col;
                self.window.cursor.want_col = col;
            }
            InsertWhere::LineEnd => {
                let len = self.buffer.line_len(self.window.cursor.line);
                self.window.cursor.col = len;
                self.window.cursor.want_col = len;
            }
            // Before any insert there's no `^`, and `gi` starts where it is.
            InsertWhere::LastInsert => {
                if let Some((line, col)) = self.buffer.mark('^') {
                    let line = line.min(self.buffer.line_count().saturating_sub(1));
                    let col = col.min(self.buffer.line_len(line));
                    self.window.cursor.line = line;
                    self.window.cursor.col = col;
                    self.window.cursor.want_col = col;
                }
            }
        }
        self.mode = Mode::Insert;
    }

    pub(super) fn replace_char(&mut self, ch: char, count: usize) {
        let line = self.window.cursor.line;
        let line_len = self.buffer.line_len(line);
        if line_len == 0 {
            return;
        }
        let start = self.buffer.pos_to_char(line, self.window.cursor.col);
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
        self.window.cursor.col += actual.saturating_sub(1);
        self.window.cursor.want_col = self.window.cursor.col;
        self.clamp_cursor_normal();
    }

    /// `R` — Insert, typing over the text instead of before it. One cursor:
    /// what each char overwrites is the primary's to put back.
    pub(super) fn enter_replace(&mut self, count: usize) {
        self.enter_insert(InsertWhere::Cursor);
        self.additional_cursors.clear();
        self.replace_session = Some(ReplaceSession {
            count: count.max(1),
            ..ReplaceSession::default()
        });
    }

    /// A char typed in Replace mode takes the place of the one under the
    /// cursor, or goes on the end once the line runs out.
    pub(super) fn replace_mode_char(&mut self, c: char) {
        let line = self.window.cursor.line;
        let col = self.window.cursor.col;
        let at = self.buffer.pos_to_char(line, col);
        let under = self
            .buffer
            .char_at(line, col)
            .filter(|_| col < self.buffer.line_len(line));
        let typed = c.to_string();
        let undo = match under {
            Some(old) => {
                self.buffer.replace_range(at, at + 1, &typed);
                ReplaceUndo::Replaced { at, old }
            }
            None => {
                self.buffer.insert_at_idx(at, &typed);
                ReplaceUndo::Added { at, len: 1 }
            }
        };
        self.cursor_to_idx(at + 1);
        if let Some(session) = self.replace_session.as_mut() {
            session.undo.push(undo);
            session.typed.push(c);
        }
    }

    /// `Backspace` in Replace mode takes back the last char typed and puts
    /// back what it overwrote. With nothing left to take back it only moves
    /// left, as Vim's does over text the session didn't type.
    pub(super) fn replace_mode_backspace(&mut self) {
        let undo = self.replace_session.as_mut().and_then(|session| {
            session.typed.pop();
            session.undo.pop()
        });
        match undo {
            Some(ReplaceUndo::Replaced { at, old }) => {
                self.buffer.replace_range(at, at + 1, &old.to_string());
                self.cursor_to_idx(at);
            }
            Some(ReplaceUndo::Added { at, len }) => {
                self.buffer.delete_range(at, at + len);
                self.cursor_to_idx(at);
            }
            None if self.window.cursor.col > 0 => {
                self.window.cursor.col -= 1;
                self.window.cursor.want_col = self.window.cursor.col;
            }
            None => {}
        }
    }

    /// `Esc` ends Replace mode, typing the text again first for a count:
    /// `3Rab` puts `ab` in three times. A line break goes in bare the
    /// second time round — Enter's indent came from the line it split.
    pub(super) fn replace_mode_finish(&mut self) {
        let Some(session) = self.replace_session.take() else {
            return;
        };
        for _ in 1..session.count {
            for c in session.typed.chars() {
                if c == '\n' {
                    let at = self
                        .buffer
                        .pos_to_char(self.window.cursor.line, self.window.cursor.col);
                    self.buffer.insert_at_idx(at, "\n");
                    self.cursor_to_idx(at + 1);
                } else {
                    self.replace_mode_char(c);
                }
            }
        }
    }

    /// Ctrl-J / Ctrl-K — move the cursor's line (Normal mode) or the
    /// selected line range (Visual mode) up or down by `count` positions.
    /// Cursor and visual anchor follow the moving block so a Visual
    /// selection stays attached to the same content after the shift.
    /// Clamped at file boundaries — moving past them is a no-op rather
    /// than scrolling content off the edge.
    pub(super) fn move_lines(&mut self, down: bool, count: usize) {
        let count = count.max(1);
        let total = self.buffer.line_count();
        if total == 0 {
            return;
        }

        let (mut start_line, mut end_line) = match (self.mode, self.window.visual_anchor) {
            (crate::mode::Mode::Visual(_), Some(anchor)) => {
                let a = anchor.line.min(self.window.cursor.line);
                let b = anchor.line.max(self.window.cursor.line);
                (a, b)
            }
            _ => (self.window.cursor.line, self.window.cursor.line),
        };

        // Effective last line excludes ropey's phantom trailing empty
        // entry when the file ends in `\n`.
        let last_real = if is_phantom_trailing_line(&self.buffer, total.saturating_sub(1)) {
            total.saturating_sub(2)
        } else {
            total.saturating_sub(1)
        };
        let step = if down {
            let headroom = last_real.saturating_sub(end_line);
            count.min(headroom)
        } else {
            count.min(start_line)
        };
        if step == 0 {
            return;
        }

        self.history.record(&self.buffer.rope, self.window.cursor);
        for _ in 0..step {
            if down {
                shift_block_down_by_one(&mut self.buffer, start_line, end_line);
                start_line += 1;
                end_line += 1;
            } else {
                shift_block_up_by_one(&mut self.buffer, start_line, end_line);
                start_line -= 1;
                end_line -= 1;
            }
        }

        let delta = step as isize * if down { 1 } else { -1 };
        self.window.cursor.line = (self.window.cursor.line as isize + delta).max(0) as usize;
        if let Some(anchor) = self.window.visual_anchor.as_mut() {
            anchor.line = (anchor.line as isize + delta).max(0) as usize;
        }
        self.clamp_cursor_normal();
    }

    pub(super) fn join_lines(&mut self, joins: usize, spaces: bool) {
        for _ in 0..joins {
            let cur_line = self.window.cursor.line;
            if cur_line + 1 >= self.buffer.line_count() {
                break;
            }
            let line_len = self.buffer.line_len(cur_line);
            let nl_idx = self.buffer.pos_to_char(cur_line, line_len);
            // `gJ`: only the line break goes.
            if !spaces {
                self.buffer.delete_range(nl_idx, nl_idx + 1);
                self.window.cursor.col = line_len;
                self.window.cursor.want_col = line_len;
                continue;
            }
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
            self.window.cursor.col = line_len;
            self.window.cursor.want_col = self.window.cursor.col;
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
        // Block selection: wrap each row's column slice independently —
        // (anchor.col, cursor.col) defines the rectangle, and we insert
        // `open`/`close` at `(c1, c2+1)` on every row in the span. Going
        // bottom-up keeps higher-row char indices stable while we edit.
        if matches!(kind, VisualKind::Block) {
            let anchor = self.window.visual_anchor.unwrap_or(self.window.cursor);
            let l1 = anchor.line.min(self.window.cursor.line);
            let l2 = anchor.line.max(self.window.cursor.line);
            let c1 = anchor.col.min(self.window.cursor.col);
            let c2 = anchor.col.max(self.window.cursor.col);
            let (open, close) = surround_open_close(ch);
            for line in (l1..=l2).rev() {
                let line_len = self.buffer.line_len(line);
                // Skip rows the rectangle doesn't actually intersect —
                // matches Vim's "block over short lines is a no-op
                // there" rule.
                if c1 > line_len {
                    continue;
                }
                let start_col = c1.min(line_len);
                let end_col = (c2 + 1).min(line_len);
                let line_start = self.buffer.line_start_idx(line);
                self.buffer.insert_at_idx(line_start + end_col, close);
                self.buffer.insert_at_idx(line_start + start_col, open);
            }
            // Land the cursor on the freshly-inserted opener of the
            // first row — same convention as charwise surround.
            self.window.cursor.line = l1;
            self.window.cursor.col = c1;
            self.window.cursor.want_col = c1;
            self.clamp_cursor_normal();
            self.exit_visual();
            return;
        }
        let (start, end, _linewise) = self.visual_range_chars(kind);
        if end <= start {
            self.exit_visual();
            return;
        }
        self.surround_wrap(start, end, ch, false);
        self.exit_visual();
    }

    /// Wraps `[start, end)` in the pair for `ch` — Visual `S` and `ys`. With
    /// `own_lines` (`yS`) the pair goes on lines of its own and the text
    /// between them is indented a level.
    pub(super) fn surround_wrap(&mut self, start: usize, end: usize, ch: char, own_lines: bool) {
        let (open, close) = surround_open_close(ch);
        // Close first in both branches, so `start` doesn't shift.
        if own_lines {
            let first = self.buffer.rope.char_to_line(start);
            let last = self
                .buffer
                .rope
                .char_to_line(end.saturating_sub(1).max(start));
            let indent: String = self
                .buffer
                .rope
                .line(first)
                .chars()
                .take_while(|c| matches!(c, ' ' | '\t'))
                .collect();
            self.buffer
                .insert_at_idx(end, &format!("\n{indent}{close}"));
            self.buffer
                .insert_at_idx(start, &format!("{open}\n{indent}"));
            self.indent_lines(first + 1, last + 1);
        } else {
            self.buffer.insert_at_idx(end, close);
            self.buffer.insert_at_idx(start, open);
        }
        self.cursor_to_idx(start);
        self.clamp_cursor_normal();
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
            let here = self
                .buffer
                .pos_to_char(self.window.cursor.line, self.window.cursor.col);
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
        let line = self.window.cursor.line;
        let line_len = self.buffer.line_len(line);
        if line_len == 0 {
            return None;
        }
        let line_start = self.buffer.line_start_idx(line);
        let here_col = self.window.cursor.col.min(line_len);
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
        for (i, &c) in chars.iter().enumerate().skip(here_col) {
            if c == target {
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
        let line = self.window.cursor.line;
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
        let from_col = self.window.cursor.col.min(chars.len());
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
        self.window.cursor.col = new_end_col;
        self.window.cursor.want_col = new_end_col;
    }

    /// Visual `Ctrl-A` / `Ctrl-X`: the first number in the selected part of
    /// each line. `progressive` (`g Ctrl-A`) takes each line that has one a
    /// step further than the last, so a column of zeros becomes 1, 2, 3.
    pub(super) fn visual_adjust_number(&mut self, delta: i64, count: usize, progressive: bool) {
        let Mode::Visual(kind) = self.mode else {
            return;
        };
        let anchor = self.window.visual_anchor.unwrap_or(self.window.cursor);
        let cursor = self.window.cursor;
        let (first, last) = if (anchor.line, anchor.col) <= (cursor.line, cursor.col) {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };
        let c1 = anchor.col.min(cursor.col);
        let c2 = anchor.col.max(cursor.col);
        let step = delta.saturating_mul(count.max(1) as i64);
        let mut found: i64 = 0;
        for line in first.line..=last.line {
            let line_len = self.buffer.line_len(line);
            let (from, to) = match kind {
                VisualKind::Block => (c1, c2 + 1),
                VisualKind::Line => (0, line_len),
                VisualKind::Char => {
                    let from = if line == first.line { first.col } else { 0 };
                    let to = if line == last.line {
                        last.col + 1
                    } else {
                        line_len
                    };
                    (from, to)
                }
            };
            let to = to.min(line_len);
            if from >= to {
                continue;
            }
            // Only the selected part of the line is searched.
            let line_start = self.buffer.line_start_idx(line);
            let chars: Vec<char> = self
                .buffer
                .rope
                .slice(line_start..line_start + to)
                .chars()
                .collect();
            let Some(num) = find_number_on_line(&chars, from) else {
                continue;
            };
            found += 1;
            let steps = if progressive { found } else { 1 };
            let value = num.value.saturating_add(step.saturating_mul(steps));
            let formatted = format_number(&num, value);
            self.buffer.replace_range(
                line_start + num.start_col,
                line_start + num.end_col,
                &formatted,
            );
        }
        if found == 0 {
            self.status_msg = "no numbers found".into();
        }
        let start_col = match kind {
            VisualKind::Block => c1,
            VisualKind::Line => 0,
            VisualKind::Char => first.col,
        };
        self.exit_visual();
        self.window.cursor.line = first.line;
        self.window.cursor.col = start_col;
        self.window.cursor.want_col = start_col;
        self.clamp_cursor_normal();
    }

    pub(super) fn toggle_case(&mut self, count: usize) {
        let line = self.window.cursor.line;
        let line_len = self.buffer.line_len(line);
        if line_len == 0 {
            return;
        }
        for _ in 0..count.max(1) {
            if self.window.cursor.col >= self.buffer.line_len(self.window.cursor.line) {
                break;
            }
            let c = match self
                .buffer
                .char_at(self.window.cursor.line, self.window.cursor.col)
            {
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
            let idx = self
                .buffer
                .pos_to_char(self.window.cursor.line, self.window.cursor.col);
            self.buffer.delete_range(idx, idx + 1);
            self.buffer
                .insert_char(self.window.cursor.line, self.window.cursor.col, new_c);
            // Advance unless we're at end of line.
            let len_now = self.buffer.line_len(self.window.cursor.line);
            if self.window.cursor.col + 1 < len_now {
                self.window.cursor.col += 1;
            }
        }
        self.window.cursor.want_col = self.window.cursor.col;
        self.clamp_cursor_normal();
    }

    pub(super) fn delete_char_forward(&mut self, count: usize, target: Option<char>) {
        let line_len = self.buffer.line_len(self.window.cursor.line);
        if line_len == 0 {
            return;
        }
        let start = self
            .buffer
            .pos_to_char(self.window.cursor.line, self.window.cursor.col);
        let max_end = self.buffer.pos_to_char(self.window.cursor.line, line_len);
        let end = (start + count).min(max_end);
        let removed = self.buffer.delete_range(start, end);
        if !removed.is_empty() {
            self.write_register(target, removed, false);
        }
        self.clamp_cursor_normal();
    }

    pub(super) fn put(
        &mut self,
        before: bool,
        count: usize,
        target: Option<char>,
        style: PutStyle,
    ) {
        let Some(reg) = self.read_register(target) else {
            return;
        };
        if reg.text.is_empty() {
            return;
        }
        if reg.linewise {
            let target_line = if before {
                self.window.cursor.line
            } else {
                self.window.cursor.line + 1
            };
            let mut text = String::new();
            for _ in 0..count {
                text.push_str(&reg.text);
            }
            if !text.ends_with('\n') {
                text.push('\n');
            }
            if style == PutStyle::Reindent {
                let line = self.window.cursor.line;
                let start = self.buffer.line_start_idx(line);
                let indent = self.first_non_blank_col(line);
                let lead = self.buffer.rope.slice(start..start + indent).to_string();
                text = reindent_lines(&text, &lead);
            }
            let lines = text.matches('\n').count();
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
            let last = crate::motion::vim_line_count(&self.buffer).saturating_sub(1);
            let (line, col) = match style {
                // Put at the end of the file there's no line after, so it
                // stays on the last, as Vim's does.
                PutStyle::CursorAfter => ((target_line + lines).min(last), 0),
                PutStyle::Reindent => (target_line, self.first_non_blank_col(target_line)),
                PutStyle::Plain => (target_line, 0),
            };
            self.window.cursor.line = line;
            self.window.cursor.col = col;
            self.window.cursor.want_col = col;
        } else {
            let target_idx = if before {
                self.buffer
                    .pos_to_char(self.window.cursor.line, self.window.cursor.col)
            } else {
                let line_len = self.buffer.line_len(self.window.cursor.line);
                if line_len == 0 {
                    self.buffer.line_start_idx(self.window.cursor.line)
                } else {
                    self.buffer
                        .pos_to_char(self.window.cursor.line, self.window.cursor.col + 1)
                }
            };
            let mut text = String::new();
            for _ in 0..count {
                text.push_str(&reg.text);
            }
            let inserted_chars = text.chars().count();
            self.buffer.insert_at_idx(target_idx, &text);
            if style == PutStyle::CursorAfter {
                self.cursor_to_idx(target_idx + inserted_chars);
            } else if inserted_chars > 0 {
                let new_idx = target_idx + inserted_chars - 1;
                self.cursor_to_idx(new_idx);
            }
            self.clamp_cursor_normal();
        }
    }

    pub(super) fn undo(&mut self) {
        if let Some(snap) = self.history.undo(&self.buffer.rope, self.window.cursor) {
            self.buffer.rope = snap.rope;
            self.window.cursor = snap.cursor;
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
        if let Some(snap) = self.history.redo(&self.buffer.rope, self.window.cursor) {
            self.buffer.rope = snap.rope;
            self.window.cursor = snap.cursor;
            self.buffer.dirty = true;
            self.buffer.version = self.buffer.version.wrapping_add(1);
            self.clamp_cursor_normal();
        } else {
            self.status_msg = "Already at newest change".into();
        }
    }
}

/// A line ending in `text` opens a block, so the line after it goes a level
/// deeper — Enter's rule, and `=`'s.
pub(super) fn opens_block(text: &str) -> bool {
    let text = text.trim_end();
    text.ends_with(['{', '[', '(', ':']) || text.ends_with("=>") || text.ends_with("->")
}

/// The indent `=` gives a line holding `body` under the line `above`: the
/// same as `above`, a level deeper when `above` opens a block, and a level
/// shallower when `body` starts by closing one. The ` * ` lines of a block
/// comment sit one space in from its `/*`, and the line after one comes
/// back out.
fn indent_below(above: &str, body: &str, unit: &str) -> String {
    let mut lead = leading_ws(above).to_string();
    let above_body = &above[lead.len()..];
    if body.starts_with('*') && above_body.starts_with("/*") {
        lead.push(' ');
        return lead;
    }
    if body.starts_with('*') && above_body.starts_with('*') {
        return lead;
    }
    if above_body.starts_with('*') && lead.ends_with(' ') {
        lead.pop();
    }
    if opens_block(above) {
        lead.push_str(unit);
    }
    if body.starts_with(['}', ']', ')']) {
        lead = dedent_once(&lead, unit);
    }
    lead
}

fn dedent_once(lead: &str, unit: &str) -> String {
    match lead.strip_suffix(unit) {
        Some(shorter) => shorter.to_string(),
        // Tabs where the unit is spaces, or the other way round.
        None => lead[..lead.len().saturating_sub(1)].to_string(),
    }
}

/// `gq`'s re-flow of whole lines `text` (no final newline) to `width`
/// columns. Lines with the same indent and comment marker — `marker` and any
/// repeat of its characters, so `///` and `//!` too — form a paragraph and
/// keep both on every line; blank and marker-only lines end one and stay.
fn reflow(text: &str, width: usize, marker: Option<&str>, tab_width: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut para: Option<(String, Vec<&str>)> = None;
    for line in text.split('\n') {
        let (prefix, body) = split_prefix(line, marker);
        let words: Vec<&str> = body.split_whitespace().collect();
        if words.is_empty() {
            fill_paragraph(&mut out, para.take(), width, tab_width);
            out.push(line.trim_end().to_string());
            continue;
        }
        match para.as_mut() {
            Some((open, so_far)) if *open == prefix => {
                so_far.extend(words);
                continue;
            }
            _ => {}
        }
        fill_paragraph(&mut out, para.take(), width, tab_width);
        para = Some((prefix, words));
    }
    fill_paragraph(&mut out, para, width, tab_width);
    out.join("\n")
}

/// A line's indent and comment marker, with the space after it, and the text
/// that follows them.
fn split_prefix<'a>(line: &'a str, marker: Option<&str>) -> (String, &'a str) {
    let indent = leading_ws(line);
    let rest = &line[indent.len()..];
    let Some(marker) = marker.filter(|m| rest.starts_with(*m)) else {
        return (indent.to_string(), rest);
    };
    let repeat: usize = rest[marker.len()..]
        .chars()
        .take_while(|c| marker.contains(*c) || *c == '!')
        .map(char::len_utf8)
        .sum();
    let full = &rest[..marker.len() + repeat];
    (format!("{indent}{full} "), &rest[full.len()..])
}

/// Lays a paragraph's words out after its prefix, a new line whenever the
/// next word would pass `width`. A word longer than that gets a line alone.
fn fill_paragraph(
    out: &mut Vec<String>,
    para: Option<(String, Vec<&str>)>,
    width: usize,
    tab_width: usize,
) {
    let Some((prefix, words)) = para else {
        return;
    };
    let prefix_width = display_width(&prefix, tab_width);
    let mut line = prefix.clone();
    let mut line_width = prefix_width;
    let mut empty = true;
    for word in words {
        let word_width = word.chars().count();
        if !empty && line_width + 1 + word_width > width {
            out.push(std::mem::replace(&mut line, prefix.clone()));
            line_width = prefix_width;
            empty = true;
        }
        if !empty {
            line.push(' ');
            line_width += 1;
        }
        line.push_str(word);
        line_width += word_width;
        empty = false;
    }
    out.push(line);
}

/// `:sort` over `lines`, stably. With `re`, each line sorts on what follows
/// its match, or on the match itself with `on_match`, and the lines it
/// doesn't match keep their order ahead of the rest — after them, reversed,
/// with `reverse` — as in Vim.
fn sort_lines(
    lines: Vec<String>,
    opts: &crate::command::SortOpts,
    re: Option<&regex::Regex>,
) -> Vec<String> {
    let mut missed = Vec::new();
    let mut keyed = Vec::new();
    for line in lines {
        let key = match re {
            None => Some(sort_key(&line, opts)),
            Some(re) => super::search::hits(re, &line).first().map(|&(start, end)| {
                let part = if opts.on_match {
                    &line[start..end]
                } else {
                    &line[end..]
                };
                sort_key(part, opts)
            }),
        };
        match key {
            Some(key) => keyed.push((key, line)),
            None => missed.push(line),
        }
    }
    keyed.sort_by(|a, b| {
        if opts.reverse {
            b.0.cmp(&a.0)
        } else {
            a.0.cmp(&b.0)
        }
    });
    if opts.unique {
        keyed.dedup_by(|a, b| a.0 == b.0);
    }
    let sorted = keyed.into_iter().map(|(_, line)| line);
    if opts.reverse {
        sorted.chain(missed.into_iter().rev()).collect()
    } else {
        missed.into_iter().chain(sorted).collect()
    }
}

/// What a line sorts on. A line with no number sorts before every one with.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum SortKey {
    Number(Option<i128>),
    Text(String),
}

fn sort_key(text: &str, opts: &crate::command::SortOpts) -> SortKey {
    if opts.numeric {
        SortKey::Number(first_number(text, 10))
    } else if opts.hex {
        SortKey::Number(first_number(text, 16))
    } else if opts.ignore_case {
        SortKey::Text(text.to_lowercase())
    } else {
        SortKey::Text(text.to_string())
    }
}

/// The first number in `text` in base `radix`: a `-` just before it counts,
/// and a hex one may start `0x`.
fn first_number(text: &str, radix: u32) -> Option<i128> {
    let start = text.find(|c: char| c.is_digit(radix))?;
    let mut digits = &text[start..];
    if radix == 16 {
        let prefixed = digits
            .strip_prefix("0x")
            .or_else(|| digits.strip_prefix("0X"));
        if let Some(rest) =
            prefixed.filter(|rest| rest.starts_with(|c: char| c.is_ascii_hexdigit()))
        {
            digits = rest;
        }
    }
    let end = digits
        .find(|c: char| !c.is_digit(radix))
        .unwrap_or(digits.len());
    let value = i128::from_str_radix(&digits[..end], radix).unwrap_or(i128::MAX);
    let signed = if text[..start].ends_with('-') {
        -value
    } else {
        value
    };
    Some(signed)
}

/// `line` with its runs of blanks laid out again for tabstop `new`, keeping
/// the columns tabstop `old` gave them — only the runs that hold a tab
/// unless `every` — in tabs where they fit when `tabs`, else in spaces.
fn retab(line: &str, old: usize, new: usize, tabs: bool, every: bool) -> String {
    let mut out = String::new();
    let mut col = 0;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != ' ' && c != '\t' {
            out.push(c);
            col += 1;
            continue;
        }
        let from = col;
        let mut run = String::new();
        let mut has_tab = false;
        let mut next = Some(c);
        while let Some(blank) = next {
            run.push(blank);
            if blank == '\t' {
                has_tab = true;
                col += old - col % old;
            } else {
                col += 1;
            }
            next = chars.next_if(|&n| n == ' ' || n == '\t');
        }
        if has_tab || every {
            out.push_str(&blanks(from, col, new, tabs));
        } else {
            out.push_str(&run);
        }
    }
    out
}

/// Blanks from column `from` to `to`: a tab for each tabstop `ts` they
/// reach when `tabs`, and spaces for the rest.
fn blanks(from: usize, to: usize, ts: usize, tabs: bool) -> String {
    let mut out = String::new();
    let mut col = from;
    while tabs && (col / ts + 1) * ts <= to {
        out.push('\t');
        col = (col / ts + 1) * ts;
    }
    out.push_str(&" ".repeat(to - col));
    out
}

fn display_width(s: &str, tab_width: usize) -> usize {
    s.chars()
        .map(|c| if c == '\t' { tab_width } else { 1 })
        .sum()
}

/// `]p`'s indent: the first line's indent becomes `lead`, and every other
/// line keeps its indent relative to the first — one less indented than the
/// first gives up that much of `lead`. Blank lines come out empty. Indents
/// compare as text, so a paste mixing tabs and spaces shifts by the first
/// line's character count.
fn reindent_lines(text: &str, lead: &str) -> String {
    let first = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .map_or("", leading_ws);
    text.split_inclusive('\n')
        .map(|line| {
            let own = leading_ws(line);
            let body = &line[own.len()..];
            if body.trim().is_empty() {
                return body.to_string();
            }
            match own.strip_prefix(first) {
                Some(extra) => format!("{lead}{extra}{body}"),
                None => {
                    let short = first.chars().count().saturating_sub(own.chars().count());
                    let keep = lead.chars().count().saturating_sub(short);
                    let lead: String = lead.chars().take(keep).collect();
                    format!("{lead}{body}")
                }
            }
        })
        .collect()
}

fn leading_ws(s: &str) -> &str {
    &s[..s.len() - s.trim_start_matches([' ', '\t']).len()]
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
            let two_back = if start >= 2 {
                Some(chars[start - 2])
            } else {
                None
            };
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
            NumberBase::Dec => digits.parse().ok(),
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

/// True if `line` is the phantom empty trailing entry ropey adds when
/// the file ends with `\n`. ropey reports `line_count() = N+1` for a
/// file with N user-visible lines that ends in `\n`; the extra index
/// is an empty line that shouldn't participate in line-move ops.
fn is_phantom_trailing_line(buffer: &crate::buffer::Buffer, line: usize) -> bool {
    let total = buffer.line_count();
    if line + 1 != total {
        return false;
    }
    let len = buffer.rope.len_chars();
    len > 0 && buffer.rope.char(len - 1) == '\n' && buffer.line_len(line) == 0
}

/// Swap the block `[start_line..=end_line]` with the single line at
/// `end_line + 1`. Trailing-newline edge case: if the swap target is
/// the file's last line (no trailing `\n`), the target gains one and
/// the block (now last) loses one, preserving the "final newline" or
/// "no final newline" property of the file as a whole.
pub(super) fn shift_block_down_by_one(
    buffer: &mut crate::buffer::Buffer,
    start_line: usize,
    end_line: usize,
) {
    let total = buffer.line_count();
    if end_line + 1 >= total {
        return;
    }
    // Skip the phantom empty trailing line ropey adds when the file
    // ends with `\n` — moving the last real line down into it would
    // produce a stray blank row.
    if is_phantom_trailing_line(buffer, end_line + 1) {
        return;
    }
    let block_first = buffer.rope.line_to_char(start_line);
    let block_end = buffer.rope.line_to_char(end_line + 1);
    let region_end = if end_line + 2 < total {
        buffer.rope.line_to_char(end_line + 2)
    } else {
        buffer.rope.len_chars()
    };

    let block_text: String = buffer.rope.slice(block_first..block_end).to_string();
    let target_text: String = buffer.rope.slice(block_end..region_end).to_string();

    let new_content = if target_text.ends_with('\n') {
        format!("{}{}", target_text, block_text)
    } else {
        let block_stripped = block_text.strip_suffix('\n').unwrap_or(&block_text);
        format!("{}\n{}", target_text, block_stripped)
    };

    buffer.delete_range(block_first, region_end);
    buffer.insert_at_idx(block_first, &new_content);
}

/// Mirror of `shift_block_down_by_one` — swap the block with the line
/// at `start_line - 1`. Handles the symmetric last-line edge case when
/// the moving block contains the file's final line.
pub(super) fn shift_block_up_by_one(
    buffer: &mut crate::buffer::Buffer,
    start_line: usize,
    end_line: usize,
) {
    if start_line == 0 {
        return;
    }
    let total = buffer.line_count();
    let target_first = buffer.rope.line_to_char(start_line - 1);
    let block_first = buffer.rope.line_to_char(start_line);
    let block_end = if end_line + 1 < total {
        buffer.rope.line_to_char(end_line + 1)
    } else {
        buffer.rope.len_chars()
    };

    let target_text: String = buffer.rope.slice(target_first..block_first).to_string();
    let block_text: String = buffer.rope.slice(block_first..block_end).to_string();

    let new_content = if block_text.ends_with('\n') {
        format!("{}{}", block_text, target_text)
    } else {
        let target_stripped = target_text.strip_suffix('\n').unwrap_or(&target_text);
        format!("{}\n{}", block_text, target_stripped)
    };

    buffer.delete_range(target_first, block_end);
    buffer.insert_at_idx(target_first, &new_content);
}

#[cfg(test)]
mod tests {
    use super::{retab, shift_block_down_by_one, shift_block_up_by_one, sort_lines};
    use crate::buffer::Buffer;
    use ropey::Rope;

    fn buf(text: &str) -> Buffer {
        Buffer {
            rope: Rope::from_str(text),
            ..Buffer::default()
        }
    }

    #[test]
    fn shift_single_line_down_in_middle() {
        let mut b = buf("a\nb\nc\n");
        shift_block_down_by_one(&mut b, 0, 0); // move "a" down past "b"
        assert_eq!(b.rope.to_string(), "b\na\nc\n");
    }

    #[test]
    fn shift_single_line_up_in_middle() {
        let mut b = buf("a\nb\nc\n");
        shift_block_up_by_one(&mut b, 1, 1); // move "b" up past "a"
        assert_eq!(b.rope.to_string(), "b\na\nc\n");
    }

    #[test]
    fn shift_down_into_last_line_preserves_no_trailing_newline() {
        let mut b = buf("a\nb"); // no trailing newline — `b` is the last line
        shift_block_down_by_one(&mut b, 0, 0);
        // `a` now becomes the last line and should lose its trailing \n;
        // `b` becomes mid-file and gains one. File-level "no final newline"
        // is preserved.
        assert_eq!(b.rope.to_string(), "b\na");
    }

    #[test]
    fn shift_up_from_last_line_preserves_no_trailing_newline() {
        let mut b = buf("a\nb");
        shift_block_up_by_one(&mut b, 1, 1);
        assert_eq!(b.rope.to_string(), "b\na");
    }

    #[test]
    fn shift_block_down_keeps_block_intact() {
        let mut b = buf("a\nb\nc\nd\ne\n");
        // Move block [b, c] down by one — should swap with d.
        shift_block_down_by_one(&mut b, 1, 2);
        assert_eq!(b.rope.to_string(), "a\nd\nb\nc\ne\n");
    }

    #[test]
    fn shift_block_up_keeps_block_intact() {
        let mut b = buf("a\nb\nc\nd\ne\n");
        // Move block [c, d] up by one — should swap with b.
        shift_block_up_by_one(&mut b, 2, 3);
        assert_eq!(b.rope.to_string(), "a\nc\nd\nb\ne\n");
    }

    #[test]
    fn shift_down_at_last_position_is_noop() {
        let mut b = buf("a\nb\nc\n");
        shift_block_down_by_one(&mut b, 2, 2); // c is line 2; there's no line 3
        assert_eq!(b.rope.to_string(), "a\nb\nc\n");
    }

    #[test]
    fn shift_up_at_first_position_is_noop() {
        let mut b = buf("a\nb\nc\n");
        shift_block_up_by_one(&mut b, 0, 0); // a is line 0; there's no line -1
        assert_eq!(b.rope.to_string(), "a\nb\nc\n");
    }

    #[test]
    fn reindent_lines_moves_the_block_to_the_new_indent() {
        let text = "  a\n    b\n  \n c\n";
        assert_eq!(
            super::reindent_lines(text, "    "),
            "    a\n      b\n\n   c\n"
        );
    }

    #[test]
    fn retab_keeps_the_columns_and_redoes_the_blanks() {
        assert_eq!(retab("\tx", 4, 4, false, false), "    x");
        assert_eq!(retab("  \ty", 4, 4, false, false), "    y");
        assert_eq!(retab("a  b", 4, 4, false, false), "a  b");
        assert_eq!(retab("    x", 4, 2, true, true), "\t\tx");
        assert_eq!(retab("a  b", 4, 2, true, true), "a\t b");
        assert_eq!(retab("\t\tx", 8, 4, true, false), "\t\t\t\tx");
    }

    #[test]
    fn sort_orders_lines_the_ways_vim_does() {
        use crate::command::SortOpts;
        let sort = |lines: &[&str], opts: SortOpts, pattern: Option<&str>| {
            let re = pattern.map(|p| crate::app::search::compile_search(p).expect("pattern"));
            let lines = lines.iter().map(|s| s.to_string()).collect();
            sort_lines(lines, &opts, re.as_ref())
        };
        let plain = SortOpts::default();
        let with = |f: fn(&mut SortOpts)| {
            let mut opts = SortOpts::default();
            f(&mut opts);
            opts
        };
        assert_eq!(
            sort(&["b", "A", "a", "B"], plain.clone(), None),
            ["A", "B", "a", "b"]
        );
        // Stable: lines that sort the same keep their order.
        assert_eq!(
            sort(&["b", "A", "a", "B"], with(|o| o.ignore_case = true), None),
            ["A", "a", "b", "B"]
        );
        // A line with no number goes first; one `-` counts.
        assert_eq!(
            sort(&["x10", "x9", "y", "x-2"], with(|o| o.numeric = true), None),
            ["y", "x-2", "x9", "x10"]
        );
        assert_eq!(
            sort(&["0x1F", "0xa", "ff"], with(|o| o.hex = true), None),
            ["0xa", "0x1F", "ff"]
        );
        assert_eq!(
            sort(&["b", "a", "b", "a"], with(|o| o.unique = true), None),
            ["a", "b"]
        );
        assert_eq!(
            sort(&["a", "c", "b"], with(|o| o.reverse = true), None),
            ["c", "b", "a"]
        );
        // `/pat/` sorts on what follows the match; lines it misses go first.
        assert_eq!(
            sort(&["k=2", "none", "k=1"], plain.clone(), Some("k=")),
            ["none", "k=1", "k=2"]
        );
        // `r` sorts on the match itself.
        assert_eq!(
            sort(
                &["b 2", "a 1", "c 3"],
                with(|o| o.on_match = true),
                Some("\\d")
            ),
            ["a 1", "b 2", "c 3"]
        );
        // Reversed, the lines the pattern misses come last, reversed too.
        assert_eq!(
            sort(
                &["n1", "k=1", "n2", "k=2"],
                with(|o| o.reverse = true),
                Some("k=")
            ),
            ["k=2", "k=1", "n2", "n1"]
        );
    }

    #[test]
    fn reflow_fills_paragraphs_and_keeps_comment_markers() {
        assert_eq!(
            super::reflow("aa bb\ncc dd ee", 8, None, 4),
            "aa bb cc\ndd ee"
        );
        assert_eq!(
            super::reflow(
                "  /// one two\n  /// three\n  ///\n  /// four",
                20,
                Some("//"),
                4
            ),
            "  /// one two three\n  ///\n  /// four"
        );
        assert_eq!(
            super::reflow("// a\n/// b", 79, Some("//"), 4),
            "// a\n/// b"
        );
        assert_eq!(super::reflow("a\n\nb\nc", 79, None, 4), "a\n\nb c");
    }
}
