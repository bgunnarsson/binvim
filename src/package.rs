//! Generic package-manager backend behind the `<leader>p` entry point. A
//! single keybinding detects the ecosystem from the active buffer's workspace
//! and drives an add/upgrade flow; the per-ecosystem CLI plumbing lives here.
//!
//! .NET (NuGet, via the `dotnet` CLI) and npm (via the `npm` CLI) are
//! implemented. cargo / go / pip slot in as additional `PackageEcosystem`
//! variants with match arms in the dispatch fns below — no caller changes —
//! the same way `lsp/specs.rs` and `dap/specs.rs` hard-wire one spec per
//! language. There is no plugin system. Those three each need an HTTP fallback
//! for the one step their CLI can't do (cargo version-list, go search, pip
//! search), which is why they're deliberately not here yet.
//!
//! The `App`-side flow (picker chaining + the background-thread channel) lives
//! in `app/package_glue.rs`; everything here is pure CLI invocation + parsing,
//! so the parsers below carry the test coverage.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageEcosystem {
    /// .NET — `.csproj` / `.fsproj` manifests, driven by the `dotnet` CLI.
    DotNet,
    /// npm — `package.json` manifests, driven by the `npm` CLI.
    Npm,
}

impl PackageEcosystem {
    pub fn label(self) -> &'static str {
        match self {
            PackageEcosystem::DotNet => "NuGet",
            PackageEcosystem::Npm => "npm",
        }
    }
}

/// A dependency manifest the user can target — a `.csproj` for .NET, later a
/// `package.json` / `Cargo.toml`.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub path: PathBuf,
    /// Short display form for the picker (workspace-relative path).
    pub display: String,
}

/// One installed top-level package as reported by the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackage {
    pub id: String,
    /// The version constraint written in the manifest (may be a range).
    pub requested: String,
    /// The single concrete version currently resolved.
    pub resolved: String,
}

/// One available version of a package from the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageVersion {
    pub version: String,
    pub prerelease: bool,
}

/// One hit from a registry search — the package id plus its latest version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub id: String,
    pub latest: String,
}

/// Detect which package ecosystem applies to the active buffer and resolve its
/// workspace root in one step. The buffer's own extension wins first (a `.cs`
/// file always resolves to NuGet, a `.ts` file to npm), then we fall back to
/// whichever ecosystem actually has a manifest in the directories above
/// `start_dir`. Returns `None` when no implemented backend matches — the caller
/// surfaces that as a status message. `start_dir` is the buffer's parent dir
/// (or the cwd for an unnamed buffer); the returned path is the workspace root
/// the rest of the flow walks for manifests.
pub fn detect(buffer_path: Option<&Path>, start_dir: &Path) -> Option<(PackageEcosystem, PathBuf)> {
    if let Some(eco) = eco_from_extension(buffer_path) {
        return Some((eco, workspace_root(eco, start_dir)));
    }
    // Marker fallback: pick the first ecosystem that has a manifest somewhere
    // above `start_dir`. .NET is probed before npm so a mixed repo (an ASP.NET
    // app with a frontend) keeps resolving to NuGet when the active buffer is
    // neither a C# nor a JS/TS file.
    for eco in [PackageEcosystem::DotNet, PackageEcosystem::Npm] {
        let root = workspace_root(eco, start_dir);
        if !find_manifests(eco, &root).is_empty() {
            return Some((eco, root));
        }
    }
    None
}

/// Resolve `eco`'s ecosystem from the active buffer's extension / basename
/// alone, without touching the filesystem. Returns `None` for unrelated files
/// so `detect` can fall through to the marker walk.
fn eco_from_extension(buffer_path: Option<&Path>) -> Option<PackageEcosystem> {
    let p = buffer_path?;
    // `package.json` is the npm manifest itself — match it by basename before
    // the generic extension test, since `.json` alone is too broad to claim.
    if p.file_name().and_then(|s| s.to_str()) == Some("package.json") {
        return Some(PackageEcosystem::Npm);
    }
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
    if matches!(
        ext,
        "cs" | "csproj" | "fsproj" | "vbproj" | "fs" | "vb" | "razor" | "cshtml"
    ) {
        return Some(PackageEcosystem::DotNet);
    }
    if matches!(
        ext,
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "vue" | "svelte" | "astro"
    ) {
        return Some(PackageEcosystem::Npm);
    }
    None
}

