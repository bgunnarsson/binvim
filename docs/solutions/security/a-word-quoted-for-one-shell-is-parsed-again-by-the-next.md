---
title: A word quoted for the shell binvim launches is parsed again by the layer after it — portable-pty, PowerShell's batch-shim re-quoting, cmd.exe's lookup, fish
date: 2026-09-22
category: security
module: src/terminal.rs
paths:
  - src/terminal.rs
  - src/app/task_glue.rs
  - src/app/side_terminal_glue.rs
  - src/format.rs
tags: [shell_launch, shell_quote, cmd_quote, pwsh_quote, fish_quote, cmd.exe, powershell, pwsh, fish, portable-pty, batbadbut, PATHEXT, BINVIM_LAUNCH, injection, windows]
symptoms:
  - "'\"exit 7&exit 9' is not recognized as an internal or external command, operable program or batch file."
  - "a cmd.exe launch prints nothing: status Some(1), stdout \"\""
  - "a package.json script named x&calc starts calc under PowerShell"
  - "the AI pane runs a claude.cmd from the project instead of the installed Claude"
  - "a task under SHELL=fish runs a command substitution from its script name"
root_cause: "each word was quoted for the shell binvim spawns, but four later layers parse the line again with other rules — portable-pty's MSVC argv escaping, PowerShell's legacy re-quoting for .cmd/.bat targets, cmd.exe's current-directory-first program lookup, and fish's single-quote escapes — so a quote that is correct for the first parser is broken by the next"
related:
  - docs/solutions/security/an-upward-search-trusts-what-other-users-own-or-can-write.md
---

## Problem

The task runner and the AI side panes gave every shell a POSIX line, `-l -i -c "cd … && exec …"`.
Making them work under cmd.exe and PowerShell (plan
`docs/plans/2026-09-22-tasks-and-ai-panes-start-under-cmd-and-powershell.md` (removed once done; `git show b98f700:docs/plans/2026-09-22-tasks-and-ai-panes-start-under-cmd-and-powershell.md`)) meant quoting each
word for the shell's own dialect. Task words come verbatim out of project files: `package.json`
script names, justfile recipes, cargo aliases. They're the injection surface, while the `:make`
tail is the user's own text and keeps its shell meaning. Every quoter was correct for the shell it
targeted. Four separate layers still reopened the line.

## What didn't work

- **Passing cmd.exe the quoted line as its `/C` argument.** portable-pty joins argv MSVC-style
  (`portable-pty-0.9.0/src/cmdbuilder.rs:702`, `append_quoted`). A `"` inside an argument becomes
  `\"`, which cmd.exe doesn't unescape, so the quote state flips and a `&` in a script name runs.
  Found by reading the code before writing any, so the line goes in the `BINVIM_LAUNCH`
  environment variable instead (`cmd.exe /V:OFF /C %BINVIM_LAUNCH%`). cmd expands it once and
  doesn't rescan the result.
- **Testing the cmd launch with a second cmd.exe inside it.** The first Windows CI run failed with
  `'"exit 7&exit 9' is not recognized as an internal or external command`, and the `cd` check
  printed nothing. The outer launch was correct. The inner cmd.exe reads its own raw command line
  and finds `/C` inside the quoted `"/C"`, so the command it takes starts with the closing `"`.
  The test now runs `powershell.exe` inside the launch, with the `&` behind a PowerShell comment
  (`exit 7 #&exit 9`). An outer split would end the line with its own `exit 9"`.
- **Single-quoting words for PowerShell and trusting it.** `& 'yarn' 'run' 'x&calc'` is literal to
  PowerShell. But for a `.cmd` or `.bat` target, PowerShell rebuilds the child command line: 5.1
  always, and 7.3+ in the default `$PSNativeCommandArgumentPassing = 'Windows'` mode. It quotes
  only words that hold whitespace, and escapes no `"`. Windows runs the shim through `cmd.exe /c`,
  so `x&calc` runs calc. This comes from PowerShell's documented behaviour (review, verdict
  PLAUSIBLE); it wasn't reproduced on Windows. A `.ps1` shim beside the `.cmd` avoids it, and
  yarn's MSI install ships only `yarn.cmd`. The acceptance test name `a&echo pwned` holds a space,
  so PowerShell happened to quote it and the test couldn't catch this.
