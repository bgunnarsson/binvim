---
title: A catalog installer must put the tool on PATH and exit zero when it has nothing to do, and a package's metadata proves neither
date: 2026-09-23
category: integration
module: src/install.rs
paths:
  - src/install.rs
  - docs/external-tools.md
tags: [install, winget, scoop, choco, brew, PATH, exit-code, UPDATE_NOT_APPLICABLE, pick_installer, build_update_plan, run_plan, catalog]
symptoms:
  - "✗ failed (exit code -1978335189)"
  - "clang-format reported failed right after clangd installed LLVM"
  - ":install finishes, but the tool is still missing and the first-run prompt comes back"
  - ":update runs winget upgrade on a tool scoop installed"
root_cause: "catalog entries were admitted on package metadata (winget's Commands field) instead of on whether the installer puts the binary on PATH, and run_plan assumed every manager exits 0 on a repeat install or an up-to-date upgrade, as brew does, while winget exits 0x8A15002B"
related:
  - docs/solutions/integration/a-tool-lookup-must-search-where-the-catalog-installs-it.md
---

## Problem

Adding winget / scoop / choco to the installer catalog (plan
`docs/plans/2026-09-23-installer-installs-through-winget-scoop-and-choco.md` (removed once done; `git show b98f700:docs/plans/2026-09-23-installer-installs-through-winget-scoop-and-choco.md`)) gave eight tools a
Windows path. Review found three ways it would fail on a real machine, although every package id
was real and every command was correct.

## What didn't work

- **Counting winget's `Commands` field as "on PATH".** The plan's rule was "a `portable` alias,
  or `Commands` naming the bin", so `LLVM.LLVM` (clangd, clang-format) and `Elixir.Elixir` (mix)
  went in ahead of scoop. winget's manifest schema says `Commands` "does not update the path"; it's
  search metadata. LLVM's installer is a WiX MSI with no PATH component (`llvm/CMakeLists.txt`,
  the WIX branch). Elixir's NSIS adds PATH only on a finish page that a silent install skips
  (`lib/elixir/scripts/windows_installer/installer.nsi`). Either install finishes and leaves
  `on_path` false, so the tool reads as missing forever. It also shadowed `Scoop("llvm")` /
  `Scoop("elixir")`, which do add PATH (`env_add_path: bin`). The choco `llvm` / `elixir`
  packages run those same installers.
- **Assuming "nothing to do" exits 0.** brew exits 0 on `brew install` of something installed
  and on `brew upgrade` of something current, so a tool sharing a package (clangd and
  clang-format both list LLVM) never showed. winget exits `APPINSTALLER_CLI_ERROR_UPDATE_NOT_APPLICABLE`
  (0x8A15002B, -1978335189 as `ExitStatus::code`) for an up-to-date `upgrade --id`. It turns a
  repeat `install` into that same upgrade (`UpdateFlow.cpp`), and `--no-upgrade` exits
  `PACKAGE_ALREADY_INSTALLED`, also non-zero. `run_plan` counts any non-zero exit as failed, so
  every current tool failed `:update`, and the second LLVM tool failed `:install`.
- **Upgrading through the first manager present.** `build_update_plan` used `pick_installer`,
  which is catalog order. With winget first and preinstalled on Windows, a `zig` from `scoop
  install zig` went to `winget upgrade`, which doesn't know it. The shape predates this change: a
  `stylua` from `cargo install` gets `brew upgrade`. Adding three more managers made it common.

## Root cause

Verified from source, not by running on Windows:

- winget's schema doc and `UpdateFlow.cpp` / `AppInstallerErrors.h` in `microsoft/winget-cli`.
- The LLVM WiX build and the Elixir NSIS script.
- The choco packages' `chocolateyinstall.ps1` on community.chocolatey.org. `zig` and
  `lua-language-server` unzip into the package, which choco shims onto PATH.

A catalog entry makes two promises that no package index states: that the binary ends up on the
`PATH` binvim probes, and that running the step again when there's nothing to do is not a
failure.

## Fix

Commit `ec7985b`, in `src/install.rs`:

- `clangd`, `clang-format` and `mix` get `Scoop` only. `Choco` stays for `zig` and
  `lua-language-server`.
- `Installer::is_noop_exit` treats winget's `UPDATE_NOT_APPLICABLE` as success.
- `run_plan` runs each distinct command once per plan and credits every tool sharing it.
- `:update` goes through `pick_update_installer`, which prefers the manager that `installed_by`
  reads off the binary's path (scoop's, winget's and choco's directories, `~/.cargo/bin`,
  Homebrew's prefix) and falls back to catalog order.
- A failed choco step says it needs an Administrator shell.

## Prevention

- **A new catalog entry cites how the package puts `bin` on `PATH`.** That means a portable
  alias, `env_add_path` / `bin`, a choco shim of an exe inside the package, or an installer
  script that adds PATH in a silent install. It never means a metadata field such as winget's
  `Commands`. The citation goes in the plan or the commit that adds it. An `Installer::Winget`
  for an MSI or NSIS package, or an `Installer::Choco` for a package that runs one, with no such
  citation, is a violation.
- **A new `Installer` variant states how its manager exits on a repeat install and on an
  up-to-date upgrade.** If either is non-zero, `is_noop_exit` covers the documented code, with a
  test. A variant added without a line saying which it is, or a `run_plan` change that treats
  every non-zero exit as failure again, is a violation.
- **A manager that installs into a directory of its own is added to `installed_by`** with a test
  path, so `:update` reaches it. A new manager name in `detect_managers` without an
  `installed_by` row (or a note that it has no directory of its own) is a violation.
