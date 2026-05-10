//! Pure response parsers. Each `parse_*_response` takes the JSON `result`
//! field of an LSP reply and produces our internal shape.

use serde_json::Value;
use std::path::PathBuf;

use super::types::{
    CodeActionItem, CompletionItem, InlayHint, LocationItem, SignatureHelp, SymbolItem,
    uri_to_path,
};

pub(super) fn parse_code_actions_response(result: &Value) -> Vec<CodeActionItem> {
    let arr = match result.as_array() {
        Some(a) => a.clone(),
        None => return Vec::new(),
    };
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        // `Command` shape: { title, command, arguments? }
        // `CodeAction` shape: { title, kind?, edit?, command?, disabled? }
        let title = match entry.get("title").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => continue,
        };
        let kind = entry
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let edit = entry.get("edit").cloned();
        let command_field = entry.get("command");
        // CodeAction's `command` is a Command object; bare Command-shaped
        // entries place the command at the top level — both reduce to the
        // same JSON we'll execute later.
        let command = if command_field.map(|v| v.is_object()).unwrap_or(false) {
            command_field.cloned()
        } else if entry.get("command").map(|v| v.is_string()).unwrap_or(false) {
            Some(entry.clone())
        } else {
            None
        };
        let disabled_reason = entry
            .get("disabled")
            .and_then(|v| v.get("reason"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        out.push(CodeActionItem {
            title,
            kind,
            edit,
            command,
            disabled_reason,
        });
    }
    out
}

/// Parse `DocumentSymbol[]` (hierarchical), `SymbolInformation[]` (flat),
/// or `WorkspaceSymbol[]` into our internal shape. Hierarchical entries
/// flatten with their container path joined by `›`.
pub(super) fn parse_symbols_response(result: &Value) -> Vec<SymbolItem> {
    let arr = match result.as_array() {
        Some(a) => a.clone(),
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in arr {
        flatten_symbol(&entry, "", &mut out);
    }
    out
}

fn flatten_symbol(entry: &Value, container: &str, out: &mut Vec<SymbolItem>) {
    let name = entry
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return;
    }
    let kind = entry
        .get("kind")
        .and_then(|v| v.as_u64())
        .map(symbol_kind_label)
        .unwrap_or_else(|| "?".into());
    // DocumentSymbol uses `selectionRange`; SymbolInformation/WorkspaceSymbol
    // uses `location.range`. WorkspaceSymbol may also use `location.uri`
    // without a range.
    let (uri, range) = if let Some(loc) = entry.get("location") {
        let uri = loc.get("uri").and_then(|v| v.as_str()).map(|s| s.to_string());
        let range = loc.get("range").or_else(|| loc.get("targetRange")).cloned();
        (uri, range)
    } else {
        (None, entry.get("selectionRange").or_else(|| entry.get("range")).cloned())
    };
    let start = range
        .as_ref()
        .and_then(|r| r.get("start"))
        .map(|s| {
            (
                s.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                s.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            )
        });
    let path = uri.and_then(|u| uri_to_path(&u));
    if let (Some(path), Some((line, col))) = (path, start) {
        out.push(SymbolItem {
            name: name.clone(),
            container: container.to_string(),
            kind,
            path,
            line,
            col,
        });
    } else if let Some((line, col)) = start {
        // DocumentSymbol with no embedded URI — leave path empty; the
        // caller knows the active buffer's path.
        out.push(SymbolItem {
            name: name.clone(),
            container: container.to_string(),
            kind,
            path: PathBuf::new(),
            line,
            col,
        });
    }
    if let Some(children) = entry.get("children").and_then(|v| v.as_array()) {
        let next_container = if container.is_empty() {
            name.clone()
        } else {
            format!("{container} › {name}")
        };
        for child in children {
            flatten_symbol(child, &next_container, out);
        }
    }
}

fn symbol_kind_label(k: u64) -> String {
    match k {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        15 => "string",
        16 => "number",
        17 => "bool",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "enum-member",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "type-param",
        _ => "?",
    }
    .into()
}

