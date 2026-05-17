# Roadmap

This is a directional roadmap, not a commitment. Items move between sections as priorities shift. The goal is
for binvim to be a viable daily-driver replacement for Neovim across the languages it ships support for —
without growing a plugin system.

Status legend: **next** = actively in scope, **planned** = agreed direction, **considering** = open question.

## Editor

- [x] **Window splits** — `<C-w>v` / `<C-w>s` / `<C-w>V` / `<C-w>S` / `<C-w>h/j/k/l` / `<C-w>q` / `<C-w>o` /
      `<C-w>=` / `<C-w>T`. Vertical and horizontal splits with per-buffer layouts (each tab carries its own
      split tree), independent cursors / viewports per pane, pick-on-split (default `<C-w>v` opens the file
      picker so the new pane lands on a different file straight away), and Vim-style same-buffer splits via
      the uppercase variants. **Shipped in 0.1.8.**
- [x] **`<C-w>` + integer resize.** `<C-w>10>` widens by 10 cols, `<C-w>5<` shrinks, `<C-w>[N]+` / `<C-w>[N]-`
      adjust height. Parser accumulates digits inside the window-leader prefix; the layout walks to the
      deepest matching-axis ancestor of the focused leaf and converts cells to a ratio against that subtree's
      own rect (clamped to `[0.1, 0.9]`).
- [x] **Built-in `:terminal` split.** `:terminal` (or `:term`) opens a
      PTY-backed shell as a bottom split pane that stacks above the
      debug pane and below the editor. `Mode::Terminal` for typing
      (xterm escape sequences for arrows / F-keys / Page / Home /
      End / Delete / Insert / Tab; Ctrl-letter → C0; Alt-prefix for
      Meta); `Mode::TerminalNormal` for Vim-style navigation +
      selection over the grid (`h/j/k/l` / `0` / `$` / `g` / `G`
      move a reading-cursor, `v` enters Visual, `y` yanks the
      selection to the unnamed register + system clipboard, `Y`
      yanks the current row, `<C-w>q` closes). vte-parsed grid
      with full SGR colour + attrs, CUP / CUF / CUB / CUU / CUD /
      CHA cursor moves, ED / EL clears, IND / RI / DECSC / DECRC /
      RIS, line wrap into a 10k-row scrollback. Mouse forwarding
      to the PTY when the inner program enables DECSET 1000 / 1002
      / 1003 / 1006 (htop, vim mouse=a, less mouse mode); otherwise
      clicks focus the pane. SGR + legacy X10 encodings both
      supported.
- [x] **Cmdline & search history.** `:<Up>` cycles previous ex commands; `/<Up>` cycles previous searches.
      Capped at 100 entries, dedup against the immediate previous, independent rings for `:` vs `/`. Persisted
      to the existing per-cwd session JSON; histories load even on `binvim foo.rs` so recall stays warm
      regardless of launch mode.
- [ ] **Tab completion in `:` ex commands.** Filenames after `:e`, buffer names after `:b`, command names from
      cold. **planned**
- [ ] **Spell check.** Toggleable per-buffer, with `]s` / `[s` to jump between misspellings and `z=` for
      suggestions. Useful for prose + comments. **considering**
- [ ] **Large-file mode.** Skip tree-sitter + LSP attach when the buffer crosses a size threshold (e.g. 5MB or
      50k lines), with a status hint. The rope handles the byte volume fine; the highlight pass is what dies.
      **planned**
- [x] **Inline ghost completion (LSP 3.18 `textDocument/inlineCompletion`).** Render the server's suggestion
      as muted italic Overlay0 after the cursor on a 250 ms idle pause; `<Tab>` accepts (honours the
      response's `range` so typed prefix isn't duplicated, trims trailing overlap with post-cursor text,
      auto-opens `{ … }` blocks for partial suggestions); `<Enter>` accepts the LSP popup item; any other key
      dismisses the ghost. Provider-neutral — Copilot's the first server wired but any server speaking the
      spec gets the UI for free. Multi-line ghost render (only the first line is currently painted; accepts
      insert all lines) is the remaining polish item.

