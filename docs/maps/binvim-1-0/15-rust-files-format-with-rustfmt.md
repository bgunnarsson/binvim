---
title: Rust files format with rustfmt on save and on demand
kind: task
mode: afk
status: open
blocked_by: [01]
claimed_by:
---

## Question

[01](01-which-stacks-are-supported-for-zero-config.md) makes Rust a promised stack, which needs a formatter, but `format_buffer` has no `"rs"` arm, so `.rs` files get "no formatter configured". Add one that runs `rustfmt` over stdin, honouring the crate's edition and `rustfmt.toml`.

## Context

- `src/format.rs:15-55` `format_buffer` and its `run_stdin_pipe` helpers.
- `src/install.rs:420` the Rust bundle's `rustfmt` tool.
- `docs/external-tools.md` formatter rows.

## Answer
