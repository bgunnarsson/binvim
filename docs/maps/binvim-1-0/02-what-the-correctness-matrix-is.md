---
title: What is the correctness matrix, and what does green mean?
kind: grilling
mode: hitl
status: resolved
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

The correctness matrix is `docs/correctness.md`, eight hostile inputs by six areas, and a cell is green only when a named `cargo test` test (an example or a proptest property) covers that input in that area, or it is marked n/a with a reason; settled with the user 2026-10-03.

- **Rows:** combining and ZWJ clusters; wide CJK; emoji (flags, skin tones, VS16); tabs and control characters; mixed CRLF/LF; very long lines (10,000 characters and more); large files (past `Buffer::is_large`'s 5 MB / 50,000 lines); invalid UTF-8, NUL bytes and a BOM.
- **Columns:** motions; text objects; edits with undo/redo; rendering and width math, horizontal scroll included; search; save round-trip. Syntax highlighting is not a column: the per-grammar fuzz suite in `src/lang.rs` covers it.
- **Green:** each cell names its tests as `file::test_name`. A manual check never makes a cell green, and a test of a neighbouring input or area does not count. A cell that cannot apply (large files by text objects, where syntax objects are switched off: `text_object.rs` `syntax_objects_skip_a_large_buffer`) says n/a and why.
- **Mixed EOL:** saving a mixed file with the majority ending (`detect_line_ending`, `src/buffer.rs:44`) is accepted, as Vim does; the cell is green when a test pins that behaviour and `docs/correctness.md` states it. A line ending is not content, so this is not a data-loss path.
- **Performance:** the large-file and long-line rows prove correctness only (no panic, the right result). Speed belongs to the budgets of [04](04-how-to-measure-startup-and-input-latency.md).
- **Shape:** the table follows `docs/terminals.md`, and the roadmap's "Correctness on hostile input" and the 1.0 bar link to it.

What exists today (scout inventory, 2026-10-03):
- Clusters, CJK and emoji are well covered for motions (`motion.rs` `motions_land_on_cluster_boundaries`, a proptest, and example tests near `motion.rs:1355`) and for rendering (`render.rs` `cluster_widths_measure_each_cluster_whole`, `draw_walk_*`, `visible_cells_reports_a_wide_glyph_straddling_an_edge_as_partial`). Text-object proptests are ASCII only (`text_object.rs:1235`). No cluster tests cover edits, undo, search or save.
- CRLF load and save round-trip (`buffer.rs` `crlf_load_save_roundtrip`) and majority detection (`detect_mixed_picks_majority`) are tested. No test saves a mixed file.
- `is_large` thresholds are tested (`buffer.rs:829-855`). Nothing edits, searches or saves a large file, and nothing tests lines of thousands of characters or horizontal scroll over them.
- Invalid UTF-8 loads as `lossy` (`invalid_utf8_marks_the_buffer_lossy`), and saving it needs a force (`src/app/save.rs:59`). No test covers NUL bytes, a BOM or control-character width.
