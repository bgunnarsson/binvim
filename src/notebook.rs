//! Jupyter notebooks (`.ipynb`) as editable text.
//!
//! A notebook is projected into "percent" text — one header line per cell
//! (`# %% id=…`, `# %% [markdown] id=…`, `# %% [raw] id=…`) followed by the
//! cell's source — and the buffer edits that text. The original JSON stays on
//! the buffer as a `NotebookDoc`; on save, `serialize` matches the text's cells
//! back to the originals by the id in each header, so metadata, attachments
//! and outputs survive edits they were never shown in.
//!
//! The id lives in the text rather than in side state so that undo, recovery
//! dumps, persisted undo history and yank/paste of whole cells all carry cell
//! identity for free — every cell operation is an ordinary text edit.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use ropey::Rope;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Code,
    Markdown,
    Raw,
}

impl CellKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CellKind::Code => "code",
            CellKind::Markdown => "markdown",
            CellKind::Raw => "raw",
        }
    }

    fn from_cell_type(s: &str) -> Option<Self> {
        match s {
            "code" => Some(CellKind::Code),
            "markdown" => Some(CellKind::Markdown),
            "raw" => Some(CellKind::Raw),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub kind: CellKind,
    pub id: Option<String>,
}

/// One cell's place in the buffer, in line numbers. `header` is `None` only
/// for text typed above the first header, which saves as a new code cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellSpan {
    pub header: Option<usize>,
    pub body: Range<usize>,
    pub kind: CellKind,
    pub id: Option<String>,
}

impl CellSpan {
    /// First line of the cell, header included.
    pub fn start(&self) -> usize {
        self.header.unwrap_or(self.body.start)
    }
}

/// The notebook as it was read from disk. Kept on the buffer so a save can
/// write back everything the text doesn't show.
#[derive(Debug, Clone)]
pub struct NotebookDoc {
    original: Vec<u8>,
    root: Map<String, Value>,
    cells: Vec<Value>,
    by_id: HashMap<String, usize>,
    /// nbformat ≥ 4.5 — cells carry an `id` field.
    has_ids: bool,
}

/// Ids made up for cells that have none on disk (nbformat < 4.5). `~` can't
/// appear in an nbformat id, so they're never mistaken for one and never
/// written to the JSON.
const SYNTHETIC_PREFIX: char = '~';

const HEADER_PREFIX: &str = "# %%";

pub fn header_line(kind: CellKind, id: Option<&str>) -> String {
    let mut out = String::from(HEADER_PREFIX);
    match kind {
        CellKind::Code => {}
        CellKind::Markdown => out.push_str(" [markdown]"),
        CellKind::Raw => out.push_str(" [raw]"),
    }
    if let Some(id) = id {
        out.push_str(" id=");
        out.push_str(id);
    }
    out
}

/// A header is exactly `# %%`, an optional `[markdown]`/`[md]`/`[raw]` and an
/// optional `id=…`. Anything else after `# %%` (a jupytext cell title, say) is
/// ordinary source, so a converted script's comments don't split cells.
pub fn parse_header(line: &str) -> Option<Header> {
    let rest = line.strip_prefix(HEADER_PREFIX)?.trim_end();
    let (kind, rest) = if let Some(r) = rest
        .strip_prefix(" [markdown]")
        .or_else(|| rest.strip_prefix(" [md]"))
    {
        (CellKind::Markdown, r)
    } else if let Some(r) = rest.strip_prefix(" [raw]") {
        (CellKind::Raw, r)
    } else {
        (CellKind::Code, rest)
    };
    if rest.is_empty() {
        return Some(Header { kind, id: None });
    }
    let id = rest.strip_prefix(" id=")?;
    if id.is_empty() || id.chars().any(char::is_whitespace) {
        return None;
    }
    Some(Header {
        kind,
        id: Some(id.to_string()),
    })
}

