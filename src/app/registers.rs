//! Registers, macro recording / replay, and the `.` repeat machinery.
//! Also owns the OS clipboard mirror for the unnamed/`+`/`*` registers.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::mode::{Mode, Operator};
use crate::parser::{Action, ParseCtx};

use super::state::{LastEdit, MACRO_REPLAY_DEPTH_LIMIT, RecordingState, Register};

impl super::App {
    pub(super) fn write_register(&mut self, target: Option<char>, text: String, linewise: bool) {
        if matches!(target, Some('_')) {
            return;
        }
        // Mirror writes to the unnamed register into the OS clipboard so
        // y/d/c land in other apps. Explicit named registers (`"ay`) stay
        // local — that's what users reach for when they want a side stash.
        if mirrors_to_system_clipboard(target) {
            set_system_clipboard(&text, self.config.clipboard.osc52);
        }
        let r = Register { text, linewise };
        self.registers.insert('"', r.clone());
        if let Some(name) = target {
            if name != '"' {
                self.registers.insert(name, r);
            }
        }
    }

    pub(super) fn write_yank_register(
        &mut self,
        target: Option<char>,
        text: String,
        linewise: bool,
    ) {
        if matches!(target, Some('_')) {
            return;
        }
        if mirrors_to_system_clipboard(target) {
            set_system_clipboard(&text, self.config.clipboard.osc52);
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

    pub(super) fn replay_macro(&mut self, name: char, count: usize) {
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
        let count = count.max(1);
        self.replaying_macro = true;
        self.macro_replay_depth = self.macro_replay_depth.saturating_add(1);
        if self.macro_replay_depth > MACRO_REPLAY_DEPTH_LIMIT {
            self.macro_replay_depth = self.macro_replay_depth.saturating_sub(1);
            self.replaying_macro = false;
            self.status_msg = format!(
                "macro recursion limit ({}) reached",
                MACRO_REPLAY_DEPTH_LIMIT
            );
            return;
        }
        'outer: for _ in 0..count {
            for k in keys.iter().copied() {
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
                    Mode::DebugPane => break 'outer,
                    // Same for the terminal pane — macro replay doesn't
                    // forward keys into a PTY, so abort cleanly if focus
                    // happens to land there mid-replay.
                    Mode::Terminal => break 'outer,
                    // And the same for the file-tree pane — replay can't
                    // open files from a sidebar mid-record cleanly, so
                    // bail rather than fire half-meaningful keystrokes.
                    Mode::FileTree => break 'outer,
                    // Rename preview is a single-purpose modal flow —
                    // macros mid-replay would race the user's accept
                    // decision; bail cleanly.
                    Mode::RenamePreview => break 'outer,
                    // Same logic for the installer overlay.
                    Mode::Installer => break 'outer,
                }
            }
        }
        self.macro_replay_depth = self.macro_replay_depth.saturating_sub(1);
        self.replaying_macro = false;
    }

    /// `:reg` / `:registers` — toggle the registers overlay. Yank
    /// registers and macro registers both render. Scroll resets so the
    /// user lands on the first row (the header).
    pub(super) fn cmd_registers(&mut self) {
        self.show_registers_page = true;
        self.registers_scroll = 0;
    }

    pub(super) fn registers_max_scroll(&self) -> usize {
        let total = self.registers_content_height.get();
        let body_rows = self.height.saturating_sub(2) as usize;
        total.saturating_sub(body_rows)
    }

    pub(super) fn registers_scroll_by(&mut self, delta: isize) {
        let max = self.registers_max_scroll();
        let new_scroll = (self.registers_scroll as isize + delta).max(0) as usize;
        self.registers_scroll = new_scroll.min(max);
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
                | Action::Operate {
                    op: Operator::Change,
                    ..
                }
                | Action::OperateLine {
                    op: Operator::Change,
                    ..
                }
                | Action::OperateTextObject {
                    op: Operator::Change,
                    ..
                }
        );
        if enters_insert {
            self.recording = Some(RecordingState {
                prelude: action.clone(),
                keys: Vec::new(),
            });
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

/// OSC 52 payloads above this many bytes are skipped: terminals commonly cap
/// the sequence length and silently truncate oversized payloads, and a
/// blocking write of a large base64 blob would stall the single-threaded event
/// loop over a slow SSH link. `arboard` (local) and the in-memory register are
/// unaffected by the skip.
const MAX_OSC52_BYTES: usize = 64 * 1024;

/// Best-effort write of `text` to the OS clipboard. `arboard` hits the
/// local machine's clipboard; when `osc52` is set we *also* emit the OSC 52
/// terminal sequence so a remote binvim (over SSH) can push the yank into the
/// local terminal's clipboard. Failures are swallowed — the editor still has
/// the text in its in-memory unnamed register. Running locally the OSC 52
/// emission is a harmless no-op: the terminal either sets the clipboard to
/// the same text or ignores the sequence.
///
/// The sequence is emitted only when the payload is non-empty, under
/// `MAX_OSC52_BYTES`, and stdout is actually a terminal — a bare `ESC]52;c;`
/// (empty payload) has terminal-dependent side effects (some treat it as a
/// clipboard *read* request), and control characters don't belong on a
/// redirected stream.
pub fn set_system_clipboard(text: &str, osc52: bool) {
    use std::io::IsTerminal;
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(text.to_string());
    }
    if osc52 && osc52_payload_accepted(text) && std::io::stdout().is_terminal() {
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = out.write_all(osc52_sequence(text).as_bytes());
        let _ = out.flush();
    }
}

/// Whether an OSC 52 payload is worth emitting: non-empty (a bare `ESC]52;c;`
/// has terminal-dependent side effects, e.g. some terminals treat it as a
/// read request) and under `MAX_OSC52_BYTES` (terminals cap/truncate oversized
/// sequences, and a huge blocking write would stall the event loop).
fn osc52_payload_accepted(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_OSC52_BYTES
}

/// The OSC 52 clipboard sequence for `text`: `ESC ] 52 ; c ; <base64> BEL`.
/// `c` selects the clipboard; the BEL terminator is the oldest, most widely
/// accepted form (ESC\ also works). The terminal performs the actual write to
/// the *local* clipboard, which is what lets a remote SSH session's yank
/// reach the user's own desktop.
fn osc52_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))
}

