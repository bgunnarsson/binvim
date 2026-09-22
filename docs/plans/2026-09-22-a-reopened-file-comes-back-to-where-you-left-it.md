---
title: A reopened file comes back to where you left it, and PR #12 is merged
date: 2026-09-22
status: in-progress
---

## Context

PR #12 (happyTonakai, `83310c8`) adds an nvim-style last-cursor cache: one JSON
file per opened file under `~/.cache/binvim/cursor/`, gated on a content hash,
written when a buffer is left or the editor quits, read when the file is
opened. The feature is wanted. The review (this session, 2026-09-22) found the
implementation has to change before it ships, and the maintainer decided to
merge it as-is and fix it on main rather than send it back.

What the review found, in the order it matters:

1. **A reproducible panic.** `adopt_window` (`src/app/windows.rs`) assigns the
   incoming window to `App.window` and only then calls `switch_to`, whose
   `snapshot_active` stores `self.window.cursor` into the *outgoing* buffer's
   stash. The stash gets another buffer's cursor. Main has always done this,
   but nothing read the value until the PR's `switch_tab` seeded a fresh
   `Window` from it. Reproduced in tmux: a 1000-line file, `900G`, `<C-w>v`
   picking a 5-line file, `<C-w>h`, `:b short` — main is fine, the PR build
   exits 101 in ropey.
2. **Windows clippy will fail after rebase.** `App.cursor_snapshot` and
   `Buffer.cursor_hash_cache` are only read inside `#[cfg(unix)]` code; in a
   bin crate rustc's never-read lint fires and CI denies warnings. The PR's
   Windows checks were already red for an unrelated main failure, which hid
   this.
3. **The cursor is stored twice.** The PR also writes `last_line`/`last_col`
   into the undo history file and widens `save_to_path` / `load_from_path`.
   Every writer of the undo file writes the cursor cache too, so the undo copy
   is never the one read.
4. **Batch flows persist cursors for buffers nobody saw.** `persist_active_cursor`
   fires from `snapshot_active`, which the project-wide `:S` loop, LSP
   `apply_concrete_edits`, `save_all` and `hydrate_from_session` all drive.
   The PR patches `:S` alone, with a `persist_cursor` parameter on
   `save_active` and a replay loop after the substitution. `:wa` after an LSP
   rename still clobbers every touched file; session hydrate hashes and
   rewrites every restored buffer before the first frame.
5. **Two copies of the prune walk.** `cursor_cache::prune_stale` reimplements
   `undo::tidy_history_dir`.
6. **A dirty buffer never persists its cursor**, because the only hash the PR
   has is of the live rope, which never matches disk once the buffer is
   dirty. `:q!` after an edit lands you on the stale position.
7. **Clean quit never clears `cursor_snapshot`**, so a SIGHUP arriving mid-quit
   can overwrite the just-written cursor with an older one. The session code
   beside it clears its snapshot under the lock for exactly this reason.
8. **`delete_buffer` persists twice**: once explicitly, once through
   `switch_tab`'s `snapshot_active`.
9. `persist_active_cursor` copies and hashes the whole rope on every
   `<C-w>` move and buffer switch.

Decisions made in this plan:

- **Merge with a merge commit**, resolving the two conflicting files in it, so
  the contributor's commit stays in history under their name and GitHub marks
  the PR merged. CONTRIBUTING's "no merge commits" line is about how a
  contributor prepares a branch, not how the maintainer lands one. The merge
  message carries no attribution beyond the PR number.
- **Fix the panic at its source**, not the symptom: the buffer switch happens
  while the outgoing window is still live, so a stash never holds another
  buffer's cursor. No clamp is added to `switch_tab`'s seed; the guard would
  defend a failure that no longer exists.
- **The buffer keeps the hash of its clean text.** `Buffer::from_path` and
  `Buffer::save` set `clean_hash`, the hash of what was last read from or
  written to disk. `open_buffer` and `save_active` already hash at exactly
  those moments for the undo file, so the cost moves rather than grows. That
  one field replaces the per-tick `cursor_hash_cache` memo, lets a dirty
  buffer persist its cursor (keyed by the content it will be reopened
  against), and takes the `to_string()` out of every window move.
