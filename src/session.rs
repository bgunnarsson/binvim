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

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub cwd: String,
    pub buffers: Vec<SessionBuffer>,
    pub active: usize,
    /// Ex command history (`:`) — oldest first, most recent at the
    /// end. Capped at `CMDLINE_HISTORY_CAP` on save. Defaulted so old
    /// session files without this field keep parsing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cmd_history: Vec<String>,
    /// Search query history (`/` / `?`) — oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_history: Vec<String>,
    /// Recorded macros — register name → key stream. Persisted in a
    /// serde-friendly shape so the in-memory `Vec<KeyEvent>` survives a
    /// restart. Defaulted for forward-compat with old session files.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub macros: HashMap<char, Vec<SessionKey>>,
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

fn is_zero_u8(n: &u8) -> bool {
    *n == 0
}

/// Serde-friendly snapshot of one `crossterm::event::KeyEvent`. We can't
/// derive `Serialize` on the upstream type, so this carries a tagged
/// `KeyCode` plus the modifier bitset (matching `KeyModifiers::bits()`).
/// Variants outside `SessionKeyCode` (kitty-keyboard release/repeat,
/// media keys, etc.) are dropped on save — macros that recorded one
/// silently lose that key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionKey {
    pub code: SessionKeyCode,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub mods: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "k", content = "v")]
pub enum SessionKeyCode {
    Char(char),
    F(u8),
    Backspace,
    BackTab,
    Delete,
    Down,
    End,
    Enter,
    Esc,
    Home,
    Insert,
    Left,
    PageDown,
    PageUp,
    Right,
    Tab,
    Up,
}

impl SessionKey {
    /// Best-effort capture of a `KeyEvent` for persistence. Returns `None`
    /// for variants we can't round-trip (media keys, kitty-keyboard
    /// `Modifier`/`Release`/`Repeat` events). Callers drop those — a macro
    /// is a stream, not a structure, so missing keys just shorten it.
    pub fn from_event(k: &KeyEvent) -> Option<Self> {
        let code = match k.code {
            KeyCode::Char(c) => SessionKeyCode::Char(c),
            KeyCode::F(n) => SessionKeyCode::F(n),
            KeyCode::Backspace => SessionKeyCode::Backspace,
            KeyCode::BackTab => SessionKeyCode::BackTab,
            KeyCode::Delete => SessionKeyCode::Delete,
            KeyCode::Down => SessionKeyCode::Down,
            KeyCode::End => SessionKeyCode::End,
            KeyCode::Enter => SessionKeyCode::Enter,
            KeyCode::Esc => SessionKeyCode::Esc,
            KeyCode::Home => SessionKeyCode::Home,
            KeyCode::Insert => SessionKeyCode::Insert,
            KeyCode::Left => SessionKeyCode::Left,
            KeyCode::PageDown => SessionKeyCode::PageDown,
            KeyCode::PageUp => SessionKeyCode::PageUp,
            KeyCode::Right => SessionKeyCode::Right,
            KeyCode::Tab => SessionKeyCode::Tab,
            KeyCode::Up => SessionKeyCode::Up,
            _ => return None,
        };
        Some(SessionKey {
            code,
            mods: k.modifiers.bits(),
        })
    }

    pub fn to_event(&self) -> KeyEvent {
        let code = match self.code {
            SessionKeyCode::Char(c) => KeyCode::Char(c),
            SessionKeyCode::F(n) => KeyCode::F(n),
            SessionKeyCode::Backspace => KeyCode::Backspace,
            SessionKeyCode::BackTab => KeyCode::BackTab,
            SessionKeyCode::Delete => KeyCode::Delete,
            SessionKeyCode::Down => KeyCode::Down,
            SessionKeyCode::End => KeyCode::End,
            SessionKeyCode::Enter => KeyCode::Enter,
            SessionKeyCode::Esc => KeyCode::Esc,
            SessionKeyCode::Home => KeyCode::Home,
            SessionKeyCode::Insert => KeyCode::Insert,
            SessionKeyCode::Left => KeyCode::Left,
            SessionKeyCode::PageDown => KeyCode::PageDown,
            SessionKeyCode::PageUp => KeyCode::PageUp,
            SessionKeyCode::Right => KeyCode::Right,
            SessionKeyCode::Tab => KeyCode::Tab,
            SessionKeyCode::Up => KeyCode::Up,
        };
        KeyEvent::new(code, KeyModifiers::from_bits_truncate(self.mods))
    }
}

