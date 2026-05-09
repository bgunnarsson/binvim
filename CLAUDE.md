# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build, run, test

```sh
cargo build --release          # release binary lands at target/release/binvim
cargo test                     # ~45 unit tests, all in `mod tests` blocks beside the code they cover
cargo test motion::tests       # run a single module's tests
cargo test motion::tests::word_forward_basic   # single test
cargo run -- path/to/file      # debug-build run; `binvim [path]` once installed
```

The user's `binvim` shell alias points at `target/release/binvim`, so any change you want them to exercise interactively needs a fresh `cargo build --release` — debug-build behaviour will not be picked up by their alias. Mention this in the hand-off if you've only built debug.

There is no `cargo fmt` / `clippy` configuration enforced in CI (no CI exists). Run them locally if you want, but they're not gating.

## Architecture

binvim is a single-binary modal TUI editor. `main.rs` does almost nothing — it parses one optional path argument and hands control to `App::run()`. Everything else lives flat under `src/`:

- **`app.rs`** is the heart. It owns the event loop, the active buffer + per-buffer stashes (`BufferStash`), all transient UI state (completion popup, hover, signature help, which-key, pickers, command line), and the dispatch from parsed `Action`s into mutations. It is large (~5k lines) on purpose — the state machine is centralised so the rest of the modules can stay pure-ish.
- **`parser.rs`** turns raw `KeyEvent`s into `Action` values via a Vim-grammar state machine. Operators (`d`, `c`, `y`, …), motion verbs, text-object verbs, counts, registers, and pending-prefix state (`g`, `z`, `[`, `]`, leader, surround) are all resolved here. `app.rs` only sees the resolved `Action`.
- **`buffer.rs`** wraps `ropey::Rope`. It exposes byte/char/line conversions and a monotonically incrementing `version` field used as a cache key by `lang.rs` and the LSP debounce.
- **`motion.rs`** + **`text_object.rs`** are pure functions over `(buffer, cursor)` returning `MotionResult` / `TextRange`. They have the densest test coverage in the project.
- **`mode.rs`**, **`cursor.rs`**, **`undo.rs`** — small data-model modules. `undo.rs` also handles persistence to `~/.cache/binvim/undo/<sha>.json`, keyed by content hash so an external edit invalidates stale history.
- **`render.rs`** is the only module that talks to crossterm for drawing. It reads the highlight cache produced by `lang.rs` plus the LSP diagnostic ranges and emits the final terminal frame. Cursor and viewport math (including horizontal scroll) live here.
- **`lang.rs`** owns tree-sitter: `Lang::detect` (extension → language), `ts_language()` / `highlights_query()`, and `compute_highlights()` which resolves overlapping captures by **pattern_index priority — later patterns win**. This is non-obvious: tree-sitter highlight queries follow the convention that general patterns come first and specific ones override later. JSON ships its own custom query (overriding the bundled one) because the upstream pattern order is incompatible with this scheme. If you change priority logic, the JSON query block at `lang.rs:88` is the canary.
- **`lsp.rs`** is a from-scratch JSON-RPC client over child-process stdio. The `LspManager` keys clients by `ServerSpec.key` and fans **multiple servers per buffer** (e.g. tsserver + Tailwind on a `.tsx` file, csharp-ls layered onto Razor). `primary_spec_for_path` (line ~224) is the per-extension dispatch; `tailwind_spec_for_path` and `csharp_aux_spec_for_path` are the layered auxiliaries. `didChange` is debounced with a 50ms burst window in `app.rs` so rapid typing doesn't flood servers.
- **`picker.rs`** is one fuzzy-picker engine reused for files, buffers, grep results, document symbols, workspace symbols, code actions, and references. The variant is `PickerKind`; the payload of the selected item is `PickerPayload`.
- **`config.rs`** loads `~/.config/binvim/config.toml`. Capture-name → colour resolution is dotted-prefix aware (`keyword.return` → `keyword`).
- **`editorconfig.rs`** parses `.editorconfig` and applies on-save transforms (final newline, trailing whitespace).
- **`format.rs`** dispatches biome on save for JS/TS/JSON variants.
- **`command.rs`** parses ex-mode (`:`) commands into `ExCommand` / `ExRange` which `app.rs` then executes.

### Adding a new LSP

`LSP_ADOPTION.md` is the authoritative recipe and stays in sync with the code. The four-file change is always: new arm in `primary_spec_for_path` (`lsp.rs`), extension → `Lang` mapping (`lang.rs`), `tree-sitter-<lang>` crate in `Cargo.toml` (only if you also want highlighting), README install table row. There is no plugin system — every server is hard-wired in `lsp.rs`.

### Adding tree-sitter highlighting for an existing LSP

Add the crate to `Cargo.toml`, then a `Lang` variant + `ts_language()` + `highlights_query()` arm. If the upstream highlights query is wrong under "later pattern wins" priority (see JSON), embed a corrected query inline rather than patching priority.

## Conventions to preserve

- **No new files unless necessary.** The flat `src/` layout is intentional — every module is a single file. Don't introduce `src/lsp/` directories or sub-modules without a real reason.
- **Tests live in `#[cfg(test)] mod tests` at the bottom of the file under test.** No separate `tests/` directory.
- **Comments explain *why*, not *what*.** Existing comments in `lang.rs` (priority resolution), `lsp.rs` (debounce window), and `app.rs` (BufferStash shape) are the pattern — load-bearing context that isn't obvious from the code. Don't add what-comments.
- **The licence is source-available, not open source.** See `LICENSE`. This affects what advice is appropriate around redistribution / forking, not how you write code, but worth knowing if a question goes that direction.
