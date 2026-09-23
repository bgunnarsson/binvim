//! Recovery files: the text of a buffer with unsaved changes, written to
//! `<cache>/binvim/recover/<key>.json` every few seconds while it stays dirty,
//! so a crash, a `kill` or a closed terminal costs seconds of typing rather
//! than everything since the last `:w`. The `App` side — when they're written,
//! applied on open and removed — is `app/recover_glue.rs`.
//!
//! One file per path, keyed like sessions (`paths::path_key`). A `[No Name]`
//! buffer has no path to key on, so it gets an `unnamed-…` key of its own, and
//! nothing opens it on its own: `:recover` does, after a crash left one.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct RecoveryFile {
    /// The file the text belongs to, so a stray recovery file is still legible.
    pub path: String,
    /// Unix seconds when the text was written.
    pub saved_at: u64,
    pub text: String,
    /// The binvim that wrote it. While that process is alive the file is its
    /// live dump, not something left by a crash.
    #[serde(default)]
    pub pid: u32,
}

/// What a recovery file belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RecoveryKey {
    Path(PathBuf),
    Unnamed(String),
}

/// A key no other binvim's unnamed dump has: the pid alone would collide
/// with a crashed binvim's whose pid was handed on to this one.
pub fn new_unnamed_key() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "unnamed-{}-{}-{}",
        std::process::id(),
        now_secs(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Where recovery files live. `None` under test, like every other persisted
/// path, so tests never leave files in the real cache.
fn recover_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(crate::paths::cache_dir()?.join("recover"))
}

/// Where `file`'s recovery text lives.
pub fn recovery_path(file: &Path) -> Option<PathBuf> {
    Some(recover_dir()?.join(format!("{}.json", crate::paths::path_key(file))))
}

pub fn recovery_path_for(key: &RecoveryKey) -> Option<PathBuf> {
    match key {
        RecoveryKey::Path(file) => recovery_path(file),
        RecoveryKey::Unnamed(name) => Some(recover_dir()?.join(format!("{name}.json"))),
    }
}

/// The `[No Name]` dumps a crash left: parsed, and not another running
/// binvim's live one. Oldest first, so `:recover` opens them in the order
/// they were written.
pub fn crash_unnamed_dumps() -> Vec<(String, RecoveryFile)> {
    recover_dir().map_or_else(Vec::new, |dir| unnamed_dumps_in(&dir))
}

fn unnamed_dumps_in(dir: &Path) -> Vec<(String, RecoveryFile)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dumps: Vec<(String, RecoveryFile)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let key = name.strip_suffix(".json")?;
            if !key.starts_with("unnamed-") {
                return None;
            }
            let rec = load_from(&e.path())?;
            (!held_by_another_process(&rec)).then(|| (key.to_string(), rec))
        })
        .collect();
    dumps.sort_by_key(|(_, rec)| rec.saved_at);
    dumps
}

pub fn write_to(dest: &Path, rec: &RecoveryFile) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        // The text may be a private file's — a key, a `.env` — so only this
        // user may look inside, whatever the files' own modes come out as.
        crate::paths::create_private_dir(parent)?;
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

/// Written by another binvim that's still running — a second editor on the
/// same file, whose dump this one must neither apply nor remove.
pub fn held_by_another_process(rec: &RecoveryFile) -> bool {
    rec.pid != 0 && rec.pid != std::process::id() && process_alive(rec.pid)
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // Matched on the command name, not only on the pid being in use: a dead
    // binvim's pid goes to whatever starts next, and `kill -0` answers yes for
    // any process of ours, which would hold the dump back while it runs.
    // Without a name to match, the dump is taken to be a crash's.
    let Some(image) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_name().map(|n| n.to_string_lossy().into_owned()))
    else {
        return false;
    };
    std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|out| ps_comm_is(&String::from_utf8_lossy(&out.stdout), &image))
}

/// Whether `ps -o comm=` output names `image`. macOS prints the executable's
/// path and Linux its name cut to 15 bytes, so the last path component is
/// compared, against the cut name too.
#[cfg(any(unix, test))]
fn ps_comm_is(stdout: &str, image: &str) -> bool {
    let comm = stdout.trim();
    let comm = comm.rsplit('/').next().unwrap_or(comm);
    if comm.is_empty() {
        return false;
    }
    let cut = image.get(..15).unwrap_or(image);
    comm == image || (comm.len() == 15 && comm == cut)
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    // Matched on the image name too: `tasklist` lists every session's
    // processes, SYSTEM's included, and Windows hands a dead binvim's pid to
    // something else quickly, which would hold its dump back for as long as
    // that process runs. Without a name to match, the dump is taken to be a
    // crash's, as it was before the check existed.
    let Some(image) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_name().map(|n| n.to_string_lossy().into_owned()))
    else {
        return false;
    };
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|out| tasklist_lists(&String::from_utf8_lossy(&out.stdout), &image, pid))
}

/// Whether `tasklist /FO CSV /NH` output has a row for `image` running as
/// `pid` (`"binvim.exe","1234",…`). Matched on the row rather than the absence
/// of the "No tasks are running" line, which Windows translates.
#[cfg(any(windows, test))]
fn tasklist_lists(stdout: &str, image: &str, pid: u32) -> bool {
    let pid = pid.to_string();
    stdout.lines().any(|line| {
        // Image names can hold commas but never quotes, so the name and PID
        // are the first two quoted fields.
        let mut fields = line.trim().split('"').skip(1).step_by(2);
        fields
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case(image))
            && fields.next() == Some(pid.as_str())
    })
}

