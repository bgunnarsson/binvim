//! The notebook page: a notebook buffer drawn the way Jupyter draws it —
//! markdown rendered and wrapped, code on a slab under its `▶ [n]` label,
//! outputs beneath — in place of the percent text, where every cell header,
//! line number and markdown marker competes with the content.
//!
//! The buffer stays the source of truth. The page is laid out from it on
//! each frame, and the cell it marks is the one holding the cursor, so the
//! text view (`Enter` on the page, `<leader>nv` back) keeps the place, and
//! undo, the kernel and saves need nothing of their own.

use std::cell::RefCell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossterm::style::Color;
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::Buffer;
use crate::config::Config;
use crate::graphics::ImageStore;
use crate::markdown_render::{ConcealAction, MarkdownLineKind, MarkdownLineMeta, TableRowKind};
use crate::notebook::{CellKind, CellSpan, OutputStyle};
use crate::render::{TAB_WIDTH, cluster_width};

/// Wide enough for `▶ [9999]`; a longer label loses its left end.
const LABEL_W: usize = 9;
/// The selection bar, the label, and the slab's left padding column.
const GUTTER: usize = 1 + LABEL_W + 1;
/// Below this pane width the labels go and only the bar is kept.
const NARROW: usize = 40;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Style {
    pub fg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

/// One grapheme cluster as the page paints it.
#[derive(Debug, Clone, PartialEq)]
pub struct Seg {
    pub text: String,
    pub width: usize,
    pub style: Style,
    /// A list bullet or quote bar standing in for its markdown marker: a
    /// wrapped item's later rows hang under the text after it.
    marker: bool,
    /// The URL a click on this cluster opens: one printed in an output.
    pub link: Option<Arc<str>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelKind {
    In,
    /// Sent to the kernel and not finished — Jupyter's `In [*]:`, with a
    /// stop button where the play button was.
    Busy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PageRow {
    /// The cell the row belongs to; `None` for the gap between two cells.
    pub cell: Option<usize>,
    pub label: Option<(String, LabelKind)>,
    /// The rest of a code line that didn't fit the row above.
    pub cont: bool,
    /// Background from the slab's padding column to the pane's edge.
    pub fill: Option<Color>,
    /// Blank columns ahead of `segs`, inside the content area.
    pub indent: usize,
    pub segs: Vec<Seg>,
    /// One row of an image, drawn in place of `segs`.
    pub image: Option<ImageRow>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageRow {
    /// The image's id in the `ImageStore`, which sends it.
    pub id: u32,
    pub row: usize,
    pub cols: usize,
}

#[derive(Debug, Clone)]
pub struct PageLayout {
    pub rows: Vec<PageRow>,
    pub spans: Vec<CellSpan>,
    /// The rows of each cell in `spans`, without the gap after it.
    pub cells: Vec<Range<usize>>,
    /// The first content column.
    pub gutter: usize,
}

/// `images` is `None` where images aren't drawn, and each is a line naming
/// it instead.
pub fn layout(
    buffer: &Buffer,
    colors: Option<&[Option<Color>]>,
    width: usize,
    config: &Config,
    images: Option<&RefCell<ImageStore>>,
) -> PageLayout {
    let gutter = if width >= NARROW { GUTTER } else { 2 };
    let content_w = width.saturating_sub(gutter + 1).max(1);
    let spans = crate::notebook::cell_spans(&buffer.rope);
    let mut page = Page {
        buffer,
        colors,
        config,
        images,
        dir: buffer
            .path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf),
        width: content_w,
        rows: Vec::new(),
        cell: 0,
    };
    let mut cells = Vec::with_capacity(spans.len());
    for (i, span) in spans.iter().enumerate() {
        if i > 0 {
            page.rows.push(PageRow {
                cell: None,
                label: None,
                cont: false,
                fill: None,
                indent: 0,
                segs: Vec::new(),
                image: None,
            });
        }
        page.cell = i;
        let start = page.rows.len();
        match span.kind {
            CellKind::Code => page.code_cell(span),
            CellKind::Markdown => page.markdown_cell(span),
            CellKind::Raw => page.raw_cell(span),
        }
        cells.push(start..page.rows.len());
    }
    PageLayout {
        rows: page.rows,
        spans,
        cells,
        gutter,
    }
}

impl PageLayout {
    /// The cell the page marks for a cursor on `line`.
    pub fn cell_of_line(&self, line: usize) -> Option<usize> {
        crate::notebook::span_at(&self.spans, line)
    }

    /// The cell drawn on `row`; the gap between two cells counts as the
    /// one below it.
    pub fn cell_at_row(&self, row: usize) -> Option<usize> {
        let r = self.rows.get(row)?;
        r.cell
            .or_else(|| self.cells.iter().position(|c| c.start > row))
    }

    pub fn max_top(&self, height: usize) -> usize {
        self.rows.len().saturating_sub(height)
    }

    /// The top row that shows all of `cell`, or its start when it is taller
    /// than the pane, moving from `top` as little as that takes.
    pub fn reveal(&self, cell: usize, top: usize, height: usize) -> usize {
        let Some(r) = self.cells.get(cell) else {
            return top.min(self.max_top(height));
        };
        let mut top = top;
        if r.end > top + height {
            top = r.end.saturating_sub(height);
        }
        if r.start < top {
            top = r.start;
        }
        top.min(self.max_top(height))
    }

    /// `top`, unless none of `cell` is on screen from it — then the top
    /// that reveals it. Lets the page scroll past the start of a long cell
    /// without being pulled back on the next frame.
    pub fn keep_in_view(&self, cell: usize, top: usize, height: usize) -> usize {
        let top = top.min(self.max_top(height));
        match self.cells.get(cell) {
            Some(r) if r.end <= top || r.start >= top + height => self.reveal(cell, top, height),
            _ => top,
        }
    }

    /// `cell` if any of it is on screen from `top`; otherwise the first
    /// cell that is (the last, when `up`), for a scroll that left the marked
    /// cell behind.
    pub fn visible_cell(&self, cell: usize, top: usize, height: usize, up: bool) -> Option<usize> {
        let shown = |r: &Range<usize>| r.end > top && r.start < top + height;
        if self.cells.get(cell).is_some_and(shown) {
            return Some(cell);
        }
        let mut on = self.cells.iter().enumerate().filter(|(_, r)| shown(r));
        if up { on.next_back() } else { on.next() }.map(|(i, _)| i)
    }

    /// The line a cursor goes to when the page marks `cell`.
    pub fn cell_line(&self, cell: usize) -> Option<usize> {
        let span = self.spans.get(cell)?;
        Some(if span.body.is_empty() {
            span.start()
        } else {
            span.body.start
        })
    }
}

struct Page<'a> {
    buffer: &'a Buffer,
    colors: Option<&'a [Option<Color>]>,
    config: &'a Config,
    images: Option<&'a RefCell<ImageStore>>,
    /// Where a markdown image's relative path starts: the notebook's folder.
    dir: Option<PathBuf>,
    /// Columns a row's text wraps at.
    width: usize,
    rows: Vec<PageRow>,
    cell: usize,
}

impl Page<'_> {
    /// Line `i` without its line break, and the byte it starts at — the
    /// highlight cache's index.
    fn line(&self, i: usize) -> (String, usize) {
        let rope = &self.buffer.rope;
        if i >= rope.len_lines() {
            return (String::new(), rope.len_bytes());
        }
        let text: String = rope.line(i).chars().collect();
        let text = text
            .trim_end_matches([
                '\n', '\r', '\u{0b}', '\u{0c}', '\u{85}', '\u{2028}', '\u{2029}',
            ])
            .to_string();
        (text, rope.line_to_byte(i))
    }

    /// The highlight colour at `byte`; `None` for text the page made, which
    /// has no place in the buffer.
    fn color_at(&self, byte: Option<usize>) -> Color {
        byte.and_then(|b| self.colors?.get(b).copied().flatten())
            .unwrap_or_else(|| self.config.theme_fg())
    }

    fn push(&mut self, segs: Vec<Seg>, words: bool, fill: Option<Color>) -> usize {
        let first = self.rows.len();
        for (n, (indent, segs)) in wrap(segs, self.width, words).into_iter().enumerate() {
            self.rows.push(PageRow {
                cell: Some(self.cell),
                label: None,
                cont: n > 0 && !words,
                fill,
                indent,
                segs,
                image: None,
            });
        }
        first
    }

    fn code_cell(&mut self, span: &CellSpan) {
        let doc = self.buffer.notebook.as_ref();
        let id = span.id.as_deref();
        let (count, _) = id.and_then(|id| doc?.cell_info(id)).unwrap_or((None, 0));
        let busy = id.is_some_and(|id| doc.is_some_and(|d| d.is_busy(id)));
        let label = match count {
            _ if busy => ("■ [*]".to_string(), LabelKind::Busy),
            Some(n) => (format!("▶ [{n}]"), LabelKind::In),
            None => ("▶ [ ]".to_string(), LabelKind::In),
        };
        let fill = Some(self.config.theme_code_bg());
        let first = self.rows.len();
        if span.body.is_empty() {
            self.push(Vec::new(), false, fill);
        }
        for i in span.body.clone() {
            let (text, byte) = self.line(i);
            let segs = self.code_segs(&text, byte);
            self.push(segs, false, fill);
        }
        self.rows[first].label = Some(label);
        let outputs: &[Value] = match (id, doc) {
            (Some(id), Some(doc)) => doc.outputs(id),
            _ => &[],
        };
        for out in crate::notebook::output_rows(outputs) {
            if out.image.is_some_and(|i| self.output_image(&outputs[i])) {
                continue;
            }
            let (fg, italic) = match out.style {
                OutputStyle::Text => (self.config.theme_fg(), false),
                OutputStyle::Stderr => (self.config.diagnostic_warning(), false),
                OutputStyle::Error => (self.config.diagnostic_error(), false),
                OutputStyle::Note => (self.config.theme_dim(), true),
            };
            let style = Style {
                fg: Some(fg),
                italic,
                ..Style::default()
            };
            let mut segs = plain_segs(&out.text, style);
            link_urls(&mut segs);
            self.push(segs, false, None);
        }
    }

    fn code_segs(&self, text: &str, byte: usize) -> Vec<Seg> {
        let mut segs = Vec::new();
        let mut col = 0;
        let mut off = byte;
        for g in text.graphemes(true) {
            let style = Style {
                fg: Some(self.color_at(Some(off))),
                ..Style::default()
            };
            off += g.len();
            if g == "\t" {
                let n = TAB_WIDTH - col % TAB_WIDTH;
                segs.extend((0..n).map(|_| seg(" ", style)));
                col += n;
                continue;
            }
            let s = seg(g, style);
            col += s.width;
            segs.push(s);
        }
        segs
    }

    fn raw_cell(&mut self, span: &CellSpan) {
        let style = Style {
            fg: Some(self.config.theme_dim()),
            ..Style::default()
        };
        if span.body.is_empty() {
            self.push(Vec::new(), false, None);
        }
        for i in span.body.clone() {
            let (text, _) = self.line(i);
            self.push(plain_segs(&text, style), false, None);
        }
    }

    fn markdown_cell(&mut self, span: &CellSpan) {
        let mut lines: Vec<(String, Option<usize>)> = span
            .body
            .clone()
            .map(|i| {
                let (text, byte) = self.line(i);
                (text, Some(byte))
            })
            .collect();
        let source: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
        if let Some(md) = html_to_markdown(&source.join("\n")) {
            lines = md.lines().map(|l| (l.to_string(), None)).collect();
        }
        // A blank line ahead of the cell keeps its first line from reading
        // as the top of a file, where `---` opens frontmatter.
        let texts: Vec<String> = std::iter::once(String::new())
            .chain(lines.iter().map(|(t, _)| t.clone()))
            .collect();
        let metas = crate::markdown_render::compute_buffer_meta(&texts);
        let start = self.rows.len();
        let mut blank_run = true;
        for ((text, byte), meta) in lines.iter().zip(metas.iter().skip(1)) {
            // Markdown renders a run of blank lines as one paragraph break,
            // and none at the cell's edges.
            let blank = text.trim().is_empty() && meta.kind == MarkdownLineKind::Default;
            if blank {
                if !blank_run {
                    self.push(Vec::new(), true, None);
                }
                blank_run = true;
                continue;
            }
            blank_run = false;
            if let Some((alt, src)) = image_ref(text) {
                if !self.markdown_image(src) {
                    let style = Style {
                        fg: Some(self.config.theme_dim()),
                        italic: true,
                        ..Style::default()
                    };
                    let name = if alt.trim().is_empty() { src } else { alt };
                    self.push(plain_segs(&format!("[image: {name}]"), style), true, None);
                }
                continue;
            }
            self.markdown_line(text, *byte, meta);
        }
        while self.rows.len() > start
            && self
                .rows
                .last()
                .is_some_and(|r| r.segs.is_empty() && r.fill.is_none() && r.image.is_none())
        {
            self.rows.pop();
        }
        if self.rows.len() == start {
            let style = Style {
                fg: Some(self.config.theme_dim()),
                italic: true,
                ..Style::default()
            };
            self.push(plain_segs("empty markdown cell", style), true, None);
        }
    }

    /// Rows for the image `key` names, read by `load` the first time; false
    /// when images aren't drawn or it doesn't decode.
    fn image(&mut self, key: &str, load: impl FnOnce() -> Option<Vec<u8>>) -> bool {
        let Some(store) = self.images else { return false };
        let mut store = store.borrow_mut();
        let Some((w, h)) = store.size(key, load) else { return false };
        let (cols, rows) = crate::graphics::fit(w, h, self.width);
        let id = store.id(key, cols, rows);
        for row in 0..rows {
            self.rows.push(PageRow {
                cell: Some(self.cell),
                label: None,
                cont: false,
                fill: None,
                indent: 0,
                segs: Vec::new(),
                image: Some(ImageRow { id, row, cols }),
            });
        }
        true
    }

    /// A markdown image: a file beside the notebook, or a `data:` URI. Not
    /// one on the web, or a notebook attachment.
    fn markdown_image(&mut self, src: &str) -> bool {
        if let Some(data) = src.strip_prefix("data:image/") {
            let Some((_, b64)) = data.split_once(";base64,") else { return false };
            let key = crate::graphics::content_key([b64]);
            return self.image(&key, || crate::notebook::base64_decode(b64));
        }
        if src.contains("://") || src.starts_with("attachment:") {
            return false;
        }
        let path = match &self.dir {
            Some(dir) => dir.join(src),
            None => PathBuf::from(src),
        };
        let Ok(meta) = std::fs::metadata(&path) else { return false };
        if !meta.is_file() || meta.len() > crate::graphics::MAX_FILE {
            return false;
        }
        // The time keys it too, so a file written again is read again.
        let key = format!("{}@{:?}", path.display(), meta.modified().ok());
        self.image(&key, || std::fs::read(&path).ok())
    }

    /// An image a cell printed, in place of its note.
    fn output_image(&mut self, out: &Value) -> bool {
        let Some(data) = out.get("data").and_then(Value::as_object) else {
            return false;
        };
        let Some(v) = ["image/png", "image/jpeg", "image/gif"]
            .iter()
            .find_map(|m| data.get(*m))
        else {
            return false;
        };
        let parts: Vec<&str> = match v {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => return false,
        };
        let key = crate::graphics::content_key(parts.iter().copied());
        self.image(&key, || crate::notebook::base64_decode(&parts.concat()))
    }

    fn markdown_line(&mut self, text: &str, byte: Option<usize>, meta: &MarkdownLineMeta) {
        let config = self.config;
        match meta.kind {
            MarkdownLineKind::Hidden => {}
            MarkdownLineKind::HorizontalRule => {
                let style = Style {
                    fg: Some(config.theme_dim()),
                    ..Style::default()
                };
                let rule = "─".repeat(self.width);
                self.push(plain_segs(&rule, style), false, None);
            }
            MarkdownLineKind::Table(TableRowKind::Header) => {
                self.replacement_row(meta, config.theme_emphasis(), true);
            }
            MarkdownLineKind::Table(TableRowKind::Separator) => {
                self.replacement_row(meta, config.theme_dim(), false);
            }
            MarkdownLineKind::Table(TableRowKind::Body) | MarkdownLineKind::CellHeader => {
                self.replacement_row(meta, config.theme_fg(), false);
            }
            MarkdownLineKind::HtmlSummary => {
                self.replacement_row(meta, config.theme_accent(), true);
            }
            MarkdownLineKind::CodeBlock => {
                let segs = self.markdown_segs(text, byte, meta);
                self.push(segs, false, Some(config.theme_code_bg()));
            }
            MarkdownLineKind::Default => {
                let segs = self.markdown_segs(text, byte, meta);
                self.push(segs, true, None);
            }
        }
    }

    /// A table row or `<summary>` the markdown pass drew whole. Cut, not
    /// wrapped, since a table's rows only line up at their own width.
    fn replacement_row(&mut self, meta: &MarkdownLineMeta, fg: Color, bold: bool) {
        let Some(text) = meta.replacement.as_deref() else {
            return;
        };
        let style = Style {
            fg: Some(fg),
            bold,
            ..Style::default()
        };
        let mut segs = plain_segs(text, style);
        let mut used = 0;
        segs.retain(|s| {
            used += s.width;
            used <= self.width
        });
        self.rows.push(PageRow {
            cell: Some(self.cell),
            label: None,
            cont: false,
            fill: None,
            indent: 0,
            segs,
            image: None,
        });
    }

    /// A markdown line with its markers hidden or swapped for glyphs and
    /// its emphasis applied, as the concealed view of a `.md` file draws it.
    fn markdown_segs(&self, text: &str, byte: Option<usize>, meta: &MarkdownLineMeta) -> Vec<Seg> {
        let mut segs = Vec::new();
        let mut col = 0;
        let mut off = 0;
        let mut active: Option<usize> = None;
        for g in text.graphemes(true) {
            let here = col;
            col += g.chars().count();
            let at = byte.map(|b| b + off);
            off += g.len();
            if active.is_some_and(|end| here < end) {
                continue;
            }
            active = None;
            if let Some(t) = meta.transforms.iter().find(|t| t.start == here) {
                if let ConcealAction::Replace { glyph, color } = &t.action {
                    let style = Style {
                        fg: Some(*color),
                        ..Style::default()
                    };
                    segs.extend(glyph.graphemes(true).map(|g| Seg {
                        marker: true,
                        ..seg(g, style)
                    }));
                }
                if t.end > here + g.chars().count() {
                    active = Some(t.end);
                }
                continue;
            }
            let range = crate::markdown_render::style_at(meta, here);
            let style = Style {
                fg: Some(
                    range
                        .and_then(|r| r.color)
                        .unwrap_or_else(|| self.color_at(at)),
                ),
                bold: range.is_some_and(|r| r.bold),
                italic: range.is_some_and(|r| r.italic),
                underline: range.is_some_and(|r| r.underline),
                strike: range.is_some_and(|r| r.strikethrough),
            };
            if g == "\t" {
                segs.extend((0..TAB_WIDTH).map(|_| seg(" ", style)));
            } else {
                segs.push(seg(g, style));
            }
        }
        segs
    }
}

/// Tags that open a block of HTML in a markdown cell, the way CommonMark
/// starts an HTML block. `<details>` / `<summary>` aren't among them: the
/// markdown pass draws those itself.
const HTML_BLOCKS: &[&str] = &[
    "blockquote",
    "center",
    "div",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "img",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "ul",
];

/// A markdown cell written in HTML — a notebook's banner table, a centred
/// image — as the markdown it would render like, since the page can't lay
/// out HTML: tags become line breaks, headings, emphasis and links, an
/// image a note naming it, and the text loses HTML's indentation. `None`
/// when no line opens an HTML block, or the cell has a code fence an HTML
/// sample may sit in.
fn html_to_markdown(src: &str) -> Option<String> {
    let opens_block = |line: &str| {
        let Some(rest) = line.trim_start().strip_prefix('<') else {
            return false;
        };
        let name: String = rest
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        HTML_BLOCKS.contains(&name.as_str())
    };
    if !src.lines().any(opens_block) || src.contains("```") || src.contains("~~~") {
        return None;
    }
    let mut out = String::new();
    let mut depth = 0usize;
    let mut skip: Option<String> = None;
    let mut pre = false;
    let mut href: Vec<String> = Vec::new();
    let mut lists: Vec<Option<usize>> = Vec::new();
    let mut rest = src;
    while !rest.is_empty() {
        let tag = rest
            .strip_prefix('<')
            .filter(|r| r.starts_with(|c: char| c.is_ascii_alphabetic() || c == '/' || c == '!'))
            .and_then(|r| {
                if let Some(c) = r.strip_prefix("!--") {
                    let end = c.find("-->")?;
                    return Some((None, 1 + 3 + end + 3));
                }
                let end = tag_end(r)?;
                Some((Some(&r[..end]), end + 2))
            });
        if let Some((body, len)) = tag {
            rest = &rest[len..];
            let Some(body) = body else { continue };
            let closing = body.starts_with('/');
            let body = body.trim_start_matches('/');
            let name: String = body
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect::<String>()
                .to_ascii_lowercase();
            if let Some(until) = &skip {
                if closing && *until == name {
                    skip = None;
                }
                continue;
            }
            let block = |out: &mut String, s: &str| {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(s);
            };
            match (name.as_str(), closing) {
                ("script" | "style", false) => skip = Some(name.clone()),
                ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                    let level = (name.as_bytes()[1] - b'0') as usize;
                    block(&mut out, &format!("\n{} ", "#".repeat(level)));
                }
                ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => block(&mut out, "\n"),
                ("br", _) => out.push('\n'),
                ("hr", _) => block(&mut out, "\n---\n\n"),
                ("img", _) => {
                    let alt = attr(body, "alt")
                        .unwrap_or_default()
                        .replace(['[', ']'], "");
                    let src = attr(body, "src").unwrap_or_default();
                    let src = if src.contains(|c: char| c.is_whitespace() || c == ')') {
                        format!("<{src}>")
                    } else {
                        src
                    };
                    block(&mut out, &format!("![{}]({src})\n", alt.trim()));
                }
                ("pre", false) => {
                    pre = true;
                    block(&mut out, "\n");
                }
                ("pre", true) => {
                    pre = false;
                    block(&mut out, "\n");
                }
                ("ul", false) => lists.push(None),
                ("ol", false) => lists.push(Some(0)),
                ("ul" | "ol", true) => {
                    lists.pop();
                    block(&mut out, "\n");
                }
                ("li", false) => {
                    let indent = "  ".repeat(lists.len().saturating_sub(1));
                    let marker = match lists.last_mut() {
                        Some(Some(n)) => {
                            *n += 1;
                            format!("{n}.")
                        }
                        _ => "-".to_string(),
                    };
                    block(&mut out, &format!("{indent}{marker} "));
                }
                ("strong" | "b", _) => out.push_str("**"),
                ("em" | "i", _) => out.push('*'),
                ("code", _) => out.push('`'),
                ("s" | "del" | "strike", _) => out.push_str("~~"),
                ("a", false) => {
                    href.push(attr(body, "href").unwrap_or_default());
                    out.push('[');
                }
                ("a", true) => {
                    let target = href.pop().unwrap_or_default();
                    out.push_str(&format!("]({target})"));
                }
                _ if HTML_BLOCKS.contains(&name.as_str())
                    || matches!(
                        name.as_str(),
                        "tr" | "td" | "th" | "thead" | "tbody" | "li" | "details" | "summary"
                    ) =>
                {
                    if closing {
                        depth = depth.saturating_sub(1);
                    } else if !body.ends_with('/') {
                        depth += 1;
                    }
                    block(&mut out, "");
                }
                _ => {}
            }
            continue;
        }
        let end = rest[1..].find('<').map_or(rest.len(), |i| i + 1);
        let text = decode_entities(&rest[..end]);
        rest = &rest[end..];
        if skip.is_some() {
            continue;
        }
        if depth == 0 || pre {
            out.push_str(&text);
            continue;
        }
        // HTML's whitespace: a run is one space, but a blank line still ends
        // a paragraph, as it ends the HTML block in Jupyter's markdown.
        let mut newlines = 0;
        let mut gap = false;
        for c in text.chars() {
            if c.is_whitespace() {
                gap = true;
                newlines += (c == '\n') as usize;
                continue;
            }
            if gap {
                push_gap(&mut out, newlines);
            }
            out.push(c);
            gap = false;
            newlines = 0;
        }
        if gap {
            push_gap(&mut out, newlines);
        }
    }
    let mut md = String::new();
    let mut blank = true;
    for line in out.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            if !blank {
                md.push('\n');
            }
            blank = true;
            continue;
        }
        blank = false;
        md.push_str(line);
        md.push('\n');
    }
    while md.ends_with("\n\n") {
        md.pop();
    }
    Some(md)
}

