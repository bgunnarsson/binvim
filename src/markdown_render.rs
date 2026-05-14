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
/// - `Table(kind)` paints the row's `replacement` string (the
///   pre-rendered box-drawn line) instead of walking source chars.
///   The kind tag drives the styling (Header bold-Lavender,
///   Separator dim Overlay0, Body normal text).
/// - `HtmlSummary` paints `replacement` as a bold-Peach
///   disclosure title prefixed with `▼ ` — matches how a `<details>`
///   block reads in a browser when expanded. Inline HTML inside the
///   summary is stripped so a doubled-up `<summary><strong>X</strong></summary>`
///   shows once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MarkdownLineKind {
    #[default]
    Default,
    Hidden,
    HorizontalRule,
    CodeBlock,
    Table(TableRowKind),
    HtmlSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableRowKind {
    Header,
    Separator,
    Body,
}

#[derive(Debug, Clone, Default)]
pub struct MarkdownLineMeta {
    pub transforms: Vec<MarkdownTransform>,
    pub styles: Vec<MarkdownStyleRange>,
    pub kind: MarkdownLineKind,
    /// Pre-rendered text that the renderer paints verbatim instead
    /// of walking source chars. Used by tables (where the rendered
    /// row width differs from the source) — `kind` decides the
    /// styling that wraps it.
    pub replacement: Option<String>,
}

// Catppuccin Mocha
const CODE_COLOR: Color = Color::Rgb { r: 0xa6, g: 0xe3, b: 0xa1 };    // Green
const LINK_COLOR: Color = Color::Rgb { r: 0x89, g: 0xb4, b: 0xfa };    // Blue
const BULLET_COLOR: Color = Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 };  // Peach
const QUOTE_COLOR: Color = Color::Rgb { r: 0x6c, g: 0x70, b: 0x86 };   // Overlay0

/// Each heading level gets its own Catppuccin accent so an outline
/// reads as a six-tone hierarchy at a glance. Stepping warm → cool
/// through the palette: H1 Red (loudest), H2 Peach, H3 Yellow,
/// H4 Green, H5 Sky, H6 Mauve. Anything deeper than H6 isn't
/// valid markdown but we land on Mauve so it still renders.
fn heading_color(level: usize) -> Color {
    match level {
        1 => Color::Rgb { r: 0xf3, g: 0x8b, b: 0xa8 }, // Red
        2 => Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 }, // Peach
        3 => Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf }, // Yellow
        4 => Color::Rgb { r: 0xa6, g: 0xe3, b: 0xa1 }, // Green
        5 => Color::Rgb { r: 0x89, g: 0xdc, b: 0xeb }, // Sky
        _ => Color::Rgb { r: 0xcb, g: 0xa6, b: 0xf7 }, // Mauve
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
/// (setext headings, code-fence interior, top-of-file frontmatter,
/// tables) are only decidable with cross-line context. Returns one
/// `MarkdownLineMeta` per input line.
pub fn compute_buffer_meta(lines: &[String]) -> Vec<MarkdownLineMeta> {
    let mut out: Vec<MarkdownLineMeta> = Vec::with_capacity(lines.len());
    let mut fence: Option<char> = None;
    let mut in_frontmatter = false;
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        // Top-of-file YAML / TOML frontmatter — opening `---` (or
        // `+++` for TOML) on line 0, runs until the matching closer.
        // Render every frontmatter row in muted Overlay0 italic so it
        // reads as metadata, not content.
        if i == 0 {
            let t = line.trim();
            if t == "---" || t == "+++" {
                in_frontmatter = true;
                out.push(frontmatter_meta(line));
                i += 1;
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
            i += 1;
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
                    i += 1;
                    continue;
                }
                None => {
                    fence = Some(ch);
                    out.push(fence_open_meta(line, leading, ch));
                    i += 1;
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
            i += 1;
            continue;
        }

        // GFM tables — header row + separator + body rows. Detection
        // requires looking ahead one line (separator pattern), so it
        // sits before the setext / HR checks (a `|---|---|` line is
        // a separator, not an HR). On match we emit one Table row
        // per source line and skip past the whole block in one jump.
        if let Some((rendered_rows, end)) = try_render_table(lines, i) {
            for (row_offset, rendered) in rendered_rows.into_iter().enumerate() {
                let kind = if row_offset == 0 {
                    TableRowKind::Header
                } else if row_offset == 1 {
                    TableRowKind::Separator
                } else {
                    TableRowKind::Body
                };
                out.push(MarkdownLineMeta {
                    kind: MarkdownLineKind::Table(kind),
                    replacement: Some(rendered),
                    ..MarkdownLineMeta::default()
                });
            }
            i = end;
            continue;
        }

        // HTML disclosure block — `<details>` / `</details>` lines
        // are chrome (hidden); `<summary>X</summary>` becomes a
        // bold-Peach disclosure title with a `▼` prefix. Cheap to
        // check before the heavier setext / HR scanners.
        let trimmed_full = line.trim();
        if is_html_open_tag(trimmed_full, "details") || trimmed_full == "</details>" {
            out.push(MarkdownLineMeta {
                kind: MarkdownLineKind::Hidden,
                ..MarkdownLineMeta::default()
            });
            i += 1;
            continue;
        }
        if let Some(title) = extract_summary_title(line) {
            out.push(MarkdownLineMeta {
                kind: MarkdownLineKind::HtmlSummary,
                replacement: Some(format!("▼ {}", title)),
                ..MarkdownLineMeta::default()
            });
            i += 1;
            continue;
        }
        // Standalone HTML comment line — `<!-- foo -->` collapses
        // to a blank row so meta-comments don't dilute prose
        // hierarchy.
        if trimmed_full.starts_with("<!--") && trimmed_full.ends_with("-->") {
            out.push(MarkdownLineMeta {
                kind: MarkdownLineKind::Hidden,
                ..MarkdownLineMeta::default()
            });
            i += 1;
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
                i += 1;
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
            i += 1;
            continue;
        }

        out.push(compute_line_meta(line));
        i += 1;
    }
    out
}

