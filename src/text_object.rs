use crate::buffer::Buffer;
use crate::cursor::Cursor;

#[derive(Debug, Clone, Copy)]
pub enum TextObjectVerb {
    Word { inner: bool },
    BigWord { inner: bool },
    Quotes { ch: char, inner: bool },
    Pair { open: char, close: char, inner: bool },
}

#[derive(Debug, Clone, Copy)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
    /// True if the range is meant to be linewise (not currently used by Phase-2 objects).
    pub linewise: bool,
}

pub fn compute(buf: &Buffer, cur: Cursor, obj: TextObjectVerb) -> Option<TextRange> {
    match obj {
        TextObjectVerb::Word { inner } => word(buf, cur, inner, false),
        TextObjectVerb::BigWord { inner } => word(buf, cur, inner, true),
        TextObjectVerb::Quotes { ch, inner } => quoted(buf, cur, ch, inner),
        TextObjectVerb::Pair { open, close, inner } => pair(buf, cur, open, close, inner),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Whitespace,
    Word,
    Punct,
}

fn cls_word(c: char) -> Class {
    if c.is_whitespace() {
        Class::Whitespace
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

fn cls_bigword(c: char) -> Class {
    if c.is_whitespace() {
        Class::Whitespace
    } else {
        Class::Word
    }
}

/// `iw` / `aw` (and big-word variants).
/// Inner: just the run of same-class chars under the cursor.
/// Around: includes trailing whitespace, or leading whitespace if no trailing.
fn word(buf: &Buffer, cur: Cursor, inner: bool, big: bool) -> Option<TextRange> {
    let cls: fn(char) -> Class = if big { cls_bigword } else { cls_word };
    let line_len = buf.line_len(cur.line);
    if line_len == 0 {
        return None;
    }
    let line_start = buf.line_start_idx(cur.line);
    let abs = line_start + cur.col;
    let here = buf.rope.get_char(abs)?;
    let here_class = cls(here);

    let mut start_col = cur.col;
    let mut end_col = cur.col;

    // Walk left while same class.
    while start_col > 0 {
        let c = buf.rope.char(line_start + start_col - 1);
        if cls(c) == here_class {
            start_col -= 1;
        } else {
            break;
        }
    }
    // Walk right while same class.
    while end_col + 1 < line_len {
        let c = buf.rope.char(line_start + end_col + 1);
        if cls(c) == here_class {
            end_col += 1;
        } else {
            break;
        }
    }
    let mut start = line_start + start_col;
    let mut end = line_start + end_col + 1;

    if !inner {
        // Around: include trailing whitespace if any, else leading.
        let mut probe = end_col + 1;
        let mut had_trailing_ws = false;
        while probe < line_len {
            let c = buf.rope.char(line_start + probe);
            if c.is_whitespace() {
                end = line_start + probe + 1;
                had_trailing_ws = true;
                probe += 1;
            } else {
                break;
            }
        }
        if !had_trailing_ws {
            // No trailing whitespace — include leading whitespace.
            let mut probe = start_col;
            while probe > 0 {
                let c = buf.rope.char(line_start + probe - 1);
                if c.is_whitespace() {
                    start = line_start + probe - 1;
                    probe -= 1;
                } else {
                    break;
                }
            }
        }
    }
    Some(TextRange { start, end, linewise: false })
}

/// `i"` / `a"` (and ', `).
/// Match the nearest pair of `ch` on the cursor's line that contains the cursor.
fn quoted(buf: &Buffer, cur: Cursor, ch: char, inner: bool) -> Option<TextRange> {
    let line_len = buf.line_len(cur.line);
    if line_len == 0 {
        return None;
    }
    let line_start = buf.line_start_idx(cur.line);

    // Collect quote columns on this line.
    let mut quotes: Vec<usize> = Vec::new();
    for c in 0..line_len {
        if buf.rope.char(line_start + c) == ch {
            quotes.push(c);
        }
    }
    if quotes.len() < 2 {
        return None;
    }
    // Find the pair containing (or surrounding) the cursor.
    // Simple model: the pair is (q[2k], q[2k+1]). Find the smallest such pair where q[2k] <= cur.col <= q[2k+1].
    let pair = quotes
        .chunks_exact(2)
        .find(|p| p[0] <= cur.col && cur.col <= p[1])
        .map(|p| (p[0], p[1]));
    let (open, close) = match pair {
        Some(p) => p,
        None => {
            // Cursor between pairs — pick the first pair after the cursor.
            let mut iter = quotes.chunks_exact(2);
            iter.find(|p| p[0] >= cur.col).map(|p| (p[0], p[1]))?
        }
    };

    let (start_col, end_col) = if inner {
        (open + 1, close)
    } else {
        (open, close + 1)
    };
    Some(TextRange {
        start: line_start + start_col,
        end: line_start + end_col,
        linewise: false,
    })
}

/// `i(` / `a(` etc. Searches the buffer (not just the line) for a balanced pair containing the cursor.
fn pair(
    buf: &Buffer,
    cur: Cursor,
    open: char,
    close: char,
    inner: bool,
) -> Option<TextRange> {
    let total = buf.total_chars();
    let line_start = buf.line_start_idx(cur.line);
    let here = line_start + cur.col;

    // Walk backward to find the matching open with depth balance.
    let mut depth = 1usize;
    let mut o_idx = None;
    let mut i = here;
    loop {
        if i == 0 {
            break;
        }
        i -= 1;
        let c = buf.rope.char(i);
        if c == close {
            depth += 1;
        } else if c == open {
            depth -= 1;
            if depth == 0 {
                o_idx = Some(i);
                break;
            }
        }
    }
    let o_idx = o_idx?;

    // Walk forward from o_idx + 1 to find the matching close.
    let mut depth = 1usize;
    let mut c_idx = None;
    let mut i = o_idx + 1;
    while i < total {
        let c = buf.rope.char(i);
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                c_idx = Some(i);
                break;
            }
        }
        i += 1;
    }
    let c_idx = c_idx?;

    let (start, end) = if inner {
        (o_idx + 1, c_idx)
    } else {
        (o_idx, c_idx + 1)
    };
    Some(TextRange { start, end, linewise: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::cursor::Cursor;
    use ropey::Rope;

    fn buf(s: &str) -> Buffer {
        Buffer { rope: Rope::from_str(s), path: None, dirty: false }
    }
    fn cur(l: usize, c: usize) -> Cursor {
        Cursor { line: l, col: c, want_col: c }
    }

    #[test]
    fn iw_inner_word() {
        let b = buf("hello world\n");
        let r = compute(&b, cur(0, 2), TextObjectVerb::Word { inner: true }).unwrap();
        assert_eq!(r.start, 0);
        assert_eq!(r.end, 5);
    }

    #[test]
    fn aw_around_word_takes_trailing_ws() {
        let b = buf("hello world\n");
        let r = compute(&b, cur(0, 2), TextObjectVerb::Word { inner: false }).unwrap();
        assert_eq!(r.start, 0);
        assert_eq!(r.end, 6); // includes the space
    }

    #[test]
    fn aw_takes_leading_ws_when_no_trailing() {
        let b = buf("hello world\n");
        let r = compute(&b, cur(0, 8), TextObjectVerb::Word { inner: false }).unwrap();
        assert_eq!(r.start, 5); // includes space before
        assert_eq!(r.end, 11);
    }

    #[test]
    fn iquot_inner() {
        let b = buf("a \"hello\" b\n");
        let r = compute(&b, cur(0, 5), TextObjectVerb::Quotes { ch: '"', inner: true }).unwrap();
        assert_eq!(r.start, 3);
        assert_eq!(r.end, 8);
    }

    #[test]
    fn aquot_around() {
        let b = buf("a \"hello\" b\n");
        let r = compute(&b, cur(0, 5), TextObjectVerb::Quotes { ch: '"', inner: false }).unwrap();
        assert_eq!(r.start, 2);
        assert_eq!(r.end, 9);
    }

    #[test]
    fn paren_pair_inner() {
        let b = buf("foo(bar baz) end\n");
        let r = compute(&b, cur(0, 5), TextObjectVerb::Pair { open: '(', close: ')', inner: true }).unwrap();
        assert_eq!(r.start, 4);
        assert_eq!(r.end, 11);
    }

    #[test]
    fn paren_pair_around() {
        let b = buf("foo(bar baz) end\n");
        let r = compute(&b, cur(0, 5), TextObjectVerb::Pair { open: '(', close: ')', inner: false }).unwrap();
        assert_eq!(r.start, 3);
        assert_eq!(r.end, 12);
    }

    #[test]
    fn paren_pair_balances_nested() {
        let b = buf("a(b(c)d)e\n");
        // cursor on 'c' (col 4) — innermost pair is (c)
        let r = compute(&b, cur(0, 4), TextObjectVerb::Pair { open: '(', close: ')', inner: true }).unwrap();
        assert_eq!(r.start, 4);
        assert_eq!(r.end, 5);
    }

    #[test]
    fn paren_pair_returns_none_if_no_pair() {
        let b = buf("no parens here\n");
        let r = compute(&b, cur(0, 3), TextObjectVerb::Pair { open: '(', close: ')', inner: true });
        assert!(r.is_none());
    }
}
