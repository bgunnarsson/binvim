# binvim

A Vim-grammar TUI editor written in Rust, with tree-sitter highlighting, LSP, and fuzzy pickers built in.

## Features

- **Modal editing** — normal/insert/visual (charwise, linewise, blockwise) with operators, text objects, marks, registers, dot-repeat, undo/redo, and macros.
- **Tree-sitter highlighting** — Rust, TypeScript/TSX, JavaScript, JSON, Go, HTML, CSS, Markdown.
- **LSP client** — diagnostics, hover, completion, and goto-definition over per-language servers, with `initializationOptions` and project-root detection.
- **Pickers** — fuzzy file picker, buffer switcher, and live grep, opened from the leader (`space`).
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

If `path` is omitted, binvim opens with an empty buffer.

## Leader bindings

| Keys      | Action            |
|-----------|-------------------|
| `<space>` | File picker       |
| `<space>b`| Buffer picker     |
| `<space>g`| Live grep         |

## Configuration

Optional config file at `~/.config/binvim/config.toml`:

```toml
schema_version = 1

[colors]
keyword = "#cba6f7"
"keyword.return" = "Magenta"
string = "#a6e3a1"
```

Values may be hex (`#rrggbb`) or a named crossterm colour. Capture names follow tree-sitter conventions (`keyword`, `string`, `function`, `type`, …); a dotted suffix matches more specifically before falling back to the head.

A missing or malformed config is ignored — the baked-in Catppuccin Mocha palette is used.

## Project layout

```
src/
  app.rs          event loop, buffer/state management
  buffer.rs       rope-backed text buffer
  command.rs      ex-command (`:`) parser and dispatch
  config.rs       config loader and colour resolution
  cursor.rs       cursor + visual selection model
  lang.rs         tree-sitter language detection and highlight cache
  lsp.rs          LSP client (diagnostics, hover, completion, goto)
  mode.rs         modes and operators
  motion.rs       motions
  parser.rs       keystroke → action parser
  picker.rs       fuzzy pickers
  render.rs       terminal rendering
  text_object.rs  text objects (`iw`, `i"`, `ap`, …)
  undo.rs         undo/redo history
```