/// Walk up from `start_dir` to the workspace root appropriate for `eco`.
pub fn workspace_root(eco: PackageEcosystem, start_dir: &Path) -> PathBuf {
    match eco {
        PackageEcosystem::DotNet => crate::dap::find_dotnet_workspace_root(start_dir),
        PackageEcosystem::Npm => find_npm_workspace_root(start_dir),
    }
}

/// Enumerate the dependency manifests under `workspace_root` for `eco`.
pub fn find_manifests(eco: PackageEcosystem, workspace_root: &Path) -> Vec<Manifest> {
    let paths = match eco {
        // Reuse the DAP layer's project discovery — it already finds
        // `.csproj/.fsproj/.vbproj`, skips bin/obj, and is depth-bounded.
        PackageEcosystem::DotNet => crate::dap::find_dotnet_projects(workspace_root),
        PackageEcosystem::Npm => find_npm_manifests(workspace_root),
    };
    paths
        .into_iter()
        .map(|path| {
            let display = path
                .strip_prefix(workspace_root)
                .unwrap_or(&path)
                .display()
                .to_string();
            Manifest { path, display }
        })
        .collect()
}

/// Walk up from `start` to the npm workspace root: the closest `.git` directory
/// (so a monorepo's sibling packages are all enumerable), else the nearest
/// ancestor holding a `package.json`, else `start` itself.
fn find_npm_workspace_root(start: &Path) -> PathBuf {
    let canon = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut dir: &Path = canon.as_path();
    let mut nearest_pkg: Option<PathBuf> = None;
    loop {
        if nearest_pkg.is_none() && dir.join("package.json").is_file() {
            nearest_pkg = Some(dir.to_path_buf());
        }
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p,
            _ => break,
        }
    }
    nearest_pkg.unwrap_or(canon)
}

/// Recursively enumerate `package.json` manifests under `dir`, skipping
/// `node_modules` and other build/VCS dirs. Depth-bounded to stay cheap on
/// large monorepos — mirrors `dap::find_dotnet_projects`.
fn find_npm_manifests(dir: &Path) -> Vec<PathBuf> {
    fn ignored(name: &str) -> bool {
        matches!(
            name,
            "node_modules" | ".git" | "bin" | "obj" | "target" | "dist" | "build" | ".next"
        )
    }
    fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
        if depth > 6 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if !name.starts_with('.') && !ignored(name) {
                    walk(&path, out, depth + 1);
                }
            } else if file_type.is_file() && name == "package.json" {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out, 0);
    out.sort();
    out
}

/// List the top-level packages already referenced by `manifest`.
pub fn list_installed(
    eco: PackageEcosystem,
    manifest: &Path,
) -> Result<Vec<InstalledPackage>, String> {
    match eco {
        PackageEcosystem::DotNet => {
            let path = manifest.to_string_lossy();
            // `--format json` needs the restore assets; .NET 10 auto-restores,
            // which is why this can take a few seconds (hence: off-thread).
            let json = run_capture(
                "dotnet",
                &["list", path.as_ref(), "package", "--format", "json"],
                manifest_dir(manifest),
                DOTNET_ENVS,
            )?;
            parse_installed_json(&json)
        }
        // Read the manifest directly rather than `npm ls` so the list works
        // before `node_modules` exists. The "resolved" version is the spec with
        // its range operator stripped — good enough to flag in the picker, but
        // it tracks the requested floor, not a lockfile-resolved version.
        PackageEcosystem::Npm => {
            let text = std::fs::read_to_string(manifest)
                .map_err(|e| format!("read {}: {e}", manifest.display()))?;
            parse_npm_installed_json(&text)
        }
    }
}

