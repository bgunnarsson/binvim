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
    /// `.yml` / `.yaml` — tree-sitter-yaml with the bundled query.
    Yaml,
    /// `.xml` / `.csproj` / `.fsproj` / `.vbproj` / `.props` / `.targets` /
    /// `.config` / `.manifest` / `.nuspec` / `.resx`. tree-sitter-xml.
    Xml,
    /// `.editorconfig` — INI-style, no tree-sitter grammar fits cleanly
    /// (TOML rejects `[*.cs]` section headers). Highlighted by a small
    /// byte-level scanner: `#`/`;` comments, `[section]` patterns,
    /// `key = value` pairs.
    EditorConfig,
    /// `.gitignore` / `.gitattributes` / `.dockerignore` / `.npmignore` —
    /// glob pattern lists. Byte-level scanner: `#` comments, `!`-negation
    /// prefix, the bare patterns themselves.
    GitIgnore,
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
                "yml" | "yaml" => return Some(Lang::Yaml),
                // XML family — the project-file flavours all use the
                // same MSBuild XML schema. Covers .NET (csproj/fsproj/
                // vbproj/props/targets), .NET config files (.config,
                // app.manifest, *.nuspec, *.resx), and plain .xml.
                "xml" | "csproj" | "fsproj" | "vbproj" | "props" | "targets" | "config"
                | "manifest" | "nuspec" | "resx" | "xaml" | "xhtml" | "xsd" | "xsl"
                | "xslt" | "plist" => return Some(Lang::Xml),
                _ => {}
            }
        }
        // Bare-name files (no extension): match by basename. Most
        // dotfiles land here.
        let name = path.file_name().and_then(|s| s.to_str())?;
        if matches!(
            name,
            ".zshrc" | ".zprofile" | ".zshenv" | ".zlogin" | ".zlogout"
            | ".bashrc" | ".bash_profile" | ".bash_login" | ".bash_logout"
            | ".profile" | ".kshrc"
        ) {
            return Some(Lang::Bash);
        }
        if matches!(name, ".editorconfig") {
            return Some(Lang::EditorConfig);
        }
        if matches!(
            name,
            ".gitignore" | ".gitattributes" | ".dockerignore" | ".npmignore"
        ) {
            return Some(Lang::GitIgnore);
        }
        None
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
            "yaml" | "yml" => Some(Lang::Yaml),
            "xml" | "csproj" | "fsproj" | "vbproj" | "xaml" => Some(Lang::Xml),
            "editorconfig" | "ini" => Some(Lang::EditorConfig),
            "gitignore" | "ignore" => Some(Lang::GitIgnore),
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
            Lang::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Lang::Xml => tree_sitter_xml::LANGUAGE_XML.into(),
            // EditorConfig / GitIgnore have no real tree-sitter
            // grammar; we use bash's lexer as a stand-in because it
            // gets us comment tokenisation for free. The actual
            // structural highlighting comes from byte-level overlays
            // in `compute_byte_colors`.
            Lang::EditorConfig | Lang::GitIgnore => tree_sitter_bash::LANGUAGE.into(),
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
            // tree-sitter-css's bundled query lumps `class_name`,
            // `id_name`, `namespace_name`, `property_name`, and
            // `feature_name` all under `@property`. Result: in a rule
            // like `.foo { color: red; }`, the selector `.foo` reads
            // the same Lavender as the `color` property — visually
            // indistinguishable. Override with a query that splits
            // them: classes get @constructor (Yellow), ids get @label
            // (Sapphire), pseudo-states stay @attribute, and property
            // names keep the original Lavender @property tone.
            Lang::Css => CSS_QUERY_OVERRIDE.into(),
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
            Lang::Yaml => tree_sitter_yaml::HIGHLIGHTS_QUERY.into(),
            Lang::Xml => tree_sitter_xml::XML_HIGHLIGHT_QUERY.into(),
            // Empty query — tree-sitter pass is a no-op; the actual
            // colouring comes from byte-level scanners in
            // `compute_byte_colors`.
            Lang::EditorConfig | Lang::GitIgnore => String::new(),
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
/// Replacement for `tree_sitter_css::HIGHLIGHTS_QUERY`. The upstream
/// query paints class names, id names, namespace names, property names,
/// and feature names with the same `@property` capture, so a rule like
/// `.foo { color: red; }` renders `.foo` and `color` in the same
/// Lavender — visually indistinguishable. This version splits them:
///
/// - `.class-name`         → `@constructor` (Yellow)
/// - `#id-name`            → `@label` (Sapphire)
/// - `property: …`         → `@property` (Lavender)
/// - `@media` etc.         → `@keyword` (Mauve)
/// - functions like `rgb(` → `@function` (Blue)
/// - `--custom-property`   → `@variable` (default text)
///
/// Most of the rest of the bundled query is preserved verbatim — the
/// rewrite is targeted at the class-vs-property collision.
const CSS_QUERY_OVERRIDE: &str = r#"
(comment) @comment

