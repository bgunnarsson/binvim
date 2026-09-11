//! Bracket-matching, HTML tag matching, and auto-pair helpers. Used by
//! the editing path, the matched-pair highlighter, and the auto-close
//! machinery on `>` / quote characters.

use crate::buffer::Buffer;
use crate::cursor::Cursor;
use crate::motion::{MotionKind, MotionResult};

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

/// `%`: from the first bracket at or after the cursor on its line to the one
/// that matches it. Inside an HTML tag of an html-like buffer it goes to the
/// `<` of the partner tag instead. Inclusive, so `d%` takes both brackets.
/// `None` when there's nothing on the line to match.
pub fn match_pair_motion(buf: &Buffer, cur: Cursor) -> Option<MotionResult> {
    let tag = if is_html_like_buffer(buf) {
        html_tag_pair_at(buf, cur.line, cur.col)
    } else {
        None
    };
    let target = match tag {
        // Both ranges are tag *names*: `open` sits right after `<`, `close`
        // right after `</`.
        Some((open, close)) => {
            let here = buf.pos_to_char(cur.line, cur.col);
            if here + 2 >= close.0 {
                open.0 - 1
            } else {
                close.0 - 2
            }
        }
        None => {
            let col = (cur.col..buf.line_len(cur.line))
                .find(|c| matches!(buf.char_at(cur.line, *c), Some(ch) if is_bracket(ch)))?;
            let idx = buf.pos_to_char(cur.line, col);
            let (open, close, forward) = bracket_pair(buf.rope.char(idx));
            if forward {
                find_match_close(buf, idx, open, close)?
            } else {
                find_match_open(buf, idx, open, close)?
            }
        }
    };
    let line = buf.rope.char_to_line(target);
    let col = target - buf.rope.line_to_char(line);
    Some(MotionResult {
        target: Cursor {
            line,
            col,
            want_col: col,
        },
        kind: MotionKind::CharInclusive,
    })
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
    matches!(
        ch,
        '(' | ')' | 'b' | '[' | ']' | '{' | '}' | 'B' | '<' | '>'
    )
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
    let prev = if col > 0 {
        buffer.char_at(line, col - 1)
    } else {
        None
    };
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
    let (open_name, close_name) = match info.kind {
        TagKind::Open => (info.name_range, pair),
        TagKind::Close => (pair, info.name_range),
        TagKind::Other => unreachable!(),
    };
    Some((open_name, close_name))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TagKind {
    Open,
    Close,
    Other,
}

struct TagInfo {
    /// Absolute char range of the full `<…>` span.
    range: (usize, usize),
    /// Absolute char range of just the tag *name* inside the span. Used
    /// for the matched-pair highlight so the user sees `main` / `main`
    /// highlighted on the open and close tags rather than the entire
    /// `<main className="…">` / `</main>` runs.
    name_range: (usize, usize),
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
            name_range: (start, start),
            name: String::new(),
            kind: TagKind::Other,
        });
    }
    let inner: String = buf.rope.slice((start + 1)..end).to_string();
    let inner_char_count = inner.chars().count();
    let leading_ws = inner.chars().take_while(|c| c.is_whitespace()).count();
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
    let slash_offset = if matches!(kind, TagKind::Close) { 1 } else { 0 };
    let after_slash = if matches!(kind, TagKind::Close) {
        &trimmed[1..]
    } else {
        trimmed
    };
    let name: String = after_slash
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
        .collect();
    // Name starts at: `<` + leading whitespace inside the span + (1 for `/`
    // on close tags) + (0 for open). End is name's char-count chars later.
    let name_chars = name.chars().count();
    let name_start = start + 1 + leading_ws + slash_offset;
    let name_end = name_start + name_chars;
    let _ = inner_char_count;
    if name.is_empty() {
        return Some(TagInfo {
            range: (start, end + 1),
            name_range: (name_start, name_start),
            name,
            kind: TagKind::Other,
        });
    }
    Some(TagInfo {
        range: (start, end + 1),
        name_range: (name_start, name_end),
        name,
        kind,
    })
}

