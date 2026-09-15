//! When recovery files (`recover.rs`) are written, applied and removed. A
//! dirty buffer's text is dumped every `RECOVERY_INTERVAL`; the file goes away
//! once the buffer is written, reverted, closed or quit on purpose, and stays
//! when the editor dies any other way — which is the point.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::recover::{RecoveryFile, load_from, now_secs, recovered_text, recovery_path, write_to};

/// How often dirty buffers are dumped, and so the most typing a `kill -9`
/// can cost. Vim's `updatetime`.
const RECOVERY_INTERVAL: Duration = Duration::from_secs(4);

impl super::App {
    pub(super) fn recovery_due_at(&self) -> Instant {
        self.recovery_checked_at + RECOVERY_INTERVAL
    }

    pub(super) fn recover_if_due(&mut self) {
        if Instant::now() < self.recovery_due_at() {
            return;
        }
        self.recovery_checked_at = Instant::now();
        self.write_recovery_now();
    }

    /// Bring every buffer's recovery file up to date: write the dirty ones
    /// whose text moved since their last dump, and remove the ones whose
    /// buffer has gone clean (undone back to what's on disk). Called on the
    /// interval, and straight away when the editor is about to die.
    pub fn write_recovery_now(&mut self) {
        let mut writes: Vec<(PathBuf, u64, String)> = Vec::new();
        let mut cleaned: Vec<PathBuf> = Vec::new();
        for i in 0..self.buffers.len() {
            let buf = if i == self.active {
                &self.buffer
            } else {
                &self.buffers[i].buffer
            };
            let Some(path) = buf.path.as_ref() else {
                continue;
            };
            let dumped = self.recovery_written.get(path).copied();
            if buf.dirty && dumped != Some(buf.version) {
                writes.push((path.clone(), buf.version, buf.rope.to_string()));
            } else if !buf.dirty && dumped.is_some() {
                cleaned.push(path.clone());
            }
        }
        for (path, version, text) in writes {
            let Some(dest) = recovery_path(&path) else {
                continue;
            };
            let rec = RecoveryFile {
                path: path.display().to_string(),
                saved_at: now_secs(),
                text,
            };
            if write_to(&dest, &rec).is_ok() {
                self.recovery_written.insert(path, version);
            }
        }
        for path in cleaned {
            self.discard_recovery(&path);
        }
    }

    /// A buffer just opened has a recovery file left by an editor that died:
    /// put its text in the buffer as unsaved changes. The file's own text is
    /// recorded as an undo step first, so `u` shows what's on disk and `:e!`
    /// throws the recovered text away; nothing reaches the file until `:w`.
    pub(super) fn apply_recovery(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            return;
        };
        // Already this session's: applied when a restored session opened it,
        // or dumped since. Read again, it would match the buffer and be
        // removed while the buffer is still dirty.
        if self.recovery_written.contains_key(&path) {
            return;
        }
        let Some(rec) = recovery_path(&path).and_then(|dest| load_from(&dest)) else {
            return;
        };
        let Some(text) = recovered_text(&rec, &self.buffer.rope.to_string()) else {
            self.discard_recovery(&path);
            return;
        };
        self.history.record(&self.buffer.rope, self.window.cursor);
        self.buffer.replace_all(text);
        self.buffer.dirty = true;
        self.clamp_cursor_normal();
        self.recovery_written.insert(path, self.buffer.version);
        let age = Duration::from_secs(now_secs().saturating_sub(rec.saved_at));
        self.status_msg = format!(
            "recovered unsaved changes from {} — :w keeps them, :e! discards them",
            super::edit::time_ago(age)
        );
    }

    pub(super) fn discard_recovery(&mut self, path: &Path) {
        self.recovery_written.remove(path);
        if let Some(dest) = recovery_path(path) {
            let _ = std::fs::remove_file(dest);
        }
    }

    /// A deliberate quit. Whatever was still dirty was thrown away with `:q!`,
    /// and a file dumped for a buffer since closed is stale either way.
    pub(super) fn discard_all_recovery(&mut self) {
        let mut paths: Vec<PathBuf> = self.recovery_written.keys().cloned().collect();
        for i in 0..self.buffers.len() {
            let buf = if i == self.active {
                &self.buffer
            } else {
                &self.buffers[i].buffer
            };
            paths.extend(buf.path.clone());
        }
        for path in paths {
            self.discard_recovery(&path);
        }
    }
}
