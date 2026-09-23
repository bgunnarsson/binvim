//! When recovery files (`recover.rs`) are written, applied and removed. A
//! dirty buffer's text is dumped every `RECOVERY_INTERVAL`; the file goes away
//! once the buffer is written, reverted, closed or quit on purpose, and stays
//! when the editor dies any other way — which is the point.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::recover::{
    RecoveryFile, RecoveryKey, held_by_another_process, load_from, now_secs, recovered_text,
    recovery_path, recovery_path_for, write_to,
};

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
        #[cfg(unix)]
        {
            let session = self.build_session();
            *self
                .session_snapshot
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(session);
            // What a quit would write for the active buffer, under the same
            // rules as `persist_active_cursor`.
            let cursor = match (&self.buffer.path, self.buffer.clean_hash) {
                (Some(path), Some(hash)) if self.active_shown => {
                    Some((path.clone(), hash, self.window.cursor))
                }
                _ => None,
            };
            *self
                .cursor_snapshot
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = cursor;
        }
    }

    /// Bring every buffer's recovery file up to date: write the dirty ones
    /// whose text moved since their last dump, and remove the ones whose
    /// buffer has gone clean (undone back to what's on disk). Called on the
    /// interval, and straight away when the editor is about to die.
    pub fn write_recovery_now(&mut self) {
        let mut writes: Vec<(RecoveryKey, u64, String)> = Vec::new();
        let mut cleaned: Vec<RecoveryKey> = Vec::new();
        for i in 0..self.buffers.len() {
            let buf = if i == self.active {
                &self.buffer
            } else {
                &self.buffers[i].buffer
            };
            let Some(path) = buf.path.as_ref() else {
                continue;
            };
            let key = RecoveryKey::Path(path.clone());
            let dumped = self.recovery_written.get(&key).copied();
            if buf.dirty && dumped != Some(buf.version) {
                writes.push((key, buf.version, buf.rope.to_string()));
            } else if !buf.dirty && dumped.is_some() {
                cleaned.push(key);
            }
        }
        let snapshot = self.recovery_snapshot.clone();
        let _writing = snapshot.lock().unwrap_or_else(|e| e.into_inner());
        for (key, version, text) in writes {
            if dump(&key, text) {
                self.recovery_written.insert(key, version);
            }
        }
        for key in cleaned {
            self.discard_recovery_key(&key);
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
        let key = RecoveryKey::Path(path);
        if self.recovery_written.contains_key(&key) {
            return;
        }
        let Some(dest) = recovery_path_for(&key) else {
            return;
        };
        let Some(rec) = load_from(&dest) else {
            return;
        };
        if held_by_another_process(&rec) {
            self.status_msg = format!(
                "another binvim (pid {}) has unsaved changes to this file",
                rec.pid
            );
            return;
        }
        let Some(text) = recovered_text(&rec, &self.buffer.rope.to_string()) else {
            // Left by a crash, but nothing in it that isn't on disk.
            let _ = std::fs::remove_file(dest);
            return;
        };
        self.history.record(&self.buffer.rope, self.window.cursor);
        self.buffer.replace_all(text);
        self.buffer.dirty = true;
        self.clamp_cursor_normal();
        self.recovery_written.insert(key, self.buffer.version);
        let age = Duration::from_secs(now_secs().saturating_sub(rec.saved_at));
        self.status_msg = format!(
            "recovered unsaved changes from {} — :w keeps them, :e! discards them",
            super::edit::time_ago(age)
        );
    }

    /// The dirty buffers as the signal thread would find them. A rope clone
    /// shares its nodes, so this costs a `Vec` per iteration, not the text.
    #[cfg(unix)]
    pub(super) fn refresh_recovery_snapshot(&mut self) {
        let dirty: Vec<(RecoveryKey, ropey::Rope)> = (0..self.buffers.len())
            .filter_map(|i| {
                let buf = if i == self.active {
                    &self.buffer
                } else {
                    &self.buffers[i].buffer
                };
                let key = RecoveryKey::Path(buf.path.clone()?);
                buf.dirty.then(|| (key, buf.rope.clone()))
            })
            .collect();
        *self
            .recovery_snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = dirty;
    }

    /// SIGTERM and SIGHUP end the process on the spot by default. They're
    /// caught on a thread of their own rather than by a flag the loop checks:
    /// a closed terminal leaves crossterm's poll spinning in `read` on the dead
    /// tty, so the loop never comes round to look. The thread writes every
    /// snapshotted buffer's recovery file, saves the session, puts the terminal
    /// back, and exits with the signal's conventional status.
    ///
    /// The session is saved here and not after a panic: a panic can come from
    /// a file's content, and a session naming that file would reopen it — and
    /// crash again — on the next bare `binvim`. A signal says nothing about
    /// what's open. The snapshot can be a recovery interval old; a buffer
    /// opened since is missing from it, though its recovery file still applies
    /// when it's next opened.
    /// A child that takes the terminal over (lazygit, yazi, `:install`) runs
    /// in binvim's process group with raw mode off, so a `Ctrl-C` typed at it
    /// — at lazygit's "not a git repository" prompt, say — sends SIGINT to
    /// binvim as well, and by default that ends binvim with its dirty buffers
    /// unwritten. SIGINT and SIGQUIT keep their default action except while a
    /// suspend has `interrupts_quit` cleared. The child is unaffected: exec
    /// resets a handled signal to its default.
    #[cfg(unix)]
    pub(super) fn guard_interrupts(&self) {
        use signal_hook::consts::{SIGINT, SIGQUIT};
        for signal in [SIGINT, SIGQUIT] {
            let _ = signal_hook::flag::register_conditional_default(
                signal,
                std::sync::Arc::clone(&self.interrupts_quit),
            );
        }
    }

    #[cfg(unix)]
    pub(super) fn spawn_signal_recovery(&self) {
        use signal_hook::consts::{SIGHUP, SIGTERM};
        let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGTERM, SIGHUP]) else {
            return;
        };
        let snapshot = self.recovery_snapshot.clone();
        let session = self.session_snapshot.clone();
        let cursor_snap = self.cursor_snapshot.clone();
        std::thread::spawn(move || {
            let Some(signal) = signals.forever().next() else {
                return;
            };
            {
                let dirty = snapshot.lock().unwrap_or_else(|e| e.into_inner());
                for (key, rope) in dirty.iter() {
                    dump(key, rope.to_string());
                }
            }
            if let Some(session) = session.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                let _ = crate::session::save_or_clear(session);
            }
            // A signal is a clean-ish exit for the cursor: the editor's view
            // state is still intact, so remember where we last were just like
            // a `:q`. The snapshot is the last write, or at most a recovery
            // interval old.
            if let Some((path, hash, cursor)) = cursor_snap
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                crate::cursor_cache::save(path, *hash, *cursor);
            }
            crate::crash::restore_terminal_best_effort();
            std::process::exit(128 + signal);
        });
    }

    /// Recovered text the user hasn't seen yet: a crash's dump for a file not
    /// opened since, or another binvim's live one. Edits made without opening
    /// the file — `:S`, an LSP rename — skip it rather than build on the file
    /// and write over that text.
    pub(super) fn pending_recovery(&self, path: &Path) -> bool {
        // Made absolute the way `Buffer::from_path` does, so the key matches:
        // ripgrep names files `./src/x.rs`, and `.` segments change the hash.
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if self
            .recovery_written
            .contains_key(&RecoveryKey::Path(path.clone()))
        {
            return false;
        }
        let Some(rec) = recovery_path(&path).and_then(|dest| load_from(&dest)) else {
            return false;
        };
        match std::fs::read_to_string(&path) {
            Ok(disk) => recovered_text(&rec, &disk).is_some(),
            Err(_) => true,
        }
    }

    /// The text `apply_recovery` will give `path`'s buffer when it's opened,
    /// when that isn't what's on disk — for a caller that has to read the
    /// text before opening the file.
    pub(super) fn recovered_text_for(&self, path: &Path) -> Option<String> {
        if self
            .recovery_written
            .contains_key(&RecoveryKey::Path(path.to_path_buf()))
        {
            return None;
        }
        let rec = recovery_path(path).and_then(|dest| load_from(&dest))?;
        if held_by_another_process(&rec) {
            return None;
        }
        let disk = std::fs::read_to_string(path).ok()?;
        recovered_text(&rec, &disk).map(str::to_string)
    }

    /// Remove `path`'s recovery file, if it's this session's — written or
    /// applied here. One a crash left for a file not opened yet, or one
    /// another binvim is still writing, isn't this session's to remove.
    pub(super) fn discard_recovery(&mut self, path: &Path) {
        self.discard_recovery_key(&RecoveryKey::Path(path.to_path_buf()));
    }

    pub(super) fn discard_recovery_key(&mut self, key: &RecoveryKey) {
        if self.recovery_written.remove(key).is_none() {
            return;
        }
        if let Some(dest) = recovery_path_for(key) {
            let _ = std::fs::remove_file(dest);
        }
    }

    /// A deliberate quit. Whatever was still dirty was thrown away with `:q!`,
    /// and a file dumped for a buffer since closed is stale either way.
    pub(super) fn discard_all_recovery(&mut self) {
        // Emptied first, so a signal arriving mid-quit has nothing to rewrite.
        self.recovery_snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let keys: Vec<RecoveryKey> = self.recovery_written.keys().cloned().collect();
        for key in keys {
            self.discard_recovery_key(&key);
        }
    }
}

/// Write `text` as `key`'s recovery file. False when there's nowhere to put
/// it (tests, no cache dir) or the write failed.
fn dump(key: &RecoveryKey, text: String) -> bool {
    let Some(dest) = recovery_path_for(key) else {
        return false;
    };
    let path = match key {
        RecoveryKey::Path(path) => path.display().to_string(),
    };
    let rec = RecoveryFile {
        path,
        saved_at: now_secs(),
        text,
        pid: std::process::id(),
    };
    write_to(&dest, &rec).is_ok()
}
