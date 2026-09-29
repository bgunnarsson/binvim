---
title: A Python probe run in the project inherits PYTHONPATH, which puts the project on sys.path and runs its sitecustomize.py before the probe's own filter
date: 2026-09-29
category: security
module: src/install.rs
paths:
  - src/install.rs
  - src/app/installer.rs
  - src/dap/**
  - src/lsp/**
  - src/format.rs
tags: [python, PYTHONPATH, sitecustomize, sys.path, find_spec, debugpy, subprocess, probe, untrusted project, python_has_module, tool_installed]
symptoms:
  - ":install shows debugpy as already installed in a project that has a debugpy/ folder, and the debugger then fails with No module named debugpy.adapter"
  - "a sitecustomize.py in the project runs when :install or :update is opened"
root_cause: "the probe filtered '' out of sys.path inside its -c script, but an inherited PYTHONPATH naming the project put it back as a non-empty entry, and Python imports sitecustomize from sys.path at startup, before any -c script runs"
related:
  - docs/solutions/integration/a-tool-lookup-must-search-where-the-catalog-installs-it.md
  - docs/solutions/security/an-upward-search-trusts-what-other-users-own-or-can-write.md
---

## Problem

`python_has_module` in `src/install.rs` asks an interpreter whether it has a module
(`<python3> -c "…find_spec(sys.argv[1])…" debugpy`), and `InstallerState::new` runs it
whenever `:install` or `:update` opens. binvim's working directory is the user's project, and a
cloned project is code the user hasn't agreed to run. Review found that the probe could count
the project's own `debugpy/` folder as installed, and run the project's `sitecustomize.py`.

## What didn't work

- **Dropping `''` from `sys.path` inside the script.** This stops the `-c` working-directory
  entry, but not a `PYTHONPATH` of `.` or of the project's path, which reaches `sys.path` as a
  non-empty entry. And the script runs too late for `sitecustomize`.
- **`-I` (isolated mode).** It removes the project, but it also hides user site-packages, which
  is where `pip install --user debugpy` (what `:install` runs outside a venv) puts the module. So
  an installed debugpy would read as missing.
- **`-P` / `PYTHONSAFEPATH`.** These only drop the script-directory entry. `PYTHONPATH` still
  applies, and neither exists before Python 3.11 (macOS's `/usr/bin/python3` is 3.9).

## Root cause

Observed on Python 3.14 with a temp dir holding `sitecustomize.py` (a print) and
`debugpy/__init__.py`:

- `python3 -c 'print("main")'` printed only `main`. Plain `-c` adds the working directory
  after `site` has run, so the project's `sitecustomize.py` isn't found at startup.
- `PYTHONPATH=. python3 -c '<the probe script>'` printed `SITECUSTOMIZE RAN` and a
  `ModuleSpec` whose origin was the temp dir's `debugpy/__init__.py`, even with the `''`
  filter.

So the whole exposure comes through the inherited environment, and the in-script filter can't
reach it, because `site` runs first.

## Fix

Commit `4d38f01`: the probe's `Command` gets `.env_remove("PYTHONPATH")`. It keeps the `''`
filter and uses `find_spec` rather than `import`, so a module that is found is never executed.
Checked in tmux: with `PYTHONPATH` pointing at a stub `debugpy`, `:update` now lists debugpy as
not installed. A venv on `PATH` with a stub debugpy in its site-packages is still found, and is
upgraded without `--user`.

## Prevention

- **A Python interpreter binvim spawns on its own, with no key pressed to run it (a probe, a
  check, a status query), runs with `PYTHONPATH` removed from its environment.** A
  `Command::new` on a `PYTHON_CANDIDATES` interpreter, or on a path resolved from it, that
  starts from opening an overlay, a buffer or a render hook with no `.env_remove("PYTHONPATH")`
  is a violation.
- **Such a probe finds a module with `importlib.util.find_spec`, never by importing it**, so a
  module that is found is not executed. An `import <module>` or `-m <module>` used only to learn
  whether a module exists is a violation.
- **Do not replace this with `-I`.** It hides user site-packages, where `pip install --user`
  installs, so an installed module reads as missing. A diff that adds `-I` to an interpreter
  whose module `Installer::PythonModule` installs with `--user` is a violation. `-P` alone is
  not the fix either: it leaves `PYTHONPATH` in effect.