/// List every available version of `id` from the registry, newest-first. Both
/// stable and prerelease versions are returned (each tagged); the caller hides
/// prereleases until the user toggles them, so this never needs a refetch.
/// Runs in the manifest's directory so the project's `nuget.config` (and any
/// private feeds it declares) is honoured.
pub fn list_versions(
    eco: PackageEcosystem,
    manifest: &Path,
    id: &str,
) -> Result<Vec<PackageVersion>, String> {
    match eco {
        PackageEcosystem::DotNet => {
            let json = run_capture(
                "dotnet",
                &[
                    "package",
                    "search",
                    id,
                    "--exact-match",
                    "--prerelease",
                    "--format",
                    "json",
                ],
                manifest_dir(manifest),
                DOTNET_ENVS,
            )?;
            parse_versions_json(&json)
        }
        // `npm view <id> versions --json` lists every published version; run in
        // the manifest dir so a project-local `.npmrc` (private registry) wins.
        PackageEcosystem::Npm => {
            let json = run_capture(
                "npm",
                &["view", id, "versions", "--json"],
                manifest_dir(manifest),
                &[],
            )?;
            parse_npm_versions_json(&json)
        }
    }
}

/// Search the registry for packages matching `query`. Runs in the manifest's
/// directory so the project's `nuget.config` / private feeds apply.
pub fn search(
    eco: PackageEcosystem,
    manifest: &Path,
    query: &str,
) -> Result<Vec<SearchHit>, String> {
    match eco {
        PackageEcosystem::DotNet => {
            let json = run_capture(
                "dotnet",
                &["package", "search", query, "--format", "json"],
                manifest_dir(manifest),
                DOTNET_ENVS,
            )?;
            parse_search_json(&json)
        }
        PackageEcosystem::Npm => {
            let json = run_capture(
                "npm",
                &["search", query, "--json"],
                manifest_dir(manifest),
                &[],
            )?;
            parse_npm_search_json(&json)
        }
    }
}

/// Add (or change the version of) `id` at `version` in `manifest`. Restores as
/// a side effect, so this is run off the main thread like the other calls.
pub fn add(eco: PackageEcosystem, manifest: &Path, id: &str, version: &str) -> Result<(), String> {
    match eco {
        PackageEcosystem::DotNet => {
            let path = manifest.to_string_lossy();
            run_capture(
                "dotnet",
                &["add", path.as_ref(), "package", id, "--version", version],
                manifest_dir(manifest),
                DOTNET_ENVS,
            )
            .map(|_| ())
        }
        // `npm install <id>@<version>` installs and writes the dependency to
        // `package.json` in one go — the npm analogue of `dotnet add package`.
        PackageEcosystem::Npm => {
            let spec = format!("{id}@{version}");
            run_capture("npm", &["install", &spec], manifest_dir(manifest), &[]).map(|_| ())
        }
    }
}

/// The directory a command should run in for a given manifest — its parent so
/// `nuget.config` discovery walks up from the project.
fn manifest_dir(manifest: &Path) -> Option<&Path> {
    manifest.parent()
}

/// A version string is a prerelease iff it carries a SemVer pre-release
/// identifier (`1.0.0-preview`, `2.0.0-rc.1`). Build metadata (`+sha`) doesn't
/// count, so we look only at the part before any `+`.
pub fn is_prerelease(version: &str) -> bool {
    let core = version.split('+').next().unwrap_or(version);
    core.contains('-')
}

/// Env vars set for every `dotnet` invocation: suppress the first-run "Welcome
/// to .NET" banner + telemetry notice, which older SDKs print to *stdout* ahead
/// of the JSON and would otherwise break the parse. `json_slice` is the belt to
/// these braces. npm needs no such env, so it passes an empty slice.
const DOTNET_ENVS: &[(&str, &str)] =
    &[("DOTNET_NOLOGO", "1"), ("DOTNET_CLI_TELEMETRY_OPTOUT", "1")];

/// Run `bin args…` to completion and return stdout. On failure returns a short
/// message with the exit code + a few lines of diagnostic output — same shape
/// as `format.rs`'s `run_stdin_pipe`, minus the stdin write. `dotnet` writes
/// some errors to stdout rather than stderr, so we fall back to stdout when
/// stderr is empty.
fn run_capture(
    bin: &str,
    args: &[&str],
    cwd: Option<&Path>,
    envs: &[(&str, &str)],
) -> Result<String, String> {
    let mut cmd = Command::new(bin);
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("failed to run {bin}: {e}"))?;
    if !output.status.success() {
        let pick = |bytes: &[u8]| -> String {
            String::from_utf8_lossy(bytes)
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .take(4)
                .collect::<Vec<_>>()
                .join(" / ")
        };
        let mut msg = pick(&output.stderr);
        if msg.is_empty() {
            msg = pick(&output.stdout);
        }
        if msg.is_empty() {
            msg = "(no output)".to_string();
        }
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "?".into());
        return Err(format!("{bin} exit {code}: {msg}"));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("{bin} stdout not utf-8: {e}"))
}

