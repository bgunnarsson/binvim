---
title: The data-loss audit is written down, path by path, and its untested gaps are tested
date: 2026-09-23
status: draft
---

## Context

0.7's first bar is "Crash/data-loss audit clean" (`ROADMAP.md`, Horizon 2 and the milestone
table). Three finished plans already built most of it:

- `2026-09-15-writes-cannot-lose-work-and-clippy-gates-ci.md`: `paths::write_atomic`, `:w`
  conflict detection, the empty-formatter guard, recovery files, and the panic / SIGTERM / SIGHUP
  paths.
- `2026-09-15-overlay-keys-follow-draw-order-and-recovery-gaps-close.md`: the failed-rename
  fallback, Windows `process_alive`, the undo directory at `0700` with pruning, and the session
  saved on SIGTERM / SIGHUP.
- `2026-09-22-whole-repo-audit-fixes.md`: LSP child-death detection and quit-time teardown.

The audit itself was never written down. Each plan's Context served as its own checklist, so
nobody can say "clean" and point at the evidence. A few paths also run with no test behind them:

- **Undo history.** `undo::History::load_from_path` (`src/undo.rs:364`) turns a corrupt or
  truncated file into "no history" through `serde_json::from_slice(..).ok()?`. It isn't tested,
  as recovery's `a_truncated_recovery_file_loads_as_none` (`src/recover.rs:179`) is.
- **Sessions.** `session.rs` has no tests at all. `hydrate_from_session` (`src/app/buffers.rs:883`)
  drops a path that no longer exists, and `load_for_cwd` (`src/session.rs:223`) guards against a
  cwd mismatch. Neither is checked.
- **A file deleted while open.** What happens to a clean or a dirty buffer when the watcher sees
  the file go? It's unverified.
- **`kill -9`.** Recovery from the last four-second dump (only a panic and SIGHUP were tried) is
  unverified.
- **Disk full.** `write_atomic` returns `Err` and leaves the target untouched when the temp write
  or `sync_all` fails. That's reasoned in the writes plan but has no test.

Decisions:

- **The audit is `docs/data-loss-audit.md`, one row per path.** A path is a way text can reach
  disk or be lost: `:w` / `:w!` / `:wa` / `:x`, `:w <file>`, format-on-save, the `:S` and LSP
  batch edits, the watcher reload, `:e!`, git hunk reset, recovery write / apply / removal, the
  undo save / load, the session save / restore, a panic, SIGTERM, SIGHUP, `kill -9`, disk full,
  and a file deleted or replaced underneath. Each row gives the guarantee, the code that keeps it,
  and the evidence: a test name, a manual check in this plan, or "reasoned, not tested" with the
  reason. `ROADMAP.md` links to it.
- **Disk full stays "reasoned, not tested."** Simulating `ENOSPC` portably needs a size-capped
  filesystem, and the failure leaves the target untouched by construction (temp file first). The
  row says so.
- **A clean buffer whose file is deleted stays open and unchanged.** It's marked as not on disk,
  and `:w` recreates the file. A dirty buffer keeps its text and its recovery dump. If the check
  in task 3 finds otherwise, that's a bug to fix in this plan. It isn't a behaviour to document.

## Relevant lore

- [Replacing a file by rename changes more than its contents](../solutions/data/replacing-a-file-by-rename-changes-more-than-its-contents.md):
  a change to `write_atomic` keeps every in-place fallback. A new write path is tested for a
  read-only target, owner, mode, symlink and hard links, not only contents. Applies to any fix
  task 3 turns up.
