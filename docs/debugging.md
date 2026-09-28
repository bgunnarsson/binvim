# Debugger (DAP)

binvim has a debugger built in for .NET, Go, Python and Rust, and attaches to Android apps: breakpoints in the gutter, stepping, locals, call frames and watch expressions in a pane below the code. It talks the
Debug Adapter Protocol to the same adapters VS Code uses; you install the adapter once, and
binvim finds your project's runnable targets itself.

## Quick start

Install the adapter for your language with `:install` (pick the language; the adapter is part of
its bundle) or by hand from [External tools](external-tools.md). Then open a file in
the project, put the cursor on a line and press `<space>db` (or `F9`) for a breakpoint, and
`<space>ds` (or `F5`) to start. The debug pane opens at the bottom; when the breakpoint is hit,
step with `F10` / `F11` / `Shift-F11` and continue with `F5`. `<space>dq` stops.

- **.NET** — adapter `netcoredbg`, built from source (`:install` shows how; it can't install it
  for you). Starting asks for the runnable project when the solution has more than one, then for
  a `launchSettings.json` profile when the project has more than one.
- **Go** — adapter `dlv` (`go install github.com/go-delve/delve/cmd/dlv@latest`). Starting asks
  which `package main` directory to run, unless the buffer's own directory is one.
- **Python** — adapter `debugpy`, installed into the Python binvim runs: `python3 -m pip install
  --user debugpy`, or without `--user` inside the venv you launch binvim from. A `pipx` install
  isn't seen by it. On a Homebrew or Debian 12+ Python, pip refuses outside a venv. The `.py`
  buffer you are in is run; from another buffer binvim looks for `main.py`, `app.py`,
  `manage.py` and the other names in the table below.
- **Rust** — adapter `lldb-dap`, which ships with LLVM 18+. Starting asks which binary when the
  workspace has more than one, builds it with `cargo build --bin <name>`, and runs
  `target/debug/<name>`. C and C++ are debugged only inside a Cargo project; a CMake or Make
  project isn't recognised.
- **Android (Java / Kotlin)** — the java-debug plugin for jdtls
  ([External tools](external-tools.md)). With the app running on a device or emulator,
  `<space>Ab` attaches to the app of the Gradle project you are in.

## Adapters

Built-in debuggers via an adapter-agnostic DAP client. Four adapters ship today:

