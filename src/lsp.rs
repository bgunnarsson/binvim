//! Minimal LSP client. First cut handles diagnostics only:
//! spawn server, send initialize/initialized/didOpen/didChange, push diagnostics
//! notifications to a channel that the main loop drains.

use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Diagnostic {
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Debug, Clone)]
pub struct DiagnosticsMessage {
    pub uri: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub enum LspIncoming {
    Diagnostics(DiagnosticsMessage),
    Response { id: u64, result: Value },
    /// Request that the editor needs to react to (e.g. goto-def jump, hover popup).
    #[allow(dead_code)]
    ErrorReply { id: u64, message: String },
    /// Server-to-client `workspace/applyEdit` — the main thread applies the
    /// edit and replies with `{ applied: true }` (or false on failure).
    ApplyEditRequest { id: u64, edit: Value },
}

#[derive(Debug, Clone)]
pub enum LspEvent {
    GotoDef { path: PathBuf, line: usize, col: usize },
    Hover { text: String },
    Completion { items: Vec<CompletionItem> },
    SignatureHelp(SignatureHelp),
    References { items: Vec<LocationItem> },
    Symbols { items: Vec<SymbolItem>, workspace: bool },
    CodeActions { items: Vec<CodeActionItem> },
    /// `WorkspaceEdit` returned from `textDocument/rename`. The applier in
    /// app.rs consumes this directly via `apply_workspace_edit`.
    Rename { edit: Value },
    /// Server asked us to apply a `WorkspaceEdit`. App applies it then
    /// uses `LspManager::send_apply_edit_response` to ack the originating
    /// request.
    ApplyEditRequest {
        client_key: String,
        id: u64,
        edit: Value,
    },
    DiagnosticsUpdated,
    NotFound(&'static str),
}

/// A code action the user can pick from `<leader>a`. We keep the raw
/// `command` and `edit` JSON values so the applier can match against
/// either shape (LSP returns `Command` or `CodeAction` interchangeably).
#[derive(Debug, Clone)]
pub struct CodeActionItem {
    pub title: String,
    pub kind: Option<String>,
    pub edit: Option<Value>,
    pub command: Option<Value>,
    /// Set when the action is published as `disabled` — we still surface it
    /// so the user can see why the server thinks it doesn't apply, but the
    /// apply path will reject it.
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SymbolItem {
    pub name: String,
    /// Container path for nested symbols, e.g. `App > render > draw`. Empty
    /// for top-level symbols.
    pub container: String,
    pub kind: String,
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
}

/// One result from a `textDocument/references` (or similar) call. `path`
/// is on disk, line/col are 0-indexed.
#[derive(Debug, Clone)]
pub struct LocationItem {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
}

/// Parsed `SignatureHelp` response. We render the active signature only —
/// most servers return one anyway, and overload menus are rarely useful in
/// a TUI.
#[derive(Debug, Clone)]
pub struct SignatureHelp {
    pub label: String,
    /// Char range in `label` covering the active parameter, if known.
    pub active_param: Option<(usize, usize)>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CompletionItem {
    pub label: String,
    pub insert_text: String,
    pub kind: Option<String>,
    pub detail: Option<String>,
    /// Server-supplied filter key. Falls back to `label` when absent.
    pub filter_text: String,
    /// Server-supplied sort key. Falls back to `label` when absent. Lets the
    /// LSP's relevance order survive client-side filtering (e.g. typescript's
    /// "0~document" sorts globals before locals when relevant).
    pub sort_text: String,
}

#[derive(Debug, Clone, Copy)]
pub enum PendingRequest {
    GotoDef,
    Hover,
    Completion,
    SignatureHelp,
    References,
    DocumentSymbols,
    WorkspaceSymbols,
    CodeActions,
    Rename,
}

/// State of a client's outgoing pipe. Until the server has answered the
/// `initialize` request we buffer frames; the reader thread flushes them in
/// order once it sees the response.
enum InitState {
    Buffering(Vec<Vec<u8>>),
    Ready,
}

pub struct LspClient {
    #[allow(dead_code)]
    pub name: String,
    _child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pub incoming_rx: Receiver<LspIncoming>,
    next_id: Arc<Mutex<u64>>,
    init_state: Arc<Mutex<InitState>>,
    #[allow(dead_code)]
    pub root_uri: String,
    pub language_id: String,
}

#[derive(Debug, Clone)]
pub struct ServerSpec {
    /// Stable key — one client per (key) per workspace root.
    pub key: String,
    /// LSP languageId sent on textDocument/didOpen.
    pub language_id: String,
    /// Candidate command paths in priority order. First one that resolves wins.
    pub cmd_candidates: Vec<String>,
    pub args: Vec<String>,
    /// Filenames whose presence marks a project root, in priority order.
    pub root_markers: Vec<String>,
    /// initializationOptions field on the initialize request.
    pub initialization_options: Value,
}

/// All LSP server specs that should attach to `path`. The first entry is the
/// "primary" server (used for hover and goto-def); any extra entries are
/// auxiliary — they receive didOpen/didChange and contribute to completions
/// but don't take over hover or definition.
///
/// The Tailwind LSP is added on top of the primary server when a
/// `tailwind.config.*` file is reachable from the buffer's directory and the
/// file's extension is one Tailwind cares about (CSS family + every web
/// framework Tailwind supports out of the box).
///
/// For Razor (.cshtml/.razor) files, csharp-ls is layered on as an
/// auxiliary so the user gets at least C# identifier completions inside
/// `@{}` and `@code{}` blocks even when no Razor-specialised server
/// (rzls/OmniSharp) is installed. The primary remains the html LSP in
/// that case so markup completions still work.
pub fn specs_for_path(path: &Path) -> Vec<ServerSpec> {
    let mut specs = Vec::new();
    if let Some(primary) = primary_spec_for_path(path) {
        specs.push(primary);
    }
    if let Some(tw) = tailwind_spec_for_path(path) {
        specs.push(tw);
    }
    if let Some(cs) = csharp_aux_spec_for_path(path) {
        // Avoid duplicate-key collision with a primary that's already csharp-ls.
        if specs.iter().all(|s| s.key != cs.key) {
            specs.push(cs);
        }
    }
    specs
}

/// Pick the primary LSP server config for a path's extension. `None` if we
/// don't know the extension.
///
/// Command candidates are bare names — `resolve_command` then walks `$PATH` to find them.
/// We only special-case `~/.cargo/bin` for rust-analyzer because that's the Rust toolchain
/// convention (and not tied to any other tool's package manager).
fn primary_spec_for_path(path: &Path) -> Option<ServerSpec> {
    let ext = path.extension().and_then(|s| s.to_str())?;
    let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/"));
    let cargo_bin = |bin: &str| format!("{}/.cargo/bin/{}", home, bin);
    let go_bin = |bin: &str| format!("{}/go/bin/{}", home, bin);
    let local_bin = |sub: &str, bin: &str| format!("{}/.local/bin/{}/{}", home, sub, bin);
    let stdio = || vec!["--stdio".to_string()];

    let ts_markers = || {
        vec![
            "package-lock.json".into(),
            "yarn.lock".into(),
            "pnpm-lock.yaml".into(),
            "bun.lockb".into(),
            "bun.lock".into(),
            "tsconfig.json".into(),
            "jsconfig.json".into(),
            "package.json".into(),
            ".git".into(),
        ]
    };
    let ts_init = || json!({ "hostInfo": "binvim", "preferences": {} });

    match ext {
        "rs" => Some(ServerSpec {
            key: "rust".into(),
            language_id: "rust".into(),
            cmd_candidates: vec!["rust-analyzer".into(), cargo_bin("rust-analyzer")],
            args: vec![],
            root_markers: vec!["Cargo.toml".into(), "rust-project.json".into(), ".git".into()],
            initialization_options: Value::Null,
        }),
        "ts" => Some(ServerSpec {
            key: "ts".into(),
            language_id: "typescript".into(),
            cmd_candidates: vec!["typescript-language-server".into()],
            args: stdio(),
            root_markers: ts_markers(),
            initialization_options: ts_init(),
        }),
        "tsx" => Some(ServerSpec {
            key: "ts".into(),
            language_id: "typescriptreact".into(),
            cmd_candidates: vec!["typescript-language-server".into()],
            args: stdio(),
            root_markers: ts_markers(),
            initialization_options: ts_init(),
        }),
        "jsx" => Some(ServerSpec {
            key: "ts".into(),
            language_id: "javascriptreact".into(),
            cmd_candidates: vec!["typescript-language-server".into()],
            args: stdio(),
            root_markers: ts_markers(),
            initialization_options: ts_init(),
        }),
        "js" | "mjs" | "cjs" => Some(ServerSpec {
            key: "ts".into(),
            language_id: "javascript".into(),
            cmd_candidates: vec!["typescript-language-server".into()],
            args: stdio(),
            root_markers: ts_markers(),
            initialization_options: ts_init(),
        }),
        "json" | "jsonc" => {
            // Biome doesn't support global installs — it lives in node_modules.
            // Walk up from the file until we find a node_modules/.bin/biome; if
            // we don't find one, no JSON LSP attaches.
            let start = path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let biome = find_node_modules_bin(&start, "biome")?;
            Some(ServerSpec {
                key: "biome".into(),
                language_id: "json".into(),
                cmd_candidates: vec![biome],
                args: vec!["lsp-proxy".into()],
                root_markers: vec!["biome.json".into(), "biome.jsonc".into(), "package.json".into(), ".git".into()],
                initialization_options: Value::Null,
            })
        }
        "go" => Some(ServerSpec {
            key: "go".into(),
            language_id: "go".into(),
            cmd_candidates: vec!["gopls".into(), go_bin("gopls")],
            args: vec![],
            root_markers: vec!["go.mod".into(), "go.work".into(), ".git".into()],
            initialization_options: Value::Null,
        }),
        "html" | "htm" => Some(ServerSpec {
            key: "html".into(),
            language_id: "html".into(),
            cmd_candidates: vec!["vscode-html-language-server".into()],
            args: stdio(),
            root_markers: vec!["package.json".into(), "*.csproj".into(), ".git".into()],
            initialization_options: Value::Null,
        }),
        "cshtml" | "razor" => {
            // Razor IntelliSense is best with rzls (Razor Language Server) but
            // it isn't packaged in mason or as a NuGet/dotnet tool today. Try
            // it anyway in case the user installed it manually — and otherwise
            // fall back to OmniSharp, which handles .cshtml as a C# document
            // and gives real IntelliSense for the embedded code blocks (@{},
            // @Model.X, etc.). Better than html-LSP-only.
            let rzls = ServerSpec {
                key: "rzls".into(),
                language_id: "razor".into(),
                cmd_candidates: vec!["rzls".into(), local_bin("rzls", "rzls")],
                args: vec![],
                root_markers: vec!["*.csproj".into(), "*.sln".into(), ".git".into()],
                initialization_options: Value::Null,
            };
            if resolve_command(&rzls.cmd_candidates).is_some() {
                return Some(rzls);
            }
            let omnisharp = ServerSpec {
                key: "omnisharp".into(),
                language_id: "razor".into(),
                cmd_candidates: vec![
                    "OmniSharp".into(),
                    "omnisharp".into(),
                    local_bin("omnisharp", "OmniSharp"),
                ],
                args: vec![
                    "-z".into(),
                    "--hostPID".into(),
                    std::process::id().to_string(),
                    "DotNet:enablePackageRestore=false".into(),
                    "--encoding".into(),
                    "utf-8".into(),
                    "--languageserver".into(),
                ],
                root_markers: vec![
                    "*.sln".into(),
                    "*.csproj".into(),
                    "*.fsproj".into(),
                    "*.vbproj".into(),
                    ".git".into(),
                ],
                initialization_options: Value::Null,
            };
            if resolve_command(&omnisharp.cmd_candidates).is_some() {
                return Some(omnisharp);
            }
            // Last resort — at least give markup IntelliSense.
            Some(ServerSpec {
                key: "html".into(),
                language_id: "html".into(),
                cmd_candidates: vec!["vscode-html-language-server".into()],
                args: stdio(),
                root_markers: vec!["package.json".into(), "*.csproj".into(), ".git".into()],
                initialization_options: Value::Null,
            })
        }
        "css" | "scss" | "less" => Some(ServerSpec {
            key: "css".into(),
            language_id: ext.into(),
            cmd_candidates: vec!["vscode-css-language-server".into()],
            args: stdio(),
            root_markers: vec!["package.json".into(), ".git".into()],
            initialization_options: Value::Null,
        }),
        "astro" => Some(ServerSpec {
            key: "astro".into(),
            language_id: "astro".into(),
            cmd_candidates: vec!["astro-ls".into()],
            args: stdio(),
            root_markers: vec!["astro.config.mjs".into(), "astro.config.ts".into(), "package.json".into(), ".git".into()],
            initialization_options: Value::Null,
        }),
        "cs" | "vb" => {
            // Roslyn-based `csharp-ls` is preferred — it returns local
            // variables and parameters in completion immediately, where
            // OmniSharp falls back to bare top-level type matches until its
            // workspace finishes loading (often 30-60s on real solutions).
            // OmniSharp stays as a fallback for environments without
            // csharp-ls installed.
            let dotnet_tools = format!("{}/.dotnet/tools/csharp-ls", home);
            let csharp_ls = ServerSpec {
                key: "csharp-ls".into(),
                language_id: if ext == "cs" { "csharp".into() } else { "vb".into() },
                cmd_candidates: vec!["csharp-ls".into(), dotnet_tools],
                args: vec![],
                root_markers: vec![
                    "*.sln".into(),
                    "*.csproj".into(),
                    "*.fsproj".into(),
                    "*.vbproj".into(),
                    ".git".into(),
                ],
                initialization_options: Value::Null,
            };
            if resolve_command(&csharp_ls.cmd_candidates).is_some() {
                return Some(csharp_ls);
            }
            Some(ServerSpec {
                key: "omnisharp".into(),
                language_id: if ext == "cs" { "csharp".into() } else { "vb".into() },
                cmd_candidates: vec![
                    "OmniSharp".into(),
                    "omnisharp".into(),
                    local_bin("omnisharp", "OmniSharp"),
                ],
                args: vec![
                    "-z".into(),
                    "--hostPID".into(),
                    std::process::id().to_string(),
                    "DotNet:enablePackageRestore=false".into(),
                    "--encoding".into(),
                    "utf-8".into(),
                    "--languageserver".into(),
                ],
                root_markers: vec![
                    "*.sln".into(),
                    "*.csproj".into(),
                    "*.fsproj".into(),
                    "*.vbproj".into(),
                    ".git".into(),
                ],
                initialization_options: Value::Null,
            })
        }
        _ => None,
    }
}

/// csharp-ls layered on top of the html LSP for Razor files so users get
/// some C# completion in `@{}` / `@code{}` blocks until they install a
/// dedicated Razor server (rzls / OmniSharp). Skipped for plain .cs since
/// csharp-ls is already the primary there — `specs_for_path` deduplicates
/// by key in that case.
fn csharp_aux_spec_for_path(path: &Path) -> Option<ServerSpec> {
    let ext = path.extension().and_then(|s| s.to_str())?.to_ascii_lowercase();
    if ext != "cshtml" && ext != "razor" {
        return None;
    }
    let home = std::env::var("HOME").ok()?;
    let dotnet_tools = format!("{}/.dotnet/tools/csharp-ls", home);
    let candidates = vec!["csharp-ls".into(), dotnet_tools];
    if resolve_command(&candidates).is_none() {
        return None;
    }
    Some(ServerSpec {
        key: "csharp-ls".into(),
        language_id: "csharp".into(),
        cmd_candidates: candidates,
        args: vec![],
        root_markers: vec![
            "*.sln".into(),
            "*.csproj".into(),
            ".git".into(),
        ],
        initialization_options: Value::Null,
    })
}

/// Tailwind augments completions and diagnostics on top of the primary
/// server. We only attach it when a `tailwind.config.*` file exists in the
/// workspace tree — otherwise the server starts and offers nothing useful.
///
/// languageId mirrors what tailwindcss-language-server expects (it has a
/// short whitelist; using the right id is what unlocks classname completion).
fn tailwind_spec_for_path(path: &Path) -> Option<ServerSpec> {
    let ext = path.extension().and_then(|s| s.to_str())?.to_ascii_lowercase();
    let language_id = match ext.as_str() {
        "html" | "htm" => "html",
        "css" => "css",
        "scss" => "scss",
        "less" => "less",
        "postcss" | "pcss" => "postcss",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "vue" => "vue",
        "svelte" => "svelte",
        "astro" => "astro",
        "razor" | "cshtml" => "razor",
        _ => return None,
    };
    let start = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let config = find_tailwind_config(&start)?;
    let workspace = config
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or(config.clone());
    let local_bin = find_node_modules_bin(&start, "tailwindcss-language-server");
    let mut cmd_candidates = Vec::new();
    if let Some(p) = local_bin {
        cmd_candidates.push(p);
    }
    cmd_candidates.push("tailwindcss-language-server".into());

    Some(ServerSpec {
        key: "tailwindcss".into(),
        language_id: language_id.into(),
        cmd_candidates,
        args: vec!["--stdio".into()],
        // Anchor the server at the directory containing the tailwind config —
        // that's how tailwindcss-language-server discovers the config and the
        // project's class catalogue.
        root_markers: vec![
            workspace
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| ".".into()),
        ],
        initialization_options: json!({
            "userLanguages": {},
            "configuration": {
                "editor": { "tabSize": 2 },
                "tailwindCSS": {
                    "validate": true,
                    "emmetCompletions": false,
                    "classAttributes": ["class", "className", "ngClass", "class:list"],
                    "includeLanguages": {}
                }
            }
        }),
    })
}

/// Walk up from `start` looking for a marker that says "this project uses
/// Tailwind." Returns the path of the marker so `:health` can show the user
/// what we matched on.
///
/// v3 markers: `tailwind.config.{js,ts,cjs,mjs,cts,mts}`.
/// v4 marker:  `package.json` declaring `tailwindcss` in (dev)dependencies —
///             v4's CSS-first config (`@import "tailwindcss"` in a CSS file)
///             leaves no JS config to walk for, so we fall back to the
///             dependency declaration to know Tailwind is present.
pub fn find_tailwind_config(start: &Path) -> Option<PathBuf> {
    let canon = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut dir: &Path = canon.as_path();
    let cfg_names = [
        "tailwind.config.js",
        "tailwind.config.ts",
        "tailwind.config.cjs",
        "tailwind.config.mjs",
        "tailwind.config.cts",
        "tailwind.config.mts",
    ];
    loop {
        for name in &cfg_names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        let pkg = dir.join("package.json");
        if pkg.is_file() && package_has_tailwind(&pkg) {
            return Some(pkg);
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p,
            _ => return None,
        }
    }
}

/// True if `package.json` lists `tailwindcss` (or `@tailwindcss/*`) under
/// `dependencies`, `devDependencies`, or `peerDependencies`. Cheap text
/// scan — robust enough that we don't pull a JSON parser into the hot path.
fn package_has_tailwind(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else { return false; };
    let needles = [
        "\"tailwindcss\"",
        "\"@tailwindcss/postcss\"",
        "\"@tailwindcss/vite\"",
        "\"@tailwindcss/cli\"",
    ];
    needles.iter().any(|n| text.contains(n))
}

/// Walk up from `start` looking for any of the marker filenames. Markers
/// starting with `*.` match any directory entry with that extension (used for
/// `.sln` / `.csproj` etc. where the actual filename varies). Falls back to
/// `start` if no marker matches.
pub fn find_workspace_root(start: &Path, markers: &[String]) -> PathBuf {
    let canon = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut dir: &Path = canon.as_path();
    loop {
        for marker in markers {
            if let Some(ext) = marker.strip_prefix("*.") {
                if dir_contains_extension(dir, ext) {
                    return dir.to_path_buf();
                }
            } else if dir.join(marker).exists() {
                return dir.to_path_buf();
            }
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p,
            _ => break,
        }
    }
    canon
}

/// Walk up from `start` looking for `node_modules/.bin/<name>`. Returns the
/// first match (the closest one to the file). Used for tools like biome that
/// don't support global installs.
pub fn find_node_modules_bin(start: &Path, name: &str) -> Option<String> {
    let canon = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut dir: &Path = canon.as_path();
    loop {
        let candidate = dir.join("node_modules").join(".bin").join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p,
            _ => return None,
        }
    }
}

fn dir_contains_extension(dir: &Path, ext: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(file_ext) = path.extension().and_then(|e| e.to_str()) {
            if file_ext.eq_ignore_ascii_case(ext) {
                return true;
            }
        }
    }
    false
}

fn resolve_command(candidates: &[String]) -> Option<(String, Vec<String>)> {
    for c in candidates {
        let path = if c.starts_with("~/") {
            let home = std::env::var("HOME").ok()?;
            format!("{}/{}", home, &c[2..])
        } else {
            c.clone()
        };
        if path.contains('/') {
            if std::path::Path::new(&path).is_file() {
                return Some((path, vec![]));
            }
            continue;
        }
        if let Some(found) = which_in_path(&path) {
            return Some((found, vec![]));
        }
    }
    None
}

fn which_in_path(name: &str) -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(':') {
        let full = std::path::Path::new(dir).join(name);
        if full.is_file() {
            return Some(full.to_string_lossy().to_string());
        }
    }
    None
}

