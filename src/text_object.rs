use crate::buffer::Buffer;
use crate::cursor::Cursor;

#[derive(Debug, Clone, Copy)]
pub enum TextObjectVerb {
    Word {
        inner: bool,
    },
    BigWord {
        inner: bool,
    },
    Quotes {
        ch: char,
        inner: bool,
    },
    Pair {
        open: char,
        close: char,
        inner: bool,
    },
    Paragraph {
        inner: bool,
    },
    /// `is` / `as` — a sentence, as `(` / `)` find them.
    Sentence {
        inner: bool,
    },
    /// `it` / `at` — the element around the cursor, between its tags or with
    /// them.
    Tag {
        inner: bool,
    },
    /// `ia` / `aa` — a comma-separated argument inside `()`, `[]` or `{}`.
    Argument {
        inner: bool,
    },
    /// `if` / `af` — the function or method around the cursor, from the
    /// buffer's tree-sitter parse.
    Function {
        inner: bool,
    },
    /// `ic` / `ac` — the class, struct, impl or interface around the cursor.
    Class {
        inner: bool,
    },
    /// `gn` / `gN` — the search match under the cursor, or the next one.
    /// The search lives on the app, which resolves this; `compute` can't.
    SearchMatch {
        forward: bool,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
    /// True for the line-wise objects (`ip` / `ap`): the register is
    /// line-wise, Visual selects whole lines, and Change keeps one line.
    pub linewise: bool,
}

pub fn compute(buf: &Buffer, cur: Cursor, obj: TextObjectVerb) -> Option<TextRange> {
    match obj {
        TextObjectVerb::Word { inner } => word(buf, cur, inner, false),
        TextObjectVerb::BigWord { inner } => word(buf, cur, inner, true),
        TextObjectVerb::Quotes { ch, inner } => quoted(buf, cur, ch, inner),
        TextObjectVerb::Pair { open, close, inner } => pair(buf, cur, open, close, inner),
        TextObjectVerb::Paragraph { inner } => paragraph(buf, cur, inner),
        TextObjectVerb::Sentence { inner } => sentence(buf, cur, inner),
        TextObjectVerb::Tag { inner } => tag(buf, cur, inner, 1),
        TextObjectVerb::Argument { inner } => argument(buf, cur, inner, 1),
        TextObjectVerb::Function { inner } => syntax_object(buf, cur, false, inner, 1),
        TextObjectVerb::Class { inner } => syntax_object(buf, cur, true, inner, 1),
        TextObjectVerb::SearchMatch { .. } => None,
    }
}

/// A text object with a count in front of it — `d2aw`, `c3i(`.
///
/// The count does not mean the same thing to every object, and vim is the
/// reference here:
///
/// - **Words** extend *forward*. `2aw` is two words and their trailing
///   whitespace. `2iw` is two `iw` objects, and a run of whitespace is one of
///   those — so on `foo bar` it takes `foo ` rather than `foo bar`. That is
///   vim's rule and it is the one that surprises people, so it is spelled out.
/// - **Pairs** expand *outward*. `2i(` is the second enclosing pair, not two
///   sibling ones.
/// - **Quotes** ignore the count entirely, as vim does. There is no sensible
///   second `i"` to reach for.
///
/// `count` of 0 or 1 is the plain object. Words are line-scoped in this
/// implementation, so a count that would run past the end of the line stops
/// there rather than continuing onto the next one.
pub fn compute_counted(
    buf: &Buffer,
    cur: Cursor,
    obj: TextObjectVerb,
    count: usize,
) -> Option<TextRange> {
    let first = compute(buf, cur, obj)?;
    if count <= 1 {
        return Some(first);
    }

    match obj {
        TextObjectVerb::Word { inner } => extend_words(buf, cur, first, inner, false, count),
        TextObjectVerb::BigWord { inner } => extend_words(buf, cur, first, inner, true, count),
        // Vim ignores a count on a quote object.
        TextObjectVerb::Quotes { .. } => Some(first),
        TextObjectVerb::Pair { open, close, inner } => {
            expand_pairs(buf, first, open, close, inner, count)
        }
        TextObjectVerb::Paragraph { inner } => extend_paragraphs(buf, first, inner, count),
        TextObjectVerb::Sentence { inner } => extend_sentences(buf, first, inner, count),
        TextObjectVerb::Tag { inner } => tag(buf, cur, inner, count),
        TextObjectVerb::Argument { inner } => argument(buf, cur, inner, count),
        TextObjectVerb::Function { inner } => syntax_object(buf, cur, false, inner, count),
        TextObjectVerb::Class { inner } => syntax_object(buf, cur, true, inner, count),
        TextObjectVerb::SearchMatch { .. } => Some(first),
    }
}

/// Repeat a word object forward `count - 1` more times, taking each next object
/// from where the previous one ended. Stops at the end of the line.
fn extend_words(
    buf: &Buffer,
    cur: Cursor,
    first: TextRange,
    inner: bool,
    big: bool,
    count: usize,
) -> Option<TextRange> {
    let line_start = buf.line_start_idx(cur.line);
    let line_end = line_start + buf.line_len(cur.line);
    let mut range = first;

    for _ in 1..count {
        if range.end >= line_end {
            break;
        }
        let next_cursor = Cursor {
            line: cur.line,
            col: range.end - line_start,
            want_col: 0,
        };
        match word(buf, next_cursor, inner, big) {
            // A next object that does not actually move us forward would loop.
            Some(next) if next.end > range.end => range.end = next.end,
            _ => break,
        }
    }
    Some(range)
}

/// Walk outward to the `count`-th enclosing pair. Each step restarts the search
/// from the character before the previous opening delimiter, which is outside
/// it — so the next match is the pair that contains it.
fn expand_pairs(
    buf: &Buffer,
    first: TextRange,
    open: char,
    close: char,
    inner: bool,
    count: usize,
) -> Option<TextRange> {
    let mut range = first;

    for _ in 1..count {
        // The opening delimiter, whether or not this range includes it.
        let open_idx = if inner {
            range.start.checked_sub(1)?
        } else {
            range.start
        };
        // One character outside it, so the backward walk cannot match it again.
        let outside = open_idx.checked_sub(1)?;
        let line = buf.rope.char_to_line(outside);
        let probe = Cursor {
            line,
            col: outside - buf.line_start_idx(line),
            want_col: 0,
        };
        // No enclosing pair left — vim fails the whole operation rather than
        // acting on the smaller one, and so do we.
        range = pair(buf, probe, open, close, inner)?;
    }
    Some(range)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Whitespace,
    Word,
    Punct,
}

/// Whether `line` counts as blank for the paragraph objects. Unlike `{` / `}`,
/// Vim's `ip` / `ap` treat a whitespace-only line as blank (`:h ap`).
fn is_blank_line(buf: &Buffer, line: usize) -> bool {
    (0..buf.line_len(line)).all(|c| matches!(buf.char_at(line, c), Some(ch) if ch.is_whitespace()))
}

/// First and last line of the run around `line` whose lines are all blank, or
/// all not.
fn line_run(buf: &Buffer, line: usize, lines: usize) -> (usize, usize) {
    let blank = is_blank_line(buf, line);
    let mut first = line;
    while first > 0 && is_blank_line(buf, first - 1) == blank {
        first -= 1;
    }
    let mut last = line;
    while last + 1 < lines && is_blank_line(buf, last + 1) == blank {
        last += 1;
    }
    (first, last)
}

fn line_span(buf: &Buffer, first: usize, last: usize) -> TextRange {
    TextRange {
        start: buf.line_start_idx(first),
        end: buf.line_start_idx(last + 1),
        linewise: true,
    }
}

/// `ip` is the run of lines around the cursor — the paragraph, or the blank
/// lines it sits on. `ap` adds the run after it; for the last paragraph in the
/// file, which has none, it takes the blank lines before it instead.
fn paragraph(buf: &Buffer, cur: Cursor, inner: bool) -> Option<TextRange> {
    let lines = crate::motion::vim_line_count(buf).max(1);
    let line = cur.line.min(lines - 1);
    let (mut first, mut last) = line_run(buf, line, lines);
    if !inner {
        if last + 1 < lines {
            last = line_run(buf, last + 1, lines).1;
        } else if first > 0 && !is_blank_line(buf, line) {
            first = line_run(buf, first - 1, lines).0;
        }
    }
    Some(line_span(buf, first, last))
}

/// `Nip` takes N runs, blank ones included; `Nap` takes N paragraphs, each
/// with the blank lines after it.
fn extend_paragraphs(
    buf: &Buffer,
    first: TextRange,
    inner: bool,
    count: usize,
) -> Option<TextRange> {
    let lines = crate::motion::vim_line_count(buf).max(1);
    let first_line = buf.rope.char_to_line(first.start);
    let mut last = buf
        .rope
        .char_to_line(first.end.saturating_sub(1).max(first.start));
    let runs_per_count = if inner { 1 } else { 2 };
    for _ in 0..(count - 1) * runs_per_count {
        if last + 1 >= lines {
            break;
        }
        last = line_run(buf, last + 1, lines).1;
    }
    Some(line_span(buf, first_line, last))
}

/// The tree-sitter node kinds `af` / `ac` take, as (functions, classes), in
/// each language that has a table.
fn syntax_kinds(
    lang: crate::lang::Lang,
) -> Option<(&'static [&'static str], &'static [&'static str])> {
    use crate::lang::Lang;
    let kinds: (&'static [&'static str], &'static [&'static str]) = match lang {
        Lang::Rust => (
            &["function_item", "closure_expression"],
            &[
                "struct_item",
                "enum_item",
                "union_item",
                "trait_item",
                "impl_item",
            ],
        ),
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => (
            &[
                "function_declaration",
                "function_expression",
                "generator_function_declaration",
                "generator_function",
                "arrow_function",
                "method_definition",
            ],
            &[
                "class_declaration",
                "abstract_class_declaration",
                "class",
                "interface_declaration",
            ],
        ),
        Lang::Python => (&["function_definition", "lambda"], &["class_definition"]),
        Lang::Go => (
            &["function_declaration", "method_declaration", "func_literal"],
            &["type_declaration"],
        ),
        Lang::CSharp => (
            &[
                "method_declaration",
                "constructor_declaration",
                "local_function_statement",
                "lambda_expression",
            ],
            &[
                "class_declaration",
                "struct_declaration",
                "interface_declaration",
                "record_declaration",
                "enum_declaration",
            ],
        ),
        Lang::Lua => (&["function_declaration", "function_definition"], &[]),
        Lang::C => (
            &["function_definition"],
            &["struct_specifier", "union_specifier", "enum_specifier"],
        ),
        Lang::Cpp => (
            &["function_definition", "lambda_expression"],
            &[
                "class_specifier",
                "struct_specifier",
                "union_specifier",
                "enum_specifier",
            ],
        ),
        _ => return None,
    };
    Some(kinds)
}

