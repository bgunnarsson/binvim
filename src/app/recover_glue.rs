//! When recovery files (`recover.rs`) are written, applied and removed. A
//! dirty buffer's text is dumped every `RECOVERY_INTERVAL`; the file goes away
//! once the buffer is written, reverted, closed or quit on purpose, and stays
//! when the editor dies any other way — which is the point.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::recover::{RecoveryFile, now_secs, recovery_path, write_to};

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