## LSP

- [x] **Semantic tokens (full).** `textDocument/semanticTokens/full` layered
      on top of the tree-sitter pass — LSP tokens win where present, tree-
      sitter fills in everything else. Server legend is captured from the
      `initialize` response, the integer delta stream is decoded into
      per-line ranges (binned for constant-time per-row lookup), and the
      renderer overlays them on the per-char paint. Token type + modifiers
      flow through the same `color_for_capture` dotted-prefix resolver the
      tree-sitter pass uses (`function.async`, `variable.readonly`), so the
      same `[colors]` config drives both. Delta requests / range mode not
      yet implemented — full refresh per buffer version, throttled the same
      way as inlay hints.
- [x] **Document highlight.** `textDocument/documentHighlight` fires on
      cursor settle in Normal / Visual mode (skipped behind pickers /
      completion popups); the ranges paint with a Surface2 bg under the
      syntax-coloured foreground so every occurrence of the symbol
      under the cursor reads at a glance. Cache stays valid while the
      cursor sits inside any returned range — moving by one column
      inside the same identifier doesn't blink the highlights off and
      on between requests. Edits invalidate the cache; moving off the
      symbol clears it. Capped at one in-flight request per buffer
      path so fast navigation can't queue up against a slow / cold-
      indexing server.
- [ ] **Code lens.** `textDocument/codeLens` for things like "Run test" / "Debug test" / reference counts
      above declarations. Renders as virtual text on the line above the anchor. **planned**
- [ ] **Workspace folders / multi-root.** Currently one project root per buffer; opening files from a sibling
      repo doesn't fan a second workspace into the same client. Important for monorepos. **considering**
- [x] **`window/showMessage` and `window/logMessage` surfacing.** Both
      notifications are captured into a bounded ring (500 entries) on
      the App. `showMessage` Error / Warning fires through `status_msg`
      so the user sees server-emitted complaints inline; `logMessage` is
      log-only. `:messages` opens a scrollable severity-coloured overlay
      (Esc / q / :q to dismiss) for reading back what was missed.
- [x] **Copilot via `copilot-language-server`.** Opt-in via `[copilot] enabled = true` in config. Attached as
      an aux LSP to every buffer; auth is device-flow surfaced in the status line with a 3 s auto-poll so the
      editor flips to "signed in" as soon as the user clicks through in the browser. `:copilot` /
      `:copilot signin` / `:copilot reload` / `:copilot signout` ex commands. No HTTP client in binvim — Node
      handles the networking.

## Debugger (DAP)

