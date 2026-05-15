//! Per-workspace session persistence. On clean shutdown the open buffer
//! set + cursor positions land in `~/.cache/binvim/sessions/<hash>.json`;
//! on launch we restore them if the user hasn't asked for a specific file
//! and the saved session matches the current cwd.
//!
//! Key design points:
//! - The session key is a hash of the canonical cwd, so per-project
//!   binvim instances don't clobber each other.
//! - We only restore when no explicit file arg was passed — opening
//!   `binvim foo.rs` always means "I want foo.rs", never "restore."
//! - Buffers that no longer exist on disk are silently dropped.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub cwd: String,
    pub buffers: Vec<SessionBuffer>,
    pub active: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionBuffer {
    pub path: String,
    pub line: usize,
    pub col: usize,
    pub view_top: usize,
    /// Per-buffer jumplist — `(line, col)` pairs the user can walk via
    /// `Ctrl-O` / `Ctrl-I`. Skipped on serialisation when empty so old
    /// session files keep parsing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jumplist: Vec<(usize, usize)>,
    /// Cursor index into `jumplist` — `Ctrl-O` walks backward from here.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub jump_idx: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// `~/.cache/binvim/sessions/<hash>.json` for the given cwd. Returns `None`
/// if we can't resolve `HOME` or canonicalise `cwd`.
pub fn session_path(cwd: &Path) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let key = hash_path(&canon);
    let mut p = PathBuf::from(home);
    p.push(".cache/binvim/sessions");
    p.push(format!("{key}.json"));
    Some(p)
}

/// FNV-1a 64-bit of the path's string representation. Stable, fast, and
/// good enough to key sessions by — collision odds are negligible at this
/// scale and a collision would only mean "restore the wrong session,"
/// which the cwd check inside `load_for_cwd` catches anyway.
fn hash_path(path: &Path) -> String {
    let bytes = path.to_string_lossy();
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

pub fn save(session: &Session) -> std::io::Result<()> {
    let Some(path) = session_path(Path::new(&session.cwd)) else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(session)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, json)
}

/// Remove the saved session for `cwd`. Called on clean shutdown when
/// no buffers are open — leaving a stale session on disk would cause
/// the next launch in the same cwd to silently revive every closed
/// buffer.
pub fn clear_for_cwd(cwd: &Path) -> std::io::Result<()> {
    let Some(path) = session_path(cwd) else {
        return Ok(());
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Load the saved session for the given cwd. Returns `None` if the file
/// doesn't exist, can't be parsed, or its embedded `cwd` doesn't match
/// the live canonicalised cwd (defensive — guards against hash collisions
/// or stale cache after a directory move).
pub fn load_for_cwd(cwd: &Path) -> Option<Session> {
    let path = session_path(cwd)?;
    let text = std::fs::read_to_string(&path).ok()?;
    let session: Session = serde_json::from_str(&text).ok()?;
    let canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    if session.cwd != canon.to_string_lossy() {
        return None;
    }
    Some(session)
}