fn source_text(cell: &Value) -> String {
    match cell.get("source") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

/// nbformat's own on-disk form: a list of lines, each keeping its newline.
fn source_value(text: &str) -> Value {
    Value::Array(
        text.split_inclusive('\n')
            .map(|l| Value::String(l.to_string()))
            .collect(),
    )
}

fn cell_kind(cell: &Value) -> Option<CellKind> {
    cell.get("cell_type")
        .and_then(Value::as_str)
        .and_then(CellKind::from_cell_type)
}

pub fn project(json: &str) -> Result<(String, NotebookDoc), String> {
    let value: Value = serde_json::from_str(json).map_err(|e| format!("invalid JSON: {e}"))?;
    let Value::Object(mut root) = value else {
        return Err("not a notebook: top level is not an object".into());
    };
    let major = root.get("nbformat").and_then(Value::as_u64);
    if major != Some(4) {
        return Err(match major {
            Some(v) => format!("nbformat {v} is not supported (only 4)"),
            None => "not a notebook: no nbformat version".into(),
        });
    }
    let minor = root
        .get("nbformat_minor")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let Some(Value::Array(cells)) = root.remove("cells") else {
        return Err("not a notebook: no cells list".into());
    };
    let mut text = String::new();
    let mut by_id = HashMap::new();
    for (i, cell) in cells.iter().enumerate() {
        let Some(kind) = cell_kind(cell) else {
            return Err(format!("cell {} has no known cell_type", i + 1));
        };
        let id = match cell.get("id").and_then(Value::as_str) {
            Some(id) if parse_header(&header_line(kind, Some(id))).is_some() => id.to_string(),
            _ => format!("{SYNTHETIC_PREFIX}{i}"),
        };
        // A later duplicate on disk loses its identity rather than shadowing
        // the first; it saves as a copy of that cell.
        by_id.entry(id.clone()).or_insert(i);
        let source = source_text(cell);
        if source.split('\n').any(|l| parse_header(l).is_some()) {
            return Err(format!(
                "cell {} contains a line that reads as a cell header",
                i + 1
            ));
        }
        text.push_str(&header_line(kind, Some(&id)));
        text.push('\n');
        text.push_str(&source);
        text.push('\n');
    }
    Ok((
        text,
        NotebookDoc {
            original: json.as_bytes().to_vec(),
            root,
            cells,
            by_id,
            has_ids: minor >= 5,
        },
    ))
}

/// A notebook that doesn't exist on disk yet — what `:e new.ipynb` edits.
pub fn empty_notebook() -> NotebookDoc {
    let json =
        "{\n \"cells\": [],\n \"metadata\": {},\n \"nbformat\": 4,\n \"nbformat_minor\": 5\n}\n";
    let (_, mut doc) = project(json).expect("the skeleton is a valid notebook");
    // Nothing on disk to reuse: the first save must always serialize.
    doc.original.clear();
    doc
}

/// Lines of `text` as the buffer numbers them, minus the empty line ropey
/// counts after a final newline.
fn text_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

fn spans_from(headers: &[Option<Header>], blank: impl Fn(usize) -> bool) -> Vec<CellSpan> {
    let n = headers.len();
    let mut spans = Vec::new();
    let first_header = headers.iter().position(Option::is_some).unwrap_or(n);
    if (0..first_header).any(|i| !blank(i)) {
        spans.push(CellSpan {
            header: None,
            body: 0..first_header,
            kind: CellKind::Code,
            id: None,
        });
    }
    let mut i = first_header;
    while i < n {
        let Some(h) = &headers[i] else {
            i += 1;
            continue;
        };
        let end = (i + 1..n).find(|&j| headers[j].is_some()).unwrap_or(n);
        spans.push(CellSpan {
            header: Some(i),
            body: i + 1..end,
            kind: h.kind,
            id: h.id.clone(),
        });
        i = end;
    }
    spans
}

pub fn cell_spans_text(text: &str) -> Vec<CellSpan> {
    let lines = text_lines(text);
    let headers: Vec<Option<Header>> = lines.iter().map(|l| parse_header(l)).collect();
    spans_from(&headers, |i| lines[i].trim().is_empty())
}

pub fn cell_spans(rope: &Rope) -> Vec<CellSpan> {
    let mut n = rope.len_lines();
    if rope.len_chars() > 0 && rope.char(rope.len_chars() - 1) == '\n' {
        n -= 1;
    }
    let line_string = |i: usize| -> String {
        let line = rope.line(i);
        let s: String = line.chars().collect();
        s.trim_end_matches('\n').to_string()
    };
    let headers: Vec<Option<Header>> = (0..n)
        .map(|i| {
            // Only a line starting with `#` can be a header; skip the
            // allocation for the rest.
            if rope.line(i).chars().next() == Some('#') {
                parse_header(&line_string(i))
            } else {
                None
            }
        })
        .collect();
    spans_from(&headers, |i| line_string(i).trim().is_empty())
}

/// The cell whose header or body holds `line`.
pub fn span_at(spans: &[CellSpan], line: usize) -> Option<usize> {
    spans
        .iter()
        .rposition(|s| s.start() <= line)
        .or(if spans.is_empty() { None } else { Some(0) })
}

struct TextCell {
    kind: CellKind,
    id: Option<String>,
    source: String,
}

fn text_cells(text: &str) -> Vec<TextCell> {
    let lines = text_lines(text);
    cell_spans_text(text)
        .into_iter()
        .map(|s| TextCell {
            kind: s.kind,
            id: s.id,
            source: lines[s.body].join("\n"),
        })
        .collect()
}

fn is_nbformat_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn fresh_id(taken: &HashSet<String>) -> String {
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    loop {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            .hash(&mut h);
        std::process::id().hash(&mut h);
        COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut h);
        let id = format!("{:08x}", h.finish() as u32);
        if !taken.contains(&id) {
            return id;
        }
    }
}