/// Attempt to parse a GFM-style table starting at `lines[start]`.
/// On success returns `(rendered_rows, end_exclusive)` where each
/// rendered row is the box-drawn replacement string the renderer
/// should paint in place of the source. Returns `None` when the
/// pair (header + separator) at `start` doesn't look like a table.
fn try_render_table(lines: &[String], start: usize) -> Option<(Vec<String>, usize)> {
    if start + 1 >= lines.len() {
        return None;
    }
    let header_cells = parse_pipe_row(&lines[start])?;
    let sep_cells = parse_separator(&lines[start + 1])?;
    if header_cells.len() != sep_cells {
        return None;
    }
    let n_cols = header_cells.len();
    let mut body_rows: Vec<Vec<String>> = Vec::new();
    let mut end = start + 2;
    while end < lines.len() {
        if let Some(mut row) = parse_pipe_row(&lines[end]) {
            row.resize(n_cols, String::new());
            body_rows.push(row);
            end += 1;
        } else {
            break;
        }
    }
    let mut col_widths: Vec<usize> = vec![0; n_cols];
    for (c, cell) in header_cells.iter().enumerate() {
        col_widths[c] = col_widths[c].max(cell.chars().count());
    }
    for row in &body_rows {
        for (c, cell) in row.iter().enumerate() {
            col_widths[c] = col_widths[c].max(cell.chars().count());
        }
    }
    let mut rendered: Vec<String> = Vec::with_capacity(2 + body_rows.len());
    rendered.push(render_pipe_row(&header_cells, &col_widths));
    rendered.push(render_separator(&col_widths));
    for row in &body_rows {
        rendered.push(render_pipe_row(row, &col_widths));
    }
    Some((rendered, end))
}

/// Parse a single `| a | b | c |` row into its cells (already
/// trimmed). Requires both leading and trailing pipes to keep
/// detection conservative — `key | value` style without outer
/// pipes is more likely to be prose with a stray bar.
fn parse_pipe_row(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') || !trimmed.ends_with('|') || trimmed.len() < 2 {
        return None;
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    Some(inner.split('|').map(|s| s.trim().to_string()).collect())
}

/// Parse a separator row (`|---|---|---|`, optionally with leading
/// or trailing colons for alignment). Returns the column count when
/// it matches; alignment markers are accepted but currently not
/// used (everything renders left-aligned).
fn parse_separator(line: &str) -> Option<usize> {
    let cells = parse_pipe_row(line)?;
    if cells.is_empty() {
        return None;
    }
    for cell in &cells {
        let chs: Vec<char> = cell.chars().collect();
        if chs.is_empty() {
            return None;
        }
        let mut i = 0;
        if chs[i] == ':' {
            i += 1;
        }
        let dash_start = i;
        while i < chs.len() && chs[i] == '-' {
            i += 1;
        }
        if i == dash_start {
            return None;
        }
        if i < chs.len() && chs[i] == ':' {
            i += 1;
        }
        if i != chs.len() {
            return None;
        }
    }
    Some(cells.len())
}

