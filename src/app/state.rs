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

/// Cached per-line markdown render meta for the active buffer. Keyed
/// by `(path, version)` so we recompute when the buffer changes or
/// the user switches to a different file. The cache is mode-independent
/// — whether to APPLY the transforms is decided per-frame by the
/// renderer via `App::markdown_render_active`.
#[derive(Debug, Clone)]
pub struct MarkdownMetaCache {
    pub path: std::path::PathBuf,
    pub version: u64,
    pub per_line: Vec<crate::markdown_render::MarkdownLineMeta>,
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
    /// Working-tree diff against the index, parsed into per-line hunk
    /// markers. Refreshed on save, buffer switch, and explicit `:Gdiff`.
    /// Drives the git stripe painted in the gutter.
    pub git_hunks: Vec<crate::git::GitHunk>,
    /// Whether `:Gblame` virtual text is currently rendered for this
    /// buffer. Toggled per-buffer so the user can have blame on for
    /// one file and off for another.
    pub blame_visible: bool,
    /// One entry per 0-indexed line — `git blame --porcelain` output.
    /// Populated lazily the first time `blame_visible` is turned on;
    /// rebuilt on save / disk reload.
    pub blame: Vec<crate::git::BlameLine>,
    /// Per-line markdown concealed-render meta — same shape as
    /// `App.markdown_meta` but cached against this specific buffer's
    /// path + version. Stashed here (rather than only on App) so an
    /// inactive markdown pane can render with concealed glyphs
    /// rather than as raw source. `None` when the buffer isn't
    /// markdown or the cache hasn't been built yet.
    pub markdown_meta: Option<MarkdownMetaCache>,
    /// Per-buffer split layout. Each buffer carries its own window
    /// tree — switching buffers (via `H`/`L`/`:b`/`:e <existing>`)
    /// swaps the layout too, so other buffers don't inherit a split
    /// the user only meant for the current one. `None` for a stash
    /// that has never been visited (e.g. just pushed by
    /// `open_buffer`); a fresh single-leaf layout is built on first
    /// activation. Inactive panes' window state lives in
    /// `tab_windows`, focused window id in `tab_active_window`.
    pub layout: Option<crate::layout::Layout>,
    pub tab_windows: std::collections::HashMap<crate::layout::WindowId, crate::window::Window>,
    pub tab_active_window: Option<crate::layout::WindowId>,
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

/// One row in the quickfix list — populated from grep results, LSP
/// references, or diagnostics. Line / column are 1-indexed (Vim
/// convention; what `:clist` shows). `text` is a short preview for the
/// list display and is otherwise unused for navigation.
#[derive(Debug, Clone)]
pub struct QuickfixEntry {
    pub path: std::path::PathBuf,
    pub line: usize,
    pub col: usize,
    pub text: String,
}

/// Quickfix list — Vim's `:cnext` / `:cprev` / `]q` / `[q` flow.
///
/// Loaded on demand from grep / references / diagnostics; `current` is
/// the index last jumped to (or `0` on fresh load). Jumping pushes the
/// previous cursor to the jumplist so `<C-o>` returns where the user
/// was. Cleared by `:cclose`.
#[derive(Debug, Clone)]
pub struct QuickfixState {
    pub entries: Vec<QuickfixEntry>,
    pub current: usize,
}

/// Active snippet expansion — Tab cycles the cursor between stops.
///
/// `stops` holds doc-char positions in tab-cycle order (`$1 → $2 → … → $0`).
/// `current` is the index into `stops` of the stop the cursor is currently
/// on (or last advanced to).
///
/// We use a delta-from-anchor scheme to keep the implementation small: at
/// session start (and after every Tab advance) we record the live buffer
/// total char count in `anchor_chars`. The next Tab compares against the
/// live count, shifts every stop after `current` by the delta, and resets
/// the anchor. The assumption is that all edits between Tab presses
/// happen at the current stop, so the cumulative buffer delta equals the
/// amount the later stops need to slide right (or left, on Backspace).
/// Type → Tab → type → Tab keeps everything aligned. Editing elsewhere
/// while in a session is unsupported and may misalign later stops.
///
/// Session ends when:
///  - Insert mode exits (Esc clears it in `handle_insert_key`)
///  - Tab advances past the final stop
#[derive(Debug, Clone)]
pub struct SnippetSession {
    pub stops: Vec<usize>,
    pub current: usize,
    pub anchor_chars: usize,
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

/// Bundle of refs to every per-buffer piece of state the renderer needs —
/// the live buffer rope, its highlight cache, its fold ranges and the
/// closed-fold set, git hunks, blame, and markdown render meta. Built by
/// `App::buffer_state(idx)`, which routes to the live `App` fields when
/// `idx == self.active` and to `App.buffers[idx]` (a `BufferStash`)
/// otherwise. This is the seam that lets each pane of a split show its
/// own buffer's contents + syntax + git stripe rather than mirroring the
/// active pane's data.
pub struct BufferState<'a> {
    pub buffer: &'a crate::buffer::Buffer,
    pub highlight_cache: Option<&'a crate::lang::HighlightCache>,
    pub folds: &'a [FoldRange],
    pub closed_folds: &'a std::collections::HashSet<usize>,
    pub git_hunks: &'a [crate::git::GitHunk],
    pub blame: &'a [crate::git::BlameLine],
    pub blame_visible: bool,
    pub markdown_meta: Option<&'a MarkdownMetaCache>,
    /// True when the renderer should apply markdown concealed-render
    /// transforms to this buffer (file is markdown AND the editor is in
    /// Normal mode). Captured at construction time so the per-line
    /// helpers can answer without re-fetching `App.mode`.
    pub markdown_render_active: bool,
}

impl<'a> BufferState<'a> {
    /// True when `line` is hidden inside a closed fold (i.e. not the
    /// start of one — the start renders as a placeholder).
    pub fn line_is_folded(&self, line: usize) -> bool {
        for f in self.folds {
            if self.closed_folds.contains(&f.start_line)
                && line > f.start_line
                && line <= f.end_line
            {
                return true;
            }
        }
        false
    }

