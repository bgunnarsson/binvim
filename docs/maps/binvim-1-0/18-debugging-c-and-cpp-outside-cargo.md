---
title: How does binvim find the program to debug in a C or C++ project that is not a Cargo project?
kind: research
mode: afk
status: open
blocked_by: [01]
claimed_by:
---

## Question

[01](01-which-stacks-are-supported-for-zero-config.md) makes C/C++ a promised stack with a debugger, and lldb-dap starts only under `Cargo.toml`. Find which project kinds to cover (CMake, Make, Meson, a lone `.c` file), how each names its build command and its executables (CMake's file API, `compile_commands.json`), and whether the result fits the existing `prelaunch` + target-picker shape or needs more.

## Context

- `src/dap/specs.rs:505-560` the lldb-dap spec, `rust_prelaunch`, `rust_launch_args`.
- `src/app/dap_glue.rs` `dap_resolve_rust` and `dap_start_session`'s dispatch.
- `src/lsp/specs.rs` clangd's root discovery.

## Answer
