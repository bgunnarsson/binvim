---
title: The first-run prompt offers a promised stack's debugger along with its LSP and formatter
kind: task
mode: afk
status: open
blocked_by: [01]
claimed_by:
---

## Question

[01](01-which-stacks-are-supported-for-zero-config.md) counts a stack only when the first-run prompt installs all of it, and `missing_core_tools` checks only `Role::Lsp` and `Role::Formatter`. Make it include the auto-installable debugger, for the six promised stacks only, so the "also works" languages' prompts are unchanged.

## Context

- `src/install.rs:684-702` `missing_core_tools`; `src/app/installer.rs` `bundle_for_lang`, `maybe_prompt_toolchain`.
- `Installer::PythonModule` and `tool_installed` for debugpy.

## Answer
