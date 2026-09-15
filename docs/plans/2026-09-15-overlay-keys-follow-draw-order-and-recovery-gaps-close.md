---
title: Overlay keys act on the page that's drawn, and the gaps left by the data-loss work are closed
date: 2026-09-15
status: in-progress
---

## Context

The data-loss cycle (`2026-09-15-writes-cannot-lose-work-and-clippy-gates-ci.md`) and the `:health`
install cycle closed with six things left open. The user asked for all of them to be done.

1. **Overlay keys hit a hidden page.** `:health` opened over `:messages`, `:registers` or test
   results is drawn on top, but `q` / `Esc` / `j` / `k` / `g` / `G` act on the page beneath it:
   the `scroll` / `dismiss` closures and the `g` / `G` arms in `src/app/input.rs:395-506` test
   test-results → messages → list → health, while `render::draw` (`src/render.rs:87-97`) paints
   install → health → messages → list → test-results. The mouse-wheel handler
   (`input.rs:1149-1184`) checks health before install, also against draw order.
2. **`write_atomic`'s failed-rename fallback has no test** (`src/paths.rs:232-239`). No real
   filesystem condition on macOS makes a rename fail while an in-place write succeeds, without a
   second user or a mount.
3. **The second-instance check is Unix-only.** `recover::process_alive` is `kill -0` on Unix and
   `false` elsewhere, so on Windows a running binvim's live dump is taken for a crash's.
