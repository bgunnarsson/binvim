use crate::buffer::Buffer;
use crossterm::style::Color;
use std::path::Path;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    Json,
    Go,
    Markdown,
}

impl Lang {
    pub fn detect(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        match ext {
            "rs" => Some(Lang::Rust),
            "ts" => Some(Lang::TypeScript),
            "tsx" | "jsx" => Some(Lang::Tsx),
            "json" => Some(Lang::Json),
            "go" => Some(Lang::Go),
            "md" | "markdown" => Some(Lang::Markdown),
            _ => None,
        }
    }

    fn ts_language(self) -> Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::Json => tree_sitter_json::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Markdown => tree_sitter_md::LANGUAGE.into(),
        }
    }

    fn highlights_query(self) -> &'static str {
        match self {
            Lang::Rust => tree_sitter_rust::HIGHLIGHTS_QUERY,
            Lang::TypeScript => tree_sitter_typescript::HIGHLIGHTS_QUERY,
            Lang::Tsx => tree_sitter_typescript::HIGHLIGHTS_QUERY,
            Lang::Json => tree_sitter_json::HIGHLIGHTS_QUERY,
            Lang::Go => tree_sitter_go::HIGHLIGHTS_QUERY,
            Lang::Markdown => tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
        }
    }
}

#[derive(Clone)]
pub struct HighlightCache {
    pub lang: Lang,
    pub buffer_version: u64,
    /// Per-byte foreground colour. `None` means use the terminal default.
    pub byte_colors: Vec<Option<Color>>,
}

pub fn compute_highlights(lang: Lang, buf: &Buffer) -> Option<HighlightCache> {
    let language = lang.ts_language();
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let source = buf.rope.to_string();
    let tree = parser.parse(&source, None)?;
    let query = Query::new(&language, lang.highlights_query()).ok()?;
    let capture_names = query.capture_names();

    let total_bytes = source.len();
    let mut colors: Vec<Option<Color>> = vec![None; total_bytes];

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(m) = matches.next() {
        for capture in m.captures {
            let name = capture_names[capture.index as usize];
            if let Some(color) = capture_to_color(name) {
                let node = capture.node;
                let s = node.start_byte().min(total_bytes);
                let e = node.end_byte().min(total_bytes);
                for slot in &mut colors[s..e] {
                    *slot = Some(color);
                }
            }
        }
    }

    Some(HighlightCache {
        lang,
        buffer_version: buf.version,
        byte_colors: colors,
    })
}

/// Map a tree-sitter capture name to a terminal colour. Returns `None` for capture
/// names we leave as default (operators, punctuation, plain identifiers).
fn capture_to_color(name: &str) -> Option<Color> {
    let head = name.split('.').next().unwrap_or(name);
    match head {
        "comment" => Some(Color::DarkGrey),
        "string" => Some(Color::Green),
        "character" => Some(Color::Green),
        "escape" => Some(Color::DarkRed),
        "keyword" | "include" | "conditional" | "repeat" | "exception" => Some(Color::Magenta),
        "function" | "method" | "macro" => Some(Color::Blue),
        "type" => Some(Color::Cyan),
        "constructor" => Some(Color::Cyan),
        "namespace" | "module" => Some(Color::Cyan),
        "constant" | "boolean" | "number" | "float" => Some(Color::Red),
        "attribute" => Some(Color::Yellow),
        "tag" => Some(Color::Magenta),
        "label" => Some(Color::Yellow),
        "preproc" => Some(Color::DarkYellow),
        "title" => Some(Color::Yellow),
        // Default-coloured: variable, parameter, property, operator, punctuation, text...
        _ => None,
    }
}
