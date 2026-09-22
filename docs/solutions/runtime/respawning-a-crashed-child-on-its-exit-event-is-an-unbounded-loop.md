---
title: Respawning a crashed child from its own exit event is an unbounded loop unless something counts the deaths
date: 2026-09-22
category: runtime
module: src/lsp
paths:
  - src/lsp/**
  - src/dap/**
  - src/test/**
  - src/app/lsp_glue.rs
  - src/app/dap_glue.rs
  - src/app/test_glue.rs
  - src/app/terminal_glue.rs
  - src/app/side_terminal_glue.rs
tags: [child-process, crash-recovery, respawn, backoff, lsp]
symptoms:
  - "status line flashing `lsp: <key> exited (N) — restarting` every frame"
  - "~10 forks a second of the same server binary for as long as the buffer stays open, visible in `ps` as a churn of fresh pids"
  - "a machine without the language's runtime (no JDK, csharp-ls with no solution) pegs a core the moment the file type is opened"
root_cause: the exit-detect → re-attach cycle had no memory — drain removed the dead client and emitted ServerExited, the handler called lsp_attach_active, ensure_for_path saw the key absent and spawned again, and a child that dies right after exec closes the loop at the event loop's own cadence
related: []
---

## Problem

The audit-fix cycle taught `LspManager` to notice a dead language server
(`try_exit_status` polled on `drain`), drop the entry and re-attach the active
buffer so the server comes back after a one-off crash. Review found that for a
server that dies *immediately* — a wrapper with no toolchain behind it, a
missing runtime, a repo-local `node_modules/.bin` binary that just exits — the
recovery is the loop: detect, remove, respawn, die, ~10 times a second for the
rest of the session, with no way out short of quitting.

## What didn't work

Nothing was tried and reverted — the loop shipped in the first version because
"respawn on death" reads like the whole fix. The reviewers had to construct the
dies-at-spawn case to see it; the kill-a-healthy-server test used to verify the
feature can't, because a healthy server stays up after its one respawn.

## Root cause

`spawn_spec` returns `Some` whenever fork/exec succeeds — it never waits for
the process or the `initialize` reply — so a binary that execs and exits still
becomes a live map entry each round. `drain` runs once per event-loop pass
(base poll 100ms) and re-detects the exit; the `ServerExited` handler called
`lsp_attach_active()` unconditionally; `ensure_for_path` respawns any spec
whose key is absent, and even cleared `crashed` — the only record of the prior
death — before inserting the new client. Nothing anywhere counted.

## Fix

`374fa8f`. `LspClient` records `spawned_at`; `LspManager` keeps
`crash_counts` per key — a death within `QUICK_EXIT_WINDOW` (60s) of spawning
increments it, a longer-lived server's death resets it to 1. At
`MAX_QUICK_EXITS` (3) the event carries `gave_up: true`, the handler stops
re-attaching, `ensure_for_path` refuses the key for the rest of the session,
and `:health` says "gave up after repeated crashes" instead of "restarts on
next attach".

## Prevention

- A handler that reacts to a child's unexpected exit by starting that child
  again must consume a budget the exits themselves replenish slowly: an
  attempt counter reset only by a long-enough healthy run, or a cooldown.
  A `Exited`/`Terminated`/EOF arm whose body reaches `spawn`, `ensure_*` or
  `attach` with no counter or timestamp check in the path is a violation,
  whichever child it supervises — LSP server, debug adapter, test runner,
  PTY tool.
- The give-up state must be visible somewhere the user will look (`:health`,
  the status line) and must actually stop the respawn at the spawn site, not
  only at the event handler — any other code path that re-attaches (buffer
  switch, config reload) reaches the same spawn.
- A test or manual check of crash recovery must include a child that exits
  immediately and repeatedly, not only a healthy process killed once. The
  kill-one-healthy-server check passes on the unbounded loop.
