use crate::cursor::Cursor;
use ropey::Rope;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Snapshot {
    pub rope: Rope,
    pub cursor: Cursor,
}

#[derive(Default, Clone)]
pub struct History {
    past: Vec<Snapshot>,
    future: Vec<Snapshot>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Save the current state before a mutation.
    pub fn record(&mut self, rope: &Rope, cursor: Cursor) {
        self.past.push(Snapshot {
            rope: rope.clone(),
            cursor,
        });
        self.future.clear();
        // Cap so a long-running session doesn't OOM. 1000 is plenty —
        // anything older is academically interesting at best.
        const MAX: usize = 1000;
        if self.past.len() > MAX {
            let drop = self.past.len() - MAX;
            self.past.drain(0..drop);
        }
    }

    /// Undo: take the last recorded snapshot and push current onto redo stack.
    pub fn undo(&mut self, current_rope: &Rope, current_cursor: Cursor) -> Option<Snapshot> {
        let snap = self.past.pop()?;
        self.future.push(Snapshot {
            rope: current_rope.clone(),
            cursor: current_cursor,
        });
        Some(snap)
    }

    pub fn redo(&mut self, current_rope: &Rope, current_cursor: Cursor) -> Option<Snapshot> {
        let snap = self.future.pop()?;
        self.past.push(Snapshot {
            rope: current_rope.clone(),
            cursor: current_cursor,
        });
        Some(snap)
    }

    /// Persist the history to `path` along with `file_hash`. We store the
    /// hash so a subsequent load can reject undo state that was recorded
    /// against a different version of the underlying file (someone edited
    /// it externally between sessions).
    pub fn save_to_path(&self, path: &Path, file_hash: u64) -> std::io::Result<()> {
        let stored = StoredHistory {
            file_hash,
            past: self.past.iter().map(StoredSnapshot::from).collect(),
            future: self.future.iter().map(StoredSnapshot::from).collect(),
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut tmp = path.to_path_buf();
        tmp.set_extension("tmp");
        let serialized = match serde_json::to_vec(&stored) {
            Ok(v) => v,
            Err(e) => return Err(std::io::Error::new(std::io::ErrorKind::Other, e)),
        };
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&serialized)?;
        f.sync_all()?;
        std::fs::rename(tmp, path)
    }

    /// Inverse of `save_to_path`. Returns `None` if the file is missing,
    /// malformed, or stamped with a different `file_hash`.
    pub fn load_from_path(path: &Path, expected_hash: u64) -> Option<Self> {
        let mut f = std::fs::File::open(path).ok()?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).ok()?;
        let stored: StoredHistory = serde_json::from_slice(&buf).ok()?;
        if stored.file_hash != expected_hash {
            return None;
        }
        Some(Self {
            past: stored.past.iter().map(Snapshot::from).collect(),
            future: stored.future.iter().map(Snapshot::from).collect(),
        })
    }
}

#[derive(Serialize, Deserialize)]
struct StoredHistory {
    file_hash: u64,
    past: Vec<StoredSnapshot>,
    future: Vec<StoredSnapshot>,
}

#[derive(Serialize, Deserialize)]
struct StoredSnapshot {
    text: String,
    line: usize,
    col: usize,
    want_col: usize,
}

impl From<&Snapshot> for StoredSnapshot {
    fn from(s: &Snapshot) -> Self {
        Self {
            text: s.rope.to_string(),
            line: s.cursor.line,
            col: s.cursor.col,
            want_col: s.cursor.want_col,
        }
    }
}

impl From<&StoredSnapshot> for Snapshot {
    fn from(s: &StoredSnapshot) -> Self {
        Self {
            rope: Rope::from_str(&s.text),
            cursor: Cursor {
                line: s.line,
                col: s.col,
                want_col: s.want_col,
            },
        }
    }
}

/// Hash a file's contents. Used as the staleness key for persisted undo.
pub fn hash_text(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Resolve the on-disk persisted-undo file for `target` under
/// `~/.cache/binvim/undo/`. Returns `None` if `$HOME` is unset or the
/// target path can't be canonicalised.
pub fn cache_path_for(target: &Path) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let canon = target.canonicalize().unwrap_or_else(|_| target.to_path_buf());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    canon.to_string_lossy().hash(&mut h);
    let id = format!("{:016x}", h.finish());
    let mut p = PathBuf::from(home);
    p.push(".cache/binvim/undo");
    p.push(format!("{id}.json"));
    Some(p)
}
