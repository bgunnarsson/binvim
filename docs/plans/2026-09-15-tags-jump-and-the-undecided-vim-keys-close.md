---
title: Ctrl-] jumps through a tags file, and the four undecided Vim keys are decided
date: 2026-09-15
status: in-progress
---

## Context

README's *Different from Vim, not yet decided* (`README.md:227`) has held four keys since
`718b835`, the commit that closed the 0.6.0 Vim-parity push and named what that push had reached
but not resolved. `KNOWN_ISSUES.md` points at the list rather than copying it. The user asked for
the four to be decided and for tag support to be built.

1. **`Ctrl-]` is unbound.** There are no tags, so there is nothing for it to jump to. `gd` covers
   the intent when a language server is attached, which leaves every buffer without one — a plain
   C project, a shell script, a language binvim has tree-sitter for but no LSP — with no
   definition jump at all.
2. **Visual `S` wraps the selection in a pair**; Vim's built-in `S` changes the selected lines.
3. **Visual `K` is unbound**; Vim looks the selection up with `keywordprg`.
4. **Char- and line-wise Visual `I` / `A` follow Vim's documented rules**, which Vim's own
   behaviour contradicts in undocumented corners (`VjA` from column 0, `vkI` backwards).

Only the first is a feature. The other three are decisions that were deferred, and three of the
four resolve as "binvim is right, say so" — the work there is moving a README bullet, not changing
a key.

Decisions:

- **Tags read a `tags` file; binvim never generates one** (chosen by the user). `Ctrl-]`, `Ctrl-T`,
  `g]`, `:tag`, `:tags`, `:tselect`, `:pop`, and a tag stack. No `:ctags` command and no
  universal-ctags row in the install catalog — that can follow if the read half proves itself, and
  adding it later costs nothing that building it now saves.
- **The match list follows Vim exactly** (chosen by the user). `:tselect` and `g]` always show the
  list, one match or twenty; `Ctrl-]` takes the first match and says `tag 1 of 3` when there are
  more; `:tnext` / `:tprevious` / `:tfirst` / `:tlast` walk them. That means a tag stack entry
  carries its own match list and position in it, as Vim's does, rather than the list being
  thrown away once the picker closes.
- **Visual `S` stays surround** (chosen by the user), and the bullet moves to *Where binvim differs
  on purpose*. The reason on record is the asymmetry of being wrong, not habit: `S` expecting a
  linewise change leaves the parser pending on a pair character, which `Esc` cancels with nothing
  done; `S)` expecting surround would delete the selection and type `)`. Expectations are genuinely
  split — vim-surround and nvim-surround bind Visual `S` to surround, LazyVim's mini.surround does
  not — so the tie is broken by which mistake is recoverable. binvim's whole surround family
  (`ys` / `yss` / `yS` / `ds` / `cs`) is vim-surround's grammar with no plugin system to opt out
  of, and `S` is where that choice meets a built-in key.
- **Visual `K` is LSP hover at the cursor** (chosen by the user), not `keywordprg`. `K` then means
  one thing in both modes, and no new configuration surface appears — the same objection that keeps
  abbreviations in README's *Left out*. `textDocument/hover` takes a position rather than a range,
  but that costs nothing here: a server resolves a position to the symbol enclosing it, so any
  point inside a symbol answers for the whole of it. **The cursor** is the point to send — it is
  one end of the selection by definition, and it is where the user already is. The selection's
  *start* would be the wrong choice: `V` on an indented line starts at column 0, so a line-wise
  `K` would hand the server whitespace and get nothing back.
- **Visual `I` / `A` keep Vim's documented rule** (chosen by the user). Vim's undocumented corners
  stay unmatched and the bullet moves to *differs on purpose*. No measurement task, no code change.
- **The tag stack is global, not per window** (made here). Vim's is per window. binvim's jump list
  is already per *buffer* — `jumplist: Vec<(usize, usize)>` in `BufferStash` (`app/state.rs:62`)
  carries no path, so it cannot hold a cross-file position and cannot be reused. A per-buffer tag
  stack would be dropped by `:bd` precisely when `Ctrl-T` is still wanted, since the point of the
  stack is returning to a buffer you have left. One `Vec` on `App`.
- **`tags` only, never `TAGS`** (made here). Vim's default `tags` option lists `./TAGS` too, but
  that is Emacs's format — a different parser for a file binvim's users are not producing.