/// Make `cell` the shape nbformat's schema wants for `kind`: code cells carry
/// `outputs` and `execution_count` and no `attachments`; markdown and raw
/// cells carry no outputs.
fn set_kind(cell: &mut Map<String, Value>, kind: CellKind) {
    cell.insert("cell_type".into(), Value::String(kind.as_str().into()));
    if kind == CellKind::Code {
        cell.remove("attachments");
        cell.entry("outputs")
            .or_insert_with(|| Value::Array(Vec::new()));
        cell.entry("execution_count").or_insert(Value::Null);
    } else {
        cell.remove("outputs");
        cell.remove("execution_count");
    }
}

impl NotebookDoc {
    /// The cells `text` describes, and which of the text's ids now name which
    /// of them. A cell the text gave no id, or an id an earlier cell already
    /// took, has no entry: on disk it carries a fresh id the text doesn't know.
    fn cells_from_text(&self, text: &str) -> (Vec<Value>, HashMap<String, usize>) {
        let parsed = text_cells(text);
        let mut taken: HashSet<String> = parsed
            .iter()
            .filter_map(|c| c.id.clone())
            .filter(|id| is_nbformat_id(id))
            .collect();
        taken.extend(self.by_id.keys().cloned());
        let mut used: HashSet<String> = HashSet::new();
        let mut out = Vec::with_capacity(parsed.len());
        let mut by_id = HashMap::new();
        for tc in parsed {
            let first_use = tc.id.as_ref().is_some_and(|id| used.insert(id.clone()));
            let orig = tc.id.as_ref().and_then(|id| self.by_id.get(id)).copied();
            let mut cell = match orig {
                Some(i) => match &self.cells[i] {
                    Value::Object(m) => m.clone(),
                    _ => Map::new(),
                },
                None => {
                    let mut m = Map::new();
                    m.insert("metadata".into(), Value::Object(Map::new()));
                    m
                }
            };
            if orig.is_none() || !first_use {
                cell.remove("id");
                if self.has_ids {
                    let id = match &tc.id {
                        Some(id) if first_use && is_nbformat_id(id) => id.clone(),
                        _ => {
                            let id = fresh_id(&taken);
                            taken.insert(id.clone());
                            id
                        }
                    };
                    cell.insert("id".into(), Value::String(id));
                }
            }
            if orig.is_none() || cell_kind(&Value::Object(cell.clone())) != Some(tc.kind) {
                set_kind(&mut cell, tc.kind);
            }
            let cell_value = Value::Object(cell);
            let cell_value = if source_text(&cell_value) == tc.source {
                cell_value
            } else {
                let Value::Object(mut m) = cell_value else { unreachable!() };
                m.insert("source".into(), source_value(&tc.source));
                Value::Object(m)
            };
            if first_use {
                if let Some(id) = tc.id {
                    by_id.insert(id, out.len());
                }
            }
            out.push(cell_value);
        }
        (out, by_id)
    }

