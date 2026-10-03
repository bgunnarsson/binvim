---
title: What do winget, AUR and .deb/apt each need from the release, and which can be automated?
kind: research
mode: afk
status: open
blocked_by: []
claimed_by:
---

## Question

Distribution breadth for 1.0 adds winget, AUR and .deb/apt. Find what each channel requires (winget manifests and the winget-pkgs PR flow, an AUR PKGBUILD for a binary or source package, cargo-deb or similar and whether an apt repository or a .deb on the GitHub release is enough), what `release.yml` and `scripts/release.sh` already produce that each can use, and what needs the user's accounts.

## Context

- `.github/workflows/release.yml`, `scripts/release.sh`, `bucket/binvim.json`, `scripts/homebrew/`, `flake.nix` as the existing channels.
- `docs/windows.md` §3 winget entry; `docs/install.md`.
- `docs/solutions/integration/a-scoop-bucket-is-read-from-bucket-or-the-root-never-another-folder.md`.
- winget-pkgs, AUR and Debian packaging docs on the web; cite links.

## Answer

