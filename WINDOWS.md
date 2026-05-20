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

Three categories of unfinished work, ordered by how visible they are to a user trying binvim on Windows.

### 1. On-device verification

CI proves the binary compiles and unit tests pass on `windows-latest`. None of the following has been exercised on a real Windows machine:

- [ ] `install.ps1` end-to-end on a fresh Windows 10/11 VM — confirm the zip downloads, extracts to `%LOCALAPPDATA%\binvim\bin\`, the PATH hint prints, and `binvim --version` works.
- [ ] LSP discovery — drop `rust-analyzer.exe` on PATH (or under `~/.cargo/bin`), open a `.rs` file, confirm diagnostics + hover + goto-def arrive. Repeat with one `.exe`-suffixed tool (`gopls.exe`) and one that should resolve via the tilde-expansion path (`csharp-ls` from `.dotnet/tools`).
- [ ] DAP launch — install `netcoredbg.exe`, open a `.NET` project, `<leader>db` to set a breakpoint, `<leader>dr` to run, confirm the breakpoint hits and locals + watches populate.
- [ ] `:terminal` — confirm `cmd.exe` spawns and `dir` runs. ConPTY requires Windows 10 1809+.
- [ ] CRLF round-trip — open a file with `\r\n` line endings in a real editor session, edit, `:w`, hex-dump the result to confirm `\r\n` is preserved. Repeat with an `.editorconfig` forcing `end_of_line = lf` to confirm conversion.

If any of these fail, the fix probably belongs in WS1-5; the plan covered the wiring but not the on-host validation.

### 2. Features that don't work on Windows yet (out of WS1-8 scope but real)

These shipped working on Unix and aren't broken — they just don't function on Windows because they assume a POSIX shell. The bare `:terminal` does work; only flows that *invoke* a shell with `-l -i -c` are affected.

- [ ] **Task runner** (`:task`, `:tasklast`, `<leader>mm` / `<leader>ml`) — `task_glue` runs `default_shell() -l -i -c "cd <cwd> && exec <command>"`. `cmd.exe` rejects POSIX-style dash flags; needs a per-shell dispatch (`/C` for `cmd.exe`, `-Command` for `pwsh.exe`, untouched for `bash`-family). The five task sources (npm / Justfile / cargo / Makefile / dotnet) and the parsed shell command line otherwise carry over.
- [ ] **AI side pane** (`<leader>jc/jx/jo` + uppercase variants for path handoff) — same `default_shell() -l -i -c "exec <tool>"` shape in `side_terminal_glue`. Same per-shell dispatch fix applies.
- [ ] **`shell_quote`** for task launching — POSIX single-quote wrapping. Windows needs the cmd.exe quoting variant (double-quote with `^` escapes for caret-interpreted chars). Trivial once the dispatch above lands; rates a `#[cfg]` switch on the helper.
- [ ] **Full SCSS highlighting** — `tree-sitter-scss 1.0.0` on crates.io passes a GCC-only flag to `cl.exe`. The crate's `master` branch already fixed it (commit [`9ab738d`](https://github.com/tree-sitter-grammars/tree-sitter-scss/commit/9ab738d)) but no release was cut. The cfg-gate in `Cargo.toml` and `lang.rs` is a one-liner removal once they ship a 1.0.1.

### 3. Explicit deferrals (still deferred)

These were called out as "out of scope for v1" in the original plan, by intent. Listed here for completeness, not as backlog items.

- [ ] **winget submission** — three YAML manifests (installer + locale + version) submitted as a PR to `microsoft/winget-pkgs` under `manifests/b/Bgunnarsson/Binvim/0.4.7/`. Requires forking under bgunnarsson. Scheduled separately.
- [ ] **Code-signing the Windows binary** — Microsoft SmartScreen will warn "Windows protected your PC" on first run of any unsigned exe. Fixing requires a real Authenticode certificate (~$100/yr from DigiCert / Sectigo / SSL.com) wired into `release.yml` between build and zip steps. Skipped for v1 — the `install.ps1` path documents the warning rather than papering over it.
- [ ] **MSI / MSIX installer** — WiX or MSIX packaging for Add/Remove-Programs registration and IT-managed enterprise installs. `cargo install` / scoop / winget cover ~95% of developer-tool installs; defer until users specifically ask.
- [ ] **PowerShell as default shell** — `terminal::default_shell()` could honour a `[terminal] shell = "pwsh"` config knob. Trivial once anyone wants it; cmd.exe is the universally-present default for now.
- [ ] **WSL path translation** — opening a `\\wsl$\Ubuntu\home\user\foo.rs` from a Windows binvim, or vice versa from a WSL binvim picking up a Windows-side path. Different problem space — defer until the native Windows build is stable enough that anyone's mixing the two.

