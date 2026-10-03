---
title: Which of winget, AUR and .deb does 1.0 build, in what form, and with whose accounts?
kind: grilling
mode: hitl
status: open
blocked_by: [05]
claimed_by:
---

## Question

[05](05-what-winget-aur-and-deb-need.md) found each channel can be automated, but needs the user's account and form choices:
- winget: a winget-pkgs fork and a classic PAT. Does it wait on Authenticode ([03](03-authenticode-signing-for-windows.md)) or go unsigned?
- AUR: `binvim-bin` only, or a source `binvim` too. It needs an AUR account and SSH key.
- .deb: a `.deb` on the GitHub release only, or a signed apt repository, and on which host?

## Context

- [05](05-what-winget-aur-and-deb-need.md)'s answer.
- `LICENSE` (source-available) for the AUR `license` field and any host's open-source tier.

## Answer