- **Parsed on demand, cached on the file's mtime and length** (made here). A `tags` file for a
  large tree is megabytes; re-reading it per `Ctrl-]` is the kind of cost that only shows up on
  someone else's repo.

## Relevant lore

- [open-buffer-runs-for-batch-edits-nobody-sees](../solutions/runtime/open-buffer-runs-for-batch-edits-nobody-sees.md):
  a path taken from outside the buffer list goes through `std::path::absolute` before it is
  compared with or hashed against buffer paths. A `tags` file's paths are relative to the
  directory holding the file, so every one of them is such a path. Governs tasks 2 and 4.
- [overlay-flags-stack-and-draw-order-decides-what-shows](../solutions/ui/overlay-flags-stack-and-draw-order-decides-what-shows.md):
  a new overlay page needs a variant in `top_overlay()` where `draw` paints it. `:tags` avoids this
  entirely by going through `show_listing` (`app/search.rs:192`), as `:jumps` and `:marks` do —
  no new `show_*_page` flag. Governs task 5; the check is that no flag is added.

## Acceptance criteria

- In a repo with a `tags` file, `Ctrl-]` on an identifier opens the defining file at the right line
  and `Ctrl-T` returns to the exact column it was pressed from, across files and after the origin
  buffer has been `:bd`-ed.
- A tag whose address is a search pattern (`/^fn main()$/`) lands on the line matching it, not on
  line 1. A pattern that no longer matches reports `E434` and does not move the cursor.
- A name with three matches: `Ctrl-]` takes the first and reports `tag 1 of 3`; `:tnext` moves to
  the second and reports `tag 2 of 3`; `:tprevious` goes back; `:tlast` reaches the third and
  `:tnext` there reports `E428: at last match`, with `:tfirst` / `E425` the mirror of it.
- `g]` and `:tselect` show the list whatever its length — a one-match name still gets a one-row
  picker, as Vim shows a one-row prompt. Picking a row makes it the current match, so `:tnext`
  from there moves to the row below it.
- `:tag {name}` jumps by name with no cursor word involved; `:tags` lists the stack with `>` at the
  current entry; `:pop` and `Ctrl-T` do the same thing.
- `Ctrl-]` with no `tags` file anywhere above the buffer says so and changes nothing. A tag naming
  a file that has been deleted reports it rather than opening an empty buffer.
- Visual `K` shows hover for the symbol under the cursor and leaves Visual, from a `v` selection
  made in either direction and from a `V` one on an indented line — the case that sending the
  selection's start would break. With no server attached it reports that, as Normal `K` does.
- README's *not yet decided* section is gone. Visual `S`, Visual `K` and Visual `I` / `A` are in
  *differs on purpose* with their reasons; `Ctrl-]` is in *What's supported*.
- `KNOWN_ISSUES.md`'s Vim-compatibility section no longer points at a section that does not exist.
- `cargo test -- --test-threads=1`, `cargo +1.98.0 clippy --locked --all-targets -- -D warnings`
  and `cargo fmt --check` pass.

## Tasks

- [x] **`src/tag.rs` — the format, as pure functions.** Flat module, per CLAUDE.md's
  "sub-modules only for `app/`, `lsp/`, `dap/`". `Tag { name, path: PathBuf, address, kind:
  Option<String> }` and `TagAddress::{Line(usize), Pattern(String)}`. `parse_tag_line(&str) ->
  Option<Tag>` splits on tabs, skips `!_TAG_` headers, and for an address takes a bare integer as
  `Line`, otherwise strips the `/…/` or `?…?` delimiters and the `;"` field terminator, unescaping
  `\/` and `\\`. Extra fields (`kind:f`, `line:42`, bare kind letters) parse when present and are
  absent without error — the old and extended ctags formats both appear in the wild.
  Verify: unit tests in `tag::tests` for a line-number address, a pattern address, a pattern
  containing an escaped slash, a header line, an extended-format line with `kind:` fields, an
  old-format line with a bare kind letter, and a truncated line with one tab. `cargo test tag::tests`.
