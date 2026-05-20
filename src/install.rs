//! Shared install catalog + runner used by both the `binvim-install`
//! CLI binary and the in-editor `:install` overlay. UI lives in the
//! consumers — this module is pure data + side-effect-free helpers
//! (catalog, PATH probing, Node.js discovery, plan building) plus
//! `run_plan`, which shells out to the chosen installers.
//!
//! Catalog conventions mirror the original CLI: one Bundle per
//! language with its LSP + formatter + DAP, plus standalone bundles
//! for Copilot, Tailwind, and editor-wide tools (ripgrep / lazygit /
//! yazi). Tools shared across bundles (`prettier`, `lldb-dap`,
//! `vscode-langservers-extracted`, `emmet-ls`) are deduplicated by
//! `bin` name at plan-build time.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ─── catalog ───────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Role {
    Lsp,
    Formatter,
    Dap,
    /// Editor-wide utility that isn't tied to a single language —
    /// `rg` (live grep), `lazygit` (git UI takeover), `yazi` (file picker).
    Tool,
}

impl Role {
    pub fn tag(self) -> &'static str {
        match self {
            Role::Lsp => "LSP",
            Role::Formatter => "FMT",
            Role::Dap => "DAP",
            Role::Tool => "TOOL",
        }
    }
}

/// One installable thing — `bin` is the name we probe on `$PATH` to know
/// whether it's already installed. `installers` is tried in order; the first
/// one whose host tool exists wins. A trailing `Manual` entry documents what
/// to do when no automatic option applies.
#[derive(Copy, Clone)]
pub struct Tool {
    pub bin: &'static str,
    pub label: &'static str,
    pub role: Role,
    pub installers: &'static [Installer],
}

