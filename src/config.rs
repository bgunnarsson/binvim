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

    /// Optional editor background. When set via `[colors] background = "#…"`
    /// (or a named colour), the buffer body, gutter, and empty rows paint
    /// against this colour instead of the terminal's default background.
    /// Themes in `./themes/` ship this so switching theme also switches the
    /// background; leave it unset for a transparent buffer that inherits the
    /// terminal's own background.
    pub fn background_color(&self) -> Option<Color> {
        self.colors.get("background").and_then(|s| parse_color(s))
    }

    /// Background for editor *chrome* — popups, status line, tab bar, side
    /// panes. Set with `[colors] chrome_bg = "#…"`. When unset and a
    /// `background` is configured, this derives a slightly *darker* shade so
    /// chrome stays visually distinct from the buffer (dark themes: 15%
    /// toward black; light themes: 5% toward black — matches the
    /// Mantle/Base relationship Catppuccin uses). With no `background`
    /// set at all, falls through to Catppuccin Mantle so chrome always
    /// has a concrete colour against any terminal.
    pub fn chrome_bg(&self) -> Color {
        if let Some(c) = self.user_color("chrome_bg") {
            return c;
        }
        match self.background_color() {
            Some(bg) if is_dark(bg) => mix(bg, Color::Rgb { r: 0x00, g: 0x00, b: 0x00 }, 0.15),
            Some(bg) => mix(bg, Color::Rgb { r: 0x00, g: 0x00, b: 0x00 }, 0.05),
            None => Color::Rgb { r: 0x18, g: 0x18, b: 0x25 },
        }
    }

    /// Generic per-key palette lookup with a default fallback. The chrome
    /// helpers below all funnel through this so every theme.toml entry that
    /// uses a recognised key wins over its baked-in Catppuccin default; an
    /// unrecognised key just falls through unchanged.
    fn theme_color(&self, key: &str, default: Color) -> Color {
        self.colors
            .get(key)
            .and_then(|s| parse_color(s))
            .unwrap_or(default)
    }

    /// Explicit `[colors]` override for `key`, ignoring the baked-in defaults.
    /// Returns None when the user hasn't set it — used by the chrome neutrals
    /// (surface, border, foreground, dim) so we can auto-derive from the
    /// configured background instead of falling back to Catppuccin tones that
    /// only look right against a dark theme.
    fn user_color(&self, key: &str) -> Option<Color> {
        self.colors.get(key).and_then(|s| parse_color(s))
    }

    /// Main fg colour for chrome text — status segments, popup body, dashboard
    /// text. Capture-name lookups in `[colors]` still drive syntax colouring;
    /// `foreground` controls chrome only. When the user only sets `background`
    /// we auto-derive a near-white (dark bg) or near-black (light bg) so a
    /// one-line theme still gives readable chrome text.
    pub fn theme_fg(&self) -> Color {
        if let Some(c) = self.user_color("foreground") {
            return c;
        }
        match self.background_color() {
            Some(bg) if is_dark(bg) => Color::Rgb { r: 0xcd, g: 0xd6, b: 0xf4 },
            Some(_) => Color::Rgb { r: 0x4c, g: 0x4f, b: 0x69 },
            None => Color::Rgb { r: 0xcd, g: 0xd6, b: 0xf4 },
        }
    }

    /// Muted secondary text — line numbers (relative), inlay hints, comments
    /// in markdown sections, footer hints, copilot ghost. Auto-derives toward
    /// a mid-grey that contrasts with the configured background.
    pub fn theme_dim(&self) -> Color {
        if let Some(c) = self.user_color("dim") {
            return c;
        }
        match self.background_color() {
            Some(bg) if is_dark(bg) => mix(bg, Color::Rgb { r: 0xff, g: 0xff, b: 0xff }, 0.45),
            Some(bg) => mix(bg, Color::Rgb { r: 0x00, g: 0x00, b: 0x00 }, 0.45),
            None => Color::Rgb { r: 0x6c, g: 0x70, b: 0x86 },
        }
    }

    /// Emphasised chrome text — active tab fg, multi-cursor marker, picker
    /// title, hover heading, whichkey title. Stays a vivid accent regardless
    /// of background so it always reads as "this is highlighted."
    pub fn theme_emphasis(&self) -> Color {
        self.theme_color("emphasis", Color::Rgb { r: 0xb4, g: 0xbe, b: 0xfe })
    }

    /// Layered chrome surface — sits above the chrome bg and is used for
    /// active tab bg, status branch chip, picker row selection, debug-pane
    /// row selection. Derived as a small step from the background toward
    /// white (dark theme) or black (light theme) so it always reads as
    /// "one layer above the bg."
    pub fn theme_surface(&self) -> Color {
        if let Some(c) = self.user_color("surface") {
            return c;
        }
        match self.background_color() {
            Some(bg) if is_dark(bg) => mix(bg, Color::Rgb { r: 0xff, g: 0xff, b: 0xff }, 0.12),
            Some(bg) => mix(bg, Color::Rgb { r: 0x00, g: 0x00, b: 0x00 }, 0.10),
            None => Color::Rgb { r: 0x45, g: 0x47, b: 0x5a },
        }
    }

    /// Borders, dividers, popup outlines, and subtle highlight backgrounds
    /// (document-highlight, match-pair). One step further from the bg than
    /// `surface` so popup outlines visually separate from the surface they
    /// sit on.
    pub fn theme_border(&self) -> Color {
        if let Some(c) = self.user_color("border") {
            return c;
        }
        match self.background_color() {
            Some(bg) if is_dark(bg) => mix(bg, Color::Rgb { r: 0xff, g: 0xff, b: 0xff }, 0.22),
            Some(bg) => mix(bg, Color::Rgb { r: 0x00, g: 0x00, b: 0x00 }, 0.18),
            None => Color::Rgb { r: 0x58, g: 0x5b, b: 0x70 },
        }
    }

    /// Primary chrome accent — debug-pane chip bg, breakpoint marker,
    /// active-terminal-tab bg, dirty-buffer dot in the tab bar.
    pub fn theme_accent(&self) -> Color {
        self.theme_color("accent", Color::Rgb { r: 0xfa, g: 0xb3, b: 0x87 })
    }

    /// Secondary chrome accent — terminal-pane chip bg, active sub-tab in
    /// the debug pane.
    pub fn theme_accent_secondary(&self) -> Color {
        self.theme_color("accent_secondary", Color::Rgb { r: 0xa6, g: 0xe3, b: 0xa1 })
    }

    /// Text colour on top of brightly-coloured chips (terminal / debug
    /// chips, active tab in terminal pane). Should contrast with all the
    /// accent colours.
    pub fn theme_chip_fg(&self) -> Color {
        self.theme_color("chip_fg", Color::Rgb { r: 0x1e, g: 0x1e, b: 0x2e })
    }

    /// Error severity — diagnostics, deleted hunks, breakpoint dots.
    pub fn theme_error(&self) -> Color {
        self.theme_color("error", Color::Rgb { r: 0xf3, g: 0x8b, b: 0xa8 })
    }

    /// Warning severity — diagnostics, modified hunks, search highlight bg.
    pub fn theme_warning(&self) -> Color {
        self.theme_color("warning", Color::Rgb { r: 0xf9, g: 0xe2, b: 0xaf })
    }

    /// Info severity — diagnostics, statusline mode chip default tint.
    pub fn theme_info(&self) -> Color {
        self.theme_color("info", Color::Rgb { r: 0x89, g: 0xb4, b: 0xfa })
    }

    /// Hint severity — diagnostics.
    pub fn theme_hint(&self) -> Color {
        self.theme_color("hint", Color::Rgb { r: 0x89, g: 0xdc, b: 0xeb })
    }

    /// Helper that returns the user's override for a specific dotted-namespace
    /// key (e.g. `notification.error`, `tab.active_bg`) or falls through to a
    /// caller-provided default. The defaults below all funnel through this so
    /// every namespaced setting respects the broader theme key it falls back
    /// to: setting `error = "..."` alone re-tints every diagnostic + notification
    /// glyph; setting `notification.error = "..."` overrides only that one
    /// surface.
    fn theme_override(&self, key: &str) -> Option<Color> {
        self.colors.get(key).and_then(|s| parse_color(s))
    }

    // ── Notifications ────────────────────────────────────────────────────
    pub fn notification_info(&self) -> Color {
        self.theme_override("notification.info").unwrap_or_else(|| self.theme_info())
    }
    pub fn notification_warning(&self) -> Color {
        self.theme_override("notification.warning").unwrap_or_else(|| self.theme_warning())
    }
    pub fn notification_success(&self) -> Color {
        self.theme_override("notification.success").unwrap_or_else(|| self.theme_accent_secondary())
    }
    pub fn notification_error(&self) -> Color {
        self.theme_override("notification.error").unwrap_or_else(|| self.theme_error())
    }

    // ── Git stripe ───────────────────────────────────────────────────────
    pub fn git_added(&self) -> Color {
        self.theme_override("git.added").unwrap_or_else(|| self.theme_accent_secondary())
    }
    pub fn git_modified(&self) -> Color {
        self.theme_override("git.modified").unwrap_or_else(|| self.theme_warning())
    }
    pub fn git_deleted(&self) -> Color {
        self.theme_override("git.deleted").unwrap_or_else(|| self.theme_error())
    }

    // ── Diagnostics (LSP) ────────────────────────────────────────────────
    pub fn diagnostic_error(&self) -> Color {
        self.theme_override("diagnostic.error").unwrap_or_else(|| self.theme_error())
    }
    pub fn diagnostic_warning(&self) -> Color {
        self.theme_override("diagnostic.warning").unwrap_or_else(|| self.theme_warning())
    }
    pub fn diagnostic_info(&self) -> Color {
        self.theme_override("diagnostic.info").unwrap_or_else(|| self.theme_info())
    }
    pub fn diagnostic_hint(&self) -> Color {
        self.theme_override("diagnostic.hint").unwrap_or_else(|| self.theme_hint())
    }

    // ── Tab bar ──────────────────────────────────────────────────────────
    pub fn tab_active_bg(&self) -> Color {
        self.theme_override("tab.active_bg").unwrap_or_else(|| self.theme_surface())
    }
    pub fn tab_active_fg(&self) -> Color {
        self.theme_override("tab.active_fg").unwrap_or_else(|| self.theme_emphasis())
    }
    pub fn tab_inactive_fg(&self) -> Color {
        self.theme_override("tab.inactive_fg").unwrap_or_else(|| self.theme_dim())
    }
    pub fn tab_dirty(&self) -> Color {
        self.theme_override("tab.dirty").unwrap_or_else(|| self.theme_accent())
    }
    pub fn tab_close(&self) -> Color {
        self.theme_override("tab.close").unwrap_or_else(|| self.theme_dim())
    }

    // ── Terminal pane ────────────────────────────────────────────────────
    pub fn terminal_chip_bg(&self) -> Color {
        self.theme_override("terminal.chip_bg").unwrap_or_else(|| self.theme_accent_secondary())
    }
    pub fn terminal_chip_fg(&self) -> Color {
        self.theme_override("terminal.chip_fg").unwrap_or_else(|| self.theme_chip_fg())
    }
    pub fn terminal_active_tab_bg(&self) -> Color {
        self.theme_override("terminal.active_tab_bg").unwrap_or_else(|| self.theme_accent())
    }

    // ── Debug pane ───────────────────────────────────────────────────────
    pub fn debug_chip_bg(&self) -> Color {
        self.theme_override("debug.chip_bg").unwrap_or_else(|| self.theme_accent())
    }
    pub fn debug_active_tab_bg(&self) -> Color {
        self.theme_override("debug.active_tab_bg").unwrap_or_else(|| self.theme_accent_secondary())
    }

    // ── Gutter signs ─────────────────────────────────────────────────────
    pub fn gutter_breakpoint(&self) -> Color {
        self.theme_override("gutter.breakpoint").unwrap_or_else(|| self.theme_error())
    }
    pub fn gutter_pc_marker(&self) -> Color {
        self.theme_override("gutter.pc_marker").unwrap_or_else(|| self.theme_accent())
    }

    // ── Buffer overlays ──────────────────────────────────────────────────
    pub fn search_highlight_bg(&self) -> Color {
        self.theme_override("search.highlight_bg").unwrap_or_else(|| self.theme_warning())
    }
    pub fn yank_flash_bg(&self) -> Color {
        self.theme_override("yank.flash_bg").unwrap_or_else(|| self.theme_accent())
    }
    pub fn multi_cursor_bg(&self) -> Color {
        self.theme_override("multi_cursor.bg").unwrap_or_else(|| self.theme_emphasis())
    }
    pub fn match_pair_bg(&self) -> Color {
        self.theme_override("match_pair.bg").unwrap_or_else(|| self.theme_border())
    }
    pub fn doc_highlight_bg(&self) -> Color {
        self.theme_override("doc_highlight.bg").unwrap_or_else(|| self.theme_border())
    }

    // ── Status-line mode chips ───────────────────────────────────────────
    pub fn mode_normal(&self) -> Color {
        self.theme_override("mode.normal").unwrap_or_else(|| self.theme_emphasis())
    }
    pub fn mode_insert(&self) -> Color {
        self.theme_override("mode.insert").unwrap_or_else(|| self.theme_accent_secondary())
    }
    pub fn mode_visual(&self) -> Color {
        let mauve = self
            .color_for_capture("keyword")
            .unwrap_or(Color::Rgb { r: 0xcb, g: 0xa6, b: 0xf7 });
        self.theme_override("mode.visual").unwrap_or(mauve)
    }
    pub fn mode_command(&self) -> Color {
        self.theme_override("mode.command").unwrap_or_else(|| self.theme_accent())
    }
    pub fn mode_search(&self) -> Color {
        self.theme_override("mode.search").unwrap_or_else(|| self.theme_accent())
    }
    pub fn mode_picker(&self) -> Color {
        self.theme_override("mode.picker").unwrap_or_else(|| self.theme_hint())
    }
    pub fn mode_prompt(&self) -> Color {
        self.theme_override("mode.prompt").unwrap_or_else(|| self.theme_accent())
    }
    pub fn mode_terminal(&self) -> Color {
        self.theme_override("mode.terminal").unwrap_or_else(|| self.theme_accent_secondary())
    }
    pub fn mode_debug(&self) -> Color {
        self.theme_override("mode.debug").unwrap_or_else(|| self.theme_accent())
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

/// True for colours whose perceived lightness is below the midpoint — used
/// to pick "lighten toward white" vs "darken toward black" for the chrome
/// neutrals. Rec. 601 luma is fine here; we only need a coarse dark/light
/// flip, not WCAG-grade contrast scoring.
fn is_dark(c: Color) -> bool {
    if let Color::Rgb { r, g, b } = c {
        let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        lum < 128.0
    } else {
        true
    }
}

/// Linear interpolate between two RGB colours. `t = 0` returns `a`, `t = 1`
/// returns `b`. Non-Rgb variants pass through `a` unchanged — palette
/// derivation is only ever invoked on hex-parsed Rgb colours.
fn mix(a: Color, b: Color, t: f32) -> Color {
    let (Color::Rgb { r: ar, g: ag, b: ab }, Color::Rgb { r: br, g: bg, b: bb }) = (a, b) else {
        return a;
    };
    let t = t.clamp(0.0, 1.0);
    let blend = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round().clamp(0.0, 255.0) as u8;
    Color::Rgb {
        r: blend(ar, br),
        g: blend(ag, bg),
        b: blend(ab, bb),
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