impl LspClient {
    /// Spawn an LSP server given a [`ServerSpec`] and a workspace root.
    /// Returns `None` if no candidate command resolves or spawning fails.
    pub fn spawn_spec(spec: &ServerSpec, root: &Path) -> Option<Self> {
        let (cmd_path, _) = resolve_command(&spec.cmd_candidates)?;
        let mut command = Command::new(&cmd_path);
        for arg in &spec.args {
            command.arg(arg);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdin = Arc::new(Mutex::new(child.stdin.take()?));
        let stdout = child.stdout.take()?;

        let (in_tx, in_rx) = channel();
        let init_state = Arc::new(Mutex::new(InitState::Buffering(Vec::new())));
        let init_state_for_reader = init_state.clone();
        let stdin_for_reader = stdin.clone();
        thread::spawn(move || {
            reader_loop(stdout, stdin_for_reader, init_state_for_reader, in_tx);
        });

        let root_uri = path_to_uri(root);
        let client = Self {
            name: spec.key.clone(),
            _child: child,
            stdin,
            incoming_rx: in_rx,
            next_id: Arc::new(Mutex::new(1)),
            init_state,
            root_uri: root_uri.clone(),
            language_id: spec.language_id.clone(),
        };

        // Send initialize directly (bypassing the queue gate, which only holds
        // back later messages). Initialized + queued frames are flushed by the
        // reader thread once the response arrives — we don't block here.
        let init_id = client.alloc_id();
        let _ = client.send_request_direct(
            init_id,
            "initialize",
            json!({
                "processId": std::process::id(),
                "clientInfo": { "name": "binvim", "version": env!("CARGO_PKG_VERSION") },
                "rootUri": root_uri,
                "rootPath": root.to_string_lossy(),
                "workspaceFolders": [{ "uri": root_uri, "name": root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "root".into()) }],
                "initializationOptions": spec.initialization_options,
                "capabilities": {
                    "general": {
                        "positionEncodings": ["utf-8", "utf-16"]
                    },
                    "textDocument": {
                        "synchronization": {
                            "dynamicRegistration": false,
                            "didSave": true
                        },
                        "publishDiagnostics": {
                            "relatedInformation": false,
                            "versionSupport": false,
                            "tagSupport": { "valueSet": [1, 2] }
                        },
                        "hover": {
                            "dynamicRegistration": false,
                            "contentFormat": ["markdown", "plaintext"]
                        },
                        "definition": {
                            "dynamicRegistration": false,
                            "linkSupport": true
                        },
                        "references": { "dynamicRegistration": false },
                        "documentSymbol": { "dynamicRegistration": false },
                        "rename": {
                            "dynamicRegistration": false,
                            "prepareSupport": false
                        },
                        "completion": {
                            "dynamicRegistration": false,
                            "completionItem": {
                                "snippetSupport": false,
                                "documentationFormat": ["markdown", "plaintext"],
                                "deprecatedSupport": true,
                                "preselectSupport": false,
                                "insertReplaceSupport": false,
                                "resolveSupport": { "properties": ["documentation", "detail"] }
                            },
                            "completionItemKind": {
                                "valueSet": (1..=25).collect::<Vec<_>>()
                            },
                            "contextSupport": true
                        },
                        "signatureHelp": {
                            "dynamicRegistration": false,
                            "signatureInformation": {
                                "documentationFormat": ["markdown", "plaintext"],
                                "parameterInformation": { "labelOffsetSupport": true },
                                "activeParameterSupport": true
                            },
                            "contextSupport": true
                        },
                        "codeAction": {
                            "dynamicRegistration": false,
                            "codeActionLiteralSupport": {
                                "codeActionKind": {
                                    "valueSet": [
                                        "", "quickfix", "refactor",
                                        "refactor.extract", "refactor.inline", "refactor.rewrite",
                                        "source", "source.organizeImports"
                                    ]
                                }
                            }
                        },
                        "formatting": { "dynamicRegistration": false }
                    },
                    "workspace": {
                        "applyEdit": true,
                        "workspaceEdit": { "documentChanges": false },
                        "configuration": true,
                        "didChangeConfiguration": { "dynamicRegistration": false },
                        "workspaceFolders": true
                    }
                }
            }),
        );
        // No blocking wait — reader thread handles "initialized" + queue flush when
        // the response comes back. The user can keep editing in the meantime.
        Some(client)
    }

