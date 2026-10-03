//! Jupyter notebooks (`.ipynb`) as editable text.
//!
//! A notebook is projected into "percent" text — one header line per cell
//! (`# %% id=…`, `# %% [markdown] id=…`, `# %% [raw] id=…`) followed by the
//! cell's source — and the buffer edits that text. The original JSON stays on
//! the buffer as a `NotebookDoc`; on save, `save` matches the text's cells
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
    /// What kernel runs have done to code cells since the file was read or
    /// last saved, by the id in the text. Kept beside `cells` rather than
    /// written into them, so a save still sees what changed: an overlay
    /// that matches the disk cell gives back the original bytes.
    runs: HashMap<String, CellRun>,
    /// Cells sent to the kernel and not finished yet. Carried across a
    /// save, which a long-running cell outlives.
    busy: HashSet<String>,
    /// Changes whenever `runs` or `busy` does, from one counter shared by
    /// every doc, so a cache keyed on it can't mistake a fresh doc (made by
    /// a save) for the one it was built from.
    rev: u64,
}

#[derive(Debug, Clone, Default)]
struct CellRun {
    count: Option<u64>,
    outputs: Vec<Value>,
    /// `clear_output(wait=True)`: clear when the next output arrives.
    clear_on_next: bool,
}

fn next_rev() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static REV: AtomicU64 = AtomicU64::new(1);
    REV.fetch_add(1, Ordering::Relaxed)
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

/// A line break ropey counts that `\n`-split text doesn't: a CR not
/// followed by LF, VT, FF, NEL, or a Unicode line or paragraph separator.
fn has_other_line_break(s: &str) -> bool {
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() != Some(&'\n') => return true,
            '\u{0b}' | '\u{0c}' | '\u{85}' | '\u{2028}' | '\u{2029}' => return true,
            _ => {}
        }
    }
    false
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
        // A later duplicate of an id on disk is named by its position
        // instead, so it keeps its own outputs rather than the first's.
        let id = match cell.get("id").and_then(Value::as_str) {
            Some(id)
                if parse_header(&header_line(kind, Some(id))).is_some()
                    && !by_id.contains_key(id) =>
            {
                id.to_string()
            }
            _ => format!("{SYNTHETIC_PREFIX}{i}"),
        };
        by_id.insert(id.clone(), i);
        let source = source_text(cell);
        // The rope breaks lines at more than `\n`, so a lone CR in a source
        // would let the cell bars find a header the save never sees — code
        // drawn as concealed markdown.
        if has_other_line_break(&source) {
            return Err(format!(
                "cell {} contains a line break other than \\n",
                i + 1
            ));
        }
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
            runs: HashMap::new(),
            busy: HashSet::new(),
            rev: next_rev(),
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
    cell_spans_text(&rope.to_string())
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

