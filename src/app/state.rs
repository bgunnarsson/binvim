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
    /// Display lines in order. The renderer iterates these; each variant
    /// carries enough context to be coloured/styled.
    pub lines: Vec<HoverLine>,
    /// One entry per fenced code block in the original markdown — the
    /// renderer runs tree-sitter on `source` once per block, then slices
    /// the byte-color map per `Code` line via its `byte_offset`/`byte_len`.
    pub code_blocks: Vec<HoverCodeBlock>,
    /// First visible line index when scrolling.
    pub scroll: usize,
    /// Width prose was wrapped to (also the popup's preferred width).
    pub wrap_width: usize,
}

#[derive(Clone)]
pub enum HoverLine {
    /// Empty separator row.
    Blank,
    /// Plain prose line. Already wrapped to `wrap_width` with indentation
    /// preserved on continuation rows.
    Prose(String),
    /// Heading line with the leading `#`s stripped. `level` is the original
    /// `#` count (1..=6) — currently used only to bold the line.
    Heading {
        #[allow(dead_code)]
        level: u8,
        text: String,
    },
    /// Horizontal rule (`---`/`***`/`___` on its own line in markdown).
    Rule,
    /// One line of a fenced code block. The renderer reconstructs colour
    /// for the line by indexing into the corresponding `HoverCodeBlock`.
    Code {
        block_idx: usize,
        byte_offset: usize,
        byte_len: usize,
    },
}

#[derive(Clone)]
pub struct HoverCodeBlock {
    pub lang: Option<crate::lang::Lang>,
    pub source: String,
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
    /// Parse the LSP hover markdown into a structured `HoverState`. Recognises
    /// fenced code blocks (with language tags), `#`-headings, and horizontal
    /// rules; everything else is treated as prose and word-wrapped to fit
    /// `wrap_width`. Code lines are *not* wrapped — they keep their original
    /// indentation, so the renderer paints them as-typed and the user can
    /// scroll horizontally if needed.
    pub fn from_lsp_text(text: &str, term_width: usize) -> Option<Self> {
        let wrap_width = HOVER_MAX_WIDTH.min(term_width.saturating_sub(8).max(20));

        let mut lines: Vec<HoverLine> = Vec::new();
        let mut code_blocks: Vec<HoverCodeBlock> = Vec::new();
        // Active code-block accumulator: (lang_tag, lines collected so far).
        let mut in_code: Option<(Option<crate::lang::Lang>, Vec<String>)> = None;

        for raw in text.lines() {
            // Use the trimmed view for fence detection so leading whitespace
            // before a fence doesn't disqualify it; the original line wins
            // for everything else (we want to keep code indentation intact).
            let trimmed = raw.trim_start();
            if let Some(rest) = trimmed.strip_prefix("```") {
                if let Some((lang, collected)) = in_code.take() {
                    // Closing fence — flush the block.
                    let source = collected.join("\n");
                    let block_idx = code_blocks.len();
                    let mut byte_offset = 0usize;
                    for (i, line) in collected.iter().enumerate() {
                        let byte_len = line.len();
                        lines.push(HoverLine::Code { block_idx, byte_offset, byte_len });
                        // +1 for the synthetic '\n' between joined lines (no trailing
                        // newline on the last line, matching `join`'s behaviour).
                        byte_offset += byte_len + if i + 1 < collected.len() { 1 } else { 0 };
                    }
                    code_blocks.push(HoverCodeBlock { lang, source });
                } else {
                    // Opening fence — record the lang tag (first run of
                    // non-whitespace after the backticks).
                    let tag: String = rest
                        .chars()
                        .take_while(|c| !c.is_whitespace())
                        .collect();
                    let lang = if tag.is_empty() {
                        None
                    } else {
                        crate::lang::Lang::from_md_tag(&tag)
                    };
                    in_code = Some((lang, Vec::new()));
                }
                continue;
            }
            if let Some((_, collected)) = in_code.as_mut() {
                // Inside a code block — preserve leading whitespace exactly,
                // strip trailing whitespace only.
                collected.push(raw.trim_end().to_string());
                continue;
            }

            // Prose / heading / rule.
            let stripped = raw.trim_end();
            if stripped.is_empty() {
                lines.push(HoverLine::Blank);
                continue;
            }
            let trimmed_full = stripped.trim();
            // Horizontal rule — `---` / `***` / `___` (≥3, only those chars).
            if trimmed_full.len() >= 3 {
                let marker = trimmed_full.chars().next().unwrap();
                if matches!(marker, '-' | '*' | '_')
                    && trimmed_full.chars().all(|c| c == marker)
                {
                    lines.push(HoverLine::Rule);
                    continue;
                }
            }
            // ATX heading — leading `#`s, up to 6, followed by space.
            if let Some(level) = atx_heading_level(trimmed_full) {
                let text = trimmed_full
                    .trim_start_matches('#')
                    .trim_start()
                    .to_string();
                lines.push(HoverLine::Heading { level, text });
                continue;
            }
            // Plain prose — wrap with leading-whitespace preserved.
            for w in wrap_prose(stripped, wrap_width) {
                lines.push(HoverLine::Prose(w));
            }
        }

        // Trailing un-closed code block (rare, but handle gracefully).
        if let Some((lang, collected)) = in_code {
            let source = collected.join("\n");
            let block_idx = code_blocks.len();
            let mut byte_offset = 0usize;
            for (i, line) in collected.iter().enumerate() {
                let byte_len = line.len();
                lines.push(HoverLine::Code { block_idx, byte_offset, byte_len });
                byte_offset += byte_len + if i + 1 < collected.len() { 1 } else { 0 };
            }
            code_blocks.push(HoverCodeBlock { lang, source });
        }

        // Trim leading and trailing Blank lines so the popup hugs its content.
        let leading_blanks = lines
            .iter()
            .take_while(|l| matches!(l, HoverLine::Blank))
            .count();
        let trailing_blanks = lines
            .iter()
            .rev()
            .take_while(|l| matches!(l, HoverLine::Blank))
            .count();
        if leading_blanks + trailing_blanks >= lines.len() {
            return None;
        }
        let keep = lines.len() - leading_blanks - trailing_blanks;
        let lines: Vec<HoverLine> = lines
            .into_iter()
            .skip(leading_blanks)
            .take(keep)
            .collect();

        if lines.is_empty() {
            return None;
        }
        Some(HoverState { lines, code_blocks, scroll: 0, wrap_width })
    }