(tag_name) @tag
(nesting_selector) @tag
(universal_selector) @tag

"~" @operator
">" @operator
"+" @operator
"-" @operator
"*" @operator
"/" @operator
"=" @operator
"^=" @operator
"|=" @operator
"~=" @operator
"$=" @operator
"*=" @operator

"and" @keyword
"or" @keyword
"not" @keyword
"only" @keyword

(attribute_selector (plain_value) @string)
(pseudo_element_selector (tag_name) @attribute)
(pseudo_class_selector (class_name) @attribute)

; Selectors — different tones so `.foo`, `#bar`, and the
; `property: value` line all read distinctly.
(class_name) @constructor
(id_name) @label
(namespace_name) @namespace
(property_name) @property
(feature_name) @property

; Custom properties (`--x: …`) and `var(--x)` references — variable
; tone so they read as user-defined values rather than built-ins.
((property_name) @variable
 (#match? @variable "^--"))

(attribute_name) @attribute

(function_name) @function

[
  "@media"
  "@import"
  "@charset"
  "@namespace"
  "@supports"
  "@keyframes"
  (at_keyword)
  (to)
  (from)
  (important)
] @keyword

(string_value) @string
(color_value) @constant
(integer_value) @number
(float_value) @number
(unit) @type
(plain_value) @constant
"#;

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

; JSX fragments `<>…</>` — paint the < / > / </ / > tokens as @tag so
; they read consistently with named elements. Without this they fall
; through to whatever generic punctuation rule the JS query emits.
; jsx_fragment for `<>…</>` would belong here but isn't a node type
; in tree-sitter-typescript 0.23 / tree-sitter-javascript 0.23 —
; adding it makes Query::new fail and wipes the highlight cache for
; every .tsx / .jsx file.

; JSX expression containers `{expr}` — the braces are JSX-template
; syntax, not an object literal, so paint them with the operator tone
; to set them apart from the object-literal braces that surround them.
(jsx_expression "{" @operator)
(jsx_expression "}" @operator)
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
(element "</" @punctuation.bracket)
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
        apply_razor_overlay(source.as_bytes(), &mut colors, config);
    }
    if matches!(lang, Lang::EditorConfig | Lang::GitIgnore) {
        apply_line_oriented_overlay(lang, source.as_bytes(), &mut colors, config);
    }
    Some(colors)
}

