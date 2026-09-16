---
title: open_buffer also opens files for batch edits that save without showing them, so anything it does to a buffer reaches disk unseen
date: 2026-09-15
category: runtime
module: src/app/buffers.rs
tags: [open_buffer, recovery, substitute, lsp, workspace-edit, save]
symptoms:
  - ":S/old/new/ writes a file's crash-recovered text to disk, and the recovery file is gone"
  - "an LSP rename lands its edits at the wrong offsets in a file that had a recovery file"
  - "the 'recovered unsaved changes' notice never shows — the substitution count replaced it"
root_cause: "open_buffer is the one open path for users and for programmatic edits alike — the project-wide :S loop and apply_concrete_edits open, edit and save files the user never sees — so a behaviour added to open_buffer on the assumption that a person is looking also runs inside those loops"
related:
  - docs/plans/2026-09-15-writes-cannot-lose-work-and-clippy-gates-ci.md
  - docs/solutions/runtime/a-recorded-pid-does-not-identify-a-process.md
  - docs/solutions/security/an-upward-file-search-trusts-files-other-users-can-plant.md
---

## Problem

Crash recovery was applied in `open_buffer`: a leftover recovery file's text replaced the buffer
as unsaved changes with a status notice, on the reasoning that nothing reaches disk until the user
types `:w`. Review found two callers that do the `:w` themselves.

## What didn't work

- **"Nothing reaches the file until `:w`".** It held for every interactive open. The project-wide
  `:S` loop (`src/app/input.rs`) runs `open_buffer` → `substitute` → `save_active` for each file
  ripgrep returns. LSP `apply_concrete_edits` (`src/app/lsp_glue.rs`) runs `open_buffer` →
  offset edits → `Buffer::save`. The user sees neither buffer, and each loop overwrites
  `status_msg` at the end.
- **The first fix, checking for pending recovery by path, missed files at first:** ripgrep names
  files `./x.rs`, and a `.` segment changes the recovery key's hash. `std::path::absolute` removes
  `.` segments, which is what `Buffer::from_path` stores; the check has to normalize the same way.
  A tmux run caught it. Its first attempt also put the cache directory inside the project, so `:S`
  rewrote the recovery JSON itself; keep test caches outside the searched tree.

## Root cause

Verified by reading the call sites. `open_buffer` has no notion of who asked. A crash's stale text
was substituted and saved over a newer file, with the save then deleting the recovery file as this
session's. LSP positions computed against disk text were applied to different text and saved
without even `save_active`'s guards.

## Fix

`befe87c`:

- **Batch edits check first.** Both flows ask `pending_recovery(path)` before opening. `:S` skips
  such a file and says so; a workspace edit refuses as a whole before touching anything.
- **Recovery files belong to their session.** A recovery file is removed only by the session that
  wrote or applied it (`recovery_written`), so a save that never showed the text can't delete it.
- **Live dumps are left alone.** Dumps record their writer's pid, and another running binvim's dump
  is neither applied nor removed.

## Prevention

- **Anything added to `open_buffer`, or to `switch_to`, which it calls,** that changes the buffer's
  text, dirty state or files on disk has to be safe with no one looking: check it against the `:S`
  loop and `apply_concrete_edits`, which open, edit and save in one pass. A change there that says
  "the user will see this before saving" is a violation unless both flows are handled.
- **A new flow that opens files in order to edit and save them** checks `pending_recovery` first,
  as `:S` and workspace edits do.
- **A path taken from outside the buffer list** — ripgrep output, an LSP URI, a picker — goes
  through `std::path::absolute` before it's compared with or hashed against buffer paths.
