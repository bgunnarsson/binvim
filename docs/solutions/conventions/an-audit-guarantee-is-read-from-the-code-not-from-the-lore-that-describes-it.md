---
title: An audit's guarantee is read from the code that keeps it, not from the plans, lore and CLAUDE.md prose that describe that code
date: 2026-09-23
category: conventions
module: docs/data-loss-audit.md
paths:
  - "docs/**/*audit*.md"
  - ROADMAP.md
  - KNOWN_ISSUES.md
tags: [audit, data-loss, guarantee, evidence, write_atomic, recovery, pid, documentation]
symptoms:
  - "The file is replaced whole or not at all. A failed write leaves the old file."
  - "A pid alone isn't trusted: the process's identity is checked too."
  - "Every buffer with unsaved changes is dumped every 4 s."
  - "(2026-09-23: no path loses text; one display gap, a deleted clean file isn't marked as gone)"
  - "an audit row's \"Kept by\" names a function or a command that doesn't exist (`reload_buffer_from_disk`, `:saveas`)"
root_cause: "docs/data-loss-audit.md's guarantees were compiled from the plans' Context sections, the solution docs and CLAUDE.md's module notes, each of which describes the common path or one platform, so the audit stated them as unconditional; the cited tests were then checked to exist, not checked to prove the sentence beside them"
related:
  - docs/solutions/runtime/config-reload-misses-state-derived-from-app-config.md
  - docs/solutions/security/an-upward-search-trusts-what-other-users-own-or-can-write.md
  - docs/solutions/data/replacing-a-file-by-rename-changes-more-than-its-contents.md
  - docs/solutions/runtime/a-recorded-pid-does-not-identify-a-process.md
  - docs/solutions/runtime/open-buffer-runs-for-batch-edits-nobody-sees.md
---

## Problem

The 0.7 data-loss audit (`docs/data-loss-audit.md`, plan
`docs/plans/2026-09-23-the-data-loss-audit-is-written-down-and-its-gaps-are-tested.md` (removed once done; `git show b98f700:docs/plans/2026-09-23-the-data-loss-audit-is-written-down-and-its-gaps-are-tested.md`)) is a
table: each path, its guarantee, the code that keeps it, the evidence. Every cited test existed,
and all five tmux checks passed. Review still confirmed nine findings, and six were claims the
code doesn't keep. The ROADMAP line built on the audit said "no path loses text".

## What didn't work

- **Taking the guarantee from the doc that introduced the mechanism.** The writes plan and
  CLAUDE.md say `write_atomic` "writes a temp file and renames it over the target … so a failed
  write can't leave the file truncated". That is the rename path. `write_atomic_with`
  (`src/paths.rs`) returns `std::fs::write(&target, bytes)`, a truncate-then-write, in five
  places: a dangling symlink, other hard links, `PermissionDenied` creating the temp file, a
  `take_owner` failure, and a failed rename. The audit's `:w` and disk-full rows said "whole or not
  at all" and "the target is untouched".
- **Taking a platform's fix as the whole fix.** The pid solution doc's Fix section says the
  Windows check matches the image name, and then "The unix `kill -0` path is unchanged." The audit
  kept the first half: "the process's identity is checked too".
- **Taking the CLAUDE.md summary's "a dirty buffer's text dumped every four seconds" as covering
  every buffer.** `write_recovery_now` and `refresh_recovery_snapshot`
  (`src/app/recover_glue.rs`) skip a buffer with no path, so a `[No Name]` buffer has no recovery
  at all.
- **One sentence for two call sites.** "A file with a pending recovery dump is skipped" is true of
  `:S` (`app/input.rs`, `skipped += 1; continue`), but `apply_concrete_edits` (`app/lsp_glue.rs`)
  bails the whole LSP batch.
- **Naming the code from memory.** "Kept by `reload_buffer_from_disk`": the dirty guard is in
  `maybe_reload_from_disk`, and the nearest real name, `reload_buffer_from_disk_inner`, has no
  guard. `:saveas` isn't a command (`src/command.rs` parses only `w <file>` into `WriteAs`).
- **Checking that cited tests exist.** The plan's verification was `cargo test <name> -- --list`
  for every name. `write_atomic_keeps_hard_links_together` and
  `a_failed_rename_writes_the_file_in_place` exist and pass, but they prove those cases *save*.
  Cited under "replaced whole or not at all", they were evidence against the row.
- **Writing a plan Decision up as a gap.** The plan decided that a deleted clean file "is marked as
  not on disk … If the check finds otherwise, that's a bug to fix in this plan. It isn't a
  behaviour to document." Work found no marker, documented it as a display gap and marked the plan
  done.

## Root cause

Verified in the code at each line above, and by review's verifiers. Every source the audit was
written from was accurate about what it set out to describe: the common path, one platform, the
call site its author was fixing. None was written as a list of exceptions. An audit made from
them adds up their best cases and loses their conditions. The tests' names made each row look
backed, and the check that the tests existed passed.

## Fix

- `bed2990`: the audit's `:w`, disk-full, pid, recovery, `:S` / LSP and watcher rows are
  corrected from the code. A `[No Name]` row is added, `:saveas` is removed, and the check
  outcomes are recorded in the audit itself. `KNOWN_ISSUES.md` carries the two open gaps
  (in-place writes, pathless buffers). The ROADMAP line names them instead of "no path loses
  text", and the README links the audit.
- `bd2cd6f`: the unix `process_alive` matches `ps -o comm=` against the executable's name, so the
  pid row is now true.
- `3a50353`: `Buffer.gone` and the `[deleted]` status-line marker. The plan's Decision is
  implemented instead of being recorded as a gap.

## Prevention

- **A guarantee row names the function that keeps it, and every `return` in that function which
  bypasses the guarantee is either listed in the row or absent.** "Whole or not at all" beside a
  function with an in-place `std::fs::write` fallback is a violation. So is "skipped" for a site
  that `bail!`s. The row is written from reading that function, not from a plan, a solution doc's
  Fix section or CLAUDE.md.
- **A claim that holds on only some platforms, some buffers or some call sites says which.** A
  `#[cfg(windows)]` fix cited under an unqualified claim, a guarantee over "every buffer" whose
  code skips `path: None`, or one sentence covering two call sites that behave differently, is a
  violation.
- **Each name in "Kept by" resolves.** Before the doc is committed, `grep -rn "fn <name>" src` for
  every function and `src/command.rs` for every `:command`. A name that doesn't resolve is a
  violation.
- **A test cited as evidence asserts the guarantee in its row, not only that the path runs.** For
  each cited test, the row's sentence has to be what the test's asserts check. A fallback test
  cited for atomicity, or a "saves" test cited for "untouched on failure", is a violation.
  `cargo test <name> -- --list` shows that a test exists, not that it proves the row.
- **A summary line (ROADMAP, a release note) claims no more than the audit's weakest row.** "No
  path loses text" beside an audit with an open gap, or one that isn't in `KNOWN_ISSUES.md`, is a
  violation.
- **A plan Decision that says "bug to fix, not a behaviour to document" is fixed before the plan is
  marked done,** or the Decision is revised in the plan with the reason. A task that ticks with
  "recorded as a gap" against such a Decision is a violation.
