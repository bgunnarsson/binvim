//! Cross-platform discovery of binvim's home / config / cache / data
//! directories. Every caller goes through here instead of probing
//! `$HOME` directly — `$HOME` is unset on Windows, where the right
//! roots are `%USERPROFILE%`, `%APPDATA%`, and `%LOCALAPPDATA%`.
//!
//! macOS uses the XDG layout (`~/.config`, `~/.cache`, `~/.local/share`)
//! rather than `~/Library/Application Support` / `~/Library/Caches`.
//! That mirrors what almost every Rust CLI does (ripgrep, fd, bat,
//! zoxide, …) and — more importantly here — matches what binvim
//! itself used pre-Windows-port, so existing macOS users don't lose
//! their configs / sessions / undo history on upgrade. The `dirs`
//! crate's macOS defaults are aimed at GUI apps; for a TUI editor
//! they're the wrong call.
//!
//! ## Per platform
//!
//! | function       | Linux                    | macOS                    | Windows                  |
//! |----------------|--------------------------|--------------------------|--------------------------|
//! | `home_dir()`   | `$HOME`                  | `$HOME`                  | `%USERPROFILE%`          |
//! | `config_dir()` | `~/.config/binvim/`      | `~/.config/binvim/`      | `%APPDATA%\binvim\`      |
//! | `cache_dir()`  | `~/.cache/binvim/`       | `~/.cache/binvim/`       | `%LOCALAPPDATA%\binvim\` |
//! | `data_dir()`   | `~/.local/share/binvim/` | `~/.local/share/binvim/` | `%APPDATA%\binvim\`      |
//!
//! `XDG_CONFIG_HOME` / `XDG_CACHE_HOME` / `XDG_DATA_HOME` are honoured
//! on both Linux and macOS so a user who's set them globally gets the
//! same routing on both platforms.
//!
//! All four return `None` when the platform can't resolve the parent
//! root (unset / unreadable env vars). Callers fall back to skipping
//! the persisted feature in that case — they never hard-error on it.

// The library crate and the binary crate each compile this file. Each
// crate uses a different subset (the bin doesn't call `find_on_path`
// directly because the LSP / DAP / format modules wrap it; the lib
// doesn't call `home_join` because tilde expansion happens in the
// editor proper). Silence dead-code for the unused-in-this-crate side
// rather than splitting the module artificially.
#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};

const APP: &str = "binvim";

/// The user's home directory. `$HOME` on Unix; `%USERPROFILE%` on Windows.
pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// `~/.config/binvim/` on Unix (Linux and macOS, both honour
/// `XDG_CONFIG_HOME`), `%APPDATA%\binvim\` on Windows. Holds
/// `config.toml`.
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        xdg_or_home("XDG_CONFIG_HOME", ".config")
    }
    #[cfg(not(unix))]
    {
        dirs::config_dir().map(|d| d.join(APP))
    }
}

/// `~/.cache/binvim/` on Unix (Linux and macOS, both honour
/// `XDG_CACHE_HOME`), `%LOCALAPPDATA%\binvim\` on Windows. Holds
/// sessions, undo history, crash logs, and recents.
pub fn cache_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        xdg_or_home("XDG_CACHE_HOME", ".cache")
    }
    #[cfg(not(unix))]
    {
        dirs::cache_dir().map(|d| d.join(APP))
    }
}

/// `~/.local/share/binvim/` on Unix (Linux and macOS, both honour
/// `XDG_DATA_HOME`), `%APPDATA%\binvim\` on Windows. Holds the spell
/// wordlist override.
pub fn data_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        xdg_or_home("XDG_DATA_HOME", ".local/share")
    }
    #[cfg(not(unix))]
    {
        dirs::data_dir().map(|d| d.join(APP))
    }
}

/// XDG-style resolution: honour `$var` if set + non-empty, otherwise
/// fall back to `$HOME/<sub>`. Returns the path with `binvim` appended
/// so call sites get the per-app subdirectory in one shot.
#[cfg(unix)]
fn xdg_or_home(var: &str, sub: &str) -> Option<PathBuf> {
    if let Some(v) = std::env::var_os(var).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(v).join(APP));
    }
    home_dir().map(|h| h.join(sub).join(APP))
}

/// Resolve `~/sub/path` against `home_dir()`. Returns `None` when the
/// home directory can't be resolved; otherwise joins the rest into a
/// `PathBuf` using the platform's path separator. Use this instead of
/// `format!("{}/{}", home, rest)` to keep separators portable.
pub fn home_join<P: AsRef<std::path::Path>>(rest: P) -> Option<PathBuf> {
    home_dir().map(|h| h.join(rest))
}