    pub fn line_is_fold_start(&self, line: usize) -> bool {
        self.closed_folds.contains(&line)
            && self.folds.iter().any(|f| f.start_line == line)
    }

    pub fn folded_line_span(&self, line: usize) -> usize {
        if let Some(f) = self
            .folds
            .iter()
            .find(|f| f.start_line == line && self.closed_folds.contains(&f.start_line))
        {
            f.end_line - f.start_line + 1
        } else {
            1
        }
    }

    pub fn markdown_line_meta(
        &self,
        line: usize,
    ) -> Option<&'a crate::markdown_render::MarkdownLineMeta> {
        self.markdown_meta?.per_line.get(line)
    }

    pub fn line_is_md_hidden(&self, line: usize) -> bool {
        if !self.markdown_render_active {
            return false;
        }
        self.markdown_line_meta(line)
            .map(|m| m.kind == crate::markdown_render::MarkdownLineKind::Hidden)
            .unwrap_or(false)
    }

    pub fn visible_rows_between(&self, from: usize, to: usize) -> usize {
        if to <= from {
            return 0;
        }
        let mut count = 0;
        let mut i = from;
        while i < to {
            if !self.line_is_folded(i) && !self.line_is_md_hidden(i) {
                count += 1;
            }
            i += 1;
        }
        count
    }

    pub fn git_hunk_kind_at(&self, line: usize) -> Option<crate::git::GitHunkKind> {
        self.git_hunks
            .iter()
            .find(|h| line >= h.start_line && line <= h.end_line)
            .map(|h| h.kind)
    }