/// The source of the cell `span` covers in `text`, as the save writes it.
pub fn span_source(text: &str, span: &CellSpan) -> String {
    text_lines(text)[span.body.clone()].join("\n")
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

/// `text` with a fresh id on every header that has none or repeats an
/// earlier one, or `None` when every header already has its own. Done to the
/// text before a save so the buffer, its undo history and the next save all
/// agree on which cell is which. Without nbformat ids the fresh ones are
/// synthetic, which are never written.
pub fn fix_ids(text: &str, has_ids: bool) -> Option<String> {
    let lines = text_lines(text);
    let mut taken: HashSet<String> = lines.iter().filter_map(|l| parse_header(l)?.id).collect();
    let mut seen = HashSet::new();
    let mut changed = false;
    let mut out = String::with_capacity(text.len() + 64);
    for line in &lines {
        match parse_header(line) {
            Some(h) if h.id.as_ref().is_none_or(|id| !seen.insert(id.clone())) => {
                let id = fresh_id(&taken);
                let id = if has_ids {
                    id
                } else {
                    format!("{SYNTHETIC_PREFIX}{id}")
                };
                taken.insert(id.clone());
                seen.insert(id.clone());
                out.push_str(&header_line(h.kind, Some(&id)));
                changed = true;
            }
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    if !text.ends_with('\n') {
        out.pop();
    }
    changed.then_some(out)
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
    /// of them. An id an earlier cell already took has no entry: on disk it
    /// carries a fresh id the text doesn't know. Text above the first header
    /// is keyed `""`, so the cell its first save made is the cell it names
    /// from then on, rather than a new one with a new id on every save.
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
            let key = tc.id.clone().unwrap_or_default();
            let first_use = used.insert(key.clone());
            let orig = self.by_id.get(&key).copied();
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
            if let Some(run) = self.runs.get(&key).filter(|_| first_use) {
                if tc.kind == CellKind::Code {
                    cell.insert("outputs".into(), Value::Array(run.outputs.clone()));
                    cell.insert(
                        "execution_count".into(),
                        run.count.map_or(Value::Null, Value::from),
                    );
                }
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
                by_id.insert(key, out.len());
            }
            out.push(cell_value);
        }
        (out, by_id)
    }

    /// The notebook's bytes for `text`, and the doc that describes the file
    /// once they are written. When every cell comes out as it went in, the
    /// file's original bytes are returned untouched. The doc's ids come from
    /// the text, not from re-reading the bytes: a
    /// synthetic `~3` keeps naming the cell it named before a cell was
    /// inserted above it, where a re-read would hand it to another cell.
    pub fn save(&self, text: &str) -> (Vec<u8>, NotebookDoc) {
        let (cells, by_id) = self.cells_from_text(text);
        if cells == self.cells && !self.original.is_empty() {
            let doc = NotebookDoc {
                by_id,
                runs: HashMap::new(),
                rev: next_rev(),
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
        let fmt = PythonFormatter(serde_json::ser::PrettyFormatter::with_indent(b" "));
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
            runs: HashMap::new(),
            busy: self.busy.clone(),
            rev: next_rev(),
        };
        (out, doc)
    }

    /// The original source of the cell `id` names, when it has one on disk.
    pub fn original_source(&self, id: &str) -> Option<String> {
        self.by_id.get(id).map(|&i| source_text(&self.cells[i]))
    }

    /// `(execution_count, number of outputs)` of the cell `id` names, as
    /// the next save would write them.
    pub fn cell_info(&self, id: &str) -> Option<(Option<u64>, usize)> {
        if let Some(run) = self.runs.get(id) {
            return Some((run.count, run.outputs.len()));
        }
        let cell = &self.cells[*self.by_id.get(id)?];
        let count = cell.get("execution_count").and_then(Value::as_u64);
        Some((count, self.outputs(id).len()))
    }

    /// The outputs of the cell `id` names, as the next save would write them.
    pub fn outputs(&self, id: &str) -> &[Value] {
        if let Some(run) = self.runs.get(id) {
            return &run.outputs;
        }
        self.by_id
            .get(id)
            .and_then(|&i| self.cells[i].get("outputs"))
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }

    pub fn rev(&self) -> u64 {
        self.rev
    }

    pub fn is_busy(&self, id: &str) -> bool {
        self.busy.contains(id)
    }

    fn run_mut(&mut self, id: &str) -> &mut CellRun {
        self.rev = next_rev();
        if !self.runs.contains_key(id) {
            let (count, _) = self.cell_info(id).unwrap_or((None, 0));
            let outputs = self.outputs(id).to_vec();
            self.runs.insert(
                id.to_string(),
                CellRun {
                    count,
                    outputs,
                    clear_on_next: false,
                },
            );
        }
        self.runs.get_mut(id).expect("inserted above")
    }

    /// The cell was sent to the kernel: its old outputs and count go, as
    /// Jupyter clears them when a cell is run.
    pub fn begin_run(&mut self, id: &str) {
        *self.run_mut(id) = CellRun::default();
        self.busy.insert(id.to_string());
    }

    pub fn finish_run(&mut self, id: &str) {
        self.run_mut(id).clear_on_next = false;
        self.busy.remove(id);
    }

    /// Every cell stops counting as running — the kernel died or was
    /// restarted, and nothing more will come for them.
    pub fn clear_busy(&mut self) {
        if !self.busy.is_empty() {
            self.busy.clear();
            self.rev = next_rev();
        }
    }

    pub fn set_count(&mut self, id: &str, count: u64) {
        self.run_mut(id).count = Some(count);
    }

    /// Append one nbformat output. Consecutive stream outputs to the same
    /// stream are merged into one, as Jupyter merges them before it saves.
    pub fn push_output(&mut self, id: &str, output: Value) {
        let run = self.run_mut(id);
        if std::mem::take(&mut run.clear_on_next) {
            run.outputs.clear();
        }
        if let Some(last) = run.outputs.last_mut() {
            let stream = |v: &Value| {
                (v.get("output_type")?.as_str()? == "stream")
                    .then(|| v.get("name").and_then(Value::as_str).map(str::to_string))?
            };
            if let (Some(a), Some(b)) = (stream(last), stream(&output)) {
                if a == b {
                    let text = multiline(last.get("text")) + &multiline(output.get("text"));
                    last["text"] = split_lines_value(&text);
                    return;
                }
            }
        }
        run.outputs.push(output);
    }

    /// `clear_output`: now, or with `wait` when the next output arrives, so
    /// a cell that redraws a progress line doesn't flicker.
    pub fn clear_outputs(&mut self, id: &str, wait: bool) {
        let run = self.run_mut(id);
        if wait {
            run.clear_on_next = true;
        } else {
            run.outputs.clear();
        }
    }

    /// `:cell clear`: no outputs and no count, as if never run.
    pub fn clear_cell(&mut self, id: &str) {
        *self.run_mut(id) = CellRun::default();
    }

    pub fn has_ids(&self) -> bool {
        self.has_ids
    }
}

/// serde_json's pretty printer with floats written the way Python's `repr`
/// writes them (`2.5e-05`, `1e+16`, where serde_json writes `0.000025`), so a
/// save that changes one cell leaves the numbers in every other cell as
/// Jupyter wrote them.
struct PythonFormatter<'a>(serde_json::ser::PrettyFormatter<'a>);

impl serde_json::ser::Formatter for PythonFormatter<'_> {
    fn write_f64<W: ?Sized + std::io::Write>(&mut self, w: &mut W, v: f64) -> std::io::Result<()> {
        w.write_all(python_float_repr(v).as_bytes())
    }
    fn begin_array<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        self.0.begin_array(w)
    }
    fn end_array<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        self.0.end_array(w)
    }
    fn begin_array_value<W: ?Sized + std::io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        self.0.begin_array_value(w, first)
    }
    fn end_array_value<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        self.0.end_array_value(w)
    }
    fn begin_object<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        self.0.begin_object(w)
    }
    fn end_object<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        self.0.end_object(w)
    }
    fn begin_object_key<W: ?Sized + std::io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        self.0.begin_object_key(w, first)
    }
    fn begin_object_value<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        self.0.begin_object_value(w)
    }
    fn end_object_value<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        self.0.end_object_value(w)
    }
}

