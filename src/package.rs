//! Generic package-manager backend behind the `<leader>p` entry point. A
//! single keybinding detects the ecosystem from the active buffer's workspace
//! and drives an add/upgrade flow; the per-ecosystem CLI plumbing lives here.
//!
//! .NET (NuGet, via `dotnet`), npm, Cargo, Go, and Python (PyPI) are all
//! implemented as `PackageEcosystem` variants with match arms in the dispatch
//! fns below — no caller changes — the same way `lsp/specs.rs` and
//! `dap/specs.rs` hard-wire one spec per language. There is no plugin system.
//! Several backends need an HTTP fallback for the one step their CLI can't do
//! (crates.io for Cargo's version list, pkg.go.dev for Go search, PyPI's JSON
//! API for Python's version list + exact-name lookup); that goes through
//! `http_get`, which shells out to `curl`. The Python backend is manifest-only:
//! it reads and edits
//! `requirements.txt` and never shells out to `pip`, sidestepping the
//! "which virtualenv?" ambiguity that `pip install` would introduce.
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
    /// Rust — `Cargo.toml` manifests; `cargo` for search/add, crates.io's HTTP
    /// API for the version list (no `cargo` command enumerates all versions).
    Cargo,
    /// Go — `go.mod` manifests; `go` for the version list/add, pkg.go.dev's
    /// search page (scraped) for search (the toolchain has no search command).
    Go,
    /// Python — `requirements.txt` manifests, edited in place; PyPI's JSON API
    /// for the version list and exact-name lookup (the search page is bot-walled,
    /// so "search" resolves an exact package name). No `pip` shell-out — `add`
    /// rewrites the requirement line directly so there's no guesswork about which
    /// interpreter / virtualenv to install into.
    Pip,
}

