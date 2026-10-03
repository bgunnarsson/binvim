---
title: For a notebook whose kernel isn't Python, what does binvim support beyond running its cells?
kind: grilling
mode: hitl
status: open
blocked_by: [08]
claimed_by:
---

## Question

[08](08-starting-kernels-other-than-python.md) settles how the kernel starts: by `metadata.kernelspec.name` through the stock `KernelSpecManager`. Everything else in binvim assumes Python:
- `Lang::Python` highlighting
- pyright
- `# %%` headers
- the IPython-line stripping

Which of these follows the notebook's language for 1.0? The headers could take the language's comment (`// %%`), highlighting and the LSP could follow `language_info.name`, and formatters could follow too. Or does a non-Python notebook only run, read as Python-shaped text? And which kernels must be tried before phase 3 counts as shipped?

## Context

- [08](08-starting-kernels-other-than-python.md)'s answer.
- `src/notebook.rs` `project` / `lsp_view` / `formattable_edits`, `src/lang.rs:119`, `src/lsp/specs.rs:285`.
- `docs/solutions/runtime/a-notebook-buffer-is-a-projection-so-text-edits-and-file-selection-see-different-things.md`.

## Answer