/// Narrow `s` to the outermost JSON object (first `{` … last `}`). `dotnet`
/// can prepend a first-run banner / telemetry notice and (on some SDKs) append
/// help text to stdout; slicing to the braces lets the parse survive that.
/// Falls back to the whole string when no braces are present.
fn json_slice(s: &str) -> &str {
    match (s.find('{'), s.rfind('}')) {
        (Some(a), Some(b)) if b >= a => &s[a..=b],
        _ => s,
    }
}

/// Parse `dotnet list <csproj> package --format json`. Unions top-level
/// packages across all target frameworks, deduping by id (a multi-targeted
/// project lists the same package once per framework), sorted by id.
pub fn parse_installed_json(json: &str) -> Result<Vec<InstalledPackage>, String> {
    let v: Value =
        serde_json::from_str(json_slice(json)).map_err(|e| format!("parse installed: {e}"))?;
    let mut out: Vec<InstalledPackage> = Vec::new();
    for proj in v
        .get("projects")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
    {
        for fw in proj
            .get("frameworks")
            .and_then(|f| f.as_array())
            .into_iter()
            .flatten()
        {
            for pkg in fw
                .get("topLevelPackages")
                .and_then(|p| p.as_array())
                .into_iter()
                .flatten()
            {
                let id = pkg.get("id").and_then(|s| s.as_str()).unwrap_or("");
                if id.is_empty() || out.iter().any(|e| e.id == id) {
                    continue;
                }
                out.push(InstalledPackage {
                    id: id.to_string(),
                    requested: pkg
                        .get("requestedVersion")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string(),
                    resolved: pkg
                        .get("resolvedVersion")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string(),
                });
            }
        }
    }
    out.sort_by_key(|p| p.id.to_lowercase());
    Ok(out)
}

/// Parse `dotnet package search <id> --exact-match --prerelease --format json`.
/// Returns every available version, newest-first. SDKs ≥ 8.0.4xx emit a
/// `version` per entry (one entry per version); older SDKs emit only a single
/// `latestVersion` — we fall back to that so the feature degrades to
/// latest-only rather than erroring.
pub fn parse_versions_json(json: &str) -> Result<Vec<PackageVersion>, String> {
    let v: Value =
        serde_json::from_str(json_slice(json)).map_err(|e| format!("parse versions: {e}"))?;
    let mut out: Vec<PackageVersion> = Vec::new();
    for src in v
        .get("searchResult")
        .and_then(|s| s.as_array())
        .into_iter()
        .flatten()
    {
        for pkg in src
            .get("packages")
            .and_then(|p| p.as_array())
            .into_iter()
            .flatten()
        {
            let ver = pkg
                .get("version")
                .and_then(|s| s.as_str())
                .or_else(|| pkg.get("latestVersion").and_then(|s| s.as_str()));
            if let Some(ver) = ver {
                if ver.is_empty() || out.iter().any(|e| e.version == ver) {
                    continue;
                }
                out.push(PackageVersion {
                    version: ver.to_string(),
                    prerelease: is_prerelease(ver),
                });
            }
        }
    }
    // The CLI lists oldest→newest; the picker wants newest first.
    out.reverse();
    Ok(out)
}

/// Parse `dotnet package search <query> --format json` — registry hits with
/// their latest version, kept in the registry's relevance order.
pub fn parse_search_json(json: &str) -> Result<Vec<SearchHit>, String> {
    let v: Value =
        serde_json::from_str(json_slice(json)).map_err(|e| format!("parse search: {e}"))?;
    let mut out: Vec<SearchHit> = Vec::new();
    for src in v
        .get("searchResult")
        .and_then(|s| s.as_array())
        .into_iter()
        .flatten()
    {
        for pkg in src
            .get("packages")
            .and_then(|p| p.as_array())
            .into_iter()
            .flatten()
        {
            let id = pkg.get("id").and_then(|s| s.as_str()).unwrap_or("");
            if id.is_empty() {
                continue;
            }
            let latest = pkg
                .get("latestVersion")
                .and_then(|s| s.as_str())
                .or_else(|| pkg.get("version").and_then(|s| s.as_str()))
                .unwrap_or("")
                .to_string();
            out.push(SearchHit {
                id: id.to_string(),
                latest,
            });
        }
    }
    Ok(out)
}