/// `af` / `if` and `ac` / `ic`: the `count`th function (or class) node
/// around the cursor in the buffer's tree-sitter parse. `af` / `ac` is the
/// whole node — whole lines when it fills them, as `ap` does — and `if` /
/// `ic` its body, inside the braces and trimmed.
fn syntax_object(
    buf: &Buffer,
    cur: Cursor,
    class: bool,
    inner: bool,
    count: usize,
) -> Option<TextRange> {
    let lang = buf.path.as_deref().and_then(crate::lang::Lang::detect)?;
    let (functions, classes) = syntax_kinds(lang)?;
    let kinds = if class { classes } else { functions };
    if kinds.is_empty() {
        return None;
    }
    let source = buf.rope.to_string();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang.ts_language()).ok()?;
    let tree = crate::lang::parse_budgeted(&mut parser, &source)?;
    let at = buf.pos_to_char(cur.line, cur.col).min(buf.total_chars());
    let byte = buf.rope.char_to_byte(at);
    let mut node = tree.root_node().descendant_for_byte_range(byte, byte)?;
    let mut left = count.max(1);
    loop {
        if kinds.contains(&node.kind()) {
            left -= 1;
            if left == 0 {
                break;
            }
        }
        node = node.parent()?;
    }
    let start = buf.rope.byte_to_char(node.start_byte());
    let end = buf.rope.byte_to_char(node.end_byte());
    if inner {
        let body = match node.child_by_field_name("body") {
            Some(b) => (
                buf.rope.byte_to_char(b.start_byte()),
                buf.rope.byte_to_char(b.end_byte()),
            ),
            // Go's `type T struct { … }` keeps its braces further down.
            None => ((start..end).find(|&i| buf.rope.char(i) == '{')?, end),
        };
        let (start, end) = unbrace(buf, body.0, body.1)?;
        return Some(TextRange {
            start,
            end,
            linewise: false,
        });
    }
    let first_line = buf.rope.char_to_line(start);
    let last_line = buf.rope.char_to_line(end.saturating_sub(1).max(start));
    let line_start = buf.line_start_idx(first_line);
    let line_end = buf.line_start_idx(last_line) + buf.line_len(last_line);
    let blank = |from: usize, to: usize| (from..to).all(|i| buf.rope.char(i).is_whitespace());
    if blank(line_start, start) && blank(end.min(line_end), line_end) {
        return Some(line_span(buf, first_line, last_line));
    }
    Some(TextRange {
        start,
        end,
        linewise: false,
    })
}