/// Walk forward from `start` to find the matching `</name>` for an open
/// tag, accounting for nested same-name openers. Returns the *name*
/// range of the matching close tag (not its full `<…>` span).
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
                    return Some(info.name_range);
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
/// tag, accounting for nested same-name closers. Returns the *name*
/// range of the matching open tag (not its full `<…>` span).
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
                    return Some(info.name_range);
                }
            }
            _ => {}
        }
        // No advance past a nested `<` is needed — we already moved one step
        // back per iteration and the inner matches use their own ranges.
    }
    None
}

/// The element around char `at`, for `it` / `at`: the nearest open tag at or
/// before it whose matching close tag ends after it, as the two tags' full
/// `<…>` spans. `depth` 2 is the element around that one, and so on — `None`
/// once there are no more, as a count past the outermost pair fails in Vim.
pub(crate) fn enclosing_element(
    buf: &Buffer,
    at: usize,
    depth: usize,
) -> Option<((usize, usize), (usize, usize))> {
    let total = buf.total_chars();
    if total == 0 {
        return None;
    }
    let mut scan_from = at.min(total - 1);
    let mut must_end_after = at;
    let mut found: Option<((usize, usize), (usize, usize))> = None;
    for level in 0..depth.max(1) {
        if let Some((open, close)) = found.filter(|_| level > 0) {
            scan_from = open.0.checked_sub(1)?;
            must_end_after = close.1 - 1;
        }
        let mut hit = None;
        let mut i = scan_from + 1;
        while i > 0 {
            i -= 1;
            if buf.rope.char(i) != '<' {
                continue;
            }
            let Some(info) = enclosing_tag(buf, i) else {
                continue;
            };
            if info.kind != TagKind::Open || is_void_html_element(&info.name) {
                continue;
            }
            let Some(close) = find_close_tag(buf, info.range.1, &info.name)
                .and_then(|name| enclosing_tag(buf, name.0))
            else {
                continue;
            };
            if close.range.1 > must_end_after {
                hit = Some((info.range, close.range));
                break;
            }
        }
        found = Some(hit?);
    }
    found
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

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn buf(s: &str) -> Buffer {
        Buffer {
            rope: Rope::from_str(s),
            ..Buffer::default()
        }
    }

    fn target(b: &Buffer, line: usize, col: usize) -> Option<(usize, usize)> {
        let cur = Cursor {
            line,
            col,
            want_col: col,
        };
        match_pair_motion(b, cur).map(|m| (m.target.line, m.target.col))
    }

    #[test]
    fn percent_jumps_from_an_opener_to_its_closer() {
        assert_eq!(target(&buf("f(a, (b))\n"), 0, 1), Some((0, 8)));
    }

    #[test]
    fn percent_jumps_from_a_closer_back_to_its_opener() {
        assert_eq!(target(&buf("f(a, (b))\n"), 0, 7), Some((0, 5)));
    }

    #[test]
    fn percent_uses_the_first_bracket_after_the_cursor() {
        assert_eq!(target(&buf("if x { y }\n"), 0, 0), Some((0, 9)));
    }

    #[test]
    fn percent_crosses_lines() {
        assert_eq!(target(&buf("fn f() {\n    x\n}\n"), 0, 7), Some((2, 0)));
    }

    #[test]
    fn percent_with_nothing_to_match_is_none() {
        assert_eq!(target(&buf("plain\n"), 0, 0), None);
        assert_eq!(target(&buf("f(x\n"), 0, 0), None);
    }

    #[test]
    fn percent_is_inclusive() {
        let cur = Cursor {
            line: 0,
            col: 0,
            want_col: 0,
        };
        let m = match_pair_motion(&buf("(x)\n"), cur).unwrap();
        assert!(matches!(m.kind, MotionKind::CharInclusive));
    }

    #[test]
    fn percent_jumps_between_html_tags() {
        let mut b = buf("<div class=\"a\">x</div>\n");
        b.path = Some(std::path::PathBuf::from("page.html"));
        assert_eq!(target(&b, 0, 3), Some((0, 16)));
        assert_eq!(target(&b, 0, 18), Some((0, 0)));
    }
}
