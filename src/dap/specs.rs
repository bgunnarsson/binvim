//! Debug-adapter registry. `adapter_for_workspace` picks the right adapter
//! for a workspace by walking up from a starting path looking for the
//! adapter's root markers. Adding an adapter means appending one
//! `DapAdapterSpec` to `BUILTIN_ADAPTERS`.
//!
//! Helpers `find_workspace_root` and `resolve_command` are duplicated from
//! `lsp::specs` rather than re-exported — the DAP layer is conceptually
//! independent of the LSP layer and shouldn't cross-import.

use serde_json::{json, Value};
use std::collections::BTreeMap;
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
    /// given a resolved launch context. Returning `Err` aborts the
    /// session start before the adapter is spawned (avoids leaking a
    /// process if the build output can't be located).
    pub build_launch_args: fn(ctx: &LaunchContext) -> Result<Value, String>,
}

/// Per-session launch context — resolved by the dispatch in
/// `app/dap_glue.rs` and passed to the adapter spec's `build_launch_args`.
/// Holds everything an adapter might need beyond the bare workspace root:
/// a specific project picked from a multi-project workspace, plus any
/// applicationUrl / environment overrides parsed out of a `.NET`
/// launchSettings.json or equivalent.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LaunchContext {
    /// Directory the prelaunch + launch should run inside. For .NET this
    /// is typically the project directory (where `bin/Debug/net*/` sits).
    pub root: PathBuf,
    /// Specific project file (`*.csproj` / `*.fsproj`) the user picked
    /// when the workspace has more than one. None means "use root as the
    /// project directly" — adapters can fall back to their default
    /// resolution.
    pub project_path: Option<PathBuf>,
    /// URLs the process should bind. For .NET we translate this into
    /// `ASPNETCORE_URLS`. Comes from `launchSettings.json` if found.
    pub application_urls: Vec<String>,
    /// Extra env vars to set on the launched process. Sorted for stable
    /// JSON output.
    pub env: BTreeMap<String, String>,
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
/// `launch` arguments netcoredbg expects. Prefers `<project>.dll` from
/// the most recently modified `net*` subdirectory so newer targets win
/// over older ones. The project dir is the one containing the chosen
/// `.csproj` (when the user picked one) or `root` otherwise.
fn dotnet_launch_args(ctx: &LaunchContext) -> Result<Value, String> {
    let project_dir = ctx
        .project_path
        .as_ref()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| ctx.root.clone());
    let project_stem = ctx
        .project_path
        .as_ref()
        .and_then(|p| p.file_stem())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .or_else(|| {
            project_dir
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();
    let bin = project_dir.join("bin").join("Debug");
    if !bin.is_dir() {
        return Err(format!(
            "no Debug build output at {} — has the project been built?",
            bin.display()
        ));
    }
    let mut frameworks: Vec<std::path::PathBuf> = std::fs::read_dir(&bin)
        .map_err(|e| format!("cannot read {}: {}", bin.display(), e))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    frameworks.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    frameworks.reverse();

    for fw in &frameworks {
        let preferred = fw.join(format!("{}.dll", project_stem));
        if preferred.is_file() {
            return Ok(dotnet_launch_payload(&preferred, &project_dir, ctx));
        }
    }
    for fw in &frameworks {
        let Ok(entries) = std::fs::read_dir(fw) else { continue };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("dll") {
                return Ok(dotnet_launch_payload(&p, &project_dir, ctx));
            }
        }
    }
    Err(format!("no *.dll found under {}", bin.display()))
}

