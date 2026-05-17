//! Registers, macro recording / replay, and the `.` repeat machinery.
//! Also owns the OS clipboard mirror for the unnamed/`+`/`*` registers.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::mode::{Mode, Operator};
use crate::parser::{Action, ParseCtx};

use super::state::{LastEdit, RecordingState, Register};

impl super::App {
    pub(super) fn write_register(&mut self, target: Option<char>, text: String, linewise: bool) {
        if matches!(target, Some('_')) {
            return;
        }
        // Mirror writes to the unnamed register into the OS clipboard so
        // y/d/c land in other apps. Explicit named registers (`"ay`) stay
        // local — that's what users reach for when they want a side stash.
        if mirrors_to_system_clipboard(target) {
            set_system_clipboard(&text);
        }
        let r = Register { text, linewise };
        self.registers.insert('"', r.clone());
        if let Some(name) = target {
            if name != '"' {
                self.registers.insert(name, r);
            }
        }
    }

    pub(super) fn write_yank_register(&mut self, target: Option<char>, text: String, linewise: bool) {
        if matches!(target, Some('_')) {
            return;
        }
        if mirrors_to_system_clipboard(target) {
            set_system_clipboard(&text);
        }
        let r = Register { text, linewise };
        self.registers.insert('"', r.clone());
        self.registers.insert('0', r.clone());
        if let Some(name) = target {
            if name != '"' && name != '0' {
                self.registers.insert(name, r);
            }
        }
    }

    pub(super) fn read_register(&self, name: Option<char>) -> Option<Register> {
        let key = name.unwrap_or('"');
        if key == '_' {
            return None;
        }
        // For the registers that mirror the OS clipboard, check the
        // clipboard first — anything the user just copied in another
        // app should win over our in-memory register, which would
        // otherwise hold a stale in-editor yank from earlier.
        if matches!(key, '"' | '+' | '*') {
            if let Some(text) = get_system_clipboard() {
                if !text.is_empty() {
                    // If the clipboard matches our last in-app yank,
                    // the clipboard came from binvim — trust the
                    // linewise flag we recorded then. Without this,
                    // `yy` of a single line round-trips through the
                    // clipboard and the heuristic below demotes it to
                    // charwise (no interior newline) — `P` then pastes
                    // inline instead of opening a new line.
                    if let Some(reg) = self.registers.get(&'"') {
                        if reg.text == text {
                            return Some(reg.clone());
                        }
                    }
                    // Otherwise the clipboard was filled by another
                    // app. Linewise heuristic: trailing `\n` AND an
                    // interior newline. Single-line payloads (e.g.
                    // terminal echo) stay charwise so paste-at-cursor
                    // doesn't open a surprise extra line.
                    let trimmed_ends_nl = text.ends_with('\n');
                    let has_interior_nl = text[..text.len().saturating_sub(1)].contains('\n');
                    let linewise = trimmed_ends_nl && has_interior_nl;
                    return Some(Register { text, linewise });
                }
            }
        }
        self.registers.get(&key).cloned()
    }

    pub(super) fn start_macro_recording(&mut self, name: char) {
        if self.recording_macro.is_some() {
            return;
        }
        self.recording_macro = Some(name);
        self.macro_buffer.clear();
        self.status_msg = format!("recording @{}", name);
    }

    pub(super) fn replay_macro(&mut self, name: char) {
        let target = if name == '@' {
            self.last_replayed_macro
        } else {
            Some(name)
        };
        let Some(name) = target else {
            self.status_msg = "No previous macro".into();
            return;
        };
        let Some(keys) = self.macros.get(&name).cloned() else {
            self.status_msg = format!("Empty register: {}", name);
            return;
        };
        self.last_replayed_macro = Some(name);
        self.replaying_macro = true;
        for k in keys {
            match self.mode {
                Mode::Normal => self.handle_keyboard(k, ParseCtx::Normal),
                Mode::Insert => self.handle_insert_key(k),
                Mode::Command => self.handle_command_key(k),
                Mode::Visual(_) => self.handle_keyboard(k, ParseCtx::Visual),
                Mode::Search { .. } => self.handle_search_key(k),
                Mode::Picker => self.handle_picker_key(k),
                Mode::Prompt(_) => self.handle_prompt_key(k),
                // Macros don't navigate the debug pane — replay aborts if
                // the user happened to start recording while focused there.
                Mode::DebugPane => break,
                // Same for the terminal pane — macro replay doesn't
                // forward keys into a PTY, so abort cleanly if focus
                // happens to land there mid-replay.
                Mode::Terminal => break,
            }
        }
        self.replaying_macro = false;
    }

    /// Decide whether an about-to-fire action should set up a recording for `.` repeat.
    pub(super) fn maybe_record_edit(&mut self, action: &Action) {
        if self.replaying {
            return;
        }
        // Actions that enter insert mode begin a recording session that ends on Esc.
        let enters_insert = matches!(
            action,
            Action::EnterInsert(_)
                | Action::Operate { op: Operator::Change, .. }
                | Action::OperateLine { op: Operator::Change, .. }
                | Action::OperateTextObject { op: Operator::Change, .. }
        );
        if enters_insert {
            self.recording = Some(RecordingState { prelude: action.clone(), keys: Vec::new() });
            return;
        }
        let plain_recordable = match action {
            Action::Operate { op, .. }
            | Action::OperateLine { op, .. }
            | Action::OperateTextObject { op, .. } => matches!(op, Operator::Delete),
            Action::DeleteCharForward { .. }
            | Action::Put { .. }
            | Action::VisualPut { .. }
            | Action::ReplaceChar { .. }
            | Action::JoinLines { .. }
            | Action::AdjustNumber { .. }
            | Action::ToggleCase { .. } => true,
            _ => false,
        };
        if plain_recordable {
            self.last_edit = Some(LastEdit::Plain(action.clone()));
        }
    }

    pub(super) fn repeat_last_edit(&mut self) {
        let Some(last) = self.last_edit.clone() else {
            self.status_msg = "No previous edit to repeat".into();
            return;
        };
        self.replaying = true;
        match last {
            LastEdit::Plain(action) => self.apply_action(action),
            LastEdit::InsertSession { prelude, keys } => {
                self.apply_action(prelude);
                for k in keys {
                    self.handle_insert_key(k);
                }
                let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
                self.handle_insert_key(esc);
            }
        }
        self.replaying = false;
    }
}

/// True when a register write should also sync into the OS clipboard. Maps
/// to: the unnamed register (no explicit target), the explicit unnamed
/// (`""`), and the X11-flavour `+`/`*` clipboard registers.
pub fn mirrors_to_system_clipboard(target: Option<char>) -> bool {
    match target {
        None => true,
        Some(c) => matches!(c, '"' | '+' | '*'),
    }
}

/// Best-effort write of `text` to the OS clipboard. A failure (no display
/// server, no clipboard access on the platform) is swallowed — the editor
/// still has the text in its in-memory unnamed register.
pub fn set_system_clipboard(text: &str) {
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(text.to_string());
    }
}

/// Best-effort read of the OS clipboard as UTF-8. Returns `None` when the
/// clipboard is empty, the platform refuses access, or the contents aren't
/// text (an image, a file list, etc.). Swallows every failure so a missing
/// display server / locked clipboard / image payload just makes `p` fall
/// back to the in-memory register instead of erroring out.
pub fn get_system_clipboard() -> Option<String> {
    let mut cb = arboard::Clipboard::new().ok()?;
    cb.get_text().ok()
}
