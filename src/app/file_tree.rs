//! Built-in sidebar tree file explorer. Enabled via
//! `[file_explorer] tree = true` in `~/.config/binvim/config.toml`;
//! when enabled, `<leader>e` toggles this pane instead of shelling
//! out to yazi. The pane sits flush against the left edge of the
//! editor band; `editor_rect()` trims width from the left so the
//! buffer panes (and the right-side AI terminal pane, if open) sit
//! cleanly to its right.
//!
//! State is a flat list of visible `TreeEntry`s rebuilt on every
//! expand / collapse. Hidden entries (dotfiles other than the
//! workspace's `.git`'s parent dir) are skipped by default — same
//! convention yazi / ripgrep use. The cursor + scroll are pane-
//! local; entering / leaving the pane never touches the buffer's
//! cursor.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::mode::Mode;

/// One row in the sidebar — either a directory or a file. Depth
/// drives the indent the renderer uses; `is_dir` selects icon /
/// expand-marker; the expanded state for directories is tracked
/// in `FileTreeState.expanded` (keyed by canonical path) so two
/// passes of `rebuild` over the same set of expansions yield the
/// same flat list.
#[derive(Debug, Clone)]
pub struct TreeEntry {
    pub path: PathBuf,
    pub depth: usize,
    pub is_dir: bool,
}

#[derive(Debug)]
pub struct FileTreeState {
    pub root: PathBuf,
    pub entries: Vec<TreeEntry>,
    pub expanded: HashSet<PathBuf>,
    pub cursor: usize,
    /// True after a leader (`<space>`) press inside the pane. The
    /// next keystroke is consumed as a leader-prefix command (today
    /// only `e` is handled — closes the pane). Reset on any other
    /// key.
    pub awaiting_leader: bool,
    /// In-flight file operation. `Create` / `Rename` swap mode to
    /// `Prompt`; the cmdline carries the user's typed input. `DeleteConfirm`
    /// stays inside `Mode::FileTree` and consumes the next key as
    /// `y` (confirm) / anything else (cancel). `None` whenever no
    /// op is mid-flight.
    pub pending_op: Option<FileTreePendingOp>,
}

#[derive(Debug, Clone)]
pub enum FileTreePendingOp {
    /// Awaiting prompt input — typed text becomes the new entry
    /// under `parent`. Trailing `/` → directory, else regular file.
    Create { parent: PathBuf },
    /// Awaiting prompt input — typed text becomes the new basename
    /// for `from` (kept in the same parent dir).
    Rename { from: PathBuf },
    /// Awaiting in-pane confirm (`y`/`Y`) before unlinking `target`.
    /// `is_dir` toggles `remove_dir_all` vs `remove_file`.
    DeleteConfirm { target: PathBuf, is_dir: bool },
}

impl FileTreeState {
    pub fn new(root: PathBuf) -> Self {
        let mut s = Self {
            root,
            entries: Vec::new(),
            expanded: HashSet::new(),
            cursor: 0,
            awaiting_leader: false,
            pending_op: None,
        };
        s.rebuild();
        s
    }

    /// Look up the path the cursor is sitting on, if any. Returns
    /// `None` when the visible list is empty (e.g. a freshly-emptied
    /// root) or the cursor index is somehow out of bounds.
    pub fn cursor_path(&self) -> Option<PathBuf> {
        self.entries.get(self.cursor).map(|e| e.path.clone())
    }

    /// The directory new entries created via `a` land in. If the
    /// cursor sits on a folder, that folder is the parent; otherwise
    /// it's the cursor entry's own parent. Falls back to `root` when
    /// the visible list is empty.
    pub fn parent_for_new_entry(&self) -> PathBuf {
        match self.entries.get(self.cursor) {
            Some(e) if e.is_dir => e.path.clone(),
            Some(e) => e
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root.clone()),
            None => self.root.clone(),
        }
    }

    /// Recompute the visible flat list from `expanded` + the
    /// filesystem. Called on init, on every expand / collapse, and
    /// after any external event that might have moved files (we
    /// don't watch for those today — `R` rebuilds on demand).
    pub fn rebuild(&mut self) {
        let mut out = Vec::new();
        push_children(&self.root, 0, &self.expanded, &mut out);
        self.entries = out;
        if self.cursor >= self.entries.len() {
            self.cursor = self.entries.len().saturating_sub(1);
        }
    }

    pub fn move_cursor(&mut self, delta: i64) {
        let len = self.entries.len();
        if len == 0 {
            return;
        }
        let new = (self.cursor as i64 + delta)
            .max(0)
            .min(len as i64 - 1) as usize;
        self.cursor = new;
    }
}

