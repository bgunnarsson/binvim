---
title: Which stacks does 1.0 call supported, and what must each one have to count?
kind: grilling
mode: hitl
status: open
blocked_by: []
claimed_by:
---

## Question

The 1.0 bar says first run works with zero manual config for the supported stacks, and the roadmap says depth is LSP + debugger + formatter + tree-sitter for every advertised language. Today only Rust, Go, Python, C/C++ and C# have all four. Decide which stacks 1.0 promises, and for each whether a debugger and formatter are required or the promise is narrower (for example LSP + formatter + highlighting, with a debugger only where advertised).

## Context

- `docs/roadmap.md` (strategic through-line, 1.0 quality bar).
- `docs/external-tools.md`, `src/install.rs` `BUNDLES`, `src/dap/specs.rs` `BUILTIN_ADAPTERS`.
- Gaps as charted: no debugger outside the five runtimes above; no formatter for Razor, TOML, Docker.

## Answer

