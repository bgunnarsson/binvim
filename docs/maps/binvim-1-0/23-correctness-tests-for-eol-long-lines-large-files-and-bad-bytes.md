---
title: The tab, EOL, long-line, large-file and bad-byte rows of the correctness matrix are green
kind: task
mode: afk
status: open
blocked_by: [21]
claimed_by:
---

## Question

Write the tests that turn the open cells green in these rows of `docs/correctness.md`, and fill those cells:
- tabs and control characters
- mixed CRLF/LF, including a test pinning that a mixed file saves with the majority ending
- very long lines (10,000 characters or more, horizontal scroll included)
- large files past `Buffer::is_large`
- invalid UTF-8, NUL bytes and a BOM

These tests prove correctness only, not speed ([02](02-what-the-correctness-matrix-is.md)). A test that fails because binvim is wrong is a defect. Fix it here when the fix is small. Otherwise ticket it, and leave the cell open.

## Context

- Ticket 02's Answer and the matrix from 21.
- `src/buffer.rs` `detect_line_ending`, `from_path` (`lossy`), `is_large`, `LARGE_FILE_BYTES` / `LARGE_FILE_LINES`; `src/app/save.rs:59` (lossy save needs a force).
- `src/render.rs` horizontal scroll and `cluster_widths`.
- Tests read no host state: scratch files go in a temp dir.

## Answer
