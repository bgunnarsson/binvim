---
title: lazygit started outside a git repo asks on a plain prompt, and declining it opens the most recent repo and fetches there
date: 2026-09-23
category: tooling
module: tmux-driven manual checks of src/app/lazygit_glue.rs
paths:
  - src/app/lazygit_glue.rs
  - TERMINALS.md
tags: [lazygit, tmux, harness, manual-testing, suspend, git, recent-repos, auto-fetch, sigint]
symptoms:
  - "Not in a git repository. Create a new git repository? (y/N):"
  - "after <space>gg the tmux pane shows the shell's main screen, not lazygit, for 10+ seconds, as though lazygit hung"
  - "lazygit's Status panel names a repo that isn't the one binvim is editing"
  - "`git reflog main` in an unrelated repo shows an update nobody made, and the Command log shows `git update-ref --stdin`"
  - "binvim is gone and the shell prompt is back after Ctrl-C at lazygit"
root_cause: "lazygit_glue starts lazygit in the active buffer's directory; outside a repo lazygit shows a cooked-mode y/N prompt on the main screen, and a declined prompt opens the first entry of its recent-repos state, where its default auto-fetch and auto-forward run against a repo the check never meant to touch"
related:
  - docs/solutions/tooling/tmux-send-keys-escape-then-a-key-arrives-as-alt.md
  - docs/solutions/tooling/tmux-new-session-runs-under-the-servers-environment-not-the-scripts.md
  - docs/solutions/runtime/hung-up-tty-leaves-crossterm-poll-spinning.md
---

## Problem

Running `TERMINALS.md` check 20 (the lazygit round trip) in tmux, the harness
opened a scratch copy of the fixture in the session scratchpad, pressed
`<space>gg`, waited a fixed 2.5 s, then went on sending the next checks' keys
(`q`, `:8` Enter, `0`, arrows, `i`, …). lazygit ended up open on
`~/Development/osar`, an unrelated client repo, and had fast-forwarded that
repo's local `main` from `3334f42a` to `origin/main` (`766cbc4b`).

## What didn't work

- **Reading the blank pane as a lazygit hang.** The pane showed the shell's
  main screen for over 10 s, and a resize didn't make lazygit draw. The same
  lazygit started directly in a tmux pane drew in about 2 s. What was on the
  main screen, near the bottom, was lazygit's question:
  `Not in a git repository. Create a new git repository? (y/N): q:8`.
- **Waiting a fixed time, then sending keys.** The keys meant for binvim were
  typed at that prompt. `q:8` plus Enter declined it.
- **`Ctrl-C` to back out of the prompt.** With binvim's raw mode off during the
  suspend, `Ctrl-C` sent SIGINT to binvim as well as lazygit, and binvim exited
  to the shell. That part was a binvim bug, fixed in `a4ace97`.
- **Isolating lazygit with `XDG_CONFIG_HOME`.** lazygit wrote a `config.yml`
  under the scratch config directory, but its recent-repos list lives in
  `~/Library/Application Support/lazygit/state.yml` on macOS. It read that list
  and wrote to it (osar went to the top).

## Root cause

Observed:

- `cmd_lazygit` (`src/app/lazygit_glue.rs`) runs lazygit in the active buffer's
  parent directory, falling back to the cwd only when there is no buffer path.
  A buffer outside any repo gives lazygit a directory with no `.git` above it.
- In that case lazygit doesn't draw its UI. It prints a plain y/N prompt on the
  main screen, after binvim has left the alternate screen, and reads a cooked
  line.
- After the declining answer, lazygit opened `~/Development/osar`, which was
  the first entry in `recentrepos` in `state.yml`.
- In osar, `main@{2026-09-23 14:39:19}` moved `3334f42a` → `766cbc4b`, which
  equals `origin/main` and is a strict fast-forward. The Command log showed
  `git update-ref --stdin`. osar's checked-out branch and working tree were
  untouched.

Inferred, not read in lazygit's source: that answering N (rather than some
later key) is what opens the most recent repo, and that the ref move came from
lazygit's default `git.autoFetch` followed by `git.autoForwardBranches`
(`onlyMainBranches`), which fast-forwards branches that aren't checked out.

## Fix

No code change for the prompt. The check was rerun from `docs/terminal-check.md`
inside the repo, so lazygit opened binvim's own repo. The scratch lazygit config
got

```yaml
git:
  autoFetch: false
  autoRefresh: false
  autoForwardBranches: none
```

and the harness polled `capture-pane` for lazygit's `Status` panel before
sending `q`, then polled for `NORMAL` before sending anything else.
`TERMINALS.md` check 20 now says the fixture has to be the in-repo one. osar's
`main` was left at `origin/main`, the user's to reset
(`git update-ref refs/heads/main 3334f42a`) if they want.

`a4ace97` made the `Ctrl-C` half harmless: SIGINT and SIGQUIT are ignored by
binvim while a suspended child owns the terminal (`interrupts_quit`,
`recover_glue::guard_interrupts`).

## Prevention

- **A harness that starts lazygit (or yazi, or `:install`) waits for that
  program's own screen before sending a key, and waits for binvim's mode line
  after it exits.** A fixed `sleep` followed by `send-keys` after `<space>gg`,
  `:lazygit` or `:lg` is a violation: every key sent in between goes to the
  child, and lazygit acts on them in whatever repo it opened.
- **lazygit is only started from a buffer inside the repo the check is about.**
  A harness that opens a scratch or `/tmp` file and then runs `<space>gg` gives
  lazygit a directory outside any repo. It answers the prompt with whatever
  keys arrive next, and declining opens a real repo of the user's.
- **A harness that runs lazygit sets `autoFetch: false` and
  `autoForwardBranches: none` in the config lazygit reads**, and doesn't count
  on `XDG_CONFIG_HOME` to keep lazygit off the user's state: `state.yml`
  (recent repos) stays in `~/Library/Application Support/lazygit` on macOS.
- **Before calling a lazygit check clean, confirm the repo lazygit showed**
  (its Status panel names it) is the one intended. If it wasn't, check
  `git reflog` in the repo it did open, and report any ref it moved.
- **A change to `cmd_lazygit`'s start directory keeps a non-repo buffer in
  mind:** a directory with no repo means the plain-text prompt, not lazygit's UI.
- Written into: CLAUDE.md
