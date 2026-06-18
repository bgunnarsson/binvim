# Changelog

All notable changes to binvim are recorded here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project
follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.5.10] - 2026-06-18

### Fixed
- **Shift-Left / Shift-Right now scroll the debug pane horizontally again.**
  The shifted arrow bindings sat below the unshifted Left/Right tab-cycle
  arms in the match, so they were unreachable and never fired. They now
  precede the cycle arms and scroll long stack-frame paths / console lines
  as intended.
- **The status line now shows a modified marker (`●`) for unsaved
  buffers.** The marker slot after the file path had two identical
  branches, so it rendered a blank space whether or not the buffer was
  dirty; it now paints a dot when there are unsaved changes.

### Changed
- Internal clippy cleanup pass — no behavioural change. Mechanical
  lint fixes across the tree (idiomatic `abs_diff` / `div_ceil` /
  `eq_ignore_ascii_case`, redundant `.max(0)` removals, dead-code
  pruning) plus scoped `#[allow]`s where the lint's rewrite would hurt
  readability.

## [0.5.9] - 2026-06-18

### Added
- **`.slnx` solutions are recognised as a .NET workspace root.** The .NET 10
  XML solution format now anchors workspace-root discovery alongside the
  classic `.sln`, so a buffer deep inside a project resolves to the solution
  directory instead of the project directory.

### Fixed
- **Heavy terminal/AI panes no longer freeze input then "replay" it.** A
  chatty TUI in a `:terminal` or AI side pane (e.g. opencode) can queue
  megabytes of output between ticks; the drain parsed the entire backlog
  in one main-loop pass, blocking the event loop long enough that
  keystrokes piled up unread and then fired in a burst once it finished.
  PTY drains are now budget-capped per tick, so leftover output catches
  up across the following ticks while input stays responsive.
- **The .NET debug picker no longer offers class libraries.** Launching the
  debugger listed every project, including class libraries that build then
  fail at launch with a missing `runtimeconfig.json`. The picker now filters
  to projects that produce a runnable executable (explicit `OutputType`
  `Exe`/`WinExe`, or the Web/Worker SDK), falling back to the full list when
  nothing looks runnable so detection gaps never strand a launch.

## [0.5.8] - 2026-06-02

### Fixed
- **Full-screen overlays now scale around open panes.** The installer,
  health, and other full-screen overlays measured themselves against the
  whole terminal, so with a side pane (terminal / AI / DAP) open they
  overflowed or clipped. They now lay out within the editor region that
  remains after the open panes are accounted for.

## [0.5.7] - 2026-06-02

### Fixed
- **Stop the 100% CPU spin on buffers without a live LSP.** A buffer with
  no attached language server still drove the per-version request hooks
  (inlay hints, semantic tokens, documentHighlight) on every render, busy-
  looping the event loop. The hooks now short-circuit when there is no
  live client for the buffer.

## [0.5.6] - 2026-06-02

### Fixed
- **TypeScript / JavaScript highlighting no longer washes out to one
  colour.** Three related fixes: LSP semantic tokens could flood the
  tree-sitter layer with a single token type and overwrite the richer
  syntax colours; property-heavy TS/JS read as one blue wash; and the
  scheme now matches Neovim / Catppuccin — a red `this` anchors access
  chains. Also added an env-gated bracketed-paste probe for diagnosing
  paste issues.

## [0.5.5] - 2026-06-01

### Fixed
- **`:terminal` handles `DCH` / `ICH` / `ECH` so mid-line edits land in the
  grid.** The vt100 parser dropped delete-character, insert-character, and
  erase-character, so programs that edited in place mangled the line.
- **Bracketed paste lands as one atomic blob.** Cmd-V (and other bracketed
  pastes) are now handled as a single insert rather than a stream of
  keystrokes, so auto-pairing / auto-indent don't mangle pasted text.
- **Live AI side panes keep up with typing.** The side-terminal poll budget
  is capped at 16ms so a busy AI pane can't starve keystroke handling.

### Changed
- Bumped `tree-sitter-bash` 0.23 → 0.25, fixing a Linux SIGSEGV in the
  highlight fuzz scanner. Bash-backed fuzz grammars are restricted to ASCII
  to dodge the remaining upstream scanner crash.

## [0.5.4] - 2026-05-31

### Added
- **`openclaw` and `hermes` in the `<leader>j` AI menu.** Both join the AI
  splits menu; the redundant "(new tab)" suffix is dropped from the labels.
- **Start page dismisses onto the first tab on any key when tabs are open.**

## [0.5.3] - 2026-05-30

### Added
- **`openclaw` and `hermes` AI splits.**
- **Double-click word select + word-drag in terminal / AI / DAP panes.**

### Fixed
- **Terminal-pane selection now highlights every covered cell.**
- **Scrolling + cursor behaviour in the AI side panels.**
- **`:terminal` now honours `CPL` / `CNL` / `VPA` cursor moves.** The vt100
  parser silently dropped `CSI nF` (cursor previous line), `CSI nE` (cursor
  next line), and `CSI nd` (vertical position absolute). The .NET MSBuild
  terminal logger uses `CSI nF` to rewind to the top of its progress block
  each frame before redrawing, so without it a `dotnet build` timer printed
  every tick (`(0.0s)`, `(0.1s)`, …) on a fresh line instead of overwriting
  in place. All three are now handled.

### Changed
- README now leads with "the first vim IDE" positioning, adds comparison
  links, and an ASCII banner at the top.

## [0.5.2] - 2026-05-28

### Added
- **DAP Console tab upgrades.** The Console tab now honours ANSI colour
  escapes, cycles its category filter with `f`, supports keyboard yank +
  clear, and offers `/` search with `n` / `N` navigation.

## [0.5.1] - 2026-05-28

### Added
- **Package-manager backends for npm, cargo, and Go (`<leader>p`).** The
  `<leader>p` flow now detects npm / cargo / go workspaces alongside
  NuGet and drives the same install / search → version → add flow
  through the right CLI. `npm` uses `npm list` / `npm view` / `npm
  install`; cargo reads `Cargo.toml` directly for installed packages
  and falls back to the crates.io HTTP API for the version list (no
  `cargo` command lists all versions), uses `cargo search` / `cargo
  add` for the rest; Go parses `go.mod` for direct requires, scrapes
  pkg.go.dev for search, and uses `go list -m -versions` / `go get`
  for the rest. Adding a backend is now one match arm per ecosystem
  with no app-layer churn.
- **Android emulator management + JDWP debug attach (`<leader>A`).** A
  new which-key submenu lists installed AVDs and connected devices,
  launches AVDs, and (for the debug menu) attaches the DAP layer to a
  running Android process over JDWP via `adb forward` plus the
  jdtls-hosted java-debug adapter. Listings + launches run on a
  background thread; the new `start_attach_session` path in
  `DapManager` skips the spawn/launch handshake and goes straight to
  initialize+attach.
- **`:terminal` pane scrollback view.** Scroll wheel and Shift+PageUp /
  Shift+PageDown (Shift+↑ / Shift+↓ for line-wise) now scroll the
  pane's scrollback even when the embedded program isn't capturing
  mouse input — previously those events were dropped on the floor at
  a shell prompt. New output streams in while the view is detached
  keeps the user anchored to the same content (tmux/screen
  behaviour); typing snaps the view back to live. The pane header
  shows `↑ N lines back · Shift+PageDown / scroll down to follow live`
  while detached.
- **`:terminal` drag-select + clipboard copy.** Plain left-drag inside
  the pane body selects, release auto-copies the covered cells to the
  system clipboard. Honours the scrollback view, so dragging across
  scrolled-back history grabs the right text. Selection is
  tab-scoped and clears on tab switch / pane close / pane resize.
- **Live build feedback when starting a debug session.** Adapter
  prelaunch commands (`dotnet build`, `cargo build`) are no longer
  synchronous. They spawn with piped stdout/stderr, stream each line
  into the debug Console tab live, and only kick off the real DAP
  initialize+launch once the build child exits successfully. Build
  failures emit an `AdapterError` but leave the pane open so the
  output stays readable.

### Fixed
- **Clipboard reads fall back to `pbpaste` / `xclip` when `arboard`
  can't reach the host clipboard.** Returning to binvim after copying
  in another app sometimes had `arboard` time out on the X11 / Wayland
  / pasteboard read; binvim now retries via the platform CLI tool so
  the paste still lands.
- **`<leader>A` which-key popup.** The new Android sub-menu didn't
  arm the which-key timer, so the popup never appeared — fixed.

## [0.5.0] - 2026-05-27

### Added
- **Package manager (`<leader>p`).** A generic package-manager entry
  point that detects the active buffer's ecosystem from its workspace
  and drives an add / upgrade flow. NuGet (via the `dotnet` CLI) is the
  first backend; cargo / npm slot in later as additional
  `PackageEcosystem` arms with no app-layer changes. `<leader>pi` picks
  a `.csproj` → installed packages → versions (the installed one
  highlighted, `Tab` toggles prereleases, type to narrow) → install;
  `<leader>ps` picks a `.csproj` → searches the registry → package →
  version → add. Backed by `dotnet list package` /
  `dotnet package search --exact-match` / `dotnet add package`, parsed
  from JSON. Network + restore calls run on background threads and post
  results to a channel drained in the main loop; an epoch guard drops
  results from cancelled flows.
- **Multi-line inline ghost completions.** A Copilot / inline-completion
  suggestion that spans several lines now renders in full: the first
  line paints inline at the cursor as before, and lines 2+ paint as
  muted italic phantom rows directly below the cursor line, pushing the
  real buffer lines beneath them down by the ghost's height. `<Tab>`
  still accepts the whole suggestion. Previously only the first line was
  shown even though the accept already inserted all of them.

### Fixed
- **NuGet parsing tolerates the dotnet first-run banner and honours a
  project's `nuget.config`.** The version / search parse crashed when an
  older SDK printed the "Welcome to .NET" banner ahead of the JSON;
  `run_capture` now sets `DOTNET_NOLOGO` + `DOTNET_CLI_TELEMETRY_OPTOUT`
  and every parser slices to the outermost `{ … }`, so any banner /
  telemetry notice around the JSON is ignored. The list / search / add
  calls also run in the manifest's directory so a project's
  `nuget.config` and its private feeds are used.
- **`<leader>p` which-key prompts.** The Package sub-menu is now
  advertised in the Leader which-key popup (`p → +Package`), and the
  which-key timer arms for the `<leader>p` prefix — previously pressing
  `<leader>p` showed no sub-menu and, on the start page, swallowed the
  `i` / `s` follow-up key.
- **Cursor viewport tracking on inlay-hint-heavy lines.** The viewport
  tracker now counts inlay-hint label widths the same way the cursor
  renderer does, via one shared `cursor_visual_col_walk` helper, so on
  hint-heavy Razor / C# lines the view scrolls to keep the rendered
  cursor on screen instead of leaving it drawn off where the user typed.

## [0.4.8] - 2026-05-25

### Added
- **`:update` command.** A new in-editor overlay that reuses the
  `:install` three-stage flow (bundles → optional Node.js versions →
  plan) but only upgrades the LSPs / formatters / DAP adapters you
  already have on `$PATH` to the catalog's pinned (or newest)
  versions — handy after a release bumps its pins. Tools that aren't
  installed are left untouched and flagged "not installed — run
  :install to add it". Managers that own their package version
  (`brew`, `apt`, `nix`) upgrade via their native upgrade command;
  pinned managers (`npm`, `cargo`, `go`, `gem`, `pipx`, `dotnet`,
  `composer`) re-run their install at the pin.
- **binvim self-update from `:update`.** The first row in the
  `:update` list is binvim itself. It detects how the running binary
  was installed — Homebrew, cargo, the install script, Scoop, or Nix —
  from the executable's path and runs the matching upgrade (`brew
  upgrade`, `cargo install --locked --force binvim`, re-running
  `install.sh`, `scoop update binvim`, `nix profile upgrade binvim`).
  A source / dev build is detected and shown with manual instructions
  instead. The new binary takes effect on the next launch.
- **Installed-tool indicator in the install / update picker.** Tools
  already on `$PATH` are now painted green in the bundle list (probed
  once when the overlay opens, deduped across bundles), so you can see
  what's installed at a glance.

## [0.4.7] - 2026-05-20

### Added
- **First-class Windows support.** binvim now builds, tests, and
  installs on `x86_64-pc-windows-msvc`. The full port spans
  directory discovery, PATH lookup, line endings, the terminal
  fallback shell, CI, and the install pipeline:

  - A new `paths` module routes every home / config / cache / data
    lookup through the `dirs` crate, so `%APPDATA%` /
    `%LOCALAPPDATA%` / `%USERPROFILE%` work the same way `$HOME` /
    `~/.config` / `~/.cache` work on Unix. Sessions, undo history,
    crash logs, recents, the spell wordlist override, and the
    `config.toml` location all relocate automatically.
  - PATH walking switched to `std::env::split_paths` (so `;` works
    on Windows), with `.exe` / `.cmd` / `.bat` candidates
    synthesised for bare LSP / DAP / formatter names. `elixir-ls`
    also probes `language_server.bat`.
  - Tilde expansion + path construction in the LSP and DAP specs
    use `PathBuf::join` instead of `format!("{}/{}", home, ...)`,
    so cargo-bin and dotnet-tools paths resolve correctly on
    Windows.
  - `:terminal`'s `$SHELL` fallback now picks `$COMSPEC` / `cmd.exe`
    on Windows (previously hard-coded `/bin/sh`). The task runner
    and AI side-pane share the same helper. The POSIX-shell
    `-l -i -c` arg wrapping isn't translated to cmd.exe yet, so
    tasks + AI launches stay Unix-only for v1.
  - `.editorconfig`'s `end_of_line = lf | crlf` is now parsed and
    honoured on save. Buffers detect the inferred line ending on
    load and re-emit the same convention by default; mixed-ending
    files collapse to LF (as before). The rope stays LF-normalised
    so motion / render / LSP see one consistent newline byte.
  - CI matrix expanded to `[ubuntu-latest, macos-latest,
    windows-latest]` for `cargo test` and `cargo clippy`. `cargo
    fmt --check` stays on Linux only.
  - `install.ps1` (PowerShell installer): downloads the new
    Windows zip from the GitHub Release, drops `binvim.exe` (+
    `binvim-install.exe`) into `%LOCALAPPDATA%\binvim\bin\`, and
    prints the PATH one-liner instead of mutating the registry.
    Honours `$env:BINVIM_VERSION` / `$env:BINVIM_INSTALL_DIR`.
    `install.sh` now prints a Windows-aware message when run from
    Git Bash / MSYS / Cygwin instead of failing as "unsupported
    OS."
  - Release pipeline gained an `x86_64-pc-windows-msvc` matrix
    entry; the artifact is a `.zip` (Windows convention) with the
    same cosign keyless signing + `.sha256` sidecars the Linux
    tarballs already had.

  Known limitation: `tree-sitter-scss 1.0.0` on crates.io has an
  MSVC-incompatible flag in its `build.rs` (a fix exists on
  upstream master but hasn't been released). The dep is gated on
  `cfg(not(target_env = "msvc"))`; SCSS files on Windows fall
  back to the CSS grammar — selectors and properties highlight
  fine, but `$var` / `@mixin` / `@include` / `#{}` /
  `%placeholder` / `&` nesting lose their dedicated captures
  until upstream cuts a new release.

  See `WINDOWS.md` for the workstream-by-workstream plan that
  guided the port.

## [0.4.6] - 2026-05-20

### Added
- **Tree-sitter highlighting for `.scss` / `.sass`.** A new
  `Lang::Scss` variant wires `tree-sitter-scss` into the highlight
  cache so SCSS-only constructs that the CSS grammar has no view of
  — `$variable` tokens, `@mixin` / `@include`, `@function` /
  `@return`, `@use` / `@forward` / `@extend`, the `@if` / `@for` /
  `@each` / `@while` control-flow at-rules, the `&` parent
  selector, `%placeholder` selectors, `#{}` interpolation, and
  `//` line comments — now render with proper colour instead of
  falling back to plain text. The overlay is layered on top of the
  existing CSS query override so selectors, properties, units, and
  colour values keep the same tones; mixin / include / function
  names ride the `@function` capture so they paint Blue alongside
  `rgb(` / `calc(` calls. `.css` and `.less` continue to use
  `tree-sitter-css` unchanged. Indented-syntax `.sass` files
  highlight only the lines the SCSS grammar's error recovery can
  salvage — there is no maintained tree-sitter grammar for
  brace-free Sass.

