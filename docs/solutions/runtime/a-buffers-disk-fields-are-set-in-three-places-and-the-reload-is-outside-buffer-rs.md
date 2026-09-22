---
title: A buffer's disk-derived fields are set in three places, and the reload is outside buffer.rs
date: 2026-09-22
category: runtime
module: src/buffer.rs, src/app/buffers.rs
paths: ["src/buffer.rs", "src/app/buffers.rs"]
tags: [buffer, reload, clean_hash, disk_mtime, cursor-cache, undo, content-hash]
symptoms:
  - "the cursor comes back on reopen after :q, but not when the file was reloaded from disk earlier in that session (the watcher, :e!, <leader>hr)"
  - "a cache entry keyed by Buffer.clean_hash is written under the hash of the text before the reload, and rejected on the next open"
  - "grep -n 'clean_hash =' src shows Buffer::from_path and Buffer::save only"
root_cause: "`reload_buffer_from_disk_inner` in src/app/buffers.rs swaps the rope with `replace_all` and re-sets `disk_mtime`, `disk_len`, `lossy` and `dirty` by hand rather than building a new Buffer, so a field describing the on-disk text that was added beside those in buffer.rs was refreshed on open and save but not on reload"
related:
  - docs/solutions/runtime/config-reload-misses-state-derived-from-app-config.md
---

## Problem

The last-cursor cache (PR #12, fixed on main the same day) keys each entry by
`Buffer.clean_hash`, the hash of the text as last read from or written to disk,
so a file changed outside binvim doesn't get a stale position back. The field
was added to `Buffer` next to `disk_mtime` and set in `Buffer::from_path` and
`Buffer::save`. After the watcher reloaded a file that another program had
changed, every cursor write for that buffer carried the hash of the old text,
and the next launch rejected it: the file opened at line 1 until the next `:w`.

## What didn't work

- **Setting the field where `disk_mtime` is set in buffer.rs.** Both sites there
  got it. The plan even said the `disk_mtime =` grep was the list of places to
  check, and the grep was never run; it has three hits, and the third is
  `reload_buffer_from_disk_inner` in `src/app/buffers.rs`.
- **The manual checks.** They covered `:q`, `:q!` and a file edited outside
  binvim *before* it was opened. None edited the file while it was open, so the
  reload path was never exercised and the feature looked complete. Review found
  it by reading.

## Root cause

Observed. `Buffer`'s fields that describe the file on disk — `disk_mtime`,
`disk_len`, `lossy`, `line_ending`, and now `clean_hash` — are refreshed at
three sites: `Buffer::from_path` and `Buffer::save` in `src/buffer.rs`, and
`reload_buffer_from_disk_inner` in `src/app/buffers.rs`, which is reached from
the auto-reload watcher (`maybe_reload_from_disk`), `:e!` (`force_reload_from_disk`)
and the git hunk reset (`git_glue.rs`). The reload doesn't construct a new
`Buffer`; it replaces the rope in place and re-sets the disk fields one by one.
A field added in buffer.rs beside its siblings is invisible from there.

## Fix

`e7482c5`: `reload_buffer_from_disk_inner` sets `clean_hash` from the reloaded
text, beside `disk_mtime` and `disk_len`. Verified in tmux: open a file, change
line 100 with `sed` outside binvim, wait for the reload (the row shows the new
text), `50G`, `:q`, reopen — line 50.

## Prevention

- **A field on `Buffer` that describes the file as it is on disk is set wherever
  the rope is replaced from disk:** `Buffer::from_path`, `Buffer::save`, and
  `reload_buffer_from_disk_inner` in `src/app/buffers.rs`. A diff that adds such a
  field and sets it in `src/buffer.rs` alone is a violation.
  `grep -n "disk_mtime =" src/buffer.rs src/app/*.rs` lists the sites; the one in
  `src/app/input.rs` (`:w {file}`) only clears them and is covered by the save.
- **A manual check of anything keyed by the on-disk text includes a reload while
  the file is open** — edit it outside binvim and wait for the watcher, or `:e!` —
  not only an edit made before the open. A check plan that lists `:q`, `:q!` and
  an external edit before launch has not covered the reload.
