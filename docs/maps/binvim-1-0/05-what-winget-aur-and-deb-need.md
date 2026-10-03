---
title: What do winget, AUR and .deb/apt each need from the release, and which can be automated?
kind: research
mode: afk
status: resolved
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

All three can be driven from the release, and each needs one thing from the user's own account.
- winget: a portable zip manifest from the existing Windows zip, submitted once by hand, then updated by winget-releaser on each tag. It needs a winget-pkgs fork and a classic PAT.
- AUR: a `binvim-bin` PKGBUILD over the existing Linux tarballs, pushed by deploy-aur. It needs an AUR account's SSH key.
- .deb: `cargo deb --no-build` over the existing musl binaries, attached to the release. A real apt repository needs a GPG key and hosting, and is a separate choice.

What the release has today (scout):
- `release.yml` builds `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` (`.tar.gz`) and `x86_64-pc-windows-msvc` (`.zip`). It builds no macOS archives. Each archive gets a `.sha256` sidecar and a cosign `.bundle` (lines 29-46, 92-157).
- `scripts/release.sh` publishes crates.io (line 577), updates `bucket/binvim.json` (line 650) and the Homebrew tap (line 720), and mirrors `install.sh` to binvim-web.
- `Cargo.toml` has no `[package.metadata.deb]`. `docs/windows.md:47` still names the winget path at 0.4.7.

Per channel:
- **winget.** The manifests are the version, installer and defaultLocale files under `manifests/b/<Publisher>/Binvim/<version>/`, schema 1.12.0. A zip uses `InstallerType: zip`, `NestedInstallerType: portable` and `NestedInstallerFiles` pointing at `binvim.exe`.
  - The first PR is made by hand (`wingetcreate new` or komac).
  - Later versions come from [winget-releaser](https://github.com/vedantmgoyal9/winget-releaser) with `installers-regex: '\.zip$'`. It needs a classic PAT with `public_repo` and a fork of `microsoft/winget-pkgs` under the same account. It runs on Linux runners.
  - Unsigned portable binaries are commonly accepted, but a Defender flag can hold a PR. That is unverified, and it bears on ticket 03.
- **AUR.** Start with `binvim-bin`: `source_x86_64` / `source_aarch64` point at the release tarballs, with `provides`/`conflicts=(binvim)` and `license=('custom')` plus the LICENSE file, since the licence is source-available.
  - `.SRCINFO` is generated with `makepkg --printsrcinfo`.
  - [github-actions-deploy-aur](https://github.com/KSXGitHub/github-actions-deploy-aur) pushes a PKGBUILD the workflow renders with the tag and sha256. It needs an AUR account, a registered SSH key stored as a secret, and a first push by hand.
  - A source package `binvim` (`cargo build --release --locked`) is optional.
- **.deb.** [cargo-deb](https://github.com/kornelski/cargo-deb) with a `[package.metadata.deb]` section. `--no-build` reuses the built binary.
  - The release binaries are musl and static, so the glibc-baseline concern the researcher raised does not apply.
  - A `.deb` on the release installs with `apt install ./binvim_*.deb`, but it gets no `apt upgrade`.
  - An apt repository (reprepro or aptly on GitHub Pages, Cloudsmith or packagecloud) needs a GPG key and a host. A Launchpad PPA needs a Debian source package per series and is a poor fit.

From the researcher's memory, not fetched: the zip/portable schema details, the validation pipeline and SmartScreen behaviour, the AUR policy, and the apt hosting details. Sources: [winget manifest](https://learn.microsoft.com/en-us/windows/package-manager/package/manifest), [installer schema 1.12.0](https://github.com/microsoft/winget-pkgs/blob/master/doc/manifest/schema/1.12.0/installer.md), [winget-create](https://github.com/microsoft/winget-create), [AUR submission guidelines](https://wiki.archlinux.org/title/AUR_submission_guidelines).
