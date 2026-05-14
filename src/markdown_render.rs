//! Markdown "concealed" render mode — paints a `.md` buffer with the
//! syntax markers (`**`, `*`, `` ` ``, `# `, `[…](…)`, `- `, `> `)
//! visually hidden or replaced with prettier glyphs. The buffer text
//! is never mutated; this is a per-line list of display transforms +
//! style overrides that the renderer consults when the active buffer
//! is markdown AND the editor is in Normal mode.
//!
//! Why a hand-rolled scanner instead of the tree-sitter-md AST: we
//! only care about a handful of structural / inline patterns, all
//! line-local except code fences (which we deliberately don't
//! handle in v1 — see the module-level limitations note in
//! ROADMAP.md). A char-walk is shorter, faster, and avoids pulling
//! the inline-grammar second pass into the render loop.
//!
//! Char-column based (not byte-column) because the renderer iterates
//! by char; mixing byte offsets in would force every consumer to
//! convert.

use crossterm::style::Color;

#[derive(Debug, Clone)]
pub struct MarkdownTransform {
    /// Char column where the transform begins (inclusive, line-relative).
    pub start: usize,
    /// Char column where the transform ends (exclusive, line-relative).
    pub end: usize,
    pub action: ConcealAction,
}

#[derive(Debug, Clone)]
pub enum ConcealAction {
    /// Render nothing — the source chars vanish from the display.
    Hide,
    /// Replace the source span with this glyph in the given colour.
    Replace { glyph: &'static str, color: Color },
}

#[derive(Debug, Clone)]
pub struct MarkdownStyleRange {
    /// Char column where the style begins (inclusive, line-relative).
    pub start: usize,
    /// Char column where the style ends (exclusive, line-relative).
    pub end: usize,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub color: Option<Color>,
}

/// Whole-line render decisions that override the per-char loop. Most
/// lines are `Default` (the renderer walks chars and applies
/// transforms / styles); these special kinds short-circuit or layer
/// on extra behaviour:
/// - `Hidden` paints a blank row (used for setext underlines that
///   collapse into the heading above).
/// - `HorizontalRule` paints a continuous `─` line in dim across the
///   buffer area's width.
/// - `CodeBlock` paints the row with a Mantle background (extending
///   to the right edge) so opener / body / closer rows all share
///   the dark chrome of the block. Per-char transforms and styles
///   still apply on top — opener hides backticks + paints lang
///   tag, body keeps tree-sitter colour, closer hides backticks
///   (renders as a blank dark row that closes the block visually).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MarkdownLineKind {
    #[default]
    Default,
    Hidden,
    HorizontalRule,
    CodeBlock,
}

#[derive(Debug, Clone, Default)]
pub struct MarkdownLineMeta {
    pub transforms: Vec<MarkdownTransform>,
    pub styles: Vec<MarkdownStyleRange>,
    pub kind: MarkdownLineKind,
}

// Catppuccin Mocha
const HEADING_COLOR: Color = Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe }; // Lavender
const CODE_COLOR: Color = Color::Rgb { r: 0xa6, g: 0xe3, b: 0xa1 };    // Green
const LINK_COLOR: Color = Color::Rgb { r: 0x89, g: 0xb4, b: 0xfa };    // Blue
const BULLET_COLOR: Color = Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 };  // Peach
const QUOTE_COLOR: Color = Color::Rgb { r: 0x6c, g: 0x70, b: 0x86 };   // Overlay0

/// Heading levels render in slightly different intensity so an outline
/// reads as a hierarchy rather than a wall of bold-Lavender. H1 / H2
/// land on Lavender (default heading); H3+ steps down through Sapphire
/// then Sky so deeper sections still pop but don't compete with the
/// top-level titles.
fn heading_color(level: usize) -> Color {
    match level {
        1 | 2 => HEADING_COLOR,
        3 => Color::Rgb { r: 0x74, g: 0xc7, b: 0xec }, // Sapphire
        _ => Color::Rgb { r: 0x89, g: 0xdc, b: 0xeb }, // Sky
    }
}