    pub fn alloc_id(&self) -> u64 {
        let mut g = self.next_id.lock().unwrap();
        let id = *g;
        *g += 1;
        id
    }

    /// Write a frame straight to stdin. Used by `send_request_direct` for the
    /// initialize request and by the reader thread when flushing the queue.
    fn write_frame_unconditional(&self, frame: &[u8]) -> std::io::Result<()> {
        let mut stdin = self.stdin.lock().unwrap();
        stdin.write_all(frame)?;
        stdin.flush()
    }

    /// Public send path — buffers if init isn't done; otherwise writes directly.
    fn send_raw(&self, msg: &Value) -> Result<()> {
        let body = serde_json::to_string(msg)?;
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes();
        let mut g = self.init_state.lock().unwrap();
        match &mut *g {
            InitState::Ready => {
                drop(g);
                self.write_frame_unconditional(&frame)?;
            }
            InitState::Buffering(q) => {
                q.push(frame);
            }
        }
        Ok(())
    }

    /// Send a request without going through the init gate. Reserved for the
    /// initialize request itself (it must be the first thing on the wire).
    fn send_request_direct(&self, id: u64, method: &str, params: Value) -> Result<()> {
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let body = serde_json::to_string(&msg)?;
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        self.write_frame_unconditional(frame.as_bytes())?;
        Ok(())
    }

