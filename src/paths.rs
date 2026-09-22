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

/// FNV-1a 64-bit of the path's string form, as 16 hex digits — the file name
/// persisted per-path and per-cwd state is keyed by. Stable across Rust
/// releases, unlike `DefaultHasher`, so the key a file got last month still
/// finds it.
pub fn path_key(path: &Path) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.to_string_lossy().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// True when another user could have put `dir.join(rel)` there: `dir`, or
/// any path from it down to the candidate, is owned by someone else, or is
/// root's and writable by others (`/tmp`). A search that climbs out of the
/// project reaches such directories, and what it finds chooses a file to
/// open, a command to run or the root a language server builds in. Every
/// step is checked, not only the candidate's parent, because another user can
/// create `/tmp/share` mode `0755` and all of it would look trusted. Owner
/// first, bits second: mounts like WSL's `/mnt/c` report everything as
/// `0777`, and the user's own project there is still theirs.
#[cfg(unix)]
pub fn others_can_plant(dir: &Path, rel: impl AsRef<Path>) -> bool {
    let (me, system) = trusted_owners();
    let mut path = dir.to_path_buf();
    !owned_safely(&path, me, system)
        || rel.as_ref().components().any(|c| {
            path.push(c);
            !owned_safely(&path, me, system)
        })
}

#[cfg(not(unix))]
pub fn others_can_plant(_: &Path, _: impl AsRef<Path>) -> bool {
    false
}

/// A path that doesn't exist is safe here: the searches check existence
/// themselves, and nothing is planted at a path that isn't there.
#[cfg(unix)]
fn owned_safely(path: &Path, me: Option<u32>, system: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(link) = std::fs::symlink_metadata(path) else { return true };
    // A symlink's own mode means nothing (Linux reports `0777`), but whoever
    // owns it chose where it points.
    if link.file_type().is_symlink() && !trusted_owner(link.uid(), 0, me, system) {
        return false;
    }
    std::fs::metadata(path).map_or(true, |m| trusted_owner(m.uid(), m.mode(), me, system))
}

#[cfg(unix)]
fn trusted_owner(owner: u32, mode: u32, me: Option<u32>, system: u32) -> bool {
    Some(owner) == me || (owner == system && mode & 0o002 == 0)
}

/// This user's uid, read off `$HOME` (std has no `getuid`), and root's.
/// Tests can't make files owned by root or by another user, so under test
/// the files they make stand in for root's: trusted unless others can write
/// them, which is the case the search tests set up with `chmod`.
#[cfg(unix)]
fn trusted_owners() -> (Option<u32>, u32) {
    use std::os::unix::fs::MetadataExt;
    if cfg!(test) {
        let exe = std::env::current_exe().and_then(std::fs::metadata);
        return (None, exe.map_or(0, |m| m.uid()));
    }
    static ME: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    let me = *ME.get_or_init(|| {
        home_dir()
            .and_then(|h| std::fs::metadata(h).ok())
            .map(|m| m.uid())
    });
    (me, 0)
}

/// Walk up from `start` looking for any of `markers` — a filename, or
/// `*.ext` matching any file in the directory with that extension (the
/// `.sln` / `.csproj` convention, where the name varies). Returns the
/// first directory holding a marker `others_can_plant` clears; a marker
/// another user could have planted is passed over, per the upward-search
/// rule. This is the single implementation — the LSP, DAP, test and task
/// walks all route here, because the last time each carried its own copy,
/// three of them shipped without the guard.
pub fn find_marker_root(start: &Path, markers: &[impl AsRef<str>]) -> Option<PathBuf> {
    let canon = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut dir: &Path = canon.as_path();
    loop {
        if has_any_marker(dir, markers) {
            return Some(dir.to_path_buf());
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p,
            _ => return None,
        }
    }
}

/// `find_marker_root`, falling back to the canonical form of `start` when
/// nothing matches — for callers that always need a path to show or work
/// from. Callers that open, run or root something in the result still owe
/// the fallback its own `others_can_plant` check, as `lsp::ensure_for_path`
/// does: finding no marker doesn't make `/tmp` a workspace.
pub fn find_marker_root_or_start(start: &Path, markers: &[impl AsRef<str>]) -> PathBuf {
    let canon = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    find_marker_root(&canon, markers).unwrap_or(canon)
}

