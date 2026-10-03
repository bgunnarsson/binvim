---
title: binvim 1.0 is released with nothing left on the roadmap
date: 2026-10-03
status: active
---

## Destination

binvim 1.0 is tagged and released, and `docs/roadmap.md` has nothing left in it.
The 1.0 bar holds and has evidence for each part:

- First run works with zero manual config for a defined set of supported stacks.
- The terminal matrix in `docs/terminals.md` has every column filled.
- A written correctness suite is green.
- No known data-loss path is left open.
- Startup and input-latency budgets are met and published on a benchmark page.
- clippy stays a hard gate.
- Distribution covers winget, AUR and .deb/apt, and the winget submission is made.
- Windows is at parity.
- A demo recording is on the README and binvim.dev.
- Notebooks phase 3 has shipped: image outputs drawn through the kitty and iTerm2
  graphics protocols, with a text fallback, and kernels other than Python.
  Notebooks phase 3 is part of this map but does not hold up the 1.0 tag.

Not part of it: a plugin API or Lua, languages added for the count, telemetry,
and the Windows deferrals listed under Out of scope.

## Notes

- binvim stays closed and single-binary: everything is in-tree or driven by
  `config.toml` (`docs/roadmap.md`, "What we explicitly will not do").
- Open questions about what the 1.0 bar means are grilling tickets the user
  answers, not settled by the lead.
- Terminal checks run on the user's machines; the lead fills rows from their
  reports (`scripts/terminal-probe.sh`, `docs/terminals.md`).
- A summary line (roadmap, README, release note) claims no more than the
  evidence's weakest row (`docs/solutions/conventions/an-audit-guarantee-is-read-from-the-code-not-from-the-lore-that-describes-it.md`).
- binvim.dev lives in the sibling repo binvim-web and deploys by hand from Dokploy.
- Release archives are already cosign-signed (`.github/workflows/release.yml`);
  Authenticode is the separate question in ticket 03.
- State as charted: brew, scoop (`bucket/binvim.json`), nix, crates and curl
  exist; winget, AUR and .deb do not. No benchmark, budget or demo recording
  exists. Only Rust, Go, Python, C/C++ and C# have LSP + debugger + formatter +
  tree-sitter; Razor, TOML and Docker also lack a formatter.

## Decisions so far

- [04](04-how-to-measure-startup-and-input-latency.md): hyperfine to first frame, a pty harness judged on the parsed screen, and sampled RSS, same machine and files for both editors; CI runs ratios only, published numbers come from the user's hardware.
- [05](05-what-winget-aur-and-deb-need.md): winget (portable zip + winget-releaser), AUR (`binvim-bin` + deploy-aur) and .deb (`cargo deb --no-build`) can all be driven from the release; each needs one thing from the user's own account.
- [07](07-drawing-images-on-the-notebook-page.md): kitty Unicode placeholders, then iTerm2 inline images when wholly visible, then half-blocks, then the label; no sixel yet.
- [08](08-starting-kernels-other-than-python.md): the Python bridge stays and starts a non-Python kernel by `metadata.kernelspec.name` through the stock `KernelSpecManager`.

## Not yet specified

- The tests that make the correctness suite green: waits on 02.
- Filling the zero-config gaps (debuggers, formatters, first-run checks) for
  the supported stacks: waits on 01.
- Closing or accepting the no-cache-directory recovery gap: waits on 10.
- The budget numbers and the benchmark page: waits on 11's first numbers.
- Building each distribution channel, and the winget submission: waits on 12.
- Wiring Authenticode into `release.yml`: waits on 03.
- Building and placing the demo recording: waits on 09.
- Drawing image outputs on the notebook page, with the iTerm2 and half-block
  fallbacks: waits on 13.
- Choosing and starting a non-Python kernel, and what follows its language:
  waits on 14.
- Full SCSS highlighting on Windows: waits on an upstream `tree-sitter-scss`
  release (1.0.1), then it is a one-line cfg removal (`docs/windows.md`).
- The 1.0 release itself: the roadmap and README brought up to date, the
  changelog, the tag. Waits on every ticket above.

## Out of scope

- MSI / MSIX installer, PowerShell as the default shell, WSL path translation:
  deferred until asked (`docs/windows.md` §3), settled with the user 2026-10-03.
- A plugin API, Lua, languages for the count, telemetry: the roadmap rules
  them out.
- Fixing `tree-sitter-bash`'s scanner on adversarial Unicode: upstream's to fix;
  binvim fuzzes it over ASCII until then (`docs/known-issues.md`).
