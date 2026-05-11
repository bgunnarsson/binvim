# UP_NEXT

Polish, follow-ups, and bigger projects parked for later. Not a roadmap
or a promise — just a place to jot down what's on the table so it
doesn't drift out of memory.

## Bigger projects

- **Floating terminal.** Run a build / test in-editor instead of
  alt-tabbing. `open_yazi` already shows how to hand the terminal
  over and reclaim it; the wrapper around any user-chosen shell
  command is small.
- **Git gutter + inline blame.** `+`/`~`/`-` sign-column markers for
  changed lines; `]c`/`[c` hunks navigation; `<leader>gb` toggle for
  per-line blame. Sign column already exists for diagnostics.

## Tracked but not assigned a tier

- **Multi-cursor on vertical motions** (`dj` / `yk` / etc.). Today's
  multi-cursor operator fan-out covers word motions, text objects,
  `dd`/`yy`/`cc`, and `x`. Vertical motions are excluded because
  multiple overlapping line spans get coalesced into one and the
  result feels surprising; if a real use case shows up, revisit.
- **More tree-sitter grammars / LSPs.** `LSP_ADOPTION.md` is the
  tiered list — Python, Lua, Bash, YAML, TOML, Vue, Svelte, …
