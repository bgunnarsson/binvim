---
title: Tasks and AI side panes start under cmd.exe and PowerShell, not only a POSIX shell
date: 2026-09-22
status: in-progress
---

## Context

0.7's Windows tier is feature parity (`ROADMAP.md`, "Feature parity (0.7)"; `WINDOWS.md` section 2).
The user asked to start 0.7 there. The first three items in that section are one fix: the task
runner and the AI side panes hand the shell POSIX flags.

- `task_kickoff` (`src/app/task_glue.rs:184-194`) spawns `default_shell() -l -i -c "cd <cwd> && exec
  <program> <args> <tail>"`, quoting the cwd, program and every discovered arg with the POSIX
  `shell_quote` (`task_glue.rs:458`), since script and recipe names come out of project files.
- `open_side_terminal` (`src/app/side_terminal_glue.rs:293-308`) spawns `default_shell() -l -i -c
  "exec <tool>"`, where the tool is a fixed name from `AiTool::command()` (`src/command.rs:361`).
- `default_shell()` (`src/terminal.rs:1556`) falls back to `$COMSPEC` / `cmd.exe` on Windows, and
  its doc comment already says the `/C` translation was left out of v1.

On Windows `cmd.exe` gets `-l -i -c` and doesn't run the task at all. PowerShell (a user's
`$SHELL` or `$COMSPEC` can name `pwsh`) rejects `-i` too, on every platform.

`:terminal` (`terminal.rs:1181`) spawns the shell with no arguments, and `:!` already dispatches
through `format::shell_for` (`src/format.rs:118`). Both work on Windows and are left alone. Every
other child process (tests, DAP prelaunch, lazygit, yazi, installers) is spawned from an argv with
no shell.

Decisions:

- **The shell is classified by its file stem.** `ShellKind::of(shell)` lowercases the stem of the
  path: `cmd` is `Cmd`, `pwsh` / `powershell` is `PowerShell`, anything else is `Posix`. It's
  decided by the shell, not by `cfg!(windows)`, so `$SHELL=pwsh` on macOS is fixed too, and Git
  Bash's `bash` keeps the POSIX launcher.
- **POSIX is unchanged byte for byte.** Still `-l -i -c "cd '<cwd>' && exec …"` with the existing
  `shell_quote`, so rc files load (the reason the shell is there at all) and no current user sees a
  difference.
- **cmd.exe gets its line through an environment variable, not an argument.** `portable-pty` builds
  the Windows command line MSVC-style (`portable-pty-0.9.0/src/cmdbuilder.rs:702`), so a `"`
  inside the one `/C` argument reaches cmd.exe as `\"`. cmd doesn't unescape that. It toggles
  cmd's quote state, and a `&` or `|` in a project-file script name would then run as a command.
  The spawn is instead `cmd.exe /V:OFF /C %BINVIM_LAUNCH%`, with the line in `BINVIM_LAUNCH`. That
  argument has no space or quote, so `portable-pty` passes it through untouched. cmd expands `%…%`
  once and doesn't rescan the result, so a `%` in a script name stays literal, and `/V:OFF` keeps
  `!` literal. Every word is wrapped in `"…"`, where cmd treats `& | < > ^ ( )` as text. Trailing
  backslashes are doubled so the child's argv parser doesn't read `\"` as a literal quote. A word
  holding `"`, CR or LF can't be quoted for cmd, so the task isn't started and the status line says
  why. `/D` isn't passed, so cmd's AutoRun still runs, as rc files do under POSIX.
- **PowerShell gets `-NoLogo -Command <line>`.** Its words are single-quoted with `'` doubled
  (fully literal in PowerShell) and run with the call operator: `& 'npm' 'run' 'build'`. .NET parses
  its argv by the same MSVC rules `portable-pty` writes, so the line arrives intact. `-NoProfile`
  isn't passed, so the profile loads like an rc file.
- **cmd and PowerShell get the cwd from the spawn, not a `cd`.** `Terminal::spawn_program` already
  sets `CommandBuilder::cwd` (`terminal.rs:1207`). It takes an optional cwd so the task's directory
  goes there, and quoting a path for either shell's `cd` becomes unnecessary. There's no `exec` on
  those shells. `/C` and `-Command` exit when the program does, which is the same result.
- **The `:make` tail keeps its shell meaning in all three.** It's the user's own typed text,
  appended unquoted as today (`task_glue.rs:127-131`).
- **The installer's winget / scoop / choco support is a separate cycle.** It shares no code with
  this change, and it needs package ids and pins looked up for 15 tools.

## Relevant lore

- [Respawning a crashed child on its exit event is an unbounded loop](../solutions/runtime/respawning-a-crashed-child-on-its-exit-event-is-an-unbounded-loop.md):
  a handler that reacts to a child's exit by spawning it again needs a budget. This plan only
  changes the side pane's launch arguments, not its exit handling. The manual check still includes
  a tool that exits at once, under cmd.exe, to confirm the pane falls back to its splash and doesn't
  relaunch.
- [An upward search trusts what other users own or can write](../solutions/security/an-upward-search-trusts-what-other-users-own-or-can-write.md):
  the task's cwd comes from `task::specs::find_root`, which already goes through
  `paths::others_can_plant`. This plan adds no search and moves the cwd unchanged into the spawn.
- [Config reload misses state derived from App.config](../solutions/runtime/config-reload-misses-state-derived-from-app-config.md):
  applies only if a setting is added. None is. The shell is read from the environment at each
  spawn, as today.

