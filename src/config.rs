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
}

fn default_schema() -> u32 {
    1
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            colors: HashMap::new(),
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
    pub fn color_for_capture(&self, name: &str) -> Option<Color> {
        let head = name.split('.').next().unwrap_or(name);
        // Try the most-specific first (e.g. "keyword.return") then the head.
        if let Some(c) = self.colors.get(name).and_then(|s| parse_color(s)) {
            return Some(c);
        }
        if let Some(c) = self.colors.get(head).and_then(|s| parse_color(s)) {
            return Some(c);
        }
        default_capture_color(head)
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

fn default_capture_color(head: &str) -> Option<Color> {
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
        _ => None,
    }
}