    pub fn max_scroll(&self, visible: usize) -> usize {
        self.lines.len().saturating_sub(visible)
    }

    pub fn scroll_by(&mut self, delta: i64, visible: usize) {
        let max = self.max_scroll(visible);
        let new = (self.scroll as i64 + delta).clamp(0, max as i64);
        self.scroll = new as usize;
    }

    /// Char width of the longest displayable line — used by the renderer
    /// to size the popup. Code lines count their original byte slice;
    /// prose / heading count their final rendered text.
    pub fn widest_line(&self) -> usize {
        self.lines
            .iter()
            .map(|l| match l {
                HoverLine::Blank => 0,
                HoverLine::Rule => 0,
                HoverLine::Prose(s) => s.chars().count(),
                HoverLine::Heading { text, .. } => text.chars().count(),
                HoverLine::Code { block_idx, byte_offset, byte_len } => {
                    let block = &self.code_blocks[*block_idx];
                    let slice = &block.source[*byte_offset..*byte_offset + *byte_len];
                    visual_width(slice)
                }
            })
            .max()
            .unwrap_or(20)
    }
}

/// Visible width of a string when tabs expand to TAB_WIDTH columns. Used
/// for sizing only — the renderer does the actual tab expansion.
fn visual_width(s: &str) -> usize {
    let mut w = 0usize;
    for c in s.chars() {
        if c == '\t' {
            w += crate::render::TAB_WIDTH;
        } else {
            w += 1;
        }
    }
    w
}

/// Returns 1..=6 if `line` is an ATX-style markdown heading. The convention
/// requires a single space between the `#`s and the heading text — e.g.
/// `## Title`. A bare `#` or `#identifier` is not a heading.
fn atx_heading_level(line: &str) -> Option<u8> {
    let mut chars = line.chars();
    let mut hashes = 0u8;
    while let Some('#') = chars.clone().next() {
        chars.next();
        hashes += 1;
        if hashes > 6 {
            return None;
        }
    }
    if hashes == 0 {
        return None;
    }
    match chars.next() {
        Some(' ') => Some(hashes),
        _ => None,
    }
}

/// Wrap a prose line at `width`, preserving any leading whitespace on every
/// produced row so wrapped continuations align under their parent.
fn wrap_prose(line: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![line.to_string()];
    }
    let lead: String = line.chars().take_while(|c| matches!(c, ' ' | '\t')).collect();
    let lead_w = lead.chars().count();
    let body: String = line.chars().skip(lead_w).collect();
    let body_w = width.saturating_sub(lead_w).max(1);
    let wrapped = wrap_words(&body, body_w);
    wrapped.into_iter().map(|w| format!("{lead}{w}")).collect()
}

/// Word-wrap a single line. Hard-breaks tokens longer than the width.
fn wrap_words(line: &str, width: usize) -> Vec<String> {
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
