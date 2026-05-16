//! Debug-adapter registry. `adapter_for_workspace` picks the right adapter
//! for a workspace by walking up from a starting path looking for the
//! adapter's root markers. Adding an adapter means appending one
//! `DapAdapterSpec` to `BUILTIN_ADAPTERS` and a launch-args builder.
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
    /// Stable key — `"dotnet"`, `"go"`, `"python"`, `"lldb"`.
    pub key: &'static str,
    /// What we send as `adapterID` in `initialize`. Some adapters care
    /// (netcoredbg keys behaviour off `"coreclr"`); most are happy with
    /// the same string as `key`.
    pub adapter_id: &'static str,
    /// Process candidates in priority order. First one that resolves on
    /// `$PATH` (or as an absolute path) wins.
    pub cmd_candidates: &'static [&'static str],
    /// Args appended to the adapter command. Typically the interpreter
    /// flag (`--interpreter=vscode` for netcoredbg, `dap` for delve).
    pub args: &'static [&'static str],
    /// Filenames / globs whose presence marks a workspace this adapter
    /// claims. `*.ext` is honoured as "any file with that extension in
    /// the directory."
    pub root_markers: &'static [&'static str],
    /// Per-target prelaunch resolver. Returns the command to run before
    /// the adapter starts (e.g. `cargo build --bin foo`, `dotnet build`).
    /// `None` means "no prelaunch" — used by adapters like delve that
    /// build implicitly when the session starts.
    pub prelaunch: fn(ctx: &LaunchContext) -> Option<PrelaunchCommand>,
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
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct LaunchContext {
    /// Directory the prelaunch + launch should run inside. For .NET this
    /// is typically the project directory (where `bin/Debug/net*/` sits).
    /// For Rust it's the manifest directory of the picked crate.
    pub root: PathBuf,
    /// Adapter-specific "what to launch" path. .NET: the picked `.csproj`.
    /// Python: the entry script. Go: the package directory. Rust: the
    /// manifest path of the picked crate (the actual binary path is
    /// resolved by `build_launch_args` after the prelaunch build).
    pub project_path: Option<PathBuf>,
    /// Adapter-specific named target. Currently only used by Rust to
    /// identify which `[[bin]]` to build (`cargo build --bin <name>`)
    /// and run. Empty for adapters without named-target dispatch.
    pub target_name: Option<String>,
    /// URLs the process should bind. For .NET we translate this into
    /// `ASPNETCORE_URLS`. Comes from `launchSettings.json` if found.
    pub application_urls: Vec<String>,
    /// Extra env vars to set on the launched process. Sorted for stable
    /// JSON output.
    pub env: BTreeMap<String, String>,
}


/// A shell command to run before the adapter session starts. The runner
/// uses the workspace root or chosen project directory as cwd.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PrelaunchCommand {
    pub program: String,
    pub args: Vec<String>,
    /// Human-readable label for the status line ("Building…", "Compiling…").
    pub label: String,
}

/// All adapters binvim ships with. `adapter_for_workspace` returns the
/// first one whose root markers match — order is by marker specificity
/// (.csproj/.sln/Cargo.toml/go.mod are exact; pyproject.toml et al. are
/// also specific). No marker overlaps between adapters today so the
/// order is mostly cosmetic.
const BUILTIN_ADAPTERS: &[DapAdapterSpec] = &[DOTNET, RUST, GO, PYTHON];

// ---------------------------------------------------------------------------
// .NET (netcoredbg)
// ---------------------------------------------------------------------------

const DOTNET: DapAdapterSpec = DapAdapterSpec {
    key: "dotnet",
    // VSCode's well-known type id for .NET — netcoredbg (and other
    // adapters) gate behaviour on it. Our internal `key` ("dotnet")
    // wouldn't match.
    adapter_id: "coreclr",
    cmd_candidates: &["netcoredbg"],
    args: &["--interpreter=vscode"],
    root_markers: &["*.csproj", "*.sln", "*.fsproj"],
    prelaunch: dotnet_prelaunch,
    build_launch_args: dotnet_launch_args,
};

fn dotnet_prelaunch(_ctx: &LaunchContext) -> Option<PrelaunchCommand> {
    Some(PrelaunchCommand {
        program: "dotnet".into(),
        args: vec!["build".into(), "-c".into(), "Debug".into()],
        label: "Building .NET project".into(),
    })
}

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

