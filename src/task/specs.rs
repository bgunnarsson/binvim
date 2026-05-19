//! Workspace walk + per-source dispatch. `discover_all(start)` is the
//! single entry point the orchestration layer calls — it walks up from
//! `start` once per source, locates the closest matching workspace
//! root, and unions the results.
//!
//! Each source contributes whatever it found at the nearest enclosing
//! root for that source. So a pnpm project nested inside a cargo
//! workspace yields both the npm scripts (from the package.json root)
//! and the cargo built-ins (from the Cargo.toml root); the picker
//! shows them grouped by source tag.

use std::path::{Path, PathBuf};

use super::types::Task;

/// Discover every task reachable from `start`. Per-source root walks
/// are independent — finding a `package.json` doesn't stop us from
/// also climbing further up to a `Cargo.toml`. Tasks are returned in
/// source order (npm, just, cargo, make, dotnet) so a stable picker
/// listing falls out naturally.
pub fn discover_all(start: &Path) -> Vec<Task> {
    let mut out = Vec::new();
    if let Some(root) = find_root(start, super::npm_scripts::ROOT_MARKERS) {
        out.extend(super::npm_scripts::discover(&root));
    }
    if let Some(root) = find_root(start, super::justfile::ROOT_MARKERS) {
        out.extend(super::justfile::discover(&root));
    }
    if let Some(root) = find_root(start, super::cargo_aliases::ROOT_MARKERS) {
        out.extend(super::cargo_aliases::discover(&root));
    }
    if let Some(root) = find_root(start, super::makefile::ROOT_MARKERS) {
        out.extend(super::makefile::discover(&root));
    }
    if let Some(root) = find_root(start, super::dotnet::ROOT_MARKERS) {
        out.extend(super::dotnet::discover(&root));
    }
    out
}

/// Walk up from `start` looking for any of `markers`. Returns the
/// matching directory, or `None` when nothing's found before we
/// reach the filesystem root. Honours `*.ext` markers (any file in
/// the directory with that extension), same convention as the test-
/// adapter walker.
fn find_root(start: &Path, markers: &[&str]) -> Option<PathBuf> {
    let canon = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut dir: &Path = canon.as_path();
    loop {
        if has_any_marker(dir, markers) {
            return Some(dir.to_path_buf());
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p,
            _ => break,
        }
    }
    None
}

fn has_any_marker(dir: &Path, markers: &[&str]) -> bool {
    for marker in markers {
        if let Some(ext) = marker.strip_prefix("*.") {
            if dir_contains_extension(dir, ext) {
                return true;
            }
        } else if dir.join(marker).exists() {
            return true;
        }
    }
    false
}

fn dir_contains_extension(dir: &Path, ext: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        if let Some(file_ext) = entry.path().extension().and_then(|e| e.to_str()) {
            if file_ext.eq_ignore_ascii_case(ext) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discover_all_unions_sources() {
        let tmp = std::env::temp_dir().join("binvim-task-discover-all");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
        fs::write(tmp.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        fs::write(tmp.join("justfile"), "build:\n    echo b\n").unwrap();
        let tasks = discover_all(&tmp);
        // Should include npm "dev", just "build", and cargo built-ins.
        assert!(tasks.iter().any(|t| t.label == "dev"));
        assert!(
            tasks
                .iter()
                .any(|t| t.label == "build" && t.program == "just")
        );
        assert!(
            tasks
                .iter()
                .any(|t| t.label == "build" && t.program == "cargo")
        );
        let _ = fs::remove_dir_all(&tmp);
    }
}