/// Inside `[start, end)`: between its braces when it's a `{ … }` span, and
/// trimmed either way; `None` when nothing's left.
fn unbrace(buf: &Buffer, start: usize, end: usize) -> Option<(usize, usize)> {
    let braced = end > start + 1 && buf.rope.char(start) == '{' && buf.rope.char(end - 1) == '}';
    if braced {
        trimmed(buf, start + 1, end - 1)
    } else {
        trimmed(buf, start, end)
    }
}

/// Why `af` / `ac` found nothing when the reason is the language, not the
/// cursor — for the status line.
pub fn syntax_object_hint(buf: &Buffer, obj: TextObjectVerb) -> Option<String> {
    let class = match obj {
        TextObjectVerb::Function { .. } => false,
        TextObjectVerb::Class { .. } => true,
        _ => return None,
    };
    let what = if class { "class" } else { "function" };
    let Some(lang) = buf.path.as_deref().and_then(crate::lang::Lang::detect) else {
        return Some(format!("no {what} objects in a file of unknown language"));
    };
    let supported = match syntax_kinds(lang) {
        Some((_, classes)) if class => !classes.is_empty(),
        Some((functions, _)) => !functions.is_empty(),
        None => false,
    };
    if supported {
        return None;
    }
    Some(format!("no {what} objects for {lang:?}"))
}

/// `it` / `at`: the element around the cursor — `it` between its open and
/// close tags, `at` with them — in any buffer, not only HTML-like ones. A
/// count takes the element that many levels out. An empty `it` is no object
/// at all, so `dit` on `<b></b>` does nothing and leaves the registers alone.
fn tag(buf: &Buffer, cur: Cursor, inner: bool, count: usize) -> Option<TextRange> {
    let at = buf.pos_to_char(cur.line, cur.col);
    let (open, close) = crate::app::pair::enclosing_element(buf, at, count)?;
    let (start, end) = if inner {
        (open.1, close.0)
    } else {
        (open.0, close.1)
    };
    (end > start).then_some(TextRange {
        start,
        end,
        linewise: false,
    })
}

/// How far above the cursor `ia` / `aa` start reading for the brackets
/// around it: far enough for any argument list, without walking the whole
/// file on every use. A string opened further up than this can throw it off.
const ARGUMENT_LOOKBACK_LINES: usize = 200;

/// `ia` / `aa`: the comma-separated argument around the cursor, in the
/// nearest `()`, `[]` or `{}` holding it — commas inside nested brackets or
/// quoted strings don't split one. `ia` is the argument, trimmed. `aa` adds
/// one comma next to it: the one after it and the whitespace up to the next
/// argument, or for the last argument the one before it. A count takes the
/// argument in the pair that many levels out.
fn argument(buf: &Buffer, cur: Cursor, inner: bool, count: usize) -> Option<TextRange> {
    let total = buf.total_chars();
    let at = buf.pos_to_char(cur.line, cur.col).min(total);
    let from = buf.line_start_idx(cur.line.saturating_sub(ARGUMENT_LOOKBACK_LINES));
    let mut open: Vec<usize> = Vec::new();
    let mut quotes = QuoteScan::default();
    for i in from..at {
        let c = buf.rope.char(i);
        if quotes.step(buf, i, c) {
            continue;
        }
        match c {
            '(' | '[' | '{' => open.push(i),
            ')' | ']' | '}'
                if open
                    .last()
                    .is_some_and(|&o| closer_of(buf.rope.char(o)) == c) =>
            {
                open.pop();
            }
            _ => {}
        }
    }
    let opener = *open.get(open.len().checked_sub(count.max(1))?)?;
    let close = closer_of(buf.rope.char(opener));
    let mut commas = Vec::new();
    let mut depth = 0usize;
    let mut quotes = QuoteScan::default();
    let mut closer = None;
    for i in opener + 1..total {
        let c = buf.rope.char(i);
        if quotes.step(buf, i, c) {
            continue;
        }
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth > 0 => depth -= 1,
            _ if c == close => {
                closer = Some(i);
                break;
            }
            ',' if depth == 0 => commas.push(i),
            _ => {}
        }
    }
    let closer = closer?;
    // Argument k runs from just after the bracket or comma before it to the
    // comma or bracket after it.
    let starts: Vec<usize> = std::iter::once(opener + 1)
        .chain(commas.iter().map(|c| c + 1))
        .collect();
    let ends: Vec<usize> = commas
        .iter()
        .copied()
        .chain(std::iter::once(closer))
        .collect();
    let k = (0..starts.len()).find(|&k| starts[k] <= at && at <= ends[k])?;
    let (arg_start, arg_end) = trimmed(buf, starts[k], ends[k])?;
    if inner {
        return Some(TextRange {
            start: arg_start,
            end: arg_end,
            linewise: false,
        });
    }
    let (start, end) = if k + 1 < starts.len() {
        let next = trimmed(buf, starts[k + 1], ends[k + 1]).map_or(ends[k] + 1, |(s, _)| s);
        (arg_start, next)
    } else if k > 0 {
        let before = trimmed(buf, starts[k - 1], ends[k - 1]).map_or(ends[k - 1], |(_, e)| e);
        (before, arg_end)
    } else {
        (arg_start, arg_end)
    };
    Some(TextRange {
        start,
        end,
        linewise: false,
    })
}

