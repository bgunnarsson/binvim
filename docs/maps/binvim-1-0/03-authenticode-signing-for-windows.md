---
title: Does 1.0 ship a Windows binary signed with Authenticode, and through what?
kind: grilling
mode: hitl
status: resolved
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

1.0 ships the Windows binary unsigned, with the SmartScreen warning documented; signing is held off, settled with the user 2026-10-03.

- The user held off all four questions: whether to sign, the route, who holds the credentials, and asking Certum about its open-source certificate. Wiring Authenticode into `release.yml` leaves the map for Out of scope, and the 1.0 roadmap drops code-signing from its Distribution & trust workstream.
- What the routes looked like, for whoever takes this up again (researcher, fetched 2026-10-03):
  - Azure Artifact Signing (formerly Trusted Signing, GA January 2026, $9.99/month, [Azure/artifact-signing-action](https://github.com/Azure/artifact-signing-action) with OIDC) takes individuals only in the US and Canada, and organisations in a list that leaves out Iceland ([quickstart](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart)). Users have reported SmartScreen warnings on each release since its CA rotation in March 2026 ([action issue #128](https://github.com/Azure/artifact-signing-action/issues/128)).
  - SignPath Foundation needs an OSI licence ([terms](https://signpath.org/terms.html)), which the source-available licence is not. No free programme for source-available projects was found.
  - The route left is a cloud OV certificate: Certum SimplySign for an individual is about $139/year ([SSLmentor](https://www.sslmentor.com/certum/certumcodecloudindividual)), signed from CI by community tools that keep the TOTP seed as a secret. Certum's €49 open-source certificate may need an open-source licence; only Certum can say.
  - No certificate, EV included, stops the warning at once: reputation builds over downloads ([Microsoft](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options)).
- Why unsigned is bearable: SmartScreen checks only files with Mark of the Web. Explorer passes it to a zip's contents; `install.ps1`'s `Expand-Archive` reportedly does not, and scoop and winget probably do not (unconfirmed). The warning mostly meets someone who downloads the zip by hand.
- winget does not require signing, but an unsigned exe can be held by its URL reputation or Defender check ([ValidationFailureGuide](https://github.com/microsoft/winget-pkgs/blob/master/doc/ValidationFailureGuide.md)). The fix there is asking the moderators or a Defender false-positive submission; that belongs to 12's submission.
- "Documented" is not yet true: the ticket's context said `scripts/install.ps1` documents the warning, but it does not. Only `docs/windows.md:48` mentions SmartScreen, as a to-do. Ticket 24 writes it.
