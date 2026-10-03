---
title: Which debug adapter debugs TypeScript and JavaScript, and how does binvim install and launch it?
kind: research
mode: afk
status: open
blocked_by: [01]
claimed_by:
---

## Question

[01](01-which-stacks-are-supported-for-zero-config.md) promises a debugger for TS/JS, and none exists. Find the adapter to use (vscode-js-debug's DAP server, or another), how it is distributed (npm, GitHub release tarball), how an `Installer` could fetch it on macOS, Linux and Windows with no manual step, and what its `launch` request needs for a Node script, a ts-node/tsx script and a test file. Note anything about its child-session (`startDebugging`) protocol that binvim's DAP client lacks.

## Context

- `src/dap/specs.rs` `DapAdapterSpec`, `BUILTIN_ADAPTERS`; `src/dap/io.rs` reverse requests.
- `src/install.rs` `Installer` variants and `pick_installer`.
- `CLAUDE.md`, "Adding a new DAP adapter".

## Answer
