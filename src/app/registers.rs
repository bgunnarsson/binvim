//! Registers, macro recording / replay, and the `.` repeat machinery.
//! Also owns the OS clipboard mirror for the unnamed/`+`/`*` registers.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::Osc52Mode;
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

    /// Insert-mode `Ctrl-R {reg}`. The text goes in literally — Vim's
    /// `Ctrl-R Ctrl-R` — because inserting it as if typed would run
    /// auto-pair and smart indent over pasted code.
    pub(super) fn insert_register_at_cursor(&mut self, name: char) {
        let Some(reg) = self.read_register(Some(name)) else {
            return;
        };
        if !self.additional_cursors.is_empty() {
            for c in reg.text.chars() {
                self.mirror_insert_char(c);
            }
            return;
        }
        let at = self
            .buffer
            .pos_to_char(self.window.cursor.line, self.window.cursor.col);
        self.buffer
            .insert_str(self.window.cursor.line, self.window.cursor.col, &reg.text);
        self.cursor_to_idx(at + reg.text.chars().count());
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
        // A mapping like `Q = "@q"` lands here mid-expansion. The macro's
        // keys were recorded as typed, so they get mappings applied the way
        // typed keys do, not the expansion's noremap treatment.
        let expanding_keymap = std::mem::replace(&mut self.expanding_keymap, false);
        'outer: for _ in 0..count {
            for k in keys.iter().copied() {
                if !self.replay_key(k) {
                    break 'outer;
                }
            }
        }
        self.expanding_keymap = expanding_keymap;
        self.macro_replay_depth = self.macro_replay_depth.saturating_sub(1);
        self.replaying_macro = false;
    }

    /// Feeds one synthetic key — a replayed macro's, or a `[keymaps]`
    /// expansion's — to the current mode's handler. `handle_event`'s guards
    /// are skipped on purpose: they judge what the user typed, and the key
    /// that started the replay already passed them. Returns `false` in a
    /// mode that can't take synthetic keys, so the caller stops feeding.
    pub(super) fn replay_key(&mut self, k: KeyEvent) -> bool {
        let start = self.key_start();
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
            Mode::DebugPane => return false,
            // Same for the terminal pane — macro replay doesn't
            // forward keys into a PTY, so abort cleanly if focus
            // happens to land there mid-replay.
            Mode::Terminal => return false,
            // And the same for the file-tree pane — replay can't
            // open files from a sidebar mid-record cleanly, so
            // bail rather than fire half-meaningful keystrokes.
            Mode::FileTree => return false,
            // Rename preview is a single-purpose modal flow —
            // macros mid-replay would race the user's accept
            // decision; bail cleanly.
            Mode::RenamePreview => return false,
            // Same logic for the installer overlay.
            Mode::Installer => return false,
        }
        self.after_key(start);
        true
    }

    /// `:normal` — `keys` typed in Normal mode on each line of `lines`, from
    /// its first column, or once where the cursor is without a range. A
    /// command left open at the end, Insert mode included, is closed as if by
    /// `Esc`, and each line undoes as one step. Without `remap` (`:normal!`)
    /// the keys aren't mapped.
    pub(super) fn exec_normal(&mut self, lines: Option<(usize, usize)>, keys: &str, remap: bool) {
        let expanding = self.expanding_keymap;
        self.expanding_keymap = expanding || !remap;
        match lines {
            None => self.normal_keys(keys),
            Some((l1, l2)) => {
                for line in l1..=l2 {
                    // Lines the keys deleted on the way are gone, as in Vim.
                    if line >= crate::motion::vim_line_count(&self.buffer) {
                        break;
                    }
                    self.window.cursor = crate::cursor::Cursor {
                        line,
                        col: 0,
                        want_col: 0,
                    };
                    self.normal_keys(keys);
                }
            }
        }
        self.expanding_keymap = expanding;
    }

    /// One run of `:normal`'s keys, closed off and made a single undo step.
    fn normal_keys(&mut self, keys: &str) {
        let depth = self.history.depth();
        self.mode = Mode::Normal;
        for c in keys.chars() {
            if !self.replay_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)) {
                break;
            }
        }
        // A key held as the start of a longer mapping would wait for more.
        if !self.keymap_held.is_empty() {
            self.keymap_flush(self.keymap_mode());
        }
        // Whatever the keys left open — Insert, a waiting operator, a `:`
        // line — ends the way `Esc` would end it.
        for _ in 0..3 {
            if self.mode == Mode::Normal && self.pending.is_clean() {
                break;
            }
            self.replay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        }
        self.history.squash_since(depth);
    }

    /// `:reg` / `:registers` — toggle the registers overlay. Yank
    /// registers and macro registers both render. Scroll resets so the
    /// user lands on the first row (the header).
    pub(super) fn cmd_registers(&mut self) {
        self.show_list_page = true;
        self.listing = None;
        self.list_scroll = 0;
    }

    pub(super) fn list_max_scroll(&self) -> usize {
        let total = self.list_content_height.get();
        let body_rows = self.height.saturating_sub(2) as usize;
        total.saturating_sub(body_rows)
    }

    pub(super) fn list_scroll_by(&mut self, delta: isize) {
        let max = self.list_max_scroll();
        let new_scroll = (self.list_scroll as isize + delta).max(0) as usize;
        self.list_scroll = new_scroll.min(max);
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
                | Action::EnterReplace { .. }
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
                resumed: false,
            });
            return;
        }
        let plain_recordable = match action {
            Action::Operate { op, .. }
            | Action::OperateLine { op, .. }
            | Action::OperateTextObject { op, .. } => {
                matches!(
                    op,
                    Operator::Delete
                        | Operator::Reindent
                        | Operator::Format { .. }
                        | Operator::Case(_)
                )
            }
            Action::DeleteCharForward { .. }
            | Action::Put { .. }
            | Action::VisualPut { .. }
            | Action::ReplaceChar { .. }
            | Action::JoinLines { .. }
            | Action::AdjustNumber { .. }
            | Action::ToggleCase { .. }
            | Action::SurroundAdd { .. } => true,
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

/// Cap on the *encoded* OSC 52 payload: terminals commonly cap the sequence
/// length and silently truncate past it, and a blocking write of a large
/// base64 blob would stall the single-threaded event loop over a slow SSH
/// link. The check has to run on the encoded length rather than the source
/// text — base64 inflates by 4/3, so a 64 KiB yank measured before encoding
/// would emit ~87 KiB. `arboard` (local) and the in-memory register are
/// unaffected by the skip.
const MAX_OSC52_ENCODED_BYTES: usize = 64 * 1024;

/// GNU screen truncates a DCS string somewhere past 768 bytes, so a long
/// sequence has to go out as several passthroughs. The outer terminal sees
/// one continuous byte stream, so splitting mid-sequence is fine.
const SCREEN_DCS_CHUNK: usize = 400;

/// Best-effort write of `text` to the OS clipboard. `arboard` hits the
/// local machine's clipboard; the OSC 52 sequence asks the *terminal* to
/// write the local one, which is what gets a yank out of a binvim running
/// over SSH and onto the user's own desktop. Failures are swallowed — the
/// editor still has the text in its in-memory unnamed register.
///
/// The sequence is emitted only when `mode` calls for it (see [`Osc52Mode`]),
/// the payload is non-empty and under [`MAX_OSC52_ENCODED_BYTES`], and stdout
/// is actually a terminal — a bare `ESC]52;c;` (empty payload) has
/// terminal-dependent side effects (some treat it as a clipboard *read*
/// request), and control characters don't belong on a redirected stream.
pub fn set_system_clipboard(text: &str, mode: Osc52Mode) {
    use std::io::IsTerminal;
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(text.to_string());
    }
    if !osc52_enabled(mode, is_ssh_session())
        || !osc52_payload_accepted(text)
        || !std::io::stdout().is_terminal()
    {
        return;
    }
    use std::io::Write;
    let wire = osc52_wire(&osc52_sequence(text), detect_multiplexer());
    let mut out = std::io::stdout();
    let _ = out.write_all(&wire);
    let _ = out.flush();
}