/// Minimal base64 encoder (no new dependency) for OSC 52 payloads — standard
/// alphabet, NUL-free, with `=` padding, exactly what terminals expect.
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Best-effort read of the OS clipboard as UTF-8. Returns `None` when the
/// clipboard is empty, the platform refuses access, or the contents aren't
/// text (an image, a file list, etc.). Swallows every failure so a missing
/// display server / locked clipboard / image payload just makes `p` fall
/// back to the in-memory register instead of erroring out.
///
/// arboard is the fast path, but its macOS reader iterates
/// `NSPasteboard.pasteboardItems` looking for one whose item exposes
/// `NSPasteboardTypeString`. Some apps (Electron-based editors, some
/// browsers, occasionally Microsoft Office) don't lay their pasteboard
/// items out that way — they put text under `public.utf8-plain-text`
/// at the pasteboard level but not on any single item — and arboard
/// returns `ContentNotAvailable` even though `pbpaste` reads them
/// fine. So if arboard fails or returns empty we shell out to the
/// platform's native clipboard reader as a fallback.
pub fn get_system_clipboard() -> Option<String> {
    if let Ok(mut cb) = arboard::Clipboard::new() {
        if let Ok(text) = cb.get_text() {
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    clipboard_fallback_read()
}

#[cfg(target_os = "macos")]
fn clipboard_fallback_read() -> Option<String> {
    let out = std::process::Command::new("pbpaste").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    if text.is_empty() { None } else { Some(text) }
}

#[cfg(target_os = "linux")]
fn clipboard_fallback_read() -> Option<String> {
    // Try wl-paste (Wayland) first, then xclip / xsel (X11). On a
    // Wayland session under XWayland both may exist; wl-paste wins
    // because it talks to the compositor directly.
    let attempts: &[(&str, &[&str])] = &[
        ("wl-paste", &["--no-newline"]),
        ("xclip", &["-selection", "clipboard", "-o"]),
        ("xsel", &["--clipboard", "--output"]),
    ];
    for (cmd, args) in attempts {
        let Ok(out) = std::process::Command::new(cmd).args(*args).output() else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let Ok(text) = String::from_utf8(out.stdout) else {
            continue;
        };
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn clipboard_fallback_read() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inlined encoder must match the RFC 4648 standard base64 that
    /// terminals expect — including the `=` padding edge cases.
    #[test]
    fn base64_encode_matches_standard() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        // Non-ASCII UTF-8 round-trips through the byte stream.
        assert_eq!(
            base64_encode("binvim 修改键".as_bytes()),
            "YmludmltIOS/ruaUuemUrg=="
        );
    }

    /// The OSC 52 sequence is well-formed: `ESC ] 52 ; c ; <base64> BEL`.
    #[test]
    fn osc52_sequence_payload_is_base64_of_text() {
        let seq = osc52_sequence("hello");
        assert!(seq.starts_with("\x1b]52;c;"));
        assert!(seq.ends_with('\x07'), "BEL terminator");
        assert_eq!(seq, "\x1b]52;c;aGVsbG8=\x07");
    }

    /// An empty payload is rejected (a bare `ESC]52;c;` is ambiguous across
    /// terminals) and oversized payloads are skipped so a huge blocking write
    /// can't stall the event loop or exceed a terminal's OSC 52 cap.
    #[test]
    fn osc52_payload_accepted_rejects_empty_and_oversized() {
        let big = "x".repeat(MAX_OSC52_BYTES + 1);
        assert!(!osc52_payload_accepted(""), "empty rejected");
        assert!(
            !osc52_payload_accepted(&big),
            "over the cap rejected: {} bytes",
            big.len()
        );
        assert!(osc52_payload_accepted("hello"), "normal payload accepted");
        assert!(
            osc52_payload_accepted(&"x".repeat(MAX_OSC52_BYTES)),
            "at the cap accepted"
        );
    }
}
