---
title: A write, a crash or a changed file can no longer lose work, and clippy warnings fail CI
date: 2026-09-15
status: done
---

## Context

ROADMAP.md Horizon 2 (0.7) sets the 1.0 promise as "this won't lose your work or fall over", and
names two of its workstreams: the data-loss and crash audit, and turning clippy into a hard gate.
The user picked both for this cycle.

An audit of the code (read, not assumed) found these paths that lose work today:

- **`:w` truncates before it writes.** `Buffer::save` (`src/buffer.rs:228`) does `File::create`
  then streams the rope. A write that fails partway (disk full, a revoked mount) leaves the file
  holding neither version. `undo.rs:346` already writes temp + `sync_all` + rename; nothing else
  does. `session::save` (`src/session.rs:184`) and the pip manifest rewrite (`src/package.rs:488`)
  are plain `std::fs::write`.
- **A file changed on disk under a dirty buffer is overwritten silently.** The watcher
  (`maybe_reload_from_disk`, `src/app/buffers.rs:301`) skips dirty buffers, and `Buffer::save`
  never compares `disk_mtime` with the file before writing. A `git pull` or another editor's
  write is lost on the next `:w`.
- **`:w other` clobbers an existing file** with no `E13`, and `:w!` does not parse at all
  (`src/command.rs:795`), so no override exists for any refusal.
- **Non-UTF-8 files are mangled on open and the damage is written back.** `from_path` runs
  `String::from_utf8_lossy` (`src/buffer.rs:194`) with no warning; `:w` then replaces every
  invalid byte with U+FFFD on disk.
- **A formatter that exits 0 with no output blanks the buffer and the file.** `save_active`
  (`src/app/save.rs:52`) takes any `Ok(formatted) if formatted != source`.
- **A crash, `kill` or closed terminal loses every unsaved edit.** The panic hook
  (`src/crash.rs`) deliberately touches no editor state; there is no SIGTERM/SIGHUP handling and
  no swap or recovery file of any kind.

The quit paths (`E37`, `E162`, `E89`, `:bda`, `:only`) were audited and are sound; they are not
touched. Clippy on macOS reports 12 warnings (`cargo clippy --locked --all-targets`); CI runs it
on ubuntu, macos and windows with `-A warnings`.

Decisions made while planning (by the planner, from the code and project rules):

- **One atomic-write helper, `paths::write_atomic`.** Temp file beside the target, `sync_all`,
  rename. It writes through a symlink to its target, copies the target's permissions onto the
  temp file, and falls back to today's in-place write when the rename route would break the file:
  more than one hard link (unix), or a directory it can't create the temp file in. `Buffer::save`,
  `session::save`, the pip manifest write and `History::save_to_path` all use it, so `undo.rs`
  stops being a second copy. Ownership isn't preserved: that needs `chown`, and the in-place
  fallback already covers a file the user can write but not replace.
- **Conflicts and lossy files refuse `:w` and take `:w!`**, Vim's model. `Buffer` records the
  file's length beside `disk_mtime` at load and save; a differing mtime or length is a conflict,
  which catches a same-second rewrite on a coarse-mtime filesystem. A file deleted underneath is
  written again without complaint. `:w!` and `:w! path` are new `ExCommand`s; `:wq!`/`:x!` are not
  added — `:w!` then `:q` covers it.
- **The formatter guard lives in `format_buffer`**, the one function both `:format` and
  format-on-save call: empty or whitespace-only output for a source that isn't is an error, and
  the buffer is left alone.
- **Recovery files are applied on open, not offered in a prompt.** Every 4 seconds (Vim's
  `updatetime`) each dirty buffer with a path whose version moved since its last dump is written
  to `<cache>/binvim/recover/<key>.json`. On open, a recovery file whose text differs from the
  file is loaded into the buffer as unsaved changes — the file's own text recorded as an undo
  step first, so `u` shows what's on disk — with a status line saying so. Nothing reaches the
  file until `:w`; `:e!` discards. A picker was rejected: session restore opens several buffers
  at once, and a prompt per buffer is worse than an undo step. Path-less `[No Name]` buffers are
  not covered.
