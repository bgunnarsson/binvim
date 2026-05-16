use crossterm::style::Color;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[allow(dead_code)]
    #[serde(default = "default_schema")]
    pub schema_version: u32,
    #[serde(default)]
    pub colors: HashMap<String, String>,
    #[serde(default)]
    pub start_page: StartPageConfig,
    #[serde(default)]
    pub whitespace: WhitespaceConfig,
    #[serde(default)]
    pub line_numbers: LineNumberConfig,
    #[serde(default)]
    pub hover: HoverConfig,
    #[serde(default)]
    pub copilot: CopilotConfig,
    #[serde(default)]
    pub lsp: LspConfig,
}

/// LSP feature toggles. Both default on — semantic tokens layer
/// richer LSP info (mutable bindings, async fns, library symbols)
/// over the tree-sitter highlight cache, and documentHighlight paints
/// every occurrence of the symbol under the cursor with a subtle bg
/// shade. Either can be turned off without affecting other LSP
/// features:
///
/// ```toml
/// [lsp]
/// semantic_tokens = false
/// document_highlight = false
/// ```
#[derive(Debug, Deserialize)]
pub struct LspConfig {
    #[serde(default = "default_lsp_semantic_tokens")]
    pub semantic_tokens: bool,
    #[serde(default = "default_lsp_document_highlight")]
    pub document_highlight: bool,
}

fn default_lsp_semantic_tokens() -> bool {
    true
}
fn default_lsp_document_highlight() -> bool {
    true
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            semantic_tokens: true,
            document_highlight: true,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct StartPageConfig {
    /// Lines to render in place of the baked-in `binvim` logo. Each entry is
    /// drawn on its own row, centered horizontally; the block as a whole is
    /// centered vertically. An empty / missing value falls back to the logo.
    #[serde(default)]
    pub lines: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct WhitespaceConfig {
    /// Render visual markers for tabs and trailing whitespace. On by default
    /// — set `show = false` in the config to turn it off.
    #[serde(default = "default_whitespace_show")]
    pub show: bool,
}

fn default_whitespace_show() -> bool {
    true
}

impl Default for WhitespaceConfig {
    fn default() -> Self {
        Self { show: true }
    }
}

/// Gutter line-number behaviour. Relative numbering (Vim's
/// `set relativenumber`) shows the absolute line on the cursor's row
/// and the *distance* from the cursor on every other row — useful with
/// Vim count-prefixed motions (`5j`, `12k`, `3dd`, …). On by default;
/// set `relative = false` to fall back to plain 1-indexed numbers.
#[derive(Debug, Deserialize)]
pub struct LineNumberConfig {
    #[serde(default = "default_line_numbers_relative")]
    pub relative: bool,
}

fn default_line_numbers_relative() -> bool {
    true
}

impl Default for LineNumberConfig {
    fn default() -> Self {
        Self { relative: true }
    }
}

/// Hover popup behaviour. By default code blocks inside an LSP hover
/// (e.g. a function signature) keep their full original line and the
/// renderer clips at the popup's right edge — that loses the trailing
/// arguments / return type. With `wrap_code = true` (the default), long
/// source lines split at the popup width into multiple rows so wide
/// signatures stay readable without horizontal overflow.
#[derive(Debug, Deserialize)]
pub struct HoverConfig {
    #[serde(default = "default_hover_wrap_code")]
    pub wrap_code: bool,
}

fn default_hover_wrap_code() -> bool {
    true
}

impl Default for HoverConfig {
    fn default() -> Self {
        Self { wrap_code: true }
    }
}

/// GitHub Copilot integration. Off by default — set `enabled = true`
/// under a `[copilot]` block in `~/.config/binvim/config.toml` to
/// attach `copilot-language-server` as an auxiliary LSP for every
/// buffer. Auth lives at `~/.config/github-copilot/hosts.json` and
/// is owned by the language server itself; binvim never sees the
/// token. Sign in via the server's device-flow prompt on first launch.
#[derive(Debug, Default, Deserialize)]
pub struct CopilotConfig {
    #[serde(default)]
    pub enabled: bool,
}

fn default_schema() -> u32 {
    1
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            colors: HashMap::new(),
            start_page: StartPageConfig::default(),
            whitespace: WhitespaceConfig::default(),
            line_numbers: LineNumberConfig::default(),
            hover: HoverConfig::default(),
            copilot: CopilotConfig::default(),
            lsp: LspConfig::default(),
        }
    }
}

impl Config {
    /// Best-effort load of `~/.config/binvim/config.toml`. Returns the default config on
    /// any IO/parse error so a malformed file never breaks the editor.
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Config::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    }

    /// Resolve a colour for a tree-sitter capture name. User values from `[colors]`
    /// override the baked-in defaults.
    ///
    /// Capture names use `.` as a specificity separator — `@variable.parameter`
    /// is "a parameter that's also a variable". User-config lookup walks
    /// longest → shortest (drop the rightmost segment each step) so an
    /// override on `variable` covers `variable.parameter` automatically.
    /// The default palette walks the other way: rightmost segment first so
    /// `string.escape` picks up PINK from "escape" rather than GREEN from
    /// "string", and `variable.parameter` picks up MAROON from "parameter"
    /// rather than `variable`'s deliberate None.
    pub fn color_for_capture(&self, name: &str) -> Option<Color> {
        let segments: Vec<&str> = name.split('.').collect();
        for n in (1..=segments.len()).rev() {
            let candidate = segments[..n].join(".");
            if let Some(c) = self.colors.get(&candidate).and_then(|s| parse_color(s)) {
                return Some(c);
            }
        }
        for seg in segments.iter().rev() {
            if let Some(c) = default_capture_color(seg) {
                return Some(c);
            }
        }
        None
    }
}