fn render_pipe_row(cells: &[String], col_widths: &[usize]) -> String {
    let mut s = String::with_capacity(64);
    s.push('│');
    for (c, cell) in cells.iter().enumerate() {
        let w = col_widths.get(c).copied().unwrap_or(0);
        let pad = w.saturating_sub(cell.chars().count());
        s.push(' ');
        s.push_str(cell);
        for _ in 0..pad {
            s.push(' ');
        }
        s.push(' ');
        s.push('│');
    }
    s
}

/// Match `<tag` / `<tag …>` / `<tag>` opener (case-insensitive on
/// the tag name, lenient on attributes). Used by block-level HTML
/// detection — must NOT match if the same line also contains the
/// closing tag (single-line `<details><summary>X</summary></details>`
/// shouldn't blow away the line as chrome).
fn is_html_open_tag(line: &str, tag: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let needle_lt = format!("<{}", tag.to_ascii_lowercase());
    let needle_close = format!("</{}>", tag.to_ascii_lowercase());
    if !lower.starts_with(&needle_lt) || !lower.ends_with('>') {
        return false;
    }
    if lower.contains(&needle_close) {
        return false;
    }
    // Char immediately after `<tag` must be `>` or whitespace —
    // otherwise it's a longer tag like `<details-list>`.
    let after = lower.as_bytes().get(needle_lt.len()).copied();
    matches!(after, Some(b'>') | Some(b' ') | Some(b'\t'))
}

/// Pull the visible title out of a `<summary>…</summary>` line. The
/// returned string has any inline HTML tags inside the summary
/// stripped — duplicated emphasis like `<summary><strong>X</strong></summary>`
/// reads as `▼ X` (not `▼ <strong>X</strong>`).
fn extract_summary_title(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("<summary") || !lower.ends_with("</summary>") {
        return None;
    }
    // Skip `<summary…>` — find first `>` after `<summary`.
    let open_close = trimmed.find('>')?;
    let body_start = open_close + 1;
    let close_idx = lower.rfind("</summary>")?;
    if close_idx < body_start {
        return None;
    }
    let inner = &trimmed[body_start..close_idx];
    let stripped = strip_html_tags(inner);
    let collapsed = stripped.trim().to_string();
    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed)
    }
}

/// Remove every `<…>` span from `s`. Cheap regex-free pass — fine
/// for the small inline-HTML strings we feed it.
fn strip_html_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// True when the chars at `chars[i..]` start with the literal
/// `pattern` (char-by-char, case-insensitive on ASCII).
fn matches_at(chars: &[char], i: usize, pattern: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    if i + pat.len() > chars.len() {
        return false;
    }
    for (k, p) in pat.iter().enumerate() {
        if !chars[i + k].eq_ignore_ascii_case(p) {
            return false;
        }
    }
    true
}