    /// Gutter column count — depends on this buffer's line count, not the
    /// active buffer's, so each pane sizes its gutter correctly.
    pub fn gutter_width(&self) -> usize {
        let n = self.buffer.line_count();
        let digits = format!("{n}").len();
        // 1 git-stripe column + 1 sign column + digits + 1 trailing space.
        digits + 3
    }
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
        ("d".into(), "+Debug".into()),
        ("h".into(), "+Hunk".into()),
        ("t".into(), "+Terminal".into()),
        ("g".into(), "Grep".into()),
        ("e".into(), "Yazi".into()),
        ("a".into(), "Code actions".into()),
        ("r".into(), "Rename".into()),
        ("f".into(), "Format".into()),
        ("/".into(), "Toggle comment".into()),
    ]
}

pub fn terminal_prefix_entries() -> Vec<(String, String)> {
    vec![
        ("t".into(), "Open / focus terminal".into()),
        ("f".into(), "Focus terminal".into()),
        ("q".into(), "Close terminal".into()),
    ]
}

pub fn buffer_prefix_entries() -> Vec<(String, String)> {
    vec![
        ("d".into(), "Delete".into()),
        ("D".into(), "Delete (force)".into()),
        ("a".into(), "Delete all".into()),
        ("A".into(), "Delete all (force)".into()),
        ("o".into(), "Only (close others)".into()),
        ("n".into(), "Next".into()),
        ("p".into(), "Prev".into()),
    ]
}

pub fn hunk_prefix_entries() -> Vec<(String, String)> {
    vec![
        ("p".into(), "Preview hunk".into()),
        ("s".into(), "Stage hunk".into()),
        ("u".into(), "Unstage hunk".into()),
        ("r".into(), "Reset hunk".into()),
    ]
}

pub fn debug_prefix_entries() -> Vec<(String, String)> {
    vec![
        ("s".into(), "Start session".into()),
        ("q".into(), "Stop session".into()),
        ("b".into(), "Toggle breakpoint".into()),
        ("B".into(), "Clear breakpoints (file)".into()),
        ("c".into(), "Continue".into()),
        ("n".into(), "Step over".into()),
        ("i".into(), "Step into".into()),
        ("O".into(), "Step out".into()),
        ("p".into(), "Toggle pane".into()),
        ("f".into(), "Focus pane".into()),
        ("o".into(), "Doc symbols".into()),
        ("S".into(), "Workspace symbols".into()),
    ]
}