impl PackageEcosystem {
    pub fn label(self) -> &'static str {
        match self {
            PackageEcosystem::DotNet => "NuGet",
            PackageEcosystem::Npm => "npm",
            PackageEcosystem::Cargo => "Cargo",
            PackageEcosystem::Go => "Go",
            PackageEcosystem::Pip => "PyPI",
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
    // above `start_dir`. .NET is probed before the rest so a mixed repo (an
    // ASP.NET app with a frontend) keeps resolving to NuGet when the active
    // buffer is neither a C# nor a JS/TS file.
    for eco in [
        PackageEcosystem::DotNet,
        PackageEcosystem::Npm,
        PackageEcosystem::Cargo,
        PackageEcosystem::Go,
        PackageEcosystem::Pip,
    ] {
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
    // `Cargo.toml` / `go.mod` carry no telling extension (`toml` is shared, and
    // `go.mod`'s "mod" isn't unique), so match them by basename. The `.rs` /
    // `.go` source extensions are handled in the final `match` below.
    match p.file_name().and_then(|s| s.to_str()) {
        Some("Cargo.toml") => return Some(PackageEcosystem::Cargo),
        Some("go.mod") => return Some(PackageEcosystem::Go),
        Some("requirements.txt") => return Some(PackageEcosystem::Pip),
        _ => {}
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
    match ext {
        "rs" => Some(PackageEcosystem::Cargo),
        "go" => Some(PackageEcosystem::Go),
        "py" | "pyi" => Some(PackageEcosystem::Pip),
        _ => None,
    }
}

/// Walk up from `start_dir` to the workspace root appropriate for `eco`.
pub fn workspace_root(eco: PackageEcosystem, start_dir: &Path) -> PathBuf {
    match eco {
        PackageEcosystem::DotNet => crate::dap::find_dotnet_workspace_root(start_dir),
        PackageEcosystem::Npm => find_root_by_marker(start_dir, "package.json"),
        PackageEcosystem::Cargo => find_root_by_marker(start_dir, "Cargo.toml"),
        PackageEcosystem::Go => find_root_by_marker(start_dir, "go.mod"),
        PackageEcosystem::Pip => find_root_by_marker(start_dir, "requirements.txt"),
    }
}

/// Enumerate the dependency manifests under `workspace_root` for `eco`.
pub fn find_manifests(eco: PackageEcosystem, workspace_root: &Path) -> Vec<Manifest> {
    let paths = match eco {
        // Reuse the DAP layer's project discovery — it already finds
        // `.csproj/.fsproj/.vbproj`, skips bin/obj, and is depth-bounded.
        PackageEcosystem::DotNet => crate::dap::find_dotnet_projects(workspace_root),
        PackageEcosystem::Npm => find_manifests_named(workspace_root, "package.json"),
        PackageEcosystem::Cargo => find_manifests_named(workspace_root, "Cargo.toml"),
        PackageEcosystem::Go => find_manifests_named(workspace_root, "go.mod"),
        PackageEcosystem::Pip => find_manifests_named(workspace_root, "requirements.txt"),
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

/// Walk up from `start` to a workspace root keyed off `marker` (a manifest
/// filename): the closest `.git` directory wins (so a monorepo's sibling
/// packages are all enumerable), else the nearest ancestor holding `marker`,
/// else `start` itself. Used for npm / Cargo / Go — .NET has its own `.sln`-
/// aware walk in `dap`.
fn find_root_by_marker(start: &Path, marker: &str) -> PathBuf {
    let canon = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut dir: &Path = canon.as_path();
    let mut nearest: Option<PathBuf> = None;
    loop {
        if nearest.is_none() && dir.join(marker).is_file() {
            nearest = Some(dir.to_path_buf());
        }
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p,
            _ => break,
        }
    }
    nearest.unwrap_or(canon)
}

/// Recursively enumerate manifests named `filename` under `dir`, skipping the
/// usual build/VCS/dependency dirs. Depth-bounded to stay cheap on large
/// monorepos — mirrors `dap::find_dotnet_projects`.
fn find_manifests_named(dir: &Path, filename: &str) -> Vec<PathBuf> {
    fn ignored(name: &str) -> bool {
        matches!(
            name,
            "node_modules"
                | ".git"
                | "bin"
                | "obj"
                | "target"
                | "vendor"
                | "dist"
                | "build"
                | ".next"
        )
    }
    fn walk(dir: &Path, filename: &str, out: &mut Vec<PathBuf>, depth: usize) {
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
                    walk(&path, filename, out, depth + 1);
                }
            } else if file_type.is_file() && name == filename {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, filename, &mut out, 0);
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
        // Both Cargo + Go read their manifest directly: the dependency table is
        // declarative, so there's no CLI round-trip and the list works offline.
        PackageEcosystem::Cargo => {
            let text = std::fs::read_to_string(manifest)
                .map_err(|e| format!("read {}: {e}", manifest.display()))?;
            parse_cargo_installed_toml(&text)
        }
        PackageEcosystem::Go => {
            let text = std::fs::read_to_string(manifest)
                .map_err(|e| format!("read {}: {e}", manifest.display()))?;
            parse_gomod_installed(&text)
        }
        // requirements.txt is line-based and declarative, so — like Cargo/Go —
        // read it directly. A `pip freeze`-style pin (`pkg==1.2.3`) resolves to
        // that exact version; looser specs leave `resolved` blank.
        PackageEcosystem::Pip => {
            let text = std::fs::read_to_string(manifest)
                .map_err(|e| format!("read {}: {e}", manifest.display()))?;
            Ok(parse_requirements_installed(&text))
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
        // crates.io's HTTP API is the only source for the full version list —
        // `cargo search` returns just the latest, and there's no `cargo
        // versions`. The endpoint already returns newest-first.
        PackageEcosystem::Cargo => {
            let url = format!("https://crates.io/api/v1/crates/{}/versions", urlencode(id));
            let json = http_get(&url)?;
            parse_crates_versions_json(&json)
        }
        // `go list -m -versions` lists every tagged version (oldest→newest) and
        // works for any module, required or not. Run in the manifest dir so the
        // module-mode toolchain + GOPROXY config apply.
        PackageEcosystem::Go => {
            let out = run_capture(
                "go",
                &["list", "-m", "-versions", id],
                manifest_dir(manifest),
                &[],
            )?;
            parse_go_versions(&out)
        }
        // PyPI's per-project JSON endpoint lists every release under `releases`
        // as an unordered map, so unlike crates.io/go we sort it ourselves with
        // a PEP 440 comparator. Releases with no live files (fully yanked) are
        // dropped; prereleases (a/b/rc/dev) are tagged for the picker toggle.
        PackageEcosystem::Pip => {
            let url = format!("https://pypi.org/pypi/{}/json", urlencode(id));
            let json = http_get(&url)?;
            parse_pypi_versions_json(&json)
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
        // `cargo search` returns `name = "ver"  # desc` lines with the latest
        // version inline — no HTTP needed for this step. The 30-row cap matches
        // what the picker can usefully show.
        PackageEcosystem::Cargo => {
            let out = run_capture(
                "cargo",
                &["search", query, "--limit", "30"],
                manifest_dir(manifest),
                &[],
            )?;
            Ok(parse_cargo_search(&out))
        }
        // Go has no search command, so scrape pkg.go.dev's search page for the
        // module paths. Versions aren't shown on the results page (the version
        // picker fetches them next), so each hit's `latest` is left blank.
        PackageEcosystem::Go => {
            let url = format!("https://pkg.go.dev/search?q={}", urlencode(query));
            let html = http_get(&url)?;
            Ok(parse_godev_search_html(&html))
        }
        // PyPI retired its search API and now serves the search *page* behind a
        // bot challenge, so neither an API call nor a scrape is viable. Resolve
        // the query as an exact (PEP 503) package name via the reliable JSON
        // endpoint instead — in an add flow you almost always know the name.
        // An unknown package surfaces as "no matches" rather than an error.
        PackageEcosystem::Pip => {
            let url = format!("https://pypi.org/pypi/{}/json", urlencode(query.trim()));
            match http_get(&url) {
                Ok(json) => Ok(parse_pypi_search_json(&json)),
                Err(e) if e.contains("curl not found") => Err(e),
                Err(_) => Ok(Vec::new()),
            }
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
        // `cargo add <id>@<version>` edits `Cargo.toml` and re-resolves; run in
        // the manifest dir so it targets this package (not a workspace sibling).
        PackageEcosystem::Cargo => {
            let spec = format!("{id}@{version}");
            run_capture("cargo", &["add", &spec], manifest_dir(manifest), &[]).map(|_| ())
        }
        // `go get <module>@<version>` rewrites the `require` line in `go.mod`.
        PackageEcosystem::Go => {
            let spec = format!("{id}@{version}");
            run_capture("go", &["get", &spec], manifest_dir(manifest), &[]).map(|_| ())
        }
        // No `pip` shell-out: rewrite the requirement line in place (or append
        // it) and let the user re-`pip install -r` on their own terms. Keeps
        // the edit deterministic and never touches an interpreter / virtualenv.
        PackageEcosystem::Pip => {
            let text = std::fs::read_to_string(manifest)
                .map_err(|e| format!("read {}: {e}", manifest.display()))?;
            let updated = requirements_set_version(&text, id, version);
            std::fs::write(manifest, updated)
                .map_err(|e| format!("write {}: {e}", manifest.display()))
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

/// Fetch `url` over HTTPS and return the body. We shell out to `curl` rather
/// than link an HTTP client: it keeps the "subprocess + parse" shape of every
/// other backend, adds no compile-time dependency, and these calls already run
/// off the main thread. `-f` turns HTTP errors (404 / 5xx) into a non-zero exit
/// so they surface as `Err`; crates.io requires a non-empty User-Agent.
fn http_get(url: &str) -> Result<String, String> {
    run_capture(
        "curl",
        &[
            "-sSLf",
            "--max-time",
            "20",
            "-A",
            "binvim (https://binvim.dev)",
            url,
        ],
        None,
        &[],
    )
    .map_err(|e| {
        if e.starts_with("failed to run curl") {
            "curl not found — it's required for crates.io / pkg.go.dev lookups".into()
        } else {
            e
        }
    })
}

/// Percent-encode `s` for use in a URL path segment / query value (RFC 3986
/// unreserved set passes through, everything else becomes `%XX`).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
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
                resolved: clean_version(&requested),
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

/// Parse a `Cargo.toml` and return its declared dependencies. Unions
/// `[dependencies]` / `[dev-dependencies]` / `[build-dependencies]` (deduping
/// by name, first wins), sorted by id. A dep value is either a bare version
/// string (`serde = "1"`) or a table (`serde = { version = "1", … }`); a
/// path/git/workspace dep with no version yields an empty `resolved`.
pub fn parse_cargo_installed_toml(text: &str) -> Result<Vec<InstalledPackage>, String> {
    let doc: toml::Value = toml::from_str(text).map_err(|e| format!("parse Cargo.toml: {e}"))?;
    let mut out: Vec<InstalledPackage> = Vec::new();
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(tbl) = doc.get(key).and_then(|d| d.as_table()) else {
            continue;
        };
        for (id, val) in tbl {
            if id.is_empty() || out.iter().any(|e| e.id == *id) {
                continue;
            }
            let requested = match val {
                toml::Value::String(s) => s.clone(),
                toml::Value::Table(t) => t
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                _ => String::new(),
            };
            out.push(InstalledPackage {
                id: id.clone(),
                resolved: clean_version(&requested),
                requested,
            });
        }
    }
    out.sort_by_key(|p| p.id.to_lowercase());
    Ok(out)
}

/// Parse the crates.io `…/versions` response. Skips yanked releases (they're not
/// normally installable) and tags prereleases. The API already returns
/// newest-first, so the order is preserved.
pub fn parse_crates_versions_json(json: &str) -> Result<Vec<PackageVersion>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("parse crates versions: {e}"))?;
    let mut out: Vec<PackageVersion> = Vec::new();
    for ver in v
        .get("versions")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
    {
        if ver.get("yanked").and_then(|y| y.as_bool()).unwrap_or(false) {
            continue;
        }
        let Some(num) = ver.get("num").and_then(|s| s.as_str()) else {
            continue;
        };
        if num.is_empty() {
            continue;
        }
        out.push(PackageVersion {
            version: num.to_string(),
            prerelease: is_prerelease(num),
        });
    }
    Ok(out)
}

/// Parse `cargo search <query> --limit N` output — lines of the form
/// `name = "version"    # description`. Non-matching lines (the trailing
/// `... and N crates more` / `note:` footer) are skipped.
pub fn parse_cargo_search(text: &str) -> Vec<SearchHit> {
    let mut out: Vec<SearchHit> = Vec::new();
    for line in text.lines() {
        let Some((name, rest)) = line.trim().split_once(" = ") else {
            continue;
        };
        let name = name.trim();
        // `rest` opens with the quoted version: `"1.2.3"    # desc`.
        let version = rest
            .trim()
            .strip_prefix('"')
            .and_then(|r| r.split('"').next());
        let Some(version) = version else { continue };
        if name.is_empty() || version.is_empty() {
            continue;
        }
        out.push(SearchHit {
            id: name.to_string(),
            latest: version.to_string(),
        });
    }
    out
}

/// Parse a `go.mod` and return its **direct** requirements. Handles both the
/// single-line `require mod ver` form and the `require ( … )` block; transitive
/// deps (marked `// indirect`) are skipped since they aren't user-managed. Go
/// versions are exact (`v1.2.3`), so `requested` == `resolved`.
pub fn parse_gomod_installed(text: &str) -> Result<Vec<InstalledPackage>, String> {
    fn push(line: &str, out: &mut Vec<InstalledPackage>) {
        if line.contains("// indirect") {
            return;
        }
        let line = line.split("//").next().unwrap_or(line);
        let mut parts = line.split_whitespace();
        let (Some(module), Some(version)) = (parts.next(), parts.next()) else {
            return;
        };
        if out.iter().any(|e| e.id == module) {
            return;
        }
        out.push(InstalledPackage {
            id: module.to_string(),
            requested: version.to_string(),
            resolved: version.to_string(),
        });
    }
    let mut out: Vec<InstalledPackage> = Vec::new();
    let mut in_block = false;
    for raw in text.lines() {
        let line = raw.trim();
        if in_block {
            if line.starts_with(')') {
                in_block = false;
            } else {
                push(line, &mut out);
            }
        } else if let Some(rest) = line.strip_prefix("require ") {
            let rest = rest.trim();
            if rest == "(" {
                in_block = true;
            } else {
                push(rest, &mut out);
            }
        }
    }
    out.sort_by_key(|p| p.id.to_lowercase());
    Ok(out)
}

/// Parse `go list -m -versions <module>` output: `<module> v1 v2 v3 …` on one
/// line, oldest→newest. Drop the leading module token and reverse to
/// newest-first. A module with no tagged versions yields an empty list.
pub fn parse_go_versions(text: &str) -> Result<Vec<PackageVersion>, String> {
    let mut tokens = text.split_whitespace();
    let _module = tokens.next();
    let mut out: Vec<PackageVersion> = tokens
        .map(|v| PackageVersion {
            prerelease: is_prerelease(v),
            version: v.to_string(),
        })
        .collect();
    out.reverse();
    Ok(out)
}

/// Scrape module paths out of a pkg.go.dev search-results page. Each hit is an
/// `<a href="/MODULE_PATH" … data-test-id="snippet-title">`; the version isn't
/// shown on the results page, so `latest` is left blank (the version picker
/// fetches it next). **Fragile by nature** — it depends on pkg.go.dev's markup,
/// so a layout change breaks only this parser (surfacing as "no results").
pub fn parse_godev_search_html(html: &str) -> Vec<SearchHit> {
    // `[^>]*` spans the newline-separated attributes between `href` and the
    // `data-test-id` marker without ever crossing the tag's closing `>`.
    let re = regex::Regex::new(r#"href="/([^"?]+)"[^>]*data-test-id="snippet-title""#).unwrap();
    let mut out: Vec<SearchHit> = Vec::new();
    for cap in re.captures_iter(html) {
        let path = cap[1].to_string();
        if out.iter().any(|h| h.id == path) {
            continue;
        }
        out.push(SearchHit {
            id: path,
            latest: String::new(),
        });
    }
    out
}

/// Strip a leading SemVer range operator (`^`, `~`, `>=`, `v`, …) off a version
/// spec so it can be compared against a concrete published version (shared by
/// npm and Cargo, whose `Cargo.toml` reqs use the same operators). A spec with
/// embedded whitespace (a compound range like `>=1 <2`) or no digit core
/// returns empty — there's no single version to flag.
fn clean_version(spec: &str) -> String {
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

// ─── Python / pip backend ──────────────────────────────────────────────────

/// The structured parts of one `requirements.txt` requirement line, split out
/// so `add` can preserve the original name casing / extras / environment marker
/// and rewrite only the version specifier.
struct ReqParts {
    /// Distribution name as written (original casing preserved for round-trips).
    name: String,
    /// The `[extra1,extra2]` group, brackets included, or empty.
    extras: String,
    /// The version specifier as written (`==1.2.3`, `>=2,<3`, or empty).
    spec: String,
    /// The PEP 508 environment marker after `;`, without the leading `;`.
    marker: Option<String>,
}

/// Trim a trailing `# comment` off a requirements line. A `#` only opens a
/// comment at the start of the line or after whitespace (so a `#` embedded in
/// a token isn't mistaken for one); VCS/URL lines that legitimately carry `#`
/// are filtered earlier by `split_requirement`.
fn strip_requirement_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            return &line[..i];
        }
    }
    line
}

/// PEP 503 name normalisation — lowercase, and collapse any run of `-`, `_`,
/// `.` to a single `-`. `Flask`, `flask`, `FLASK`, `fl_ask` all normalise to the
/// same key, which is how pip itself decides two requirements name the same
/// distribution. Used for dedup + matching an existing line in `add`.
fn normalize_pkg_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_sep = false;
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !prev_sep {
                out.push('-');
                prev_sep = true;
            }
        } else {
            out.push(c.to_ascii_lowercase());
            prev_sep = false;
        }
    }
    out
}

/// Split one raw requirements line into its parts, or `None` for lines we don't
/// manage: blanks, `# comments`, option lines (`-r`, `-e`, `--hash`, …), and
/// VCS/URL requirements (no clean PyPI name to pin a version against).
fn split_requirement(raw: &str) -> Option<ReqParts> {
    let line = strip_requirement_comment(raw).trim();
    if line.is_empty() || line.starts_with('-') || line.contains("://") {
        return None;
    }
    // Peel the environment marker (`; python_version < "3.8"`) off the tail.
    let (head, marker) = match line.split_once(';') {
        Some((h, m)) => (h.trim(), Some(m.trim().to_string())),
        None => (line, None),
    };
    let name_end = head
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
        .unwrap_or(head.len());
    let name = &head[..name_end];
    if name.is_empty() {
        return None;
    }
    let rest = head[name_end..].trim_start();
    // Optional `[extras]` come between the name and the version specifier.
    let (extras, after_extras) = match (rest.starts_with('['), rest.find(']')) {
        (true, Some(end)) => (rest[..=end].to_string(), rest[end + 1..].trim_start()),
        _ => (String::new(), rest),
    };
    Some(ReqParts {
        name: name.to_string(),
        extras,
        spec: after_extras.trim().to_string(),
        marker,
    })
}

/// The single concrete version a specifier pins to, or empty when there isn't
/// one. Only an exact-equality clause (`==1.2.3` / `===1.2.3`) with no second
/// clause resolves; ranges (`>=1,<2`) and floors (`>=1`) leave it blank — the
/// picker only marks a row when a single installed version is known.
fn pinned_version(spec: &str) -> String {
    let s = spec.trim();
    if s.contains(',') {
        return String::new();
    }
    for op in ["===", "=="] {
        if let Some(v) = s.strip_prefix(op) {
            let v = v.trim();
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    String::new()
}

/// Parse a `requirements.txt` into its top-level requirements. Comments, option
/// lines, and VCS/URL entries are skipped; duplicates (by normalised name) keep
/// the first. Sorted by name so the picker order is stable.
pub fn parse_requirements_installed(text: &str) -> Vec<InstalledPackage> {
    let mut out: Vec<InstalledPackage> = Vec::new();
    for raw in text.lines() {
        let Some(req) = split_requirement(raw) else {
            continue;
        };
        let norm = normalize_pkg_name(&req.name);
        if out.iter().any(|e| normalize_pkg_name(&e.id) == norm) {
            continue;
        }
        let resolved = pinned_version(&req.spec);
        out.push(InstalledPackage {
            id: req.name,
            requested: req.spec,
            resolved,
        });
    }
    out.sort_by_key(|p| p.id.to_lowercase());
    out
}

/// Rewrite `text` so `id` is pinned to `version`. If a line already names `id`
/// (matched by PEP 503 normalisation), its specifier is replaced with
/// `==version` while preserving the original name casing, extras, and
/// environment marker; otherwise a `id==version` line is appended. Only the
/// first matching line is rewritten. The file's trailing-newline state is
/// preserved.
pub fn requirements_set_version(text: &str, id: &str, version: &str) -> String {
    let target = normalize_pkg_name(id);
    let had_trailing_newline = text.ends_with('\n');
    let mut found = false;
    let mut lines: Vec<String> = Vec::new();
    for raw in text.lines() {
        if !found {
            if let Some(req) = split_requirement(raw) {
                if normalize_pkg_name(&req.name) == target {
                    let marker = req.marker.map(|m| format!(" ; {m}")).unwrap_or_default();
                    lines.push(format!("{}{}=={}{}", req.name, req.extras, version, marker));
                    found = true;
                    continue;
                }
            }
        }
        lines.push(raw.to_string());
    }
    if !found {
        lines.push(format!("{id}=={version}"));
    }
    let mut joined = lines.join("\n");
    if had_trailing_newline {
        joined.push('\n');
    }
    joined
}

/// A comparable PEP 440 sort key: `(epoch, release, milestone, milestone_num,
/// dev_flag, dev_num)`. The milestone orders dev < pre < final < post within a
/// release; `dev_flag` (0 when a `.dev` segment is present) sinks dev variants
/// below their non-dev sibling at the same milestone. Tuple ordering does the
/// rest.
type Pep440Key = (i64, Vec<i64>, i64, i64, i64, i64);

/// Parsed PEP 440 version. `pre` is `(stage, n)` with stage 0/1/2 = a/b/rc.
struct Pep440 {
    epoch: i64,
    release: Vec<i64>,
    pre: Option<(i64, i64)>,
    post: Option<i64>,
    dev: Option<i64>,
}

/// Parse a PEP 440 version (`1!2.3.4a1.post2.dev3+local`). Returns `None` for
/// strings that don't fit the grammar, so the caller can fall back to a looser
/// heuristic and a sink sort key.
fn pep440_parse(v: &str) -> Option<Pep440> {
    // Single per-call compile is fine at picker volumes; the parsers elsewhere
    // (godev/cargo) build their regex the same way.
    let re = regex::Regex::new(
        r"(?i)^\s*v?(?:(\d+)!)?(\d+(?:\.\d+)*)(?:[-_.]?(a|b|c|rc|alpha|beta|pre|preview)[-_.]?(\d*))?(?:[-_.]?(?:post|rev|r)[-_.]?(\d*))?(?:[-_.]?dev[-_.]?(\d*))?(?:\+[a-z0-9][a-z0-9.\-_]*)?\s*$",
    )
    .ok()?;
    let caps = re.captures(v.trim())?;
    let epoch = caps
        .get(1)
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0);
    let release: Vec<i64> = caps
        .get(2)?
        .as_str()
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();
    if release.is_empty() {
        return None;
    }
    // Each numeric segment defaults to 0 when the keyword carries no digits
    // (`1.0.dev` ≡ `1.0.dev0`); the keyword for post is non-capturing, so the
    // presence of group 5/6 alone marks a post / dev release.
    let num = |m: regex::Match| m.as_str().parse().ok().unwrap_or(0);
    let pre = caps.get(3).map(|m| {
        let stage = match m.as_str().to_ascii_lowercase().as_str() {
            "a" | "alpha" => 0,
            "b" | "beta" => 1,
            _ => 2, // c, rc, pre, preview
        };
        (stage, caps.get(4).map(num).unwrap_or(0))
    });
    let post = caps.get(5).map(num);
    let dev = caps.get(6).map(num);
    Some(Pep440 {
        epoch,
        release,
        pre,
        post,
        dev,
    })
}

/// Build the sort key described on [`Pep440Key`].
fn pep440_sort_key(p: &Pep440) -> Pep440Key {
    let (milestone, num) = if let Some((stage, n)) = p.pre {
        (10 + stage, n) // a=10, b=11, rc=12
    } else if let Some(n) = p.post {
        (30, n) // post-releases sort above the plain release
    } else if p.dev.is_some() {
        (0, 0) // a bare .dev release sits below any pre-release
    } else {
        (20, 0) // final release
    };
    let dev_flag = i64::from(p.dev.is_none()); // dev sinks below non-dev peers
    (
        p.epoch,
        p.release.clone(),
        milestone,
        num,
        dev_flag,
        p.dev.unwrap_or(0),
    )
}

/// Parse PyPI's `…/<pkg>/json` response into a newest-first version list.
/// `releases` is an unordered map of version → file list, so we sort with the
/// PEP 440 comparator. Releases with no installable file (empty list or every
/// file yanked) are dropped; `a`/`b`/`rc`/`.dev` releases are tagged so the
/// picker can hide them behind the prerelease toggle.
pub fn parse_pypi_versions_json(json: &str) -> Result<Vec<PackageVersion>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("parse PyPI versions: {e}"))?;
    let releases = v
        .get("releases")
        .and_then(|r| r.as_object())
        .ok_or("PyPI response has no `releases` map")?;
    let mut items: Vec<(Pep440Key, PackageVersion)> = Vec::new();
    for (ver, files) in releases {
        if ver.is_empty() {
            continue;
        }
        // No installable artefact → not a version the user can pick.
        if let Some(arr) = files.as_array() {
            let all_yanked = arr
                .iter()
                .all(|f| f.get("yanked").and_then(|y| y.as_bool()).unwrap_or(false));
            if arr.is_empty() || all_yanked {
                continue;
            }
        }
        let parsed = pep440_parse(ver);
        let prerelease = parsed
            .as_ref()
            .map(|p| p.pre.is_some() || p.dev.is_some())
            .unwrap_or_else(|| is_prerelease(ver));
        // Unparseable versions sink to the bottom of the newest-first list.
        let key =
            parsed
                .as_ref()
                .map(pep440_sort_key)
                .unwrap_or((i64::MIN, Vec::new(), 0, 0, 0, 0));
        items.push((
            key,
            PackageVersion {
                version: ver.clone(),
                prerelease,
            },
        ));
    }
    items.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(items.into_iter().map(|(_, v)| v).collect())
}

