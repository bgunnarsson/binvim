---
title: The cluster, wide-char and emoji rows of the correctness matrix are green
kind: task
mode: afk
status: open
blocked_by: [21]
claimed_by:
---

## Question

Write the tests that turn the open cells in the cluster, CJK and emoji rows of `docs/correctness.md` green, and fill those cells. From 02's inventory, the expected gaps are text objects (the proptests are ASCII only), edits with undo/redo, search and save round-trip. Where `motion.rs`'s cluster strategy (`arb_cluster_buf_and_cursor`) fits, reuse it rather than writing a second one. A test that fails because binvim is wrong is a defect. Fix it here when the fix is small. Otherwise ticket it, and leave the cell open.

## Context

- Ticket 02's Answer and the matrix from 21.
- `src/motion.rs` `arb_cluster_buf_and_cursor`, `src/text_object.rs` `arb_text`.
- `CLAUDE.md` on `Cursor.col` and the grapheme helpers in `src/buffer.rs`.

## Answer
