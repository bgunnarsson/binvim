//! Justfile recipe discovery. Skips internal recipes (`_name`),
//! parameterised recipes (`name foo:`), and anything inside a `#`
//! comment. Doesn't shell out to `just --list` — the file format is
//! stable enough that a single-pass scan beats a process spawn on
//! every picker open.

use std::path::Path;

use super::types::{Task, TaskSource};

/// File basenames this adapter claims. `justfile` is the canonical name;
/// `.justfile` and the capitalised forms cover the common variants
/// `just` itself accepts.
pub const ROOT_MARKERS: &[&str] = &["justfile", ".justfile", "Justfile"];

/// Walk the closest Justfile under `root` and emit one `Task` per
/// runnable recipe. Returns empty when nothing's found or the file
/// can't be read.
pub fn discover(root: &Path) -> Vec<Task> {
    let path = ROOT_MARKERS
        .iter()
        .map(|m| root.join(m))
        .find(|p| p.is_file());
    let Some(path) = path else {
        return Vec::new();
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    parse_recipes(&contents)
        .into_iter()
        .map(|(name, description)| Task {
            label: name.clone(),
            source: TaskSource::Justfile,
            cwd: root.to_path_buf(),
            program: "just".to_string(),
            args: vec![name],
            description,
        })
        .collect()
}

/// Pull recipe names + their preceding doc-comments out of a Justfile.
/// Recipe header shape: optional attributes (`[private]` etc.) on the
/// preceding line, then `recipe-name[:|\\s+param...]:` with no leading
/// indentation. The body is whatever follows on indented lines until
/// the next un-indented line or EOF; we don't need to capture it for
/// v1.
fn parse_recipes(text: &str) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    let mut pending_doc: Vec<String> = Vec::new();
    let mut hidden_next = false;
    for raw in text.lines() {
        let line = raw.trim_end();
        // `# ` comments accumulate as the next recipe's description.
        // Multiple comment lines fold together with a single space; we
        // keep it terse for the picker.
        if let Some(rest) = line.strip_prefix('#') {
            let doc = rest.trim_start_matches([' ', '\t']);
            if !doc.is_empty() {
                pending_doc.push(doc.to_string());
            }
            continue;
        }
        // Attribute line — `[private]` / `[no-cd]` / etc. The next
        // recipe header may be hidden; for now we honour `[private]`.
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if trimmed.contains("private") {
                hidden_next = true;
            }
            continue;
        }
        if line.is_empty() {
            pending_doc.clear();
            hidden_next = false;
            continue;
        }
        // Recipe header — un-indented, contains `:`, doesn't look like
        // an assignment (`var := value`) or a setting (`set foo := bar`).
        if line.starts_with(char::is_whitespace) {
            // Body lines reset doc accumulation only at the next blank.
            continue;
        }
        if let Some(name) = recipe_name(line) {
            let private = name.starts_with('_') || hidden_next;
            if !private {
                let description = if pending_doc.is_empty() {
                    None
                } else {
                    Some(pending_doc.join(" "))
                };
                out.push((name, description));
            }
            pending_doc.clear();
            hidden_next = false;
            continue;
        }
        // Top-level non-recipe (variable assignment, `set` directive,
        // import, etc.). Doc-comments before these aren't ours.
        pending_doc.clear();
        hidden_next = false;
    }
    out
}

/// Extract a recipe name from a header line like `build target='x':`.
/// Returns `None` when the line doesn't look like a recipe header
/// (variable assignment `:=`, setting `set foo := bar`, alias `alias a
/// := b`). The name is everything up to the first whitespace or `:`.
fn recipe_name(line: &str) -> Option<String> {
    // Filter out assignments and aliases: `x := y` / `alias a := b` /
    // `set tempdir := /tmp`.
    if line.contains(":=") {
        return None;
    }
    // Recipe header must have a colon. Param defaults can contain
    // colons inside strings, so we settle for "first colon outside a
    // single-quoted string" which is what `just` itself does.
    let colon_idx = first_unquoted_colon(line)?;
    let head = &line[..colon_idx];
    // First whitespace-delimited token is the recipe name. Strip any
    // `@` modifier (`@build:` means "don't echo the command").
    let first_token = head.split_whitespace().next()?;
    let name = first_token.trim_start_matches('@');
    if name.is_empty() {
        return None;
    }
    // Reserved words `just` ships with that mustn't be picker-runnable.
    const RESERVED: &[&str] = &["alias", "set", "export", "import", "mod"];
    if RESERVED.iter().any(|r| *r == name) {
        return None;
    }
    Some(name.to_string())
}

fn first_unquoted_colon(line: &str) -> Option<usize> {
    let mut in_single = false;
    let mut in_double = false;
    for (i, b) in line.bytes().enumerate() {
        match b {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b':' if !in_single && !in_double => return Some(i),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_recipes() {
        let just = r#"
# Build the project
build:
    cargo build --release

# Run the test suite
test:
    cargo test

_private:
    echo hidden
"#;
        let r = parse_recipes(just);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0, "build");
        assert_eq!(r[0].1.as_deref(), Some("Build the project"));
        assert_eq!(r[1].0, "test");
    }

    #[test]
    fn skips_private_attribute() {
        let just = r#"
[private]
helper:
    echo nope

public:
    echo yep
"#;
        let r = parse_recipes(just);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "public");
    }

    #[test]
    fn ignores_assignments_and_aliases() {
        let just = r#"
tempdir := "/tmp"
alias b := build

build:
    cargo build
"#;
        let r = parse_recipes(just);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "build");
    }

    #[test]
    fn captures_recipes_with_params() {
        let just = r#"
deploy env="prod":
    echo deploying to {{env}}
"#;
        let r = parse_recipes(just);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "deploy");
    }
}
