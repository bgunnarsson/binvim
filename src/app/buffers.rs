//! Buffer-stash machinery — switching, opening, deleting, and the
//! disk-watch reload loop. Plus persisted recents tracking that the file
//! picker reads from.

use anyhow::Result;
use ropey::Rope;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::buffer::Buffer;
use crate::cursor::Cursor;
use crate::undo::History;

use super::state::BufferStash;

pub(super) const RECENTS_CAP: usize = 100;
pub(super) const DISK_CHECK_INTERVAL: Duration = Duration::from_millis(1000);

impl super::App {
    pub(super) fn snapshot_active(&mut self) -> BufferStash {
        BufferStash {
            buffer: std::mem::take(&mut self.buffer),
            // Window-level fields are `Copy` — read them instead of
            // taking so the focused Window state stays intact on
            // `App.window` for a follow-up tab snapshot (switch_tab
            // needs to move the focused window into tab_windows with
            // its cursor/viewport preserved).
            cursor: self.window.cursor,
            view_top: self.window.view_top,
            view_left: self.window.view_left,
            history: std::mem::take(&mut self.history),
            visual_anchor: self.window.visual_anchor,
            marks: std::mem::take(&mut self.marks),
            jumplist: std::mem::take(&mut self.jumplist),
            jump_idx: std::mem::take(&mut self.jump_idx),
            highlight_cache: self.highlight_cache.take(),
            folds: std::mem::take(&mut self.folds),
            folds_version: std::mem::replace(&mut self.folds_version, u64::MAX),
            closed_folds: std::mem::take(&mut self.closed_folds),
            git_hunks: std::mem::take(&mut self.git_hunks),
            blame_visible: std::mem::take(&mut self.blame_visible),
            blame: std::mem::take(&mut self.blame),
            markdown_meta: self.markdown_meta.take(),
            // Layout state belongs to the tab, not the buffer — it
            // gets snapshotted separately via `snapshot_tab` when the
            // tab actually changes (H/L/:b), so the buffer-level
            // switch path (open_buffer / :e) keeps the current tab
            // intact.
            layout: None,
            tab_windows: std::collections::HashMap::new(),
            tab_active_window: None,
        }
    }

    fn load_stash(&mut self, stash: BufferStash) {
        self.buffer = stash.buffer;
        self.window.cursor = stash.cursor;
        self.window.view_top = stash.view_top;
        self.window.view_left = stash.view_left;
        self.history = stash.history;
        self.window.visual_anchor = stash.visual_anchor;
        self.marks = stash.marks;
        self.jumplist = stash.jumplist;
        self.jump_idx = stash.jump_idx;
        self.highlight_cache = stash.highlight_cache;
        self.folds = stash.folds;
        self.folds_version = stash.folds_version;
        self.closed_folds = stash.closed_folds;
        self.git_hunks = stash.git_hunks;
        self.blame_visible = stash.blame_visible;
        self.blame = stash.blame;
        self.markdown_meta = stash.markdown_meta;
    }

    /// Buffer-only swap: replace App's live buffer fields with the
    /// content of `idx`, but keep the current tab's layout intact.
    /// Used by `:e` and the file picker — the active pane's buffer
    /// changes but other panes (and the split structure) survive.
    pub(super) fn switch_to(&mut self, idx: usize) -> Result<()> {
        if idx >= self.buffers.len() {
            anyhow::bail!("invalid buffer index {idx}");
        }
        if idx == self.active {
            return Ok(());
        }
        let active = self.active;
        let snap = self.snapshot_active();
        self.buffers[active] = snap;
        let stash = std::mem::take(&mut self.buffers[idx]);
        self.load_stash(stash);
        self.active = idx;
        // Active window in the current tab now points at the new
        // buffer — keep the window's `buffer_idx` in sync so
        // per-window buffer-state lookups (BufferState routing in the
        // renderer, focus-change buffer swaps in window_focus /
        // window_close) agree with App.active.
        self.window.buffer_idx = idx;
        // Single-window tab: the tab's identity is just whichever
        // buffer is focused in it. `:e other.txt` from a non-split
        // view should update the tabline highlight to match, since
        // the user perceives it as "switched to other.txt's tab."
        // Multi-pane layouts keep their `active_tab` separately so
        // a tab the user explicitly built (e.g. via <C-w>v + picker)
        // stays highlighted even when focus moves between panes.
        if matches!(self.layout.root, crate::layout::LayoutNode::Leaf(_)) {
            self.active_tab = idx;
        }
        Ok(())
    }