// ---------------------------------------------------------------------------
// Go (delve)
// ---------------------------------------------------------------------------

const GO: DapAdapterSpec = DapAdapterSpec {
    key: "go",
    adapter_id: "go",
    cmd_candidates: &["dlv"],
    args: &["dap"],
    // `go.mod` is the only universal marker — single-file scripts get
    // surfaced as their containing dir via the active-buffer fallback in
    // `dap_glue.rs`, not the marker walk.
    root_markers: &["go.mod"],
    prelaunch: |_| None,
    build_launch_args: go_launch_args,
};

/// Delve's launch request. `mode: "debug"` builds the package at
/// `program` and runs it under the debugger. `program` should be a
/// directory containing `package main` (or a single `.go` file).
fn go_launch_args(ctx: &LaunchContext) -> Result<Value, String> {
    let program = ctx
        .project_path
        .clone()
        .unwrap_or_else(|| ctx.root.clone());
    let cwd = if program.is_dir() {
        program.clone()
    } else {
        program
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| ctx.root.clone())
    };
    let mut payload = json!({
        "name": "Go Launch",
        "type": "go",
        "request": "launch",
        "mode": "debug",
        "program": program.display().to_string(),
        "cwd": cwd.display().to_string(),
        "stopOnEntry": false,
    });
    if !ctx.env.is_empty() {
        let env_obj: serde_json::Map<String, Value> = ctx
            .env
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        payload["env"] = Value::Object(env_obj);
    }
    Ok(payload)
}

/// Scan `workspace_root` for directories that declare `package main`.
/// One directory may contain multiple `.go` files with the same package
/// declaration — we surface the directory once. Bounded depth so a
/// monorepo with thousands of packages doesn't stall the picker open.
pub fn find_go_main_dirs(workspace_root: &Path) -> Vec<PathBuf> {
    fn ignored(name: &str) -> bool {
        matches!(
            name,
            "vendor" | "node_modules" | ".git" | "target" | "bin" | "obj" | "testdata"
        )
    }
    fn walk(dir: &Path, found: &mut Vec<PathBuf>, depth: usize) {
        if depth > 6 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut has_main_pkg = false;
        let mut subdirs: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if name.starts_with('.') && name != "." {
                continue;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                if !ignored(name) {
                    subdirs.push(path);
                }
            } else if ft.is_file()
                && path.extension().and_then(|s| s.to_str()) == Some("go")
                && !name.ends_with("_test.go")
                && !has_main_pkg
            {
                // Cheap line scan — first non-comment, non-blank line is
                // `package <ident>`. If it's `package main`, mark the dir.
                if let Ok(text) = std::fs::read_to_string(&path) {
                    for line in text.lines().take(50) {
                        let t = line.trim_start();
                        if t.is_empty() || t.starts_with("//") {
                            continue;
                        }
                        if t.starts_with("package ") {
                            let rest = t[8..].trim();
                            if rest == "main" || rest.starts_with("main") {
                                has_main_pkg = true;
                            }
                            break;
                        }
                    }
                }
            }
        }
        if has_main_pkg {
            found.push(dir.to_path_buf());
        }
        for sub in subdirs {
            walk(&sub, found, depth + 1);
        }
    }
    let mut out = Vec::new();
    walk(workspace_root, &mut out, 0);
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// Python (debugpy)
// ---------------------------------------------------------------------------

const PYTHON: DapAdapterSpec = DapAdapterSpec {
    key: "python",
    adapter_id: "debugpy",
    // `python -m debugpy.adapter` is the canonical stdio adapter
    // entrypoint. We try `python3` first because most modern systems
    // (macOS, recent Debian) ship `python` as 2.x or not at all.
    cmd_candidates: &["python3", "python"],
    args: &["-m", "debugpy.adapter"],
    root_markers: &["pyproject.toml", "setup.py", "requirements.txt", "Pipfile"],
    prelaunch: |_| None,
    build_launch_args: python_launch_args,
};

