//! LSP event handling and the request-side helpers — completion popup
//! plumbing, hover, goto, references, signature help, code actions,
//! workspace edits, rename prompt, and the diagnostics->`:health` glue.

use anyhow::Result;
use serde_json::Value as JsonValue;
use std::path::PathBuf;
use std::time::Instant;

use crate::lsp::{
    CodeActionItem, CompletionItem, Diagnostic, LocationItem, LspEvent, Severity, SymbolItem,
};
use crate::mode::Mode;
use crate::picker::{PickerKind, PickerPayload, PickerState};

use super::state::{CompletionState, HoverState, LSP_SYNC_DEBOUNCE};

impl super::App {
    pub(super) fn handle_lsp_events(&mut self, events: Vec<LspEvent>) {
        for ev in events {
            match ev {
                LspEvent::GotoDef { path, line, col } => {
                    self.push_jump();
                    if let Err(e) = self.open_buffer(path) {
                        self.status_msg = format!("error: {e}");
                        continue;
                    }
                    self.cursor.line = line;
                    self.cursor.col = col;
                    self.cursor.want_col = col;
                    self.clamp_cursor_normal();
                }
                LspEvent::Hover { text } => {
                    self.hover = HoverState::from_lsp_text(
                        &text,
                        self.width as usize,
                        self.config.hover.wrap_code,
                    );
                    if self.hover.is_none() {
                        self.status_msg = "LSP: empty hover".into();
                    }
                }
                LspEvent::SignatureHelp(sig) => {
                    self.signature_help = Some(sig);
                }
                LspEvent::References { items } => {
                    self.open_locations_picker("References", items);
                }
                LspEvent::Symbols { items, workspace } => {
                    if workspace {
                        self.update_workspace_symbols_picker(items);
                    } else {
                        self.open_symbols_picker(items);
                    }
                }
                LspEvent::CodeActions { items } => {
                    self.open_code_actions_picker(items);
                }
                LspEvent::Rename { edit } => match self.apply_workspace_edit(&edit) {
                    Ok((edits, files)) if edits > 0 => {
                        self.status_msg = format!(
                            "renamed {edits} occurrence{} across {files} file{}",
                            if edits == 1 { "" } else { "s" },
                            if files == 1 { "" } else { "s" },
                        );
                    }
                    Ok(_) => self.status_msg = "rename: no edits returned".into(),
                    Err(e) => self.status_msg = format!("rename error: {e}"),
                },
                LspEvent::ApplyEditRequest { client_key, id, edit } => {
                    let applied = match self.apply_workspace_edit(&edit) {
                        Ok((edits, _)) => edits > 0,
                        Err(_) => false,
                    };
                    self.lsp.send_apply_edit_response(&client_key, id, applied);
                }
                LspEvent::DiagnosticsUpdated => {}
                LspEvent::InlayHints { path, hints } => {
                    if hints.is_empty() {
                        self.inlay_hints.remove(&path);
                    } else {
                        self.inlay_hints.insert(path, hints);
                    }
                }
                LspEvent::NotFound(kind) => {
                    if kind == "completions" {
                        // Auto-trigger fires on every keystroke; silently
                        // ignore an empty reply. With multi-server fan-out
                        // (e.g. Tailwind alongside tsserver) one server can
                        // return nothing while another still has matches —
                        // the next Completion event will replace or merge,
                        // so leaving the popup alone is correct.
                    } else if kind == "signature" {
                        // Server has nothing to say at this position —
                        // dismiss the popup so it doesn't linger after the
                        // cursor leaves the function call.
                        self.signature_help = None;
                    } else {
                        self.status_msg = format!("LSP: no {kind} found");
                    }
                }
                LspEvent::Completion { items } => {
                    // Servers (typescript-language-server especially) often dump
                    // their entire symbol table and expect the client to filter.
                    // Match the items against the user's typed prefix
                    // (anchor → cursor) so the popup actually narrows as you type.
                    let (anchor_line, anchor_col) = self.word_prefix_start();
                    let start_idx = self.buffer.pos_to_char(anchor_line, anchor_col);
                    let end_idx = self
                        .buffer
                        .pos_to_char(self.cursor.line, self.cursor.col);
                    let prefix = if end_idx > start_idx {
                        self.buffer.rope.slice(start_idx..end_idx).to_string()
                    } else {
                        String::new()
                    };
                    // Multi-server fan-out: when a popup is already open at
                    // the same anchor (i.e. another server already replied
                    // for this same request burst), merge new items with the
                    // existing list and re-filter together. Otherwise this
                    // is a fresh request — replace.
                    let mut merged_items = items;
                    let preserve = match self.completion.as_ref() {
                        Some(c) if c.anchor_line == anchor_line && c.anchor_col == anchor_col => {
                            true
                        }
                        _ => false,
                    };
                    if preserve {
                        if let Some(existing) = self.completion.take() {
                            merged_items.extend(existing.items);
                        }
                        let mut seen = std::collections::HashSet::new();
                        merged_items.retain(|item| seen.insert(item.label.clone()));
                    }
                    let filtered = filter_completion_items(merged_items, &prefix);
                    if filtered.is_empty() {
                        self.completion = None;
                    } else {
                        self.completion = Some(CompletionState {
                            items: filtered,
                            selected: 0,
                            anchor_line,
                            anchor_col,
                        });
                    }
                }
            }
        }
    }

