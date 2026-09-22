---
title: Every upward search that picks a command, a server root or a program skips what other users can plant
date: 2026-09-22
status: in-progress
---

## Context

`517f81c` made `tag::find_tags_file` pass over a `tags` file other users can write, and `f84d252`
wrote the rule into CLAUDE.md for every search that walks up past the buffer's directory and whose
result is opened, run, or used as a server root. The code has only caught up in `tag.rs`. The user
asked for the rest.

The searches that still take the first match, whoever wrote it:

- **`lsp::find_node_modules_bin`** (`src/lsp/specs.rs:788`). The path it returns is executed:
  `run_prettier` and `run_biome` on save (`src/format.rs:235`, `:444`), and the biome, Tailwind and
  Copilot language servers on open (`src/lsp/specs.rs:184`, `:632`, `:883`). A `.md` saved in
  `/tmp/scratch` runs `/tmp/node_modules/.bin/prettier` if nothing nearer exists. This is the worst
  of them: no server has to misbehave, binvim runs the planted file itself.
- **`lsp::find_workspace_root`** (`src/lsp/specs.rs:764`) and **`find_tailwind_config`**
  (`:706`). The root a language server is started in (`src/lsp/manager.rs:182`), which for
  rust-analyzer means a cargo check that runs `build.rs`.
- **`dap::find_workspace_root`** (`src/dap/specs.rs:920`), **`find_dotnet_workspace_root`**
  (`:723`) and **`find_cargo_target_dir`** (`:697`). The root is the prelaunch build's working
  directory (`src/dap/manager.rs:339` — `cargo build` / `dotnet build` there runs the planted
  project's build scripts), and the target dir holds the program the debugger launches.
- **`package::find_root_by_marker`** (`src/package.rs:207`). The root is `run_capture`'s working
  directory for `npm` / `cargo add` / `go get` (`src/package.rs:532`); `npm` runs a planted
  `package.json`'s lifecycle scripts.
- **`git::find_repo_root`** (`src/git.rs:108`). The root is `git -C`'s argument, and a planted
  `.git/config` can name commands (`core.fsmonitor`, `diff.external`). Git 2.35.2 and later already
  refuse a repository owned by another user (`safe.directory`); the check is still one call, and
  binvim does not require that git.

Decisions (made here, from the code and the rule):

- **The check covers every path the attacker needs to create, not just the file and its
  parent.** `tag.rs`'s `writable_by_others` checks a file and the directory holding it, which is
  enough for `tags` but not for `node_modules/.bin/prettier`: another user can create
  `/tmp/node_modules` mode `0755` and everything under it looks trusted. The shared helper takes
  the ancestor directory and the relative candidate, and reports true when the ancestor or any path
  from it down to the candidate has the `0o002` bit. Mode bits, not ownership, as in `tag.rs`
  (binvim has no `libc`); Unix only; a group-writable directory is still trusted.
- **The helper moves to `paths.rs`** as `pub fn others_can_plant(dir: &Path, rel: &Path) -> bool`,
  beside `find_on_path` and `write_atomic`, and `tag.rs`'s private copy is deleted. Six callers
  across four modules is past "a second file needs it".
- **The buffer's own directory is checked too**, as `find_tags_file` does. When nothing trusted is
  found, each search keeps its existing fallback (`canon`, `None`, `start`). What a language
  server discovers by itself from a root in `/tmp` is out of binvim's hands and out of scope.
- **Glob markers (`*.sln`, `*.csproj`) check the directory only.** The matched file isn't named
  until `dir_contains_extension` has read the directory, and a directory other users can't write
  can only hold a planted file if that file is itself `0o002`, which rewrites content but can't
  create a project.
- **Out of scope, because nothing is opened, run or rooted:** `editorconfig.rs`'s walks (they set
  indent and whitespace options on save), `save::detect_git_branch` (reads `.git/HEAD` for the
  status line), and the `:health` display of `find_tailwind_config` (fixed anyway, since it goes
  through the same function).
- **CLAUDE.md's rule stays as written.** It is already general; the lore doc's Fix section is
  what gets updated, in compound.

## Relevant lore

- [A search that climbs out of the project trusts files other users can plant](../solutions/security/an-upward-file-search-trusts-files-other-users-can-plant.md):
  a walk up whose result chooses a file to open, a server root or a command skips candidates
  other users can write, and its test puts the file in a world-writable ancestor and asserts the
  search goes past it to the next one. Every task below follows both halves; the helper's
  widening to intermediate directories is this plan's addition to it.

## Acceptance criteria

