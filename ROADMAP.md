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
- [x] **Built-in `:terminal` split.** `:terminal` (or `:term`,
      `<leader>to`) opens a PTY-backed shell as a bottom split pane
      that stacks above the debug pane and below the editor. Single
      mode — `Mode::Terminal` forwards every keystroke (including
      `Esc`) to the PTY, so it behaves like any other terminal.
      `<C-w>` is the escape hatch: drops to Normal + primes the
      window-leader parser so `<C-w>k`, `<C-w>q`, `<C-w>>`, etc.
      continue to work. `<leader>tf` re-focuses the pane.
      Selection / copy uses the host terminal app's native
      Shift+drag → Cmd-C (no Vim-style yank reinvented). Mouse
      forwarding to the PTY when DECSET 1000/1002/1003/1006 is set
      (htop, vim mouse=a, less mouse mode); otherwise clicks pull
      focus. SGR + legacy X10 encodings both supported. vte-parsed
      grid with full SGR colour + attrs, CUP / CUF / CUB / CUU /
      CUD / CHA cursor moves, ED / EL clears, IND / RI / DECSC /
      DECRC / RIS, line wrap into a 10k-row scrollback.
- [x] **Multiple terminals (in tabs).** The `:terminal` pane hosts
      a `Vec<Terminal>` instead of a single `Option<Terminal>`.
      `<leader>tt` (or `:terminal`) always spawns a new tab and
      focuses it; the previous "open or focus existing" semantics
      moved to `<leader>tf`. With one terminal the header keeps its
      hint line; with two or more it sprouts a clickable tab strip
      (active tab = blue bg + white text; inactive = muted on the
      pane bg). `<leader>tq` closes the active tab — if it was the
      last, the pane hides. Each tab owns its own PTY + 10k
      scrollback + cursor state; the pane only ever renders the
      active one, but every tab keeps draining on every frame so
      `pnpm dev` / `cargo watch` / a long build don't stall while
      focus is on a sibling tab. Host-terminal resizes are
      broadcast to every PTY so background tabs don't reflow on
      switch. Keyboard nav between tabs is left for a follow-up
      (click is the path today).
- [x] **Built-in sidebar tree file explorer.** Opt in via
      `[file_explorer] tree = true` in `~/.config/binvim/config.toml`
      (default `false` keeps yazi). `<leader>e` toggles a left-side
      tree pane rooted at the cwd; `editor_rect` trims width from
      the left so buffer panes and the right-side AI terminal pane
      sit cleanly to its right. j/k navigate, Enter/l opens a file
      or expands a folder, h collapses or jumps to parent, g/G top
      / bottom, r rebuilds. Three-state `<leader>e` toggle (closed →
      focused → unfocused-but-visible → closed) so a click into the
      editor drops focus without losing the pane. Two row styles:
      a `theme_surface` highlight follows the j/k cursor; the file
      open in the focused editor window renders in the accent
      colour + bold so it stays identifiable independently. Same
      `icon_for_basename` Nerd Font glyph the picker uses for
      files; `\u{f07b}` / `\u{f07c}` for folders. Double-click in
      the pane opens. File ops: `a` creates an entry under the
      cursor's parent directory (trailing `/` makes a folder;
      intermediate dirs are auto-created; refuses paths containing
      `..` or leading `/`); `r` renames the cursor entry, prompt
      pre-filled with the basename (basename-only — `/` is refused;
      the buffer's path is rewritten if the renamed file is open so
      saves keep landing); `d` arms a delete and the next key
      consumes the y/N confirmation (`remove_dir_all` for folders;
      anything other than `y`/`Y` cancels). `R` rebuilds (was both
      `r` and `R`; `r` moved to rename).
