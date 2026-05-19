//! LSP rename preview overlay — build / navigate / apply.
//!
//! The user invokes rename via `<leader>r`, types a new name in the
//! prompt, and the server replies with a `WorkspaceEdit`. Historically
//! we applied it on the spot; now we route through this overlay so the
//! user sees every site, can de-select stray matches (LSP renames
//! across the codebase are confident but not infallible — string
//! matches inside doc-comments, conditional-compile blocks the server
//! can't see, …), and only the kept rows actually mutate files.
//!
//! State lives at `App.pending_rename_preview`. While `Some`, the
//! editor is in `Mode::RenamePreview` and this module's input handler
//! owns every keystroke until the user accepts (`Enter`) or cancels
//! (`Esc`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

use crate::app::state::{ConcreteEdit, PreviewKind, RenamePreview, RenamePreviewEdit};
use crate::mode::Mode;

impl super::App {
    /// Build a `RenamePreview` from a `WorkspaceEdit` and switch into
    /// `Mode::RenamePreview`. Reads each affected file off disk *once*
    /// to extract the original line text + compose an after-snapshot
    /// per edit. Per-file reads are cached against this build so a
    /// rename touching N edits in one file doesn't fan out into N
    /// reads. Errors per-file degrade gracefully — we keep the row
    /// but the before/after text is empty so the user still sees the
    /// position.
    ///
    /// `rename_anchor` (set when the prompt opened) carries the
    /// original symbol name; we fall back to a placeholder if it's
    /// somehow been cleared, but it'll normally still be there
    /// because the prompt path doesn't clear it before firing the
    /// server request.
    pub(super) fn open_rename_preview(&mut self, edit: &JsonValue) {
        let parsed = self.parse_workspace_edit(edit);
        if parsed.is_empty() {
            self.status_msg = "rename: no edits returned".into();
            return;
        }
        let original = self
            .rename_anchor
            .as_ref()
            .map(|(_, _, _, term)| term.clone())
            .unwrap_or_else(|| "(symbol)".to_string());
        let new_name = first_new_text(&parsed)
            .unwrap_or_else(|| "(unknown)".to_string());
        let kind = PreviewKind::Rename { original, new_name };
        self.show_preview_overlay(kind, parsed);
    }

    /// Open the preview overlay for a `WorkspaceEdit` returned by a
    /// code action — same UI as rename, with the title bar adjusted to
    /// "Apply: <action title>". Routed only when
    /// `[lsp] preview_workspace_edits = true`; the no-preview path
    /// stays in `run_code_action`.
    pub(super) fn open_code_action_preview(&mut self, title: String, edit: &JsonValue) {
        let parsed = self.parse_workspace_edit(edit);
        if parsed.is_empty() {
            self.status_msg = format!("'{title}' had no edits");
            return;
        }
        self.show_preview_overlay(PreviewKind::CodeAction { title }, parsed);
    }

    /// Open the preview overlay for a server-initiated
    /// `workspace/applyEdit` request. The server is blocked waiting
    /// for a response, so we stash `(client_key, request_id)` on the
    /// preview state; accept replies `applied: true`, cancel replies
    /// `applied: false`. If a preview is already open we reject the
    /// new request immediately so the server doesn't hang.
    ///
    /// `label` is a short human-readable description used in the
    /// title bar — typically the client_key, since the server didn't
    /// give us a more meaningful name for the apply.
    pub(super) fn open_server_apply_edit_preview(
        &mut self,
        client_key: String,
        request_id: u64,
        label: String,
        edit: &JsonValue,
    ) -> bool {
        if self.pending_rename_preview.is_some() {
            return false;
        }
        let parsed = self.parse_workspace_edit(edit);
        if parsed.is_empty() {
            return false;
        }
        let kind = PreviewKind::ApplyEditFromServer {
            label,
            client_key,
            request_id,
        };
        self.show_preview_overlay(kind, parsed);
        true
    }

