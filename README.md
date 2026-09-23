```
██████╗ ██╗███╗   ██╗██╗   ██╗██╗███╗   ███╗
██╔══██╗██║████╗  ██║██║   ██║██║████╗ ████║
██████╔╝██║██╔██╗ ██║██║   ██║██║██╔████╔██║
██╔══██╗██║██║╚██╗██║╚██╗ ██╔╝██║██║╚██╔╝██║
██████╔╝██║██║ ╚████║ ╚████╔╝ ██║██║ ╚═╝ ██║
╚═════╝ ╚═╝╚═╝  ╚═══╝  ╚═══╝  ╚═╝╚═╝     ╚═╝
```

# binvim: the first vim IDE

**binvim is the first vim IDE**: a vim-native integrated development environment in a single binary, rather than a plain modal editor (Vim, Helix), an editor you assemble into an IDE from plugins (Neovim + distributions like AstroNvim / LazyVim), or a GUI IDE with bolted-on vim emulation (VS Code, Visual Studio, Rider). → [What is a vim IDE?](https://www.binvim.dev/what-is-a-vim-ide.html)

A Vim-grammar TUI editor written in Rust. One binary, no plugin system; everything below ships built in.

- **Editing**: modal editing with operators, motions, text objects, marks, registers, macros and dot-repeat; real multi-cursor and Sublime-style `Ctrl-N` selections; surround, code folding, smart indent, persistent undo, OS-clipboard paste, horizontal scrolling; writes that can't lose work and recovery after a crash. → [Editing](docs/editing.md)
- **Highlighting**: tree-sitter for Rust, TypeScript / TSX / JSX, JavaScript, JSON, Go, Python, C / C++, Java, Ruby, PHP, Lua, TOML, Svelte, Zig, Nix, Elixir, Dockerfile, SQL, HTML, CSS / SCSS, Markdown, C#, Razor, YAML, XML with the `.csproj` family, and Bash, plus hand-scanned colouring for `.editorconfig` and `.gitignore`. → [Highlighting](docs/highlighting.md)
- **LSP**: several servers per buffer (tsserver plus Tailwind, csharp-ls over Razor): completion, snippets, hover, rename, code actions, inlay hints, semantic tokens over tree-sitter, document highlight, signature help, references, document and workspace symbols, and `:messages` for what the servers say. Opt-in GitHub Copilot ghost completions. → [LSP](docs/lsp.md)
- **Debugging**: DAP for .NET (netcoredbg), Go (delve), Python (debugpy) and Rust / C / C++ (lldb-dap): project, bin and script pickers, .NET launch profiles, conditional breakpoints, frames, locals with lazy expansion, watches, and the Visual Studio / Rider F-keys. → [Debugger](docs/debugging.md)
- **Formatting**: one formatter per extension (biome, csharpier, gofmt / goimports, ruff, clang-format, shfmt, stylua, prettier, taplo, rufo, php-cs-fixer, google-java-format, zig fmt, nixfmt, mix format, ktfmt, sql-formatter) plus `.editorconfig` on every save. → [External tools](docs/external-tools.md)
- **Workspace**: fuzzy pickers for files, grep, symbols and more; window splits with pick-on-split (`<C-w>v` → picker → side by side); a tab bar; sessions with per-buffer jumplists; a git gutter with hunk staging; an embedded terminal, task runner and test runner; a `:health` dashboard; and a Catppuccin Mocha palette with ready-made themes. → [Keys](docs/keys.md), [Configuration](docs/configuration.md)

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

Scoop, the Nix flake, building from source, and `binvim-install`, the one-shot setup for the LSPs, formatters and debug adapters binvim drives, are in [docs/install.md](docs/install.md).

## Run

```sh
binvim
binvim [path]
bim
bim [path]
```

If `path` is omitted and a session exists for this cwd, the session restores (start page + tab row above it). Otherwise the start page renders alone. Press `:` for a command (`:e <path>`, `:q`) or `<space><space>` to open the file picker.

## Documentation

- [Install](docs/install.md): Homebrew, the install scripts, Scoop, crates.io, the Nix flake, from source, and `binvim-install` / `:install` / `:update` for the toolchains.
- [Editing](docs/editing.md): modal editing, motions and text objects, registers and macros, search, marks, folds, persistent undo, the git gutter, writes and recovery, sessions and tabs, pickers.
- [Keys](docs/keys.md): leader bindings, buffer / tab navigation, window splits.
- [Ex commands](docs/ex-commands.md): the `:` commands beyond the standard set.
- [Vim compatibility](docs/vim-compatibility.md): what's supported, where binvim differs on purpose, and what's left out.
- [Tree-sitter highlighting](docs/highlighting.md): the languages and the per-language query tweaks.
- [LSP](docs/lsp.md): capabilities, their bindings, and multi-server fan-out.
- [Debugger](docs/debugging.md): the DAP adapters, bindings, breakpoints and the debug pane.
- [External tools](docs/external-tools.md): every LSP, formatter and debug adapter binvim spawns, with the install command for each.
- [Configuration](docs/configuration.md): `~/.config/binvim/config.toml` section by section, keymaps, and the theme presets.
- [Project layout](docs/project-layout.md): what lives where in `src/`.

Alongside these: [CHANGELOG.md](CHANGELOG.md), [KNOWN_ISSUES.md](KNOWN_ISSUES.md), [ROADMAP.md](ROADMAP.md), [WINDOWS.md](WINDOWS.md) and [CONTRIBUTING.md](CONTRIBUTING.md).

## Licence

Source-available, not open source. Copyright (c) 2026 B. Gunnarsson; see [LICENSE](LICENSE) for the full text. In short: you may read the source, run it locally, modify your own copy, and submit pull requests upstream. You may not redistribute, publicly fork, or run it as a hosted service. For anything outside that scope, contact the licensor on Twitter/X at [@bgunnarssonis](https://twitter.com/bgunnarssonis).
