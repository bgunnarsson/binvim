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
    /// Razor (`.cshtml` / `.razor`) — tree-sitter-razor extends the C#
    /// grammar with Razor's `@`-prefixed directives, code blocks, and
    /// implicit/explicit expressions. We pair its tree with the C# highlight
    /// query plus a Razor overlay so C# inside `@{}`/`@if`/`@expr` actually
    /// gets coloured, instead of falling out as plain text under the HTML
    /// grammar.
    Razor,
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
                "html" | "htm" => return Some(Lang::Html),
                "cshtml" | "razor" => return Some(Lang::Razor),
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
            "html" | "htm" | "xhtml" => Some(Lang::Html),
            "cshtml" | "razor" => Some(Lang::Razor),
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
            Lang::Razor => tree_sitter_razor::LANGUAGE.into(),
            Lang::Bash => tree_sitter_bash::LANGUAGE.into(),
        }
    }

    fn highlights_query(self) -> String {
        match self {
            Lang::Rust => tree_sitter_rust::HIGHLIGHTS_QUERY.into(),
            // tree-sitter-typescript ships only a TS-specific overlay (5
            // captures). Combine with the tree-sitter-javascript query for
            // full coverage. *Pure* TypeScript's grammar has no JSX nodes,
            // so the JSX overlay would fail `Query::new` and wipe the cache.
            Lang::TypeScript => format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            ),
            // TSX + JS both have JSX nodes — layer the JSX overlay so HTML
            // tags get `@tag` and component-style tags get `@constructor`
            // instead of the bundled query's catch-all `@variable`.
            Lang::Tsx => format!(
                "{}\n{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                JSX_OVERLAY_QUERY,
            ),
            Lang::JavaScript => format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                JSX_OVERLAY_QUERY,
            ),
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
            // Razor's grammar extends C#, so the bundled C# query already
            // matches every C# node inside `@{}`, `@if`, `@(expr)`, etc.
            // The Razor overlay tags the `@`-marker directives and the
            // Razor / HTML comment nodes that the C# query has no view of.
            Lang::Razor => format!(
                "{}\n{}",
                tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
                RAZOR_OVERLAY_QUERY,
            ),
            Lang::Bash => tree_sitter_bash::HIGHLIGHT_QUERY.into(),
        }
    }
}