- **Pane-scoped mouse-drag selection in the side terminal.** The
  host terminal's native Shift+drag selects across the whole
  window and has no awareness of where the side pane ends, which
  made it impossible to grab just the embedded tool's output. A
  plain left-drag inside the side pane now selects within that
  pane's grid (stream-style, like a text editor — same-row column
  span; multi-row → from start to end-of-line, full middle rows,
  then start-of-line to end col), paints the covered cells with
  inverted SGR so the user sees what they're grabbing, and copies
  the text to the system clipboard on release with a `ai: copied N
  chars` status line. Trailing whitespace per row is trimmed so
  what lands on the clipboard matches what the user saw, not the
  blank padding TUIs use to fill rows. The selection is scoped to
  one tab: switching tabs mid-drag, closing the pane, resizing,
  or any non-drag mouse-down clears the highlight so coords can't
  leak into the wrong grid. A plain click (`Down` → `Up` at the
  same position) still reaches the PTY so AI tools' clickable
  buttons keep working.

## [0.4.5] - 2026-05-19

### Changed
- **Release pipeline brought in sync with crates.io.** 0.4.4 was
  published to crates.io as a one-off `cargo publish --locked` to
  claim the name and put binvim on `cargo install --locked binvim`;
  that single command bypassed the rest of the release pipeline
  (no `v0.4.4` git tag, no GitHub Release artifacts, no Homebrew
  formula bump, no `install.sh` mirror to binvim-web). 0.4.5 cuts
  a full release through `scripts/release.sh` so the v0.4.5 git
  tag, the per-target tarballs attached to the GitHub Release, the
  Homebrew formula on `bgunnarsson/homebrew-binvim`, and the
  `install.sh` served from binvim.dev all line up with the
  crates.io tarball. No editor-visible behaviour change — the
  source tree is identical to crates.io 0.4.4 modulo CHANGELOG
  reconciliation (the `[Unreleased]` section's features all
  actually shipped as part of crates.io 0.4.4, so they've been
  rolled down into the 0.4.4 section below to match what the
  tarball contains).

## [0.4.4] - 2026-05-19

### Added
- **Nix flake.** New `flake.nix` at the repo root builds both
  `binvim` and `binvim-install` from one `rustPlatform.build-
  RustPackage` derivation, re-using the committed `Cargo.lock`
  for vendoring so the nix-built artifacts match what `cargo
  install --locked binvim` produces. Outputs: `packages.default`
  (the editor), `apps.{binvim,binvim-install}` (so `nix run
  github:bgunnarsson/binvim#binvim-install` works), `devShells.
  default` (cargo + rustfmt + clippy + pkg-config), and
  `overlays.default` so downstream NixOS / home-manager configs
  can `nixpkgs.overlays = [ binvim.overlays.default ]` and
  reference `pkgs.binvim`. Linux build adds `xorg.libxcb` for
  `arboard`'s X11 clipboard path; macOS uses Cocoa via the
  default rust stdenv. The BSAL v1.0 license is declared inline
  (`free = false`, `redistributable = false`) rather than the
  predefined `unfree` tag so the actual terms stay visible. The
  flake scopes its own `allowUnfreePredicate` to the binvim
  package so `nix run github:bgunnarsson/binvim` works out of the
  box without `NIXPKGS_ALLOW_UNFREE=1 --impure` on every call.
  `flake.lock` is committed so the input set is reproducible
  across machines; each `apps.*` carries `meta` so `nix flake
  check` runs clean with no warnings. README install section
  adds the `nix run` / `nix profile install` /
  system-config-overlay paths.

- **`cargo install --locked binvim` from crates.io.** Live at
  https://crates.io/crates/binvim — 0.4.4 was the initial publish
  that claimed the name. Cargo.toml carries the crates.io metadata
  (repository, homepage, keywords, `command-line-utilities` +
  `text-editors` categories, anchored `include` whitelist) so the
  playground tree, themes presets, scripts, and `.github/` stay
  out of the published tarball — the crate weighs ~98 files
  instead of ~389 with the default git-tracked behaviour.
  `publish = false` is gone, `rust-version = "1.85"` declares the
  edition-2024 floor. `scripts/release.sh` publishes as step 3b:
  bump → push to main → `cargo publish --locked` → tag → GitHub
  Release → Homebrew → web. The publish runs before the tag push,
  by design: a failed publish doesn't leave a dangling tag,
  GitHub Release, or Homebrew bump pointing at a non-existent
  crates.io version; re-running the script with the same version
  is idempotent (the bump commit is already on main, step 3 is a
  no-op). The pre-flight in step 1 checks for either
  `CARGO_REGISTRY_TOKEN` or a credentials file under
  `${CARGO_HOME:-$HOME/.cargo}/` so a missing token can't blow up
  mid-flow. README install section adds the `cargo install`
  invocation alongside the Homebrew / install.sh / source paths.

- **`:install` — in-editor toolchain installer overlay.** Same
  three-stage flow as the `binvim-install` CLI (bundles → optional
  Node.js versions → plan), rendered inside the editor as a
  full-screen overlay. The ASCII banner up top mirrors the CLI; the
  body is a checkbox list with the standard `j/k` · `Space` · `a`/`n`
  · `Enter` · `q`/`Esc` keys. On the Plan stage `y` suspends the
  editor lazygit-style (pops kitty kbd protocol, disables mouse
  capture, leaves alt screen, drops raw mode) so install output
  streams to the host terminal, then reclaims everything on
  completion and prints a status-line summary. `n` on the plan
  stage walks back to the previous picker preserving prior
  selections. To make this work the install catalog + runner moved
  out of `src/bin/binvim-install.rs` into a new `binvim::install`
  library module — both the CLI binary and the editor now drive
  the same data, so adding a language only touches one place.

- **`binvim-install` — interactive toolchain installer.** New second
  binary in the same crate. Run `binvim-install` and you get a
  checkbox list (j/k navigate, Space toggle, a/n all-none, Enter
  confirm) of every external thing binvim can drive: one bundle per
  language (LSP + formatter + DAP), plus a GitHub Copilot bundle
  (`copilot-language-server`), a Tailwind aux LSP bundle, and three
  editor-tool bundles for `ripgrep` (live grep), `lazygit` (git
  takeover), and `yazi` (file picker). `emmet-ls` is folded into
  every markup-flavoured language bundle (HTML, CSS, TS/JS, Vue,
  Svelte, Astro, Razor) so picking any of them installs Emmet once
  via the dedupe step — there's no separate "Emmet" checkbox to
  remember. The tool detects which package managers are on `$PATH`
  (`brew`, `apt-get`, `npm`, `cargo`, `rustup`, `go`, `pipx`, `pip`,
  `gem`, `dotnet`, `nix`, `composer`), dedupes shared tools across
  the selection (`prettier`, `lldb-dap`, `vscode-langservers-
  extracted`, `emmet-ls`, …), picks the first runnable installer
  per tool from a candidate list, shows the plan with the bundles
  each tool is needed by, and shells out only after a `[y/N]`
  confirmation. Already-present binaries are skipped; tools that
  can't be auto-installed (`netcoredbg`, OmniSharp) print manual
  instructions. Catppuccin Mocha palette + the same ASCII banner
  the editor's start page uses. Ships in the release tarball from
  this version on — `install.sh` extracts it next to `binvim`,
  Homebrew picks it up automatically via `cargo install`'s default
  all-binaries behaviour.
  Node.js version handling: any plan that includes one or more
  `npm install -g` steps triggers a second multi-select prompt
  listing every Node.js install we can find on the system — nvm
  (`~/.nvm/versions/node/*`), fnm (`~/.local/share/fnm/...` and
  `~/.fnm/...`), asdf (`~/.asdf/installs/nodejs/*`), mise
  (`~/.local/share/mise/installs/node/*`), volta
  (`~/.volta/tools/image/node/*`), n (`/usr/local/n/versions/node/*`),
  plus the `npm` on `$PATH` deduped via canonicalize. Single match
  → no prompt, just use it. Multiple matches → checkbox list with
  the newest version pre-checked; multi-select means npm installs
  loop over each chosen version. The version's `bin/` is prepended
  to PATH per spawn so the npm script's `#!/usr/bin/env node`
  shebang resolves to the matching node binary regardless of which
  one the host shell has active. For npm-installable tools the
  on-PATH skip is bypassed (the binary on PATH belongs to one
  Node version only — the user may have picked others).

- **CI: `cargo fmt --check` gate.** New `rustfmt.toml` at the
  repo root pins the style policy — `max_width = 100` +
  `single_line_let_else_max_width = 100` so the compact
  `let Some(x) = … else { return; };` and single-line method
  chains binvim leans on survive the formatter. The tree was
  normalised once with `cargo fmt` (73 files, ~5k line edits,
  no behaviour change); the CI job keeps it that way going
  forward. Run `cargo fmt` before pushing or the gate fails.

### Changed
- **Install catalog now pins versions matching binvim.dev.** Every
  `npm install -g …` / `go install …` / `cargo install …` /
  `pipx install …` / `gem install …` / `dotnet tool install …` /
  `composer global require …` step in the catalog now carries the
  same version pin shown on the binvim.dev install table — bumping
  a pin here keeps the CLI installer, the in-editor `:install`
  overlay, and the web table in sync. Brew / nix / apt formulas
  aren't pinned in the command (their package manager owns the
  version). `dlv`, `debugpy`, and `lazygit` stay un-pinned because
  binvim-web doesn't track them. The `Installer::Gem` and
  `Installer::DotnetTool` variants gained an `Option<&'static str>`
  version field so the `-v <v>` / `--version <v>` flags don't have
  to be encoded into the package name.

- Tree-wide `cargo fmt` pass under the new `rustfmt.toml`.
  Mostly: rustfmt collapsing multi-line method chains that fit
  on one line, expanding a handful of long-lined `if/else`
  blocks into block form, and re-ordering `use {json, Value}`
  imports alphabetically. No behaviour change.

## [0.4.3] - 2026-05-19

### Added
- **Refactor preview v2 — opt-in for code-action `WorkspaceEdit`s
  and server-initiated `workspace/applyEdit`.** New config flag
  `[lsp] preview_workspace_edits = true` (default `false`) routes
  both flows through the same modal overlay rename already uses.
  Rename itself is unaffected — it always uses the overlay. The
  preview's title bar reflects the source ("Apply: <action title>"
  for code actions, "Apply: <client>" for server-initiated). For
  server-initiated `workspace/applyEdit`, the LSP client is left
  blocking on a response — accept replies `applied: true`, cancel
  replies `applied: false`, no preview / empty-edit replies
  `false`. A second concurrent request while a preview is already
  open is auto-rejected so the server doesn't hang.
- **Task runner v2 — quickfix scrape on exit + long-running
  annotation.** When a task-spawned terminal tab's child process
  exits, the visible grid + scrollback is scraped for compiler /
  linter errors (`path:line:col[:end_col]: <msg>` covering
  gcc / clang / rustc / ruff / biome / eslint, plus tsc's
  `path(line,col):` legacy form) and the matches replace the
  quickfix list. `]q` / `[q` then walks the errors as usual. Log
  lines like `12:34:56 INFO: …` and rustc's `-->` decoration are
  filtered. Long-running tasks (label containing `dev` / `watch` /
  `serve` / `start` / `preview` as a word token) carry a `[bg]`
  badge in the picker; `:tasklast` on one of them adds a
  cautionary status hint about the previous instance possibly
  still being alive.

### Changed
- `Terminal` gained `poll_exit()` (first-time-only exit detection
  via `try_wait`) and `has_exited()`. `Grid` gained
  `text_lines()` that flattens scrollback + visible into trimmed
  `Vec<String>` for downstream parsers.
- `RenamePreview`'s `original` + `new_name` fields collapsed into
  a `PreviewKind` enum (Rename / CodeAction /
  ApplyEditFromServer); renderer + accept handler match on the
  variant for title + status formatting.

## [0.4.2] - 2026-05-19

### Added
- **Property tests for `motion` / `text_object` via proptest.**
  New `[dev-dependencies] proptest = "1"`. The bottom of
  `src/motion.rs` and `src/text_object.rs` carries a property
  block: every motion lands in bounds (`col <= line_len` — exclusive
  motions like `dw` legitimately sit one past the last char),
  `left + right` round-trips when there's room, `word_forward` is
  non-retreating in linear position and `word_backward` is
  non-advancing, `goto_line` clamps to the last line, `find_char`
  stays on the cursor's line. For text-objects: `compute(verb)`
  always returns `start <= end <= total_chars`, and the around-form
  range always contains the inner-form range for word / quotes /
  pair. Catches off-by-one regressions that the named-case unit
  tests miss.
- **Panic-hardening fuzz pass for tree-sitter + LSP shape
  extractors.** Proptest-driven rather than libFuzzer — stays on
  stable, runs in `cargo test`, fits the existing CI gate.
  `compute_byte_colors` is fuzzed against arbitrary UTF-8 across
  every `Lang` variant binvim ships (catches a future ABI bump or
  query edit that emits a capture range past `source.len()`). Every
  `parse_*_response` in `lsp/parse.rs` plus the three `extract_*`
  initialize-response inspectors and `parse_publish_diagnostics`
  in `lsp/io.rs` get a recursive arbitrary-`Value` JSON generator;
  a malformed reply must produce empty / None, never panic the
  reader thread.

### Changed
- `Buffer` now derives `Debug` (required for proptest shrinking
  output on failures involving buffers).

## [0.4.1] - 2026-05-19

### Added
- **Lazygit integration (`:lazygit` / `:lg` / `<leader>gg`).**
  Yazi-style full-screen takeover — suspend the editor, hand the
  host terminal to `lazygit`, and on exit reclaim the terminal +
  refresh `refresh_all_git_hunks()` across every open buffer so
  stages / commits / checkouts show up in the gutter immediately.
  Not a PTY-embedded pane: lazygit gets the whole screen (its UI
  hard-codes panel widths and the bottom `:terminal` pane caps at
  20 rows). Exit detection is free — when the blocking `status()`
  returns, lazygit is done. `<leader>g` is now a git sub-leader;
  grep (formerly `<leader>g`) moved to `<leader>G`.
- **Integrated task runner (`:task` / `:tasklast` / `<leader>m{m,l}`).**
  Discovers workspace tasks from five sources, all unioned per
  workspace: **npm scripts** (npm / pnpm / yarn auto-picked from
  the lockfile), **Justfile** recipes (skips `_private` +
  `[private]`), **cargo aliases** + builtin verbs (`build` /
  `check` / `test` / `clippy` / `run` / `fmt` / `doc`), **Makefile**
  top-level targets, and **dotnet** verbs (`build` / `run` / `test`
  / `restore` / `clean` / `publish`). Picker rows tag the source
  for disambiguation; selecting a task spawns it in a fresh
  bottom-terminal tab labelled with the task name. `Terminal`
  gained a `label: Option<String>` field so the tab strip shows
  `[ build ]` / `[ dev ]` instead of `[ 1 ]` / `[ 2 ]` for
  task-spawned tabs.
- **LSP rename preview (`<leader>r`).** Modal overlay between the
  server's `WorkspaceEdit` reply and the on-disk apply. Layout:
  file headers (path + edit count) with selectable rows below,
  scrollable with `j`/`k` + `Ctrl-D`/`U` + `g`/`G`. Keys:
  `<Space>` toggle, `a`/`n` flip all on/off, `o` jump to edit site
  (cancels), `<Enter>` apply enabled, `<Esc>` cancel. Split
  `apply_workspace_edit` into `parse_workspace_edit`
  (JSON → typed `ConcreteEdit`) + `apply_concrete_edits` (writer);
  code-action and `workspace/applyEdit` paths still apply blind.
  New `Mode::RenamePreview` for strict modality. File contents
  read once per affected file when the overlay opens, cached for
  the lifetime of the overlay.
- **Conditional + hit-count breakpoints (`:dapb if/hit/plain`).**
  `:dapb if <expr>` attaches a `condition` (creates an
  unconditional breakpoint first if none exists); `:dapb hit
  <expr>` attaches a `hitCondition` (DAP-style: bare integer for
  "pause after N hits", comparators like `>= 5`); `:dapb plain`
  strips both fields. Aliases: `cond` / `condition` / `hitcount`
  / `clear`. Conditional breakpoints render as `◆` in the gutter
  (plain stays `●`); the breakpoints pane lists each row's
  expression inline. New `DapManager` API: `breakpoint_at`,
  `set_breakpoint_condition`, `set_breakpoint_hit_condition`,
  `strip_breakpoint_conditions`.
