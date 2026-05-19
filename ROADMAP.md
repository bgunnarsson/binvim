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
- [x] **Lazygit integration (`:lazygit` / `:lg` / `<leader>gg`).** Shipped as a yazi-style
      full-screen takeover rather than a PTY-embedded pane. binvim suspends (pops kitty keyboard
      protocol, disables mouse capture, leaves the alt screen, drops raw mode), spawns `lazygit` as
      a foreground child with the host terminal handed directly to it via inherited stdio, and
      blocks until exit. On return we reclaim the terminal (re-enable raw mode, re-enter alt
      screen, re-arm mouse capture + keyboard enhancement flags) and call
      `refresh_all_git_hunks` so every open buffer's git gutter reflects the new index / worktree
      state, plus refresh the status-line branch label. The takeover model gives lazygit the full
      screen — its UI hard-codes panel widths against terminal cols, and the bottom `:terminal`
      pane caps out at 20 rows — and clean exit detection for free: when the blocking `status()`
      call returns, lazygit is done. No PTY plumbing, no tab management, no SIGWINCH dance.
      `<leader>g` became a git sub-leader; grep (formerly `<leader>g`) moved to `<leader>G`.
      `<leader>g` hold surfaces the hint via the which-key popup.
- [x] **AI side pane file-context handoff — shift-pair leader bindings.** Each AI tool has
      two leader bindings now: a lowercase letter opens the tool as-is, an uppercase letter
      opens it AND pre-types `@<cwd-relative path>` into the input field once the tool is
      ready. `<leader>jc` / `<leader>jC` for Claude, `<leader>jx` / `<leader>jX` for Codex,
      `<leader>jo` / `<leader>jO` for opencode. The shift-pair convention puts the choice
      one keypress away — same letter, just hold shift when you want file context. No config
      knob and no global "path handoff on/off" toggle; the per-invocation decision is the
      knob. `:claude` / `:codex` / `:opencode` ex commands open without handoff.
      The handoff writes bytes after a per-tool quiet window (Claude ~300ms, Codex ~800ms,
      opencode ~1500ms) so the input field is wired up before the path arrives and the
      front of the string doesn't get eaten by startup. You press Enter to submit. Earlier
      auto-submit attempts (drip at typing cadence + discrete `\r`, atomic + pause, `\r`
      appended, various delays) failed to settle reliably across all three tools — each
      classified the Enter differently depending on context (autocomplete capture, paste-
      mode newline, debounce window). Pre-typing the path is the part that worked
      universally, so that's where we landed.
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
- [x] **Project-wide refactor UI (LSP rename).** Shipped — `<leader>r` now opens a modal preview
      overlay between the server's `WorkspaceEdit` reply and the on-disk apply. Layout: file
      headers (path + edit count) with one selectable checkbox row per edit underneath, scrollable
      with `j`/`k` + `Ctrl-D`/`U` + `g`/`G`. Per-row `<Space>` toggle, `a` / `n` flip every edit
      on / off, `o` jumps to the cursor edit (cancels the preview), `<Enter>` applies only the
      enabled edits, `<Esc>` cancels the whole rename. Implementation split `apply_workspace_edit`
      into `parse_workspace_edit` (JSON → typed `ConcreteEdit`) + `apply_concrete_edits` (writer);
      the rename path builds a `RenamePreview` from the parsed list and stores it on App, the
      writer is reused by the apply path. Code actions and the server-initiated `workspace/applyEdit`
      flow still apply blind — they're typically single-file quick fixes where a preview would just
      add friction. Lives on a dedicated `Mode::RenamePreview`; the test-results-style overlay-
      passthrough gates don't apply (the preview is strictly modal).
- [x] **Refactor preview v2 — same UI for `workspace/applyEdit` + code actions.** Shipped as
      an opt-in via `[lsp] preview_workspace_edits = true` (default off — most code actions are
      still single-file quick fixes where a confirmation step is friction). When on, code-action
      `WorkspaceEdit`s and server-initiated `workspace/applyEdit` requests route through the
      same modal overlay rename already uses. `RenamePreview` grew a `kind: PreviewKind`
      discriminating Rename / CodeAction / ApplyEditFromServer; renderer + accept handler match
      on the variant for the title bar + status formatting. For server-initiated apply: the
      preview state stashes `(client_key, request_id)`; accept replies `applied: true`, cancel
      replies `applied: false`, no-preview / empty-edit / second-while-open replies `false`
      immediately so the server doesn't hang.