/// Find the next occurrence of `pattern` in `chars[start..]`.
/// Case-insensitive on ASCII. Returns the absolute char index.
fn find_pattern(chars: &[char], start: usize, pattern: &str) -> Option<usize> {
    let pat: Vec<char> = pattern.chars().collect();
    if pat.is_empty() || pat.len() > chars.len() {
        return None;
    }
    let mut k = start;
    while k + pat.len() <= chars.len() {
        let mut ok = true;
        for (j, p) in pat.iter().enumerate() {
            if !chars[k + j].eq_ignore_ascii_case(p) {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(k);
        }
        k += 1;
    }
    None
}

fn render_separator(col_widths: &[usize]) -> String {
    let mut s = String::with_capacity(64);
    s.push('├');
    for (c, w) in col_widths.iter().enumerate() {
        for _ in 0..(w + 2) {
            s.push('─');
        }
        if c + 1 < col_widths.len() {
            s.push('┼');
        }
    }
    s.push('┤');
    s
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
    'scan: while i < n {
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

        // Inline HTML — small set of tags people commonly mix into
        // markdown. Each pair hides the open + close tags and folds
        // the appropriate style onto the inner span. Self-closing
        // `<br>` variants and HTML comments collapse entirely.
        if c == '<' {
            // <!-- comment --> — hide the whole span.
            if matches_at(chars, i, "<!--") {
                if let Some(end) = find_pattern(chars, i + 4, "-->") {
                    let close_end = end + 3;
                    meta.transforms.push(MarkdownTransform {
                        start: i,
                        end: close_end,
                        action: ConcealAction::Hide,
                    });
                    i = close_end;
                    continue;
                }
            }
            // <br>, <br/>, <br /> — self-closing line break.
            for variant in &["<br />", "<br/>", "<br>"] {
                if matches_at(chars, i, variant) {
                    let len = variant.chars().count();
                    meta.transforms.push(MarkdownTransform {
                        start: i,
                        end: i + len,
                        action: ConcealAction::Hide,
                    });
                    i += len;
                    continue 'scan;
                }
            }
            // Paired tags — `<strong>`, `<em>`, `<i>`, `<u>`, `<code>`.
            // Each entry: (open, close, bold, italic, underline, color).
            // First match wins.
            const PAIRS: &[(&str, &str, bool, bool, bool, Option<Color>)] = &[
                ("<strong>", "</strong>", true, false, false, None),
                ("<b>", "</b>", true, false, false, None),
                ("<em>", "</em>", false, true, false, None),
                ("<i>", "</i>", false, true, false, None),
                ("<u>", "</u>", false, false, true, None),
                ("<code>", "</code>", false, false, false, Some(CODE_COLOR)),
            ];
            let mut matched = false;
            for (open, close, bold, italic, underline, color) in PAIRS {
                if matches_at(chars, i, open) {
                    let open_len = open.chars().count();
                    if let Some(close_start) =
                        find_pattern(chars, i + open_len, close)
                    {
                        let close_len = close.chars().count();
                        meta.transforms.push(MarkdownTransform {
                            start: i,
                            end: i + open_len,
                            action: ConcealAction::Hide,
                        });
                        meta.transforms.push(MarkdownTransform {
                            start: close_start,
                            end: close_start + close_len,
                            action: ConcealAction::Hide,
                        });
                        meta.styles.push(MarkdownStyleRange {
                            start: i + open_len,
                            end: close_start,
                            bold: *bold,
                            italic: *italic,
                            underline: *underline,
                            strikethrough: false,
                            color: *color,
                        });
                        i = close_start + close_len;
                        matched = true;
                        break;
                    }
                }
            }
            if matched {
                continue;
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
    fn each_heading_level_has_a_distinct_colour() {
        let inputs = ["# A", "## A", "### A", "#### A", "##### A", "###### A"];
        let mut colors = Vec::new();
        for (i, src) in inputs.iter().enumerate() {
            let m = compute_line_meta(src);
            let body_col = i + 2; // hash count + space
            let c = style_at(&m, body_col)
                .unwrap_or_else(|| panic!("H{} missing style", i + 1))
                .color
                .unwrap_or_else(|| panic!("H{} missing color", i + 1));
            colors.push(c);
        }
        for i in 0..colors.len() {
            for j in (i + 1)..colors.len() {
                assert_ne!(
                    colors[i],
                    colors[j],
                    "H{} and H{} share a colour",
                    i + 1,
                    j + 1
                );
            }
        }
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
    fn details_open_close_lines_collapse() {
        let lines = vec![
            "<details>".to_string(),
            "<summary>Setup</summary>".to_string(),
            "".to_string(),
            "Body".to_string(),
            "</details>".to_string(),
        ];
        let metas = compute_buffer_meta(&lines);
        assert_eq!(metas[0].kind, MarkdownLineKind::Hidden);
        assert_eq!(metas[1].kind, MarkdownLineKind::HtmlSummary);
        assert_eq!(metas[1].replacement.as_deref(), Some("▼ Setup"));
        // Body line is plain markdown.
        assert!(matches!(metas[3].kind, MarkdownLineKind::Default));
        assert_eq!(metas[4].kind, MarkdownLineKind::Hidden);
    }

    #[test]
    fn details_with_attributes_still_collapses() {
        let lines = vec!["<details open>".to_string(), "</details>".to_string()];
        let metas = compute_buffer_meta(&lines);
        assert_eq!(metas[0].kind, MarkdownLineKind::Hidden);
        assert_eq!(metas[1].kind, MarkdownLineKind::Hidden);
    }

    #[test]
    fn summary_strips_inline_html() {
        let lines = vec![
            "<summary><strong>Setup the frontend</strong></summary>".to_string(),
        ];
        let metas = compute_buffer_meta(&lines);
        assert_eq!(metas[0].replacement.as_deref(), Some("▼ Setup the frontend"));
    }

    #[test]
    fn html_comment_line_collapses() {
        let lines = vec!["<!-- TODO: revise -->".to_string()];
        let metas = compute_buffer_meta(&lines);
        assert_eq!(metas[0].kind, MarkdownLineKind::Hidden);
    }

    #[test]
    fn inline_strong_em_u_hide_tags() {
        let m = compute_line_meta("a <strong>b</strong> c");
        // `<strong>` at cols 2..10 hidden; `</strong>` at cols 11..20 hidden.
        let hidden = cols_hidden(&m);
        for c in [2, 3, 4, 5, 6, 7, 8, 9] {
            assert!(hidden.contains(&c), "open col {} hidden", c);
        }
        for c in 11..20 {
            assert!(hidden.contains(&c), "close col {} hidden", c);
        }
        let s = style_at(&m, 10).unwrap();
        assert!(s.bold);
    }

    #[test]
    fn inline_em_styles_italic() {
        let m = compute_line_meta("<em>x</em>");
        let s = style_at(&m, 4).unwrap();
        assert!(s.italic);
    }

    #[test]
    fn inline_u_styles_underline() {
        let m = compute_line_meta("<u>x</u>");
        let s = style_at(&m, 3).unwrap();
        assert!(s.underline);
    }

    #[test]
    fn inline_br_collapses() {
        let m = compute_line_meta("a<br>b");
        // `<br>` at cols 1..5 hidden.
        let hidden = cols_hidden(&m);
        for c in 1..5 {
            assert!(hidden.contains(&c), "col {}", c);
        }
    }

    #[test]
    fn inline_br_self_closing_variants() {
        for src in &["a<br/>b", "a<br />b"] {
            let m = compute_line_meta(src);
            assert!(!m.transforms.is_empty(), "no transforms for {:?}", src);
        }
    }

    #[test]
    fn inline_html_comment_collapses() {
        let m = compute_line_meta("a <!-- hi --> b");
        let hidden = cols_hidden(&m);
        for c in 2..13 {
            assert!(hidden.contains(&c), "col {}", c);
        }
    }

    #[test]
    fn table_renders_box_drawn_rows() {
        let lines = vec![
            "| A | B |".to_string(),
            "|---|---|".to_string(),
            "| 1 | 22 |".to_string(),
        ];
        let metas = compute_buffer_meta(&lines);
        assert_eq!(metas.len(), 3);
        // Header
        match metas[0].kind {
            MarkdownLineKind::Table(TableRowKind::Header) => {}
            other => panic!("row 0 kind: {:?}", other),
        }
        // Column widths come from widest cell per column: col 0 = "A"
        // / "1" / "" → max 1; col 1 = "B" / "22" → max 2. Each cell
        // is bracketed with one space of padding either side, so the
        // header reads `│ A │ B  │`.
        assert_eq!(metas[0].replacement.as_deref(), Some("│ A │ B  │"));
        // Separator widths match: 1+2 = 3 dashes col0, 2+2 = 4 dashes col1.
        match metas[1].kind {
            MarkdownLineKind::Table(TableRowKind::Separator) => {}
            other => panic!("row 1 kind: {:?}", other),
        }
        assert_eq!(metas[1].replacement.as_deref(), Some("├───┼────┤"));
        // Body
        match metas[2].kind {
            MarkdownLineKind::Table(TableRowKind::Body) => {}
            other => panic!("row 2 kind: {:?}", other),
        }
        assert_eq!(metas[2].replacement.as_deref(), Some("│ 1 │ 22 │"));
    }

    #[test]
    fn table_alignment_markers_accepted() {
        let lines = vec![
            "| L | C | R |".to_string(),
            "|:---|:---:|---:|".to_string(),
            "| 1 | 2 | 3 |".to_string(),
        ];
        let metas = compute_buffer_meta(&lines);
        // Should be detected as a table (alignment markers parsed).
        match metas[0].kind {
            MarkdownLineKind::Table(_) => {}
            other => panic!("row 0 kind: {:?}", other),
        }
    }

    #[test]
    fn non_table_pipe_lines_stay_default() {
        // Just one row with pipes, no separator → not a table.
        let lines = vec!["| not a table |".to_string()];
        let metas = compute_buffer_meta(&lines);
        assert!(matches!(metas[0].kind, MarkdownLineKind::Default));
    }

    #[test]
    fn header_separator_col_count_must_match() {
        let lines = vec![
            "| A | B | C |".to_string(),
            "|---|---|".to_string(), // 2 cols, mismatch
            "| 1 | 2 | 3 |".to_string(),
        ];
        let metas = compute_buffer_meta(&lines);
        // Falls through to default classification — first row is a
        // valid prose line with pipes.
        assert!(matches!(metas[0].kind, MarkdownLineKind::Default));
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
