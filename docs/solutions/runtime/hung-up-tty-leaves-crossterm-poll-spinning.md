---
title: A hung-up terminal leaves crossterm's poll spinning in read, so a signal flag the event loop checks is never seen
date: 2026-09-15
category: runtime
module: src/app/recover_glue.rs
tags: [signals, sighup, sigterm, crossterm, event-loop, recovery]
symptoms:
  - "binvim at 100% CPU after its tmux session or terminal window is closed"
  - "closing the terminal doesn't write recovery files, though kill -TERM does"
  - "sample shows the main thread in UnixInternalEventSource::try_read → FileDesc::read, never returning"
root_cause: "when the tty hangs up, crossterm::event::poll's unix source keeps reading the dead fd in a loop inside try_read and never returns to the caller; before a SIGHUP handler was installed the default action killed the process, which hid it"
related:
  - docs/plans/2026-09-15-writes-cannot-lose-work-and-clippy-gates-ci.md
  - docs/solutions/tooling/tmux-send-keys-escape-then-a-key-arrives-as-alt.md
---

## Problem

The plan caught SIGTERM and SIGHUP with `signal_hook::flag::register` on an `AtomicBool` that the
run loop checked after each poll, then dumped dirty buffers and exited. SIGTERM worked (exit in
0.13s, recovery file written). Closing the tmux session left binvim alive at 100% CPU with no
recovery file.

## What didn't work

- **The flag checked by the loop.** The loop never came round to check it: `sample` on a debug
  build showed the main thread inside `crossterm::event::poll` →
  `InternalEventReader::poll` → `UnixInternalEventSource::try_read` → `FileDesc::read`,
  spinning. Capping the poll timeout doesn't help, because the spin is inside the one call.
- **Relying on the next draw to fail.** The plan assumed a SIGHUP'd terminal would fail the next
  render with EIO and return `Err` out of `run`. The loop never reaches the render.
- **First test runs were misread** as handler failures (`EXIT=143`, keys echoed by the shell): on
  macOS a freshly rebuilt binary can take over a second to start, so keys sent after a fixed sleep
  arrived before raw mode. Waiting until the pane showed `NORMAL` fixed the harness.

## Root cause

Observed with `sample`, not read in crossterm's source: after a hangup, crossterm's unix event
source keeps getting nothing from the dead tty and retries inside `try_read` rather than
returning. With SIGHUP's default action replaced by a handler, the process no longer dies, so the
spin is what's left. A process that ignores SIGHUP (under `nohup`, say) would hit the same spin.

## Fix

`d271d62`: `App::spawn_signal_recovery` runs a `signal_hook::iterator::Signals` thread for
SIGTERM and SIGHUP. The run loop refreshes `recovery_snapshot` (each dirty buffer's path and a
rope clone, which shares nodes) every iteration. The thread writes recovery files from it, restores
the terminal with `crash::restore_terminal_best_effort`, and `process::exit(128 + signal)`. The
same mutex guards the loop's own dumps. A signalled exit doesn't save the session.

## Prevention

- **Work that must happen on SIGHUP never waits for the event loop.** A handler that sets a flag
  for the loop to read, or code that expects `crossterm::event::poll` / `read` to return after the
  terminal closes, is a violation. It runs on a signal thread from state already shared with it.
- **Installing a handler for a signal whose default action is to exit** is checked by delivering
  that signal the real way (tmux `kill-session` for SIGHUP) and confirming the process exits. The
  handler removes the exit that was hiding whatever the process does next.
- **A tmux check of a rebuilt binary waits for the UI to draw** (poll `capture-pane` for the mode
  line), never a fixed sleep.
- **A tmux check that ends with `tmux kill-session` has sent SIGHUP,** which writes recovery files
  for dirty buffers and, since `20a5dcf`, the session. The next check in the same file or cwd
  applies that recovery text: a buffer read `delta PANICKED PANICKED` after two runs that each
  typed the word once. Checks quit binvim with `:e!` / `:q!` first, or use a fresh file and
  directory per run, and delete only the session files whose `cwd` is that run's own directory.
  A harness that reuses a file after `kill-session` without doing either is a violation.