    /// Walk back from the cursor through identifier-class chars to find where the
    /// in-progress word started — that's the chunk we'll replace on completion accept.
    /// `-` is included so CSS property names (`border-color`) and Tailwind class
    /// names (`bg-blue-500`) are treated as one continuous token.
    fn word_prefix_start(&self) -> (usize, usize) {
        let line = self.cursor.line;
        let mut col = self.cursor.col;
        while col > 0 {
            let prev = self
                .buffer
                .char_at(line, col - 1)
                .unwrap_or(' ');
            if prev.is_alphanumeric() || prev == '_' || prev == '-' {
                col -= 1;
            } else {
                break;
            }
        }
        (line, col)
    }

    pub(super) fn lsp_request_completion(&mut self, trigger_char: Option<char>) {
        let Some(path) = self.buffer.path.clone() else {
            return;
        };
        // Push the latest buffer to the server before asking — otherwise the
        // request lands against last frame's text and the server sees stale
        // content (no `.`, wrong identifier prefix, etc).
        self.lsp_sync_active();
        let line = self.cursor.line;
        let col = self.cursor.col;
        if !self.lsp.request_completion(&path, line, col, trigger_char) {
            // No LSP — silently ignore so editing isn't disrupted.
        }
    }

    /// Open a picker showing a list of LSP locations (used by `gr` find-
    /// references and any other future location-list query). Each row is
    /// `relpath:line:col` so the user can disambiguate before pressing
    /// Enter to jump.
    fn open_locations_picker(&mut self, title: &str, items: Vec<LocationItem>) {
        if items.is_empty() {
            self.status_msg = format!("LSP: no {} found", title.to_lowercase());
            return;
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let entries: Vec<(String, PickerPayload)> = items
            .into_iter()
            .map(|it| {
                let rel = it
                    .path
                    .strip_prefix(&cwd)
                    .ok()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| it.path.display().to_string());
                let display = format!("{}:{}:{}", rel, it.line + 1, it.col + 1);
                (
                    display,
                    PickerPayload::Location {
                        path: it.path,
                        line: it.line + 1,
                        col: it.col + 1,
                    },
                )
            })
            .collect();
        self.picker = Some(PickerState::new(
            PickerKind::References,
            title.into(),
            entries,
        ));
        self.mode = Mode::Picker;
    }