- **A recovery file is removed** when its buffer is written, reverted with `:e!`, deleted with
  `:bd!`, or goes clean by undo, and for every open buffer on a clean quit (`:q!` is a deliberate
  discard). It is kept on a panic, an error out of `run`, or a signal.
- **Keys are a stable hash.** `undo::cache_path_for` uses `DefaultHasher`, whose output may change
  between Rust releases; recovery keys use the FNV-1a hash `session.rs` already has, moved to
  `paths::path_key` so both share it. Undo keying is left as it is: changing it orphans every
  user's existing history.
- **Panics and run errors dump from `main`.** `main` owns `App`, so it wraps `app.run()` in
  `catch_unwind` and writes every dirty buffer's recovery file on a panic or an `Err` (a SIGHUP'd
  terminal makes the next render fail with EIO), then re-raises or returns. The panic hook stays
  state-free. Background-thread panics don't unwind the main thread and remain covered only by the
  4-second dumps.
- **SIGTERM and SIGHUP set a flag (unix only).** `signal-hook` 0.3 is already in `Cargo.lock`
  through crossterm; it becomes a direct `cfg(unix)` dependency. The run loop checks the flag after
  each poll, dumps recovery files, saves the session, and exits. Windows console-close events are
  not handled; the periodic dump bounds the loss there.
- **Every new persisted path returns `None` under `cfg!(test)`**, per the project memory on tests
  and host state. `undo::cache_path_for` isn't gated today, so `:w` in tests writes undo files into
  the real cache; it gets the same gate.
- **The clippy gate is fixed forward in CI.** Only macOS can be linted here. CI flips to
  `-D warnings`; any ubuntu- or windows-only warning that appears is fixed in a follow-up commit
  until all three clippy jobs pass.

## Relevant lore

None found. `docs/solutions` holds a config-reload doc and an overlay-flags doc; neither covers
writes, recovery or clippy.

## Acceptance criteria

- `cargo clippy --locked --all-targets -- -D warnings` passes locally, CI's clippy step runs with
  `-D warnings`, and all three clippy jobs are green.
- `:w` on a symlinked file leaves the symlink in place with the target updated; a `0600` file is
  still `0600` after `:w`; a write into a directory where the temp file can't be created still
  succeeds in place.
- A dirty buffer whose file was rewritten on disk refuses `:w` with a message naming `:w!`; `:w!`
  writes it.
- `:w existing.txt` from another buffer refuses with `E13`; `:w! existing.txt` writes it.
- Opening a file with invalid UTF-8 says so in the status line, and `:w` refuses until `:w!`.
- A formatter returning empty output for a non-empty buffer leaves the buffer unchanged and
  reports the failure.
- After editing without saving, `kill -9` more than 4 seconds later, `kill -TERM`, or closing the
  terminal (tmux `kill-session`), relaunching on the file shows the unsaved text, dirty, with a
  status line naming recovery; `u` shows the file's text; `:e!` discards and removes the recovery
  file; `:w` writes and removes it.
- A clean `:q` or `:q!` leaves no recovery file for any open buffer.
- The session file and the pip manifest are written through `write_atomic`; tests write nothing
  under the real cache (undo included).
- README documents `:w!`, write conflicts, non-UTF-8 files and recovery; CHANGELOG `[Unreleased]`
  has entries.

## Tasks