- [A buffer's disk fields are set in three places](../solutions/runtime/a-buffers-disk-fields-are-set-in-three-places-and-the-reload-is-outside-buffer-rs.md):
  a field describing the file on disk is set in `Buffer::from_path`, `Buffer::save` and
  `reload_buffer_from_disk_inner`. A "not on disk" flag, if task 3 needs one, goes in all three.
  The manual checks include a reload while the file is open.
- [A recorded pid doesn't identify a process](../solutions/runtime/a-recorded-pid-does-not-identify-a-process.md):
  the `kill -9` row relies on `RecoveryFile.pid` liveness, which also checks identity. The row
  cites that test.
- [A hung-up tty leaves crossterm's poll spinning](../solutions/runtime/hung-up-tty-leaves-crossterm-poll-spinning.md):
  signal work runs on the signal thread. SIGHUP is checked by really delivering it (`tmux
  kill-session`), and that leaves recovery and session files the next check must start clear of.
- [open_buffer runs for batch edits nobody sees](../solutions/runtime/open-buffer-runs-for-batch-edits-nobody-sees.md):
  the `:S` / LSP batch-edit rows check `pending_recovery` before editing.
- [tmux new-session runs under the server's environment](../solutions/tooling/tmux-new-session-runs-under-the-servers-environment-not-the-scripts.md)
  and [tmux send-keys Escape then a key arrives as Alt](../solutions/tooling/tmux-send-keys-escape-then-a-key-arrives-as-alt.md):
  the manual checks set `HOME` / `XDG_*` in the pane's own command. They confirm the scratch cache
  got the files, and send `Escape` in its own `send-keys` call.

## Acceptance criteria

- `docs/data-loss-audit.md` exists, has one row per path listed under Decisions, and every row
  names its evidence. `ROADMAP.md` links it from Horizon 2.
- `undo::tests` has a truncated-file and a garbage-file case: both load as `None` and neither
  panics.
- `session::tests` covers a save/load round trip, a garbage file loading as `None`, and a session
  whose recorded cwd differs being refused. An `App`-side test (or a documented tmux check, if
  `App` can't be built in a test) shows a session naming a deleted file restores the others.
- Deleting a clean open file and then running `:w` recreates it with the buffer's text. Deleting a
  dirty open file loses no text: the buffer keeps it, and the recovery dump survives.
- After `kill -9` on a binvim with a buffer dirty for more than 5 s, relaunching and opening the
  file offers the recovered text.

## Tasks

- [ ] Add `undo::tests` for a truncated and a garbage undo file, beside the recovery test's shape.
  Verify with `cargo test undo::tests`.
- [ ] Add `session::tests`: round trip, garbage file, cwd mismatch. Session paths are `None` under
  `cfg!(test)`, so test the parse/serialize functions on a scratch path, or add a `_from(path)`
  seam as `recover::load_from` does. Add the deleted-file restore case if `App` can be built in a
  test, and otherwise note it for task 3. Verify with `cargo test session::tests`.
- [ ] Run the manual checks in tmux with an isolated `HOME` / `XDG_CACHE_HOME`:
  1. Delete a clean open file, then `:w`.
  2. Delete a dirty open file, wait for the watcher, confirm the buffer's text, then quit and
     reopen from recovery.
  3. `kill -9` a binvim dirty for more than 5 s, then relaunch.
  4. A session naming a deleted file.
  5. An external rewrite while the buffer is dirty, then `:w` (the conflict prompt).

  Fix anything that loses text, as its own commit with a test where one can be written. Verify by
  recording each check's outcome in the audit doc.
- [ ] Write `docs/data-loss-audit.md` with every row and its evidence, and link it from
  `ROADMAP.md`. Verify each cited test name exists (`cargo test <name> -- --list`).

## Files

- `src/undo.rs` (`load_from_path` 364, `mod tests`), `src/session.rs` (`load_for_cwd` 223,
  `save`), and `src/app/buffers.rs` (`hydrate_from_session` 883, the watcher reload around 357).
  The model is `src/recover.rs`'s `load_from` / `write_to` seams and its tests.
- `docs/data-loss-audit.md` (new), `ROADMAP.md`, and `CHANGELOG.md` if task 3 fixes a behaviour.

## Verification

- `cargo test -- --test-threads=1` and pinned clippy green locally and in CI.
- The five tmux checks from task 3, run against a fresh `cargo build --release`, each recorded in
  the audit doc. Confirm the scratch cache directory received files before trusting a check.
- Read the finished audit doc end to end. Every row has evidence, and none says "tested" without
  a test name or a check in this plan.