/// Byte-level scan that fills in the colours tree-sitter-razor can't reach.
/// The grammar uses anonymous lexer rules for `_tag_name` /
/// `_html_attribute_name`, and on real-world Razor files (Tailwind brackets
/// in `class` values, BOM headers, etc.) parse errors cascade and leave
/// whole `<div>` openers as loose `<` / `=` / `>` tokens inside an ERROR
/// node — no `element` wrapper to query against. A pure byte-level pass
/// catches both cases uniformly, and only paints bytes the main query
/// left uncoloured, so anything tree-sitter *did* reach keeps its colour.
/// Byte-level scanner for the two line-oriented formats without a real
/// tree-sitter grammar.
///
/// - `EditorConfig`: `#` / `;` comments, `[pattern]` section headers,
///   `key = value` pairs.
/// - `GitIgnore`-family: `#` comments, `!`-negation prefix, the bare
///   patterns themselves.
fn apply_line_oriented_overlay(
    lang: Lang,
    source: &[u8],
    colors: &mut [Option<Color>],
    config: &Config,
) {
    let comment_color = config.color_for_capture("comment");
    let section_color = config.color_for_capture("tag");
    let key_color = config.color_for_capture("property");
    let string_color = config.color_for_capture("string");
    let operator_color = config.color_for_capture("operator");
    let neg_color = config.color_for_capture("keyword");

    let paint = |colors: &mut [Option<Color>], range: std::ops::Range<usize>, c: Option<Color>| {
        if let Some(c) = c {
            for k in range {
                if k < colors.len() && colors[k].is_none() {
                    colors[k] = Some(c);
                }
            }
        }
    };

    let mut line_start = 0usize;
    while line_start < source.len() {
        // Find end-of-line.
        let mut eol = line_start;
        while eol < source.len() && source[eol] != b'\n' {
            eol += 1;
        }
        // Trim leading whitespace inside the logical line.
        let mut content_start = line_start;
        while content_start < eol && matches!(source[content_start], b' ' | b'\t') {
            content_start += 1;
        }
        if content_start == eol {
            line_start = eol + 1;
            continue;
        }
        let first = source[content_start];

        // Comments — `#` (always) and `;` (.editorconfig only).
        let is_comment = first == b'#' || (lang == Lang::EditorConfig && first == b';');
        if is_comment {
            paint(colors, content_start..eol, comment_color);
            line_start = eol + 1;
            continue;
        }

        match lang {
            Lang::EditorConfig => {
                if first == b'[' {
                    // Section header `[pattern]` — paint the whole run.
                    paint(colors, content_start..eol, section_color);
                } else {
                    // `key = value` — key + `=` + value, painted
                    // separately so the value reads as a string.
                    if let Some(eq) = source[content_start..eol]
                        .iter()
                        .position(|b| *b == b'=' || *b == b':')
                    {
                        let key_end = content_start + eq;
                        // Strip trailing whitespace from key span.
                        let mut k_end = key_end;
                        while k_end > content_start
                            && matches!(source[k_end - 1], b' ' | b'\t')
                        {
                            k_end -= 1;
                        }
                        paint(colors, content_start..k_end, key_color);
                        paint(colors, key_end..key_end + 1, operator_color);
                        // Skip whitespace after the `=` for the value.
                        let mut val_start = key_end + 1;
                        while val_start < eol
                            && matches!(source[val_start], b' ' | b'\t')
                        {
                            val_start += 1;
                        }
                        paint(colors, val_start..eol, string_color);
                    } else {
                        // Stray bareword line (root = true is the only
                        // valid example, but that has an `=`). Treat as
                        // a key with no value.
                        paint(colors, content_start..eol, key_color);
                    }
                }
            }
            Lang::GitIgnore => {
                // `!` negation prefix — paint as a keyword so the
                // user sees "this UN-ignores".
                if first == b'!' {
                    paint(colors, content_start..content_start + 1, neg_color);
                    paint(colors, content_start + 1..eol, key_color);
                } else {
                    paint(colors, content_start..eol, key_color);
                }
            }
            _ => {}
        }
        line_start = eol + 1;
    }
}

