# Windows Port

## Status (post-0.4.7)

All eight workstreams from the original plan are shipped. The editor builds, tests, and runs on `x86_64-pc-windows-msvc`; CI exercises every push against `windows-latest` alongside ubuntu + macos; the v0.4.7 release produces a signed Windows zip alongside the Linux tarballs.

- [x] **WS1** — `src/paths.rs` centralises home / config / cache / data lookups behind the `dirs` crate
- [x] **WS2** — `paths::find_on_path` handles `;`-split PATHs + `.exe` / `.cmd` / `.bat` synthesis
- [x] **WS3** — tilde + path joins go through `PathBuf::join`; `is_path?` accepts `\` + `Path::is_absolute`
- [x] **WS4** — `terminal::default_shell()` resolves `$COMSPEC` / `cmd.exe` on Windows
- [x] **WS5** — `.editorconfig` `end_of_line` parsed; CRLF round-trip on load + save
- [x] **WS6** — CI matrix is `[ubuntu-latest, macos-latest, windows-latest]` for test + clippy
- [x] **WS7** — `install.ps1` + `release.yml` Windows-msvc zip + README install section + scoop bucket
- [x] **WS8** — test fixtures portable (`std::env::temp_dir()` / `tempfile::TempDir`)

Plus followups: tree-sitter-scss cfg-gated off MSVC (upstream master has the fix; no release cut yet) with CSS-grammar fallback, four Windows-only test fixes (pytest/vitest path normalisation, sh.exe resolution, scss test cfg-gate).

## What's left

Three categories, ordered by how visible they are to a user trying binvim on Windows. The first is done; the other two are what is left.

### 1. On-device verification

CI proves the binary compiles and unit tests pass on `windows-latest`. Each of the following was exercised on a real Windows machine on 2026-09-22 and passed:

- [x] `install.ps1` end-to-end on a fresh Windows 10/11 VM — confirm the zip downloads, extracts to `%LOCALAPPDATA%\binvim\bin\`, the PATH hint prints, and `binvim --version` works.
- [x] LSP discovery — drop `rust-analyzer.exe` on PATH (or under `~/.cargo/bin`), open a `.rs` file, confirm diagnostics + hover + goto-def arrive. Repeat with one `.exe`-suffixed tool (`gopls.exe`) and one that should resolve via the tilde-expansion path (`csharp-ls` from `.dotnet/tools`).
- [x] DAP launch — install `netcoredbg.exe`, open a `.NET` project, `<leader>db` to set a breakpoint, `<leader>dr` to run, confirm the breakpoint hits and locals + watches populate.
- [x] `:terminal` — confirm `cmd.exe` spawns and `dir` runs. ConPTY requires Windows 10 1809+.
- [x] CRLF round-trip — open a file with `\r\n` line endings in a real editor session, edit, `:w`, hex-dump the result to confirm `\r\n` is preserved. Repeat with an `.editorconfig` forcing `end_of_line = lf` to confirm conversion.
- [x] Second-instance recovery — open a file in binvim, dirty it and wait 5 s, then open the same file in a second binvim: it must not report recovered changes, and the first binvim's file under `%LOCALAPPDATA%\binvim\recover\` must still be there. Exercises `recover::process_alive`'s `tasklist` call.

### 2. Features that don't work on Windows yet (out of WS1-8 scope but real)

These shipped working on Unix and aren't broken — they just don't function on Windows because they assume a POSIX shell. The bare `:terminal` does work; only flows that *invoke* a shell with `-l -i -c` are affected.

- [x] **Task runner** (`:task`, `:tasklast`, `<leader>mm` / `<leader>ml`) — launches through `terminal::shell_launch`, which reads the shell's dialect off its file name: `/V:OFF /C` for `cmd.exe` (the line carried in `BINVIM_LAUNCH`, since portable-pty would escape a quoted word's `"` as `\"`, which cmd doesn't unescape), `-Command "& …"` for PowerShell, and `-l -i -c` unchanged for POSIX shells and fish. The task runs in its own directory (`pushd` for a UNC one under cmd.exe).
- [x] **AI side pane** (`<leader>jc/jx/jo`) — the same `shell_launch`. Under cmd.exe the tool is resolved through `PATH` first, because cmd looks in the current directory (the project) before `PATH`.
- [x] **`shell_quote`** for task launching — one quoter per dialect in `terminal.rs`: `shell_quote` (POSIX), `fish_quote`, `cmd_quote` (double quotes; refuses `"`, a line break or NUL) and `pwsh_quote` (single quotes; `shell_launch` also refuses a word holding a character cmd.exe would act on, since PowerShell re-quotes words unsafely for a `.cmd` shim).
- [ ] **Full SCSS highlighting** — `tree-sitter-scss 1.0.0` on crates.io passes a GCC-only flag to `cl.exe`. The crate's `master` branch already fixed it (commit [`9ab738d`](https://github.com/tree-sitter-grammars/tree-sitter-scss/commit/9ab738d)) but no release was cut. The cfg-gate in `Cargo.toml` and `lang.rs` is a one-liner removal once they ship a 1.0.1. 0.7 ships with the CSS-grammar fallback (`docs/known-issues.md`); this lands after it, whenever upstream releases.
- [x] **`binvim-install` / `:install` — native Windows package managers (winget / scoop / choco).** `Installer::Winget` / `Scoop` / `Choco` carry a package id each, and `detect_managers` probes all three. They come after every other installer in a `Tool`, so no macOS / Linux pick is a Windows one (`no_unix_host_picks_a_windows_manager`), and on Windows winget wins when present. scoop runs through PowerShell, since it is a PowerShell shim, and only its `main` bucket is used. An entry exists only where the package puts the tool on `PATH`: winget's portable packages (zig, zls, lua-language-server, marksman) do, LLVM's MSI and Elixir's silent NSIS install don't, so clangd, clang-format and mix come from scoop. winget's "no applicable update" exit counts as success, a command shared by two tools runs once, and `:update` upgrades through the manager whose directory the binary is in. lldb-dap, jdtls, google-java-format, kotlin-language-server and ktfmt have no Windows package that puts them on `PATH` and stay `NoManager`. The ids and where each was confirmed are in `docs/plans/2026-09-23-installer-installs-through-winget-scoop-and-choco.md` (removed once done; `git show b98f700:docs/plans/2026-09-23-installer-installs-through-winget-scoop-and-choco.md`).