- `paths::others_can_plant` is the only implementation of the check; `grep -rn
  writable_by_others src` finds nothing.
- Each of `find_node_modules_bin`, `lsp::find_workspace_root`, `find_tailwind_config`,
  `dap::find_workspace_root`, `find_dotnet_workspace_root`, `find_cargo_target_dir`,
  `find_root_by_marker` and `git::find_repo_root` has a `#[cfg(unix)]` test that puts a candidate
  in a `0o777` ancestor and asserts the search returns a trusted candidate further up (or its
  fallback when there is none).
- `find_node_modules_bin`'s test plants `node_modules/.bin/<name>` with `0o755` directories and a
  `0o755` file under a `0o777` ancestor — the case the old file-plus-parent check passes.
- `tags_files_others_can_write_are_passed_over` passes unchanged.
- Saving a `.md` in a project under a `0o777` directory that holds `node_modules/.bin/prettier`
  does not run that prettier (checked in tmux, see Verification).
- `cargo test -- --test-threads=1`, `cargo +1.98.0 clippy --locked --all-targets -- -D warnings`
  and `cargo fmt --check` pass.

## Tasks

- [x] Add `paths::others_can_plant(dir, rel)` (`#[cfg(unix)]` / `#[cfg(not(unix))]` split, the
  why-comment moved from `tag.rs` and widened to name commands and server roots), with tests in
  `paths.rs` for: clean tree → false; `0o777` `dir` → true; `0o777` intermediate directory → true;
  `0o666` candidate → true. Switch `find_tags_file` to it and delete `tag.rs`'s
  `writable_by_others`. Verify: `cargo test paths::tests` and `cargo test tag::tests`.
- [x] `find_node_modules_bin` skips a candidate when `others_can_plant(dir,
  "node_modules/.bin/<name>")`. Test as in the acceptance criteria, plus a trusted install further
  up being found, in a new `mod tests` at the bottom of `src/lsp/specs.rs`, which has none yet.
  Verify: `cargo test lsp::specs::tests`.
- [x] `lsp::find_workspace_root` skips a marker when `others_can_plant(dir, marker)` (dir only for
  `*.` markers); `find_tailwind_config` skips a config or `package.json` the same way. One test
  each. Verify: `cargo test lsp::specs::tests`.
- [x] `dap::find_workspace_root` (through `has_any_marker`), `find_dotnet_workspace_root` (the
  `.sln` / `.git` pass) and `find_cargo_target_dir` skip planted candidates. One test each.
  Verify: `cargo test dap::specs::tests`.
- [ ] `package::find_root_by_marker` ignores a planted marker for `nearest` and a planted `.git`
  for the early stop. Test covering both. Verify: `cargo test package::tests`.
- [ ] `git::find_repo_root` skips a planted `.git`. Test. Verify: `cargo test git::tests`.
- [ ] Full gate and the tmux check in Verification; `cargo build --release`.

## Files

- `src/paths.rs`: new `others_can_plant` and its tests; the module already has
  `#![allow(dead_code)]` for the lib/bin split.
- `src/tag.rs`: `find_tags_file` calls `crate::paths::others_can_plant`; the private helper goes.
  Its test `tags_files_others_can_write_are_passed_over` (`src/tag.rs:305`) is the model for every
  new test: a scratch tree, a `0o777` shared directory, `set_permissions`, cleanup with
  `remove_dir_all`.
- `src/lsp/specs.rs`: `find_node_modules_bin`, `find_workspace_root`, `find_tailwind_config`.
- `src/dap/specs.rs`: `find_workspace_root` / `has_any_marker`, `find_dotnet_workspace_root`,
  `find_cargo_target_dir`.
- `src/package.rs`: `find_root_by_marker`.
- `src/git.rs`: `find_repo_root`.

## Verification

1. `cargo test -- --test-threads=1` (parallel runs fail unrelated tests on macOS),
   `cargo +1.98.0 clippy --locked --all-targets -- -D warnings`, `cargo fmt --check`.
2. In the scratchpad, make `shared/` mode `0777` holding `node_modules/.bin/prettier` — a script
   that writes `shared/ran` — and `shared/proj/a.md`. Open `a.md` in tmux with the debug build,
   wait for the mode line, `ix`, `Escape` in its own `send-keys`, `:w`, `:q!`. `shared/ran` must
   not exist. Then `chmod 755 shared`, repeat, and `shared/ran` must exist — proving the check,
   not a missing prettier, is what stopped it.
3. `cargo build --release`, so the user's `binvim` alias carries the fix.