    /// The notebook's bytes for `text`. When every cell comes out as it went
    /// in, the file's original bytes are returned untouched, so a save that
    /// changed nothing can't reformat a number Python and serde_json print
    /// differently (`1e-05`).
    pub fn serialize(&self, text: &str) -> Vec<u8> {
        self.save(text).0
    }

    /// `serialize`, plus the doc that describes the file once those bytes are
    /// written. Its ids come from the text, not from re-reading the bytes: a
    /// synthetic `~3` keeps naming the cell it named before a cell was
    /// inserted above it, where a re-read would hand it to another cell.
    pub fn save(&self, text: &str) -> (Vec<u8>, NotebookDoc) {
        let (cells, by_id) = self.cells_from_text(text);
        if cells == self.cells && !self.original.is_empty() {
            let doc = NotebookDoc {
                by_id,
                ..self.clone()
            };
            return (self.original.clone(), doc);
        }
        let mut root = self.root.clone();
        root.insert("cells".into(), Value::Array(cells.clone()));
        // nbformat writes `json.dumps(sort_keys=True, indent=1,
        // ensure_ascii=False)` plus a newline; serde_json's default map is
        // sorted, so a Jupyter-written file comes back byte for byte.
        let mut out = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(b" ");
        let mut ser = serde_json::Serializer::with_formatter(&mut out, fmt);
        let value = Value::Object(root);
        serde::Serialize::serialize(&value, &mut ser).expect("a JSON value always serializes");
        let Value::Object(mut root) = value else { unreachable!() };
        out.push(b'\n');
        root.remove("cells");
        let doc = NotebookDoc {
            original: out.clone(),
            root,
            cells,
            by_id,
            has_ids: self.has_ids,
        };
        (out, doc)
    }

    /// The original source of the cell `id` names, when it has one on disk.
    pub fn original_source(&self, id: &str) -> Option<String> {
        self.by_id.get(id).map(|&i| source_text(&self.cells[i]))
    }

    /// `(execution_count, number of outputs)` of the cell `id` names on disk.
    pub fn cell_info(&self, id: &str) -> Option<(Option<u64>, usize)> {
        let cell = &self.cells[*self.by_id.get(id)?];
        let count = cell.get("execution_count").and_then(Value::as_u64);
        let outputs = cell
            .get("outputs")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        Some((count, outputs))
    }

    pub fn has_ids(&self) -> bool {
        self.has_ids
    }
}

/// Line kinds of `text`: `Some(kind)` for a body line, `None` for a header.
fn line_kinds(text: &str) -> Vec<Option<CellKind>> {
    let n = text_lines(text).len();
    let mut kinds = vec![Some(CellKind::Code); n];
    for span in cell_spans_text(text) {
        if let Some(h) = span.header {
            kinds[h] = None;
        }
        for l in span.body {
            kinds[l] = Some(span.kind);
        }
    }
    kinds
}

/// What the language server is shown: code lines as they are, every other
/// line — headers, markdown, raw, IPython `%magic` and `!shell` lines —
/// emptied. The line count doesn't change, so diagnostics and edits land on
/// buffer lines with no translation.
pub fn lsp_view(text: &str) -> String {
    let kinds = line_kinds(text);
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let keep = match kinds.get(i) {
            Some(Some(CellKind::Code)) => {
                let t = line.trim_start();
                !t.starts_with('%') && !t.starts_with('!')
            }
            Some(_) => false,
            // The empty line after a final newline.
            None => true,
        };
        if keep {
            out.push_str(line);
        }
    }
    out
}

