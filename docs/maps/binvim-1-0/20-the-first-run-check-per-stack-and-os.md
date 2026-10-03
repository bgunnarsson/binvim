---
title: How does the scripted first-run check run per stack and per OS?
kind: research
mode: afk
status: open
blocked_by: [01]
claimed_by:
---

## Question

[01](01-which-stacks-are-supported-for-zero-config.md) proves each promised stack with a scripted fresh-environment run: open a sample project, accept the first-run prompt, then check that hover answers, that format changes the file, and that a breakpoint is hit. Decide where the six sample projects live, how the script drives binvim and judges the screen (the pty harness from [04](04-how-to-measure-startup-and-input-latency.md) or tmux), what the Linux CI container starts from, how the user runs it on macOS and Windows, and where the results are recorded (a stacks table beside `docs/terminals.md`).

## Context

- [04](04-how-to-measure-startup-and-input-latency.md)'s pty harness; `scripts/terminal-probe.sh` and `docs/terminals.md` as the pattern for rows filled from the user's reports.
- `CLAUDE.md` on tmux checks, `-f /dev/null`, and waiting for the screen before sending keys.
- `.github/workflows/ci.yml`.

## Answer