/// True when `dir` holds one of `markers` that `others_can_plant` clears.
pub(crate) fn has_any_marker(dir: &Path, markers: &[impl AsRef<str>]) -> bool {
    for marker in markers {
        let marker = marker.as_ref();
        if let Some(ext) = marker.strip_prefix("*.") {
            if dir_contains_extension(dir, ext) && !others_can_plant(dir, "") {
                return true;
            }
        } else if dir.join(marker).exists() && !others_can_plant(dir, marker) {
            return true;
        }
    }
    false
}

/// Any entry in `dir` with extension `ext`, compared case-insensitively.
pub(crate) fn dir_contains_extension(dir: &Path, ext: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        if let Some(file_ext) = entry.path().extension().and_then(|e| e.to_str()) {
            if file_ext.eq_ignore_ascii_case(ext) {
                return true;
            }
        }
    }
    false
}

/// Create `dir` (and its parents) and narrow it to `0700`. For every
/// directory that holds user text or paths the editor acts on — what's
/// inside may be a private file's contents, so only this user may look,
/// and a directory nobody else can enter is one nobody else can plant
/// files in. Narrowed on every call, not only on first create, so a
/// directory an older build made at `0755` is tightened the next time
/// it's used.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Write `bytes` to `path` so that a write failing partway — a full disk, a
/// dropped mount — leaves the old file whole rather than truncated: the bytes
/// go to a temp file beside the target, are synced, and are renamed over it.
///
/// A rename replaces the directory entry, so the ways it would change the file
/// rather than its contents are handled here. A symlink is written through to
/// what it names, and the temp file is created with the target's permissions
/// and given its owner and group. Where the rename would still change the file,
/// it's written in place instead, keeping the inode: other hard links (a rename
/// would split them), an owner or group this user can't give the temp file
/// (`sudo binvim` on someone else's file), a directory the temp file can't be
/// created in (a file the user may write but not replace), and a rename that
/// fails. ACLs and extended
/// attributes aren't carried over.
///
/// The temp file sits beside the target, possibly in a directory other users
/// can write, so it's created exclusively under a name they can't predict: a
/// symlink or file planted there makes the create fail rather than redirecting
/// the write, and its mode is right from the moment it exists.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_with(path, bytes, |from, to| std::fs::rename(from, to))
}

