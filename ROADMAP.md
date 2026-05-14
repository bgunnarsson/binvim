# Roadmap

This is a directional roadmap, not a commitment. Items move between
sections as priorities shift. The goal is for binvim to be a viable
daily-driver replacement for Neovim across the languages it ships
support for — without growing a plugin system.

Status legend: **next** = actively in scope, **planned** = agreed
direction, **considering** = open question.

## Editor

- [ ] **Window splits — `<C-w>v` / `<C-w>s` / `<C-w>h/j/k/l`.** Currently
  every buffer takes the full editor area; tabs cover "many open files"
  but not "two files side-by-side." This is the single biggest Vim
  feature still missing. Touches `App` (window tree instead of one
  active buffer), `render.rs` (rect partitioning), and viewport math
  per-pane. **next**
- [ ] **Built-in `:terminal` split.** A pane running a shell, with a
  way to yank from its scrollback. Reuses the split work above. After
  splits land. **planned**
- [ ] **Cmdline & search history.** `:<Up>` cycles previous ex
  commands; `/<Up>` cycles previous searches. Persist across sessions
  alongside the existing session file. **next**
- [ ] **Tab completion in `:` ex commands.** Filenames after `:e`,
  buffer names after `:b`, command names from cold. **planned**
- [ ] **Spell check.** Toggleable per-buffer, with `]s` / `[s` to jump
  between misspellings and `z=` for suggestions. Useful for prose +
  comments. **considering**
- [ ] **Large-file mode.** Skip tree-sitter + LSP attach when the
  buffer crosses a size threshold (e.g. 5MB or 50k lines), with a
  status hint. The rope handles the byte volume fine; the highlight
  pass is what dies. **planned**

## LSP

- [ ] **Semantic tokens.** `textDocument/semanticTokens/full` and
  `…/range`, layered on top of the tree-sitter pass. Servers like
  rust-analyzer / tsserver / clangd carry richer info than any static
  query (e.g. mutable vs immutable bindings, async functions). **next**
- [ ] **Document highlight.** `textDocument/documentHighlight` —
  highlight every other occurrence of the symbol under the cursor in
  the current buffer. Standard editor affordance. **planned**
- [ ] **Code lens.** `textDocument/codeLens` for things like "Run test"
  / "Debug test" / reference counts above declarations. Renders as
  virtual text on the line above the anchor. **planned**
- [ ] **Workspace folders / multi-root.** Currently one project root
  per buffer; opening files from a sibling repo doesn't fan a second
  workspace into the same client. Important for monorepos. **considering**
- [ ] **`window/showMessage` and `window/logMessage` surfacing.**
  Server-emitted notifications and logs route to the notification box
  / a `:messages`-like buffer instead of being dropped. **planned**

## Debugger (DAP)

- [ ] **delve (Go).** Second adapter. The registry was built for this.
  **next**
- [ ] **debugpy (Python).** Third adapter. **planned**
- [ ] **lldb-dap (Rust / C / C++).** Native-code debugging closes the
  loop on the systems-language side. **planned**
- [ ] **Watch expressions.** A user-managed list above locals,
  evaluated via `evaluate` per stop. **planned**
- [ ] **Conditional + hit-count breakpoints.** Existing breakpoints are
  unconditional; DAP `breakpoint.condition` / `hitCondition` already
  carry the wire format. **considering**

## Git

- [ ] **`:Gstatus` panel.** A split listing modified / staged /
  untracked files with `s` / `u` / `d` / `=` shortcuts. Currently you
  shell out for status; the gutter + `<leader>h*` cover hunks but not
  cross-file overview. **planned**
- [ ] **`:Glog` viewer.** Scrollable commit history for the current
  file or whole repo, with `<CR>` to open a diff view. **considering**
- [ ] **Commit from editor.** `:Gcommit` opens a buffer with the
  message template; `:wq` runs the commit. **considering**

## Quality / Tooling

- [ ] **CI: `cargo test` + `cargo clippy` on PRs.** Today only
  `release.yml` runs (on tag push). Tests exist but nothing gates them.
  **next**
- [ ] **CI: `cargo fmt --check`.** Cheap, catches drift. **next**
- [ ] **Crash-handler.** Catch panics in the event loop, restore the
  terminal, write the panic + buffer state to `~/.cache/binvim/crash/`,
  and exit cleanly. Currently a panic leaves the terminal in raw mode.
  **next**
- [ ] **Property tests for motion / text-object.** Both modules are
  pure functions — good targets for `proptest`. The existing unit
  tests cover named cases; properties would surface boundary bugs
  on Unicode, empty buffers, multi-byte sequences. **planned**
- [ ] **Fuzz tree-sitter + LSP message dispatch.** Dual-purpose:
  hardens parsers and exercises edge cases in the JSON-RPC reader.
  **considering**

## Distribution

- [ ] **macOS prebuilt binaries in `release.yml`.** Today release CI
  builds Linux musl only; macOS users go through Homebrew, which
  builds from source (slow on first install). Add `aarch64-apple-darwin`
  and `x86_64-apple-darwin` matrix entries. **next**
- [ ] **Windows.** A real undertaking — terminal, clipboard, file
  paths, child-process plumbing all need audit. Probably ConPTY +
  `arboard` Windows backend + `\\?\` long-path handling. Consider only
  after the editor is feature-complete enough to be worth the porting
  cost. **considering**
- [ ] **Nix flake.** `nix run github:bgunnarsson/binvim` and a flake
  output for use in a system config. **planned**
- [ ] **`cargo install binvim` from crates.io.** Currently install
  paths are Homebrew tap or `install.sh`; crates.io would catch the
  Rust-tooling crowd. Requires the licence story to permit it
  (source-available — verify). **considering**

## AI / Completion

- [ ] **Inline AI completion.** Ghost text from a model (Claude / a
  local LLM) rendered the same way LSP completion ghosts work today.
  Trigger on idle pause; accept with `<Tab>`. Pluggable backend so
  users can point at whichever endpoint. **planned**
- [ ] **`:claude` chat sidebar.** A right-pane conversation tied to
  the active buffer / selection. Out of scope until splits land.
  **considering**

## Architecture / non-goals

These are explicit decisions worth recording so they don't get
relitigated every release.

- **No plugin system.** Every language, formatter, and LSP is
  hard-wired. Adding a language is a five-file PR (see CLAUDE.md).
  This keeps the binary self-contained and the codebase greppable.
- **Source-available, not open source.** See `LICENSE`. Contributions
  welcome under the existing terms; redistribution and forks are
  governed by the licence.
- **Single binary, no runtime config beyond `~/.config/binvim/config.toml`.**
  No init script, no Lua / Vimscript layer, no `:source`-able files.
- **Catppuccin Mocha is the only built-in theme.** Colour overrides
  go through `config.toml`. No theme-pack ecosystem.