/// debugpy `launch` request. `program` must be the script path; `cwd`
/// defaults to the script's directory unless an explicit workspace root
/// has been passed in.
fn python_launch_args(ctx: &LaunchContext) -> Result<Value, String> {
    let program = ctx
        .project_path
        .clone()
        .ok_or_else(|| "python: no entry script picked".to_string())?;
    if !program.is_file() {
        return Err(format!(
            "python: entry script {} not found",
            program.display()
        ));
    }
    let cwd = program
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| ctx.root.clone());
    let mut payload = json!({
        "name": "Python Launch",
        "type": "python",
        "request": "launch",
        "program": program.display().to_string(),
        "cwd": cwd.display().to_string(),
        "console": "internalConsole",
        "justMyCode": false,
        "stopOnEntry": false,
    });
    if !ctx.env.is_empty() {
        let env_obj: serde_json::Map<String, Value> = ctx
            .env
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        payload["env"] = Value::Object(env_obj);
    }
    Ok(payload)
}

/// Common Python entry-point script names at the workspace root, in
/// priority order. Used as fallbacks when the active buffer isn't a `.py`.
pub fn find_python_entry_scripts(workspace_root: &Path) -> Vec<PathBuf> {
    let candidates = [
        "main.py",
        "__main__.py",
        "app.py",
        "manage.py",
        "run.py",
        "server.py",
        "cli.py",
    ];
    let mut out = Vec::new();
    for name in &candidates {
        let p = workspace_root.join(name);
        if p.is_file() {
            out.push(p);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// lldb-dap (Rust / C / C++)
// ---------------------------------------------------------------------------

const RUST: DapAdapterSpec = DapAdapterSpec {
    key: "lldb",
    adapter_id: "lldb-dap",
    // `lldb-dap` is the modern binary name (LLVM 18+). `lldb-vscode` is
    // the legacy name shipped by older Xcode and Homebrew installs.
    cmd_candidates: &["lldb-dap", "lldb-vscode"],
    args: &[],
    root_markers: &["Cargo.toml"],
    prelaunch: rust_prelaunch,
    build_launch_args: rust_launch_args,
};

fn rust_prelaunch(ctx: &LaunchContext) -> Option<PrelaunchCommand> {
    let mut args: Vec<String> = vec!["build".into()];
    let label = if let Some(name) = &ctx.target_name {
        args.push("--bin".into());
        args.push(name.clone());
        format!("Building Rust bin: {name}")
    } else {
        "Building Rust crate".into()
    };
    Some(PrelaunchCommand {
        program: "cargo".into(),
        args,
        label,
    })
}

fn rust_launch_args(ctx: &LaunchContext) -> Result<Value, String> {
    // For workspace members, `target/debug` lives at the workspace root
    // (where the top-level Cargo.toml + Cargo.lock sit), not the member
    // crate directory. Walk up from `ctx.root` looking for `target/`,
    // falling back to `ctx.root` itself if we never see one (first build).
    let target_dir = find_cargo_target_dir(&ctx.root).unwrap_or_else(|| ctx.root.join("target"));
    let bin_name = ctx
        .target_name
        .clone()
        .or_else(|| {
            // Fall back to the crate's package name from the manifest
            // dir's `Cargo.toml`. We only consult `[package].name` —
            // workspace virtual manifests are picked at the member level
            // by the discovery step before this is called.
            ctx.project_path
                .as_ref()
                .and_then(|m| {
                    let text = std::fs::read_to_string(m).ok()?;
                    let val: toml::Value = text.parse().ok()?;
                    val.get("package")
                        .and_then(|p| p.get("name"))
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string())
                })
        })
        .ok_or_else(|| "rust: cannot determine target bin name".to_string())?;
    let program = target_dir.join("debug").join(&bin_name);
    if !program.is_file() {
        return Err(format!(
            "rust: built binary {} not found — did `cargo build` succeed?",
            program.display()
        ));
    }
    let cwd = ctx.root.clone();
    let mut payload = json!({
        "name": "Rust Launch",
        "type": "lldb-dap",
        "request": "launch",
        "program": program.display().to_string(),
        "cwd": cwd.display().to_string(),
        "stopOnEntry": false,
    });
    if !ctx.env.is_empty() {
        let env_arr: Vec<Value> = ctx
            .env
            .iter()
            .map(|(k, v)| Value::String(format!("{k}={v}")))
            .collect();
        // lldb-dap expects env as an array of "KEY=VALUE" strings,
        // unlike most adapters which want an object.
        payload["env"] = Value::Array(env_arr);
    }
    Ok(payload)
}

/// One bin target discovered in a Rust workspace. `manifest_dir` is
/// where prelaunch / launch runs; `bin_name` is what gets passed to
/// `cargo build --bin` and read out of `target/debug/`.
#[derive(Debug, Clone)]
pub struct RustBinTarget {
    pub manifest_path: PathBuf,
    pub bin_name: String,
}

/// Discover bin targets in a Rust workspace. Parses the top-level
/// `Cargo.toml` for `[workspace].members`; for each member (or the
/// crate itself if not a workspace) reads `[package].name` and any
/// `[[bin]]` entries. Auto-bin detection from `src/bin/*.rs` is
/// applied per member.
pub fn find_rust_bin_targets(workspace_root: &Path) -> Vec<RustBinTarget> {
    let top_manifest = workspace_root.join("Cargo.toml");
    if !top_manifest.is_file() {
        return Vec::new();
    }
    let Ok(top_text) = std::fs::read_to_string(&top_manifest) else { return Vec::new() };
    let Ok(top_val): Result<toml::Value, _> = top_text.parse() else { return Vec::new() };

    let mut member_manifests: Vec<PathBuf> = Vec::new();
    if let Some(members) = top_val
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
    {
        for m in members {
            if let Some(s) = m.as_str() {
                // Members can be glob patterns (`crates/*`). Expand the
                // common single-* case; literal paths are joined as-is.
                if s.contains('*') {
                    if let Some(rest) = s.strip_suffix("/*") {
                        let base = workspace_root.join(rest);
                        if let Ok(entries) = std::fs::read_dir(&base) {
                            for entry in entries.flatten() {
                                let p = entry.path();
                                let mf = p.join("Cargo.toml");
                                if mf.is_file() {
                                    member_manifests.push(mf);
                                }
                            }
                        }
                    }
                } else {
                    let mf = workspace_root.join(s).join("Cargo.toml");
                    if mf.is_file() {
                        member_manifests.push(mf);
                    }
                }
            }
        }
    }
    // Virtual-manifest case is members-only; a real crate at the root
    // also produces bins. Non-workspace single-crate case has no
    // [workspace] section but has a [package], so we just include the
    // top manifest.
    if top_val.get("package").is_some() {
        member_manifests.push(top_manifest.clone());
    } else if member_manifests.is_empty() {
        // Top-level Cargo.toml has neither [package] nor [workspace] —
        // unusable manifest. Bail.
        return Vec::new();
    }

    let mut out: Vec<RustBinTarget> = Vec::new();
    for manifest in member_manifests {
        let Ok(text) = std::fs::read_to_string(&manifest) else { continue };
        let Ok(val): Result<toml::Value, _> = text.parse() else { continue };
        let manifest_dir = manifest.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let pkg_name = val
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string());
        let mut bins_in_this_manifest: Vec<String> = Vec::new();
        // Explicit [[bin]] tables — each row's `name` wins.
        if let Some(bins) = val.get("bin").and_then(|v| v.as_array()) {
            for bin in bins {
                if let Some(name) = bin.get("name").and_then(|n| n.as_str()) {
                    bins_in_this_manifest.push(name.to_string());
                }
            }
        }
        // Auto-bins from `src/bin/*.rs` and `src/bin/<name>/main.rs`.
        let auto_bins_dir = manifest_dir.join("src").join("bin");
        if let Ok(entries) = std::fs::read_dir(&auto_bins_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else { continue };
                if p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("rs") {
                    bins_in_this_manifest.push(stem.to_string());
                } else if p.is_dir() && p.join("main.rs").is_file() {
                    bins_in_this_manifest.push(stem.to_string());
                }
            }
        }
        // Default bin: crates with `src/main.rs` produce a bin named
        // after the package. Don't double-add if [[bin]] already named
        // one with the same name.
        if let Some(name) = pkg_name.as_ref() {
            if manifest_dir.join("src").join("main.rs").is_file()
                && !bins_in_this_manifest.iter().any(|n| n == name)
            {
                bins_in_this_manifest.push(name.clone());
            }
        }
        for bin_name in bins_in_this_manifest {
            out.push(RustBinTarget {
                manifest_path: manifest.clone(),
                bin_name,
            });
        }
    }
    out.sort_by(|a, b| a.bin_name.cmp(&b.bin_name));
    out
}

