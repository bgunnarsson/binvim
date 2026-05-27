//! Generic package-manager backend behind the `<leader>p` entry point. A
//! single keybinding detects the ecosystem from the active buffer's workspace
//! and drives an add/upgrade flow; the per-ecosystem CLI plumbing lives here.
//!
//! Today only .NET (NuGet, via the `dotnet` CLI) is implemented. npm and cargo
//! slot in as additional `PackageEcosystem` arms + match arms in the dispatch
//! fns below — no caller changes — the same way `lsp/specs.rs` and
//! `dap/specs.rs` hard-wire one spec per language. There is no plugin system.
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
}

impl PackageEcosystem {
    pub fn label(self) -> &'static str {
        match self {
            PackageEcosystem::DotNet => "NuGet",
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

/// Detect which package ecosystem applies to the active buffer. The buffer's
/// own extension wins first (a `.cs` file always resolves to NuGet), then we
/// fall back to marker files under the workspace root. Returns `None` when no
/// implemented backend matches — the caller surfaces that as a status message.
pub fn detect(buffer_path: Option<&Path>, workspace_root: &Path) -> Option<PackageEcosystem> {
    if let Some(p) = buffer_path {
        let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
        if matches!(
            ext,
            "cs" | "csproj" | "fsproj" | "vbproj" | "fs" | "vb" | "razor" | "cshtml"
        ) {
            return Some(PackageEcosystem::DotNet);
        }
    }
    // Workspace fallback: any .csproj/.fsproj/.vbproj under the root marks a
    // .NET workspace even when the active buffer isn't a C# file.
    if !crate::dap::find_dotnet_projects(workspace_root).is_empty() {
        return Some(PackageEcosystem::DotNet);
    }
    None
}

/// Enumerate the dependency manifests under `workspace_root` for `eco`.
pub fn find_manifests(eco: PackageEcosystem, workspace_root: &Path) -> Vec<Manifest> {
    match eco {
        // Reuse the DAP layer's project discovery — it already finds
        // `.csproj/.fsproj/.vbproj`, skips bin/obj, and is depth-bounded.
        PackageEcosystem::DotNet => crate::dap::find_dotnet_projects(workspace_root)
            .into_iter()
            .map(|path| {
                let display = path
                    .strip_prefix(workspace_root)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                Manifest { path, display }
            })
            .collect(),
    }
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
            )?;
            parse_installed_json(&json)
        }
    }
}

/// List every available version of `id` from the registry, newest-first. Both
/// stable and prerelease versions are returned (each tagged); the caller hides
/// prereleases until the user toggles them, so this never needs a refetch.
pub fn list_versions(eco: PackageEcosystem, id: &str) -> Result<Vec<PackageVersion>, String> {
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
            )?;
            parse_versions_json(&json)
        }
    }
}

/// Search the registry for packages matching `query`.
pub fn search(eco: PackageEcosystem, query: &str) -> Result<Vec<SearchHit>, String> {
    match eco {
        PackageEcosystem::DotNet => {
            let json = run_capture("dotnet", &["package", "search", query, "--format", "json"])?;
            parse_search_json(&json)
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
            )
            .map(|_| ())
        }
    }
}

/// A version string is a prerelease iff it carries a SemVer pre-release
/// identifier (`1.0.0-preview`, `2.0.0-rc.1`). Build metadata (`+sha`) doesn't
/// count, so we look only at the part before any `+`.
pub fn is_prerelease(version: &str) -> bool {
    let core = version.split('+').next().unwrap_or(version);
    core.contains('-')
}

/// Run `bin args…` to completion and return stdout. On failure returns a short
/// message with the exit code + a few lines of diagnostic output — same shape
/// as `format.rs`'s `run_stdin_pipe`, minus the stdin write. `dotnet` writes
/// some errors to stdout rather than stderr, so we fall back to stdout when
/// stderr is empty.
fn run_capture(bin: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(bin)
        .args(args)
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

/// Parse `dotnet list <csproj> package --format json`. Unions top-level
/// packages across all target frameworks, deduping by id (a multi-targeted
/// project lists the same package once per framework), sorted by id.
pub fn parse_installed_json(json: &str) -> Result<Vec<InstalledPackage>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("parse installed: {e}"))?;
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
    let v: Value = serde_json::from_str(json).map_err(|e| format!("parse versions: {e}"))?;
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
    let v: Value = serde_json::from_str(json).map_err(|e| format!("parse search: {e}"))?;
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
    fn prerelease_detection() {
        assert!(is_prerelease("8.0.0-preview.1"));
        assert!(is_prerelease("2.0.0-rc.1"));
        assert!(!is_prerelease("13.0.3"));
        assert!(!is_prerelease("1.0.0+build.5")); // build metadata only
    }

    #[test]
    fn detects_dotnet_from_buffer_extension() {
        let tmp = std::env::temp_dir();
        assert_eq!(
            detect(Some(Path::new("/x/Foo.cs")), &tmp),
            Some(PackageEcosystem::DotNet)
        );
        assert_eq!(
            detect(Some(Path::new("/x/Foo.csproj")), &tmp),
            Some(PackageEcosystem::DotNet)
        );
    }
}
