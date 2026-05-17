//! Test-adapter registry. `adapter_for_workspace` picks the right
//! adapter for a workspace by walking up from a starting path looking
//! for the adapter's root markers. Adding an adapter means appending
//! one `TestAdapterSpec` to `BUILTIN_ADAPTERS` plus its sibling parser
//! module (e.g. `cargo.rs`).
//!
//! Mirrors the layout of `dap/specs.rs` deliberately — the two systems
//! are structurally parallel but conceptually independent, so the
//! root-walk helpers are duplicated here rather than cross-imported.

use std::path::{Path, PathBuf};

use super::types::{ResolvedCommand, TestEvent, TestRunRequest};

/// One test-runner adapter the editor knows how to drive.
#[derive(Debug, Clone)]
pub struct TestAdapterSpec {
    /// Stable key — `"cargo"`, future: `"go"`, `"pytest"`, `"vitest"`.
    pub key: &'static str,
    /// Human label shown in the overlay header. Currently read only by
    /// the future `:health` panel; the active overlay already
    /// surfaces the full command line.
    #[allow(dead_code)]
    pub display_name: &'static str,
    /// Filenames whose presence marks a workspace this adapter claims.
    /// `*.ext` is honoured as "any file with that extension in the
    /// directory."
    pub root_markers: &'static [&'static str],
    /// Build the discovery command — used to enumerate test items for
    /// the `:test` picker. Returns `None` if discovery isn't supported
    /// by this adapter.
    pub build_list_command: fn(root: &Path) -> Option<ResolvedCommand>,
    /// Parse the discovery command's stdout into a flat list of test
    /// names. Adapters that don't support `:test` picker should return
    /// `Vec::new()`.
    pub parse_list_output: fn(stdout: &str) -> Vec<String>,
    /// Build the run command for the given request. Returning `Err`
    /// aborts the run before any process is spawned (e.g. the workspace
    /// root doesn't actually contain anything the adapter can run).
    pub build_run_command: fn(req: &TestRunRequest) -> Result<ResolvedCommand, String>,
    /// Parse one line of adapter stdout into zero-or-more `TestEvent`s.
    /// The reader thread calls this once per line; `state` is owned by
    /// the parser and threaded across calls so adapters can accumulate
    /// multi-line panic blocks etc.
    pub parse_event_line: fn(line: &str, state: &mut LineParseState) -> Vec<TestEvent>,
    /// Drain any state held by the parser at end-of-stream into a final
    /// burst of events. Typically used to flush a summary the parser
    /// was still accumulating when the stream closed.
    pub flush_parser: fn(state: &mut LineParseState) -> Vec<TestEvent>,
    /// Build a filter substring for `:testfile` from the active
    /// buffer's path. Returns `None` to mean "no filter" (run
    /// everything reachable from `workspace_root`).
    pub filter_for_file: fn(file: &Path, root: &Path) -> Option<String>,
    /// Build a filter substring for `:testnearest` — walks the buffer
    /// text upward from `cursor_line` for an adapter-specific test-
    /// case anchor (Rust: `#[test]` then `fn name(`).
    pub filter_for_nearest: fn(buffer_text: &str, cursor_line: usize) -> Option<String>,
}

/// Per-run parser state. Owned by the reader thread, threaded into
/// `parse_event_line` so each adapter parser can keep accumulators
/// (current panic message, current failures-section list, …).
#[derive(Debug, Default)]
pub struct LineParseState {
    pub cargo: super::cargo::CargoParseState,
    pub vitest: super::vitest::VitestParseState,
    pub pytest: super::pytest::PytestParseState,
    pub gotest: super::gotest::GoTestParseState,
    pub dotnet: super::dotnet::DotnetParseState,
}

/// All test adapters binvim ships with. Walked in order by
/// `adapter_for_workspace`; first match wins. Vitest comes before
/// cargo so a vitest project nested inside a cargo workspace (e.g.
/// binvim's `playground/typescript/vitest/`) picks the right runner
/// for that subtree; the walk anchors on the closest `vitest.config.*`
/// before climbing further up to a `Cargo.toml`. Pytest / Go / dotnet
/// each pick by their own root markers (`pyproject.toml` / `pytest.ini`,
/// `go.mod`, `*.csproj` / `*.sln`).
const BUILTIN_ADAPTERS: &[TestAdapterSpec] = &[VITEST, PYTEST, GOTEST, DOTNET, CARGO];