| Language     | Adapter binary                | Selection                                                                                                                 |
|--------------|-------------------------------|---------------------------------------------------------------------------------------------------------------------------|
| .NET         | [netcoredbg](https://github.com/Samsung/netcoredbg) | `*.csproj` / `*.sln` / `*.slnx` / `*.fsproj` walks up to a `.sln` / `.slnx` / `.git` root. Two-stage picker: runnable project (class libraries filtered out), then `launchSettings.json` profile. |
| Go           | `dlv dap`                     | `go.mod`. Picker enumerates every directory with `package main` under the workspace root; the buffer's own dir auto-picks when it matches. |
| Python       | `python3 -m debugpy.adapter`  | `pyproject.toml` / `setup.py` / `requirements.txt` / `Pipfile`. Active `.py` buffer wins; otherwise picks from `main.py` / `__main__.py` / `app.py` / `manage.py` / `run.py` / `server.py` / `cli.py`. |
| Rust, and C / C++ in a Cargo project | `lldb-dap` (or legacy `lldb-vscode`) | `Cargo.toml`. Picker rows are each `[[bin]]` / `src/main.rs` / `src/bin/*.rs` across the workspace; prelaunch `cargo build --bin <name>`, launch `target/debug/<name>`. |

Adding a fifth adapter is one row in `dap/specs.rs`'s registry plus a `build_launch_args` fn and a `dap_resolve_*` resolver in `app/dap_glue.rs`.

## Keys and commands

| Capability               | Binding              | Notes                                                                                                                                                                          |
|--------------------------|----------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Start                    | `<leader>ds` / `F5`  | Walks up from the active buffer to pick the adapter, then enumerates targets (`.csproj` / `package main` / `.py` script / `[[bin]]`). 0 → error, 1 → straight through, >1 → picker. Auto-restarts an active session (collapses the old `dq → ds` round-trip into one keystroke; waits up to 1.5 s for the previous debuggee to release its listening port). |
| Launch profile           | (after .NET project pick) | Reads `Properties/launchSettings.json`. Profiles with `commandName: "Project"` (Kestrel hosting) are runnable. 0 → framework defaults; 1 → use directly; >1 → profile picker. The chosen profile's `applicationUrl` becomes `ASPNETCORE_URLS`; its `environmentVariables` flow into the launched process env. |
| Stop                     | `<leader>dq` / `Shift+F5` | Sends `disconnect terminateDebuggee:true`; closes the bottom pane.                                                                                                              |
| Continue                 | `<leader>dc` / `F5` (while paused) |                                                                                                                                                                                |
| Step over / into / out   | `<leader>dn` / `di` / `dO` / `F10` / `F11` / `Shift+F11` |                                                                                                                                            |
| Toggle breakpoint        | `<leader>db` / `F9`  | Gutter `●` marker (or `◆` when conditional). Kept across debug sessions until binvim quits (not saved with the session); resent to the adapter on every toggle so conditions are never silently dropped.                          |
| Conditional breakpoint   | `:dapb if <expr>`    | Attach a `condition` to the cursor line's breakpoint (creates one if absent). Use `:dapb if` (no arg) to strip the condition while keeping the breakpoint. Aliases: `:dapb cond`, `:dapb condition`.                                                                          |
| Hit-count breakpoint     | `:dapb hit <expr>`   | Attach a `hitCondition`. Most adapters accept bare integers (`:dapb hit 5` = pause after the fifth hit); some also accept comparators (`>= 10`). `:dapb hit` (no arg) strips it.                                                                                                |
| Strip conditions         | `:dapb plain`        | Drop both `condition` and `hitCondition` from the cursor line's breakpoint, keeping it as an unconditional pause.                                                                |
| Clear breakpoints (file) | `<leader>dB`         | Drops every breakpoint in the active buffer; resends to the adapter if a session's alive.                                                                                       |
| Toggle pane              | `<leader>dp`         | Bottom split — one header row (a `DEBUG | <adapter>` chip plus the tab labels) with the active tab's rows below. Auto-opens on session start, auto-closes on session end.                                                  |
| Focus pane               | `<leader>df` / click on the pane | Enters `Mode::DebugPane`. Five tabs across the top: **Console**, **Locals**, **Breakpoints**, **Frames**, **Watches**. Sessions open on the Console tab so launch chatter is visible immediately. `Tab` / `Shift-Tab` (or `←`/`→`, or click the label) cycles between them. `1`-`5` jumps to a tab by number. `j`/`k`/`g`/`G` navigate within the active tab. `Enter` (or `Space`) on a Locals row expands the structured value. `Ctrl-Y`/`Ctrl-E` scrolls vertically; `h`/`l` (or `Shift-←`/`Shift-→`, or `Shift+ScrollWheel`) scrolls horizontally for rows wider than the pane; `0` snaps back to column 0. A `«` / `»` glyph marks hidden content on either edge. `c`/`n`/`i`/`O` step without leaving the pane. `:` enters the command line. `Esc` returns to Normal. |
| Doc / Workspace symbols  | `<leader>do` / `dS`  | LSP pickers, scoped under the debug menu so "navigate around code while debugging" actions cluster in one place.                                                                 |
| Watch expressions        | `:dapwatch <expr>` / `:dapunwatch <n>` / `:dapunwatch all` / `:dapwatches` | User-managed list, evaluated against the top frame on every `stopped` event. Rendered on the Watches tab (red value when the server returns an error for the expression — typo, name not in scope at the current frame). Kept across debug sessions until binvim quits (not saved with the session); only the cached value clears between stops. |

**Variable expansion** — structured locals render with `▶`/`▼` markers; expansion lazily fetches `children` per `variables_reference` and caches them across re-renders. All caches clear on `stopped`/`continued` (DAP doesn't promise vref stability between stops).

**Diagnostic surfacing** — adapter stderr (e.g. netcoredbg's `dlopen() error: libdbgshim.dylib not found`) streams into the pane's status_line and output buffer instead of vanishing into `Stdio::null()`. Unverified breakpoints, JIT-rebinding events, and `setBreakpoints` failures show up as console-category output so a never-hits is diagnosable instead of mysterious.

## Debugging a test

`:debugtest` (also `:dt`) starts the debugger on the test under the cursor, so a breakpoint in the
test or the code it calls is hit. It works for Go tests (needs `dlv`) and for pytest, with `debugpy`
installed, in a project with a `pyproject.toml`, `setup.py`, `requirements.txt` or `Pipfile` (the
files the Python debugger looks for; `pytest.ini` alone isn't enough). For other test runners
(cargo, dotnet, Vitest and the rest) it says so in the status line; run those tests with
`:testnearest` instead.

## Colours

The debug pane and gutter take their colours from the theme. Override them under `[colors]`
([Configuration](configuration.md)): `"debug.chip_bg"` (the `DEBUG` chip), `"debug.active_tab_bg"`
(the active pane tab), `"gutter.breakpoint"` (the `●` / `◆` marker), `"gutter.pc_marker"` (the
line the debuggee is stopped on) and `"mode.debug"` (the mode chip while the pane has focus).

## What it doesn't do

- No logpoints, no breakpoints on exceptions, no run-to-cursor.
- No C or C++ project outside Cargo: CMake and Make projects aren't recognised.
- No console for evaluating expressions: expressions are evaluated as watches (`:dapwatch`).
- No attaching to a process you pick: every session launches its target, except Android's
  `<space>Ab`, which attaches to the project's app.
- No launch configuration of your own: there is no `[dap]` section and no `launch.json`. The
  target, arguments and environment come from the project as described above (for .NET, from
  `launchSettings.json`).
- Breakpoints and watches aren't saved when binvim quits.
- When the debug session ends, the pane closes; nothing restarts the session.
