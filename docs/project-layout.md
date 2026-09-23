# Project layout

```
src/
  app.rs           slim entry — App struct + new/run + TerminalGuard
  app/
    state.rs       supporting types (Register, BufferStash, HoverState, …)
    pair.rs        bracket and HTML tag matching + auto-pair helpers
    view.rs        viewport, scrolling, folds, highlight cache, tab-bar geometry
    search.rs      search, jumps, per-line range queries for the renderer
    registers.rs   registers, macros, dot-repeat, OS-clipboard mirror
    buffers.rs     buffer switching, open/close, disk reload, recents, sessions
    save.rs        save flow, formatter, .editorconfig on-save, git branch
    edit.rs        primitive edits — insert / replace / surround / undo / number / multi-cursor mirror
    visual.rs      visual-mode helpers (incl. block + Ctrl-N multi-selection)
    dispatch.rs    apply_action — operator / motion / text-object glue
    input.rs       per-mode key handlers, mouse handler, `:`-command dispatch
    lsp_glue.rs    LSP event handling, request helpers, snippet expansion
    dap_glue.rs    DAP event handling, debug-pane focus mode, project / profile pickers
    picker_glue.rs picker open / handle / refilter, yazi shell-out
    file_tree.rs   built-in sidebar tree explorer — state + key handler + click flow
    health.rs      `:health` output
    recover_glue.rs recovery files — periodic dump, apply on open, signal thread
  buffer.rs        rope-backed text buffer
  command.rs       ex-command (`:`) parser
  config.rs        config loader and colour resolution
  cursor.rs        cursor + visual selection model
  editorconfig.rs  .editorconfig parser + on-save transforms
  format.rs        formatter dispatch (one arm per extension; stdin→stdout helper + temp-file dance for tools without stdin support)
  lang.rs          tree-sitter language detection and highlight cache
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
    specs.rs       adapter registry (.NET / Go / Python / Rust), per-adapter target discovery, $PATH lookup
    client.rs      DapClient — spawn + stdin / stdout / stderr fan-out
    io.rs          reader-thread loop (Content-Length framing, same as LSP)
    manager.rs     DapManager — protocol state machine + drain
  mode.rs          modes and operators
  motion.rs        motions
  parser.rs        keystroke → action parser
  picker.rs        fuzzy pickers
  recover.rs       recovery-file format + paths
  render.rs        terminal rendering (incl. tab bar)
  session.rs       per-workspace session persistence
  text_object.rs   text objects (`iw`, `i"`, `ap`, …)
  undo.rs          undo/redo history (in-memory + on-disk persistence)
```
