---
title: Is binvim without a cache directory a known data-loss path that blocks 1.0?
kind: grilling
mode: hitl
status: open
blocked_by: []
claimed_by:
---

## Question

The 1.0 bar says no known data-loss path. With no cache directory (no HOME, or `sudo binvim` where the cache belongs to another user), nothing is dumped for recovery and an in-place write is not copied aside first, so a write that fails partway leaves the file truncated. Decide whether 1.0 closes this (somewhere else to write, or refusing an unguarded in-place write) or accepts it as documented, and so what the roadmap's summary may claim.

## Context

- `docs/known-issues.md` Recovery section, `docs/data-loss-audit.md`.
- `src/paths.rs` `cache_dir`, `write_in_place`.
- `docs/solutions/conventions/an-audit-guarantee-is-read-from-the-code-not-from-the-lore-that-describes-it.md`: a summary claims no more than the weakest row.

## Answer

