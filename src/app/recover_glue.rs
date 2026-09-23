//! When recovery files (`recover.rs`) are written, applied and removed. A
//! dirty buffer's text is dumped every `RECOVERY_INTERVAL`; the file goes away
//! once the buffer is written, reverted, closed or quit on purpose, and stays
//! when the editor dies any other way — which is the point.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::buffer::Buffer;
use crate::recover::{
    RecoveryFile, RecoveryKey, held_by_another_process, load_from, new_unnamed_key, now_secs,
    recovered_text, recovery_path, recovery_path_for, write_to,
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
                &mut self.buffer
            } else {
                &mut self.buffers[i].buffer
            };
            let Some(key) = dump_key(buf) else {
                continue;
            };
            let (dirty, version) = (buf.dirty, buf.version);
            let dumped = self.recovery_written.get(&key).copied();
            if dirty && dumped != Some(version) {
                let buf = if i == self.active {
                    &self.buffer
                } else {
                    &self.buffers[i].buffer
                };
                writes.push((key, version, buf.rope.to_string()));
            } else if !dirty && dumped.is_some() {
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

    /// Say at launch that a crash left `[No Name]` text behind, since nothing
    /// opens it until `:recover` is asked for. A launch message already
    /// showing wins; the next launch counts again.
    pub(super) fn announce_unnamed_recovery(&mut self) {
        let n = crate::recover::crash_unnamed_dumps().len();
        if n == 0 || !self.status_msg.is_empty() {
            return;
        }
        let what = if n == 1 {
            "1 unnamed buffer".to_string()
        } else {
            format!("{n} unnamed buffers")
        };
        self.status_msg = format!("{what} left by a crash — :recover opens them");
    }

    /// `:recover` — each `[No Name]` dump a crash left, opened as a buffer
    /// with unsaved changes. The dump is rewritten under this binvim's pid
    /// straight away: until then it names the dead one, and another binvim's
    /// `:recover` would take it too.
    pub(super) fn cmd_recover(&mut self) {
        let dumps: Vec<(String, RecoveryFile)> = crate::recover::crash_unnamed_dumps()
            .into_iter()
            .filter(|(key, _)| {
                !self
                    .recovery_written
                    .contains_key(&RecoveryKey::Unnamed(key.clone()))
            })
            .collect();
        if dumps.is_empty() {
            self.status_msg = "no unnamed buffers to recover".into();
            return;
        }
        let n = dumps.len();
        for (name, rec) in dumps {
            if let Err(e) = self.open_empty_buffer() {
                self.status_msg = format!("recover: {e}");
                return;
            }
            self.history.record(&self.buffer.rope, self.window.cursor);
            self.buffer.replace_all(&rec.text);
            self.buffer.dirty = true;
            self.buffer.unnamed_key = Some(name.clone());
            self.clamp_cursor_normal();
            let key = RecoveryKey::Unnamed(name);
            dump(&key, rec.text);
            self.recovery_written.insert(key, self.buffer.version);
        }
        self.status_msg = if n == 1 {
            "recovered 1 unnamed buffer — :w {file} keeps it".into()
        } else {
            format!("recovered {n} unnamed buffers — :w {{file}} keeps each")
        };
    }

    /// The dirty buffers as the signal thread would find them. A rope clone
    /// shares its nodes, so this costs a `Vec` per iteration, not the text.
    #[cfg(unix)]
    pub(super) fn refresh_recovery_snapshot(&mut self) {
        let mut dirty: Vec<(RecoveryKey, ropey::Rope)> = Vec::new();
        for i in 0..self.buffers.len() {
            let buf = if i == self.active {
                &mut self.buffer
            } else {
                &mut self.buffers[i].buffer
            };
            if buf.dirty
                && let Some(key) = dump_key(buf)
            {
                dirty.push((key, buf.rope.clone()));
            }
        }
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

/// What `buf`'s recovery file is kept under, if it has one. A `[No Name]`
/// buffer keeps its unnamed key until a save under its new name goes through
/// (`save_active` drops it): `:w name` names the buffer before writing, and a
/// write that fails would otherwise move the text to a dump for a file that
/// doesn't exist, which neither the launch notice nor `:recover` looks at.
pub(super) fn recovery_key(buf: &Buffer) -> Option<RecoveryKey> {
    match (&buf.unnamed_key, &buf.path) {
        (Some(name), _) => Some(RecoveryKey::Unnamed(name.clone())),
        (None, Some(path)) => Some(RecoveryKey::Path(path.clone())),
        (None, None) => None,
    }
}

/// `recovery_key`, naming a user's `[No Name]` buffer the first time it's
/// asked. An internal path-less buffer (`display_name`: `[Command Line]`,
/// `[config defaults]`) isn't the user's text, and has none.
fn dump_key(buf: &mut Buffer) -> Option<RecoveryKey> {
    if buf.path.is_none() && buf.display_name.is_none() && buf.unnamed_key.is_none() {
        buf.unnamed_key = Some(new_unnamed_key());
    }
    recovery_key(buf)
}

/// Write `text` as `key`'s recovery file. False when there's nowhere to put
/// it (tests, no cache dir) or the write failed.
fn dump(key: &RecoveryKey, text: String) -> bool {
    let Some(dest) = recovery_path_for(key) else {
        return false;
    };
    let path = match key {
        RecoveryKey::Path(path) => path.display().to_string(),
        RecoveryKey::Unnamed(_) => "[No Name]".to_string(),
    };
    let rec = RecoveryFile {
        path,
        saved_at: now_secs(),
        text,
        pid: std::process::id(),
    };
    write_to(&dest, &rec).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_no_name_buffer_keeps_its_unnamed_key_until_a_save_drops_it() {
        let mut buf = Buffer::empty();
        let Some(RecoveryKey::Unnamed(first)) = dump_key(&mut buf) else {
            panic!("a [No Name] buffer is dumped under an unnamed key");
        };
        assert_eq!(
            dump_key(&mut buf),
            Some(RecoveryKey::Unnamed(first.clone()))
        );
        // `:w name` names it first; a write that then fails leaves it here.
        buf.path = Some("/tmp/named.txt".into());
        assert_eq!(dump_key(&mut buf), Some(RecoveryKey::Unnamed(first)));
        buf.unnamed_key = None;
        assert_eq!(
            dump_key(&mut buf),
            Some(RecoveryKey::Path("/tmp/named.txt".into()))
        );
    }

    #[test]
    fn an_internal_buffer_has_no_recovery_key() {
        let mut buf = Buffer::empty();
        buf.display_name = Some("[Command Line]".into());
        assert_eq!(dump_key(&mut buf), None);
        assert_eq!(buf.unnamed_key, None);
    }
}
