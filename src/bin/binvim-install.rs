//! `binvim-install` — interactive installer for the LSPs, formatters, and DAP
//! adapters binvim drives. The main binary is intentionally feature-detecting
//! at runtime (any missing tool is silently skipped); this helper exists so a
//! fresh install doesn't have to read the README to know what to install.
//!
//! Flow: ASCII banner → multi-select language checkbox UI → dedupe tools
//! across the selection → if any `npm install -g` is in the plan, scan for
//! installed Node.js versions across nvm / fnm / asdf / mise / volta / n
//! (plus the system `npm` on `$PATH`) and let the user pick which to target
//! → confirm plan → shell out to each tool's installer, picked from the
//! first runnable candidate (brew / apt / npm / cargo / rustup / go / pipx
//! / gem / dotnet / nix / composer). npm steps loop over every chosen Node
//! version, with the version's bin/ prepended to PATH so the npm shebang
//! resolves to the matching node binary. Finally a summary.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Write, stdout};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Result, anyhow};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode};
use crossterm::{execute, queue};

// ─── banner ────────────────────────────────────────────────────────────────

const BANNER: &[&str] = &[
    "██████╗ ██╗███╗   ██╗██╗   ██╗██╗███╗   ███╗",
    "██╔══██╗██║████╗  ██║██║   ██║██║████╗ ████║",
    "██████╔╝██║██╔██╗ ██║██║   ██║██║██╔████╔██║",
    "██╔══██╗██║██║╚██╗██║╚██╗ ██╔╝██║██║╚██╔╝██║",
    "██████╔╝██║██║ ╚████║ ╚████╔╝ ██║██║ ╚═╝ ██║",
    "╚═════╝ ╚═╝╚═╝  ╚═══╝  ╚═══╝  ╚═╝╚═╝     ╚═╝",
];

// Catppuccin Mocha — keep in sync with the editor's palette.
const MAUVE: Color = Color::Rgb {
    r: 203,
    g: 166,
    b: 247,
};
const TEAL: Color = Color::Rgb {
    r: 148,
    g: 226,
    b: 213,
};
const GREEN: Color = Color::Rgb {
    r: 166,
    g: 227,
    b: 161,
};
const RED: Color = Color::Rgb {
    r: 243,
    g: 139,
    b: 168,
};
const YELLOW: Color = Color::Rgb {
    r: 249,
    g: 226,
    b: 175,
};
const SUBTLE: Color = Color::Rgb {
    r: 108,
    g: 112,
    b: 134,
};
const ACCENT: Color = Color::Rgb {
    r: 250,
    g: 179,
    b: 135,
};

// ─── catalog ───────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
enum Role {
    Lsp,
    Formatter,
    Dap,
    /// Editor-wide utility that isn't tied to a single language —
    /// `rg` (live grep), `lazygit` (git UI takeover), `yazi` (file picker).
    Tool,
}