const CARGO: TestAdapterSpec = TestAdapterSpec {
    key: "cargo",
    display_name: "cargo test",
    root_markers: &["Cargo.toml"],
    build_list_command: super::cargo::build_list_command,
    parse_list_output: super::cargo::parse_list_output,
    build_run_command: super::cargo::build_run_command,
    parse_event_line: super::cargo::parse_event_line,
    flush_parser: super::cargo::flush_parser,
    filter_for_file: super::cargo::filter_for_file,
    filter_for_nearest: super::cargo::filter_for_nearest,
};

const VITEST: TestAdapterSpec = TestAdapterSpec {
    key: "vitest",
    display_name: "vitest",
    root_markers: &[
        "vitest.config.ts",
        "vitest.config.mts",
        "vitest.config.js",
        "vitest.config.mjs",
        "vitest.config.cjs",
    ],
    build_list_command: super::vitest::build_list_command,
    parse_list_output: super::vitest::parse_list_output,
    build_run_command: super::vitest::build_run_command,
    parse_event_line: super::vitest::parse_event_line,
    flush_parser: super::vitest::flush_parser,
    filter_for_file: super::vitest::filter_for_file,
    filter_for_nearest: super::vitest::filter_for_nearest,
};

const PYTEST: TestAdapterSpec = TestAdapterSpec {
    key: "pytest",
    display_name: "pytest",
    root_markers: &[
        "pytest.ini",
        "pyproject.toml",
        "setup.cfg",
        "tox.ini",
        "conftest.py",
    ],
    build_list_command: super::pytest::build_list_command,
    parse_list_output: super::pytest::parse_list_output,
    build_run_command: super::pytest::build_run_command,
    parse_event_line: super::pytest::parse_event_line,
    flush_parser: super::pytest::flush_parser,
    filter_for_file: super::pytest::filter_for_file,
    filter_for_nearest: super::pytest::filter_for_nearest,
};

const GOTEST: TestAdapterSpec = TestAdapterSpec {
    key: "go",
    display_name: "go test",
    root_markers: &["go.mod"],
    build_list_command: super::gotest::build_list_command,
    parse_list_output: super::gotest::parse_list_output,
    build_run_command: super::gotest::build_run_command,
    parse_event_line: super::gotest::parse_event_line,
    flush_parser: super::gotest::flush_parser,
    filter_for_file: super::gotest::filter_for_file,
    filter_for_nearest: super::gotest::filter_for_nearest,
};

const DOTNET: TestAdapterSpec = TestAdapterSpec {
    key: "dotnet",
    display_name: "dotnet test",
    root_markers: &["*.sln", "*.csproj", "*.fsproj"],
    build_list_command: super::dotnet::build_list_command,
    parse_list_output: super::dotnet::parse_list_output,
    build_run_command: super::dotnet::build_run_command,
    parse_event_line: super::dotnet::parse_event_line,
    flush_parser: super::dotnet::flush_parser,
    filter_for_file: super::dotnet::filter_for_file,
    filter_for_nearest: super::dotnet::filter_for_nearest,
};

/// Pick the first adapter whose root markers match a directory at or
/// above `start`. Returns the spec plus the resolved workspace root.
pub fn adapter_for_workspace(start: &Path) -> Option<(TestAdapterSpec, PathBuf)> {
    for spec in BUILTIN_ADAPTERS {
        let markers: Vec<String> = spec.root_markers.iter().map(|s| s.to_string()).collect();
        let root = find_workspace_root(start, &markers);
        if has_any_marker(&root, &markers) {
            return Some((spec.clone(), root));
        }
    }
    None
}

/// Walk up from `start` until any of `markers` is found. Returns the
/// matching directory, or a canonical form of `start` when nothing
/// matches (so callers always get a useful path back).
pub fn find_workspace_root(start: &Path, markers: &[String]) -> PathBuf {
    let canon = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut dir: &Path = canon.as_path();
    loop {
        if has_any_marker(dir, markers) {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p,
            _ => break,
        }
    }
    canon
}

fn has_any_marker(dir: &Path, markers: &[String]) -> bool {
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
    fn adapter_for_workspace_picks_cargo_on_cargo_toml() {
        let tmp = std::env::temp_dir().join("binvim-test-adapter-cargo");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src")).unwrap();
        fs::write(tmp.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let picked = adapter_for_workspace(&tmp.join("src"));
        assert!(picked.is_some());
        let (spec, root) = picked.unwrap();
        assert_eq!(spec.key, "cargo");
        assert_eq!(root.canonicalize().unwrap(), tmp.canonicalize().unwrap());
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn adapter_for_workspace_returns_none_outside_known_markers() {
        let tmp = std::env::temp_dir().join("binvim-test-adapter-empty");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        assert!(adapter_for_workspace(&tmp).is_none());
        fs::remove_dir_all(&tmp).unwrap();
    }
}