    /// Open the rename prompt — captures the symbol under the cursor for
    /// the eventual LSP request. The user types the new name; Enter fires
    /// `textDocument/rename`, Esc cancels.
    pub(super) fn start_rename_prompt(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "Save the buffer to rename".into();
            return;
        };
        let line = self.cursor.line;
        let col = self.cursor.col;
        // Pre-fill with the current word so common renames are a few-char edit.
        let current = self.word_under_cursor().unwrap_or_default();
        self.rename_anchor = Some((path, line, col, current.clone()));
        self.cmdline = current;
        self.mode = Mode::Prompt(crate::mode::PromptKind::Rename);
    }

    /// Open the literal-string replace-all prompt. Captures the word
    /// under the cursor as the search term and stashes it in
    /// `rename_anchor`; the prompt key handler routes the typed
    /// replacement to `finish_replace_all`.
    pub(super) fn start_replace_all_prompt(&mut self) {
        let Some(current) = self.word_under_cursor() else {
            self.status_msg = "No word under cursor".into();
            return;
        };
        // We reuse `rename_anchor` to carry the original word — the path
        // / line / col fields are unused for replace-all but the tuple
        // is the only place a prompt action has to stash arbitrary data
        // alongside the typed string.
        let placeholder = self.buffer.path.clone().unwrap_or_default();
        self.rename_anchor = Some((placeholder, 0, 0, current.clone()));
        self.cmdline = current;
        self.mode = Mode::Prompt(crate::mode::PromptKind::ReplaceAll);
    }

    /// Apply the typed replacement to every occurrence of the captured
    /// word in the current buffer. Uses the same machinery as `:%s` for
    /// the actual substitution.
    pub(super) fn finish_replace_all(&mut self, new_text: String) {
        let Some((_, _, _, original)) = self.rename_anchor.clone() else {
            self.status_msg = "replace: lost anchor".into();
            return;
        };
        if new_text == original {
            self.status_msg = "replace: unchanged".into();
            return;
        }
        if new_text.is_empty() {
            self.status_msg = "replace cancelled (empty)".into();
            return;
        }
        self.history.record(&self.buffer.rope, self.cursor);
        let n = self
            .substitute(
                crate::command::ExRange::Whole,
                &original,
                &new_text,
                true,
                false,
            )
            .unwrap_or(0);
        self.status_msg = if n == 0 {
            format!("Pattern not found: {original}")
        } else {
            format!(
                "{n} replacement{}",
                if n == 1 { "" } else { "s" }
            )
        };
    }

    pub(super) fn finish_rename(&mut self, new_name: String) {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            self.status_msg = "rename cancelled (empty name)".into();
            return;
        }
        let Some((path, line, col, original)) = self.rename_anchor.clone() else {
            self.status_msg = "rename: lost anchor".into();
            return;
        };
        if trimmed == original {
            self.status_msg = "rename: name unchanged".into();
            return;
        }
        self.lsp_sync_active();
        if !self.lsp.request_rename(&path, line, col, trimmed) {
            self.status_msg = "LSP: not active for this buffer".into();
        }
    }

    pub(super) fn lsp_request_references(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "LSP: buffer has no file".into();
            return;
        };
        self.lsp_sync_active();
        let line = self.cursor.line;
        let col = self.cursor.col;
        if !self.lsp.request_references(&path, line, col) {
            self.status_msg = "LSP: not active for this buffer".into();
        }
    }

    pub(super) fn lsp_request_signature_help(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            return;
        };
        self.lsp_sync_active();
        let line = self.cursor.line;
        let col = self.cursor.col;
        let _ = self.lsp.request_signature_help(&path, line, col);
    }

    pub(super) fn completion_cycle(&mut self, delta: i64) {
        let Some(c) = self.completion.as_mut() else {
            return;
        };
        if c.items.is_empty() {
            return;
        }
        let n = c.items.len() as i64;
        c.selected = ((c.selected as i64 + delta).rem_euclid(n)) as usize;
    }

    pub(super) fn completion_accept(&mut self) {
        let Some(c) = self.completion.take() else {
            return;
        };
        let Some(item) = c.items.get(c.selected).cloned() else {
            return;
        };
        // Prefer the server-provided textEdit range — it's the authoritative
        // span to replace. Fall back to the client-side word-prefix guess
        // (anchor → cursor) when the server didn't include a textEdit.
        let (start, end) = match item.text_edit_range {
            Some((s_line, s_col, e_line, e_col)) => {
                let s = self.buffer.pos_to_char(s_line, s_col);
                let e = self.buffer.pos_to_char(e_line, e_col);
                (s.min(e), s.max(e))
            }
            None => {
                if c.anchor_line != self.cursor.line {
                    return;
                }
                let s = self.buffer.pos_to_char(c.anchor_line, c.anchor_col);
                let e = self.buffer.pos_to_char(self.cursor.line, self.cursor.col);
                (s.min(e), s.max(e))
            }
        };
        if end > start {
            self.buffer.delete_range(start, end);
        }
        // Snippet items go through the placeholder expander so `${1:foo}`
        // doesn't end up as literal text in the buffer. Plain items insert
        // verbatim.
        let (text, stop_offsets) = if item.is_snippet {
            expand_snippet(&item.insert_text)
        } else {
            (item.insert_text.clone(), Vec::new())
        };
        self.buffer.insert_at_idx(start, &text);
        let inserted = text.chars().count();
        let landing = match stop_offsets.first() {
            Some(&off) => start + off.min(inserted),
            None => start + inserted,
        };
        self.cursor_to_idx(landing);
        // Two-or-more stops → Tab cycling kicks in (one stop has nothing
        // to cycle to). Convert relative offsets into absolute doc-char
        // positions; subsequent edits shift later stops via
        // `snippet_session_record_insert` / `_record_delete`.
        if stop_offsets.len() >= 2 {
            let stops: Vec<usize> = stop_offsets
                .iter()
                .map(|&off| start + off.min(inserted))
                .collect();
            self.snippet_session = Some(crate::app::state::SnippetSession {
                stops,
                current: 0,
                anchor_chars: self.buffer.total_chars(),
            });
        } else {
            self.snippet_session = None;
        }
    }

    /// Advance the cursor to the next snippet stop. Returns `true` if a
    /// session was active and a stop was consumed — the caller then
    /// suppresses the Tab key's normal indent behaviour. Returns `false`
    /// if no session is active (Tab falls through to indent insertion).
    ///
    /// On reaching the final stop the session is cleared. The cumulative
    /// buffer-char delta since the previous Tab (or expansion) is applied
    /// to every later stop before jumping — that's how user-typed text at
    /// the active stop pushes the remaining stops along.
    pub(super) fn advance_snippet_session(&mut self) -> bool {
        let Some(session) = self.snippet_session.as_mut() else {
            return false;
        };
        let now = self.buffer.total_chars();
        let delta = now as isize - session.anchor_chars as isize;
        if delta != 0 {
            for off in session.stops.iter_mut().skip(session.current + 1) {
                let shifted = *off as isize + delta;
                *off = shifted.max(0) as usize;
            }
        }
        session.anchor_chars = now;
        let next = session.current + 1;
        if next >= session.stops.len() {
            self.snippet_session = None;
            return true;
        }
        session.current = next;
        let target = session.stops[next].min(now);
        self.cursor_to_idx(target);
        true
    }

    pub(super) fn lsp_attach_active(&mut self) {
        let Some(path) = self.buffer.path.clone() else { return; };
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if self.lsp.ensure_for_path(&path, &cwd).is_none() {
            return;
        }
        let text = self.buffer.rope.to_string();
        // Every attached server (primary + auxiliaries like Tailwind) needs
        // its own didOpen — each carries its own languageId, derived from
        // the spec for this path (not the client's stored one).
        self.lsp.did_open_all(&path, &text);
        self.last_sent_version
            .insert(path, self.buffer.version);
    }

    /// Force-flush the active buffer to every attached LSP. Used right
    /// before a request that needs fresh text (completion / hover / goto)
    /// and from `lsp_sync_active_debounced` once the burst window expires.
    pub(super) fn lsp_sync_active(&mut self) {
        let Some(path) = self.buffer.path.clone() else { return; };
        let last = self.last_sent_version.get(&path).copied().unwrap_or(u64::MAX);
        if last == self.buffer.version {
            return;
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if self.lsp.ensure_for_path(&path, &cwd).is_none() {
            return;
        }
        let text = self.buffer.rope.to_string();
        if last == u64::MAX {
            self.lsp.did_open_all(&path, &text);
        } else {
            self.lsp.did_change_all(&path, self.buffer.version, &text);
        }
        self.last_sent_version
            .insert(path, self.buffer.version);
        self.last_lsp_sync_at = Instant::now();
    }

    /// Render-loop sync: only flush when the last successful flush is more
    /// than `LSP_SYNC_DEBOUNCE` ago. The main loop wakes early at the
    /// deadline (see `lsp_sync_due_at`) so a short burst still flushes
    /// promptly after the user pauses.
    pub(super) fn lsp_sync_active_debounced(&mut self) {
        let Some(path) = self.buffer.path.as_ref() else { return; };
        let last = self.last_sent_version.get(path).copied().unwrap_or(u64::MAX);
        if last == self.buffer.version {
            return;
        }
        // First-ever sync (e.g. didOpen on attach) shouldn't be delayed.
        if last != u64::MAX
            && Instant::now().duration_since(self.last_lsp_sync_at) < LSP_SYNC_DEBOUNCE
        {
            return;
        }
        self.lsp_sync_active();
    }

    /// Earliest wall-clock at which a debounced sync would fire if no key
    /// arrived first. `None` when the buffer is already fully shipped.
    pub(super) fn lsp_sync_due_at(&self) -> Option<Instant> {
        let path = self.buffer.path.as_ref()?;
        let last = self.last_sent_version.get(path).copied().unwrap_or(u64::MAX);
        if last == self.buffer.version {
            return None;
        }
        Some(self.last_lsp_sync_at + LSP_SYNC_DEBOUNCE)
    }

    pub(super) fn lsp_request_goto(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "LSP: buffer has no file".into();
            return;
        };
        let line = self.cursor.line;
        let col = self.cursor.col;
        if !self.lsp.request_definition(&path, line, col) {
            self.status_msg = "LSP: not active for this buffer".into();
        }
    }

    /// Ask the active buffer's LSP for inlay hints — once per buffer
    /// version. Throttled by `last_inlay_request_version` so we don't
    /// spam the server on every keystroke; the debounced sync upstream
    /// already coalesces text changes.
    pub(super) fn lsp_request_inlay_hints_if_due(&mut self) {
        let Some(path) = self.buffer.path.clone() else { return; };
        let version = self.buffer.version;
        let last = self
            .last_inlay_request_version
            .get(&path)
            .copied()
            .unwrap_or(u64::MAX);
        if last == version {
            return;
        }
        let end_line = self.buffer.line_count();
        if self.lsp.request_inlay_hints(&path, end_line) {
            self.last_inlay_request_version.insert(path, version);
        }
    }

    pub(super) fn lsp_request_hover(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "LSP: buffer has no file".into();
            return;
        };
        let line = self.cursor.line;
        let col = self.cursor.col;
        if !self.lsp.request_hover(&path, line, col) {
            self.status_msg = "LSP: not active for this buffer".into();
        }
    }

    pub fn line_diagnostics(&self, line: usize) -> Vec<&Diagnostic> {
        let Some(path) = self.buffer.path.as_ref() else { return Vec::new(); };
        let Some(diags) = self.lsp.diagnostics_for(path) else { return Vec::new(); };
        diags
            .iter()
            .filter(|d| d.line == line)
            .collect()
    }

    pub fn worst_diagnostic(&self, line: usize) -> Option<Severity> {
        let mut worst: Option<Severity> = None;
        for d in self.line_diagnostics(line) {
            worst = match (worst, d.severity) {
                (None, s) => Some(s),
                (Some(Severity::Error), _) => Some(Severity::Error),
                (_, Severity::Error) => Some(Severity::Error),
                (Some(Severity::Warning), _) => Some(Severity::Warning),
                (_, Severity::Warning) => Some(Severity::Warning),
                (Some(s), _) => Some(s),
            };
        }
        worst
    }

    /// Diagnostics overlapping the cursor position, serialised in the LSP
    /// JSON shape so we can pass them straight to `textDocument/codeAction`'s
    /// `context.diagnostics` field. Empty when nothing's there.
    pub(super) fn diagnostics_at_cursor_for_lsp(&self) -> Vec<JsonValue> {
        let Some(path) = self.buffer.path.as_deref() else { return Vec::new(); };
        let Some(diags) = self.lsp.diagnostics_for(path) else { return Vec::new(); };
        let line = self.cursor.line;
        let col = self.cursor.col;
        diags
            .iter()
            .filter(|d| {
                let on_line = d.line <= line && line <= d.end_line;
                if !on_line {
                    return false;
                }
                if d.line == d.end_line {
                    col >= d.col && col <= d.end_col
                } else {
                    true
                }
            })
            .map(|d| {
                let severity = match d.severity {
                    Severity::Error => 1,
                    Severity::Warning => 2,
                    Severity::Info => 3,
                    Severity::Hint => 4,
                };
                serde_json::json!({
                    "range": {
                        "start": { "line": d.line, "character": d.col },
                        "end": { "line": d.end_line, "character": d.end_col },
                    },
                    "severity": severity,
                    "message": d.message,
                })
            })
            .collect()
    }

    fn open_code_actions_picker(&mut self, items: Vec<CodeActionItem>) {
        if items.is_empty() {
            self.status_msg = "LSP: no code actions".into();
            return;
        }
        let entries: Vec<(String, PickerPayload)> = items
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let mut display = match &a.kind {
                    Some(k) if !k.is_empty() => format!("[{}] {}", k, a.title),
                    _ => a.title.clone(),
                };
                if let Some(reason) = &a.disabled_reason {
                    display.push_str(&format!(" — disabled: {reason}"));
                }
                (display, PickerPayload::CodeActionIdx(i))
            })
            .collect();
        self.pending_code_actions = items;
        let mut state = PickerState::new(
            PickerKind::CodeActions,
            "Code actions".into(),
            entries,
        );
        state.refilter();
        self.picker = Some(state);
        self.mode = Mode::Picker;
    }

    /// Apply a chosen code action — runs its embedded `WorkspaceEdit` (if
    /// any) then surfaces a status note. Multi-file edits are supported by
    /// switching buffers, applying, saving, and restoring.
    pub(super) fn run_code_action(&mut self, idx: usize) {
        let Some(action) = self.pending_code_actions.get(idx).cloned() else { return; };
        if let Some(reason) = action.disabled_reason {
            self.status_msg = format!("disabled: {reason}");
            return;
        }
        let mut applied = false;
        if let Some(edit) = action.edit.as_ref() {
            match self.apply_workspace_edit(edit) {
                Ok((edits, files)) if edits > 0 => {
                    self.status_msg = format!(
                        "applied {edits} edit{} across {files} file{}",
                        if edits == 1 { "" } else { "s" },
                        if files == 1 { "" } else { "s" },
                    );
                    applied = true;
                }
                Ok(_) => {}
                Err(e) => {
                    self.status_msg = format!("error: {e}");
                    return;
                }
            }
        }
        // Some servers ship code actions as a `Command` rather than a
        // `WorkspaceEdit`. Fire `workspace/executeCommand`; the server
        // typically pushes the effect back through a follow-up
        // `workspace/applyEdit` request, which the main loop handles via
        // `LspEvent::ApplyEditRequest`.
        if let Some(cmd) = action.command.as_ref() {
            if let Some(path) = self.buffer.path.clone() {
                if self.lsp.execute_command(&path, cmd) {
                    if !applied {
                        self.status_msg = format!("running '{}'…", action.title);
                    }
                    return;
                }
            }
        }
        if !applied {
            self.status_msg = format!("'{}' had no edits", action.title);
        }
    }

    /// Apply a `WorkspaceEdit` JSON value to disk and to any open buffers.
    /// Returns (total edits, distinct files affected). Saves each modified
    /// buffer so the LSP server sees the result on its next didChange.
    fn apply_workspace_edit(&mut self, edit: &JsonValue) -> Result<(usize, usize)> {
        let mut grouped: Vec<(PathBuf, Vec<JsonValue>)> = Vec::new();
        let mut push = |path: PathBuf, edits: Vec<JsonValue>| {
            if let Some(slot) = grouped.iter_mut().find(|(p, _)| *p == path) {
                slot.1.extend(edits);
            } else {
                grouped.push((path, edits));
            }
        };
        if let Some(doc_changes) = edit.get("documentChanges").and_then(|v| v.as_array()) {
            for ch in doc_changes {
                let Some(uri) = ch
                    .get("textDocument")
                    .and_then(|d| d.get("uri"))
                    .and_then(|v| v.as_str())
                else { continue };
                let Some(path) = crate::lsp::uri_to_path(uri) else { continue };
                let Some(edits) = ch.get("edits").and_then(|v| v.as_array()) else { continue };
                push(path, edits.clone());
            }
        } else if let Some(changes) = edit.get("changes").and_then(|v| v.as_object()) {
            for (uri, v) in changes {
                let Some(path) = crate::lsp::uri_to_path(uri) else { continue };
                let Some(edits) = v.as_array() else { continue };
                push(path, edits.clone());
            }
        }
        if grouped.is_empty() {
            return Ok((0, 0));
        }

        let original_active = self.active;
        let mut total_edits = 0usize;
        let files = grouped.len();
        for (path, edits) in grouped {
            self.open_buffer(path.clone())?;
            self.history.record(&self.buffer.rope, self.cursor);
            let mut concrete: Vec<(usize, usize, String)> = Vec::with_capacity(edits.len());
            for e in &edits {
                let Some(range) = e.get("range") else { continue };
                let s = range.get("start");
                let n = range.get("end");
                let (Some(s), Some(n)) = (s, n) else { continue };
                let s_line = s.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let s_col = s.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let e_line = n.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let e_col = n.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let new_text = e.get("newText").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let s_idx = self.buffer.pos_to_char(s_line, s_col);
                let e_idx = self.buffer.pos_to_char(e_line, e_col);
                concrete.push((s_idx, e_idx, new_text));
            }
            // Apply in reverse position order so earlier edits don't shift later offsets.
            concrete.sort_by(|a, b| b.0.cmp(&a.0));
            for (s, e, text) in &concrete {
                if *e > *s {
                    self.buffer.delete_range(*s, *e);
                }
                self.buffer.insert_at_idx(*s, text);
            }
            total_edits += concrete.len();
            self.clamp_cursor_normal();
            // Save so the LSP picks up the new contents.
            let _ = self.buffer.save();
        }
        // Restore the original active buffer so the user lands back where
        // they were when they invoked the action.
        if original_active < self.buffers.len() && self.active != original_active {
            let _ = self.switch_to(original_active);
        }
        Ok((total_edits, files))
    }

    /// Build a picker out of `textDocument/documentSymbol` results.
    fn open_symbols_picker(&mut self, items: Vec<SymbolItem>) {
        if items.is_empty() {
            self.status_msg = "LSP: no symbols".into();
            return;
        }
        let active_path = self.buffer.path.clone();
        let entries: Vec<(String, PickerPayload)> = items
            .into_iter()
            .map(|s| {
                let display = if s.container.is_empty() {
                    format!("{} {} :{}", s.kind, s.name, s.line + 1)
                } else {
                    format!("{} {} › {} :{}", s.kind, s.container, s.name, s.line + 1)
                };
                let path = if s.path.as_os_str().is_empty() {
                    active_path.clone().unwrap_or_default()
                } else {
                    s.path
                };
                (
                    display,
                    PickerPayload::Location {
                        path,
                        line: s.line + 1,
                        col: s.col + 1,
                    },
                )
            })
            .collect();
        let mut state = PickerState::new(
            PickerKind::DocumentSymbols,
            "Doc symbols".into(),
            entries,
        );
        state.refilter();
        self.picker = Some(state);
        self.mode = Mode::Picker;
    }

    /// Replace the current workspace-symbols picker's items with fresh
    /// server-side results. No-op if the user already closed it.
    fn update_workspace_symbols_picker(&mut self, items: Vec<SymbolItem>) {
        let Some(picker) = self.picker.as_mut() else { return; };
        if !matches!(picker.kind, PickerKind::WorkspaceSymbols) {
            return;
        }
        let entries: Vec<(String, PickerPayload)> = items
            .into_iter()
            .map(|s| {
                let display = if s.container.is_empty() {
                    format!(
                        "{} {} :{} {}",
                        s.kind,
                        s.name,
                        s.line + 1,
                        s.path.display()
                    )
                } else {
                    format!(
                        "{} {} › {} :{} {}",
                        s.kind,
                        s.container,
                        s.name,
                        s.line + 1,
                        s.path.display()
                    )
                };
                (
                    display,
                    PickerPayload::Location {
                        path: s.path,
                        line: s.line + 1,
                        col: s.col + 1,
                    },
                )
            })
            .collect();
        crate::picker::replace_items(picker, entries);
    }
}

