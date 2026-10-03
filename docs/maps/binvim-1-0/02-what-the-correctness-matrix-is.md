---
title: What is the correctness matrix, and what does green mean?
kind: grilling
mode: hitl
status: open
blocked_by: []
claimed_by:
---

## Question

The 1.0 bar requires the correctness matrix to be green, but no matrix is written down. Decide its rows (graphemes, wide chars, emoji, mixed EOL, very long lines, huge files, ...) and columns (motions, text objects, rendering/width math, edits, save round-trip, ...), where it lives, and what evidence makes a cell green (a named test, a fuzz property, a manual check).

## Context

- `docs/roadmap.md` Horizon 2, "Correctness on hostile input".
- Existing coverage: `src/render.rs` tests (cluster widths, wide-glyph edges), `src/buffer.rs` grapheme tests incl. CRLF, proptests in `src/motion.rs`, `src/text_object.rs`, `src/notebook.rs`, `src/lsp/`, per-grammar fuzz in `src/lang.rs`. Charted gaps: no mixed CRLF/LF file tests, no very-long-line tests.
- `docs/terminals.md` as the shape the user already accepted for a matrix.

## Answer