fn push_children(
    dir: &Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    out: &mut Vec<TreeEntry>,
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<(PathBuf, bool, String)> = Vec::new();
    for e in rd.flatten() {
        let path = e.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if is_hidden(&name) {
            continue;
        }
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        items.push((path, is_dir, name));
    }
    // Directories first, then files, each group alphabetised.
    items.sort_by(|a, b| match (a.1, b.1) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.2.to_lowercase().cmp(&b.2.to_lowercase()),
    });
    for (path, is_dir, _) in items {
        out.push(TreeEntry {
            path: path.clone(),
            depth,
            is_dir,
        });
        if is_dir && expanded.contains(&path) {
            push_children(&path, depth + 1, expanded, out);
        }
    }
}

fn is_hidden(name: &str) -> bool {
    // Dotfiles by default. `.git` itself is hidden, but everything
    // else in a project (e.g. `.editorconfig`, `.env.example`) is
    // useful enough to want visible — so we hide *only* `.git`.
    // Other dotfiles stay visible. This mirrors how most modern
    // file-tree plugins behave out of the box.
    name == ".git"
}

impl super::App {
    /// Entry point for `<leader>e` when `[file_explorer] tree = true`.
    /// Three-state cycle: closed → open + focused → open but editor
    /// focused → closed. Lets a click into the editor drop focus
    /// without losing the tree, and `<leader>e` then pulls focus
    /// back without having to reopen the pane.
    pub(super) fn toggle_file_tree(&mut self) {
        if self.file_tree.is_none() {
            self.open_file_tree();
        } else if matches!(self.mode, Mode::FileTree) {
            self.close_file_tree();
        } else {
            self.mode = Mode::FileTree;
        }
    }

    pub(super) fn open_file_tree(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut state = FileTreeState::new(cwd);
        // Seed the cursor on the active buffer's path if the file
        // sits anywhere inside the tree — easier to land in context
        // when you reopen the pane mid-edit.
        if let Some(active_path) = self.buffer.path.clone() {
            seed_cursor_on_path(&mut state, &active_path);
        }
        self.file_tree = Some(state);
        self.mode = Mode::FileTree;
    }

    pub(super) fn close_file_tree(&mut self) {
        self.file_tree = None;
        if matches!(self.mode, Mode::FileTree) {
            self.mode = Mode::Normal;
        }
    }