    pub fn send_request(&self, id: u64, method: &str, params: Value) -> Result<()> {
        self.send_raw(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
    }

    pub fn send_notification(&self, method: &str, params: Value) -> Result<()> {
        self.send_raw(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    /// Reply to a server-initiated request the client received earlier
    /// (e.g. `workspace/applyEdit`).
    pub fn send_response(&self, id: u64, result: Value) -> Result<()> {
        self.send_raw(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
    }

    pub fn did_open(&self, path: &Path, text: &str) -> Result<()> {
        self.send_notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": path_to_uri(path),
                    "languageId": self.language_id,
                    "version": 1,
                    "text": text,
                }
            }),
        )
    }

    pub fn did_change(&self, path: &Path, version: u64, text: &str) -> Result<()> {
        self.send_notification(
            "textDocument/didChange",
            json!({
                "textDocument": {
                    "uri": path_to_uri(path),
                    "version": version,
                },
                "contentChanges": [{ "text": text }],
            }),
        )
    }
}

fn reader_loop(
    stdout: impl Read + Send + 'static,
    stdin: Arc<Mutex<ChildStdin>>,
    init_state: Arc<Mutex<InitState>>,
    tx: Sender<LspIncoming>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() || line.is_empty() {
                return;
            }
            let trimmed = line.trim_end_matches(&['\r', '\n'][..]);
            if trimmed.is_empty() {
                break;
            }
            if let Some(rest) = trimmed.to_lowercase().strip_prefix("content-length:") {
                content_length = rest.trim().parse().ok();
            }
        }
        let Some(len) = content_length else { return; };
        let mut body = vec![0u8; len];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&body) else { continue };
        dispatch(value, &stdin, &init_state, &tx);
    }
}