pub fn compute_line_meta(line: &str) -> MarkdownLineMeta {
    let chars: Vec<char> = line.chars().collect();
    let mut meta = MarkdownLineMeta::default();
    if chars.is_empty() {
        return meta;
    }

    let leading_ws = chars.iter().take_while(|c| **c == ' ').count();

    // Heading — `^( *)#{1,6} text`. Bail early; headings consume the
    // whole line and don't get inline-marker scanning.
    if leading_ws < chars.len() && chars[leading_ws] == '#' {
        let mut hash_end = leading_ws;
        while hash_end < chars.len() && chars[hash_end] == '#' {
            hash_end += 1;
        }
        let level = hash_end - leading_ws;
        if (1..=6).contains(&level)
            && hash_end < chars.len()
            && chars[hash_end] == ' '
        {
            meta.transforms.push(MarkdownTransform {
                start: leading_ws,
                end: hash_end + 1,
                action: ConcealAction::Hide,
            });
            meta.styles.push(MarkdownStyleRange {
                start: hash_end + 1,
                end: chars.len(),
                bold: true,
                italic: false,
                underline: false,
                strikethrough: false,
                color: Some(heading_color(level)),
            });
            return meta;
        }
    }

    // Block quote — `^( *)> ` (or trailing `>` with no body).
    let mut body_start = 0usize;
    if leading_ws < chars.len() && chars[leading_ws] == '>' {
        let after = leading_ws + 1;
        let end = if after < chars.len() && chars[after] == ' ' {
            after + 1
        } else {
            after
        };
        meta.transforms.push(MarkdownTransform {
            start: leading_ws,
            end,
            action: ConcealAction::Replace { glyph: "▎ ", color: QUOTE_COLOR },
        });
        meta.styles.push(MarkdownStyleRange {
            start: end,
            end: chars.len(),
            bold: false,
            italic: true,
            underline: false,
            strikethrough: false,
            color: Some(QUOTE_COLOR),
        });
        body_start = end;
    }

    // Bullet list — `^( *)([-*+]) `. Replace just the marker char.
    if leading_ws < chars.len()
        && body_start <= leading_ws
        && matches!(chars[leading_ws], '-' | '*' | '+')
        && leading_ws + 1 < chars.len()
        && chars[leading_ws + 1] == ' '
    {
        meta.transforms.push(MarkdownTransform {
            start: leading_ws,
            end: leading_ws + 1,
            action: ConcealAction::Replace { glyph: "•", color: BULLET_COLOR },
        });
    }

    scan_inline(&chars, body_start, &mut meta);

    meta.transforms.sort_by_key(|t| t.start);
    meta.styles.sort_by_key(|s| s.start);
    meta
}

/// Multi-line pass — needs the whole buffer because some classifications
/// (setext headings, code-fence interior, top-of-file frontmatter) are
/// only decidable with cross-line context. Returns one `MarkdownLineMeta`
/// per input line.
pub fn compute_buffer_meta(lines: &[String]) -> Vec<MarkdownLineMeta> {
    let mut out: Vec<MarkdownLineMeta> = Vec::with_capacity(lines.len());
    let mut fence: Option<char> = None;
    let mut in_frontmatter = false;
    for (i, line) in lines.iter().enumerate() {
        // Top-of-file YAML / TOML frontmatter — opening `---` (or
        // `+++` for TOML) on line 0, runs until the matching closer.
        // Render every frontmatter row in muted Overlay0 italic so it
        // reads as metadata, not content.
        if i == 0 {
            let t = line.trim();
            if t == "---" || t == "+++" {
                in_frontmatter = true;
                out.push(frontmatter_meta(line));
                continue;
            }
        }
        if in_frontmatter {
            let t = line.trim();
            // YAML closes on `---` or `...`; TOML closes on `+++`.
            if t == "---" || t == "..." || t == "+++" {
                in_frontmatter = false;
            }
            out.push(frontmatter_meta(line));
            continue;
        }

        // Code fence — open or close. Inside a fence we suppress
        // inline transforms and decorate the fence boundaries
        // themselves.
        let trimmed = line.trim_start();
        let leading = line.chars().take_while(|c| *c == ' ').count();
        let fence_ch = if leading <= 3 {
            if trimmed.starts_with("```") {
                Some('`')
            } else if trimmed.starts_with("~~~") {
                Some('~')
            } else {
                None
            }
        } else {
            None
        };
        if let Some(ch) = fence_ch {
            match fence {
                Some(open_ch) if open_ch == ch => {
                    fence = None;
                    out.push(fence_close_meta(line, leading, ch));
                    continue;
                }
                None => {
                    fence = Some(ch);
                    out.push(fence_open_meta(line, leading, ch));
                    continue;
                }
                Some(_) => {
                    // Different fence char inside an open fence —
                    // not a close, treat as code content.
                }
            }
        }
        if fence.is_some() {
            // Inside a code block — no transforms, no styling
            // overrides; tree-sitter / config syntax colour wins.
            // The CodeBlock kind tells the renderer to paint the
            // row with the Mantle background so the block reads as
            // a unified dark slab.
            out.push(MarkdownLineMeta {
                kind: MarkdownLineKind::CodeBlock,
                ..MarkdownLineMeta::default()
            });
            continue;
        }

        // Setext underline (`====` / `----` on a line below prose).
        // Re-classifies the previous line as H1 / H2 and hides this
        // underline row. Otherwise this is either an HR or normal
        // content (handled below).
        if let Some(level) = setext_level(line) {
            let prev_idx = i.checked_sub(1);
            let prev_is_prose = prev_idx
                .and_then(|p| lines.get(p))
                .map(|p| is_plain_prose(p))
                .unwrap_or(false);
            // Only treat as setext if the previous line is plain
            // prose AND we haven't already classified it as something
            // else (HR / heading / fence / blockquote / list).
            let prev_was_default = prev_idx
                .and_then(|p| out.get(p))
                .map(|m| m.kind == MarkdownLineKind::Default)
                .unwrap_or(false);
            if prev_is_prose && prev_was_default {
                let prev_chars = lines[prev_idx.unwrap()].chars().count();
                let m = &mut out[prev_idx.unwrap()];
                m.transforms.clear();
                m.styles.clear();
                m.styles.push(MarkdownStyleRange {
                    start: 0,
                    end: prev_chars,
                    bold: true,
                    italic: false,
                    underline: false,
                    strikethrough: false,
                    color: Some(heading_color(level)),
                });
                out.push(MarkdownLineMeta {
                    kind: MarkdownLineKind::Hidden,
                    ..MarkdownLineMeta::default()
                });
                continue;
            }
        }

        // Horizontal rule — standalone `---` / `***` / `___` (3+ of
        // the same char, optional spaces). Renders as a continuous
        // dim `─` line spanning the buffer width.
        if is_hr_line(line) {
            out.push(MarkdownLineMeta {
                kind: MarkdownLineKind::HorizontalRule,
                ..MarkdownLineMeta::default()
            });
            continue;
        }

        out.push(compute_line_meta(line));
    }
    out
}

