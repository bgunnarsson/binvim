//! Makefile target discovery — scrapes top-level `^target:` lines.
//!
//! Intentionally lightweight: we don't expand variables, follow
//! `include` directives, parse pattern rules, or honour `.PHONY`
//! granularity beyond using it as a hint. The picker just wants a list
//! of names a user might reasonably want to run; cases where this is
//! wrong (computed targets, $(VAR):) silently fall through.

use std::path::Path;

use super::types::{Task, TaskSource};

pub const ROOT_MARKERS: &[&str] = &["Makefile", "makefile", "GNUmakefile"];

pub fn discover(root: &Path) -> Vec<Task> {
    let path = ROOT_MARKERS
        .iter()
        .map(|m| root.join(m))
        .find(|p| p.is_file());
    let Some(path) = path else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    parse_targets(&text)
        .into_iter()
        .map(|name| Task {
            label: name.clone(),
            source: TaskSource::Makefile,
            cwd: root.to_path_buf(),
            program: "make".to_string(),
            args: vec![name],
            description: None,
        })
        .collect()
}

/// Pull target names out of a Makefile body. The shape we accept:
/// `name: <prereqs>` at column 0, single colon (`::` double-colon
/// rules also accepted), name made of word-chars + `-` + `.`. Targets
/// containing variable references (`$(BIN):`) and pattern rules
/// (`%.o:`) are skipped so the picker doesn't list nonsense.
fn parse_targets(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for raw in text.lines() {
        // Recipe lines (tab-indented) are not targets.
        if raw.starts_with('\t') || raw.starts_with(' ') {
            continue;
        }
        let line = raw.split('#').next().unwrap_or("").trim_end();
        if line.is_empty() {
            continue;
        }
        // Need a colon, not `:=` (variable) and not `::=` (POSIX
        // immediate assignment).
        if line.contains(":=") || line.contains("::=") {
            continue;
        }
        let colon_idx = match line.find(':') {
            Some(i) => i,
            None => continue,
        };
        let name = line[..colon_idx].trim();
        // Targets can be space-separated (a single rule producing
        // several outputs); pick the first name.
        let first = name.split_whitespace().next().unwrap_or("");
        if first.is_empty() {
            continue;
        }
        if !is_plain_target(first) {
            continue;
        }
        if seen.insert(first.to_string()) {
            out.push(first.to_string());
        }
    }
    out
}

/// True when `name` is a normal target — letters / digits / `_` /
/// `-` / `.`, no `$()` variable refs, no `%` pattern wildcards. We're
/// deliberately strict; better to skip a few exotic targets than to
/// surface noise in the picker.
fn is_plain_target(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    name.chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_targets() {
        let m = "
build:
\techo build

test:
\techo test

%.o: %.c
\techo pattern

$(BIN):
\techo var

CFLAGS := -O2
";
        let t = parse_targets(m);
        assert_eq!(t, vec!["build", "test"]);
    }

    #[test]
    fn dedupes_repeated_targets() {
        let m = "a:\nb:\na:\n";
        let t = parse_targets(m);
        assert_eq!(t, vec!["a", "b"]);
    }
}