    /// Tab swap: stash the current tab's layout + buffer content,
    /// load buffer `idx`'s tab. Used by `H`/`L`/`:b` so switching
    /// between buffers also switches between their independent split
    /// layouts. A tab that has never been visited is given a fresh
    /// single-leaf layout pointing at its buffer on first load.
    pub(super) fn switch_tab(&mut self, idx: usize) -> Result<()> {
        if idx >= self.buffers.len() {
            anyhow::bail!("invalid buffer index {idx}");
        }
        if idx == self.active_tab {
            // Already on this tab — fall through to a buffer-only
            // swap in case the user expects `:b N` to also re-focus
            // the primary pane on its own buffer.
            return self.switch_to(idx);
        }
        let outgoing_tab = self.active_tab;
        let outgoing_active = self.active;
        // 1. Stash buffer-level state of the focused buffer.
        let buf_snap = self.snapshot_active();
        self.buffers[outgoing_active] = buf_snap;
        // 2. Move App.window into self.windows so the full pane set
        //    is captured uniformly, then stash layout + windows +
        //    active_window onto the outgoing tab's BufferStash.
        self.windows
            .insert(self.active_window, std::mem::take(&mut self.window));
        let (placeholder, _) = crate::layout::Layout::new();
        let layout = std::mem::replace(&mut self.layout, placeholder);
        let tab_windows = std::mem::take(&mut self.windows);
        let tab_active_window = self.active_window;
        {
            let stash = &mut self.buffers[outgoing_tab];
            stash.layout = Some(layout);
            stash.tab_windows = tab_windows;
            stash.tab_active_window = Some(tab_active_window);
        }
        // 3. Pull out the incoming tab's stashed layout, or build a
        //    fresh single-leaf layout pointing at the incoming buffer.
        let incoming_layout = self.buffers[idx].layout.take();
        let incoming_aw = self.buffers[idx].tab_active_window.take();
        let mut incoming_windows = std::mem::take(&mut self.buffers[idx].tab_windows);
        let (new_layout, new_active_window, focused_buffer_idx) =
            match (incoming_layout, incoming_aw) {
                (Some(l), Some(aw)) => {
                    let focused = incoming_windows
                        .get(&aw)
                        .map(|w| w.buffer_idx)
                        .unwrap_or(idx);
                    (l, aw, focused)
                }
                _ => {
                    let (l, root) = crate::layout::Layout::new();
                    incoming_windows.insert(
                        root,
                        crate::window::Window {
                            buffer_idx: idx,
                            ..Default::default()
                        },
                    );
                    (l, root, idx)
                }
            };
        // 4. Load buffer content for the focused buffer first (may
        //    differ from `idx` if the previous focused pane was on a
        //    cross-tab buffer). `load_stash` also writes cursor /
        //    view_top / view_left from the buffer's last-known
        //    position onto `App.window` — step 5 then overrides them
        //    with the focused window's per-window cursor.
        let focused_stash = std::mem::take(&mut self.buffers[focused_buffer_idx]);
        self.load_stash(focused_stash);
        self.active = focused_buffer_idx;
        // 5. Lift the focused window out of the windows map onto
        //    App.window. The window state (cursor, viewport,
        //    visual_anchor, buffer_idx) replaces what load_stash
        //    wrote — cursor / viewport are per-window, so a pane's
        //    own position wins over the buffer's last-known cursor.
        let focused_window = incoming_windows
            .remove(&new_active_window)
            .unwrap_or_default();
        self.layout = new_layout;
        self.windows = incoming_windows;
        self.active_window = new_active_window;
        self.window = focused_window;
        self.active_tab = idx;
        Ok(())
    }