    /// Shared body — caches per-file line text and switches into
    /// `Mode::RenamePreview`. Used by rename, code-action, and
    /// server-apply preview entry points.
    fn show_preview_overlay(&mut self, kind: PreviewKind, parsed: Vec<ConcreteEdit>) {
        // Read each file once. The active buffer is in memory; for
        // anything else we hit disk. Keep both keyed by path so an
        // edit list interleaving files doesn't re-read.
        let mut line_cache: HashMap<std::path::PathBuf, Vec<String>> = HashMap::new();
        let active_path = self.buffer.path.clone();
        for e in &parsed {
            if line_cache.contains_key(&e.path) {
                continue;
            }
            let lines = if active_path.as_ref() == Some(&e.path) {
                // Live buffer — its disk contents may already be
                // stale; reading the rope gives the user-visible state.
                let count = self.buffer.line_count();
                (0..count)
                    .map(|i| {
                        let s = self.buffer.rope.line(i).to_string();
                        // Strip trailing `\n` so the preview row's
                        // before/after don't render an empty wrap.
                        s.strip_suffix('\n').unwrap_or(&s).to_string()
                    })
                    .collect::<Vec<_>>()
            } else {
                match std::fs::read_to_string(&e.path) {
                    Ok(s) => s.split('\n').map(|l| l.to_string()).collect(),
                    Err(_) => Vec::new(),
                }
            };
            line_cache.insert(e.path.clone(), lines);
        }
        let edits: Vec<RenamePreviewEdit> = parsed
            .into_iter()
            .map(|e| build_preview_row(&e, &line_cache))
            .collect();
        self.pending_rename_preview = Some(RenamePreview {
            kind,
            edits,
            cursor: 0,
            scroll: 0,
        });
        self.mode = Mode::RenamePreview;
        self.status_msg.clear();
    }

