# Project layout

```
src/
  main.rs          parses one optional path, installs the panic hook, hands off to App::run
  lib.rs           exports only `install`, so the editor and binvim-install share the catalog
  bin/
    binvim-install.rs  standalone CLI installer (checkbox picker → confirm → run) over binvim::install
  app.rs           slim entry — App struct + new/run + TerminalGuard
  app/
    state.rs       supporting types (Register, BufferStash, HoverState, …), constants, top_overlay
    pair.rs        bracket and HTML tag matching + auto-pair helpers
    view.rs        viewport, scrolling, folds, highlight cache, tab-bar geometry
    search.rs      search, jumps, per-line range queries for the renderer
    registers.rs   registers, macros, dot-repeat, OS-clipboard mirror + OSC 52, replay_key
    buffers.rs     buffer switching, open/close, disk reload, recents, session hydration
    save.rs        save flow, formatter, .editorconfig on-save, git branch
    edit.rs        primitive edits — insert / replace / surround / undo / number / re-flow / align
    visual.rs      visual-mode helpers (incl. block + Ctrl-N multi-selection)
    multi_cursor.rs operator fan-out across the additional cursors in Normal mode
    comment.rs     `<leader>/` comment / uncomment toggle
    dispatch.rs    apply_action — operator / motion / text-object glue
    input.rs       per-mode key handlers, mouse handler, `:`-command dispatch, keymap_take
    cmdline_complete.rs  Tab completion in the `:` prompt
    cmdline_history.rs   `:` and `/` history — Up/Down recall, the `q:` window, persisted in the session
    lsp_glue.rs    LSP event handling, request helpers, snippet expansion, code lenses
    rename_preview.rs    LSP rename preview overlay — build / navigate / apply
    copilot.rs     Copilot ghost completions — sign-in status, idle debounce, Tab accept
    dap_glue.rs    DAP event handling, debug-pane focus mode, project / profile pickers, F-keys
    git_glue.rs    gutter hunks — ]h / [h, preview, stage, unstage, reset, blame
    picker_glue.rs picker open / handle / refilter, yazi shell-out
    grep_glue.rs   live-grep picker — debounced, backgrounded ripgrep
    quickfix.rs    quickfix list — populate from grep / references / diagnostics, navigate
    file_tree.rs   built-in sidebar tree explorer — state + key handler + click flow
    windows.rs     `<C-w>` split / focus / close / resize over layout.rs
    terminal_glue.rs      `:terminal` bottom pane — PTY spawn, key forwarding, selection, drain
    side_terminal_glue.rs right-side pane for the AI tools (`<leader>j…`)
    lazygit_glue.rs       `:lazygit` / `<leader>gg` full-screen suspend-and-resume
    task_glue.rs   task picker + spawn into a labelled terminal tab, `:make`
    test_glue.rs   `:test*` commands, TestEvent drain, quickfix + results overlay bridge
    package_glue.rs `<leader>p` package-manager pickers over background threads
    android_glue.rs `<leader>A` Android emulator manager over android.rs
    spell_glue.rs  per-buffer spell toggle, ]s / [s, z= picker
    tag_glue.rs    ctags jumps — Ctrl-], Ctrl-T, g], `:tag`
    installer.rs   `:install` / `:update` overlay over binvim::install + first-run toolchain prompt
    health.rs      `:health` output
    config_glue.rs `:config` open / reload / defaults
    update_glue.rs startup update check — spawn / drain / surface
    recover_glue.rs recovery files — periodic dump, apply on open, signal thread
  android.rs       Android SDK CLI backend (sdkmanager / avdmanager / adb / emulator) + parsers
  ansi.rs          ANSI / SGR parser + colour tables shared by terminal.rs
  buffer.rs        rope-backed text buffer (+ marks)
  code_lens_synth.rs client-side code lenses for vitest tests (tree-sitter walk)
  command.rs       ex-command (`:`) parser
  config.rs        config loader and colour resolution
  crash.rs         panic hook — restore the terminal, write ~/.cache/binvim/crash/
  cursor.rs        Cursor { line, col, want_col }
  cursor_cache.rs  per-file last-cursor cache, content-hash gated
  editorconfig.rs  .editorconfig parser + on-save transforms
  format.rs        formatter dispatch (one arm per extension; stdin→stdout helper + temp-file dance for tools without stdin support)
  git.rs           `git diff --unified=0` hunks, patch build / apply, blame — shells out to git
  install.rs       shared install catalog (BUNDLES), manager detection, plan builder, runner
  keymap.rs        `[keymaps]` — Vim key notation + per-mode mapping tables
  lang.rs          tree-sitter language detection and highlight cache
  layout.rs        window-split binary tree — partition + geometric focus_neighbor
  lsp.rs           slim entry — re-exports public API
  lsp/
    types.rs       wire-side types + URI helpers
    specs.rs       per-extension server dispatch + workspace discovery
    client.rs      LspClient — spawn + send/recv frames
    io.rs          reader-thread loop + JSON-RPC dispatcher
    manager.rs     LspManager — fan-out + response routing
    parse.rs       response parsers
  dap.rs           slim entry — re-exports public API
  dap/
    types.rs       wire-side types — DapIncoming / DapEvent / breakpoint / frame / variable structs
    specs.rs       adapter registry (.NET / Go / Python / lldb for Rust, C, C++), per-adapter target discovery, $PATH lookup
    client.rs      DapClient — spawn + stdin / stdout / stderr fan-out
    io.rs          reader-thread loop (Content-Length framing, same as LSP)
    manager.rs     DapManager — protocol state machine + drain
  markdown_render.rs hand-rolled markdown conceal transforms for Normal-mode `.md` buffers
  mode.rs          modes, VisualKind and operators
  motion.rs        motions
  package.rs       package-manager backends (NuGet / npm / Cargo / Go / PyPI) + http_get via curl
  parser.rs        keystroke → action parser
  paths.rs         home / config / cache dirs, write_atomic, others_can_plant
  picker.rs        fuzzy pickers
  recover.rs       recovery-file format + paths
  render.rs        terminal rendering (incl. tab bar and every overlay page)
  session.rs       per-workspace session persistence
  spell.rs         spell check over the system wordlist
  tag.rs           ctags `tags` file parsing + address resolution
  task.rs          slim entry — task runner (discover_all over the sources below)
  task/
    types.rs       adapter-agnostic task types
    specs.rs       discovery registry — walks up once per source and unions results
    cargo_aliases.rs  cargo built-in verbs + `[alias]` entries
    dotnet.rs      dotnet verbs against the enclosing .sln / .csproj
    justfile.rs    justfile recipes
    makefile.rs    Makefile targets
    npm_scripts.rs package.json scripts
  terminal.rs      PTY-backed terminal model — grid, cursor, scrollback, shell launch
  test.rs          slim entry — test runner
  test/
    types.rs       TestEvent and run types
    specs.rs       adapter registry (root markers → adapter)
    manager.rs     TestManager — spawn, reader thread, drain
    cargo.rs       `cargo test` discovery + libtest output parser
    dotnet.rs      `dotnet test` discovery + parser
    gotest.rs      `go test -v` discovery + parser
    pytest.rs      `pytest` discovery + parser
    vitest.rs      `vitest` discovery + verbose-reporter parser
  text_object.rs   text objects (`iw`, `i"`, `ap`, …)
  undo.rs          undo/redo history (in-memory + on-disk persistence)
  update.rs        crates.io "newer binvim?" check, cached 24h
  window.rs        Window — per-pane cursor / viewport / visual anchor / buffer index
```