fn dispatch(
    msg: Value,
    stdin: &Arc<Mutex<ChildStdin>>,
    init_state: &Arc<Mutex<InitState>>,
    tx: &Sender<LspIncoming>,
) {
    // Server-to-client request: has both `id` and `method`. Auto-reply so the server
    // doesn't stall waiting for a response we won't otherwise produce.
    let id = msg.get("id").and_then(|v| v.as_u64());
    let method = msg.get("method").and_then(|v| v.as_str()).map(|s| s.to_string());
    if let (Some(id), Some(method)) = (id, method.clone()) {
        // workspace/applyEdit needs the main thread to actually mutate
        // buffers — bounce it through the channel and have the main loop
        // reply via `LspManager::send_response`.
        if method == "workspace/applyEdit" {
            let edit = msg
                .get("params")
                .and_then(|p| p.get("edit"))
                .cloned()
                .unwrap_or(Value::Null);
            let _ = tx.send(LspIncoming::ApplyEditRequest { id, edit });
            return;
        }
        auto_respond(stdin, id, &method, msg.get("params"));
        return;
    }

    // Response: has `id` and either `result` or `error`.
    if let Some(id) = id {
        if let Some(result) = msg.get("result").cloned() {
            // First response while still buffering = answer to `initialize`.
            // Promote the queue to Ready, send "initialized", then flush queued
            // frames in order. We hold the lock for the whole flush so any
            // main-thread sends wait until we're done — preserving order.
            let mut g = init_state.lock().unwrap();
            if matches!(*g, InitState::Buffering(_)) {
                let frames = match std::mem::replace(&mut *g, InitState::Ready) {
                    InitState::Buffering(f) => f,
                    InitState::Ready => Vec::new(),
                };
                let init_notif = json!({
                    "jsonrpc": "2.0",
                    "method": "initialized",
                    "params": {},
                });
                if let Ok(body) = serde_json::to_string(&init_notif) {
                    let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
                    if let Ok(mut s) = stdin.lock() {
                        let _ = s.write_all(frame.as_bytes());
                        let _ = s.flush();
                    }
                }
                for frame in frames {
                    if let Ok(mut s) = stdin.lock() {
                        let _ = s.write_all(&frame);
                        let _ = s.flush();
                    }
                }
            }
            drop(g);
            let _ = tx.send(LspIncoming::Response { id, result });
            return;
        }
        if let Some(err) = msg.get("error") {
            let message = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let _ = tx.send(LspIncoming::ErrorReply { id, message });
            return;
        }
    }

    // Plain notification (no `id`).
    let Some(method) = msg.get("method").and_then(|v| v.as_str()) else { return; };
    if method == "textDocument/publishDiagnostics" {
        if let Some(params) = msg.get("params") {
            if let Some(d) = parse_publish_diagnostics(params) {
                let _ = tx.send(LspIncoming::Diagnostics(d));
            }
        }
    }
}

/// Reply to server-to-client requests with reasonable defaults so the server's
/// initialization (and ongoing operation) isn't blocked waiting for us.
fn auto_respond(
    stdin: &Arc<Mutex<ChildStdin>>,
    id: u64,
    method: &str,
    params: Option<&Value>,
) {
    let result = match method {
        // workspace/configuration → array of nulls, sized to params.items.len().
        "workspace/configuration" => {
            let n = params
                .and_then(|p| p.get("items"))
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            json!(vec![Value::Null; n])
        }
        // workspace/applyEdit is handled out-of-band by the main thread —
        // see `dispatch`. Default arm here just to keep this match
        // exhaustive on future adds.
        "workspace/applyEdit" => json!({ "applied": false }),
        // Various capability registrations / progress windows → null is fine.
        _ => Value::Null,
    };
    let resp = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    let body = match serde_json::to_string(&resp) {
        Ok(s) => s,
        Err(_) => return,
    };
    let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
    if let Ok(mut s) = stdin.lock() {
        let _ = s.write_all(frame.as_bytes());
        let _ = s.flush();
    }
}

fn parse_publish_diagnostics(params: &Value) -> Option<DiagnosticsMessage> {
    let uri = params.get("uri")?.as_str()?.to_string();
    let arr = params.get("diagnostics")?.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for d in arr {
        let range = d.get("range")?;
        let start = range.get("start")?;
        let end = range.get("end")?;
        let line = start.get("line")?.as_u64()? as usize;
        let col = start.get("character")?.as_u64()? as usize;
        let end_line = end.get("line")?.as_u64()? as usize;
        let end_col = end.get("character")?.as_u64()? as usize;
        let severity = match d.get("severity").and_then(|v| v.as_u64()) {
            Some(1) => Severity::Error,
            Some(2) => Severity::Warning,
            Some(3) => Severity::Info,
            Some(4) => Severity::Hint,
            _ => Severity::Info,
        };
        let message = d
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(Diagnostic {
            line,
            col,
            end_line,
            end_col,
            severity,
            message,
        });
    }
    Some(DiagnosticsMessage {
        uri,
        diagnostics: out,
    })
}

pub fn path_to_uri(path: &Path) -> String {
    let abs = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf());
    let s = abs.to_string_lossy().to_string();
    if s.starts_with('/') {
        format!("file://{}", s)
    } else {
        format!("file:///{}", s.replace('\\', "/"))
    }
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let stripped = uri.strip_prefix("file://")?;
    Some(PathBuf::from(stripped))
}

#[derive(Debug, Clone)]
pub struct LspHealth {
    pub key: String,
    pub language_id: String,
    pub root_uri: String,
    pub pending_requests: usize,
}

#[derive(Debug, Clone)]
pub struct ActiveBufferLspStatus {
    pub key: String,
    pub language_id: String,
    /// Resolved path on disk (from `$PATH` or absolute) — `None` means no
    /// candidate command exists on the system.
    pub resolved_binary: Option<String>,
    pub running: bool,
}

/// Container for per-language LSP clients keyed by `ServerSpec.key`.
pub struct LspManager {
    clients: HashMap<String, LspClient>,
    pub diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
    /// Each client allocates IDs from its own counter, so the global key is
    /// `(client_key, id)` rather than just `id` to avoid cross-server clashes.
    pending: HashMap<(String, u64), PendingRequest>,
}

