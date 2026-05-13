//! Per-extension server dispatch and workspace discovery. The four
//! `*_spec_for_path` functions are the only place we hard-code language
//! servers; adding a new one means editing `primary_spec_for_path`.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

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
pub fn specs_for_path(path: &Path) -> Vec<ServerSpec> {
    let mut specs = Vec::new();
    if let Some(primary) = primary_spec_for_path(path) {
        specs.push(primary);
    }
    if let Some(tw) = tailwind_spec_for_path(path) {
        specs.push(tw);
    }
    if let Some(em) = emmet_spec_for_path(path) {
        specs.push(em);
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
            // OmniSharp handles .cshtml as a C# document and gives real
            // IntelliSense for the embedded code blocks (@{}, @Model.X, etc.).
            // If OmniSharp isn't installed, fall back to the html LSP so at
            // least markup completion still works.
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
        "py" | "pyi" => Some(ServerSpec {
            key: "pyright".into(),
            language_id: "python".into(),
            // basedpyright is a maintained fork some users prefer — try
            // both binaries so either install works without config.
            cmd_candidates: vec![
                "pyright-langserver".into(),
                "basedpyright-langserver".into(),
            ],
            args: stdio(),
            root_markers: vec![
                "pyproject.toml".into(),
                "setup.py".into(),
                "setup.cfg".into(),
                "requirements.txt".into(),
                "Pipfile".into(),
                "Pipfile.lock".into(),
                ".git".into(),
            ],
            initialization_options: Value::Null,
        }),
        "c" | "h" | "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "c++" | "h++" => {
            // clangd handles both C and C++ — language id flips per
            // extension so the server interprets the buffer correctly.
            let language_id = match ext {
                "c" | "h" => "c",
                _ => "cpp",
            };
            Some(ServerSpec {
                key: "clangd".into(),
                language_id: language_id.into(),
                cmd_candidates: vec!["clangd".into()],
                args: vec![],
                root_markers: vec![
                    "compile_commands.json".into(),
                    "compile_flags.txt".into(),
                    "CMakeLists.txt".into(),
                    "Makefile".into(),
                    ".git".into(),
                ],
                initialization_options: Value::Null,
            })
        }
        "sh" | "bash" | "zsh" | "ksh" => Some(ServerSpec {
            key: "bashls".into(),
            language_id: "shellscript".into(),
            cmd_candidates: vec!["bash-language-server".into()],
            args: vec!["start".into()],
            root_markers: vec![".git".into()],
            initialization_options: Value::Null,
        }),
        "yml" | "yaml" => Some(ServerSpec {
            key: "yamlls".into(),
            language_id: "yaml".into(),
            cmd_candidates: vec!["yaml-language-server".into()],
            args: stdio(),
            root_markers: vec![".git".into()],
            initialization_options: Value::Null,
        }),
        "lua" => Some(ServerSpec {
            key: "lua-ls".into(),
            language_id: "lua".into(),
            cmd_candidates: vec!["lua-language-server".into()],
            args: vec![],
            root_markers: vec![
                ".luarc.json".into(),
                ".luarc.jsonc".into(),
                "init.lua".into(),
                ".git".into(),
            ],
            initialization_options: Value::Null,
        }),
        "vue" => Some(ServerSpec {
            key: "vue".into(),
            language_id: "vue".into(),
            cmd_candidates: vec!["vue-language-server".into()],
            args: stdio(),
            root_markers: vec![
                "vue.config.js".into(),
                "vue.config.ts".into(),
                "vite.config.ts".into(),
                "vite.config.js".into(),
                "package.json".into(),
                ".git".into(),
            ],
            initialization_options: Value::Null,
        }),
        "svelte" => Some(ServerSpec {
            key: "svelte".into(),
            language_id: "svelte".into(),
            cmd_candidates: vec!["svelteserver".into()],
            args: stdio(),
            root_markers: vec![
                "svelte.config.js".into(),
                "svelte.config.ts".into(),
                "package.json".into(),
                ".git".into(),
            ],
            initialization_options: Value::Null,
        }),
        "md" | "markdown" => Some(ServerSpec {
            key: "marksman".into(),
            language_id: "markdown".into(),
            cmd_candidates: vec!["marksman".into()],
            // marksman defaults to `server` which is the LSP mode; the
            // bare invocation works too on recent versions, but pass it
            // explicitly so an older install doesn't drop into help.
            args: vec!["server".into()],
            root_markers: vec![
                ".marksman.toml".into(),
                ".git".into(),
            ],
            initialization_options: Value::Null,
        }),
        "toml" => Some(ServerSpec {
            key: "taplo".into(),
            language_id: "toml".into(),
            cmd_candidates: vec!["taplo".into(), cargo_bin("taplo")],
            args: vec!["lsp".into(), "stdio".into()],
            root_markers: vec![".taplo.toml".into(), "taplo.toml".into(), ".git".into()],
            initialization_options: Value::Null,
        }),
        "rb" | "rake" | "gemspec" => Some(ServerSpec {
            key: "ruby-lsp".into(),
            language_id: "ruby".into(),
            cmd_candidates: vec!["ruby-lsp".into()],
            args: vec!["stdio".into()],
            root_markers: vec![
                "Gemfile".into(),
                ".ruby-version".into(),
                ".git".into(),
            ],
            initialization_options: Value::Null,
        }),
        "php" => Some(ServerSpec {
            key: "intelephense".into(),
            language_id: "php".into(),
            cmd_candidates: vec!["intelephense".into()],
            args: stdio(),
            root_markers: vec!["composer.json".into(), ".git".into()],
            initialization_options: Value::Null,
        }),
        "java" => {
            // jdtls insists on a per-project workspace data dir or it
            // pollutes a default location and silently fails to load
            // changed files between projects. Hash the file's parent
            // path so each project gets its own slot under
            // `~/.cache/binvim/jdtls/`.
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let project_key = path
                .parent()
                .map(|p| {
                    let mut h = DefaultHasher::new();
                    p.canonicalize().unwrap_or_else(|_| p.to_path_buf()).hash(&mut h);
                    format!("{:x}", h.finish())
                })
                .unwrap_or_else(|| "default".into());
            let workspace = format!("{}/.cache/binvim/jdtls/{}", home, project_key);
            Some(ServerSpec {
                key: "jdtls".into(),
                language_id: "java".into(),
                cmd_candidates: vec!["jdtls".into()],
                args: vec!["-data".into(), workspace],
                root_markers: vec![
                    "pom.xml".into(),
                    "build.gradle".into(),
                    "build.gradle.kts".into(),
                    ".git".into(),
                ],
                initialization_options: Value::Null,
            })
        }
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

/// Emmet abbreviation expansion as an LSP. Layers on top of the primary
/// server for any markup-flavoured buffer — typing `div` then accepting the
/// completion item expands to `<div></div>`, `ul>li*3` builds the nested
/// list, `.foo` becomes `<div class="foo">…</div>`, and so on.
///
/// We send the closest core language id emmet-ls recognises (`html`, `css`,
/// `scss`, `less`, `sass`, `javascriptreact`, `typescriptreact`, `vue`,
/// `svelte`, `astro`). Razor / cshtml report as `html` — emmet-ls has no
/// explicit razor mode, but the markup half of a Razor file is HTML, so
/// HTML emmet completions fire wherever the cursor is in an HTML context.
fn emmet_spec_for_path(path: &Path) -> Option<ServerSpec> {
    let ext = path.extension().and_then(|s| s.to_str())?.to_ascii_lowercase();
    let language_id = match ext.as_str() {
        "html" | "htm" | "cshtml" | "razor" => "html",
        "css" => "css",
        "scss" => "scss",
        "less" => "less",
        "sass" => "sass",
        "jsx" => "javascriptreact",
        "tsx" => "typescriptreact",
        "vue" => "vue",
        "svelte" => "svelte",
        "astro" => "astro",
        _ => return None,
    };
    Some(ServerSpec {
        key: "emmet".into(),
        language_id: language_id.into(),
        cmd_candidates: vec!["emmet-ls".into(), "emmet-language-server".into()],
        args: vec!["--stdio".into()],
        root_markers: vec![".git".into()],
        initialization_options: Value::Null,
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

pub(crate) fn resolve_command(candidates: &[String]) -> Option<(String, Vec<String>)> {
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
