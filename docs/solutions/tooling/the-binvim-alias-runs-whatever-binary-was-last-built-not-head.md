---
title: the binvim alias runs whatever binary was last built, not HEAD
date: 2026-09-26
category: tooling
module: dev workflow — verifying a fix through the user's binvim alias
paths: []
tags: [target/release, cargo build --release, stale binary, verification, mtime]
symptoms:
  - "zsh:1: command not found: binai (still, after a commit that was supposed to fix exactly this)"
  - "a fix is independently re-verified working in an isolated shell probe, yet the user's own binvim still shows the pre-fix symptom"
  - "a session concludes a shipped, reviewed fix 'didn't even fix what we set out to fix' and starts re-diagnosing the code"
root_cause: "the user's `binvim` shell alias points at target/release/binvim, a compiled artifact — the report was tested against a binary built before the fix commit landed, so every report against it was necessarily pre-fix behaviour"
related:
  - docs/solutions/runtime/exec-hides-a-word-from-alias-expansion.md
---

## Problem

A bug in the side terminal's `binai` launch (`docs/solutions/runtime/exec-hides-a-word-from-alias-expansion.md`)
was fixed, reviewed by five reviewers, and independently re-verified by a coarchitect that ran real
`zsh -i` alias probes and confirmed the shell semantics. The fix landed as commit `69f776a`. A
following session still saw `zsh:1: command not found: binai` when exercising the feature through
binvim itself, and concluded the fix must be wrong after all.

## What didn't work

- Re-confirming the shell semantics in the abstract (reviewer consensus, then a coarchitect running
  isolated `alias probe=...; exec probe` vs `probe` probes). This was correct and thorough, but it
  answers "is the generated shell line right", not "is the user running that line."
- Reproducing the exact failing invocation (`/bin/zsh -l -i -c binai`) against the user's real,
  148-line `~/.zshrc` (oh-my-zsh, nvm, many PATH exports) in a live shell. This also worked
  flawlessly end to end — the alias expanded, `binai` launched, `type binai` and `whence binai`
  both resolved to the alias target. It proved the source was correct and still left the original
  report unexplained.
- What was missing from both: comparing `target/release/binvim`'s mtime against the fix commit's
  timestamp. Nobody checked whether the binary the user's `binvim` alias actually runs was built
  after the fix, before spending a review cycle and two live-reproduction passes re-litigating
  whether the fix itself was correct.

## Root cause

`~/.zshrc` aliases `binvim` straight to `target/release/binvim` — a compiled binary, not `cargo run`
against source. `target/release/binvim`'s mtime was `2026-09-25T23:26:26Z`; the fix commit
`69f776a` landed at `2026-09-26T00:30:59Z`, about an hour later. Every time the user ran `binvim`
between the fix landing and the next `cargo build --release`, they were running the pre-fix binary,
which still had `exec binai` in it. The fix in source was correct the whole time; the report was
stale-binary noise, not new evidence against it.

## Fix

No source change. `cargo build --release` produced a binary with mtime `2026-09-26T01:13:09Z`,
after HEAD's commit time — the user's alias now runs post-fix code.

## Prevention

- **Before re-diagnosing a fix that "still reproduces" through the user's normal `binvim`
  invocation, compare `target/release/binvim`'s mtime against the fix commit's `git log -1
  --format=%cI` for the files it touched.** A binary mtime before the fix commit means the report
  is stale-binary noise: rebuild and ask the user to retest before touching the code again.
- **A live reproduction that runs the fixed logic directly** (an isolated shell probe, a unit test,
  `cargo run`) **proves the source is correct — it does not prove the user's own binary is
  current.** Don't let "I reproduced the fixed behaviour and it works" and "the user says it's
  still broken" coexist without checking which binary the user is actually running; they are
  answers to different questions.
- CLAUDE.md's "Build, run, test" section already says any change meant to be exercised through the
  user's alias needs a fresh `cargo build --release` — this applies as much to *verifying* a fix as
  to trying a new feature, including a fix produced in the same session that's now being doubted.