/// `<cache>/binvim/sessions/<hash>.json` for the given cwd. Returns `None`
/// if the cache dir can't be resolved.
pub fn session_path(cwd: &Path) -> Option<PathBuf> {
    // Tests build their editor through `App::new(None)`, which would restore
    // the session a real run left for this checkout — and a restored buffer
    // made the fold tests fail for as long as that file existed.
    if cfg!(test) {
        return None;
    }
    let canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    // A collision would only restore the wrong session, which the cwd check
    // inside `load_for_cwd` catches.
    let key = crate::paths::path_key(&canon);
    let mut p = crate::paths::cache_dir()?;
    p.push("sessions");
    p.push(format!("{key}.json"));
    Some(p)
}

pub fn save(session: &Session) -> std::io::Result<()> {
    let Some(path) = session_path(Path::new(&session.cwd)) else {
        return Ok(());
    };
    write_to(&path, session)
}

/// `save` against an explicit path — the seam tests write through, since
/// `session_path` is `None` under `cfg!(test)`.
fn write_to(path: &Path, session: &Session) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        // Sessions carry `:` and `/` history and recorded macros — anything
        // the user typed, occasionally a pasted secret — so the directory is
        // private like recovery's and undo's.
        crate::paths::create_private_dir(parent)?;
    }
    let json = serde_json::to_string_pretty(session)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::paths::write_atomic(path, json.as_bytes())
}

/// Save `session`, or remove this cwd's saved one when there's nothing worth
/// restoring (no buffers AND no histories). Buffers-empty-but-history-non-empty
/// still saves: the `<leader>bA` flow shouldn't wipe `:` / `/` recall, and
/// `hydrate_from_session` already tolerates a session whose tracked files have
/// all been deleted.
pub fn save_or_clear(session: &Session) -> std::io::Result<()> {
    if !session.buffers.is_empty()
        || !session.cmd_history.is_empty()
        || !session.search_history.is_empty()
        || !session.macros.is_empty()
    {
        save(session)
    } else {
        clear_for_cwd(Path::new(&session.cwd))
    }
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
    load_from(&session_path(cwd)?, cwd)
}

/// `load_for_cwd` against an explicit path.
fn load_from(path: &Path, cwd: &Path) -> Option<Session> {
    let text = std::fs::read_to_string(path).ok()?;
    let session: Session = serde_json::from_str(&text).ok()?;
    let canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    if session.cwd != canon.to_string_lossy() {
        return None;
    }
    Some(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_in(cwd: &Path, paths: &[&str]) -> Session {
        Session {
            cwd: cwd.to_string_lossy().into_owned(),
            buffers: paths
                .iter()
                .map(|p| SessionBuffer {
                    path: (*p).into(),
                    line: 3,
                    col: 1,
                    view_top: 0,
                    jumplist: vec![(1, 0)],
                    jump_idx: 1,
                })
                .collect(),
            active: 0,
            cmd_history: vec!["w".into()],
            search_history: vec!["foo".into()],
            macros: HashMap::new(),
        }
    }

    #[test]
    fn a_saved_session_loads_back_for_its_cwd() {
        let cwd = crate::paths::test_scratch_dir("session", "round-trip");
        let file = cwd.join("s.json");
        write_to(&file, &session_in(&cwd, &["/p/a.rs", "/p/b.rs"])).unwrap();
        let back = load_from(&file, &cwd).unwrap();
        let paths: Vec<&str> = back.buffers.iter().map(|b| b.path.as_str()).collect();
        assert_eq!(paths, ["/p/a.rs", "/p/b.rs"]);
        assert_eq!((back.buffers[0].line, back.buffers[0].col), (3, 1));
        assert_eq!(back.buffers[0].jumplist, [(1, 0)]);
        assert_eq!(back.cmd_history, ["w"]);
        assert_eq!(back.search_history, ["foo"]);
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn a_truncated_or_garbage_session_file_loads_as_none() {
        let cwd = crate::paths::test_scratch_dir("session", "corrupt");
        let file = cwd.join("s.json");
        write_to(&file, &session_in(&cwd, &["/p/a.rs"])).unwrap();
        let text = std::fs::read(&file).unwrap();
        std::fs::write(&file, &text[..text.len() / 2]).unwrap();
        assert!(load_from(&file, &cwd).is_none());
        std::fs::write(&file, [0xff, 0x00, b'[']).unwrap();
        assert!(load_from(&file, &cwd).is_none());
        assert!(load_from(&cwd.join("missing.json"), &cwd).is_none());
        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn a_session_saved_for_another_cwd_is_refused() {
        let cwd = crate::paths::test_scratch_dir("session", "cwd-a");
        let other = crate::paths::test_scratch_dir("session", "cwd-b");
        let file = cwd.join("s.json");
        write_to(&file, &session_in(&other, &["/p/a.rs"])).unwrap();
        assert!(load_from(&file, &cwd).is_none());
        assert!(load_from(&file, &other).is_some());
        std::fs::remove_dir_all(&cwd).ok();
        std::fs::remove_dir_all(&other).ok();
    }
}