impl HoverState {
    /// Parse the LSP hover markdown into a structured `HoverState`. Recognises
    /// fenced code blocks (with language tags), `#`-headings, and horizontal
    /// rules; everything else is treated as prose and word-wrapped to fit
    /// `wrap_width`.
    ///
    /// `wrap_code` controls whether long code lines (e.g. a wide function
    /// signature) get hard-wrapped to `wrap_width`. When false, code keeps
    /// its original line breaks and the renderer clips at the popup edge.
    pub fn from_lsp_text(text: &str, term_width: usize, wrap_code: bool) -> Option<Self> {
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
                    emit_code_lines(
                        &mut lines, block_idx, &collected, wrap_code, wrap_width,
                    );
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
            emit_code_lines(&mut lines, block_idx, &collected, wrap_code, wrap_width);
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

/// Push one `HoverLine::Code` per source line of `collected`, or one
/// `HoverLine::Code` per `wrap_width`-char chunk when `wrap` is true.
/// Each chunk records the byte range it spans in the joined source so
/// the renderer can index into the per-block byte-color cache without
/// re-running tree-sitter.
fn emit_code_lines(
    lines: &mut Vec<HoverLine>,
    block_idx: usize,
    collected: &[String],
    wrap: bool,
    wrap_width: usize,
) {
    let mut byte_offset = 0usize;
    let last = collected.len().saturating_sub(1);
    for (i, line) in collected.iter().enumerate() {
        if !wrap || line.chars().count() <= wrap_width || wrap_width == 0 {
            lines.push(HoverLine::Code {
                block_idx,
                byte_offset,
                byte_len: line.len(),
            });
        } else {
            // Walk char-by-char, accumulating bytes per chunk so the
            // chunk boundary always lands on a char boundary. Multi-byte
            // UTF-8 chars are rare in code but worth handling — splitting
            // mid-char would scramble the byte-color cache.
            let mut chunk_chars = 0usize;
            let mut chunk_start_byte = 0usize;
            let mut byte_pos = 0usize;
            for c in line.chars() {
                if chunk_chars >= wrap_width {
                    lines.push(HoverLine::Code {
                        block_idx,
                        byte_offset: byte_offset + chunk_start_byte,
                        byte_len: byte_pos - chunk_start_byte,
                    });
                    chunk_start_byte = byte_pos;
                    chunk_chars = 0;
                }
                byte_pos += c.len_utf8();
                chunk_chars += 1;
            }
            // Flush the final tail (always non-empty since we just
            // checked the line had more than wrap_width chars).
            if byte_pos > chunk_start_byte {
                lines.push(HoverLine::Code {
                    block_idx,
                    byte_offset: byte_offset + chunk_start_byte,
                    byte_len: byte_pos - chunk_start_byte,
                });
            }
        }
        // +1 for the synthetic '\n' between joined lines (no trailing
        // newline on the last line, matching `join`'s behaviour).
        byte_offset += line.len() + if i < last { 1 } else { 0 };
    }
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
        // H / L cycle buffers — let them through so a restored session's
        // buffers are reachable from the start page.
        KeyCode::Char('H') if no_mods => true,
        KeyCode::Char('L') if no_mods => true,
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => true,
        KeyCode::Esc => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_wrap_off_keeps_long_signatures_on_one_row() {
        // 40-col term → wrap_width = 32. Signature is 75 chars wide,
        // would wrap with the flag on; off, stays as one row.
        let long = "fn foo(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32) -> Result<(), Error>";
        let text = format!("```rust\n{long}\n```");
        let h = HoverState::from_lsp_text(&text, 40, false).expect("hover");
        let code_rows: Vec<&HoverLine> = h
            .lines
            .iter()
            .filter(|l| matches!(l, HoverLine::Code { .. }))
            .collect();
        assert_eq!(code_rows.len(), 1, "wrap_code=false → one Code row per source line");
    }

    #[test]
    fn hover_wrap_on_splits_long_signatures_into_chunks() {
        let long = "fn foo(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32) -> Result<(), Error>";
        let text = format!("```rust\n{long}\n```");
        // 40-col term forces wrap_width = 32; the 75-char signature
        // should split into 3 chunks (32 / 32 / 11 bytes).
        let h = HoverState::from_lsp_text(&text, 40, true).expect("hover");
        let code_rows: Vec<&HoverLine> = h
            .lines
            .iter()
            .filter(|l| matches!(l, HoverLine::Code { .. }))
            .collect();
        assert!(
            code_rows.len() > 1,
            "wrap_code=true → long signature splits into multiple Code rows (got {})",
            code_rows.len(),
        );
        // Sum of byte_lens == original line length (single source line, no '\n').
        let total: usize = code_rows
            .iter()
            .map(|l| match l {
                HoverLine::Code { byte_len, .. } => *byte_len,
                _ => 0,
            })
            .sum();
        assert_eq!(total, long.len(), "chunks together cover the whole source line");
    }

    #[test]
    fn hover_wrap_on_preserves_multi_line_blocks() {
        let text = "```rust\nfn foo() {\n    bar();\n}\n```";
        let h = HoverState::from_lsp_text(text, 200, true).expect("hover");
        let code_rows: Vec<&HoverLine> = h
            .lines
            .iter()
            .filter(|l| matches!(l, HoverLine::Code { .. }))
            .collect();
        // Three short source lines → three Code rows (none gets wrapped).
        assert_eq!(code_rows.len(), 3);
    }
}