/// Whether to emit the sequence at all. `Auto` gates on being the far end of
/// an SSH connection — see [`Osc52Mode::Auto`] for why local emission is a
/// cost rather than a no-op.
fn osc52_enabled(mode: Osc52Mode, is_ssh: bool) -> bool {
    match mode {
        Osc52Mode::Never => false,
        Osc52Mode::Always => true,
        Osc52Mode::Auto => is_ssh,
    }
}

/// Whether this binvim is on the far end of an SSH connection. sshd sets all
/// three of these; any one is enough, and a set-but-empty value doesn't count
/// (that's how a login shell clears an inherited one).
fn is_ssh_session() -> bool {
    ["SSH_TTY", "SSH_CONNECTION", "SSH_CLIENT"]
        .iter()
        .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
}

/// Whether an OSC 52 payload is worth emitting: non-empty (a bare `ESC]52;c;`
/// has terminal-dependent side effects, e.g. some terminals treat it as a
/// read request) and, once encoded, under `MAX_OSC52_ENCODED_BYTES`.
fn osc52_payload_accepted(text: &str) -> bool {
    !text.is_empty() && base64_encoded_len(text.len()) <= MAX_OSC52_ENCODED_BYTES
}

/// Length of `n` bytes once base64-encoded, `=` padding included.
fn base64_encoded_len(n: usize) -> usize {
    n.div_ceil(3) * 4
}

