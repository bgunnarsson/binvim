# Changelog

All notable changes to binvim are recorded here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project
follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed
- **HTML-tag matched-pair highlight narrows to the tag name.** Cursor on
  `<main className="…">` used to highlight the whole opening run and the
  whole `</main>` run; now both halves underline just the tag name
  (`main` ↔ `main`).

## [0.1.1] - 2026-05-10

### Added
- `CONTRIBUTING.md` — repo conventions, architecture quick-reference, and a
  step-by-step PR walkthrough for the most common contribution shapes
  (new LSP, new tree-sitter grammar, motion/text-object).

### Fixed
- **Multi-selection delete cursor shift.** After `d` / `c` on a
  Ctrl-N multi-selection, secondary cursors landed at stale positions
  — they used original char indices without accounting for the
  cumulative shift left from earlier (lower-indexed) deletes. Now each
  cursor's final position subtracts the total length of every prior
  deletion, so all cursors sit exactly where the deleted ranges were.

### Changed
- **`H` / `L` cycle buffers (tabs).** Shift-h / shift-l in Normal mode
  now switch to the previous / next buffer respectively (same as
  `:bp`/`:bn` and `<leader>bp`/`<leader>bn`). They no longer fire the
  Vim viewport-top / viewport-bottom motions — use `M` (viewport
  middle) and the scroll commands for those workflows.
- **Tab bar reverted to one row.** Drops the 2-row variant. File-type
  icon column also removed; each tab is just ` label[ +]  × ` now.
- **Restored sessions land on the start page.** The saved buffers are
  loaded in the background; the welcome screen is on top until the
  user reaches for one (`H`/`L` cycle, `:bn`/`:bp`, `:b<n>`, `:e`,
  `<leader>e`, or clicking any tab). Solo-buffer sessions: `H`/`L`
  still dismiss the start page and surface the one buffer (with the
  usual "Only one buffer" hint). The tab bar IS shown above the start
  page so the user can see what was restored — no tab is highlighted
  as active until you actually pick one.

### Fixed
- **Snippet / completion accept honours `textEdit.range`.** Servers
  send an explicit replacement span (e.g. covering a trailing `.` when
  proposing `?.method`); previously we always used the client-side
  word-prefix guess, which produced `obj.?.method` instead of
  `obj?.method`. Now we honour the server's range when present and
  fall back to the prefix guess only when it's absent.
- **Session restore no longer leaves a phantom `[No Name]` buffer.**
  `App::new()` seeds `buffers[0]` with an empty stash before
  `hydrate_from_session` runs; the restore path now strips that initial
  empty slot when any session buffer is successfully opened.

### Changed
- **Tab bar polish.** Each tab now carries a 2-char language icon
  (colour-coded per file type: `rs`/`ts`/`js`/`go`/`{}`/`md`/etc.) on
  the left and a clickable `×` close button on the right. Padding
  bumped to give the label and chrome breathing room. Overflow
  chevrons render `‹` / `›` at the bar edges when tabs have scrolled
  off either side. Close-clicks honour the same dirty guard as `:bd`
  (refuse, status message); use the buffer prefix's `D` for force.
- **Tab bar replaces the buffer picker.** When 2+ buffers are open
  (outside the start page), a Catppuccin-Mantle tab row paints across
  the top of the screen — active tab in Surface1 + Lavender + bold,
  inactive in Subtext0, dirty buffers carry a Peach `+`. Left-click on
  a tab switches to it. Buffer area shifts down by one row when the
  bar is visible. The `<leader>b<space>` picker binding is gone; use
  the tab bar (click), `:bn`/`:bp`, or `<leader>bn`/`<leader>bp`.

### Added
- **Sublime-style multi-selection.** In Visual-char mode, `Ctrl-N`
  finds the next literal-text occurrence of the current selection and
  adds it as a parallel selection. Repeat to keep selecting. `d`, `c`,
  `y` then apply to every selection at once: `d` deletes all,
  `c` deletes and enters Insert mode with a mirrored cursor at every
  former selection start (typing then mirrors across all sites via
  the existing multi-cursor machinery). Wraps the buffer once; status
  message when only one occurrence exists or none are left.
