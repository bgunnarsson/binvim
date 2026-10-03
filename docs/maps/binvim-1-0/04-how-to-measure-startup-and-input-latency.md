---
title: How are startup time, input latency and memory measured, reproducibly, against a Neovim distro?
kind: research
mode: afk
status: open
blocked_by: []
claimed_by:
---

## Question

The 1.0 bar needs budgets that are met and published. Before numbers can be set, decide the method: the tools (hyperfine, typometer-style key-to-photon, a pty harness timing a key to its redraw, RSS sampling), the comparison setup (LazyVim version, files opened), what can run as a script in this repo and in CI, and what has to run by hand.

## Context

- `docs/roadmap.md` Horizon 2, "Performance budget with numbers" (render coalescing).
- `src/app.rs` run loop and `src/render.rs` for where a key becomes a frame.
- `scripts/terminal-probe.sh` and `scripts/terminal-check.rs` as the pattern for a check script.
- Web sources on measuring terminal editor latency; cite links.

## Answer

