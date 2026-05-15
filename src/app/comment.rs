//! `<leader>/` toggle: comment / uncomment the current line (Normal)
//! or every line in the visual selection (Visual). All-or-nothing
//! convention — if every non-blank line in the range already starts
//! with the language's comment prefix, the operation strips them;
//! otherwise it prepends the prefix at the minimum-indent column so
//! a uniformly-indented block stays aligned.
//!
//! Languages without a line-comment marker (HTML, Markdown, CSS, XML,
//! Razor) fall back to wrapping the range in their block-comment pair
//! — single `<!--` before the first line and `-->` after the last,
//! preserving content unchanged in between.

use crate::lang::Lang;
use crate::mode::{Mode, VisualKind};

impl super::App {
    /// Compute the line range the toggle should operate on. Normal
    /// mode → just the cursor line; Visual char / line → every line
    /// touched by the selection (anchor + cursor inclusive); Visual
    /// block → the row span of the rectangular selection.
    fn comment_range(&self) -> (usize, usize) {
        match self.mode {
            Mode::Visual(_) => {
                let anchor = match self.window.visual_anchor {
                    Some(a) => a,
                    None => return (self.window.cursor.line, self.window.cursor.line),
                };
                let (a, b) = (anchor.line, self.window.cursor.line);
                (a.min(b), a.max(b))
            }
            _ => (self.window.cursor.line, self.window.cursor.line),
        }
    }

    pub(super) fn toggle_comment_range(&mut self) {
        let lang = match self
            .buffer
            .path
            .as_deref()
            .and_then(Lang::detect)
        {
            Some(l) => l,
            None => {
                self.status_msg = "comment toggle: unknown language".into();
                return;
            }
        };
        let (start, end) = self.comment_range();
        // Record an undo step before mutating so a single `u` undoes
        // the whole toggle, regardless of how many lines moved.
        self.history.record(&self.buffer.rope, self.window.cursor);
        let was_visual = matches!(self.mode, Mode::Visual(_));
        if let Some(prefix) = lang.line_comment_prefix() {
            self.toggle_line_comments(start, end, prefix);
        } else if let Some((open, close)) = lang.block_comment_pair() {
            self.toggle_block_comment(start, end, open, close);
        } else {
            self.status_msg = "comment toggle: language has no comment marker".into();
            return;
        }
        if was_visual {
            // Drop back to Normal — matches what VS Code / Neovim's
            // gc operator do after the toggle. The user can re-enter
            // Visual if they want to keep operating on the range.
            self.mode = Mode::Normal;
            self.window.visual_anchor = None;
        }
        self.clamp_cursor_normal();
    }

    /// All-or-nothing line-comment toggle. Inspect every non-blank
    /// line in `[start..=end]`: if every one already starts with
    /// `prefix` (after leading whitespace), strip the prefix +
    /// optional single trailing space. Otherwise prepend
    /// `"{prefix} "` at the column equal to the minimum indent of
    /// the non-blank lines.
    fn toggle_line_comments(&mut self, start: usize, end: usize, prefix: &str) {
        // First pass — decide direction + find min indent.
        let mut all_commented = true;
        let mut any_non_blank = false;
        let mut min_indent = usize::MAX;
        for line in start..=end {
            let text = self.line_text(line);
            let trimmed = text.trim_start();
            if trimmed.is_empty() {
                continue;
            }
            any_non_blank = true;
            let indent = text.chars().count() - trimmed.chars().count();
            if indent < min_indent {
                min_indent = indent;
            }
            if !trimmed.starts_with(prefix) {
                all_commented = false;
            }
        }
        if !any_non_blank {
            // Blank-only selection — nothing useful to toggle.
            return;
        }
        if all_commented {
            // Uncomment: strip `prefix` + one optional space after it
            // from each non-blank line. Walk highest-line first so
            // earlier deletions don't shift line numbers we still
            // need to address.
            for line in (start..=end).rev() {
                let text = self.line_text(line);
                let trimmed = text.trim_start();
                if !trimmed.starts_with(prefix) {
                    continue;
                }
                let indent_chars = text.chars().count() - trimmed.chars().count();
                let prefix_chars = prefix.chars().count();
                // Drop the prefix + one trailing space (if present).
                let after_prefix: String = trimmed.chars().skip(prefix_chars).collect();
                let drop_space = after_prefix.starts_with(' ');
                let removal_chars = prefix_chars + usize::from(drop_space);
                let start_idx = self.buffer.line_start_idx(line) + indent_chars;
                self.buffer
                    .delete_range(start_idx, start_idx + removal_chars);
            }
        } else {
            // Comment: prepend `"{prefix} "` at `min_indent`. Walk
            // highest-line first for the same shift-safety reason.
            let insertion = format!("{prefix} ");
            for line in (start..=end).rev() {
                let text = self.line_text(line);
                if text.trim_start().is_empty() {
                    continue;
                }
                let start_idx = self.buffer.line_start_idx(line) + min_indent;
                self.buffer.insert_at_idx(start_idx, &insertion);
            }
        }
    }

