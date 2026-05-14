//! Minimal `.editorconfig` reader. Walks up from a target file collecting and
//! merging settings; the closest section overrides earlier ones. Supports the
//! handful of properties binvim acts on:
//! * `indent_style` — space | tab
//! * `indent_size`  — integer (or `tab`, treated as `tab_width`)
//! * `tab_width`    — integer
//! * `trim_trailing_whitespace` — bool
//! * `insert_final_newline`     — bool

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentStyle {
    Spaces,
    Tabs,
}

#[derive(Debug, Clone, Copy)]
pub struct EditorConfig {
    pub indent_style: IndentStyle,
    pub indent_size: usize,
    pub tab_width: usize,
    pub trim_trailing_whitespace: bool,
    pub insert_final_newline: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            indent_style: IndentStyle::Spaces,
            indent_size: 2,
            tab_width: 4,
            trim_trailing_whitespace: false,
            insert_final_newline: true,
        }
    }
}

impl EditorConfig {
    /// Walk up from `target_file`'s directory collecting `.editorconfig` files.
    /// Apply matching sections in the right order (most general first → most
    /// specific last) so closer files override.
    pub fn detect(target_file: &Path) -> Self {
        let mut cfg = EditorConfig::default();
        let canon = target_file
            .canonicalize()
            .unwrap_or_else(|_| target_file.to_path_buf());

        // Collect all .editorconfig files from root → target dir, plus a `root=true`
        // boundary if seen. We walk DOWN from the topmost found, applying each
        // file's matching sections in order — that mirrors the spec.
        let mut configs: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        let mut dir = canon.parent().map(|p| p.to_path_buf());
        while let Some(d) = dir {
            let candidate = d.join(".editorconfig");
            if candidate.is_file() {
                if let Ok(bytes) = std::fs::read(&candidate) {
                    configs.push((d.clone(), bytes));
                    if has_root_true(&configs.last().unwrap().1) {
                        break;
                    }
                }
            }
            dir = d.parent().map(|p| p.to_path_buf());
        }
        // Apply outermost first → closest last (closest wins).
        configs.reverse();
        for (root_dir, bytes) in &configs {
            apply_file(&mut cfg, &String::from_utf8_lossy(bytes), root_dir, &canon);
        }
        cfg
    }

    /// Every `.editorconfig` that contributed to `detect(target_file)`,
    /// in nearest-to-farthest order. Used by the `:health` dashboard to
    /// show the user *where* the resolved indent / newline settings came
    /// from. Returns an empty vec when no `.editorconfig` was found —
    /// the dashboard then says "(defaults — no .editorconfig found)".
    pub fn sources(target_file: &Path) -> Vec<PathBuf> {
        let canon = target_file
            .canonicalize()
            .unwrap_or_else(|_| target_file.to_path_buf());
        let mut sources: Vec<PathBuf> = Vec::new();
        let mut dir = canon.parent().map(|p| p.to_path_buf());
        while let Some(d) = dir {
            let candidate = d.join(".editorconfig");
            if candidate.is_file() {
                let stop = std::fs::read(&candidate)
                    .map(|bytes| has_root_true(&bytes))
                    .unwrap_or(false);
                sources.push(candidate);
                if stop {
                    break;
                }
            }
            dir = d.parent().map(|p| p.to_path_buf());
        }
        sources
    }

    pub fn indent_string(&self) -> String {
        match self.indent_style {
            IndentStyle::Tabs => "\t".to_string(),
            IndentStyle::Spaces => " ".repeat(self.indent_size.max(1)),
        }
    }

    #[allow(dead_code)]
    pub fn indent_width(&self) -> usize {
        match self.indent_style {
            IndentStyle::Tabs => self.tab_width.max(1),
            IndentStyle::Spaces => self.indent_size.max(1),
        }
    }
}

fn has_root_true(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if trimmed.starts_with('[') {
            // Done with the preamble.
            break;
        }
        if let Some((k, v)) = split_kv(trimmed) {
            if k.eq_ignore_ascii_case("root") && v.eq_ignore_ascii_case("true") {
                return true;
            }
        }
    }
    false
}

fn apply_file(cfg: &mut EditorConfig, text: &str, root_dir: &Path, file: &Path) {
    let mut current_section: Option<String> = None;
    let mut applies_to_file = false;
    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(section) = rest.strip_suffix(']') {
                applies_to_file = section_matches(section, root_dir, file);
                current_section = Some(section.to_string());
                continue;
            }
        }
        if current_section.is_none() {
            // Preamble (root=true etc.)
            continue;
        }
        if !applies_to_file {
            continue;
        }
        if let Some((key, value)) = split_kv(line) {
            apply_property(cfg, &key.to_lowercase(), &value.to_lowercase());
        }
    }
}