/// The alt text and source of a line that is only a markdown image,
/// `![alt](src "title")`.
fn image_ref(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim().strip_prefix("![")?;
    let (alt, rest) = rest.split_once("](")?;
    let target = rest.strip_suffix(')')?.trim();
    let src = match target.strip_prefix('<') {
        Some(t) => t.split_once('>')?.0,
        None => target.split_whitespace().next()?,
    };
    Some((alt, src))
}

/// A gap of `newlines` in HTML text: a space, or a paragraph break for a
/// blank line. Nothing at the start of a line, so indentation goes.
fn push_gap(out: &mut String, newlines: usize) {
    if newlines >= 2 {
        out.push_str("\n\n");
    } else if !out.is_empty() && !out.ends_with(['\n', ' ']) {
        out.push(' ');
    }
}

/// Where the tag `r` (past its `<`) ends, with `>` inside a quoted
/// attribute value passed over.
fn tag_end(r: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in r.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

/// The value of attribute `name` in a tag's body, quoted or not.
fn attr(body: &str, name: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let before = lower[..at].chars().next_back();
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let rest = body[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        return Some(match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or("").to_string(),
            _ => rest
                .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
                .next()
                .unwrap_or("")
                .to_string(),
        });
    }
    None
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let entity = rest[1..]
            .find(';')
            .filter(|&n| n <= 10)
            .map(|n| &rest[1..=n]);
        let ch = entity.and_then(|e| match e {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => {
                let n = e.strip_prefix('#')?;
                let code = match n.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => n.parse().ok()?,
                };
                char::from_u32(code)
            }
        });
        match (ch, entity) {
            (Some(c), Some(e)) => {
                out.push(c);
                rest = &rest[e.len() + 2..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn seg(g: &str, style: Style) -> Seg {
    Seg {
        text: g.to_string(),
        width: cluster_width(g, TAB_WIDTH),
        style,
        marker: false,
        link: None,
    }
}

/// Underlines each URL in `segs` and links its clusters to it, so a click
/// on `Running on local URL:  http://127.0.0.1:7860` opens the server. A
/// URL is a blank-free run with a `://`, less the brackets and punctuation
/// around it, as `gx` reads one.
fn link_urls(segs: &mut [Seg]) {
    let blank = |s: &Seg| s.text.trim().is_empty();
    let mut i = 0;
    while i < segs.len() {
        if blank(&segs[i]) {
            i += 1;
            continue;
        }
        let end = (i..segs.len())
            .find(|&j| blank(&segs[j]))
            .unwrap_or(segs.len());
        let mut start = i;
        while start < end && matches!(segs[start].text.as_str(), "(" | "<" | "[" | "\"" | "'") {
            start += 1;
        }
        let mut stop = end;
        while stop > start
            && matches!(
                segs[stop - 1].text.as_str(),
                "." | "," | ";" | ":" | "!" | "?" | ")" | "]" | "}" | ">" | "\"" | "'"
            )
        {
            stop -= 1;
        }
        let url: String = segs[start..stop].iter().map(|s| s.text.as_str()).collect();
        if url.contains("://") {
            let link: Arc<str> = url.into();
            for s in &mut segs[start..stop] {
                s.link = Some(link.clone());
                s.style.underline = true;
            }
        }
        i = end;
    }
}

fn plain_segs(text: &str, style: Style) -> Vec<Seg> {
    text.graphemes(true)
        .map(|g| {
            if g == "\t" {
                seg(" ", style)
            } else {
                seg(g, style)
            }
        })
        .collect()
}

/// Columns a wrapped list item or quote hangs its later rows at: past the
/// leading spaces and the marker glyph with the spaces after it.
fn hang(segs: &[Seg]) -> usize {
    let mut i = segs.iter().take_while(|s| s.text == " ").count();
    let lead = i;
    while segs.get(i).is_some_and(|s| s.marker) {
        i += 1;
    }
    if i == lead {
        return 0;
    }
    while segs.get(i).is_some_and(|s| s.text == " ") {
        i += 1;
    }
    segs[..i].iter().map(|s| s.width).sum()
}

/// `segs` cut into rows of at most `width` columns, each with the blank
/// columns it starts at. Prose (`words`) breaks after a space where it can
/// and drops the space it broke at; code breaks at the column, so every
/// character stays where it can be counted.
fn wrap(segs: Vec<Seg>, width: usize, words: bool) -> Vec<(usize, Vec<Seg>)> {
    let hang = if words { hang(&segs) } else { 0 };
    let hang = if hang * 2 > width { 0 } else { hang };
    let mut out = Vec::new();
    let mut row: Vec<Seg> = Vec::new();
    let mut indent = 0;
    let mut used = 0;
    for s in segs {
        if used + s.width > width - indent && !row.is_empty() {
            let mut carry = Vec::new();
            if words && s.text != " " {
                let min = if out.is_empty() { hang.max(1) } else { 1 };
                if let Some(sp) = row.iter().rposition(|s| s.text == " ") {
                    if sp >= min {
                        carry = row.split_off(sp + 1);
                        row.pop();
                    }
                }
            }
            out.push((indent, row));
            indent = hang;
            row = carry;
            used = row.iter().map(|s| s.width).sum();
            if words && row.is_empty() && s.text == " " {
                continue;
            }
        }
        used += s.width;
        row.push(s);
    }
    out.push((indent, row));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(row: &PageRow) -> String {
        let segs: String = row.segs.iter().map(|s| s.text.as_str()).collect();
        format!("{}{segs}", " ".repeat(row.indent))
    }

    fn notebook(name: &str, cells: &str) -> Buffer {
        let dir = crate::paths::test_scratch_dir("notebook_page", name);
        let path = dir.join("nb.ipynb");
        std::fs::write(
            &path,
            format!(
                r#"{{"cells": [{cells}], "metadata": {{}}, "nbformat": 4, "nbformat_minor": 5}}"#
            ),
        )
        .unwrap();
        let buf = Buffer::from_path(path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        buf
    }

    #[test]
    fn prose_wraps_at_words_and_a_list_item_hangs_under_its_text() {
        let style = Style::default();
        let rows = |s: &str, w| -> Vec<String> {
            wrap(plain_segs(s, style), w, true)
                .into_iter()
                .map(|(i, segs)| {
                    " ".repeat(i) + &segs.iter().map(|s| s.text.as_str()).collect::<String>()
                })
                .collect()
        };
        assert_eq!(rows("one two three four", 9), ["one two", "three", "four"]);
        assert_eq!(rows("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        let mut item = vec![Seg {
            marker: true,
            ..seg("•", style)
        }];
        item.extend(plain_segs(" alpha beta gamma", style));
        let wrapped: Vec<(usize, String)> = wrap(item, 12, true)
            .into_iter()
            .map(|(i, s)| (i, s.iter().map(|s| s.text.as_str()).collect()))
            .collect();
        assert_eq!(wrapped, [(0, "• alpha beta".into()), (2, "gamma".into())]);
    }

    #[test]
    fn code_wraps_at_the_column_and_keeps_its_spaces() {
        let rows: Vec<String> = wrap(plain_segs("    x = 1 + 2", Style::default()), 6, false)
            .into_iter()
            .map(|(_, s)| s.iter().map(|s| s.text.as_str()).collect())
            .collect();
        assert_eq!(rows, ["    x ", "= 1 + ", "2"]);
    }

    #[test]
    fn images_draw_where_the_terminal_can_and_are_named_where_it_cannot() {
        const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
        let buf = notebook(
            "images",
            &format!(
                r##"{{"cell_type": "markdown", "id": "m", "metadata": {{}}, "source": "![dot](data:image/png;base64,{PNG})\n![logo](https://x.dev/logo.png)"}},
                  {{"cell_type": "code", "execution_count": 1, "id": "c", "metadata": {{}}, "outputs": [{{"output_type": "display_data", "metadata": {{}}, "data": {{"image/png": "{PNG}", "text/plain": "<Figure>"}}}}], "source": "plot()"}}"##
            ),
        );
        let store = RefCell::new(ImageStore::default());
        let page = layout(&buf, None, 60, &Config::default(), Some(&store));
        let images: Vec<bool> = page.rows.iter().map(|r| r.image.is_some()).collect();
        assert_eq!(images, [true, false, false, false, true]);
        assert_eq!(text(&page.rows[1]), "[image: logo]");
        assert_eq!(
            page.rows[0].image.unwrap().id,
            page.rows[4].image.unwrap().id
        );

        let page = layout(&buf, None, 60, &Config::default(), None);
        let rows: Vec<String> = page.rows.iter().map(text).collect();
        assert_eq!(
            rows,
            [
                "[image: dot]",
                "[image: logo]",
                "",
                "plot()",
                "[image/png] :cell output opens it"
            ]
        );
    }

    #[test]
    fn the_page_shows_cells_as_jupyter_does() {
        let buf = notebook(
            "layout",
            r##"{"cell_type": "markdown", "id": "m", "metadata": {}, "source": "# Title\n\n\n**bold** text\n"},
              {"cell_type": "code", "execution_count": 4, "id": "c", "metadata": {}, "outputs": [{"output_type": "stream", "name": "stdout", "text": "hi\n"}], "source": "print('hi')"},
              {"cell_type": "code", "execution_count": null, "id": "e", "metadata": {}, "outputs": [], "source": ""}"##,
        );
        let page = layout(&buf, None, 60, &Config::default(), None);
        let rows: Vec<String> = page.rows.iter().map(text).collect();
        assert_eq!(
            rows,
            ["Title", "", "bold text", "", "print('hi')", "hi", "", ""]
        );
        assert_eq!(page.cells, [0..3, 4..6, 7..8]);
        assert_eq!(page.rows[4].label, Some(("▶ [4]".into(), LabelKind::In)));
        assert!(page.rows[4].fill.is_some(), "code sits on a slab");
        assert!(page.rows[5].fill.is_none(), "output doesn't");
        assert_eq!(page.rows[7].label, Some(("▶ [ ]".into(), LabelKind::In)));
        assert!(page.rows[2].segs[0].style.bold);
        assert_eq!(page.cell_at_row(3), Some(1), "a gap belongs below");
        assert_eq!(page.cell_of_line(7), Some(1));
        assert_eq!(page.cell_line(1), Some(7));
        assert_eq!(page.cell_line(2), Some(9));
    }

    #[test]
    fn an_html_cell_reads_as_the_markdown_it_would_render_like() {
        let cell = r#"<table style="margin: 0; text-align: left;">
    <tr>
        <td style="width: 150px; height: 150px;">
            <img src="../assets/business.jpg" width="150" height="150" />
        </td>
        <td>
            <h2 style="color:#181;">Business Applications</h2>
            <span style="color:#181;">Gradio makes it <b>easy</b> &amp; quick.

Consider how you could <a href="https://x.dev">apply</a> it.</span>
        </td>
    </tr>
</table>"#;
        assert_eq!(
            html_to_markdown(cell).as_deref(),
            Some(
                "![](../assets/business.jpg)\n\n## Business Applications\n\n\
                 Gradio makes it **easy** & quick.\n\n\
                 Consider how you could [apply](https://x.dev) it.\n"
            )
        );
        assert_eq!(html_to_markdown("Some <b>bold</b> prose"), None);
        assert_eq!(html_to_markdown("```html\n<div>x</div>\n```"), None);
        assert_eq!(
            html_to_markdown("<ol><li>one</li><li>two</li></ol><hr>").as_deref(),
            Some("1. one\n2. two\n\n---\n")
        );
    }

    #[test]
    fn a_printed_url_is_underlined_and_linked() {
        let mut segs = plain_segs("* Running on (http://127.0.0.1:7861).", Style::default());
        link_urls(&mut segs);
        let linked: String = segs
            .iter()
            .filter(|s| s.link.as_deref() == Some("http://127.0.0.1:7861"))
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(linked, "http://127.0.0.1:7861");
        assert!(segs.iter().all(|s| s.style.underline == s.link.is_some()));
    }

    #[test]
    fn scrolling_keeps_the_marked_cell_on_screen_without_pinning_it() {
        let buf = notebook(
            "scroll",
            r#"{"cell_type": "code", "execution_count": null, "id": "a", "metadata": {}, "outputs": [], "source": "1\n2\n3\n4\n5\n6"},
              {"cell_type": "code", "execution_count": null, "id": "b", "metadata": {}, "outputs": [], "source": "x"}"#,
        );
        let page = layout(&buf, None, 60, &Config::default(), None);
        assert_eq!(page.cells, [0..6, 7..8]);
        // Revealing the second cell from the top brings its row to the
        // bottom of a 4-row pane; revealing the tall first cell shows its top.
        assert_eq!(page.reveal(1, 0, 4), 4);
        assert_eq!(page.reveal(0, 4, 4), 0);
        // Scrolled into the middle of the first cell, it stays put.
        assert_eq!(page.keep_in_view(0, 2, 4), 2);
        assert_eq!(page.keep_in_view(1, 0, 4), 4);
        assert_eq!(page.visible_cell(0, 4, 4, false), Some(0));
        assert_eq!(page.visible_cell(0, 6, 4, false), Some(1));
    }
}