#[cfg(not(any(unix, windows)))]
fn process_alive(_: u32) -> bool {
    false
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
    fn only_parsed_unnamed_dumps_are_offered() {
        let dir = scratch("unnamed");
        let rec = |saved_at, text: &str| RecoveryFile {
            path: "[No Name]".into(),
            saved_at,
            text: text.into(),
            pid: 0,
        };
        write_to(&dir.join("unnamed-1-20-0.json"), &rec(20, "later")).unwrap();
        write_to(&dir.join("unnamed-1-10-1.json"), &rec(10, "earlier")).unwrap();
        write_to(&dir.join("0123abcd.json"), &rec(5, "a named file's")).unwrap();
        std::fs::write(dir.join("unnamed-1-30-2.json"), "{ cut short").unwrap();
        let found: Vec<(String, String)> = unnamed_dumps_in(&dir)
            .into_iter()
            .map(|(key, rec)| (key, rec.text))
            .collect();
        assert_eq!(
            found,
            [
                ("unnamed-1-10-1".to_string(), "earlier".to_string()),
                ("unnamed-1-20-0".to_string(), "later".to_string()),
            ]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unnamed_keys_are_this_process_s_and_never_repeat() {
        let (a, b) = (new_unnamed_key(), new_unnamed_key());
        assert!(a.starts_with(&format!("unnamed-{}-", std::process::id())));
        assert_ne!(a, b);
    }

    #[test]
    fn a_written_recovery_file_loads_back() {
        let dir = scratch("roundtrip");
        let dest = dir.join("nested").join("r.json");
        let rec = RecoveryFile {
            path: "/tmp/a.txt".into(),
            saved_at: 42,
            text: "unsaved\n".into(),
            pid: 7,
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
            pid: 0,
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
            pid: 0,
        };
        assert_eq!(recovered_text(&rec, "same\n"), None);
        assert_eq!(recovered_text(&rec, "older\n"), Some("same\n"));
    }

    #[test]
    fn a_file_without_a_pid_loads_as_left_by_a_crash() {
        let dir = scratch("nopid");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("r.json");
        std::fs::write(&dest, r#"{"path":"/tmp/a.txt","saved_at":4,"text":"x"}"#).unwrap();
        let rec = load_from(&dest).unwrap();
        assert_eq!(rec.pid, 0);
        assert!(!held_by_another_process(&rec));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ps_output_names_binvim_only_for_binvim() {
        assert!(ps_comm_is("/Users/me/.cargo/bin/binvim\n", "binvim"));
        assert!(ps_comm_is("binvim\n", "binvim"));
        // The pid is in use, but by something else: a crash's dump.
        assert!(!ps_comm_is("/bin/zsh\n", "binvim"));
        assert!(!ps_comm_is("binvim-dev\n", "binvim"));
        assert!(!ps_comm_is("", "binvim"));
        // Linux cuts `comm` to 15 bytes.
        assert!(ps_comm_is("binvim-nightly-\n", "binvim-nightly-build"));
        assert!(!ps_comm_is("binvim-nightly\n", "binvim-nightly-build"));
    }

    #[test]
    fn tasklist_output_lists_binvim_only_under_its_own_pid() {
        let row = "\"binvim.exe\",\"1234\",\"Console\",\"1\",\"12,345 K\"\r\n";
        assert!(tasklist_lists(row, "binvim.exe", 1234));
        assert!(tasklist_lists(row, "BINVIM.EXE", 1234));
        assert!(!tasklist_lists(row, "binvim.exe", 123));
        assert!(!tasklist_lists(row, "binvim.exe", 1));
        let reused = "\"svchost.exe\",\"1234\",\"Services\",\"0\",\"9,120 K\"";
        assert!(!tasklist_lists(reused, "binvim.exe", 1234));
        let comma_name = "\"a,b.exe\",\"77\",\"Console\",\"1\",\"1,024 K\"";
        assert!(tasklist_lists(comma_name, "a,b.exe", 77));
        let none = "INFO: No tasks are running which match the specified criteria.\r\n";
        assert!(!tasklist_lists(none, "binvim.exe", 1234));
        assert!(!tasklist_lists("", "binvim.exe", 1234));
    }

    #[cfg(unix)]
    #[test]
    fn a_dump_is_held_only_while_its_writer_runs() {
        // A `sleep` under this binary's name stands in for another binvim:
        // a link, since macOS kills a copy of a system binary.
        let dir = crate::paths::test_scratch_dir("recover", "held");
        let exe = std::env::current_exe().unwrap();
        let twin = dir.join(exe.file_name().unwrap());
        std::os::unix::fs::symlink("/bin/sleep", &twin).unwrap();
        let mut child = std::process::Command::new(&twin).arg("30").spawn().unwrap();
        let mut rec = RecoveryFile {
            path: "/tmp/a.txt".into(),
            saved_at: 0,
            text: "x".into(),
            pid: child.id(),
        };
        assert!(held_by_another_process(&rec));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!held_by_another_process(&rec));
        // A live process that isn't binvim, holding the pid a crash left.
        let mut other = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        rec.pid = other.id();
        assert!(!held_by_another_process(&rec));
        other.kill().unwrap();
        other.wait().unwrap();
        rec.pid = std::process::id();
        assert!(!held_by_another_process(&rec));
        std::fs::remove_dir_all(&dir).ok();
    }
}
