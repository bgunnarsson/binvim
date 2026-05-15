//! Save flow: format on save, .editorconfig on-save transforms (final
//! newline, trailing whitespace), undo persistence, plus the in-process
//! `:format` command and the git-branch refresh that runs after a buffer
//! switch.

use anyhow::Result;
use std::path::PathBuf;

use crate::editorconfig::EditorConfig;

impl super::App {
    /// Pipe the active buffer through the configured formatter for its
    /// extension (currently biome for ts/tsx/js/jsx/json) and replace the
    /// contents in place. Records an undo entry first; clamps the cursor into
    /// the (possibly-shorter) result.
    pub(super) fn format_active(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "format: buffer has no file path".into();
            return;
        };
        let source = self.buffer.rope.to_string();
        match crate::format::format_buffer(&path, &source) {
            Ok(formatted) => {
                if formatted == source {
                    self.status_msg = "already formatted".into();
                    return;
                }
                self.history.record(&self.buffer.rope, self.window.cursor);
                let total = self.buffer.total_chars();
                self.buffer.delete_range(0, total);
                self.buffer.insert_at_idx(0, &formatted);
                let last_line = self.buffer.line_count().saturating_sub(1);
                if self.window.cursor.line > last_line {
                    self.window.cursor.line = last_line;
                }
                self.clamp_cursor_normal();
                self.status_msg = "formatted".into();
            }
            Err(msg) => {
                self.status_msg = format!("format: {msg}");
            }
        }
    }

    /// Run the configured formatter (if any), apply .editorconfig on-save
    /// transforms, then write to disk. Records a `format_status` message that
    /// the caller can surface — this is the only signal the user gets that
    /// the formatter ran or didn't.
    pub(super) fn save_active(&mut self) -> Result<Option<String>> {
        let mut format_note: Option<String> = None;
        if let Some(path) = self.buffer.path.clone() {
            let source = self.buffer.rope.to_string();
            match crate::format::format_buffer(&path, &source) {
                Ok(formatted) if formatted != source => {
                    self.history.record(&self.buffer.rope, self.window.cursor);
                    let total = self.buffer.total_chars();
                    self.buffer.delete_range(0, total);
                    self.buffer.insert_at_idx(0, &formatted);
                    let last_line = self.buffer.line_count().saturating_sub(1);
                    if self.window.cursor.line > last_line {
                        self.window.cursor.line = last_line;
                    }
                    self.clamp_cursor_normal();
                    format_note = Some("formatted".into());
                }
                Ok(_) => {} // already formatted — quiet
                Err(msg) if msg.starts_with("no formatter") => {} // expected for unsupported extensions
                Err(msg) => format_note = Some(format!("fmt: {msg}")),
            }
        }
        if self.editorconfig.trim_trailing_whitespace {
            self.trim_trailing_whitespace();
        }
        if self.editorconfig.insert_final_newline {
            self.ensure_final_newline();
        }
        self.buffer.save()?;
        // Refresh git stripe after a successful write — the index hasn't
        // moved but the working tree just did, so hunks may have grown,
        // shrunk, or disappeared entirely.
        self.refresh_git_hunks();
        // Persist undo so the next session can keep walking history.
        if let Some(path) = self.buffer.path.as_deref() {
            if let Some(cache) = crate::undo::cache_path_for(path) {
                let hash = crate::undo::hash_text(&self.buffer.rope.to_string());
                let _ = self.history.save_to_path(&cache, hash);
            }
        }
        Ok(format_note)
    }

    fn trim_trailing_whitespace(&mut self) {
        let line_count = self.buffer.line_count();
        // Iterate top-down — we only ever shrink lines, so indices stay valid.
        for line in 0..line_count {
            let line_len = self.buffer.line_len(line);
            if line_len == 0 {
                continue;
            }
            let mut last_non_ws = line_len;
            while last_non_ws > 0 {
                let c = self.buffer.char_at(line, last_non_ws - 1);
                match c {
                    Some(ch) if ch.is_whitespace() => last_non_ws -= 1,
                    _ => break,
                }
            }
            if last_non_ws < line_len {
                let line_start = self.buffer.line_start_idx(line);
                let trim_start = line_start + last_non_ws;
                let trim_end = line_start + line_len;
                self.buffer.delete_range(trim_start, trim_end);
            }
        }
        self.clamp_cursor_normal();
    }

    fn ensure_final_newline(&mut self) {
        let total = self.buffer.total_chars();
        if total == 0 {
            return;
        }
        let last_char = self.buffer.rope.get_char(total - 1);
        if last_char != Some('\n') {
            self.buffer.insert_at_idx(total, "\n");
        }
    }

    pub(super) fn refresh_editorconfig(&mut self) {
        self.editorconfig = match self.buffer.path.as_ref() {
            Some(p) => EditorConfig::detect(p),
            None => EditorConfig::default(),
        };
    }

    pub(super) fn refresh_git_branch(&mut self) {
        let start = self
            .buffer
            .path
            .as_ref()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        self.git_branch = detect_git_branch(&start);
    }

    /// Recompute the active buffer's working-tree diff against the index
    /// and refresh `self.git_hunks`. Cheap on small files (single
    /// `git diff -U0` invocation, parsed locally). No-op when the buffer
    /// has no on-disk path or isn't inside a git repo.
    pub(super) fn refresh_git_hunks(&mut self) {
        self.git_hunks = match self.buffer.path.as_ref() {
            Some(p) => crate::git::diff_against_worktree(p).unwrap_or_default(),
            None => Vec::new(),
        };
    }
}

/// Walk up from `start` looking for a `.git` dir; return the current branch (or short SHA in
/// detached-HEAD mode). Returns `None` outside a git repo.
pub fn detect_git_branch(start: &std::path::Path) -> Option<String> {
    let mut dir = start.canonicalize().ok()?;
    loop {
        let git_dir = dir.join(".git");
        if git_dir.exists() {
            // .git can be a directory or a file (worktrees / submodules).
            let head_path = if git_dir.is_dir() {
                git_dir.join("HEAD")
            } else {
                // .git file: contains "gitdir: <path>" — not handling worktrees in v1.
                return None;
            };
            let text = std::fs::read_to_string(&head_path).ok()?;
            let trimmed = text.trim();
            if let Some(rest) = trimmed.strip_prefix("ref: refs/heads/") {
                return Some(rest.to_string());
            }
            if trimmed.len() >= 7 {
                return Some(format!("{}…", &trimmed[..7]));
            }
            return None;
        }
        let parent = dir.parent()?.to_path_buf();
        if parent == dir {
            return None;
        }
        dir = parent;
    }
}
