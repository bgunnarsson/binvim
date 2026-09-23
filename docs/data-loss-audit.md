# Data-loss audit

binvim's 1.0 promise is that it won't lose your work (`ROADMAP.md`, Horizon 2). This is the
audit behind that promise: every way text reaches disk or could be lost, what binvim guarantees
there, the code that keeps the guarantee, and the evidence.

Evidence is one of three things:

- a **test** you can run with `cargo test <name>`
- a **check**, run by hand in tmux against a release build, with its date and its outcome
- **reasoned**: the code was read and the guarantee follows from it, but nothing exercises it,
  with the reason

The 2026-09-23 checks ran with `HOME`, `XDG_CACHE_HOME` and `XDG_CONFIG_HOME` pointed at a
scratch directory, set on the pane's own command line, which received the cache files. Their
outcomes are recorded here, in the rows that cite them. The checklist they came from is in
`docs/plans/2026-09-23-the-data-loss-audit-is-written-down-and-its-gaps-are-tested.md`.

Last audited 2026-09-23. A change to any code named below updates its row.

## Writing a file

| Path | Guarantee | Kept by | Evidence |
| --- | --- | --- | --- |
| `:w` | Usually the file is replaced whole or not at all: a temp file is written and synced, then renamed over it. Mode, owner, symlinks and hard links are kept. **Not atomic** where a rename would break the file: a file with other hard links, a directory you can't create files in, an owner the temp file can't be given, a failed rename, or a dangling symlink. Those write the file in place, and a write that fails partway leaves it truncated. The text is still in the buffer, so nothing typed is lost unless binvim also dies before the next `:w`. | `paths::write_atomic` from `Buffer::save` | Tests: `write_atomic_creates_and_replaces_a_file`, `write_atomic_keeps_the_file_mode`, `write_atomic_keeps_the_owner_and_group`, `write_atomic_writes_through_a_symlink`, `write_atomic_keeps_hard_links_together`, `write_atomic_refuses_a_read_only_file`, `write_atomic_writes_in_place_when_the_directory_is_read_only`, `a_failed_rename_writes_the_file_in_place`, `an_exclusive_create_refuses_a_planted_symlink` |
| `:w` after another program rewrote the file | `:w` refuses, and names `:w!`. The other program's text stays on disk until you choose. | `Buffer::changed_on_disk` (mtime + length) in `save_active` | Tests: `w_refuses_a_file_rewritten_on_disk_until_w_bang`, `changed_on_disk_follows_writes_by_other_programs`. Check 5 (2026-09-23): the message was `file changed on disk since it was read (:w! overwrites it)`, and `:w!` then wrote the buffer. |
| `:w` on a file that wasn't valid UTF-8 | The buffer is marked lossy, so a write can't silently replace bytes it never held. | `Buffer.lossy` | Tests: `invalid_utf8_marks_the_buffer_lossy`, `e_bang_on_a_file_now_valid_utf8_clears_lossy` |
| `:wa`, `:wq`, `:x`, `:xa` | Each buffer goes through the same path as `:w`. | `save_all` / `save_active` (`app/buffers.rs`, `app/save.rs`) | Reasoned: they call the `:w` path, whose rows above are tested. No test drives `:wa` itself. |
| `:w <file>` | The new file is written atomically, and the buffer's disk fields follow it. | `write_atomic` | Test: `write_atomic_creates_and_replaces_a_file`. The rest is reasoned. |
| Format on save | A formatter that prints nothing doesn't empty the file. | `format::reject_empty` | Test: `empty_formatter_output_is_refused_for_a_non_empty_buffer` |
| Disk full | On the rename path, the write fails and the target is untouched, because the temp file is written and synced before the rename. On an in-place fallback (see `:w`), the target can be left truncated, and the buffer still holds the text. | `write_atomic` | **Reasoned, not tested.** Simulating `ENOSPC` portably needs a size-capped filesystem. |

## Text changed by something other than typing

| Path | Guarantee | Kept by | Evidence |
| --- | --- | --- | --- |
| File watcher reload | A buffer with unsaved changes is never reloaded under you. | `maybe_reload_from_disk` returns before a reload on `buffer.dirty` (`app/buffers.rs`) | Check 5 (2026-09-23): after an outside rewrite, the dirty buffer still read `original mine`. |
| `:e!` | Discards your changes on request, and clears the recovery dump only when this session wrote or applied it. | `force_reload_from_disk`, `recovery_written` | Test: `e_bang_reverts_to_the_file_on_disk` |
| Git hunk reset | Refuses on a buffer with unsaved changes, then reloads from disk. | `hunk_reset` (`app/git_glue.rs`) | Reasoned: the `buffer.dirty` guard comes first. |
| `:S` and LSP workspace edits | Files nobody sees are opened, edited and saved in one pass, and neither builds on the disk text of a file with a pending recovery dump. `:S` skips that file and edits the rest. An LSP workspace edit refuses the whole batch before touching any file, and names the file. | `pending_recovery` in `app/input.rs` (`:S`) and `apply_concrete_edits` in `app/lsp_glue.rs` | Reasoned, with the rule in `docs/solutions/runtime/open-buffer-runs-for-batch-edits-nobody-sees.md`. No test drives either. |