- **Multi-root LSP workspaces (`:workspaces` / `:ws`).** Opening
  files from sibling project roots no longer spawns a second
  language server — `ensure_for_path` now checks the running
  client's `workspace_folders` set and fires
  `workspace/didChangeWorkspaceFolders` with the new folder if
  the server advertised the capability. rust-analyzer / tsserver
  / gopls / jdtls all support this; servers that don't fall back
  to the previous "first root wins" behaviour cleanly. New
  `:workspaces` ex command dumps `key: ~/path  +  ~/path · key:
  ~/path` to the status line so the multi-root state is
  observable.
- **AI side pane: shift-pair leader bindings + path handoff.**
  Each AI tool now has two leader bindings: lowercase opens a
  fresh tab as-is, uppercase opens AND pre-types
  `@<cwd-relative path>` into the input field once it's ready.
  `<leader>jc` / `<leader>jC` for Claude, `<leader>jx` /
  `<leader>jX` for Codex, `<leader>jo` / `<leader>jO` for
  opencode. Per-tool quiet windows (~300ms / ~800ms / ~1500ms)
  before the path write so the front of the string doesn't get
  eaten during the tool's input-field initialisation. You press
  Enter to submit — auto-submit was attempted and dropped
  because no single timing made `\r` register as a discrete
  keypress across all three tools.
- **AI side pane: dedicated focus / toggle bindings (`<leader>jf`
  / `<leader>jp`).** Mirrors `<leader>tf` / `<leader>tp` for the
  bottom pane. `<leader>j{c,x,o}` (and their handoff variants)
  always spawn a fresh tab now — the previous dedup-by-label
  re-focus path is gone, replaced by the explicit focus binding.

### Changed
- **Side-pane mouse forwarding.** Scroll wheel (and drag /
  right-click) inside the `:claude` / `:codex` / `:opencode` pane
  now forwards to the PTY when the embedded program has enabled
  DECSET 1000 / 1002 / 1003 / 1006 mouse tracking. Previously the
  side-pane mouse handler only had header tab clicks and
  focus-on-click; scroll events fell into the swallow arm and
  never reached the tool. Extracted `encode_mouse_event_for_pty`
  out of the bottom-pane handler so both panes share the encoder.
- **File tree folder icons.** Now render in Catppuccin Mocha blue
  (`#89b4fa`) instead of mauve. The mauve was inheriting from
  the `keyword` syntax-capture colour, which competed visually
  with the warmer terminal-chip / breakpoint / dirty-dot accents.
  Override via `[colors] "file_tree.folder"`.
- **Side-pane loading splash.** Now shows a small box-drawing
  robot head (`╔═══════╗ / ║ ◉ ─ ◉ ║ / ╚═══════╝`) instead of
  the binvim wordmark. The pane is about to host Claude / Codex /
  opencode — claiming editor identity on its boot splash was
  off-key. Multi-variant width fallback also dropped (the head
  fits every pane width we ever render at).
- **File-tree delete confirm is a popup, not a status-line
  notification.** Matches the chrome of the `a` (create) and `r`
  (rename) prompts so the three file-tree ops feel uniform. Title
  `Delete`, body shows `! <name>/  y to delete · N / Esc to
  cancel` with the prompt glyph in the error accent so the
  destructive intent reads at a glance.
- **Cmdline cursor positioning + arrow navigation.** The cmdline
  model was "string with cursor always at end" — Backspace popped
  the last char, typing appended. Now it tracks a real
  `cmdline_cursor` (byte offset, UTF-8-safe) and supports `<Left>`
  / `<Right>` / `<Home>` / `<End>` / `<Delete>`, plus inserts /
  Backspace at the cursor position. Applies across all cmdline-
  style prompts (Command `:`, Search `/` `?`, LSP rename,
  ReplaceAll, file-tree create / rename).
- **Painted cursor cell in cmdline popups.** Some terminals dropped
  the system cursor's visibility after a `SetCursorStyle` round-
  trip inside a synchronized update; the popups now render the
  cursor as a single inverted cell inside the body row so it's
  visible regardless of terminal cursor state. System cursor is
  hidden while these popups are up.

### Fixed
- **File-tree create / rename prompts had no title.** The
  `cmdline_chrome` match in the renderer had arms for `Rename` and
  `ReplaceAll` but the two file-tree prompt kinds fell through to
  the empty default, leaving the popup's title slot blank. Added
  explicit `New entry` / `Rename` titles.
- **DAP `setBreakpoints` resend dropping conditional fields.**
  The post-toggle resend path built `{"line": N}` inline, silently
  dropping `condition` and `hitCondition`. Any conditional set
  before a toggle reverted to a plain breakpoint on the next
  adapter sync. Extracted `encode_source_breakpoint` so both the
  initial `configurationDone` path and the post-toggle resend
  share the same encoder.

## [0.4.0] - 2026-05-18

### Added
- **Debug test.** `:debugtest` (alias `:dt`) walks up from the
  cursor for the enclosing test function, then routes through the
  DAP layer instead of the test runner. `LaunchContext` gained
  two fields — `test_filter` (the name) and `test_file` (the
  source path) — which the per-adapter `build_launch_args`
  consults to emit a test-mode invocation. Wired for pytest
  (`module: pytest`, `args: [<file>::<test>, -s]`) and go (delve
  `mode: test`, `args: ["-test.run", "^<name>$", "-test.v"]`).
  cargo / dotnet / vitest surface a "not yet supported" status —
  the wire path is in place, per-adapter test-binary discovery is
  the remaining work.
- **Spell check.** `:spell` toggles spell-check on the active
  buffer; `]s` / `[s` walk between misspelled words; `z=` opens a
  suggestion picker for the word under the cursor (single-edit
  neighbours filtered against the dictionary, capped at 12). No
  external library — the wordlist loads from
  `~/.local/share/binvim/words` (user override) or
  `/usr/share/dict/words` (system default). The tokeniser splits
  camelCase / snake_case / kebab-case so identifiers only trip on
  unknown constituents; pure-uppercase abbreviations and tokens
  under 3 chars are skipped. Per-buffer enable flag, version-keyed
  cache.
- **Test adapters for pytest, go test, and dotnet test.** Three
  new `TestAdapterSpec` entries in `BUILTIN_ADAPTERS` alongside
  cargo / vitest, each with its own sibling parser module:
  - `src/test/pytest.rs` — root markers `pytest.ini`,
    `pyproject.toml`, `setup.cfg`, `tox.ini`, `conftest.py`. Runs
    `pytest -v --tb=line --color=no`; the streaming verdict comes
    from `path::test_name PASSED / FAILED / SKIPPED` rows; failure
    locations + messages come from the `--tb=line` row and the
    `FAILED path::test - …` short-summary block.
  - `src/test/gotest.rs` — root marker `go.mod`. Runs `go test -v
    -run ^<name>$ ./...` (or a positional `./pkg/...` filter); the
    parser pairs `=== RUN` / `--- PASS/FAIL/SKIP` and harvests the
    indented `    foo_test.go:14: msg` line for failure location.
    Subtest paths (`TestParent/case_one`) stay intact for re-run.
  - `src/test/dotnet.rs` — root markers `*.sln`, `*.csproj`,
    `*.fsproj`. Runs `dotnet test
    --logger:"console;verbosity=normal"`; per-test verdicts are
    `Passed/Failed/Skipped FQN [Nms]`. `Error Message:` blocks fold
    into the failure message; `Stack Trace:` `in <path>:line N`
    rows feed the location. `FullyQualifiedName~<name>` filter by
    default; raw `--filter` expressions pass through verbatim.
