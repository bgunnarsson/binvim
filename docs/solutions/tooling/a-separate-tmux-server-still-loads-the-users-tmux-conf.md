---
title: A separate tmux server (`tmux -L <name>`) still loads the user's ~/.tmux.conf, and its plugins act on the user's saved sessions
date: 2026-09-23
category: tooling
module: tmux-driven manual checks
paths: []
tags: [tmux, tmux-resurrect, tmux-continuum, harness, manual-testing, isolation]
symptoms:
  - "`tmux -L dl ls` lists sessions the check never created: `cms: 1 windows (created …)`, `eimskip: 1 windows (created …)`"
  - "a scratch tmux server started for a check comes up holding copies of the user's own sessions"
  - "~/.local/share/tmux/resurrect/restore is touched by a run that only meant to start binvim"
root_cause: "`-L <name>` picks a different socket, not a different configuration: the new server reads ~/.tmux.conf like any other, so tmux-continuum's `@continuum-restore 'on'` restores the user's saved sessions into it, and its periodic save could later write that server's layout over the user's resurrect state"
related:
  - docs/solutions/tooling/tmux-new-session-runs-under-the-servers-environment-not-the-scripts.md
  - docs/solutions/tooling/tmux-send-keys-escape-then-a-key-arrives-as-alt.md
---

## Problem

The data-loss checks ran binvim in a tmux server of their own, started with
`tmux -L dl new-session …`, following the other tmux doc's advice to use a
separate server rather than `export`. The first `tmux -L dl ls` listed two
sessions the check had never made, `cms` and `eimskip`: the user's own, restored
into the scratch server.

## What didn't work

- **Taking `-L` as isolation.** It isolates the socket, and with it the running
  server's environment, which is what the other doc needed. The configuration
  is still the user's, and so are the plugins.

## Root cause

Observed: the user's `~/.tmux.conf` loads tmux-resurrect and tmux-continuum,
with `@continuum-restore 'on'` and `@continuum-save-interval '15'`. A server
started with only `-L dl` came up holding the user's `cms` and `eimskip`
sessions, and `~/.local/share/tmux/resurrect/restore` was touched at that
minute. Inferred from continuum's documented behaviour, not seen here: a server
left running past the save interval saves its sessions as the latest resurrect
state, so the user's next restore would bring back the scratch layout instead of
their own. The server was killed within minutes. No save file was written: the
newest was months old.

## Fix

No code change. Checks start their server with no configuration:

```sh
tmux -f /dev/null -L <name> new-session -d -s x "env HOME=… XDG_CACHE_HOME=… $BIN file"
```

`-f /dev/null` makes the server skip `~/.tmux.conf`, so no plugin loads. Every
later command on that socket needs the same `-L <name>`, and `-f` only matters
on the command that starts the server. The scratch server is ended with
`tmux -L <name> kill-server` once binvim has quit through `:q!`.

## Prevention

- **A check that starts a tmux server passes `-f /dev/null` with `-L <name>`.**
  A `tmux -L <name> new-session` (or `tmux new-session` against a fresh socket)
  without `-f /dev/null`, in a harness or a plan's verification steps, is a
  violation.
- **Before a check's first key, list the server's sessions** (`tmux -L <name>
  ls`). Any session the check didn't create means the user's configuration
  loaded: kill that server (`tmux -L <name> kill-server`, never the default
  server), and confirm no new file appeared in the user's resurrect directory.
- Written into: CLAUDE.md
