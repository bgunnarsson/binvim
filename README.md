# binvim

A Vim-grammar TUI editor written in Rust. Tree-sitter highlighting, multi-server LSP fan-out with rename / code-actions / signature-help / find-references / document & workspace symbols, fuzzy pickers, persistent undo, code folding, surround operations, smart-indent, system-clipboard yank, horizontal scrolling, and a Catppuccin Mocha palette — all in one binary, no plugins.

## Features

### Editor

- **Modal editing** — normal / insert / visual (charwise, linewise, blockwise) with operators, text objects, marks, registers, dot-repeat, undo/redo, and macros.
- **Surround operations** — `ds<char>` strips the surrounding pair, `cs<old><new>` swaps it, visual `S<char>` wraps the selection. Pair chars: `(/)/b`, `[/]`, `{/}/B`, `<`, `"`, `'`, `` ` ``.
- **Numeric `Ctrl-A` / `Ctrl-X`** — increment / decrement the next number on the current line. Recognises decimal, `0x…`, `0b…`, `0o…`, with optional leading `-`. Preserves leading zeros.
- **Smart indent on Enter** — copies the current line's leading whitespace and adds one indent unit after `{ [ ( :` `=>` `->`. Pressing Enter inside an auto-paired `{|}` splits the pair into three lines with the cursor on the indented middle.
- **HTML tag auto-completion** — typing `>` after `<div` writes `<div>|</div>`. Self-closing tags, void elements, generics (`Array<T>`), comments and declarations all skipped. Active in `.html`, `.cshtml`, `.razor`, `.jsx`, `.tsx`, `.vue`, `.svelte`, `.astro`, `.xml`, `.md`.
- **Bracket and HTML-tag matching** — when the cursor is on (or just past) a bracket or anywhere inside an HTML tag, the matching partner highlights with a Surface2 background + bold. Works through arbitrary nesting.
- **Code folding** — indent-based folds, toggled with `za`/`zo`/`zc`, with `zR`/`zM` to open/close all. Folded blocks render as `⏷ N lines`.
- **Persistent undo** — undo history is serialised per file under `~/.cache/binvim/undo/<hash>.json` on save and reloaded on the next session, keyed by content hash so external edits invalidate stale history.
- **System-clipboard yank** — `y`, `yy`, `Y`, `:y`, visual yank, and the implicit yank on `d`/`c`/`x` mirror to the OS clipboard via `arboard` whenever they target the unnamed register. Named registers (`"ay`) stay local.
- **Yank flash** — yanked range flashes a Catppuccin Peach background for 200ms so you see what's been picked up.
- **Horizontal scrolling** — long lines scroll automatically as the cursor moves past the edge; trackpad / mouse-wheel horizontal events scroll without moving the cursor; Vim-style `zh` / `zl` (1 col) and `zH` / `zL` (half-width) work too.
- **Whitespace markers** — every space, tab, non-breaking space, and end-of-line surface as a muted glyph (`·`, `→`, `⎵`, `¬`). Configurable.
- **Format on save** — biome for JS / TS / JSX / TSX / JSON / JSONC. `.editorconfig` directives applied on save (final newline, trailing whitespace).
- **Auto-reload on disk change** — when an open file changes externally and the buffer isn't dirty, binvim notices via mtime poll and reloads with a status note.
- **Recents in the file picker** — most-recently-opened files surface at the top of the file picker on an empty query, persisted at `~/.cache/binvim/recents`.

### Tree-sitter highlighting

Rust, TypeScript / TSX, JavaScript, JSON, Go, HTML, CSS, Markdown, C#. Pattern-priority resolution so `(method_declaration name: (identifier) @function)` deterministically beats the catch-all `(identifier) @variable`.

### LSP

Per-language servers with `initializationOptions`, project-root detection, and a debounced `didChange` (50ms burst window) so rapid typing doesn't flood the server.

| Capability                  | Binding              | Notes                                                                                                                                  |
|-----------------------------|----------------------|----------------------------------------------------------------------------------------------------------------------------------------|
| Completion                  | auto + `Ctrl-N`/`Ctrl-P` | Multi-server fan-out — completion items from primary plus auxiliary servers (e.g. Tailwind alongside tsserver) are merged in the popup. |
| Hover                       | `K`                  |                                                                                                                                        |
| Goto-definition             | `gd`                 |                                                                                                                                        |
| Find references             | `gr`                 | Results open in a fuzzy picker; Enter jumps.                                                                                           |
| Document symbols            | `<space>o`           | File outline. Hierarchy preserved with `›` separators.                                                                                  |
| Workspace symbols           | `<space>S`           | Live server-side filter as you type.                                                                                                   |
| Signature help              | auto on `(` / `,`    | Parameter being typed gets a Catppuccin Yellow highlight inside the popup.                                                             |
| Code actions                | `<leader>a`          | Picks render with kind tag. Supports both `WorkspaceEdit` and command-shaped actions; round-trips `workspace/applyEdit` from the server. |
| Rename                      | `<leader>r`          | Prompt pre-fills the current word; submission applies the `WorkspaceEdit` across every affected file.                                   |
| Diagnostics                 | inline + sign column | Undercurl on the offending range, severity glyph in the gutter.                                                                        |

**Multi-server fan-out** — primary servers (rust-analyzer, tsserver, gopls, biome, OmniSharp, csharp-ls, …) plus auxiliaries layered on top. Tailwind class-name completion attaches alongside CSS / HTML / JSX / TSX / JS / TS / Astro / Vue / Svelte / Razor whenever Tailwind is detected (v3 `tailwind.config.*` or v4 CSS-first via a `tailwindcss` dependency in `package.json`). csharp-ls is layered onto Razor files so `@code{}` blocks get C# completion even without a dedicated Razor server.

### Pickers

Fuzzy file picker, buffer switcher, and live grep — all opened from leader (`space`).

### Catppuccin Mocha defaults

Colours overridable via `~/.config/binvim/config.toml`.

## Install

### macOS — Homebrew

```sh
brew install bgunnarsson/binvim/binvim
```

The tap lives at [github.com/bgunnarsson/homebrew-binvim](https://github.com/bgunnarsson/homebrew-binvim) and ships pre-built bottles for Apple Silicon and Intel.

### Linux — install script

```sh
curl -fsSL https://raw.githubusercontent.com/bgunnarsson/binvim/main/install.sh | sh
```

Pulls the matching musl-static tarball (`x86_64` or `aarch64`) from the latest GitHub Release, verifies its SHA-256, and drops the binary at `~/.local/bin/binvim`. Override with `BINVIM_VERSION=v0.1.0` or `BINVIM_INSTALL_DIR=/opt/bin` if needed.

### From source

```sh
cargo build --release
```

The binary lands at `target/release/binvim`. Requires a stable Rust toolchain.

## Run

```sh
binvim [path]
```

If `path` is omitted, binvim opens on the start page. Press `:` for a command (`:e <path>`, `:q`) or `<space>` to open the file picker.

## Leader bindings

| Keys       | Action            |
|------------|-------------------|
| `<space>`  | File picker       |
| `<space>?` | Recent files      |
| `<space>b` | Buffer picker     |
| `<space>g` | Live grep         |
| `<space>e` | Yazi file manager |
| `<space>o` | Document symbols  |
| `<space>S` | Workspace symbols |
| `<space>a` | Code actions      |
| `<space>r` | Rename            |

## Ex commands

Beyond the standard `:w`, `:q`, `:e <path>`, `:bd`, `:s/pat/repl/g`, etc.:

| Command            | Description                                                                                                   |
|--------------------|---------------------------------------------------------------------------------------------------------------|
| `:health`          | Open a scratch buffer summarising version, CPU / RAM share, buffers, attached LSPs (with binary path + running flag), and Tailwind config detection. `:checkhealth` works too. |
| `:fmt` / `:format` | Run the configured formatter on the active buffer.                                                            |
| `:noh`             | Clear the search highlight.                                                                                   |

## External tools

binvim spawns these on demand. Each is optional — when a binary isn't on `$PATH` (or in a relevant `node_modules/.bin/`) the editor just skips that capability.

| Tool                            | Purpose                                  | Install                                                                  |
|---------------------------------|------------------------------------------|--------------------------------------------------------------------------|
| `rust-analyzer`                 | Rust LSP                                 | `rustup component add rust-analyzer`                                     |
| `typescript-language-server`    | JS / TS / JSX / TSX LSP                  | `npm i -g typescript-language-server typescript`                         |
| `gopls`                         | Go LSP                                   | `go install golang.org/x/tools/gopls@latest`                             |
| `vscode-css-language-server`    | CSS / SCSS / Less LSP                    | `npm i -g vscode-langservers-extracted`                                  |
| `vscode-html-language-server`   | HTML LSP                                 | `npm i -g vscode-langservers-extracted`                                  |
| `tailwindcss-language-server`   | Tailwind class-name completion           | `npm i -g @tailwindcss/language-server` (the unscoped npm package is an empty stub — use the scoped one) |
| `astro-ls`                      | Astro LSP                                | `npm i -g @astrojs/language-server`                                      |
| `csharp-ls`                     | C# LSP (Roslyn-based, preferred)         | `dotnet tool install --global csharp-ls`                                 |
| `OmniSharp`                     | Razor / `.cshtml` IntelliSense (full)    | binvim probes `~/.local/bin/omnisharp/OmniSharp` plus `$PATH`. Drop the official tarball there. |
| `biome` (project-local)         | JSON LSP + JS / TS / JSON formatter      | `npm i -D @biomejs/biome` in the project                                  |
| `rg`                            | Live grep backend                        | `brew install ripgrep`                                                   |
| `yazi`                          | `<space>e` file manager                  | `brew install yazi`                                                      |

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

Source-available, not open source. Copyright (c) 2026 B. Gunnarsson — see [LICENSE](LICENSE) for the full text. In short: you may read the source, run it locally, modify your own copy, and submit pull requests upstream. You may not redistribute, publicly fork, or run it as a hosted service. For anything outside that scope, contact the licensor on Twitter/X at [@bgunnarssonis](https://twitter.com/bgunnarssonis).

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
  undo.rs          undo/redo history (in-memory + on-disk persistence)
```
