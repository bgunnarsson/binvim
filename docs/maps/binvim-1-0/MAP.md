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
  exists. Only Go and Python have LSP + debugger + formatter + tree-sitter all
  installed by first run; Rust lacks a formatter arm, C#'s debugger is a manual
  install, C/C++ debugs only under Cargo, and Docker lacks a formatter (ticket 01).

## Decisions so far

- [04](04-how-to-measure-startup-and-input-latency.md): hyperfine to first frame, a pty harness judged on the parsed screen, and sampled RSS, same machine and files for both editors; CI runs ratios only, published numbers come from the user's hardware.
- [05](05-what-winget-aur-and-deb-need.md): winget (portable zip + winget-releaser), AUR (`binvim-bin` + deploy-aur) and .deb (`cargo deb --no-build`) can all be driven from the release; each needs one thing from the user's own account.
- [07](07-drawing-images-on-the-notebook-page.md): kitty Unicode placeholders, then iTerm2 inline images when wholly visible, then half-blocks, then the label; no sixel yet.
- [08](08-starting-kernels-other-than-python.md): the Python bridge stays and starts a non-Python kernel by `metadata.kernelspec.name` through the stock `KernelSpecManager`.
- [01](01-which-stacks-are-supported-for-zero-config.md): 1.0 promises six stacks, Rust, Go, Python, C/C++, C#/.NET and TypeScript/JavaScript (with HTML, CSS and JSON), each with LSP + debugger + formatter + tree-sitter installed from the first-run prompt with no manual step on macOS, Linux and Windows, shown by a scripted fresh-environment run per stack per OS; every other language is "also works", outside the promise.
- [02](02-what-the-correctness-matrix-is.md): The correctness matrix is `docs/correctness.md`, eight hostile inputs by six areas, and a cell is green only when a named `cargo test` test (an example or a proptest property) covers that input in that area, or it is marked n/a with a reason; settled with the user 2026-10-03.

## Not yet specified

- Building the TS/JS debugger: waits on 16.
- Installing netcoredbg automatically: waits on 17.
- Building C/C++ debugging outside Cargo: waits on 18.
- Building the first-run check and filling its rows on each OS: waits on 20.
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
- The 1.0 release itself: the roadmap and README brought up to date (naming
  the six promised stacks and the "also works" rest), the changelog, the tag. Waits on every ticket above.

## Out of scope

- MSI / MSIX installer, PowerShell as the default shell, WSL path translation:
  deferred until asked (`docs/windows.md` §3), settled with the user 2026-10-03.
- A plugin API, Lua, languages for the count, telemetry: the roadmap rules
  them out.
- Razor in the .NET promise, and a debugger for the "also works" languages:
  outside the six promised stacks, settled with the user 2026-10-03 (ticket 01).
- A speed limit on the correctness matrix's large-file and long-line rows: they
  prove correctness only; speed is 04's budgets (ticket 02).
- Keeping each line's own ending when a mixed CRLF/LF file is saved: the majority
  ending is accepted, as in Vim (ticket 02).
- Fixing `tree-sitter-bash`'s scanner on adversarial Unicode: upstream's to fix;
  binvim fuzzes it over ASCII until then (`docs/known-issues.md`).