/// Parse a `Location[]` (or `LocationLink[]`) response into our internal
/// shape. Used by `references` and reusable for any future symbol query
/// that returns the same shape.
pub(super) fn parse_locations_response(result: &Value) -> Vec<LocationItem> {
    let arr = match result.as_array() {
        Some(a) => a.clone(),
        None => return Vec::new(),
    };
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        // Either { uri, range: { start: {line, character} } } (Location) or
        // { targetUri, targetSelectionRange: { start: ... } } (LocationLink).
        let uri = entry
            .get("uri")
            .and_then(|v| v.as_str())
            .or_else(|| entry.get("targetUri").and_then(|v| v.as_str()));
        let range = entry
            .get("range")
            .or_else(|| entry.get("targetSelectionRange"))
            .or_else(|| entry.get("targetRange"));
        let (Some(uri), Some(range)) = (uri, range) else { continue };
        let Some(path) = uri_to_path(uri) else { continue };
        let Some(start) = range.get("start") else { continue };
        let Some(line) = start.get("line").and_then(|v| v.as_u64()) else { continue };
        let Some(col) = start.get("character").and_then(|v| v.as_u64()) else { continue };
        out.push(LocationItem {
            path,
            line: line as usize,
            col: col as usize,
        });
    }
    out
}

/// Picks the active signature out of the response and resolves the active
/// parameter range. Servers commonly return a `parameters` array of either
/// `{ label: string }` (a substring of `signature.label`) or
/// `{ label: [start, end] }` (char indices into `signature.label`). Both
/// shapes are handled here.
pub(super) fn parse_signature_help_response(result: &Value) -> Option<SignatureHelp> {
    if result.is_null() {
        return None;
    }
    let sigs = result.get("signatures")?.as_array()?;
    if sigs.is_empty() {
        return None;
    }
    let active_sig = result
        .get("activeSignature")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let sig = sigs.get(active_sig).or_else(|| sigs.first())?;
    let label = sig.get("label")?.as_str()?.to_string();
    let active_param_idx = sig
        .get("activeParameter")
        .and_then(|v| v.as_u64())
        .or_else(|| result.get("activeParameter").and_then(|v| v.as_u64()))
        .map(|n| n as usize);
    let active_param = (|| -> Option<(usize, usize)> {
        let params = sig.get("parameters")?.as_array()?;
        let idx = active_param_idx?;
        let p = params.get(idx)?;
        let plabel = p.get("label")?;
        if let Some(arr) = plabel.as_array() {
            // [start, end] in chars (UTF-16 per spec but we treat chars
            // approximately — close enough for ASCII signatures).
            let start = arr.first()?.as_u64()? as usize;
            let end = arr.get(1)?.as_u64()? as usize;
            return Some((start, end));
        }
        if let Some(needle) = plabel.as_str() {
            // Substring form — find first occurrence inside the label.
            let bytes = label.as_bytes();
            let needle_bytes = needle.as_bytes();
            let pos = bytes
                .windows(needle_bytes.len())
                .position(|w| w == needle_bytes)?;
            // Convert byte pos → char pos.
            let prefix = &label[..pos];
            let cstart = prefix.chars().count();
            let cend = cstart + needle.chars().count();
            return Some((cstart, cend));
        }
        None
    })();
    Some(SignatureHelp { label, active_param })
}

pub(super) fn parse_completion_response(result: &Value) -> Vec<CompletionItem> {
    let arr = if result.is_array() {
        result.as_array().cloned().unwrap_or_default()
    } else if let Some(items) = result.get("items").and_then(|v| v.as_array()) {
        items.clone()
    } else {
        return Vec::new();
    };
    // Don't cap here — the client filters by typed prefix afterwards, and
    // capping at the wire would silently drop relevant items past the cap
    // (typescript-language-server can return several thousand for a top-level
    // identifier position).
    let mut out = Vec::with_capacity(arr.len());
    for item in arr.iter() {
        let label = item
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if label.is_empty() {
            continue;
        }
        let insert_text = item
            .get("insertText")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                item.get("textEdit")
                    .and_then(|t| t.get("newText"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| label.clone());
        let kind = item.get("kind").and_then(|v| v.as_u64()).map(kind_label);
        let detail = item
            .get("detail")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let filter_text = item
            .get("filterText")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| label.clone());
        let sort_text = item
            .get("sortText")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| label.clone());
        // `insertTextFormat`: 1 = PlainText, 2 = Snippet. Some servers also
        // tag `textEdit.insertTextFormat`; fall back to that when the
        // top-level field is absent.
        let is_snippet = item
            .get("insertTextFormat")
            .and_then(|v| v.as_u64())
            .or_else(|| {
                item.get("textEdit")
                    .and_then(|t| t.get("insertTextFormat"))
                    .and_then(|v| v.as_u64())
            })
            .map(|n| n == 2)
            .unwrap_or(false);
        // Capture `textEdit.range` (or `insert`/`replace` from
        // `InsertReplaceEdit`) so the accept path can replace the exact
        // span the server expects instead of guessing client-side.
        let text_edit_range = item
            .get("textEdit")
            .and_then(|t| {
                t.get("range")
                    .or_else(|| t.get("replace"))
                    .or_else(|| t.get("insert"))
            })
            .and_then(|range| {
                let s = range.get("start")?;
                let e = range.get("end")?;
                let s_line = s.get("line")?.as_u64()? as usize;
                let s_col = s.get("character")?.as_u64()? as usize;
                let e_line = e.get("line")?.as_u64()? as usize;
                let e_col = e.get("character")?.as_u64()? as usize;
                Some((s_line, s_col, e_line, e_col))
            });
        out.push(CompletionItem {
            label,
            insert_text,
            kind,
            detail,
            filter_text,
            sort_text,
            is_snippet,
            text_edit_range,
        });
    }
    out
}

