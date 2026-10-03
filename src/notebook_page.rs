//! The notebook page: a notebook buffer drawn the way Jupyter draws it —
//! markdown rendered and wrapped, code on a slab under its `In [n]:` label,
//! outputs beneath — in place of the percent text, where every cell header,
//! line number and markdown marker competes with the content.
//!
//! The buffer stays the source of truth. The page is laid out from it on
//! each frame, and the cell it marks is the one holding the cursor, so the
//! text view (`Enter` on the page, `<leader>nv` back) keeps the place, and
//! undo, the kernel and saves need nothing of their own.

use std::ops::Range;

use crossterm::style::Color;
use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::Buffer;
use crate::config::Config;
use crate::markdown_render::{ConcealAction, MarkdownLineKind, MarkdownLineMeta, TableRowKind};
use crate::notebook::{CellKind, CellSpan, OutputStyle};
use crate::render::{TAB_WIDTH, cluster_width};

/// Wide enough for `In [999]:`; a longer label loses its left end.
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelKind {
    In,
    /// Sent to the kernel and not finished — Jupyter's `In [*]:`.
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

pub fn layout(
    buffer: &Buffer,
    colors: Option<&[Option<Color>]>,
    width: usize,
    config: &Config,
) -> PageLayout {
    let gutter = if width >= NARROW { GUTTER } else { 2 };
    let content_w = width.saturating_sub(gutter + 1).max(1);
    let spans = crate::notebook::cell_spans(&buffer.rope);
    let mut page = Page {
        buffer,
        colors,
        config,
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

    fn color_at(&self, byte: usize) -> Color {
        self.colors
            .and_then(|c| c.get(byte).copied().flatten())
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
            _ if busy => ("In [*]:".to_string(), LabelKind::Busy),
            Some(n) => (format!("In [{n}]:"), LabelKind::In),
            None => ("In [ ]:".to_string(), LabelKind::In),
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
        let outputs = match (id, doc) {
            (Some(id), Some(doc)) => crate::notebook::output_rows(doc.outputs(id)),
            _ => Vec::new(),
        };
        for out in outputs {
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
            let segs = plain_segs(&out.text, style);
            self.push(segs, false, None);
        }
    }

    fn code_segs(&self, text: &str, byte: usize) -> Vec<Seg> {
        let mut segs = Vec::new();
        let mut col = 0;
        let mut off = byte;
        for g in text.graphemes(true) {
            let style = Style {
                fg: Some(self.color_at(off)),
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
        let lines: Vec<(String, usize)> = span.body.clone().map(|i| self.line(i)).collect();
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
            self.markdown_line(text, *byte, meta);
        }
        while self.rows.len() > start
            && self
                .rows
                .last()
                .is_some_and(|r| r.segs.is_empty() && r.fill.is_none())
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

    fn markdown_line(&mut self, text: &str, byte: usize, meta: &MarkdownLineMeta) {
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
        });
    }

    /// A markdown line with its markers hidden or swapped for glyphs and
    /// its emphasis applied, as the concealed view of a `.md` file draws it.
    fn markdown_segs(&self, text: &str, byte: usize, meta: &MarkdownLineMeta) -> Vec<Seg> {
        let mut segs = Vec::new();
        let mut col = 0;
        let mut off = byte;
        let mut active: Option<usize> = None;
        for g in text.graphemes(true) {
            let here = col;
            col += g.chars().count();
            let at = off;
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

fn seg(g: &str, style: Style) -> Seg {
    Seg {
        text: g.to_string(),
        width: cluster_width(g, TAB_WIDTH),
        style,
        marker: false,
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
    fn the_page_shows_cells_as_jupyter_does() {
        let buf = notebook(
            "layout",
            r##"{"cell_type": "markdown", "id": "m", "metadata": {}, "source": "# Title\n\n\n**bold** text\n"},
              {"cell_type": "code", "execution_count": 4, "id": "c", "metadata": {}, "outputs": [{"output_type": "stream", "name": "stdout", "text": "hi\n"}], "source": "print('hi')"},
              {"cell_type": "code", "execution_count": null, "id": "e", "metadata": {}, "outputs": [], "source": ""}"##,
        );
        let page = layout(&buf, None, 60, &Config::default());
        let rows: Vec<String> = page.rows.iter().map(text).collect();
        assert_eq!(
            rows,
            ["Title", "", "bold text", "", "print('hi')", "hi", "", ""]
        );
        assert_eq!(page.cells, [0..3, 4..6, 7..8]);
        assert_eq!(page.rows[4].label, Some(("In [4]:".into(), LabelKind::In)));
        assert!(page.rows[4].fill.is_some(), "code sits on a slab");
        assert!(page.rows[5].fill.is_none(), "output doesn't");
        assert_eq!(page.rows[7].label, Some(("In [ ]:".into(), LabelKind::In)));
        assert!(page.rows[2].segs[0].style.bold);
        assert_eq!(page.cell_at_row(3), Some(1), "a gap belongs below");
        assert_eq!(page.cell_of_line(7), Some(1));
        assert_eq!(page.cell_line(1), Some(7));
        assert_eq!(page.cell_line(2), Some(9));
    }

    #[test]
    fn scrolling_keeps_the_marked_cell_on_screen_without_pinning_it() {
        let buf = notebook(
            "scroll",
            r#"{"cell_type": "code", "execution_count": null, "id": "a", "metadata": {}, "outputs": [], "source": "1\n2\n3\n4\n5\n6"},
              {"cell_type": "code", "execution_count": null, "id": "b", "metadata": {}, "outputs": [], "source": "x"}"#,
        );
        let page = layout(&buf, None, 60, &Config::default());
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
