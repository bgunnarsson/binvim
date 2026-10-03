---
title: How can netcoredbg be installed with no manual step on macOS, Linux and Windows?
kind: research
mode: afk
status: open
blocked_by: [01]
claimed_by:
---

## Question

[01](01-which-stacks-are-supported-for-zero-config.md) needs C#'s debugger installed by the first-run prompt, and netcoredbg is `Installer::Manual` today. Find where each OS and architecture can get it (Samsung's GitHub releases, brew, scoop, winget, a fork with Apple-silicon builds), whether any is missing (macOS arm64), and what new `Installer` kind fetching a release archive would take, including its checksum or signature.

## Context

- `src/install.rs` C# bundle, `Installer` variants, `detect_managers`.
- `src/dap/specs.rs:114-126` the DOTNET adapter's `cmd_candidates`.
- `docs/external-tools.md` netcoredbg row.

## Answer
