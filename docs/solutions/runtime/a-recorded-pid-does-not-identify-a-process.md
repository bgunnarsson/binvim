---
title: A recorded pid doesn't identify a process, so "is that pid alive" says yes for whatever inherited it
date: 2026-09-15
category: runtime
module: src/recover.rs
paths:
  - src/recover.rs
  - src/app/recover_glue.rs
tags: [pid, recovery, windows, tasklist, kill, second-instance]
symptoms:
  - "another binvim (pid 8124) has unsaved changes to this file — with no other binvim running"
  - "after a crash or power loss, reopening the file never offers the recovered text, and the message repeats on every open"
  - "recovery works again once some unrelated program exits"
root_cause: "RecoveryFile.pid is checked with a liveness probe (`tasklist /FI \"PID eq N\"` on Windows, `kill -0` on unix) that answers for any process holding that pid, and operating systems hand a dead process's pid to new ones"
related:
  - docs/solutions/runtime/open-buffer-runs-for-batch-edits-nobody-sees.md
  - docs/plans/2026-09-15-overlay-keys-follow-draw-order-and-recovery-gaps-close.md
---

## Problem

Recovery files record their writer's pid so a second binvim leaves a running one's dump alone
(`recover::held_by_another_process`). Windows had no check, so `process_alive` was given a
`tasklist` call that looked for a row with the pid. Review found that a crashed binvim's dump
would be held back whenever the pid had been given to any other process.

## What didn't work

- **Matching the pid field alone.** `tasklist_lists_pid` parsed the CSV carefully: the second
  quoted field, so a pid that's only a prefix of another doesn't match, and not keyed on the
  "No tasks are running" line, which Windows translates. Every test passed. But the question it
  answered was "does any process have this pid", not "is the binvim that wrote this still
  running".
- **Assuming the unix check was the model.** `kill -0` has the same flaw for processes the
  same user owns. It's less exposed only because another user's or root's process fails with
  `EPERM` and so counts as dead. `tasklist` lists every session's processes, SYSTEM services
  included, so on Windows nearly every reused pid matches.

## Root cause

Read in the code, and confirmed by the review's verifier: `held_by_another_process` is
`rec.pid != 0 && rec.pid != std::process::id() && process_alive(rec.pid)`, and nothing after
it checks what the process is. `apply_recovery` (`src/app/recover_glue.rs`) returns before
applying when it's true, and it stays true for as long as the process that inherited the pid
runs. The dump isn't deleted, only held back.

Not observed: nobody ran this on Windows. That `tasklist` without `/V` lists other sessions'
processes without elevation, and that Windows reuses small pids quickly, come from Windows'
documented behaviour rather than a run.

## Fix

`516040f`: the Windows `process_alive` also matches the row's image name, case-insensitively,
against `std::env::current_exe()`'s file name (`tasklist_lists`), so a pid held by
`svchost.exe` no longer counts. If there's no exe name, the dump is treated as a crash's, which
is how Windows behaved before the check. A binary renamed between the two runs
(`binvim-dev.exe`) misses a live instance and falls back to the same behaviour.

`bd2cd6f`: the unix `process_alive` does the same through `ps -o comm= -p <pid>`, whose last
path component (`ps_comm_is`) must be the executable's name. macOS prints a path and Linux a
name cut to 15 bytes, so both are handled.

## Prevention

- **A liveness check on a stored pid also checks the process is what wrote it:** an image name,
  a start time, or a token in the record. A new `process_alive(pid)` style probe whose only
  input is the pid is a violation.
- **A test for such a check includes a row or process with the right pid and the wrong
  identity** (for Windows, `"svchost.exe","1234",…`). Tests that only vary the pid don't catch
  this.
- **When the identity can't be established, the check answers "not running"** and the dump is
  treated as a crash's. Holding recovered text back on an unconfirmed match is the worse failure.