- [x] **Clippy clean, then gated.** Fix the 12 warnings: `lang.rs:853` (`if .. else`),
  `picker.rs:304` and `:348` and `markdown_render.rs:1670` and `install.rs:1296`/`:1316`
  (iterate instead of indexing), `lsp/io.rs:365-366` and `lsp/parse.rs:848-849` (`Value::from`),
  and move the items after the test module in `app/lsp_glue.rs:2491` and `lsp/io.rs:319` above it.
  In `.github/workflows/ci.yml` replace `-A warnings` with `-D warnings` and rewrite the comment
  above it. Verify: `cargo clippy --locked --all-targets -- -D warnings` exits 0;
  `cargo test -- --test-threads=1`; push and `gh run watch` the ci run. If an ubuntu or windows
  clippy job fails, add a task below this one for its warnings and fix it next.
  Deviation: CI's floating `stable` was clippy 1.98, local was 1.95, so all three OS jobs failed on
  two `question_mark` lints 1.95 doesn't have (`task_glue.rs:364`, `lsp/manager.rs:1091`) plus
  `SCSS_QUERY_OVERLAY` dead on MSVC. Fixed, and the clippy job is pinned to 1.98.0 — with warnings
  denied, a floating toolchain fails main whenever a release adds a lint. Local lint runs use
  `cargo +1.98.0 clippy`.
- [x] **`paths::write_atomic`.** Add `pub fn write_atomic(path: &Path, bytes: &[u8]) ->
  io::Result<()>` to `src/paths.rs`: resolve a symlink with `canonicalize` when
  `symlink_metadata` says it is one; temp file `.<name>.binvim-<pid>.tmp` in the target's
  directory; copy the existing target's permissions onto it; write, `sync_all`, rename; remove
  the temp file on any error. Fall back to truncate-and-write when the temp file can't be created
  or (`cfg(unix)`, `MetadataExt::nlink`) the target has more than one link. `Buffer::save` builds
  its bytes with `write_rope_with_eol` into a `Vec` and calls it. Verify with `paths::tests`
  (temp dirs under `std::env::temp_dir()`): a new file is written; an existing `0600` file keeps
  its mode (unix); writing through a symlink updates the target and leaves the link a link
  (unix); a hard-linked file is written in place so both names see the new text (unix); a
  read-only directory holding a writable file still gets the write (unix, skipped when running as
  root); no `.tmp` file is left behind. Plus `buffer::tests` round-trips still pass.
  `cargo test paths::tests buffer::tests`.
  Deviation: only a `PermissionDenied` from creating the temp file falls back to writing in place;
  any other failure (a full disk) returns the error with the file untouched, since an in-place
  retry would truncate it. A dangling symlink is written through with `std::fs::write`.
- [x] **Every other writer shares it.** `session::save`, the `PackageEcosystem::Pip` manifest
  write in `src/package.rs`, and `History::save_to_path` in `src/undo.rs` call `write_atomic`.
  Gate `undo::cache_path_for` with `if cfg!(test) { return None; }` and a why-comment matching
  `session_path`'s. Move `session.rs`'s `hash_path` to `pub fn path_key(&Path) -> String` in
  `src/paths.rs`, called by `session_path` (same output, so existing sessions still resolve).
  Verify: `cargo test undo::tests session paths::tests package::tests`, and `path_key` gives the
  same string `hash_path` did for a fixed path (a test pinning one known value).
- [x] **Buffers know when they're lossy or stale.** Add `pub lossy: bool` (set when
  `std::str::from_utf8(&bytes)` fails in `from_path`) and `pub disk_len: Option<u64>` (set beside
  `disk_mtime` in `from_path`, `save`, and `reload_buffer_from_disk_inner`) to `Buffer`, and
  `pub fn changed_on_disk(&self) -> bool`: false with no path, no recorded mtime, or no file on
  disk; true when the file's mtime or length differs from what was recorded. `open_buffer` and
  `App::run` (the CLI buffer) set the status line to `"<name>" is not valid UTF-8 — invalid bytes
  show as �, and :w! writes them that way` when `lossy`. Verify with `buffer::tests`: invalid
  bytes set `lossy`, valid ones don't; `changed_on_disk` is false after load, true after the file
  is rewritten with different length, false after `save`, false after the file is deleted.
  `cargo test buffer::tests`.
