# binvim roadmap

The two goals through 1.0 are **grow adoption** and **reach 1.0 quality**, while
staying what binvim is: a **closed, curated, single-binary vim IDE** — no plugin
system, no Lua, everything hard-wired or driven by `config.toml`.

## The strategic through-line

Most editors grow through their ecosystem (Neovim's plugins, VS Code's
marketplace). binvim has deliberately given that up. So the growth lever is the
thing ecosystem editors are bad at: **zero-config depth.** The pitch is one
sentence — *install one binary, open your repo, and a full IDE for your stack is
just working: no config, no plugin hunting* — and the whole roadmap exists to
make that sentence true and provable.

Consequences that hold for every item below:

- Breadth of languages is no longer the game. **Depth** is: does every advertised
  language have LSP + DAP + formatter + tree-sitter, all working with zero config?
- Config discoverability **is** the extension story, because it's the only one.
  It has to feel first-class.
- The differentiator is measurable *fast, light, and it-just-works* — so we
  produce numbers, not adjectives.

The arc: **Horizon 1 makes it convert · Horizon 2 makes it trustworthy · Horizon 3
makes it visible** — all without opening the binary up.

---

## Horizon 1 — "Nothing bounces on first run"  (shipped in 0.6)

The biggest adoption leak for a closed IDE: someone `brew install`s it, opens a
file, and completion / highlighting / format isn't there because the toolchain
isn't installed. `binvim-install` and the `:install` overlay exist, but they're a
*second, deliberate* step. Close that gap.

- **First-run detection → guided setup.** On opening a repo, detect the stack
  (already done for packages/DAP) and, if the matching LSP/DAP/formatter is
  missing, surface a one-keystroke "install the toolchain for this project?"
  prompt instead of silent nothing. Fire the `:install` machinery *contextually*.
- **A prescriptive "why isn't this working?" surface.** Make `:health` the thing
  every confused newcomer is pointed at, and make it *fix*, not just diagnose
  ("csharp-ls not found → press `y` to install").
- **Config ergonomics** (config is the only extension point): `:config` to
  open + live-reload `config.toml`, schema validation with inline errors, and
  `:config default` to dump the annotated defaults.

## Horizon 2 — 1.0 hardening  (target: 0.7)

1.0 is a promise: *this won't lose your work or fall over.* Audit against it
literally.

