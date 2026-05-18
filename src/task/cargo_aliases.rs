//! Cargo task discovery. Two sources, unioned:
//!
//! 1. **Built-in verbs** — `build`, `check`, `test`, `clippy`, `run`,
//!    `fmt`, `doc`. These are the day-to-day Rust dev commands; the
//!    picker would feel incomplete without them even though they're
//!    not "defined" anywhere in the workspace.
//! 2. **User aliases** — `[alias]` entries in `<workspace>/.cargo/config.toml`
//!    or `<workspace>/.cargo/config`. Each becomes a runnable task.
//!
//! Aliases that collide with built-in verbs win, because that's
//! presumably why the user defined the alias.

use std::path::Path;

use super::types::{Task, TaskSource};

pub const ROOT_MARKERS: &[&str] = &["Cargo.toml"];

/// Built-in cargo verbs the picker should always offer for a cargo
/// workspace. Listed in the rough order someone would pick them.
const BUILTIN_VERBS: &[(&str, &str)] = &[
    ("build", "cargo build"),
    ("check", "cargo check"),
    ("test", "cargo test"),
    ("clippy", "cargo clippy"),
    ("run", "cargo run"),
    ("fmt", "cargo fmt"),
    ("doc", "cargo doc"),
];

pub fn discover(root: &Path) -> Vec<Task> {
    let aliases = read_aliases(root);
    let mut out: Vec<Task> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (name, expansion) in &aliases {
        seen.insert(name.clone());
        out.push(Task {
            label: name.clone(),
            source: TaskSource::CargoAlias,
            cwd: root.to_path_buf(),
            program: "cargo".to_string(),
            args: vec![name.clone()],
            description: Some(format!("cargo {} = {}", name, expansion)),
        });
    }
    for (verb, desc) in BUILTIN_VERBS {
        if seen.contains(*verb) {
            continue;
        }
        out.push(Task {
            label: (*verb).to_string(),
            source: TaskSource::CargoAlias,
            cwd: root.to_path_buf(),
            program: "cargo".to_string(),
            args: vec![(*verb).to_string()],
            description: Some((*desc).to_string()),
        });
    }
    out
}

/// Read `[alias]` entries from `<root>/.cargo/config.toml` (or the
/// extension-less `config`). Hand-rolled scan rather than a TOML
/// dependency — the file is small, the format is stable, and we only
/// look at one section. Returns `(name, expansion-display)` pairs.
fn read_aliases(root: &Path) -> Vec<(String, String)> {
    let candidates = [
        root.join(".cargo").join("config.toml"),
        root.join(".cargo").join("config"),
    ];
    let path = match candidates.iter().find(|p| p.is_file()) {
        Some(p) => p,
        None => return Vec::new(),
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    parse_alias_section(&text)
}

fn parse_alias_section(text: &str) -> Vec<(String, String)> {
    let mut in_alias = false;
    let mut out: Vec<(String, String)> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            // Match `[alias]` and `[alias.foo]` (the table-of-tables
            // form) — both signal alias entries.
            in_alias = section == "alias" || section.starts_with("alias.");
            continue;
        }
        if !in_alias {
            continue;
        }
        if let Some((name, value)) = line.split_once('=') {
            let name = name.trim().trim_matches('"');
            if name.is_empty() {
                continue;
            }
            let display = value.trim().trim_matches('"').to_string();
            out.push((name.to_string(), display));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_alias_section() {
        let toml = r#"
[alias]
b = "build"
t = "test --release"
lint = ["clippy", "--", "-D", "warnings"]
"#;
        let r = parse_alias_section(toml);
        assert!(r.iter().any(|(k, _)| k == "b"));
        assert!(r.iter().any(|(k, _)| k == "t"));
        assert!(r.iter().any(|(k, _)| k == "lint"));
    }

    #[test]
    fn discover_unions_builtins_with_aliases() {
        let tmp = std::env::temp_dir().join("binvim-task-cargo-aliases");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join(".cargo")).unwrap();
        std::fs::write(
            tmp.join(".cargo").join("config.toml"),
            "[alias]\nb = \"build --release\"\n",
        )
        .unwrap();
        let tasks = discover(&tmp);
        // Alias `b` plus the 7 builtins (none of which is `b`).
        assert!(tasks.iter().any(|t| t.label == "b"));
        assert!(tasks.iter().any(|t| t.label == "build"));
        assert!(tasks.iter().any(|t| t.label == "test"));
        // The alias should be first (we emit aliases before builtins).
        assert_eq!(tasks[0].label, "b");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