fn closer_of(open: char) -> char {
    match open {
        '(' => ')',
        '[' => ']',
        _ => '}',
    }
}

/// `[start, end)` without the whitespace at either end; `None` when that
/// leaves nothing.
fn trimmed(buf: &Buffer, start: usize, end: usize) -> Option<(usize, usize)> {
    let is_ws = |i: usize| buf.rope.char(i).is_whitespace();
    let first = (start..end).find(|&i| !is_ws(i))?;
    let last = (start..end).rev().find(|&i| !is_ws(i))?;
    Some((first, last + 1))
}

/// Walks quoted strings for the argument object. `"` and `` ` `` always
/// quote; `'` only when it isn't an apostrophe (`don't`) and closes on its
/// line, so a Rust lifetime (`&'a str`) doesn't. A backslash escapes the
/// next char, and `"` / `'` strings end at the line.
#[derive(Default)]
struct QuoteScan {
    quote: Option<char>,
    escaped: bool,
}

impl QuoteScan {
    /// Whether the char `c` at `i` is part of a quoted string, its quotes
    /// included.
    fn step(&mut self, buf: &Buffer, i: usize, c: char) -> bool {
        if let Some(q) = self.quote {
            if self.escaped {
                self.escaped = false;
            } else if c == '\\' {
                self.escaped = true;
            } else if c == q || (c == '\n' && q != '`') {
                self.quote = None;
            }
            return true;
        }
        let opens = match c {
            '"' | '`' => true,
            '\'' => {
                let after_word = i > 0 && {
                    let p = buf.rope.char(i - 1);
                    p.is_alphanumeric() || p == '_'
                };
                let closes = (i + 1..buf.total_chars())
                    .map(|j| buf.rope.char(j))
                    .take_while(|&ch| ch != '\n')
                    .any(|ch| ch == '\'');
                !after_word && closes
            }
            _ => false,
        };
        if opens {
            self.quote = Some(c);
        }
        opens
    }
}

/// `is` / `as`. `is` is the sentence up to its end mark; `as` adds the spaces
/// after it, or the ones before it when there are none after. On the
/// whitespace between two sentences, `is` is that whitespace and `as` runs on
/// through the sentence after it. Neither takes a line break the sentence
/// ends on, so `das` never joins lines.
fn sentence(buf: &Buffer, cur: Cursor, inner: bool) -> Option<TextRange> {
    let total = buf.total_chars();
    if total == 0 {
        return None;
    }
    let at = buf.pos_to_char(cur.line, cur.col).min(total - 1);
    let start = (0..=at)
        .rev()
        .find(|&i| crate::motion::is_sentence_start(buf, i))
        .unwrap_or(0);
    let (body_end, next) = sentence_bounds(buf, start);
    if at >= body_end {
        let end = if inner {
            next
        } else {
            sentence_bounds(buf, next).0
        };
        return (end > body_end).then_some(TextRange {
            start: body_end,
            end,
            linewise: false,
        });
    }
    if inner {
        return Some(TextRange {
            start,
            end: body_end,
            linewise: false,
        });
    }
    let trailing = (body_end..next)
        .take_while(|&i| matches!(buf.rope.char(i), ' ' | '\t'))
        .count();
    if trailing > 0 {
        return Some(TextRange {
            start,
            end: body_end + trailing,
            linewise: false,
        });
    }
    let leading = (0..start)
        .rev()
        .take_while(|&i| matches!(buf.rope.char(i), ' ' | '\t'))
        .count();
    Some(TextRange {
        start: start - leading,
        end: body_end,
        linewise: false,
    })
}

/// Where the sentence starting at `start` ends, before the whitespace after
/// it, and where the next one starts.
fn sentence_bounds(buf: &Buffer, start: usize) -> (usize, usize) {
    let total = buf.total_chars();
    let next = (start + 1..total)
        .find(|&i| crate::motion::is_sentence_start(buf, i))
        .unwrap_or(total);
    let mut end = next;
    while end > start && matches!(buf.rope.char(end - 1), ' ' | '\t' | '\n') {
        end -= 1;
    }
    (end, next)
}