/// Look up an executable on `$PATH`. Splits with `std::env::split_paths`
/// (so `;`-separated entries work on Windows, `:` on Unix). On Windows,
/// when `name` has no extension, also probes `name.exe` / `name.cmd` /
/// `name.bat` — these cover the bulk of dev-tool installs.
///
/// Returns the first match. Use over `path.split(':')` everywhere —
/// the `:` split silently fails on Windows.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let candidates = candidate_names(name);
    for dir in std::env::split_paths(&path_var) {
        for cand in &candidates {
            let p = dir.join(cand);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// True when `find_on_path(name)` would return `Some(_)`. Use when the
/// path itself isn't needed — the implementation is identical.
pub fn on_path(name: &str) -> bool {
    find_on_path(name).is_some()
}

fn candidate_names(name: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(4);
    out.push(name.to_string());
    // Only synthesise extensions on Windows. The check uses `Path::extension`
    // rather than a naive `contains('.')` so dotted filenames like
    // `language_server.sh` are treated as already-extended.
    if cfg!(windows) && std::path::Path::new(name).extension().is_none() {
        for ext in ["exe", "cmd", "bat"] {
            out.push(format!("{name}.{ext}"));
        }
    }
    out
}

/// Write `bytes` to `path` so that a write failing partway — a full disk, a
/// dropped mount — leaves the old file whole rather than truncated: the bytes
/// go to a temp file beside the target, are synced, and are renamed over it.
///
/// A rename replaces the directory entry, so the ways it would change the file
/// rather than its contents are handled here. A symlink is written through to
/// what it names, and the target's permissions are copied onto the temp file.
/// Two cases fall back to writing in place, which keeps the inode: a file with
/// other hard links (a rename would split them), and a directory the temp file
/// can't be created in (a file the user may write but not replace).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let target = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => match path.canonicalize() {
            Ok(real) => real,
            // Dangling: writing through it creates the file it names.
            Err(_) => return std::fs::write(path, bytes),
        },
        _ => path.to_path_buf(),
    };
    let existing = std::fs::metadata(&target).ok();
    if existing.as_ref().is_some_and(has_other_links) {
        return std::fs::write(&target, bytes);
    }
    let name = target.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name")
    })?;
    let tmp = target.with_file_name(format!(
        ".{}.binvim-{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    let file = match std::fs::File::create(&tmp) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return std::fs::write(&target, bytes);
        }
        Err(e) => return Err(e),
    };
    let written = (|| {
        let mut file = file;
        if let Some(meta) = &existing {
            file.set_permissions(meta.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        // Closed before the rename — Windows won't rename an open file.
        drop(file);
        std::fs::rename(&tmp, &target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(unix)]
fn has_other_links(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.nlink() > 1
}

#[cfg(not(unix))]
fn has_other_links(_: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory per test, so parallel runs don't share files.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("binvim_paths_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn leftover_temp_files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn write_atomic_creates_and_replaces_a_file() {
        let dir = scratch("replace");
        let file = dir.join("a.txt");
        write_atomic(&file, b"one\n").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"one\n");
        write_atomic(&file, b"two\n").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"two\n");
        assert!(leftover_temp_files(&dir).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_keeps_the_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode");
        let file = dir.join("secret.txt");
        std::fs::write(&file, "old").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&file, b"new").unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_writes_through_a_symlink() {
        let dir = scratch("symlink");
        let real = dir.join("real.txt");
        let link = dir.join("link.txt");
        std::fs::write(&real, "old").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, b"new").unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&real).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_keeps_hard_links_together() {
        let dir = scratch("hardlink");
        let first = dir.join("first.txt");
        let second = dir.join("second.txt");
        std::fs::write(&first, "old").unwrap();
        std::fs::hard_link(&first, &second).unwrap();
        write_atomic(&first, b"new").unwrap();
        assert_eq!(std::fs::read(&second).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_writes_in_place_when_the_directory_is_read_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("readonly");
        let file = dir.join("a.txt");
        std::fs::write(&file, "old").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        // Root ignores directory permissions, so the temp file gets created
        // and this would test the ordinary path instead.
        let root = std::fs::File::create(dir.join("probe")).is_ok();
        let result = write_atomic(&file, b"new");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        if root {
            return;
        }
        result.unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new");
        assert!(leftover_temp_files(&dir).is_empty());
    }
}