fn dotnet_launch_payload(program: &Path, cwd: &Path, ctx: &LaunchContext) -> Value {
    // ASPNETCORE_URLS is what Kestrel reads to pick the listening
    // endpoint(s). Merge it into the env so a launchSettings profile's
    // applicationUrl wins over the framework default of :5000.
    let mut env = ctx.env.clone();
    if !ctx.application_urls.is_empty() && !env.contains_key("ASPNETCORE_URLS") {
        env.insert("ASPNETCORE_URLS".into(), ctx.application_urls.join(";"));
    }
    let mut payload = json!({
        "name": ".NET Core Launch",
        "type": "coreclr",
        "request": "launch",
        "program": program.display().to_string(),
        "cwd": cwd.display().to_string(),
        "console": "internalConsole",
        "stopAtEntry": false,
        // Off so breakpoints inside minimal-API endpoint lambdas — which
        // dispatch through framework code — actually bind on JIT. With
        // JMC on, netcoredbg silently rebinds the breakpoint to the
        // nearest user-code sequence point (the MapGet registration
        // line), so it fires once during startup and never again on
        // request.
        "justMyCode": false,
        // Tell the adapter we don't care if the source file's hash
        // matches the PDB — useful when the user has edited the file
        // since the last build but hasn't re-built yet.
        "requireExactSource": false,
    });
    if !env.is_empty() {
        let env_obj: serde_json::Map<String, Value> = env
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect();
        payload["env"] = Value::Object(env_obj);
    }
    payload
}

/// Walk up from `start` looking for the most-enclosing workspace
/// container: a `.sln` directory first, then a `.git` directory. Falls
/// back to the directory containing any matching root marker (the
/// previous behaviour). Used to widen the search so a buffer inside
/// `repo/MyProject/Foo.cs` resolves to `repo/` instead of `repo/MyProject/`
/// when there are sibling projects worth picking from.
pub fn find_dotnet_workspace_root(start: &Path) -> PathBuf {
    let canon = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    // First pass: .sln or .git. Take the closest of either as the
    // workspace root, preferring .sln when both are present in the same
    // ancestor.
    let mut dir: &Path = canon.as_path();
    loop {
        if dir_contains_extension(dir, "sln") {
            return dir.to_path_buf();
        }
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p,
            _ => break,
        }
    }
    // No .sln / .git — fall back to the immediate-.csproj directory.
    let markers: Vec<String> = ["*.csproj", "*.fsproj", "*.vbproj", "*.sln"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    find_workspace_root(&canon, &markers)
}

/// Recursive enumeration of `.csproj` / `.fsproj` / `.vbproj` files
/// rooted at `dir`. Skips obvious build/output directories so we don't
/// surface generated `bin/Debug/.../*.csproj` siblings. Depth bounded to
/// keep the walk cheap on large monorepos.
pub fn find_dotnet_projects(dir: &Path) -> Vec<PathBuf> {
    fn ignored(name: &str) -> bool {
        matches!(
            name,
            "bin" | "obj" | "node_modules" | ".git" | ".vs" | "TestResults" | "target"
        )
    }
    fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
        if depth > 6 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if name.starts_with('.') && name != "." {
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                if !ignored(name) {
                    walk(&path, out, depth + 1);
                }
            } else if file_type.is_file() {
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    let ext_lc = ext.to_ascii_lowercase();
                    if matches!(ext_lc.as_str(), "csproj" | "fsproj" | "vbproj") {
                        out.push(path);
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out, 0);
    out.sort();
    out
}

/// Parsed `Properties/launchSettings.json`. We surface every profile
/// whose `commandName` is `Project` (Kestrel hosting via `dotnet run`) —
/// that's the only commandName we know how to drive from netcoredbg.
/// IIS / IIS Express profiles are ignored.
#[derive(Debug, Clone)]
pub struct LaunchProfile {
    #[allow(dead_code)]
    pub name: String,
    pub application_urls: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// Read `<project_dir>/Properties/launchSettings.json` and return every
/// runnable profile. Returns an empty vec when the file is missing or
/// malformed — that's the same as "no profile-level overrides".
pub fn load_launch_profiles(project_dir: &Path) -> Vec<LaunchProfile> {
    let path = project_dir.join("Properties").join("launchSettings.json");
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    let Ok(value): Result<Value, _> = serde_json::from_str(&text) else { return Vec::new() };
    let Some(profiles) = value.get("profiles").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, profile) in profiles {
        let command_name = profile
            .get("commandName")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if command_name != "Project" {
            continue;
        }
        let application_urls = profile
            .get("applicationUrl")
            .and_then(|v| v.as_str())
            .map(|s| s.split(';').map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).collect())
            .unwrap_or_default();
        let env = profile
            .get("environmentVariables")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| {
                        v.as_str().map(|s| (k.clone(), s.to_string()))
                    })
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        out.push(LaunchProfile { name: name.clone(), application_urls, env });
    }
    out
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
