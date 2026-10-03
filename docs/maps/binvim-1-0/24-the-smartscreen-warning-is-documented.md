---
title: The SmartScreen warning on the unsigned Windows binary is documented where Windows users install
kind: task
mode: afk
status: open
blocked_by: [03]
claimed_by:
---

## Question

[03](03-authenticode-signing-for-windows.md) ships 1.0 unsigned with the SmartScreen warning documented, but nothing documents it yet. Say, where a Windows user installs binvim, that SmartScreen may show "Windows protected your PC" for a zip extracted in Explorer, why (the binary is unsigned; the release archives are cosign-signed and can be verified), and how to proceed (More info, then Run anyway, or verify first). Turn the code-signing entry in `docs/windows.md` from a to-do into a decision.

## Context

- README.md:52 Windows install line, and `docs/windows.md:48` code-signing entry.
- `scripts/install.ps1`: its own path probably avoids the warning, since `Expand-Archive` reportedly drops Mark of the Web; say only what holds.
- `.github/workflows/release.yml:135-157` cosign verification, for the "verify first" line.
- binvim.dev's install page in the sibling repo binvim-web, which deploys by hand from Dokploy.

## Answer