- [x] **Workspace folders / multi-root.** Shipped — `LspClient` carries a runtime-mutable set of
      attached `workspace_folders` plus a `workspace_folders_supported` flag captured from the
      `initialize` response (object form `{"supported": true}` and the older bool shorthand both
      honoured). `ensure_for_path` now branches on "client exists?": if yes and the file's resolved
      root isn't already attached, `add_workspace_folder` appends it and (when the server supports
      the cap) fires `workspace/didChangeWorkspaceFolders` with the new folder; if no, spawn as
      before. Net effect: opening files from sibling repos into the same session grows the
      existing server's known-folder set instead of spawning a second process. rust-analyzer /
      tsserver / gopls / jdtls all advertise the cap; the few that don't fall back to the
      previous "first root wins" behaviour with no change. Surfaced via `:workspaces` / `:ws`,
      which dumps `key: ~/path  +  ~/path · key: ~/path` to the status line.
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
- [x] **Conditional + hit-count breakpoints.** Shipped via `:dapb` arg forms — `:dapb if <expr>`
      attaches a `condition`, `:dapb hit <expr>` attaches a `hitCondition` (DAP-style: bare
      integer for "pause after N hits", `>= 5` for comparators); both create an unconditional
      breakpoint first if none exists at the line, so it's one keystroke from cold.
      `:dapb plain` strips both fields while keeping the breakpoint as an unconditional pause;
      `:dapb if` / `:dapb hit` with no arg clears just that field. Aliases: `cond` / `condition`
      / `hitcount` / `clear`. Sites with either field render as `◆` in the gutter (vs `●` for
      plain) and the breakpoints pane lists each row's expression inline. Side fix: extracted
      `encode_source_breakpoint` so `resend_breakpoints_for` (the post-toggle path) carries
      `condition` + `hitCondition` — the previous code built `{"line": N}` inline and silently
      dropped both, so a conditional set before a toggle reverted to plain on the next adapter
      sync.

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

## Task runner