fn frontmatter_meta(line: &str) -> MarkdownLineMeta {
    let mut meta = MarkdownLineMeta::default();
    let n = line.chars().count();
    if n > 0 {
        meta.styles.push(MarkdownStyleRange {
            start: 0,
            end: n,
            bold: false,
            italic: true,
            underline: false,
            strikethrough: false,
            color: Some(QUOTE_COLOR),
        });
    }
    meta
}

/// Build the meta for a fence opener — hide the fence chars, style
/// the language tag in bold-Peach, mark the row as `CodeBlock` so the
/// renderer paints the Mantle background across the full width.
fn fence_open_meta(line: &str, leading: usize, fence_ch: char) -> MarkdownLineMeta {
    let chars: Vec<char> = line.chars().collect();
    let backtick_start = leading;
    let mut backtick_end = backtick_start;
    while backtick_end < chars.len() && chars[backtick_end] == fence_ch {
        backtick_end += 1;
    }
    let mut meta = MarkdownLineMeta {
        kind: MarkdownLineKind::CodeBlock,
        ..MarkdownLineMeta::default()
    };
    meta.transforms.push(MarkdownTransform {
        start: backtick_start,
        end: backtick_end,
        action: ConcealAction::Hide,
    });
    if backtick_end < chars.len() {
        meta.styles.push(MarkdownStyleRange {
            start: backtick_end,
            end: chars.len(),
            bold: true,
            italic: false,
            underline: false,
            strikethrough: false,
            color: Some(BULLET_COLOR),
        });
    }
    meta
}

/// Build the meta for a fence closer — hide every char on the line
/// (so the row appears empty) but mark `CodeBlock` so the renderer
/// still paints the Mantle background. Visually this gives the
/// block a "footer" row of solid dark bg that closes the slab
/// without re-displaying the backticks.
fn fence_close_meta(line: &str, leading: usize, fence_ch: char) -> MarkdownLineMeta {
    let chars: Vec<char> = line.chars().collect();
    let mut backtick_end = leading;
    while backtick_end < chars.len() && chars[backtick_end] == fence_ch {
        backtick_end += 1;
    }
    let mut meta = MarkdownLineMeta {
        kind: MarkdownLineKind::CodeBlock,
        ..MarkdownLineMeta::default()
    };
    if leading < backtick_end {
        meta.transforms.push(MarkdownTransform {
            start: leading,
            end: backtick_end,
            action: ConcealAction::Hide,
        });
    }
    meta
}

/// Detect a setext heading underline. Returns `Some(1)` for `====`,
/// `Some(2)` for `----`. Both must consist solely of that char (with
/// optional surrounding whitespace) — otherwise it's not a setext
/// underline.
fn setext_level(line: &str) -> Option<usize> {
    let t = line.trim();
    if t.is_empty() {
        return None;
    }
    if t.chars().all(|c| c == '=') {
        Some(1)
    } else if t.chars().all(|c| c == '-') {
        Some(2)
    } else {
        None
    }
}

/// Thematic break detector — 3+ of `-`, `*`, or `_` on a line, with
/// only whitespace between them. CommonMark allows up to 3 leading
/// spaces of indent.
fn is_hr_line(line: &str) -> bool {
    let t = line.trim();
    if t.chars().count() < 3 {
        return false;
    }
    let first = match t.chars().next() {
        Some(c) if c == '-' || c == '*' || c == '_' => c,
        _ => return false,
    };
    let mut count = 0;
    for c in t.chars() {
        if c == first {
            count += 1;
        } else if !c.is_whitespace() {
            return false;
        }
    }
    count >= 3
}

