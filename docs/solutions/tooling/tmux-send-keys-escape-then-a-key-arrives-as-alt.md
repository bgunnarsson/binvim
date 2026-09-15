---
title: tmux send-keys with Escape and the next key in one call reaches binvim as Alt+key, so the Escape never happens
date: 2026-09-15
category: tooling
module: tmux-driven manual checks
paths: []
tags: [tmux, crossterm, escape, alt, manual-testing, harness]
symptoms:
  - "a tmux check's :w didn't save — the file on disk is unchanged"
  - "the buffer ends up containing the command text, e.g. `xy:w` then `:q` on the next line"
  - "a check reads the wrong result (undo directory still 755) as though the code under test failed"
root_cause: "`tmux send-keys A y Escape ':w' Enter` writes ESC and ':' to the pty together; crossterm reads an ESC immediately followed by another byte as Alt+that key, so binvim stays in Insert mode and types the rest"
related:
  - docs/solutions/runtime/hung-up-tty-leaves-crossterm-poll-spinning.md
---

## Problem

Verifying that `:w` narrows `~/.cache/binvim/undo` to `0700`, the harness sent
`tmux send-keys -t un A 'y' Escape ':w' Enter`, then `:q`. `stat` printed `755`, which looked
like the permission change had failed.

## What didn't work

- **Reading `755` as a code failure.** The save had never run: `u.txt` still held `x`. The
  buffer's text, recovered on the next launch, was `xy:w` / `:q`. Escape had been lost, and
  `:w` and `:q` were typed into the buffer in Insert mode.
- **Polling for the UI before sending keys.** The harness already waited for `NORMAL` before
  typing, as `hung-up-tty-leaves-crossterm-poll-spinning.md` requires. That fixes a startup
  race, not how keys sent together are parsed.

## Root cause

Observed: with Escape and `:w` in one `send-keys` call, Escape had no effect and `:` was
inserted as text. With Escape in its own call, followed by a 0.3 s pause before `:w`, the save
ran and `stat` printed `700`.

Inferred, not read in crossterm's source: a terminal can't tell an Alt+key press from ESC typed
just before the key, because both arrive as `\e` plus the key. crossterm treats an ESC followed
by another byte in the same read as Alt+key, and one `send-keys` call writes all its keys at
once.

## Fix

No code change. The check sent `Escape` in a separate `send-keys` call and paused before the
next key. It then passed, and the undo directory was `700`.

## Prevention

- **A tmux harness never follows `Escape` with another key in the same `send-keys` call.** Send
  `Escape` on its own, then pause (or poll `capture-pane` for `NORMAL`) before the next keys.
  `send-keys … Escape ':w'` or `… Escape ':q' Enter` in a script is a violation.
- **Before judging an on-disk result, confirm the action ran:** the file's contents after `:w`,
  or the mode line after Escape. A check that goes straight from sending keys to `stat` or
  `ls` can report the harness's mistake as a bug.