/// `2is` / `3as`: each next object starts where the last one ended — for
/// `is` that alternates sentence and whitespace, as in Vim.
fn extend_sentences(
    buf: &Buffer,
    first: TextRange,
    inner: bool,
    count: usize,
) -> Option<TextRange> {
    let total = buf.total_chars();
    let mut end = first.end;
    for _ in 1..count {
        if end >= total {
            break;
        }
        let line = buf.rope.char_to_line(end);
        let col = end - buf.rope.line_to_char(line);
        let at = Cursor {
            line,
            col,
            want_col: col,
        };
        let Some(next) = sentence(buf, at, inner) else {
            break;
        };
        end = next.end.max(end);
    }
    Some(TextRange {
        start: first.start,
        end,
        linewise: false,
    })
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
    Some(TextRange {
        start,
        end,
        linewise: false,
    })
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
fn pair(buf: &Buffer, cur: Cursor, open: char, close: char, inner: bool) -> Option<TextRange> {
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
    Some(TextRange {
        start,
        end,
        linewise: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::cursor::Cursor;
    use ropey::Rope;

    fn buf(s: &str) -> Buffer {
        Buffer {
            rope: Rope::from_str(s),
            ..Buffer::default()
        }
    }
    fn cur(l: usize, c: usize) -> Cursor {
        Cursor {
            line: l,
            col: c,
            want_col: c,
        }
    }

    // 0 a · 1 b · 2 "" · 3 c · 4 d · 5 "" · 6 e
    const PARAS: &str = "a\nb\n\nc\nd\n\ne\n";

    fn para_lines(s: &str, line: usize, inner: bool, count: usize) -> (usize, usize) {
        let b = buf(s);
        let obj = TextObjectVerb::Paragraph { inner };
        let r = compute_counted(&b, cur(line, 0), obj, count).unwrap();
        assert!(r.linewise);
        (b.rope.char_to_line(r.start), b.rope.char_to_line(r.end - 1))
    }

    #[test]
    fn ip_is_the_paragraph_and_ap_adds_the_blank_lines_after() {
        assert_eq!(para_lines(PARAS, 3, true, 1), (3, 4));
        assert_eq!(para_lines(PARAS, 3, false, 1), (3, 5));
    }

    #[test]
    fn ap_on_the_last_paragraph_takes_the_blank_lines_before() {
        assert_eq!(para_lines(PARAS, 6, false, 1), (5, 6));
    }

    #[test]
    fn ip_on_a_blank_line_is_the_blank_run() {
        assert_eq!(para_lines(PARAS, 2, true, 1), (2, 2));
        assert_eq!(para_lines(PARAS, 2, false, 1), (2, 4));
    }

    #[test]
    fn counted_paragraph_objects() {
        assert_eq!(para_lines(PARAS, 0, true, 2), (0, 2));
        assert_eq!(para_lines(PARAS, 0, false, 2), (0, 5));
    }

    #[test]
    fn whitespace_only_lines_bound_the_paragraph_objects() {
        assert_eq!(para_lines("a\n  \nb\n", 0, true, 1), (0, 0));
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

    // ---------------------------------------------------------------------
    // Counted objects — `d2aw`, `c3i(`. Before these, the count was parsed and
    // then dropped, so `d2aw` silently behaved as `daw`.
    // ---------------------------------------------------------------------

    #[test]
    fn count_one_is_the_plain_object() {
        let b = buf("one two three\n");
        let plain = compute(&b, cur(0, 0), TextObjectVerb::Word { inner: false }).unwrap();
        for n in [0, 1] {
            let r =
                compute_counted(&b, cur(0, 0), TextObjectVerb::Word { inner: false }, n).unwrap();
            assert_eq!((r.start, r.end), (plain.start, plain.end));
        }
    }

    #[test]
    fn two_aw_takes_two_words_and_their_whitespace() {
        // `d2aw` on `one two three` at `o` should leave `three`.
        let b = buf("one two three\n");
        let r = compute_counted(&b, cur(0, 0), TextObjectVerb::Word { inner: false }, 2).unwrap();
        assert_eq!(r.start, 0);
        assert_eq!(r.end, 8); // "one two "
    }

    #[test]
    fn two_iw_counts_the_whitespace_run_as_an_object() {
        // vim's rule, and the one that surprises: `iw` treats a run of
        // whitespace as an object of its own, so `2iw` on `one two` is
        // `one` plus the space — not `one two`.
        let b = buf("one two three\n");
        let r = compute_counted(&b, cur(0, 0), TextObjectVerb::Word { inner: true }, 2).unwrap();
        assert_eq!(r.start, 0);
        assert_eq!(r.end, 4); // "one "

        let r = compute_counted(&b, cur(0, 0), TextObjectVerb::Word { inner: true }, 3).unwrap();
        assert_eq!(r.end, 7); // "one two"
    }

    #[test]
    fn a_count_past_the_end_of_the_line_stops_there() {
        // Words are line-scoped here, so `d9aw` takes the rest of the line
        // rather than running on into the next one.
        let b = buf("one two\nthree\n");
        let r = compute_counted(&b, cur(0, 0), TextObjectVerb::Word { inner: false }, 9).unwrap();
        assert_eq!(r.start, 0);
        assert_eq!(r.end, 7); // "one two", not into line 2
    }

    #[test]
    fn two_aw_on_big_words_spans_the_punctuation() {
        // `aW` treats `a.b` as one word, so `2aW` reaches past `c.d`.
        let b = buf("a.b c.d e\n");
        let r =
            compute_counted(&b, cur(0, 0), TextObjectVerb::BigWord { inner: false }, 2).unwrap();
        assert_eq!(r.start, 0);
        assert_eq!(r.end, 8); // "a.b c.d "
    }

    #[test]
    fn two_i_paren_is_the_second_enclosing_pair() {
        //            0123456789
        let b = buf("f(g(x) y)\n");
        // Cursor on `x`, inside both pairs.
        let inner = TextObjectVerb::Pair {
            open: '(',
            close: ')',
            inner: true,
        };
        let one = compute_counted(&b, cur(0, 4), inner, 1).unwrap();
        assert_eq!((one.start, one.end), (4, 5)); // "x"
        let two = compute_counted(&b, cur(0, 4), inner, 2).unwrap();
        assert_eq!((two.start, two.end), (2, 8)); // "g(x) y"
    }

    #[test]
    fn two_a_paren_is_the_second_enclosing_pair_with_its_delimiters() {
        let b = buf("f(g(x) y)\n");
        let around = TextObjectVerb::Pair {
            open: '(',
            close: ')',
            inner: false,
        };
        let one = compute_counted(&b, cur(0, 4), around, 1).unwrap();
        assert_eq!((one.start, one.end), (3, 6)); // "(x)"
        let two = compute_counted(&b, cur(0, 4), around, 2).unwrap();
        assert_eq!((two.start, two.end), (1, 9)); // "(g(x) y)"
    }

    #[test]
    fn a_pair_count_with_nothing_left_to_expand_into_fails() {
        // vim refuses the whole operation rather than acting on the smaller
        // pair, so `d3i(` inside two levels of nesting does nothing.
        let b = buf("f(g(x) y)\n");
        let inner = TextObjectVerb::Pair {
            open: '(',
            close: ')',
            inner: true,
        };
        assert!(compute_counted(&b, cur(0, 4), inner, 3).is_none());
    }

    #[test]
    fn a_count_on_a_quote_object_is_ignored() {
        // As in vim — there is no sensible second `i"` to reach for.
        let b = buf("a \"x\" b \"y\" c\n");
        let obj = TextObjectVerb::Quotes {
            ch: '"',
            inner: true,
        };
        let one = compute_counted(&b, cur(0, 3), obj, 1).unwrap();
        let two = compute_counted(&b, cur(0, 3), obj, 2).unwrap();
        assert_eq!((one.start, one.end), (two.start, two.end));
    }

    #[test]
    fn iquot_inner() {
        let b = buf("a \"hello\" b\n");
        let r = compute(
            &b,
            cur(0, 5),
            TextObjectVerb::Quotes {
                ch: '"',
                inner: true,
            },
        )
        .unwrap();
        assert_eq!(r.start, 3);
        assert_eq!(r.end, 8);
    }

    #[test]
    fn aquot_around() {
        let b = buf("a \"hello\" b\n");
        let r = compute(
            &b,
            cur(0, 5),
            TextObjectVerb::Quotes {
                ch: '"',
                inner: false,
            },
        )
        .unwrap();
        assert_eq!(r.start, 2);
        assert_eq!(r.end, 9);
    }

    #[test]
    fn paren_pair_inner() {
        let b = buf("foo(bar baz) end\n");
        let r = compute(
            &b,
            cur(0, 5),
            TextObjectVerb::Pair {
                open: '(',
                close: ')',
                inner: true,
            },
        )
        .unwrap();
        assert_eq!(r.start, 4);
        assert_eq!(r.end, 11);
    }

    #[test]
    fn paren_pair_around() {
        let b = buf("foo(bar baz) end\n");
        let r = compute(
            &b,
            cur(0, 5),
            TextObjectVerb::Pair {
                open: '(',
                close: ')',
                inner: false,
            },
        )
        .unwrap();
        assert_eq!(r.start, 3);
        assert_eq!(r.end, 12);
    }

    #[test]
    fn paren_pair_balances_nested() {
        let b = buf("a(b(c)d)e\n");
        // cursor on 'c' (col 4) — innermost pair is (c)
        let r = compute(
            &b,
            cur(0, 4),
            TextObjectVerb::Pair {
                open: '(',
                close: ')',
                inner: true,
            },
        )
        .unwrap();
        assert_eq!(r.start, 4);
        assert_eq!(r.end, 5);
    }

    #[test]
    fn paren_pair_returns_none_if_no_pair() {
        let b = buf("no parens here\n");
        let r = compute(
            &b,
            cur(0, 3),
            TextObjectVerb::Pair {
                open: '(',
                close: ')',
                inner: true,
            },
        );
        assert!(r.is_none());
    }

    use proptest::prelude::*;

    fn arb_text() -> impl Strategy<Value = String> {
        // Bias toward characters that exercise the pair / quoted code paths.
        "[a-zA-Z0-9_ \t\n.,;:()\\[\\]{}\"'<>+\\-*/]{0,160}".prop_map(|s| s)
    }

    fn arb_buf_and_cursor() -> impl Strategy<Value = (Buffer, Cursor)> {
        (arb_text(), 0usize..200, 0usize..200).prop_map(|(s, line_hint, col_hint)| {
            let b = buf(&s);
            let line = line_hint % b.line_count();
            let llen = b.line_len(line);
            let col = if llen == 0 { 0 } else { col_hint % llen };
            (
                b,
                Cursor {
                    line,
                    col,
                    want_col: col,
                },
            )
        })
    }

    fn arb_verb() -> impl Strategy<Value = TextObjectVerb> {
        prop_oneof![
            any::<bool>().prop_map(|inner| TextObjectVerb::Word { inner }),
            any::<bool>().prop_map(|inner| TextObjectVerb::BigWord { inner }),
            (prop_oneof![Just('"'), Just('\''), Just('`')], any::<bool>())
                .prop_map(|(ch, inner)| TextObjectVerb::Quotes { ch, inner }),
            (
                prop_oneof![
                    Just(('(', ')')),
                    Just(('[', ']')),
                    Just(('{', '}')),
                    Just(('<', '>'))
                ],
                any::<bool>()
            )
                .prop_map(|((open, close), inner)| TextObjectVerb::Pair {
                    open,
                    close,
                    inner
                }),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        // Range is always well-formed (start <= end), in-buffer, and never
        // wraps. Compute may return None when the verb isn't applicable —
        // that's fine; what we don't tolerate is a panic or a bogus range.
        #[test]
        fn compute_returns_well_formed_range((b, c) in arb_buf_and_cursor(), v in arb_verb()) {
            if let Some(r) = compute(&b, c, v) {
                prop_assert!(r.start <= r.end, "start > end: {:?}", r);
                prop_assert!(r.end <= b.total_chars(), "end past buffer: {:?} / {}", r, b.total_chars());
            }
        }

        // For every verb that has both an inner and around form, computing
        // both at the same cursor must give around ⊇ inner (start no later,
        // end no earlier). Catches off-by-one regressions where the around
        // form forgets to widen.
        #[test]
        fn around_contains_inner_for_word((b, c) in arb_buf_and_cursor()) {
            let inner = compute(&b, c, TextObjectVerb::Word { inner: true });
            let around = compute(&b, c, TextObjectVerb::Word { inner: false });
            if let (Some(i), Some(a)) = (inner, around) {
                prop_assert!(a.start <= i.start, "around start {} > inner start {}", a.start, i.start);
                prop_assert!(a.end >= i.end, "around end {} < inner end {}", a.end, i.end);
            }
        }

        #[test]
        fn around_contains_inner_for_quotes((b, c) in arb_buf_and_cursor(), ch in prop_oneof![Just('"'), Just('\''), Just('`')]) {
            let inner = compute(&b, c, TextObjectVerb::Quotes { ch, inner: true });
            let around = compute(&b, c, TextObjectVerb::Quotes { ch, inner: false });
            if let (Some(i), Some(a)) = (inner, around) {
                prop_assert!(a.start <= i.start);
                prop_assert!(a.end >= i.end);
            }
        }

        #[test]
        fn around_contains_inner_for_pair((b, c) in arb_buf_and_cursor(), pair in prop_oneof![Just(('(', ')')), Just(('[', ']')), Just(('{', '}'))]) {
            let (open, close) = pair;
            let inner = compute(&b, c, TextObjectVerb::Pair { open, close, inner: true });
            let around = compute(&b, c, TextObjectVerb::Pair { open, close, inner: false });
            if let (Some(i), Some(a)) = (inner, around) {
                prop_assert!(a.start <= i.start);
                prop_assert!(a.end >= i.end);
            }
        }
    }

    fn sentence_text(s: &str, col: usize, inner: bool, count: usize) -> String {
        let b = buf(s);
        let obj = TextObjectVerb::Sentence { inner };
        let r = compute_counted(&b, cur(0, col), obj, count).unwrap();
        assert!(!r.linewise);
        b.rope.slice(r.start..r.end).to_string()
    }

    #[test]
    fn is_is_the_sentence_and_as_adds_the_space_after_it() {
        let s = "One two. Three four! Five.\n";
        assert_eq!(sentence_text(s, 10, true, 1), "Three four!");
        assert_eq!(sentence_text(s, 10, false, 1), "Three four! ");
        // The last one has no space after it, so `as` takes the one before.
        assert_eq!(sentence_text(s, 22, false, 1), " Five.");
        // On the space between two, `is` is that space.
        assert_eq!(sentence_text(s, 8, true, 1), " ");
    }

    #[test]
    fn a_count_on_a_sentence_object_takes_that_many() {
        let s = "One. Two. Three.\n";
        assert_eq!(sentence_text(s, 0, false, 2), "One. Two. ");
        assert_eq!(sentence_text(s, 0, true, 3), "One. Two.");
    }

    fn tag_text(s: &str, col: usize, inner: bool, count: usize) -> Option<String> {
        let b = buf(s);
        let obj = TextObjectVerb::Tag { inner };
        let r = compute_counted(&b, cur(0, col), obj, count)?;
        Some(b.rope.slice(r.start..r.end).to_string())
    }

    #[test]
    fn it_and_at_take_the_element_around_the_cursor() {
        // <div> 0-4 · <b> 5-7 · hi 8-9 · </b> 10-13 · " there" 14-19 · </div> 20-25
        let s = "<div><b>hi</b> there</div>";
        assert_eq!(tag_text(s, 9, true, 1).as_deref(), Some("hi"));
        assert_eq!(tag_text(s, 9, false, 1).as_deref(), Some("<b>hi</b>"));
        assert_eq!(tag_text(s, 16, true, 1).as_deref(), Some("<b>hi</b> there"));
        assert_eq!(tag_text(s, 2, true, 1).as_deref(), Some("<b>hi</b> there"));
    }

    #[test]
    fn a_count_on_a_tag_object_reaches_outer_elements() {
        let s = "<div><b>hi</b> there</div>";
        assert_eq!(tag_text(s, 9, true, 2).as_deref(), Some("<b>hi</b> there"));
        assert_eq!(tag_text(s, 9, false, 2).as_deref(), Some(s));
        assert_eq!(tag_text(s, 9, true, 3), None);
        assert_eq!(
            tag_text("<i><i>x</i></i>", 6, false, 2).as_deref(),
            Some("<i><i>x</i></i>")
        );
    }

    #[test]
    fn it_on_an_empty_element_is_no_object() {
        assert_eq!(tag_text("<a></a>", 1, true, 1), None);
        assert_eq!(tag_text("<a></a>", 1, false, 1).as_deref(), Some("<a></a>"));
    }

    fn arg_text(s: &str, col: usize, inner: bool, count: usize) -> Option<String> {
        let b = buf(s);
        let obj = TextObjectVerb::Argument { inner };
        let r = compute_counted(&b, cur(0, col), obj, count)?;
        Some(b.rope.slice(r.start..r.end).to_string())
    }

    #[test]
    fn ia_and_aa_take_an_argument_and_one_comma() {
        // f0 (1 a2 ,3 _4 g5 (6 b7 ,8 _9 c10 )11 ,12 _13 "14 x15 ,16 _17 y18 "19 )20
        let s = "f(a, g(b, c), \"x, y\")";
        assert_eq!(arg_text(s, 2, true, 1).as_deref(), Some("a"));
        assert_eq!(arg_text(s, 2, false, 1).as_deref(), Some("a, "));
        assert_eq!(arg_text(s, 5, true, 1).as_deref(), Some("g(b, c)"));
        assert_eq!(arg_text(s, 10, true, 1).as_deref(), Some("c"));
        assert_eq!(arg_text(s, 10, false, 1).as_deref(), Some(", c"));
        assert_eq!(arg_text(s, 15, true, 1).as_deref(), Some("\"x, y\""));
        assert_eq!(arg_text(s, 15, false, 1).as_deref(), Some(", \"x, y\""));
    }

    #[test]
    fn a_count_on_an_argument_object_reaches_the_outer_pair() {
        let s = "f(a, g(b, c), \"x, y\")";
        assert_eq!(arg_text(s, 10, true, 2).as_deref(), Some("g(b, c)"));
        assert_eq!(arg_text(s, 10, true, 3), None);
    }

    #[test]
    fn argument_objects_skip_lifetimes_and_empty_lists() {
        assert_eq!(
            arg_text("f(x: &'a str, y)", 14, true, 1).as_deref(),
            Some("y")
        );
        assert_eq!(
            arg_text("f(x: &'a str, y)", 14, false, 1).as_deref(),
            Some(", y")
        );
        assert_eq!(arg_text("f()", 2, true, 1), None);
    }

    fn syntax_text(
        file: &str,
        s: &str,
        at: &str,
        obj: TextObjectVerb,
        count: usize,
    ) -> Option<String> {
        let mut b = buf(s);
        b.path = Some(std::path::PathBuf::from(file));
        let idx = s.find(at).unwrap();
        let line = b.rope.char_to_line(idx);
        let col = idx - b.rope.line_to_char(line);
        let r = compute_counted(&b, cur(line, col), obj, count)?;
        Some(b.rope.slice(r.start..r.end).to_string())
    }

    const F_IN: TextObjectVerb = TextObjectVerb::Function { inner: true };
    const F_OUT: TextObjectVerb = TextObjectVerb::Function { inner: false };
    const C_IN: TextObjectVerb = TextObjectVerb::Class { inner: true };
    const C_OUT: TextObjectVerb = TextObjectVerb::Class { inner: false };

    #[test]
    fn syntax_objects_in_rust() {
        let s = "struct S {\n    a: u8,\n}\n\nimpl S {\n    fn f(&self) -> u8 {\n        self.a\n    }\n}\n";
        assert_eq!(
            syntax_text("x.rs", s, "self.a", F_IN, 1).as_deref(),
            Some("self.a")
        );
        assert_eq!(
            syntax_text("x.rs", s, "self.a", F_OUT, 1).as_deref(),
            Some("    fn f(&self) -> u8 {\n        self.a\n    }\n")
        );
        assert_eq!(
            syntax_text("x.rs", s, "a: u8", C_IN, 1).as_deref(),
            Some("a: u8,")
        );
        assert!(
            syntax_text("x.rs", s, "self.a", C_OUT, 1)
                .unwrap()
                .starts_with("impl S {")
        );
        let nested = "fn a() {\n    let c = || {\n        1\n    };\n}\n";
        assert_eq!(
            syntax_text("x.rs", nested, "1", F_IN, 1).as_deref(),
            Some("1")
        );
        assert_eq!(
            syntax_text("x.rs", nested, "1", F_IN, 2).as_deref(),
            Some("let c = || {\n        1\n    };")
        );
    }

    #[test]
    fn syntax_objects_in_python_typescript_and_javascript() {
        let py = "class A:\n    def f(self):\n        return 1\n";
        assert_eq!(
            syntax_text("x.py", py, "return", F_IN, 1).as_deref(),
            Some("return 1")
        );
        assert_eq!(
            syntax_text("x.py", py, "return", C_OUT, 1).as_deref(),
            Some(py)
        );
        let ts = "class C {\n  m() {\n    return 1;\n  }\n}\n";
        assert_eq!(
            syntax_text("x.ts", ts, "return", F_IN, 1).as_deref(),
            Some("return 1;")
        );
        assert_eq!(
            syntax_text("x.ts", ts, "return", C_IN, 1).as_deref(),
            Some("m() {\n    return 1;\n  }")
        );
        let js = "const f = (x) => x + 1;\n";
        assert_eq!(
            syntax_text("x.js", js, "x + 1", F_IN, 1).as_deref(),
            Some("x + 1")
        );
        assert_eq!(
            syntax_text("x.js", js, "x + 1", F_OUT, 1).as_deref(),
            Some("(x) => x + 1")
        );
    }

    #[test]
    fn syntax_objects_in_go_csharp_lua_c_and_cpp() {
        let go = "type T struct {\n\tA int\n}\n\nfunc (t T) M() int {\n\treturn t.A\n}\n";
        assert_eq!(
            syntax_text("x.go", go, "return", F_IN, 1).as_deref(),
            Some("return t.A")
        );
        assert_eq!(
            syntax_text("x.go", go, "A int", C_IN, 1).as_deref(),
            Some("A int")
        );
        let cs = "class K {\n    int M() {\n        return 1;\n    }\n}\n";
        assert_eq!(
            syntax_text("x.cs", cs, "return", F_IN, 1).as_deref(),
            Some("return 1;")
        );
        assert_eq!(
            syntax_text("x.cs", cs, "return", C_OUT, 1).as_deref(),
            Some(cs)
        );
        let lua = "local function f()\n  return 1\nend\n";
        assert_eq!(
            syntax_text("x.lua", lua, "return", F_IN, 1).as_deref(),
            Some("return 1")
        );
        assert_eq!(
            syntax_text("x.lua", lua, "return", F_OUT, 1).as_deref(),
            Some(lua)
        );
        let c = "int f(void) {\n    return 1;\n}\n";
        assert_eq!(
            syntax_text("x.c", c, "return", F_IN, 1).as_deref(),
            Some("return 1;")
        );
        let cpp = "class K {\n  int m() { return 1; }\n};\n";
        assert_eq!(
            syntax_text("x.cpp", cpp, "return", F_IN, 1).as_deref(),
            Some("return 1;")
        );
        assert_eq!(
            syntax_text("x.cpp", cpp, "return", C_OUT, 1).as_deref(),
            Some("class K {\n  int m() { return 1; }\n}")
        );
    }

    #[test]
    fn syntax_objects_say_when_the_language_has_no_table() {
        let mut b = buf("def f\nend\n");
        b.path = Some(std::path::PathBuf::from("x.rb"));
        assert!(
            syntax_object_hint(&b, F_IN)
                .unwrap()
                .contains("no function objects")
        );
        b.path = Some(std::path::PathBuf::from("x.lua"));
        assert!(
            syntax_object_hint(&b, C_IN)
                .unwrap()
                .contains("no class objects")
        );
        assert_eq!(syntax_object_hint(&b, F_IN), None);
    }
}