impl LspManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            diagnostics: HashMap::new(),
            pending: HashMap::new(),
        }
    }

    /// Spawn every spec that applies to `path` (primary + auxiliary) and
    /// return the primary client. The primary is the first entry from
    /// `specs_for_path`; auxiliaries (like Tailwind) are kept inside the
    /// manager so they receive didOpen/didChange and contribute to
    /// completions, but they don't take over hover/goto-def.
    pub fn ensure_for_path(&mut self, path: &Path, fallback_root: &Path) -> Option<&LspClient> {
        let specs = specs_for_path(path);
        let primary_key = specs.first().map(|s| s.key.clone())?;
        for spec in &specs {
            if self.clients.contains_key(&spec.key) {
                continue;
            }
            let start = path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| fallback_root.to_path_buf());
            let root = find_workspace_root(&start, &spec.root_markers);
            if let Some(client) = LspClient::spawn_spec(spec, &root) {
                self.clients.insert(spec.key.clone(), client);
            }
        }
        self.clients.get(&primary_key)
    }

    /// What `:health` should say about the active buffer's LSP attachments.
    /// Walks every spec that *would* apply to the path and reports whether
    /// the binary resolves on PATH and whether the client is currently
    /// running. Lets the user see "Tailwind matched but binary missing"
    /// without having to grep their PATH manually.
    pub fn active_buffer_status(&self, path: &Path) -> Vec<ActiveBufferLspStatus> {
        specs_for_path(path)
            .into_iter()
            .map(|spec| ActiveBufferLspStatus {
                resolved_binary: resolve_command(&spec.cmd_candidates).map(|(p, _)| p),
                running: self.clients.contains_key(&spec.key),
                key: spec.key,
                language_id: spec.language_id,
            })
            .collect()
    }

    /// Snapshot of every running LSP client for the `:health` view. Sorted by
    /// key so the report order is stable across calls.
    pub fn health_summary(&self) -> Vec<LspHealth> {
        let mut out: Vec<LspHealth> = self
            .clients
            .iter()
            .map(|(key, client)| {
                let pending = self
                    .pending
                    .keys()
                    .filter(|(k, _)| k == key)
                    .count();
                LspHealth {
                    key: key.clone(),
                    language_id: client.language_id.clone(),
                    root_uri: client.root_uri.clone(),
                    pending_requests: pending,
                }
            })
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
    }

    /// All running clients that match the path's spec list, primary first.
    /// Used to fan out didOpen/didChange and completion requests across the
    /// primary server and any attached auxiliaries.
    pub fn clients_for_path(&self, path: &Path) -> Vec<&LspClient> {
        specs_for_path(path)
            .into_iter()
            .filter_map(|spec| self.clients.get(&spec.key))
            .collect()
    }

    pub fn client_for_path(&self, path: &Path) -> Option<&LspClient> {
        self.clients_for_path(path).into_iter().next()
    }

    /// Drain pending LSP messages, bounded per call. Returns `(events, more)`
    /// where `more` is true if any client still has unread messages — the main
    /// loop uses this to know whether to keep polling for input or come back
    /// for another drain pass. Without the bound, OmniSharp's initial
    /// diagnostics flood (hundreds of files re-published in a burst) starves
    /// the event poll for tens of seconds — fine for slow keyboard input but
    /// painfully visible for mouse clicks.
    pub fn drain(&mut self) -> (Vec<LspEvent>, bool) {
        const MAX_PER_CALL: usize = 64;
        let mut events = Vec::new();
        let mut diagnostics_changed = false;
        let mut processed = 0usize;
        let mut more = false;
        for (client_key, client) in self.clients.iter() {
            while processed < MAX_PER_CALL {
                let Ok(msg) = client.incoming_rx.try_recv() else {
                    break;
                };
                processed += 1;
                match msg {
                    LspIncoming::Diagnostics(d) => {
                        if let Some(path) = uri_to_path(&d.uri) {
                            self.diagnostics.insert(path, d.diagnostics);
                            diagnostics_changed = true;
                        }
                    }
                    LspIncoming::Response { id, result } => {
                        if let Some(req) = self.pending.remove(&(client_key.clone(), id)) {
                            if let Some(ev) = handle_response(req, &result) {
                                events.push(ev);
                            }
                        }
                    }
                    LspIncoming::ErrorReply { id, .. } => {
                        self.pending.remove(&(client_key.clone(), id));
                    }
                    LspIncoming::ApplyEditRequest { id, edit } => {
                        events.push(LspEvent::ApplyEditRequest {
                            client_key: client_key.clone(),
                            id,
                            edit,
                        });
                    }
                }
            }
            // If we hit the per-call cap, peek the rest of the clients to know
            // whether to flag `more` without actually processing them this turn.
            if processed >= MAX_PER_CALL {
                more = true;
                break;
            }
        }
        if diagnostics_changed {
            events.push(LspEvent::DiagnosticsUpdated);
        }
        (events, more)
    }

    pub fn diagnostics_for(&self, path: &Path) -> Option<&Vec<Diagnostic>> {
        if let Some(d) = self.diagnostics.get(path) {
            return Some(d);
        }
        let canon = path.canonicalize().ok()?;
        self.diagnostics.get(&canon)
    }

    pub fn did_change_all(&self, path: &Path, version: u64, text: &str) {
        for client in self.clients.values() {
            let _ = client.did_change(path, version, text);
        }
    }

    /// Reply to a server's `workspace/applyEdit` request after the main
    /// thread has applied (or failed to apply) the edit.
    pub fn send_apply_edit_response(&self, client_key: &str, id: u64, applied: bool) {
        if let Some(client) = self.clients.get(client_key) {
            let _ = client.send_response(id, json!({ "applied": applied }));
        }
    }

    /// Fire `workspace/executeCommand` against the path's primary server.
    /// `command_obj` must be the LSP `Command` shape (`{ title, command,
    /// arguments? }`). The server's response is fire-and-forget — most
    /// servers respond null and instead push their effect through a
    /// follow-up `workspace/applyEdit` request.
    pub fn execute_command(&mut self, path: &Path, command_obj: &Value) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let cmd = command_obj
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if cmd.is_empty() {
            return false;
        }
        let mut params = json!({ "command": cmd });
        if let Some(args) = command_obj.get("arguments").cloned() {
            params["arguments"] = args;
        }
        let _ = client.send_request(id, "workspace/executeCommand", params);
        // No PendingRequest variant — we don't surface the response, the
        // server delivers the effect via follow-up applyEdit requests.
        true
    }

    pub fn request_definition(&mut self, path: &Path, line: usize, col: usize) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/definition",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": col }
            }),
        );
        self.pending.insert((client.name.clone(), id), PendingRequest::GotoDef);
        true
    }

    pub fn request_hover(&mut self, path: &Path, line: usize, col: usize) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/hover",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": col }
            }),
        );
        self.pending.insert((client.name.clone(), id), PendingRequest::Hover);
        true
    }

    /// Fan out a completion request to every server attached to this path.
    /// Each server's reply arrives as its own `LspEvent::Completion`; the
    /// caller is responsible for merging them into the in-flight popup.
    pub fn request_completion(
        &mut self,
        path: &Path,
        line: usize,
        col: usize,
        trigger_char: Option<char>,
    ) -> bool {
        // LSP CompletionTriggerKind: 1=Invoked, 2=TriggerCharacter.
        // Servers use this to decide whether to return member-access
        // completions (after `.`, `:`, etc.) versus general scope items.
        let context = match trigger_char {
            Some(c) => json!({ "triggerKind": 2, "triggerCharacter": c.to_string() }),
            None => json!({ "triggerKind": 1 }),
        };
        let mut sent = Vec::new();
        for client in self.clients_for_path(path) {
            let id = client.alloc_id();
            let _ = client.send_request(
                id,
                "textDocument/completion",
                json!({
                    "textDocument": { "uri": path_to_uri(path) },
                    "position": { "line": line, "character": col },
                    "context": context,
                }),
            );
            sent.push((client.name.clone(), id));
        }
        let any = !sent.is_empty();
        for k in sent {
            self.pending.insert(k, PendingRequest::Completion);
        }
        any
    }

    /// Request `textDocument/rename` with the user's chosen new name.
    pub fn request_rename(
        &mut self,
        path: &Path,
        line: usize,
        col: usize,
        new_name: &str,
    ) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/rename",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": col },
                "newName": new_name,
            }),
        );
        self.pending
            .insert((client.name.clone(), id), PendingRequest::Rename);
        true
    }

    /// Request `textDocument/codeAction` for the cursor position. The
    /// caller passes the diagnostics overlapping that position (only those
    /// — passing the full file's worth made tsserver hang on big projects).
    pub fn request_code_actions(
        &mut self,
        path: &Path,
        line: usize,
        col: usize,
        diagnostics: Vec<Value>,
    ) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "range": {
                    "start": { "line": line, "character": col },
                    "end":   { "line": line, "character": col },
                },
                "context": {
                    "diagnostics": diagnostics,
                    "triggerKind": 1,
                },
            }),
        );
        self.pending
            .insert((client.name.clone(), id), PendingRequest::CodeActions);
        true
    }

    /// Request `textDocument/documentSymbol` to populate the outline picker.
    pub fn request_document_symbols(&mut self, path: &Path) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": path_to_uri(path) } }),
        );
        self.pending
            .insert((client.name.clone(), id), PendingRequest::DocumentSymbols);
        true
    }

    /// Request `workspace/symbol`. The server-side fuzzy matcher does the
    /// ranking; we just relay results to the picker. `query` may be empty.
    pub fn request_workspace_symbols(&mut self, path: &Path, query: &str) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(id, "workspace/symbol", json!({ "query": query }));
        self.pending
            .insert((client.name.clone(), id), PendingRequest::WorkspaceSymbols);
        true
    }

    /// Request `textDocument/references` from the primary server with
    /// `includeDeclaration: true` so the user sees the definition site too.
    pub fn request_references(&mut self, path: &Path, line: usize, col: usize) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/references",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": col },
                "context": { "includeDeclaration": true },
            }),
        );
        self.pending
            .insert((client.name.clone(), id), PendingRequest::References);
        true
    }

    /// Request `textDocument/signatureHelp` from the primary server. Goes
    /// to one server only — multi-server fan-out wouldn't help here, the
    /// primary is the source of truth for the language's call syntax.
    pub fn request_signature_help(&mut self, path: &Path, line: usize, col: usize) -> bool {
        let Some(client) = self.client_for_path(path) else { return false; };
        let id = client.alloc_id();
        let _ = client.send_request(
            id,
            "textDocument/signatureHelp",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": col }
            }),
        );
        self.pending
            .insert((client.name.clone(), id), PendingRequest::SignatureHelp);
        true
    }
}

