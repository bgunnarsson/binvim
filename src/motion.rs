use crate::buffer::Buffer;
use crate::cursor::Cursor;

#[derive(Debug, Clone, Copy)]
pub enum MotionKind {
    /// Range is [from, to) — `to` is NOT included.
    CharExclusive,
    /// Range is [from, to] — `to` IS included.
    CharInclusive,
    /// Range covers full lines from min(from.line, to.line) to max(...).
    Linewise,
}

#[derive(Debug, Clone, Copy)]
pub struct MotionResult {
    pub target: Cursor,
    pub kind: MotionKind,
}

pub fn left(_buf: &Buffer, cur: Cursor, count: usize) -> MotionResult {
    let new_col = cur.col.saturating_sub(count);
    MotionResult {
        target: Cursor { line: cur.line, col: new_col, want_col: new_col },
        kind: MotionKind::CharExclusive,
    }
}

pub fn right(buf: &Buffer, cur: Cursor, count: usize) -> MotionResult {
    let len = buf.line_len(cur.line);
    let max = if len == 0 { 0 } else { len - 1 };
    let new_col = (cur.col + count).min(max);
    MotionResult {
        target: Cursor { line: cur.line, col: new_col, want_col: new_col },
        kind: MotionKind::CharExclusive,
    }
}

pub fn up(buf: &Buffer, cur: Cursor, count: usize) -> MotionResult {
    let new_line = cur.line.saturating_sub(count);
    let len = buf.line_len(new_line);
    let max = if len == 0 { 0 } else { len - 1 };
    let new_col = cur.want_col.min(max);
    MotionResult {
        target: Cursor { line: new_line, col: new_col, want_col: cur.want_col },
        kind: MotionKind::Linewise,
    }
}

pub fn down(buf: &Buffer, cur: Cursor, count: usize) -> MotionResult {
    let last = buf.line_count().saturating_sub(1);
    let new_line = (cur.line + count).min(last);
    let len = buf.line_len(new_line);
    let max = if len == 0 { 0 } else { len - 1 };
    let new_col = cur.want_col.min(max);
    MotionResult {
        target: Cursor { line: new_line, col: new_col, want_col: cur.want_col },
        kind: MotionKind::Linewise,
    }
}

pub fn line_start(_buf: &Buffer, cur: Cursor) -> MotionResult {
    MotionResult {
        target: Cursor { line: cur.line, col: 0, want_col: 0 },
        kind: MotionKind::CharExclusive,
    }
}

pub fn line_end(buf: &Buffer, cur: Cursor) -> MotionResult {
    let len = buf.line_len(cur.line);
    let col = if len == 0 { 0 } else { len - 1 };
    MotionResult {
        target: Cursor { line: cur.line, col, want_col: usize::MAX },
        kind: MotionKind::CharInclusive,
    }
}

pub fn first_line(_buf: &Buffer, _cur: Cursor) -> MotionResult {
    MotionResult {
        target: Cursor { line: 0, col: 0, want_col: 0 },
        kind: MotionKind::Linewise,
    }
}

pub fn last_line(buf: &Buffer, _cur: Cursor) -> MotionResult {
    let line = buf.line_count().saturating_sub(1);
    MotionResult {
        target: Cursor { line, col: 0, want_col: 0 },
        kind: MotionKind::Linewise,
    }
}

pub fn goto_line(buf: &Buffer, n: usize) -> MotionResult {
    let line = n.saturating_sub(1).min(buf.line_count().saturating_sub(1));
    MotionResult {
        target: Cursor { line, col: 0, want_col: 0 },
        kind: MotionKind::Linewise,
    }
}

pub fn first_non_blank(buf: &Buffer, cur: Cursor) -> MotionResult {
    let line_len = buf.line_len(cur.line);
    let mut col = 0;
    while col < line_len {
        match buf.char_at(cur.line, col) {
            Some(c) if c.is_whitespace() => col += 1,
            _ => break,
        }
    }
    MotionResult {
        target: Cursor { line: cur.line, col, want_col: col },
        kind: MotionKind::CharExclusive,
    }
}

pub fn last_non_blank(buf: &Buffer, cur: Cursor) -> MotionResult {
    let line_len = buf.line_len(cur.line);
    if line_len == 0 {
        return MotionResult {
            target: cur,
            kind: MotionKind::CharInclusive,
        };
    }
    let mut col = line_len - 1;
    loop {
        match buf.char_at(cur.line, col) {
            Some(c) if !c.is_whitespace() => break,
            _ => {
                if col == 0 {
                    break;
                }
                col -= 1;
            }
        }
    }
    MotionResult {
        target: Cursor { line: cur.line, col, want_col: col },
        kind: MotionKind::CharInclusive,
    }
}

