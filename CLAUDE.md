# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build, run, test

```sh
cargo build --release          # release binary lands at target/release/binvim
cargo test                     # ~207 unit tests, all in `mod tests` blocks beside the code they cover
cargo test motion::tests       # run a single module's tests
cargo test motion::tests::word_forward_basic   # single test
cargo run -- path/to/file      # debug-build run; `binvim [path]` once installed
```

The user's `binvim` shell alias points at `target/release/binvim`, so any change you want them to exercise interactively needs a fresh `cargo build --release` — debug-build behaviour will not be picked up by their alias. Mention this in the hand-off if you've only built debug.

There is no `cargo fmt` / `clippy` configuration enforced in CI (no CI exists). Run them locally if you want, but they're not gating.

## Architecture

binvim is a single-binary modal TUI editor. `main.rs` does almost nothing — it parses one optional path argument and hands control to `App::run()`. Most modules live flat under `src/`; the three largest — `app`, `lsp`, and `dap` — are sub-module directories. The parent file (`src/app.rs` / `src/lsp.rs` / `src/dap.rs`) declares the children and re-exports the public API, so external callers still address `crate::app::App` / `crate::lsp::LspManager` / `crate::dap::DapManager` directly.

- **`app.rs` + `app/`** is the heart — `App` owns the event loop, the active buffer + per-buffer stashes (`BufferStash`), all transient UI state (completion popup, hover, signature help, which-key, pickers, command line), and the dispatch from parsed `Action`s into mutations. The struct definition + `new`/`run`/`TerminalGuard` live in `app.rs`; everything else is split across `app/<topic>.rs` files (each contains `impl super::App { … }` blocks). The map: **`state`** (UI types, constants, small helpers), **`pair`** (bracket and HTML-tag matching, auto-pair), **`view`** (viewport, scrolling, folds, highlight cache), **`search`** (search, jumps, per-line range queries), **`registers`** (registers, macros, `.` repeat, clipboard mirror), **`buffers`** (open/switch/delete + recents + disk reload), **`save`** (formatter + .editorconfig on-save + git branch), **`edit`** (insert, replace, surround, undo, …), **`visual`** (visual-mode helpers), **`comment`** (`<leader>/` comment toggle), **`multi_cursor`** (multi-cursor edits), **`dispatch`** (`apply_action` + operator/motion glue), **`input`** (per-mode key/mouse handlers + `:`-command dispatch), **`lsp_glue`** (LSP event handling + request helpers), **`dap_glue`** (DAP event handling + breakpoint/step requests), **`git_glue`** (gutter diff refresh + hunk navigation), **`copilot`** (GitHub Copilot LSP integration — ghost-text suggestions), **`picker_glue`** (generic picker open/handle + yazi), **`quickfix`** (quickfix/location-list state + navigation), **`windows`** (window-split focus + lifecycle around `layout.rs`), **`health`** (`:health` output).
- **`parser.rs`** turns raw `KeyEvent`s into `Action` values via a Vim-grammar state machine. Operators (`d`, `c`, `y`, …), motion verbs, text-object verbs, counts, registers, and pending-prefix state (`g`, `z`, `[`, `]`, leader, surround) are all resolved here. `app.rs` only sees the resolved `Action`.
- **`buffer.rs`** wraps `ropey::Rope`. It exposes byte/char/line conversions and a monotonically incrementing `version` field used as a cache key by `lang.rs` and the LSP debounce.
- **`motion.rs`** + **`text_object.rs`** are pure functions over `(buffer, cursor)` returning `MotionResult` / `TextRange`. They have the densest test coverage in the project.
- **`mode.rs`**, **`cursor.rs`**, **`undo.rs`** — small data-model modules. `undo.rs` also handles persistence to `~/.cache/binvim/undo/<sha>.json`, keyed by content hash so an external edit invalidates stale history.
- **`window.rs`** + **`layout.rs`** carry the window-split system. A `Window` is a view onto a buffer (cursor, viewport, visual anchor, buffer index); the active one lives inline on `App.window`, the rest in `App.windows: HashMap<WindowId, Window>`. `Layout` is the binary split tree whose leaves are `WindowId`s — `partition()` walks it against a parent `Rect` and emits `(WindowId, Rect)` per leaf, and `focus_neighbor()` uses those rects for **geometric** `h`/`j`/`k`/`l` navigation, not tree-order.
- **`session.rs`** persists the open-buffer set + cursor positions on clean shutdown to `~/.cache/binvim/sessions/<hash-of-cwd>.json`. Restored on launch **only** if no explicit file arg was passed — `binvim foo.rs` always means "open foo.rs."
- **`render.rs`** is the only module that talks to crossterm for drawing. It reads the highlight cache produced by `lang.rs` plus the LSP diagnostic ranges and emits the final terminal frame. Cursor and viewport math (including horizontal scroll) live here.
- **`markdown_render.rs`** is a hand-rolled markdown scanner producing per-line conceal transforms (`**bold**` → `bold`, `# h1` → glyph, etc.) consulted by `render.rs` when the active buffer is `.md` **and** the editor is in Normal mode. Deliberately not tree-sitter — see the module header for why.
- **`git.rs`** shells out to `git diff --unified=0` to compute working-tree hunks against the index/HEAD for the gutter stripe. `unified=0` means one hunk per contiguous change with no surrounding context muddying the line math.
- **`lang.rs`** owns tree-sitter: `Lang::detect` (extension → language), `ts_language()` / `highlights_query()`, and `compute_highlights()` which resolves overlapping captures by **pattern_index priority — later patterns win**. This is non-obvious: tree-sitter highlight queries follow the convention that general patterns come first and specific ones override later. JSON ships its own custom query (overriding the bundled one) because the upstream pattern order is incompatible with this scheme. If you change priority logic, the JSON query block at `lang.rs:88` is the canary.
- **`lsp.rs` + `lsp/`** is a from-scratch JSON-RPC client over child-process stdio. `lsp.rs` is a thin entry that re-exports `LspManager` and the public types; the implementation is split into **`types`** (data types + URI helpers), **`specs`** (`primary_spec_for_path` and the auxiliary dispatch + workspace discovery), **`client`** (`LspClient` spawn + send/recv + per-client semantic-tokens legend), **`io`** (reader thread + JSON-RPC dispatcher + `auto_respond` + initialize-response legend extraction + `showMessage` / `logMessage` notification routing), **`manager`** (`LspManager` + `handle_response`), **`parse`** (response parsers, incl. semantic-tokens delta-stream decode + documentHighlight ranges). The manager keys clients by `ServerSpec.key` and fans **multiple servers per buffer** (e.g. tsserver + Tailwind on a `.tsx` file, csharp-ls layered onto Razor). `didChange` is debounced with a 50ms burst window in `app/lsp_glue.rs` so rapid typing doesn't flood servers. Three render-loop "if-due" hooks fire per-version requests for inlay hints, semantic tokens, and documentHighlight; each guards against re-firing for an unchanged anchor (cursor + buffer version).
- **`dap.rs` + `dap/`** is the Debug Adapter Protocol client, structurally parallel to `lsp/` — same split into **`types`**, **`specs`** (adapter registry + workspace-root discovery + per-adapter target discovery + `$PATH` lookup), **`client`**, **`io`**, **`manager`**. `DapManager` owns the active debug session, the user's breakpoint table, and the receiver side of the reader-thread channel; adapter-agnostic. Concrete adapters today: netcoredbg (.NET), `dlv dap` (Go), `python3 -m debugpy.adapter` (Python), `lldb-dap` (Rust / C / C++). Each is one `DapAdapterSpec` in `dap/specs.rs` plus a `build_launch_args` fn; `prelaunch` is a `fn(&LaunchContext) -> Option<PrelaunchCommand>` so per-target builds (`cargo build --bin foo`) work without an extra codepath. The dispatch in `app/dap_glue.rs` picks the adapter via `adapter_for_workspace` then routes to a per-adapter resolver (`dap_resolve_dotnet` / `_go` / `_python` / `_rust`) that handles the project / bin / script picker before kicking off the session.
- **`picker.rs`** is one fuzzy-picker engine reused for files, buffers, grep results, document symbols, workspace symbols, code actions, and references. The variant is `PickerKind`; the payload of the selected item is `PickerPayload`.
- **`config.rs`** loads `~/.config/binvim/config.toml`. Capture-name → colour resolution is dotted-prefix aware (`keyword.return` → `keyword`).
- **`editorconfig.rs`** parses `.editorconfig` and applies on-save transforms (final newline, trailing whitespace).
- **`format.rs`** dispatches one formatter per extension (biome for JS/TS/JSON, csharpier for C#, gofmt/goimports for Go, ruff/black for Python, clang-format for C/C++, shfmt for shell, stylua for Lua, prettier for everything biome doesn't cover — Markdown/MDX/Vue/Svelte/HTML/CSS/SCSS/Less/YAML/GraphQL, with project-local `node_modules/.bin/prettier` preferred over a global install, taplo for TOML, rufo for Ruby, php-cs-fixer for PHP, google-java-format for Java, zig fmt for Zig, nixfmt/alejandra for Nix, mix format for Elixir, ktfmt for Kotlin, sql-formatter for SQL, plus `.editorconfig` indent reflow for Razor). Tools without stdin support (csharpier, php-cs-fixer, ktfmt) go through a temp-file dance; everything else uses the shared `run_stdin_pipe` helper.
- **`command.rs`** parses ex-mode (`:`) commands into `ExCommand` / `ExRange` which `app/input.rs` then executes.

### Adding a new LSP

The five-file change is always: new arm in `primary_spec_for_path` (`lsp/specs.rs`), `Lang` variant + extension/basename entry in `Lang::detect()` and matching `ts_language()` / `highlights_query()` arms (`lang.rs`) — skip this only if you don't want tree-sitter highlighting, icon + lang_name in the two exhaustive `Lang` matches (`render.rs`), formatter arm in `format_buffer` (`format.rs`) plus `tree-sitter-<lang>` crate in `Cargo.toml`, README install table rows for the LSP and the formatter. There is no plugin system — every server is hard-wired in `lsp/specs.rs`.

### Adding tree-sitter highlighting for an existing LSP

Add the crate to `Cargo.toml`, then a `Lang` variant + `ts_language()` + `highlights_query()` arm. If the upstream highlights query is wrong under "later pattern wins" priority (see JSON), embed a corrected query inline rather than patching priority.

### Adding a new DAP adapter

Three places to touch:

1. **`dap/specs.rs`** — append a `DapAdapterSpec` to `BUILTIN_ADAPTERS` with `key`, `adapter_id`, `cmd_candidates`, `args`, `root_markers`, a `prelaunch` fn (return `None` if the adapter builds implicitly, e.g. delve), and a `build_launch_args` fn that produces the DAP `launch` request JSON. Add any per-adapter target discovery here too (`find_<lang>_*` helper) and `pub use` it via `dap.rs`.
2. **`app/dap_glue.rs`** — add a `dap_resolve_<lang>` method that wraps the discovery → 0/1/many → auto-pick or open `DebugTarget` picker flow. Match arm in `dap_start_session`'s adapter dispatch routes to it.
3. **`dap.rs`** — re-export any new public types / helpers.

Adapter-specific behaviour stays in `specs.rs` + `dap_glue.rs`; `manager.rs` and the wire protocol in `types`/`io`/`client` are adapter-agnostic. There is no plugin system — like the LSP layer, every adapter is hard-wired.

## Conventions to preserve

- **Sub-modules only for `app/`, `lsp/`, and `dap/`.** Other modules stay flat — don't introduce `src/foo/` directories without a real reason. Inside `app/`, child modules contain `impl super::App { … }` blocks; sibling-visible methods are `pub(super)`. Visibility-narrowing (`pub(super)` instead of bare `pub`) is preferred whenever a helper is only consumed by other `app/` siblings.
- **Tests live in `#[cfg(test)] mod tests` at the bottom of the file under test.** No separate `tests/` directory.
- **Comments explain *why*, not *what*.** Existing comments in `lang.rs` (priority resolution), `lsp/manager.rs` (debounce window in the `drain` cap), and `app/state.rs` (BufferStash shape) are the pattern — load-bearing context that isn't obvious from the code. Don't add what-comments.
- **The licence is source-available, not open source.** See `LICENSE`. This affects what advice is appropriate around redistribution / forking, not how you write code, but worth knowing if a question goes that direction.
