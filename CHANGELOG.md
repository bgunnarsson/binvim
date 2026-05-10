# Changelog

All notable changes to binvim are recorded here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project
follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- `CONTRIBUTING.md` — repo conventions, architecture quick-reference, and a
  step-by-step PR walkthrough for the most common contribution shapes
  (new LSP, new tree-sitter grammar, motion/text-object).

### Changed
- **Picker redesigned as a centered floating popup.** Catppuccin-Mantle
  bordered box (~80% wide, ~60% tall, clamped) replacing the full-width
  bottom panel. Title chip in the top border with a counter on the right;
  Peach `›` prompt arrow; selected row gets a Lavender `▌` accent + Surface1
  background + bold; directory part of each path renders dim while the
  basename pops bright; smart truncation prefers keeping the basename
  intact; footer hint row (`↵ open  ^N/^P navigate  esc cancel`) with a
  shorter fallback on narrow terminals.
- **Source layout: `src/app/` and `src/lsp/` are now sub-module directories.**
  The two largest files (formerly `app.rs` at ~5.6k LoC and `lsp.rs` at ~2k)
  have been split into 14 and 6 focused children respectively. Each parent
  (`src/app.rs`, `src/lsp.rs`) is now a slim entry that declares the
  children and re-exports the public API; external addresses (`crate::app::App`,
  `crate::lsp::LspManager`) are unchanged. No behaviour changes.
- `CLAUDE.md` / `CONTRIBUTING.md` / `LSP_ADOPTION.md` updated to point at
  the new file paths and document the sub-module convention.

## [0.1.0] - 2026-05-09

Initial public release. binvim is a single-binary modal TUI editor written
in Rust, drop-in usable as a daily-driver Vim alternative for codebases
covered by its bundled language stack.

### Added

#### Modal editing
- Normal / Insert / Visual (char + line) / Command / Search / Picker /
  Prompt modes.