#[derive(Copy, Clone)]
pub enum Installer {
    Brew(&'static str),
    Apt(&'static str),
    /// `npm i -g pkg[@version] [pkg[@version] ...]` — version pin goes
    /// in the package string (`@x.y.z`). Each entry in the slice is
    /// passed as a separate `npm` argument.
    Npm(&'static [&'static str]),
    /// `cargo install <pkg> <extra...>`. Pinning is in `extra` via
    /// `--version X.Y.Z`.
    Cargo(&'static str, &'static [&'static str]),
    Rustup(&'static str),
    /// `go install <module>` — append `@vX.Y.Z` to the module path
    /// (or `@latest` when binvim-web hasn't pinned a specific
    /// release for this tool).
    Go(&'static str),
    /// `pipx install <pkg[==version]>` — pin syntax embedded in the
    /// package string.
    Pipx(&'static str),
    /// `pip install --user <pkg[==version]>` — pin syntax embedded in
    /// the package string.
    Pip(&'static str),
    /// `gem install <pkg> [-v <version>]`. `None` skips the flag.
    Gem(&'static str, Option<&'static str>),
    /// `dotnet tool install --global <pkg> [--version <version>]`.
    /// `None` skips the flag.
    DotnetTool(&'static str, Option<&'static str>),
    Nix(&'static str),
    /// `composer global require <pkg[:version]>` — pin syntax embedded
    /// in the package string.
    Composer(&'static str),
    Manual(&'static str),
}

impl Installer {
    pub fn manager(&self) -> &'static str {
        match self {
            Installer::Brew(_) => "brew",
            Installer::Apt(_) => "apt-get",
            Installer::Npm(_) => "npm",
            Installer::Cargo(_, _) => "cargo",
            Installer::Rustup(_) => "rustup",
            Installer::Go(_) => "go",
            Installer::Pipx(_) => "pipx",
            Installer::Pip(_) => "pip",
            Installer::Gem(_, _) => "gem",
            Installer::DotnetTool(_, _) => "dotnet",
            Installer::Nix(_) => "nix",
            Installer::Composer(_) => "composer",
            Installer::Manual(_) => "",
        }
    }

    pub fn display(&self) -> String {
        match self {
            Installer::Brew(p) => format!("brew install {p}"),
            Installer::Apt(p) => format!("sudo apt-get install -y {p}"),
            Installer::Npm(pkgs) => format!("npm install -g {}", pkgs.join(" ")),
            Installer::Cargo(p, extra) if extra.is_empty() => format!("cargo install {p}"),
            Installer::Cargo(p, extra) => format!("cargo install {p} {}", extra.join(" ")),
            Installer::Rustup(c) => format!("rustup component add {c}"),
            Installer::Go(m) => format!("go install {m}"),
            Installer::Pipx(p) => format!("pipx install {p}"),
            Installer::Pip(p) => format!("pip install --user {p}"),
            Installer::Gem(p, None) => format!("gem install {p}"),
            Installer::Gem(p, Some(v)) => format!("gem install {p} -v {v}"),
            Installer::DotnetTool(p, None) => format!("dotnet tool install --global {p}"),
            Installer::DotnetTool(p, Some(v)) => {
                format!("dotnet tool install --global {p} --version {v}")
            }
            Installer::Nix(r) => format!("nix profile install {r}"),
            Installer::Composer(p) => format!("composer global require {p}"),
            Installer::Manual(s) => format!("manual: {s}"),
        }
    }

    /// Build the `Command` we will spawn. `None` for `Manual` — caller prints
    /// the instructions and moves on. `None` for `Npm` too — npm needs a
    /// `NodeVersion` to target, so the runner constructs that command
    /// directly rather than going through `build_command`.
    pub fn build_command(&self) -> Option<Command> {
        let mut cmd = match self {
            Installer::Brew(p) => {
                let mut c = Command::new("brew");
                c.args(["install", p]);
                c
            }
            Installer::Apt(p) => {
                let mut c = Command::new("sudo");
                c.args(["apt-get", "install", "-y", p]);
                c
            }
            Installer::Npm(_) => return None,
            Installer::Cargo(p, extra) => {
                let mut c = Command::new("cargo");
                c.args(["install", p]);
                c.args(extra.iter().copied());
                c
            }
            Installer::Rustup(component) => {
                let mut c = Command::new("rustup");
                c.args(["component", "add", component]);
                c
            }
            Installer::Go(module) => {
                let mut c = Command::new("go");
                c.args(["install", module]);
                c
            }
            Installer::Pipx(p) => {
                let mut c = Command::new("pipx");
                c.args(["install", p]);
                c
            }
            Installer::Pip(p) => {
                let mut c = Command::new("pip");
                c.args(["install", "--user", p]);
                c
            }
            Installer::Gem(p, version) => {
                let mut c = Command::new("gem");
                c.args(["install", p]);
                if let Some(v) = version {
                    c.args(["-v", v]);
                }
                c
            }
            Installer::DotnetTool(p, version) => {
                let mut c = Command::new("dotnet");
                c.args(["tool", "install", "--global", p]);
                if let Some(v) = version {
                    c.args(["--version", v]);
                }
                c
            }
            Installer::Nix(r) => {
                let mut c = Command::new("nix");
                c.args(["profile", "install", r]);
                c
            }
            Installer::Composer(p) => {
                let mut c = Command::new("composer");
                c.args(["global", "require", p]);
                c
            }
            Installer::Manual(_) => return None,
        };
        cmd.stdin(Stdio::inherit());
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());
        Some(cmd)
    }
}

pub struct Bundle {
    pub name: &'static str,
    pub tools: &'static [Tool],
}

/// `emmet-ls` attaches to every markup-flavoured buffer binvim recognises
/// (HTML, CSS, JSX/TSX, Vue, Svelte, Astro, Razor — see `README.md:75`).
/// Declared once and folded into each of those bundles so picking any one
/// of them installs Emmet, and the dedupe in `build_plan` collapses the
/// install to a single npm run no matter how many you picked.
pub const EMMET_LS: Tool = Tool {
    bin: "emmet-ls",
    label: "emmet-ls",
    role: Role::Lsp,
    installers: &[Installer::Npm(&["emmet-ls@0.4.2"])],
};

/// The catalog. Mirrors the README install table at `README.md:283+`. When a
/// tool appears under multiple languages (prettier, lldb-dap, vscode-
/// langservers-extracted, biome, EMMET_LS, …) it's repeated literally —
/// `build_plan` dedupes by `bin` at plan time.
///
/// **Version pinning** — npm / go / cargo / pipx / gem / dotnet / composer
/// installers carry the same pins used on binvim.dev's install table.
/// Bumping a pin here keeps the CLI installer and the in-editor `:install`
/// overlay in sync. Brew / nix / apt formulas aren't pinned in the command
/// (their package manager owns the version), and `dlv` / `debugpy` /
/// `lazygit` aren't pinned because binvim-web doesn't track them.
#[rustfmt::skip]
pub const BUNDLES: &[Bundle] = &[
    Bundle { name: "Rust", tools: &[
        Tool { bin: "rust-analyzer", label: "rust-analyzer", role: Role::Lsp,
            installers: &[Installer::Rustup("rust-analyzer")] },
        Tool { bin: "rustfmt", label: "rustfmt", role: Role::Formatter,
            installers: &[Installer::Rustup("rustfmt")] },
        Tool { bin: "lldb-dap", label: "lldb-dap", role: Role::Dap,
            installers: &[Installer::Brew("llvm"), Installer::Apt("lldb")] },
    ]},
    Bundle { name: "TypeScript / JavaScript", tools: &[
        Tool { bin: "typescript-language-server", label: "typescript-language-server", role: Role::Lsp,
            installers: &[Installer::Npm(&["typescript-language-server@5.1.3", "typescript@6.0.3"])] },
        Tool { bin: "biome", label: "biome", role: Role::Formatter,
            installers: &[Installer::Npm(&["@biomejs/biome@2.4.10"])] },
        Tool { bin: "prettier", label: "prettier (fallback formatter)", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier@3.8.3"])] },
        EMMET_LS,
    ]},
    Bundle { name: "Go", tools: &[
        Tool { bin: "gopls", label: "gopls", role: Role::Lsp,
            installers: &[Installer::Go("golang.org/x/tools/gopls@v0.21.1")] },
        Tool { bin: "goimports", label: "goimports", role: Role::Formatter,
            installers: &[Installer::Go("golang.org/x/tools/cmd/goimports@v0.45.0")] },
        Tool { bin: "dlv", label: "delve (dlv)", role: Role::Dap,
            installers: &[Installer::Go("github.com/go-delve/delve/cmd/dlv@latest")] },
    ]},
    Bundle { name: "Python", tools: &[
        Tool { bin: "pyright-langserver", label: "pyright", role: Role::Lsp,
            installers: &[Installer::Npm(&["pyright@1.1.409"])] },
        Tool { bin: "ruff", label: "ruff", role: Role::Formatter,
            installers: &[Installer::Pipx("ruff==0.15.13")] },
        // debugpy has no binary on PATH — we probe `python3 -m debugpy.adapter`.
        // The sentinel `python3-debugpy` ensures the PATH check fails so the
        // install runs; the installer itself drops it into the user's
        // site-packages. Re-runs reinvoke the installer; pip says "already
        // satisfied" which is harmless. Un-pinned because binvim-web doesn't
        // track a debugpy version.
        Tool { bin: "python3-debugpy", label: "debugpy", role: Role::Dap,
            installers: &[Installer::Pipx("debugpy"), Installer::Pip("debugpy")] },
    ]},
    Bundle { name: "C / C++", tools: &[
        Tool { bin: "clangd", label: "clangd", role: Role::Lsp,
            installers: &[Installer::Brew("llvm"), Installer::Apt("clangd")] },
        Tool { bin: "clang-format", label: "clang-format", role: Role::Formatter,
            installers: &[Installer::Brew("llvm"), Installer::Apt("clang-format")] },
        Tool { bin: "lldb-dap", label: "lldb-dap", role: Role::Dap,
            installers: &[Installer::Brew("llvm"), Installer::Apt("lldb")] },
    ]},
    Bundle { name: "C#", tools: &[
        Tool { bin: "csharp-ls", label: "csharp-ls", role: Role::Lsp,
            installers: &[Installer::DotnetTool("csharp-ls", Some("0.24.0"))] },
        Tool { bin: "csharpier", label: "csharpier", role: Role::Formatter,
            installers: &[Installer::DotnetTool("csharpier", Some("1.2.6"))] },
        Tool { bin: "netcoredbg", label: "netcoredbg", role: Role::Dap,
            installers: &[Installer::Manual(
                "Build from https://github.com/Samsung/netcoredbg (v3.1.3-1062) — keep libdbgshim.dylib + ManagedPart.dll siblings next to the binary on $PATH.",
            )] },
    ]},
    Bundle { name: "Razor / .cshtml", tools: &[
        Tool { bin: "OmniSharp", label: "OmniSharp (Razor IntelliSense)", role: Role::Lsp,
            installers: &[Installer::Manual(
                "Download the official OmniSharp tarball and unpack to ~/.local/bin/omnisharp/ (binvim probes that path plus $PATH).",
            )] },
        EMMET_LS,
    ]},
    Bundle { name: "Bash / Shell", tools: &[
        Tool { bin: "bash-language-server", label: "bash-language-server", role: Role::Lsp,
            installers: &[Installer::Npm(&["bash-language-server@5.6.0"])] },
        Tool { bin: "shfmt", label: "shfmt", role: Role::Formatter,
            installers: &[Installer::Brew("shfmt"), Installer::Go("mvdan.cc/sh/v3/cmd/shfmt@latest")] },
    ]},
    Bundle { name: "YAML", tools: &[
        Tool { bin: "yaml-language-server", label: "yaml-language-server", role: Role::Lsp,
            installers: &[Installer::Npm(&["yaml-language-server@1.23.0"])] },
        Tool { bin: "prettier", label: "prettier", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier@3.8.3"])] },
    ]},
    Bundle { name: "Lua", tools: &[
        Tool { bin: "lua-language-server", label: "lua-language-server", role: Role::Lsp,
            installers: &[Installer::Brew("lua-language-server")] },
        Tool { bin: "stylua", label: "stylua", role: Role::Formatter,
            installers: &[Installer::Brew("stylua"), Installer::Cargo("stylua", &["--version", "2.5.2"])] },
    ]},
    Bundle { name: "Vue", tools: &[
        Tool { bin: "vue-language-server", label: "vue-language-server", role: Role::Lsp,
            installers: &[Installer::Npm(&["@vue/language-server@3.3.0"])] },
        Tool { bin: "prettier", label: "prettier", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier@3.8.3"])] },
        EMMET_LS,
    ]},
    Bundle { name: "Svelte", tools: &[
        Tool { bin: "svelteserver", label: "svelte-language-server", role: Role::Lsp,
            installers: &[Installer::Npm(&["svelte-language-server@0.18.0"])] },
        // prettier-plugin-svelte stays un-pinned per binvim-web — it has to
        // live in the svelte project's `node_modules` to be discovered by
        // prettier, so the global install is more of a fallback.
        Tool { bin: "prettier", label: "prettier + prettier-plugin-svelte", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier@3.8.3", "prettier-plugin-svelte"])] },
        EMMET_LS,
    ]},
    Bundle { name: "Markdown", tools: &[
        Tool { bin: "marksman", label: "marksman", role: Role::Lsp,
            installers: &[Installer::Brew("marksman")] },
        Tool { bin: "prettier", label: "prettier", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier@3.8.3"])] },
    ]},
    Bundle { name: "TOML", tools: &[
        Tool { bin: "taplo", label: "taplo (LSP + formatter)", role: Role::Lsp,
            installers: &[Installer::Cargo("taplo-cli", &["--version", "0.10.0", "--features", "lsp"])] },
    ]},
    Bundle { name: "Ruby", tools: &[
        Tool { bin: "ruby-lsp", label: "ruby-lsp", role: Role::Lsp,
            installers: &[Installer::Gem("ruby-lsp", Some("0.26.9"))] },
        Tool { bin: "rufo", label: "rufo", role: Role::Formatter,
            installers: &[Installer::Gem("rufo", Some("0.18.2"))] },
    ]},
    Bundle { name: "PHP", tools: &[
        Tool { bin: "intelephense", label: "intelephense", role: Role::Lsp,
            installers: &[Installer::Npm(&["intelephense@1.18.3"])] },
        Tool { bin: "php-cs-fixer", label: "php-cs-fixer", role: Role::Formatter,
            installers: &[Installer::Composer("friendsofphp/php-cs-fixer:3.95.2")] },
    ]},
    Bundle { name: "Java", tools: &[
        Tool { bin: "jdtls", label: "jdtls", role: Role::Lsp,
            installers: &[Installer::Brew("jdtls")] },
        Tool { bin: "google-java-format", label: "google-java-format", role: Role::Formatter,
            installers: &[Installer::Brew("google-java-format")] },
    ]},
    Bundle { name: "Zig", tools: &[
        Tool { bin: "zls", label: "zls", role: Role::Lsp,
            installers: &[Installer::Brew("zls")] },
        Tool { bin: "zig", label: "zig (includes `zig fmt`)", role: Role::Formatter,
            installers: &[Installer::Brew("zig")] },
    ]},
    Bundle { name: "Nix", tools: &[
        Tool { bin: "nil", label: "nil", role: Role::Lsp,
            installers: &[Installer::Nix("nixpkgs#nil")] },
        Tool { bin: "nixfmt", label: "nixfmt-rfc-style", role: Role::Formatter,
            installers: &[Installer::Nix("nixpkgs#nixfmt-rfc-style")] },
    ]},
    Bundle { name: "Elixir", tools: &[
        Tool { bin: "elixir-ls", label: "elixir-ls", role: Role::Lsp,
            installers: &[Installer::Brew("elixir-ls")] },
        Tool { bin: "mix", label: "elixir (includes `mix format`)", role: Role::Formatter,
            installers: &[Installer::Brew("elixir")] },
    ]},
    Bundle { name: "Kotlin", tools: &[
        Tool { bin: "kotlin-language-server", label: "kotlin-language-server", role: Role::Lsp,
            installers: &[Installer::Brew("kotlin-language-server")] },
        Tool { bin: "ktfmt", label: "ktfmt", role: Role::Formatter,
            installers: &[Installer::Brew("ktfmt")] },
    ]},
    Bundle { name: "Docker", tools: &[
        Tool { bin: "docker-langserver", label: "dockerfile-language-server-nodejs", role: Role::Lsp,
            installers: &[Installer::Npm(&["dockerfile-language-server-nodejs@0.15.0"])] },
    ]},
    Bundle { name: "SQL", tools: &[
        Tool { bin: "sqls", label: "sqls", role: Role::Lsp,
            installers: &[Installer::Go("github.com/sqls-server/sqls@v0.2.47")] },
        Tool { bin: "sql-formatter", label: "sql-formatter", role: Role::Formatter,
            installers: &[Installer::Npm(&["sql-formatter@15.8.0"])] },
    ]},
    Bundle { name: "CSS / SCSS / Less", tools: &[
        Tool { bin: "vscode-css-language-server", label: "vscode-langservers-extracted", role: Role::Lsp,
            installers: &[Installer::Npm(&["vscode-langservers-extracted@4.10.0"])] },
        Tool { bin: "prettier", label: "prettier", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier@3.8.3"])] },
        EMMET_LS,
    ]},
    Bundle { name: "HTML", tools: &[
        Tool { bin: "vscode-html-language-server", label: "vscode-langservers-extracted", role: Role::Lsp,
            installers: &[Installer::Npm(&["vscode-langservers-extracted@4.10.0"])] },
        Tool { bin: "prettier", label: "prettier", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier@3.8.3"])] },
        EMMET_LS,
    ]},
    Bundle { name: "Tailwind (aux)", tools: &[
        Tool { bin: "tailwindcss-language-server", label: "tailwindcss-language-server", role: Role::Lsp,
            installers: &[Installer::Npm(&["@tailwindcss/language-server@0.14.29"])] },
    ]},
    Bundle { name: "Astro", tools: &[
        Tool { bin: "astro-ls", label: "@astrojs/language-server", role: Role::Lsp,
            installers: &[Installer::Npm(&["@astrojs/language-server@2.16.9"])] },
        Tool { bin: "prettier", label: "prettier", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier@3.8.3"])] },
        EMMET_LS,
    ]},
    Bundle { name: "GitHub Copilot", tools: &[
        Tool { bin: "copilot-language-server", label: "copilot-language-server", role: Role::Lsp,
            installers: &[Installer::Npm(&["@github/copilot-language-server@1.487.0"])] },
    ]},
    Bundle { name: "ripgrep (live grep)", tools: &[
        Tool { bin: "rg", label: "ripgrep", role: Role::Tool,
            installers: &[Installer::Brew("ripgrep"), Installer::Apt("ripgrep"),
                          Installer::Cargo("ripgrep", &[])] },
    ]},
    Bundle { name: "lazygit (git UI)", tools: &[
        Tool { bin: "lazygit", label: "lazygit", role: Role::Tool,
            installers: &[Installer::Brew("lazygit"),
                          Installer::Go("github.com/jesseduffield/lazygit@latest")] },
    ]},
    Bundle { name: "yazi (file picker)", tools: &[
        Tool { bin: "yazi", label: "yazi", role: Role::Tool,
            installers: &[Installer::Brew("yazi"), Installer::Cargo("yazi-fm", &[])] },
    ]},
];

pub fn bundle_summary(b: &Bundle) -> String {
    b.tools
        .iter()
        .map(|t| t.label)
        .collect::<Vec<_>>()
        .join(" · ")
}

// ─── PATH / package-manager detection ──────────────────────────────────────

pub fn on_path(name: &str) -> bool {
    crate::paths::on_path(name)
}

pub fn find_on_path(name: &str) -> Option<PathBuf> {
    crate::paths::find_on_path(name)
}

pub fn detect_managers() -> BTreeSet<&'static str> {
    let candidates = [
        "brew", "apt-get", "npm", "cargo", "rustup", "go", "pipx", "pip", "gem", "dotnet", "nix",
        "composer", "sudo",
    ];
    candidates.into_iter().filter(|c| on_path(c)).collect()
}

pub fn pick_installer<'a>(
    tool: &'a Tool,
    managers: &BTreeSet<&'static str>,
) -> Option<&'a Installer> {
    tool.installers.iter().find(|inst| match inst {
        Installer::Manual(_) => false,
        Installer::Apt(_) => managers.contains("apt-get") && managers.contains("sudo"),
        other => managers.contains(other.manager()),
    })
}

// ─── Node.js version discovery ─────────────────────────────────────────────

/// One Node.js install we can drive `npm install -g` against. We discover
/// these by scanning the well-known directories used by nvm / fnm / asdf /
/// mise / volta / n, plus the system `npm` on `$PATH`. `bin_dir` is
/// prepended to `PATH` when invoking npm so its `#!/usr/bin/env node`
/// shebang resolves to the matching node binary rather than whatever the
/// host shell's PATH would pick.
#[derive(Clone, Debug)]
pub struct NodeVersion {
    pub label: String,
    pub npm_path: PathBuf,
    pub bin_dir: PathBuf,
    /// `(major, minor, patch)` for sorting; `(0, 0, 0)` if unparseable.
    pub sort_key: (u32, u32, u32),
}

pub fn discover_node_versions() -> Vec<NodeVersion> {
    let mut out: Vec<NodeVersion> = Vec::new();
    let home = crate::paths::home_dir();

    if let Some(h) = home.as_ref() {
        scan_node_root(
            &h.join(".nvm").join("versions").join("node"),
            Path::new("bin/npm"),
            "nvm",
            &mut out,
        );
        scan_node_root(
            &h.join(".local")
                .join("share")
                .join("fnm")
                .join("node-versions"),
            Path::new("installation/bin/npm"),
            "fnm",
            &mut out,
        );
        scan_node_root(
            &h.join(".fnm").join("node-versions"),
            Path::new("installation/bin/npm"),
            "fnm",
            &mut out,
        );
        scan_node_root(
            &h.join(".asdf").join("installs").join("nodejs"),
            Path::new("bin/npm"),
            "asdf",
            &mut out,
        );
        scan_node_root(
            &h.join(".local")
                .join("share")
                .join("mise")
                .join("installs")
                .join("node"),
            Path::new("bin/npm"),
            "mise",
            &mut out,
        );
        scan_node_root(
            &h.join(".volta").join("tools").join("image").join("node"),
            Path::new("bin/npm"),
            "volta",
            &mut out,
        );
    }
    // `n` always installs under `/usr/local/n/...`; there's no Windows
    // equivalent and the path doesn't make sense there, so skip it.
    #[cfg(unix)]
    scan_node_root(
        Path::new("/usr/local/n/versions/node"),
        Path::new("bin/npm"),
        "n",
        &mut out,
    );

    if let Some(system) = find_on_path("npm") {
        let canonical = std::fs::canonicalize(&system).unwrap_or_else(|_| system.clone());
        let already_listed = out.iter().any(|v| {
            std::fs::canonicalize(&v.npm_path).unwrap_or_else(|_| v.npm_path.clone()) == canonical
        });
        if !already_listed {
            let bin_dir = canonical
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            let (label, sort_key) = label_system_npm(&bin_dir);
            out.push(NodeVersion {
                label,
                npm_path: canonical,
                bin_dir,
                sort_key,
            });
        }
    }

    out.sort_by_key(|v| std::cmp::Reverse(v.sort_key));
    out
}

fn scan_node_root(root: &Path, suffix: &Path, manager: &str, out: &mut Vec<NodeVersion>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let npm_path = dir.join(suffix);
        if !npm_path.is_file() {
            continue;
        }
        let bin_dir = npm_path.parent().map(Path::to_path_buf).unwrap_or_default();
        let raw = entry.file_name().to_string_lossy().into_owned();
        let sort_key = parse_node_version(&raw).unwrap_or((0, 0, 0));
        out.push(NodeVersion {
            label: format!("{raw}  [{manager}]"),
            npm_path,
            bin_dir,
            sort_key,
        });
    }
}

pub fn parse_node_version(name: &str) -> Option<(u32, u32, u32)> {
    let trimmed = name.trim_start_matches('v');
    let mut parts = trimmed.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    let raw_patch = parts.next()?;
    let patch_digits: String = raw_patch
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let patch: u32 = patch_digits.parse().ok()?;
    Some((major, minor, patch))
}

fn label_system_npm(bin_dir: &Path) -> (String, (u32, u32, u32)) {
    let node = bin_dir.join("node");
    let version = Command::new(&node)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    match version {
        Some(v) if !v.is_empty() => {
            let key = parse_node_version(&v).unwrap_or((0, 0, 0));
            (format!("{v}  [system]"), key)
        }
        _ => ("system npm  [system]".to_string(), (0, 0, 0)),
    }
}

// ─── plan ──────────────────────────────────────────────────────────────────

pub struct PlanItem {
    pub tool: &'static Tool,
    pub used_by: Vec<&'static str>,
    pub chosen: Choice,
}

pub enum Choice {
    /// Already on `$PATH`. Skipped during run.
    Already,
    Install(&'static Installer),
    /// Caller should print the message to the user — we have no automatic
    /// install path (netcoredbg, OmniSharp).
    Manual(&'static str),
    /// We had non-manual installers but none of their package managers are
    /// available. The strings are the install commands we *would* have run
    /// — useful for telling the user "install brew first" or similar.
    NoManager(Vec<String>),
}

pub fn build_plan(selected: &[usize], managers: &BTreeSet<&'static str>) -> Vec<PlanItem> {
    let mut by_bin: BTreeMap<&'static str, (Tool, Vec<&'static str>)> = BTreeMap::new();
    for &idx in selected {
        let bundle = &BUNDLES[idx];
        for tool in bundle.tools {
            by_bin
                .entry(tool.bin)
                .and_modify(|(_, names)| names.push(bundle.name))
                .or_insert_with(|| (*tool, vec![bundle.name]));
        }
    }

    let mut plan = Vec::new();
    for (_, (tool_copy, used_by)) in by_bin {
        let tool: &'static Tool = find_static_tool(tool_copy.bin).expect("tool came from BUNDLES");
        // For npm installs we ignore the on-PATH shortcut: the binary on
        // PATH belongs to exactly one Node version, but the user may have
        // asked us to target other versions too. Always run the npm
        // installer for those — npm will say "up to date" if the package
        // is already there. Non-npm installers keep the original
        // skip-when-already-on-PATH behaviour.
        let installer = pick_installer(tool, managers);
        let chosen = match installer {
            Some(inst @ Installer::Npm(_)) => Choice::Install(inst),
            _ if on_path(tool.bin) => Choice::Already,
            Some(inst) => Choice::Install(inst),
            None => {
                if let Some(Installer::Manual(s)) = tool.installers.first() {
                    Choice::Manual(s)
                } else {
                    let missing: Vec<String> = tool
                        .installers
                        .iter()
                        .filter(|i| !matches!(i, Installer::Manual(_)))
                        .map(|i| i.display())
                        .collect();
                    Choice::NoManager(missing)
                }
            }
        };
        plan.push(PlanItem {
            tool,
            used_by,
            chosen,
        });
    }
    plan.sort_by_key(|p| {
        let role_rank = match p.tool.role {
            Role::Lsp => 0,
            Role::Formatter => 1,
            Role::Dap => 2,
            Role::Tool => 3,
        };
        (role_rank, p.tool.label)
    });
    plan
}

pub fn find_static_tool(bin: &str) -> Option<&'static Tool> {
    for b in BUNDLES {
        for t in b.tools {
            if t.bin == bin {
                return Some(t);
            }
        }
    }
    None
}

/// True iff the plan contains at least one `npm install -g` step, i.e. the
/// user needs to be prompted for which Node.js installation(s) to target.
pub fn plan_needs_node(plan: &[PlanItem]) -> bool {
    plan.iter()
        .any(|p| matches!(p.chosen, Choice::Install(Installer::Npm(_))))
}

// ─── run ───────────────────────────────────────────────────────────────────

pub struct Summary {
    pub installed: usize,
    pub skipped: usize,
    pub manual: usize,
    pub failed: Vec<(String, String)>,
}

/// Shell out to each plan item's installer, looping over the chosen Node
/// versions for `Installer::Npm` steps. Stdio is inherited — the caller
/// (CLI or editor takeover) is responsible for having relinquished the
/// terminal first.
pub fn run_plan(plan: &[PlanItem], node_versions: &[NodeVersion]) -> Summary {
    use std::io::Write;
    let mut summary = Summary {
        installed: 0,
        skipped: 0,
        manual: 0,
        failed: Vec::new(),
    };
    let mut stdout = std::io::stdout();
    for item in plan {
        match &item.chosen {
            Choice::Already => {
                summary.skipped += 1;
            }
            Choice::Manual(_) => {
                summary.manual += 1;
            }
            Choice::NoManager(_) => {
                summary.failed.push((
                    item.tool.label.to_string(),
                    "no package manager available".into(),
                ));
            }
            Choice::Install(Installer::Npm(pkgs)) => {
                if node_versions.is_empty() {
                    summary.failed.push((
                        item.tool.label.to_string(),
                        "no Node.js version selected".into(),
                    ));
                    continue;
                }
                for v in node_versions {
                    let _ = writeln!(
                        stdout,
                        "\n→ {} — npm install -g {}  (for {})",
                        item.tool.label,
                        pkgs.join(" "),
                        v.label
                    );
                    let mut cmd = Command::new(&v.npm_path);
                    cmd.args(["install", "-g"]);
                    cmd.args(pkgs.iter().copied());
                    let host_path = std::env::var_os("PATH").unwrap_or_default();
                    let mut paths: Vec<PathBuf> = vec![v.bin_dir.clone()];
                    paths.extend(std::env::split_paths(&host_path));
                    if let Ok(joined) = std::env::join_paths(paths) {
                        cmd.env("PATH", joined);
                    }
                    cmd.stdin(Stdio::inherit());
                    cmd.stdout(Stdio::inherit());
                    cmd.stderr(Stdio::inherit());
                    let label = format!("{} (for {})", item.tool.label, v.label);
                    match cmd.status() {
                        Ok(s) if s.success() => {
                            summary.installed += 1;
                            let _ = writeln!(stdout, "✓ installed");
                        }
                        Ok(s) => {
                            let msg = format!("exit code {}", s.code().unwrap_or(-1));
                            let _ = writeln!(stdout, "✗ failed ({msg})");
                            summary.failed.push((label, msg));
                        }
                        Err(e) => {
                            let msg = format!("spawn error: {e}");
                            let _ = writeln!(stdout, "✗ {msg}");
                            summary.failed.push((label, msg));
                        }
                    }
                }
            }
            Choice::Install(inst) => {
                let _ = writeln!(stdout, "\n→ {} — {}", item.tool.label, inst.display());
                let Some(mut cmd) = inst.build_command() else {
                    summary.manual += 1;
                    continue;
                };
                match cmd.status() {
                    Ok(s) if s.success() => {
                        summary.installed += 1;
                        let _ = writeln!(stdout, "✓ installed");
                    }
                    Ok(s) => {
                        let msg = format!("exit code {}", s.code().unwrap_or(-1));
                        let _ = writeln!(stdout, "✗ failed ({msg})");
                        summary.failed.push((item.tool.label.to_string(), msg));
                    }
                    Err(e) => {
                        let msg = format!("spawn error: {e}");
                        let _ = writeln!(stdout, "✗ {msg}");
                        summary.failed.push((item.tool.label.to_string(), msg));
                    }
                }
            }
        }
    }
    summary
}
