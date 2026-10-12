---
title: A file under the temp directory is written in place, because the program that handed it over checks the inode
date: 2026-10-12
category: data
module: src/paths.rs
tags: [atomic-write, in-place, inode, crontab, temp-dir, write_atomic]
symptoms:
  - "EDITOR=binvim crontab -e prints 'crontab: temp file must be edited in place' and drops the edit (macOS)"
  - "a file under /tmp or $TMPDIR changes inode on every save, so anything holding the old path open — or re-checking dev+ino — is looking at the file from before the edit"
root_cause: "write_atomic writes a temp file beside the target and renames it over, which installs a new inode; crontab's temp file is stat'ed before the editor runs and again after it exits, and macOS/BSD crontab errx()es on a dev+ino it doesn't recognise instead of installing the crontab"
related:
  - docs/solutions/data/replacing-a-file-by-rename-changes-more-than-its-contents.md
---

## Problem

`crontab -e` creates `/tmp/crontab.XXXXXXXXXX` (mode `0600`, one link), runs `$EDITOR` on it, and
then checks the file it gets back. On macOS and the BSDs that check is the inode:

```c
if (statbuf.st_dev != fsbuf.st_dev || statbuf.st_ino != fsbuf.st_ino)
        errx(ERROR_EXIT, "temp file must be edited in place");
```

`write_atomic`'s rename gives it a different inode, so the editing is thrown away. Linux's cronie
is a different program here — it sets the temp file's mtime to 1970, edits, and then re-opens the
file *by path*, checking only `S_ISREG` and a non-zero mtime — so a rename is accepted there and
the same bug is invisible on Linux.

## What didn't work

- **Reading it as a crontab problem.** The obvious fixes are a `crontab.` name test, an
  environment variable in a wrapper script, or "use another editor for `crontab -e`". None of
  them explain why the same command has worked for years under Neovim with no config at all.
- **Assuming Vim renames.** `'backupcopy'` is `auto` in Vim and Neovim (`optiondefs.h`: Vi
  default `yes` on Unix, Vim default `auto`), and `auto` renames *for an ordinary file* — measured:
  a save under `$HOME` changes the inode. The rename only happens as part of making a backup,
  though, which is the part that matters.

## Root cause

The rename is not unconditional; it is what making a backup *is*. In `bufwrite.c`:

```c
dobackup = (p_wb || p_bk || *p_pm != NUL);
if (dobackup && *p_bsk != NUL && match_file_list(p_bsk, sfname, ffname))
        dobackup = FALSE;
```

and the whole `backup_copy` / rename block is inside `if (... && dobackup)`. `'backupskip'` gets
its default from `set_init_default_backupskip()`, identical in Vim (`src/option.c:133`) and Neovim
(`src/nvim/option.c:205`) — both grep-verified against their master, 2026-10-12: `/tmp` on Unix, `/private/tmp` on macOS, plus `$TMPDIR`, `$TEMP` and
`$TMP`. A file in one of those therefore never gets a backup, so it is written in place, inode and
all. Measured with `nvim -u NONE`: a save of `/tmp/x` kept the inode, a save of `~/x` changed it.
The user's LazyVim config set nothing about it — it is the stock default.

The same rule, other editors: Emacs renames only with `file-precious-flag` or
`break-hardlink-on-save` (both nil by default) and its `normal-backup-enable-predicate` refuses
backups in the temp directory; VS Code renames only when a caller passes `atomic: { postfix }`
(`diskFileSystemProvider.writeFile`), and an ordinary save is a truncating `fs.writeFile`.

## Fix

`paths.rs`, no config: `temp_dirs()` (the environment's temp directory, `/tmp` on Unix, and
`$TMPDIR`/`$TEMP`/`$TMP`) and `in_temp_dir()`, which canonicalises both sides so `/tmp` and
`/private/tmp` are one directory and a symlinked temp directory still matches. `write_atomic` hands
them to `write_atomic_with`, which takes the in-place branch when the target exists and is under
one of them.

Two things stay deliberate:

- **The gate is `existing.is_some()`.** A new file has no inode for anyone to be holding, and
  creating it beside the target and renaming is what keeps a planted symlink in a world-writable
  `/tmp` from taking the write.
- **The temp directories are a parameter.** The tests' own files are all under the temp directory,
  so `write_atomic_with` takes the list and the tests that are about the rename path pass `&[]`
  (`write_atomic_elsewhere`). Without that, the whole atomic path — mode, owner, symlink, the
  failed-rename fallback — would quietly stop being tested.
- **The read-only probe stays ahead of that branch,** so a refused save of a `0444` file in a temp
  directory reports the permission error and leaves no backup copy, as it did before.

Verified end to end: `:w` of a `/tmp` file kept its inode after the change and changed it before,
and `EDITOR=<built binvim> crontab -e` went from `crontab: temp file must be edited in place` to
`crontab: installing new crontab`.

## Prevention

- **A file under the temp directory is written in place** (`temp_dirs`, `in_temp_dir`). Dropping
  the rule, or the `existing.is_some()` half of it, is a violation: the first breaks every
  `crontab -e`, the second lets a symlink in `/tmp` take a new file's write. Tests:
  `a_file_in_the_temp_directory_keeps_its_inode`,
  `a_new_file_in_the_temp_directory_is_still_renamed_into_place`,
  `the_temp_directory_is_the_only_place_a_path_counts_as_being_in` — the first passes a rename
  that panics, so the fallback can't silently become the bug again, and
  `write_atomic_writes_an_existing_temp_file_in_place` goes through `write_atomic` itself, which is
  the half a test giving the list explicitly can't cover.
- **The temp-dir rule is not gated on `paths::others_can_plant`,** and must not be: the whole
  point of these paths is that they are in a directory others can write.
- **The in-place write still copies the file to `<cache>/binvim/backup/` first,** so giving up the
  rename doesn't give up the "never truncated" guarantee — with a cache directory, a failed write
  leaves the copy and names it in the error.
- **The other editors are the reference, not a custom rule.** A narrower `crontab.` name test
  would work today; Vim, Neovim and Emacs' temp-directory rule is what keeps it working for the
  next program that hands the editor a file and checks it by inode.
- **The environment's temp variables are taken at face value,** as `'backupskip'` takes them:
  `TMPDIR=/` or `TMPDIR=$HOME` would make every save in-place. That is the user's own setting on
  their own machine, and the backup copy still stands behind it; a guard against it would be a
  rule the other editors don't have.