/// Resolve a TextMate-style LSP snippet into plain text and the char
/// offset of the first tab stop. Recognises `$N`, `${N}`, `${N:default}`,
/// `$0`, and `\$` for escaping. Anything more exotic (regex transforms,
/// choice lists) is left untouched — landing the cursor at `$1` and
/// expanding defaults covers the >95% of snippets servers emit.
///
/// Returns `(resolved_text, first_stop_char_offset)`. The offset prefers
/// `$1`; if no `$1` exists it falls back to `$0`; otherwise the cursor
/// lands at the end of the resolved text.
/// Expand a TextMate snippet template into its literal text + the ordered
/// list of tab-stop char offsets (sorted by stop index, with `$0` last).
///
/// Tab cycling consumes the full ordered list: the caller stores it on the
/// app, lands the cursor at `stops[0]`, and on `Tab` advances to the next
/// entry. The first-occurrence-only dedup is intentional — mirrored `$N`
/// references should track each other (we don't want to tab through them
/// individually), so the second `$1` is dropped here.
pub(super) fn expand_snippet(template: &str) -> (String, Vec<usize>) {
    use std::collections::HashMap;
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    // (tab_stop_index, char_offset_into_out)
    let mut stops: Vec<(u32, usize)> = Vec::new();
    // First-seen default text per stop. Subsequent bare `$N` references
    // mirror this — matches what most LSP servers expect from a snippet
    // consumer for the common `for (let ${1:i} = 0; $1 < $1.length; $1++)`
    // pattern.
    let mut defaults: HashMap<u32, String> = HashMap::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c != '$' {
            out.push(c);
            i += 1;
            continue;
        }
        let next = chars.get(i + 1).copied();
        match next {
            Some(d) if d.is_ascii_digit() => {
                // `$N` — read run of digits.
                let mut j = i + 1;
                let mut idx: u32 = 0;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    idx = idx.saturating_mul(10).saturating_add(chars[j] as u32 - '0' as u32);
                    j += 1;
                }
                let here = out.chars().count();
                if let Some(def) = defaults.get(&idx) {
                    out.push_str(def);
                }
                stops.push((idx, here));
                i = j;
            }
            Some('{') => {
                // `${N}` or `${N:default}` — find the matching `}`.
                let mut j = i + 2;
                let mut idx: u32 = 0;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    idx = idx.saturating_mul(10).saturating_add(chars[j] as u32 - '0' as u32);
                    j += 1;
                }
                let here = out.chars().count();
                let mut default_text = String::new();
                if chars.get(j) == Some(&':') {
                    j += 1;
                    while j < chars.len() && chars[j] != '}' {
                        if chars[j] == '\\' && j + 1 < chars.len() {
                            default_text.push(chars[j + 1]);
                            j += 2;
                            continue;
                        }
                        default_text.push(chars[j]);
                        j += 1;
                    }
                }
                if chars.get(j) == Some(&'}') {
                    j += 1;
                }
                if default_text.is_empty() {
                    if let Some(prev) = defaults.get(&idx) {
                        out.push_str(prev);
                    }
                } else {
                    out.push_str(&default_text);
                    defaults.entry(idx).or_insert(default_text);
                }
                stops.push((idx, here));
                i = j;
            }
            _ => {
                out.push('$');
                i += 1;
            }
        }
    }
    // First-occurrence dedup so mirrored `$N` references collapse to a
    // single tab stop. `$0` is the final landing position so it sorts
    // after all positive indices regardless of source order.
    let mut seen: HashMap<u32, ()> = HashMap::new();
    let mut ordered: Vec<(u32, usize)> = Vec::new();
    for (idx, off) in stops {
        if seen.insert(idx, ()).is_none() {
            ordered.push((idx, off));
        }
    }
    ordered.sort_by_key(|(idx, _)| if *idx == 0 { u32::MAX } else { *idx });
    let stop_offsets: Vec<usize> = ordered.into_iter().map(|(_, off)| off).collect();
    (out, stop_offsets)
}