/// What sits between binvim and the terminal that owns the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Multiplexer {
    None,
    Tmux,
    Screen,
}

fn detect_multiplexer() -> Multiplexer {
    if std::env::var_os("TMUX").is_some() {
        Multiplexer::Tmux
    } else if std::env::var_os("STY").is_some() {
        Multiplexer::Screen
    } else {
        Multiplexer::None
    }
}

/// The bytes to actually write for `seq`, given what it has to travel through.
///
/// Under tmux we emit the raw sequence *and* the DCS-wrapped one, because the
/// two routes out are gated separately and a user has typically enabled at
/// most one: the raw sequence needs `set-clipboard on` (the default,
/// `external`, drops what applications send), and the wrapped one needs
/// `allow-passthrough on` (off by default since tmux 3.3). Sending both means
/// either setting is enough. If both are on the clipboard is set twice to the
/// same text, which nobody can observe; if neither is, tmux swallows both and
/// nothing reaches the screen.
fn osc52_wire(seq: &str, mux: Multiplexer) -> Vec<u8> {
    match mux {
        Multiplexer::None => seq.as_bytes().to_vec(),
        Multiplexer::Tmux => {
            // Every ESC inside a tmux passthrough has to be doubled, or tmux
            // ends the DCS at the first one and prints the rest.
            let escaped = seq.replace('\x1b', "\x1b\x1b");
            format!("{seq}\x1bPtmux;{escaped}\x1b\\").into_bytes()
        }
        Multiplexer::Screen => {
            let mut out = Vec::with_capacity(seq.len() + 32);
            for chunk in seq.as_bytes().chunks(SCREEN_DCS_CHUNK) {
                out.extend_from_slice(b"\x1bP");
                out.extend_from_slice(chunk);
                out.extend_from_slice(b"\x1b\\");
            }
            out
        }
    }
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
    ///
    /// The cap is measured *after* encoding — a source-text check would let
    /// base64's 4/3 inflation push the wire payload a third past the limit.
    #[test]
    fn osc52_payload_accepted_measures_the_encoded_length() {
        assert!(!osc52_payload_accepted(""), "empty rejected");
        assert!(osc52_payload_accepted("hello"), "normal payload accepted");

        // Largest source text that still encodes within the cap, and one
        // group of three bytes past it.
        let at_cap = "x".repeat(MAX_OSC52_ENCODED_BYTES / 4 * 3);
        assert_eq!(base64_encoded_len(at_cap.len()), MAX_OSC52_ENCODED_BYTES);
        assert!(osc52_payload_accepted(&at_cap), "at the cap accepted");

        let over = "x".repeat(at_cap.len() + 3);
        assert!(!osc52_payload_accepted(&over), "over the cap rejected");

        // The regression this guards: a source text under the cap whose
        // encoding is not. Measured before encoding, this would have gone out
        // as ~87 KiB.
        let inflates_past = "x".repeat(MAX_OSC52_ENCODED_BYTES);
        assert!(base64_encoded_len(inflates_past.len()) > MAX_OSC52_ENCODED_BYTES);
        assert!(
            !osc52_payload_accepted(&inflates_past),
            "source text at the cap encodes past it, so it must be rejected"
        );
    }

    /// `Auto` — the default — is the only mode that consults the environment.
    #[test]
    fn osc52_auto_emits_only_over_ssh() {
        assert!(!osc52_enabled(Osc52Mode::Auto, false), "local: no sequence");
        assert!(osc52_enabled(Osc52Mode::Auto, true), "ssh: sequence");
        assert!(osc52_enabled(Osc52Mode::Always, false));
        assert!(osc52_enabled(Osc52Mode::Always, true));
        assert!(!osc52_enabled(Osc52Mode::Never, false));
        assert!(!osc52_enabled(Osc52Mode::Never, true));
    }

    /// Bare terminal: the sequence goes out exactly as built.
    #[test]
    fn osc52_wire_is_untouched_without_a_multiplexer() {
        let seq = osc52_sequence("hi");
        assert_eq!(osc52_wire(&seq, Multiplexer::None), seq.as_bytes());
    }

    /// tmux gets both routes: the raw sequence (for `set-clipboard on`) and
    /// the DCS passthrough (for `allow-passthrough on`), with every ESC in
    /// the wrapped copy doubled so tmux doesn't end the DCS early.
    #[test]
    fn osc52_wire_wraps_for_tmux_and_doubles_escapes() {
        let wire = String::from_utf8(osc52_wire("\x1b]52;c;aGk=\x07", Multiplexer::Tmux)).unwrap();
        assert_eq!(
            wire,
            "\x1b]52;c;aGk=\x07\x1bPtmux;\x1b\x1b]52;c;aGk=\x07\x1b\\"
        );
        // The passthrough body must carry no lone ESC, or tmux terminates
        // the DCS at it and prints the remainder to the pane. Peel the pairs
        // off and nothing should be left.
        let body = wire.split_once("\x1bPtmux;").unwrap().1;
        let body = body.strip_suffix("\x1b\\").unwrap();
        assert!(
            !body.replace("\x1b\x1b", "").contains('\x1b'),
            "lone ESC left in passthrough body"
        );
    }

    /// screen truncates a long DCS, so the sequence goes out in chunks that
    /// the outer terminal reassembles into one byte stream.
    #[test]
    fn osc52_wire_chunks_for_screen() {
        let short = osc52_wire("\x1b]52;c;aGk=\x07", Multiplexer::Screen);
        assert_eq!(short, b"\x1bP\x1b]52;c;aGk=\x07\x1b\\");

        // A payload spanning several chunks: strip the DCS framing and the
        // original sequence must come back byte for byte.
        let seq = osc52_sequence(&"x".repeat(SCREEN_DCS_CHUNK * 2));
        let wire = String::from_utf8(osc52_wire(&seq, Multiplexer::Screen)).unwrap();
        let chunks: Vec<&str> = wire
            .split("\x1b\\")
            .filter(|s| !s.is_empty())
            .map(|s| s.strip_prefix("\x1bP").expect("chunk carries DCS prefix"))
            .collect();
        assert!(chunks.len() > 1, "long sequence must be split");
        assert!(chunks.iter().all(|c| c.len() <= SCREEN_DCS_CHUNK));
        assert_eq!(chunks.concat(), seq);
    }
}
