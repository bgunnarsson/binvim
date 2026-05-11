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
- **Multi-cursor: Enter / newline mirroring.** Char insert and
  Backspace mirror across all cursors; Enter currently only acts at
  the primary. Smart-indent across N positions is non-trivial — for
  v1 a literal `\n` insert mirror is probably enough.
- **Inlay-hint kind styling.** `InlayHint.kind` (1=Type, 2=Parameter)
  is captured but ignored; both render in the same dim italic. Could
  differentiate parameter hints with a slightly different colour or a
  trailing `:`.
- **Picker match-character highlighting.** The fuzzy matcher already
  returns matched positions in `fuzzy_match`; the renderer doesn't
  use them. Painting the matched chars in Sky/Yellow inside each row
  would visually explain *why* a row ranked.
- **`:S` regex support.** Project-wide substitute currently uses
  ripgrep `--fixed-strings`. Drop that flag (or add a `/r` toggle)
  for regex search. Replacement side is harder — `String::replace`
  doesn't know about capture groups; would need `regex::Regex`.
- **Clickable overflow chevrons.** Tab-bar `‹` / `›` are pure
  indicators today. Click → scroll the visible slice by one tab.
  Easy now that `tab_layout` is the shared source of truth.
- **Middle-click closes a tab.** Faster than reaching for the `×`,
  matches every other tabbed editor's convention. Currently `:bd`
  refuses dirty — middle-click could honour the same guard.
- **`gt` / `gT` keybindings.** Vim convention for next / previous
  tab. `H` / `L` already cover this; `gt`/`gT` would be aliases.
- **Persistent jumplist.** Serialise `jumplist` + `jump_idx` per
  workspace alongside the session JSON; restore on launch. The
  user asked about this — explained but not implemented.

## Bigger projects

- **Split windows / panes.** `:vsp` / `:sp`. The biggest functional
  gap. App's single-buffer rendering assumes one viewport; adding
  splits means a `Window` tree, per-window cursor/viewport, and the
  layout math in `render.rs` becomes substantial. High ROI but real
  work.
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
- **DAP (debugger).** Biggest lift on the list, biggest payoff if
  Rust / Go is the day job. Would touch the LSP module's
  send-request infrastructure to bolt on a parallel DAP client.
- **Multi-cursor across motions / operators.** Today only Insert-
  mode typing / Backspace mirror. Mirroring `dw` / `cw` / `yy` etc.
  across N cursors requires every primitive in `app/edit.rs` and
  friends to know it might apply at multiple positions. Real
  work — but a natural extension once the data model is there.

## Infra

- **CI Node 20 → 24.** GitHub Actions warn that `actions/checkout@v4`
  and `actions/upload-artifact@v4` are on Node 20 — deprecated
  June 2026. Bump the workflow before then.
- **`gh release` workflow signs the tarballs?** Currently we publish
  `.sha256` sidecars; signing with cosign or sigstore would be a nice
  next step for downloaders verifying authenticity.

## Tracked but not assigned a tier

- **JSX overlay**: currently distinguishes lowercase tags from
  PascalCase components, but `<></>` fragments and `{children}`
  expressions could pick up more specific captures.
- **Hover popup**: very long doc strings can dwarf the buffer. A
  max-line cap (with a `…` indicator + scroll already wired) helps,
  but very wide signatures still feel cramped — consider a
  word-wrap toggle for code blocks.
- **Visual-block + surround.** `visual_range_chars(Block)` returns a
  coarse anchor-to-cursor span as a fallback; a proper "wrap each
  row in pair" implementation would be more useful.
- **Picker file-type icons.** We removed them from tabs (Pink-tag /
  Yellow-component carries the info already), but picker rows are
  another place icons could land usefully.
- **More tree-sitter grammars / LSPs.** `LSP_ADOPTION.md` is the
  tiered list — Python, Lua, Bash, YAML, TOML, Vue, Svelte, …
