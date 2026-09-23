---
title: tmux new-session runs its command under the tmux server's environment, so a script's exported HOME and XDG dirs never reach binvim
date: 2026-09-22
category: tooling
module: tmux-driven manual checks
paths: []
tags: [tmux, environment, HOME, XDG_CACHE_HOME, harness, manual-testing, cache]
symptoms:
  - "the scratch XDG_CACHE_HOME is empty after a run: `no matches found: …/cache/binvim/cursor/*.json`"
  - "files from a check turn up in the real ~/.cache/binvim: cursor/, undo/, sessions/<hash>.json whose cwd is the scratch directory, crash/, and scratch paths in recents"
  - "`tmux show-environment -g` prints HOME=/Users/<user> while the script exported another"
root_cause: "a running tmux server gives `new-session` its own global environment, captured when the server started, not the calling shell's exports, so `export HOME=… XDG_CACHE_HOME=…` before `tmux new-session … \"binvim file\"` changes nothing the pane's command sees"
related:
  - docs/solutions/tooling/a-separate-tmux-server-still-loads-the-users-tmux-conf.md
  - docs/solutions/tooling/lazygit-outside-a-repo-prompts-then-opens-another-repo.md
  - docs/solutions/tooling/tmux-send-keys-escape-then-a-key-arrives-as-alt.md
  - docs/solutions/runtime/hung-up-tty-leaves-crossterm-poll-spinning.md
---

## Problem

A harness for the last-cursor cache exported `HOME`, `XDG_CACHE_HOME` and
`XDG_CONFIG_HOME` under the session's scratchpad and then ran
`tmux new-session -d -s x "binvim d.txt"`, expecting the cursor, undo and
session files under the scratch directory. After `:q` the scratch cache had no
`binvim/` at all. Every earlier tmux run that day had written into the user's
real `~/.cache/binvim`: cursor and undo entries for scratch files, two session
files keyed to scratch directories, a crash log from the panic being reproduced,
and sixteen scratch paths in `recents`.

## What didn't work

- **Exporting the variables in the script.** The screen-based checks (mode
  line, cursor row) passed for hours with the exports in place, so nothing
  showed they were ignored. Only a check that read the scratch cache directory
  noticed.
- **Reading the empty directory as a code failure.** The first reading was that
  the quit path had not persisted the cursor. `find` over the scratchpad found
  no JSON anywhere, which pointed at the environment rather than the write.

## Root cause

Observed: `tmux show-environment -g` printed `HOME=/Users/<user>` while the
script's `HOME` was the scratch directory, and the files were in the real cache.
Inferred from tmux's documented behaviour, not read in its source: a `new-session`
against an already running server starts the pane's command from the server's
global environment, which is captured when the server starts; the client's
environment only updates the variables listed in `update-environment`, and
`HOME` and the XDG variables are not among them.

## Fix

No code change. The harness sets the variables in the command itself:

```sh
tmux new-session -d -s x "env HOME=$HOME XDG_CACHE_HOME=$XDG_CACHE_HOME XDG_CONFIG_HOME=$XDG_CONFIG_HOME $BIN d.txt; echo BINVIM_EXIT=\$?; sleep 30"
```

After that the scratch cache held exactly the entries each scenario predicted.
The files that had landed in the real cache were removed by hand: the session
files whose `cwd` was a scratch directory, the cursor and undo entries and the
crash log by their timestamps, and the `recents` lines containing the
scratchpad path.

## Prevention

- **A tmux harness that isolates binvim's files sets them in the pane's command**
  (`"env HOME=… XDG_CACHE_HOME=… binvim …"`) or starts its own server
  (`tmux -f /dev/null -L <name> …`; without `-f /dev/null` that server loads
  the user's `~/.tmux.conf` and its plugins, see
  `a-separate-tmux-server-still-loads-the-users-tmux-conf.md`), never by
  `export` before `tmux new-session`. An
  `export HOME=` or `XDG_CACHE_HOME=` line followed by
  `tmux new-session … "binvim …"` in a script is a violation.
- **Before trusting a run, confirm the isolated directory received files**
  (`ls <scratch>/cache/binvim`). An empty one means the real cache took them, and
  `~/.cache/binvim` then holds sessions keyed to the scratch directory, cursor
  and undo entries for scratch files, and scratch paths in `recents`, which have
  to be removed before the check is called clean.