fn strip_comment(line: &str) -> &str {
    let mut split = line.len();
    for (i, c) in line.char_indices() {
        if c == '#' || c == ';' {
            split = i;
            break;
        }
    }
    &line[..split]
}

fn split_kv(line: &str) -> Option<(String, String)> {
    let (k, v) = line.split_once('=')?;
    Some((k.trim().to_string(), v.trim().to_string()))
}

fn apply_property(cfg: &mut EditorConfig, key: &str, value: &str) {
    match key {
        "indent_style" => match value {
            "tab" => cfg.indent_style = IndentStyle::Tabs,
            "space" => cfg.indent_style = IndentStyle::Spaces,
            _ => {}
        },
        "indent_size" => {
            if value == "tab" {
                cfg.indent_size = cfg.tab_width;
            } else if let Ok(n) = value.parse::<usize>() {
                cfg.indent_size = n.max(1);
            }
        }
        "tab_width" => {
            if let Ok(n) = value.parse::<usize>() {
                cfg.tab_width = n.max(1);
            }
        }
        "trim_trailing_whitespace" => match value {
            "true" => cfg.trim_trailing_whitespace = true,
            "false" => cfg.trim_trailing_whitespace = false,
            _ => {}
        },
        "insert_final_newline" => match value {
            "true" => cfg.insert_final_newline = true,
            "false" => cfg.insert_final_newline = false,
            _ => {}
        },
        _ => {}
    }
}

fn section_matches(section: &str, root_dir: &Path, file: &Path) -> bool {
    // EditorConfig glob: a leading `/` anchors at root_dir; otherwise the pattern
    // matches anywhere under root_dir. Either way the comparison is against the
    // path of `file` relative to `root_dir`.
    let rel = match file.strip_prefix(root_dir) {
        Ok(p) => p.to_string_lossy().replace('\\', "/"),
        Err(_) => return false,
    };
    let basename = file
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    for pattern in expand_braces(section) {
        let anchored = pattern.starts_with('/');
        let pat = if anchored { &pattern[1..] } else { pattern.as_str() };
        if anchored {
            if glob_match(pat, &rel) {
                return true;
            }
        } else {
            // Match the pattern against the basename or against any suffix of `rel`.
            if glob_match(pat, &basename) || glob_match(pat, &rel) {
                return true;
            }
            // Also try matching against each path tail (e.g. `*.rs` should match
            // `src/foo/bar.rs` even though glob_match without ** wouldn't otherwise).
            for (i, c) in rel.char_indices() {
                if c == '/' {
                    let tail = &rel[i + 1..];
                    if glob_match(pat, tail) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Expand `{a,b,c}` alternation into separate patterns. Nested braces are not
/// supported (rare in real configs).
fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    let Some(close_rel) = pattern[open..].find('}') else {
        return vec![pattern.to_string()];
    };
    let close = open + close_rel;
    let prefix = &pattern[..open];
    let alternatives = &pattern[open + 1..close];
    let suffix = &pattern[close + 1..];
    let mut out = Vec::new();
    for alt in alternatives.split(',') {
        let combined = format!("{}{}{}", prefix, alt, suffix);
        out.extend(expand_braces(&combined));
    }
    out
}

/// Glob match — supports `*` (no `/`), `**` (any), `?`, and literal chars.
fn glob_match(pattern: &str, path: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = path.chars().collect();
    glob_match_chars(&p, &s, 0, 0)
}

fn glob_match_chars(p: &[char], s: &[char], pi: usize, si: usize) -> bool {
    if pi == p.len() {
        return si == s.len();
    }
    match p[pi] {
        '*' if pi + 1 < p.len() && p[pi + 1] == '*' => {
            for k in si..=s.len() {
                if glob_match_chars(p, s, pi + 2, k) {
                    return true;
                }
            }
            false
        }
        '*' => {
            if glob_match_chars(p, s, pi + 1, si) {
                return true;
            }
            if si < s.len() && s[si] != '/' {
                return glob_match_chars(p, s, pi, si + 1);
            }
            false
        }
        '?' => {
            if si < s.len() && s[si] != '/' {
                glob_match_chars(p, s, pi + 1, si + 1)
            } else {
                false
            }
        }
        c => si < s.len() && s[si] == c && glob_match_chars(p, s, pi + 1, si + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glob_match_with_braces(pattern: &str, path: &str) -> bool {
        expand_braces(pattern).iter().any(|p| glob_match(p, path))
    }

    #[test]
    fn glob_basic() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "main.toml"));
        assert!(glob_match_with_braces("*.{rs,toml}", "main.toml"));
        assert!(glob_match("**/*.ts", "src/foo/bar.ts"));
        assert!(glob_match("Makefile", "Makefile"));
    }

    #[test]
    fn brace_expansion() {
        let v = expand_braces("*.{rs,toml,json}");
        assert_eq!(v, vec!["*.rs", "*.toml", "*.json"]);
    }
}