impl Role {
    fn tag(self) -> &'static str {
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
struct Tool {
    bin: &'static str,
    label: &'static str,
    role: Role,
    installers: &'static [Installer],
}

#[derive(Copy, Clone)]
enum Installer {
    Brew(&'static str),
    Apt(&'static str),
    Npm(&'static [&'static str]),
    Cargo(&'static str, &'static [&'static str]),
    Rustup(&'static str),
    Go(&'static str),
    Pipx(&'static str),
    Pip(&'static str),
    Gem(&'static str),
    DotnetTool(&'static str),
    Nix(&'static str),
    Composer(&'static str),
    Manual(&'static str),
}

impl Installer {
    fn manager(&self) -> &'static str {
        match self {
            Installer::Brew(_) => "brew",
            Installer::Apt(_) => "apt-get",
            Installer::Npm(_) => "npm",
            Installer::Cargo(_, _) => "cargo",
            Installer::Rustup(_) => "rustup",
            Installer::Go(_) => "go",
            Installer::Pipx(_) => "pipx",
            Installer::Pip(_) => "pip",
            Installer::Gem(_) => "gem",
            Installer::DotnetTool(_) => "dotnet",
            Installer::Nix(_) => "nix",
            Installer::Composer(_) => "composer",
            Installer::Manual(_) => "",
        }
    }

    fn display(&self) -> String {
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
            Installer::Gem(p) => format!("gem install {p}"),
            Installer::DotnetTool(p) => format!("dotnet tool install --global {p}"),
            Installer::Nix(r) => format!("nix profile install {r}"),
            Installer::Composer(p) => format!("composer global require {p}"),
            Installer::Manual(s) => format!("manual: {s}"),
        }
    }

    /// Build the `Command` we will spawn. `None` for `Manual` — caller prints
    /// the instructions and moves on.
    fn build_command(&self) -> Option<Command> {
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
            Installer::Npm(pkgs) => {
                let mut c = Command::new("npm");
                c.args(["install", "-g"]);
                c.args(pkgs.iter().copied());
                c
            }
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
            Installer::Gem(p) => {
                let mut c = Command::new("gem");
                c.args(["install", p]);
                c
            }
            Installer::DotnetTool(p) => {
                let mut c = Command::new("dotnet");
                c.args(["tool", "install", "--global", p]);
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

struct Bundle {
    name: &'static str,
    tools: &'static [Tool],
}

/// `emmet-ls` attaches to every markup-flavoured buffer binvim recognises
/// (HTML, CSS, JSX/TSX, Vue, Svelte, Astro, Razor — see `README.md:75`).
/// Declared once and folded into each of those bundles so picking any one
/// of them installs Emmet, and the dedupe in `build_plan` collapses the
/// install to a single npm run no matter how many you picked.
const EMMET_LS: Tool = Tool {
    bin: "emmet-ls",
    label: "emmet-ls",
    role: Role::Lsp,
    installers: &[Installer::Npm(&["emmet-ls"])],
};

/// The catalog. Mirrors the README install table at `README.md:283+`. When a
/// tool appears under multiple languages (prettier, lldb-dap, vscode-
/// langservers-extracted, biome, EMMET_LS, …) it's repeated literally —
/// `unique_tools` dedupes by `bin` at plan time.
#[rustfmt::skip]
const BUNDLES: &[Bundle] = &[
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
            installers: &[Installer::Npm(&["typescript-language-server", "typescript"])] },
        Tool { bin: "biome", label: "biome", role: Role::Formatter,
            installers: &[Installer::Npm(&["@biomejs/biome"])] },
        Tool { bin: "prettier", label: "prettier (fallback formatter)", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier"])] },
        EMMET_LS,
    ]},
    Bundle { name: "Go", tools: &[
        Tool { bin: "gopls", label: "gopls", role: Role::Lsp,
            installers: &[Installer::Go("golang.org/x/tools/gopls@latest")] },
        Tool { bin: "goimports", label: "goimports", role: Role::Formatter,
            installers: &[Installer::Go("golang.org/x/tools/cmd/goimports@latest")] },
        Tool { bin: "dlv", label: "delve (dlv)", role: Role::Dap,
            installers: &[Installer::Go("github.com/go-delve/delve/cmd/dlv@latest")] },
    ]},
    Bundle { name: "Python", tools: &[
        Tool { bin: "pyright-langserver", label: "pyright", role: Role::Lsp,
            installers: &[Installer::Npm(&["pyright"])] },
        Tool { bin: "ruff", label: "ruff", role: Role::Formatter,
            installers: &[Installer::Pipx("ruff")] },
        // debugpy has no binary on PATH — we probe `python3 -m debugpy.adapter`.
        // Use the sentinel `python3-debugpy` so the PATH check naturally fails
        // and the install runs; the installer itself drops it into the user's
        // site-packages. Acceptable false-negative on re-runs (we'll attempt
        // to install again, pip will say "already satisfied").
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
            installers: &[Installer::DotnetTool("csharp-ls")] },
        Tool { bin: "csharpier", label: "csharpier", role: Role::Formatter,
            installers: &[Installer::DotnetTool("csharpier")] },
        Tool { bin: "netcoredbg", label: "netcoredbg", role: Role::Dap,
            installers: &[Installer::Manual(
                "Build from https://github.com/Samsung/netcoredbg — keep libdbgshim.dylib + ManagedPart.dll siblings next to the binary on $PATH.",
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
            installers: &[Installer::Npm(&["bash-language-server"])] },
        Tool { bin: "shfmt", label: "shfmt", role: Role::Formatter,
            installers: &[Installer::Brew("shfmt"), Installer::Go("mvdan.cc/sh/v3/cmd/shfmt@latest")] },
    ]},
    Bundle { name: "YAML", tools: &[
        Tool { bin: "yaml-language-server", label: "yaml-language-server", role: Role::Lsp,
            installers: &[Installer::Npm(&["yaml-language-server"])] },
        Tool { bin: "prettier", label: "prettier", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier"])] },
    ]},
    Bundle { name: "Lua", tools: &[
        Tool { bin: "lua-language-server", label: "lua-language-server", role: Role::Lsp,
            installers: &[Installer::Brew("lua-language-server")] },
        Tool { bin: "stylua", label: "stylua", role: Role::Formatter,
            installers: &[Installer::Brew("stylua"), Installer::Cargo("stylua", &[])] },
    ]},
    Bundle { name: "Vue", tools: &[
        Tool { bin: "vue-language-server", label: "vue-language-server", role: Role::Lsp,
            installers: &[Installer::Npm(&["@vue/language-server"])] },
        Tool { bin: "prettier", label: "prettier", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier"])] },
        EMMET_LS,
    ]},
    Bundle { name: "Svelte", tools: &[
        Tool { bin: "svelteserver", label: "svelte-language-server", role: Role::Lsp,
            installers: &[Installer::Npm(&["svelte-language-server"])] },
        Tool { bin: "prettier", label: "prettier + prettier-plugin-svelte", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier", "prettier-plugin-svelte"])] },
        EMMET_LS,
    ]},
    Bundle { name: "Markdown", tools: &[
        Tool { bin: "marksman", label: "marksman", role: Role::Lsp,
            installers: &[Installer::Brew("marksman")] },
        Tool { bin: "prettier", label: "prettier", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier"])] },
    ]},
    Bundle { name: "TOML", tools: &[
        Tool { bin: "taplo", label: "taplo (LSP + formatter)", role: Role::Lsp,
            installers: &[Installer::Cargo("taplo-cli", &["--features", "lsp"])] },
    ]},
    Bundle { name: "Ruby", tools: &[
        Tool { bin: "ruby-lsp", label: "ruby-lsp", role: Role::Lsp,
            installers: &[Installer::Gem("ruby-lsp")] },
        Tool { bin: "rufo", label: "rufo", role: Role::Formatter,
            installers: &[Installer::Gem("rufo")] },
    ]},
    Bundle { name: "PHP", tools: &[
        Tool { bin: "intelephense", label: "intelephense", role: Role::Lsp,
            installers: &[Installer::Npm(&["intelephense"])] },
        Tool { bin: "php-cs-fixer", label: "php-cs-fixer", role: Role::Formatter,
            installers: &[Installer::Composer("friendsofphp/php-cs-fixer")] },
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
            installers: &[Installer::Npm(&["dockerfile-language-server-nodejs"])] },
    ]},
    Bundle { name: "SQL", tools: &[
        Tool { bin: "sqls", label: "sqls", role: Role::Lsp,
            installers: &[Installer::Go("github.com/sqls-server/sqls@latest")] },
        Tool { bin: "sql-formatter", label: "sql-formatter", role: Role::Formatter,
            installers: &[Installer::Npm(&["sql-formatter"])] },
    ]},
    Bundle { name: "CSS / SCSS / Less", tools: &[
        Tool { bin: "vscode-css-language-server", label: "vscode-langservers-extracted", role: Role::Lsp,
            installers: &[Installer::Npm(&["vscode-langservers-extracted"])] },
        Tool { bin: "prettier", label: "prettier", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier"])] },
        EMMET_LS,
    ]},
    Bundle { name: "HTML", tools: &[
        Tool { bin: "vscode-html-language-server", label: "vscode-langservers-extracted", role: Role::Lsp,
            installers: &[Installer::Npm(&["vscode-langservers-extracted"])] },
        Tool { bin: "prettier", label: "prettier", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier"])] },
        EMMET_LS,
    ]},
    Bundle { name: "Tailwind (aux)", tools: &[
        Tool { bin: "tailwindcss-language-server", label: "tailwindcss-language-server", role: Role::Lsp,
            installers: &[Installer::Npm(&["@tailwindcss/language-server"])] },
    ]},
    Bundle { name: "Astro", tools: &[
        Tool { bin: "astro-ls", label: "@astrojs/language-server", role: Role::Lsp,
            installers: &[Installer::Npm(&["@astrojs/language-server"])] },
        Tool { bin: "prettier", label: "prettier", role: Role::Formatter,
            installers: &[Installer::Npm(&["prettier"])] },
        EMMET_LS,
    ]},
    // GitHub Copilot is an LSP that attaches universally (not language-
    // specific). Opt-in via `[copilot] enabled = true` in
    // ~/.config/binvim/config.toml — see `README.md` §Configuration.
    Bundle { name: "GitHub Copilot", tools: &[
        Tool { bin: "copilot-language-server", label: "copilot-language-server", role: Role::Lsp,
            installers: &[Installer::Npm(&["@github/copilot-language-server"])] },
    ]},
    // Editor-wide utilities that aren't tied to a language. `rg` powers
    // live grep (`<space>G`); `lazygit` is the suspend-takeover for
    // `<leader>gg`; `yazi` is the `<space>e` file picker (the built-in
    // sidebar tree is the alternative — see `[file_explorer] tree`).
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

// ─── PATH / package-manager detection ──────────────────────────────────────

fn on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else { return false };
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return true;
        }
        // Windows would need .exe / .cmd suffix probing; binvim is POSIX-only
        // for now (see ROADMAP "Windows" item), so don't bother.
    }
    false
}

fn detect_managers() -> BTreeSet<&'static str> {
    let candidates = [
        "brew", "apt-get", "npm", "cargo", "rustup", "go", "pipx", "pip", "gem", "dotnet", "nix",
        "composer", "sudo",
    ];
    candidates.into_iter().filter(|c| on_path(c)).collect()
}

fn pick_installer<'a>(tool: &'a Tool, managers: &BTreeSet<&'static str>) -> Option<&'a Installer> {
    tool.installers.iter().find(|inst| match inst {
        Installer::Manual(_) => false,
        Installer::Apt(_) => managers.contains("apt-get") && managers.contains("sudo"),
        other => managers.contains(other.manager()),
    })
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let p = dir.join(name);
        p.is_file().then_some(p)
    })
}

// ─── Node.js version discovery ─────────────────────────────────────────────

/// One Node.js install we can drive `npm install -g` against. We discover
/// these by scanning the well-known directories used by nvm / fnm / asdf /
/// mise / volta / n, plus the system `npm` on `$PATH`. `bin_dir` is
/// prepended to `PATH` when invoking npm so its `#!/usr/bin/env node`
/// shebang resolves to the matching node binary rather than whatever the
/// host shell's PATH would pick.
#[derive(Clone)]
struct NodeVersion {
    label: String,
    npm_path: PathBuf,
    bin_dir: PathBuf,
    /// `(major, minor, patch)` for sorting; `(0, 0, 0)` if unparseable.
    sort_key: (u32, u32, u32),
}

fn discover_node_versions() -> Vec<NodeVersion> {
    let mut out: Vec<NodeVersion> = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);

    if let Some(h) = home.as_ref() {
        // nvm
        scan_node_root(
            &h.join(".nvm").join("versions").join("node"),
            Path::new("bin/npm"),
            "nvm",
            &mut out,
        );
        // fnm (XDG default)
        scan_node_root(
            &h.join(".local")
                .join("share")
                .join("fnm")
                .join("node-versions"),
            Path::new("installation/bin/npm"),
            "fnm",
            &mut out,
        );
        // fnm (legacy / custom dir)
        scan_node_root(
            &h.join(".fnm").join("node-versions"),
            Path::new("installation/bin/npm"),
            "fnm",
            &mut out,
        );
        // asdf
        scan_node_root(
            &h.join(".asdf").join("installs").join("nodejs"),
            Path::new("bin/npm"),
            "asdf",
            &mut out,
        );
        // mise
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
        // volta
        scan_node_root(
            &h.join(".volta").join("tools").join("image").join("node"),
            Path::new("bin/npm"),
            "volta",
            &mut out,
        );
    }
    // n
    scan_node_root(
        Path::new("/usr/local/n/versions/node"),
        Path::new("bin/npm"),
        "n",
        &mut out,
    );

    // Add the system `npm` on PATH if it isn't already represented by one of
    // the scans above. De-duped via canonicalize — nvm / fnm shim a symlink
    // into the user's PATH, so we'd otherwise list the same Node twice.
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

    // Newest first.
    out.sort_by_key(|v| std::cmp::Reverse(v.sort_key));
    out
}

fn scan_node_root(root: &Path, suffix: &Path, manager: &str, out: &mut Vec<NodeVersion>) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
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

fn parse_node_version(name: &str) -> Option<(u32, u32, u32)> {
    let trimmed = name.trim_start_matches('v');
    let mut parts = trimmed.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    // patch may carry a pre-release suffix ("0-rc.1") — strip non-digits.
    let raw_patch = parts.next()?;
    let patch_digits: String = raw_patch
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let patch: u32 = patch_digits.parse().ok()?;
    Some((major, minor, patch))
}

/// Shell out to `<bin_dir>/node --version` so the picker shows the actual
/// version string of the system Node (`v20.10.0  [system]`) rather than a
/// generic "system" label. Falls back to `(0, 0, 0)` on parse failure so it
/// sorts to the bottom rather than the top.
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

// ─── checkbox UI ───────────────────────────────────────────────────────────

struct PickerState {
    cursor: usize,
    checked: Vec<bool>,
}

/// Generic multi-select checkbox prompt — drives both the language bundle
/// picker and the Node-version picker. Each `(name, summary)` pair becomes
/// one row; `default_checked` are pre-selected indices so `Enter` alone
/// picks a sensible default for the Node picker (newest version).
fn run_multi_select(
    subtitle: &str,
    items: &[(String, String)],
    default_checked: &[usize],
) -> Result<Option<Vec<usize>>> {
    if !stdout().is_terminal() {
        return Err(anyhow!(
            "binvim-install needs a TTY for the checkbox UI — run it directly in a terminal."
        ));
    }
    if items.is_empty() {
        return Ok(Some(Vec::new()));
    }

    let mut state = PickerState {
        cursor: 0,
        checked: vec![false; items.len()],
    };
    for &i in default_checked {
        if let Some(slot) = state.checked.get_mut(i) {
            *slot = true;
        }
    }
    let name_width = items
        .iter()
        .map(|(n, _)| n.chars().count())
        .max()
        .unwrap_or(0)
        .max(20);

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, Hide)?;

    let result = (|| -> Result<Option<Vec<usize>>> {
        loop {
            render_list(&mut out, &state, items, subtitle, name_width)?;
            let Event::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            }) = event::read()?
            else {
                continue;
            };
            if kind == KeyEventKind::Release {
                continue;
            }
            match (code, modifiers) {
                (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => return Ok(None),
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(None),
                (KeyCode::Char('j'), _) | (KeyCode::Down, _) => {
                    if state.cursor + 1 < items.len() {
                        state.cursor += 1;
                    }
                }
                (KeyCode::Char('k'), _) | (KeyCode::Up, _) => {
                    state.cursor = state.cursor.saturating_sub(1);
                }
                (KeyCode::Char('g'), _) | (KeyCode::Home, _) => state.cursor = 0,
                (KeyCode::Char('G'), _) | (KeyCode::End, _) => {
                    state.cursor = items.len().saturating_sub(1);
                }
                (KeyCode::Char(' '), _) => {
                    let c = &mut state.checked[state.cursor];
                    *c = !*c;
                }
                (KeyCode::Char('a'), _) => state.checked.iter_mut().for_each(|c| *c = true),
                (KeyCode::Char('n'), _) => state.checked.iter_mut().for_each(|c| *c = false),
                (KeyCode::Enter, _) => {
                    let picks: Vec<usize> = state
                        .checked
                        .iter()
                        .enumerate()
                        .filter_map(|(i, &c)| if c { Some(i) } else { None })
                        .collect();
                    return Ok(Some(picks));
                }
                _ => {}
            }
        }
    })();

    execute!(out, Show)?;
    disable_raw_mode()?;
    println!();
    result
}

fn render_list(
    out: &mut impl Write,
    state: &PickerState,
    items: &[(String, String)],
    subtitle: &str,
    name_width: usize,
) -> Result<()> {
    queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;

    queue!(
        out,
        SetForegroundColor(MAUVE),
        SetAttribute(Attribute::Bold)
    )?;
    for (i, line) in BANNER.iter().enumerate() {
        queue!(out, MoveTo(0, i as u16), Print(line))?;
    }
    queue!(
        out,
        SetAttribute(Attribute::Reset),
        SetForegroundColor(SUBTLE)
    )?;
    queue!(
        out,
        MoveTo(0, BANNER.len() as u16),
        Print(format!("  {subtitle}"))
    )?;
    queue!(out, ResetColor)?;

    let help_row = (BANNER.len() + 2) as u16;
    queue!(out, MoveTo(0, help_row), SetForegroundColor(SUBTLE))?;
    queue!(
        out,
        Print("  j/k move · space toggle · a all · n none · Enter confirm · q quit")
    )?;
    queue!(out, ResetColor)?;

    let list_top = help_row + 2;
    for (i, (name, summary)) in items.iter().enumerate() {
        let row = list_top + i as u16;
        let active = i == state.cursor;
        let checked = state.checked[i];
        queue!(out, MoveTo(0, row))?;
        if active {
            queue!(
                out,
                SetForegroundColor(ACCENT),
                SetAttribute(Attribute::Bold),
                Print("▸ ")
            )?;
        } else {
            queue!(out, Print("  "))?;
        }
        let mark = if checked { "[x]" } else { "[ ]" };
        let mark_color = if checked { GREEN } else { SUBTLE };
        queue!(out, SetForegroundColor(mark_color), Print(mark), Print(" "))?;
        if active {
            queue!(
                out,
                SetForegroundColor(ACCENT),
                SetAttribute(Attribute::Bold)
            )?;
        } else {
            queue!(out, ResetColor)?;
        }
        queue!(out, Print(format!("{:<width$}", name, width = name_width)))?;
        queue!(
            out,
            SetAttribute(Attribute::Reset),
            SetForegroundColor(SUBTLE)
        )?;
        queue!(out, Print(format!("  {summary}")))?;
        queue!(out, ResetColor)?;
    }

    out.flush()?;
    Ok(())
}

fn pick_bundles() -> Result<Option<Vec<usize>>> {
    let items: Vec<(String, String)> = BUNDLES
        .iter()
        .map(|b| (b.name.to_string(), bundle_summary(b)))
        .collect();
    run_multi_select("install — pick the languages you want set up", &items, &[])
}

fn pick_node_versions(versions: &[NodeVersion]) -> Result<Option<Vec<usize>>> {
    let items: Vec<(String, String)> = versions
        .iter()
        .map(|v| (v.label.clone(), v.npm_path.display().to_string()))
        .collect();
    // Default-check the first (newest) so Enter alone picks a sane target.
    run_multi_select(
        "npm packages — pick which Node.js installations to install for",
        &items,
        &[0],
    )
}

fn bundle_summary(b: &Bundle) -> String {
    b.tools
        .iter()
        .map(|t| t.label)
        .collect::<Vec<_>>()
        .join(" · ")
}

// ─── plan + run ────────────────────────────────────────────────────────────

struct PlanItem {
    tool: &'static Tool,
    used_by: Vec<&'static str>,
    chosen: Choice,
}

enum Choice {
    Already, // already on PATH
    Install(&'static Installer),
    Manual(&'static str),
    NoManager(Vec<String>), // we have a non-manual installer list but no PM available
}

fn build_plan(selected: &[usize], managers: &BTreeSet<&'static str>) -> Vec<PlanItem> {
    // Dedupe by `bin` while collecting the union of "which bundles wanted it".
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
        // Re-resolve against the catalog so every reference we hold from here
        // on (Tool, Installer) is 'static — PlanItem stores 'static refs.
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
    // Stable display order: by role (LSP first, then formatter, then DAP,
    // then editor-wide tools), then by label.
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

/// Re-resolve a tool by `bin` against the catalog so we get a `&'static Tool`
/// (the iteration above gave us owned `Tool` copies through `by_bin`).
fn find_static_tool(bin: &str) -> Option<&'static Tool> {
    for b in BUNDLES {
        for t in b.tools {
            if t.bin == bin {
                return Some(t);
            }
        }
    }
    None
}

fn print_plan(plan: &[PlanItem], node_versions: &[NodeVersion]) {
    println!();
    println!("Plan:");
    println!();
    for item in plan {
        let used = item.used_by.join(", ");
        match &item.chosen {
            Choice::Already => {
                let_color(GREEN, " ✓ ");
                print!("{}", item.tool.label);
                let_color(
                    SUBTLE,
                    &format!("  [{}] — already on PATH ({})", item.tool.role.tag(), used),
                );
                println!();
            }
            Choice::Install(inst) => {
                let_color(TEAL, " → ");
                print!("{}", item.tool.label);
                let_color(SUBTLE, &format!("  [{}]  ", item.tool.role.tag()));
                print!("{}", inst.display());
                let_color(SUBTLE, &format!("   ({used})"));
                println!();
                if matches!(inst, Installer::Npm(_)) {
                    let targets = node_versions
                        .iter()
                        .map(|v| v.label.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let_color(SUBTLE, &format!("     targeting: {targets}\n"));
                }
            }
            Choice::Manual(msg) => {
                let_color(YELLOW, " ! ");
                print!("{}", item.tool.label);
                let_color(
                    SUBTLE,
                    &format!("  [{}] — manual install:", item.tool.role.tag()),
                );
                println!();
                let_color(SUBTLE, &format!("     {msg}"));
                println!();
            }
            Choice::NoManager(opts) => {
                let_color(RED, " ✗ ");
                print!("{}", item.tool.label);
                let_color(
                    SUBTLE,
                    &format!(
                        "  [{}] — no installer available, tried:",
                        item.tool.role.tag()
                    ),
                );
                println!();
                for o in opts {
                    let_color(SUBTLE, &format!("     {o}"));
                    println!();
                }
            }
        }
    }
    println!();
}

fn let_color(c: Color, s: &str) {
    let mut out = stdout();
    let _ = execute!(out, SetForegroundColor(c), Print(s), ResetColor);
}

fn confirm_proceed() -> Result<bool> {
    print!("Proceed with installs? [y/N] ");
    stdout().flush()?;
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;
    let trimmed = buf.trim().to_ascii_lowercase();
    Ok(trimmed == "y" || trimmed == "yes")
}

struct Summary {
    installed: usize,
    skipped: usize,
    manual: usize,
    failed: Vec<(String, String)>,
}

fn run_plan(plan: &[PlanItem], node_versions: &[NodeVersion]) -> Summary {
    let mut summary = Summary {
        installed: 0,
        skipped: 0,
        manual: 0,
        failed: Vec::new(),
    };
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
                    // No node target picked — shouldn't normally happen
                    // because `run()` aborts before reaching us, but stay
                    // defensive.
                    summary.failed.push((
                        item.tool.label.to_string(),
                        "no Node.js version selected".into(),
                    ));
                    continue;
                }
                for v in node_versions {
                    println!();
                    let_color(TEAL, "→ ");
                    println!(
                        "{} — npm install -g {}  (for {})",
                        item.tool.label,
                        pkgs.join(" "),
                        v.label
                    );
                    let mut cmd = Command::new(&v.npm_path);
                    cmd.args(["install", "-g"]);
                    cmd.args(pkgs.iter().copied());
                    // Prepend the matching node's bin dir to PATH so the
                    // npm script's `#!/usr/bin/env node` shebang resolves
                    // to the right node binary regardless of which version
                    // the host shell happens to have active.
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
                            let_color(GREEN, "✓ installed\n");
                        }
                        Ok(s) => {
                            let msg = format!("exit code {}", s.code().unwrap_or(-1));
                            let_color(RED, &format!("✗ failed ({msg})\n"));
                            summary.failed.push((label, msg));
                        }
                        Err(e) => {
                            let msg = format!("spawn error: {e}");
                            let_color(RED, &format!("✗ {msg}\n"));
                            summary.failed.push((label, msg));
                        }
                    }
                }
            }
            Choice::Install(inst) => {
                println!();
                let_color(TEAL, "→ ");
                println!("{} — {}", item.tool.label, inst.display());
                let Some(mut cmd) = inst.build_command() else {
                    summary.manual += 1;
                    continue;
                };
                match cmd.status() {
                    Ok(s) if s.success() => {
                        summary.installed += 1;
                        let_color(GREEN, "✓ installed\n");
                    }
                    Ok(s) => {
                        let msg = format!("exit code {}", s.code().unwrap_or(-1));
                        let_color(RED, &format!("✗ failed ({msg})\n"));
                        summary.failed.push((item.tool.label.to_string(), msg));
                    }
                    Err(e) => {
                        let msg = format!("spawn error: {e}");
                        let_color(RED, &format!("✗ {msg}\n"));
                        summary.failed.push((item.tool.label.to_string(), msg));
                    }
                }
            }
        }
    }
    summary
}

fn print_summary(s: &Summary) {
    println!();
    println!("─────────────────────────────");
    println!("Summary:");
    let_color(GREEN, &format!("  {} installed\n", s.installed));
    let_color(SUBTLE, &format!("  {} already present\n", s.skipped));
    if s.manual > 0 {
        let_color(YELLOW, &format!("  {} manual\n", s.manual));
    }
    if !s.failed.is_empty() {
        let_color(RED, &format!("  {} failed\n", s.failed.len()));
        for (label, why) in &s.failed {
            let_color(RED, &format!("    {label}: {why}\n"));
        }
    }
    println!();
    if s.installed > 0 {
        let_color(
            SUBTLE,
            "Some installers extend $PATH (cargo, go, dotnet tool, gem, pipx).\n",
        );
        let_color(
            SUBTLE,
            "Open a fresh shell or re-source your rc file before launching binvim.\n",
        );
    }
}

// ─── entry ─────────────────────────────────────────────────────────────────

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<()> {
    let picks = match pick_bundles()? {
        Some(p) => p,
        None => {
            println!("Cancelled.");
            return Ok(());
        }
    };
    if picks.is_empty() {
        println!("Nothing selected.");
        return Ok(());
    }

    let managers = detect_managers();
    print_managers(&managers);

    let plan = build_plan(&picks, &managers);

    // If the plan has any `npm install -g` step, discover installed Node
    // versions across the common version managers (+ system) and let the
    // user pick which ones to install for. The user's request was: "all
    // installs that include npm -g need to be asked what version" — we
    // ask once per run rather than once per package (15 prompts is
    // unfriendly) and apply the selection to every npm step.
    let needs_npm = plan
        .iter()
        .any(|p| matches!(p.chosen, Choice::Install(Installer::Npm(_))));
    let node_versions = if needs_npm {
        select_node_versions()?
    } else {
        Vec::new()
    };
    if needs_npm && node_versions.is_empty() {
        // `select_node_versions` already printed the reason.
        return Ok(());
    }

    print_plan(&plan, &node_versions);

    if !confirm_proceed()? {
        println!("Aborted.");
        return Ok(());
    }

    let summary = run_plan(&plan, &node_versions);
    print_summary(&summary);
    Ok(())
}

/// Discover Node.js installations, prompt the user when there's more than
/// one, and return the picked set. Empty `Vec` signals "abort the run" —
/// either no Node was found at all, or the user cancelled / picked zero
/// versions. The caller prints "Cancelled." / "Aborted." as appropriate.
fn select_node_versions() -> Result<Vec<NodeVersion>> {
    let detected = discover_node_versions();
    if detected.is_empty() {
        let_color(
            RED,
            "No Node.js installation found. Install Node.js (nvm, fnm, brew, apt, …) and re-run.\n",
        );
        return Ok(Vec::new());
    }
    if detected.len() == 1 {
        let_color(
            SUBTLE,
            &format!("Using Node {} for npm installs.\n", detected[0].label),
        );
        return Ok(vec![detected[0].clone()]);
    }
    let_color(
        SUBTLE,
        &format!(
            "Detected {} Node.js installations — pick which to install npm packages for.\n",
            detected.len()
        ),
    );
    match pick_node_versions(&detected)? {
        None => {
            println!("Cancelled.");
            Ok(Vec::new())
        }
        Some(indices) if indices.is_empty() => {
            println!("No Node version selected — aborting (npm installs need a target).");
            Ok(Vec::new())
        }
        Some(indices) => Ok(indices.into_iter().map(|i| detected[i].clone()).collect()),
    }
}

fn print_managers(managers: &BTreeSet<&'static str>) {
    let_color(SUBTLE, "Detected package managers: ");
    if managers.is_empty() {
        let_color(RED, "none on $PATH\n");
        return;
    }
    let_color(
        TEAL,
        &format!(
            "{}\n",
            managers.iter().copied().collect::<Vec<_>>().join(", ")
        ),
    );
}
