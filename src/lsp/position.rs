//! Position-encoding conversion between the editor's char columns and the
//! unit a server counts `Position.character` in. The client offers
//! `utf-32` (char indices — no conversion) first, but a server is free to
//! pick `utf-8`, and one that answers nothing means `utf-16`. Columns are
//! converted at the manager boundary so everything past it — parsers,
//! `LspEvent`s, the app — stays in chars.

use ropey::Rope;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::types::uri_to_path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PositionEncoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}

impl PositionEncoding {
    /// Reads `capabilities.positionEncoding` from an `initialize` result.
    /// An absent or unknown value is the spec's default, UTF-16.
    pub(super) fn from_init_result(result: &Value) -> Self {
        match result
            .get("capabilities")
            .and_then(|c| c.get("positionEncoding"))
            .and_then(|v| v.as_str())
        {
            Some("utf-8") => Self::Utf8,
            Some("utf-32") => Self::Utf32,
            _ => Self::Utf16,
        }
    }

    fn units(self, c: char) -> usize {
        match self {
            Self::Utf8 => c.len_utf8(),
            Self::Utf16 => c.len_utf16(),
            Self::Utf32 => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
    ToChar,
    ToWire,
}

/// Wire column → char column within one line. A column that lands inside
/// a multi-unit char resolves to that char; one past the line's end keeps
/// its excess, so an out-of-range position stays out of range rather than
/// being silently pulled onto the last char.
pub(super) fn wire_to_char(
    line: impl Iterator<Item = char>,
    col: usize,
    enc: PositionEncoding,
) -> usize {
    let mut units = 0;
    let mut idx = 0;
    for c in line.take_while(|&c| c != '\n' && c != '\r') {
        let next = units + enc.units(c);
        if next > col {
            return idx;
        }
        units = next;
        idx += 1;
    }
    idx + (col - units.min(col))
}

/// Char column → wire column within one line; the inverse of
/// [`wire_to_char`], with the same treatment of columns past the end.
pub(super) fn char_to_wire(
    line: impl Iterator<Item = char>,
    col: usize,
    enc: PositionEncoding,
) -> usize {
    let mut units = 0;
    let mut idx = 0;
    for c in line.take_while(|&c| c != '\n' && c != '\r').take(col) {
        units += enc.units(c);
        idx += 1;
    }
    units + (col - idx)
}

/// Converts positions for one client's encoding. Line text comes from the
/// documents the manager has sent the servers (what the server counted
/// against), falling back to disk for files that were never opened — a
/// rename or references reply routinely names those. Disk reads are cached
/// for the converter's lifetime, one reply.
pub(super) struct PosConv<'a> {
    enc: PositionEncoding,
    docs: &'a HashMap<PathBuf, Rope>,
    disk: HashMap<PathBuf, Option<Rope>>,
}

impl<'a> PosConv<'a> {
    pub(super) fn new(enc: PositionEncoding, docs: &'a HashMap<PathBuf, Rope>) -> Self {
        Self {
            enc,
            docs,
            disk: HashMap::new(),
        }
    }

    pub(super) fn is_identity(&self) -> bool {
        self.enc == PositionEncoding::Utf32
    }

    /// Convert `col` on `line` of `path`. A line the document doesn't have
    /// leaves the column as it is.
    pub(super) fn col(&mut self, path: &Path, line: usize, col: usize, dir: Direction) -> usize {
        if self.is_identity() {
            return col;
        }
        let enc = self.enc;
        let Some(rope) = self.rope(path) else { return col };
        if line >= rope.len_lines() {
            return col;
        }
        let chars = rope.line(line).chars();
        match dir {
            Direction::ToChar => wire_to_char(chars, col, enc),
            Direction::ToWire => char_to_wire(chars, col, enc),
        }
    }

    fn rope(&mut self, path: &Path) -> Option<&Rope> {
        if let Some(r) = lookup_doc(self.docs, path) {
            return Some(r);
        }
        self.disk
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                std::fs::read_to_string(path)
                    .ok()
                    .map(|s| Rope::from_str(&s))
            })
            .as_ref()
    }

    /// Rewrite every `Position` (an object with numeric `line` and
    /// `character`) under `value`. The document a position belongs to is
    /// the nearest enclosing `uri` / `targetUri` / `textDocument.uri` or
    /// `changes` key, else `default_path` — the request's own document.
    /// `data`, `command` and `arguments` are skipped: the server reads
    /// those back verbatim, in its own units.
    pub(super) fn convert_value(
        &mut self,
        value: &mut Value,
        default_path: Option<&Path>,
        dir: Direction,
    ) {
        if self.is_identity() {
            return;
        }
        self.walk(
            value,
            default_path.map(Path::to_path_buf),
            default_path,
            dir,
        );
    }

    fn walk(
        &mut self,
        value: &mut Value,
        path: Option<PathBuf>,
        origin: Option<&Path>,
        dir: Direction,
    ) {
        match value {
            Value::Array(items) => {
                for item in items {
                    self.walk(item, path.clone(), origin, dir);
                }
            }
            Value::Object(map) => {
                if let (Some(line), Some(col)) = (
                    map.get("line").and_then(Value::as_u64),
                    map.get("character").and_then(Value::as_u64),
                ) {
                    if let Some(p) = &path {
                        let converted = self.col(p, line as usize, col as usize, dir);
                        map.insert("character".into(), Value::from(converted));
                    }
                    return;
                }
                let own = map
                    .get("uri")
                    .or_else(|| map.get("targetUri"))
                    .or_else(|| map.get("textDocument").and_then(|t| t.get("uri")))
                    .and_then(Value::as_str)
                    .and_then(uri_to_path);
                let path = own.or(path);
                for (key, child) in map.iter_mut() {
                    match key.as_str() {
                        "data" | "command" | "arguments" => {}
                        "changes" => {
                            if let Value::Object(by_uri) = child {
                                for (uri, edits) in by_uri.iter_mut() {
                                    self.walk(edits, uri_to_path(uri), origin, dir);
                                }
                            }
                        }
                        // A `LocationLink`'s origin range is in the
                        // requesting document, not the target.
                        "originSelectionRange" => {
                            self.walk(child, origin.map(Path::to_path_buf), origin, dir)
                        }
                        _ => self.walk(child, path.clone(), origin, dir),
                    }
                }
            }
            _ => {}
        }
    }
}

