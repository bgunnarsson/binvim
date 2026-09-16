---
title: A search that climbs out of the project trusts files other users can plant, and opening what they name starts a language server
date: 2026-09-16
category: security
module: src/tag.rs
paths:
  - src/tag.rs
  - src/app/tag_glue.rs
  - src/package.rs
  - src/editorconfig.rs
  - "src/lsp/**"
  - "src/dap/**"
tags: [tags, ctags, find_tags_file, ancestors, world-writable, tmp, lsp, open_buffer]
symptoms:
  - "Ctrl-] in a file under /tmp opens a file nobody in the project wrote"
  - "a tags file in /tmp or /private/tmp is used for a buffer in /tmp/scratch"
  - "rust-analyzer starts on a workspace outside the project after a tag jump"
root_cause: "find_tags_file walked every ancestor to / and took the first file named tags, whoever could write it, and the file it names is opened with open_buffer, which attaches a language server that may build the project it finds"
related:
  - docs/solutions/runtime/open-buffer-runs-for-batch-edits-nobody-sees.md
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

## Fix

`517f81c`: `find_tags_file` passes over a `tags` file that is itself world-writable, or that
sits in a world-writable directory (`writable_by_others`, `src/tag.rs`; Unix only).
`tags_files_others_can_write_are_passed_over` covers both cases and checks that the search goes
on to the next ancestor. binvim has no uid API (no `libc`), so mode bits are the check rather than
ownership. A group-writable directory is still trusted.

## Prevention

- **A search that walks up out of the buffer's directory, and whose result chooses a file to open,
  a directory to root a server in, or a command to run, skips candidates that other users can
  write.** A new `ancestors()` / `parent()` loop with `.find(|p| p.is_file())` (or `.exists()`)
  and no ownership or `0o002` check is a violation. That applies to a new marker search in
  `package.rs`, `lsp/`, `dap/` or `editorconfig.rs` too, when its result is acted on.
- **A path read from a file found that way is untrusted input.** Passing it to `open_buffer`,
  `Command::new` or a server's root without the check above is a violation, even though the
  read itself is harmless.
- **The test for such a search includes a world-writable ancestor holding the file,** and asserts
  that the search goes past it. A test that only covers "found two levels up" and "found nowhere"
  misses this.