- **File-tree create / delete / rename.** Inside the sidebar tree
  pane (`[file_explorer] tree = true`): `a` creates an entry under
  the cursor's parent directory — trailing `/` makes a folder, any
  intermediate dirs are auto-created, and inputs containing `..`
  or starting with `/` are refused so stray edits stay inside the
  project. `r` renames the cursor entry with a basename pre-filled
  prompt; if the renamed file is open in a buffer, the buffer's
  path is rewritten so saves keep landing in the right file. `d`
  arms a delete and the next key consumes the y/N confirmation —
  any non-`y` cancels (so a double-`d` doesn't unlink). `R` keeps
  the rebuild action; `r` moved to rename. Errors (already exists,
  permission denied, …) surface through the status line.
- **Large-file mode.** `Buffer::is_large()` trips when the rope
  crosses 5MB (`LARGE_FILE_BYTES`) or 50k lines
  (`LARGE_FILE_LINES`). The gate short-circuits
  `ensure_highlights` (tree-sitter never runs), `lsp_attach_active`,
  `lsp_sync_active`, and `lsp_sync_active_debounced` (no server ever
  sees the file). Status-line hint fires on first open via the CLI
  or `:e`. Editing, scrolling, yank, and undo still work — only the
  syntax pass and LSP traffic are suppressed.
- **Tab completion inside `:` ex commands.** `Tab` / `Shift-Tab`
  cycle candidates in the cmdline. Three modes picked by the head:
  command names before the first space (every alias the parser
  knows, filtered by prefix); filesystem entries after `:e` /
  `:edit` / `:w` / `:write` (directories get a trailing `/`,
  dotfiles hidden unless the basename starts with `.`); open-buffer
  basenames after `:b` / `:buffer`. Any non-Tab key (typing,
  Backspace, history walk) drops the cycle so the next `Tab`
  re-derives candidates against the latest cmdline text.
- **Built-in sidebar tree file explorer.** Opt in via
  `[file_explorer] tree = true` in `~/.config/binvim/config.toml`
  (default `false` keeps the existing yazi shell-out). When
  enabled, `<leader>e` toggles a left-side tree pane rooted at
  the cwd in place of yazi. `j` / `k` / arrows navigate, `Enter`
  or `l` opens a file (or expands a folder), `h` collapses (or
  jumps to the parent), `g` / `G` top / bottom, `r` rebuilds
  after external file changes, `<space>e` from inside the pane
  closes it. Three-state `<leader>e` toggle from the editor:
  closed → focused → unfocused-but-visible → closed, so clicking
  into a buffer drops focus without losing the pane and
  `<leader>e` pulls focus back. Two row styles: a `theme_surface`
  bg highlight follows the j/k cursor; the file currently open
  in the focused editor window renders its name in the accent
  colour + bold so it stays identifiable when the cursor moves
  elsewhere. Path icons use the same `icon_for_basename` helper
  the picker uses (per-language Nerd Font glyph + generic
  fallback); folders use `\u{f07b}` / `\u{f07c}` (closed / open).
  Click in the pane focuses + moves the cursor; a second click
  on the same entry inside the editor's 350ms double-click
  window opens the file (same as Enter). Leader-popup label
  renamed `Yazi` → `File explorer` so it reads correctly
  regardless of which mode is enabled.

### Changed
- **Powerline lang chip wedge: bg/fg swap.** The right-edge
  `\u{e0b2}` glyph between the path segment and the language
  chip had its background / foreground flipped, so the triangle
  was filled with the dark path colour on the right half of the
  cell — reading as a dark block butted up against the chip
  rather than a slanted divider. The wedge cell now takes the
  path colour as bg (left half) and the chip colour as fg (right
  half), tapering cleanly between segments. Same pattern the
  left-side mode → branch → path transitions use.

### Fixed
- **Click past EOL in Insert mode parks the cursor at end of
  line.** `visual_col_to_char_col` was clamping every past-EOL
  click to `line_len - 1` so the cursor sat on the last char
  instead of after it — correct for Normal / Visual (cursor sits
  *on* a character) but wrong for Insert (cursor sits *between*
  characters, can be at `line_len`). The mapper is now mode-
  aware: Insert allows past-EOL, Normal / Visual still snap to
  the last char.
- **Cursor placement and horizontal scroll respect the active
  pane's width.** `adjust_viewport` and `adjust_viewport_to`
  were computing the horizontal scroll budget against
  `self.width` (the full terminal width), not the active pane's
  width — so when the left tree pane (or the right AI pane) was
  open, the cursor could slip off the editor pane's right edge
  before view_left bumped. Both now use
  `active_pane_rect().w - gutter`.
- **Buffer click → cursor mapping respects the active pane's
  left offset.** The buffer-area mouse handler was passing the
  raw screen `col` straight into the gutter check and
  `visual_col` math. With the tree pane open, a click on the
  first character of a line landed at column `tree_width`
  cells past EOL — the past-EOL clamp then snapped the cursor
  to the last char. The click is now translated to a pane-local
  column (`col - active_pane_rect().x`) before either check;
  same translation applied to the code-lens click hit-test.

## [0.3.2] - 2026-05-17

### Added
- **Monokai theme.** Classic Wimer Hazenberg palette (`#272822`
  background, pink keywords, green functions, cyan types, purple
  constants, yellow strings, orange parameters). Brings the
  bundled-theme count to 15.
- **Namespaced `[colors]` keys for fine-grained control.** Every
  themable role has both a broad key and an optional namespaced
  override. Set the broad one to retint a whole class; set the
  namespaced one to single out one surface.
  - `notification.{info,warning,success,error}` → fall back to
    `info` / `warning` / `accent_secondary` / `error`
  - `git.{added,modified,deleted}` → fall back to
    `accent_secondary` / `warning` / `error`
  - `diagnostic.{error,warning,info,hint}` → fall back to the
    matching severity key
  - `gutter.{breakpoint,pc_marker}` → fall back to `error` / `accent`
  - `tab.{active_bg,active_fg,inactive_fg,dirty,close}` → fall
    back to `surface` / `emphasis` / `dim` / `accent` / `dim`
  - `terminal.{chip_bg,chip_fg,active_tab_bg}` → fall back to
    `accent_secondary` / `chip_fg` / `accent`
  - `debug.{chip_bg,active_tab_bg}` → fall back to `accent` /
    `accent_secondary`
  - `mode.{normal,insert,visual,command,search,picker,prompt,terminal,debug}`
    → drive the status-line mode chip per mode
  - `search.highlight_bg`, `yank.flash_bg`, `multi_cursor.bg`,
    `match_pair.bg`, `doc_highlight.bg` → buffer overlays
- **All 14 bundled themes populated with the chrome palette.**
  Each `themes/<name>/theme.toml` now ships explicit values for
  `foreground`, `dim`, `emphasis`, `surface`, `border`, `accent`,
  `accent_secondary`, `chip_fg`, `error`, `warning`, `info`, `hint`
  drawn from that theme's canonical palette. Switching theme now
  flips every chrome surface to the theme's own tones — no more
  Catppuccin tint leaking through Dracula / Tokyo Night / Light Owl.

### Changed
- **Chrome bg always differs from the buffer bg.** Tabs and popups
  (whichkey, hover, signature, notification, floating cmdline,
  picker, completion, terminal/debug headers) used to share the
  buffer's background when only `background` was set, so chrome
  visually merged with the editor surface. `chrome_bg` now derives
  by mixing the configured background toward black (15% on dark
  themes, 5% on light) — the same Mantle/Base relationship
  Catppuccin uses — and `chrome_bg = "#…"` can override it
  explicitly per theme.
- **One-line themes now produce coherent chrome.** When only
  `background` is set in `[colors]`, `surface`, `border`,
  `foreground`, and `dim` auto-derive from it via luminance-
  aware mixing toward white (dark bg) or black (light bg). So
  `[colors] background = "#1e1e2e"` alone now yields a fully
  consistent dark UI; `background = "#fbfbfb"` yields a fully
  consistent light UI — no more Catppuccin Surface1 active-tab
  bg leaking through a light theme. Setting any of the four
  neutrals explicitly still wins.

### Fixed
- **Debug pane: no buffer leak between tab labels.** The 1-col
  gap between Console / Locals / Frames / Watches / Breakpoints
  tabs was advancing the cursor without painting that column,
  so git-stripe glyphs from the buffer beneath leaked through
  on every frame. Paint the gap with `pane_bg` (same pattern
  the terminal pane already uses).

### Added
- **Full chrome palette in `[colors]`.** Twelve new keys in
  `~/.config/binvim/config.toml` (or any `themes/<name>/theme.toml`)
  drive every chrome colour the editor paints. They fall back to
  Catppuccin Mocha defaults when unset, so existing setups don't
  change.
  - `background` — buffer body + every chrome surface
  - `foreground` — main text on chrome
  - `dim` — muted text (line numbers, hints, comments)
  - `emphasis` — active tab fg, multi-cursor block, picker title
  - `surface` — layered chrome (active tab bg, picker selection)
  - `border` — popup borders, dividers, doc-highlight bg
  - `accent` — debug-pane chip, breakpoint, dirty-tab marker
  - `accent_secondary` — terminal chip, active debug sub-tab, git added
  - `chip_fg` — fg on coloured chips
  - `error` / `warning` / `info` / `hint` — diagnostics + git stripe
  See `themes/catppuccin-mocha/theme.toml` for an annotated example.
  Tab bar, terminal pane, debug pane (header + DAP rows), status
  line, gutter signs, severity glyphs, search/yank/multi-cursor
  flashes, notifications, popups, dashboard, and the `:health`
  banner all read from these keys now — switch themes and the
  whole UI follows.
- **Multiple terminals (tabs).** The `:terminal` pane now hosts
  more than one PTY at a time. `<leader>tt` (or `:terminal`)
  always spawns a new tab — first invocation opens the pane; each
  subsequent one appends another shell. With one terminal the
  header keeps its hint line; with two or more it sprouts a
  clickable tab strip (active tab = peach bg + base text;
  inactive = muted on the pane bg). Each tab owns its own PTY +
  10k scrollback + cursor state. Every tab keeps draining on
  every frame, so `pnpm dev` / `cargo watch` / a long build don't
  stall while focus is on a sibling tab. Host-terminal resize
  broadcasts to every PTY so background tabs don't reflow on
  switch. `<leader>tq` closes the active tab; the pane auto-
  hides when the last tab is dropped. Click a tab label in the
  header to switch.

### Changed
- **`:q` from the editor quits — even with terminals open.** The
  old branch order treated an open terminal pane as "close it
  before quitting," so `:q` from the editor with N terminal tabs
  took N+1 invocations to actually quit. Now `:q` drops every
  terminal in one go and exits; each Terminal-drop releases the
  master PTY fd, which signals SIGHUP to the child group so
  background `pnpm dev` / `cargo watch` / SSH sessions don't
  orphan on the OS. `:q!` and `:wq` do the same.

### Fixed
- **No more blank-line gap between prompts in the second tab.**
  `cmd_open_terminal` was firing a redundant `resize_all_terminals`
  after `Terminal::spawn`. The new PTY was already opened at the
  pane body's size and existing tabs hadn't lost any rows, so the
  resize was a no-op AT BEST — but it hit zsh + starship before
  they finished their startup sequence, and they reacted by
  emitting extra clearing escapes that landed as a blank row
  between prompts in the freshly-spawned tab's grid. Dropped the
  call; resize still fires from toggle-show and the host-resize
  event handler where it's actually needed.
- **`<leader>tp` no longer kills the running process.** Toggle now
  hides/shows the pane while keeping the PTY alive — `pnpm dev`,
  `cargo watch`, a long-running REPL session, etc. survive being
  tucked away and brought back. Only `<leader>tq` drops the
  active tab; the PTY is resized to the current pane dimensions
  on re-show so a host-terminal resize while hidden doesn't leave
  the shell with the old `winsize`.

## [0.3.1] - 2026-05-17

### Added
- **`<leader>tp` toggles the terminal pane.** Mirrors `<leader>dp`
  for the debug pane — open + focus if no terminal is alive, close
  if one is. The existing `<leader>tt` ("open / focus") stays as
  the open-and-keep-open verb.
- **Horizontal scrolling in the debug pane.** Long rows (deep
  stack-frame paths, expanded structured locals, log lines that
  outgrow the pane width) now scroll horizontally. `h`/`l`,
  `Shift-←`/`Shift-→`, or `Shift+ScrollWheel` shift one column
  (capital / shift variants jump by 10); `0` snaps back to column
  0. A muted `«` replaces the leading pad once scrolled right, and
  the rightmost cell becomes `»` when there's content beyond the
  pane edge. Per-tab — each of Console / Locals / Frames / Watches
  / Breakpoints remembers its own offset. Resets on `stopped` and
  on session start. Console mouse selection (drag-select + Cmd-
  click on URLs) accounts for the offset when mapping screen col
  back to char col, so selecting hidden text after scrolling right
  still produces the correct clipboard payload.
- **Crash handler.** A global `panic::set_hook` installed before any
  terminal-touching code best-effort restores the terminal (disable
  raw mode, leave alt screen, show cursor, drop kitty keyboard flags)
  and writes a diagnostic log with payload + location + force-captured
  backtrace + binvim version + unix timestamp to
  `~/.cache/binvim/crash/<ts>.log`. Path is echoed to stderr after the
  unwind so the user knows where to look. No more "panic leaves the
  terminal stuck in raw mode" failure mode.
- **CI.** `cargo test --locked` and `cargo clippy --locked
  --all-targets` run on every push to main and every PR via
  `.github/workflows/ci.yml`. Concurrent runs cancel on rapid pushes
  to the same ref. `cargo fmt --check` deliberately skipped — codebase
  has hand-rolled formatting (compact let-else, single-line method
  chains) that stock rustfmt would rewrite across ~560 places; needs a
  conscious style-policy decision before turning the gate on.
- **macOS prebuilt binaries.** `release.yml` matrix gains
  `aarch64-apple-darwin` (macos-14 runner) and `x86_64-apple-darwin`
  (macos-13). Targets build natively per arch — no cross-compile
  toolchain to wrangle, and the resulting binary picks up the host's
  codesigning so Gatekeeper doesn't trip on first launch. Homebrew
  first install drops from minutes (compile from source) to seconds.
  `install.sh` now resolves Darwin/{arm64,x86_64} so the `curl … | sh`
  path works on Mac too.
- **`:terminal` bottom split.** Earlier in this cycle the terminal
  shipped as a fullscreen overlay; promoted now to a real bottom-
  split pane that stacks above the debug pane and below the editor.
  Two paired modes:
  - `Mode::Terminal` forwards every keystroke to the PTY (xterm
    sequences for arrows / F-keys / Page / Home / End / Delete /
    Insert / Tab; Ctrl-letter → C0; Alt-prefix for Meta).
  - `Mode::TerminalNormal` (`Esc` from Terminal) — Vim-style
    grid navigation + selection:  `h`/`j`/`k`/`l` (and arrows)
    move a reading-cursor, `0`/`$`/`g`/`G` move to row / col
    extremes, `v` toggles Visual selection, `y` yanks the selected
    region to the unnamed register and the OS clipboard (multi-row
    selections produce one `\n`-separated string with trailing
    whitespace per row trimmed), `Y` yanks the current row, `i`/`a`
    re-enters Terminal, `<C-w>q` (or `:q`) closes the pane.
  - **Mouse forwarding** when the inner program enables DECSET
    mouse tracking (1000 button-only, 1002 button+drag, 1003
    any-motion, 1006 SGR encoding). htop / less mouse mode / vim
    mouse=a / lazygit all get clicks + drag + scroll forwarded
    with the appropriate xterm escape. When mouse tracking is
    off, clicking the pane just pulls focus into Terminal mode.
    Both SGR (modern, unlimited coords) and legacy X10 (button
    state byte + coord bytes offset by 32) encodings are
    implemented.
- **Terminal model (PTY + vte parser + grid + scrollback).** New
  self-contained `terminal` module — landed earlier in this cycle
  as scaffolding for the `:terminal` overlay above. PTY spawn via `portable-pty`,
  reader thread funnels bytes into an mpsc channel, vte 0.15 parses
  escape sequences and mutates a `Cell` grid + cursor + pen state.
  Covers CUP / CUF / CUB / CUU / CUD / CHA, ED / EL, SGR (basic +
  bright + 256-colour + truecolor RGB + bold / italic / underline /
  reverse), IND / RI / DECSC / DECRC / RIS, line wrap with bounded
  scrollback (10k rows). 12 tests (10 pure-model, 2 end-to-end against
  /bin/sh).
- **Watch expressions (DAP).** User-managed list evaluated against the
  top frame on every `stopped` event via DAP `evaluate`. Add via
  `:dapwatch <expr>`, remove via `:dapunwatch <n>` / `:dapunwatch
  all`, dump current state to status line via `:dapwatches`. Renders
  above the frame list in the debug pane; failed evaluations
  (typo / name-not-in-scope) show in red instead of the default
  text colour. Survives across sessions — the list lives on
  `DapManager`; only the cached `result` clears at session start /
  every stop.

## [0.3.0] - 2026-05-16

### Added
- **LSP server stderr capture.** Each spawned LSP's `stderr` was
  previously routed to `Stdio::null()` — any panic backtrace,
  capability error, or wrapper-binary complaint disappeared. Stderr
  is now piped and forwarded into the same channel as protocol
  messages as a synthetic `LspIncoming::ServerMessage` with
  severity=Log, surfacing via `:messages` alongside real
  `window/showMessage` / `window/logMessage` notifications. Makes
  the common "LSP running but not responding" failure mode (e.g.
  the rustup proxy refusing to invoke a missing `rust-analyzer`
  component) actually diagnosable.
- **`:health` `NOT INITIALIZED` chip + cache counters.** The LSP
  SERVERS section flags any client still in `InitState::Buffering`
  after startup with a red `NOT INITIALIZED` chip plus a peach hint
  pointing at `:messages` — the chip is what you see when the
  binary exists and "runs" but never answers `initialize`, so
  requests pile up in the init queue forever. Per-kind pending
  breakdown ("8× SemanticTokens stuck") replaces the flat pending
  count. ACTIVE BUFFER section gains a `doc-hi: N cached · sem-tok:
  M cached` row so you can tell at a glance whether the LSP is
  producing data vs the renderer being broken.
- **One-in-flight cap for inlay-hint / semantic-token /
  documentHighlight requests per buffer path.** Previously the
  throttles dedupe by `(line, col, version)` or `version` — fast
  cursor navigation or rapid typing against a slow / cold-indexing
  server queued hundreds of requests the server hadn't gotten to
  yet. Now each kind has an `in_flight: HashSet<PathBuf>` on App;
  intermediate cursor positions during the in-flight window are
  skipped, and the next render after the response fires for
  wherever the cursor has settled. `LspEvent::RequestFailed { kind,
  path }` from ErrorReply frees the slot even when the server says
  no, so a failing request can't leak the slot forever.
- **`[lsp]` config block toggles for semantic tokens + document
  highlight.** Both default `true`. Set `semantic_tokens = false` /
  `document_highlight = false` under a `[lsp]` block in
  `~/.config/binvim/config.toml` to gate the requests off entirely —
  the manager won't fire them and the renderer won't paint anything.
- **Visible defaults for semantic-token modifiers.** Plain
  `function` / `variable` / `keyword` map to the same colour as the
  tree-sitter pass and produce no visible delta. The payoff comes
  from modifiers: `function.async` (Lavender), `variable.mutable`
  (Red — Rust `let mut`), `function.defaultLibrary` (Sapphire —
  `std::` symbols), `variable.static` (Teal), `*.readonly` (Peach),
  `*.deprecated` (Red). `default_capture_color` is the lookup table;
  the resolver's rightmost-first dotted walk picks the modifier hit
  over the base type, so a `let mut foo` lights up in red without
  any user config.
- **DocumentHighlight bg bumped to Surface2.** Surface1 was a 2-3%
  delta vs editor background — invisible in practice. Surface2
  matches the bracket-pair tone and reads clearly on Catppuccin
  Mocha without strobing.
- **Semantic tokens (`textDocument/semanticTokens/full`).** Layered on
  top of the tree-sitter highlight cache — LSP tokens win where they
  apply, tree-sitter fills in the rest. Captures the server's legend
  from the `initialize` response, decodes the bit-packed integer
  stream into per-line ranges, and bins them by line for
  constant-time lookup during the per-char paint. Token type +
  modifiers feed the existing `color_for_capture` dotted-prefix
  resolver (`function.async`, `variable.readonly`), so the same
  `[colors]` config drives both layers. Refreshed once per buffer
  version, same throttle shape as inlay hints. Full only — delta /
  range not yet implemented.
- **Document highlight (`textDocument/documentHighlight`).** Fires on
  cursor settle in Normal / Visual mode (silent behind pickers /
  completion); paints every occurrence of the symbol under the
  cursor with a Surface2 background so the syntax-coloured foreground
  still reads through. The cache stays valid as long as the cursor
  sits inside *any* of the returned ranges and the buffer version
  matches — moving by one column inside the identifier doesn't blink
  the highlights off and on between round-trips. Edits invalidate
  the cache; moving off the symbol clears it.
- **`window/showMessage` and `window/logMessage` capture +
  `:messages` overlay.** Notifications previously dropped on the
  floor now flow into a bounded 500-entry ring on the App.
  `showMessage` Error / Warning also fires through `status_msg` (so
  the user notices a server complaint at the moment it lands);
  `logMessage` is log-only. `:messages` opens a scrollable
  severity-coloured overlay — Esc / q / :q to dismiss, j / k / Ctrl-D
  / Ctrl-U / g / G to scroll — sharing the dismiss + scroll shape
  with `:health`. The reader thread snapshots the server's
  semantic-tokens legend at the same time, so the same code path now
  carries both features off `initialize`.
- **Three new DAP adapters: delve (Go), debugpy (Python), and lldb-dap
  (Rust).** The registry that shipped in 0.2.0 was structured for this —
  each adapter is one row in `BUILTIN_ADAPTERS` plus a launch-args
  builder. `:debug` / `<leader>ds` now picks the right adapter by
  walking up from the active buffer looking for `go.mod`, `Cargo.toml`,
  `pyproject.toml` / `setup.py` / `requirements.txt` / `Pipfile`, or
  any of the existing `.csproj` / `.sln` / `.fsproj` markers.
  - **Go** runs `dlv dap` on stdio. Discovery scans the workspace for
    directories containing `package main`; multiple mains open a
    picker (the buffer's own directory is preferred when it's one of
    them). `mode: debug` so delve builds + runs in one step — no
    prelaunch from binvim's side.
  - **Python** runs `python3 -m debugpy.adapter` (falls back to
    `python`). If the active buffer is a `.py` it launches that
    directly; otherwise it picks from `main.py` / `__main__.py` /
    `app.py` / `manage.py` / `run.py` / `server.py` / `cli.py` at the
    workspace root. `justMyCode: false` so step-into into third-party
    packages works.
  - **Rust (and C / C++)** runs `lldb-dap` (falls back to the legacy
    `lldb-vscode`). The Cargo.toml at the workspace root — plus any
    `[workspace].members` (incl. `crates/*` globs) — is parsed for
    `[[bin]]` entries, `src/main.rs`, and `src/bin/*.rs`. Each bin is
    one row in the picker; a single-bin crate auto-picks. Prelaunch is
    `cargo build --bin <name>`; the resulting binary in
    `target/debug/<name>` becomes the `launch.program`. `env` is
    serialised as the `["K=V", ...]` array shape lldb-dap expects
    (not the object form the other adapters take).
- **Generalised `DapAdapterSpec`.** `prelaunch` is now a function
  pointer that takes the resolved `LaunchContext` so per-target build
  commands (`cargo build --bin foo`) work without a separate codepath.
  `adapter_id` is a spec field instead of a hard-coded constant —
  netcoredbg keeps `coreclr`, debugpy gets `debugpy`, delve gets `go`,
  lldb-dap gets `lldb-dap`. Adding a fifth adapter is again a single
  row + a launch-args fn.

## [0.2.1] - 2026-05-15

### Fixed
- **Start page survives a history-only relaunch.** With the cmdline-
  history change keeping the per-cwd session file alive across a
  `<leader>bA` (so recall doesn't get wiped), the next bare `binvim`
  launch saw `saved_session.is_some()` and flipped
  `show_start_page = false`. `hydrate_from_session` then silently
  returned because `session.buffers` was empty, leaving the user on
  the bare `[No Name]` seed buffer (dismissable, with no buffers to
  go back to). `restore_buffers` now also requires `!session.buffers.is_empty()`,
  so a history-only session falls back to the start page exactly
  like a brand-new launch.
- **`H` / `L` on the start page no longer dismiss it when the only
  buffer is the empty seed.** `cycle_buffer` unconditionally set
  `show_start_page = false` before its "Only one buffer" early
  return, so pressing `L` on a fresh launch with no restored
  buffers dropped the user into the bare `[No Name]` slot. The
  dismissal now checks the seed-shape (no path + empty rope); a
  real lone buffer from a restored session or CLI arg still gets
  the original "press `L` to land on it" behaviour.

### Added
- **Cmdline (`:`) and search (`/` / `?`) history with `<Up>` / `<Down>`
  recall, persisted across sessions.** Each successful Enter records
  the line into a per-cwd ring (dedup against the previous entry,
  capped at 100). Inside the prompt, `<Up>` walks one step older and
  `<Down>` walks newer — the first `<Up>` snapshots whatever was
  already typed so walking past the most recent entry restores that
  draft instead of leaving an empty cmdline. The two histories are
  independent: `:` recall doesn't surface `/` queries and vice versa.
  Persistence rides on the existing per-cwd `~/.cache/binvim/sessions/<hash>.json`
  file — `serde(default)` keeps old session files parseable, and a
  `binvim foo.rs` launch now loads histories even though it skips
  buffer restoration, so `:` / `/` recall stays warm regardless of
  launch mode. The "no buffers left = clear the session file" rule
  was relaxed to "no buffers AND no history" so closing every buffer
  via `<leader>bA` no longer wipes recall.
- **`<C-w> [N] >` / `<` / `+` / `-` resize the active window by N
  cells.** Widens (`>`) or narrows (`<`) along the vertical axis;
  grows (`+`) or shrinks (`-`) the height. Count defaults to 1 when
  omitted (`<C-w>>` = +1 col), and the count goes between `<C-w>`
  and the resize key in Vim's positional order (`<C-w>10>`). Internally
  the parser accumulates digits while the window-leader prefix is
  pending; the layout walks the split tree to find the **deepest**
  ancestor of the focused pane whose axis matches, converts the
  cell-delta to a ratio against that subtree's own rect (so a 10-col
  widen in a 40-col half doesn't behave the same as in a 200-col
  one), and clamps to the existing `[0.1, 0.9]` visibility band so a
  pane can't vanish. No-op when the layout has no split along the
  requested axis (e.g. `<C-w>+` in a vertical-only column).
- **Inline `<script>` / `<style>` highlighting in HTML, Razor, and
  Svelte buffers.** Tree-sitter-html (and friends) parses the
  surrounding markup but leaves `<script>` / `<style>` contents as
  bare `raw_text` nodes — so CSS rules and JavaScript code inside
  them used to render as flat plain text. A new injection pass
  scans the byte stream for `<script>…</script>` and
  `<style>…</style>` regions, runs the appropriate sub-language
  through `compute_byte_colors` (CSS for `<style>`; JSON for
  `<script type="application/json">` / `application/ld+json`;
  JavaScript otherwise — handles `module`, `text/javascript`, and
  no-`type`), and splats the resulting colours back onto the main
  map. Works regardless of which outer grammar parsed the document
  (HTML / Razor / Svelte all benefit) and degrades gracefully on
  empty / self-closed / unterminated blocks.

## [0.2.0] - 2026-05-15

### Added
- **GitHub Copilot integration via `copilot-language-server`.** Opt
  in with `[copilot] enabled = true` in `~/.config/binvim/config.toml`
  (defaults to off). binvim attaches the official npm-distributed
  language server as an aux LSP on every buffer; no HTTP client
  lives in binvim itself — Node handles all networking + auth. Sign
  in is device-flow: on first launch the status line shows the
  verification URL + user code, status auto-polls every 3 s so the
  editor flips to "signed in as <user>" within seconds of you
  finishing in the browser. Inline ghost completions render as
  muted italic Overlay0 text after the cursor in Insert mode on a
  ~250 ms idle pause. `<Tab>` accepts (wins over the LSP popup
  when both are visible; popup auto-closes on accept), `<Enter>`
  accepts the LSP completion popup item, any other key dismisses
  the ghost. Accept paths are smart: the response's `range` is
  honoured so already-typed prefix isn't duplicated; trailing
  overlap with post-cursor text is trimmed so suggestions ending
  in `)` against an auto-paired `()` don't land as `))`; partial
  suggestions ending in `{` auto-open the block (`\n<indent>|\n}`)
  so the user lands inside the body; `didChange` is flushed before
  every inline request so Copilot never works from stale text; and
  the next request fires immediately after accept (no 250 ms wait)
  so chained partial suggestions feel live. New ex commands:
  `:copilot` (report status), `:copilot signin` (re-fire auth),
  `:copilot reload` (force a status refresh), `:copilot signout`.
- **`<leader>/` toggle line comments.** Normal mode toggles the
  current line; Visual mode toggles every line in the selection
  with the all-or-nothing rule (every non-blank line commented →
  uncomment all; any uncommented → comment all at the min-indent
  column so indented blocks stay aligned). Per-language prefixes
  via new `Lang::line_comment_prefix`: `//` for Rust / TS / JS /
  JSON / Go / C# / C / C++ / Java / PHP / Svelte / Zig / Kotlin /
  SQL; `#` for Python / Bash / Ruby / YAML / TOML / Nix / Elixir /
  Dockerfile / `.editorconfig` / `.gitignore`; `--` for Lua. Block-
  only languages (HTML, Markdown, XML, Razor, CSS) wrap the range
  with their block-comment pair (`<!-- … -->` / `@* … *@` /
  `/* … */`). Drops back to Normal after the toggle.

## [0.1.8] - 2026-05-15

### Added
- **Theme presets — `themes/<name>/theme.toml`.** Drop-in `[colors]`
  blocks for the most common editor themes: Dracula, Tokyo Night,
  One Dark, GitHub Dark, Catppuccin Mocha, Night Owl, Gruvbox, Nord,
  Visual Studio (Dark+), GitHub Light, Solarized Light, Catppuccin
  Latte, Ayu Light, and Light Owl. There is no built-in theme loader
  — copy the contents
  of any `theme.toml` into your `~/.config/binvim/config.toml` (or
  `cat themes/dracula/theme.toml >> ~/.config/binvim/config.toml`)
  to apply it. The baked-in default palette remains Catppuccin
  Mocha; the `themes/catppuccin-mocha/theme.toml` file mirrors it
  explicitly as a copy-paste starting point.
- **Window splits — `<C-w>v` / `<C-w>s` / `<C-w>V` / `<C-w>S` /
  `<C-w>h/j/k/l` / `<C-w>q` / `<C-w>o` / `<C-w>=` / `<C-w>T`.**
  Vertical and horizontal splits with independent cursors,
  viewports, and per-pane buffer selection. Lowercase `v`/`s` open
  the file picker immediately so the new pane lands on a *different*
  file — the typical "show A on the left, B on the right" workflow
  is one keystroke + a fuzzy pick. Uppercase `V`/`S` keep Vim's
  same-buffer behaviour (two viewports of one file with independent
  cursors, mirroring edits — useful for skimming the top of a long
  file while editing the bottom). Focus moves geometrically:
  `<C-w>l` picks the right-side neighbour with the largest vertical
  overlap, not tree-order. Crossing focus into a window pointing at
  a different buffer swaps App's live buffer state under you so each
  pane keeps its own syntax-highlight cache, fold ranges, git
  stripe, diagnostics, blame, and markdown concealed render. `:e
  other.txt` / picker selections / `<C-w>q` collapse / `<C-w>o`
  close-others / `<C-w>=` equalize behave the way Vim users expect.
  The split tree lives in `src/layout.rs` (binary tree of
  `WindowId`s); per-pane view state lives in `src/window.rs`
  alongside a `buffer_idx`; the renderer routes through a new
  `BufferState` struct (`src/app/state.rs`) so each pane reads its
  own buffer's state rather than mirroring the active one.
- **Per-buffer split layouts.** Each tab in the tabline carries its
  own layout. Splitting buffer A doesn't follow you when you `L` to
  buffer B — B's tab shows up as a single window (or its own
  previously-saved split state), and going back to A restores its
  split intact. `App.active` and `App.active_tab` track the focused
  buffer and the loaded tab separately; tab swaps via `H`/`L`/`:b`
  stash the outgoing tab's layout into its `BufferStash` and load
  the incoming one, while `:e` / picker keep the current tab and
  only update the focused pane's buffer. Single-window `:e` also
  slides the tabline highlight to the new buffer so the highlighted
  tab matches what you actually see on screen.
