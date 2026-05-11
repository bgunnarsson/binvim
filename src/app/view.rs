//! Viewport tracking, fold computation, scroll/page motion, the modal-
//! overlay query, syntax-highlight cache refresh, and the hover popup
//! scroll handler. All viewport math has to agree across these methods,
//! so they live together.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::buffer::Buffer;
use crate::lang;
use crate::mode::Mode;
use crate::parser::{FoldOp, PageScrollKind, ViewportAdjust};

use super::state::{FoldRange, HOVER_MAX_HEIGHT};

impl super::App {
    /// Returns `true` if the key was consumed to scroll the hover popup. Otherwise
    /// the caller should dismiss the popup and let the key fall through.
    pub(super) fn try_scroll_hover(&mut self, key: &KeyEvent) -> bool {
        let Some(h) = self.hover.as_mut() else { return false };
        let visible = HOVER_MAX_HEIGHT;
        match key.code {
            KeyCode::Down => { h.scroll_by(1, visible); true }
            KeyCode::Up => { h.scroll_by(-1, visible); true }
            KeyCode::PageDown => { h.scroll_by(visible as i64, visible); true }
            KeyCode::PageUp => { h.scroll_by(-(visible as i64), visible); true }
            KeyCode::Char('j') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                h.scroll_by(1, visible);
                true
            }
            KeyCode::Char('k') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                h.scroll_by(-1, visible);
                true
            }
            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => match c {
                'd' | 'D' => { h.scroll_by((visible / 2) as i64, visible); true }
                'u' | 'U' => { h.scroll_by(-((visible / 2) as i64), visible); true }
                'n' | 'N' => { h.scroll_by(1, visible); true }
                'p' | 'P' => { h.scroll_by(-1, visible); true }
                _ => false,
            },
            _ => false,
        }
    }

    pub(super) fn page_scroll(&mut self, kind: PageScrollKind) {
        let rows = self.buffer_rows();
        if rows == 0 {
            return;
        }
        let last = self.buffer.line_count().saturating_sub(1);
        match kind {
            PageScrollKind::HalfDown | PageScrollKind::HalfUp => {
                let amount = (rows / 2).max(1);
                let down = matches!(kind, PageScrollKind::HalfDown);
                self.shift_view_and_cursor(amount, down, last);
            }
            PageScrollKind::FullDown | PageScrollKind::FullUp => {
                let amount = rows.saturating_sub(2).max(1);
                let down = matches!(kind, PageScrollKind::FullDown);
                self.shift_view_and_cursor(amount, down, last);
            }
            PageScrollKind::LineDown => {
                self.view_top = (self.view_top + 1).min(last);
                if self.cursor.line < self.view_top {
                    self.cursor.line = self.view_top;
                }
                self.snap_cursor_col_to_want();
            }
            PageScrollKind::LineUp => {
                self.view_top = self.view_top.saturating_sub(1);
                if self.cursor.line > self.view_top + rows.saturating_sub(1) {
                    self.cursor.line = self.view_top + rows.saturating_sub(1);
                }
                self.snap_cursor_col_to_want();
            }
        }
    }

    fn shift_view_and_cursor(&mut self, amount: usize, down: bool, last: usize) {
        if down {
            self.view_top = (self.view_top + amount).min(last);
            self.cursor.line = (self.cursor.line + amount).min(last);
        } else {
            self.view_top = self.view_top.saturating_sub(amount);
            self.cursor.line = self.cursor.line.saturating_sub(amount);
        }
        self.snap_cursor_col_to_want();
    }

    fn snap_cursor_col_to_want(&mut self) {
        let len = self.buffer.line_len(self.cursor.line);
        let max = if len == 0 { 0 } else { len - 1 };
        self.cursor.col = self.cursor.want_col.min(max);
    }

    pub(super) fn adjust_viewport_to(&mut self, kind: ViewportAdjust) {
        let rows = self.buffer_rows();
        let cur = self.cursor.line;
        let buffer_cols = (self.width as usize).saturating_sub(self.gutter_width());
        match kind {
            ViewportAdjust::Top if rows > 0 => self.view_top = cur,
            ViewportAdjust::Center if rows > 0 => self.view_top = cur.saturating_sub(rows / 2),
            ViewportAdjust::Bottom if rows > 0 => {
                self.view_top = cur.saturating_sub(rows.saturating_sub(1))
            }
            ViewportAdjust::Left => self.scroll_horizontal(-1),
            ViewportAdjust::Right => self.scroll_horizontal(1),
            ViewportAdjust::HalfLeft => {
                let step = (buffer_cols / 2).max(1) as i64;
                self.scroll_horizontal(-step);
            }
            ViewportAdjust::HalfRight => {
                let step = (buffer_cols / 2).max(1) as i64;
                self.scroll_horizontal(step);
            }
            _ => {}
        }
    }

    /// Nudge the horizontal viewport without moving the cursor. The cursor's
    /// visual column may end up off-screen; that's intentional — `zh`/`zl`
    /// in Vim does the same. The next motion will pull the viewport back
    /// via `adjust_viewport`.
    pub fn scroll_horizontal(&mut self, delta: i64) {
        let new_left = (self.view_left as i64 + delta).max(0) as usize;
        self.view_left = new_left;
    }

    pub(super) fn cursor_to_idx(&mut self, idx: usize) {
        let total = self.buffer.total_chars();
        let idx = idx.min(total);
        let line = self.buffer.rope.char_to_line(idx);
        let line_start = self.buffer.rope.line_to_char(line);
        let col = idx - line_start;
        self.cursor.line = line;
        self.cursor.col = col;
        self.cursor.want_col = col;
    }

    pub(super) fn clamp_cursor_normal(&mut self) {
        let last = self.buffer.line_count().saturating_sub(1);
        if self.cursor.line > last {
            self.cursor.line = last;
        }
        let len = self.buffer.line_len(self.cursor.line);
        let max = if len == 0 { 0 } else { len - 1 };
        if self.cursor.col > max {
            self.cursor.col = max;
        }
    }

    /// Mouse-wheel scroll: shift the viewport by `delta` lines and drag the
    /// cursor along just enough to keep it inside the scroll-off zone, so the
    /// next `adjust_viewport` doesn't snap the view back. Positive = down,
    /// negative = up.
    pub(super) fn scroll_view(&mut self, delta: i64) {
        let buffer_rows = self.buffer_rows();
        if buffer_rows == 0 {
            return;
        }
        let line_count = self.buffer.line_count();
        if line_count == 0 {
            return;
        }
        let last = line_count.saturating_sub(1);
        let max_top = line_count.saturating_sub(buffer_rows.min(line_count));
        let scrolloff = 3.min(buffer_rows / 2);

        // Move the viewport.
        let new_top = (self.view_top as i64 + delta).max(0) as usize;
        self.view_top = new_top.min(max_top);

        // Drag the cursor by the same amount, then clamp it into the scroll-off
        // zone of the (possibly clamped) viewport.
        let new_cursor_line = (self.cursor.line as i64 + delta).max(0) as usize;
        let mut line = new_cursor_line.min(last);
        let top_min = self.view_top + scrolloff;
        let bot_max = self
            .view_top
            .saturating_add(buffer_rows.saturating_sub(scrolloff + 1));
        line = line.max(top_min).min(bot_max).min(last);
        self.cursor.line = line;
        self.clamp_cursor_normal();
    }

    pub(super) fn adjust_viewport(&mut self) {
        let buffer_rows = self.buffer_rows();
        if buffer_rows > 0 {
            let scrolloff = 3.min(buffer_rows / 2);
            let cur = self.cursor.line;
            if cur < self.view_top + scrolloff {
                self.view_top = cur.saturating_sub(scrolloff);
            }
            if cur >= self.view_top + buffer_rows.saturating_sub(scrolloff) {
                let want = cur + scrolloff + 1;
                self.view_top = want.saturating_sub(buffer_rows);
            }
        }

        // Horizontal — track the cursor's visual column instead of the char
        // index so tabs (TAB_WIDTH columns) don't make the viewport jump.
        let buffer_cols = (self.width as usize).saturating_sub(self.gutter_width());
        if buffer_cols == 0 {
            return;
        }
        let scrolloff_h = 5.min(buffer_cols / 4);
        let cur_vis = self.cursor_visual_col();
        if cur_vis < self.view_left + scrolloff_h {
            self.view_left = cur_vis.saturating_sub(scrolloff_h);
        }
        let right_edge = self.view_left + buffer_cols.saturating_sub(scrolloff_h);
        if cur_vis >= right_edge {
            let want = cur_vis + scrolloff_h + 1;
            self.view_left = want.saturating_sub(buffer_cols);
        }
    }

    /// Visual column of the cursor on its own line, treating tabs as
    /// `TAB_WIDTH` columns. Used by horizontal viewport tracking and cursor
    /// placement.
    pub fn cursor_visual_col(&self) -> usize {
        if self.cursor.line >= self.buffer.line_count() {
            return 0;
        }
        let line = self.buffer.rope.line(self.cursor.line);
        let mut v = 0usize;
        for (i, c) in line.chars().enumerate() {
            if i >= self.cursor.col {
                break;
            }
            if c == '\t' {
                v += crate::render::TAB_WIDTH;
            } else {
                v += 1;
            }
        }
        v
    }

    pub fn buffer_rows(&self) -> usize {
        // Reserve the status line at the bottom, (when applicable) one row at
        // the top for the tab bar, and the debug pane rows at the bottom when
        // a debug session is up or the pane is pinned open.
        (self.height as usize)
            .saturating_sub(1)
            .saturating_sub(self.buffer_top())
            .saturating_sub(self.debug_pane_rows())
    }

    /// Number of rows the bottom debug pane occupies. Zero when the pane is
    /// closed or when the terminal is too short to hold it without squashing
    /// the editor below a usable threshold — opening the pane on a tiny
    /// terminal silently becomes a no-op rather than corrupting the layout.
    pub fn debug_pane_rows(&self) -> usize {
        if !self.debug_pane_open {
            return 0;
        }
        let h = self.height as usize;
        let chrome = self.buffer_top() + 1;
        let avail = h.saturating_sub(chrome);
        if avail < 10 {
            return 0;
        }
        let target = (h / 3).clamp(8, 20);
        target.min(avail.saturating_sub(6))
    }

    /// First terminal row occupied by the debug pane. Sits directly above the
    /// status line. Caller must check `debug_pane_rows() > 0` first — when the
    /// pane is closed this returns the status-line row, which is not a valid
    /// drawing target.
    pub fn debug_pane_top(&self) -> usize {
        (self.height as usize)
            .saturating_sub(1)
            .saturating_sub(self.debug_pane_rows())
    }

    /// True when the tab bar should be painted. Shown whenever any
    /// real (path-backed) buffer is open, or whenever there's more
    /// than one buffer — so the bar reflects what the user actually
    /// has loaded. A fresh launch with just the `[No Name]` seed
    /// keeps the bar hidden.
    pub fn show_tabs(&self) -> bool {
        if self.buffers.len() > 1 {
            return true;
        }
        self.buffer.path.is_some()
    }

    /// Y of the topmost buffer row. Equal to the tab-bar height —
    /// 1 when tabs are showing, 0 otherwise.
    pub fn buffer_top(&self) -> usize {
        if self.show_tabs() { 1 } else { 0 }
    }

    /// Any overlay (command line, search prompt, picker, hover, completion) is active —
    /// the buffer should render dimmed so the overlay is the focal point.
    pub fn has_modal_overlay(&self) -> bool {
        // Completion is intentionally absent — it's an inline assist that
        // shouldn't dim the buffer or capture mouse input while you type.
        matches!(
            self.mode,
            Mode::Command | Mode::Search { .. } | Mode::Picker | Mode::Prompt(_)
        )
            || self.hover.is_some()
            || self.picker.is_some()
            || self.whichkey.is_some()
    }

    pub fn gutter_width(&self) -> usize {
        let n = self.buffer.line_count();
        let digits = format!("{n}").len();
        // 1 git-stripe column + 1 sign column + digits + 1 trailing space.
        digits + 3
    }

    /// Hunk kind covering `line` (0-indexed), if any. Linear scan over the
    /// active buffer's `git_hunks` — typical hunk counts are well under
    /// 100, and we call this per-visible-row at render time only.
    pub fn git_hunk_kind_at(&self, line: usize) -> Option<crate::git::GitHunkKind> {
        self.git_hunks
            .iter()
            .find(|h| line >= h.start_line && line <= h.end_line)
            .map(|h| h.kind)
    }

    /// Recompute fold ranges if the buffer's version moved past the
    /// cached snapshot. Cheap on small buffers (single linear pass).
    pub(super) fn ensure_folds(&mut self) {
        if self.folds_version == self.buffer.version {
            return;
        }
        self.folds = compute_indent_folds(&self.buffer);
        self.folds_version = self.buffer.version;
        // Drop closed-fold entries that are no longer real fold starts.
        let starts: std::collections::HashSet<usize> =
            self.folds.iter().map(|f| f.start_line).collect();
        self.closed_folds.retain(|s| starts.contains(s));
    }

    pub(super) fn apply_fold_op(&mut self, op: FoldOp) {
        self.ensure_folds();
        match op {
            FoldOp::OpenAll => {
                self.closed_folds.clear();
            }
            FoldOp::CloseAll => {
                // Close every fold whose range covers >1 line so the user
                // sees a meaningful collapse rather than a million `…`s.
                self.closed_folds = self
                    .folds
                    .iter()
                    .filter(|f| f.end_line > f.start_line)
                    .map(|f| f.start_line)
                    .collect();
            }
            FoldOp::Open => {
                if let Some(f) = self.innermost_closed_fold_at(self.cursor.line) {
                    self.closed_folds.remove(&f.start_line);
                }
            }
            FoldOp::Close => {
                if let Some(f) = self.innermost_open_fold_at(self.cursor.line) {
                    self.closed_folds.insert(f.start_line);
                    // Snap cursor to the fold's start so it's never on a
                    // hidden row.
                    if self.cursor.line > f.start_line && self.cursor.line <= f.end_line {
                        self.cursor.line = f.start_line;
                        self.clamp_cursor_normal();
                    }
                }
            }
            FoldOp::Toggle => {
                if let Some(f) = self.innermost_closed_fold_at(self.cursor.line) {
                    self.closed_folds.remove(&f.start_line);
                } else if let Some(f) = self.innermost_open_fold_at(self.cursor.line) {
                    self.closed_folds.insert(f.start_line);
                    if self.cursor.line > f.start_line && self.cursor.line <= f.end_line {
                        self.cursor.line = f.start_line;
                        self.clamp_cursor_normal();
                    }
                }
            }
        }
    }

    /// True when `line` is hidden inside a closed fold (i.e. not the start
    /// of one — the start renders as a placeholder).
    pub fn line_is_folded(&self, line: usize) -> bool {
        for f in &self.folds {
            if self.closed_folds.contains(&f.start_line)
                && line > f.start_line
                && line <= f.end_line
            {
                return true;
            }
        }
        false
    }

    /// True if `line` is the start of a closed fold (rendered as the
    /// `… N lines` placeholder).
    pub fn line_is_fold_start(&self, line: usize) -> bool {
        self.closed_folds.contains(&line)
            && self.folds.iter().any(|f| f.start_line == line)
    }

    /// Return the innermost fold (smallest range) containing `line`.
    #[allow(dead_code)]
    fn innermost_fold_at(&self, line: usize) -> Option<&FoldRange> {
        self.folds
            .iter()
            .filter(|f| f.start_line <= line && line <= f.end_line)
            .min_by_key(|f| f.end_line - f.start_line)
    }

    fn innermost_closed_fold_at(&self, line: usize) -> Option<FoldRange> {
        self.folds
            .iter()
            .filter(|f| f.start_line <= line && line <= f.end_line)
            .filter(|f| self.closed_folds.contains(&f.start_line))
            .min_by_key(|f| f.end_line - f.start_line)
            .cloned()
    }

    fn innermost_open_fold_at(&self, line: usize) -> Option<FoldRange> {
        self.folds
            .iter()
            .filter(|f| f.start_line <= line && line <= f.end_line)
            .filter(|f| !self.closed_folds.contains(&f.start_line))
            .min_by_key(|f| f.end_line - f.start_line)
            .cloned()
    }

    /// Number of lines `line` represents on screen — 1 normally, the full
    /// fold span when this is the start of a closed fold.
    pub fn folded_line_span(&self, line: usize) -> usize {
        if let Some(f) = self
            .folds
            .iter()
            .find(|f| f.start_line == line && self.closed_folds.contains(&f.start_line))
        {
            f.end_line - f.start_line + 1
        } else {
            1
        }
    }

    pub(super) fn ensure_highlights(&mut self) {
        let lang = self
            .buffer
            .path
            .as_deref()
            .and_then(lang::Lang::detect);
        let need_refresh = match (&self.highlight_cache, lang) {
            (None, Some(_)) => true,
            (Some(c), Some(l)) => c.lang != l || c.buffer_version != self.buffer.version,
            (Some(_), None) => true,
            (None, None) => false,
        };
        if !need_refresh {
            return;
        }
        self.highlight_cache = match lang {
            Some(l) => lang::compute_highlights(l, &self.buffer, &self.config),
            None => None,
        };
    }
}