- **Guard the mechanism, not the callers.** A cursor is persisted only for a
  buffer that has been drawn since it became active (`App.active_shown`,
  cleared in `load_stash` and wherever `App.buffer` is replaced, set after
  `render::draw`). One check in `persist_active_cursor` covers `:S`, LSP
  workspace edits, `:wa`, session hydrate and the CLI-path open. The
  `persist_cursor` parameter and the `:S` replay loop go.
- **The undo file keeps its shape.** Revert the `src/undo.rs` half of the PR
  entirely.
- **Share the tidy, keep the keys apart.** `tidy_history_dir` moves to
  `paths.rs` as a crate-visible helper both caches call. The path *key* is not
  unified: undo files are named by `DefaultHasher`, and switching them to
  `paths::path_key` would orphan every user's undo history in one upgrade.
  That is a separate decision, noted here and left alone.
- **`cursor_cache.rs` stays a flat top-level module.** CONTRIBUTING prefers
  extending an existing module, but this is a distinct persisted artifact with
  its own directory, and after the deduplication it is small. Folding it into
  `undo.rs` would make undo own something that is not undo.
- **One changelog entry**, under a new `## [Unreleased]` / `### Added`. The
  intermediate state never ships, so the fixes get no `### Fixed` lines.
- **The PR comment** is written after the follow-up commits are pushed, so it
  can link them. The maintainer asked for it in this cycle's request.

## Relevant lore

- [open_buffer runs for batch edits nobody sees](../solutions/runtime/open-buffer-runs-for-batch-edits-nobody-sees.md):
  anything hung off `open_buffer` / `switch_to` must stay correct with nobody
  looking, checked explicitly against the `:S` loop and `apply_concrete_edits`.
  This is why task 5 guards `persist_active_cursor` on the buffer having been
  drawn, instead of patching callers.
- [Config reload misses state derived from App.config](../solutions/runtime/config-reload-misses-state-derived-from-app-config.md):
  a cache derived from mutable state is invalidated where the source changes,
  found by grep, not memory. `clean_hash` (task 4) is set in the two places
  the disk text changes, `Buffer::from_path` and `Buffer::save`, and the
  grep for `disk_mtime =` is the list of places to check.
