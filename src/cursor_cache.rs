//! nvim-style "last cursor" cache: remember where you last were in each file
//! so a later reopen comes back to it — independent of whether you ever saved
//! the buffer. It's deliberately separate from undo history (which is only
//! written on `:w`): this is one tiny per-file JSON written whenever you leave
//! a buffer or quit, and read back on open. It's gated by a content hash, so a
//! file changed on disk while closed (by you outside, or by an external
//! program) doesn't get a stale cursor — exactly like the undo cache treats
//! touched files.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cursor::Cursor;

/// Cache subdirectory under `~/.cache/binvim/`.
const DIR: &str = "cursor";
/// Entries not written for this many days are pruned, matching `UNDO_MAX_AGE`.
const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(90 * 24 * 60 * 60);

/// The per-file shape. Stored as plain line/col/want_col instead of a `Cursor`
/// because `Cursor` isn't serde and session handles it the same way.
#[derive(Serialize, Deserialize)]
struct CursorFile {
    file_hash: u64,
    #[serde(default)]
    line: usize,
    #[serde(default)]
    col: usize,
    #[serde(default)]
    want_col: usize,
}

impl CursorFile {
    fn from_cursor(file_hash: u64, cursor: Cursor) -> Self {
        CursorFile {
            file_hash,
            line: cursor.line,
            col: cursor.col,
            want_col: cursor.want_col,
        }
    }
}

/// `<cache>/binvim/cursor/`, or `None` under tests (like `undo_dir`, so a test
/// writing buffers doesn't leave cache files for temp paths).
fn cursor_dir() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    let mut p = crate::paths::cache_dir()?;
    p.push(DIR);
    Some(p)
}

/// Resolve the on-disk cursor-cache file for `target`, keyed by the stable
/// `paths::path_key` so the file survives Rust releases. The path is
/// canonicalized first (like `undo::cache_path_for`) so opening the same real
/// file through a symlink maps onto the same cache entry as the direct path.
pub fn cache_path_for(target: &Path) -> Option<std::path::PathBuf> {
    let canon = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    let id = crate::paths::path_key(&canon);
    let mut p = cursor_dir()?;
    p.push(format!("{id}.json"));
    Some(p)
}

/// Write `cursor` into `path` stamped with the content `file_hash`. The hash
/// gates a later read, so an open of a changed file won't restore onto a
/// different version. Best-effort, like session and undo persistence, so
/// failures are silently dropped rather than surfacing in the TUI.
fn save_to(path: &Path, file_hash: u64, cursor: Cursor) {
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // Every entry is tiny but this is a user-private cache dir — keep it
    // owner-only like the undo dir, not whatever the umask happens to make.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let cf = CursorFile::from_cursor(file_hash, cursor);
    let Ok(bytes) = serde_json::to_vec(&cf) else { return };
    let _ = crate::paths::write_atomic(path, &bytes);
}

/// Persist `cursor` for `target`, keyed by a hash of the buffer's current
/// content. Best-effort.
pub fn save(target: &Path, file_hash: u64, cursor: Cursor) {
    if let Some(p) = cache_path_for(target) {
        save_to(&p, file_hash, cursor);
    }
}

/// Read the cursor previously written into `path`, but only when the stored
/// content hash matches the file on disk (`expected_hash`). `None` for a
/// changed, never saved, or cache-less file — callers fall back to the top of
/// the buffer.
fn load_from(path: &Path, expected_hash: u64) -> Option<Cursor> {
    let bytes = std::fs::read(path).ok()?;
    let cf: CursorFile = serde_json::from_slice(&bytes).ok()?;
    if cf.file_hash != expected_hash {
        return None;
    }
    Some(Cursor {
        line: cf.line,
        col: cf.col,
        want_col: cf.want_col,
    })
}

/// Read the cached cursor for `target` under the disk `expected_hash`.
pub fn load(target: &Path, expected_hash: u64) -> Option<Cursor> {
    let p = cache_path_for(target)?;
    load_from(&p, expected_hash)
}

/// Narrow the cursor directory to its owner and drop entries not written for
/// `MAX_AGE`. Mirror of the undo-dir tidy, so the cache doesn't grow forever.
pub fn prune_stale() {
    let Some(dir) = cursor_dir() else { return };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let now = std::time::SystemTime::now();
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_file() {
            let stale = meta
                .modified()
                .ok()
                .and_then(|m| now.duration_since(m).ok())
                .map(|age| age > MAX_AGE)
                .unwrap_or(false);
            if stale {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway dir unique per test process, so parallel runs don't collide.
    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("binvim-cc-{tag}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trip_and_hash_gate() {
        let dir = scratch("rt");
        let p = dir.join("a.json");
        let c = Cursor {
            line: 4,
            col: 2,
            want_col: 7,
        };
        save_to(&p, 123, c);
        // The core safety property: content changed on disk ⇒ the cached
        // cursor must be rejected, even though a cache file exists.
        assert!(load_from(&p, 999).is_none());
        // Unchanged content ⇒ the position comes back.
        assert_eq!(load_from(&p, 123), Some(c));
        // No cache file at all ⇒ None, never a panic/default.
        assert_eq!(load_from(&dir.join("missing.json"), 123), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_fields_default_to_zero() {
        let dir = scratch("fields");
        let p = dir.join("a.json");
        // An older/corrupt entry with only the hash parses and keeps it, but
        // the optional position fields fall back to 0.
        std::fs::write(&p, br#"{"file_hash":7}"#).unwrap();
        let got = load_from(&p, 7).unwrap();
        assert_eq!((got.line, got.col, got.want_col), (0, 0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
