---
title: A notebook buffer's text is a projection of its file, so a text edit can break the structure lines and a file search over disk misses what the edit would match
date: 2026-10-03
category: runtime
module: src/app/input.rs
paths:
  - src/app/input.rs
  - src/app/notebook_glue.rs
  - src/notebook.rs
  - src/buffer.rs
tags: [notebook, ipynb, projection, substitute, ripgrep, cell-header]
symptoms:
  - ":S/\\<id\\>/uid/g turns `# %% id=abc` into `# %% uid=abc`, and the save writes the whole notebook as one code cell with every output gone"
  - "markdown cells come back as code after a project-wide substitution"
  - ":S/^import pandas/import polars/ reports N substitutions across M files but leaves a notebook whose cells start with that line unchanged"
  - "a pattern containing a quote, backslash or tab never substitutes inside a .ipynb"
root_cause: "a .ipynb buffer holds percent-format text (`# %% id=…` header lines between cell sources), not the file's JSON — text operations treated the headers as ordinary lines, and :S chose its files by grepping the JSON while the substitution ran on the projection"
related:
  - docs/solutions/runtime/open-buffer-runs-for-batch-edits-nobody-sees.md
  - docs/solutions/runtime/ropey-breaks-lines-at-more-than-lf-so-a-split-on-newline-disagrees-with-the-buffer.md
---

## Problem

Notebooks open as editable text: each cell is a `# %% id=…` header (`[markdown]` / `[raw]` for
other kinds) followed by its source, and the save maps cells back to the JSON by id so outputs
and metadata survive. Review found that `:s` and `:S`, written before notebooks existed, broke
that mapping in both directions.

## What didn't work

- **Assuming the save would refuse a damaged header.** It doesn't, by design: a line that no
  longer parses as a header is ordinary source, so a cell's text can contain anything. A rewritten
  header silently joins its cell to the one above; with every header gone, the notebook saves as
  one new code cell with no outputs. `:S` saves without showing the buffer, so nobody sees it
  happen.
- **Letting ripgrep pick the candidate files, as for every other file type.** ripgrep reads the
  JSON, where a cell line sits inside `"…\n",` with leading spaces and escapes: `^` never matches a
  cell line's start, and `"`, `\` and tab are written `\"`, `\\` and `\t`. Notebooks the
  substitution would have changed were never opened, and the summary counted only the files it did
  open.

## Root cause

Verified by tests and a tmux run against the release build. The buffer and the file are two
different texts for a notebook. Code that matches against one and edits the other — or edits text
the save reads as structure — is wrong for notebooks even when it is right for every other file.

## Fix

`b9142bd`:

- `App::is_cell_header` (`src/app/input.rs`) is true for a line `notebook::parse_header` accepts
  in a notebook buffer. `replace_matches` skips those lines and `sub_confirm_seek` steps over them,
  so `:s`, `:%s`, `:s///c` and `:S` never rewrite a header.
- `project_substitute` adds every `*.ipynb` from `rg --files --iglob` to the candidates, so each
  notebook is opened and judged on its projected text; the early "pattern not found" bail when rg
  matched nothing was removed for the same reason.

## Prevention

- **Any text operation that runs over a whole buffer or a line range** (substitute, a global
  command, a filter, a formatter, a bulk LSP edit) must skip or refuse lines where
  `is_cell_header` / `notebook::parse_header` is true when the buffer is a notebook. A new
  line-rewriting loop in `src/app/` with no such check is a violation.
- **A search that picks files by grepping disk content, whose results are then edited in the
  buffer**, must not use the disk match for `.ipynb`: add notebooks unconditionally and let the
  projected text decide. A `rg --files-with-matches` (or `-l`) whose list feeds `open_buffer` and
  has no notebook branch is the giveaway.