    pub fn open_buffer(&mut self, path: PathBuf) -> Result<()> {
        // Switch to existing buffer if this path is already open.
        if self.buffer.path.as_deref() == Some(path.as_path()) {
            self.show_start_page = false;
            return Ok(());
        }
        for (i, stash) in self.buffers.iter().enumerate() {
            if i == self.active {
                continue;
            }
            if stash.buffer.path.as_deref() == Some(path.as_path()) {
                return self.switch_to(i);
            }
        }
        let buf = Buffer::from_path(path)?;
        // Restore persisted undo if the cached snapshot matches the file
        // content on disk — no point reusing history recorded against a
        // different version.
        let history = buf
            .path
            .as_deref()
            .and_then(crate::undo::cache_path_for)
            .and_then(|p| {
                let hash = crate::undo::hash_text(&buf.rope.to_string());
                crate::undo::History::load_from_path(&p, hash)
            })
            .unwrap_or_default();
        let stash = BufferStash {
            buffer: buf,
            history,
            ..Default::default()
        };
        self.buffers.push(stash);
        let new_idx = self.buffers.len() - 1;
        self.switch_to(new_idx)?;
        self.lsp_attach_active();
        self.refresh_git_branch();
        self.refresh_git_hunks();
        self.refresh_editorconfig();
        self.show_start_page = false;
        self.touch_recent();
        // Strip the phantom `[No Name]` seed that App::new() seeds at
        // index 0 — only on the transition from "fresh launch" (one
        // empty no-path buffer) to a first real file. Skip the strip
        // if any inactive window is still showing the phantom: that
        // pane was opened deliberately (via `<C-w>v` / `<C-w>s` while
        // on the start page) and dragging it onto the freshly-opened
        // file would erase what the user explicitly split for.
        let phantom_in_use = self
            .windows
            .values()
            .any(|w| w.buffer_idx == 0);
        if self.buffers.len() > 1
            && self.active != 0
            && self.buffers[0].buffer.path.is_none()
            && self.buffers[0].buffer.rope.len_chars() == 0
            && !phantom_in_use
        {
            self.buffers.remove(0);
            self.active = self.active.saturating_sub(1);
            self.active_tab = self.active_tab.saturating_sub(1);
            // Phantom `[No Name]` at index 0 just got stripped — every
            // Window's `buffer_idx` shifts down to match.
            self.remap_windows_after_remove(0);
        }
        Ok(())
    }

    /// Watcher: if the active buffer's file has been modified on disk
    /// while we weren't editing it (`!buffer.dirty`), reload from disk so
    /// the user sees the latest version. Throttled to once per second so
    /// the syscall cost is negligible.
    pub(super) fn maybe_reload_from_disk(&mut self) {
        if self.buffer.dirty {
            return;
        }
        let now = Instant::now();
        if now.duration_since(self.last_disk_check) < DISK_CHECK_INTERVAL {
            return;
        }
        self.last_disk_check = now;
        let Some(path) = self.buffer.path.clone() else { return };
        let Ok(meta) = std::fs::metadata(&path) else { return };
        let Ok(disk_mtime) = meta.modified() else { return };
        match self.buffer.disk_mtime {
            Some(prev) if disk_mtime <= prev => return,
            _ => {}
        }
        if let Some(name) = self.reload_buffer_from_disk_inner(&path, Some(disk_mtime)) {
            self.status_msg = format!("reloaded {name} (changed on disk)");
        }
    }

    /// Force-reload the active buffer from disk, bypassing the dirty
    /// guard and the once-per-second throttle. Returns the file's name
    /// for status reporting (or `None` if the reload failed).
    pub(super) fn force_reload_from_disk(&mut self) -> Option<String> {
        let path = self.buffer.path.clone()?;
        self.reload_buffer_from_disk_inner(&path, None)
    }

    fn reload_buffer_from_disk_inner(
        &mut self,
        path: &std::path::Path,
        disk_mtime: Option<std::time::SystemTime>,
    ) -> Option<String> {
        let raw = std::fs::read_to_string(path).ok()?;
        // Normalize CRLF → LF (matches Buffer::from_path) so reloaded
        // CRLF files don't leak `\r` chars into the rope.
        let text = raw.replace("\r\n", "\n");
        let _ = Rope::from_str(&text); // touch ropey so caches invalidate downstream
        let total = self.buffer.total_chars();
        self.buffer.delete_range(0, total);
        self.buffer.insert_at_idx(0, &text);
        self.buffer.disk_mtime = disk_mtime.or_else(|| {
            std::fs::metadata(path).and_then(|m| m.modified()).ok()
        });
        self.buffer.dirty = false;
        let last = self.buffer.line_count().saturating_sub(1);
        if self.window.cursor.line > last {
            self.window.cursor.line = last;
        }
        self.clamp_cursor_normal();
        // Blame is keyed by the on-disk file's line numbers; a reload
        // can shift them. Re-fetch when visible, clear when not.
        if self.blame_visible {
            self.blame = crate::git::blame(path).unwrap_or_default();
        } else {
            self.blame.clear();
        }
        path.file_name()
            .map(|s| s.to_string_lossy().to_string())
            .or_else(|| Some(path.display().to_string()))
    }