    pub(super) fn handle_file_tree_key(&mut self, key: KeyEvent) {
        let Some(state) = self.file_tree.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        // Delete-confirm: the previous `d` set up a DeleteConfirm; the
        // next key is interpreted as y/N. Any non-`y` cancels and
        // falls back to the normal handler (so `d` then `j` reads as
        // "cancel + move down" — destructive ops never get to "you
        // just typed it twice by accident").
        if let Some(FileTreePendingOp::DeleteConfirm { .. }) = state.pending_op {
            let confirmed = matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y'));
            self.finish_file_tree_delete(confirmed);
            return;
        }
        // Leader (`<space>`) prefix handling — mirrors the editor's
        // leader so `<leader>e` toggles the tree closed even with
        // the pane focused. A second key after space is consumed as
        // the leader command; unknown keys just reset.
        if state.awaiting_leader {
            state.awaiting_leader = false;
            if matches!(key.code, KeyCode::Char('e')) {
                self.close_file_tree();
            }
            return;
        }
        if matches!(key.code, KeyCode::Char(' ')) {
            state.awaiting_leader = true;
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.close_file_tree();
            }
            KeyCode::Down => state.move_cursor(1),
            KeyCode::Up => state.move_cursor(-1),
            KeyCode::Char('j') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.move_cursor(1);
            }
            KeyCode::Char('k') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.move_cursor(-1);
            }
            KeyCode::Char('g') => {
                state.cursor = 0;
            }
            KeyCode::Char('G') => {
                state.cursor = state.entries.len().saturating_sub(1);
            }
            KeyCode::Char('R') => {
                state.rebuild();
            }
            KeyCode::Char('a') => self.start_file_tree_create(),
            KeyCode::Char('r') => self.start_file_tree_rename(),
            KeyCode::Char('d') => self.start_file_tree_delete(),
            KeyCode::Char('h') | KeyCode::Left => {
                self.file_tree_collapse_or_parent();
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                self.file_tree_open_or_expand();
            }
            _ => {}
        }
    }

    /// Open the Create prompt. Pre-fills nothing — the user types the
    /// new basename. Trailing `/` makes it a directory.
    pub(super) fn start_file_tree_create(&mut self) {
        let Some(state) = self.file_tree.as_mut() else { return };
        let parent = state.parent_for_new_entry();
        state.pending_op = Some(FileTreePendingOp::Create {
            parent: parent.clone(),
        });
        self.cmdline = String::new();
        self.mode = Mode::Prompt(crate::mode::PromptKind::FileTreeCreate);
        // Surface the parent so the user can see where the new entry
        // will land before typing.
        let label = parent
            .strip_prefix(&self.file_tree.as_ref().unwrap().root)
            .ok()
            .and_then(|p| p.to_str())
            .filter(|s| !s.is_empty())
            .map(|s| format!("{s}/"))
            .unwrap_or_else(|| "./".into());
        self.status_msg = format!("new entry in {label} (trailing `/` for dir)");
    }

    /// Open the Rename prompt. Pre-fills with the current basename
    /// so common renames are a few-char edit.
    pub(super) fn start_file_tree_rename(&mut self) {
        let Some(state) = self.file_tree.as_mut() else { return };
        let Some(target) = state.cursor_path() else { return };
        let basename = target
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        state.pending_op = Some(FileTreePendingOp::Rename {
            from: target.clone(),
        });
        self.cmdline = basename;
        self.mode = Mode::Prompt(crate::mode::PromptKind::FileTreeRename);
    }

    /// Snapshot of the active delete-confirm, if one is armed —
    /// `(basename, is_dir)`. Used by the renderer to populate the
    /// confirmation popup without exposing the private
    /// `FileTreePendingOp` enum across the module boundary.
    pub fn file_tree_pending_delete(&self) -> Option<(String, bool)> {
        let state = self.file_tree.as_ref()?;
        match &state.pending_op {
            Some(FileTreePendingOp::DeleteConfirm { target, is_dir }) => {
                let name = target
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
                    .to_string();
                Some((name, *is_dir))
            }
            _ => None,
        }
    }

    /// Arm a delete-confirm on the cursor entry. The next key in the
    /// file-tree handler is interpreted as `y` (confirm) / anything
    /// else (cancel). The question itself renders as a popup over
    /// the editor (see `draw_file_tree_confirm`) — same chrome as
    /// the create / rename prompts, so the three file-tree ops feel
    /// uniform. We deliberately don't push to `status_msg`; popup
    /// owns the prompt, notification line owns "operation result"
    /// (`deleted X` / `delete: cancelled`).
    pub(super) fn start_file_tree_delete(&mut self) {
        let Some(state) = self.file_tree.as_mut() else { return };
        let Some(target) = state.cursor_path() else { return };
        let is_dir = state
            .entries
            .get(state.cursor)
            .map(|e| e.is_dir)
            .unwrap_or(false);
        state.pending_op = Some(FileTreePendingOp::DeleteConfirm {
            target,
            is_dir,
        });
    }

    /// Commit a Create prompt. Trailing `/` selects directory; an
    /// empty / whitespace-only input is rejected. Errors surface
    /// through `status_msg`; on success the parent dir is auto-
    /// expanded so the new entry is visible.
    pub(super) fn finish_file_tree_create(&mut self, input: String) {
        let Some(state) = self.file_tree.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        let parent = match state.pending_op.take() {
            Some(FileTreePendingOp::Create { parent }) => parent,
            other => {
                state.pending_op = other;
                self.mode = Mode::FileTree;
                return;
            }
        };
        let trimmed = input.trim();
        if trimmed.is_empty() {
            self.status_msg = "create: empty name".into();
            self.mode = Mode::FileTree;
            return;
        }
        // Refuse names that try to escape the parent. Bare `/` /
        // segments containing `..` are easy to fat-finger and we don't
        // want them silently writing outside the project.
        if trimmed.contains("..") || trimmed.starts_with('/') {
            self.status_msg = "create: refused (path escapes parent)".into();
            self.mode = Mode::FileTree;
            return;
        }
        let is_dir = trimmed.ends_with('/');
        let name = trimmed.trim_end_matches('/');
        let target = parent.join(name);
        let result = if is_dir {
            std::fs::create_dir_all(&target)
        } else {
            // Ensure any intermediate dirs in `name` exist (so `a` →
            // `foo/bar.txt` works even when `foo/` doesn't yet).
            if let Some(p) = target.parent() {
                if let Err(e) = std::fs::create_dir_all(p) {
                    self.status_msg = format!("create: {e}");
                    self.mode = Mode::FileTree;
                    return;
                }
            }
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map(|_| ())
        };
        match result {
            Ok(()) => {
                if let Some(state) = self.file_tree.as_mut() {
                    state.expanded.insert(parent.clone());
                    state.rebuild();
                    // Land the cursor on the freshly-created entry so
                    // a follow-up `r` / `d` targets it without
                    // re-navigating.
                    if let Some(idx) =
                        state.entries.iter().position(|e| e.path == target)
                    {
                        state.cursor = idx;
                    }
                }
                let kind = if is_dir { "dir" } else { "file" };
                self.status_msg = format!("created {kind} {name}");
            }
            Err(e) => {
                self.status_msg = format!("create: {e}");
            }
        }
        self.mode = Mode::FileTree;
    }

    /// Commit a Rename prompt. Empty inputs (or no-op renames) are
    /// silently dropped. If the renamed entry was the currently-
    /// loaded buffer's path, the buffer's path is rewritten so saves
    /// continue to land in the right file.
    pub(super) fn finish_file_tree_rename(&mut self, input: String) {
        let Some(state) = self.file_tree.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        let from = match state.pending_op.take() {
            Some(FileTreePendingOp::Rename { from }) => from,
            other => {
                state.pending_op = other;
                self.mode = Mode::FileTree;
                return;
            }
        };
        let trimmed = input.trim();
        if trimmed.is_empty() {
            self.status_msg = "rename: empty name".into();
            self.mode = Mode::FileTree;
            return;
        }
        if trimmed.contains('/') || trimmed.contains("..") {
            self.status_msg = "rename: basename only (no `/`)".into();
            self.mode = Mode::FileTree;
            return;
        }
        let current_basename = from.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if trimmed == current_basename {
            self.mode = Mode::FileTree;
            return;
        }
        let Some(parent) = from.parent() else {
            self.status_msg = "rename: target has no parent".into();
            self.mode = Mode::FileTree;
            return;
        };
        let to = parent.join(trimmed);
        match std::fs::rename(&from, &to) {
            Ok(()) => {
                self.adopt_renamed_path(&from, &to);
                if let Some(state) = self.file_tree.as_mut() {
                    // Carry the expanded-state forward — if `from` was
                    // expanded as a directory, mark `to` expanded too.
                    if state.expanded.remove(&from) {
                        state.expanded.insert(to.clone());
                    }
                    state.rebuild();
                    if let Some(idx) =
                        state.entries.iter().position(|e| e.path == to)
                    {
                        state.cursor = idx;
                    }
                }
                self.status_msg = format!("renamed {current_basename} → {trimmed}");
            }
            Err(e) => {
                self.status_msg = format!("rename: {e}");
            }
        }
        self.mode = Mode::FileTree;
    }

    /// Consume the y/N confirmation for an armed delete. Cancels the
    /// pending op either way; only `y`/`Y` actually unlinks.
    pub(super) fn finish_file_tree_delete(&mut self, confirmed: bool) {
        let Some(state) = self.file_tree.as_mut() else { return };
        let (target, is_dir) = match state.pending_op.take() {
            Some(FileTreePendingOp::DeleteConfirm { target, is_dir }) => (target, is_dir),
            other => {
                state.pending_op = other;
                return;
            }
        };
        if !confirmed {
            self.status_msg = "delete: cancelled".into();
            return;
        }
        let result = if is_dir {
            std::fs::remove_dir_all(&target)
        } else {
            std::fs::remove_file(&target)
        };
        match result {
            Ok(()) => {
                let name = target
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
                    .to_string();
                if let Some(state) = self.file_tree.as_mut() {
                    state.expanded.remove(&target);
                    state.rebuild();
                }
                self.status_msg = format!("deleted {name}");
            }
            Err(e) => {
                self.status_msg = format!("delete: {e}");
            }
        }
    }

    /// When a file is renamed under the tree, rewrite the path on any
    /// buffer that still points at the old name. Walks both the
    /// active buffer and every stashed inactive buffer so an open tab
    /// that wasn't the focused one survives the move too.
    fn adopt_renamed_path(&mut self, from: &Path, to: &Path) {
        if self.buffer.path.as_deref() == Some(from) {
            self.buffer.path = Some(to.to_path_buf());
        }
        for stash in self.buffers.iter_mut() {
            if stash.buffer.path.as_deref() == Some(from) {
                stash.buffer.path = Some(to.to_path_buf());
            }
        }
    }

    pub(super) fn file_tree_activate_cursor(&mut self) {
        self.file_tree_open_or_expand();
    }

    fn file_tree_open_or_expand(&mut self) {
        let Some(state) = self.file_tree.as_mut() else { return };
        let Some(entry) = state.entries.get(state.cursor).cloned() else { return };
        if entry.is_dir {
            if state.expanded.contains(&entry.path) {
                state.expanded.remove(&entry.path);
            } else {
                state.expanded.insert(entry.path.clone());
            }
            state.rebuild();
            return;
        }
        // File — open it in the active editor window. Close the
        // pane after opening so the buffer can use the full width;
        // mirrors yazi's "pick + exit" flow.
        let path = entry.path;
        self.close_file_tree();
        if let Err(e) = self.open_buffer(path) {
            self.status_msg = format!("error: {e}");
        }
    }

    fn file_tree_collapse_or_parent(&mut self) {
        let Some(state) = self.file_tree.as_mut() else { return };
        let Some(entry) = state.entries.get(state.cursor).cloned() else { return };
        if entry.is_dir && state.expanded.contains(&entry.path) {
            state.expanded.remove(&entry.path);
            state.rebuild();
            return;
        }
        // On a file (or a collapsed dir) — jump the cursor up to the
        // entry's parent in the visible list, so successive `h` walks
        // up the tree the way you'd expect.
        if entry.depth == 0 {
            return;
        }
        let parent_depth = entry.depth - 1;
        let mut i = state.cursor;
        while i > 0 {
            i -= 1;
            if state.entries[i].depth == parent_depth && state.entries[i].is_dir {
                state.cursor = i;
                break;
            }
        }
    }
}

fn seed_cursor_on_path(state: &mut FileTreeState, target: &Path) {
    // Walk every ancestor between `target` and `state.root`,
    // mark it expanded so the file becomes visible, then put the
    // cursor on the file's row. Aborts silently if `target` isn't
    // under `root` (e.g. `/tmp/scratch.rs` opened from another cwd).
    let Ok(rel) = target.strip_prefix(&state.root) else {
        return;
    };
    let mut cur = state.root.clone();
    for component in rel.components() {
        let part = match component.as_os_str().to_str() {
            Some(p) => p,
            None => return,
        };
        cur.push(part);
        if cur == *target {
            break;
        }
        state.expanded.insert(cur.clone());
    }
    state.rebuild();
    if let Some(idx) = state.entries.iter().position(|e| e.path == target) {
        state.cursor = idx;
    }
}