/// True when a line is "plain prose" — a candidate for setext
/// heading promotion. Excludes block-element openers (ATX heading,
/// blockquote, list item, fence, HR pattern) so a `---` underneath
/// a `# Foo` doesn't get misread as a setext H2.
fn is_plain_prose(line: &str) -> bool {
    let t = line.trim_start();
    if t.is_empty() {
        return false;
    }
    if t.starts_with('#') || t.starts_with('>') {
        return false;
    }
    if t.starts_with("```") || t.starts_with("~~~") {
        return false;
    }
    let mut chars = t.chars();
    if let Some(first) = chars.next() {
        if matches!(first, '-' | '*' | '+') && chars.next() == Some(' ') {
            return false;
        }
    }
    if is_hr_line(line) {
        return false;
    }
    true
}

/// Walk the line looking for inline markers. `body_start` lets the
/// blockquote prefix opt out of being re-scanned (so `> **bold**`
/// works without the leading `>` confusing the bold detector).
fn scan_inline(chars: &[char], body_start: usize, meta: &mut MarkdownLineMeta) {
    let n = chars.len();
    let mut i = body_start;
    while i < n {
        let c = chars[i];

        // Inline code — `` `text` ``. Highest priority: anything inside a
        // code span shouldn't get bold/italic/link scanning, so we match
        // first and skip past the close marker.
        if c == '`' {
            if let Some(end) = find_close(chars, i + 1, '`') {
                if end > i + 1 {
                    meta.transforms.push(MarkdownTransform {
                        start: i,
                        end: i + 1,
                        action: ConcealAction::Hide,
                    });
                    meta.transforms.push(MarkdownTransform {
                        start: end,
                        end: end + 1,
                        action: ConcealAction::Hide,
                    });
                    meta.styles.push(MarkdownStyleRange {
                        start: i + 1,
                        end,
                        bold: false,
                        italic: false,
                        underline: false,
                        strikethrough: false,
                        color: Some(CODE_COLOR),
                    });
                    i = end + 1;
                    continue;
                }
            }
        }

        // Strikethrough — `~~text~~`. GFM extension; same shape as
        // bold (double-marker) so we use the same flanking guards
        // (opener not before whitespace, closer not after whitespace).
        // No intraword restriction needed — `~` isn't a word char so
        // `f~~o~~o` is plausibly intentional.
        if c == '~' && chars.get(i + 1).copied() == Some('~') {
            let after_open = chars.get(i + 2).copied();
            if !is_ws(after_open) {
                if let Some(close) = find_double_close(chars, i + 2, '~') {
                    if close > i + 2 {
                        let before_close = chars.get(close - 1).copied();
                        if !is_ws(before_close) {
                            meta.transforms.push(MarkdownTransform {
                                start: i,
                                end: i + 2,
                                action: ConcealAction::Hide,
                            });
                            meta.transforms.push(MarkdownTransform {
                                start: close,
                                end: close + 2,
                                action: ConcealAction::Hide,
                            });
                            meta.styles.push(MarkdownStyleRange {
                                start: i + 2,
                                end: close,
                                bold: false,
                                italic: false,
                                underline: false,
                                strikethrough: true,
                                color: Some(QUOTE_COLOR),
                            });
                            i = close + 2;
                            continue;
                        }
                    }
                }
            }
        }

        // Bold — `**text**` / `__text__`. Match before italic so the
        // inner `*` of a bold pair doesn't trip the single-marker
        // italic case. Same flanking rules as italic below: an opener
        // can't sit before whitespace, a closer can't sit after
        // whitespace, and `__` won't open/close intraword (CommonMark
        // forbids underscore emphasis flanked by alphanumerics on
        // both sides — saves us from matching the `_API_` inside
        // `ANTHROPIC_API_KEY`).
        if (c == '*' || c == '_') && chars.get(i + 1).copied() == Some(c) {
            let after_open = chars.get(i + 2).copied();
            let before_open = if i > 0 { Some(chars[i - 1]) } else { None };
            let opener_ok = !is_ws(after_open)
                && !(c == '_' && is_word(before_open) && is_word(after_open));
            if opener_ok {
                if let Some(close) = find_double_close(chars, i + 2, c) {
                    if close > i + 2 {
                        let before_close = chars.get(close - 1).copied();
                        let after_close = chars.get(close + 2).copied();
                        let closer_ok = !is_ws(before_close)
                            && !(c == '_'
                                && is_word(before_close)
                                && is_word(after_close));
                        if closer_ok {
                            meta.transforms.push(MarkdownTransform {
                                start: i,
                                end: i + 2,
                                action: ConcealAction::Hide,
                            });
                            meta.transforms.push(MarkdownTransform {
                                start: close,
                                end: close + 2,
                                action: ConcealAction::Hide,
                            });
                            meta.styles.push(MarkdownStyleRange {
                                start: i + 2,
                                end: close,
                                bold: true,
                                italic: false,
                                underline: false,
                                strikethrough: false,
                                color: None,
                            });
                            i = close + 2;
                            continue;
                        }
                    }
                }
            }
        }

        // Italic — `*text*` / `_text_`. Adjacent same-char rules:
        // - skip if next char is the same (would have been bold)
        // - skip if previous char is the same (we're mid-bold close)
        // - close marker must not be followed by the same char either
        // Plus flanking: opener can't precede whitespace; closer
        // can't follow whitespace; `_` can't open/close intraword
        // (`f_o_o` is not italic per CommonMark).
        if c == '*' || c == '_' {
            let next = chars.get(i + 1).copied();
            let prev = if i > 0 { Some(chars[i - 1]) } else { None };
            if next != Some(c)
                && prev != Some(c)
                && !is_ws(next)
                && !(c == '_' && is_word(prev) && is_word(next))
            {
                if let Some(close) = find_close(chars, i + 1, c) {
                    if close > i + 1 && chars.get(close + 1).copied() != Some(c) {
                        let before_close = chars.get(close - 1).copied();
                        let after_close = chars.get(close + 1).copied();
                        let closer_ok = !is_ws(before_close)
                            && !(c == '_'
                                && is_word(before_close)
                                && is_word(after_close));
                        if closer_ok {
                            meta.transforms.push(MarkdownTransform {
                                start: i,
                                end: i + 1,
                                action: ConcealAction::Hide,
                            });
                            meta.transforms.push(MarkdownTransform {
                                start: close,
                                end: close + 1,
                                action: ConcealAction::Hide,
                            });
                            meta.styles.push(MarkdownStyleRange {
                                start: i + 1,
                                end: close,
                                bold: false,
                                italic: true,
                                underline: false,
                                strikethrough: false,
                                color: None,
                            });
                            i = close + 1;
                            continue;
                        }
                    }
                }
            }
        }

        // Link — `[text](url)`. Hide the brackets and the parenthesised
        // URL, leaving just the visible link text underlined in Blue.
        if c == '[' {
            if let Some(rb) = find_close(chars, i + 1, ']') {
                if rb + 1 < n
                    && chars[rb + 1] == '('
                    && rb > i + 1
                {
                    if let Some(rp) = find_close(chars, rb + 2, ')') {
                        meta.transforms.push(MarkdownTransform {
                            start: i,
                            end: i + 1,
                            action: ConcealAction::Hide,
                        });
                        meta.transforms.push(MarkdownTransform {
                            start: rb,
                            end: rp + 1,
                            action: ConcealAction::Hide,
                        });
                        meta.styles.push(MarkdownStyleRange {
                            start: i + 1,
                            end: rb,
                            bold: false,
                            italic: false,
                            underline: true,
                            strikethrough: false,
                            color: Some(LINK_COLOR),
                        });
                        i = rp + 1;
                        continue;
                    }
                }
            }
        }

        i += 1;
    }
}