    /// Move the active buffer's path to the front of the recents list and
    /// persist. Caps at `RECENTS_CAP` to keep the file from growing
    /// without bound.
    pub(super) fn touch_recent(&mut self) {
        let Some(path) = self.buffer.path.clone() else { return };
        let canon = path.canonicalize().unwrap_or(path);
        self.recents.retain(|p| *p != canon);
        self.recents.insert(0, canon);
        self.recents.truncate(RECENTS_CAP);
        save_recents(&self.recents);
    }

    pub(super) fn cycle_buffer(&mut self, step: i64) {
        // Any buffer-cycle press dismisses the start page — including the
        // single-buffer case, where there's nothing to switch *to* but
        // the user clearly wants to leave the welcome screen.
        self.show_start_page = false;
        if self.buffers.len() <= 1 {
            self.status_msg = "Only one buffer".into();
            return;
        }
        // Cycle by tab rather than focused-buffer index — each buffer
        // with its own tab cycles independently; split-companion
        // buffers (no stashed layout, not the active_tab) are
        // skipped so H/L matches the tabs the user actually sees in
        // the bar.
        let visible = self.visible_tab_indices();
        if visible.is_empty() {
            return;
        }
        let pos = visible
            .iter()
            .position(|&i| i == self.active_tab)
            .unwrap_or(0);
        let n = visible.len() as i64;
        let next_pos = ((pos as i64) + step).rem_euclid(n) as usize;
        let next = visible[next_pos];
        if let Err(e) = self.switch_tab(next) {
            self.status_msg = format!("error: {e}");
        }
    }

    pub(super) fn switch_buffer_by_spec(&mut self, spec: &str) -> Result<()> {
        let spec = spec.trim();
        if spec.is_empty() {
            anyhow::bail!("E94: No matching buffer");
        }
        // Numeric: 1-based buffer number.
        if let Ok(n) = spec.parse::<usize>() {
            if n == 0 || n > self.buffers.len() {
                anyhow::bail!("E86: Buffer {n} does not exist");
            }
            return self.switch_tab(n - 1);
        }
        // Substring match against buffer paths.
        let mut matches: Vec<usize> = Vec::new();
        for (i, stash) in self.buffers.iter().enumerate() {
            let path = if i == self.active {
                self.buffer.path.as_ref()
            } else {
                stash.buffer.path.as_ref()
            };
            if let Some(p) = path {
                if p.to_string_lossy().contains(spec) {
                    matches.push(i);
                }
            }
        }
        match matches.len() {
            0 => anyhow::bail!("E94: No matching buffer for '{spec}'"),
            1 => self.switch_tab(matches[0]),
            _ => anyhow::bail!("E93: More than one match for '{spec}'"),
        }
    }

    pub(super) fn delete_buffer(&mut self, force: bool) -> Result<()> {
        if !force && self.buffer.dirty {
            anyhow::bail!("E89: No write since last change (use :bd!)");
        }
        if self.buffers.len() == 1 {
            // Last buffer — replace with an empty one and resurface the start page.
            self.buffer = Buffer::empty();
            self.window.cursor = Cursor::default();
            self.window.view_top = 0;
            self.window.view_left = 0;
            self.history = History::default();
            self.window.visual_anchor = None;
            self.marks.clear();
            self.jumplist.clear();
            self.jump_idx = 0;
            self.buffers[0] = BufferStash::default();
            // Every Window now points at the same fresh empty slot.
            self.remap_windows_to_single(0);
            self.show_start_page = true;
            self.status_msg = "Buffer closed".into();
            return Ok(());
        }
        let prev = self.active;
        let next = if prev + 1 < self.buffers.len() { prev + 1 } else { prev - 1 };
        // Closing a buffer drops its whole tab (and its layout) — use
        // switch_tab so the user lands on the next tab's saved layout
        // rather than getting the soon-to-be-removed tab's split
        // structure carried over.
        self.switch_tab(next)?;
        // Now the slot at `prev` holds the snapshot we want to drop.
        self.buffers.remove(prev);
        if self.active > prev {
            self.active -= 1;
        }
        if self.active_tab > prev {
            self.active_tab -= 1;
        }
        // Every Window's buffer_idx may need fixing after the shift.
        self.remap_windows_after_remove(prev);
        Ok(())
    }