#[cfg(test)]
mod tests {
    use super::expand_snippet;

    #[test]
    fn snippet_plain_text_passthrough() {
        assert_eq!(expand_snippet("hello").0, "hello");
        assert!(expand_snippet("hello").1.is_empty());
    }

    #[test]
    fn snippet_basic_stop() {
        let (text, stops) = expand_snippet("console.log($1)");
        assert_eq!(text, "console.log()");
        assert_eq!(stops, vec![12]); // right between the parens
    }

    #[test]
    fn snippet_default_text_expanded() {
        let (text, stops) = expand_snippet("for (let ${1:i} = 0; $1 < ${2:n}; $1++) {\n\t$0\n}");
        assert_eq!(text, "for (let i = 0; i < n; i++) {\n\t\n}");
        // $1 at "for (let " (9), $2 at "for (let i = 0; i < " (20), $0 at
        // "for (let i = 0; i < n; i++) {\n\t" — \n counts as 1 char, so
        // 9 + 17 + 1 = 27 → +1 tab + 1 newline put $0 at 31.
        assert_eq!(stops.len(), 3);
        assert_eq!(stops[0], 9);
        assert_eq!(stops[1], 20);
    }

    #[test]
    fn snippet_zero_stop_used_when_no_one() {
        let (text, stops) = expand_snippet("return $0;");
        assert_eq!(text, "return ;");
        assert_eq!(stops, vec![7]);
    }