- **Split companions stay out of the tabline.** A file picked via
  `<C-w>v` + picker is visible in its split pane but doesn't claim a
  tab slot until you promote it. `<C-w>T` is the dedicated promote
  binding (non-destructive: the split survives, the buffer just
  gains a tab entry). `:b <name>` or `:e <path>` from another
  single-window tab also promotes. The "is this buffer a tab" state
  lives in an explicit `App.tabs: HashSet<usize>` so the rule
  doesn't have to be inferred from the layout structure.
- **Session: closed-buffers stay closed.** Quitting with no open
  buffers (typically after `<leader>bA`) now deletes the saved
  session file for the cwd. Previously the stale file lingered on
  disk and the next launch silently revived every just-closed
  buffer.
- **`<leader>ba` / `<leader>bA` — close all buffers.** New buffer-prefix
  bindings that drop every open buffer in one keystroke. Lowercase
  refuses if any buffer is dirty (mirrors `<leader>bd`); uppercase forces
  through unsaved changes. Lands on the start page with a single
  `[No Name]` slot, identical terminal state to closing the last buffer
  via `:bd`. Which-key popup and README updated to match.

## [0.1.7] - 2026-05-15

### Added
- **Markdown rendering polish.** Five additions on top of the
  existing concealed-render mode:
  - **Strikethrough.** GFM `~~text~~` hides the markers and renders
    the inner span with the terminal's strikethrough attribute in
    Overlay0 (so it reads as "deprecated / done"). Same flanking
    rules as bold — opener can't precede whitespace, closer can't
    follow whitespace.
  - **Code-fence chrome.** ` ```rust ` (or ` ~~~ `) opener now
    hides the fence chars and styles the language tag in
    bold-Peach. Opener + body + closer rows all paint with a
    Mantle background extended to the right edge so the block
    reads as a unified dark slab; the closer row hides its
    backticks and shows as a solid bottom-of-block bar. EOL
    `¬` whitespace markers are suppressed inside the slab so
    the dark background isn't broken by chrome glyphs.
  - **Setext headings.** `Title\n=====` becomes a bold-Lavender
    H1; `Title\n-----` becomes bold-Lavender H2. The underline
    row collapses. CommonMark's prose-only constraint is honoured
    (so `# ATX heading\n---` stays as ATX heading + horizontal
    rule, not a setext promotion).
  - **Horizontal rules.** Standalone `---` / `***` / `___` (3+
    of the same char) renders as a continuous `─` line in
    Overlay0 across the buffer width.
  - **Frontmatter.** YAML (`---`…`---`) and TOML (`+++`…`+++`)
    blocks at the top of file render in muted Overlay0 italic so
    they read as metadata chrome, not content.
  - **Inline HTML.** A handful of common tags fold into native
    styling: `<strong>` / `<b>` → bold, `<em>` / `<i>` → italic,
    `<u>` → underline, `<code>` → inline-code Green; `<br>` /
    `<br/>` / `<br />` collapse entirely; `<!--…-->` comments
    collapse on the line they appear on.
  - **Hidden rows truly collapse.** Previously rows marked
    `Hidden` (setext underlines, `<details>`/`</details>` chrome,
    standalone HTML comments) painted as blank rows — they took
    a vertical row of viewport space. They now skip the render
    walk entirely so subsequent rows shift up. A `<details>`
    block that used to span ~4 chrome rows of breathing room
    now collapses tight against the surrounding prose. Cursor
    placement, mouse-click → source-line mapping, and `j`/`k`
    motion all walk past the same hidden runs so navigation
    stays consistent. Tradeoff (accepted): the gutter's
    line-number now reflects the source line painted at that
    visible row, so `:N` jumps still land on the right source
    line but the visible row of N may shift compared to the
    raw-source layout.
  - **`<details>` disclosure blocks.** Standalone `<details>` /
    `</details>` rows hide as chrome; `<summary>X</summary>`
    becomes a bold-Peach `▼ X` disclosure title (always-expanded
    in a TUI). Inline tags inside the summary are stripped, so
    `<summary><strong>Setup the frontend</strong></summary>`
    reads cleanly as `▼ Setup the frontend`. Body content
    between the open/close tags renders as ordinary markdown.
  - **Six distinct heading colours.** Each ATX level now lands on
    its own Catppuccin accent so an outline reads as a six-tone
    hierarchy at a glance: H1 Red, H2 Peach, H3 Yellow, H4 Green,
    H5 Sky, H6 Mauve. Previously H1+H2 collapsed into Lavender
    and H4-H6 all rendered Sky.
  - **Tables.** GFM tables (header + `|---|---|` separator + body
    rows) render as box-drawn `│ … │ … │` with `├─┼─┤` separator;
    cells column-pad to the widest entry per column. Header bold
    + Lavender, separator dim Overlay0, body normal text.
    Alignment markers (`:---`, `---:`, `:---:`) are accepted but
    everything renders left-aligned in v1. Cursor on a table row
    keeps source-byte semantics so editing via `i` / Visual still
    works on the raw `| … |` pipes.
