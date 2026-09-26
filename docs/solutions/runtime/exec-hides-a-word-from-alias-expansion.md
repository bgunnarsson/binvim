---
title: exec puts its argument outside alias-expansion command position in bash, zsh and fish
date: 2026-09-26
category: runtime
module: src/terminal.rs
paths:
  - src/terminal.rs
  - src/app/side_terminal_glue.rs
tags: [shell_launch, shell_launch_bare_word, exec, alias, bash, zsh, fish, command position]
symptoms:
  - "a shell alias for the launched tool name is silently ignored — the real binary off PATH runs instead"
  - "zsh: command not found: <name> (when the alias was the only thing defining that name)"
  - "unquoting a word before handing it to `exec <word>` looks like it should let a same-named alias match, and doesn't"
root_cause: "exec's argument is never in alias-expansion command position: bash and zsh only expand an alias for the first word of a simple command, and in `exec word` that first word is `exec`, not `word`. Fish's `alias` defines a shell function, and fish's `exec` can only invoke a real external command, never a function, so it can't reach a fish alias either. Quoting was never the only thing standing between a word and alias expansion — `exec` itself was."
related:
  - docs/solutions/security/a-word-quoted-for-one-shell-is-parsed-again-by-the-next.md
---

## Problem

`open_side_terminal` (`src/app/side_terminal_glue.rs`) launches an AI tool (`claude`, `binai`, …)
through the user's login shell so `.zshrc`/`.zprofile` etc. load first. `shell_launch` always quoted
the tool name with `shell_quote`/`fish_quote`, which is correct for safety but turns it into a
literal command name — a shell alias the user defined for that name (`alias binai='binai --flag'`)
never matched. The fix was meant to be: add `shell_launch_bare_word`, skip the quoting for a
strictly validated bare word, and let the alias expand.

## What didn't work

- **Unquoting the word but keeping the `exec ` prefix** (commit `8f4a975`, then a follow-up test
  commit `dd500c5`). The generated Posix/Fish line was `exec <word>` — quoting was gone, but the
  feature still didn't work. Five independent reviewers (correctness, known-mistakes, security,
  conventions, simplicity) and a sixth verification pass that actually ran the shells all reported
  the same defect independently. Confirmed on-machine with `zsh -i`:
  `alias probe='echo X'; exec probe` → `zsh: command not found: probe`, while
  `alias probe='echo X'; probe` (no `exec`) → `X`. The unit tests added alongside `8f4a975` and
  `dd500c5` only asserted the generated *string* (`"exec binai"`), which can't catch this — the
  string was exactly as designed, and the design was the bug.

## Root cause

Bash and zsh only check a word for alias expansion when it is the first word of a simple command.
`exec word` is one simple command whose first word is `exec`; `word` is merely `exec`'s argument,
so it is never even considered for expansion, whether or not it's quoted. Fish's `alias` defines a
shell function, not a special alias table, and fish's `exec` builtin can only replace the process
with a real external command — it cannot invoke a function — so `exec word` can't reach a fish
alias either. This holds regardless of quoting: quoting only ever controlled how a word round-trips
through parsing, not which position in a command line it occupies.

## Fix

`src/terminal.rs`, commit `69f776a`: on the bare path only (`shell_launch_impl`'s new `bare`
parameter), the leading `exec ` is no longer emitted. The line becomes just the (unquoted) word
itself — the whole `-c` script when there's no `cd`, or the first word after `cd … &&` — which *is*
command position in all three shells. `shell_launch`'s existing quoted/`exec`'d path (the task
runner) is untouched; only `shell_launch_bare_word`'s generated line changed, from
`exec binai` to `binai`.

This trades away process replacement on the bare path: the launched word now forks as an ordinary
foreground command of the `-l -i -c` script instead of replacing the shell's process image. That's
fine here — the wrapping shell has nothing left in its script once the command finishes, so it exits
right after, and the terminal still shows no residual prompt (`src/app/side_terminal_glue.rs`,
comment fixed in follow-up commit `2877be6` to stop crediting `exec` for that).

## Prevention

- **A launch meant to let a shell alias override it puts the alias-target word as the command
  itself, never as an argument to `exec` or any other builtin.** A generated line of the form
  `exec <word>` (or `<builtin> <word>` for any builtin that doesn't itself re-check aliases) is a
  violation, no matter whether `<word>` is quoted. Grep the generated line, not just whether a
  `shell_quote`/`fish_quote` call was removed.
- **A test for "alias still expands" asserts the actual generated line/args, and ideally proves it
  behaviourally** (e.g. `alias probe=...; <the exact generated line>` under a real shell), not only
  that quoting was dropped. A test that only checks a word survived unquoted, without checking its
  position relative to `exec` or another leading builtin, passes on this exact bug.