- [x] **Finding and caching the file.** `find_tags_file(start: &Path) -> Option<PathBuf>` walks up
  from the buffer's directory to the filesystem root looking for `tags`, as `package::workspace_root`
  walks for manifests. `TagIndex { path, mtime, len, tags: Vec<Tag> }` on `App`, reloaded when the
  file's mtime or length has moved and reused otherwise. Every `Tag.path` is resolved against the
  tags file's own directory and run through `std::path::absolute` at parse time, per the lore, so
  nothing downstream compares a relative path against a buffer path.
  Verify: tests over a scratch tree — a `tags` file two directories up is found, none anywhere
  returns `None`, a relative `../src/main.rs` resolves against the tags file's directory and comes
  back absolute; a second lookup with the file untouched does not re-read it (assert on a parse
  counter), and touching it does. `cargo test tag::tests`.
  Deviation: `std::path::absolute` keeps `..` on Unix, so `resolve_path` also folds `..` away —
  otherwise `sub/../src/main.rs` opens a second buffer beside `src/main.rs`. "None anywhere" is
  tested from `/`, since a scratch directory can't rule out a `tags` file above it.
- [ ] **Resolving an address to a line.** `resolve_address(&TagAddress, &Buffer) -> Option<usize>`:
  `Line(n)` clamps to the buffer's length; `Pattern(p)` strips `^` / `$` anchors and finds the first
  line equal to the anchored text, or containing it when unanchored. Vim's tag patterns are literal
  apart from the anchors — treating them as regex would misfire on any tag whose line holds `.` or
  `*`, which is most of them.
  Verify: tests for an anchored pattern matching one line of several similar ones, an unanchored
  one, a pattern that matches nothing (`None`), and a line number past the end of the file.
  `cargo test tag::tests`.
- [ ] **The jump, the stack, and `Ctrl-T`.** `App.tagstack: Vec<TagStackEntry>` where an entry is
  the *origin* — `{ tag: String, path: PathBuf, line: usize, col: usize }` — captured **before**
  `open_buffer`, unlike the `PickerPayload::Location` arm in `picker_glue.rs:181`, which pushes its
  jump after the buffer has changed. `tag_jump(name)` in a new `app/tag_glue.rs`: look the name up,
  0 matches → `E426: tag not found: {name}`, otherwise take the first, push the origin, open, resolve
  the address, move the cursor. `tag_pop()` restores the top entry's file and position. Bind
  `Ctrl-]` and `Ctrl-T` in the CONTROL arm of `parse_key` (`parser.rs:1065`, both free) to
  `Action::TagJump` / `Action::TagPop`.
  Verify: tests over a scratch tree for jump-then-pop across two files, pop with an empty stack
  (message, no move), and a tag naming a deleted file (message, no move, stack unchanged). Then by
  hand in tmux against a release build, per CLAUDE.md's manual-check rules: `Ctrl-]`, `Ctrl-T`,
  and `Ctrl-]` → `:bd` the origin → `Ctrl-T`, finishing with `:q!`. Confirm the terminal actually
  delivers `Ctrl-]` as `Char(']')` with CONTROL — some send `\x1d` — and record which it was.
- [ ] **The match list, and walking it.** A tag stack entry gains `matches: Vec<Tag>` and
  `match_idx: usize`, as Vim's stack entries carry theirs — without them `:tnext` after a `Ctrl-]`
  has nothing to step through. `tag_goto_match(n)` moves within the current entry's list, reusing
  the open-and-resolve half of `tag_jump` and leaving the stack depth alone, so walking matches is
  not a second entry to pop back through. `Ctrl-]` reports `tag 1 of {n}` when `n > 1` and stays
  silent when the name is unique.
  Verify: tests for `:tnext` past the end (`E428`, no move), `:tprevious` past the start (`E425`),
  `:tfirst` / `:tlast` from the middle, and that four `:tnext`s followed by one `Ctrl-T` land back
  at the original origin rather than three matches deep. `cargo test tag`.
