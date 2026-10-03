---
title: How does binvim find and start a notebook's kernel when it is not Python?
kind: research
mode: afk
status: open
blocked_by: []
claimed_by:
---

## Question

Notebooks phase 3 includes kernels other than the Python one the notebook's interpreter provides. Find how the bridge can start any installed kernelspec (`jupyter_client` kernel names, `jupyter kernelspec list`), how to choose it from the notebook's `metadata.kernelspec` / `language_info`, and what else a non-Python notebook changes in binvim: highlighting, the pyright `lsp_view`, formatters, the percent-format headers, `:install`.

## Context

- `src/kernel.rs`, `src/kernel_bridge.py`, `find_interpreter`.
- `src/notebook.rs` (`project`, `lsp_view`, `formattable_edits`), `src/app/kernel_glue.rs`.
- `docs/solutions/runtime/a-notebook-buffer-is-a-projection-so-text-edits-and-file-selection-see-different-things.md`, `docs/solutions/security/a-python-probe-inherits-pythonpath-and-runs-the-projects-sitecustomize.md`.
- jupyter_client docs on the web; cite links.

## Answer