- **Data-loss & crash resilience.** Pressure-test `crash.rs` + persistent undo:
  crash recovery of unsaved buffers, atomic saves, session robustness when files
  vanish or change underneath. This bug class ends adoption permanently.
  The audit, path by path with the evidence for each, is
  [`docs/data-loss-audit.md`](docs/data-loss-audit.md) (2026-09-23: no path loses
  text; one display gap, a deleted clean file isn't marked as gone).
- **Correctness on hostile input.** Grapheme clusters / wide chars / emoji /
  mixed EOL / very long lines / huge files (a "large file mode" that degrades
  tree-sitter + LSP gracefully rather than stalling). Extend the density of the
  motion/text-object test suites to rendering and width math. Includes the
  known `tree-sitter-md` scanner crash: a narrow `isdigit` on a Unicode
  codepoint in an ordered-list marker (`1€` at line start) aborts on glibc.
  Bounded out of the fuzz suite for now; wants an upstream fix or a vendored
  grammar to close for real.
- **Terminal compatibility matrix.** Test + document Ghostty, Kitty, WezTerm,
  Alacritty, tmux, Windows Terminal, and over-SSH. The published matrix doubles
  as a hardening checklist and marketing.
- **Performance budget with numbers.** Formalize the render-coalescing win into
  input-latency and startup-time budgets, and a benchmark page (startup + memory
  vs a Neovim distro like LazyVim). The clippy gate is on — pinned to 1.98.0,
  warnings denied — and stays part of the 1.0 bar.

## Horizon 3 — Adoption proof & reach  (0.8–0.9, in parallel)

- **Distribution breadth.** Already: brew / scoop / nix / crates / curl. Add the
  reach gaps: `winget`, AUR, `.deb`/apt.
- **Proof assets.** A 30-second demo GIF / asciinema on the README and site; the
  benchmark numbers from Horizon 2; and "coming from Neovim / from VS Code"
  *migration* guides to pair with the existing comparison pages (convert intent
  into a first session).
- **One or two headline features only a closed IDE does well** — integrated
  because we own the whole binary. Candidates already latent: the AI side panes
  (`:claude` / `:codex`) and DAP-across-four-runtimes. Polish them into headline
  features, not footnotes.

## Windows: first-class parity  (cross-cutting, lands across 0.6–1.0)

The Windows port is mostly shipped — WS1–8 are done, the editor builds, tests, and
runs on `x86_64-pc-windows-msvc`, and CI exercises every push against
`windows-latest` alongside ubuntu + macos. The detailed tracker lives in
[`WINDOWS.md`](WINDOWS.md); the roadmap-level commitment is to close the gap
between "compiles and unit-tests pass in CI" and "a Windows developer gets the
same zero-config IDE a macOS developer does." Three tiers, the first done:

- **On-device verification (0.6) — done.** `install.ps1` end-to-end, LSP
  discovery, DAP launch, `:terminal` (ConPTY), CRLF round-trip and
  second-instance recovery all passed on a real Windows machine.
- **Feature parity (0.7).** Ship the flows that assume a POSIX shell: per-shell
  dispatch for the **task runner** and **AI side panes** (`/C` for cmd.exe,
  `-Command` for pwsh, untouched for bash) and the cmd.exe variant of
  `shell_quote` — done, through `terminal::shell_launch` — then
  native **winget / scoop / choco** support in `binvim-install` + `:install`
  (three `Installer` variants + `detect_managers` probes), and full SCSS
  highlighting once `tree-sitter-scss` cuts a release with the MSVC fix.
- **Distribution & trust (0.8–1.0).** winget submission to `microsoft/winget-pkgs`,
  and **code-signing the Windows binary** so SmartScreen stops warning on first
  run — the single biggest trust bounce for a new Windows user. MSI/MSIX,
  PowerShell-as-default-shell, and WSL path translation stay deferred until asked.

Zero-config depth has to mean *on Windows too*, or the adoption pitch has an
asterisk. Windows parity is therefore part of the 1.0 quality bar, not a nice-to-have.

## 1.0 — the quality bar

1.0 ships when: first run installs and works with zero manual config for the
supported stacks; no known data-loss path; the correctness + terminal matrices
are green; the performance budget is met and published; clippy is a hard gate; and
Windows reaches feature parity (see the Windows workstream above).

---

## What we explicitly will not do

- **No plugin API, no Lua.** Every "can I add X?" is answered by adding X in-tree
  or via config — never by an extension API. This is the differentiator; guard it.
- **No new languages purely for the count.** Breadth is table-stakes. Depth —
  every advertised language with the full LSP + DAP + formatter + tree-sitter
  quartet, zero-config — beats a 40th language.
- **No telemetry that compromises the privacy / single-binary story.** If we want
  an adoption signal, it's opt-in.

---

## Milestone summary

| Version | Theme | Ships when |
|---------|-------|------------|
| 0.6 (shipped) | First-run setup | Contextual toolchain install, prescriptive `:health`, `:config` live-reload + validation; Windows on-device verification |
| 0.7 | Hardening | Crash/data-loss audit clean, correctness suite green, terminal matrix documented; Windows feature parity (task runner, AI panes, winget/scoop/choco) |
| 0.8–0.9 | Perf & proof | Published benchmarks, demo assets, migration guides, `winget`/AUR/apt; winget submission + Windows code-signing |
| 1.0 | Quality bar | Zero-config first run, no data-loss path, matrices green, perf budget met, clippy gated, Windows at parity |
