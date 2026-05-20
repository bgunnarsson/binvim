//! Cross-platform discovery of binvim's home / config / cache / data
//! directories. Every caller goes through here instead of probing
//! `$HOME` directly — `$HOME` is unset on Windows, where the right
//! roots are `%USERPROFILE%`, `%APPDATA%`, and `%LOCALAPPDATA%`.
//!
//! The `dirs` crate handles the per-platform conventions; this module
//! just scopes each root under our `binvim` subdirectory and exposes
//! it as a single function call.
//!
//! ## Per platform
//!
//! | function       | Linux                    | macOS                              | Windows                                 |
//! |----------------|--------------------------|------------------------------------|-----------------------------------------|
//! | `home_dir()`   | `$HOME`                  | `$HOME`                            | `%USERPROFILE%`                         |
//! | `config_dir()` | `~/.config/binvim/`      | `~/Library/Application Support/binvim/` | `%APPDATA%\binvim\`                 |
//! | `cache_dir()`  | `~/.cache/binvim/`       | `~/Library/Caches/binvim/`         | `%LOCALAPPDATA%\binvim\`                |
//! | `data_dir()`   | `~/.local/share/binvim/` | `~/Library/Application Support/binvim/` | `%APPDATA%\binvim\`                 |
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

use std::path::PathBuf;

const APP: &str = "binvim";

/// The user's home directory. `$HOME` on Unix; `%USERPROFILE%` on Windows.
pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// `~/.config/binvim/` on Linux, `~/Library/Application Support/binvim/`
/// on macOS, `%APPDATA%\binvim\` on Windows. Holds `config.toml`.
pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(APP))
}

/// `~/.cache/binvim/` on Linux, `~/Library/Caches/binvim/` on macOS,
/// `%LOCALAPPDATA%\binvim\` on Windows. Holds sessions, undo history,
/// crash logs, and recents.
pub fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join(APP))
}

/// `~/.local/share/binvim/` on Linux; the Application Support path on
/// macOS; `%APPDATA%\binvim\` on Windows. Holds the spell wordlist
/// override.
pub fn data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(APP))
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