    /// Block-comment toggle for languages with no line comment
    /// (HTML, Markdown, CSS, XML, Razor). If the first non-blank
    /// content of the range already opens with `open` and the last
    /// closes with `close`, strip them; otherwise wrap the range
    /// with a leading `open ` line at `start` and a trailing `close`
    /// line at `end + 1`.
    fn toggle_block_comment(&mut self, start: usize, end: usize, open: &str, close: &str) {
        let first = self.line_text(start);
        let last = self.line_text(end);
        let trimmed_first = first.trim_start();
        let trimmed_last = last.trim_end();
        // Detect already-wrapped: opener on the first line, closer on the last.
        let already_open = trimmed_first.starts_with(open);
        let already_close = trimmed_last.ends_with(close);
        if already_open && already_close && start != end {
            // Strip closer first (so the line indices we computed
            // for the opener don't shift).
            let last_text = self.line_text(end);
            let close_chars = close.chars().count();
            let total_chars = last_text.chars().count();
            // Position of the closer's start, character-wise.
            let close_start_chars = total_chars - close_chars
                - last_text
                    .chars()
                    .rev()
                    .take_while(|c| c.is_whitespace())
                    .count();
            let close_start_idx = self.buffer.line_start_idx(end) + close_start_chars;
            // Strip closer (+ a single preceding space if present).
            let drop_space = last_text
                .chars()
                .nth(close_start_chars.saturating_sub(1))
                .map(|c| c == ' ')
                .unwrap_or(false);
            let remove_from = close_start_idx - usize::from(drop_space);
            let remove_to = close_start_idx + close_chars;
            self.buffer.delete_range(remove_from, remove_to);
            // Strip opener: leading whitespace stays, drop the marker
            // + a single trailing space if present.
            let first_text = self.line_text(start);
            let leading = first_text.chars().count() - first_text.trim_start().chars().count();
            let open_chars = open.chars().count();
            let open_start = self.buffer.line_start_idx(start) + leading;
            let after_open: String = first_text
                .chars()
                .skip(leading + open_chars)
                .collect();
            let trim_space = after_open.starts_with(' ');
            let total_remove = open_chars + usize::from(trim_space);
            self.buffer
                .delete_range(open_start, open_start + total_remove);
        } else {
            // Wrap: append `{open} ` at start of `start` and ` {close}`
            // at end of `end`. Same shift-safety: end first.
            let end_text = self.line_text(end);
            let end_idx = self.buffer.line_start_idx(end) + end_text.chars().count();
            self.buffer.insert_at_idx(end_idx, &format!(" {close}"));
            let first_text = self.line_text(start);
            let leading = first_text.chars().count() - first_text.trim_start().chars().count();
            let start_idx = self.buffer.line_start_idx(start) + leading;
            self.buffer.insert_at_idx(start_idx, &format!("{open} "));
        }
    }

    /// One line's content as a `String`. Strips the trailing newline
    /// (and `\r` for paranoia) so callers can measure char counts
    /// against visible content alone.
    fn line_text(&self, line: usize) -> String {
        let mut s: String = self.buffer.rope.line(line).chars().collect();
        if s.ends_with('\n') {
            s.pop();
        }
        if s.ends_with('\r') {
            s.pop();
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use std::path::PathBuf;

    fn app_with(path: &str, content: &str) -> crate::app::App {
        // Build a minimal App by reusing the harness from other test
        // modules: open binvim with no path, then swap the buffer.
        let mut app = crate::app::App::new(None).expect("App::new");
        app.buffer = Buffer {
            rope: ropey::Rope::from_str(content),
            path: Some(PathBuf::from(path)),
            dirty: false,
            version: 0,
            disk_mtime: None,
            display_name: None,
        };
        app
    }

    fn buffer_text(app: &crate::app::App) -> String {
        app.buffer.rope.to_string()
    }

    #[test]
    fn comment_single_line_rust() {
        let mut app = app_with("a.rs", "let x = 1;\n");
        app.window.cursor.line = 0;
        app.toggle_comment_range();
        assert_eq!(buffer_text(&app), "// let x = 1;\n");
    }

    #[test]
    fn uncomment_single_line_rust() {
        let mut app = app_with("a.rs", "// let x = 1;\n");
        app.window.cursor.line = 0;
        app.toggle_comment_range();
        assert_eq!(buffer_text(&app), "let x = 1;\n");
    }

    #[test]
    fn comment_preserves_min_indent() {
        let mut app = app_with("a.rs", "    let x = 1;\n        let y = 2;\n");
        app.window.cursor.line = 0;
        app.window.visual_anchor = Some(app.window.cursor);
        app.window.cursor.line = 1;
        app.mode = crate::mode::Mode::Visual(crate::mode::VisualKind::Line);
        app.toggle_comment_range();
        assert_eq!(
            buffer_text(&app),
            "    // let x = 1;\n    //     let y = 2;\n"
        );
    }

    #[test]
    fn toggle_skips_blank_lines() {
        let mut app = app_with("a.py", "a\n\nb\n");
        app.window.cursor.line = 0;
        app.window.visual_anchor = Some(app.window.cursor);
        app.window.cursor.line = 2;
        app.mode = crate::mode::Mode::Visual(crate::mode::VisualKind::Line);
        app.toggle_comment_range();
        assert_eq!(buffer_text(&app), "# a\n\n# b\n");
    }

    #[test]
    fn block_comment_wraps_html() {
        let mut app = app_with("a.html", "<p>hi</p>\n");
        app.window.cursor.line = 0;
        app.toggle_comment_range();
        assert_eq!(buffer_text(&app), "<!-- <p>hi</p> -->\n");
    }

    #[test]
    fn unknown_language_status_msg() {
        let mut app = app_with("a.xyz", "foo\n");
        app.window.cursor.line = 0;
        app.toggle_comment_range();
        assert!(app.status_msg.contains("unknown language"));
        assert_eq!(buffer_text(&app), "foo\n");
    }

    #[test]
    #[allow(unused_imports)]
    fn unused_kind_import_compiles() {
        // Touches the import so the dead-code lint stays quiet across
        // refactors that strip the Mode::Visual reach into VisualKind.
        let _ = VisualKind::Line;
    }
}