- [A hung-up tty leaves crossterm's poll spinning](../solutions/runtime/hung-up-tty-leaves-crossterm-poll-spinning.md):
  work needed on SIGHUP/SIGTERM runs on the signal thread against a shared
  snapshot, never via a flag the loop checks. Task 6 keeps the cursor write on
  the signal thread and refreshes its snapshot on the recovery interval. The
  tmux verification quits with `:q!`, never `tmux kill-session`.
- [Replacing a file by rename changes more than its contents](../solutions/data/replacing-a-file-by-rename-changes-more-than-its-contents.md):
  a new persisted file goes through `paths::write_atomic`, and its directory
  is narrowed to `0700` at startup as well as at write time. The PR already
  does both; task 7 keeps them through the shared tidy.

## Acceptance criteria

- PR #12 shows as merged on GitHub, with `83310c8` reachable from `main`.
- The tmux sequence from the review (1000-line file, `900G`, `<C-w>v` picking a
  5-line file, `<C-w>h`, `:b short`) leaves the editor running with the short
  file at line 1.
- Open a file, `50G`, `:q`, reopen: the cursor is on line 50. Open it, `10G`,
  edit a line, `:q!`, reopen: the cursor is on line 10.
- Run a project-wide `:S` over three files while looking at a fourth, then
  open one of the three: no cursor cache entry for it matches (it opens at
  the top), and the fourth file's saved position is unchanged.
- `src/undo.rs` is byte-identical to main before the merge.
- `grep -rn cursor_hash_cache src` and `grep -rn persist_cursor src` return
  nothing; `grep -rn 'fn tidy' src/undo.rs` returns nothing.
- `cargo test -- --test-threads=1`, `cargo fmt --check` and
  `cargo +1.98.0 clippy --locked --all-targets -- -D warnings` pass locally,
  and CI is green on all three OSes for the final push.
- The PR carries one comment from the maintainer thanking the contributor and
  listing what was changed after the merge, with commit links.

## Tasks

- [x] **Merge PR #12.** `git merge --no-ff pr-12`. Resolve `src/buffer.rs` by
  keeping main's `..Self::empty()` literals and adding `cursor_hash_cache` to
  `Buffer::empty` (it is deleted again in task 4). Resolve `src/app.rs` by
  keeping both the `create_private_dir` block and the prune spawn at startup,
  and placing `persist_active_cursor()` after `lsp.shutdown_all()` on the quit
  path. Message: `Merge PR #12: a last-cursor cache restores your place on reopen`.
  Verify: `cargo build` and `cargo test -- --test-threads=1` pass on the
  merge commit; `git log --oneline main..pr-12` is empty.
- [x] **Revert the undo-file half.** `git checkout 7e1b1d6 -- src/undo.rs`, then
  drop the `cursor` argument from `save_to_path` in `src/app/save.rs` and the
  tuple destructure in `loaded_buf_state`. Verify: `git diff 7e1b1d6 --
  src/undo.rs` is empty; `cargo test undo::tests`.
- [x] **Switch the buffer before the outgoing window leaves `App.window`.** In
  `adopt_window`, call `switch_to(incoming.buffer_idx)` first, while
  `self.window` is still the window that showed the outgoing buffer, then
  assign `self.window = incoming`; the cache-and-reapply of cursor, viewport
  and anchor becomes unnecessary and goes. `focus_window` and `window_close`
  copy the outgoing window into `self.windows` instead of `mem::take`-ing it,
  so it is still live during the switch. Remove the `window.buffer_idx !=
  self.active` clause from `persist_active_cursor`, and the comments in
  `focus_window` / `window_close` that explain the mismatch. Verify: the tmux
  sequence in the acceptance criteria on a debug build, plus `<C-w>v`,
  `<C-w>h`, `<C-w>l` round-trips keep each pane's cursor.
- [x] **`Buffer.clean_hash` replaces the live-rope hashing.** Add `clean_hash:
  Option<u64>` to `Buffer`, `None` in `empty`, set from `undo::hash_text` of
  the loaded text in `from_path` and of the written rope in `save`.
  `loaded_buf_state` and `save_active` read it instead of hashing.
  `persist_active_cursor` keys on it and drops the `dirty` bail. Delete
  `cursor_hash_cache` and `refresh_cursor_snapshot`'s memo. Verify: a
  `buffer::tests` test that `from_path` sets the hash, an edit leaves it, and
  `save` updates it; `grep -rn 'hash_text(&self.buffer' src/app` returns
  nothing.
- [x] **Persist only what was drawn.** Add `App.active_shown: bool`, false in
  `App::new`, cleared in `load_stash` and where `App.buffer` is replaced
  (`delete_buffer`'s last-buffer branch, `:new` / `:vnew`), set true right
  after `render::draw` in `run`. `persist_active_cursor` returns early when it
  is false. Remove `save_active`'s `persist_cursor` parameter (all five
  callers), the `restored_cursors` replay in the `:S` loop, and the explicit
  `persist_active_cursor` in `delete_buffer`'s `switch_tab` path (keep the
  last-buffer branch). `save_active` calls `persist_active_cursor` instead of
  writing the cache itself. Verify: `grep -rn persist_cursor src` empty; the
  `:S` acceptance sequence in tmux; `cargo test`.