/// `text` with every byte outside `kind`'s cells (headers included) turned
/// into a space, newlines kept. Byte offsets are unchanged, so a highlighter
/// run over it colours exactly the bytes of the buffer it should.
pub fn masked(text: &str, kind: CellKind) -> String {
    let kinds = line_kinds(text);
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if kinds.get(i).copied().flatten() == Some(kind) {
            out.push_str(line);
        } else {
            out.extend(std::iter::repeat_n(' ', line.len()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Written by `json.dumps(nb, sort_keys=True, indent=1,
    /// ensure_ascii=False) + "\n"`, the form `nbformat.write` produces.
    const JUPYTER: &str = r##"{
 "cells": [
  {
   "cell_type": "code",
   "execution_count": 3,
   "id": "a1b2c3d4",
   "metadata": {},
   "outputs": [
    {
     "name": "stdout",
     "output_type": "stream",
     "text": [
      "hi ünïcode\n"
     ]
    }
   ],
   "source": [
    "import os\n",
    "print('hi')"
   ]
  },
  {
   "cell_type": "markdown",
   "id": "e5f6a7b8",
   "metadata": {
    "tags": [
     "intro"
    ]
   },
   "source": [
    "# Title\n",
    "\n",
    "Some **bold**  "
   ]
  },
  {
   "cell_type": "raw",
   "id": "c9d0e1f2",
   "metadata": {},
   "source": []
  }
 ],
 "metadata": {
  "kernelspec": {
   "display_name": "Python 3",
   "language": "python",
   "name": "python3"
  },
  "language_info": {
   "name": "python",
   "version": "3.12.0"
  }
 },
 "nbformat": 4,
 "nbformat_minor": 5
}
"##;

    fn cells(bytes: &[u8]) -> Vec<Value> {
        let v: Value = serde_json::from_slice(bytes).unwrap();
        v["cells"].as_array().unwrap().clone()
    }

    #[test]
    fn projects_one_header_per_cell() {
        let (text, _) = project(JUPYTER).unwrap();
        assert_eq!(
            text,
            "# %% id=a1b2c3d4\nimport os\nprint('hi')\n\
             # %% [markdown] id=e5f6a7b8\n# Title\n\nSome **bold**  \n\
             # %% [raw] id=c9d0e1f2\n\n"
        );
    }

    #[test]
    fn unedited_round_trip_is_byte_identical() {
        let (text, doc) = project(JUPYTER).unwrap();
        assert_eq!(doc.serialize(&text), JUPYTER.as_bytes());
    }

    #[test]
    fn reserialized_jupyter_output_is_byte_identical() {
        let (text, mut doc) = project(JUPYTER).unwrap();
        // Force the serializer path rather than the original-bytes shortcut.
        doc.original.clear();
        assert_eq!(String::from_utf8(doc.serialize(&text)).unwrap(), JUPYTER);
    }

    #[test]
    fn exponent_float_survives_an_unedited_save() {
        let json = JUPYTER.replace(
            " \"metadata\": {\n  \"kernelspec\"",
            " \"metadata\": {\n  \"scale\": 1e-05,\n  \"kernelspec\"",
        );
        assert_ne!(json, JUPYTER);
        let (text, doc) = project(&json).unwrap();
        assert_eq!(doc.serialize(&text), json.as_bytes());
    }

    #[test]
    fn edited_cell_keeps_outputs_and_only_its_source_changes() {
        let (text, doc) = project(JUPYTER).unwrap();
        let edited = text.replace("print('hi')", "print('bye')");
        let out = doc.serialize(&edited);
        let before = cells(JUPYTER.as_bytes());
        let after = cells(&out);
        assert_eq!(after[0]["execution_count"], 3);
        assert_eq!(after[0]["outputs"], before[0]["outputs"]);
        assert_eq!(
            after[0]["source"],
            serde_json::json!(["import os\n", "print('bye')"])
        );
        assert_eq!(after[1], before[1]);
        assert_eq!(after[2], before[2]);
    }

    #[test]
    fn unchanged_string_source_stays_a_string() {
        let json = JUPYTER.replace(
            "\"source\": [\n    \"# Title\\n\",\n    \"\\n\",\n    \"Some **bold**  \"\n   ]",
            "\"source\": \"# Title\\n\\nSome **bold**  \"",
        );
        assert_ne!(json, JUPYTER);
        let (text, doc) = project(&json).unwrap();
        let edited = text.replace("print('hi')", "print(1)");
        let after = cells(&doc.serialize(&edited));
        assert_eq!(after[1]["source"], "# Title\n\nSome **bold**  ");
    }

    #[test]
    fn type_change_adds_and_removes_code_keys() {
        let (text, doc) = project(JUPYTER).unwrap();
        let edited = text
            .replace("# %% id=a1b2c3d4", "# %% [markdown] id=a1b2c3d4")
            .replace("# %% [raw] id=c9d0e1f2", "# %% id=c9d0e1f2");
        let after = cells(&doc.serialize(&edited));
        assert_eq!(after[0]["cell_type"], "markdown");
        assert!(after[0].get("outputs").is_none());
        assert!(after[0].get("execution_count").is_none());
        assert_eq!(after[2]["cell_type"], "code");
        assert_eq!(after[2]["outputs"], serde_json::json!([]));
        assert_eq!(after[2]["execution_count"], Value::Null);
    }

    #[test]
    fn new_cell_gets_an_id_only_at_nbformat_4_5() {
        let (text, doc) = project(JUPYTER).unwrap();
        let edited = format!("{text}# %% [markdown]\nnew\n");
        let after = cells(&doc.serialize(&edited));
        assert_eq!(after.len(), 4);
        assert_eq!(after[3]["cell_type"], "markdown");
        assert_eq!(after[3]["source"], serde_json::json!(["new"]));
        assert!(after[3]["id"].as_str().is_some_and(is_nbformat_id));
        assert!(after[3].get("outputs").is_none());

        let old = JUPYTER.replace("\"nbformat_minor\": 5", "\"nbformat_minor\": 4");
        let (text, doc) = project(&old).unwrap();
        let edited = format!("{text}# %%\nnew\n");
        let after = cells(&doc.serialize(&edited));
        assert!(after[3].get("id").is_none());
        assert_eq!(after[3]["outputs"], serde_json::json!([]));
    }

    #[test]
    fn synthetic_ids_are_never_written() {
        let old = JUPYTER
            .replace("   \"id\": \"a1b2c3d4\",\n", "")
            .replace("\"nbformat_minor\": 5", "\"nbformat_minor\": 4");
        let (text, doc) = project(&old).unwrap();
        assert!(text.starts_with("# %% id=~0\n"));
        let edited = text.replace("print('hi')", "print(2)");
        let after = cells(&doc.serialize(&edited));
        assert!(after[0].get("id").is_none());
        assert_eq!(after[0]["execution_count"], 3);
    }

    #[test]
    fn synthetic_ids_follow_their_cell_across_saves() {
        let old = JUPYTER
            .replace("   \"id\": \"a1b2c3d4\",\n", "")
            .replace("   \"id\": \"e5f6a7b8\",\n", "")
            .replace("   \"id\": \"c9d0e1f2\",\n", "")
            .replace("\"nbformat_minor\": 5", "\"nbformat_minor\": 4");
        let (text, doc) = project(&old).unwrap();
        let inserted = format!("# %%\nnew\n{text}");
        let (_, doc) = doc.save(&inserted);
        // `~0` still names the code cell with outputs, now second on disk.
        let edited = inserted.replace("print('hi')", "print(3)");
        let after = cells(&doc.serialize(&edited));
        assert_eq!(after[1]["execution_count"], 3);
        assert_eq!(after[0]["outputs"], serde_json::json!([]));
    }

    #[test]
    fn pasted_duplicate_is_a_copy_with_a_fresh_id() {
        let (text, doc) = project(JUPYTER).unwrap();
        let edited = format!("{text}# %% id=a1b2c3d4\nimport os\nprint('hi')\n");
        let after = cells(&doc.serialize(&edited));
        assert_eq!(after.len(), 4);
        assert_eq!(after[0]["id"], "a1b2c3d4");
        assert_ne!(after[3]["id"], "a1b2c3d4");
        assert_eq!(after[3]["outputs"], after[0]["outputs"]);
    }

    #[test]
    fn trailing_newline_in_a_source_round_trips() {
        let json = JUPYTER.replace("\"print('hi')\"", "\"print('hi')\\n\"");
        let (text, doc) = project(&json).unwrap();
        assert!(text.contains("print('hi')\n\n# %% [markdown]"));
        assert_eq!(doc.serialize(&text), json.as_bytes());
        let mut doc = doc;
        doc.original.clear();
        assert_eq!(doc.serialize(&text), json.as_bytes());
    }

    #[test]
    fn empty_notebook_round_trips() {
        let doc = empty_notebook();
        let out = doc.serialize("");
        let (text, _) = project(std::str::from_utf8(&out).unwrap()).unwrap();
        assert_eq!(text, "");
        assert_eq!(cells(&out), Vec::<Value>::new());
    }

    #[test]
    fn text_above_the_first_header_saves_as_a_code_cell() {
        let doc = empty_notebook();
        let after = cells(&doc.serialize("x = 1\n"));
        assert_eq!(after.len(), 1);
        assert_eq!(after[0]["cell_type"], "code");
        assert_eq!(after[0]["source"], serde_json::json!(["x = 1"]));
        assert!(cells(&doc.serialize("\n\n")).is_empty());
    }

    #[test]
    fn header_like_source_line_refuses_projection() {
        let json = JUPYTER.replace("\"import os\\n\"", "\"# %% [markdown]\\n\"");
        assert!(project(&json).unwrap_err().contains("cell header"));
        // A jupytext cell title is not a header.
        let json = JUPYTER.replace("\"import os\\n\"", "\"# %% Load data\\n\"");
        assert!(project(&json).is_ok());
    }

    #[test]
    fn unsupported_files_are_refused() {
        assert!(project("{\"nbformat\": 3, \"worksheets\": []}").is_err());
        assert!(project("[1, 2]").is_err());
        assert!(
            project("{ not json")
                .unwrap_err()
                .starts_with("invalid JSON")
        );
        assert!(project("{\"nbformat\": 4, \"cells\": [{\"cell_type\": \"x\"}]}").is_err());
    }

    #[test]
    fn header_grammar() {
        assert_eq!(
            parse_header("# %% [md] id=x  "),
            Some(Header {
                kind: CellKind::Markdown,
                id: Some("x".into())
            })
        );
        assert_eq!(
            parse_header("# %%"),
            Some(Header {
                kind: CellKind::Code,
                id: None
            })
        );
        assert_eq!(parse_header("# %%x"), None);
        assert_eq!(parse_header("# %% id="), None);
        assert_eq!(parse_header("# %% title"), None);
        assert_eq!(parse_header(" # %%"), None);
    }

    #[test]
    fn spans_match_between_text_and_rope() {
        let (text, _) = project(JUPYTER).unwrap();
        let rope = Rope::from_str(&text);
        let spans = cell_spans(&rope);
        assert_eq!(spans, cell_spans_text(&text));
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].header, Some(0));
        assert_eq!(spans[0].body, 1..3);
        assert_eq!(spans[1].body, 4..7);
        assert_eq!(spans[2].body, 8..9);
        assert_eq!(span_at(&spans, 5), Some(1));
        assert_eq!(span_at(&spans, 0), Some(0));
    }

    #[test]
    fn lsp_view_blanks_everything_but_code() {
        let text = "# %% id=a\n%matplotlib inline\nx = 1\n  !pip install y\n\
                    # %% [markdown] id=b\nundefined words\n";
        let view = lsp_view(text);
        assert_eq!(view, "\n\nx = 1\n\n\n\n");
        assert_eq!(view.split('\n').count(), text.split('\n').count());
    }

    #[test]
    fn masked_keeps_byte_length() {
        let text = "# %% id=a\ndef f(): pass\n# %% [markdown] id=b\nüber def\n";
        let code = masked(text, CellKind::Code);
        let md = masked(text, CellKind::Markdown);
        assert_eq!(code.len(), text.len());
        assert_eq!(md.len(), text.len());
        assert!(code.contains("def f(): pass"));
        assert!(!code.contains("über"));
        assert!(md.contains("über def"));
        assert!(!md.contains("def f"));
        assert!(!md.contains("# %%"));
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(128))]
        #[test]
        fn arbitrary_text_serializes_without_panicking(
            text in "(# %%( \\[markdown\\]| \\[raw\\])?( id=[a-z~0-9]{1,3})?\n|[a-zé %#\\[\\]=]{0,8}\n){0,12}"
        ) {
            let (_, doc) = project(JUPYTER).unwrap();
            let out = doc.serialize(&text);
            let (round, _) = project(std::str::from_utf8(&out).unwrap()).unwrap();
            proptest::prop_assert_eq!(cell_spans_text(&round).len(), cell_spans_text(&text).len());
            let _ = lsp_view(&text);
            proptest::prop_assert_eq!(masked(&text, CellKind::Code).len(), text.len());
        }
    }
}
