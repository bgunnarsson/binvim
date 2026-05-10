use crate::buffer::Buffer;
use crate::config::Config;
use crossterm::style::Color;
use std::path::Path;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Json,
    Go,
    Html,
    Css,
    Markdown,
    CSharp,
    Bash,
}

impl Lang {
    pub fn detect(path: &Path) -> Option<Self> {
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            match ext {
                "rs" => return Some(Lang::Rust),
                "ts" => return Some(Lang::TypeScript),
                "tsx" => return Some(Lang::Tsx),
                "jsx" | "js" | "mjs" | "cjs" => return Some(Lang::JavaScript),
                "json" | "jsonc" => return Some(Lang::Json),
                "go" => return Some(Lang::Go),
                "html" | "htm" | "cshtml" | "razor" => return Some(Lang::Html),
                "css" | "scss" | "less" => return Some(Lang::Css),
                "md" | "markdown" => return Some(Lang::Markdown),
                "cs" => return Some(Lang::CSharp),
                "sh" | "bash" | "zsh" | "ksh" => return Some(Lang::Bash),
                _ => {}
            }
        }
        // Shell rc/profile files have no extension — Path::extension returns
        // None for names like `.zshrc` because there's only a leading dot.
        // tree-sitter-bash handles zsh/ksh syntax well enough as a fallback.
        let name = path.file_name().and_then(|s| s.to_str())?;
        match name {
            ".zshrc" | ".zprofile" | ".zshenv" | ".zlogin" | ".zlogout"
            | ".bashrc" | ".bash_profile" | ".bash_login" | ".bash_logout"
            | ".profile" | ".kshrc" => Some(Lang::Bash),
            _ => None,
        }
    }

    /// Map a markdown code-fence tag (`typescript`, `rs`, `c#`, …) to one
    /// of our supported languages. Used by the hover popup when an LSP
    /// returns markdown with fenced code blocks. Case-insensitive; handles
    /// the common short and long aliases LSP servers actually emit.
    pub fn from_md_tag(tag: &str) -> Option<Self> {
        match tag.trim().to_ascii_lowercase().as_str() {
            "rust" | "rs" => Some(Lang::Rust),
            "typescript" | "ts" => Some(Lang::TypeScript),
            "tsx" | "typescriptreact" => Some(Lang::Tsx),
            "javascript" | "js" | "jsx" | "javascriptreact" | "mjs" | "cjs" => {
                Some(Lang::JavaScript)
            }
            "json" | "jsonc" => Some(Lang::Json),
            "go" | "golang" => Some(Lang::Go),
            "html" | "htm" | "xhtml" | "cshtml" | "razor" => Some(Lang::Html),
            "css" | "scss" | "sass" | "less" => Some(Lang::Css),
            "markdown" | "md" => Some(Lang::Markdown),
            "csharp" | "cs" | "c#" => Some(Lang::CSharp),
            "bash" | "sh" | "shell" | "zsh" | "ksh" => Some(Lang::Bash),
            _ => None,
        }
    }

    fn ts_language(self) -> Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::Json => tree_sitter_json::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Html => tree_sitter_html::LANGUAGE.into(),
            Lang::Css => tree_sitter_css::LANGUAGE.into(),
            Lang::Markdown => tree_sitter_md::LANGUAGE.into(),
            Lang::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Lang::Bash => tree_sitter_bash::LANGUAGE.into(),
        }
    }

    fn highlights_query(self) -> String {
        match self {
            Lang::Rust => tree_sitter_rust::HIGHLIGHTS_QUERY.into(),
            // tree-sitter-typescript ships only a TS-specific overlay (5 captures).
            // Combine with the tree-sitter-javascript query for full coverage.
            Lang::TypeScript | Lang::Tsx => format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
            Lang::JavaScript => tree_sitter_javascript::HIGHLIGHT_QUERY.into(),
            // Replace the bundled tree-sitter-json query — its pattern
            // order (specific @string.special.key BEFORE general
            // @string) is incompatible with how compute_highlights
            // priorities work (later pattern wins), so keys ended up
            // recoloured by the @string rule. This rewrite puts the
            // general string rule first and adds punctuation captures
            // so braces/brackets/colons/commas render in the muted
            // overlay tone like every other language.
            Lang::Json => r#"
(string) @string

(pair
  key: (_) @string.special.key)

(number) @number

[
  (null)
  (true)
  (false)
] @constant.builtin

(escape_sequence) @escape

(comment) @comment

[
  "{"
  "}"
  "["
  "]"
] @punctuation.bracket

[
  ":"
  ","
] @punctuation.delimiter
"#
            .into(),
            Lang::Go => tree_sitter_go::HIGHLIGHTS_QUERY.into(),
            Lang::Html => tree_sitter_html::HIGHLIGHTS_QUERY.into(),
            Lang::Css => tree_sitter_css::HIGHLIGHTS_QUERY.into(),
            Lang::Markdown => tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into(),
            Lang::CSharp => tree_sitter_c_sharp::HIGHLIGHTS_QUERY.into(),
            Lang::Bash => tree_sitter_bash::HIGHLIGHT_QUERY.into(),
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

pub fn compute_highlights(lang: Lang, buf: &Buffer, config: &Config) -> Option<HighlightCache> {
    let source = buf.rope.to_string();
    let colors = compute_byte_colors(lang, &source, config)?;
    Some(HighlightCache {
        lang,
        buffer_version: buf.version,
        byte_colors: colors,
    })
}

/// Run tree-sitter highlighting over a raw source string and return the
/// per-byte foreground colour map. Reused by the hover popup for fenced
/// code blocks where there's no underlying `Buffer`. See `compute_highlights`
/// for the priority-resolution rationale that this shares.
pub fn compute_byte_colors(lang: Lang, source: &str, config: &Config) -> Option<Vec<Option<Color>>> {
    let language = lang.ts_language();
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(source, None)?;
    let query_src = lang.highlights_query();
    let query = Query::new(&language, &query_src).ok()?;
    let capture_names = query.capture_names();

    let total_bytes = source.len();
    let mut colors: Vec<Option<Color>> = vec![None; total_bytes];
    // Per-byte priority. Tree-sitter highlight queries follow a well-known
    // convention: general patterns come first (e.g. `(identifier) @variable`)
    // and specific ones override them later (`(method_declaration name:
    // (identifier) @function)`). We treat `pattern_index` as the priority
    // — later patterns win for any byte they touch. Without this the result
    // depended on iterator ordering, which left method names and types
    // sometimes coloured as plain identifiers in C# and other languages.
    let mut byte_priority: Vec<u16> = vec![0; total_bytes];

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(m) = matches.next() {
        let priority = (m.pattern_index as u16).saturating_add(1);
        for capture in m.captures {
            let name = capture_names[capture.index as usize];
            if let Some(color) = config.color_for_capture(name) {
                let node = capture.node;
                let s = node.start_byte().min(total_bytes);
                let e = node.end_byte().min(total_bytes);
                for i in s..e {
                    if priority >= byte_priority[i] {
                        colors[i] = Some(color);
                        byte_priority[i] = priority;
                    }
                }
            }
        }
    }
    Some(colors)
}
