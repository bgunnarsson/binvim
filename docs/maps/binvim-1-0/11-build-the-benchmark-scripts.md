---
title: Build the scripts that time startup and key-to-redraw and sample RSS, for binvim and a pinned LazyVim
kind: task
mode: hitl
status: open
blocked_by: [04]
claimed_by:
---

## Question

Build the method [04](04-how-to-measure-startup-and-input-latency.md) settled: a hyperfine startup run to first frame, a pty harness that judges a key's redraw on the parsed screen, and an RSS sample, all three against binvim and a pinned LazyVim (with `nvim --clean` as the floor) over the same files. Run them on the user's machine for the first numbers, and decide whether CI runs them as ratios.

## Context

- [04](04-how-to-measure-startup-and-input-latency.md)'s answer: tools, files, run counts, what to publish.
- `src/app.rs:1280-1480` run loop; `src/render.rs:102` `render::draw`.
- `.github/workflows/ci.yml` if a ratio job is added.

## Answer

