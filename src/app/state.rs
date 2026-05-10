//! UI/data types and small helpers used across the App's submodules.
//! Constants live here too. Everything is `pub` so siblings can address
//! them directly via `super::state::Foo`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::buffer::Buffer;
use crate::cursor::Cursor;
use crate::lang::HighlightCache;
use crate::lsp::CompletionItem;
use crate::parser::{Action, MotionVerb};
use crate::undo::History;

#[derive(Debug, Clone)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct FindRecord {
    pub ch: char,
    pub forward: bool,
    pub before: bool,
}

/// Per-buffer state. The active buffer's state lives directly on App fields;
/// inactive buffers are stored as stashes in `App.buffers`.
#[derive(Default, Clone)]
pub struct BufferStash {
    pub buffer: Buffer,
    pub cursor: Cursor,
    pub view_top: usize,
    /// Visual columns hidden off the left edge — drives horizontal scrolling
    /// for long lines. Counted in display columns (tabs count as TAB_WIDTH).
    pub view_left: usize,
    pub history: History,
    /// Cached fold ranges + the buffer version they were computed against.
    /// Recomputed lazily when buffer.version drifts.
    pub folds: Vec<FoldRange>,
    pub folds_version: u64,
    /// Start lines of currently-closed folds. We key by start line so
    /// closed-state survives small edits that don't shift line numbers.
    pub closed_folds: std::collections::HashSet<usize>,
    pub visual_anchor: Option<Cursor>,
    pub marks: HashMap<char, (usize, usize)>,
    pub jumplist: Vec<(usize, usize)>,
    pub jump_idx: usize,
    /// Per-buffer syntax-highlight cache. Stashed alongside the buffer so a
    /// switch doesn't leave the previous file's byte-color array pointing at
    /// the new file's contents (which would render as scrambled colours).
    pub highlight_cache: Option<HighlightCache>,
}

#[derive(Debug, Clone)]
pub enum LastEdit {
    Plain(Action),
    InsertSession {
        prelude: Action,
        keys: Vec<KeyEvent>,
    },
}

#[derive(Debug)]
pub struct RecordingState {
    pub prelude: Action,
    pub keys: Vec<KeyEvent>,
}

pub struct CompletionState {
    pub items: Vec<CompletionItem>,
    pub selected: usize,
    /// Position where the existing word-prefix begins; replaced with the chosen item on accept.
    pub anchor_line: usize,
    pub anchor_col: usize,
}

pub struct HoverState {
    /// Word-wrapped display lines.
    pub lines: Vec<String>,
    /// First visible line index when scrolling.
    pub scroll: usize,
    /// Width the lines were wrapped to.
    pub wrap_width: usize,
}

pub const HOVER_MAX_HEIGHT: usize = 15;
pub const HOVER_MAX_WIDTH: usize = 80;

/// A which-key style helper. Shown after the user holds a prefix (currently leader) for
/// `WHICHKEY_DELAY` without resolving it.
pub struct WhichKeyState {
    pub title: String,
    pub entries: Vec<(String, String)>,
}

pub const WHICHKEY_DELAY: Duration = Duration::from_millis(250);

/// Maximum keystroke-burst window before binvim flushes accumulated text
/// changes to attached LSP servers. Per-keystroke didChange floods slow
/// servers (typescript-language-server in particular) and chews CPU; 50ms
/// is fast enough that it feels live but coalesces typical bursts.
pub const LSP_SYNC_DEBOUNCE: Duration = Duration::from_millis(50);

/// How long the yank flash stays painted before it fades.
pub const YANK_FLASH_DURATION: Duration = Duration::from_millis(200);

/// One foldable range in a buffer. `start_line` is the row that becomes
/// the placeholder when the fold closes; `end_line` is inclusive (so the
/// range covers `start_line..=end_line`).
#[derive(Debug, Clone)]
pub struct FoldRange {
    pub start_line: usize,
    pub end_line: usize,
}

/// A char-index range that's currently flashing in the buffer to confirm a
/// yank. Cleared automatically once `expires_at` passes.
pub struct YankHighlight {
    pub start: usize,
    pub end: usize,
    pub expires_at: Instant,
}

pub fn leader_entries() -> Vec<(String, String)> {
    vec![
        ("<space>".into(), "Files".into()),
        ("b".into(), "+Buffer".into()),
        ("g".into(), "Grep".into()),
        ("e".into(), "Yazi".into()),
        ("o".into(), "Doc symbols".into()),
        ("S".into(), "Workspace symbols".into()),
        ("a".into(), "Code actions".into()),
        ("r".into(), "Rename".into()),
    ]
}