/// In-line find: scan the cursor's line for `ch`. `before=true` for `t`/`T` (stop one char before).
/// Returns `None` if not found — caller stays put.
pub fn find_char(
    buf: &Buffer,
    cur: Cursor,
    ch: char,
    forward: bool,
    before: bool,
    count: usize,
) -> Option<MotionResult> {
    let line = cur.line;
    let line_len = buf.line_len(line);
    if line_len == 0 || count == 0 {
        return None;
    }
    let mut hits = 0usize;
    let found_col = if forward {
        let mut found = None;
        let mut c = cur.col + 1;
        while c < line_len {
            if buf.char_at(line, c) == Some(ch) {
                hits += 1;
                if hits == count {
                    found = Some(c);
                    break;
                }
            }
            c += 1;
        }
        found?
    } else {
        if cur.col == 0 {
            return None;
        }
        let mut found = None;
        let mut c = cur.col - 1;
        loop {
            if buf.char_at(line, c) == Some(ch) {
                hits += 1;
                if hits == count {
                    found = Some(c);
                    break;
                }
            }
            if c == 0 {
                break;
            }
            c -= 1;
        }
        found?
    };
    let target_col = if before {
        if forward { found_col - 1 } else { found_col + 1 }
    } else {
        found_col
    };
    Some(MotionResult {
        target: Cursor { line, col: target_col, want_col: target_col },
        kind: MotionKind::CharInclusive,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Whitespace,
    Word,
    Punct,
}

type ClassFn = fn(char) -> CharClass;

fn class_word(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Whitespace
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

fn class_bigword(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Whitespace
    } else {
        CharClass::Word
    }
}

// === word-start forward (w / W) ============================================
pub fn word_forward(buf: &Buffer, cur: Cursor, count: usize) -> MotionResult {
    iter_motion(buf, cur, count, |b, c| next_word_start(b, c, class_word), MotionKind::CharExclusive)
}

pub fn big_word_forward(buf: &Buffer, cur: Cursor, count: usize) -> MotionResult {
    iter_motion(buf, cur, count, |b, c| next_word_start(b, c, class_bigword), MotionKind::CharExclusive)
}

// === word-start backward (b / B) ===========================================
pub fn word_backward(buf: &Buffer, cur: Cursor, count: usize) -> MotionResult {
    iter_motion(buf, cur, count, |b, c| prev_word_start(b, c, class_word), MotionKind::CharExclusive)
}

pub fn big_word_backward(buf: &Buffer, cur: Cursor, count: usize) -> MotionResult {
    iter_motion(buf, cur, count, |b, c| prev_word_start(b, c, class_bigword), MotionKind::CharExclusive)
}

// === word-end forward (e / E) ==============================================
pub fn end_word(buf: &Buffer, cur: Cursor, count: usize) -> MotionResult {
    iter_motion(buf, cur, count, |b, c| next_word_end(b, c, class_word), MotionKind::CharInclusive)
}

pub fn big_end_word(buf: &Buffer, cur: Cursor, count: usize) -> MotionResult {
    iter_motion(buf, cur, count, |b, c| next_word_end(b, c, class_bigword), MotionKind::CharInclusive)
}

// === word-end backward (ge / gE) ===========================================
pub fn end_word_backward(buf: &Buffer, cur: Cursor, count: usize) -> MotionResult {
    iter_motion(buf, cur, count, |b, c| prev_word_end(b, c, class_word), MotionKind::CharInclusive)
}

pub fn big_end_word_backward(buf: &Buffer, cur: Cursor, count: usize) -> MotionResult {
    iter_motion(buf, cur, count, |b, c| prev_word_end(b, c, class_bigword), MotionKind::CharInclusive)
}

fn iter_motion<F>(buf: &Buffer, cur: Cursor, count: usize, step: F, kind: MotionKind) -> MotionResult
where
    F: Fn(&Buffer, Cursor) -> Cursor,
{
    let mut target = cur;
    for _ in 0..count {
        target = step(buf, target);
    }
    MotionResult { target, kind }
}

// --- internal walkers ------------------------------------------------------

fn next_word_start(buf: &Buffer, cur: Cursor, cf: ClassFn) -> Cursor {
    let mut line = cur.line;
    let mut col = cur.col;

    let start = buf.char_at(line, col).map(cf);
    if let Some(start) = start {
        if start != CharClass::Whitespace {
            loop {
                match advance_one(buf, line, col) {
                    None => return past_end(buf, line),
                    Some((nl, nc)) => {
                        let crossed_line = nl != line;
                        line = nl;
                        col = nc;
                        if crossed_line {
                            break;
                        }
                        let cls = buf.char_at(line, col).map(cf);
                        if cls != Some(start) {
                            break;
                        }
                    }
                }
            }
            if let Some(c) = buf.char_at(line, col) {
                if !c.is_whitespace() {
                    return Cursor { line, col, want_col: col };
                }
            }
        }
    }
    skip_whitespace_forward(buf, &mut line, &mut col);
    // If we ended on whitespace or past-end with no further word, return past-end so
    // exclusive-motion operators delete through trailing whitespace at EOF.
    match buf.char_at(line, col) {
        Some(c) if !c.is_whitespace() => Cursor { line, col, want_col: col },
        _ => past_end(buf, line),
    }
}

fn past_end(buf: &Buffer, line: usize) -> Cursor {
    let len = buf.line_len(line);
    Cursor { line, col: len, want_col: len }
}

fn next_word_end(buf: &Buffer, cur: Cursor, cf: ClassFn) -> Cursor {
    let mut line = cur.line;
    let mut col = cur.col;

    // Always advance one position first (so repeated `e` makes progress).
    match advance_one(buf, line, col) {
        None => return cur,
        Some((l, c)) => {
            line = l;
            col = c;
        }
    }
    skip_whitespace_forward(buf, &mut line, &mut col);

    let start = match buf.char_at(line, col) {
        Some(c) => cf(c),
        None => return Cursor { line, col, want_col: col },
    };

    // Walk forward through the run; stop one position past the last same-class char.
    loop {
        match advance_one(buf, line, col) {
            None => return Cursor { line, col, want_col: col },
            Some((nl, nc)) => {
                let crossed_line = nl != line;
                if crossed_line {
                    return Cursor { line, col, want_col: col };
                }
                let cls = buf.char_at(nl, nc).map(cf);
                if cls == Some(start) {
                    line = nl;
                    col = nc;
                } else {
                    return Cursor { line, col, want_col: col };
                }
            }
        }
    }
}

fn prev_word_start(buf: &Buffer, cur: Cursor, cf: ClassFn) -> Cursor {
    let mut line = cur.line;
    let mut col = cur.col;

    match retreat_one(buf, line, col) {
        Some((l, c)) => {
            line = l;
            col = c;
        }
        None => return Cursor { line: 0, col: 0, want_col: 0 },
    }
    skip_whitespace_backward(buf, &mut line, &mut col);

    let start = match buf.char_at(line, col) {
        Some(c) => cf(c),
        None => return Cursor { line, col, want_col: col },
    };
    loop {
        match retreat_one(buf, line, col) {
            Some((l, c)) => {
                let cls = buf.char_at(l, c).map(cf);
                if cls == Some(start) {
                    line = l;
                    col = c;
                } else {
                    break;
                }
            }
            None => return Cursor { line: 0, col: 0, want_col: 0 },
        }
    }
    Cursor { line, col, want_col: col }
}

fn prev_word_end(buf: &Buffer, cur: Cursor, cf: ClassFn) -> Cursor {
    let mut line = cur.line;
    let mut col = cur.col;

    // Skip current run (if we're inside a word) so we land in whitespace before the prev word.
    let starting = buf.char_at(line, col).map(cf);
    if let Some(start) = starting {
        if start != CharClass::Whitespace {
            // Walk back while same class; stop at first different class or BOF.
            loop {
                match retreat_one(buf, line, col) {
                    Some((l, c)) => {
                        let cls = buf.char_at(l, c).map(cf);
                        line = l;
                        col = c;
                        if cls != Some(start) {
                            break;
                        }
                    }
                    None => return Cursor { line: 0, col: 0, want_col: 0 },
                }
            }
        }
    }
    skip_whitespace_backward(buf, &mut line, &mut col);
    Cursor { line, col, want_col: col }
}

fn skip_whitespace_forward(buf: &Buffer, line: &mut usize, col: &mut usize) {
    loop {
        match buf.char_at(*line, *col) {
            Some(c) if c.is_whitespace() => match advance_one(buf, *line, *col) {
                None => return,
                Some((l, c)) => {
                    *line = l;
                    *col = c;
                }
            },
            None => match advance_one(buf, *line, *col) {
                None => return,
                Some((l, c)) => {
                    *line = l;
                    *col = c;
                }
            },
            Some(_) => return,
        }
    }
}

fn skip_whitespace_backward(buf: &Buffer, line: &mut usize, col: &mut usize) {
    loop {
        match buf.char_at(*line, *col) {
            Some(c) if c.is_whitespace() => match retreat_one(buf, *line, *col) {
                None => return,
                Some((l, c)) => {
                    *line = l;
                    *col = c;
                }
            },
            None => match retreat_one(buf, *line, *col) {
                None => return,
                Some((l, c)) => {
                    *line = l;
                    *col = c;
                }
            },
            Some(_) => return,
        }
    }
}

fn advance_one(buf: &Buffer, line: usize, col: usize) -> Option<(usize, usize)> {
    let len = buf.line_len(line);
    if len > 0 && col + 1 < len {
        Some((line, col + 1))
    } else if line + 1 < buf.line_count() {
        Some((line + 1, 0))
    } else {
        None
    }
}

fn retreat_one(buf: &Buffer, line: usize, col: usize) -> Option<(usize, usize)> {
    if col > 0 {
        Some((line, col - 1))
    } else if line > 0 {
        let prev_len = buf.line_len(line - 1);
        let c = if prev_len == 0 { 0 } else { prev_len - 1 };
        Some((line - 1, c))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn buf(s: &str) -> Buffer {
        Buffer {
            rope: Rope::from_str(s),
            path: None,
            dirty: false,
            version: 0,
        }
    }

    fn cur(line: usize, col: usize) -> Cursor {
        Cursor { line, col, want_col: col }
    }

    #[test]
    fn left_clamps_at_zero() {
        let b = buf("hello\n");
        let r = left(&b, cur(0, 0), 1);
        assert_eq!(r.target, cur(0, 0));
    }

    #[test]
    fn right_clamps_at_line_end() {
        let b = buf("hello\n");
        let r = right(&b, cur(0, 4), 5);
        assert_eq!(r.target.col, 4);
    }

    #[test]
    fn down_preserves_want_col() {
        let b = buf("hello world\nhi\nlong line here\n");
        let mut c = cur(0, 8);
        c.want_col = 8;
        let r = down(&b, c, 1);
        assert_eq!(r.target.line, 1);
        assert_eq!(r.target.col, 1);
        assert_eq!(r.target.want_col, 8);
        let r2 = down(&b, r.target, 1);
        assert_eq!(r2.target.line, 2);
        assert_eq!(r2.target.col, 8);
    }

    #[test]
    fn line_start_motion() {
        let b = buf("    hello\n");
        let r = line_start(&b, cur(0, 7));
        assert_eq!(r.target.col, 0);
    }

    #[test]
    fn line_end_motion() {
        let b = buf("hello\n");
        let r = line_end(&b, cur(0, 0));
        assert_eq!(r.target.col, 4);
    }

    #[test]
    fn word_forward_basic() {
        let b = buf("hello world foo\n");
        let r = word_forward(&b, cur(0, 0), 1);
        assert_eq!(r.target.col, 6);
        let r = word_forward(&b, cur(0, 0), 2);
        assert_eq!(r.target.col, 12);
    }

    #[test]
    fn word_forward_punct_class_break() {
        let b = buf("foo.bar\n");
        let r = word_forward(&b, cur(0, 0), 1);
        assert_eq!(r.target.col, 3);
        let r = word_forward(&b, cur(0, 0), 2);
        assert_eq!(r.target.col, 4);
    }

    #[test]
    fn big_word_forward_treats_punct_as_word() {
        let b = buf("foo.bar baz\n");
        let r = big_word_forward(&b, cur(0, 0), 1);
        assert_eq!(r.target.col, 8); // jumps past "foo.bar" to "baz"
    }

    #[test]
    fn word_backward_basic() {
        let b = buf("hello world foo\n");
        let r = word_backward(&b, cur(0, 12), 1);
        assert_eq!(r.target.col, 6);
        let r = word_backward(&b, cur(0, 12), 2);
        assert_eq!(r.target.col, 0);
    }

    #[test]
    fn end_word_basic() {
        let b = buf("hello world\n");
        let r = end_word(&b, cur(0, 0), 1);
        assert_eq!(r.target.col, 4);
        let r = end_word(&b, cur(0, 0), 2);
        assert_eq!(r.target.col, 10);
    }

    #[test]
    fn end_word_stops_at_line_break() {
        let b = buf("hello\nworld\n");
        let r = end_word(&b, cur(0, 0), 1);
        assert_eq!(r.target.line, 0);
        assert_eq!(r.target.col, 4);
        let r = end_word(&b, cur(0, 0), 2);
        assert_eq!(r.target.line, 1);
        assert_eq!(r.target.col, 4);
    }

    #[test]
    fn end_word_backward_skips_current_word() {
        let b = buf("alpha beta gamma\n");
        // cursor on 'e' of "beta" (col 7) — `ge` should land on 'a' of "alpha" (col 4)
        let r = end_word_backward(&b, cur(0, 7), 1);
        assert_eq!(r.target.col, 4);
    }

    #[test]
    fn first_and_last_line() {
        let b = buf("one\ntwo\nthree\n");
        let r = last_line(&b, cur(0, 0));
        assert_eq!(r.target.line, 3);
        let r = first_line(&b, cur(2, 1));
        assert_eq!(r.target.line, 0);
    }

    #[test]
    fn goto_line_clamps() {
        let b = buf("one\ntwo\nthree\n");
        let r = goto_line(&b, 2);
        assert_eq!(r.target.line, 1);
        let r = goto_line(&b, 999);
        assert_eq!(r.target.line, 3);
    }

    #[test]
    fn down_at_last_line_stays() {
        let b = buf("one\ntwo\n");
        let r = down(&b, cur(2, 0), 5);
        assert_eq!(r.target.line, 2);
    }

    #[test]
    fn word_forward_past_end_at_eof() {
        // No trailing newline. Cursor at start; word_forward must return col == line_len
        // so an exclusive-motion operator (dw) deletes through end of buffer.
        let b = buf("hello");
        let r = word_forward(&b, cur(0, 0), 1);
        assert_eq!(r.target.line, 0);
        assert_eq!(r.target.col, 5); // line_len, past last char
    }

    #[test]
    fn word_forward_past_trailing_whitespace_at_eof() {
        let b = buf("hello   ");
        let r = word_forward(&b, cur(0, 0), 1);
        assert_eq!(r.target.col, 8);
    }

    #[test]
    fn find_char_forward_inclusive() {
        let b = buf("hello world\n");
        let r = find_char(&b, cur(0, 0), 'o', true, false, 1).unwrap();
        assert_eq!(r.target.col, 4);
    }

    #[test]
    fn find_char_forward_before_t() {
        let b = buf("hello world\n");
        // tw lands on the char before 'w'
        let r = find_char(&b, cur(0, 0), 'w', true, true, 1).unwrap();
        assert_eq!(r.target.col, 5);
    }

    #[test]
    fn find_char_backward() {
        let b = buf("hello world\n");
        let r = find_char(&b, cur(0, 8), 'o', false, false, 1).unwrap();
        assert_eq!(r.target.col, 7); // 'o' of "world"
    }

    #[test]
    fn find_char_count() {
        let b = buf("axbxcxdx\n");
        let r = find_char(&b, cur(0, 0), 'x', true, false, 3).unwrap();
        assert_eq!(r.target.col, 5); // 3rd x
    }

    #[test]
    fn find_char_not_found_returns_none() {
        let b = buf("hello\n");
        let r = find_char(&b, cur(0, 0), 'z', true, false, 1);
        assert!(r.is_none());
    }

    #[test]
    fn find_char_does_not_cross_lines() {
        let b = buf("hello\nworld\n");
        // 'w' exists on next line but find shouldn't cross
        let r = find_char(&b, cur(0, 0), 'w', true, false, 1);
        assert!(r.is_none());
    }

    #[test]
    fn first_non_blank_skips_leading_ws() {
        let b = buf("    hello\n");
        let r = first_non_blank(&b, cur(0, 0));
        assert_eq!(r.target.col, 4);
    }

    #[test]
    fn first_non_blank_empty_line() {
        let b = buf("\nhello\n");
        let r = first_non_blank(&b, cur(0, 0));
        assert_eq!(r.target.col, 0);
    }

    #[test]
    fn last_non_blank_skips_trailing_ws() {
        let b = buf("hello   \n");
        let r = last_non_blank(&b, cur(0, 0));
        assert_eq!(r.target.col, 4);
    }
}
