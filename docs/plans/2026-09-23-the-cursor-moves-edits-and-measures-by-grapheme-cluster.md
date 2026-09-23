---
title: The cursor moves, edits and measures text by grapheme cluster, not by codepoint
date: 2026-09-23
status: in-progress
---

## Context

0.7's correctness bar names "grapheme clusters / wide chars / emoji" (`ROADMAP.md`, Horizon 2).
The user chose grapheme-aware motion and edits for 0.7 over a tests-only pass.

Today `Cursor.col` (`src/cursor.rs`) is a char index into the line, and motions step it one
codepoint at a time. `left` / `right` (`src/motion.rs:20-45`), `advance_one` / `retreat_one`
(`motion.rs:971-992`, under every word motion) and `clamp_cursor_normal`
(`src/app/view.rs:190`) all do `col ± 1`. Width is summed per codepoint through
`render::char_width` (`src/render.rs:26`). The effect was measured with the locked crates
(`unicode-width` 0.2.2, `unicode-segmentation` 1.13.2):

| text | clusters | width as one cluster (what terminals draw) | per-codepoint sum (what binvim lays out) |
| --- | --- | --- | --- |
| 👨‍👩‍👧 (ZWJ family) | 1 | 2 | 6 |
| 👍🏽 (skin tone) | 1 | 2 | 4 |
| ❤️ (VS16) | 1 | 2 | 1 |
| e + U+0301 | 1 | 1 | 1 |
| 🇮🇸, 한 | 1 | 2 | 2 |

So `l` takes five presses to cross the family emoji, `x` deletes one codepoint of it, and every
column after it is drawn four cells off. Combining marks already have width 0, which is why they
render correctly today (`cursor_visual_col_walk_zero_width_chars_advance_nothing`,
`render.rs:5145`).

Decisions:

- **`Cursor.col` stays a char index.** Changing its unit would touch every caller: LSP positions,
  marks, search, multi-cursor, undo. Instead, every motion and edit lands on and spans whole
  clusters, and `clamp_cursor_normal` snaps a col set mid-cluster from outside (an LSP jump, a
  search match, a mark) back to the cluster's start.
- **Boundaries come from `unicode_segmentation::GraphemeCursor` walked over rope chunks,** as in
  ropey's `examples/graphemes_step.rs` (ropey 1.6.1). Each lookup stays local to the cursor, so a
  multi-megabyte line doesn't cost a full scan per keypress. Extended clusters
  (`is_extended = true`).
- **A cluster's width is `UnicodeWidthStr::width(cluster)`.** A single-char cluster keeps
  `char_width`'s current answer, including `\t` → tab width and a control char → 1, so nothing
  that renders correctly today moves.
- **In scope:** `h` / `l`, word motions, `$`, `|`, `gm`, `g0`, `g$`, vertical motion's `want_col`,
  `x` / `X` / `r` / `~` / `s`, `a` / `A`, Insert-mode Backspace / Delete, charwise Visual and
  operator ranges, mouse click, and the render walks.
- **Out of scope:** `f` / `t` search one char as typed, and a `/` match may start mid-cluster
  until the clamp snaps it. The embedded terminal's grid (`src/terminal.rs:479-490`) stores one
  codepoint per cell; that's a separate subsystem.
- **A terminal that draws a cluster at a different width than `unicode-width` still misaligns.**
  Older ones split ZWJ sequences, for example. The terminal-matrix plan
  (`2026-09-23-a-terminal-compatibility-matrix-is-tested-and-published.md`) records which do.

## Relevant lore

None applies. No doc covers text width or cursor motion. The lore on buffers
(`a-buffers-disk-fields-are-set-in-three-places…`) is about disk-derived fields, which this plan
doesn't add.

## Acceptance criteria

- On the line `a👨‍👩‍👧b`, `l` from `a` lands on the emoji, and `l` again lands on `b`. `x` on the
  emoji leaves `ab`. `r` on `e` + U+0301 replaces both codepoints.
- The cursor's screen column and every glyph after `👨‍👩‍👧`, `👍🏽` or `❤️` match the table's cluster
  width, checked through `cursor_visual_col_walk` and the draw walk.
- `v` on the emoji then `y` yanks all five of its codepoints. `dl` / `cl` do the same.
- A proptest over lines drawn from ASCII, CJK, combining marks and emoji sequences holds that
  every motion target is a grapheme boundary.