- **Multi-cursor (real, mirrored).** `Ctrl-click` in Normal mode adds
  a secondary cursor at the click position; cursors render as Lavender
  blocks. Entering Insert via `i` keeps cursors anchored; via `a`
  shifts each cursor by +1 (so typing lands *after* each char). Other
  Insert-entries (`I`/`A`/`o`/`O`) collapse the multi-cursors. Inside
  Insert mode, typing and Backspace mirror at the primary cursor and
  every additional cursor (bottom-up edit order keeps indices stable).
  `Esc` exits Insert and collapses; a plain (non-Ctrl) click also
  collapses. Auto-pair is disabled while multi-cursor is active — the
  user is in mass-edit mode anyway.
- **Inlay hints.** Added `textDocument/inlayHint` request +
  response parser; hints render between buffer chars in dim italic
  (Overlay1). Refetched once per buffer-version; respects horizontal
  scroll and shares the line's width budget with the buffer chars.
  Servers that publish hints (rust-analyzer, typescript-language-server,
  gopls, …) now annotate types and parameters inline.
- **Snippet expansion.** LSP completion items marked
  `insertTextFormat == 2` get their `$N` / `${N:default}` / `$0`
  placeholders parsed and resolved. The cursor lands at `$1` (or `$0`
  if no `$1`); first-seen defaults mirror to later bare references of
  the same stop. `snippetSupport: true` advertised in the initialize
  payload so servers actually send them. Tab cycling between stops is
  a follow-up.
- **Project-wide substitute (`:S/pat/repl/[g]`).** Walks ripgrep for
  every file containing `pat` under `cwd`, opens each, applies the
  substitution buffer-wide, saves. Status reports total substitutions
  + file count + any per-file errors. Range prefix ignored.
- **Replace-all in buffer (`<leader>R`).** Literal-string counterpart
  to LSP rename: gathers the word under the cursor, opens a prompt
  pre-filled with it, on submit runs `:%s/word/new/g`. Useful when the
  symbol isn't LSP-tracked (config files, comments, prose).
- **Double-click to select word.** A second left-click at the same buffer
  position within 350ms expands to the inner word under the cursor and
  enters Visual-char mode.
- **Visual-block mode (`Ctrl-V`).** Third visual kind alongside char and
  line. Rectangular column selection across a line span; `d`/`c`/`y`
  apply column-wise per row. `>`/`<` fall back to indent/outdent over
  the line range. Selection survives drag, mode label reads `V-BLOCK`.
- **Sessions.** Open buffer set + per-buffer cursor + viewport are
  persisted to `~/.cache/binvim/sessions/<cwd-hash>.json` on clean
  shutdown and restored on launch when no file argument is passed. A
  buffer whose path no longer exists is silently dropped.
- **Better completion popup.** Each row renders a colour-coded kind chip
  (`fn`, `var`, `cls`, `if`, `fld`, `mod`, `snp`, `kw`, `K`, …) on the
  left and the server-supplied `detail` (e.g. the function signature)
  right-aligned in Overlay2 grey. Popup width caps at 80 cols; detail
  trims first when space is tight.
- **Picker scrolling.** Mouse wheel inside the picker moves the selection
  by ±3 (instead of scrolling the buffer behind it); PageUp/PageDown jump
  by a full visible page; Ctrl-U/Ctrl-D jump by half a page; Ctrl-G /
  Home and Ctrl-Shift-G / End jump to first/last result. Existing
  arrow-keys and Ctrl-J/Ctrl-K still move by one row; footer hint
  advertises `^J`/`^K` to match Vim convention. (Ctrl-N/Ctrl-P aliases
  removed — single Vim-style binding only.)

### Changed
- **Hover popup now syntax-highlights code and preserves indentation.**
  The markdown returned by an LSP is parsed into structured lines (prose,
  headings, horizontal rules, fenced code blocks); each fenced block's
  language tag drives a tree-sitter pass and the byte-colour map paints
  per-character, so signatures (e.g. `defineField<{ name: string; … }>`)
  finally render the way they would in the buffer. Code lines keep their
  leading whitespace verbatim (tabs expand to four columns); prose still
  word-wraps to the popup width but indentation carries through to wrapped
  continuations. Headings render bold + Lavender; rules render as a
  horizontal divider in the border colour.
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

[Unreleased]: https://github.com/bgunnarsson/binvim/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.1
[0.1.0]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.0
