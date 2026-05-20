//! LSP event handling and the request-side helpers — completion popup
//! plumbing, hover, goto, references, signature help, code actions,
//! workspace edits, rename prompt, and the diagnostics->`:health` glue.

use anyhow::Result;
use serde_json::Value as JsonValue;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::lsp::{
    CodeActionItem, CompletionItem, Diagnostic, LocationItem, LspEvent, Severity, SymbolItem,
};
use crate::mode::Mode;
use crate::picker::{PickerKind, PickerPayload, PickerState};

use super::state::{
    CODE_LENS_EMPTY_RETRY, CompletionState, HoverState, LSP_SYNC_DEBOUNCE, PendingRefAugment,
};

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
                    self.window.cursor.line = line;
                    self.window.cursor.col = col;
                    self.window.cursor.want_col = col;
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
                    self.present_references(items);
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
                LspEvent::Rename { edit } => {
                    self.open_rename_preview(&edit);
                }
                LspEvent::ApplyEditRequest {
                    client_key,
                    id,
                    edit,
                } => {
                    // Preview-flag opt-in: route the server-initiated edit
                    // through `Mode::RenamePreview` and reply when the user
                    // accepts / cancels (or right away if there's nothing
                    // to apply or a preview is already on screen).
                    let routed = self.config.lsp.preview_workspace_edits
                        && self.open_server_apply_edit_preview(
                            client_key.clone(),
                            id,
                            client_key.clone(),
                            &edit,
                        );
                    if !routed {
                        let applied = match self.apply_workspace_edit(&edit) {
                            Ok((edits, _)) => edits > 0,
                            Err(_) => false,
                        };
                        self.lsp.send_apply_edit_response(&client_key, id, applied);
                    }
                }
                LspEvent::DiagnosticsUpdated => {}
                LspEvent::InlayHints { path, hints } => {
                    self.inlay_hints_in_flight.remove(&path);
                    if hints.is_empty() {
                        self.inlay_hints.remove(&path);
                    } else {
                        self.inlay_hints.insert(path, hints);
                    }
                }
                LspEvent::SemanticTokens {
                    path,
                    buffer_version,
                    tokens,
                } => {
                    self.semantic_tokens_in_flight.remove(&path);
                    // Drop the reply if the buffer has moved on — the
                    // token col indices are anchored to the version we
                    // asked for, and re-anchoring them against a newer
                    // buffer would mis-align them.
                    let live_version = self
                        .buffer
                        .path
                        .as_ref()
                        .filter(|p| *p == &path)
                        .map(|_| self.buffer.version);
                    let stale = match live_version {
                        Some(v) => v != buffer_version,
                        // Different buffer is active — accept and cache;
                        // when the user switches back, the cache is
                        // still valid for the version it was built on.
                        None => false,
                    };
                    if stale || tokens.is_empty() {
                        self.semantic_tokens.remove(&path);
                    } else {
                        // Bin by line so the renderer doesn't walk the
                        // full token list per row. Tokens already
                        // arrive in line-then-col order from the decode.
                        let line_count = tokens
                            .iter()
                            .map(|t| t.line)
                            .max()
                            .map(|m| m + 1)
                            .unwrap_or(0);
                        let mut by_line: Vec<Vec<crate::lsp::SemanticToken>> =
                            vec![Vec::new(); line_count];
                        for tok in tokens {
                            let line = tok.line;
                            if line < by_line.len() {
                                by_line[line].push(tok);
                            }
                        }
                        self.semantic_tokens.insert(
                            path,
                            crate::app::SemanticTokensCache {
                                buffer_version,
                                by_line,
                            },
                        );
                    }
                }
                LspEvent::CodeLens {
                    path,
                    buffer_version,
                    lenses,
                } => {
                    self.code_lens_in_flight.remove(&path);
                    // Drop stale replies — when the buffer has moved
                    // past the version the request was anchored on,
                    // the lens line indices no longer line up. A
                    // different active buffer still gets cached at
                    // the version we asked for; the renderer will
                    // re-check on draw.
                    let live_version = self
                        .buffer
                        .path
                        .as_ref()
                        .filter(|p| *p == &path)
                        .map(|_| self.buffer.version);
                    let stale = match live_version {
                        Some(v) => v != buffer_version,
                        None => false,
                    };
                    if !stale {
                        // Servers that defer titles to `codeLens/resolve`
                        // (csharp-ls, OmniSharp, …) hand us items with
                        // an absent or empty `command.title`. Fire a
                        // resolve per such item right after caching the
                        // batch — responses patch the slot in place via
                        // `LspEvent::CodeLensResolved` and trigger a
                        // re-merge. Only worth doing when the server
                        // advertised `resolveProvider: true`; otherwise
                        // the request would error.
                        let wants_resolve = self.lsp.code_lens_resolve_capability(&path);
                        // Always record the LSP answer — even an
                        // empty array is meaningful: it says "I have
                        // no lenses for this buffer version" and
                        // gates the retry check.
                        self.lsp_only_code_lens
                            .insert(path.clone(), (buffer_version, lenses.clone()));
                        self.refresh_merged_code_lens(&path);
                        if wants_resolve {
                            for (idx, lens) in lenses.iter().enumerate() {
                                let needs = lens
                                    .command
                                    .as_ref()
                                    .map(|c| c.title.is_empty())
                                    .unwrap_or(true);
                                if !needs {
                                    continue;
                                }
                                if lens.raw.is_null() {
                                    continue;
                                }
                                self.lsp.request_code_lens_resolve(
                                    &path,
                                    lens.raw.clone(),
                                    buffer_version,
                                    idx,
                                );
                            }
                        }
                    }
                }
                LspEvent::CodeLensResolved {
                    path,
                    buffer_version,
                    lens_index,
                    command,
                } => {
                    // Patch the slot only when the original batch is
                    // still in the cache. A later `textDocument/codeLens`
                    // reply could have replaced it; in that case the
                    // resolve answer is for a now-gone item and would
                    // mis-attribute the title to whatever lives at
                    // `lens_index` now.
                    let still_current = self
                        .lsp_only_code_lens
                        .get(&path)
                        .map(|(v, _)| *v == buffer_version)
                        .unwrap_or(false);
                    if !still_current {
                        continue;
                    }
                    if let Some((_, list)) = self.lsp_only_code_lens.get_mut(&path) {
                        if let Some(item) = list.get_mut(lens_index) {
                            item.command = command;
                        }
                    }
                    self.refresh_merged_code_lens(&path);
                }
                LspEvent::DocumentHighlights {
                    path,
                    anchor_line,
                    anchor_col,
                    anchor_version,
                    ranges,
                } => {
                    // Free the in-flight slot for this path so the
                    // next idle render can fire for wherever the
                    // cursor has moved to in the meantime.
                    self.document_highlight_in_flight.remove(&path);
                    // Always store the cache — even when `ranges` is
                    // empty — so the cache-anchor check has a "we
                    // already asked this anchor" signal. The
                    // renderer's `line_document_highlights` already
                    // returns empty when `ranges` is empty, so an
                    // empty cache draws nothing.
                    self.document_highlights.insert(
                        path,
                        crate::app::DocumentHighlightCache {
                            anchor_line,
                            anchor_col,
                            anchor_version,
                            ranges,
                        },
                    );
                }
                LspEvent::ServerMessage {
                    client_key,
                    severity,
                    text,
                    is_show,
                } => {
                    self.handle_lsp_server_message(client_key, severity, text, is_show);
                }
                LspEvent::CopilotStatus { kind, user } => {
                    self.apply_copilot_status(kind, user);
                }
                LspEvent::CopilotInline {
                    path,
                    line,
                    col,
                    replace_start_line,
                    replace_start_col,
                    text,
                    buffer_version,
                } => {
                    // Drop the suggestion if the buffer or cursor have
                    // moved on since the request — a stale ghost would
                    // either render against the wrong byte range or
                    // accept-insert into the wrong place.
                    let stale = self.buffer.path.as_deref() != Some(&path)
                        || self.buffer.version != buffer_version
                        || self.window.cursor.line != line
                        || self.window.cursor.col != col
                        || !matches!(self.mode, crate::mode::Mode::Insert);
                    if !stale {
                        self.copilot_ghost = Some(crate::app::CopilotGhost {
                            text,
                            line,
                            col,
                            replace_start_line,
                            replace_start_col,
                            path,
                        });
                    }
                }
                LspEvent::RequestFailed { kind, path } => {
                    if let Some(p) = path {
                        match kind {
                            "InlayHints" => {
                                self.inlay_hints_in_flight.remove(&p);
                            }
                            "DocumentHighlight" => {
                                self.document_highlight_in_flight.remove(&p);
                            }
                            "SemanticTokens" => {
                                self.semantic_tokens_in_flight.remove(&p);
                            }
                            "CodeLens" => {
                                self.code_lens_in_flight.remove(&p);
                            }
                            _ => {}
                        }
                    }
                    if kind == "References" {
                        // Server errored on references — still run the
                        // Razor grep augment if it was queued.
                        if self.pending_ref_augment.is_some() {
                            self.present_references(Vec::new());
                        }
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
                    } else if kind == "copilot inline" {
                        // Copilot returns no suggestion on most idle
                        // pauses — that's normal, not an error worth
                        // shouting about. Swallow it; the ghost render
                        // path already shows nothing when there's
                        // nothing to show.
                    } else if kind == "references" {
                        // Hand off to the merge step — it'll fold in
                        // the Razor grep augment if one was queued,
                        // or post the "no references" status if both
                        // sources are empty.
                        self.present_references(Vec::new());
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
                        .pos_to_char(self.window.cursor.line, self.window.cursor.col);
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
        let line = self.window.cursor.line;
        let mut col = self.window.cursor.col;
        while col > 0 {
            let prev = self.buffer.char_at(line, col - 1).unwrap_or(' ');
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
        let line = self.window.cursor.line;
        let col = self.window.cursor.col;
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
        let line = self.window.cursor.line;
        let col = self.window.cursor.col;
        // Pre-fill with the current word so common renames are a few-char edit.
        let current = self.word_under_cursor().unwrap_or_default();
        self.rename_anchor = Some((path, line, col, current.clone()));
        self.cmdline_cursor = current.len();
        self.cmdline = current;
        self.mode = Mode::Prompt(crate::mode::PromptKind::Rename);
    }

    /// Open the literal-string replace-all prompt. Source of the
    /// search term:
    ///
    ///   - Visual mode → the selected text. Newline-spanning
    ///     selections are rejected (substitute is a line-oriented op
    ///     against a literal needle).
    ///   - Normal mode → the word under the cursor.
    ///
    /// Stashes the term in `rename_anchor` so the prompt's key handler
    /// can pass it to `finish_replace_all`. Pre-fills the cmdline with
    /// the term so the user can edit instead of retyping.
    pub(super) fn start_replace_all_prompt(&mut self) {
        let current = if let Mode::Visual(kind) = self.mode {
            let (start, end, _) = self.visual_range_chars(kind);
            if end <= start {
                self.status_msg = "replace: empty selection".into();
                self.exit_visual();
                return;
            }
            let text: String = self.buffer.rope.slice(start..end).to_string();
            // Strip a trailing newline (linewise selections include the
            // closing `\n`) so the substitute below matches actual
            // content on the line rather than line-break-anchored runs.
            let text = text.trim_end_matches('\n').to_string();
            if text.contains('\n') {
                self.status_msg = "replace: selection spans multiple lines (not supported)".into();
                self.exit_visual();
                return;
            }
            if text.is_empty() {
                self.status_msg = "replace: empty selection".into();
                self.exit_visual();
                return;
            }
            self.exit_visual();
            text
        } else {
            let Some(word) = self.word_under_cursor() else {
                self.status_msg = "No word under cursor".into();
                return;
            };
            word
        };
        // We reuse `rename_anchor` to carry the original term — the path
        // / line / col fields are unused for replace-all but the tuple
        // is the only place a prompt action has to stash arbitrary data
        // alongside the typed string.
        let placeholder = self.buffer.path.clone().unwrap_or_default();
        self.rename_anchor = Some((placeholder, 0, 0, current.clone()));
        self.cmdline_cursor = current.len();
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
        self.history.record(&self.buffer.rope, self.window.cursor);
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
            format!("{n} replacement{}", if n == 1 { "" } else { "s" })
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
        let line = self.window.cursor.line;
        let col = self.window.cursor.col;

        // Stash the C# / Razor augment context up front — it survives
        // both success and failure paths so the merge step can fire
        // regardless of how the LSP reply arrives.
        self.pending_ref_augment = razor_ref_augment(&path, self.word_under_cursor());

        if self.lsp.request_references(&path, line, col) {
            return;
        }
        // No LSP client attached to this buffer. If we still have an
        // augment to grep, present the grep-only list; otherwise this
        // is a true "nothing to do".
        if self.pending_ref_augment.is_some() {
            self.present_references(Vec::new());
        } else {
            self.status_msg = "LSP: not active for this buffer".into();
        }
    }

    /// Merge the LSP `textDocument/references` result with the Razor
    /// grep augment (if one was queued) and open the locations picker.
    /// Called from every references reply path — success, empty
    /// (`NotFound("references")`), and failure (`RequestFailed`) —
    /// so the augment fires whether the server returned matches or not.
    fn present_references(&mut self, items: Vec<LocationItem>) {
        let augment = self.pending_ref_augment.take();
        let pre_merge = items.len();
        let mut merged = items;
        if let Some(aug) = augment {
            let grep_items = run_razor_ref_grep(&aug);
            let mut seen: std::collections::HashSet<(PathBuf, usize, usize)> = merged
                .iter()
                .map(|i| (i.path.clone(), i.line, i.col))
                .collect();
            for g in grep_items {
                let key = (g.path.clone(), g.line, g.col);
                if seen.insert(key) {
                    merged.push(g);
                }
            }
        }
        // Mark the title when the Razor grep contributed extra matches
        // beyond what the LSP returned. Quiet when the augment found
        // nothing new — the picker title is the only persistent
        // notification surface for this UI.
        let added = merged.len().saturating_sub(pre_merge);
        let title = if added > 0 {
            format!("References (+{added} razor)")
        } else {
            "References".to_string()
        };
        self.open_locations_picker(&title, merged);
    }

    pub(super) fn lsp_request_signature_help(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            return;
        };
        self.lsp_sync_active();
        let line = self.window.cursor.line;
        let col = self.window.cursor.col;
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
                if c.anchor_line != self.window.cursor.line {
                    return;
                }
                let s = self.buffer.pos_to_char(c.anchor_line, c.anchor_col);
                let e = self
                    .buffer
                    .pos_to_char(self.window.cursor.line, self.window.cursor.col);
                (s.min(e), s.max(e))
            }
        };
        if end > start {
            self.buffer.delete_range(start, end);
        }
        // Snippet items go through the placeholder expander so `${1:foo}`
        // doesn't end up as literal text in the buffer. Plain items insert
        // verbatim.
        let (mut text, mut stop_offsets) = if item.is_snippet {
            expand_snippet(&item.insert_text)
        } else {
            (item.insert_text.clone(), Vec::new())
        };
        // Multi-line snippet bodies (emmet's `ul>li*3`, language-server
        // function templates) carry no indent on the continuation lines —
        // the server doesn't know what column the buffer is sitting at.
        // VS Code / Neovim prepend the current line's leading whitespace
        // to every line after the first; without it, `</ul>` lands at
        // column 0 even though `<ul>` is nested several levels deep.
        if text.contains('\n') {
            let line_idx = self.buffer.rope.char_to_line(start);
            let line_start = self.buffer.rope.line_to_char(line_idx);
            let indent: String = self
                .buffer
                .rope
                .slice(line_start..)
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            if !indent.is_empty() {
                text = indent_continuation_lines(&text, &mut stop_offsets, &indent);
            }
        }
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
        // Large-file gate: skip server attach entirely. didOpen of a
        // multi-MB buffer typically wedges tsserver / rust-analyzer /
        // gopls for minutes; the user opened this to read it, not to
        // get inlay hints on a 200k-line generated bundle.
        if self.buffer.is_large() {
            return;
        }
        let Some(path) = self.buffer.path.clone() else {
            return;
        };
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if !self.lsp.ensure_for_path(&path, &cwd) {
            return;
        }
        let text = self.buffer.rope.to_string();
        // Every attached server (primary + auxiliaries like Tailwind) needs
        // its own didOpen — each carries its own languageId, derived from
        // the spec for this path (not the client's stored one).
        self.lsp.did_open_all(&path, &text);
        self.last_sent_version.insert(path, self.buffer.version);
    }

    /// Force-flush the active buffer to every attached LSP. Used right
    /// before a request that needs fresh text (completion / hover / goto)
    /// and from `lsp_sync_active_debounced` once the burst window expires.
    pub(super) fn lsp_sync_active(&mut self) {
        if self.buffer.is_large() {
            return;
        }
        let Some(path) = self.buffer.path.clone() else {
            return;
        };
        let last = self
            .last_sent_version
            .get(&path)
            .copied()
            .unwrap_or(u64::MAX);
        if last == self.buffer.version {
            return;
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if !self.lsp.ensure_for_path(&path, &cwd) {
            return;
        }
        let text = self.buffer.rope.to_string();
        if last == u64::MAX {
            self.lsp.did_open_all(&path, &text);
        } else {
            self.lsp.did_change_all(&path, self.buffer.version, &text);
        }
        self.last_sent_version.insert(path, self.buffer.version);
        self.last_lsp_sync_at = Instant::now();
    }

    /// Render-loop sync: only flush when the last successful flush is more
    /// than `LSP_SYNC_DEBOUNCE` ago. The main loop wakes early at the
    /// deadline (see `lsp_sync_due_at`) so a short burst still flushes
    /// promptly after the user pauses.
    pub(super) fn lsp_sync_active_debounced(&mut self) {
        if self.buffer.is_large() {
            return;
        }
        let Some(path) = self.buffer.path.as_ref() else {
            return;
        };
        let last = self
            .last_sent_version
            .get(path)
            .copied()
            .unwrap_or(u64::MAX);
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
        let last = self
            .last_sent_version
            .get(path)
            .copied()
            .unwrap_or(u64::MAX);
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
        let line = self.window.cursor.line;
        let col = self.window.cursor.col;
        if !self.lsp.request_definition(&path, line, col) {
            self.status_msg = "LSP: not active for this buffer".into();
        }
    }

    /// Ask the active buffer's LSP for inlay hints — once per buffer
    /// version, capped to one in-flight per path. Throttled by both
    /// `last_inlay_request_version` (so a stable buffer doesn't ask
    /// repeatedly for the same version) and `inlay_hints_in_flight`
    /// (so rapid typing across many versions can't queue up against
    /// a slow / indexing server).
    pub(super) fn lsp_request_inlay_hints_if_due(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            return;
        };
        let version = self.buffer.version;
        let last = self
            .last_inlay_request_version
            .get(&path)
            .copied()
            .unwrap_or(u64::MAX);
        if last == version {
            return;
        }
        if self.inlay_hints_in_flight.contains(&path) {
            return;
        }
        let end_line = self.buffer.line_count();
        if self.lsp.request_inlay_hints(&path, end_line) {
            self.last_inlay_request_version
                .insert(path.clone(), version);
            self.inlay_hints_in_flight.insert(path);
        }
    }

    /// True when the active buffer has an empty lens cache for the
    /// current version AND enough time has passed since the last
    /// request to retry. Lets the main loop force a render tick at
    /// the retry deadline so the lens row appears as soon as rust-
    /// analyzer finishes indexing — the user shouldn't have to type
    /// or move the cursor for the lenses to materialise.
    pub(super) fn code_lens_retry_due(&self) -> bool {
        if !self.config.lsp.code_lens {
            return false;
        }
        let Some(path) = self.buffer.path.as_ref() else {
            return false;
        };
        if self.code_lens_in_flight.contains(path) {
            return false;
        }
        let last_version = match self.last_code_lens_request_version.get(path).copied() {
            Some(v) => v,
            None => return false,
        };
        if last_version != self.buffer.version {
            return false;
        }
        // Retry only when the LSP itself hasn't given us a non-empty
        // answer yet for the current version. A non-empty synth-only
        // cache shouldn't suppress the retry — the LSP might still
        // come online with its own lenses (references, etc.).
        let lsp_has_content = self
            .lsp_only_code_lens
            .get(path)
            .map(|(v, l)| *v == self.buffer.version && !l.is_empty())
            .unwrap_or(false);
        if lsp_has_content {
            return false;
        }
        self.last_code_lens_request_at
            .get(path)
            .map(|t| t.elapsed() >= CODE_LENS_EMPTY_RETRY)
            .unwrap_or(true)
    }

    /// Run the tree-sitter synth pass for the active buffer if the
    /// version has moved since we last walked. Merges results into
    /// the merged `code_lens` cache. No-op when `lsp.code_lens` is
    /// disabled or the language doesn't have a synth implementation.
    pub(super) fn synth_code_lens_if_due(&mut self) {
        if !self.config.lsp.code_lens {
            return;
        }
        let Some(path) = self.buffer.path.clone() else {
            return;
        };
        let version = self.buffer.version;
        if self.last_synth_lens_version.get(&path) == Some(&version) {
            return;
        }
        let Some(lang) = crate::lang::Lang::detect(&path) else {
            self.last_synth_lens_version.insert(path, version);
            return;
        };
        // Languages without a synth implementation (everything outside
        // JS/TS/TSX) bail before touching `synth_only_code_lens` and
        // before triggering a `refresh_merged_code_lens` pass. The old
        // shape inserted an empty Vec on every keystroke, which still
        // ran the merge and briefly collapsed any cached LSP lenses to
        // empty — that's what made `.cs` (csharp-ls) and `.cshtml`
        // (OmniSharp) buffers reflow on each Enter.
        let Some(synth) = crate::code_lens_synth::synthesize_lenses(lang, &self.buffer) else {
            self.last_synth_lens_version.insert(path, version);
            return;
        };
        self.last_synth_lens_version.insert(path.clone(), version);
        self.synth_only_code_lens
            .insert(path.clone(), (version, synth));
        self.refresh_merged_code_lens(&path);
    }

    /// Rebuild `code_lens[path]` as the union of `lsp_only_code_lens`
    /// and `synth_only_code_lens`. Neither source is version-gated:
    /// `synth_code_lens_if_due` runs synchronously on every keystroke
    /// at the new version, while the LSP half only refreshes when the
    /// server responds (50–200 ms after a `didChange`). Filtering both
    /// halves on the live version would briefly collapse the merged
    /// cache to "synth-only" on each Enter, which drops the LSP-side
    /// anchors — and with them the per-anchor phantom row that
    /// `visible_rows_between` counts. The viewport then jumps up,
    /// then back down once the LSP response lands. Keeping the stale
    /// LSP half in the merge means anchor positions may sit at
    /// slightly outdated line numbers for a frame, but the row count
    /// stays steady so the screen doesn't reflow.
    pub(super) fn refresh_merged_code_lens(&mut self, path: &std::path::Path) {
        let version = self.buffer_version_for_path(path);
        let lsp_part: Vec<crate::lsp::CodeLensItem> = self
            .lsp_only_code_lens
            .get(path)
            .map(|(_, l)| l.clone())
            .unwrap_or_default();
        let synth_part: Vec<crate::lsp::CodeLensItem> = self
            .synth_only_code_lens
            .get(path)
            .map(|(_, l)| l.clone())
            .unwrap_or_default();
        if lsp_part.is_empty() && synth_part.is_empty() {
            self.code_lens.remove(path);
            return;
        }
        let mut merged = lsp_part;
        merged.extend(synth_part);
        let anchor_lines = merged.iter().map(|l| l.line).collect();
        self.code_lens.insert(
            path.to_path_buf(),
            crate::app::CodeLensCache {
                buffer_version: version,
                lenses: merged,
                anchor_lines,
            },
        );
    }

    /// Look up the live buffer version for `path`. For the active
    /// buffer this matches `self.buffer.version`; for inactive
    /// stashed buffers we don't refresh the cache anyway, so falling
    /// back to whatever version the cache thinks it has is fine.
    fn buffer_version_for_path(&self, path: &std::path::Path) -> u64 {
        if self.buffer.path.as_deref() == Some(path) {
            return self.buffer.version;
        }
        // Inactive panes don't get a code-lens cache refresh today
        // (we only request for the active buffer). Returning the
        // existing cached version means a stale `code_lens` entry
        // for a switched-away buffer keeps painting correctly.
        self.code_lens
            .get(path)
            .map(|c| c.buffer_version)
            .unwrap_or(0)
    }

    /// Fire `textDocument/codeLens` once per buffer version, capped
    /// to one in-flight per path. Same dual-throttle shape as inlay
    /// hints / semantic tokens — version dedup for the stable case,
    /// in-flight dedup for the fast-typing-against-a-busy-server
    /// case. Gated behind `serverCapabilities.codeLensProvider` so
    /// servers that don't advertise the capability never see the
    /// request.
    ///
    /// One extra rule on top of the inlay-hints / semantic-tokens
    /// pattern: when the cache is empty (no entry, i.e. the server
    /// returned `[]` last time) we allow a retry every
    /// `CODE_LENS_EMPTY_RETRY` even when the version hasn't moved.
    /// rust-analyzer routinely returns an empty array while it's
    /// still indexing the workspace; without this slow retry the
    /// version-dedupe would pin us at "already asked for v0" and the
    /// lens row would never show up unless the user edited the
    /// buffer (which bumps the version and re-fires naturally).
    pub(super) fn lsp_request_code_lens_if_due(&mut self) {
        if !self.config.lsp.code_lens {
            return;
        }
        let Some(path) = self.buffer.path.clone() else {
            return;
        };
        // Razor opts out — see the matching guard in
        // `synth_code_lens_if_due` for why. Skipping the request keeps
        // `code_lens[path]` empty so the renderer never reserves a
        // phantom row that would later vanish on the next keystroke.
        if matches!(
            crate::lang::Lang::detect(&path),
            Some(crate::lang::Lang::Razor)
        ) {
            return;
        }
        let version = self.buffer.version;
        let last = self
            .last_code_lens_request_version
            .get(&path)
            .copied()
            .unwrap_or(u64::MAX);
        if self.code_lens_in_flight.contains(&path) {
            return;
        }
        if last == version {
            let cache_present = self.code_lens.contains_key(&path);
            if cache_present {
                // Already have a non-empty answer for this version —
                // nothing more to ask for until an edit or a manual
                // refresh moves us off.
                return;
            }
            // Empty-cache slow retry: throttle to one request per
            // `CODE_LENS_EMPTY_RETRY` so we don't spam the server
            // while it's still cold-starting.
            let due = self
                .last_code_lens_request_at
                .get(&path)
                .map(|t| t.elapsed() >= CODE_LENS_EMPTY_RETRY)
                .unwrap_or(true);
            if !due {
                return;
            }
        }
        if self.lsp.request_code_lens(&path, version) {
            self.last_code_lens_request_version
                .insert(path.clone(), version);
            self.last_code_lens_request_at
                .insert(path.clone(), Instant::now());
            self.code_lens_in_flight.insert(path);
        }
    }

    /// Fire `textDocument/semanticTokens/full` once per buffer version,
    /// capped to one in-flight per path. Same dual-throttle shape as
    /// inlay hints — version dedup for the stable case, in-flight
    /// dedup for the fast-typing-against-a-busy-server case.
    pub(super) fn lsp_request_semantic_tokens_if_due(&mut self) {
        if !self.config.lsp.semantic_tokens {
            return;
        }
        let Some(path) = self.buffer.path.clone() else {
            return;
        };
        let version = self.buffer.version;
        let last = self
            .last_semantic_tokens_request_version
            .get(&path)
            .copied()
            .unwrap_or(u64::MAX);
        if last == version {
            return;
        }
        if self.semantic_tokens_in_flight.contains(&path) {
            return;
        }
        if self.lsp.request_semantic_tokens_full(&path, version) {
            self.last_semantic_tokens_request_version
                .insert(path.clone(), version);
            self.semantic_tokens_in_flight.insert(path);
        }
    }

    /// Fire `textDocument/documentHighlight` when the cursor has landed
    /// on a position the server hasn't been asked about yet for the
    /// current buffer version. Only one request is allowed in flight
    /// per buffer path at a time — fast cursor movement while the
    /// server is busy (cold-start indexing in particular) would
    /// otherwise queue hundreds of requests against a server that
    /// can't drain them, and we'd never catch up. Intermediate cursor
    /// positions get skipped when the user moves fast; once the
    /// in-flight request returns, the next idle render fires for
    /// wherever the cursor has settled.
    pub(super) fn lsp_request_document_highlight_if_due(&mut self) {
        if !self.config.lsp.document_highlight {
            return;
        }
        // Don't fire while a popup / picker is up (those overlays
        // suspend the cursor's editing meaning) or in Insert mode
        // (we'd be requesting on every keystroke and the user can't
        // see the highlights through the typing flow anyway).
        if !matches!(
            self.mode,
            crate::mode::Mode::Normal | crate::mode::Mode::Visual(_)
        ) {
            return;
        }
        if self.picker.is_some() || self.completion.is_some() {
            return;
        }
        let Some(path) = self.buffer.path.clone() else {
            return;
        };
        let line = self.window.cursor.line;
        let col = self.window.cursor.col;
        let version = self.buffer.version;
        // Skip if the cache covers the current cursor position — we
        // already have the answer for this symbol and the renderer is
        // painting it. Buffer version still has to match exactly so
        // an edit invalidates the column indices and forces a refetch.
        if let Some(cache) = self.document_highlights.get(&path) {
            if cache.anchor_version == version
                && cache.ranges.iter().any(|r| {
                    r.start_line == line
                        && line == r.end_line
                        && col >= r.start_col
                        && col < r.end_col
                })
            {
                return;
            }
        }
        // Skip if any documentHighlight request for this path is
        // already in flight. The response handler clears the marker,
        // so the next render after the reply fires for wherever the
        // cursor has moved to in the meantime.
        if self.document_highlight_in_flight.contains(&path) {
            return;
        }
        if self
            .lsp
            .request_document_highlight(&path, line, col, version)
        {
            self.document_highlight_in_flight.insert(path);
        }
    }

    /// Char-column ranges of document-highlight matches on `line` of
    /// `path`. Paints whenever the live cursor sits inside any of the
    /// cached ranges (i.e. still on the same symbol the request was
    /// anchored to) — moving the cursor within a multi-char identifier
    /// doesn't blink the highlights off and on between requests, and
    /// stale ranges from a previous symbol stop painting the moment
    /// the cursor leaves them. Buffer version still has to match
    /// exactly: a single edit invalidates the column indices the
    /// server returned, and re-anchoring them blind would smear
    /// highlights onto unrelated tokens.
    pub fn line_document_highlights(
        &self,
        path: &std::path::Path,
        line: usize,
    ) -> Vec<(usize, usize)> {
        let active_path = match self.buffer.path.as_deref() {
            Some(p) => p,
            None => return Vec::new(),
        };
        let Some(cache) = self.document_highlights.get(active_path) else {
            return Vec::new();
        };
        if cache.anchor_version != self.buffer.version {
            return Vec::new();
        }
        let cursor_line = self.window.cursor.line;
        let cursor_col = self.window.cursor.col;
        let cursor_in_match = cache.ranges.iter().any(|r| {
            r.start_line == cursor_line
                && cursor_line == r.end_line
                && cursor_col >= r.start_col
                && cursor_col < r.end_col
        });
        if !cursor_in_match {
            return Vec::new();
        }
        // Only paint into a pane displaying the same path — otherwise
        // an inactive pane showing a different file would pick up the
        // active buffer's highlights, which is just noise.
        if active_path != path {
            return Vec::new();
        }
        let mut out = Vec::new();
        for r in &cache.ranges {
            if r.start_line == line && r.end_line == line {
                if r.end_col > r.start_col {
                    out.push((r.start_col, r.end_col));
                }
            } else if r.start_line <= line && line <= r.end_line {
                // Multi-line ranges: clip to the visible line's column
                // span. Rare for documentHighlight (most servers return
                // single-line ranges) but the spec allows them.
                let buffer_len = self.buffer.line_len(line);
                let start = if r.start_line == line { r.start_col } else { 0 };
                let end = if r.end_line == line {
                    r.end_col
                } else {
                    buffer_len
                };
                if end > start {
                    out.push((start, end));
                }
            }
        }
        out
    }

    pub(super) fn lsp_request_hover(&mut self) {
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "LSP: buffer has no file".into();
            return;
        };
        let line = self.window.cursor.line;
        let col = self.window.cursor.col;
        if !self.lsp.request_hover(&path, line, col) {
            self.status_msg = "LSP: not active for this buffer".into();
        }
    }

    /// Diagnostics for `line` of the active buffer. Convenience for
    /// callers that only ever want the focused buffer's reports;
    /// inactive panes call `line_diagnostics_for` with their own path.
    pub fn line_diagnostics(&self, line: usize) -> Vec<&Diagnostic> {
        let Some(path) = self.buffer.path.as_ref() else {
            return Vec::new();
        };
        self.line_diagnostics_for(path, line)
    }

    /// Diagnostics for `line` of whichever buffer has `path`. Used by
    /// the renderer when drawing an inactive pane — diagnostics are
    /// keyed by path on `LspManager`, so any buffer's reports can be
    /// fetched without needing to make that buffer "live" first.
    pub fn line_diagnostics_for(&self, path: &std::path::Path, line: usize) -> Vec<&Diagnostic> {
        let Some(diags) = self.lsp.diagnostics_for(path) else {
            return Vec::new();
        };
        diags.iter().filter(|d| d.line == line).collect()
    }

    pub fn worst_diagnostic(&self, line: usize) -> Option<Severity> {
        let Some(path) = self.buffer.path.as_ref() else {
            return None;
        };
        self.worst_diagnostic_for(path, line)
    }

    pub fn worst_diagnostic_for(&self, path: &std::path::Path, line: usize) -> Option<Severity> {
        let mut worst: Option<Severity> = None;
        for d in self.line_diagnostics_for(path, line) {
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
        let Some(path) = self.buffer.path.as_deref() else {
            return Vec::new();
        };
        let Some(diags) = self.lsp.diagnostics_for(path) else {
            return Vec::new();
        };
        let line = self.window.cursor.line;
        let col = self.window.cursor.col;
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
        let mut state = PickerState::new(PickerKind::CodeActions, "Code actions".into(), entries);
        state.refilter();
        self.picker = Some(state);
        self.mode = Mode::Picker;
    }

    /// Apply a chosen code action — runs its embedded `WorkspaceEdit` (if
    /// any) then surfaces a status note. Multi-file edits are supported by
    /// switching buffers, applying, saving, and restoring.
    pub(super) fn run_code_action(&mut self, idx: usize) {
        let Some(action) = self.pending_code_actions.get(idx).cloned() else {
            return;
        };
        if let Some(reason) = action.disabled_reason {
            self.status_msg = format!("disabled: {reason}");
            return;
        }
        let mut applied = false;
        if let Some(edit) = action.edit.as_ref() {
            // Preview-flag opt-in: open the same overlay rename uses
            // instead of writing through immediately. Skipping the
            // edit-only command path below — the preview accept handler
            // will report the result and we don't want to also fire the
            // action's command (which would race with the user still
            // looking at the overlay).
            if self.config.lsp.preview_workspace_edits {
                self.open_code_action_preview(action.title.clone(), edit);
                return;
            }
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

    /// Parse a `WorkspaceEdit` JSON payload into typed `ConcreteEdit`s,
    /// in source order grouped by file. Honours both shapes the spec
    /// uses (`documentChanges` and the older `changes` map) and folds
    /// duplicate-file entries together. Returns `(grouped, total)` so
    /// callers that just want the count don't have to re-walk.
    pub(super) fn parse_workspace_edit(
        &self,
        edit: &JsonValue,
    ) -> Vec<crate::app::state::ConcreteEdit> {
        let mut out: Vec<crate::app::state::ConcreteEdit> = Vec::new();
        let mut push = |path: &PathBuf, edits: &[JsonValue]| {
            for e in edits {
                let Some(range) = e.get("range") else { continue };
                let s = range.get("start");
                let n = range.get("end");
                let (Some(s), Some(n)) = (s, n) else { continue };
                let start_line = s.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let start_col = s.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let end_line = n.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let end_col = n.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let new_text = e
                    .get("newText")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push(crate::app::state::ConcreteEdit {
                    path: path.clone(),
                    start_line,
                    start_col,
                    end_line,
                    end_col,
                    new_text,
                });
            }
        };
        if let Some(doc_changes) = edit.get("documentChanges").and_then(|v| v.as_array()) {
            for ch in doc_changes {
                let Some(uri) = ch
                    .get("textDocument")
                    .and_then(|d| d.get("uri"))
                    .and_then(|v| v.as_str())
                else {
                    continue;
                };
                let Some(path) = crate::lsp::uri_to_path(uri) else { continue };
                let Some(edits) = ch.get("edits").and_then(|v| v.as_array()) else { continue };
                push(&path, edits);
            }
        } else if let Some(changes) = edit.get("changes").and_then(|v| v.as_object()) {
            for (uri, v) in changes {
                let Some(path) = crate::lsp::uri_to_path(uri) else { continue };
                let Some(edits) = v.as_array() else { continue };
                push(&path, edits);
            }
        }
        out
    }

    /// Apply a `WorkspaceEdit` JSON value to disk and to any open buffers.
    /// Convenience wrapper around `parse_workspace_edit` +
    /// `apply_concrete_edits` — kept for the server-initiated apply
    /// path (`workspace/applyEdit`) and the code-action flow, where we
    /// don't surface a preview UI. Returns (total edits, distinct files
    /// affected).
    fn apply_workspace_edit(&mut self, edit: &JsonValue) -> Result<(usize, usize)> {
        let parsed = self.parse_workspace_edit(edit);
        self.apply_concrete_edits(&parsed)
    }

    /// Write a pre-parsed batch of `ConcreteEdit`s to disk. Groups by
    /// file, applies each file's edits in reverse position order so
    /// earlier edits don't shift later offsets, and saves so the LSP
    /// picks up the new contents on its next didChange. Returns
    /// (total edits, distinct files affected).
    pub(super) fn apply_concrete_edits(
        &mut self,
        edits: &[crate::app::state::ConcreteEdit],
    ) -> Result<(usize, usize)> {
        if edits.is_empty() {
            return Ok((0, 0));
        }
        // Group by file path, preserving first-seen order so the user-
        // visible status message lists files in source-of-WorkspaceEdit
        // order rather than HashMap-arbitrary order.
        let mut grouped: Vec<(PathBuf, Vec<&crate::app::state::ConcreteEdit>)> = Vec::new();
        for e in edits {
            if let Some(slot) = grouped.iter_mut().find(|(p, _)| *p == e.path) {
                slot.1.push(e);
            } else {
                grouped.push((e.path.clone(), vec![e]));
            }
        }
        let original_active = self.active;
        let mut total_edits = 0usize;
        let files = grouped.len();
        for (path, group) in grouped {
            self.open_buffer(path.clone())?;
            self.history.record(&self.buffer.rope, self.window.cursor);
            let mut concrete: Vec<(usize, usize, String)> = Vec::with_capacity(group.len());
            for e in &group {
                let s_idx = self.buffer.pos_to_char(e.start_line, e.start_col);
                let e_idx = self.buffer.pos_to_char(e.end_line, e.end_col);
                concrete.push((s_idx, e_idx, e.new_text.clone()));
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
        let mut state =
            PickerState::new(PickerKind::DocumentSymbols, "Doc symbols".into(), entries);
        state.refilter();
        self.picker = Some(state);
        self.mode = Mode::Picker;
    }

    /// Replace the current workspace-symbols picker's items with fresh
    /// server-side results. No-op if the user already closed it.
    fn update_workspace_symbols_picker(&mut self, items: Vec<SymbolItem>) {
        let Some(picker) = self.picker.as_mut() else {
            return;
        };
        if !matches!(picker.kind, PickerKind::WorkspaceSymbols) {
            return;
        }
        let entries: Vec<(String, PickerPayload)> = items
            .into_iter()
            .map(|s| {
                let display = if s.container.is_empty() {
                    format!("{} {} :{} {}", s.kind, s.name, s.line + 1, s.path.display())
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

    /// Capture a `window/showMessage` or `window/logMessage` notification
    /// into the ring buffer and — for the loud `showMessage` Error /
    /// Warning case — flash it through the status line. logMessage
    /// notifications are log-only; the user reads them via `:messages`.
    pub(super) fn handle_lsp_server_message(
        &mut self,
        client_key: String,
        severity: crate::lsp::MessageSeverity,
        text: String,
        is_show: bool,
    ) {
        const LSP_MESSAGE_LOG_CAP: usize = 500;
        // Status-line surface: only for showMessage Error / Warning. Info
        // and Log entries are kept silent so a chatty server doesn't
        // hijack the status line; the user can still pull them up via
        // `:messages`.
        if is_show
            && matches!(
                severity,
                crate::lsp::MessageSeverity::Error | crate::lsp::MessageSeverity::Warning
            )
        {
            let tag = match severity {
                crate::lsp::MessageSeverity::Error => "error",
                crate::lsp::MessageSeverity::Warning => "warn",
                _ => "lsp",
            };
            // Servers sometimes ship multi-line messages (stack traces).
            // The status line is single-line, so pick the first non-empty.
            let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or(&text);
            self.status_msg = format!("{client_key} {tag}: {first}");
        }
        self.lsp_messages.push(crate::app::LspServerMessage {
            client_key,
            severity,
            text,
            is_show,
            when: std::time::Instant::now(),
        });
        if self.lsp_messages.len() > LSP_MESSAGE_LOG_CAP {
            let excess = self.lsp_messages.len() - LSP_MESSAGE_LOG_CAP;
            self.lsp_messages.drain(0..excess);
        }
    }

    /// `:messages` — toggle the server-messages overlay. `messages_scroll`
    /// resets so the user always lands on the latest entry on open.
    pub(super) fn cmd_messages(&mut self) {
        if self.lsp_messages.is_empty() {
            self.status_msg = "messages: nothing logged yet".into();
            return;
        }
        self.show_messages_page = true;
        self.messages_scroll = 0;
    }

    pub(super) fn messages_max_scroll(&self) -> usize {
        let total = self.messages_content_height.get();
        let body_rows = self.height.saturating_sub(2) as usize;
        total.saturating_sub(body_rows)
    }

    pub(super) fn messages_scroll_by(&mut self, delta: isize) {
        let max = self.messages_max_scroll();
        let new_scroll = (self.messages_scroll as isize + delta).max(0) as usize;
        self.messages_scroll = new_scroll.min(max);
    }

    /// `:codelens` — diagnostic dump for "why isn't the lens row
    /// showing up?" troubleshooting. Reports whether the active
    /// buffer's primary LSP advertised `codeLensProvider`, whether a
    /// request has fired for this buffer version, whether a request
    /// is currently in flight, and the contents of the cache (count,
    /// anchor lines, resolved-command states).
    /// `:workspaces` — dump every running LSP client + its attached
    /// workspace folders to the status line. Output shape:
    /// `rust-analyzer: ~/code/api  +  ~/code/shared-lib · tsserver: ~/code/web`.
    /// Paths are rendered relative to `$HOME` when possible (the
    /// editor's cwd would be ambiguous in a multi-root session).
    pub(super) fn cmd_workspaces(&mut self) {
        let per_client = self.lsp.workspace_folders_per_client();
        if per_client.is_empty() {
            self.status_msg = "workspaces: no LSP clients running".into();
            return;
        }
        let home = crate::paths::home_dir();
        let pretty = |p: &Path| -> String {
            if let Some(h) = home.as_ref() {
                if let Ok(rest) = p.strip_prefix(h) {
                    return format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display());
                }
            }
            p.display().to_string()
        };
        let mut parts: Vec<String> = Vec::with_capacity(per_client.len());
        for (key, folders) in &per_client {
            let folders_str = folders
                .iter()
                .map(|f| pretty(f))
                .collect::<Vec<_>>()
                .join("  +  ");
            parts.push(format!("{key}: {folders_str}"));
        }
        self.status_msg = parts.join(" · ");
    }

    pub(super) fn cmd_code_lens_status(&mut self) {
        if !self.config.lsp.code_lens {
            self.status_msg = "codelens: disabled in config ([lsp] code_lens = false)".into();
            return;
        }
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "codelens: buffer has no file".into();
            return;
        };
        let cap = self.lsp.code_lens_capability(&path);
        let last_req = self.last_code_lens_request_version.get(&path).copied();
        let in_flight = self.code_lens_in_flight.contains(&path);
        let buf_version = self.buffer.version;
        let cap_str = match cap {
            Some(true) => "capability=yes",
            Some(false) => "capability=NO (server didn't advertise codeLensProvider)",
            None => "capability=? (no LSP client attached)",
        };
        let req_str = match last_req {
            Some(v) if v == buf_version => format!("request=fired@v{v}"),
            Some(v) => format!("request=fired@v{v} (live=v{buf_version})"),
            None => "request=NOT FIRED".to_string(),
        };
        let flight_str = if in_flight { " in-flight" } else { "" };
        let lsp_count = self
            .lsp_only_code_lens
            .get(&path)
            .filter(|(v, _)| *v == buf_version)
            .map(|(_, l)| l.len())
            .unwrap_or(0);
        let synth_count = self
            .synth_only_code_lens
            .get(&path)
            .filter(|(v, _)| *v == buf_version)
            .map(|(_, l)| l.len())
            .unwrap_or(0);
        self.status_msg = format!(
            "codelens: {cap_str} · {req_str}{flight_str} · lsp={lsp_count} synth={synth_count} merged={}",
            self.code_lens
                .get(&path)
                .filter(|c| c.buffer_version == buf_version)
                .map(|c| c.lenses.len())
                .unwrap_or(0),
        );
    }

    /// `<leader>l` — invoke the code lens anchored on the cursor line.
    /// Server-side commands the editor knows how to interpret locally
    /// (today: rust-analyzer's `rust-analyzer.runSingle`) are routed
    /// through the integrated test runner so the lens, `:testnearest`,
    /// and `<leader>sn` all share one engine. Everything else falls
    /// back to `workspace/executeCommand`. Multiple lenses on the same
    /// line → opens a picker; one match → invoke directly.
    pub(super) fn execute_code_lens_under_cursor(&mut self) {
        if !self.config.lsp.code_lens {
            self.status_msg = "code lens: disabled in config (lsp.code_lens = false)".into();
            return;
        }
        let Some(path) = self.buffer.path.clone() else {
            self.status_msg = "code lens: buffer has no file".into();
            return;
        };
        let cursor_line = self.window.cursor.line;
        let commands = self.lens_commands_on_line(&path, cursor_line);
        // When the cursor is parked on a specific phantom segment, fire
        // that segment directly — skip the picker. Out-of-range falls
        // through to the standard branch (server response shrank the list).
        if let Some(idx) = self.phantom_lens_idx {
            if idx < commands.len() {
                let cmd = commands.into_iter().nth(idx).unwrap();
                self.invoke_lens_command(&path, cmd);
                return;
            }
        }
        match commands.len() {
            0 => self.status_msg = "code lens: none on this line".into(),
            1 => {
                let cmd = commands.into_iter().next().unwrap();
                self.invoke_lens_command(&path, cmd);
            }
            _ => self.open_code_lens_picker(commands),
        }
    }

    /// Left-click on a lens row → invoke the lens whose title contains
    /// the clicked column. The render layout is `gutter blanks` +
    /// titles joined by ` │ `; we replay the same widths to figure
    /// out which segment was hit. A click on the separator (or
    /// trailing blanks) is treated as a no-op.
    pub(super) fn click_code_lens_row(&mut self, line: usize, col: usize, gutter: usize) {
        if !self.config.lsp.code_lens {
            return;
        }
        let Some(path) = self.buffer.path.clone() else {
            return;
        };
        let commands = self.lens_commands_on_line(&path, line);
        if commands.is_empty() || col < gutter {
            return;
        }
        let text_col = col - gutter;
        let separator_w = " │ ".chars().count();
        let mut start = 0usize;
        for (i, cmd) in commands.iter().enumerate() {
            let w = cmd.title.chars().count();
            if text_col >= start && text_col < start + w {
                let cmd = commands[i].clone();
                self.invoke_lens_command(&path, cmd);
                return;
            }
            start += w;
            if i + 1 < commands.len() {
                start += separator_w;
            }
        }
    }

    /// Resolved lens commands anchored on `line` for `path`, in
    /// render order. Filters out unresolved lenses (no `command`) and
    /// stale-version caches. Centralised here because both the
    /// keyboard (`<leader>l`) and the mouse-click paths need to ask
    /// the same question and want the same answer shape.
    pub(crate) fn lens_commands_on_line(
        &self,
        path: &std::path::Path,
        line: usize,
    ) -> Vec<crate::lsp::LspCommand> {
        let Some(cache) = self.code_lens.get(path) else {
            return Vec::new();
        };
        if cache.buffer_version != self.buffer.version {
            return Vec::new();
        }
        cache
            .lenses
            .iter()
            .filter(|l| l.line == line)
            .filter_map(|l| l.command.clone())
            .filter(|c| !c.title.is_empty())
            .collect()
    }

    /// Number of resolved lens commands anchored on `line` in the active
    /// buffer. Used by `h`/`l` along the phantom row to clamp index.
    pub(super) fn lens_count_on_line(&self, line: usize) -> usize {
        let Some(path) = self.buffer.path.as_ref() else {
            return 0;
        };
        self.lens_commands_on_line(path, line).len()
    }

    /// Open the multi-lens picker. Each row is a lens title; on
    /// accept we route through `invoke_lens_command` for the picked
    /// index. Same staging-via-index pattern as `pending_code_actions`.
    fn open_code_lens_picker(&mut self, commands: Vec<crate::lsp::LspCommand>) {
        let items: Vec<(String, crate::picker::PickerPayload)> = commands
            .iter()
            .enumerate()
            .map(|(i, c)| {
                (
                    c.title.clone(),
                    crate::picker::PickerPayload::CodeLensIdx(i),
                )
            })
            .collect();
        self.pending_code_lens_commands = commands;
        self.picker = Some(crate::picker::PickerState::new(
            crate::picker::PickerKind::CodeLens,
            "Code lens".into(),
            items,
        ));
        self.mode = crate::mode::Mode::Picker;
    }

    /// Picker accept handler for `PickerPayload::CodeLensIdx`. Looks
    /// up the stashed lens command, clears the staging buffer, and
    /// invokes it.
    pub(super) fn run_picked_code_lens(&mut self, idx: usize) {
        let Some(path) = self.buffer.path.clone() else {
            self.pending_code_lens_commands.clear();
            return;
        };
        let cmd = if idx < self.pending_code_lens_commands.len() {
            Some(self.pending_code_lens_commands.remove(idx))
        } else {
            None
        };
        self.pending_code_lens_commands.clear();
        if let Some(cmd) = cmd {
            self.invoke_lens_command(&path, cmd);
        }
    }

    /// Run a resolved lens `Command`. Shared by `<leader>l`, click,
    /// and the multi-lens picker accept.
    pub(super) fn invoke_lens_command(
        &mut self,
        path: &std::path::Path,
        cmd: crate::lsp::LspCommand,
    ) {
        // Client-side commands the editor knows how to interpret
        // without round-tripping to the LSP. Synthetic lenses
        // (`binvim.runTestByName`) and rust-analyzer runnables both
        // resolve to "run a test by name through the integrated
        // runner"; the only difference is the argument shape.
        if cmd.command == crate::code_lens_synth::SYNTHETIC_RUN_COMMAND {
            if let Some(name) = extract_synthetic_run_filter(&cmd.arguments) {
                self.run_test_filter_through_kickoff(name);
                return;
            }
        }
        if cmd.command == "rust-analyzer.runSingle" || cmd.command == "rust-analyzer.debugSingle" {
            if let Some(name) = extract_rust_analyzer_runnable_filter(&cmd.arguments) {
                self.run_test_filter_through_kickoff(name);
                return;
            }
        }
        // VS Code's built-in "show references" command (used by gopls,
        // some servers) and rust-analyzer's variant both ship the
        // resolved locations inline as the third argument. Open the
        // same picker `gr` uses instead of forwarding the command to
        // the LSP — the server has no way to *display* anything, and
        // `executeCommand` would no-op.
        if cmd.command == "editor.action.showReferences"
            || cmd.command == "rust-analyzer.showReferences"
            || cmd.command == "csharp.showReferences"
        {
            let items = extract_show_references_locations(&cmd.arguments);
            if !items.is_empty() {
                // Route through the merge step so the Razor grep
                // augment also fires on code-lens clicks (the lens
                // carries the locations inline, so without this it
                // would bypass `present_references` entirely). The
                // needle is derived from the lens's encoded position,
                // not the cursor — the user might have clicked the
                // lens with the mouse from anywhere on screen.
                if let Some(path) = self.buffer.path.clone() {
                    let needle = extract_text_document_position(&cmd.arguments)
                        .and_then(|(line, col)| self.identifier_at(line, col));
                    self.pending_ref_augment = razor_ref_augment(&path, needle);
                }
                self.present_references(items);
                return;
            }
            self.status_msg = "code lens: no references attached".into();
            return;
        }
        // csharp-ls (and other Roslyn-based servers) uses the LSP method
        // name `textDocument/references` as the lens command — the
        // contract is "fire this method at the lens anchor, then open
        // your picker." Extract the position from `arguments`
        // (standard `ReferenceParams` shape) and fire a real
        // references request; the reply comes back through
        // `LspEvent::References` which already opens the picker.
        if cmd.command == "textDocument/references" {
            let (line, col) = extract_text_document_position(&cmd.arguments)
                .unwrap_or((self.window.cursor.line, self.window.cursor.col));
            // Stash the Razor grep augment using the lens's encoded
            // position — the LSP reply arrives via `LspEvent::References`
            // and the merge step picks it up there.
            let needle = self.identifier_at(line, col);
            self.pending_ref_augment = razor_ref_augment(path, needle);
            if !self.lsp.request_references(path, line, col) {
                self.status_msg = "code lens: no LSP client for this buffer".into();
                self.pending_ref_augment = None;
            }
            return;
        }
        let command_obj = serde_json::json!({
            "title": cmd.title,
            "command": cmd.command,
            "arguments": cmd.arguments,
        });
        if self.lsp.execute_command(path, &command_obj) {
            self.status_msg = format!("code lens: {}", cmd.title);
        } else {
            self.status_msg = "code lens: no LSP client for this buffer".into();
        }
    }

    /// Shared "run this test by name" entry point — used by both the
    /// rust-analyzer runnable interception and the synthetic vitest
    /// / pytest lenses. Resolves the workspace adapter and routes
    /// the filter through `test_kickoff` so the picker + overlay
    /// behaviour matches `:testnearest`.
    fn run_test_filter_through_kickoff(&mut self, name: String) {
        let Some((spec, root)) = self.test_resolve_adapter() else {
            self.status_msg = "code lens: no test adapter for this workspace".into();
            return;
        };
        let label = format!("lens: {name}");
        let req = crate::test::TestRunRequest {
            filter: Some(name),
            workspace_root: root,
            label,
        };
        self.test_kickoff(&spec, req);
    }
}

/// Extract the libtest substring filter from a rust-analyzer
/// `Runnable` argument. The shape varies a little between releases
/// but the test name has consistently lived at
/// `args.executableArgs[0]` (e.g. `"motion::tests::word_forward_basic"`)
/// — the rest of the args are `--exact` / `--nocapture` flags we don't
/// need to thread through to the in-tree test runner.
/// Synthetic lens shape — emitted by `code_lens_synth`. The args
/// list contains a single object `{ "name": "...", "kind":
/// "it"|"test"|"describe" }`; we feed the name to the test runner as
/// a substring filter.
fn extract_synthetic_run_filter(arguments: &[serde_json::Value]) -> Option<String> {
    let obj = arguments.first()?;
    let name = obj.get("name")?.as_str()?.to_string();
    if name.is_empty() {
        return None;
    }
    Some(name)
}

/// Decode the third argument of an `editor.action.showReferences`-style
/// lens command into our internal `LocationItem` list. The shape is
/// the standard VS Code one: `arguments = [uri, position, locations]`
/// where `locations` is an array of `Location` objects (`{ uri, range
/// { start: { line, character } } }`). Returns an empty Vec on any
/// parse failure so the caller can fall back gracefully.
fn extract_show_references_locations(
    arguments: &[serde_json::Value],
) -> Vec<crate::lsp::LocationItem> {
    let Some(arr) = arguments.get(2).and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        let uri = entry
            .get("uri")
            .and_then(|v| v.as_str())
            .or_else(|| entry.get("targetUri").and_then(|v| v.as_str()));
        let range = entry
            .get("range")
            .or_else(|| entry.get("targetSelectionRange"))
            .or_else(|| entry.get("targetRange"));
        let (Some(uri), Some(range)) = (uri, range) else { continue };
        let Some(path) = crate::lsp::uri_to_path(uri) else { continue };
        let Some(start) = range.get("start") else { continue };
        let Some(line) = start.get("line").and_then(|v| v.as_u64()) else { continue };
        let Some(col) = start.get("character").and_then(|v| v.as_u64()) else { continue };
        out.push(crate::lsp::LocationItem {
            path,
            line: line as usize,
            col: col as usize,
        });
    }
    out
}

/// Pull a `(line, col)` out of an LSP `ReferenceParams`-shaped
/// arguments list. Accepts either `[{ textDocument, position }]`
/// (object-style) or `[uri, position]` (array-style); both forms
/// turn up across servers that bind a method-name as a lens
/// command. Returns `None` when no position is recoverable so the
/// caller can fall back to the cursor.
fn extract_text_document_position(arguments: &[serde_json::Value]) -> Option<(usize, usize)> {
    let position = arguments
        .iter()
        .find_map(|v| v.get("position"))
        .or_else(|| arguments.get(1))?;
    let line = position.get("line").and_then(|v| v.as_u64())? as usize;
    let col = position.get("character").and_then(|v| v.as_u64())? as usize;
    Some((line, col))
}

fn extract_rust_analyzer_runnable_filter(arguments: &[serde_json::Value]) -> Option<String> {
    let runnable = arguments.first()?;
    let exec_args = runnable
        .get("args")
        .and_then(|v| v.get("executableArgs"))
        .and_then(|v| v.as_array())?;
    let first = exec_args.iter().find_map(|v| v.as_str())?.to_string();
    if first.is_empty() {
        return None;
    }
    Some(first)
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
                    idx = idx
                        .saturating_mul(10)
                        .saturating_add(chars[j] as u32 - '0' as u32);
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
                    idx = idx
                        .saturating_mul(10)
                        .saturating_add(chars[j] as u32 - '0' as u32);
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

/// Prepend `indent` after every newline in `text` and shift `stops` to
/// match. Stops at or before a given newline don't move; stops strictly
/// after shift forward by `indent.chars().count()` for that newline.
///
/// LSP servers emit snippet bodies as if they're being pasted at column 0
/// (`<ul>\n\t<li>…\n</ul>`). The buffer is usually nested deeper than
/// that, so continuation lines need the caller's indent applied for the
/// closing tag (and inner siblings) to line up.
pub(super) fn indent_continuation_lines(text: &str, stops: &mut [usize], indent: &str) -> String {
    let indent_chars = indent.chars().count();
    if indent_chars == 0 || !text.contains('\n') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + indent.len() * 4);
    let mut i = 0usize;
    for c in text.chars() {
        out.push(c);
        if c == '\n' {
            for stop in stops.iter_mut() {
                if *stop > i {
                    *stop += indent_chars;
                }
            }
            out.push_str(indent);
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{expand_snippet, razor_ref_augment};
    use std::path::Path;

    #[test]
    fn ref_augment_cs_buffer_targets_razor_extensions() {
        let aug = razor_ref_augment(Path::new("/tmp/Foo.cs"), Some("Bar".into())).unwrap();
        assert_eq!(aug.needle, "Bar");
        assert_eq!(aug.extensions, vec!["cshtml", "razor"]);
    }

    #[test]
    fn ref_augment_razor_buffer_includes_cs() {
        let aug = razor_ref_augment(Path::new("/tmp/Index.cshtml"), Some("Model".into())).unwrap();
        assert_eq!(aug.extensions, vec!["cs", "cshtml", "razor"]);
        let aug = razor_ref_augment(Path::new("/tmp/Page.razor"), Some("Model".into())).unwrap();
        assert_eq!(aug.extensions, vec!["cs", "cshtml", "razor"]);
    }

    #[test]
    fn ref_augment_skips_unrelated_buffers() {
        assert!(razor_ref_augment(Path::new("/tmp/main.rs"), Some("foo".into())).is_none());
        assert!(razor_ref_augment(Path::new("/tmp/script.ts"), Some("foo".into())).is_none());
    }

    #[test]
    fn ref_augment_skips_non_identifier_needles() {
        assert!(razor_ref_augment(Path::new("/tmp/Foo.cs"), Some("=>".into())).is_none());
        assert!(razor_ref_augment(Path::new("/tmp/Foo.cs"), Some("@".into())).is_none());
        assert!(razor_ref_augment(Path::new("/tmp/Foo.cs"), Some(String::new())).is_none());
        assert!(razor_ref_augment(Path::new("/tmp/Foo.cs"), None).is_none());
    }

    #[test]
    fn ref_augment_allows_underscore_identifiers() {
        let aug =
            razor_ref_augment(Path::new("/tmp/Foo.cs"), Some("_private_field".into())).unwrap();
        assert_eq!(aug.needle, "_private_field");
    }

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

    #[test]
    fn indent_lines_prepends_after_each_newline_and_shifts_stops() {
        // Emmet-shape snippet: parent at the cursor's indent, children
        // already indented one extra level, closing tag at column 0.
        // The continuation indent should make the closer line up with
        // the opener.
        let mut stops = vec![13]; // position of the `$1` inside <li>
        let out = super::indent_continuation_lines("<ul>\n\t<li>x</li>\n</ul>", &mut stops, "\t");
        assert_eq!(out, "<ul>\n\t\t<li>x</li>\n\t</ul>");
        // The stop at original char 13 (the 'x') is after the first
        // newline, so it shifts by one (the inserted tab).
        assert_eq!(stops, vec![14]);
    }

    #[test]
    fn indent_lines_noop_without_newline() {
        let mut stops = vec![0];
        let out = super::indent_continuation_lines("foo", &mut stops, "\t");
        assert_eq!(out, "foo");
        assert_eq!(stops, vec![0]);
    }

    #[test]
    fn indent_lines_noop_with_empty_indent() {
        let mut stops = vec![5];
        let out = super::indent_continuation_lines("a\nb\nc", &mut stops, "");
        assert_eq!(out, "a\nb\nc");
        assert_eq!(stops, vec![5]);
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
    tiered.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.sort_text.cmp(&b.1.sort_text))
    });
    tiered.truncate(VISIBLE_CAP);
    tiered.into_iter().map(|(_, item)| item).collect()
}

/// Build the Razor / C# find-references augment for a buffer. The C#
/// language servers we ship (csharp-ls, OmniSharp) don't index
/// `.cshtml` / `.razor` files, so a `textDocument/references` request
/// for a C# symbol misses every Razor use — and references *from* a
/// Razor buffer don't work at all because the Razor LSP itself is
/// unwired. We work around both by grepping the relevant file
/// extensions for the symbol name and merging the matches into the
/// LSP reply. Returns `None` for buffers / cursor positions where
/// the augment doesn't apply.
fn razor_ref_augment(path: &Path, needle: Option<String>) -> Option<PendingRefAugment> {
    let needle = needle?;
    if needle.is_empty() {
        return None;
    }
    // Identifier-only — punctuation runs (`@`, `=>`, `=`) are useless
    // as a needle and would produce thousands of bogus matches.
    if !needle.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())?
        .to_ascii_lowercase();
    // From a .cs buffer the LSP already covers .cs, so grep is only
    // useful for the extensions it can't see. From a Razor buffer the
    // LSP is unusable, so grep is the whole story — include .cs too.
    let extensions: Vec<&'static str> = match ext.as_str() {
        "cs" => vec!["cshtml", "razor"],
        "cshtml" | "razor" => vec!["cs", "cshtml", "razor"],
        _ => return None,
    };
    // Use the *outermost* marker (`.sln` / `.git`) — not the closest
    // `.csproj`. Multi-project .NET solutions keep the C# in
    // `Foo.Core/` and the Razor views in `Foo.Web/`, both siblings
    // under the same `.sln`. Anchoring on `Foo.Core.csproj` would
    // miss every `.cshtml` in `Foo.Web/`.
    let start = path.parent().unwrap_or_else(|| Path::new("."));
    let root = find_outermost_root(start);
    Some(PendingRefAugment {
        needle,
        extensions,
        root,
    })
}

/// Walk up from `start` and return the topmost ancestor that contains a
/// `.sln` or `.git` marker. Falls back to the closest `.csproj` /
/// `.fsproj` ancestor when there's no solution / git boundary above
/// it, and finally to `start` itself if nothing matched. Differs from
/// `find_workspace_root` (which returns the *closest* marker) — the
/// Razor augment specifically wants the wider scope so it can reach
/// sibling projects under the same solution.
fn find_outermost_root(start: &Path) -> PathBuf {
    let canon = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut outermost: Option<PathBuf> = None;
    let mut closest_project: Option<PathBuf> = None;
    let mut dir: &Path = canon.as_path();
    loop {
        let has_sln = dir_has_extension(dir, "sln");
        let has_git = dir.join(".git").exists();
        if has_sln || has_git {
            outermost = Some(dir.to_path_buf());
        }
        if closest_project.is_none()
            && (dir_has_extension(dir, "csproj") || dir_has_extension(dir, "fsproj"))
        {
            closest_project = Some(dir.to_path_buf());
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p,
            _ => break,
        }
    }
    outermost
        .or(closest_project)
        .unwrap_or_else(|| canon.clone())
}

fn dir_has_extension(dir: &Path, ext: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        if let Some(e) = entry.path().extension().and_then(|s| s.to_str()) {
            if e.eq_ignore_ascii_case(ext) {
                return true;
            }
        }
    }
    false
}

/// Run `rg` for the augment's needle across the listed extensions and
/// return one `LocationItem` per match. Whole-word, fixed-string match
/// against `<root>` so PascalCase C# identifiers don't get matched as
/// substrings of unrelated names. ripgrep emits `path:line:col:content`
/// — we drop the content column. Silently returns an empty list when
/// `rg` is missing from PATH or the pattern has no matches; the merge
/// step handles the "empty + empty" case via the picker's own empty-
/// list status message.
fn run_razor_ref_grep(aug: &PendingRefAugment) -> Vec<LocationItem> {
    let mut rg = std::process::Command::new("rg");
    rg.arg("--no-heading")
        .arg("--color=never")
        .arg("--line-number")
        .arg("--column")
        .arg("--word-regexp")
        .arg("--fixed-strings");
    for ext in &aug.extensions {
        rg.arg("-g").arg(format!("*.{ext}"));
    }
    rg.arg("--").arg(&aug.needle).arg(&aug.root);
    let Ok(out) = rg.output() else {
        return Vec::new();
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut items = Vec::new();
    for line in stdout.lines() {
        // `path:line:col:content` — content may itself contain ':',
        // so cap the split at 4 and discard the trailing field.
        let mut parts = line.splitn(4, ':');
        let path = parts.next();
        let lnum = parts.next();
        let cnum = parts.next();
        let (Some(p), Some(l), Some(c)) = (path, lnum, cnum) else {
            continue;
        };
        let Ok(l): Result<usize, _> = l.parse() else { continue };
        let Ok(c): Result<usize, _> = c.parse() else { continue };
        items.push(LocationItem {
            path: PathBuf::from(p),
            line: l.saturating_sub(1),
            col: c.saturating_sub(1),
        });
    }
    items
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
