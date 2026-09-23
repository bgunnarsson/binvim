---
title: The installer installs through winget, scoop and choco on Windows
date: 2026-09-23
status: draft
---

## Context

This is the last actionable 0.7 Windows item (`ROADMAP.md`, "Feature parity (0.7)"; `WINDOWS.md`
section 2). The task runner and the AI panes shipped in
`2026-09-22-tasks-and-ai-panes-start-under-cmd-and-powershell.md`. Full SCSS highlighting is still
blocked upstream: crates.io has only `tree-sitter-scss` 1.0.0, so that stays a `WINDOWS.md` note.

`pick_installer` (`src/install.rs:621`) takes the first installer whose manager `detect_managers`
(`src/install.rs:613`) found on `PATH`. The `Installer` enum (`src/install.rs:54-84`) has no
Windows-native manager. So on Windows, a tool whose only automatic paths are `Brew` / `BrewCask`
/ `Apt` lands on `Choice::NoManager`, even when winget has it. Thirteen tools are in that state:
`lldb-dap`, `clangd`, `clang-format`, `lua-language-server`, `marksman`, `jdtls`,
`google-java-format`, `zls`, `zig`, `elixir-ls`, `mix` (Elixir), `kotlin-language-server` and
`ktfmt`.

Decisions:

- **Three variants, each carrying one package id:** `Winget(&str)`, `Scoop(&str)`, `Choco(&str)`.
  Each has a `manager()` (`"winget"` / `"scoop"` / `"choco"`) and arms in `display`,
  `build_command`, `upgrade_command` and `upgrade_display`, following `Brew`'s shape.
- **Unpinned, like brew, nix and apt.** Each manager owns its version. The `CLAUDE.md` pinning
  rule covers npm / go / pipx / gem / dotnet / composer / cargo, and binvim.dev's table pins none
  of these three.
- **The commands:**
  - winget: `winget install --id <id> --exact --accept-source-agreements
    --accept-package-agreements`, and `winget upgrade --id <id> --exact …` to upgrade.
  - scoop: `scoop install <app>`, and `scoop update <app>` to upgrade.
  - choco: `choco install <pkg> -y`, and `choco upgrade <pkg> -y` to upgrade.
  - Each is spawned directly (`Command::new("winget")`), as `Brew` is. `run_plan` already
    inherits stdio, so choco's elevation prompt reaches the user.
- **Scoop entries come from the `main` bucket only.** `build_command` returns one `Command`, and
  an app in `extras` would first need `scoop bucket add extras`. Where only `extras` has a tool,
  the `Tool` gets no `Scoop` entry.
- **Order within a `Tool`:** after the existing entries, as `winget`, then `scoop`, then `choco`.
  macOS and Linux never find these managers, so their plans don't change. On Windows, winget is
  preinstalled on 10/11 and runs without admin, so it wins when present.
- **`sdkmanager` and `adb` stay as they are.** winget's Android packages are a zip-extraction
  shape, unlike the rest of the catalog. The Android bundle keeps its `Manual` fallback.
- **An id is only added once it's confirmed in the manager's own index.** That means
  `microsoft/winget-pkgs` manifests, `ScoopInstaller/Main`'s `bucket/`, and
  `community.chocolatey.org/packages`. A tool with no confirmed id in a manager gets no entry
  there.

## Relevant lore

None found. No doc covers the installer catalog, and `detect_managers` does no upward search, so
the `others_can_plant` rule
([an-upward-search-trusts-what-other-users-own-or-can-write](../solutions/security/an-upward-search-trusts-what-other-users-own-or-can-write.md))
doesn't bind here.

## Acceptance criteria

- `detect_managers` probes `winget`, `scoop` and `choco`.
- With only `winget` on `PATH`, `build_plan` for the Zig bundle resolves `zig` and `zls` to
  `Choice::Install(Installer::Winget(…))` rather than `NoManager`.
- `display()` / `upgrade_display()` for the three variants print the exact commands above.
- The macOS / Linux plan for every bundle is unchanged: no existing `Tool`'s first matching
  installer moves.
- `docs/external-tools.md` shows the Windows command beside the brew / apt one for each
  tool that gained an entry. `WINDOWS.md` section 2's installer item is ticked, with prose
  describing what shipped.

## Tasks

- [ ] Look up and record the package id for each of the 13 tools in winget, scoop `main` and
  choco, from the three indexes named under Decisions. Write the table (tool → id or "none") into
  this plan under a new `## Package ids` section. Verify each id against its manifest or package
  page URL, and cite that URL in the table.
- [ ] Add the `Winget` / `Scoop` / `Choco` variants with their `manager`, `display`,
  `build_command`, `upgrade_command` and `upgrade_display` arms, and add the three names to
  `detect_managers`. Verify with new `install::tests` cases asserting each variant's `display()`
  and `upgrade_display()` strings, extending `upgrade_display_uses_manager_upgrade_verbs`.
- [ ] Append the confirmed ids to the 13 `Tool`s in `BUNDLES`. Verify with a test that, for every
  bundle, `pick_installer` under a manager set of `{brew, npm, cargo, go, pipx}` picks the same
  installer as before. The test compares against a `Choice` list captured before the change, so
  the macOS path is pinned. Add a second test: under `{winget}`, each of the 13 tools picks
  `Winget`, or has no winget id in the table.
- [ ] Update `docs/external-tools.md` with the Windows command
  per tool, and tick `WINDOWS.md`'s installer item with prose saying what shipped. Verify by
  reading the diff.
- [ ] Update binvim.dev's install table in the sibling `binvim-web` repo to match. Verify by
  reading its diff, and note in the report that the site isn't live until the user redeploys it
  in Dokploy.

## Files

- `src/install.rs`: `Installer` (54-84), `manager` (88), `display` (110), `build_command` (140),
  `upgrade_command` / `upgrade_display` (~213 / ~265), `detect_managers` (613), `BUNDLES` (328+),
  `mod tests` (1280+). Reuses `pick_installer` and `build_plan` unchanged.
- `docs/external-tools.md`, `WINDOWS.md`, `CHANGELOG.md` (Unreleased `### Added`).
- `binvim-web`: its install table page.

## Verification

- `cargo test install::tests`, then `cargo test -- --test-threads=1` and `cargo +1.98.0 clippy
  --locked --all-targets -- -D warnings`. CI green on `windows-latest`, which compiles and lints
  the new arms.
- On macOS, `binvim-install` lists the same plan for the Zig bundle as before the change (compare
  its output from before and after).
- On the user's Windows machine: `:install`, select Zig, and confirm the plan shows `winget
  install --id … zig` and `zls`. Run it and check `zig version` works. Repeat with `choco` only on
  `PATH` for one tool.