/// `repr(v)` for a finite Python float: the shortest digits that round-trip,
/// in exponent form when the decimal point falls more than 4 places before
/// the first digit or more than 16 after it.
fn python_float_repr(v: f64) -> String {
    let sci = format!("{v:e}");
    let (sign, sci) = sci
        .strip_prefix('-')
        .map_or(("", sci.as_str()), |rest| ("-", rest));
    let (mantissa, exp) = sci.split_once('e').expect("{:e} always has an exponent");
    let exp: i32 = exp.parse().expect("{:e} exponent is an integer");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    // The value is 0.<digits> × 10^point.
    let point = exp + 1;
    if !(-4 < point && point <= 16) {
        let (head, tail) = digits.split_at(1);
        let dot = if tail.is_empty() { "" } else { "." };
        let esign = if exp < 0 { '-' } else { '+' };
        return format!("{sign}{head}{dot}{tail}e{esign}{:02}", exp.abs());
    }
    let n = digits.len() as i32;
    if point <= 0 {
        format!("{sign}0.{}{digits}", "0".repeat((-point) as usize))
    } else if point >= n {
        format!("{sign}{digits}{}.0", "0".repeat((point - n) as usize))
    } else {
        let (int, frac) = digits.split_at(point as usize);
        format!("{sign}{int}.{frac}")
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

/// An IPython `%magic` or `!shell` line, which no Python tool can parse —
/// not a continuation line that starts with `% 3` or `!= b`.
fn is_ipython_line(line: &str) -> bool {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix('%') {
        rest.starts_with(|c: char| c == '%' || c.is_ascii_alphabetic())
    } else if let Some(rest) = t.strip_prefix('!') {
        !rest.starts_with('=')
    } else {
        false
    }
}

/// A code cell that opens with a `%%bash` / `%%writefile` cell magic, whose
/// whole body is for another program.
fn is_cell_magic(body: &[&str]) -> bool {
    body.iter()
        .find(|l| !l.trim().is_empty())
        .is_some_and(|l| l.trim_start().starts_with("%%"))
}

/// Code cells in `text` a Python formatter should run over, with their
/// source: those that differ from the disk cell their id names (edited, new
/// or pasted), and that hold no IPython line the formatter would refuse.
/// Formatting only these keeps a save from rewriting cells nobody touched.
pub fn formattable_edits(doc: &NotebookDoc, text: &str) -> Vec<(Range<usize>, String)> {
    let lines = text_lines(text);
    cell_spans_text(text)
        .into_iter()
        .filter(|s| s.kind == CellKind::Code)
        .filter_map(|s| {
            let body = &lines[s.body.clone()];
            if body.iter().all(|l| l.trim().is_empty()) || body.iter().any(|l| is_ipython_line(l)) {
                return None;
            }
            let source = body.join("\n");
            let original = s.id.as_deref().and_then(|id| doc.original_source(id));
            (original.as_deref() != Some(source.as_str())).then_some((s.body, source))
        })
        .collect()
}

/// `text` with each line range in `edits` replaced by its new source.
/// Ranges are in buffer lines and must not overlap.
pub fn replace_bodies(text: &str, edits: &[(Range<usize>, String)]) -> String {
    let lines = text_lines(text);
    let mut out = String::with_capacity(text.len());
    let mut next = 0;
    let mut sorted: Vec<&(Range<usize>, String)> = edits.iter().collect();
    sorted.sort_by_key(|(r, _)| r.start);
    for (range, source) in sorted {
        for line in &lines[next..range.start] {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(source);
        out.push('\n');
        next = range.end;
    }
    for line in &lines[next..] {
        out.push_str(line);
        out.push('\n');
    }
    if !text.ends_with('\n') {
        out.pop();
    }
    out
}

/// What the language server is shown: code lines as they are, every other
/// line — headers, markdown, raw, IPython `%magic` and `!shell` lines —
/// emptied. The line count doesn't change, so diagnostics and edits land on
/// buffer lines with no translation.
pub fn lsp_view(text: &str) -> String {
    let lines = text_lines(text);
    let mut keep = vec![false; lines.len()];
    for span in cell_spans_text(text) {
        if span.kind != CellKind::Code || is_cell_magic(&lines[span.body.clone()]) {
            continue;
        }
        for l in span.body {
            keep[l] = !is_ipython_line(lines[l]);
        }
    }
    let mut out = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        if keep[i] {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !text.ends_with('\n') {
        out.pop();
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

/// A whole-cell operation from `:cell` or `<leader>n`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellEdit {
    Add { kind: CellKind, above: bool },
    Delete,
    Move { down: bool },
    Type(CellKind),
    Split,
    Join,
}

/// Apply `edit` to the cell holding `line`, returning the new text and the
/// line the cursor belongs on. New headers carry no id: the save's
/// `fix_ids` pass names them, the same as a header typed by hand.
pub fn edit_cells(text: &str, line: usize, edit: CellEdit) -> Result<(String, usize), String> {
    let mut lines: Vec<String> = text_lines(text).into_iter().map(String::from).collect();
    let spans = cell_spans_text(text);
    let Some(here) = span_at(&spans, line) else {
        let CellEdit::Add { kind, .. } = edit else {
            return Err("no cell here".into());
        };
        return Ok((format!("{}\n\n", header_line(kind, None)), 1));
    };
    let span = &spans[here];
    let cursor = match edit {
        CellEdit::Add { kind, above } => {
            let at = if above { span.start() } else { span.body.end };
            lines.splice(at..at, [header_line(kind, None), String::new()]);
            at + 1
        }
        CellEdit::Delete => {
            lines.drain(span.start()..span.body.end);
            match (spans.get(here + 1), here.checked_sub(1)) {
                (Some(_), _) => span.start(),
                (None, Some(prev)) => spans[prev].start(),
                (None, None) => 0,
            }
        }
        CellEdit::Move { down } => {
            let other = if down {
                spans.get(here + 1).map(|_| here + 1)
            } else {
                here.checked_sub(1)
            }
            .ok_or(if down {
                "no cell below"
            } else {
                "no cell above"
            })?;
            // Text above the first header has no header of its own, so
            // moved below another cell it would join that cell's body.
            if span.header.is_none() || spans[other].header.is_none() {
                return Err("the text above the first header isn't a cell".into());
            }
            let (first, second) = if down {
                (span, &spans[other])
            } else {
                (&spans[other], span)
            };
            let upper_len = first.body.end - first.start();
            lines[first.start()..second.body.end].rotate_left(upper_len);
            let offset = line - span.start();
            if down {
                first.start() + (second.body.end - second.start()) + offset
            } else {
                first.start() + offset
            }
        }
        CellEdit::Type(kind) => match span.header {
            Some(_) if span.kind == kind => {
                return Err(format!("already a {} cell", kind.as_str()));
            }
            Some(h) => {
                lines[h] = header_line(kind, span.id.as_deref());
                line
            }
            None => {
                lines.insert(span.start(), header_line(kind, None));
                line + 1
            }
        },
        CellEdit::Split => {
            if span.header == Some(line) {
                return Err("the cursor is on the cell's header".into());
            }
            if line == span.body.start {
                return Err("nothing above the cursor in this cell".into());
            }
            lines.insert(line, header_line(span.kind, None));
            line + 1
        }
        CellEdit::Join => {
            let next = spans.get(here + 1).ok_or("no cell below")?;
            if let Some(h) = next.header {
                lines.remove(h);
            }
            line
        }
    };
    let mut out = lines.join("\n");
    if text.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
    Ok((out, cursor))
}

/// An nbformat multi-line string — one string, or a list of lines.
fn multiline(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

/// `text` as nbformat writes it: `str.splitlines(True)`, which breaks at
/// every line break Python knows, not only `\n`.
fn split_lines_value(text: &str) -> Value {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let end = match c {
            '\r' if chars.peek().map(|&(_, n)| n) == Some('\n') => {
                chars.next();
                i + 2
            }
            '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}'
            | '\u{2028}' | '\u{2029}' => i + c.len_utf8(),
            _ => continue,
        };
        out.push(Value::String(text[start..end].to_string()));
        start = end;
    }
    if start < text.len() {
        out.push(Value::String(text[start..].to_string()));
    }
    Value::Array(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStyle {
    Text,
    Stderr,
    Error,
    /// Something drawn in place of an output that has no text form.
    Note,
}

/// One screen row of a cell's outputs, drawn under the cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputRow {
    pub style: OutputStyle,
    pub text: String,
}

/// Rows a cell's outputs may take under it before the middle is elided;
/// `:cell output` shows the whole of it.
pub const MAX_OUTPUT_ROWS: usize = 30;

/// `line` as a terminal shows it: ANSI escapes dropped (IPython colours
/// its tracebacks), tabs as spaces, other control characters gone, and only
/// what follows the last carriage return, which a progress bar redraws over.
fn display_line(line: &str) -> String {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let line = line.rsplit('\r').next().unwrap_or(line);
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                if chars.next() == Some('[') {
                    for c in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            break;
                        }
                    }
                }
            }
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn image_mime(data: &Map<String, Value>) -> Option<&str> {
    data.keys()
        .map(String::as_str)
        .find(|k| k.starts_with("image/"))
}

/// The text each output shows, with its style. An image shows a note in
/// its place: its `text/plain` is only `<Figure size 640x480 …>`.
fn output_lines(outputs: &[Value]) -> Vec<(OutputStyle, String)> {
    let mut rows = Vec::new();
    let mut push_text = |style, text: &str| {
        let text = text.strip_suffix('\n').unwrap_or(text);
        rows.extend(text.split('\n').map(|l| (style, display_line(l))));
    };
    for out in outputs {
        match out.get("output_type").and_then(Value::as_str) {
            Some("stream") => {
                let style = if out.get("name").and_then(Value::as_str) == Some("stderr") {
                    OutputStyle::Stderr
                } else {
                    OutputStyle::Text
                };
                push_text(style, &multiline(out.get("text")));
            }
            Some("execute_result" | "display_data") => {
                let Some(data) = out.get("data").and_then(Value::as_object) else {
                    continue;
                };
                if let Some(mime) = image_mime(data) {
                    push_text(
                        OutputStyle::Note,
                        &format!("[{mime}] :cell output opens it"),
                    );
                } else if let Some(text) = data.get("text/plain") {
                    push_text(OutputStyle::Text, &multiline(Some(text)));
                } else if let Some(mime) = data.keys().next() {
                    push_text(OutputStyle::Note, &format!("[{mime}]"));
                }
            }
            Some("error") => {
                let tb: Vec<String> = out
                    .get("traceback")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                let text = if tb.is_empty() {
                    format!(
                        "{}: {}",
                        multiline(out.get("ename")),
                        multiline(out.get("evalue"))
                    )
                } else {
                    tb.join("\n")
                };
                push_text(OutputStyle::Error, &text);
            }
            _ => {}
        }
    }
    rows
}

/// The rows drawn under a cell for its outputs. Past `MAX_OUTPUT_ROWS` the
/// middle is elided rather than the end, so a traceback keeps the line
/// that names the error and a log keeps its last lines.
pub fn output_rows(outputs: &[Value]) -> Vec<OutputRow> {
    let lines = output_lines(outputs);
    let row = |(style, text): (OutputStyle, String)| OutputRow { style, text };
    if lines.len() <= MAX_OUTPUT_ROWS {
        return lines.into_iter().map(row).collect();
    }
    let head = MAX_OUTPUT_ROWS / 2;
    let tail = MAX_OUTPUT_ROWS - head - 1;
    let hidden = lines.len() - head - tail;
    let mut rows: Vec<OutputRow> = lines[..head].iter().cloned().map(row).collect();
    rows.push(OutputRow {
        style: OutputStyle::Note,
        text: format!("… {hidden} more lines — :cell output shows them all"),
    });
    rows.extend(lines[lines.len() - tail..].iter().cloned().map(row));
    rows
}

/// Every output's text, for `:cell output` to show whole.
pub fn output_text(outputs: &[Value]) -> String {
    let mut text: String = output_lines(outputs)
        .into_iter()
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n");
    text.push('\n');
    text
}

/// The images among `outputs`, as a file extension and the decoded bytes.
/// SVG is stored as text, every other image as base64.
pub fn output_images(outputs: &[Value]) -> Vec<(&'static str, Vec<u8>)> {
    let mut images = Vec::new();
    for out in outputs {
        let Some(data) = out.get("data").and_then(Value::as_object) else {
            continue;
        };
        for (mime, ext) in [
            ("image/png", "png"),
            ("image/jpeg", "jpg"),
            ("image/gif", "gif"),
            ("image/svg+xml", "svg"),
        ] {
            let Some(v) = data.get(mime) else { continue };
            let text = multiline(Some(v));
            let bytes = if ext == "svg" {
                Some(text.into_bytes())
            } else {
                base64_decode(&text)
            };
            if let Some(bytes) = bytes {
                images.push((ext, bytes));
                break;
            }
        }
    }
    images
}

/// Standard base64, whitespace ignored, as Jupyter stores image data.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for b in text.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b if b.is_ascii_whitespace() => continue,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
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
    fn only_edited_and_new_code_cells_are_formattable() {
        let (text, doc) = project(JUPYTER).unwrap();
        assert!(formattable_edits(&doc, &text).is_empty());
        let text = text
            .replace("print('hi')", "print( 'hi' )")
            .replace("Some **bold**", "Some  **bold**")
            + "# %%\nx=1\n# %%\n%time x\n";
        let edits = formattable_edits(&doc, &text);
        let sources: Vec<&str> = edits.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(sources, ["import os\nprint( 'hi' )", "x=1"]);
        let formatted: Vec<_> = edits
            .into_iter()
            .map(|(r, s)| (r, s.replace("( 'hi' )", "('hi')").replace("x=1", "x = 1")))
            .collect();
        let out = replace_bodies(&text, &formatted);
        assert!(out.contains("print('hi')\n# %% [markdown]"));
        assert!(out.ends_with("# %%\nx = 1\n# %%\n%time x\n"));
        assert_eq!(out.lines().count(), text.lines().count());
    }

    #[test]
    fn fix_ids_rewrites_missing_and_repeated_ids_only() {
        let text = "# %% id=aa\nx\n# %% id=aa\ny\n# %% [markdown]\nz\n# %% id=bb\n";
        let fixed = fix_ids(text, true).unwrap();
        let ids: Vec<String> = fixed.lines().filter_map(|l| parse_header(l)?.id).collect();
        assert_eq!(ids.len(), 4);
        assert_eq!((ids[0].as_str(), ids[3].as_str()), ("aa", "bb"));
        assert!(ids.iter().collect::<HashSet<_>>().len() == 4);
        assert!(fixed.contains("# %% [markdown] id="));
        assert!(is_nbformat_id(&ids[1]));
        assert_eq!(fix_ids(&fixed, true), None);
        let synthetic = fix_ids("# %%\nx\n", false).unwrap();
        assert!(synthetic.starts_with("# %% id=~"));
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
        assert_eq!(doc.save(&text).0, JUPYTER.as_bytes());
    }

    #[test]
    fn reserialized_jupyter_output_is_byte_identical() {
        let (text, mut doc) = project(JUPYTER).unwrap();
        // Force the serializer path rather than the original-bytes shortcut.
        doc.original.clear();
        assert_eq!(String::from_utf8(doc.save(&text).0).unwrap(), JUPYTER);
    }

    #[test]
    fn exponent_float_survives_an_unedited_save() {
        let json = JUPYTER.replace(
            " \"metadata\": {\n  \"kernelspec\"",
            " \"metadata\": {\n  \"scale\": 1e-05,\n  \"kernelspec\"",
        );
        assert_ne!(json, JUPYTER);
        let (text, doc) = project(&json).unwrap();
        assert_eq!(doc.save(&text).0, json.as_bytes());
    }

    #[test]
    fn an_edit_leaves_floats_in_other_cells_as_python_wrote_them() {
        let json = JUPYTER.replace(
            "\"metadata\": {\n    \"tags\"",
            "\"metadata\": {\n    \"duration\": 2.5e-05,\n    \"rate\": 1e+16,\n    \"tags\"",
        );
        assert_ne!(json, JUPYTER);
        let (text, doc) = project(&json).unwrap();
        let out = doc.save(&text.replace("print('hi')", "print('ho')")).0;
        assert_eq!(
            String::from_utf8(out).unwrap(),
            json.replace("\"print('hi')\"", "\"print('ho')\"")
        );
    }

    #[test]
    fn python_float_repr_matches_python() {
        for (v, want) in [
            (2.5e-05, "2.5e-05"),
            (1e-05, "1e-05"),
            (0.0001, "0.0001"),
            (1e16, "1e+16"),
            (1234567890123456.0, "1234567890123456.0"),
            (1.5e300, "1.5e+300"),
            (0.1, "0.1"),
            (1.0, "1.0"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (-3.25, "-3.25"),
            (123.456, "123.456"),
        ] {
            assert_eq!(python_float_repr(v), want, "{v}");
        }
    }

    #[test]
    fn a_repeated_disk_id_keeps_each_cells_own_outputs() {
        let json = JUPYTER.replace("\"id\": \"e5f6a7b8\"", "\"id\": \"a1b2c3d4\"");
        let (text, doc) = project(&json).unwrap();
        assert!(text.contains("# %% [markdown] id=~1\n"));
        assert_eq!(fix_ids(&text, true), None);
        assert_eq!(doc.cell_info("~1"), Some((None, 0)));
        assert_eq!(doc.save(&text).0, json.as_bytes());
    }

    #[test]
    fn text_above_the_first_header_keeps_its_id_across_saves() {
        let (first, doc) = empty_notebook().save("print(1)\n");
        let (second, _) = doc.save("print(1)\n");
        assert_eq!(first, second);
        let id = cells(&first)[0]["id"].clone();
        let (_, doc) = doc.save("print(2)\n");
        let third = doc.save("print(2)\n").0;
        assert_eq!(cells(&third)[0]["id"], id);
    }

    #[test]
    fn a_line_break_other_than_lf_refuses_projection() {
        for brk in ["\\r", "\\u2028", "\\f"] {
            let json = JUPYTER.replace(
                "\"import os\\n\"",
                &format!("\"x = 1{brk}# %% [markdown] id=zz\\n\""),
            );
            assert!(project(&json).unwrap_err().contains("line break"), "{brk}");
        }
        let json = JUPYTER.replace("\"import os\\n\"", "\"import os\\r\\n\"");
        assert!(project(&json).is_ok());
    }

    #[test]
    fn edited_cell_keeps_outputs_and_only_its_source_changes() {
        let (text, doc) = project(JUPYTER).unwrap();
        let edited = text.replace("print('hi')", "print('bye')");
        let out = doc.save(&edited).0;
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

    fn stream(name: &str, text: &str) -> Value {
        serde_json::json!({"output_type": "stream", "name": name, "text": [text]})
    }

    #[test]
    fn a_run_replaces_the_cells_outputs_and_the_save_writes_them() {
        let (text, mut doc) = project(JUPYTER).unwrap();
        let rev = doc.rev();
        doc.begin_run("a1b2c3d4");
        assert!(doc.is_busy("a1b2c3d4"));
        assert_ne!(doc.rev(), rev);
        assert_eq!(doc.cell_info("a1b2c3d4"), Some((None, 0)));
        doc.set_count("a1b2c3d4", 7);
        doc.push_output("a1b2c3d4", stream("stdout", "one\n"));
        doc.push_output("a1b2c3d4", stream("stdout", "two\n"));
        doc.push_output("a1b2c3d4", stream("stderr", "warn\n"));
        doc.finish_run("a1b2c3d4");
        assert!(!doc.is_busy("a1b2c3d4"));
        assert_eq!(doc.cell_info("a1b2c3d4"), Some((Some(7), 2)));
        let (bytes, saved) = doc.save(&text);
        let after = cells(&bytes);
        assert_eq!(after[0]["execution_count"], 7);
        assert_eq!(
            after[0]["outputs"],
            serde_json::json!([
                {"output_type": "stream", "name": "stdout", "text": ["one\n", "two\n"]},
                stream("stderr", "warn\n"),
            ])
        );
        assert_eq!(after[1], cells(JUPYTER.as_bytes())[1]);
        // The saved doc describes the file: saving again changes nothing.
        assert_eq!(saved.save(&text).0, bytes);
        assert_eq!(saved.cell_info("a1b2c3d4"), Some((Some(7), 2)));
    }

    #[test]
    fn a_run_that_reproduces_the_disk_outputs_saves_the_original_bytes() {
        let (text, mut doc) = project(JUPYTER).unwrap();
        doc.begin_run("a1b2c3d4");
        doc.set_count("a1b2c3d4", 3);
        doc.push_output("a1b2c3d4", stream("stdout", "hi ünïcode\n"));
        doc.finish_run("a1b2c3d4");
        assert_eq!(doc.save(&text).0, JUPYTER.as_bytes());
    }

    #[test]
    fn a_waiting_clear_takes_effect_at_the_next_output() {
        let (_, mut doc) = project(JUPYTER).unwrap();
        doc.begin_run("a1b2c3d4");
        doc.push_output("a1b2c3d4", stream("stdout", "10%\n"));
        doc.clear_outputs("a1b2c3d4", true);
        assert_eq!(doc.outputs("a1b2c3d4").len(), 1);
        doc.push_output("a1b2c3d4", stream("stdout", "20%\n"));
        assert_eq!(doc.outputs("a1b2c3d4"), [stream("stdout", "20%\n")]);
        doc.clear_outputs("a1b2c3d4", false);
        assert!(doc.outputs("a1b2c3d4").is_empty());
    }

    #[test]
    fn clearing_a_cell_drops_its_count_and_outputs_on_save() {
        let (text, mut doc) = project(JUPYTER).unwrap();
        doc.clear_cell("a1b2c3d4");
        let after = cells(&doc.save(&text).0);
        assert_eq!(after[0]["execution_count"], Value::Null);
        assert_eq!(after[0]["outputs"], serde_json::json!([]));
    }

    #[test]
    fn a_save_keeps_a_cell_running_and_its_later_output_appends() {
        let (text, mut doc) = project(JUPYTER).unwrap();
        doc.begin_run("a1b2c3d4");
        doc.push_output("a1b2c3d4", stream("stdout", "a\n"));
        let (_, mut saved) = doc.save(&text);
        assert!(saved.is_busy("a1b2c3d4"));
        saved.push_output("a1b2c3d4", stream("stdout", "b\n"));
        assert_eq!(
            saved.outputs("a1b2c3d4"),
            [
                serde_json::json!({"output_type": "stream", "name": "stdout", "text": ["a\n", "b\n"]})
            ]
        );
    }

    #[test]
    fn output_lines_split_the_way_python_splits_them() {
        assert_eq!(
            split_lines_value("a\r\nb\rc\u{2028}d"),
            serde_json::json!(["a\r\n", "b\r", "c\u{2028}", "d"])
        );
        assert_eq!(split_lines_value(""), serde_json::json!([]));
    }

    #[test]
    fn output_rows_show_what_a_terminal_would() {
        let error = serde_json::json!({
            "output_type": "error", "ename": "ValueError", "evalue": "bad",
            "traceback": ["\u{1b}[0;31mValueError\u{1b}[0m: bad"],
        });
        let image = serde_json::json!({
            "output_type": "display_data", "metadata": {},
            "data": {"image/png": "iVBORw0KGgo=", "text/plain": ["<Figure>"]},
        });
        let result = serde_json::json!({
            "output_type": "execute_result", "execution_count": 1, "metadata": {},
            "data": {"text/plain": ["42"]},
        });
        let rows = output_rows(&[
            stream("stdout", "10%\r50%\r100%\n"),
            stream("stderr", "a\tb\n"),
            result,
            image,
            error,
        ]);
        let shown: Vec<(OutputStyle, &str)> =
            rows.iter().map(|r| (r.style, r.text.as_str())).collect();
        assert_eq!(
            shown,
            [
                (OutputStyle::Text, "100%"),
                (OutputStyle::Stderr, "a    b"),
                (OutputStyle::Text, "42"),
                (OutputStyle::Note, "[image/png] :cell output opens it"),
                (OutputStyle::Error, "ValueError: bad"),
            ]
        );
    }

    #[test]
    fn long_output_elides_its_middle() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let rows = output_rows(&[stream("stdout", &text)]);
        assert_eq!(rows.len(), MAX_OUTPUT_ROWS);
        assert_eq!(rows[0].text, "line 0");
        assert_eq!(rows[MAX_OUTPUT_ROWS / 2].style, OutputStyle::Note);
        assert!(rows[MAX_OUTPUT_ROWS / 2].text.contains("71 more lines"));
        assert_eq!(rows.last().unwrap().text, "line 99");
    }

    #[test]
    fn output_images_decode_base64_and_keep_svg_as_text() {
        let outputs = [
            serde_json::json!({"output_type": "display_data", "metadata": {},
                "data": {"image/png": "aGVs\nbG8=\n"}}),
            serde_json::json!({"output_type": "display_data", "metadata": {},
                "data": {"image/svg+xml": ["<svg>\n", "</svg>"]}}),
        ];
        assert_eq!(
            output_images(&outputs),
            [
                ("png", b"hello".to_vec()),
                ("svg", b"<svg>\n</svg>".to_vec())
            ]
        );
        assert_eq!(base64_decode("not*base64"), None);
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
        let after = cells(&doc.save(&edited).0);
        assert_eq!(after[1]["source"], "# Title\n\nSome **bold**  ");
    }

    #[test]
    fn type_change_adds_and_removes_code_keys() {
        let (text, doc) = project(JUPYTER).unwrap();
        let edited = text
            .replace("# %% id=a1b2c3d4", "# %% [markdown] id=a1b2c3d4")
            .replace("# %% [raw] id=c9d0e1f2", "# %% id=c9d0e1f2");
        let after = cells(&doc.save(&edited).0);
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
        let after = cells(&doc.save(&edited).0);
        assert_eq!(after.len(), 4);
        assert_eq!(after[3]["cell_type"], "markdown");
        assert_eq!(after[3]["source"], serde_json::json!(["new"]));
        assert!(after[3]["id"].as_str().is_some_and(is_nbformat_id));
        assert!(after[3].get("outputs").is_none());

        let old = JUPYTER.replace("\"nbformat_minor\": 5", "\"nbformat_minor\": 4");
        let (text, doc) = project(&old).unwrap();
        let edited = format!("{text}# %%\nnew\n");
        let after = cells(&doc.save(&edited).0);
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
        let after = cells(&doc.save(&edited).0);
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
        let after = cells(&doc.save(&edited).0);
        assert_eq!(after[1]["execution_count"], 3);
        assert_eq!(after[0]["outputs"], serde_json::json!([]));
    }

    #[test]
    fn pasted_duplicate_is_a_copy_with_a_fresh_id() {
        let (text, doc) = project(JUPYTER).unwrap();
        let edited = format!("{text}# %% id=a1b2c3d4\nimport os\nprint('hi')\n");
        let after = cells(&doc.save(&edited).0);
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
        assert_eq!(doc.save(&text).0, json.as_bytes());
        let mut doc = doc;
        doc.original.clear();
        assert_eq!(doc.save(&text).0, json.as_bytes());
    }

    #[test]
    fn empty_notebook_round_trips() {
        let doc = empty_notebook();
        let out = doc.save("").0;
        let (text, _) = project(std::str::from_utf8(&out).unwrap()).unwrap();
        assert_eq!(text, "");
        assert_eq!(cells(&out), Vec::<Value>::new());
    }

    #[test]
    fn text_above_the_first_header_saves_as_a_code_cell() {
        let doc = empty_notebook();
        let after = cells(&doc.save("x = 1\n").0);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0]["cell_type"], "code");
        assert_eq!(after[0]["source"], serde_json::json!(["x = 1"]));
        assert!(cells(&doc.save("\n\n").0).is_empty());
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
    fn lsp_view_keeps_continuation_lines_and_drops_cell_magics() {
        let text = "# %% id=a\nok = (a\n      != b)\ny = (x\n     % 3)\n! ls\n\
                    # %% id=b\n%%bash\necho hi\n";
        assert_eq!(
            lsp_view(text),
            "\nok = (a\n      != b)\ny = (x\n     % 3)\n\n\n\n\n"
        );
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

    const CELLS: &str = "# %% id=a\nx = 1\n# %% [markdown] id=b\nhi\nthere\n# %% id=c\ny\n";

    fn edit(line: usize, e: CellEdit) -> (String, usize) {
        edit_cells(CELLS, line, e).unwrap()
    }

    #[test]
    fn adding_a_cell_goes_below_or_above_the_cursors_cell() {
        let below = CellEdit::Add {
            kind: CellKind::Markdown,
            above: false,
        };
        assert_eq!(
            edit(3, below),
            (
                "# %% id=a\nx = 1\n# %% [markdown] id=b\nhi\nthere\n# %% [markdown]\n\n# %% id=c\ny\n".into(),
                6
            )
        );
        let above = CellEdit::Add {
            kind: CellKind::Code,
            above: true,
        };
        assert_eq!(edit(1, above), (format!("# %%\n\n{CELLS}"), 1));
        assert_eq!(edit_cells("", 0, above).unwrap(), ("# %%\n\n".into(), 1));
    }

    #[test]
    fn deleting_a_cell_takes_its_header_and_lands_on_a_neighbour() {
        assert_eq!(
            edit(3, CellEdit::Delete),
            ("# %% id=a\nx = 1\n# %% id=c\ny\n".into(), 2)
        );
        assert_eq!(
            edit(6, CellEdit::Delete),
            (
                "# %% id=a\nx = 1\n# %% [markdown] id=b\nhi\nthere\n".into(),
                2
            )
        );
        assert_eq!(
            edit_cells("# %% id=a\nx\n", 1, CellEdit::Delete).unwrap(),
            (String::new(), 0)
        );
        assert!(edit_cells("", 0, CellEdit::Delete).is_err());
    }

    #[test]
    fn moving_a_cell_swaps_it_with_its_neighbour_and_the_cursor_follows() {
        let swapped = "# %% id=a\nx = 1\n# %% id=c\ny\n# %% [markdown] id=b\nhi\nthere\n";
        assert_eq!(edit(4, CellEdit::Move { down: true }), (swapped.into(), 6));
        assert_eq!(edit(6, CellEdit::Move { down: false }), (swapped.into(), 3));
        assert!(edit_cells(CELLS, 6, CellEdit::Move { down: true }).is_err());
        assert!(edit_cells(CELLS, 0, CellEdit::Move { down: false }).is_err());
        assert!(edit_cells("pre\n# %% id=a\nx\n", 2, CellEdit::Move { down: false }).is_err());
    }

    #[test]
    fn changing_a_cells_type_rewrites_its_header_and_keeps_the_id() {
        assert_eq!(
            edit(1, CellEdit::Type(CellKind::Raw)).0,
            CELLS.replacen("# %% id=a", "# %% [raw] id=a", 1)
        );
        assert!(edit_cells(CELLS, 1, CellEdit::Type(CellKind::Code)).is_err());
        assert_eq!(
            edit_cells("x\n", 0, CellEdit::Type(CellKind::Markdown)).unwrap(),
            ("# %% [markdown]\nx\n".into(), 1)
        );
    }

    #[test]
    fn splitting_and_joining_cells() {
        let split = edit(4, CellEdit::Split);
        assert_eq!(
            split,
            (
                "# %% id=a\nx = 1\n# %% [markdown] id=b\nhi\n# %% [markdown]\nthere\n# %% id=c\ny\n".into(),
                5
            )
        );
        assert!(edit_cells(CELLS, 2, CellEdit::Split).is_err());
        assert!(edit_cells(CELLS, 3, CellEdit::Split).is_err());
        assert_eq!(
            edit(1, CellEdit::Join),
            ("# %% id=a\nx = 1\nhi\nthere\n# %% id=c\ny\n".into(), 1)
        );
        assert!(edit_cells(CELLS, 6, CellEdit::Join).is_err());
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(128))]
        #[test]
        fn arbitrary_text_serializes_without_panicking(
            text in "(# %%( \\[markdown\\]| \\[raw\\])?( id=[a-z~0-9]{1,3})?\n|[a-zé %#\\[\\]=]{0,8}\n){0,12}"
        ) {
            let (_, doc) = project(JUPYTER).unwrap();
            let out = doc.save(&text).0;
            let (round, _) = project(std::str::from_utf8(&out).unwrap()).unwrap();
            proptest::prop_assert_eq!(cell_spans_text(&round).len(), cell_spans_text(&text).len());
            let _ = lsp_view(&text);
            proptest::prop_assert_eq!(masked(&text, CellKind::Code).len(), text.len());
        }
    }
}