- Motions: word / WORD / end-of-word, line start/end, first/last non-blank,
  buffer first/last, `gg` / `G`, `H` / `M` / `L`, `f` / `F` / `t` / `T` and
  `;` / `,`, `n` / `N`, marks (`m{a-z}`, `'` / `` ` ``).
- Operators: `d`, `c`, `y` over motions, text objects, and linewise.
- Text objects: word, big-word, paragraph, quotes (`'"\``), brackets
  (`()[]{}<>`) — both `i` (inner) and `a` (around) variants.
- Indent / outdent (`>`, `<`, `>>`, `<<`) including visual-mode versions
  that keep the selection alive.
- Surround: `ds{c}`, `cs{old}{new}`, visual `S{c}` (Vim-surround style).
- Counts and registers — named, unnamed, and the `+`/`*` clipboard
  registers mirroring through to the OS clipboard via `arboard`.
- Macros (`q{a-z}` record, `@{a-z}` replay, `@@` replay last).
- `.` repeats the last edit, including full insert sessions.
- Jumplist with `Ctrl-O` / `Ctrl-I`.
- Undo / redo with persistent history at `~/.cache/binvim/undo/<sha>.json`
  (content-hash keyed, so out-of-band edits invalidate stale history).
- `Ctrl-A` / `Ctrl-X` increment / decrement on the line, recognising
  decimal, `0x…`, `0o…`, `0b…`, leading-zero padding, and a leading `-`
  when standalone.
- `~` toggle-case on the current char(s).
- `I` / `A` insert shortcuts.
- Indent-based folding with `za` / `zo` / `zc` / `zR` / `zM`.
- Yank flash — yanked range pulses Peach for 200ms.
- Smart Enter — copies leading whitespace, adds an indent unit after openers,
  splits `{|}` / `[|]` / `(|)` onto three lines with the cursor double-indented.
- Auto-pair brackets, quotes, backticks; auto-close HTML tags on typing `>`;
  comparison-aware guarding so `a < b` doesn't sprout a stray `>`.
- Matched bracket / matched HTML-tag highlight under the cursor.

#### Multi-buffer
- `:e` / `:bd` / `:bd!` / `:bn` / `:bp` / `:ls` / `:b{spec}`, plus the buffer
  picker (`<leader>b<space>`).
- Per-buffer state stash (cursor, viewport, history, marks, jumplist, folds,
  highlight cache).
- Auto-reload on external disk change for non-dirty buffers.
- Recent files at `~/.cache/binvim/recents`, surfaced in the file picker
  and as its own `<leader>?` shortcut.

#### Search & ex commands
- `/` / `?` / `n` / `N` / `*` / `#` with wrap-around, search-highlight, and
  `:noh` to silence it.
- `:s/pattern/replacement/[g]`, `:d`, `:y`, `:e`, `:goto`, `:health`,
  `:fmt` / `:format`.
- Ex range syntax: `1,5d`, `%s/foo/bar/g`, `.,$y`, etc.

#### LSP (hand-written JSON-RPC client over child-process stdio)
- Multiple servers per buffer: a primary (hover / goto / refs) plus
  auxiliaries (currently Tailwind on CSS/HTML/JSX/TSX/JS/TS/Astro/etc.,
  and csharp-ls layered onto Razor).
- Bundled server specs: rust-analyzer, typescript-language-server, Biome
  (JSON), gopls, vscode-html-language-server, vscode-css-language-server,
  astro-ls, csharp-ls (preferred for .cs/.vb), OmniSharp (fallback for
  .cs/.vb/Razor).
- Async `initialize` handshake — no blocking on the first open.
- 50ms `didChange` burst debounce so rapid typing doesn't flood servers.
- Drain cap on incoming traffic so diagnostic floods (OmniSharp) don't
  starve keyboard input.
- Tailwind detection covers v3 (`tailwind.config.{js,ts,cjs,…}`) and v4
  (`tailwindcss` listed in `package.json` (dev)dependencies).
- Capabilities: hover (`K`), goto-definition (`gd`), references (`gr`),
  document symbols (`<leader>o`), workspace symbols (`<leader>S`),
  completion (auto-triggered + `Ctrl-N`/`Ctrl-P`), signature help on
  `(` / `,`, code actions (`<leader>a`) including `workspace/executeCommand`
  + `workspace/applyEdit` round-trip, rename (`<leader>r`).
- Inline Error-Lens-style diagnostics with undercurl on the offending range.
- Hover popup: markdown cleanup, word-wrap, capped height, scrolling.
- Completion popup: prefix-tier ranking with substring + subsequence
  fallback; respects server `sortText` / `filterText`; preserves the
  popup across multi-server fan-out.
- `:health` opens a scratch buffer summarising version / pid / cwd /
  branch / config / CPU+RSS / open buffers / running LSPs / per-buffer
  attachment status / Tailwind config.

#### Tree-sitter highlighting
- Bundled grammars: Rust, TypeScript, JavaScript, JSX, TSX, JSON, HTML,
  CSS, Astro, Bash / Zsh, Markdown, C#, Razor.
- Capture priority by `pattern_index` — later patterns win, matching the
  upstream highlight-query convention. JSON ships a custom query (keys
  distinct from strings) because the upstream order is incompatible with
  this scheme.
- Catppuccin Mocha default theme; theme overrides via
  `~/.config/binvim/config.toml`. Capture-name resolution is dotted-prefix
  aware (`keyword.return` falls back to `keyword`).

#### Rendering & UI
- Powerline-style status line; floating command line / search prompt;
  notifications float top-right.
- Notification box wraps multi-row, caps at half terminal width, severity
  colours its border, click-to-copy lands the message in the clipboard
  and unnamed register.
- Mouse support: click, drag-select, wheel scroll (vertical + horizontal,
  with cursor drag-along).
- Horizontal viewport that follows the cursor; `zh` / `zl` / `zH` / `zL`
  for manual nudging.
- Visible tab / trailing-whitespace / EOL / non-breaking-space markers,
  configurable.
- Modal overlays dim the buffer behind them.
- Which-key popup after a 250ms leader hold.
- Start page (centred Mocha-Blue logo + configurable text) when launched
  with no path.

#### Pickers (one fuzzy engine, multiple roles)
- Files (`<leader><space>`), Buffers (`<leader>b<space>`), Grep
  (`<leader>g`, ripgrep-backed), Recents (`<leader>?`), Doc Symbols
  (`<leader>o`), Workspace Symbols (`<leader>S`), References, Code Actions.
- Skips `node_modules` and `.git/`, shows dotfiles.
- Yazi integration (`<leader>e`) — hands the terminal over and reclaims
  it cleanly on exit.

#### Save / format
- `:w`, `:wq`, `:q`, `:q!`.
- Format on save via Biome for JS / TS / JSX / TSX / JSON variants;
  resolved from project `node_modules/.bin/biome`.
- `.editorconfig` support: `indent_style`, `indent_size`,
  `insert_final_newline`, `trim_trailing_whitespace`.

#### Distribution
- Homebrew tap (`brew install bgunnarsson/binvim/binvim`).
- Linux install.sh (mirrored to `binvim-web` on each release).
- `bim` symlink installed alongside the `binvim` binary.
- binman-style local release scripts.

[Unreleased]: https://github.com/bgunnarsson/binvim/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.0