fn handle_response(req: PendingRequest, result: &Value) -> Option<LspEvent> {
    match req {
        PendingRequest::GotoDef => match parse_def_response(result) {
            Some((path, line, col)) => Some(LspEvent::GotoDef { path, line, col }),
            None => Some(LspEvent::NotFound("definition")),
        },
        PendingRequest::Hover => match parse_hover_response(result) {
            Some(text) => Some(LspEvent::Hover { text }),
            None => Some(LspEvent::NotFound("hover")),
        },
        PendingRequest::Completion => {
            let items = parse_completion_response(result);
            if items.is_empty() {
                Some(LspEvent::NotFound("completions"))
            } else {
                Some(LspEvent::Completion { items })
            }
        }
        PendingRequest::SignatureHelp => match parse_signature_help_response(result) {
            Some(sig) => Some(LspEvent::SignatureHelp(sig)),
            None => Some(LspEvent::NotFound("signature")),
        },
        PendingRequest::References => {
            let items = parse_locations_response(result);
            if items.is_empty() {
                Some(LspEvent::NotFound("references"))
            } else {
                Some(LspEvent::References { items })
            }
        }
        PendingRequest::DocumentSymbols => {
            let items = parse_symbols_response(result);
            if items.is_empty() {
                Some(LspEvent::NotFound("symbols"))
            } else {
                Some(LspEvent::Symbols { items, workspace: false })
            }
        }
        PendingRequest::WorkspaceSymbols => {
            let items = parse_symbols_response(result);
            // Empty results during live filtering shouldn't toast — the
            // caller distinguishes by the `workspace: true` flag.
            Some(LspEvent::Symbols { items, workspace: true })
        }
        PendingRequest::CodeActions => {
            let items = parse_code_actions_response(result);
            if items.is_empty() {
                Some(LspEvent::NotFound("code actions"))
            } else {
                Some(LspEvent::CodeActions { items })
            }
        }
        PendingRequest::Rename => {
            if result.is_null() {
                Some(LspEvent::NotFound("rename target"))
            } else {
                Some(LspEvent::Rename { edit: result.clone() })
            }
        }
    }
}

fn parse_code_actions_response(result: &Value) -> Vec<CodeActionItem> {
    let arr = match result.as_array() {
        Some(a) => a.clone(),
        None => return Vec::new(),
    };
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        // `Command` shape: { title, command, arguments? }
        // `CodeAction` shape: { title, kind?, edit?, command?, disabled? }
        let title = match entry.get("title").and_then(|v| v.as_str()) {
            Some(t) => t.to_string(),
            None => continue,
        };
        let kind = entry
            .get("kind")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let edit = entry.get("edit").cloned();
        let command_field = entry.get("command");
        // CodeAction's `command` is a Command object; bare Command-shaped
        // entries place the command at the top level — both reduce to the
        // same JSON we'll execute later.
        let command = if command_field.map(|v| v.is_object()).unwrap_or(false) {
            command_field.cloned()
        } else if entry.get("command").map(|v| v.is_string()).unwrap_or(false) {
            Some(entry.clone())
        } else {
            None
        };
        let disabled_reason = entry
            .get("disabled")
            .and_then(|v| v.get("reason"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        out.push(CodeActionItem {
            title,
            kind,
            edit,
            command,
            disabled_reason,
        });
    }
    out
}

/// Parse `DocumentSymbol[]` (hierarchical), `SymbolInformation[]` (flat),
/// or `WorkspaceSymbol[]` into our internal shape. Hierarchical entries
/// flatten with their container path joined by `›`.
fn parse_symbols_response(result: &Value) -> Vec<SymbolItem> {
    let arr = match result.as_array() {
        Some(a) => a.clone(),
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in arr {
        flatten_symbol(&entry, "", &mut out);
    }
    out
}

fn flatten_symbol(entry: &Value, container: &str, out: &mut Vec<SymbolItem>) {
    let name = entry
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return;
    }
    let kind = entry
        .get("kind")
        .and_then(|v| v.as_u64())
        .map(symbol_kind_label)
        .unwrap_or_else(|| "?".into());
    // DocumentSymbol uses `selectionRange`; SymbolInformation/WorkspaceSymbol
    // uses `location.range`. WorkspaceSymbol may also use `location.uri`
    // without a range.
    let (uri, range) = if let Some(loc) = entry.get("location") {
        let uri = loc.get("uri").and_then(|v| v.as_str()).map(|s| s.to_string());
        let range = loc.get("range").or_else(|| loc.get("targetRange")).cloned();
        (uri, range)
    } else {
        (None, entry.get("selectionRange").or_else(|| entry.get("range")).cloned())
    };
    let start = range
        .as_ref()
        .and_then(|r| r.get("start"))
        .map(|s| {
            (
                s.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                s.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            )
        });
    let path = uri.and_then(|u| uri_to_path(&u));
    if let (Some(path), Some((line, col))) = (path, start) {
        out.push(SymbolItem {
            name: name.clone(),
            container: container.to_string(),
            kind,
            path,
            line,
            col,
        });
    } else if let Some((line, col)) = start {
        // DocumentSymbol with no embedded URI — leave path empty; the
        // caller knows the active buffer's path.
        out.push(SymbolItem {
            name: name.clone(),
            container: container.to_string(),
            kind,
            path: PathBuf::new(),
            line,
            col,
        });
    }
    if let Some(children) = entry.get("children").and_then(|v| v.as_array()) {
        let next_container = if container.is_empty() {
            name.clone()
        } else {
            format!("{container} › {name}")
        };
        for child in children {
            flatten_symbol(child, &next_container, out);
        }
    }
}

fn symbol_kind_label(k: u64) -> String {
    match k {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        15 => "string",
        16 => "number",
        17 => "bool",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "enum-member",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "type-param",
        _ => "?",
    }
    .into()
}

/// Parse a `Location[]` (or `LocationLink[]`) response into our internal
/// shape. Used by `references` and reusable for any future symbol query
/// that returns the same shape.
fn parse_locations_response(result: &Value) -> Vec<LocationItem> {
    let arr = match result.as_array() {
        Some(a) => a.clone(),
        None => return Vec::new(),
    };
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        // Either { uri, range: { start: {line, character} } } (Location) or
        // { targetUri, targetSelectionRange: { start: ... } } (LocationLink).
        let uri = entry
            .get("uri")
            .and_then(|v| v.as_str())
            .or_else(|| entry.get("targetUri").and_then(|v| v.as_str()));
        let range = entry
            .get("range")
            .or_else(|| entry.get("targetSelectionRange"))
            .or_else(|| entry.get("targetRange"));
        let (Some(uri), Some(range)) = (uri, range) else { continue };
        let Some(path) = uri_to_path(uri) else { continue };
        let Some(start) = range.get("start") else { continue };
        let Some(line) = start.get("line").and_then(|v| v.as_u64()) else { continue };
        let Some(col) = start.get("character").and_then(|v| v.as_u64()) else { continue };
        out.push(LocationItem {
            path,
            line: line as usize,
            col: col as usize,
        });
    }
    out
}