    /// Modal key handler for `Mode::RenamePreview`. Returns no value
    /// — every keystroke either mutates the preview state or routes
    /// to apply/cancel.
    pub(super) fn handle_rename_preview_key(&mut self, key: KeyEvent) {
        let no_mods = key.modifiers.is_empty();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.rename_preview_cancel(),
            KeyCode::Enter => self.rename_preview_accept(),
            KeyCode::Char(' ') if no_mods => self.rename_preview_toggle_current(),
            KeyCode::Char('a') if no_mods => self.rename_preview_set_all(true),
            KeyCode::Char('n') if no_mods => self.rename_preview_set_all(false),
            KeyCode::Char('o') if no_mods => self.rename_preview_open_file(),
            KeyCode::Char('j') | KeyCode::Down if no_mods => self.rename_preview_move(1),
            KeyCode::Char('k') | KeyCode::Up if no_mods => self.rename_preview_move(-1),
            KeyCode::Char('g') if no_mods => self.rename_preview_jump(0),
            KeyCode::Char('G') if no_mods => self.rename_preview_jump(isize::MAX),
            KeyCode::Char('d') if ctrl => self.rename_preview_move(8),
            KeyCode::Char('u') if ctrl => self.rename_preview_move(-8),
            KeyCode::PageDown => self.rename_preview_move(12),
            KeyCode::PageUp => self.rename_preview_move(-12),
            _ => {}
        }
    }

    fn rename_preview_cancel(&mut self) {
        let Some(preview) = self.pending_rename_preview.take() else {
            self.mode = Mode::Normal;
            return;
        };
        self.mode = Mode::Normal;
        // Server-initiated applyEdit is blocked waiting for our
        // response — tell it the user declined so it doesn't hang.
        if let PreviewKind::ApplyEditFromServer { client_key, request_id, .. } = &preview.kind {
            self.lsp.send_apply_edit_response(client_key, *request_id, false);
        }
        self.status_msg = match &preview.kind {
            PreviewKind::Rename { .. } => "rename cancelled".into(),
            PreviewKind::CodeAction { title } => format!("'{title}' cancelled"),
            PreviewKind::ApplyEditFromServer { label, .. } => {
                format!("'{label}' workspace edit declined")
            }
        };
    }

    fn rename_preview_accept(&mut self) {
        let Some(preview) = self.pending_rename_preview.take() else {
            self.mode = Mode::Normal;
            return;
        };
        self.mode = Mode::Normal;
        let kind = preview.kind;
        let enabled: Vec<crate::app::state::ConcreteEdit> = preview
            .edits
            .into_iter()
            .filter(|e| e.enabled)
            .map(|e| e.edit)
            .collect();
        if enabled.is_empty() {
            // No work to apply. Server-initiated requests still need
            // an answer or the server hangs — `applied: false` is
            // honest here (we didn't write anything).
            if let PreviewKind::ApplyEditFromServer { client_key, request_id, .. } = &kind {
                self.lsp.send_apply_edit_response(client_key, *request_id, false);
            }
            self.status_msg = match &kind {
                PreviewKind::Rename { .. } => "rename: no edits selected".into(),
                PreviewKind::CodeAction { title } => format!("'{title}': no edits selected"),
                PreviewKind::ApplyEditFromServer { label, .. } => {
                    format!("'{label}': no edits selected")
                }
            };
            return;
        }
        let outcome = self.apply_concrete_edits(&enabled);
        // Tell the server how we did *before* updating status — same
        // reasoning as the cancel path, just reflecting the actual
        // apply result instead of a flat false.
        if let PreviewKind::ApplyEditFromServer { client_key, request_id, .. } = &kind {
            let applied = matches!(outcome, Ok((n, _)) if n > 0);
            self.lsp.send_apply_edit_response(client_key, *request_id, applied);
        }
        match outcome {
            Ok((edits, files)) => {
                let summary = match &kind {
                    PreviewKind::Rename { original, new_name } => format!(
                        "renamed {original} → {new_name} ({edits} edit{} across {files} file{})",
                        if edits == 1 { "" } else { "s" },
                        if files == 1 { "" } else { "s" },
                    ),
                    PreviewKind::CodeAction { title } => format!(
                        "applied '{title}' ({edits} edit{} across {files} file{})",
                        if edits == 1 { "" } else { "s" },
                        if files == 1 { "" } else { "s" },
                    ),
                    PreviewKind::ApplyEditFromServer { label, .. } => format!(
                        "applied '{label}' ({edits} edit{} across {files} file{})",
                        if edits == 1 { "" } else { "s" },
                        if files == 1 { "" } else { "s" },
                    ),
                };
                self.status_msg = summary;
            }
            Err(e) => {
                self.status_msg = match &kind {
                    PreviewKind::Rename { .. } => format!("rename error: {e}"),
                    PreviewKind::CodeAction { title } => format!("'{title}' error: {e}"),
                    PreviewKind::ApplyEditFromServer { label, .. } => {
                        format!("'{label}' error: {e}")
                    }
                };
            }
        }
    }

    fn rename_preview_toggle_current(&mut self) {
        let Some(p) = self.pending_rename_preview.as_mut() else { return; };
        if let Some(row) = p.edits.get_mut(p.cursor) {
            row.enabled = !row.enabled;
        }
    }

    fn rename_preview_set_all(&mut self, enabled: bool) {
        let Some(p) = self.pending_rename_preview.as_mut() else { return; };
        for row in p.edits.iter_mut() {
            row.enabled = enabled;
        }
    }

    fn rename_preview_move(&mut self, delta: isize) {
        let Some(p) = self.pending_rename_preview.as_mut() else { return; };
        if p.edits.is_empty() {
            return;
        }
        let n = p.edits.len() as isize;
        let cur = p.cursor as isize;
        let next = (cur + delta).clamp(0, n - 1) as usize;
        p.cursor = next;
        clamp_scroll(p);
    }

    fn rename_preview_jump(&mut self, to: isize) {
        let Some(p) = self.pending_rename_preview.as_mut() else { return; };
        if p.edits.is_empty() {
            return;
        }
        let n = p.edits.len() as isize;
        let target = if to == isize::MAX { n - 1 } else { to.clamp(0, n - 1) };
        p.cursor = target as usize;
        clamp_scroll(p);
    }

    fn rename_preview_open_file(&mut self) {
        let Some(p) = self.pending_rename_preview.as_ref() else { return; };
        let Some(row) = p.edits.get(p.cursor).cloned() else { return; };
        self.pending_rename_preview = None;
        self.mode = Mode::Normal;
        if let Err(e) = self.open_buffer(row.edit.path.clone()) {
            self.status_msg = format!("rename: open failed: {e}");
            return;
        }
        self.push_jump();
        self.window.cursor.line = row.edit.start_line;
        self.window.cursor.col = row.edit.start_col;
        self.window.cursor.want_col = row.edit.start_col;
        self.clamp_cursor_normal();
        self.status_msg = "rename preview cancelled — opened edit site".into();
    }
}