pub fn buffer_prefix_entries() -> Vec<(String, String)> {
    vec![
        ("<space>".into(), "Picker".into()),
        ("d".into(), "Delete".into()),
        ("D".into(), "Delete (force)".into()),
        ("o".into(), "Only (close others)".into()),
        ("n".into(), "Next".into()),
        ("p".into(), "Prev".into()),
    ]
}

impl HoverState {
    pub fn from_lsp_text(text: &str, term_width: usize) -> Option<Self> {
        let stripped: Vec<String> = text
            .lines()
            .filter(|line| !is_code_fence(line))
            .map(|s| s.trim_end().to_string())
            .collect();
        let start = stripped
            .iter()
            .position(|l| !l.trim().is_empty())
            .unwrap_or(stripped.len());
        let end = stripped
            .iter()
            .rposition(|l| !l.trim().is_empty())
            .map(|i| i + 1)
            .unwrap_or(start);
        if start >= end {
            return None;
        }
        let wrap_width = HOVER_MAX_WIDTH.min(term_width.saturating_sub(8).max(20));
        let mut lines = Vec::new();
        for raw in &stripped[start..end] {
            if raw.is_empty() {
                lines.push(String::new());
            } else {
                lines.extend(wrap_line(raw, wrap_width));
            }
        }
        if lines.is_empty() {
            return None;
        }
        Some(HoverState { lines, scroll: 0, wrap_width })
    }

    pub fn max_scroll(&self, visible: usize) -> usize {
        self.lines.len().saturating_sub(visible)
    }

    pub fn scroll_by(&mut self, delta: i64, visible: usize) {
        let max = self.max_scroll(visible);
        let new = (self.scroll as i64 + delta).clamp(0, max as i64);
        self.scroll = new as usize;
    }
}

fn is_code_fence(line: &str) -> bool {
    let t = line.trim();
    if !t.starts_with("```") {
        return false;
    }
    t.chars()
        .skip(3)
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '+' || c == '.')
}

/// Word-wrap a single line. Hard-breaks tokens longer than the width.
fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![line.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    for word in line.split(' ') {
        if word.is_empty() {
            if current_len < width && !current.is_empty() {
                current.push(' ');
                current_len += 1;
            }
            continue;
        }
        let word_len = word.chars().count();
        if word_len > width {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
                current_len = 0;
            }
            let chars: Vec<char> = word.chars().collect();
            for chunk in chars.chunks(width) {
                let s: String = chunk.iter().collect();
                if s.chars().count() == width {
                    out.push(s);
                } else {
                    current = s;
                    current_len = current.chars().count();
                }
            }
            continue;
        }
        let need = if current.is_empty() {
            word_len
        } else {
            current_len + 1 + word_len
        };
        if need <= width {
            if !current.is_empty() {
                current.push(' ');
                current_len += 1;
            }
            current.push_str(word);
            current_len += word_len;
        } else {
            out.push(std::mem::take(&mut current));
            current.push_str(word);
            current_len = word_len;
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Motions that should push to the jumplist before they fire — `''` and `<C-o>`
/// only resurface jump-class destinations, not every cursor movement.
pub fn is_jump_motion(m: MotionVerb) -> bool {
    matches!(
        m,
        MotionVerb::FirstLine
            | MotionVerb::LastLine
            | MotionVerb::GotoLine(_)
            | MotionVerb::Mark { .. }
            | MotionVerb::ViewportTop
            | MotionVerb::ViewportMiddle
            | MotionVerb::ViewportBottom
            | MotionVerb::SearchNext { .. }
    )
}

/// Keys that survive the start-page guard while in Normal mode: the cmdline
/// (`:`) and the leader (`<space>`) are the only routes off the start page,
/// plus the usual cancel/interrupt no-ops.
pub fn is_start_page_passthrough(k: &KeyEvent) -> bool {
    let no_mods = !k.modifiers.contains(KeyModifiers::CONTROL)
        && !k.modifiers.contains(KeyModifiers::ALT);
    match k.code {
        KeyCode::Char(':') if no_mods => true,
        KeyCode::Char(' ') if no_mods => true,
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => true,
        KeyCode::Esc => true,
        _ => false,
    }
}