    #[test]
    fn snippet_escaped_dollar() {
        let (text, stops) = expand_snippet("\\$keep $1");
        assert_eq!(text, "$keep ");
        assert_eq!(stops, vec![6]);
    }

    #[test]
    fn snippet_zero_sorts_last_regardless_of_source_order() {
        // $0 placed before $1 / $2 in the template must still be the
        // final tab destination.
        let (_text, stops) = expand_snippet("$0 $2 $1");
        assert_eq!(stops.len(), 3);
        // stops returned in order $1 → $2 → $0
        // offsets in output "  " — wait, $N with no default expands to
        // empty, so the output is "  " (two spaces between three empty
        // stops). Positions: $0 at 0, " " 1, $2 at 1, " " 2, $1 at 2.
        // Ordered by index 1→2→0: [2, 1, 0].
        assert_eq!(stops, vec![2, 1, 0]);
    }

    #[test]
    fn snippet_mirrored_stop_dedups_to_first_occurrence() {
        // `$1` appears 3 times. Only the first occurrence is a tab stop;
        // the others are mirrors and should not produce extra stops.
        let (_text, stops) = expand_snippet("$1.foo($1, $1)");
        assert_eq!(stops.len(), 1);
        assert_eq!(stops[0], 0);
    }
}

/// Narrow a server-returned completion list to entries that match what the
/// user has actually typed. Matches case-insensitively against `filter_text`
/// (falls back to label inside the item itself), grouped by tier: prefix
/// matches first, then substring, then subsequence (fuzzy). Within each tier
/// the server's `sort_text` decides order — that's how typescript-language-
/// server signals that `document` outranks `documentElement` for prefix
/// `docu`. Capped to 200 visible items after filtering. An empty prefix
/// passes everything through, sorted by `sort_text`.
fn filter_completion_items(items: Vec<CompletionItem>, prefix: &str) -> Vec<CompletionItem> {
    const VISIBLE_CAP: usize = 200;
    if prefix.is_empty() {
        let mut sorted = items;
        sorted.sort_by(|a, b| a.sort_text.cmp(&b.sort_text));
        sorted.truncate(VISIBLE_CAP);
        return sorted;
    }
    let needle = prefix.to_lowercase();
    let mut tiered: Vec<(u8, CompletionItem)> = items
        .into_iter()
        .filter_map(|item| {
            let hay = item.filter_text.to_lowercase();
            let tier = if hay.starts_with(&needle) {
                0
            } else if hay.contains(&needle) {
                1
            } else if subsequence_match(&hay, &needle) {
                2
            } else {
                return None;
            };
            Some((tier, item))
        })
        .collect();
    tiered.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.sort_text.cmp(&b.1.sort_text)));
    tiered.truncate(VISIBLE_CAP);
    tiered.into_iter().map(|(_, item)| item).collect()
}

/// True if every char of `needle` appears in `hay` in order (not necessarily
/// contiguous). Both inputs should already be lowercased.
fn subsequence_match(hay: &str, needle: &str) -> bool {
    let mut hay_iter = hay.chars();
    'outer: for nc in needle.chars() {
        for hc in hay_iter.by_ref() {
            if hc == nc {
                continue 'outer;
            }
        }
        return false;
    }
    true
}