- Insert-mode Backspace after the emoji removes the whole cluster, and a mouse click on its second
  cell puts the cursor on its start.
- Every existing `motion::tests`, `text_object::tests` and `render::tests` case passes unchanged.

## Tasks

- [x] Add `Buffer::next_grapheme_col`, `prev_grapheme_col` and `grapheme_start_col` (by line and
  char col) over `GraphemeCursor` and rope chunks, in `src/buffer.rs`. Verify with `buffer::tests`
  on the table's strings, on line start and end, on an empty line, and on a line longer than one
  rope chunk.
- [x] Add `render::cluster_width` and a line-cluster iterator yielding `(char_col, char_len,
  width)`, then convert the render walks at `render.rs:936`, `:6138` and `:8271`
  (`cursor_visual_col_walk`). Verify with new `render::tests` for the table's widths through
  `cursor_visual_col_walk`, plus the existing `cursor_visual_col_*` tests unchanged.
  Deviation: instead of a cluster iterator, `render::cluster_widths` gives each char of a line
  `Some(cluster width)` at a cluster's start and `None` on its continuation chars, so the draw
  walk keeps painting char by char (selection, syntax colour and clipping are all per char). A
  cluster whose first char is clipped to a `<` / `>` edge marker has its other chars skipped, or
  the terminal would draw the glyph anyway. `paint_code_line` walks clusters directly.
- [ ] Convert the remaining width walks:
  - `markdown_render.rs:1189` and `:1230`
  - `app/state.rs:1311`
  - the mouse-click column at `app/input.rs:125`
  - `motion::col_at_visual` (`motion.rs:410`)
  - the two `UnicodeWidthChar` loops at `render.rs:6485-6536`, if they measure buffer text

  Verify with a click-on-second-cell test and a markdown `visual_col_for_buffer_col` emoji case.
- [ ] Make `left`, `right`, `line_end`, `advance_one`, `retreat_one` and the `want_col` snap step
  by cluster. Verify with `motion::tests` on the acceptance lines, plus a proptest (next to the
  existing motion proptests) asserting every motion target is a boundary.
- [ ] Make the edits span clusters. That means `delete_char_forward` (`app/edit.rs:1219`) and `X`,
  `replace_char` (`:646`), `toggle_case` (`:1179`), `a` / `A`, and Insert Backspace / Delete
  (`app/input.rs` around 1931 and 2150). The `CharInclusive` range end in `app/dispatch.rs:1084`
  and `app/multi_cursor.rs:290` extends to the cluster's end. `clamp_cursor_normal`
  (`app/view.rs:190`) snaps to the cluster start. Verify with `app::input` / `app::edit` tests for
  each edit on the emoji and on `e` + U+0301, and with the existing tests for `x`, `r`, `~` and
  Visual unchanged.
- [ ] Add a `CHANGELOG.md` Unreleased `### Changed` entry. If the terminal matrix finds a
  terminal that draws clusters differently, add a `KNOWN_ISSUES.md` entry for it. Verify by
  reading the diff.

## Files

- `src/buffer.rs` (new helpers, tests), `src/render.rs` (`char_width` 26, walks 936 / 6138 / 8271
  / 6485-6536, tests), `src/markdown_render.rs` (1189, 1230), `src/motion.rs` (20-45, 88, 410,
  971-992), `src/app/view.rs` (190), `src/app/edit.rs` (646, 1179, 1219), `src/app/input.rs` (125,
  the Insert Backspace / Delete arms), `src/app/dispatch.rs` (1084), `src/app/multi_cursor.rs`
  (290), `src/app/state.rs` (1311).
- The pattern comes from ropey 1.6.1's `examples/graphemes_step.rs`. The `unicode-segmentation`
  dependency is already declared (`Cargo.toml:34`) and unused.

## Verification

- `cargo test -- --test-threads=1` and pinned clippy green locally and in CI.
- A tmux check on a fresh `cargo build --release`, with a file of the table's strings, each
  followed by `|x|`:
  1. `l` across each line.
  2. `x`, `r`, `~` on each cluster.
  3. `v` + `y` + `p`.
  4. Insert Backspace.

  The `|` after each cluster lines up with the line above in `capture-pane`. tmux applies its own
  width table, so confirm by eye in Ghostty too.
- Open a 50 000-line file whose lines are 10 000 chars of emoji. `l`, `$` and `x` respond without
  a visible stall.