- **Markdown "concealed render" mode.** `.md` buffers in Normal mode
  paint with structural markers folded into prettier glyphs:
  headings drop their `#`s and render bold (Lavender for H1/H2,
  Sapphire for H3, Sky for H4+); `**bold**` and `*italic*` / `_x_`
  hide their markers and style the inner span; `` `inline code` ``
  hides the backticks and tints the body Catppuccin Green;
  `[text](url)` collapses to underlined-Blue `text` (the URL and
  brackets vanish); `- ` / `* ` / `+ ` bullets become `•` (Peach);
  `> ` blockquotes become `▎ ` with the body in muted Overlay0
  italic. The buffer text is never mutated — entering Insert or
  Visual mode flips back to raw markdown source instantly so you
  edit what's actually on disk. Cursor placement and mouse clicks
  walk the same per-line transforms the renderer uses, so navigating
  hidden ranges and clicking on rendered glyphs both land on the
  expected source position. Fenced code blocks (` ``` ` / `~~~`)
  are tracked across lines so inline transforms are suppressed
  inside them — `_API_` inside `ANTHROPIC_API_KEY=…` no longer
  renders as italic-`API`. Italic / bold flanking rules also
  follow CommonMark: underscore can't open or close emphasis if
  it sits between two word chars (so `f_o_o` and `FOO__BAR__BAZ`
  stay literal); a `*` or `_` opener can't precede whitespace,
  a closer can't follow whitespace.
- **Scroll the `:health` dashboard.** `j`/`k` (or arrow keys) scroll
  one row, `Ctrl-D`/`Ctrl-U` half a page, `Ctrl-F`/`Ctrl-B` and
  PageDown/PageUp a full page, `g`/`G` (or Home/End) jump to top /
  bottom, mouse wheel also scrolls. Footer now hints what's clipped:
  "more above" / "more below" / "↑ k ↓ j to scroll" when both
  directions have content offscreen. Useful on short terminals where
  the LSP / BUFFERS / TAILWIND sections used to fall off the bottom.
- **`:health` dashboard: five new sections after TAILWIND.**
  FORMATTER (on-save tool for the active buffer + binary resolution,
  including project-local `node_modules` resolution and the
  ruff→black / nixfmt→alejandra fallback chains), EDITORCONFIG
  (effective indent / tab width / trim-trailing-ws / final-newline
  + the chain of `.editorconfig` files that produced them, or
  "defaults — no .editorconfig found" if none), TREE-SITTER (detected
  language + highlight-cache state and byte-count, "no parser
  configured" when the extension isn't wired up), SESSION (whether
  the launch restored a session, the cwd-keyed session file path,
  recent-files count), TERMINAL ($TERM, $COLORTERM, truecolor
  detection, $TERM_PROGRAM, current size). Useful for diagnosing
  "why didn't save reformat" / "where is this indent coming from" /
  "are my colours actually 24-bit" without grepping the source.

## [0.1.6] - 2026-05-14

### Added
- **Replace selected text across the current buffer.** Visual-select
  the literal text, `<leader>R`, type the replacement in the prompt,
  Enter. The selection text pre-fills the cmdline so you can edit
  instead of retyping; the prompt's top border carries a "Replace in
  buffer" title. Multi-line selections are rejected with a status
  hint (the underlying substitute is a single-line literal match);
  empty selections show "replace: empty selection". Leader chord
  also now activates in Visual mode generally, not just Normal —
  other actions (file picker, format, rename) work from Visual too.

### Changed
- **`/` search is case-insensitive.** `/foo` matches `Foo` / `FOO` /
  `fOo` / etc. Applies to `n`, `N`, `*`, `#` and the search-result
  highlight painter as well; positions stay correct on Unicode text
  because the lowercase fold is ASCII-only (only A-Z get touched).

## [0.1.5] - 2026-05-14

### Added
- **`:health` is now a full-screen TUI dashboard.** Replaces the
  previous scratch-buffer with an ASCII-banner header and bordered
  PROCESS / RESOURCES / ENVIRONMENT / ACTIVE BUFFER / LSP SERVERS /
  GIT / BUFFERS / TAILWIND sections. New data the old plain-text
  report didn't have: detected language / line count / indent style
  / cursor in ACTIVE BUFFER, diagnostics chips coloured per
  severity, GIT panel with ahead / behind / modified / untracked
  counts. Auto-refreshes every second so resource numbers actually
  move. Esc / `q` / `:q` dismiss.
- **Smart Enter splits HTML / JSX / TSX tag pairs.** `<div>|</div>`
  + Enter expands to three lines with the body indented, matching
  the existing `{|}` / `[|]` / `(|)` behaviour.
- **Tier 1 language coverage — Python, C / C++, Bash, YAML, Lua.** Each
  gets the full stack: LSP (`pyright`-langserver with `basedpyright`
  fallback, `clangd` with `language_id` flipping on extension,
  `bash-language-server`, `yaml-language-server`, `lua-language-server`),
  tree-sitter highlighting (Python, C, C++, Lua — Bash and YAML
  grammars were already wired), and a format-on-save formatter
  (`ruff format` with `black` fallback, `clang-format`, `shfmt`,
  `stylua`). YAML LSP + highlight only at this point; formatter
  landed in the prettier-coverage commit below.
- **Tier 2 language coverage — Vue, Svelte, Markdown, TOML, Ruby, PHP,
  Java.** LSPs (`vue-language-server`, `svelteserver`, `marksman`,
  `taplo lsp stdio`, `ruby-lsp`, `intelephense`, `jdtls` with a
  per-project workspace data dir hashed under
  `~/.cache/binvim/jdtls/<hash>`). Tree-sitter via `tree-sitter-java`,
  `tree-sitter-ruby`, `tree-sitter-php` (`LANGUAGE_PHP` —
  `<?php … ?>`-inside-HTML shape), `tree-sitter-toml-ng` (the `-ng`
  fork — upstream `tree-sitter-toml` is archived),
  `tree-sitter-svelte-ng` (same situation). Vue grammar skipped —
  `tree-sitter-vue` 0.0.3 is alpha. Formatters via Prettier (Markdown
  / Vue / Svelte), `taplo format`, `rufo`, `php-cs-fixer` (temp-file
  dance — no stdin mode), `google-java-format`.
- **Tier 3 language coverage — Zig, Nix, Elixir, Kotlin, Dockerfile /
  Containerfile, SQL.** LSPs (`zls`, `nil` with `nixd` fallback,
  `elixir-ls` with `language_server.sh` fallback,
  `kotlin-language-server`, `docker-langserver`, `sqls`). Dockerfile
  is filename-based: `Dockerfile`, `Containerfile`,
  `Dockerfile.<suffix>`, `Containerfile.<suffix>`, or `*.dockerfile`.
  Tree-sitter via `tree-sitter-zig`, `tree-sitter-nix`,
  `tree-sitter-elixir`, `tree-sitter-containerfile` (the maintained
  successor to `tree-sitter-dockerfile`, which is stuck on
  tree-sitter 0.20), `tree-sitter-sequel` (successor to
  `tree-sitter-sql`); Kotlin's parser via `tree-sitter-kotlin-ng` but
  the crate ships no highlights query so `.kt` files render as plain
  text (LSP carries semantic info). Formatters via `zig fmt`,
  `nixfmt` with `alejandra` fallback, `mix format`, `ktfmt`
  (temp-file dance), `sql-formatter`.
- **Emmet auxiliary LSP.** `emmet-ls` layers alongside the primary
  server for HTML / CSS / JSX / TSX / Vue / Svelte / Astro / Razor
  buffers. Typing `ul>li*3>a[href]` and accepting the completion
  expands to the nested markup; mirrors the auxiliary shape used for
  Tailwind. Razor reports as `html` since emmet-ls has no Razor
  mode — the markup half of a Razor file is HTML.
- **Snippet continuation-line indent.** Multi-line LSP snippet bodies
  (emmet expansions, function templates, …) arrive with continuation
  lines starting at column 0 — the server has no view of where the
  buffer sits. Prepend the current line's leading whitespace to every
  line after the first, mirroring VS Code / Neovim. Tab stops shift
  forward for each indent inserted before them, so cycling lands at
  the right column.
- **`Ctrl-J` / `Ctrl-K` move lines down / up.** Single line in Normal
  mode, the full selected line range in Visual mode (any kind).
  Cursor and visual anchor follow the moving block so the selection
  stays attached to the same content across the shift. Count prefix
  works: `3<C-J>`. File-boundary clamps prevent the cursor from
  drifting past row 0 / EOF; trailing-newline correctness preserves
  whether the file ends with `\n` or not. Ropey's phantom empty
  trailing line is detected and skipped so the last real line doesn't
  produce a stray blank row when moved down at the edge.
- **Esc strips whitespace-only lines on Insert→Normal.** Vim
  convention: after Enter on an indented line you land at an
  auto-indented column; if you Esc without typing content, the
  indent is left behind. Now stripped — cursor parks at column 0
  rather than the standard col-1 step-back.
- **Notification box auto-dismisses after 10 seconds.** Status
  messages (save confirms, format results, LSP errors, hunk-op
  outcomes) used to linger until the next keypress. Keypress still
  dismisses instantly; the timeout is a fallback for the no-input
  case. Tracked via a new `status_msg_at: Option<Instant>` field;
  the event loop's poll budget shrinks to the deadline so the box
  clears on time without any input.
- **Prettier as the cross-format fallback.** Format-on-save dispatch
  routes everything biome doesn't yet cover to Prettier — Markdown,
  MDX, Vue, Svelte, HTML, CSS preprocessor variants (`.scss`,
  `.less`), YAML, GraphQL. Project-local
  `node_modules/.bin/prettier` wins over a global install, but
  global works too (no node_modules required), so a `.yaml` or
  `.md` file in any random project gets the same canonical
  formatter on `<leader>f`.

### Changed
- **`:w` status shows the basename, not the full path.** The save
  confirmation used to render the buffer's absolute path
  (`"/Users/…/binvim/CLAUDE.md" 51L written`), which wrapped the
  notification box across multiple rows on any deep tree. Now prints
  just the basename — disambiguating between two same-named files
  isn't this message's job; the tab bar already carries that signal.
- **Notifications park below the tab bar.** `draw_notification`
  hard-coded `top = 0`, which sat the box on top of the active tab's
  label. Now uses `app.buffer_top()` (which is 1 when tabs are
  showing, 0 otherwise) so the notification anchors to the first row
  of buffer content instead of overlapping tab labels.

### Fixed
- **Auxiliary LSPs (emmet, Tailwind, …) never received `didOpen` when
  the primary server's binary wasn't installed.** `LspManager::
  ensure_for_path` returned the primary client specifically. When the
  primary failed to spawn (vscode-html-language-server missing on a
  `.html` file is the canonical case), the function returned `None`
  and the two callers (`lsp_attach_active` / `lsp_sync_active`)
  early-returned on `None` — `didOpen` never went out, and emmet-ls
  / Tailwind appeared as "running" in `:health` but produced empty
  responses to every textDocument/* request. Specific user-facing
  symptom: typing `!` in a blank `.html` file should pop the HTML5
  boilerplate via emmet-ls, but nothing appeared. Fixed by changing
  `ensure_for_path` to return `bool` indicating "at least one client
  is running" rather than the primary client specifically.
- **CSS custom properties rendered identical to regular properties.**
  The CSS query's `((property_name) @variable (#match? @variable
  "^--"))` override was meant to flip `--color-primary` away from the
  Lavender `@property` tone toward the default-text `@variable`. But
  `@variable` resolves to `None` colour in `config.rs`, and the
  highlight priority loop only updated `byte_priority` inside the
  `if let Some(color) = …` branch — a more-specific later match
  meaning "this should be uncoloured" silently failed to clear the
  earlier general match's colour. Resolve the colour outside the
  `Option` branch; `None` captures now also bump priority, so the
  override visibly takes effect. Safe because the queries we use
  follow the standard "general first, specific later" convention
  (the one outlier — the JSON bundled query — already got rewritten
  inline when this priority system was introduced).
- **Git gutter: stripe landed on the wrong block when adding a duplicate
  alongside an existing one.** Adding a structurally-similar block (e.g.
  a second `defineField({...})` next to an existing one) made the green
  Added stripe paint on the *original* block instead of the new one.
  This was Myers diff being clever — two equally-valid edit scripts
  produce the same final file, and Myers tie-breaks toward the earlier
  position. Switched all three `git diff` invocations to
  `--diff-algorithm=histogram`, which anchors hunks on landmark lines
  and lines up with what humans expect for additions like this.

### Added (earlier in cycle)
- **Git gutter (stage 1 of git integration).** A coloured stripe at the
  leftmost gutter column shows working-tree changes against the index:
  Green `▎` for added lines, Yellow `▎` for modified lines, Red `▁` for
  deletions (painted on the line that now sits at the deletion point).
  Hunks are refreshed on save, on buffer switch, and on initial open.
  Implementation shells out to `git diff --no-color --unified=0` and
  parses the hunk headers; no libgit2 dependency. Gutter widens by one
  column to make room (`digits + 3`).
- **Inline git blame (stage 4 — git integration complete).** `:Gblame`
  toggles per-line virtual text at the end of every row: author •
  relative age (`3d`, `2w`, `4mo`) • short SHA, in muted italic so the
  text reads as metadata not content. Sourced from `git blame
  --porcelain`, parsed locally. Each buffer toggles independently, so
  blame can be on for one file and off for another. Re-fetched on
  reload (e.g. after `<leader>hr`) since line numbers shift; cleared
  when the toggle is off so memory doesn't grow. Suppressed on rows
  that already show an inline diagnostic to avoid stacking overlays.
- **Stage / unstage / reset hunk (stage 3 of git integration).**
  `<leader>hs` stages the hunk under the cursor by building a one-file
  unified diff and piping it through `git apply --cached --unidiff-zero`.
  `<leader>hu` unstages a staged hunk via the reverse patch. `<leader>hr`
  discards the working-tree change — applies the reversed patch to the
  working tree (not the index) and reloads the buffer from disk. Reset
  refuses to run while the buffer is dirty, so unsaved edits can't be
  silently overwritten. Each action refreshes the gutter and reports
  success / failure in the status line. Reuses the existing reload-from-
  disk path (now exposed as `force_reload_from_disk` for cases that
  bypass the dirty guard).
- **Hunk navigation + preview (stage 2 of git integration).** `]h` /
  `[h` jump to the next / previous git hunk in the active buffer (bonks
  at the end, no wrap-around — matches `]q` / `[q`). `<leader>hp`
  previews the hunk under the cursor in a hover popup, rendered from
  `git diff -U3` so three lines of surrounding context come along.
  Status line gains a `+A ~M -D` counter next to the branch name when
  the working tree has any added / modified / deleted hunks. New
  `<leader>h` which-key entry advertises the sub-menu (`s` / `u` / `r`
  reserved for stage 3).
- **Word-aware drag after double-click.** Double-click a word to select
  it, then drag forward or backward to extend the visual selection
  word-by-word. The anchor pins to the side of the original word
  opposite the drag direction; the cursor snaps to the word boundary
  under the mouse. Dragging through whitespace holds the selection at
  the previous word boundary so it only grows when a new word is
  crossed. Falls back to the existing char-by-char drag if the drag
  didn't start from a double-click.

## [0.1.4] - 2026-05-11

### Added
- **Alt / Ctrl + Backspace deletes a word; Cmd / Super + Backspace
  deletes to start of line.** Insert-mode Backspace now reads
  `key.modifiers` and routes to one of three paths: SUPER/META wipes
  from `line_start` to the cursor; ALT/CONTROL peels trailing
  whitespace then one homogeneous run (word chars or punctuation),
  matching macOS Option-Delete; otherwise the existing single-char
  delete with auto-pair / line-join logic. None of the three span
  lines, so an accidental Cmd-Backspace can't eat into the row above.

### Fixed
- **Cursor placement skewed by inlay hints.** `place_cursor` walked
  only buffer chars + tab widths, so when an inlay hint took N visual
  cells before the cursor's buffer column, the terminal cursor landed
  N cells short of where the buffer actually sat. Backspace then
  removed a char that visually looked like it was "ahead" of the
  cursor. Mouse-click mapping had the inverse error: clicks inside a
  hint label scattered into the next visible token. A shared helper
  `inlay_hint_widths_for_line` now feeds both: cursor placement adds
  hint widths for cols *strictly before* `cursor.col` (the at-cursor
  hint renders on the cursor's far side so the cursor sits on the
  near edge of the hint), and click mapping snaps clicks-inside-a-hint
  to the buffer col at the hint's anchor.
- **C# / Razor file-type icon was tofu on Nerd Fonts v3.** The
  previous codepoint `\u{f81a}` was a v2-only glyph. Switched to
  `\u{e648}` (`nf-seti-c_sharp`), stable across v2 and v3. Affects
  both the picker icon and the status-line "razor" tag.

## [0.1.3] - 2026-05-11

### Added
- **YAML / XML syntax highlighting.** New `Lang::Yaml` and `Lang::Xml`
  variants wired to `tree-sitter-yaml` and `tree-sitter-xml`. XML
  detection covers the MSBuild + .NET family — `.xml`, `.csproj`,
  `.fsproj`, `.vbproj`, `.props`, `.targets`, `.config`, `.manifest`,
  `.nuspec`, `.resx`, `.xaml`, `.xhtml`, `.xsd`, `.xsl`, `.xslt`,
  `.plist`.
- **`.editorconfig` highlighting.** Byte-level scanner: `#`/`;`
  comments in Overlay1, `[*.cs]`-style section headers in Pink,
  `key = value` pairs with the key in Lavender, the `=` in Sky, and
  the value in Green. TOML grammar can't parse the bracket-pattern
  section headers so it's a custom pass.
- **`.gitignore` family highlighting.** Covers `.gitignore`,
  `.gitattributes`, `.dockerignore`, `.npmignore`. `#` comments,
  `!`-negation prefix in Mauve, pattern lines in Lavender.

### Changed
- **CSS highlight query replaces the bundled tree-sitter-css one.**
  Upstream paints `class_name`, `id_name`, `namespace_name`,
  `property_name`, and `feature_name` all as `@property` — so in
  `.foo { color: red; }` the selector `.foo` and the property
  `color` rendered in the same Lavender. The replacement query
  splits them: `.class-name` is `@constructor` (Yellow),
  `#id-name` is `@label` (Sapphire), `property:` stays `@property`
  (Lavender), `--custom-prop` is `@variable`, at-rules
  (`@media`/`@keyframes`/…) are `@keyword` (Mauve).

### Added (continued)
- **DAP multi-project workspace support.** `<leader>ds` works from
  any file now — not just `.cs`. When the workspace has more than
  one `.csproj` (or `.fsproj` / `.vbproj`), a picker opens listing
  every project relative to the cwd. Walking up looks for `.sln` or
  `.git` first to find the workspace root, then enumerates projects
  beneath it (skipping `bin/`, `obj/`, `node_modules/`, etc., with
  a 6-deep bound). The buffer's path doesn't need to be inside the
  picked project — opens fine from a README at the repo root.
- **DAP reads `Properties/launchSettings.json`.** Every profile
  with `commandName == "Project"` (Kestrel hosting via `dotnet run`)
  becomes a launchable option:
  - 0 profiles → start with framework defaults (no overrides).
  - 1 profile → use it directly.
  - >1 profiles → open the launch-profile picker, one row per
    profile, displaying the profile name + its applicationUrl
    (e.g. `Umbraco.Web.UI  (https://localhost:44317, http://…)`).
  The chosen profile's `applicationUrl` becomes `ASPNETCORE_URLS`
  on the launched process so the app binds to the user's configured
  port instead of the framework default `http://localhost:5000` —
  fixes the "Failed to bind to 127.0.0.1:5000: address already in
  use" case when another local service squats on :5000. The
  profile's `environmentVariables` pass through as `env` on the
  launch payload (e.g. `ASPNETCORE_ENVIRONMENT=Local`).
- **Picker file-type icons.** Files / Recents / Buffers / Grep /
  References rows get a Nerd Font icon per row from `Lang::detect`
  on the basename. Symbol / CodeAction pickers stay unchanged (rows
  aren't files). Grep / references rows now correctly split off the
  trailing `:LN:COL:…` suffix instead of treating it as part of the
  filename.
- **Picker fuzzy-match char highlighting.** `fuzzy_match` returns
  match positions alongside the score; matched chars render in
  Catppuccin Yellow + Bold so it's obvious which query letters
  produced the row's rank.
- **Inlay-hint kind styling.** Parameter hints (LSP `kind == 2`)
  render in a warmer Overlay2 tone; type hints (`kind == 1` or
  unknown) keep the muted Overlay1 they had — both categories scan
  apart on a mixed line.
- **JSX overlay: `{expr}` braces tagged as JSX-template syntax.**
  JSX expression containers get `@operator` on their braces so they
  read as JSX-template syntax instead of being mistaken for object
  literals. (Originally also covered `<>…</>` fragments via a
  `jsx_fragment` capture, but that node type isn't in
  tree-sitter-typescript / tree-sitter-javascript 0.23 — adding it
  made `Query::new` fail and wiped the entire highlight cache for
  every .tsx / .jsx file. Reverted, regression-tested.)
- **`<leader>do` / `<leader>dS` for Doc / Workspace symbols.** Moved
  from top-level `<leader>o` / `<leader>S` so all "navigate around
  code while debugging" pickers cluster in the debug sub-menu.
  Existing debug bindings shift: `dq` is Stop session, `dO` is Step
  out (was `dS` / `do`).
- **`gt` / `gT` Vim aliases for next / previous buffer.** Same path
  as `H` / `L`.
- **Middle-click closes a tab.** Subject to the same dirty-buffer
  guard as `:bd`. Faster than aiming for the `×`.
- **Clickable tab-bar overflow chevrons.** `‹` / `›` shift the
  visible slice by one tab when clicked — sets the active buffer to
  one step before the first visible tab (`‹`) or one step after the
  last visible tab (`›`).
- **Multi-cursor Enter mirroring.** Pressing Enter with additional
  cursors active inserts a literal `\n` at every cursor. Smart
  indent at the secondaries is non-trivial (neighbouring context can
  disagree across positions) so v1 keeps them in basic sync.
- **Persistent jumplist.** Each `SessionBuffer` now serialises its
  per-buffer `jumplist` + `jump_idx`; on launch the values restore
  alongside cursor/viewport. Entries are clamped against the
  current buffer's bounds so a file shortened since the last session
  doesn't carry an out-of-range jump.
- **`:s` / `:S` regex flag.** Add `r` to the flag tail (e.g.
  `:s/foo.*bar/x/gr` or `:%S/^let /var /r`) to interpret the pattern
  as a regex. Replacement honours `$1`/`$2`/… capture references. No
  flag → fixed-string substitution, same as before. Project-wide
  `:S` also drops `--fixed-strings` from the ripgrep candidate
  search when `r` is set.
- **Proper visual-block surround.** `Vb`-selected rectangle now
  wraps each row's column slice independently — `(c1, c2+1)` on
  every row in the rectangle — instead of falling back to a coarse
  anchor-to-cursor span across the whole region.
- **Razor / `.cshtml` syntax highlighting.** New `Lang::Razor` variant
  routed via `tree-sitter-razor`, paired with the bundled C# highlights
  query plus a Razor overlay that tags every `@`-marker directive
  (`@inject`, `@using`, `@{`, `@if`, `@(expr)`, `@Identifier`, `@*…*@`,
  …) as `@keyword.directive` and HTML element delimiters as
  `@punctuation.bracket`. Tag names and attribute names — anonymous
  lexer rules in the grammar — get coloured by a byte-level overlay so
  they still light up when the grammar's parser produces ERROR nodes
  (typical for Tailwind `class="…[16px]…"` attribute values, BOM
  headers, etc.). C# keywords inside broken regions get a regex-based
  fallback paint so an `else { if (x) {…} }` block keeps its colour
  even when the tree-sitter pass can't reach the tokens.
- **`<leader>f` formats the active buffer.** Same path as `:fmt` /
  `:format`. Picks the right tool per extension:
- **csharpier integration for `.cs` (and `.cshtml` / `.razor` once a
  future csharpier supports them).** Resolved from `$PATH` or
  `~/.dotnet/tools/csharpier`. The buffer goes through a sibling temp
  file so the project's `.csharpierrc` / `.editorconfig` resolution
  still works. csharpier 1.x prints `Is an unsupported file type` and
  exits 0 for Razor; the dispatch detects that signal and falls back.
- **`.editorconfig`-driven indent normaliser for `.cshtml` / `.razor`.**
  Reflows leading whitespace per `indent_style` and `indent_size` —
  tabs → spaces, spaces → tabs, with intermediate columns preserved.
  Inner whitespace (column-aligned attributes, mid-line tabs) is left
  alone. Wired into the csharpier fallback so saving a Razor file
  applies the indent settings even though csharpier itself doesn't
  format it.
- **gofmt / goimports for `.go`.** Prefers `goimports` when on `$PATH`
  (formats + organises imports), falls back to plain `gofmt`. Both read
  stdin and write to stdout so no temp file is needed.
- **Visual-mode `p` / `P` replaces the selection.** Previously only
  Normal-mode parsed the put keys. Now `viw` → `p`, `V<motion>` →
  `p`, and block-select → `p` all swap the visual span with the
  register's contents. Block paste deletes the rectangle and inserts
  at the corner; charwise + linewise behave like Vim's `cpoptions-=>`
  with a linewise-over-charwise trim.
- **Paste from the OS clipboard.** `p` / `P` against the unnamed /
  `+` / `*` registers consult the clipboard first — anything you
  copied in another app wins over the in-memory yank. Falls back to
  the in-memory register when the clipboard is empty, locked, or
  non-text. Linewise heuristic: trailing newline *and* interior
  newline → linewise paste; single line with trailing newline stays
  charwise.
- **.NET Core debugger via DAP.** New `src/dap/` module spawns Samsung's
  netcoredbg (looked up on `$PATH`), drives the full Debug Adapter
  Protocol handshake (initialize → launch → setBreakpoints →
  configurationDone), and surfaces stop / step / output events in a new
  bottom debug pane. `:debug` (or `<leader>ds`) auto-builds the project
  via `dotnet build` and auto-resolves the dll under `bin/Debug/net*/`,
  preferring `<project>.dll` from the most recently built target. The
  adapter registry in `src/dap/specs.rs` is adapter-agnostic — additional
  adapters (delve, debugpy, lldb-dap) plug in with one struct entry.
  Tested against .NET 10 on Apple Silicon.
- **Bottom debug pane.** `:dappane` / `<leader>dp` toggles a split below
  the editor (painted in the tab bar's Mantle shade so it reads as
  chrome rather than a buffer split). The pane silently no-ops on
  terminals too short to keep the editor usable. Header shows
  `DEBUG │ <adapter> · <status>`; body splits into call stack +
  locals on the left, debug-console tail on the right.
- **`Mode::DebugPane` for in-pane navigation.** `<leader>df` focuses
  the pane; `j`/`k`/`g`/`G` move the locals selection (with auto-follow
  scroll), `Enter` / `Tab` / `<space>` expand a structured value,
  `Ctrl-Y` / `Ctrl-E` free-scroll the left column without losing the
  selection, `J` / `K` scroll the console column, `c`/`n`/`i`/`O` step
  without leaving the pane, `:` enters the command line, `Esc` returns
  to Normal.
- **Variable expansion in the locals tree.** Structured locals render
  with ▶ / ▼ markers; expansion lazily fetches `children` per
  `variables_reference` and caches them. Children fetched concurrently
  are routed back to the right parent via a `request_seq → vref` map.
  All maps clear on `stopped` / `continued` — DAP doesn't promise vref
  stability between stops.
- **Breakpoint and current-PC gutter markers.** User breakpoints render
  as ● in the sign column; the currently-stopped top frame renders as
  ▶ (Catppuccin Peach). Both win over LSP diagnostic severity on the
  same line.
- **Visual Studio / Rider F-key bindings.** `F5` continue (or start
  when no session), `Shift+F5` stop, `F9` toggle breakpoint, `F10`
  step over, `F11` step into, `Shift+F11` step out. Work in any
  editor mode so the muscle memory carries through Insert / DebugPane.
- **Auto-jump to stopped frame.** When a `stopped` event arrives, the
  editor opens the source of the top frame (if not already open),
  jumps the cursor onto the paused line, and the gutter ▶ marker
  lands there. Scroll positions reset so each new stop starts from a
  predictable viewport.
- **Adapter stderr surfaced in the pane.** Any line netcoredbg writes
  to stderr streams into the debug-console buffer and the pane status
  header — adapter crashes no longer look like silent hangs. An
  unexpected `Child::try_wait` exit becomes an `AdapterError` event so
  the user sees the cause instead of a frozen pane.
- **Unverified-breakpoint surfacing.** netcoredbg's `setBreakpoints`
  response includes per-breakpoint `verified` flags; unverified entries
  now stream into the console + status line so the user spots a
  missing-PDB / wrong-line / "bind by pattern" misfire instead of
  silently never hitting.

### Changed
- **`<leader>d` is the debugger sub-menu.** Doc symbols moved from
  top-level `<leader>o` to `<leader>do`; Workspace symbols from
  `<leader>S` to `<leader>dS`. The leader-pick popup lists `+Debug`
  alongside `+Buffer`. The leader-entry hints update with the new
  mapping.
- **Debug pane uses the tab-bar's Mantle shade.** Previously rendered
  in Surface0 / Surface1, which read as a second buffer split. Now
  both header and body paint as `#181825` so the pane visually sits
  in the chrome layer.

### Fixed
- **CRLF files clobbered inline diagnostics.** `Buffer::from_path` kept
  every line's trailing `\r`; the renderer stripped only `\n`, so the
  `\r` reached the terminal and reset the cursor to column 0 right
  before the Error-Lens inline diagnostic painted on top of the
  source. Files with CRLF on the line a diagnostic landed on (common
  in Vettvangur's mixed-line-ending `.cs` files) rendered as
  `● Unnecessary using directive.PublishedContent;` with the leading
  source obliterated. Now normalises `\r\n` → `\n` on load + on
  disk-watch reload, with a defensive trailing-`\r` strip in the
  renderer as belt-and-suspenders for paste / LSP-applied edits.
- **Mouse clicks on tab-indented lines landed past the click.** The
  click handler treated the post-gutter screen column as the buffer
  char column directly. A tab takes `TAB_WIDTH` visual cells but one
  buffer char, so two leading tabs (visual col 8) used to map to char
  8 — well past `<partial` on the line. The handler now replays the
  renderer's display-width walk to translate visual position back to
  char position; drag selection shares the same mapping.
- **Razor closing-tag names rendered uncoloured.** The post-pass's
  child-node skip set listed `<`, `>`, `/>`, `=`, `"` but missed
  `</`, so the scanner treated the closing-tag opener as a nested
  child and stopped before reaching `section` / `div`. Added `</`
  to the skip set so opening and closing tag names match in colour,
  plus an overlay capture so the `</` token itself paints with the
  punctuation tone like `<` / `>` / `/>`.
- **Stale frames + locals after step / breakpoint hit.** `stackTrace`,
  `scopes`, and `variables` responses mutated session state but didn't
  emit a user-facing `DapEvent`, so the main loop's `events.is_empty()`
  gate skipped the re-render and the ▶ marker + locals appeared only
  on the next keypress. `DapManager::drain` now returns
  `(events, progress)` and the main loop renders on either condition.
- **DAP polling latency.** The main loop's 100ms poll ceiling stacked
  ~500ms of perceived lag onto a single step (4-5 request/response
  round-trips, each waiting on the next poll wake). The poll budget
  now caps at 16ms while `DapManager::is_active()`; idle keeps the
  100ms cadence.
- **Deep stacks hiding locals.** With `justMyCode=false`, an ASP.NET
  breakpoint produced 10-15 framework frames that filled the left
  column and pushed locals off-screen. The renderer used to cap the
  frame list, but now every frame is in the list and the column
  scrolls — `Ctrl-Y` / `Ctrl-E` peek upward without losing the
  selection.
- **`adapterID` for netcoredbg.** The adapter keys behaviour on the
  well-known `coreclr` adapter id; passing our internal `"dotnet"` key
  produced a degraded session. The initialize request now sends
  `adapterID: "coreclr"` while the editor-side registry continues to
  use the `dotnet` key.
- **Lambdas inside minimal-API endpoints never hitting.** Default
  `justMyCode=true` caused netcoredbg to rebind breakpoints inside
  endpoint lambdas to the nearest user-code sequence point (the
  `MapGet` registration line), so they fired once during startup and
  never again on request. Switched the default to `false`; the
  `breakpoint` DAP event is now handled so adapter-side rebinds
  propagate to the gutter.
- **netcoredbg empty stackTrace immediately after stop.** Some adapters
  refuse to populate `stackTrace` until the client has called
  `threads`. The `stopped` event now fires both requests back-to-back
  so the frame list is populated by the time `stackTrace` returns.

## [0.1.2] - 2026-05-11

### Changed
- **Tab bar shows for single buffers too.** `show_tabs()` now returns
  true whenever any path-backed buffer is open (or multiple buffers
  are loaded), not only the multi-buffer case. The fresh-launch
  `[No Name]` seed still hides the bar. `open_buffer` strips the
  phantom seed buffer on the transition into the first real file —
  same shape as the session-restore cleanup — so the bar starts as a
  single tab rather than `[No Name] | foo.rs`.

### Fixed
- **`.tsx` files parsed as TypeScript by tsserver.** A single
  `typescript-language-server` client serves both `.ts` and `.tsx`
  (same `ServerSpec.key`). On `didOpen` we were reusing the client's
  stored `language_id` — whichever spec spawned it first — so a
  `.tsx` file opened after a `.ts` would get `languageId: typescript`
  and every JSX `<…>` came back as a syntax error. `LspClient::did_open`
  now takes the languageId per call; `LspManager::did_open_all` looks
  up the spec for each path and pipes the right one through.
- **Pure-TypeScript syntax highlighting regression.** The JSX overlay
  query referenced `jsx_*` node types that don't exist in
  tree-sitter-typescript's pure-TS grammar (only TSX has them), so
  `Query::new` errored out and the whole `.ts` highlight cache came
  back empty — every token rendered as plain text. The overlay is now
  only layered onto TSX and plain JS, leaving pure TS to its JS+TS
  query stack as before.
- **JSX / TSX tag highlighting.** Tree-sitter-javascript's bundled
  highlight query categorises every JSX element name as a generic
  identifier, leaving `<div>` / `<main>` / `<span>` indistinguishable
  from surrounding variables. Added an overlay query that tags
  lowercase JSX names as `@tag` (Mauve) and PascalCase names as
  `@constructor` (Yellow), with attribute names rendering as
  `@attribute`. Member-access components (`Foo.Bar`) are coloured on
  both halves. Layered onto JS / TS / TSX.
- **HTML-tag matched-pair highlight narrows to the tag name.** Cursor on
  `<main className="…">` used to highlight the whole opening run and the
  whole `</main>` run; now both halves underline just the tag name
  (`main` ↔ `main`).

## [0.1.1] - 2026-05-10

### Added
- `CONTRIBUTING.md` — repo conventions, architecture quick-reference, and a
  step-by-step PR walkthrough for the most common contribution shapes
  (new LSP, new tree-sitter grammar, motion/text-object).

### Fixed
- **Multi-selection delete cursor shift.** After `d` / `c` on a
  Ctrl-N multi-selection, secondary cursors landed at stale positions
  — they used original char indices without accounting for the
  cumulative shift left from earlier (lower-indexed) deletes. Now each
  cursor's final position subtracts the total length of every prior
  deletion, so all cursors sit exactly where the deleted ranges were.

### Changed
- **`H` / `L` cycle buffers (tabs).** Shift-h / shift-l in Normal mode
  now switch to the previous / next buffer respectively (same as
  `:bp`/`:bn` and `<leader>bp`/`<leader>bn`). They no longer fire the
  Vim viewport-top / viewport-bottom motions — use `M` (viewport
  middle) and the scroll commands for those workflows.
- **Tab bar reverted to one row.** Drops the 2-row variant. File-type
  icon column also removed; each tab is just ` label[ +]  × ` now.
- **Restored sessions land on the start page.** The saved buffers are
  loaded in the background; the welcome screen is on top until the
  user reaches for one (`H`/`L` cycle, `:bn`/`:bp`, `:b<n>`, `:e`,
  `<leader>e`, or clicking any tab). Solo-buffer sessions: `H`/`L`
  still dismiss the start page and surface the one buffer (with the
  usual "Only one buffer" hint). The tab bar IS shown above the start
  page so the user can see what was restored — no tab is highlighted
  as active until you actually pick one.

### Fixed
- **Snippet / completion accept honours `textEdit.range`.** Servers
  send an explicit replacement span (e.g. covering a trailing `.` when
  proposing `?.method`); previously we always used the client-side
  word-prefix guess, which produced `obj.?.method` instead of
  `obj?.method`. Now we honour the server's range when present and
  fall back to the prefix guess only when it's absent.
- **Session restore no longer leaves a phantom `[No Name]` buffer.**
  `App::new()` seeds `buffers[0]` with an empty stash before
  `hydrate_from_session` runs; the restore path now strips that initial
  empty slot when any session buffer is successfully opened.

### Changed
- **Tab bar polish.** Each tab now carries a 2-char language icon
  (colour-coded per file type: `rs`/`ts`/`js`/`go`/`{}`/`md`/etc.) on
  the left and a clickable `×` close button on the right. Padding
  bumped to give the label and chrome breathing room. Overflow
  chevrons render `‹` / `›` at the bar edges when tabs have scrolled
  off either side. Close-clicks honour the same dirty guard as `:bd`
  (refuse, status message); use the buffer prefix's `D` for force.
- **Tab bar replaces the buffer picker.** When 2+ buffers are open
  (outside the start page), a Catppuccin-Mantle tab row paints across
  the top of the screen — active tab in Surface1 + Lavender + bold,
  inactive in Subtext0, dirty buffers carry a Peach `+`. Left-click on
  a tab switches to it. Buffer area shifts down by one row when the
  bar is visible. The `<leader>b<space>` picker binding is gone; use
  the tab bar (click), `:bn`/`:bp`, or `<leader>bn`/`<leader>bp`.

### Added
- **Sublime-style multi-selection.** In Visual-char mode, `Ctrl-N`
  finds the next literal-text occurrence of the current selection and
  adds it as a parallel selection. Repeat to keep selecting. `d`, `c`,
  `y` then apply to every selection at once: `d` deletes all,
  `c` deletes and enters Insert mode with a mirrored cursor at every
  former selection start (typing then mirrors across all sites via
  the existing multi-cursor machinery). Wraps the buffer once; status
  message when only one occurrence exists or none are left.
- **Multi-cursor (real, mirrored).** `Ctrl-click` in Normal mode adds
  a secondary cursor at the click position; cursors render as Lavender
  blocks. Entering Insert via `i` keeps cursors anchored; via `a`
  shifts each cursor by +1 (so typing lands *after* each char). Other
  Insert-entries (`I`/`A`/`o`/`O`) collapse the multi-cursors. Inside
  Insert mode, typing and Backspace mirror at the primary cursor and
  every additional cursor (bottom-up edit order keeps indices stable).
  `Esc` exits Insert and collapses; a plain (non-Ctrl) click also
  collapses. Auto-pair is disabled while multi-cursor is active — the
  user is in mass-edit mode anyway.
- **Inlay hints.** Added `textDocument/inlayHint` request +
  response parser; hints render between buffer chars in dim italic
  (Overlay1). Refetched once per buffer-version; respects horizontal
  scroll and shares the line's width budget with the buffer chars.
  Servers that publish hints (rust-analyzer, typescript-language-server,
  gopls, …) now annotate types and parameters inline.
- **Snippet expansion.** LSP completion items marked
  `insertTextFormat == 2` get their `$N` / `${N:default}` / `$0`
  placeholders parsed and resolved. The cursor lands at `$1` (or `$0`
  if no `$1`); first-seen defaults mirror to later bare references of
  the same stop. `snippetSupport: true` advertised in the initialize
  payload so servers actually send them. Tab cycling between stops is
  a follow-up.
- **Project-wide substitute (`:S/pat/repl/[g]`).** Walks ripgrep for
  every file containing `pat` under `cwd`, opens each, applies the
  substitution buffer-wide, saves. Status reports total substitutions
  + file count + any per-file errors. Range prefix ignored.
- **Replace-all in buffer (`<leader>R`).** Literal-string counterpart
  to LSP rename: gathers the word under the cursor, opens a prompt
  pre-filled with it, on submit runs `:%s/word/new/g`. Useful when the
  symbol isn't LSP-tracked (config files, comments, prose).
- **Double-click to select word.** A second left-click at the same buffer
  position within 350ms expands to the inner word under the cursor and
  enters Visual-char mode.
- **Visual-block mode (`Ctrl-V`).** Third visual kind alongside char and
  line. Rectangular column selection across a line span; `d`/`c`/`y`
  apply column-wise per row. `>`/`<` fall back to indent/outdent over
  the line range. Selection survives drag, mode label reads `V-BLOCK`.
- **Sessions.** Open buffer set + per-buffer cursor + viewport are
  persisted to `~/.cache/binvim/sessions/<cwd-hash>.json` on clean
  shutdown and restored on launch when no file argument is passed. A
  buffer whose path no longer exists is silently dropped.
- **Better completion popup.** Each row renders a colour-coded kind chip
  (`fn`, `var`, `cls`, `if`, `fld`, `mod`, `snp`, `kw`, `K`, …) on the
  left and the server-supplied `detail` (e.g. the function signature)
  right-aligned in Overlay2 grey. Popup width caps at 80 cols; detail
  trims first when space is tight.
- **Picker scrolling.** Mouse wheel inside the picker moves the selection
  by ±3 (instead of scrolling the buffer behind it); PageUp/PageDown jump
  by a full visible page; Ctrl-U/Ctrl-D jump by half a page; Ctrl-G /
  Home and Ctrl-Shift-G / End jump to first/last result. Existing
  arrow-keys and Ctrl-J/Ctrl-K still move by one row; footer hint
  advertises `^J`/`^K` to match Vim convention. (Ctrl-N/Ctrl-P aliases
  removed — single Vim-style binding only.)

### Changed
- **Hover popup now syntax-highlights code and preserves indentation.**
  The markdown returned by an LSP is parsed into structured lines (prose,
  headings, horizontal rules, fenced code blocks); each fenced block's
  language tag drives a tree-sitter pass and the byte-colour map paints
  per-character, so signatures (e.g. `defineField<{ name: string; … }>`)
  finally render the way they would in the buffer. Code lines keep their
  leading whitespace verbatim (tabs expand to four columns); prose still
  word-wraps to the popup width but indentation carries through to wrapped
  continuations. Headings render bold + Lavender; rules render as a
  horizontal divider in the border colour.
- **Picker redesigned as a centered floating popup.** Catppuccin-Mantle
  bordered box (~80% wide, ~60% tall, clamped) replacing the full-width
  bottom panel. Title chip in the top border with a counter on the right;
  Peach `›` prompt arrow; selected row gets a Lavender `▌` accent + Surface1
  background + bold; directory part of each path renders dim while the
  basename pops bright; smart truncation prefers keeping the basename
  intact; footer hint row (`↵ open  ^N/^P navigate  esc cancel`) with a
  shorter fallback on narrow terminals.
- **Source layout: `src/app/` and `src/lsp/` are now sub-module directories.**
  The two largest files (formerly `app.rs` at ~5.6k LoC and `lsp.rs` at ~2k)
  have been split into 14 and 6 focused children respectively. Each parent
  (`src/app.rs`, `src/lsp.rs`) is now a slim entry that declares the
  children and re-exports the public API; external addresses (`crate::app::App`,
  `crate::lsp::LspManager`) are unchanged. No behaviour changes.
- `CLAUDE.md` / `CONTRIBUTING.md` / `LSP_ADOPTION.md` updated to point at
  the new file paths and document the sub-module convention.

## [0.1.0] - 2026-05-09

Initial public release. binvim is a single-binary modal TUI editor written
in Rust, drop-in usable as a daily-driver Vim alternative for codebases
covered by its bundled language stack.

### Added

#### Modal editing
- Normal / Insert / Visual (char + line) / Command / Search / Picker /
  Prompt modes.
- Motions: word / WORD / end-of-word, line start/end, first/last non-blank,
  buffer first/last, `gg` / `G`, `H` / `M` / `L`, `f` / `F` / `t` / `T` and
  `;` / `,`, `n` / `N`, marks (`m{a-z}`, `'` / `` ` ``).
- Operators: `d`, `c`, `y` over motions, text objects, and linewise.
- Text objects: word, big-word, paragraph, quotes (`'"\``), brackets
  (`()[]{}<>`) — both `i` (inner) and `a` (around) variants.
- Indent / outdent (`>`, `<`, `>>`, `<<`) including visual-mode versions
  that keep the selection alive.
- Surround: `ds{c}`, `cs{old}{new}`, visual `S{c}` (Vim-surround style).
- Counts and registers — named, unnamed, and the `+`/`*` clipboard
  registers mirroring through to the OS clipboard via `arboard`.
- Macros (`q{a-z}` record, `@{a-z}` replay, `@@` replay last).
- `.` repeats the last edit, including full insert sessions.
- Jumplist with `Ctrl-O` / `Ctrl-I`.
- Undo / redo with persistent history at `~/.cache/binvim/undo/<sha>.json`
  (content-hash keyed, so out-of-band edits invalidate stale history).
- `Ctrl-A` / `Ctrl-X` increment / decrement on the line, recognising
  decimal, `0x…`, `0o…`, `0b…`, leading-zero padding, and a leading `-`
  when standalone.
- `~` toggle-case on the current char(s).
- `I` / `A` insert shortcuts.
- Indent-based folding with `za` / `zo` / `zc` / `zR` / `zM`.
- Yank flash — yanked range pulses Peach for 200ms.
- Smart Enter — copies leading whitespace, adds an indent unit after openers,
  splits `{|}` / `[|]` / `(|)` onto three lines with the cursor double-indented.
- Auto-pair brackets, quotes, backticks; auto-close HTML tags on typing `>`;
  comparison-aware guarding so `a < b` doesn't sprout a stray `>`.
- Matched bracket / matched HTML-tag highlight under the cursor.

#### Multi-buffer
- `:e` / `:bd` / `:bd!` / `:bn` / `:bp` / `:ls` / `:b{spec}`, plus the buffer
  picker (`<leader>b<space>`).
- Per-buffer state stash (cursor, viewport, history, marks, jumplist, folds,
  highlight cache).
- Auto-reload on external disk change for non-dirty buffers.
- Recent files at `~/.cache/binvim/recents`, surfaced in the file picker
  and as its own `<leader>?` shortcut.

#### Search & ex commands
- `/` / `?` / `n` / `N` / `*` / `#` with wrap-around, search-highlight, and
  `:noh` to silence it.
- `:s/pattern/replacement/[g]`, `:d`, `:y`, `:e`, `:goto`, `:health`,
  `:fmt` / `:format`.
- Ex range syntax: `1,5d`, `%s/foo/bar/g`, `.,$y`, etc.

#### LSP (hand-written JSON-RPC client over child-process stdio)
- Multiple servers per buffer: a primary (hover / goto / refs) plus
  auxiliaries (currently Tailwind on CSS/HTML/JSX/TSX/JS/TS/Astro/etc.,
  and csharp-ls layered onto Razor).
- Bundled server specs: rust-analyzer, typescript-language-server, Biome
  (JSON), gopls, vscode-html-language-server, vscode-css-language-server,
  astro-ls, csharp-ls (preferred for .cs/.vb), OmniSharp (fallback for
  .cs/.vb/Razor).
- Async `initialize` handshake — no blocking on the first open.
- 50ms `didChange` burst debounce so rapid typing doesn't flood servers.
- Drain cap on incoming traffic so diagnostic floods (OmniSharp) don't
  starve keyboard input.
- Tailwind detection covers v3 (`tailwind.config.{js,ts,cjs,…}`) and v4
  (`tailwindcss` listed in `package.json` (dev)dependencies).
- Capabilities: hover (`K`), goto-definition (`gd`), references (`gr`),
  document symbols (`<leader>o`), workspace symbols (`<leader>S`),
  completion (auto-triggered + `Ctrl-N`/`Ctrl-P`), signature help on
  `(` / `,`, code actions (`<leader>a`) including `workspace/executeCommand`
  + `workspace/applyEdit` round-trip, rename (`<leader>r`).
- Inline Error-Lens-style diagnostics with undercurl on the offending range.
- Hover popup: markdown cleanup, word-wrap, capped height, scrolling.
- Completion popup: prefix-tier ranking with substring + subsequence
  fallback; respects server `sortText` / `filterText`; preserves the
  popup across multi-server fan-out.
- `:health` opens a scratch buffer summarising version / pid / cwd /
  branch / config / CPU+RSS / open buffers / running LSPs / per-buffer
  attachment status / Tailwind config.

#### Tree-sitter highlighting
- Bundled grammars: Rust, TypeScript, JavaScript, JSX, TSX, JSON, HTML,
  CSS, Astro, Bash / Zsh, Markdown, C#, Razor.
- Capture priority by `pattern_index` — later patterns win, matching the
  upstream highlight-query convention. JSON ships a custom query (keys
  distinct from strings) because the upstream order is incompatible with
  this scheme.
- Catppuccin Mocha default theme; theme overrides via
  `~/.config/binvim/config.toml`. Capture-name resolution is dotted-prefix
  aware (`keyword.return` falls back to `keyword`).

#### Rendering & UI
- Powerline-style status line; floating command line / search prompt;
  notifications float top-right.
- Notification box wraps multi-row, caps at half terminal width, severity
  colours its border, click-to-copy lands the message in the clipboard
  and unnamed register.
- Mouse support: click, drag-select, wheel scroll (vertical + horizontal,
  with cursor drag-along).
- Horizontal viewport that follows the cursor; `zh` / `zl` / `zH` / `zL`
  for manual nudging.
- Visible tab / trailing-whitespace / EOL / non-breaking-space markers,
  configurable.
- Modal overlays dim the buffer behind them.
- Which-key popup after a 250ms leader hold.
- Start page (centred Mocha-Blue logo + configurable text) when launched
  with no path.

#### Pickers (one fuzzy engine, multiple roles)
- Files (`<leader><space>`), Buffers (`<leader>b<space>`), Grep
  (`<leader>g`, ripgrep-backed), Recents (`<leader>?`), Doc Symbols
  (`<leader>o`), Workspace Symbols (`<leader>S`), References, Code Actions.
- Skips `node_modules` and `.git/`, shows dotfiles.
- Yazi integration (`<leader>e`) — hands the terminal over and reclaims
  it cleanly on exit.

#### Save / format
- `:w`, `:wq`, `:q`, `:q!`.
- Format on save via Biome for JS / TS / JSX / TSX / JSON variants;
  resolved from project `node_modules/.bin/biome`.
- `.editorconfig` support: `indent_style`, `indent_size`,
  `insert_final_newline`, `trim_trailing_whitespace`.

#### Distribution
- Homebrew tap (`brew install bgunnarsson/binvim/binvim`).
- Linux install.sh (mirrored to `binvim-web` on each release).
- `bim` symlink installed alongside the `binvim` binary.
- binman-style local release scripts.

[Unreleased]: https://github.com/bgunnarsson/binvim/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/bgunnarsson/binvim/releases/tag/v0.2.1
[0.2.0]: https://github.com/bgunnarsson/binvim/releases/tag/v0.2.0
[0.1.8]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.8
[0.1.7]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.7
[0.1.6]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.6
[0.1.5]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.5
[0.1.4]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.4
[0.1.3]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.3
[0.1.2]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.2
[0.1.1]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.1
[0.1.0]: https://github.com/bgunnarsson/binvim/releases/tag/v0.1.0