/// Parse a `package.json` and return its top-level dependencies. Unions
/// `dependencies` + `devDependencies` (deduping by name, first wins), sorted by
/// id. `requested` is the verbatim version spec (`^4.17.21`, `~1.2.0`, `*`);
/// `resolved` is that spec with its range operator stripped, which is what the
/// version picker flags as "● installed".
pub fn parse_npm_installed_json(json: &str) -> Result<Vec<InstalledPackage>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("parse package.json: {e}"))?;
    let mut out: Vec<InstalledPackage> = Vec::new();
    for key in ["dependencies", "devDependencies"] {
        let Some(deps) = v.get(key).and_then(|d| d.as_object()) else {
            continue;
        };
        for (id, spec) in deps {
            if id.is_empty() || out.iter().any(|e| e.id == *id) {
                continue;
            }
            let requested = spec.as_str().unwrap_or("").to_string();
            out.push(InstalledPackage {
                id: id.clone(),
                resolved: clean_npm_version(&requested),
                requested,
            });
        }
    }
    out.sort_by_key(|p| p.id.to_lowercase());
    Ok(out)
}

/// Parse `npm view <id> versions --json`. npm emits a JSON array of version
/// strings oldest→newest (or a bare string when only one version exists); we
/// reverse to newest-first and tag prereleases. Empty / `null` output means the
/// package has no published versions.
pub fn parse_npm_versions_json(json: &str) -> Result<Vec<PackageVersion>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("parse npm versions: {e}"))?;
    let raw: Vec<String> = match v {
        Value::Array(arr) => arr
            .into_iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        Value::String(s) => vec![s],
        _ => Vec::new(),
    };
    let mut out: Vec<PackageVersion> = raw
        .into_iter()
        .map(|version| PackageVersion {
            prerelease: is_prerelease(&version),
            version,
        })
        .collect();
    out.reverse(); // npm lists oldest→newest; the picker wants newest first.
    Ok(out)
}

/// Parse `npm search <query> --json` — an array of `{name, version, …}` hits,
/// kept in the registry's relevance order.
pub fn parse_npm_search_json(json: &str) -> Result<Vec<SearchHit>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("parse npm search: {e}"))?;
    let mut out: Vec<SearchHit> = Vec::new();
    for pkg in v.as_array().into_iter().flatten() {
        let id = pkg.get("name").and_then(|s| s.as_str()).unwrap_or("");
        if id.is_empty() {
            continue;
        }
        out.push(SearchHit {
            id: id.to_string(),
            latest: pkg
                .get("version")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
        });
    }
    Ok(out)
}