/// Indent-based fold computation. Builds a fold range starting at every
/// line whose indent level is *strictly less than* the next non-blank
/// line's. Blank lines belong to whichever fold they fall inside (they
/// don't break a range).
pub fn compute_indent_folds(buf: &Buffer) -> Vec<FoldRange> {
    let count = buf.line_count();
    if count == 0 {
        return Vec::new();
    }
    let levels: Vec<i32> = (0..count)
        .map(|i| {
            let line = buf.rope.line(i);
            let mut n = 0i32;
            for c in line.chars() {
                match c {
                    ' ' => n += 1,
                    '\t' => n += crate::render::TAB_WIDTH as i32,
                    '\n' | '\r' => return -1,
                    _ => return n,
                }
            }
            -1
        })
        .collect();
    let mut folds = Vec::new();
    for i in 0..count {
        if levels[i] < 0 {
            continue;
        }
        // Find next non-blank line.
        let mut next = i + 1;
        while next < count && levels[next] < 0 {
            next += 1;
        }
        if next >= count {
            continue;
        }
        if levels[next] <= levels[i] {
            continue;
        }
        // Walk forward until indent drops back to <= levels[i].
        let mut end = i + 1;
        while end < count {
            if levels[end] >= 0 && levels[end] <= levels[i] {
                break;
            }
            end += 1;
        }
        // `end` now points one past the last folded line.
        let last = end.saturating_sub(1);
        if last > i {
            folds.push(FoldRange {
                start_line: i,
                end_line: last,
            });
        }
    }
    folds
}