- [ ] **AI side pane file-context handoff.** When the active buffer
      has a path, pre-type `@<project-relative path> ` into the
      newly-opened `:claude` / `:codex` / `:opencode` side pane so
      the tool starts the conversation already aware of what the
      user is editing. Universal — all three tools accept `@`-prefix
      file references — and tool-agnostic (no per-tool CLI flag
      coupling). Doesn't disable each tool's normal project-wide
      context (CLAUDE.md / AGENTS.md auto-load, on-demand file
      reads); the `@` reference is additive, not exclusive. The
      cost worth flagging: `@<path>` inlines the file's contents
      into the first turn for most tools, so every open of an
      assistant against a large file (think 1k+ lines, generated
      schemas, JSON dumps) eats 3–5k+ tokens before the user types
      anything. Two design knobs to decide before shipping:
      (a) every invocation vs. only the first focus per session
      (so re-focusing an existing tab doesn't keep stuffing
      `@path` into an ongoing conversation); (b) selection-aware
      ranges (`@src/foo.rs:42-58`) when Visual mode is active vs.
      file-only. **considering**
- [x] **Cmdline & search history.** `:<Up>` cycles previous ex commands; `/<Up>` cycles previous searches.
      Capped at 100 entries, dedup against the immediate previous, independent rings for `:` vs `/`. Persisted
      to the existing per-cwd session JSON; histories load even on `binvim foo.rs` so recall stays warm
      regardless of launch mode.
- [x] **Tab completion in `:` ex commands.** Cycle on `Tab` / `Shift-Tab`. Three completion kinds picked
      by the head of the cmdline: command names from a static list before the first space; filesystem
      entries (with `/` suffix on directories, dotfiles hidden unless the basename starts with `.`) after
      `:e` / `:edit` / `:w` / `:write`; open-buffer basenames after `:b` / `:buffer`. Any non-Tab key
      (typing, Backspace, history walk) drops the cached cycle so the next Tab re-derives candidates
      against the latest cmdline text.
- [x] **Macro polish.** Macros (`q<reg>` record, `@<reg>` replay, `@@` repeat) had been shipping bare;
      five gaps on top of that are now in: (1) count prefix on replay — `5@a` runs the macro five times;
      (2) `Q` as a one-keystroke alias for `@@`; (3) `macros: HashMap<char, Vec<KeyEvent>>` persists to the
      per-cwd session JSON alongside the existing cmdline / search history (the in-memory `KeyEvent` is
      mapped to a serde-friendly `SessionKey` — tagged code + modifier bitset — on save, mapped back on
      load); (4) `:reg` / `:registers` opens a scrollable overlay listing yank registers AND macro
      registers in one view, control chars rendered as `^X`, macro keys as `<C-x>`/`<Esc>`/`<CR>`/etc.;
      (5) `MACRO_REPLAY_DEPTH_LIMIT` (200, Vim's default) caps nested replay so `qa@aq` then `@a` aborts
      with a status message rather than wedging the editor.
- [x] **Spell check.** Toggleable per-buffer via `:spell`; `]s` / `[s` walk between misspellings;
      `z=` opens a suggestion picker for the word under the cursor. No external library — the wordlist
      loads from `~/.local/share/binvim/words` (override) or `/usr/share/dict/words` (system default);
      check is a `HashSet` membership against lowercased tokens, suggestions enumerate every
      single-edit neighbour (insert / delete / substitute / transpose) and filter against the same
      set. The tokeniser splits camelCase / snake_case / kebab-case so a `getPlayerName` only trips
      on the constituents the dictionary doesn't know; pure-uppercase abbreviations and tokens
      shorter than 3 chars are skipped to keep the false-positive rate low on source code. The cache
      is version-keyed per buffer; recomputation happens lazily the next time `]s` / `[s` is
      pressed after an edit. Render-side undercurl on misspelled spans isn't wired yet — navigation
      is the source of truth for now.
- [x] **Large-file mode.** `Buffer::is_large()` trips when the rope crosses 5MB or 50k lines (constants
      `LARGE_FILE_BYTES` / `LARGE_FILE_LINES` in `src/buffer.rs`). The gate short-circuits
      `ensure_highlights` (so tree-sitter never runs against multi-MB buffers), `lsp_attach_active`,
      `lsp_sync_active`, and `lsp_sync_active_debounced` (so no server ever sees the file). A status-line
      hint ("large file — tree-sitter + LSP disabled") fires on the first open via either the CLI
      (`binvim huge.json`) or `:e`. The rope itself handles the byte volume fine, so editing / scrolling
      / yank / undo all work — only the syntax pass and LSP traffic are suppressed.
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
- [x] **Code lens.** `textDocument/codeLens` for "Run test" / "Debug test" / reference counts above
      declarations, opt in via `[lsp] code_lens = true`. Renders as virtual text on the row above the
      anchor (per-line vertical offset in the render walk + viewport re-measure against it). The cache
      lives parallel to `inlay_hints` / `semantic_tokens` (per-buffer + version-keyed, request-on-due in
      the render loop, one in-flight at a time per path) and merges LSP-only results with a synthesized
      fallback (`src/code_lens_synth.rs`) so languages whose servers don't ship the capability — or ship
      it gated behind an experimental flag — still get a "Run test" / "Debug test" lens above each
      detected test function. Click (or a keybind on the anchor line) invokes the lens's `command` field;
      server-side commands like rust-analyzer's `rust-analyzer.runSingle` are intercepted client-side and
      routed into the integrated test runner (`cmd_test_nearest` codepath) so lens + `:testnearest` share
      one engine.
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
- [ ] **Conditional + hit-count breakpoints.** Wire format is already plumbed — `BreakpointSpec`
      carries an `Option<String> condition` field that flows through `setBreakpoints`
      (`dap/manager.rs:964`). What's missing is the UI to set it: an ex-command (`:dapbreak <expr>`
      style) or a keybind that prompts for the condition on the current line. **considering**