/// Picks the active signature out of the response and resolves the active
/// parameter range. Servers commonly return a `parameters` array of either
/// `{ label: string }` (a substring of `signature.label`) or
/// `{ label: [start, end] }` (char indices into `signature.label`). Both
/// shapes are handled here.
fn parse_signature_help_response(result: &Value) -> Option<SignatureHelp> {
    if result.is_null() {
        return None;
    }
    let sigs = result.get("signatures")?.as_array()?;
    if sigs.is_empty() {
        return None;
    }
    let active_sig = result
        .get("activeSignature")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let sig = sigs.get(active_sig).or_else(|| sigs.first())?;
    let label = sig.get("label")?.as_str()?.to_string();
    let active_param_idx = sig
        .get("activeParameter")
        .and_then(|v| v.as_u64())
        .or_else(|| result.get("activeParameter").and_then(|v| v.as_u64()))
        .map(|n| n as usize);
    let active_param = (|| -> Option<(usize, usize)> {
        let params = sig.get("parameters")?.as_array()?;
        let idx = active_param_idx?;
        let p = params.get(idx)?;
        let plabel = p.get("label")?;
        if let Some(arr) = plabel.as_array() {
            // [start, end] in chars (UTF-16 per spec but we treat chars
            // approximately — close enough for ASCII signatures).
            let start = arr.first()?.as_u64()? as usize;
            let end = arr.get(1)?.as_u64()? as usize;
            return Some((start, end));
        }
        if let Some(needle) = plabel.as_str() {
            // Substring form — find first occurrence inside the label.
            let bytes = label.as_bytes();
            let needle_bytes = needle.as_bytes();
            let pos = bytes
                .windows(needle_bytes.len())
                .position(|w| w == needle_bytes)?;
            // Convert byte pos → char pos.
            let prefix = &label[..pos];
            let cstart = prefix.chars().count();
            let cend = cstart + needle.chars().count();
            return Some((cstart, cend));
        }
        None
    })();
    Some(SignatureHelp { label, active_param })
}

fn parse_completion_response(result: &Value) -> Vec<CompletionItem> {
    let arr = if result.is_array() {
        result.as_array().cloned().unwrap_or_default()
    } else if let Some(items) = result.get("items").and_then(|v| v.as_array()) {
        items.clone()
    } else {
        return Vec::new();
    };
    // Don't cap here — the client filters by typed prefix afterwards, and
    // capping at the wire would silently drop relevant items past the cap
    // (typescript-language-server can return several thousand for a top-level
    // identifier position).
    let mut out = Vec::with_capacity(arr.len());
    for item in arr.iter() {
        let label = item
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if label.is_empty() {
            continue;
        }
        let insert_text = item
            .get("insertText")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                item.get("textEdit")
                    .and_then(|t| t.get("newText"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| label.clone());
        let kind = item.get("kind").and_then(|v| v.as_u64()).map(kind_label);
        let detail = item
            .get("detail")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let filter_text = item
            .get("filterText")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| label.clone());
        let sort_text = item
            .get("sortText")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| label.clone());
        out.push(CompletionItem {
            label,
            insert_text,
            kind,
            detail,
            filter_text,
            sort_text,
        });
    }
    out
}

fn kind_label(k: u64) -> String {
    // Mapping per LSP spec.
    match k {
        1 => "text",
        2 => "method",
        3 => "function",
        4 => "constructor",
        5 => "field",
        6 => "variable",
        7 => "class",
        8 => "interface",
        9 => "module",
        10 => "property",
        11 => "unit",
        12 => "value",
        13 => "enum",
        14 => "keyword",
        15 => "snippet",
        16 => "color",
        17 => "file",
        18 => "reference",
        19 => "folder",
        20 => "enum-member",
        21 => "constant",
        22 => "struct",
        23 => "event",
        24 => "operator",
        25 => "type-param",
        _ => "?",
    }
    .into()
}

fn parse_def_response(result: &Value) -> Option<(PathBuf, usize, usize)> {
    if result.is_null() {
        return None;
    }
    let loc = if result.is_array() {
        result.as_array()?.first()?
    } else {
        result
    };
    // Location | LocationLink — try .uri first, then .targetUri.
    let uri = loc
        .get("uri")
        .and_then(|v| v.as_str())
        .or_else(|| loc.get("targetUri").and_then(|v| v.as_str()))?;
    let path = uri_to_path(uri)?;
    let range = loc
        .get("range")
        .or_else(|| loc.get("targetSelectionRange"))
        .or_else(|| loc.get("targetRange"))?;
    let start = range.get("start")?;
    let line = start.get("line")?.as_u64()? as usize;
    let col = start.get("character")?.as_u64()? as usize;
    Some((path, line, col))
}

fn parse_hover_response(result: &Value) -> Option<String> {
    if result.is_null() {
        return None;
    }
    let contents = result.get("contents")?;
    if let Some(s) = contents.as_str() {
        return Some(s.to_string());
    }
    if let Some(obj) = contents.as_object() {
        if let Some(v) = obj.get("value").and_then(|v| v.as_str()) {
            return Some(v.to_string());
        }
    }
    if let Some(arr) = contents.as_array() {
        let mut out = String::new();
        for item in arr {
            let s = item
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| item.get("value").and_then(|v| v.as_str()).map(|s| s.to_string()));
            if let Some(s) = s {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&s);
            }
        }
        if !out.is_empty() {
            return Some(out);
        }
    }
    None
}