- [ ] **`g]`, `:tag`, `:tags`, `:tselect`, `:pop`, `:tnext` / `:tprevious` / `:tfirst` / `:tlast`.**
  `PickerKind::Tags` with `PickerPayload::Location`, which already carries `{ path, line, col }` and
  is already opened by `picker_glue.rs:181`; rows are `name`, kind and the file:line. `g]` and
  `:tselect` open it at any length, Vim showing a one-row prompt for a unique name rather than
  skipping it; accepting a row sets `match_idx` so `:tnext` continues from there. `g]` goes in the
  `g`-prefix block beside `gd` (`parser.rs:1897`). `ExCommand::{Tag(String), Tags,
  TSelect(Option<String>), Pop, TNext(usize), TPrev(usize), TFirst, TLast}` in `command.rs` with
  Vim's aliases — `tag` / `ta`, `tags`, `tselect` / `ts`, `pop` / `po`, `tnext` / `tn`,
  `tprevious` / `tp` / `tN`, `tfirst` / `tr`, `tlast` / `tl` — dispatched in `app/input.rs`.
  `:tnext` and `:tprevious` take a count, as Vim's do. `:tags` builds its rows through
  `show_listing`, as `cmd_jumps` does — no new `show_*_page` flag, per the lore.
  Verify: `command::tests` for each alias, its argument and its count; a `tag_glue` test that a
  one-match name still opens a one-row picker and that accepting row 2 of 3 makes `:tnext` go to
  row 3; by hand, `:tags` from inside `:registers` to confirm the stacked-overlay keys still act on
  the top page.
- [ ] **Visual `K` hovers at the cursor.** In the Visual arm of `parse_key`, `K` →
  `Action::LspHover`. `lsp_request_hover` (`app/lsp_glue.rs:1435`) already reads
  `self.window.cursor`, so it needs no change at all — the work is leaving Visual before the
  request so the popup isn't drawn over a live selection.
  Verify: a parser test that Visual `K` yields `LspHover`; by hand on a `.ts` buffer, `viw` over a
  typed symbol from both directions and `V` on an indented line holding one, confirming all three
  give the same popup Normal `K` gives and that none of them comes back empty.
- [ ] **README, KNOWN_ISSUES, CHANGELOG.** Delete *Different from Vim, not yet decided*. Move Visual
  `S`, Visual `K` and Visual `I` / `A` into *Where binvim differs on purpose*, each with the reason
  from Decisions above — Visual `K` hovering at the cursor, which is what makes it the same lookup
  Normal `K` does. Add `Ctrl-]` / `Ctrl-T` / `g]` and the eight `:` commands to *What's
  supported* and the Ex-commands table, saying plainly that binvim reads a `tags` file and does not
  write one. Rewrite `KNOWN_ISSUES.md`'s Vim-compatibility section, which currently points at the
  deleted section. CHANGELOG Unreleased entries for tags and for Visual `K`.
  Verify: read the sections back; `scripts/check-ai-attribution.sh`.
- [ ] **Gates.** `cargo fmt`, then `cargo test -- --test-threads=1` and
  `cargo +1.98.0 clippy --locked --all-targets -- -D warnings`. `cargo build --release` before
  handing back, since the user's `binvim` alias runs the release binary.

## Files

- `src/tag.rs` (new): `Tag`, `TagAddress`, `parse_tag_line`, `find_tags_file`, `TagIndex`,
  `resolve_address`. The upward walk mirrors `package::find_root_by_marker`.
- `src/app/tag_glue.rs` (new): `tag_jump`, `tag_pop`, `tag_goto_match`, `tag_select`, `cmd_tags`.
  Opens files through `open_buffer` and lists through `show_listing`, as `cmd_jumps` does.
- `src/app.rs`, `src/app/state.rs`: `App.tagstack`, `App.tag_index`, the pending picker matches.
- `src/parser.rs`: `Action::TagJump` / `TagPop` / `TagSelect`, `Ctrl-]` / `Ctrl-T`, `g]`,
  Visual `K`.
- `src/command.rs`, `src/app/input.rs`: the eight `:` commands and their dispatch.
- `src/picker.rs`, `src/app/picker_glue.rs`: `PickerKind::Tags`.
- `README.md`, `KNOWN_ISSUES.md`, `CHANGELOG.md`.

## Verification

- `cargo test -- --test-threads=1`, `cargo +1.98.0 clippy --locked --all-targets -- -D warnings`,
  `cargo fmt --check`, `scripts/check-ai-attribution.sh`.
- `cargo build --release`, then in tmux against `target/release/binvim`, in a scratch directory
  with two source files and a hand-written `tags` file holding a line-number tag, a pattern tag
  and a name with three matches: `Ctrl-]` and `Ctrl-T` across files, `Ctrl-]` → `:bd` → `Ctrl-T`,
  `:tnext` / `:tlast` / `:tnext` (`E428`), `g]` on a unique name, `:tags`, and `:registers` →
  `:tags` → `q`. Each run ends with `:q!`.