/// Strip a leading SemVer range operator (`^`, `~`, `>=`, `v`, …) off an npm
/// version spec so it can be compared against a concrete published version.
/// A spec with embedded whitespace (a compound range like `>=1 <2`) or no
/// digit core returns empty — there's no single version to flag.
fn clean_npm_version(spec: &str) -> String {
    let spec = spec.trim();
    let core = spec.trim_start_matches(['^', '~', '>', '<', '=', 'v', ' ']);
    if core.is_empty()
        || core.contains(char::is_whitespace)
        || !core.starts_with(|c: char| c.is_ascii_digit())
    {
        return String::new();
    }
    core.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured verbatim from `dotnet list lib.csproj package --format json`.
    const INSTALLED: &str = r#"{"version":1,"parameters":"","projects":[{"path":"/x/lib.csproj","frameworks":[{"framework":"net10.0","topLevelPackages":[{"id":"Newtonsoft.Json","requestedVersion":"13.0.1","resolvedVersion":"13.0.1"}]}]}]}"#;

    #[test]
    fn parses_installed_packages() {
        let pkgs = parse_installed_json(INSTALLED).unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].id, "Newtonsoft.Json");
        assert_eq!(pkgs[0].requested, "13.0.1");
        assert_eq!(pkgs[0].resolved, "13.0.1");
    }

    #[test]
    fn dedupes_installed_across_frameworks() {
        let json = r#"{"projects":[{"frameworks":[
            {"framework":"net8.0","topLevelPackages":[{"id":"Serilog","requestedVersion":"3.0.0","resolvedVersion":"3.0.0"}]},
            {"framework":"net10.0","topLevelPackages":[{"id":"Serilog","requestedVersion":"3.0.0","resolvedVersion":"3.0.0"}]}
        ]}]}"#;
        let pkgs = parse_installed_json(json).unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].id, "Serilog");
    }

    #[test]
    fn empty_installed_is_ok() {
        assert!(
            parse_installed_json(r#"{"projects":[]}"#)
                .unwrap()
                .is_empty()
        );
        assert!(parse_installed_json(r#"{}"#).unwrap().is_empty());
    }

    #[test]
    fn parses_versions_newest_first_with_prerelease_flags() {
        // Captured shape from `dotnet package search … --exact-match
        // --prerelease --format json` (oldest→newest in the source).
        let json = r#"{"version":2,"problems":[],"searchResult":[{"sourceName":"nuget.org","packages":[
            {"id":"Newtonsoft.Json","version":"3.5.8"},
            {"id":"Newtonsoft.Json","version":"12.0.3"},
            {"id":"Newtonsoft.Json","version":"13.0.0-beta1"},
            {"id":"Newtonsoft.Json","version":"13.0.3"}
        ]}]}"#;
        let vs = parse_versions_json(json).unwrap();
        assert_eq!(vs.len(), 4);
        assert_eq!(vs[0].version, "13.0.3"); // newest first
        assert_eq!(vs.last().unwrap().version, "3.5.8");
        assert!(
            vs.iter()
                .find(|v| v.version == "13.0.0-beta1")
                .unwrap()
                .prerelease
        );
        assert!(
            !vs.iter()
                .find(|v| v.version == "13.0.3")
                .unwrap()
                .prerelease
        );
    }

    #[test]
    fn versions_fall_back_to_latest_version_key() {
        // Older SDKs emit only `latestVersion` per package.
        let json = r#"{"searchResult":[{"packages":[{"id":"X","latestVersion":"2.1.0"}]}]}"#;
        let vs = parse_versions_json(json).unwrap();
        assert_eq!(vs.len(), 1);
        assert_eq!(vs[0].version, "2.1.0");
        assert!(!vs[0].prerelease);
    }

    #[test]
    fn parses_search_hits() {
        let json = r#"{"searchResult":[{"packages":[
            {"id":"Newtonsoft.Json","latestVersion":"13.0.3","totalDownloads":1},
            {"id":"Newtonsoft.Json.Bson","latestVersion":"1.0.2"}
        ]}]}"#;
        let hits = parse_search_json(json).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "Newtonsoft.Json");
        assert_eq!(hits[0].latest, "13.0.3");
        assert_eq!(hits[1].id, "Newtonsoft.Json.Bson");
    }

    #[test]
    fn parses_versions_with_dotnet_first_run_banner() {
        // Older SDKs print the first-run banner to stdout ahead of the JSON;
        // the parser must tolerate it (this is the reported crash).
        let json = "\nWelcome to .NET 8.0!\r\n---------------------\r\nSDK Version: 8.0.419\n\nTelemetry\r\n---------\r\nThe .NET tools collect usage data...\n\n{\"searchResult\":[{\"packages\":[{\"id\":\"X\",\"version\":\"1.2.3\"}]}]}\n";
        let vs = parse_versions_json(json).unwrap();
        assert_eq!(vs.len(), 1);
        assert_eq!(vs[0].version, "1.2.3");
    }

    #[test]
    fn json_slice_extracts_object() {
        assert_eq!(json_slice("noise {\"a\":1} trailing"), "{\"a\":1}");
        assert_eq!(json_slice("{\"a\":1}"), "{\"a\":1}");
        assert_eq!(json_slice("no braces here"), "no braces here");
    }

    #[test]
    fn prerelease_detection() {
        assert!(is_prerelease("8.0.0-preview.1"));
        assert!(is_prerelease("2.0.0-rc.1"));
        assert!(!is_prerelease("13.0.3"));
        assert!(!is_prerelease("1.0.0+build.5")); // build metadata only
    }

    #[test]
    fn detects_ecosystem_from_buffer_extension() {
        let tmp = std::env::temp_dir();
        let eco = |p: &str| detect(Some(Path::new(p)), &tmp).map(|(e, _)| e);
        assert_eq!(eco("/x/Foo.cs"), Some(PackageEcosystem::DotNet));
        assert_eq!(eco("/x/Foo.csproj"), Some(PackageEcosystem::DotNet));
        assert_eq!(eco("/x/app.ts"), Some(PackageEcosystem::Npm));
        assert_eq!(eco("/x/app.tsx"), Some(PackageEcosystem::Npm));
        assert_eq!(eco("/x/package.json"), Some(PackageEcosystem::Npm));
        // A generic `.json` is too broad to claim for npm by extension alone.
        assert_eq!(eco("/x/appsettings.json"), None);
    }

    #[test]
    fn parses_npm_installed_deps_and_dev_deps() {
        let json = r#"{
            "name": "demo",
            "dependencies": { "lodash": "^4.17.21", "react": "18.2.0" },
            "devDependencies": { "typescript": "~5.4.0" }
        }"#;
        let pkgs = parse_npm_installed_json(json).unwrap();
        assert_eq!(pkgs.len(), 3);
        // Sorted by id (lowercase).
        assert_eq!(pkgs[0].id, "lodash");
        assert_eq!(pkgs[0].requested, "^4.17.21");
        assert_eq!(pkgs[0].resolved, "4.17.21");
        assert_eq!(pkgs[1].id, "react");
        assert_eq!(pkgs[1].resolved, "18.2.0");
        assert_eq!(pkgs[2].id, "typescript");
        assert_eq!(pkgs[2].resolved, "5.4.0");
    }

    #[test]
    fn npm_installed_dedupes_dep_in_both_sections() {
        let json = r#"{
            "dependencies": { "esbuild": "0.20.0" },
            "devDependencies": { "esbuild": "0.21.0" }
        }"#;
        let pkgs = parse_npm_installed_json(json).unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].resolved, "0.20.0"); // dependencies wins
    }

    #[test]
    fn empty_npm_manifest_is_ok() {
        assert!(
            parse_npm_installed_json(r#"{"name":"x"}"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parses_npm_versions_newest_first_with_prerelease_flags() {
        let json = r#"["1.0.0","2.0.0-beta.1","2.0.0"]"#;
        let vs = parse_npm_versions_json(json).unwrap();
        assert_eq!(vs.len(), 3);
        assert_eq!(vs[0].version, "2.0.0"); // newest first
        assert_eq!(vs.last().unwrap().version, "1.0.0");
        assert!(
            vs.iter()
                .find(|v| v.version == "2.0.0-beta.1")
                .unwrap()
                .prerelease
        );
        assert!(!vs.iter().find(|v| v.version == "2.0.0").unwrap().prerelease);
    }

    #[test]
    fn npm_versions_handles_single_string_form() {
        // npm emits a bare string when a package has exactly one version.
        let vs = parse_npm_versions_json(r#""1.2.3""#).unwrap();
        assert_eq!(vs.len(), 1);
        assert_eq!(vs[0].version, "1.2.3");
    }

    #[test]
    fn parses_npm_search_hits() {
        let json = r#"[
            {"name":"lodash","version":"4.17.21","description":"…"},
            {"name":"lodash.merge","version":"4.6.2"}
        ]"#;
        let hits = parse_npm_search_json(json).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "lodash");
        assert_eq!(hits[0].latest, "4.17.21");
        assert_eq!(hits[1].id, "lodash.merge");
    }

    #[test]
    fn clean_npm_version_strips_range_operators() {
        assert_eq!(clean_npm_version("^4.17.21"), "4.17.21");
        assert_eq!(clean_npm_version("~1.2.0"), "1.2.0");
        assert_eq!(clean_npm_version(">=3.0.0"), "3.0.0");
        assert_eq!(clean_npm_version("18.2.0"), "18.2.0");
        // No single version to flag for these.
        assert_eq!(clean_npm_version("*"), "");
        assert_eq!(clean_npm_version(">=1 <2"), "");
        assert_eq!(clean_npm_version("workspace:*"), "");
    }
}
