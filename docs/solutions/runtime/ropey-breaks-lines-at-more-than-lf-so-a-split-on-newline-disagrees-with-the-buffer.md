---
title: Ropey breaks lines at CR, VT, FF, NEL and U+2028/9 as well as LF, so a check done with split('\n') sees different lines than the buffer
date: 2026-10-03
category: runtime
module: src/notebook.rs
paths:
  - src/notebook.rs
  - src/buffer.rs
  - src/app/notebook_glue.rs
tags: [ropey, line-break, notebook, cell-header, unicode]
symptoms:
  - "a code cell whose source holds `x = 1\\r# %% [markdown] id=zz` is drawn as a separate, concealed markdown cell, while Jupyter runs the text as code"
  - "project() accepts a cell source with a header-like line, yet the rope shows a cell header there"
  - "line numbers from str::lines() or split('\\n') are off from the buffer's after a lone \\r or \\u{2028}"
root_cause: "ropey's default line breaks are every Unicode one (CR alone, VT, FF, NEL, U+2028, U+2029, CRLF, LF), so text checked line by line with split('\\n') can hide a line the rope counts"
related:
  - docs/solutions/runtime/changing-the-cursors-unit-leaves-col-plus-one-steps-in-the-old-unit.md
  - docs/solutions/runtime/a-notebook-buffer-is-a-projection-so-text-edits-and-file-selection-see-different-things.md
---

## Problem

`notebook::project` refuses a notebook whose cell source contains a line that parses as a cell
header, because that line would split the cell when the buffer is read back. The check split each
source on `\n`. Security review showed a source with a lone `\r` before a fake header passed it.

## What didn't work

- **Checking each `\n`-split line for a header.** The fake header follows a `\r`, not a `\n`, so
  it is the middle of one `\n`-line and never starts one. Ropey, which the cell bars, conceal and
  `]c` read through, starts a new line after the `\r`, and there it is a header.

## Root cause

Verified with a test (`a_line_break_other_than_lf_refuses_projection`). Ropey is built with its
default `unicode_lines` behaviour: `Rope::lines()`, `line_to_char` and `len_lines` break at CR
(alone), VT, FF, NEL, U+2028 and U+2029 as well as LF and CRLF. `str::lines()` breaks only at LF
(dropping a CR before it). Any line-wise check on a `String` disagrees with the rope once one of
those characters is in the text. The save path (`cells_from_text`) splits on `\n`, so the display
and the file disagreed about where cells start.

## Fix

`4b55c25`: `has_other_line_break` (`src/notebook.rs`) makes `project` refuse a source holding any
line break ropey counts and `\n` splitting doesn't (CRLF inside a source is still allowed); the
notebook opens as raw JSON with "cell N contains a line break other than \n". `cell_spans` over a
rope was folded into `cell_spans_text`, so there is one scanner.

## Prevention

- **A check that decides what the rope will show line by line** — a header, a fold, a gutter mark —
  must split the same way ropey does, or refuse text holding CR-without-LF, `\u{0b}`, `\u{0c}`,
  `\u{85}`, `\u{2028}` or `\u{2029}`. `str::lines()` / `split('\n')` over text that then goes into
  a `Rope` and is read with `line(n)` is the pattern to question.
- **Two scanners for the same structure, one over a `Rope` and one over a `&str`,** is a
  violation: keep one, over one representation, so they can't disagree on line breaks.