## Test runner

- [x] **Integrated test runner (cargo).** Per-language adapter pattern parallel to DAP — one
      `TestAdapterSpec` per toolchain in `test/specs.rs`, picked by walking the active buffer up for the
      adapter's root markers. UI surface: `:test` opens a fuzzy picker of discovered tests (cargo's
      `cargo test -- --list --format=terse` for now); `:testnearest` walks the buffer up for
      `#[test]` / `#[tokio::test]` / `#[rstest]` / `#[async_std::test]` and runs the enclosing fn;
      `:testfile` derives a libtest substring filter from the active path's module location; `:testlast`
      re-runs the most recent request; `:testcancel` kills the in-flight adapter. Streaming results render
      into a `:health`-style scrollable overlay (`j`/`k`/`Ctrl-D`/`Ctrl-U`/`g`/`G`, dismiss with Esc /
      `q` / `:q`); pass / fail / ignored counts surface in the status line on completion. Failures populate
      the quickfix list with parsed `panicked at FILE:LINE:COL` locations so `]q` / `[q` walks them.
- [x] **vitest adapter.** `src/test/vitest.rs` + `VITEST` entry in `BUILTIN_ADAPTERS`. Root markers walk
      for `vitest.config.{ts,mts,js,mjs,cjs}`; the workspace lookup prefers vitest over cargo so a nested
      JS project inside a cargo workspace picks the right runner. Streaming JSON reporter parser feeds
      the same overlay + quickfix as cargo.
- [x] **pytest adapter.** `src/test/pytest.rs` + `PYTEST` entry in `BUILTIN_ADAPTERS`. Root markers
      `pytest.ini`, `pyproject.toml`, `setup.cfg`, `tox.ini`, `conftest.py`. Runs with `pytest -v
      --tb=line --color=no`; the streaming verdict line is `path::test_name PASSED [ NN%]`. Failure
      locations come from the `--tb=line` `<path>:<line>: ExceptionName: msg` row; messages fall back to
      the `FAILED path::test - …` short-summary row when `--tb=line` couldn't pin a location.
      `filter_for_nearest` walks upward for the closest `def test_*` / `async def test_*`; class-based
      tests rely on `-k <method_name>` substring matching, which xUnit / unittest also recognise.
- [x] **go test adapter.** `src/test/gotest.rs` + `GOTEST` entry. Root marker `go.mod`. Runs with `go
      test -v -run ^<name>$ ./...` (or a positional `./pkg/...` path filter); the parser tracks `=== RUN`
      → `--- PASS/FAIL/SKIP` pairings and grabs the indented `    file_test.go:LINE: msg` line for
      failure locations. Subtests (`TestParent/case_one`) keep their full slash-separated names so
      `:testlast` re-runs them faithfully. `filter_for_nearest` recognises `func Test* / Benchmark* /
      Example* / Fuzz*`.
