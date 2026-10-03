---
title: docs/correctness.md holds the matrix, filled from the tests that exist
kind: task
mode: afk
status: open
blocked_by: [02]
claimed_by:
---

## Question

[02](02-what-the-correctness-matrix-is.md) settles the matrix: eight hostile inputs by six areas, and a cell is green only when a named `cargo test` test covers it, or it is n/a with a reason. Write `docs/correctness.md` in the shape of `docs/terminals.md`. Fill every cell the existing tests already make green, mark the n/a cells with their reasons, and leave the rest visibly open. State the mixed-EOL save behaviour. Link the page from `docs/roadmap.md`'s "Correctness on hostile input".

## Context

- Ticket 02's Answer: the rows, the columns, the green rule and today's inventory.
- `docs/terminals.md` for the shape.
- Tests in `src/motion.rs`, `src/text_object.rs`, `src/render.rs`, `src/buffer.rs`, `src/app/search.rs`, `src/app/edit.rs`, `src/undo.rs`. Name a test only after reading it: a test of a neighbouring input does not count.

## Answer