4. **Undo history is readable by other users**, and `~/.cache/binvim/undo/` is never cleaned
   (401 files on the user's machine). Undo files hold full file text, like recovery files.
5. **The panic path was never triggered**, and a SIGTERM / SIGHUP exit doesn't save the session.
6. **Two rules offered for `CLAUDE.md`** were never added: overlay flag order, and `open_buffer`
   being safe for `:S` / LSP edits.

Decisions:

- **One source of truth for overlay precedence** (made here). An `OverlayPage` enum and
  `App::top_overlay()` return the page `draw` paints; `draw`, the key block and the mouse wheel
  all read it. The scroll / dismiss / top / bottom logic moves out of closures into `App` methods
  that take the page, which makes them unit-testable — the closures in `handle_event` can't be
  reached from a test. Four callers and a proven ordering bug pass ARCHITECTURE's extraction test.
- **The rename failure is tested through a seam** (made here): `write_atomic` becomes a thin call
  into a private `write_atomic_with(path, bytes, rename)` taking the rename as a `fn`, and the
  test passes one that fails. Nothing public changes.
- **Windows uses `tasklist`, no new dependency** (made here), matching the `kill -0` shell-out:
  `tasklist /FI "PID eq <n>" /FO CSV /NH`, with the pid matched in the CSV's second field rather
  than by the "No tasks" text, which Windows localises. The parser is a pure fn tested on every
  platform; the Windows call itself is only compiled in CI, and a manual check goes into
  `WINDOWS.md`'s checklist.
- **Undo files not modified in 90 days are pruned at startup, off-thread** (chosen by the user).
  A constant, not a config key. History is written on `:w`, so a file not saved in 90 days loses
  its undo history. The undo directory is narrowed to `0700` on every save, as `recover::write_to`
  does.
- **The session is saved on SIGTERM / SIGHUP, not on a panic** (made here). A panic can be caused
  by a file's content (a tree-sitter or render bug on open); saving the session then would reopen
  that file on the next bare `binvim` and crash again. A signal isn't caused by content. The
  signal thread can't build a session — it only holds shared state — so a `Session` snapshot is
  refreshed on the 4-second recovery interval and saved from the thread. It can be up to 4 seconds
  stale (a buffer opened just before the hang-up is missing from it; its recovery file still
  applies when it's opened). Until the first refresh the snapshot is `None` and nothing is saved,
  so an early signal can't clear an existing session.
- **The panic path is checked with a throwaway panic** (made here): a temporary `panic!` behind a
  keypress in a local build, exercised in tmux, then reverted before committing. No trigger ships.

## Relevant lore

- [overlay-flags-stack-and-draw-order-decides-what-shows](../solutions/ui/overlay-flags-stack-and-draw-order-decides-what-shows.md): key and dismiss handlers test flags in `render::draw`'s order, never "the other flags are clear"; manual checks open one overlay from another's `:` prompt. Governs tasks 1–2 and the first `CLAUDE.md` rule in task 8.
- [replacing-a-file-by-rename-changes-more-than-its-contents](../solutions/data/replacing-a-file-by-rename-changes-more-than-its-contents.md): every in-place fallback in `write_atomic` stays; cache files holding user text live in a `0700` directory modelled on `recover::write_to`. Governs tasks 3 and 5.
- [hung-up-tty-leaves-crossterm-poll-spinning](../solutions/runtime/hung-up-tty-leaves-crossterm-poll-spinning.md): signal work runs on the signal thread against shared state, never a flag the loop checks; a signal handler is checked by delivering the real signal (tmux `kill-session` for SIGHUP) and polling for the UI rather than sleeping. Governs task 7.
- [open-buffer-runs-for-batch-edits-nobody-sees](../solutions/runtime/open-buffer-runs-for-batch-edits-nobody-sees.md): its Prevention section is the source of the second `CLAUDE.md` rule in task 8.

## Acceptance criteria

- With `:registers` open, `:health` then `q` closes the dashboard and leaves the registers page
  showing; a second `q` closes that. `j` / `G` scroll the dashboard, not the page beneath.
- `render::draw`, the overlay key block and the mouse-wheel handler all branch on
  `App::top_overlay()`; no `show_*_page` precedence chain remains in them.
- A unit test fails a rename inside `write_atomic` and sees the target hold the new bytes, its
  inode unchanged, and no `.tmp` left behind.
- `recover::process_alive` has a `#[cfg(windows)]` implementation, CI's Windows jobs build and lint
  it, and a test covers the `tasklist` CSV parser for a listed pid, an unlisted one and the
  localised no-match line.
- After a `:w`, `~/.cache/binvim/undo` is mode `0700`. A launch removes undo files whose mtime is
  more than 90 days old and leaves newer ones.
- `tmux kill-session` on a binvim with two open files, one dirty, leaves a session file that a bare
  `binvim` restores with both files, and the dirty one's recovered text applied.
- A deliberate panic in a local build leaves a recovery file for a dirty buffer, which the next
  launch applies. The panic is not in any commit.
- `CLAUDE.md` "Conventions to preserve" carries both rules.
- `cargo test -- --test-threads=1`, `cargo +1.98.0 clippy --locked --all-targets -- -D warnings`
  and `cargo fmt --check` pass.

## Tasks

- [x] **`OverlayPage` + `top_overlay()`, used by `draw`.** Add the enum and `App::top_overlay()` in
  `src/app/state.rs` in draw's order (install, health, messages, list, test-results); `render::draw`
  matches on it. Verify: unit test in `app/state.rs` setting flags in pairs (health + messages,
  install + health, list + test-results) and asserting the page returned; `cargo test state::tests`.
- [x] **Overlay keys and the mouse wheel follow `top_overlay()`.** Replace the `scroll` / `dismiss`
  closures and the `g` / `G` branches with `App` methods taking an `OverlayPage`
  (`overlay_scroll_by`, `overlay_dismiss`, `overlay_scroll_to_top`, `overlay_scroll_to_bottom`,
  beside the existing `*_scroll_by` fns); the key block runs when the top page is one of health /
  messages / list / test-results; the mouse wheel matches on `top_overlay()` (install included).
  Keep the `i` arm keyed on health being the top page. Verify: unit tests with health + list set —
  `overlay_dismiss(top)` clears health only, `overlay_scroll_to_bottom(top)` moves `health_scroll`
  and leaves `list_scroll`; then by hand in tmux against a release build: `:registers` → `:health`
  → `G`, `q` (registers still showing), `q` (editor); same from `:messages`.
  Deviation: `ExCommand::Quit` carried the same flag chain (in draw's order already), so `:q` on an
  overlay goes through `top_overlay()` / `overlay_dismiss` too, leaving no chain outside `top_overlay`.
- [x] **Test the failed-rename fallback.** Split `write_atomic` into a private
  `write_atomic_with(path, bytes, rename: fn(&Path, &Path) -> io::Result<()>)`; `write_atomic`
  passes `std::fs::rename`. Every existing fallback stays as it is. Verify: a `#[cfg(unix)]` test in
  `paths::tests` whose rename returns an error — target holds the new bytes, same inode
  (`MetadataExt::ino`), `leftover_temp_files` empty; `cargo test paths::tests`.
- [x] **`process_alive` on Windows.** Replace the `#[cfg(not(unix))]` stub with a `#[cfg(windows)]`
  `tasklist` call and a pure `tasklist_lists_pid(stdout, pid)` compiled under
  `#[cfg(any(windows, test))]`; keep a `#[cfg(not(any(unix, windows)))]` `false` fallback. Add a
  manual check line to `WINDOWS.md`'s checklist (second binvim on a dirty file leaves the first's
  recovery file alone). Verify: parser tests in `recover::tests` (listed pid, pid only as a prefix
  of another, `INFO: No tasks…` line, empty output); `cargo test recover::tests`; the Windows CI jobs
  after push.
- [x] **Undo directory is private.** In `UndoHistory::save_to_path` (`src/undo.rs:346`), after
  `create_dir_all`, set the parent to `0700` under `#[cfg(unix)]`, as `recover::write_to` does.
  Verify: `#[cfg(unix)]` test in `undo::tests` saving into a scratch dir and asserting mode `0o700`;
  `cargo test undo::tests`.
  Deviation: the type is `History`, not `UndoHistory`.
- [x] **Prune undo files older than 90 days at startup.** Factor the `<cache>/binvim/undo` lookup out
  of `cache_path_for` into `undo_dir()` (still `None` under test); add
  `prune_older_than(dir, max_age, now) -> usize` removing regular files whose mtime is before
  `now - max_age`, and a `prune_stale_history()` that runs it on `undo_dir()` with
  `UNDO_MAX_AGE = 90 days`; spawn it once from `App::run` beside `update_spawn_check`
  (`src/app.rs:1194`). Verify: test in `undo::tests` with two scratch files, one back-dated with
  `File::set_modified` — only it is removed; `cargo test undo::tests`.
- [x] **Save the session on SIGTERM / SIGHUP.** Move the save-or-clear branch at `src/app.rs:1557-1567`
  into `session::save_or_clear(&Session)` and call it there. Add
  `App.session_snapshot: Arc<Mutex<Option<Session>>>`, refreshed under `#[cfg(unix)]` from
  `recover_if_due` via `build_session`; `spawn_signal_recovery` saves it after the recovery dumps
  when it is `Some`. The panic and run-error arms in `main.rs` stay recovery-only. Verify:
  `cargo test session::tests`; then in tmux against a release build, per the lore: open two files,
  dirty one, wait 5 s, `tmux kill-session`; relaunch bare `binvim` in the same cwd, poll for the UI —
  both buffers restored, the dirty one reporting recovered changes. Repeat with `kill -TERM`.
  Deviation: `session.rs` has no `tests` module and `session_path` is `None` under test, so there was
  nothing for `cargo test session::tests` to run; the tmux runs are the check. The relaunch parks the
  restored tabs behind the start page, which hides the recovered-changes notice, so "recovered" was
  confirmed by the dirty marker, the buffer's text and `u` returning the disk text. The quit path also
  empties the session snapshot under its lock, so a signal mid-quit can't save an older session over it.
- [x] **Trigger the panic path once.** Temporarily add a `panic!` to a normal-mode key in a local
  build (not committed); in tmux open a file, dirty it, wait 5 s, press the key; confirm the crash
  log path is printed, a recovery file exists, and relaunching on the file reports recovered
  changes. Revert with `git checkout -- <file>` and confirm `git status` is clean. Record the result
  under this task; no commit beyond the plan tick.
  Result (2026-09-15, release build, F12 → `panic!` at the top of the key handler): with `d.txt`
  dirtied and F12 pressed 0.3 s later, inside the 4 s interval so only `main.rs`'s panic arm could
  write, the shell printed `binvim crashed — log: ~/.cache/binvim/crash/1789506915.log` and exit 101;
  the recovery file held `delta FRESH` with `saved_at` 1789506915 and the dead pid. `binvim d.txt`
  showed `delta FRESH`, dirty, with "recovered unsaved changes from 0s ago", and `u` gave `delta`.
  `git checkout -- src/app/input.rs` left `git status` clean.
- [x] **`CLAUDE.md` rules.** Under "Conventions to preserve", add: overlay page flags can be set
  together, and every handler reads `App::top_overlay()` rather than testing flags itself; and
  anything added to `open_buffer` / `switch_to` that changes buffer text, dirty state or disk must be
  safe for `:S` and `apply_concrete_edits`, which open, edit and save with nobody looking. Also
  update the `recover_glue` description (session saved on signal) and the `undo.rs` line (0700,
  90-day prune). Update the overlay lore doc's closing note that the ordering was left as-is.
  Verify: read the section back; `scripts/check-ai-attribution.sh`.
- [x] **Docs.** CHANGELOG Unreleased entries for the overlay fix, Windows second-instance check,
  undo privacy and pruning, and session-on-signal; README wherever recovery or undo persistence is
  described. Verify: `grep -n "undo" README.md` read back against the new behaviour.

## Files

- `src/app/state.rs`: `OverlayPage`, `App::top_overlay()`, and the four `overlay_*` methods over the
  existing `health_scroll_by` (`app/health.rs:197`), `list_scroll_by` (`app/registers.rs:424`),
  `messages_scroll_by` (`app/lsp_glue.rs:1923`), `test_results_scroll_by` (`app/test_glue.rs:370`)
  and the `*_max_scroll` fns.
- `src/render.rs`: `draw`'s overlay chain (`:87-97`) becomes a match on `top_overlay()`.
- `src/app/input.rs`: the overlay key block (`:395-516`) and the mouse wheel (`:1149-1184`).
- `src/paths.rs`: `write_atomic` → `write_atomic_with`; test beside
  `write_atomic_creates_and_replaces_a_file`, reusing `scratch` and `leftover_temp_files`.
- `src/recover.rs`: `process_alive` for Windows and `tasklist_lists_pid`.
- `src/undo.rs`: `save_to_path` permissions, `undo_dir`, `prune_older_than`, `prune_stale_history`.
- `src/app.rs`: prune spawn beside `update_spawn_check`; `session_snapshot` field; quit path calls
  `session::save_or_clear`.
- `src/app/recover_glue.rs`: snapshot refresh in `recover_if_due`; session save in
  `spawn_signal_recovery`.
- `src/session.rs`: `save_or_clear`.
- `CLAUDE.md`, `CHANGELOG.md`, `README.md`, `WINDOWS.md`,
  `docs/solutions/ui/overlay-flags-stack-and-draw-order-decides-what-shows.md`.

## Verification

1. `cargo fmt --check`, `cargo +1.98.0 clippy --locked --all-targets -- -D warnings`,
   `cargo test -- --test-threads=1`.
2. `cargo build --release`, then in tmux against `target/release/binvim`, polling for each screen:
   - `:registers` → `:health` → `j`, `G`, `q`: registers page remains; `q` again returns to the
     editor. Repeat from `:messages`. Mouse wheel on the stacked pages scrolls the one shown.
   - Save a file, `stat -f %Lp ~/.cache/binvim/undo` prints `700`.
   - Back-date a copy of an undo file with `touch -t` to over 90 days ago, launch binvim, and see it
     gone while current files remain.
   - Two files open, one dirty, 5 s, `tmux kill-session`; bare `binvim` restores both with the
     recovered text. Same with `kill -TERM`.
3. Push and watch CI until every job is green, including the Windows build and clippy.
