---
title: An upward search trusts a candidate another user owns or can write, at any step down to it, and a fallback to the buffer's own directory roots servers in /tmp
date: 2026-09-22
category: security
module: src/paths.rs
paths:
  - src/paths.rs
  - src/tag.rs
  - src/app/tag_glue.rs
  - src/package.rs
  - src/editorconfig.rs
  - src/git.rs
  - src/android.rs
  - src/format.rs
  - "src/lsp/**"
  - "src/dap/**"
  - "src/task/**"
  - "src/test/**"
tags: [tags, ctags, find_tags_file, ancestors, world-writable, tmp, lsp, open_buffer, others_can_plant, node_modules, rootUri]
symptoms:
  - "Ctrl-] in a file under /tmp opens a file nobody in the project wrote"
  - "a tags file in /tmp or /private/tmp is used for a buffer in /tmp/scratch"
  - "rust-analyzer starts on a workspace outside the project after a tag jump"
  - "saving a .md under /tmp runs /tmp/node_modules/.bin/prettier"
  - "a planted /tmp/build.rs runs after opening /tmp/x.rs, and :health shows the server with (no root)"
  - "the git gutter, LSP root and node_modules formatters are missing on WSL /mnt/c or an exFAT drive"
root_cause: "upward searches took the first match, checked at most the file and its parent by mode bits, and fell back to the buffer's own directory, so a file another user planted under /tmp, or in a 0755 directory they own, was run, opened or used as a server root"
related:
  - docs/solutions/runtime/open-buffer-runs-for-batch-edits-nobody-sees.md
  - docs/solutions/security/a-word-quoted-for-one-shell-is-parsed-again-by-the-next.md
  - docs/solutions/security/an-upward-file-search-trusts-files-other-users-can-plant.md
  - docs/solutions/conventions/an-audit-guarantee-is-read-from-the-code-not-from-the-lore-that-describes-it.md
---

## Problem

Tag support finds its `tags` file by walking up from the buffer's directory to `/`, as the plan
asked (the same shape as `package::find_root_by_marker`). Security review found that the walk has
no boundary. A buffer under `/tmp/scratch` with no nearer `tags` file picks up `/tmp/tags`, which
any local user can create. That file can name any path, since absolute paths are allowed, and
`Ctrl-]` opens the named file.

## What didn't work

- **Treating the walk as a read.** Finding and parsing a `tags` file only reads, and nothing from
  it is written or run directly. But the file chooses what `open_buffer` opens, and `open_buffer`
  calls `lsp_attach_active` (`src/app/buffers.rs`). A planted `lib.rs` next to a planted
  `Cargo.toml` gets rust-analyzer rooted there, and a cargo check runs `build.rs` as the user.
- **Copying Vim's search.** Vim's default `tags` option, `./tags,tags`, doesn't search upward at
  all. Only `tags;` does, and the user opts into that. binvim's upward walk is its own choice, so
  Vim's behaviour doesn't make it safe.

## Root cause

This was verified by reading the code, not by running an attack. `find_tags_file` was
`ancestors().map(|d| d.join("tags")).find(|p| p.is_file())`, with no stop at a project root,
`$HOME` or an owner. `resolve_path` keeps absolute tag paths. `tag_open` passes the path to
`open_buffer`. `lsp/specs.rs` roots rust-analyzer at the nearest `Cargo.toml`. That a cargo check
then runs `build.rs` is rust-analyzer's documented default, not something observed here.

Inside a repository the user cloned, the upward walk adds nothing: opening any file there already
starts the same server. The new exposure is a file *above* the project, in a directory someone
else can write to.

Widening the check to every other upward search (`2026-09-22`, plan
`docs/plans/2026-09-22-upward-searches-skip-what-other-users-can-plant.md` (removed once done; `git show b98f700:docs/plans/2026-09-22-upward-searches-skip-what-other-users-can-plant.md`)) found three more ways
the same planting gets through, each confirmed by review and the last observed in a tmux check in
`/tmp`:

- **A file-plus-parent check misses a nested candidate.** Another user can create
  `/tmp/node_modules` mode `0755`; `.bin/prettier` under it passes a check of the file and its
  directory, and `find_node_modules_bin`'s result is executed on save.
- **Mode bits trust a directory another user owns.** `/tmp/share` (`0755`, theirs) with their own
  `node_modules` beside a `0777` drop folder the victim works in passes every bit check. The same
  bits reject the user's own project on mounts that report everything as `0777` (WSL's `/mnt/c`,
  FAT and exFAT on macOS), which took the git gutter, LSP roots and project formatters with it.
