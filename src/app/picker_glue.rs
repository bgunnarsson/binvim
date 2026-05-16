//! Generic picker open/handle/refilter and the yazi shell-out. The
//! LSP-specific pickers (code actions, symbols, references) live in
//! `lsp_glue.rs` because they need the LSP request machinery.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::io;
use std::path::PathBuf;

use crate::mode::Mode;
use crate::parser::PickerLeader;
use crate::picker::{self, PickerKind, PickerPayload, PickerState};

impl super::App {
    pub(super) fn open_picker(&mut self, kind: PickerLeader) {
        let state = match kind {
            PickerLeader::Files => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let items = picker::enumerate_files(&cwd, 5000);
                if items.is_empty() {
                    self.status_msg = "No files found".into();
                    return;
                }
                PickerState::new(PickerKind::Files, "Files".into(), items)
            }
            PickerLeader::Recents => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let mut items: Vec<(String, PickerPayload)> = Vec::new();
                for r in &self.recents {
                    if !r.is_file() {
                        continue;
                    }
                    let display = r
                        .strip_prefix(&cwd)
                        .ok()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| r.display().to_string());
                    items.push((display, PickerPayload::Path(r.clone())));
                }
                if items.is_empty() {
                    self.status_msg = "No recent files".into();
                    return;
                }
                PickerState::new(PickerKind::Recents, "Recents".into(), items)
            }
            PickerLeader::Grep => {
                PickerState::new(PickerKind::Grep, "Grep".into(), Vec::new())
            }
            PickerLeader::Buffers => {
                let mut items: Vec<(String, PickerPayload)> = Vec::new();
                for (i, stash) in self.buffers.iter().enumerate() {
                    let name = if i == self.active {
                        self.buffer
                            .path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "[No Name]".into())
                    } else {
                        stash
                            .buffer
                            .path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "[No Name]".into())
                    };
                    items.push((name, PickerPayload::BufferIdx(i)));
                }
                PickerState::new(PickerKind::Buffers, "Buffers".into(), items)
            }
            PickerLeader::DocumentSymbols => {
                if let Some(path) = self.buffer.path.clone() {
                    self.lsp_sync_active();
                    if !self.lsp.request_document_symbols(&path) {
                        self.status_msg = "LSP: not active for this buffer".into();
                    }
                } else {
                    self.status_msg = "Save the buffer to query symbols".into();
                }
                return;
            }
            PickerLeader::CodeActions => {
                if let Some(path) = self.buffer.path.clone() {
                    self.lsp_sync_active();
                    let line = self.window.cursor.line;
                    let col = self.window.cursor.col;
                    let diags = self.diagnostics_at_cursor_for_lsp();
                    if !self.lsp.request_code_actions(&path, line, col, diags) {
                        self.status_msg = "LSP: not active for this buffer".into();
                    }
                } else {
                    self.status_msg = "Save the buffer to query code actions".into();
                }
                return;
            }
            PickerLeader::WorkspaceSymbols => {
                // Open an empty picker immediately so the user can start
                // typing; queries fire as they go via `refilter_picker`.
                let state = PickerState::new(
                    PickerKind::WorkspaceSymbols,
                    "Workspace symbols".into(),
                    Vec::new(),
                );
                self.picker = Some(state);
                self.mode = Mode::Picker;
                if let Some(path) = self.buffer.path.clone() {
                    self.lsp_sync_active();
                    let _ = self.lsp.request_workspace_symbols(&path, "");
                }
                return;
            }
        };
        self.picker = Some(state);
        self.mode = Mode::Picker;
    }

    pub(super) fn handle_picker_key(&mut self, key: KeyEvent) {
        let Some(picker) = self.picker.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.picker = None;
                self.mode = Mode::Normal;
                // Cancel cleans up any pending debug-launch state so a
                // half-finished flow doesn't leak into the next picker.
                self.pending_debug_project = None;
                self.pending_debug_profiles.clear();
            }
            KeyCode::Enter => {
                let payload = picker.current().cloned();
                // Grep / references → snapshot the filtered result set
                // into the quickfix list before tearing the picker down,
                // so `:cnext` / `]q` can step through the remaining
                // matches without re-running the search.
                let qf_snapshot = if matches!(payload, Some(PickerPayload::Location { .. })) {
                    let picked = picker.selected;
                    let entries = crate::app::quickfix::entries_from_picker(picker);
                    if entries.is_empty() {
                        None
                    } else {
                        let current = picked.min(entries.len().saturating_sub(1));
                        Some(crate::app::state::QuickfixState { entries, current })
                    }
                } else {
                    None
                };
                self.picker = None;
                self.mode = Mode::Normal;
                if let Some(state) = qf_snapshot {
                    self.quickfix = Some(state);
                }
                if let Some(p) = payload {
                    match p {
                        PickerPayload::Path(path) => {
                            if let Err(e) = self.open_buffer(path) {
                                self.status_msg = format!("error: {e}");
                            }
                        }
                        PickerPayload::BufferIdx(idx) => {
                            if let Err(e) = self.switch_to(idx) {
                                self.status_msg = format!("error: {e}");
                            }
                        }
                        PickerPayload::Location { path, line, col } => {
                            if let Err(e) = self.open_buffer(path) {
                                self.status_msg = format!("error: {e}");
                            } else {
                                self.push_jump();
                                self.window.cursor.line = line.saturating_sub(1);
                                self.window.cursor.col = col.saturating_sub(1);
                                self.window.cursor.want_col = self.window.cursor.col;
                                self.clamp_cursor_normal();
                            }
                        }
                        PickerPayload::CodeActionIdx(idx) => {
                            self.run_code_action(idx);
                        }
                        PickerPayload::DebugProject(project) => {
                            self.dap_start_session_with_project(project);
                        }
                        PickerPayload::DebugProfile(idx) => {
                            let project = self.pending_debug_project.take();
                            let profile = if idx < self.pending_debug_profiles.len() {
                                Some(self.pending_debug_profiles.remove(idx))
                            } else {
                                None
                            };
                            self.pending_debug_profiles.clear();
                            if let Some(project) = project {
                                self.dap_start_session_with_profile(project, profile);
                            } else {
                                self.status_msg = "debug: profile pick lost context".into();
                            }
                        }
                        PickerPayload::DebugTarget {
                            adapter_key,
                            path,
                            name,
                        } => {
                            self.dap_start_target(&adapter_key, path, name);
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                picker.input.pop();
                self.refilter_picker();
            }
            KeyCode::Up => picker.move_up(),
            KeyCode::Down => picker.move_down(),
            KeyCode::PageUp => {
                let page = crate::render::picker_visible_rows(self).max(1) as i64;
                if let Some(p) = self.picker.as_mut() { p.move_by(-page); }
            }
            KeyCode::PageDown => {
                let page = crate::render::picker_visible_rows(self).max(1) as i64;
                if let Some(p) = self.picker.as_mut() { p.move_by(page); }
            }
            KeyCode::Home => picker.move_by(i64::MIN / 2),
            KeyCode::End => picker.move_by(i64::MAX / 2),
            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => match c {
                'j' => picker.move_down(),
                'k' => picker.move_up(),
                'd' | 'D' => {
                    let half = (crate::render::picker_visible_rows(self) / 2).max(1) as i64;
                    if let Some(p) = self.picker.as_mut() { p.move_by(half); }
                }
                'u' | 'U' => {
                    let half = (crate::render::picker_visible_rows(self) / 2).max(1) as i64;
                    if let Some(p) = self.picker.as_mut() { p.move_by(-half); }
                }
                'g' => picker.move_by(i64::MIN / 2),
                'G' => picker.move_by(i64::MAX / 2),
                _ => {}
            },
            KeyCode::Char(c) => {
                picker.input.push(c);
                self.refilter_picker();
            }
            _ => {}
        }
    }

    fn refilter_picker(&mut self) {
        let Some(picker) = self.picker.as_mut() else { return; };
        match picker.kind {
            PickerKind::Files
            | PickerKind::Recents
            | PickerKind::Buffers
            | PickerKind::References
            | PickerKind::DocumentSymbols
            | PickerKind::CodeActions
            | PickerKind::DebugProject
            | PickerKind::DebugProfile
            | PickerKind::DebugTarget => picker.refilter(),
            PickerKind::Grep => {
                if picker.input.len() < 2 {
                    picker::replace_items(picker, Vec::new());
                    return;
                }
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let query = picker.input.clone();
                let results = picker::run_ripgrep(&query, &cwd, 500);
                picker::replace_items(picker, results);
            }
            PickerKind::WorkspaceSymbols => {
                let query = picker.input.clone();
                if let Some(path) = self.buffer.path.clone() {
                    let _ = self.lsp.request_workspace_symbols(&path, &query);
                }
            }
        }
    }

    pub(super) fn open_yazi(&mut self) {
        use crossterm::{
            cursor::{Hide, Show},
            event::{
                DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags,
                PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
            },
            execute,
            terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
        };
        use std::process::Command;

        let start_dir = self
            .buffer
            .path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let chooser = std::env::temp_dir()
            .join(format!("binvim-yazi-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&chooser);

        let mut stdout = io::stdout();
        // Hand the terminal over to yazi: pop the kitty keyboard protocol
        // (yazi needs vanilla keys), disable our mouse capture and raw
        // mode, leave the alternate screen so yazi has a clean canvas.
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        let _ = execute!(stdout, DisableMouseCapture, LeaveAlternateScreen, Show);
        let _ = disable_raw_mode();

        let status = Command::new("yazi")
            .arg("--chooser-file")
            .arg(&chooser)
            .arg(&start_dir)
            .status();

        // Reclaim the terminal — must re-enable mouse capture explicitly,
        // otherwise clicks stop working in the editor after yazi exits.
        let _ = enable_raw_mode();
        let _ = execute!(stdout, EnterAlternateScreen, EnableMouseCapture, Hide);
        let _ = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES),
        );

        match status {
            Err(_) => {
                self.status_msg = "yazi not on PATH".into();
            }
            Ok(_) => {
                if let Ok(text) = std::fs::read_to_string(&chooser) {
                    for line in text.lines() {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let path = PathBuf::from(trimmed);
                        if let Err(e) = self.open_buffer(path) {
                            self.status_msg = format!("error: {e}");
                        }
                        break;
                    }
                }
            }
        }
        let _ = std::fs::remove_file(&chooser);
    }
}
