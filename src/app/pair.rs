//! Bracket-matching, HTML tag matching, and auto-pair helpers. Used by
//! the editing path, the matched-pair highlighter, and the auto-close
//! machinery on `>` / quote characters.

use crate::buffer::Buffer;

pub fn is_bracket(c: char) -> bool {
    matches!(c, '(' | ')' | '[' | ']' | '{' | '}')
}

/// `(open, close, forward_search)` — `forward_search=true` means the cursor
/// is on the opener and we walk forward to find the closer.
pub fn bracket_pair(c: char) -> (char, char, bool) {
    match c {
        '(' => ('(', ')', true),
        '[' => ('[', ']', true),
        '{' => ('{', '}', true),
        ')' => ('(', ')', false),
        ']' => ('[', ']', false),
        '}' => ('{', '}', false),
        _ => (c, c, true),
    }
}

pub fn find_match_close(buf: &Buffer, open_idx: usize, open: char, close: char) -> Option<usize> {
    let total = buf.total_chars();
    let mut depth = 1usize;
    let mut i = open_idx + 1;
    while i < total {
        let c = buf.rope.char(i);
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

pub fn find_match_open(buf: &Buffer, close_idx: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 1usize;
    let mut i = close_idx;
    while i > 0 {
        i -= 1;
        let c = buf.rope.char(i);
        if c == close {
            depth += 1;
        } else if c == open {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Map a Vim-surround pair-id char to its open/close strings. `b`/`B`
/// match Vim's shorthand for parens/braces.
pub fn surround_open_close(ch: char) -> (&'static str, &'static str) {
    match ch {
        '(' | ')' | 'b' => ("(", ")"),
        '[' | ']' => ("[", "]"),
        '{' | '}' | 'B' => ("{", "}"),
        '<' | '>' => ("<", ">"),
        '"' => ("\"", "\""),
        '\'' => ("'", "'"),
        '`' => ("`", "`"),
        _ => (" ", " "),
    }
}

pub fn is_paired_bracket(ch: char) -> bool {
    matches!(ch, '(' | ')' | 'b' | '[' | ']' | '{' | '}' | 'B' | '<' | '>')
}

/// Map an opening pair character to its closing counterpart, or `None` for
/// chars that don't auto-pair.
pub fn open_pair_for(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '<' => Some('>'),
        '\'' => Some('\''),
        '"' => Some('"'),
        '`' => Some('`'),
        _ => None,
    }
}

pub fn is_close_char(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '>' | '\'' | '"' | '`')
}

/// Decide whether to auto-pair when typing `c` at `(line, col)`. Quotes/backticks
/// skip pairing when adjacent to identifier-class characters (so `don't` and
/// trailing apostrophes don't pair surprisingly). `<` skips pairing when both
/// sides are whitespace (so `a < b` comparisons don't sprout a stray `>`).
/// Brackets always pair.
pub fn should_auto_pair(c: char, buffer: &Buffer, line: usize, col: usize) -> bool {
    let prev = if col > 0 { buffer.char_at(line, col - 1) } else { None };
    let next = buffer.char_at(line, col);
    match c {
        '\'' | '"' | '`' => {
            let is_word = |c: char| c.is_alphanumeric() || c == '_';
            !prev.map(is_word).unwrap_or(false) && !next.map(is_word).unwrap_or(false)
        }
        '<' => {
            let is_ws = |ch: char| ch.is_whitespace();
            let prev_ws = prev.map(is_ws).unwrap_or(true);
            let next_ws = next.map(is_ws).unwrap_or(true);
            !(prev_ws && next_ws)
        }
        _ => true,
    }
}

/// Find the matching pair for an HTML tag the cursor sits inside. Returns
/// `(open_range, close_range)` where each range is `(start_char_idx,
/// end_char_idx)` covering the full `<…>` of the open and close tag.
///
/// Bails on:
///   - cursor not inside a `<…>` span,
///   - self-closing tag (`<br/>`),
///   - void HTML elements,
///   - declarations / comments / processing instructions (`<!`, `<?`),
///   - unmatched / malformed input,
///   - tag name that contains chars we don't accept.
pub fn html_tag_pair_at(
    buf: &Buffer,
    line: usize,
    col: usize,
) -> Option<((usize, usize), (usize, usize))> {
    let total = buf.total_chars();
    let here = buf.pos_to_char(line, col).min(total);
    let info = enclosing_tag(buf, here)?;
    if info.kind == TagKind::Other {
        return None;
    }
    if is_void_html_element(&info.name) {
        return None;
    }
    let pair = match info.kind {
        TagKind::Open => find_close_tag(buf, info.range.1, &info.name)?,
        TagKind::Close => find_open_tag(buf, info.range.0, &info.name)?,
        TagKind::Other => return None,
    };
    let (open_range, close_range) = match info.kind {
        TagKind::Open => (info.range, pair),
        TagKind::Close => (pair, info.range),
        TagKind::Other => unreachable!(),
    };
    Some((open_range, close_range))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TagKind {
    Open,
    Close,
    Other,
}

struct TagInfo {
    range: (usize, usize),
    name: String,
    kind: TagKind,
}

/// Find the `<…>` span (if any) that contains `here`, parse the tag name
/// and direction, and return all of it. The cursor can be anywhere inside
/// the angle brackets (inclusive).
fn enclosing_tag(buf: &Buffer, here: usize) -> Option<TagInfo> {
    let total = buf.total_chars();
    if total == 0 {
        return None;
    }
    // Walk back to a `<`, bailing if we hit a `>` (we're outside any tag)
    // or a newline (don't cross lines for the simple matcher).
    let here = here.min(total.saturating_sub(1));
    let mut start = here;
    loop {
        let c = buf.rope.char(start);
        if c == '<' {
            break;
        }
        if c == '>' || c == '\n' {
            return None;
        }
        if start == 0 {
            return None;
        }
        start -= 1;
    }
    // Walk forward to the matching `>` on the same line.
    let mut end = start;
    while end < total {
        let c = buf.rope.char(end);
        if c == '>' {
            break;
        }
        if c == '\n' {
            return None;
        }
        end += 1;
    }
    if end >= total || buf.rope.char(end) != '>' {
        return None;
    }
    // Self-closing — char before `>` is `/`.
    if end > start && buf.rope.char(end - 1) == '/' {
        return Some(TagInfo {
            range: (start, end + 1),
            name: String::new(),
            kind: TagKind::Other,
        });
    }
    let inner: String = buf.rope.slice((start + 1)..end).to_string();
    let trimmed = inner.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let first = trimmed.chars().next().unwrap();
    let kind = match first {
        '!' | '?' => TagKind::Other,
        '/' => TagKind::Close,
        c if c.is_alphabetic() || c == '_' => TagKind::Open,
        _ => TagKind::Other,
    };
    let after_slash = if matches!(kind, TagKind::Close) {
        &trimmed[1..]
    } else {
        trimmed
    };
    let name: String = after_slash
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
        .collect();
    if name.is_empty() {
        return Some(TagInfo {
            range: (start, end + 1),
            name,
            kind: TagKind::Other,
        });
    }
    Some(TagInfo {
        range: (start, end + 1),
        name,
        kind,
    })
}

/// Walk forward from `start` to find the matching `</name>` for an open
/// tag, accounting for nested same-name openers.
fn find_close_tag(buf: &Buffer, start: usize, name: &str) -> Option<(usize, usize)> {
    let total = buf.total_chars();
    let mut depth = 1usize;
    let mut i = start;
    while i < total {
        if buf.rope.char(i) != '<' {
            i += 1;
            continue;
        }
        let info = enclosing_tag(buf, i)?;
        match info.kind {
            TagKind::Open if info.name == name => depth += 1,
            TagKind::Close if info.name == name => {
                depth -= 1;
                if depth == 0 {
                    return Some(info.range);
                }
            }
            _ => {}
        }
        // Advance past this tag.
        i = info.range.1.max(i + 1);
    }
    None
}

/// Walk backward from `end` to find the matching `<name…>` for a close
/// tag, accounting for nested same-name closers.
fn find_open_tag(buf: &Buffer, end: usize, name: &str) -> Option<(usize, usize)> {
    let mut depth = 1usize;
    let mut i = end;
    while i > 0 {
        i -= 1;
        if buf.rope.char(i) != '<' {
            continue;
        }
        let Some(info) = enclosing_tag(buf, i) else {
            continue;
        };
        match info.kind {
            TagKind::Close if info.name == name => depth += 1,
            TagKind::Open if info.name == name => {
                depth -= 1;
                if depth == 0 {
                    return Some(info.range);
                }
            }
            _ => {}
        }
        // No advance past a nested `<` is needed — we already moved one step
        // back per iteration and the inner matches use their own ranges.
    }
    None
}

/// True when this buffer is the kind of file where `<div>` should auto-close
/// to `<div></div>`. Markdown is in here because GitHub-flavoured markdown
/// embeds raw HTML; XML follows the same tag-pair rules; framework formats
/// (jsx/tsx/vue/svelte/astro) are HTML-shaped at the markup layer.
pub fn is_html_like_buffer(buffer: &Buffer) -> bool {
    let Some(ext) = buffer
        .path
        .as_ref()
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
    else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "html"
            | "htm"
            | "xhtml"
            | "xml"
            | "cshtml"
            | "razor"
            | "jsx"
            | "tsx"
            | "vue"
            | "svelte"
            | "astro"
            | "md"
            | "markdown"
    )
}

/// HTML void elements — these never carry a separate closing tag, so the
/// auto-completion must skip them. Comparison is ASCII-case-insensitive
/// because HTML attributes/tag names are case-insensitive.
fn is_void_html_element(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Walk back from the cursor (which sits immediately after a freshly-typed
/// `>`) to find the corresponding `<` and extract the tag name. Returns
/// `None` if the prefix doesn't look like a real opening tag — closing tags
/// (`</…>`), comments (`<!--…>`), declarations (`<!DOCTYPE>`), processing
/// instructions (`<?xml…>`), self-closing tags (`<… />`), JSX fragments
/// (`<>`), and HTML void elements all yield no auto-close.
pub fn detect_open_tag_to_close(buffer: &Buffer, line: usize, col_after: usize) -> Option<String> {
    if col_after == 0 {
        return None;
    }
    // The `>` we just typed sits at col_after - 1. Walk back across the line
    // to find the matching `<`.
    let line_str = buffer.rope.line(line).to_string();
    let chars: Vec<char> = line_str.chars().collect();
    let gt_idx = col_after.checked_sub(1)?;
    if chars.get(gt_idx).copied() != Some('>') {
        return None;
    }
    // Self-closing `… />` — preceding char is `/`.
    if gt_idx > 0 && chars[gt_idx - 1] == '/' {
        return None;
    }

    let mut lt_idx: Option<usize> = None;
    let mut i = gt_idx;
    while i > 0 {
        i -= 1;
        match chars[i] {
            '>' => return None, // unbalanced — earlier `>` between
            '<' => {
                lt_idx = Some(i);
                break;
            }
            _ => {}
        }
    }
    let lt_idx = lt_idx?;
    // Heuristic for TSX/Razor/etc.: if the `<` follows an identifier
    // character (or `.`), this is almost certainly a generic parameter —
    // `Array<string>`, `Foo.Bar<T>`. Don't try to auto-close in that case.
    if lt_idx > 0 {
        let prev = chars[lt_idx - 1];
        if prev.is_alphanumeric() || prev == '_' || prev == '.' {
            return None;
        }
    }
    let inner: String = chars[lt_idx + 1..gt_idx].iter().collect();
    let inner_trimmed = inner.trim_start();
    if inner_trimmed.is_empty() {
        return None;
    }
    let first = inner_trimmed.chars().next().unwrap();
    // Closing tag, declaration, comment, processing instruction.
    if matches!(first, '/' | '!' | '?') {
        return None;
    }
    // Tag name is the leading run of name-class chars.
    let name: String = inner_trimmed
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
        .collect();
    if name.is_empty() {
        return None;
    }
    if !name.chars().next().unwrap().is_alphabetic() {
        return None;
    }
    if is_void_html_element(&name) {
        return None;
    }
    Some(name)
}