/// Compose the per-row preview struct from a parsed edit + the cached
/// line table for its file. Single-line edits (the common rename
/// case — symbol references are always on one line) get a proper
/// before/after; multi-line edits keep the start line as "before" and
/// the spliced result as "after" so the row still shows something
/// useful, even if it's an approximation.
fn build_preview_row(
    e: &ConcreteEdit,
    line_cache: &HashMap<std::path::PathBuf, Vec<String>>,
) -> RenamePreviewEdit {
    let lines = line_cache.get(&e.path);
    let line_text = lines
        .and_then(|l| l.get(e.start_line).cloned())
        .unwrap_or_default();
    let after_text = compose_after(&line_text, e, lines);
    RenamePreviewEdit {
        edit: e.clone(),
        line_text,
        after_text,
        enabled: true,
    }
}

fn compose_after(
    line_text: &str,
    e: &ConcreteEdit,
    lines: Option<&Vec<String>>,
) -> String {
    if e.start_line != e.end_line {
        // Cross-line replacement — splice with the end line's tail.
        let mut after = line_text
            .chars()
            .take(char_clamp(line_text, e.start_col))
            .collect::<String>();
        after.push_str(&e.new_text);
        if let Some(end_line) = lines.and_then(|l| l.get(e.end_line)) {
            after.push_str(
                &end_line
                    .chars()
                    .skip(char_clamp(end_line, e.end_col))
                    .collect::<String>(),
            );
        }
        after
    } else {
        let mut after = line_text
            .chars()
            .take(char_clamp(line_text, e.start_col))
            .collect::<String>();
        after.push_str(&e.new_text);
        after.push_str(
            &line_text
                .chars()
                .skip(char_clamp(line_text, e.end_col))
                .collect::<String>(),
        );
        after
    }
}

/// Clamp an LSP column to the line's actual char count. Servers can
/// return positions one past the end (for "insert at end of line"
/// edits) which would otherwise produce a panic on `chars().take`.
fn char_clamp(line: &str, col: usize) -> usize {
    col.min(line.chars().count())
}

/// Pull the `newText` of the first edit out of the parsed list — that
/// IS the new name for the rename case (every edit is the same
/// replacement string). Falls back to `None` for an empty list (the
/// caller guards on that before getting here).
fn first_new_text(edits: &[ConcreteEdit]) -> Option<String> {
    edits.first().map(|e| e.new_text.clone())
}

/// Pull `scroll` toward `cursor` so the cursor row stays visible. The
/// renderer is what actually clamps to its visible-rows budget; here we
/// just keep `scroll <= cursor` and shove it down when the cursor
/// drops past a generous window so the next render doesn't have to
/// hunt back upward.
fn clamp_scroll(p: &mut RenamePreview) {
    if p.cursor < p.scroll {
        p.scroll = p.cursor;
    } else if p.cursor.saturating_sub(p.scroll) > 24 {
        // Loose upper bound — the renderer trims tighter against its
        // actual visible-row count. 24 just keeps the cursor from
        // racing off the bottom on Ctrl-D / G.
        p.scroll = p.cursor.saturating_sub(24);
    }
}