/// Exact key first, then the canonical path — servers often report a
/// symlink-resolved path for a file the editor opened through a symlink.
pub(super) fn lookup_doc<'d>(docs: &'d HashMap<PathBuf, Rope>, path: &Path) -> Option<&'d Rope> {
    if let Some(r) = docs.get(path) {
        return Some(r);
    }
    docs.get(&path.canonicalize().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use PositionEncoding::{Utf8, Utf16, Utf32};

    #[test]
    fn ascii_is_identity() {
        for enc in [Utf8, Utf16, Utf32] {
            assert_eq!(wire_to_char("hello".chars(), 3, enc), 3);
            assert_eq!(char_to_wire("hello".chars(), 3, enc), 3);
        }
    }

    #[test]
    fn multibyte_columns_round_trip() {
        // `é` is 2 UTF-8 bytes / 1 UTF-16 unit; `😀` is 4 bytes / 2 units.
        let line = "é😀x";
        assert_eq!(char_to_wire(line.chars(), 2, Utf8), 6);
        assert_eq!(char_to_wire(line.chars(), 2, Utf16), 3);
        assert_eq!(wire_to_char(line.chars(), 6, Utf8), 2);
        assert_eq!(wire_to_char(line.chars(), 3, Utf16), 2);
        assert_eq!(wire_to_char(line.chars(), 2, Utf32), 2);
    }

    #[test]
    fn column_inside_a_char_resolves_to_that_char() {
        assert_eq!(wire_to_char("😀x".chars(), 1, Utf16), 0);
        assert_eq!(wire_to_char("😀x".chars(), 3, Utf8), 0);
    }

    #[test]
    fn column_past_line_end_keeps_its_excess() {
        assert_eq!(wire_to_char("é\n".chars(), 4, Utf8), 3);
        assert_eq!(char_to_wire("é\n".chars(), 3, Utf8), 4);
    }

    #[test]
    fn encoding_from_init_result() {
        let r = |e: &str| json!({ "capabilities": { "positionEncoding": e } });
        assert_eq!(PositionEncoding::from_init_result(&r("utf-8")), Utf8);
        assert_eq!(PositionEncoding::from_init_result(&r("utf-32")), Utf32);
        assert_eq!(
            PositionEncoding::from_init_result(&json!({ "capabilities": {} })),
            Utf16
        );
    }

    fn docs_with(path: &str, text: &str) -> HashMap<PathBuf, Rope> {
        HashMap::from([(PathBuf::from(path), Rope::from_str(text))])
    }

    #[test]
    fn workspace_edit_changes_convert_against_their_own_document() {
        let docs = docs_with("/w/a.rs", "let é = 1;\nlet b = é;\n");
        let mut conv = PosConv::new(Utf16, &docs);
        let mut edit = json!({ "changes": { "file:///w/a.rs": [
            { "range": { "start": { "line": 1, "character": 8 }, "end": { "line": 1, "character": 9 } }, "newText": "c" }
        ] } });
        conv.convert_value(&mut edit, None, Direction::ToChar);
        assert_eq!(
            edit["changes"]["file:///w/a.rs"][0]["range"]["start"]["character"],
            8
        );

        let docs = docs_with("/w/a.rs", "é😀 = x;\n");
        let mut conv = PosConv::new(Utf8, &docs);
        let mut edit = json!({ "documentChanges": [{
            "textDocument": { "uri": "file:///w/a.rs", "version": 1 },
            "edits": [{ "range": { "start": { "line": 0, "character": 9 }, "end": { "line": 0, "character": 10 } }, "newText": "y" }]
        }] });
        conv.convert_value(&mut edit, None, Direction::ToChar);
        let range = &edit["documentChanges"][0]["edits"][0]["range"];
        assert_eq!(range["start"]["character"], 5);
        assert_eq!(range["end"]["character"], 6);
    }

    #[test]
    fn bare_ranges_use_the_request_document_and_data_is_left_alone() {
        let docs = docs_with("/w/a.ts", "é.foo\n");
        let mut conv = PosConv::new(Utf8, &docs);
        let mut items = json!([{
            "textEdit": { "range": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 6 } } },
            "data": { "position": { "line": 0, "character": 3 } }
        }]);
        conv.convert_value(&mut items, Some(Path::new("/w/a.ts")), Direction::ToChar);
        assert_eq!(items[0]["textEdit"]["range"]["start"]["character"], 2);
        assert_eq!(items[0]["textEdit"]["range"]["end"]["character"], 5);
        assert_eq!(items[0]["data"]["position"]["character"], 3);
    }

    #[test]
    fn unknown_document_leaves_columns_alone() {
        let docs = HashMap::new();
        let mut conv = PosConv::new(Utf8, &docs);
        assert_eq!(
            conv.col(Path::new("/nonexistent/x.rs"), 0, 7, Direction::ToChar),
            7
        );
    }
}