- [x] **delve (Go).** `dlv dap` on stdio. `package main` directories are
      discovered under the workspace root (with the buffer's own dir
      preferred when it's one of them); single match auto-picks, multiple
      open the project picker. `mode: debug` so delve handles build + run
      — no binvim-side prelaunch.
- [x] **debugpy (Python).** `python3 -m debugpy.adapter` on stdio. Active
      `.py` buffer is launched directly; otherwise the workspace root's
      `main.py` / `__main__.py` / `app.py` / `manage.py` / `run.py` /
      `server.py` / `cli.py` candidates feed the picker. `justMyCode:
      false`.
- [x] **lldb-dap (Rust / C / C++).** `lldb-dap` (with legacy
      `lldb-vscode` fallback). Cargo workspace members (incl. `crates/*`
      globs) are walked for `[[bin]]` / `src/main.rs` / `src/bin/*.rs`;
      each bin is one picker row. Prelaunch `cargo build --bin <name>`,
      launch `target/debug/<name>`. `env` serialised as the
      `["K=V", ...]` array form lldb-dap requires.
- [x] **Watch expressions.** `:dapwatch <expr>` appends; `:dapunwatch
      <n>` removes one (1-based); `:dapunwatch all` clears.
      `:dapwatches` dumps the current list + last values to the status
      line. The manager re-evaluates every watch against the top frame
      on every `stopped` event via DAP `evaluate`; results render above
      the frame list in the debug pane (red value when the server
      reports an error for the expression). Survives across sessions
      — the user list is on `DapManager`; only the cached `result`
      clears at session start.
- [ ] **Conditional + hit-count breakpoints.** Existing breakpoints are unconditional; DAP
      `breakpoint.condition` / `hitCondition` already carry the wire format. **considering**

## Quality / Tooling

- [ ] **CI: `cargo test` + `cargo clippy` on PRs.** Today only `release.yml` runs (on tag push). Tests exist
      but nothing gates them. **next**
- [ ] **CI: `cargo fmt --check`.** Cheap, catches drift. **next**
- [ ] **Crash-handler.** Catch panics in the event loop, restore the terminal, write the panic + buffer state
      to `~/.cache/binvim/crash/`, and exit cleanly. Currently a panic leaves the terminal in raw mode.
      **next**
- [ ] **Property tests for motion / text-object.** Both modules are pure functions — good targets for
      `proptest`. The existing unit tests cover named cases; properties would surface boundary bugs on
      Unicode, empty buffers, multi-byte sequences. **planned**
- [ ] **Fuzz tree-sitter + LSP message dispatch.** Dual-purpose: hardens parsers and exercises edge cases in
      the JSON-RPC reader. **considering**

## Distribution

- [ ] **macOS prebuilt binaries in `release.yml`.** Today release CI builds Linux musl only; macOS users go
      through Homebrew, which builds from source (slow on first install). Add `aarch64-apple-darwin` and
      `x86_64-apple-darwin` matrix entries. **next**
- [ ] **Windows.** A real undertaking — terminal, clipboard, file paths, child-process plumbing all need
      audit. Probably ConPTY + `arboard` Windows backend + `\\?\` long-path handling. Consider only after the
      editor is feature-complete enough to be worth the porting cost. **considering**
- [ ] **Nix flake.** `nix run github:bgunnarsson/binvim` and a flake output for use in a system config.
      **planned**
- [ ] **`cargo install binvim` from crates.io.** Currently install paths are Homebrew tap or `install.sh`;
      crates.io would catch the Rust-tooling crowd. Requires the licence story to permit it (source-available
      — verify). **considering**

## Architecture / non-goals

These are explicit decisions worth recording so they don't get relitigated every release.

- **No plugin system.** Every language, formatter, and LSP is hard-wired. Adding a language is a five-file PR
  (see CLAUDE.md). This keeps the binary self-contained and the codebase greppable.
- **No in-binary LLM client, no chat sidebar.** binvim doesn't embed an HTTP client to talk to Anthropic /
  OpenAI / Gemini / etc. directly, and there's no `:claude`-style conversation pane bolted onto the editor.
  Users who want chat-driven coding run a dedicated tool (Claude Code, Aider, etc.) alongside binvim —
  terminal multiplexers and split panes are the integration layer. This rules out direct API integrations as
  first-class infrastructure but **not** AI features that speak LSP — Copilot, supermaven, codeium-lsp, tabby,
  and any future server implementing `textDocument/inlineCompletion` are wired the same way as rust-analyzer
  or tsserver, with no HTTP stack on binvim's side. See the LSP / Editor sections.
- **Source-available, not open source.** See `LICENSE`. Contributions welcome under the existing terms;
  redistribution and forks are governed by the licence.
- **Single binary, no runtime config beyond `~/.config/binvim/config.toml`.** No init script, no Lua /
  Vimscript layer, no `:source`-able files.
- **Catppuccin Mocha is the only built-in theme.** Colour overrides go through `config.toml`. No theme-pack
  ecosystem.