fn config_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let path = PathBuf::from(home).join(".config/binvim/config.toml");
    Some(path)
}

fn parse_color(name: &str) -> Option<Color> {
    if let Some(hex) = name.strip_prefix('#') {
        return parse_hex(hex);
    }
    match name {
        "Black" => Some(Color::Black),
        "DarkGrey" | "DarkGray" => Some(Color::DarkGrey),
        "Red" => Some(Color::Red),
        "DarkRed" => Some(Color::DarkRed),
        "Green" => Some(Color::Green),
        "DarkGreen" => Some(Color::DarkGreen),
        "Yellow" => Some(Color::Yellow),
        "DarkYellow" => Some(Color::DarkYellow),
        "Blue" => Some(Color::Blue),
        "DarkBlue" => Some(Color::DarkBlue),
        "Magenta" => Some(Color::Magenta),
        "DarkMagenta" => Some(Color::DarkMagenta),
        "Cyan" => Some(Color::Cyan),
        "DarkCyan" => Some(Color::DarkCyan),
        "White" => Some(Color::White),
        "Grey" | "Gray" => Some(Color::Grey),
        "Reset" | "default" => Some(Color::Reset),
        _ => None,
    }
}

fn parse_hex(s: &str) -> Option<Color> {
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb { r, g, b })
}

// Catppuccin Mocha palette helpers — used as the baked-in defaults so a fresh
// install renders sensibly on first launch.
const CATP_MAUVE: Color = Color::Rgb { r: 0xcb, g: 0xa6, b: 0xf7 };
const CATP_GREEN: Color = Color::Rgb { r: 0xa6, g: 0xe3, b: 0xa1 };
const CATP_BLUE: Color = Color::Rgb { r: 0x89, g: 0xb4, b: 0xfa };
const CATP_YELLOW: Color = Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf };
const CATP_PEACH: Color = Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 };
#[allow(dead_code)]
const CATP_RED: Color = Color::Rgb { r: 0xf3, g: 0x8b, b: 0xa8 };
const CATP_MAROON: Color = Color::Rgb { r: 0xeb, g: 0xa0, b: 0xac };
const CATP_PINK: Color = Color::Rgb { r: 0xf5, g: 0xc2, b: 0xe7 };
const CATP_SKY: Color = Color::Rgb { r: 0x89, g: 0xdc, b: 0xeb };
const CATP_SAPPHIRE: Color = Color::Rgb { r: 0x74, g: 0xc7, b: 0xec };
const CATP_TEAL: Color = Color::Rgb { r: 0x94, g: 0xe2, b: 0xd5 };
const CATP_LAVENDER: Color = Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe };
const CATP_OVERLAY1: Color = Color::Rgb { r: 0x7f, g: 0x84, b: 0x9c };
const CATP_OVERLAY2: Color = Color::Rgb { r: 0x93, g: 0x99, b: 0xb2 };

fn default_capture_color(head: &str) -> Option<Color> {
    match head {
        // LSP semantic-token modifiers. `color_for_capture` walks
        // dotted capture names rightmost-first when looking up
        // defaults, so a token of type "function" with modifier
        // "async" arrives here as just `"async"` after the dotted
        // walk strips the prefix — and a hit here wins over the
        // base "function" default. This is what produces the
        // visible delta vs the tree-sitter pass: plain `function`
        // is still blue (matches tree-sitter exactly, no diff),
        // but `function.async` paints in lavender, `function.defaultLibrary`
        // (e.g. `std::println!`) in sapphire, `variable.mutable`
        // (Rust `let mut`) in red, `variable.constant` / `variable.static`
        // distinctly from local bindings. Only the visually-loud
        // modifiers are listed — noisy ones like `declaration`,
        // `definition`, `documentation`, `abstract` are deliberately
        // absent so the rightmost-first walk falls through to the
        // base type for them.
        "async" => Some(CATP_LAVENDER),
        "mutable" => Some(CATP_RED),
        "static" => Some(CATP_TEAL),
        "readonly" => Some(CATP_PEACH),
        "defaultLibrary" => Some(CATP_SAPPHIRE),
        "deprecated" => Some(CATP_RED),
        "comment" => Some(CATP_OVERLAY1),
        "string" => Some(CATP_GREEN),
        "character" => Some(CATP_TEAL),
        "escape" => Some(CATP_PINK),
        "keyword" | "include" | "conditional" | "repeat" | "exception" => Some(CATP_MAUVE),
        "function" | "method" => Some(CATP_BLUE),
        "macro" => Some(CATP_PEACH),
        "type" => Some(CATP_YELLOW),
        "constructor" => Some(CATP_YELLOW),
        "namespace" | "module" => Some(CATP_YELLOW),
        "constant" | "boolean" | "number" | "float" => Some(CATP_PEACH),
        "operator" => Some(CATP_SKY),
        "attribute" => Some(CATP_YELLOW),
        "tag" => Some(CATP_PINK),
        "label" => Some(CATP_SAPPHIRE),
        "property" | "key" => Some(CATP_LAVENDER),
        "parameter" => Some(CATP_MAROON),
        "variable" => None, // default text colour
        "punctuation" => Some(CATP_OVERLAY2),
        "preproc" => Some(CATP_PEACH),
        "title" => Some(CATP_YELLOW),
        "text" => None,
        "regex" => Some(CATP_PINK),
        _ => None,
    }
}