- [ ] **The signal thread's snapshot is refreshed on the interval and cleared
  on quit.** Move the cursor snapshot refresh into `recover_if_due`'s
  `#[cfg(unix)]` block beside the session snapshot, as `(path, clean_hash,
  cursor)` of the active buffer when `active_shown`. Delete the per-tick
  `refresh_cursor_snapshot`. On the quit path, take the snapshot under its
  lock and set it to `None` before `persist_active_cursor`, mirroring
  `session_snapshot`. Verify: `cargo +1.98.0 clippy --locked --all-targets --
  -D warnings`; in tmux, open a file, `30G`, `kill -TERM <pid>` from another
  pane, reopen: cursor on line 30.
- [ ] **One tidy for both caches.** Move `tidy_history_dir` to `src/paths.rs`
  as `pub(crate) fn tidy_private_dir`, keeping its signature and doc, called
  by `undo::prune_stale_history` and `cursor_cache::prune_stale`. Spawn both
  from one thread in `run`. `cursor_cache`'s tests use
  `paths::test_scratch_dir("cursor", …)` instead of a private `scratch`.
  Verify: `cargo test cursor_cache::tests` and `cargo test undo::tests`.
- [ ] **Changelog.** Add `## [Unreleased]` above `## [0.6.5]` with one
  `### Added` entry in the file's style: binvim remembers where you were in
  each file and comes back to it on reopen, unless the file changed on disk
  meanwhile. Verify: `cargo test` (the changelog has no test; read it back).
- [ ] **Push and watch CI.** `git push`, then `gh run watch` the ci run on the
  final commit until all seven jobs pass. Verify: `gh pr view 12 --json state`
  is `MERGED`.
- [ ] **Comment on the PR.** One `gh pr comment 12` thanking the contributor for
  the feature and listing, in plain language, what was changed after the
  merge and why, with a link to each follow-up commit. No attribution lines.
  Verify: `gh pr view 12 --comments` shows it.

## Files

- `src/app/windows.rs`: `adopt_window`, `focus_window`, `window_close` (task 3).
- `src/app/buffers.rs`: `snapshot_active`, `load_stash`, `loaded_buf_state`,
  `open_buffer`, `switch_tab`, `delete_buffer`, `persist_active_cursor`
  (tasks 2 to 5). Reuse the PR's `restored_cursor` as is.
- `src/buffer.rs`: `Buffer::empty`, `from_path`, `save` (task 4).
- `src/app/save.rs`: `save_active` (tasks 2, 4, 5).
- `src/app/input.rs`: the `:S` loop and the `WriteQuit` arms (task 5).
- `src/app/recover_glue.rs`: `recover_if_due`, `spawn_signal_recovery`,
  delete `refresh_cursor_snapshot` (task 6).
- `src/app.rs`: `App` fields, `run`'s loop and quit path (tasks 5, 6, 7).
- `src/undo.rs`: reverted to main (task 2); `tidy_history_dir` removed (task 7).
- `src/paths.rs`: `tidy_private_dir` (task 7); `test_scratch_dir` reused.
- `src/cursor_cache.rs`: `prune_stale`, tests (task 7).
- `CHANGELOG.md` (task 8).

## Verification

Build a debug binary and drive it in tmux with an isolated `HOME` and
`XDG_CACHE_HOME` under the scratchpad, waiting for the mode line before each
key, sending `Escape` in its own `send-keys`, and quitting with `:q!`:

1. The split sequence from the review, on the final build: no panic, short
   file at line 1 after `:b short`.
2. Open `long.txt`, `50G`, `:q`; reopen: line 50. `10G`, `x`, `:q!`; reopen:
   line 10. Edit the file with `sed -i` outside binvim; reopen: line 1.
3. Open four files, `:S old/new/g` matching in three of them while the fourth
   is active; reopen one of the three: line 1; reopen the fourth: its position.
4. `30G`, `kill -TERM` the process from a second pane; reopen: line 30.
5. `cargo test -- --test-threads=1`, `cargo fmt --check`,
   `cargo +1.98.0 clippy --locked --all-targets -- -D warnings`, then push and
   watch CI. `cargo build --release` at the end so the user's alias picks it
   up.