/// Extra tree-sitter highlight captures layered onto the JS / TS / TSX
/// queries. The bundled tree-sitter-javascript query categorises every
/// JSX element name as a generic identifier, so HTML tags (`<div>`,
/// `<main>`) and React components (`<Foo>`) end up rendered like any
/// other variable — visually unhelpful. This overlay tags them per the
/// upstream tree-sitter conventions: lowercase JSX names → `@tag`,
/// PascalCase JSX names → `@constructor`. We also tag JSX attribute
/// names so `className=` reads as a property rather than a variable
/// reference.
const JSX_OVERLAY_QUERY: &str = r#"
; HTML-style tag names (lowercase) — open + close + self-closing.
((jsx_opening_element
   name: (identifier) @tag)
 (#match? @tag "^[a-z]"))

((jsx_closing_element
   name: (identifier) @tag)
 (#match? @tag "^[a-z]"))

((jsx_self_closing_element
   name: (identifier) @tag)
 (#match? @tag "^[a-z]"))

; Component-style tag names (PascalCase).
((jsx_opening_element
   name: (identifier) @constructor)
 (#match? @constructor "^[A-Z]"))

((jsx_closing_element
   name: (identifier) @constructor)
 (#match? @constructor "^[A-Z]"))

((jsx_self_closing_element
   name: (identifier) @constructor)
 (#match? @constructor "^[A-Z]"))

; Member-access components (`Foo.Bar`).
(jsx_opening_element
  name: (member_expression
    object: (identifier) @constructor
    property: (property_identifier) @constructor))

(jsx_closing_element
  name: (member_expression
    object: (identifier) @constructor
    property: (property_identifier) @constructor))

(jsx_self_closing_element
  name: (member_expression
    object: (identifier) @constructor
    property: (property_identifier) @constructor))

; Attribute names — `className=`, `onClick=`, etc.
(jsx_attribute (property_identifier) @attribute)
"#;

/// Extra captures layered on top of the C# highlight query for Razor files.
/// The `at_*` nodes are aliases the grammar attaches to the `@`-prefixed
/// keyword sequences (`@inject`, `@if`, `@{`, `@(...)`, `@*…*@` opener, …) —
/// matching them as `@keyword.directive` paints both the `@` and the keyword
/// in the same Mauve tone, the way an LSP highlighter would. HTML tag and
/// attribute names are produced by anonymous lexer rules in the grammar so
/// they aren't reachable from a tree-sitter query; those are handled by the
/// `apply_razor_html_overlay` regex post-pass below.
const RAZOR_OVERLAY_QUERY: &str = r#"
; `at_*` are anonymous string aliases in the grammar (`alias(seq("@", "if"),
; "at_if")`), so they need string-literal query syntax, not named-node parens.
[
  "at_page"
  "at_using"
  "at_model"
  "at_inherits"
  "at_layout"
  "at_attribute"
  "at_implements"
  "at_typeparam"
  "at_inject"
  "at_namespace"
  "at_rendermode"
  "at_preservewhitespace"
  "at_block"
  "at_section"
  "at_explicit"
  "at_implicit"
  "at_await"
  "at_lock"
  "at_if"
  "at_try"
  "at_switch"
  "at_for"
  "at_foreach"
  "at_while"
  "at_do"
  "at_colon_transition"
  "at_at_escape"
] @keyword.directive

(razor_comment) @comment
(html_comment) @comment

; HTML element delimiters and the attribute `=` — the tag name and
; attribute name themselves are anonymous in the grammar, so we colour
; them in a regex post-pass instead. Painting the brackets here gives
; the structural cue even before that pass runs.
(element "<" @punctuation.bracket)
(element ">" @punctuation.bracket)
(element "/>" @punctuation.bracket)
(element "=" @operator)
"#;

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
    if lang == Lang::Razor {
        apply_razor_html_overlay(&tree, source.as_bytes(), &mut colors, config);
    }
    Some(colors)
}

/// HTML tag and attribute names are produced by anonymous lexer rules in
/// tree-sitter-razor (`_tag_name`, `_html_attribute_name`), so they don't
/// appear as queryable nodes — only their surrounding `<`/`>`/`=` do.
/// Walk the parsed tree, find each `element`, and colour the tag name (the
/// run of `[A-Za-z0-9_:-]+` right after `<` or `</`) plus every attribute
/// name (a run before `=`). Only paints over uncoloured bytes so any nested
/// Razor expression already coloured by the main query stays put.
fn apply_razor_html_overlay(
    tree: &tree_sitter::Tree,
    source: &[u8],
    colors: &mut [Option<Color>],
    config: &Config,
) {
    let tag_color = config.color_for_capture("tag");
    let attr_color = config.color_for_capture("attribute");
    if tag_color.is_none() && attr_color.is_none() {
        return;
    }
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "element" {
            colour_element_names(node, source, colors, tag_color, attr_color);
            // Nested elements still need their own pass — keep walking.
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
}

fn colour_element_names(
    node: tree_sitter::Node,
    source: &[u8],
    colors: &mut [Option<Color>],
    tag_color: Option<Color>,
    attr_color: Option<Color>,
) {
    let start = node.start_byte();
    let end = node.end_byte().min(source.len());
    if end <= start {
        return;
    }
    // Build the set of byte ranges that belong to direct child nodes — for
    // an `element`, those are nested elements, Razor expressions, the
    // `_end_tag` etc. We want to paint within `element`'s direct text only,
    // skipping any descendant's bytes so we never overwrite C#/Razor colours.
    let mut child_ranges: Vec<(usize, usize)> = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        // The literal `<`/`>`/`/>`/`=` tokens are 1-2 bytes and contain no
        // identifier we'd want to colour — keeping them in `child_ranges`
        // would just create gaps in the scan, so allow them.
        let kind = child.kind();
        if matches!(kind, "<" | ">" | "/>" | "=" | "\"") {
            continue;
        }
        child_ranges.push((child.start_byte(), child.end_byte().min(source.len())));
    }
    child_ranges.sort_by_key(|r| r.0);
    let in_child = |pos: usize| -> bool {
        child_ranges.iter().any(|(s, e)| pos >= *s && pos < *e)
    };
    let is_name_byte = |b: u8| -> bool {
        b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':')
    };
    // Tag name: right after a `<` or `</`, skipping descendant bytes.
    let mut i = start;
    while i < end {
        if in_child(i) {
            i += 1;
            continue;
        }
        let b = source[i];
        if b == b'<' {
            let mut j = i + 1;
            if j < end && source[j] == b'/' {
                j += 1;
            }
            let name_start = j;
            while j < end && !in_child(j) && is_name_byte(source[j]) {
                j += 1;
            }
            if j > name_start {
                if let Some(color) = tag_color {
                    for k in name_start..j {
                        if colors[k].is_none() {
                            colors[k] = Some(color);
                        }
                    }
                }
            }
            i = j.max(i + 1);
            continue;
        }
        // Attribute name: a run of name chars immediately followed by `=`.
        if is_name_byte(b) {
            let name_start = i;
            let mut j = i;
            while j < end && !in_child(j) && is_name_byte(source[j]) {
                j += 1;
            }
            if j < end && !in_child(j) && source[j] == b'=' {
                if let Some(color) = attr_color {
                    for k in name_start..j {
                        if colors[k].is_none() {
                            colors[k] = Some(color);
                        }
                    }
                }
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn razor_detects_cshtml() {
        assert_eq!(
            Lang::detect(std::path::Path::new("foo.cshtml")),
            Some(Lang::Razor)
        );
        assert_eq!(
            Lang::detect(std::path::Path::new("Bar.razor")),
            Some(Lang::Razor)
        );
        // Plain HTML still goes to the HTML grammar — Razor's tag-name
        // overlay is regex-based and would underperform tree-sitter-html
        // on files that don't need any Razor support.
        assert_eq!(
            Lang::detect(std::path::Path::new("page.html")),
            Some(Lang::Html)
        );
    }

    #[test]
    fn razor_colours_directives_and_csharp_blocks() {
        let src = r#"@inject IFoo foo
@using Some.Namespace
@{
    var x = "hello";
    if (x.Length > 0) { return; }
}
<div class="card"><span>@x</span></div>
"#;
        let cfg = Config::default();
        let colors = compute_byte_colors(Lang::Razor, src, &cfg).expect("highlight ok");
        // `@inject` at byte 0..7 should be coloured as a directive.
        assert!(colors[0].is_some(), "@ in @inject should be coloured");
        assert!(colors[5].is_some(), "inject keyword should be coloured");
        // `var` is a C# keyword inside the @{} block.
        let var_idx = src.find("var").unwrap();
        assert!(
            colors[var_idx].is_some(),
            "`var` inside @{{}} should pick up C# keyword colour"
        );
        // The string literal `"hello"` should be coloured.
        let hello_idx = src.find("\"hello\"").unwrap();
        assert!(
            colors[hello_idx + 1].is_some(),
            "`hello` inside string literal should be coloured"
        );
        // Tag name `div` and attribute `class` should be picked up by the
        // regex post-pass. (Skip if the grammar misparses; the post-pass
        // only paints over uncoloured bytes anyway.)
        if let Some(div_idx) = src.find("<div") {
            assert!(
                colors[div_idx + 1].is_some(),
                "HTML tag name `div` should be coloured via the post-pass"
            );
        }
    }

    /// End-to-end smoke: load the user's actual file from disk via the
    /// same path the editor takes (`Buffer::from_path`, which strips CRLF)
    /// and verify that the resulting highlight cache colours the tag and
    /// attribute names. Skipped on machines that don't have the project.
    #[test]
    fn razor_e2e_real_cshtml() {
        let p = std::path::Path::new(
            "/Users/bgunnarsson/Development/mms-namsefni/Vettvangur.Site/Views/ProductCategory.cshtml",
        );
        if !p.exists() {
            return;
        }
        let buf = crate::buffer::Buffer::from_path(p.to_path_buf()).expect("load");
        let cfg = Config::default();
        let cache = compute_highlights(Lang::Razor, &buf, &cfg).expect("highlight");
        let source = buf.rope.to_string();
        let pink = Color::Rgb { r: 0xf5, g: 0xc2, b: 0xe7 };
        let yellow = Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf };
        let section_idx = source.find("<section").unwrap() + 1;
        assert_eq!(
            cache.byte_colors.get(section_idx).copied().flatten(),
            Some(pink),
            "tag name `section` should be Pink",
        );
        let class_idx = source.find(" class=").unwrap() + 1;
        assert_eq!(
            cache.byte_colors.get(class_idx).copied().flatten(),
            Some(yellow),
            "attribute `class=` should be Yellow",
        );
    }

    /// Regression for the screenshot the user shared — even when the
    /// grammar throws ERROR nodes on the Tailwind `class="… pt-[60px]"`
    /// substring, the surrounding `<section>` / `<div>` elements still
    /// parse and their tag/attribute names should still light up.
    #[test]
    fn razor_colours_tags_despite_bracket_attrs() {
        let src = r#"@{
    Layout = "Master.cshtml";
}

<section class="store-category pt-[60px]">
    <div class="wrapper">
        <partial name="MMS/Components/Headline" />
    </div>
</section>
"#;
        let cfg = Config::default();
        let colors = compute_byte_colors(Lang::Razor, src, &cfg).expect("highlight ok");
        for name in ["section", "div", "partial"] {
            let idx = src.find(&format!("<{}", name)).unwrap() + 1;
            assert!(
                colors[idx].is_some(),
                "tag name `{}` (byte {}) should be coloured",
                name,
                idx,
            );
            assert!(
                colors[idx + name.len() - 1].is_some(),
                "last char of `{}` should be coloured",
                name,
            );
        }
        // Attribute names too — these come right before the `=`.
        for attr in ["class", "name"] {
            let pat = format!(" {}=", attr);
            let idx = src.find(&pat).unwrap() + 1; // skip leading space
            assert!(
                colors[idx].is_some(),
                "attribute `{}` should be coloured",
                attr,
            );
        }
    }
}