- [x] **`:w` refuses what would lose work; `:w!` overrides.** `src/command.rs`: `w!`/`write!`
  parse to `ExCommand::WriteForce`, with a path to `WriteAsForce(String)`; tests beside the
  existing `parse("w")` ones. `save_active(&mut self, force: bool)`: unless `force`, bail before
  formatting with `file changed on disk since it was read (add ! to overwrite)` when
  `changed_on_disk()`, and `"<name>" is not valid UTF-8 — writing would replace its invalid bytes
  (add ! to write anyway)` when `lossy`; a forced write clears `lossy`. Existing callers
  (`save_all`, `WriteQuit`, `WriteQuitIfModified`, the project-wide `:S` substitute loop in
  `input.rs:4223`) pass `false`; `write_and_report(force)`. `WriteAs`: unless forced, a target
  that exists and isn't the buffer's own path bails `E13: File exists (add ! to override)` before
  the path is changed; on any path change set `disk_mtime`/`disk_len` to `None` so the new target
  isn't judged against the old file. Verify with `app::input::tests`, following
  `wa_writes_every_modified_buffer_and_stays_put`'s temp-file setup: rewriting the file behind a
  dirty buffer makes `:w` leave the file and set a status containing `:w!`, and `:w!` writes it;
  `:w <existing>` leaves both files and says `E13`, `:w! <existing>` writes; a lossy buffer's
  `:w` refuses and `:w!` writes. `cargo test app::input::tests command::tests`.
  Deviation: `Buffer::save` clears `lossy` (rather than `save_active`), so every successful write
  does; `:w! <Tab>` completes paths like `:w <Tab>`. The refusal texts end `(:w! overwrites it)` /
  `(:w! writes it anyway)` so the override is named in the message.
- [x] **Empty formatter output is an error.** In `format_buffer` (`src/format.rs`), after the
  dispatch, pass the result through `fn reject_empty(source: &str, formatted: String) ->
  Result<String, String>`: `Err("formatter returned no output; buffer left unchanged")` when
  `formatted.trim()` is empty and `source.trim()` isn't. Verify with `format::tests`: empty output
  for non-empty source is `Err`; empty for empty is `Ok`; ordinary output passes through.
  `cargo test format::tests`.
- [x] **Recovery files.** New flat `src/recover.rs` (declared in `main.rs`):
  `#[derive(Serialize, Deserialize)] pub struct RecoveryFile { pub path: String, pub saved_at:
  u64, pub text: String }`; `pub fn recovery_path(file: &Path) -> Option<PathBuf>` —
  `cfg!(test)` → `None`, else `<cache_dir>/recover/<path_key(file)>.json`; `write_to(dest,
  &RecoveryFile)` via `write_atomic`; `load_from(dest) -> Option<RecoveryFile>` (missing or
  malformed → `None`); `pub fn recovered_text<'a>(rec: &'a RecoveryFile, disk: &str) ->
  Option<&'a str>` — `None` when the texts match. Verify with `recover::tests` on temp paths: write
  then load round-trips; a truncated file loads as `None`; `recovered_text` is `None` for
  identical text and `Some` otherwise. `cargo test recover::tests`.
  Deviation: also `now_secs()` for `saved_at`. Its items are unused until the next two tasks, so
  this commit is held back from the push until they land — the clippy gate denies dead code.