- [x] **Task runner (`:task` / `:tasklast` / `<leader>m{m,l}`).** v1 shipped — discovery + picker +
      labelled bottom-terminal-tab execution. Five sources unioned per workspace: **npm scripts**
      (npm / pnpm / yarn auto-picked from the lockfile), **Justfile** recipes (skips `_private` +
      `[private]`), **cargo aliases** + the builtin verbs (`build` / `check` / `test` / `clippy` /
      `run` / `fmt` / `doc`), **Makefile** top-level targets, and **dotnet** verbs (`build` / `run`
      / `test` / `restore` / `clean` / `publish`). Each source has its own root walk in
      `src/task/specs.rs` so a pnpm project nested inside a cargo workspace yields both. Picker
      rows tag the source for disambiguation (`npm  dev  · vite`); selecting a task spawns
      `$SHELL -l -i -c "cd <cwd> && exec <command>"` in a fresh bottom-terminal tab whose label
      (rendered in the tab strip) is the task name. Reuses everything the `:terminal` pane
      already gives — vte grid, scrollback, mouse forwarding, multi-tab.
      Deliberately **not** a dedicated streaming overlay (the test runner has one; tasks
      don't need per-event parsing — the terminal grid does it for free).
- [x] **Task runner v2 — quickfix scrape + long-running annotation.** Shipped. On a task tab's
      child-process exit, the visible grid + scrollback is scraped for `path:line:col[:end_col]:
      <msg>` (gcc / clang / rustc / ruff / biome / eslint / generic POSIX) and `path(line,col):
      <msg>` (tsc legacy); matches replace the quickfix list so `]q` / `[q` walks compiler
      errors after a `build`. Log lines like `12:34:56 INFO: …` and rustc's `-->` decoration
      are filtered. `Terminal::poll_exit()` (first-fire `try_wait`) drives the detection;
      `Grid::text_lines()` extracts the post-vte-parsed text. Long-running tasks (label
      containing `dev` / `watch` / `serve` / `start` / `preview` as a word token) carry a
      `[bg]` badge in the picker; `:tasklast` on one of them appends a cautionary status
      hint about the previous instance possibly still being alive (the kickoff itself still
      proceeds — the user might genuinely want a second instance).

## Quality / Tooling

- [x] **CI: `cargo test` + `cargo clippy` on PRs.** `.github/workflows/ci.yml`
      runs `cargo test --locked` and `cargo clippy --locked --all-targets`
      on every push to main and every PR. Concurrent runs cancel on rapid
      pushes to the same ref.
- [x] **CI: `cargo fmt --check`.** Shipped. `rustfmt.toml` at the repo root pins
      `max_width = 100` + `single_line_let_else_max_width = 100` so the compact
      `let Some(x) = … else { return; };` and single-line method chains the codebase
      relies on survive the formatter. The 0.4.4 cut ran `cargo fmt` once to normalise
      the tree (73 files, ~5k line edits, no behaviour change); the new `fmt` job in
      `ci.yml` keeps it that way going forward. Style policy decision was: preserve
      what the config can preserve, accept everything else as a one-shot diff.
- [x] **Crash-handler.** `panic::set_hook` installed before any terminal-
      touching code: best-effort restores the terminal (disables raw mode,
      leaves alt screen, shows cursor, drops kitty keyboard flags) and
      writes a diagnostic log (payload + location + force-captured
      backtrace + binvim version + unix timestamp) to
      `~/.cache/binvim/crash/<ts>.log`. The path is echoed to stderr after
      the unwind so the user knows where to look.
- [x] **Property tests for motion / text-object.** `proptest` drives the new property block at the
      bottom of `src/motion.rs` and `src/text_object.rs`: every motion lands in bounds (`col <=
      line_len`, since exclusive-motion targets like `dw` legitimately sit one past the last char),
      `left+right` round-trips when there's room, `word_forward` is non-retreating in linear
      position and `word_backward` is non-advancing, `goto_line` clamps to the last line, and
      `find_char` never crosses lines. `compute(verb)` on text-objects always returns
      `start <= end <= total_chars`, and around-form ⊇ inner-form for word / quotes / pair.
      Catches off-by-one bugs that the named-case unit tests miss.
- [x] **Fuzz tree-sitter + LSP message dispatch.** Proptest-driven panic-hardening rather than
      libFuzzer/cargo-fuzz — stays on stable, runs as part of `cargo test`, and the existing CI
      gate covers it. `compute_byte_colors` is fuzzed against arbitrary UTF-8 across every
      `Lang` variant binvim ships (a future tree-sitter ABI bump or query edit that makes a
      capture range exceed `source.len()` would surface here). Every `parse_*_response` in
      `lsp/parse.rs` plus the three `extract_*` initialize-response inspectors and
      `parse_publish_diagnostics` in `lsp/io.rs` get a recursive arbitrary-`Value` JSON
      generator — a malformed reply must produce empty / None, never panic the reader
      thread. Coverage-guided libFuzzer is a separate decision if we ever want deeper
      mutation; the surface this covers is the panic-on-weird-input one.

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
- [x] **`cargo install binvim` from crates.io.** `Cargo.toml` carries the crates.io metadata
      (repository, homepage, keywords, categories, anchored `include` whitelist so the playground tree
      stays out of the published tarball); `scripts/release.sh` runs `cargo publish --locked` as step
      3b — after the bump commit lands on main, before the tag push — so a failed publish doesn't leave
      a dangling tag / GitHub Release / Homebrew bump pointing at a non-existent crates.io version. The
      pre-flight in step 1 checks for either `CARGO_REGISTRY_TOKEN` or a credentials file under
      `${CARGO_HOME:-$HOME/.cargo}/` so the publish step never fails midway through for a missing
      token. `cargo install --locked binvim` is the install path; both `binvim` and `binvim-install`
      land in `~/.cargo/bin/`.

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