## Acceptance criteria

- On macOS and Linux the task and AI-pane spawns are unchanged: `shell_launch` for a POSIX shell
  returns `["-l", "-i", "-c", "cd '<cwd>' && exec '<prog>' '<arg>' <tail>"]`, and the existing
  quoting tests pass unedited in substance.
- With `cmd.exe` as the shell, `:make` / `:task` / `<leader>mm` run the task in the task's
  directory and the output reaches the terminal pane. A `package.json` script named `a&echo pwned`
  runs `npm run "a&echo pwned"` and prints no `pwned`.
- With `pwsh` or `powershell` as the shell, the same tasks run, and the same script name is passed
  literally.
- `<leader>jc` (and the other AI panes) start the tool under cmd.exe and PowerShell.
- A task word holding `"` under cmd.exe doesn't spawn anything, and the status line names the
  reason.
- `WINDOWS.md` section 2 has the task-runner, AI-side-pane and `shell_quote` items ticked, and
  `default_shell`'s doc comment no longer says the translation is out of scope.

## Tasks

- [x] Add `ShellKind` + `ShellKind::of` and the three word quoters (POSIX `shell_quote` moved from
  `task_glue.rs` with its tests, `cmd_quote` returning `None` for `"` / CR / LF, `pwsh_quote`) to
  `src/terminal.rs`. Verify with unit tests: stem classification (`C:\Windows\System32\cmd.exe`,
  `CMD.EXE`, `pwsh`, `/usr/local/bin/pwsh`, `powershell.exe`, `/bin/zsh`, `bash.exe`), each quoter
  on spaces / `&` / `%` / `!` / `'` / trailing backslash, and `cmd_quote` rejecting `"`.
- [x] Add `shell_launch(shell, cwd, words, tail) -> Result<Launch, String>` in `src/terminal.rs`.
  `Launch` holds the program, args, optional cwd and env pairs, built per `ShellKind` as decided
  above. `Terminal::spawn_program` takes the optional cwd and extra env. Its bare `:terminal`
  caller passes `None` / `&[]`. Verify with unit tests for the exact args/env/cwd of each kind,
  including the `:make` tail appended unquoted and a POSIX result identical to today's launcher.
- [x] Route `task_kickoff` through `shell_launch` with `[program, args…]` + `shell_tail`, show the
  error in `status_msg` when it refuses, and drop `launcher_exec_line`. Move its test onto
  `shell_launch`. Verify with `cargo test task_glue::tests` and `cargo test terminal::tests`.
  Deviation: the task-to-words step is a small `task_launch` helper in `task_glue.rs`, so its test
  stays there and checks the POSIX line and the cmd.exe `BINVIM_LAUNCH` value for the same task.
- [x] Route `open_side_terminal` through `shell_launch` with `[command]` and no cwd, and update the
  comment above it that describes the POSIX-only launcher. Verify with `cargo build` and the manual
  AI-pane check below.
- [ ] Add `#[cfg(windows)]` tests in `terminal::tests` that run a `Launch` through
  `std::process::Command` (same argv, env and cwd). For `cmd.exe`, words `["cmd.exe", "/D", "/C",
  "exit 7"]` exit 7, and `["cmd.exe", "/D", "/C", "cd"]` in a temp dir prints that dir. The same
  exit test runs under `powershell.exe`. Verify it on the `windows-latest` CI job.
- [ ] Update docs: tick the three `WINDOWS.md` section 2 items, note the progress under
  `ROADMAP.md` "Feature parity (0.7)", fix `default_shell`'s doc comment, and add an Unreleased
  `### Fixed` entry to `CHANGELOG.md`. Verify by reading the diff.

## Files

- `src/terminal.rs`: `default_shell` (doc comment), `spawn_program` (optional cwd + env), new
  `ShellKind`, quoters, `Launch`, `shell_launch`, tests. Mirrors `format::shell_for`'s one-place
  dispatch and `recover::process_alive`'s `#[cfg(windows)]` test shape.
- `src/app/task_glue.rs`: `task_kickoff` (lines 181-198) calls `shell_launch`, and `shell_quote` /
  `launcher_exec_line` and their tests move out.
- `src/app/side_terminal_glue.rs`: `open_side_terminal` (lines 286-308) calls `shell_launch`.
- `WINDOWS.md`, `ROADMAP.md`, `CHANGELOG.md`.

## Verification

- `cargo fmt`, `cargo test -- --test-threads=1`, and `cargo +1.98.0 clippy --locked --all-targets --
  -D warnings` on macOS, plus the `windows-latest` CI job green, including the new `#[cfg(windows)]`
  spawn tests.
- On macOS in tmux, with a fresh `cargo build --release`: `:make` in this repo and `<leader>jc` still
  start as before. Then with `SHELL=pwsh` passed on the tmux command line, since tmux panes run
  under the server's environment: a task runs, and a pane starts if `claude` is on PATH.
- On the user's Windows machine, under `cmd.exe` (no `$SHELL`) and again with `SHELL=pwsh`:
  1. `:make` in a cargo project under a path containing a space.
  2. An npm project whose `package.json` has a script named `a&echo pwned`. Run it with `:task`;
     no `pwned` line should appear.
  3. `<leader>jc` starts Claude.
  4. Point an AI pane at a tool that isn't installed. The pane falls back to its splash once and
     doesn't relaunch.