- [x] **Dirty buffers are dumped, and cleaned up.** App fields `recovery_written: HashMap<PathBuf,
  u64>` (path → version dumped) and `recovery_checked_at: Instant`; `const RECOVERY_INTERVAL:
  Duration = 4s` in `app/state.rs`. New `src/app/recover_glue.rs` (registered in `app.rs`,
  added to CLAUDE.md's app map): `recover_if_due()` — once per interval, for the active buffer and
  every stash with a path: dirty and version unlike the map → write and record; clean and in the
  map → remove file and entry; `write_recovery_now()` — every dirty buffer regardless of interval;
  `discard_recovery(path)`. The run loop calls `recover_if_due` each iteration and caps
  `poll_dur` at the next check, beside the health-page cap. `save_active` calls
  `discard_recovery` after a successful write; `force_reload_from_disk` and a forced
  `delete_buffer` of a dirty buffer call it too. After the loop, a normal quit removes the
  recovery file of every open buffer. Verify by hand (`recovery_path` is `None` in tests), in
  Verification step 3; `cargo build --release`.
  Deviation: `RECOVERY_INTERVAL` lives in `recover_glue.rs`, its only reader, not `state.rs`.
  `delete_buffer` discards the recovery file on every close, not only a forced one — a buffer
  undone back to clean before the next tick still has a dump of its dirty text. A clean quit also
  discards files dumped for buffers already closed (`discard_all_recovery`). Checked in tmux
  against the release build: an unsaved edit left idle 5.5s was dumped with its text, and `:q!`
  removed the file.
- [x] **Recovery is applied on open.** `apply_recovery()` in `recover_glue.rs`, called in
  `open_buffer` after `switch_to` and in `App::run` for the CLI buffer: load the active path's
  recovery file; `recovered_text` against the rope; `None` → remove the file; `Some(text)` →
  `history.record` the current rope, `replace_all(text)`, `dirty = true`, record the version in
  `recovery_written`, status `recovered unsaved changes from <HH:MM> — :w keeps them, :e!
  discards`. Verify by hand in Verification step 3.
  Deviation: the age reads `from 4s ago` through the `time_ago` helper `:undolist` already uses
  (made `pub(super)`) — local `HH:MM` needs a timezone library the project doesn't carry. A path
  already in `recovery_written` is skipped: a restored session applies recovery in `open_buffer`,
  and `App::run`'s second call would otherwise find the text matching and remove the file while the
  buffer was still dirty. Checked in tmux against the release build: after `kill -9` a relaunch
  shows the text dirty with the notice, `u` shows the file's text, `:w` writes and removes the
  recovery file, and `:e!` reverts and removes it.
- [x] **Panics and signals dump before exit.** `main.rs`: run `app.run()` inside
  `std::panic::catch_unwind(AssertUnwindSafe(..))`; on a panic call `app.write_recovery_now()`
  then `resume_unwind`; on `Err` call it then return the error. `Cargo.toml`:
  `[target.'cfg(unix)'.dependencies] signal-hook = "0.3"` with a why-comment. `App::run`
  (`cfg(unix)`) registers SIGTERM and SIGHUP with `signal_hook::flag::register` on an
  `Arc<AtomicBool>` field; the loop breaks when it is set, and after the loop a signalled exit
  calls `write_recovery_now` instead of removing recovery files; the session is saved either way.
  Verify by hand in Verification step 3 (`kill -TERM`, tmux `kill-session`). If a signal isn't
  noticed within a second because the poll doesn't wake, cap `poll_dur` at 500ms and record it
  as a deviation. `cargo build --release`.
  Deviation (approach changed, with the user's agreement): the flag cannot work for SIGHUP. A closed
  terminal leaves crossterm's `poll` spinning at 100% CPU in `read` on the dead tty
  (`UnixInternalEventSource::try_read`, seen with `sample` on a debug build), so the loop never
  checks the flag — and with the default action replaced, the process no longer dies either. Instead
  `spawn_signal_recovery` runs a `signal_hook::iterator::Signals` thread that writes recovery files
  from `recovery_snapshot` (dirty buffers' ropes, refreshed each loop iteration; clones share
  nodes), restores the terminal (`crash::restore_terminal_best_effort`, now `pub`) and exits
  `128 + signal`. The same mutex guards the loop's own dumps so the two never write one temp file
  at once. A signalled exit doesn't save the session — the thread can't reach `App`. Checked in
  tmux against the release build: `kill -TERM` straight after typing exits 143 with the terminal
  back in canonical mode and the text in the recovery file; `tmux kill-session` exits (no spin)
  with the text written; `:q!` still leaves no file. The panic branch was reviewed, not triggered.
- [x] **Docs.** README: a `:w!` row beside `:w` in the ex-command table; a short "Unsaved work"
  paragraph covering write conflicts, non-UTF-8 files, recovery files (where they live, when
  they're applied and removed, `[No Name]` not covered, Windows covered only by the periodic
  dump). CLAUDE.md: `recover.rs` and `app/recover_glue.rs` in the architecture map, and
  `paths::write_atomic` as the way to write a user's file. CHANGELOG `[Unreleased]`: `Added`
  (recovery, `:w!`), `Fixed` (atomic writes, conflicts, `E13`, non-UTF-8, empty formatter output).
  Verify by reading; `cargo fmt --check`.
  Deviation: README gets two feature bullets ("Writes that can't lose work", "Recovery after a
  crash") beside auto-reload rather than one paragraph, plus the layout entries; CLAUDE.md's CI
  line now says clippy is pinned and denies warnings.

## Files

- `.github/workflows/ci.yml`; the clippy sites listed in task 1.
- `src/paths.rs`: `write_atomic`, `path_key`.
- `src/buffer.rs`: `save` via `write_atomic`, `lossy`, `disk_len`, `changed_on_disk`.
- `src/session.rs`, `src/package.rs`, `src/undo.rs`: `write_atomic`; `path_key`; undo path gate.
- `src/command.rs`: `WriteForce`, `WriteAsForce`, modelled on `QuitForce`.
- `src/app/save.rs`: `save_active(force)`, conflict and lossy refusals, recovery discard.
- `src/app/input.rs`: `write_and_report(force)`, `WriteAs` `E13`, new arms; tests.
- `src/app/buffers.rs`: `open_buffer` lossy notice + `apply_recovery`; `save_all` passes `false`;
  `force_reload_from_disk` and `delete_buffer` discard recovery.
- `src/format.rs`: `reject_empty`.
- `src/recover.rs` (new), `src/app/recover_glue.rs` (new), `src/app.rs` (fields, loop hook,
  signal flag, exit handling, module line), `src/app/state.rs` (`RECOVERY_INTERVAL`),
  `src/main.rs` (module line, `catch_unwind`), `Cargo.toml` (`signal-hook`).
- `README.md`, `CLAUDE.md`, `CHANGELOG.md`.

## Verification

1. `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --
   --test-threads=1`; the pushed ci run is green on every job, clippy on all three OSes included.
2. `cargo build --release`. In a scratch directory with `XDG_CACHE_HOME=$SCRATCH/cache`, in tmux:
   - `ln -s real.txt link.txt`, open `link.txt`, edit, `:w` — `link.txt` is still a symlink and
     `real.txt` has the edit. `chmod 600 real.txt`, edit, `:w` — still `-rw-------`.
   - Edit `a.txt` without saving; `echo outside > a.txt` from the shell; `:w` — refused, naming
     `:w!`, and `a.txt` still says `outside`; `:w!` writes the buffer.
   - From `a.txt`, `:w b.txt` where `b.txt` exists — `E13`, `b.txt` unchanged.
   - `printf 'caf\xe9\n' > latin1.txt`, open it — the status line says it isn't valid UTF-8;
     `:w` refuses; `xxd latin1.txt` still shows `e9`.
3. Recovery, same setup:
   - Open `r.txt`, type text, don't save, wait 5 seconds, `kill -9` the binvim pid.
     `ls $SCRATCH/cache/binvim/recover/` shows one file. Relaunch on `r.txt` — the typed text is
     there, the buffer is dirty, the status line names recovery; `u` shows the file's text; redo,
     `:w` — the recovery file is gone.
   - Type again, `kill -TERM` straight away (under 4 seconds) — binvim exits within a second, and
     relaunching recovers the text; `:e!` discards it and removes the file.
   - Type again, `tmux kill-session` — relaunching recovers the text.
   - Type, `:q!` — no recovery file remains.
