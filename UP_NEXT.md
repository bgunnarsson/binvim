# UP_NEXT

Polish, follow-ups, and bigger projects parked for later. Not a roadmap
or a promise — just a place to jot down what's on the table so it
doesn't drift out of memory.

## Polish (small, contained)

- **Snippet Tab cycling.** Snippet expansion lands the cursor at `$1`
  but Tab doesn't jump to `$2`/`$3`/`$0`. Needs a position-tracking
  layer that updates on every insert/delete; the data model would be
  `SnippetSession { stops: Vec<usize>, current: usize }` and Insert-
  mode Tab routes to "advance" when a session is active.

## Bigger projects

- **Quickfix list.** Step through compile errors / grep results /
  LSP diagnostics with `:cnext`. The pickers cover discovery; a
  quickfix list covers iteration.
- **Floating terminal.** Run a build / test in-editor instead of
  alt-tabbing. `open_yazi` already shows how to hand the terminal
  over and reclaim it; the wrapper around any user-chosen shell
  command is small.
- **Git gutter + inline blame.** `+`/`~`/`-` sign-column markers for
  changed lines; `]c`/`[c` hunks navigation; `<leader>gb` toggle for
  per-line blame. Sign column already exists for diagnostics.
- **Multi-cursor across motions / operators.** Today only Insert-
  mode typing / Backspace mirror. Mirroring `dw` / `cw` / `yy` etc.
  across N cursors requires every primitive in `app/edit.rs` and
  friends to know it might apply at multiple positions. Real
  work — but a natural extension once the data model is there.

## Infra

- **`gh release` workflow signs the tarballs?** Currently we publish
  `.sha256` sidecars; signing with cosign or sigstore would be a nice
  next step for downloaders verifying authenticity.

## Tracked but not assigned a tier

- **Hover popup**: very long doc strings can dwarf the buffer. A
  max-line cap (with a `…` indicator + scroll already wired) helps,
  but very wide signatures still feel cramped — consider a
  word-wrap toggle for code blocks.
- **More tree-sitter grammars / LSPs.** `LSP_ADOPTION.md` is the
  tiered list — Python, Lua, Bash, YAML, TOML, Vue, Svelte, …