/// Turn a PyPI `…/<pkg>/json` response into a single search hit — the canonical
/// distribution name (`info.name`) and its latest release (`info.version`).
/// Returns empty for a body that isn't a package document, so a 404 (already an
/// `Err` from `http_get`) and a malformed response both read as "no matches".
pub fn parse_pypi_search_json(json: &str) -> Vec<SearchHit> {
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let Some(info) = v.get("info") else {
        return Vec::new();
    };
    let id = info
        .get("name")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    if id.is_empty() {
        return Vec::new();
    }
    let latest = info
        .get("version")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    vec![SearchHit { id, latest }]
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
        // Hermetic root: an empty temp subdir, not the shared `temp_dir()`.
        // The marker-fallback branch of `detect` scans `start_dir` recursively
        // for manifests, so pointing it at the process-wide temp dir races any
        // sibling test that drops a `Cargo.toml` / `go.mod` there — the dap
        // fixtures (`binvim_dap_test_*`) do exactly that, which intermittently
        // made the generic `.json` case resolve to `Some(Cargo)`.
        let tmp = std::env::temp_dir().join("binvim_pkg_detect_ext");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let eco = |p: &str| detect(Some(Path::new(p)), &tmp).map(|(e, _)| e);
        assert_eq!(eco("/x/Foo.cs"), Some(PackageEcosystem::DotNet));
        assert_eq!(eco("/x/Foo.csproj"), Some(PackageEcosystem::DotNet));
        assert_eq!(eco("/x/app.ts"), Some(PackageEcosystem::Npm));
        assert_eq!(eco("/x/app.tsx"), Some(PackageEcosystem::Npm));
        assert_eq!(eco("/x/package.json"), Some(PackageEcosystem::Npm));
        assert_eq!(eco("/x/main.rs"), Some(PackageEcosystem::Cargo));
        assert_eq!(eco("/x/Cargo.toml"), Some(PackageEcosystem::Cargo));
        assert_eq!(eco("/x/main.go"), Some(PackageEcosystem::Go));
        assert_eq!(eco("/x/go.mod"), Some(PackageEcosystem::Go));
        // A generic `.json` is too broad to claim for npm by extension alone.
        assert_eq!(eco("/x/appsettings.json"), None);
        let _ = std::fs::remove_dir_all(&tmp);
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
    fn clean_version_strips_range_operators() {
        assert_eq!(clean_version("^4.17.21"), "4.17.21");
        assert_eq!(clean_version("~1.2.0"), "1.2.0");
        assert_eq!(clean_version(">=3.0.0"), "3.0.0");
        assert_eq!(clean_version("18.2.0"), "18.2.0");
        // No single version to flag for these.
        assert_eq!(clean_version("*"), "");
        assert_eq!(clean_version(">=1 <2"), "");
        assert_eq!(clean_version("workspace:*"), "");
    }

    // ── Cargo ────────────────────────────────────────────────────────────────

    #[test]
    fn parses_cargo_deps_string_and_table_forms() {
        let toml = r#"
            [package]
            name = "demo"

            [dependencies]
            serde = "1.0.219"
            tokio = { version = "1.40", features = ["full"] }

            [dev-dependencies]
            proptest = "1"

            [build-dependencies]
            cc = "1.0"
        "#;
        let pkgs = parse_cargo_installed_toml(toml).unwrap();
        assert_eq!(pkgs.len(), 4);
        assert_eq!(pkgs[0].id, "cc"); // sorted
        let serde = pkgs.iter().find(|p| p.id == "serde").unwrap();
        assert_eq!(serde.requested, "1.0.219");
        assert_eq!(serde.resolved, "1.0.219");
        let tokio = pkgs.iter().find(|p| p.id == "tokio").unwrap();
        assert_eq!(tokio.requested, "1.40"); // version pulled out of the table
    }

    #[test]
    fn cargo_path_dep_has_empty_resolved() {
        let toml = r#"
            [dependencies]
            local = { path = "../local" }
        "#;
        let pkgs = parse_cargo_installed_toml(toml).unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].resolved, "");
    }

    #[test]
    fn parses_crates_versions_skips_yanked_keeps_order() {
        // Shape captured from `https://crates.io/api/v1/crates/<c>/versions`
        // (newest-first).
        let json = r#"{"versions":[
            {"num":"1.0.228","yanked":false},
            {"num":"1.0.227","yanked":true},
            {"num":"1.0.0-rc.1","yanked":false},
            {"num":"0.9.0","yanked":false}
        ]}"#;
        let vs = parse_crates_versions_json(json).unwrap();
        assert_eq!(vs.len(), 3); // yanked dropped
        assert_eq!(vs[0].version, "1.0.228"); // order preserved (newest-first)
        assert!(
            vs.iter()
                .find(|v| v.version == "1.0.0-rc.1")
                .unwrap()
                .prerelease
        );
        assert!(!vs[0].prerelease);
    }

    #[test]
    fn parses_cargo_search_lines() {
        let text = "\
serde = \"1.0.228\"          # A generic serialization/deserialization framework
serde_json = \"1.0.140\"     # A JSON serialization file format
... and 16345 crates more (use --limit N to see more)
note: to learn more about a package, run `cargo info <name>`";
        let hits = parse_cargo_search(text);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "serde");
        assert_eq!(hits[0].latest, "1.0.228");
        assert_eq!(hits[1].id, "serde_json");
    }

    // ── Go ───────────────────────────────────────────────────────────────────

    #[test]
    fn parses_gomod_direct_requires_skipping_indirect() {
        let gomod = "\
module example.com/x

go 1.22

require (
\tgithub.com/foo/bar v1.2.3
\tgolang.org/x/text v0.14.0 // indirect
)

require github.com/baz/qux v2.0.0
";
        let pkgs = parse_gomod_installed(gomod).unwrap();
        assert_eq!(pkgs.len(), 2); // indirect dep excluded
        assert_eq!(pkgs[0].id, "github.com/baz/qux");
        assert_eq!(pkgs[0].resolved, "v2.0.0");
        assert_eq!(pkgs[1].id, "github.com/foo/bar");
        assert!(!pkgs.iter().any(|p| p.id == "golang.org/x/text"));
    }

    #[test]
    fn parses_go_versions_newest_first() {
        // `go list -m -versions M` → module then space-separated versions.
        let out = "golang.org/x/text v0.1.0 v0.2.0 v0.14.0\n";
        let vs = parse_go_versions(out).unwrap();
        assert_eq!(vs.len(), 3);
        assert_eq!(vs[0].version, "v0.14.0"); // newest first
        assert_eq!(vs.last().unwrap().version, "v0.1.0");
    }

    #[test]
    fn go_module_with_no_tagged_versions_is_empty() {
        let vs = parse_go_versions("example.com/x\n").unwrap();
        assert!(vs.is_empty());
    }

    #[test]
    fn scrapes_godev_search_module_paths() {
        // Markup captured from a real pkg.go.dev search page — attributes span
        // newlines between `href` and the `data-test-id` marker.
        let html = r#"
          <h2>
            <a href="/gopkg.in/yaml.v3" data-gtmc="search result" data-gtmv="0"
                data-test-id="snippet-title">
              yaml <span class="SearchSnippet-header-path">(gopkg.in/yaml.v3)</span>
            </a>
          </h2>
          <a href="/sigs.k8s.io/yaml" data-test-id="snippet-title">k8s yaml</a>
          <a href="/some/other/link">not a result</a>
        "#;
        let hits = parse_godev_search_html(html);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "gopkg.in/yaml.v3");
        assert_eq!(hits[1].id, "sigs.k8s.io/yaml");
        assert!(hits[0].latest.is_empty()); // version not on the results page
    }

    #[test]
    fn urlencode_escapes_reserved() {
        assert_eq!(urlencode("github.com/foo/bar"), "github.com%2Ffoo%2Fbar");
        assert_eq!(urlencode("hello world"), "hello%20world");
        assert_eq!(urlencode("serde_json-1.0"), "serde_json-1.0");
    }

    #[test]
    fn parses_requirements_installed_with_comments_extras_markers() {
        let txt = "# top comment\n\
                   requests==2.31.0\n\
                   Flask>=3.0  # inline comment\n\
                   httpx\n\
                   django[argon2]==5.0 ; python_version >= \"3.10\"\n\
                   -r dev-requirements.txt\n\
                   -e .\n\
                   git+https://github.com/x/y.git#egg=z\n\
                   requests==1.0\n";
        let pkgs = parse_requirements_installed(txt);
        // requests deduped (first wins); options + VCS lines skipped.
        assert_eq!(pkgs.len(), 4);
        // sorted case-insensitively by name.
        assert_eq!(
            pkgs.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["django", "Flask", "httpx", "requests"]
        );
        let req = pkgs.iter().find(|p| p.id == "requests").unwrap();
        assert_eq!(req.requested, "==2.31.0");
        assert_eq!(req.resolved, "2.31.0"); // exact pin resolves
        let flask = pkgs.iter().find(|p| p.id == "Flask").unwrap();
        assert_eq!(flask.requested, ">=3.0");
        assert_eq!(flask.resolved, ""); // a floor isn't a single version
        let httpx = pkgs.iter().find(|p| p.id == "httpx").unwrap();
        assert_eq!(httpx.requested, "");
        let django = pkgs.iter().find(|p| p.id == "django").unwrap();
        assert_eq!(django.resolved, "5.0"); // extras + marker stripped off the spec
    }

    #[test]
    fn requirements_set_version_updates_existing_preserving_extras_and_marker() {
        let txt = "requests==2.0\n\
                   django[argon2]==4.0 ; python_version >= \"3.8\"\n";
        let out = requirements_set_version(txt, "django", "5.0");
        assert_eq!(
            out,
            "requests==2.0\n\
             django[argon2]==5.0 ; python_version >= \"3.8\"\n"
        );
    }

    #[test]
    fn requirements_set_version_matches_by_normalized_name() {
        // `Flask-Login` and `flask_login` are the same distribution per PEP 503.
        let out = requirements_set_version("Flask-Login==0.6\n", "flask_login", "0.7");
        assert_eq!(out, "Flask-Login==0.7\n"); // original casing preserved
    }

    #[test]
    fn requirements_set_version_appends_when_absent() {
        assert_eq!(
            requirements_set_version("requests==2.0\n", "flask", "3.0"),
            "requests==2.0\nflask==3.0\n"
        );
        // No trailing newline in → none added, new line still separated.
        assert_eq!(
            requirements_set_version("requests==2.0", "flask", "3.0"),
            "requests==2.0\nflask==3.0"
        );
        // Empty file → just the new pin.
        assert_eq!(requirements_set_version("", "flask", "3.0"), "flask==3.0");
    }

    #[test]
    fn parses_pypi_versions_newest_first_skipping_yanked_and_empty() {
        let json = r#"{"releases":{
            "1.0.0":[{"yanked":false}],
            "2.0.0":[{"yanked":false}],
            "2.0.0rc1":[{"yanked":false}],
            "1.5.0":[{"yanked":true}],
            "1.9.0":[{"yanked":false}],
            "1.10.0":[{"yanked":false}],
            "0.0.0":[]
        }}"#;
        let vs = parse_pypi_versions_json(json).unwrap();
        // 1.5.0 (all yanked) and 0.0.0 (no files) dropped.
        assert_eq!(
            vs.iter().map(|v| v.version.as_str()).collect::<Vec<_>>(),
            ["2.0.0", "2.0.0rc1", "1.10.0", "1.9.0", "1.0.0"]
        );
        // numeric ordering, not lexical: 1.10.0 sorts above 1.9.0.
        let rc = vs.iter().find(|v| v.version == "2.0.0rc1").unwrap();
        assert!(rc.prerelease);
        let final_ = vs.iter().find(|v| v.version == "2.0.0").unwrap();
        assert!(!final_.prerelease);
    }

    #[test]
    fn pep440_orders_dev_pre_final_post_within_a_release() {
        let json = r#"{"releases":{
            "1.0.dev1":[{"yanked":false}],
            "1.0a1":[{"yanked":false}],
            "1.0a1.dev1":[{"yanked":false}],
            "1.0":[{"yanked":false}],
            "1.0.post1":[{"yanked":false}]
        }}"#;
        let vs = parse_pypi_versions_json(json).unwrap();
        assert_eq!(
            vs.iter().map(|v| v.version.as_str()).collect::<Vec<_>>(),
            ["1.0.post1", "1.0", "1.0a1", "1.0a1.dev1", "1.0.dev1"]
        );
        // post-release is not a prerelease; the dev/pre variants are.
        assert!(
            !vs.iter()
                .find(|v| v.version == "1.0.post1")
                .unwrap()
                .prerelease
        );
        assert!(
            vs.iter()
                .find(|v| v.version == "1.0.dev1")
                .unwrap()
                .prerelease
        );
    }

    #[test]
    fn pypi_search_resolves_exact_name_from_json() {
        // Shape from `https://pypi.org/pypi/<pkg>/json` — `info` carries the
        // canonical name + latest version.
        let json = r#"{"info":{"name":"Flask","version":"3.1.3"},"releases":{}}"#;
        let hits = parse_pypi_search_json(json);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "Flask"); // canonical casing
        assert_eq!(hits[0].latest, "3.1.3");
    }

    #[test]
    fn pypi_search_empty_for_non_package_body() {
        assert!(parse_pypi_search_json("not json").is_empty());
        assert!(parse_pypi_search_json(r#"{"message":"Not Found"}"#).is_empty());
    }

    #[test]
    fn detects_pip_from_buffer_extension_and_basename() {
        use std::path::Path;
        assert_eq!(
            eco_from_extension(Some(Path::new("/proj/app.py"))),
            Some(PackageEcosystem::Pip)
        );
        assert_eq!(
            eco_from_extension(Some(Path::new("/proj/requirements.txt"))),
            Some(PackageEcosystem::Pip)
        );
    }
}