- [x] **dotnet test adapter.** `src/test/dotnet.rs` + `DOTNET` entry. Root markers `*.sln`,
      `*.csproj`, `*.fsproj`. Runs with `dotnet test --logger:"console;verbosity=normal"`; the
      streaming reporter prints `Passed FQN [Nms]` / `Failed FQN [Nms]` / `Skipped FQN [Nms]` per test.
      `Error Message:` blocks collapse into the per-failure message, `Stack Trace:` `in <path>:line N`
      rows feed the location. `FullyQualifiedName~<filter>` substring matching by default; raw
      `--filter` expressions (containing `=` / `!=` / `&` / `|`) pass through verbatim.
      `filter_for_nearest` recognises `[Fact]` / `[Theory]` / `[Test]` / `[TestMethod]` / `[TestCase]`
      attribute-decorated methods.
- [x] **Debug test.** `:debugtest` (alias `:dt`) walks up from the cursor for the enclosing test
      function, then routes through the DAP layer instead of the test runner. `LaunchContext` carries
      two new fields — `test_filter` (the name) and `test_file` (the source path) — which the per-
      adapter `build_launch_args` consults to emit a test-mode invocation. Wired for pytest
      (`module: pytest`, `args: [<file>::<test>, -s]`) and go (`mode: test`, `args: ["-test.run",
      "^<name>$", "-test.v"]`). cargo / dotnet / vitest fall back to a "not yet supported" status
      message — wire format is in place, the per-adapter test-binary discovery is the open part.

## Quality / Tooling

- [x] **CI: `cargo test` + `cargo clippy` on PRs.** `.github/workflows/ci.yml`
      runs `cargo test --locked` and `cargo clippy --locked --all-targets`
      on every push to main and every PR. Concurrent runs cancel on rapid
      pushes to the same ref.
- [ ] **CI: `cargo fmt --check`.** Cheap on paper, but the codebase has
      hand-rolled formatting (compact let-else, single-line method chains)
      that stock rustfmt rewrites in ~560 places. Needs a conscious
      style-policy decision before turning the gate on. **considering**
- [x] **Crash-handler.** `panic::set_hook` installed before any terminal-
      touching code: best-effort restores the terminal (disables raw mode,
      leaves alt screen, shows cursor, drops kitty keyboard flags) and
      writes a diagnostic log (payload + location + force-captured
      backtrace + binvim version + unix timestamp) to
      `~/.cache/binvim/crash/<ts>.log`. The path is echoed to stderr after
      the unwind so the user knows where to look.
- [ ] **Property tests for motion / text-object.** Both modules are pure functions — good targets for
      `proptest`. The existing unit tests cover named cases; properties would surface boundary bugs on
      Unicode, empty buffers, multi-byte sequences. **planned**
- [ ] **Fuzz tree-sitter + LSP message dispatch.** Dual-purpose: hardens parsers and exercises edge cases in
      the JSON-RPC reader. **considering**

## Distribution

- [x] **macOS prebuilt binaries in `release.yml`.** Matrix gained
      `aarch64-apple-darwin` (macos-14 runner) and `x86_64-apple-darwin`
      (macos-13). Each target builds natively per arch — no cross-compile
      toolchain to wrangle — and the resulting binary picks up the host's
      codesigning so Gatekeeper doesn't trip on first launch. `install.sh`
      now resolves `Darwin/{arm64,x86_64}` so the `curl … | sh` path works
      on Mac too.
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
- **Themes ship as `themes/<name>/theme.toml` copy-paste presets, not a theme registry.** The default is
  Catppuccin Mocha (baked into the chrome-palette defaults so a config-less install renders correctly).
  `themes/` carries 15 presets covering both dark and light schemes (`catppuccin-mocha`, `catppuccin-latte`,
  `dracula`, `tokyo-night`, `night-owl`, `light-owl`, `one-dark`, `gruvbox`, `nord`, `github-dark`,
  `github-light`, `solarized-light`, `ayu-light`, `monokai`, `visual-studio`). There is no theme loader —
  copy a preset's `[colors]` block into `config.toml`. No theme-pack ecosystem, no installer, no plugin
  registry.