## Crashes and closed terminals

| Path | Guarantee | Kept by | Evidence |
| --- | --- | --- | --- |
| Recovery dumps | Every buffer with a file name and unsaved changes is dumped every 4 s. On reopen the dump is applied as an undo step. It's removed on write, on `:e!` or close, and on a clean quit. | `recover.rs`, `app/recover_glue.rs` | Tests: `a_written_recovery_file_loads_back`, `a_truncated_recovery_file_loads_as_none`, `text_matching_the_file_is_nothing_to_recover`, `the_recovery_directory_is_private` |
| Another binvim's dump | A dump belonging to a running binvim is left alone. A pid alone isn't trusted: the process's name must match binvim's too (`ps` on unix, `tasklist` on Windows). | `RecoveryFile.pid`, `process_alive` | Tests: `a_dump_is_held_only_while_its_writer_runs`, `ps_output_names_binvim_only_for_binvim`, `tasklist_output_lists_binvim_only_under_its_own_pid`, `a_file_without_a_pid_loads_as_left_by_a_crash` |
| `kill -9` | Text typed up to the last 4 s dump survives. | the recovery interval | Check 3 (2026-09-23): typed, waited 5.5 s, `kill -9`. On relaunch the text was offered and `:w` kept it. |
| A panic | Recovery files are written for every dirty buffer. The session isn't saved, since a file's content can cause a panic. | `catch_unwind` in `main.rs`, and `crash.rs` for terminal restore and the log | Test: `panic_hook_writes_log_with_payload`. Check (2026-09-15, `2026-09-15-overlay-keys-follow-draw-order-and-recovery-gaps-close.md`): a throwaway `panic!` left a dump the next launch applied. |
| A buffer with no file name | **Gap: no recovery.** A `[No Name]` buffer has no path to key a dump on, so its text is lost to a crash, `kill -9` or a closed terminal. A normal `:q` still refuses to quit with it unsaved. | none | Read in `write_recovery_now` and `refresh_recovery_snapshot` (`app/recover_glue.rs`), which skip pathless buffers. Tracked in `KNOWN_ISSUES.md`. |
| SIGTERM, SIGHUP (closed terminal) | Recovery files and the session are written from the signal thread, since a hung-up terminal never lets the event loop run again. | `signal_hook` thread, `recovery_snapshot`, `session_snapshot` | Checks (2026-09-15, `2026-09-15-writes-cannot-lose-work-and-clippy-gates-ci.md` Verification 3 and the overlay plan): `kill -TERM` and `tmux kill-session` left the text in recovery, and a bare relaunch restored the session. |

## The file itself goes away or changes

| Path | Guarantee | Kept by | Evidence |
| --- | --- | --- | --- |
| An open file is deleted | The buffer keeps its text, the status line marks it `[deleted]` with a message that `:w` writes it again, and `:w` recreates the file. A dirty buffer is marked too. | `Buffer.gone`, set by `maybe_reload_from_disk` and cleared by `Buffer::save` and a reload | Test: `a_file_deleted_under_its_buffer_is_marked_gone_until_written`. Check 1 (2026-09-23, before the marker): the text stayed and `:w` recreated the file. |
| An open file with unsaved changes is deleted | The buffer keeps its text, is marked `[deleted]`, and the recovery dump survives. After a crash, reopening the missing path offers the text, and `:w` writes it. | the watcher's dirty guard, and recovery | Check 2 (2026-09-23): passed, finished with a `kill -9` and a relaunch. |
| An open file with unsaved changes is rewritten | Neither version is lost. See the `:w` conflict row. | `changed_on_disk` | Check 5 (2026-09-23) |

## Editor state on disk

| Path | Guarantee | Kept by | Evidence |
| --- | --- | --- | --- |
| Undo history | Saved atomically, keyed to the file's text. A corrupt, truncated or other-text history loads as none rather than applying. The directory is `0700` and pruned after 90 days. | `undo::History::save_to_path` / `load_from_path`, `prune_stale_history` | Tests: `a_truncated_or_garbage_undo_file_loads_as_no_history`, `files_in_the_linear_format_load_and_save_unchanged`, `the_undo_directory_is_private`, `tidying_prunes_only_stale_files_and_makes_the_directory_private` |
| Session | Saved atomically on a clean quit and on SIGTERM / SIGHUP. Refused when it's corrupt or saved for another cwd. A session naming a deleted file restores the rest. | `session::save` / `load_for_cwd`, `hydrate_from_session` | Tests: `a_saved_session_loads_back_for_its_cwd`, `a_truncated_or_garbage_session_file_loads_as_none`, `a_session_saved_for_another_cwd_is_refused`, `a_session_naming_a_deleted_file_restores_the_rest`. Check 4 (2026-09-23): with one of two files deleted, a bare relaunch restored the other. |
| Cache directories holding your text | Recovery, undo and session directories are private to you, and one an older build left wider is narrowed at startup. | `paths::create_private_dir` | Tests: `create_private_dir_narrows_a_wider_existing_dir`, `the_recovery_directory_is_private`, `the_undo_directory_is_private` |