/// `write_atomic` with the rename passed in, so a test can make it fail.
fn write_atomic_with(
    path: &Path,
    bytes: &[u8],
    rename: fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
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
    if existing.is_some() {
        // A rename only needs the directory to be writable, so without this a
        // read-only file would be replaced. Asking to open it for writing —
        // what writing in place needed — keeps it refusing as it always did.
        std::fs::OpenOptions::new().write(true).open(&target)?;
    }
    let (tmp, file) = match create_temp_beside(&target, existing.as_ref()) {
        Ok(created) => created,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return std::fs::write(&target, bytes);
        }
        Err(e) => return Err(e),
    };
    if let Some(meta) = &existing {
        if !take_owner(&file, meta) {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            return std::fs::write(&target, bytes);
        }
    }
    let written = (|| {
        let mut file = file;
        if let Some(meta) = &existing {
            // The create applied the mode through the umask; this restores
            // any bits it took away.
            file.set_permissions(meta.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()
        // Closed here, before the rename — Windows won't rename an open file.
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if rename(&tmp, &target).is_err() {
        let _ = std::fs::remove_file(&tmp);
        // The bytes all fit, so the disk isn't the problem — the target just
        // can't be replaced: a file another Windows program holds open, a
        // single-file bind mount (EBUSY). Writing in place still works there,
        // as saves always did.
        return std::fs::write(&target, bytes);
    }
    Ok(())
}

/// Exclusively create a temp file in `target`'s directory, retrying under a
/// new name if one is taken.
fn create_temp_beside(
    target: &Path,
    existing: Option<&std::fs::Metadata>,
) -> std::io::Result<(PathBuf, std::fs::File)> {
    create_temp_with_suffix(target, existing, "tmp")
}

/// `create_temp_beside` for a tool that has to see the file's real
/// extension (an in-place formatter): the unpredictable-name and
/// exclusive-create guarantees are the same, only the final suffix is
/// `ext` instead of `tmp`, and the temp copies the target's mode when the
/// target exists so a 0600 file's contents aren't briefly world-readable.
pub(crate) fn create_temp_with_ext(
    target: &Path,
    ext: &str,
) -> std::io::Result<(PathBuf, std::fs::File)> {
    let existing = std::fs::metadata(target).ok();
    create_temp_with_suffix(target, existing.as_ref(), ext)
}

fn create_temp_with_suffix(
    target: &Path,
    existing: Option<&std::fs::Metadata>,
    suffix: &str,
) -> std::io::Result<(PathBuf, std::fs::File)> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = target.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name")
    })?;
    let mut last = None;
    for _ in 0..16 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let tmp = target.with_file_name(format!(
            ".{}.binvim-{}-{nanos:08x}-{}.{suffix}",
            name.to_string_lossy(),
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        match create_exclusive(&tmp, existing) {
            Ok(file) => return Ok((tmp, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no free temp file name")))
}

/// `O_CREAT | O_EXCL`, which fails on an existing path — a symlink included,
/// since an exclusive create never follows one.
fn create_exclusive(
    path: &Path,
    existing: Option<&std::fs::Metadata>,
) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(existing.map_or(0o666, |m| m.permissions().mode() & 0o7777));
    }
    #[cfg(not(unix))]
    let _ = existing;
    options.open(path)
}

/// Give the temp file `target`'s owner and group. False when this user can't,
/// so the caller writes in place rather than change who owns the file.
#[cfg(unix)]
fn take_owner(file: &std::fs::File, target: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = file.metadata() else {
        return false;
    };
    if meta.uid() == target.uid() && meta.gid() == target.gid() {
        return true;
    }
    std::os::unix::fs::fchown(file, Some(target.uid()), Some(target.gid())).is_ok()
}

#[cfg(not(unix))]
fn take_owner(_: &std::fs::File, _: &std::fs::Metadata) -> bool {
    true
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
    fn path_key_is_pinned() {
        // Session files already on disk are named by this value; a change
        // would orphan every one of them.
        assert_eq!(path_key(Path::new("/tmp/project")), "ebab4cfadaecf751");
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
    fn a_failed_rename_writes_the_file_in_place() {
        use std::os::unix::fs::MetadataExt;
        let dir = scratch("renamefails");
        let file = dir.join("a.txt");
        std::fs::write(&file, "old").unwrap();
        let inode = std::fs::metadata(&file).unwrap().ino();
        write_atomic_with(&file, b"new", |_, _| Err(std::io::Error::other("busy"))).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new");
        assert_eq!(std::fs::metadata(&file).unwrap().ino(), inode);
        assert!(leftover_temp_files(&dir).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn an_exclusive_create_refuses_a_planted_symlink() {
        let dir = scratch("planted");
        let victim = dir.join("victim.txt");
        std::fs::write(&victim, "untouched").unwrap();
        let planted = dir.join(".a.txt.tmp");
        std::os::unix::fs::symlink(&victim, &planted).unwrap();
        let err = create_exclusive(&planted, None).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
    }

    #[cfg(unix)]
    #[test]
    fn marker_roots_others_could_plant_are_passed_over() {
        use std::os::unix::fs::PermissionsExt;
        let set = |p: &Path, mode: u32| {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap()
        };
        let root = scratch("markerwalk");
        std::fs::create_dir_all(root.join("shared/project")).unwrap();
        let root = root.canonicalize().unwrap();
        let project = root.join("shared/project");
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("shared/Cargo.toml"), "").unwrap();
        std::fs::write(root.join("shared/App.sln"), "").unwrap();
        set(&root, 0o755);
        set(&root.join("shared"), 0o777);
        let markers = ["Cargo.toml", "*.sln"];
        // Both nested candidates (plain and `*.ext`) sit in a directory
        // others can write — the walk goes past them to the next one up.
        assert_eq!(find_marker_root(&project, &markers), Some(root.clone()));
        // A marker file others can write is just as plantable.
        std::fs::write(project.join("Cargo.toml"), "").unwrap();
        set(&project.join("Cargo.toml"), 0o666);
        assert_eq!(find_marker_root(&project, &markers), Some(root.clone()));
        set(&project.join("Cargo.toml"), 0o644);
        assert_eq!(find_marker_root(&project, &markers), Some(project.clone()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn find_marker_root_without_a_match_is_none_and_the_fallback_is_start() {
        let dir = scratch("markerfall");
        let markers = ["binvim-no-such-marker.xyz"];
        assert_eq!(find_marker_root(&dir, &markers), None);
        assert_eq!(
            find_marker_root_or_start(&dir, &markers),
            dir.canonicalize().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_private_dir_narrows_a_wider_existing_dir() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("private").join("inner");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        create_private_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn create_temp_with_ext_lands_beside_the_target_with_its_extension() {
        let dir = scratch("toolext");
        let target = dir.join("Program.cs");
        std::fs::write(&target, "class A {}").unwrap();
        let (a, _fa) = create_temp_with_ext(&target, "cs").unwrap();
        let (b, _fb) = create_temp_with_ext(&target, "cs").unwrap();
        assert_ne!(a, b);
        for p in [&a, &b] {
            assert_eq!(p.parent(), target.parent());
            assert_eq!(p.extension().and_then(|e| e.to_str()), Some("cs"));
            assert!(p.file_name().unwrap().to_string_lossy().starts_with('.'));
            assert!(p.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn create_temp_with_ext_copies_the_target_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("toolmode");
        let target = dir.join("secret.php");
        std::fs::write(&target, "x").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        let (tmp, _f) = create_temp_with_ext(&target, "php").unwrap();
        let mode = std::fs::metadata(&tmp).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_gives_a_new_file_the_usual_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("newmode");
        let plain = dir.join("plain.txt");
        std::fs::write(&plain, "x").unwrap();
        let atomic = dir.join("atomic.txt");
        write_atomic(&atomic, b"x").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&atomic), mode(&plain));
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_refuses_a_read_only_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("readonlyfile");
        let file = dir.join("locked.txt");
        std::fs::write(&file, "old").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
        // Root may write a read-only file, so there's nothing to refuse.
        if std::fs::OpenOptions::new().write(true).open(&file).is_ok() {
            return;
        }
        let err = write_atomic(&file, b"new").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "old");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_keeps_the_owner_and_group() {
        use std::os::unix::fs::MetadataExt;
        let dir = scratch("owner");
        let file = dir.join("a.txt");
        std::fs::write(&file, "old").unwrap();
        let before = std::fs::metadata(&file).unwrap();
        write_atomic(&file, b"new").unwrap();
        let after = std::fs::metadata(&file).unwrap();
        assert_eq!((after.uid(), after.gid()), (before.uid(), before.gid()));
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

    #[cfg(unix)]
    #[test]
    fn others_can_plant_checks_every_step_down_to_the_candidate() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("plant");
        let set = |p: &Path, mode| {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        let bin = dir.join("node_modules/.bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("tool"), "").unwrap();
        for p in [&dir, &dir.join("node_modules"), &bin, &bin.join("tool")] {
            set(p, 0o755);
        }
        let rel = "node_modules/.bin/tool";
        assert!(!others_can_plant(&dir, rel));
        set(&dir, 0o777);
        assert!(others_can_plant(&dir, rel));
        set(&dir, 0o755);
        set(&dir.join("node_modules"), 0o777);
        assert!(others_can_plant(&dir, rel));
        set(&dir.join("node_modules"), 0o755);
        set(&bin.join("tool"), 0o757);
        assert!(others_can_plant(&dir, rel));
        set(&bin.join("tool"), 0o775);
        assert!(
            !others_can_plant(&dir, rel),
            "group-writable is still trusted"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn trusted_owner_takes_the_owner_before_the_bits() {
        let (me, root, other) = (Some(501), 0, 502);
        assert!(
            trusted_owner(501, 0o777, me, root),
            "the user's own 0777 mount"
        );
        assert!(trusted_owner(root, 0o755, me, root));
        assert!(!trusted_owner(root, 0o1777, me, root), "/tmp");
        assert!(
            !trusted_owner(other, 0o755, me, root),
            "another user's 0755 directory"
        );
        assert!(
            !trusted_owner(501, 0o755, None, root),
            "no uid for this user"
        );
    }
}
