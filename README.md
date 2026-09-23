```
██████╗ ██╗███╗   ██╗██╗   ██╗██╗███╗   ███╗
██╔══██╗██║████╗  ██║██║   ██║██║████╗ ████║
██████╔╝██║██╔██╗ ██║██║   ██║██║██╔████╔██║
██╔══██╗██║██║╚██╗██║╚██╗ ██╔╝██║██║╚██╔╝██║
██████╔╝██║██║ ╚████║ ╚████╔╝ ██║██║ ╚═╝ ██║
╚═════╝ ╚═╝╚═╝  ╚═══╝  ╚═══╝  ╚═╝╚═╝     ╚═╝
```

# binvim — the first vim IDE

**binvim is the first vim IDE** — a vim-native integrated development environment in a single binary, rather than a plain modal editor (Vim, Helix), an editor you assemble into an IDE from plugins (Neovim + distributions like AstroNvim / LazyVim), or a GUI IDE with bolted-on vim emulation (VS Code, Visual Studio, Rider). → [What is a vim IDE?](https://www.binvim.dev/what-is-a-vim-ide.html)

A Vim-grammar TUI editor written in Rust. Tree-sitter highlighting (Rust, TS/TSX/JSX, JS, JSON, Go, Python, C / C++, Java, Ruby, PHP, Lua, TOML, Svelte, Zig, Nix, Elixir, Dockerfile, SQL, HTML, CSS, Markdown, C#, Razor, YAML, XML / `.csproj` / `.manifest` family, Bash, `.editorconfig`, `.gitignore`), multi-server LSP fan-out (rename, code-actions, inlay hints, semantic tokens layered over tree-sitter, document highlight, signature help, snippet expansion, find-references, document & workspace symbols, `:messages` capture for `window/showMessage` + `window/logMessage` + server stderr), opt-in GitHub Copilot via `copilot-language-server` with inline ghost completions, built-in debuggers via DAP for .NET (netcoredbg), Go (delve), Python (debugpy), and Rust / C / C++ (lldb-dap) with project / bin / script pickers, .NET launchSettings profiles, breakpoints, stack frames, locals with lazy expansion, VS / Rider F-keys, per-language formatters dispatched by extension (biome, csharpier, gofmt/goimports, ruff, clang-format, shfmt, stylua, prettier, taplo, rufo, php-cs-fixer, google-java-format, zig fmt, nixfmt, mix format, ktfmt, sql-formatter, plus `.editorconfig` reflow on every save), window splits with per-buffer layouts and pick-on-split (`<C-w>v` → picker → instant side-by-side), real multi-cursor with Sublime-style `Ctrl-N` selections, fuzzy pickers with file-type icons and match-character highlighting, `<leader>/` per-language comment toggle, sessions with persistent per-buffer jumplists, tab bar, persistent undo, code folding, surround operations, smart-indent, OS-clipboard paste, horizontal scrolling, a `:health` dashboard that surfaces per-LSP init state + pending-request breakdown + cache counts, and a Catppuccin Mocha palette — all in one binary, no plugins.

## How binvim compares

binvim is the first vim IDE. Here's how it stacks up against the editors and IDEs people usually weigh:

- [What is a vim IDE?](https://www.binvim.dev/what-is-a-vim-ide.html)
- [binvim vs Neovim](https://www.binvim.dev/binvim-vs-neovim.html)
- [binvim vs Helix](https://www.binvim.dev/binvim-vs-helix.html)
- [binvim vs AstroNvim / LazyVim](https://www.binvim.dev/binvim-vs-astronvim.html)
- [binvim vs VS Code](https://www.binvim.dev/binvim-vs-vscode.html)
- [binvim vs Visual Studio](https://www.binvim.dev/binvim-vs-visual-studio.html)
- [binvim vs Rider](https://www.binvim.dev/binvim-vs-rider.html)

## Install

```sh
brew install bgunnarsson/binvim/binvim              # macOS
curl -fsSL https://binvim.dev/install.sh | sh        # Linux
cargo install --locked binvim                        # anywhere with a Rust toolchain
```

```powershell
iwr https://binvim.dev/install.ps1 -UseBasicParsing | iex   # Windows
```

Scoop, the Nix flake, building from source, and `binvim-install` — the one-shot setup for the LSPs, formatters and debug adapters binvim drives — are in [docs/install.md](docs/install.md).

## Run

```sh
binvim [path]
```

If `path` is omitted and a session exists for this cwd, the session restores (start page + tab row above it). Otherwise the start page renders alone. Press `:` for a command (`:e <path>`, `:q`) or `<space>` to open the file picker.

## Documentation

- [Install](docs/install.md) — Homebrew, the install scripts, Scoop, crates.io, the Nix flake, from source, and `binvim-install` / `:install` / `:update` for the toolchains.
- [Editing](docs/editing.md) — modal editing, motions and text objects, registers and macros, search, marks, folds, persistent undo, the git gutter, writes and recovery, sessions and tabs, pickers.
- [Keys](docs/keys.md) — leader bindings, buffer / tab navigation, window splits.
- [Ex commands](docs/ex-commands.md) — the `:` commands beyond the standard set.
- [Vim compatibility](docs/vim-compatibility.md) — what's supported, where binvim differs on purpose, and what's left out.
- [Tree-sitter highlighting](docs/highlighting.md) — the languages and the per-language query tweaks.
- [LSP](docs/lsp.md) — capabilities, their bindings, and multi-server fan-out.
- [Debugger](docs/debugging.md) — the DAP adapters, bindings, breakpoints and the debug pane.
- [External tools](docs/external-tools.md) — every LSP, formatter and debug adapter binvim spawns, with the install command for each.
- [Configuration](docs/configuration.md) — `~/.config/binvim/config.toml` section by section, keymaps, and the theme presets.
- [Project layout](docs/project-layout.md) — what lives where in `src/`.

Alongside these: [CHANGELOG.md](CHANGELOG.md), [KNOWN_ISSUES.md](KNOWN_ISSUES.md), [ROADMAP.md](ROADMAP.md), [WINDOWS.md](WINDOWS.md) and [CONTRIBUTING.md](CONTRIBUTING.md).

## Licence

Source-available, not open source. Copyright (c) 2026 B. Gunnarsson — see [LICENSE](LICENSE) for the full text. In short: you may read the source, run it locally, modify your own copy, and submit pull requests upstream. You may not redistribute, publicly fork, or run it as a hosted service. For anything outside that scope, contact the licensor on Twitter/X at [@bgunnarssonis](https://twitter.com/bgunnarssonis).