---

## Original plan (historical)

Everything below is the plan that drove WS1-8. Kept for reference; reviewers landing in the repo now should read the **Status** + **What's left** sections above first.

## Context

binvim today targets macOS and Linux only — `install.sh` refuses unknown OSes, CI is `ubuntu-latest` only, and README has no Windows section. However, the foundation is already largely cross-platform: `crossterm`, `portable-pty` (with a Cargo.toml comment explicitly anticipating ConPTY: "the same `:terminal` code works on macOS / Linux / (eventually) Windows."), and `arboard` all support Windows out of the box, and there are zero `#[cfg(unix)]` gates anywhere in `src/`. The blockers are concentrated in a small number of areas: directory discovery (`$HOME`), `PATH` splitting (`:`), tilde expansion, the `:terminal` shell fallback, line-ending handling, and CI/install tooling.

This plan brings binvim to "first-class Windows support": `cargo install binvim` works on Windows, the editor runs and edits files correctly, configured LSPs/DAPs/formatters are discovered, `:terminal` launches a Windows shell, `.editorconfig` `end_of_line = crlf` round-trips correctly, and CI catches Windows regressions on every PR.

**Out of scope for v1:** WSL-specific path translation, Windows-only LSPs not already supported on Unix, code-signing the release binary, and an MSI/MSIX installer (we'll publish a `.exe` zip via GitHub Releases and document `scoop`/`cargo install` as the install paths).

## Current state — what works vs. what breaks

| Area | Status |
|---|---|
| `crossterm` (raw mode, mouse, alt screen) | ✅ Works on Windows via Win32 APIs |
| `portable-pty` for `:terminal` | ✅ Crate ships ConPTY backend |
| Clipboard (`arboard`) | ✅ Cross-platform |
| LSP/DAP JSON-RPC over stdio | ✅ Plain `Command` + pipes — works |
| Tree-sitter highlighting | ✅ No platform deps |
| `Command::new(...)` invocations | ✅ All use `.arg()` arrays — no shell `-c` to port |
| **Directory discovery** | ❌ Hardcoded `$HOME` + `.cache/.config` everywhere |
| **`PATH` parsing** | ❌ Splits on `:` only in `which_in_path` (LSP + DAP) |
| **Tilde expansion** | ❌ `~/...` resolution uses `$HOME` + forward-slash concat |
| **`:terminal` shell fallback** | ❌ Falls back to `/bin/sh` |
| **EditorConfig `end_of_line`** | ❌ Not parsed |
| **CRLF round-trip** | ❌ Stripped on load, never written back |
| **CI** | ❌ Linux-only |
| **Install / release** | ❌ `install.sh` excludes Windows; no `.exe` artifact in Release pipeline |

## Workstreams

The port breaks into eight workstreams, each shippable as its own commit (and reviewable independently). Land them in this order — earlier ones unblock later ones.

### 1. Centralize directory discovery behind a `paths` module

**Why:** `std::env::var("HOME")` appears at ~10 sites, each silently returning `None` on Windows. Rather than rewriting each call, introduce one module that owns the question "where do binvim's config / cache / data dirs live."

**Approach:**
- Add `dirs` crate (already a transitive dep via several others — but explicitly list it) to `Cargo.toml`.
- New file `src/paths.rs` with:
  - `pub fn config_dir() -> Option<PathBuf>` → on Unix: `~/.config/binvim/`; on Windows: `%APPDATA%\binvim\` (via `dirs::config_dir().map(|d| d.join("binvim"))`).
  - `pub fn cache_dir() -> Option<PathBuf>` → on Unix: `~/.cache/binvim/`; on Windows: `%LOCALAPPDATA%\binvim\Cache\` (via `dirs::cache_dir().map(|d| d.join("binvim"))`).
  - `pub fn data_dir() -> Option<PathBuf>` → for spell wordlist (`~/.local/share/binvim/words` on Unix; `%APPDATA%\binvim\data\` on Windows).
  - `pub fn home_dir() -> Option<PathBuf>` → `dirs::home_dir()`, the only entry point for tilde expansion.
- Migrate call sites to use these:
  - `src/session.rs:156` → `paths::cache_dir().map(|d| d.join("sessions"))`
  - `src/crash.rs:122–123` → `paths::cache_dir().map(|d| d.join("crash"))`
  - `src/undo.rs:152,160` → `paths::cache_dir().map(|d| d.join("undo"))`
  - `src/config.rs:745–746` → `paths::config_dir().map(|d| d.join("config.toml"))`
  - `src/app/health.rs:165–166` → same as config
  - `src/app/buffers.rs:752,754` → `paths::cache_dir().map(|d| d.join("recents"))`
  - `src/spell.rs` + `src/app/spell_glue.rs:27` → `paths::data_dir().map(|d| d.join("words"))`; on Windows skip the `/usr/share/dict/words` fallback (gate with `#[cfg(unix)]`).

**Non-goals:** Don't migrate `lsp/specs.rs` / `dap/specs.rs` tilde-expansion logic here — that goes in workstream 3.

### 2. Cross-platform `PATH` splitting + `.exe` extension handling

**Why:** Two `which_in_path` implementations split on `:`, which on Windows breaks discovery of every LSP and DAP binary. Separately, Windows binaries have an `.exe` suffix that bare names lack.

**Approach:**
- In `src/lsp/specs.rs:870–885` and `src/dap/specs.rs:947–...`, replace `path.split(':')` with `std::env::split_paths(&path)`. (Note: `src/format.rs:742` already uses `split_paths` — so this is the proven pattern.)
- Inside the lookup loop, on Windows also try `cmd_name.exe`, `cmd_name.cmd`, `cmd_name.bat` (PowerShell-installed shims often land as `.cmd`). Pull this into a `find_on_path(name: &str) -> Option<PathBuf>` helper in the shared `paths` module so LSP + DAP + format all use one implementation. (`format.rs:742`'s helper is already close — promote it.)
- For Elixir's `language_server.sh` (`lsp/specs.rs:409`): on Windows, additionally probe `language_server.bat` (that's the actual filename in the Elixir-LS release archive).

**Affected sites:**
- `src/lsp/specs.rs:879` — `path.split(':')` → `std::env::split_paths`
- `src/dap/specs.rs:949` — same
- `src/format.rs:742` — already correct, but extract to shared helper
- `src/lsp/specs.rs:409` — add `.bat` fallback for Elixir-LS

### 3. Tilde expansion + path construction cleanup

**Why:** Five sites build paths with `format!("{}/...", home_dir, ...)` — forward-slash concatenation that works only by coincidence on Windows (Win32 generally accepts `/`, but `PathBuf::join` is the correct primitive and avoids edge cases like `\\?\` prefixed paths and UNC paths).

**Approach:**
- In `lsp/specs.rs:53–56`, `lsp/specs.rs:858–860`, `dap/specs.rs:928–934` — replace `format!("{}/{}", home, rest)` with `paths::home_dir()?.join(rest)`.
- The "is this an absolute path?" check at `lsp/specs.rs:864` and `dap/specs.rs:934` (`if path.contains('/')`) should use `Path::new(&path).is_absolute() || path.contains(std::path::MAIN_SEPARATOR)`.
- Display-side tilde rewriting at `src/render.rs:4778` and `src/app/lsp_glue.rs:1789` — use `pathdiff::diff_paths` or `Path::strip_prefix(home_dir)` and then `format!("~{sep}{rest}", sep = std::path::MAIN_SEPARATOR)`. On Windows the displayed form becomes `~\foo\bar` which is correct for the platform.
- Drop the hardcoded `/usr/local/n/versions/node` probe at `src/install.rs:560` on Windows — gate with `#[cfg(unix)]`.

### 4. `:terminal` Windows shell fallback

**Why:** `src/terminal.rs:866` does `std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())`. Windows has no `$SHELL` and no `/bin/sh`; this is a hard crash for `:terminal`.

**Approach:**
- Replace the unwrap with platform-aware logic:
  ```rust
  let shell = std::env::var("SHELL").ok().unwrap_or_else(default_shell);
  ```
  where `default_shell()` is `cfg!(windows)` → check `$COMSPEC`, falling back to `"cmd.exe"`; otherwise `"/bin/sh"`. (PowerShell could be a future opt-in via config, but `cmd.exe` is universally present and is the documented default for `portable-pty`'s Windows quickstart.)
- Audit `src/app/task_glue.rs:132` and `src/app/side_terminal_glue.rs:116` (any other `/bin/sh` references) and apply the same helper. Centralize in `src/terminal.rs` and re-export.

### 5. EditorConfig `end_of_line` + CRLF round-trip

**Why:** `src/editorconfig.rs` does not parse `end_of_line`. `src/buffer.rs:67` aggressively strips CRLF on load with no record of the original. A Windows user opening a CRLF file will silently rewrite it as LF on save — a footgun.

**Approach:**
- **Detect on load:** in `Buffer::from_path` (`src/buffer.rs:67`), before normalizing, count `\r\n` vs lone `\n` occurrences. Store the inferred line-ending mode (`LineEnding::Crlf | Lf | Mixed`) on the `Buffer` struct (single field, `enum LineEnding`). Mixed → preserve as LF (existing behavior).
- **Honor on save:** in `Buffer::save` (lines 89–97), if the buffer's line-ending is `Crlf` (either inferred from disk or explicitly set by `.editorconfig`), write LF→CRLF as the rope streams out. ropey supports streaming via `write_to`; a thin `CrlfWriter<W>` newtype around the `BufWriter` that replaces `\n` bytes is enough.
- **Parse `.editorconfig`:** extend `EditorConfig` in `src/editorconfig.rs:12–37` with `end_of_line: Option<LineEnding>`. Apply in the on-save transform pipeline (the same place existing transforms like `trim_trailing_whitespace` apply). `.editorconfig` overrides inferred-from-disk.
- **Render belt-and-braces:** keep the `trim_end_matches('\r')` at `src/render.rs` — it costs nothing and protects against rogue `\r` from external edits.
- Add tests for the three transitions (load LF / save LF, load CRLF / save CRLF, load CRLF + `.editorconfig` says LF / save LF).

### 6. CI matrix: add `windows-latest` (and `macos-latest`)

**Why:** Currently `cargo test`, `clippy`, and `fmt` run only on `ubuntu-latest` (`.github/workflows/ci.yml`). Without a Windows job, every workstream above can silently regress on the very platform we're porting to.

**Approach:**
- Change the three jobs (`test`, `clippy`, `fmt`) to use a `matrix.os` strategy with `[ubuntu-latest, macos-latest, windows-latest]`.
- macOS catches darwin-specific path quirks (case-insensitive FS, `~/Library` vs `~/.config`) that we'd otherwise miss — cheap to add now.
- Be wary of one known issue: `cargo test --test-threads=1` (recent CI change) is slow on Windows runners; document that we accept the time cost. If it pushes past 10 min, we can split the matrix per-OS to parallelize.
- `cargo fmt --check` should be Linux-only (no point running it 3x — formatting is platform-agnostic).
- `cargo clippy` runs on all three: catches Windows-only warnings (e.g. `non_camel_case_types` in re-exports, or `#[cfg]`-gated unused imports).

### 7. Install + release pipeline

**Why:** `install.sh` line 30–46 only handles Linux x86_64 and aarch64; Windows users have no documented install path.

**Approach:**
- **`install.sh`:** add no Windows logic — instead, when `OSTYPE` indicates MinGW / Cygwin / MSYS, print a helpful message: "binvim on Windows: install via `cargo install binvim` or download from the GitHub Releases page." Mirror the macOS pattern (line 36–44).
- **`install.ps1`** (new): a thin PowerShell installer that downloads the `windows-x86_64.zip` from Releases, unzips to `$env:LOCALAPPDATA\binvim\bin`, and prints a one-liner for adding it to `PATH`. Keep it under 60 lines.
- **GitHub Actions release workflow:** extend whatever currently produces the Linux musl tarball to also build `x86_64-pc-windows-msvc` on `windows-latest` and upload `binvim-<version>-windows-x86_64.zip`. (If there's no release workflow yet, this is a follow-up — `cargo install binvim` already works from crates.io, so the bare-minimum install path doesn't depend on a binary release.)
- **README.md:** add a Windows section between macOS and Linux, listing `cargo install binvim`, `scoop install binvim` (if/when bucketed), and the `install.ps1` one-liner. Note that ConPTY support requires Windows 10 1809+ (a `portable-pty` requirement worth flagging).

### 8. Test fixture paths

**Why:** Tests at `src/app/task_glue.rs:395,401`, `src/app/lsp_glue.rs:2346–2376`, `src/dap/manager.rs:1235–1319`, and the per-runner test modules (`src/test/{vitest,dotnet,gotest,pytest}.rs`) hardcode `/tmp` workspace roots. CI on Windows will fail every one of these.

**Approach:**
- Replace `Path::new("/tmp/...")` and `PathBuf::from("/tmp")` with `std::env::temp_dir().join(...)`.
- For tests that need a *real* unique directory (e.g. ones that create files), use `tempfile::TempDir` — already in the dependency tree from other tests.
- For tests that just need a plausible-looking path string (and don't actually touch the filesystem), `std::env::temp_dir().join("x y")` is fine.

## Critical files to modify

Path module + migrations:
- `src/paths.rs` (new)
- `src/session.rs:156–160`
- `src/crash.rs:122–123`
- `src/undo.rs:152,160`
- `src/config.rs:745–746`
- `src/app/health.rs:165–166`
- `src/app/buffers.rs:752,754`
- `src/spell.rs`, `src/app/spell_glue.rs:27`

PATH + tilde + `.exe`:
- `src/lsp/specs.rs:53–56, 409, 858–885`
- `src/dap/specs.rs:928–949`
- `src/format.rs:742` (extract helper)
- `src/render.rs:4740, 4778`
- `src/app/lsp_glue.rs:1789`
- `src/install.rs:560`

`:terminal` shell:
- `src/terminal.rs:859–866`
- `src/app/task_glue.rs:132`
- `src/app/side_terminal_glue.rs:116`

Line endings:
- `src/buffer.rs:60–97` (load + save)
- `src/editorconfig.rs:3–37` (parse + apply)
- `src/render.rs` (leave the `\r` trim as defense-in-depth)

CI + install + release:
- `.github/workflows/ci.yml`
- `install.sh:30–46`
- `install.ps1` (new)
- `README.md:126–149`

Tests:
- `src/app/task_glue.rs:395,401`
- `src/app/lsp_glue.rs:2346–2376`
- `src/dap/manager.rs:1235–1319`
- `src/test/vitest.rs`, `dotnet.rs`, `gotest.rs`, `pytest.rs`

## Reusable code already present

- `std::env::split_paths` is already used correctly at `src/format.rs:742` — promote that helper.
- `arboard` clipboard at `src/app/registers.rs:264–266` — nothing to do.
- `portable-pty` already handles ConPTY — no PTY rewrite needed.
- LSP URI conversion at `src/lsp/types.rs:399` already does `s.replace('\\', "/")` — so `file:///` URIs already round-trip Windows paths correctly.
- `crossterm`'s SIGWINCH abstraction handles Windows resize natively — no signal-handling work.

## Verification

End-to-end checks, in order:

1. **`cargo test` on all three platforms** — workstreams 1, 2, 3, 5, 8 are covered by existing unit tests once paths are made portable. Confirm the CI matrix from workstream 6 stays green.
2. **`cargo build --release` on Windows** — produces `target\release\binvim.exe`. Run it on a sample Rust project; confirm the buffer loads, normal-mode movement works, `:w` saves with the same line endings as the source file.
3. **LSP discovery on Windows** — install `rust-analyzer.exe` via rustup, open a `.rs` file, confirm diagnostics appear. Repeat with one PATH-installed tool (`gopls.exe`) and one `~/.cargo/bin`-style tool to exercise the tilde path.
4. **DAP launch on Windows** — install `netcoredbg.exe`, open a `.NET` project, press `<leader>db`, confirm breakpoints hit. (DAP is more sensitive to path quirks than LSP — good integration test.)
5. **`:terminal` on Windows** — run `:terminal`, confirm `cmd.exe` spawns and `dir` works.
6. **`.editorconfig` CRLF round-trip** — create a file with `\r\n` endings, set `.editorconfig`'s `end_of_line = crlf`, edit + save, `xxd` (or `Format-Hex` on Windows) confirms `\r\n` preserved. Repeat with `end_of_line = lf` on a CRLF source file → confirms conversion.
7. **`install.ps1` smoke test** — fresh Windows VM, run the one-liner, confirm `binvim --version` works.
8. **README install instructions** — follow each documented path verbatim on a clean machine; correct anything that drifted.

If anything specific blocks (e.g. ConPTY misbehaves on Windows Server runners), document the workaround in CLAUDE.md under a new "Windows-specific notes" section so it's not lost.

## Out of scope / follow-ups

- WSL integration (translating Linux paths to Windows paths when binvim runs in WSL). Worth considering once the native Windows build is stable.
- PowerShell as the default shell instead of `cmd.exe`. Users can override via `$SHELL`; making it config-driven is a nice-to-have.
- Code-signing the Windows binary (Windows SmartScreen will warn on the first run; signing requires a certificate, a process, and ~$100/yr).
- An MSI or MSIX installer for "real" Windows install UX. Scoop/cargo cover 95% of developer-tool installs today.
- `scoop` bucket registration once the Release pipeline is producing artifacts.
