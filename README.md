# binvim

A Vim-grammar TUI editor written in Rust, with tree-sitter highlighting, LSPs, and fuzzy pickers built in.

## Features

- **Modal editing** — normal / insert / visual (charwise, linewise, blockwise) with operators, text objects, marks, registers, dot-repeat, undo/redo, and macros.
- **Tree-sitter highlighting** — Rust, TypeScript/TSX, JavaScript, JSON, Go, HTML, CSS, Markdown, C#.
- **LSP client** — diagnostics, hover, completion, and goto-definition over per-language servers, with `initializationOptions` and project-root detection.
- **Multi-server fan-out** — primary servers (rust-analyzer, tsserver, gopls, biome, …) plus auxiliaries layered on top. Tailwind class-name completion attaches alongside CSS / HTML / JSX / TSX / JS / TS / Astro / Vue / Svelte / Razor whenever Tailwind is detected (v3 `tailwind.config.*` or v4 CSS-first via a `tailwindcss` dependency in `package.json`).
- **Pickers** — fuzzy file picker, buffer switcher, and live grep, opened from the leader (`space`).
- **Format on save** — biome for JS/TS/JSX/TSX/JSON/JSONC; `.editorconfig` directives applied on save (final newline, trailing whitespace).
- **Whitespace markers** — every space, tab, non-breaking space, and end-of-line surfaces as a muted glyph (`·`, `→`, `⎵`, `¬`) so layout-affecting characters are visible at a glance.
- **Start page** — when launched with no path, binvim opens on a centered `binvim` logo (configurable). The page is read-only; press `:` for a command or `<space>` for the file picker.
- **Catppuccin Mocha defaults** — colours overridable via `~/.config/binvim/config.toml`.

## Build

```sh
cargo build --release
```

The binary lands at `target/release/binvim`.

## Run

```sh
binvim [path]
```

If `path` is omitted, binvim opens on the start page. Press `:` for a command (`:e <path>`, `:q`) or `<space>` to open the file picker.

## Leader bindings

| Keys       | Action            |
|------------|-------------------|
| `<space>`  | File picker       |
| `<space>b` | Buffer picker     |
| `<space>g` | Live grep         |

## Ex commands

Beyond the standard `:w`, `:q`, `:e <path>`, `:bd`, `:s/pat/repl/g`, etc.:

| Command            | Description                                                                                                   |
|--------------------|---------------------------------------------------------------------------------------------------------------|
| `:health`          | Open a scratch buffer summarising version, CPU/RAM share, buffers, attached LSPs, per-buffer LSP status (binary path + running flag), and Tailwind config detection. `:checkhealth` works too. |
| `:fmt` / `:format` | Run the configured formatter on the active buffer.                                                            |
| `:noh`             | Clear the search highlight.                                                                                   |

## External tools

binvim spawns these on demand. Each is optional — when a binary isn't on `$PATH` (or in a relevant `node_modules/.bin/`) the editor just skips that capability.

| Tool                                | Purpose                                  | Install                                                                  |
|-------------------------------------|------------------------------------------|--------------------------------------------------------------------------|
| `rust-analyzer`                     | Rust LSP                                 | `rustup component add rust-analyzer`                                     |
| `typescript-language-server`        | JS/TS/JSX/TSX LSP                        | `npm i -g typescript-language-server typescript`                         |
| `gopls`                             | Go LSP                                   | `go install golang.org/x/tools/gopls@latest`                             |
| `vscode-css-language-server`        | CSS/SCSS/Less LSP                        | `npm i -g vscode-langservers-extracted`                                  |
| `vscode-html-language-server`       | HTML LSP                                 | `npm i -g vscode-langservers-extracted`                                  |
| `tailwindcss-language-server`       | Tailwind class-name completion           | `npm i -g @tailwindcss/language-server` (note: scoped — the unscoped `tailwindcss-language-server` on npm is an empty stub) |
| `astro-ls`                          | Astro LSP                                | `npm i -g @astrojs/language-server`                                      |
| `csharp-ls`                         | C# LSP (Roslyn-based, preferred)         | `dotnet tool install --global csharp-ls`                                 |
| `OmniSharp`                         | Razor / .cshtml IntelliSense (full)      | `dotnet tool install --global omnisharp` or `brew install omnisharp`     |
| `rzls`                              | Razor language server (full)             | community tool — install if you have it; binvim auto-detects             |
| `biome` (project-local)             | JSON LSP + JS/TS/JSON formatter on save  | `npm i -D @biomejs/biome` in the project                                  |

binvim auto-discovers project-local binaries by walking up to the closest `node_modules/.bin/`, so a `devDependency` in your project takes precedence over a global install.

## Configuration

Optional config file at `~/.config/binvim/config.toml`:

```toml
schema_version = 1

[colors]
keyword = "#cba6f7"
"keyword.return" = "Magenta"
string = "#a6e3a1"

[start_page]
lines = [
    "  hello, world  ",
    "  press : to start ",
]

[whitespace]
show = true   # space=`·`, tab=`→ `, nbsp=`⎵`, eol=`¬`. On by default.
```

**`[colors]`** — values may be hex (`#rrggbb`) or a named crossterm colour. Capture names follow tree-sitter conventions (`keyword`, `string`, `function`, `type`, …); a dotted suffix matches more specifically before falling back to the head (`keyword.return` overrides `keyword`).

**`[start_page]`** — `lines` overrides the baked-in ASCII logo shown when binvim is launched with no path. Each entry renders on its own row, horizontally centered; the block as a whole is vertically centered. Omit it (or leave it empty) to keep the default logo.

**`[whitespace]`** — `show = true` (the default) renders every space as `·`, every tab as `→` plus space-fill to the tab width, every non-breaking space (U+00A0) as `⎵`, and the end-of-line as `¬`. All in the muted overlay colour. Set `show = false` to disable.

A missing or malformed config is ignored — the baked-in Catppuccin Mocha palette is used.

## Licence

Source-available, not open source. Copyright (c) 2026 B. Gunnarsson — see [LICENSE](LICENSE) for the full text. In short: you may read the source, run it locally, modify your own copy, and submit pull requests upstream. You may not redistribute, publicly fork, or run it as a hosted service. For anything outside that scope, contact the licensor.

## Project layout

```
src/
  app.rs           event loop, buffer/state management, ex-command dispatch
  buffer.rs        rope-backed text buffer
  command.rs       ex-command (`:`) parser
  config.rs        config loader and colour resolution
  cursor.rs        cursor + visual selection model
  editorconfig.rs  .editorconfig parser + on-save transforms
  format.rs        formatter dispatch (biome integration)
  lang.rs          tree-sitter language detection and highlight cache
  lsp.rs           LSP client (diagnostics, hover, completion, goto, multi-server fan-out)
  mode.rs          modes and operators
  motion.rs        motions
  parser.rs        keystroke → action parser
  picker.rs        fuzzy pickers
  render.rs        terminal rendering
  text_object.rs   text objects (`iw`, `i"`, `ap`, …)
  undo.rs          undo/redo history
```