### 3. Explicit deferrals (still deferred)

These were called out as "out of scope for v1" in the original plan, by intent. Listed here for completeness, not as backlog items.

- [ ] **winget submission** — three YAML manifests (installer + locale + version) submitted as a PR to `microsoft/winget-pkgs` under `manifests/b/Bgunnarsson/Binvim/0.4.7/`. Requires forking under bgunnarsson. Scheduled separately.
- [ ] **Code-signing the Windows binary** — Microsoft SmartScreen will warn "Windows protected your PC" on first run of any unsigned exe. Fixing requires a real Authenticode certificate (~$100/yr from DigiCert / Sectigo / SSL.com) wired into `release.yml` between build and zip steps. Skipped for v1 — the `install.ps1` path documents the warning rather than papering over it.
- [ ] **MSI / MSIX installer** — WiX or MSIX packaging for Add/Remove-Programs registration and IT-managed enterprise installs. `cargo install` / scoop / winget cover ~95% of developer-tool installs; defer until users specifically ask.
- [ ] **PowerShell as default shell** — `terminal::default_shell()` could honour a `[terminal] shell = "pwsh"` config knob. Trivial once anyone wants it; cmd.exe is the universally-present default for now.
- [ ] **WSL path translation** — opening a `\\wsl$\Ubuntu\home\user\foo.rs` from a Windows binvim, or vice versa from a WSL binvim picking up a Windows-side path. Different problem space — defer until the native Windows build is stable enough that anyone's mixing the two.

The plan that drove WS1-8 is in git history: `git show 182069a:WINDOWS.md`.
