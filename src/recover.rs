//! Recovery files: the text of a buffer with unsaved changes, written to
//! `<cache>/binvim/recover/<key>.json` every few seconds while it stays dirty,
//! so a crash, a `kill` or a closed terminal costs seconds of typing rather
//! than everything since the last `:w`. The `App` side — when they're written,
//! applied on open and removed — is `app/recover_glue.rs`.
//!
//! One file per path, keyed like sessions (`paths::path_key`). Only buffers
//! with a path are covered: a `[No Name]` buffer has nothing to be recovered
//! into.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct RecoveryFile {
    /// The file the text belongs to, so a stray recovery file is still legible.
    pub path: String,
    /// Unix seconds when the text was written.
    pub saved_at: u64,
    pub text: String,
}

/// Where `file`'s recovery text lives. `None` under test, like every other
/// persisted path, so tests never leave files in the real cache.
pub fn recovery_path(file: &Path) -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let mut p = crate::paths::cache_dir()?;
    p.push("recover");
    p.push(format!("{}.json", crate::paths::path_key(file)));
    Some(p)
}

pub fn write_to(dest: &Path, rec: &RecoveryFile) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
        // The text may be a private file's — a key, a `.env` — so only this
        // user may look inside, whatever the files' own modes come out as.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    let json = serde_json::to_vec(rec).map_err(std::io::Error::other)?;
    crate::paths::write_atomic(dest, &json)
}

/// `None` for a missing file, and for one that doesn't parse — text cut short
/// mid-write can't be trusted to be the user's work, so it isn't offered.
pub fn load_from(dest: &Path) -> Option<RecoveryFile> {
    let bytes = std::fs::read(dest).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The text to recover, or `None` when it's what the file already holds —
/// a dump written just before a `:w` the crash didn't interrupt.
pub fn recovered_text<'a>(rec: &'a RecoveryFile, disk: &str) -> Option<&'a str> {
    (rec.text != disk).then_some(rec.text.as_str())
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("binvim_recover_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_written_recovery_file_loads_back() {
        let dir = scratch("roundtrip");
        let dest = dir.join("nested").join("r.json");
        let rec = RecoveryFile {
            path: "/tmp/a.txt".into(),
            saved_at: 42,
            text: "unsaved\n".into(),
        };
        write_to(&dest, &rec).unwrap();
        let back = load_from(&dest).unwrap();
        assert_eq!(back.path, "/tmp/a.txt");
        assert_eq!(back.saved_at, 42);
        assert_eq!(back.text, "unsaved\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn the_recovery_directory_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("private");
        let dest = dir.join("recover").join("r.json");
        let rec = RecoveryFile {
            path: "/tmp/a.txt".into(),
            saved_at: 0,
            text: "secret\n".into(),
        };
        write_to(&dest, &rec).unwrap();
        let mode = std::fs::metadata(dest.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_truncated_recovery_file_loads_as_none() {
        let dir = scratch("truncated");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("r.json");
        std::fs::write(&dest, r#"{"path":"/tmp/a.txt","saved_at":4"#).unwrap();
        assert!(load_from(&dest).is_none());
        assert!(load_from(&dir.join("missing.json")).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn text_matching_the_file_is_nothing_to_recover() {
        let rec = RecoveryFile {
            path: "/tmp/a.txt".into(),
            saved_at: 0,
            text: "same\n".into(),
        };
        assert_eq!(recovered_text(&rec, "same\n"), None);
        assert_eq!(recovered_text(&rec, "older\n"), Some("same\n"));
    }
}
