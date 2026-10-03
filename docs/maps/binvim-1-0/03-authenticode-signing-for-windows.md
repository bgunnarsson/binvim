---
title: Does 1.0 ship a Windows binary signed with Authenticode, and through what?
kind: grilling
mode: hitl
status: open
blocked_by: []
claimed_by:
---

## Question

SmartScreen warns on first run of an unsigned exe, which the roadmap calls the biggest trust bounce for a new Windows user. Signing needs a certificate or a signing service, which costs money. Decide whether 1.0 signs, which route (an OV certificate, Azure Trusted Signing, SignPath for open projects or other), and who holds the credentials; or that 1.0 ships unsigned with the warning documented.

## Context

- `docs/windows.md` §3 (code-signing entry), `docs/roadmap.md` Windows workstream.
- `.github/workflows/release.yml`: archives are already cosign-signed; that is not Authenticode.
- `scripts/install.ps1` documents the warning today.
- The licence is source-available, not open source, which may rule out free signing programmes for open source.

## Answer