- **Handing cmd.exe a bare program name.** `"claude"` looks correct, but cmd.exe checks its current
  directory before `PATH`, trying every `PATHEXT` extension. For the AI pane that directory is the
  project, so a cloned repo's `claude.cmd` ran on `<leader>jc` (review, CONFIRMED).
- **POSIX single quotes for every shell cmd and PowerShell didn't claim.** fish reads `\'` and `\\`
  as escapes inside single quotes. `shell_quote`'s `'\''` therefore lets a word holding `\'` close
  its quote early, and `(touch /tmp/pwned)` runs as a command substitution. The old launcher had
  the same hole; making `Posix` the catch-all classification kept it.

## Root cause

Quoting a word is only correct relative to one parser. A launch has a chain of them: the PTY
crate's argv join, the shell, whatever the shell rebuilds for a native program, and the program
lookup. Each one was verified by reading its code or documentation. The portable-pty escaping and
the inner cmd.exe misreading its own switches were also observed on the `windows-latest` runner.
PowerShell's batch-shim re-quoting and cmd.exe's current-directory lookup were not run here; they
follow from Microsoft's documented behaviour. The fish escape rules are fish's documented rules,
traced by hand against `shell_quote`'s output.

Two smaller gaps sit in the same chain. A NUL in a word would end the `BINVIM_LAUNCH` value and
start a new variable in the child's environment block: portable-pty checks argv for NUL but not
env values (`cmdbuilder.rs:638-655`). And cmd.exe won't start in a UNC directory; it falls back to
`C:\Windows`.

## Fix

`src/terminal.rs`, commits `d9addae` (the launch) and `dfc50af` (the review fixes):

- `ShellKind::of` classifies by file stem: `cmd`, `fish`, `powershell`, and any stem starting with
  `pwsh` (`pwsh-preview`, `pwsh-7.4`). Everything else is POSIX.
- `shell_launch` builds one `Launch` per kind:
  - **POSIX and fish:** `-l -i -c` with `shell_quote` or `fish_quote`.
  - **cmd.exe:** the line goes in `BINVIM_LAUNCH`. `cmd_quote` refuses `"`, CR, LF and NUL. A bare
    program name is resolved through `paths::find_on_path`, and one that isn't on `PATH` isn't
    run. A UNC directory is `pushd`ed into rather than set on the spawn.
  - **PowerShell:** `-Command "& …"`, refusing a word that holds `& | < > ^ % ! ( ) "`, CR, LF or
    NUL (`PWSH_REFUSED`), because whether the target is a batch shim can't be known up front.
- `#[cfg(windows)]` tests run a real cmd.exe and a real PowerShell launch, and read back the exit
  code and the directory.

## Prevention

- **A word from a project file reaches a shell only through `terminal::shell_launch`.** A new
  `spawn_program(…, &shell, &["-c", …])`, a `format!` that splices a task, script or recipe name
  into a shell line, or a new caller of `shell_quote` outside `shell_launch` is a violation.
  `format::shell_for` is the exception: it runs the user's own `:!` text, which is meant to keep
  its shell meaning.
- **A quoter is checked against every parser after the shell, not only the shell.** A new
  `ShellKind`, or a change to a quoter, states in its doc comment what the next layer does:
  portable-pty's argv join, how the shell passes arguments to a native or batch target, and how
  the program name is looked up. A test for it includes a word with no whitespace holding `&`, a
  word holding `"`, a word holding `\'`, and a NUL. A test whose only metacharacter word also
  holds a space (`a&echo pwned`) doesn't count.
- **Under cmd.exe, the program is a resolved path, never a bare name.** A cmd launch whose first
  word can reach cmd.exe without a `/` or `\` is a violation, because cmd.exe looks in the current
  directory before `PATH`.
- **A Windows launch test doesn't run cmd.exe inside cmd.exe.** cmd.exe reads its own raw command
  line, so quoted switches (`"/C"`) garble the inner run and hide what the outer one did. Run
  `powershell.exe` or another argv-parsing program inside the launch instead.
- **A value carried in an environment variable is checked for NUL.** portable-pty rejects a NUL in
  argv but writes env values into the environment block unchecked.
