//! Debug-adapter registry. `adapter_for_workspace` picks the right adapter
//! for a workspace by walking up from a starting path looking for the
//! adapter's root markers. Adding an adapter means appending one
//! `DapAdapterSpec` to `BUILTIN_ADAPTERS`.
//!
//! Helpers `find_workspace_root` and `resolve_command` are duplicated from
//! `lsp::specs` rather than re-exported — the DAP layer is conceptually
//! independent of the LSP layer and shouldn't cross-import.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// One debug adapter the editor knows how to launch.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DapAdapterSpec {
    /// Stable key — `"dotnet"`, `"go"`, `"python"`, …
    pub key: &'static str,
    /// Process candidates in priority order. First one that resolves on
    /// `$PATH` (or as an absolute path) wins.
    pub cmd_candidates: &'static [&'static str],
    /// Args appended to the adapter command. Typically the interpreter
    /// flag (`--interpreter=vscode` for netcoredbg).
    pub args: &'static [&'static str],
    /// Filenames / globs whose presence marks a workspace this adapter
    /// claims. `*.ext` is honoured as "any file with that extension in
    /// the directory."
    pub root_markers: &'static [&'static str],
    /// Optional pre-launch step — run before sending `launch` to the
    /// adapter. For .NET this is `dotnet build`.
    pub prelaunch: Option<PrelaunchCommand>,
    /// Builds the `launch` request `arguments` JSON for this adapter
    /// given the resolved workspace root. Returning `Err` aborts the
    /// session start before the adapter is spawned (avoids leaking a
    /// process if the build output can't be located).
    pub build_launch_args: fn(root: &Path) -> Result<Value, String>,
}

/// A shell command to run before the adapter session starts. `args` are
/// passed through; the runner uses the resolved workspace root as cwd.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct PrelaunchCommand {
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// Human-readable label for the status line ("Building…", "Compiling…").
    pub label: &'static str,
}

/// All adapters binvim ships with. Order doesn't matter — `adapter_for_workspace`
/// returns the first one whose root markers match.
const BUILTIN_ADAPTERS: &[DapAdapterSpec] = &[DOTNET];

const DOTNET: DapAdapterSpec = DapAdapterSpec {
    key: "dotnet",
    cmd_candidates: &["netcoredbg"],
    args: &["--interpreter=vscode"],
    root_markers: &["*.csproj", "*.sln", "*.fsproj"],
    prelaunch: Some(PrelaunchCommand {
        program: "dotnet",
        args: &["build", "-c", "Debug"],
        label: "Building .NET project",
    }),
    build_launch_args: dotnet_launch_args,
};

/// Locate the built `*.dll` under `bin/Debug/netN.0/` and build the
/// `launch` arguments netcoredbg expects. Prefers `<root-name>.dll` and
/// the most recently modified `net*` subdirectory, so the .NET 10 build
/// wins when older targets are also present.
fn dotnet_launch_args(root: &Path) -> Result<Value, String> {
    let bin = root.join("bin").join("Debug");
    if !bin.is_dir() {
        return Err(format!(
            "no Debug build output at {} — has the project been built?",
            bin.display()
        ));
    }
    let project_name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let mut frameworks: Vec<std::path::PathBuf> = std::fs::read_dir(&bin)
        .map_err(|e| format!("cannot read {}: {}", bin.display(), e))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    frameworks.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
    });
    frameworks.reverse();

    for fw in &frameworks {
        let preferred = fw.join(format!("{}.dll", project_name));
        if preferred.is_file() {
            return Ok(dotnet_launch_payload(&preferred, root));
        }
    }
    for fw in &frameworks {
        let Ok(entries) = std::fs::read_dir(fw) else { continue };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("dll") {
                return Ok(dotnet_launch_payload(&p, root));
            }
        }
    }
    Err(format!("no *.dll found under {}", bin.display()))
}

fn dotnet_launch_payload(program: &Path, cwd: &Path) -> Value {
    json!({
        "name": ".NET Core Launch",
        "type": "coreclr",
        "request": "launch",
        "program": program.display().to_string(),
        "cwd": cwd.display().to_string(),
        "console": "internalConsole",
        "stopAtEntry": false,
        "justMyCode": true,
    })
}

/// Pick the adapter that claims `start_dir`, walking up parent directories
/// looking for any spec's root markers. Returns the spec and the resolved
/// workspace root (the directory the marker was found in).
#[allow(dead_code)]
pub fn adapter_for_workspace(start: &Path) -> Option<(DapAdapterSpec, PathBuf)> {
    for spec in BUILTIN_ADAPTERS {
        let markers: Vec<String> = spec.root_markers.iter().map(|s| s.to_string()).collect();
        let root = find_workspace_root(start, &markers);
        if has_any_marker(&root, &markers) {
            return Some((spec.clone(), root));
        }
    }
    None
}

/// Walk up from `start` until any of `markers` is found in a directory.
/// `*.ext` markers match any file in the directory with that extension.
/// Returns the matching directory, or the canonical form of `start` if
/// nothing was found (so callers can still emit a useful path).
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

/// Resolve the first command candidate to an absolute path. Bare names go
/// through `$PATH`; absolute / relative paths must exist on disk. `~/` is
/// expanded against `$HOME`. Returns `None` if nothing resolves.
#[allow(dead_code)]
pub fn resolve_command(candidates: &[&str]) -> Option<String> {
    for c in candidates {
        let path = if let Some(rest) = c.strip_prefix("~/") {
            let home = std::env::var("HOME").ok()?;
            format!("{}/{}", home, rest)
        } else {
            (*c).to_string()
        };
        if path.contains('/') {
            if Path::new(&path).is_file() {
                return Some(path);
            }
            continue;
        }
        if let Some(found) = which_in_path(&path) {
            return Some(found);
        }
    }
    None
}

fn which_in_path(name: &str) -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(':') {
        let full = Path::new(dir).join(name);
        if full.is_file() {
            return Some(full.to_string_lossy().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn adapter_for_workspace_finds_dotnet_via_csproj() {
        let tmp = std::env::temp_dir().join("binvim_dap_test_csproj");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("hello.csproj"), "<Project/>").unwrap();
        let found = adapter_for_workspace(&tmp).expect("should find dotnet adapter");
        assert_eq!(found.0.key, "dotnet");
        assert_eq!(found.1.canonicalize().unwrap(), tmp.canonicalize().unwrap());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn adapter_for_workspace_finds_nothing_for_empty_dir() {
        let tmp = std::env::temp_dir().join("binvim_dap_test_empty");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        assert!(adapter_for_workspace(&tmp).is_none());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_command_picks_first_existing() {
        // /bin/sh exists on macOS and Linux; this is a stable target.
        let found = resolve_command(&["definitely_not_a_real_binary_xyz", "sh"]);
        assert!(found.is_some());
        let s = found.unwrap();
        assert!(s.ends_with("/sh"), "expected absolute path ending in /sh, got {s}");
    }
}