fn kind_label(k: u64) -> String {
    // Mapping per LSP spec.
    match k {
        1 => "text",
        2 => "method",
        3 => "function",
        4 => "constructor",
        5 => "field",
        6 => "variable",
        7 => "class",
        8 => "interface",
        9 => "module",
        10 => "property",
        11 => "unit",
        12 => "value",
        13 => "enum",
        14 => "keyword",
        15 => "snippet",
        16 => "color",
        17 => "file",
        18 => "reference",
        19 => "folder",
        20 => "enum-member",
        21 => "constant",
        22 => "struct",
        23 => "event",
        24 => "operator",
        25 => "type-param",
        _ => "?",
    }
    .into()
}

pub(super) fn parse_def_response(result: &Value) -> Option<(PathBuf, usize, usize)> {
    if result.is_null() {
        return None;
    }
    let loc = if result.is_array() {
        result.as_array()?.first()?
    } else {
        result
    };
    // Location | LocationLink — try .uri first, then .targetUri.
    let uri = loc
        .get("uri")
        .and_then(|v| v.as_str())
        .or_else(|| loc.get("targetUri").and_then(|v| v.as_str()))?;
    let path = uri_to_path(uri)?;
    let range = loc
        .get("range")
        .or_else(|| loc.get("targetSelectionRange"))
        .or_else(|| loc.get("targetRange"))?;
    let start = range.get("start")?;
    let line = start.get("line")?.as_u64()? as usize;
    let col = start.get("character")?.as_u64()? as usize;
    Some((path, line, col))
}

/// Parse `textDocument/inlayHint` response. The LSP spec allows the
/// `label` field to be either a string or an array of `InlayHintLabelPart`
/// objects; we flatten the latter into a single string and ignore part
/// metadata (tooltips, command refs) for now.
pub(super) fn parse_inlay_hints_response(result: &Value) -> Vec<InlayHint> {
    let arr = match result.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        let Some(pos) = entry.get("position") else { continue };
        let line = pos.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let col = pos.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let label = match entry.get("label") {
            Some(v) if v.is_string() => v.as_str().unwrap_or("").to_string(),
            Some(v) if v.is_array() => {
                let mut s = String::new();
                for part in v.as_array().unwrap() {
                    if let Some(t) = part.get("value").and_then(|v| v.as_str()) {
                        s.push_str(t);
                    }
                }
                s
            }
            _ => continue,
        };
        if label.is_empty() {
            continue;
        }
        let kind = entry.get("kind").and_then(|v| v.as_u64()).unwrap_or(1) as u8;
        out.push(InlayHint { line, col, label, kind });
    }
    out
}

pub(super) fn parse_hover_response(result: &Value) -> Option<String> {
    if result.is_null() {
        return None;
    }
    let contents = result.get("contents")?;
    if let Some(s) = contents.as_str() {
        return Some(s.to_string());
    }
    if let Some(obj) = contents.as_object() {
        if let Some(v) = obj.get("value").and_then(|v| v.as_str()) {
            return Some(v.to_string());
        }
    }
    if let Some(arr) = contents.as_array() {
        let mut out = String::new();
        for item in arr {
            let s = item
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| item.get("value").and_then(|v| v.as_str()).map(|s| s.to_string()));
            if let Some(s) = s {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&s);
            }
        }
        if !out.is_empty() {
            return Some(out);
        }
    }
    None
}
