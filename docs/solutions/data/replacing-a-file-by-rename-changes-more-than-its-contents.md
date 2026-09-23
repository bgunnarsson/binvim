---
title: Replacing a file by temp-file-and-rename changes who can read, own and replace it, not just its contents
date: 2026-09-15
category: data
module: src/paths.rs
tags: [atomic-write, rename, permissions, symlink, security, write_atomic]
symptoms:
  - ":w on a 0444 file succeeds and replaces it, where it used to fail with permission denied"
  - "after sudo binvim ~user/file, the file is owned by root"
  - "a symlink planted at the temp file's name redirects a save into another of the user's files"
  - "the temp copy of a 0600 file is briefly 0644"
  - ":w fails on a file another Windows program holds open, or a single-file bind mount, where it used to work"
root_cause: "the old save truncated and rewrote the target's inode, so permission checks, owner, mode and the file's identity all stayed with the file; a rename installs a new inode in a directory entry, so every one of those has to be carried over or checked by hand, and the temp name beside the target is a new path an attacker can reach"
related:
  - docs/solutions/conventions/an-audit-guarantee-is-read-from-the-code-not-from-the-lore-that-describes-it.md
---

## Problem

`Buffer::save` used `File::create` on the target, which empties the file before writing, so a
full disk mid-write left neither version. The plan replaced it with `paths::write_atomic`: write
`.<name>.binvim-<pid>.tmp` beside the target, `sync_all`, rename it over. It handled symlinks,
permission bits and hard links, passed its tests and a tmux check, and shipped. Review then found
four ways the rename changed the file itself.

## What didn't work

- **Treating "atomic" as the whole requirement.** The first version copied the mode bits and
  followed symlinks, and tests checked exactly those. Nothing tested what the old in-place write
  had given for free: the permission check on the file, its owner and group, and a path nobody
  else could predict.
- **`File::create` for the temp file.** It is `O_CREAT | O_TRUNC` with no `O_EXCL`, so in a
  directory another user can write (`/tmp` on macOS has no `protected_symlinks`; any
  group-writable project directory on Linux) a symlink planted at the pid-based name redirects the
  write into a file the victim owns.
- **Narrowing the mode after creating the file.** The temp file was created at `0666 & ~umask`
  and `set_permissions` ran afterwards; a reader who opened it in that gap keeps the descriptor.

## Root cause

Verified by the review's verifiers, one of which reproduced the read-only case in a scratch crate:

- **Permission to write the file isn't checked.** `rename` needs write permission on the
  directory, not the file, so a `0444` file in a writable directory was replaced (inode changed,
  mode still `0444`).
- **Owner and group aren't kept.** The new inode belongs to whoever saved it. Copying
  `Permissions` carries mode bits only; ACLs and xattrs are dropped too.
- **The temp name is attack surface** that the in-place write never had.
- **The rename can fail where writing in place works:** Windows sharing violations, `EBUSY` on
  a single-file bind mount.

## Fix

`d1c27f5`, `f4f228f`, `b1da8b7`, all in `write_atomic` (`src/paths.rs`):

- **Temp file:** created with `create_new` (`O_EXCL`, which fails on an existing symlink) under
  a name with nanoseconds and a counter, retried on `AlreadyExists`, and opened with the target's
  mode from the start.
- **Owner and group:** `fchown`ed to the target's (`take_owner`); where that isn't permitted,
  the file is written in place.
- **Read-only files:** an existing target is opened for writing first, so a read-only file
  refuses as before.
- **Failed rename:** falls back to an in-place write. By then every byte has been written to the
  temp file, so the disk isn't the problem.

## Prevention

- **A change to `write_atomic` keeps every in-place fallback and check:** hard links,
  `take_owner`, the write-permission probe on the existing target, `PermissionDenied` creating the
  temp file, and a failed rename. Removing any of them as "simplification" is a violation.
- **Temp files beside a user's file are created with `create_new`, never `File::create`,**
  under a name that isn't derived only from the pid, and with their final mode passed at creation.
  A `File::create(&tmp)` or `set_permissions` as the only mode control is a violation.
- **A new way of writing a user's file is tested for what the old one kept:** a read-only
  target, owner and group, mode, symlink and hard-link targets — not only the contents.
- **Cache files holding a user's text** (recovery, undo) live in a directory created or narrowed
  to `0700`; `recover::write_to` is the model. When the directory can already exist, made by an
  older build that didn't narrow it, it's also narrowed without waiting for a write, as
  `undo::tidy_history_dir` does at startup (`2be2c5b`). Narrowing only at write time leaves
  every file already in the directory readable to other users until the next save. A
  permission change added only to a save path, for a directory earlier releases already
  created, is a violation.