    /// Close every open buffer. Without `force`, refuses if any buffer is
    /// dirty. Leaves the editor on a single empty `[No Name]` slot with the
    /// start page resurfaced — same terminal state as deleting the last
    /// buffer with `:bd`.
    pub(super) fn delete_all_buffers(&mut self, force: bool) -> Result<()> {
        if !force {
            if self.buffer.dirty {
                anyhow::bail!("E89: active buffer has unsaved changes (use <leader>bA)");
            }
            for (i, stash) in self.buffers.iter().enumerate() {
                if i == self.active {
                    continue;
                }
                if stash.buffer.dirty {
                    anyhow::bail!(
                        "E89: buffer {} has unsaved changes (use <leader>bA)",
                        i + 1
                    );
                }
            }
        }
        let count = self.buffers.len();
        self.buffers.clear();
        self.buffers.push(BufferStash::default());
        self.active = 0;
        self.active_tab = 0;
        self.buffer = Buffer::empty();
        self.window.cursor = Cursor::default();
        self.window.view_top = 0;
        self.window.view_left = 0;
        self.history = History::default();
        self.window.visual_anchor = None;
        self.marks.clear();
        self.jumplist.clear();
        self.jump_idx = 0;
        // Drop every split too — single fresh tab with single window.
        let (fresh_layout, fresh_root) = crate::layout::Layout::new();
        self.layout = fresh_layout;
        self.windows = std::collections::HashMap::new();
        self.active_window = fresh_root;
        // Every Window now points at the single fresh `[No Name]` slot.
        self.remap_windows_to_single(0);
        self.show_start_page = true;
        self.status_msg = format!("closed {count} buffer{}", if count == 1 { "" } else { "s" });
        Ok(())
    }

    /// Close every buffer except the active one. Refuses if any of them is dirty.
    pub(super) fn buffer_only(&mut self) -> Result<()> {
        // Check for dirty inactive buffers first.
        for (i, stash) in self.buffers.iter().enumerate() {
            if i == self.active {
                continue;
            }
            if stash.buffer.dirty {
                anyhow::bail!(
                    "E89: buffer {} has unsaved changes (use :bd! or save)",
                    i + 1
                );
            }
        }
        // Remove from highest to lowest so indices stay valid.
        let mut to_drop: Vec<usize> = (0..self.buffers.len())
            .filter(|i| *i != self.active)
            .collect();
        to_drop.sort_by(|a, b| b.cmp(a));
        for idx in to_drop {
            self.buffers.remove(idx);
            if self.active > idx {
                self.active -= 1;
            }
            if self.active_tab > idx {
                self.active_tab -= 1;
            }
        }
        // Only one buffer survives — every Window points at it.
        self.active_tab = self.active;
        self.remap_windows_to_single(self.active);
        self.status_msg = format!("kept buffer {}", self.active + 1);
        Ok(())
    }

