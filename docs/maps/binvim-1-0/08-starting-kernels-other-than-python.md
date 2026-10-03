---
title: How does binvim find and start a notebook's kernel when it is not Python?
kind: research
mode: afk
status: resolved
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

The bridge stays Python and keeps driving `jupyter_client`. When the notebook names another kernel, it starts that kernel by its `metadata.kernelspec.name` through the stock `KernelSpecManager`, which runs the spec's own `argv`. `metadata.kernelspec.language`, then `language_info.name`, picks a stand-in when the name isn't installed. A non-Python notebook therefore still needs a Python with `jupyter_client` to host the bridge.

- **Today Python is forced.** The bridge's own `Spec` subclass returns `sys.executable -m ipykernel_launcher` for any name (`src/kernel_bridge.py:83-86`), and `KernelManager(kernel_name="python3", …)` (line 107). `find_interpreter` searches the project env, then `python3` / `python`, then the `:install` venv (`src/kernel.rs:264-283`). The Python path should stay as it is: it is what makes a project `.venv` the kernel without a registered spec.
- **The stock manager.**
  - `find_kernel_specs()` / `get_kernel_spec(name)` search `$JUPYTER_PATH`, the user data dir (`~/Library/Jupyter/kernels` on macOS, `~/.local/share/jupyter/kernels` on Linux), then `{sys.prefix}/share/jupyter/kernels`, then the system dirs.
  - A missing name raises `NoSuchKernel`, a subclass of `KeyError`.
  - `jupyter kernelspec list --json` shows the same set.
  - A kernel registered only inside some other env is not found unless that env's prefix is on `JUPYTER_PATH`.
- **What else assumes Python:**
  - `.ipynb` is `Lang::Python` (`src/lang.rs:119`) and always gets pyright (`src/lsp/specs.rs:285`).
  - `lsp_view` and `formattable_edits` strip IPython `%`/`!` lines (`src/notebook.rs:769-860`).
  - `project` never reads `kernelspec` or `language_info` (`src/notebook.rs:193`).
  - Jupytext's percent format puts the cell marker in the language's own comment: `# %%` for Python, R and Julia; `// %%` for Rust, Go, C#, TypeScript and C++; `-- %%` for Lua and Haskell. binvim's headers are `# %%` regardless.
  - `:install` has ipykernel only, in the Python bundle (`src/install.rs:455-459`).
- **Non-Python kernels differ.**
  - Some omit `execution_count` or lack `interrupt_mode: message`.
  - `kernel_info_reply.language_info` is the authoritative language.
  - Outputs can arrive as `text/html` or vendor MIME types (.NET Interactive), so `text/plain` remains the fallback.
  - The common kernels are IRkernel `ir`, IJulia `julia-1.x`, evcxr `rust`, Deno `deno`, xeus-cling `xcpp17`, GoNB `gonb` and .NET `.net-csharp`. These names are from the researcher's memory: check them against `jupyter kernelspec list --json`.

Unverified (researcher's memory): the kernel name table, how JupyterLab and VS Code resolve an unknown kernelspec, and the jupytext SQL token. Sources: [jupyter_client kernels](https://jupyter-client.readthedocs.io/en/stable/kernels.html), [kernelspec.py](https://github.com/jupyter/jupyter_client/blob/main/jupyter_client/kernelspec.py), [nbformat format](https://nbformat.readthedocs.io/en/latest/format_description.html), [jupytext languages.py](https://github.com/mwouts/jupytext/blob/main/src/jupytext/languages.py).