- **A fallback to the buffer's own directory roots the server in `/tmp`.** With nothing trusted
  above `/tmp/x.rs`, `find_workspace_root` returns `/tmp` itself, beside the marker it just
  skipped. Starting the server with no `rootUri` doesn't help: rust-analyzer then took its
  inherited working directory, `/private/tmp`, as the project and ran a planted `build.rs`
  (observed: the file it wrote appeared, with `:health` showing the server as `(no root)`).

The plan's list of searches also missed three that run commands (`task/specs.rs`,
`test/specs.rs`, `android::find_gradle_root`). It came from the lore doc's module list and a
reviewer's summary rather than a grep of `src`.

## Fix

`517f81c`: `find_tags_file` passes over a `tags` file that is itself world-writable, or that
sits in a world-writable directory (`writable_by_others`, `src/tag.rs`; Unix only).
`tags_files_others_can_write_are_passed_over` covers both cases and checks that the search goes
on to the next ancestor.

Then every search went through one check, `paths::others_can_plant(dir, rel)` (`4433383`,
`42f4cba`). It is true when `dir`, or any path from it down to `dir/rel`, is owned by another user,
or is root's and writable by others. A path the user owns is trusted whatever its mode. A symlink's
owner is checked as well as its target, because a symlink's own mode means nothing. std has no
`getuid`, so the user's uid is read off `$HOME`; tests can't make files owned by root or another
user, so under `cfg(test)` the files they make stand in for root's and `chmod 0777` is how a test
plants. Callers: `find_tags_file`, `find_node_modules_bin`, `lsp::find_workspace_root`,
`find_tailwind_config`, `dap::find_workspace_root` / `find_dotnet_workspace_root` /
`find_cargo_target_dir`, `package::find_root_by_marker`, `git::find_repo_root` (`8416a49`,
`8e0f1f0`, `d0e672a`, `6219886`, `627bf48`), and `task::specs::find_root`,
`test::specs::find_workspace_root`, `android::find_gradle_root` (`8eeeb86`).

A fallback directory is checked too: `package::workspace_root` returns `None` for one other users
can write (`a4dc54d`), and `LspManager::ensure_for_path` starts no server there, putting the
directory in `skipped_root` for the status line (`76424b4`). Out of scope, recorded rather than
fixed: tools binvim runs still find their own config above a trusted root (prettier's
`prettier.config.js`, cargo's `.cargo/config.toml`), and `editorconfig.rs` / `detect_git_branch`
only read what they find.

## Prevention

- **A search that walks up out of the buffer's directory, and whose result chooses a file to open,
  a directory to root a server in, or a command to run, skips candidates that other users can
  write.** A new `ancestors()` / `parent()` loop with `.find(|p| p.is_file())` (or `.exists()`)
  and no `paths::others_can_plant` call is a violation, wherever it lives (`task/`, `test/` and
  `android.rs` were missed because nobody grepped for them). A second implementation of the check
  is a violation too: it drifts, as `tag.rs`'s file-plus-parent version did.
- **The check covers every path from the searched directory to the candidate, and asks the owner
  before the bits.** A check of only the candidate and its parent, or of mode bits alone, is a
  violation: the first passes `/tmp/node_modules/.bin/x`, the second passes a `0755` directory
  another user owns and rejects the user's own project on a `0777` mount.
- **A search's fallback directory is held to the same check before anything is rooted or run
  there.** Returning the buffer's own directory when nothing was found, and spawning in it, is a
  violation when that directory is `/tmp`. So is starting a language server with no root as the
  way out: rust-analyzer roots itself at its working directory.
- **A path read from a file found that way is untrusted input.** Passing it to `open_buffer`,
  `Command::new` or a server's root without the check above is a violation, even though the
  read itself is harmless.
- **The test for such a search includes a world-writable ancestor holding the file,** and asserts
  that the search goes past it. A test that only covers "found two levels up" and "found nowhere"
  misses this. For a nested candidate the test plants it with `0755` directories under the `0777`
  ancestor, which is the case a parent-only check passes.
- **Before calling such a change done, `grep -rn "\.parent()\|ancestors()" src`** and account for
  every loop whose result is opened, run or used as a root. A plan that lists searches from memory
  is how three were missed.
- Written into: CLAUDE.md