fn apply_razor_overlay(source: &[u8], colors: &mut [Option<Color>], config: &Config) {
    let tag_color = config.color_for_capture("tag");
    let attr_color = config.color_for_capture("attribute");
    let kw_color = config.color_for_capture("keyword");
    let str_color = config.color_for_capture("string");

    let paint = |colors: &mut [Option<Color>], s: usize, e: usize, c: Option<Color>| {
        if let Some(c) = c {
            for k in s..e {
                if colors[k].is_none() {
                    colors[k] = Some(c);
                }
            }
        }
    };
    let is_name = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':');
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';

    // C# / Razor control-flow keywords we want coloured even when the
    // parse is broken enough that the C# query never reaches them.
    // Type-name keywords (`int`, `string`, …) and modifiers (`public`,
    // `static`, …) are included so an entire C# block reads consistently
    // in regions where the grammar gave up.
    const KEYWORDS: &[&[u8]] = &[
        b"if", b"else", b"for", b"foreach", b"while", b"do",
        b"try", b"catch", b"finally", b"switch", b"case", b"default",
        b"return", b"break", b"continue", b"throw", b"new", b"yield",
        b"var", b"null", b"true", b"false", b"this", b"base",
        b"public", b"private", b"protected", b"internal",
        b"static", b"readonly", b"const", b"async", b"await",
        b"using", b"namespace", b"class", b"interface", b"struct", b"record",
        b"void", b"int", b"long", b"short", b"byte", b"string",
        b"bool", b"double", b"float", b"decimal", b"object", b"dynamic",
        b"override", b"virtual", b"abstract", b"sealed", b"partial",
        b"is", b"as", b"in", b"out", b"ref", b"params", b"typeof",
        b"get", b"set", b"add", b"remove",
    ];

    let mut i = 0;
    while i < source.len() {
        let b = source[i];

        // `<tagname` or `</tagname`. Paint the run after `<` (or `</`) as a
        // tag name. Doesn't check whether the `<` itself is uncoloured —
        // the C# query captures `<` as @operator everywhere it appears,
        // so the gate would skip every tag opener.
        if b == b'<' && i + 1 < source.len() {
            let mut j = i + 1;
            if source[j] == b'/' {
                j += 1;
            }
            let name_start = j;
            while j < source.len() && is_name(source[j]) {
                j += 1;
            }
            if j > name_start {
                paint(colors, name_start, j, tag_color);
                i = j;
                continue;
            }
        }

        // String literal `"…"` — colour the whole run including quotes.
        // Useful for HTML attribute values, which are anonymous in the
        // grammar and so don't get captured as `(string_literal) @string`.
        // The scan stops at a newline to avoid over-running on broken
        // unterminated strings.
        if b == b'"' && colors[i].is_none() {
            let mut j = i + 1;
            while j < source.len() && source[j] != b'"' && source[j] != b'\n' {
                j += 1;
            }
            if j < source.len() && source[j] == b'"' {
                paint(colors, i, j + 1, str_color);
                i = j + 1;
                continue;
            }
        }

        // Word starts: identifier run that's not glued to a preceding word
        // char. We try, in order: attribute name (`word="`), then C#
        // keyword. Anything else just advances past the word.
        let at_word_start = is_name(b) && (i == 0 || !is_name(source[i - 1]));
        if at_word_start {
            let name_start = i;
            let mut j = i;
            while j < source.len() && is_name(source[j]) {
                j += 1;
            }
            // `word="` — attribute name. Require `=` *and* an immediately
            // following `"` to avoid mis-painting C# assignments
            // (`x = 5`), which have whitespace or non-`"` chars after `=`.
            if j + 1 < source.len() && source[j] == b'=' && source[j + 1] == b'"' {
                paint(colors, name_start, j, attr_color);
                i = j;
                continue;
            }
            // C# keyword. Strict word boundaries on both sides — already
            // ensured on the left by `at_word_start`; check the right by
            // requiring the word to be terminated by a non-name byte
            // (rules out `if-loaded` matching the `if` keyword).
            let word_end_clean = j == source.len() || !is_name(source[j]);
            if word_end_clean {
                let word = &source[name_start..j];
                // Strip a trailing `-`/`:` — `is_name` lets those run,
                // but C# tokens never contain them, so trim before the
                // keyword lookup.
                let word_clean = {
                    let mut e = word.len();
                    while e > 0 && !is_word(word[e - 1]) {
                        e -= 1;
                    }
                    &word[..e]
                };
                if !word_clean.is_empty() && KEYWORDS.contains(&word_clean) {
                    paint(colors, name_start, name_start + word_clean.len(), kw_color);
                }
            }
            i = j;
            continue;
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// Find the byte offset of a whole-word occurrence of `word` in
    /// `source` — bounded by non-identifier chars on both sides so
    /// `if (organization` doesn't accidentally match `notify`.
    fn find_word(source: &str, word: &str) -> Option<usize> {
        let is_id = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        let bytes = source.as_bytes();
        let mut start = 0;
        while let Some(off) = source[start..].find(word) {
            let i = start + off;
            let left_ok = i == 0 || !is_id(bytes[i - 1]);
            let right_ok = i + word.len() >= bytes.len() || !is_id(bytes[i + word.len()]);
            if left_ok && right_ok {
                return Some(i);
            }
            start = i + word.len();
        }
        None
    }

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
    /// MMSHeader.cshtml has a BOM and class attribute values that include
    /// `[12px]` brackets — both push the Razor grammar into long ERROR
    /// regions, so `<div class="…">` openers and the `else` / `if`
    /// keywords inside `@if/else` bodies never become proper tokens.
    /// The byte-level overlay should still paint them.
    #[test]
    fn razor_e2e_mmsheader_broken_regions() {
        let p = std::path::Path::new(
            "/Users/bgunnarsson/Development/mms-namsefni/Vettvangur.Site/Views/Partials/MMS/Components/Header/MMSHeader.cshtml",
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
        let mauve = Color::Rgb { r: 0xcb, g: 0xa6, b: 0xf7 };

        // The first broken `<div class="…px-[12px]…">` opener.
        let div_idx = source.find("<div class=\"mms-header px-").unwrap() + 1;
        assert_eq!(
            cache.byte_colors.get(div_idx).copied().flatten(),
            Some(pink),
            "broken `<div` tag name should be Pink",
        );
        let class_idx = div_idx + 4; // skip `div `
        assert_eq!(
            cache.byte_colors.get(class_idx).copied().flatten(),
            Some(yellow),
            "broken `class` attribute name should be Yellow",
        );

        // `else` inside @if/else body — bare token, no parent node.
        // The repo's indentation can be either tabs or spaces depending on
        // whether the user's editorconfig has re-indented since the last
        // save, so search for the `else` keyword by content rather than
        // matching a specific leading-whitespace run.
        let else_idx = find_word(&source, "else").expect("else keyword");
        assert_eq!(
            cache.byte_colors.get(else_idx).copied().flatten(),
            Some(mauve),
            "Razor `else` should pick up keyword Mauve via the byte-level fallback",
        );

        // C# `if` inside the else body — find the `if (organization` site
        // specifically so we don't pick up the @if at the top.
        let if_idx = source.find("if (organization").expect("if (organization");
        assert_eq!(
            cache.byte_colors.get(if_idx).copied().flatten(),
            Some(mauve),
            "C# `if` in broken region should also be Mauve",
        );
    }

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
        // Closing tag names too — these come after `</`. Opening and
        // closing tags should match in colour for visual balance.
        let pink = Color::Rgb { r: 0xf5, g: 0xc2, b: 0xe7 };
        for closer in ["</section>", "</div>", "</partial>"] {
            if let Some(idx) = src.find(closer) {
                let name_start = idx + 2; // skip `</`
                assert_eq!(
                    colors[name_start],
                    Some(pink),
                    "closing-tag name in `{}` should be Pink",
                    closer,
                );
            }
        }
    }

    #[test]
    fn yaml_highlights_keys_and_values() {
        let src = "name: hello\nversion: 1.2.3\nlist:\n  - foo\n  - bar\n";
        let cfg = Config::default();
        let colors = compute_byte_colors(Lang::Yaml, src, &cfg).expect("highlight ok");
        // `name` should be coloured (block_mapping_pair key in the
        // bundled query gets some capture). We don't pin the exact tone,
        // just check the first char is non-None.
        let name_idx = src.find("name").unwrap();
        assert!(colors[name_idx].is_some(), "yaml key `name` should be coloured");
    }

    #[test]
    fn xml_highlights_csproj_tags() {
        let src = r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>net8.0</TargetFramework>
  </PropertyGroup>
</Project>"#;
        let cfg = Config::default();
        let colors = compute_byte_colors(Lang::Xml, src, &cfg).expect("highlight ok");
        let tag_idx = src.find("Project").unwrap();
        assert!(colors[tag_idx].is_some(), "xml `Project` tag should be coloured");
        let attr_idx = src.find("Sdk").unwrap();
        assert!(colors[attr_idx].is_some(), "xml `Sdk` attribute should be coloured");
    }

    #[test]
    fn editorconfig_sections_and_pairs_get_distinct_colours() {
        let src = "# A comment\nroot = true\n\n[*.cs]\nindent_style = space\nindent_size = 4\n";
        let cfg = Config::default();
        let colors = compute_byte_colors(Lang::EditorConfig, src, &cfg).expect("highlight ok");
        let comment_idx = src.find("# A comment").unwrap();
        let section_idx = src.find("[*.cs]").unwrap();
        let key_idx = src.find("indent_style").unwrap();
        let value_idx = src.find("space").unwrap();
        let comment = colors[comment_idx];
        let section = colors[section_idx];
        let key = colors[key_idx];
        let value = colors[value_idx];
        assert!(comment.is_some(), "# comment should be coloured");
        assert!(section.is_some(), "[*.cs] section should be coloured");
        assert!(key.is_some(), "key `indent_style` should be coloured");
        assert!(value.is_some(), "value `space` should be coloured");
        // Section / key / value should pick up distinct colours.
        assert_ne!(section, key, "section and key should differ");
        assert_ne!(key, value, "key and value should differ");
    }

    #[test]
    fn gitignore_negation_and_patterns_get_coloured() {
        let src = "# Build output\nbin/\nobj/\n!keep-this.txt\n";
        let cfg = Config::default();
        let colors = compute_byte_colors(Lang::GitIgnore, src, &cfg).expect("highlight ok");
        let comment_idx = src.find("# Build").unwrap();
        let pattern_idx = src.find("bin/").unwrap();
        let bang_idx = src.find('!').unwrap();
        assert!(colors[comment_idx].is_some());
        assert!(colors[pattern_idx].is_some());
        assert!(colors[bang_idx].is_some());
        assert_ne!(colors[comment_idx], colors[bang_idx]);
    }

    #[test]
    fn tsx_highlight_cache_is_populated() {
        // Regression: any unknown node type in the JSX overlay (e.g.
        // `jsx_fragment` from older binvim versions) made `Query::new`
        // fail and the whole TSX highlight cache came back empty, so
        // every char rendered as plain text. Make sure a representative
        // TSX snippet still gets meaningful colour after the overlay
        // runs.
        let src = r#"import { Foo } from "./foo";
export function Page() {
    const items = [1, 2, 3];
    return (
        <div className="card">
            {items.map(n => <span key={n}>{n}</span>)}
        </div>
    );
}
"#;
        let cfg = Config::default();
        let colors = compute_byte_colors(Lang::Tsx, src, &cfg).expect("tsx highlight cache");
        // `import` keyword should be coloured.
        assert!(
            colors[src.find("import").unwrap()].is_some(),
            "TSX keyword `import` should be coloured"
        );
        // `Page` (function declaration) should be coloured.
        assert!(
            colors[src.find("Page").unwrap()].is_some(),
            "TSX function `Page` should be coloured"
        );
        // `div` (lowercase JSX tag) should be coloured (Pink @tag).
        assert!(
            colors[src.find("<div").unwrap() + 1].is_some(),
            "TSX `<div>` should be coloured"
        );
    }

    #[test]
    fn jsx_highlight_cache_is_populated() {
        // Same regression for .jsx — JavaScript grammar plus JSX overlay.
        let src = "export const Page = () => <div className=\"c\">{x}</div>;\n";
        let cfg = Config::default();
        let colors = compute_byte_colors(Lang::JavaScript, src, &cfg).expect("js highlight cache");
        assert!(
            colors[src.find("export").unwrap()].is_some(),
            "JSX keyword `export` should be coloured"
        );
        assert!(
            colors[src.find("<div").unwrap() + 1].is_some(),
            "JSX `<div>` should be coloured"
        );
    }

    #[test]
    fn css_class_and_property_get_different_colours() {
        let src = ".my-class { color: red; padding: 10px; }\n#my-id { display: block; }\n";
        let cfg = Config::default();
        let colors = compute_byte_colors(Lang::Css, src, &cfg).expect("highlight ok");
        let class_idx = src.find("my-class").unwrap();
        let id_idx = src.find("my-id").unwrap();
        let property_idx = src.find("color").unwrap();
        let class_c = colors[class_idx];
        let id_c = colors[id_idx];
        let property_c = colors[property_idx];
        assert!(class_c.is_some() && id_c.is_some() && property_c.is_some());
        assert_ne!(class_c, property_c, "class and property should differ");
        assert_ne!(id_c, property_c, "id and property should differ");
    }
}