/// Walk up from `start` looking for a directory that contains a
/// `target/` subdirectory (Cargo's default build dir). Used by Rust
/// `build_launch_args` to find the workspace's `target/debug/<bin>`
/// when the crate is a workspace member. Returns the `target/` path
/// itself, not the parent.
fn find_cargo_target_dir(start: &Path) -> Option<PathBuf> {
    let canon = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut dir: &Path = canon.as_path();
    loop {
        let candidate = dir.join("target");
        if candidate.is_dir() {
            return Some(candidate);
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p,
            _ => return None,
        }
    }
}

// ---------------------------------------------------------------------------
// .NET workspace + project helpers (kept distinct from generic find_workspace_root
// so the multi-project picker walks past .csproj-only ancestors to a .sln/.git root).
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Generic adapter selection + shared helpers
// ---------------------------------------------------------------------------

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
    fn adapter_for_workspace_finds_go_via_go_mod() {
        let tmp = std::env::temp_dir().join("binvim_dap_test_gomod");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("go.mod"), "module example.com/foo\n").unwrap();
        let found = adapter_for_workspace(&tmp).expect("should find go adapter");
        assert_eq!(found.0.key, "go");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn adapter_for_workspace_finds_python_via_pyproject() {
        let tmp = std::env::temp_dir().join("binvim_dap_test_pyproject");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("pyproject.toml"), "[project]\nname = 'x'\n").unwrap();
        let found = adapter_for_workspace(&tmp).expect("should find python adapter");
        assert_eq!(found.0.key, "python");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn adapter_for_workspace_finds_rust_via_cargo_toml() {
        let tmp = std::env::temp_dir().join("binvim_dap_test_cargo");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("Cargo.toml"), "[package]\nname = 'x'\nversion = '0.1.0'\n").unwrap();
        let found = adapter_for_workspace(&tmp).expect("should find rust adapter");
        assert_eq!(found.0.key, "lldb");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_rust_bin_targets_picks_up_default_main() {
        let tmp = std::env::temp_dir().join("binvim_dap_test_rust_default");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src")).unwrap();
        fs::write(
            tmp.join("Cargo.toml"),
            "[package]\nname = \"hello\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(tmp.join("src").join("main.rs"), "fn main(){}\n").unwrap();
        let bins = find_rust_bin_targets(&tmp);
        assert_eq!(bins.len(), 1);
        assert_eq!(bins[0].bin_name, "hello");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_rust_bin_targets_picks_up_src_bin_files() {
        let tmp = std::env::temp_dir().join("binvim_dap_test_rust_src_bin");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src").join("bin")).unwrap();
        fs::write(
            tmp.join("Cargo.toml"),
            "[package]\nname = \"libcrate\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(tmp.join("src").join("bin").join("alpha.rs"), "fn main(){}\n").unwrap();
        fs::write(tmp.join("src").join("bin").join("beta.rs"), "fn main(){}\n").unwrap();
        let bins = find_rust_bin_targets(&tmp);
        let names: Vec<_> = bins.iter().map(|b| b.bin_name.clone()).collect();
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_go_main_dirs_picks_up_main_packages() {
        let tmp = std::env::temp_dir().join("binvim_dap_test_go_main");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("cmd").join("server")).unwrap();
        fs::create_dir_all(tmp.join("internal")).unwrap();
        fs::write(tmp.join("go.mod"), "module example.com/x\n").unwrap();
        fs::write(
            tmp.join("cmd").join("server").join("main.go"),
            "package main\nfunc main(){}\n",
        )
        .unwrap();
        fs::write(
            tmp.join("internal").join("util.go"),
            "package internal\n",
        )
        .unwrap();
        let dirs = find_go_main_dirs(&tmp);
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].ends_with("cmd/server"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_python_entry_scripts_lists_common_names() {
        let tmp = std::env::temp_dir().join("binvim_dap_test_py_entry");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("main.py"), "print('x')\n").unwrap();
        fs::write(tmp.join("manage.py"), "print('y')\n").unwrap();
        fs::write(tmp.join("README.md"), "").unwrap();
        let scripts = find_python_entry_scripts(&tmp);
        let names: Vec<_> = scripts
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
            .collect();
        assert!(names.contains(&"main.py".to_string()));
        assert!(names.contains(&"manage.py".to_string()));
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
