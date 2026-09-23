---
title: Changing the cursor's unit to grapheme clusters leaves every col ± 1 outside the plan's list stepping one codepoint
date: 2026-09-23
category: runtime
module: src/app, src/motion.rs
paths:
  - src/app/**
  - src/motion.rs
  - src/text_object.rs
  - src/buffer.rs
tags: [grapheme, unicode, cursor, col, emoji, combining, motion, visual, insert, put, clamp_cursor_normal]
symptoms:
  - "after `i`, 👍🏽, Esc, `x` leaves a bare 👍"
  - "`p` on 👨‍👩‍👧 gives `a👨b‍👩‍👧b`"
  - "`v` then `A` on an emoji starts Insert between 👨 and a ZWJ"
  - "`vrZ` on an emoji writes five Z's"
  - "`<C-v>d` on an emoji leaves a dangling ZWJ (U+200D)"
  - "the cursor rests on a skin-tone modifier, VS16 or ZWJ"
root_cause: "the grapheme plan made the cursor rest only on a cluster's first char but converted the sites its task list named; code elsewhere meaning the next or previous character as `cursor.col + 1`, `col - 1` or `c2 + 1` kept stepping one codepoint, and the central clamp that snaps a stray col never ran on those paths"
related:
  - docs/solutions/runtime/a-buffers-disk-fields-are-set-in-three-places-and-the-reload-is-outside-buffer-rs.md
  - docs/solutions/runtime/config-reload-misses-state-derived-from-app-config.md
---

## Problem

The plan `docs/plans/2026-09-23-the-cursor-moves-edits-and-measures-by-grapheme-cluster.md` made
`h` / `l`, the word motions, `x`, `r`, `~`, `a`, charwise ranges and Insert Backspace move by
grapheme cluster. `Cursor.col` stayed a char index, with the rule that it only rests on a
cluster's first char. Its tests and the tmux check passed. Review then found five more places,
none of them on the plan's list, that still split a cluster:

- Esc, Ctrl-C and Ctrl-O leaving Insert (`src/app/input.rs`) stepped back with `col -= 1`.
- Charwise `p` (`src/app/edit.rs`, `put`) inserted at `cursor.col + 1`.
- Visual `A` (`src/app/visual.rs`, `visual_insert`) entered Insert at `cursor.col + 1`.
- Visual `r` wrote one replacement char per codepoint.
- Every Visual-block path ended a row at `c2 + 1` chars.

## What didn't work

- **Listing the sites by file and line in the plan.** The list came from reading `motion.rs`,
  `edit.rs` and the render walks. It was complete for motions and missed the step-backs and
  put-afters that sit in Insert-exit handlers, `put` and Visual helpers. The acceptance tests
  only ran commands the list covered, so they couldn't find the rest.
- **Relying on the central clamp.** The plan made `clamp_cursor_normal` snap any col to its
  cluster's start, to catch a col set from outside (a search match, an LSP jump). The Esc arms
  never call it, and `p`, Visual `A` and `r` change the text before any clamp runs, so the clamp
  repairs the cursor after the damage, or not at all.

## Root cause

Observed in review, each reproduced by a verifier's throwaway test: a site whose `± 1` means "the
neighbouring character in text already in the buffer" is wrong as soon as a character can span
several codepoints. The unit change was made where the plan looked, and `+ 1` is too common to
find by reading around.

Not every `± 1` is wrong. Moving past a char this code has just inserted or deleted, past a
known one-codepoint char (an ASCII closer, `(`), or a 1-based display column is still right.
That is why a blanket search-and-replace would have been wrong too.

Not verified: the search-offset stepping in `src/app/search.rs` (`col + 1 < line_len`) and the
`iw` class walk in `src/text_object.rs` still step one char. Review didn't flag them, and
nobody has checked whether they split a cluster.

## Fix

Commit `0f79317`. The step-backs use `Buffer::prev_grapheme_col`, and put-after and Visual `A`
use `next_grapheme_col`. Visual `r` maps graphemes, applying a block's rows bottom-up because a
row can now get shorter. Every block path goes through the new `Buffer::block_row_cols`.
Ctrl-O's check for "the cursor is on the last char" uses `next_grapheme_col(...) == line_len`
rather than `col + 1 == line_len`.

## Prevention

- **A `± 1` on a cursor col or range end that means the neighbouring character in existing text
  goes through the grapheme helpers:** `next_grapheme_col` / `prev_grapheme_col` /
  `grapheme_start_col` / `inclusive_end_idx` / `block_row_cols`. In a diff to `src/app/**`,
  `src/motion.rs` or `src/text_object.rs`, a new `cursor.col + 1`, `col - 1`, `c2 + 1` or
  `col += 1` over text the code didn't just write is a violation. A `± 1` right after
  `insert_char`, or past a char the code has just matched as a single ASCII char, or in a 1-based
  display string, is not.
- **A path that sets `cursor.col` and returns to Normal without calling `clamp_cursor_normal`
  computes a cluster start itself.** A mode-exit or insert-point arm that assigns a col from
  arithmetic, with neither the clamp nor a grapheme helper, is a violation.
- **A plan that changes the unit a position is measured in lists its sites from a tree-wide grep
  for the old unit's arithmetic, not from reading the obvious modules.** For the cursor that is
  `grep -nE 'col \+ 1|col - 1|col \+= 1|col -= 1|c2 \+ 1' src/app/*.rs src/motion.rs src/text_object.rs`,
  with each hit named in the plan as converted or as left in the old unit on purpose, and why. A
  plan whose site list has no such grep behind it is a violation.