    /// Open every buffer recorded in the session, restore each one's
    /// cursor + viewport, and land on the previously active buffer.
    /// Buffers that no longer exist on disk are silently dropped.
    pub(super) fn hydrate_from_session(&mut self, session: crate::session::Session) {
        let mut opened_any = false;
        for sb in &session.buffers {
            let path = PathBuf::from(&sb.path);
            if !path.exists() {
                continue;
            }
            if self.open_buffer(path.clone()).is_err() {
                continue;
            }
            // After open_buffer the active buffer is the one we just
            // opened — restore its cursor + viewport.
            let last = self.buffer.line_count().saturating_sub(1);
            self.window.cursor.line = sb.line.min(last);
            let line_len = self.buffer.line_len(self.window.cursor.line);
            self.window.cursor.col = sb.col.min(line_len.saturating_sub(1).max(0));
            self.window.cursor.want_col = self.window.cursor.col;
            self.window.view_top = sb.view_top.min(last);
            // Restore jumplist — clamp each entry against the current
            // buffer's bounds so a file shortened since the last session
            // doesn't carry an out-of-range jump.
            self.jumplist = sb
                .jumplist
                .iter()
                .map(|(l, c)| {
                    let line = (*l).min(last);
                    let col_max = self.buffer.line_len(line).saturating_sub(1);
                    (line, (*c).min(col_max))
                })
                .collect();
            self.jump_idx = sb.jump_idx.min(self.jumplist.len());
            opened_any = true;
        }
        if !opened_any {
            return;
        }
        // App::new() pre-seeded buffers[0] with a default empty stash —
        // strip it so the restored session isn't polluted by a phantom
        // `[No Name]` slot. Index 0's stash has no path AND a fresh
        // (empty) buffer, distinguishing it from anything we just
        // restored.
        let phantom_in_use = self.windows.values().any(|w| w.buffer_idx == 0);
        if self.buffers.len() > 1
            && self.active != 0
            && self.buffers[0].buffer.path.is_none()
            && self.buffers[0].buffer.rope.len_chars() == 0
            && !phantom_in_use
        {
            self.buffers.remove(0);
            self.active = self.active.saturating_sub(1);
            self.active_tab = self.active_tab.saturating_sub(1);
            self.remap_windows_after_remove(0);
        }
        // Honour the session's `active` index — clamp to whatever we
        // actually managed to open.
        let target = session.active.min(self.buffers.len().saturating_sub(1));
        let _ = self.switch_to(target);
        // Land on the start page rather than the active buffer — restored
        // buffers stay in the background until the user reaches for one
        // via H/L, :bn, :b<n>, etc. Open_buffer set this to false during
        // the per-buffer loop above; flip it back here.
        self.show_start_page = true;
    }

    /// Snapshot the current buffer set into a `Session`. Buffers without a
    /// path (start page, `[Health]` scratch) are skipped — we can't reopen
    /// them on the next launch.
    pub(super) fn build_session(&self) -> crate::session::Session {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let canon = cwd.canonicalize().unwrap_or(cwd);
        let mut buffers: Vec<crate::session::SessionBuffer> = Vec::new();
        let mut active_in_session: usize = 0;
        for (i, stash) in self.buffers.iter().enumerate() {
            let (path, line, col, view_top, jumplist, jump_idx) = if i == self.active {
                (
                    self.buffer.path.as_ref(),
                    self.window.cursor.line,
                    self.window.cursor.col,
                    self.window.view_top,
                    self.jumplist.clone(),
                    self.jump_idx,
                )
            } else {
                (
                    stash.buffer.path.as_ref(),
                    stash.cursor.line,
                    stash.cursor.col,
                    stash.view_top,
                    stash.jumplist.clone(),
                    stash.jump_idx,
                )
            };
            let Some(path) = path else { continue };
            if i == self.active {
                active_in_session = buffers.len();
            }
            buffers.push(crate::session::SessionBuffer {
                path: path.display().to_string(),
                line,
                col,
                view_top,
                jumplist,
                jump_idx,
            });
        }
        crate::session::Session {
            cwd: canon.to_string_lossy().to_string(),
            buffers,
            active: active_in_session,
        }
    }

    pub(super) fn list_buffers(&self) -> String {
        let mut out = String::new();
        for (i, stash) in self.buffers.iter().enumerate() {
            let (path, dirty) = if i == self.active {
                (
                    self.buffer.path.as_ref().map(|p| p.display().to_string()),
                    self.buffer.dirty,
                )
            } else {
                (
                    stash.buffer.path.as_ref().map(|p| p.display().to_string()),
                    stash.buffer.dirty,
                )
            };
            let name = path.unwrap_or_else(|| "[No Name]".into());
            let marker = if i == self.active { "%" } else { " " };
            let dirty_marker = if dirty { "+" } else { " " };
            if !out.is_empty() {
                out.push_str(" | ");
            }
            out.push_str(&format!("{} {}{} {}", i + 1, marker, dirty_marker, name));
        }
        if out.is_empty() {
            "[No buffers]".into()
        } else {
            out
        }
    }
}

fn recents_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let mut p = PathBuf::from(home);
    p.push(".cache/binvim/recents");
    Some(p)
}

pub(super) fn load_recents() -> Vec<PathBuf> {
    let Some(p) = recents_path() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(&p) else { return Vec::new() };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(PathBuf::from)
        .collect()
}

fn save_recents(list: &[PathBuf]) {
    let Some(p) = recents_path() else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = list
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let _ = std::fs::write(&p, text);
}