/// CommonMark "word char" for emphasis flanking purposes — alnum or
/// `_`. `None` (off the line edge) is treated as non-word so a marker
/// at the start / end of a line behaves like it has whitespace
/// flanking it.
fn is_word(c: Option<char>) -> bool {
    matches!(c, Some(ch) if ch.is_alphanumeric() || ch == '_')
}

fn is_ws(c: Option<char>) -> bool {
    matches!(c, Some(ch) if ch.is_whitespace())
}

fn find_close(chars: &[char], start: usize, target: char) -> Option<usize> {
    let mut i = start;
    while i < chars.len() {
        if chars[i] == target {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_double_close(chars: &[char], start: usize, target: char) -> Option<usize> {
    let mut i = start;
    while i + 1 < chars.len() {
        if chars[i] == target && chars[i + 1] == target {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Visual column at which buffer column `target_col` would render under
/// `meta`. Hidden ranges contribute zero width; replaced ranges
/// contribute their replacement glyph's char count. A target col that
/// lands *inside* a transform clamps to the start of the rendered
/// transform (cursor sits at the marker boundary).
pub fn visual_col_for_buffer_col(
    chars: &[char],
    meta: &MarkdownLineMeta,
    target_col: usize,
    tab_width: usize,
) -> usize {
    let mut visual = 0usize;
    let mut col = 0usize;
    let mut t_idx = 0usize;
    while col < target_col && col < chars.len() {
        while t_idx < meta.transforms.len() && meta.transforms[t_idx].end <= col {
            t_idx += 1;
        }
        if let Some(t) = meta.transforms.get(t_idx) {
            if col == t.start {
                let span_end = t.end.min(chars.len());
                if target_col <= span_end {
                    return visual;
                }
                visual += match &t.action {
                    ConcealAction::Hide => 0,
                    ConcealAction::Replace { glyph, .. } => glyph.chars().count(),
                };
                col = span_end;
                t_idx += 1;
                continue;
            }
        }
        let c = chars[col];
        let w = if c == '\t' { tab_width } else { 1 };
        visual += w;
        col += 1;
    }
    visual
}

/// Inverse of `visual_col_for_buffer_col` — given a target visual col,
/// return the buffer col it maps to. Used for click-to-position.
pub fn buffer_col_for_visual_col(
    chars: &[char],
    meta: &MarkdownLineMeta,
    target_visual: usize,
    tab_width: usize,
) -> usize {
    let mut visual = 0usize;
    let mut col = 0usize;
    let mut t_idx = 0usize;
    while col < chars.len() {
        while t_idx < meta.transforms.len() && meta.transforms[t_idx].end <= col {
            t_idx += 1;
        }
        if let Some(t) = meta.transforms.get(t_idx) {
            if col == t.start {
                let span_end = t.end.min(chars.len());
                let w = match &t.action {
                    ConcealAction::Hide => 0,
                    ConcealAction::Replace { glyph, .. } => glyph.chars().count(),
                };
                if visual + w > target_visual {
                    // Click landed inside the rendered replacement — anchor
                    // at the source-marker start so the cursor sits at the
                    // boundary (visible feedback that you're "on" the marker).
                    return col;
                }
                visual += w;
                col = span_end;
                t_idx += 1;
                continue;
            }
        }
        let c = chars[col];
        let w = if c == '\t' { tab_width } else { 1 };
        if visual + w > target_visual {
            return col;
        }
        visual += w;
        col += 1;
    }
    col
}

/// Look up the style range covering `col` (if any). Used by the
/// renderer to fold bold/italic/underline/colour overrides on top of
/// the syntax-highlight pass.
pub fn style_at(meta: &MarkdownLineMeta, col: usize) -> Option<&MarkdownStyleRange> {
    meta.styles.iter().find(|s| col >= s.start && col < s.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols_hidden(meta: &MarkdownLineMeta) -> Vec<usize> {
        let mut out = Vec::new();
        for t in &meta.transforms {
            if matches!(t.action, ConcealAction::Hide) {
                for c in t.start..t.end {
                    out.push(c);
                }
            }
        }
        out
    }

    #[test]
    fn heading_hides_hashes_and_styles_body() {
        let m = compute_line_meta("# Hello world");
        assert_eq!(cols_hidden(&m), vec![0, 1]); // `# ` hidden
        let s = style_at(&m, 2).unwrap();
        assert!(s.bold);
        assert!(s.color.is_some());
    }

    #[test]
    fn h3_uses_a_different_colour_than_h1() {
        let h1 = compute_line_meta("# A");
        let h3 = compute_line_meta("### A");
        let c1 = style_at(&h1, 2).unwrap().color.unwrap();
        let c3 = style_at(&h3, 4).unwrap().color.unwrap();
        assert_ne!(c1, c3);
    }

    #[test]
    fn bold_hides_double_stars() {
        let m = compute_line_meta("a **bold** b");
        // `**` at cols 2,3 and 8,9
        assert_eq!(cols_hidden(&m), vec![2, 3, 8, 9]);
        let s = style_at(&m, 4).unwrap();
        assert!(s.bold);
    }

    #[test]
    fn italic_hides_single_marker() {
        let m = compute_line_meta("a *it* b");
        assert_eq!(cols_hidden(&m), vec![2, 5]);
        let s = style_at(&m, 3).unwrap();
        assert!(s.italic);
    }

    #[test]
    fn italic_does_not_collide_with_bold() {
        let m = compute_line_meta("**bold**");
        // Should be bold, not italic — only the `**` markers hidden.
        assert_eq!(cols_hidden(&m), vec![0, 1, 6, 7]);
        let s = style_at(&m, 2).unwrap();
        assert!(s.bold && !s.italic);
    }

    #[test]
    fn inline_code_hides_backticks() {
        let m = compute_line_meta("see `code` here");
        assert_eq!(cols_hidden(&m), vec![4, 9]);
        let s = style_at(&m, 5).unwrap();
        assert_eq!(s.color, Some(CODE_COLOR));
    }

    #[test]
    fn link_hides_brackets_and_url() {
        // `[text](http://x)` — text 1..5, hide 0, hide 5..16
        let m = compute_line_meta("[text](http://x)");
        let hidden = cols_hidden(&m);
        assert!(hidden.contains(&0));
        for c in 5..16 {
            assert!(hidden.contains(&c), "col {} should be hidden", c);
        }
        let s = style_at(&m, 1).unwrap();
        assert!(s.underline);
        assert_eq!(s.color, Some(LINK_COLOR));
    }

    #[test]
    fn bullet_replaces_marker() {
        let m = compute_line_meta("- item");
        assert_eq!(m.transforms.len(), 1);
        let t = &m.transforms[0];
        assert_eq!((t.start, t.end), (0, 1));
        assert!(matches!(t.action, ConcealAction::Replace { glyph: "•", .. }));
    }

    #[test]
    fn blockquote_replaces_marker_and_styles_body() {
        let m = compute_line_meta("> quoted");
        assert!(matches!(
            m.transforms[0].action,
            ConcealAction::Replace { glyph: "▎ ", .. }
        ));
        let s = style_at(&m, 2).unwrap();
        assert_eq!(s.color, Some(QUOTE_COLOR));
        assert!(s.italic);
    }

    #[test]
    fn visual_col_skips_hidden_markers() {
        // `**bold**` — buffer col 2 ('b') should map to visual col 0
        // because the leading `**` is hidden.
        let chars: Vec<char> = "**bold**".chars().collect();
        let m = compute_line_meta("**bold**");
        let v = visual_col_for_buffer_col(&chars, &m, 2, 4);
        assert_eq!(v, 0);
        // Buffer col 6 (right after `d`, where the trailing `**` starts)
        // maps to visual col 4.
        let v = visual_col_for_buffer_col(&chars, &m, 6, 4);
        assert_eq!(v, 4);
    }

    #[test]
    fn visual_col_inside_hidden_clamps_to_marker_start() {
        // Cursor on the second `*` of `**bold**` — visually still at the
        // start of `b`.
        let chars: Vec<char> = "**bold**".chars().collect();
        let m = compute_line_meta("**bold**");
        let v = visual_col_for_buffer_col(&chars, &m, 1, 4);
        assert_eq!(v, 0);
    }

    #[test]
    fn buffer_col_inverse_of_visual_col() {
        // `**bold**` rendered as `bold` (length 4) — visible cols are
        // [0..4] and map to source cols [2..6].
        let chars: Vec<char> = "**bold**".chars().collect();
        let m = compute_line_meta("**bold**");
        // Visual col 0 lands on the 'b' (source col 2). The hidden `**`
        // has zero width, so visual 0 doesn't fall "inside" the conceal.
        assert_eq!(buffer_col_for_visual_col(&chars, &m, 0, 4), 2);
        // Visual col 1 lands on 'o' (source col 3).
        assert_eq!(buffer_col_for_visual_col(&chars, &m, 1, 4), 3);
        // Past EOL of the rendered text — clamps to chars.len().
        assert_eq!(buffer_col_for_visual_col(&chars, &m, 99, 4), 8);
    }

    #[test]
    fn buffer_col_lands_on_replacement_glyph() {
        // `- item` renders as `• item`. Replacement is one glyph wide,
        // so visual col 0 lands inside the replacement → source col 0.
        let chars: Vec<char> = "- item".chars().collect();
        let m = compute_line_meta("- item");
        assert_eq!(buffer_col_for_visual_col(&chars, &m, 0, 4), 0);
        // Visual col 1 is the space after the bullet (source col 1).
        assert_eq!(buffer_col_for_visual_col(&chars, &m, 1, 4), 1);
        // Visual col 2 is 'i' (source col 2).
        assert_eq!(buffer_col_for_visual_col(&chars, &m, 2, 4), 2);
    }

    #[test]
    fn empty_line_yields_empty_meta() {
        let m = compute_line_meta("");
        assert!(m.transforms.is_empty());
        assert!(m.styles.is_empty());
    }

    #[test]
    fn plain_paragraph_has_no_transforms() {
        let m = compute_line_meta("Just a normal sentence.");
        assert!(m.transforms.is_empty());
        assert!(m.styles.is_empty());
    }

    #[test]
    fn strikethrough_hides_double_tildes() {
        let m = compute_line_meta("a ~~done~~ b");
        assert_eq!(cols_hidden(&m), vec![2, 3, 8, 9]);
        let s = style_at(&m, 4).unwrap();
        assert!(s.strikethrough);
    }

    #[test]
    fn strikethrough_skips_when_opener_before_whitespace() {
        let m = compute_line_meta("a ~~ done ~~ b");
        // Opener `~~` followed by space — should not match.
        assert!(m.transforms.is_empty(), "{:?}", m.transforms);
    }

    #[test]
    fn fence_open_hides_backticks_and_styles_lang() {
        let lines = vec!["```rust".to_string(), "let x = 1;".to_string(), "```".to_string()];
        let metas = compute_buffer_meta(&lines);
        assert_eq!(metas.len(), 3);
        // Opener: hide ```, style "rust" bold-Peach.
        assert_eq!(metas[0].transforms.len(), 1);
        assert_eq!(metas[0].transforms[0].start, 0);
        assert_eq!(metas[0].transforms[0].end, 3);
        let s = &metas[0].styles[0];
        assert_eq!(s.start, 3);
        assert_eq!(s.end, 7);
        assert!(s.bold);
        // Body: no transforms, CodeBlock kind so renderer paints
        // the Mantle background.
        assert_eq!(metas[1].kind, MarkdownLineKind::CodeBlock);
        assert!(metas[1].transforms.is_empty());
        // Closer: CodeBlock with the backticks hidden — renders
        // as a solid dark bg row that visually closes the block.
        assert_eq!(metas[2].kind, MarkdownLineKind::CodeBlock);
        assert_eq!(metas[2].transforms.len(), 1);
        assert!(matches!(metas[2].transforms[0].action, ConcealAction::Hide));
    }

    #[test]
    fn inside_fence_no_emphasis_applied() {
        // `_API_` inside a code block should pass through unchanged.
        let lines = vec![
            "```bash".to_string(),
            "ANTHROPIC_API_KEY=foo".to_string(),
            "```".to_string(),
        ];
        let metas = compute_buffer_meta(&lines);
        assert!(metas[1].transforms.is_empty());
        assert!(metas[1].styles.is_empty());
    }

    #[test]
    fn frontmatter_styles_block_dim() {
        let lines = vec![
            "---".to_string(),
            "title: Foo".to_string(),
            "---".to_string(),
            "# Heading".to_string(),
        ];
        let metas = compute_buffer_meta(&lines);
        // All three frontmatter rows styled dim italic.
        for i in 0..3 {
            assert!(!metas[i].styles.is_empty(), "row {} should be styled", i);
            assert!(metas[i].styles[0].italic);
        }
        // Heading after frontmatter is processed normally.
        assert!(!metas[3].transforms.is_empty());
    }

    #[test]
    fn frontmatter_only_at_top_of_file() {
        // `---` mid-file is not frontmatter; it's an HR.
        let lines = vec![
            "Some prose here.".to_string(),
            "".to_string(),
            "---".to_string(),
            "More prose.".to_string(),
        ];
        let metas = compute_buffer_meta(&lines);
        assert_eq!(metas[2].kind, MarkdownLineKind::HorizontalRule);
    }

    #[test]
    fn setext_h1_promotes_previous_line() {
        let lines = vec!["My Title".to_string(), "========".to_string()];
        let metas = compute_buffer_meta(&lines);
        let s = &metas[0].styles[0];
        assert!(s.bold);
        assert_eq!(metas[1].kind, MarkdownLineKind::Hidden);
    }

    #[test]
    fn setext_h2_with_dashes_promotes_previous_line() {
        let lines = vec!["My Title".to_string(), "--------".to_string()];
        let metas = compute_buffer_meta(&lines);
        assert!(metas[0].styles[0].bold);
        assert_eq!(metas[1].kind, MarkdownLineKind::Hidden);
    }

    #[test]
    fn dashes_after_atx_heading_stays_hr() {
        // `---` under `# Foo` is HR, not setext (heading isn't prose).
        let lines = vec!["# Foo".to_string(), "---".to_string()];
        let metas = compute_buffer_meta(&lines);
        assert_eq!(metas[1].kind, MarkdownLineKind::HorizontalRule);
    }

    #[test]
    fn standalone_dashes_render_as_hr() {
        let lines = vec!["".to_string(), "---".to_string(), "".to_string()];
        let metas = compute_buffer_meta(&lines);
        assert_eq!(metas[1].kind, MarkdownLineKind::HorizontalRule);
    }

    #[test]
    fn intraword_underscore_is_not_italic() {
        // `ANTHROPIC_API_KEY` — `_API_` is flanked by alnum on both
        // sides, must not trigger emphasis.
        let m = compute_line_meta("ANTHROPIC_API_KEY=foo");
        assert!(m.transforms.is_empty(), "{:?}", m.transforms);
        assert!(m.styles.is_empty(), "{:?}", m.styles);
    }

    #[test]
    fn intraword_underscore_bold_does_not_match() {
        let m = compute_line_meta("FOO__BAR__BAZ");
        assert!(m.transforms.is_empty(), "{:?}", m.transforms);
    }

    #[test]
    fn underscore_italic_works_at_word_boundary() {
        // ` _word_ ` — both flanks non-word, should italicise.
        let m = compute_line_meta("a _word_ b");
        let hidden = cols_hidden(&m);
        assert_eq!(hidden, vec![2, 7]);
    }

    #[test]
    fn star_can_open_intraword() {
        // `f*oo*` — `*` allows intraword opening per CommonMark.
        let m = compute_line_meta("f*oo*");
        let hidden = cols_hidden(&m);
        assert_eq!(hidden, vec![1, 4]);
    }

    #[test]
    fn opener_cannot_precede_whitespace() {
        // `a * foo * b` — markers are flanked by whitespace on the
        // wrong side; should not match.
        let m = compute_line_meta("a * foo * b");
        assert!(m.transforms.is_empty(), "{:?}", m.transforms);
    }

    #[test]
    fn bullet_with_inline_bold() {
        // `- **important** thing` — bullet replaced + bold inside body
        let m = compute_line_meta("- **important** thing");
        // Bullet replace
        assert!(m
            .transforms
            .iter()
            .any(|t| t.start == 0 && matches!(t.action, ConcealAction::Replace { .. })));
        // Bold marker hides (cols 2,3 and 13,14)
        let hidden = cols_hidden(&m);
        for c in [2, 3, 13, 14] {
            assert!(hidden.contains(&c), "col {} should be hidden", c);
        }
    }
}
