---
title: A tool lookup must search where the catalog installs the tool, and a comment saying a package has no global install is a claim to check
date: 2026-09-23
category: integration
module: src/lsp/specs.rs, src/format.rs
paths:
  - src/lsp/specs.rs
  - src/format.rs
  - src/install.rs
tags: [biome, node_modules, PATH, find_node_modules_bin, find_on_path, which_in_path, install, catalog, formatter, lsp, health]
symptoms:
  - "biome not found in node_modules"
  - "<leader>f says biome is missing in a JS / TS / JSON buffer right after :install put it on PATH"
  - "no JSON language server attaches to a .json file although `which biome` finds one"
  - ":health lists biome as found while :fmt reports it missing"
root_cause: "the JSON server spec and the biome formatter searched only the project's node_modules/.bin, on a code comment claiming biome has no global install, while the catalog installs it with `npm install -g`, and :health's formatter probe searched PATH as well, so three probes for one tool resolved it three ways"
related:
  - docs/solutions/integration/a-catalog-installer-must-put-the-tool-on-path-and-exit-zero-with-nothing-to-do.md
  - docs/solutions/security/an-upward-search-trusts-what-other-users-own-or-can-write.md
---

## Problem

Auditing `docs/external-tools.md` against the source turned up a catalog entry and a lookup that
disagreed about the same tool. `BUNDLES` in `src/install.rs` installs biome with
`Installer::Npm(&["@biomejs/biome@2.4.10"])`, which `run_plan` runs as `npm install -g`. The
JSON language-server spec in `src/lsp/specs.rs` and `run_biome` in `src/format.rs` looked for
the binary only by walking up to the closest `node_modules/.bin/biome`. So `:install` finished,
biome sat on `$PATH`, and the two code paths that use it still reported it missing.

## What didn't work

Nothing — the first fix held. The mismatch was found by reading, not by a failed attempt: the
symptoms above are what the code paths produce (the formatter's old error string, the spec
returning `None`, `:health` probing `$PATH`), not something watched in a terminal.

## Root cause

Both lookups carried the same comment: "Biome doesn't support global installs — it lives in
node_modules." The package has installed globally for as long as the catalog has existed; the
catalog itself is the proof, and nothing in the repository or the package says otherwise. The
comment was a belief written as a fact, and once written it read as the reason the lookup stopped
at `node_modules`.

The inconsistency was already visible inside the repo. `primary_formatter_for_path` in
`src/format.rs`, which feeds `:health`, resolved biome as `node_modules/.bin` first and then
`$PATH`, so `:health` said the formatter was found while `<leader>f` said it wasn't. Prettier,
resolved in the same file, had the fallback all along.

## Fix

Commit `6c8c05d`, in `src/lsp/specs.rs` and `src/format.rs`: both lookups fall back to `$PATH`
after `node_modules/.bin`, the order prettier already used, so the version a project pins in
`node_modules` still beats a global one. The formatter's error names both install commands. The
comments on `find_node_modules_bin`, `run_biome` and the JSON spec now say why `node_modules`
comes first instead of claiming a global install is impossible.

## Prevention

- **The code that probes for a catalog tool's binary searches where every `Installer` entry for
  that tool puts it.** `Installer::Npm` is `npm install -g`, so the lookup checks `$PATH`; a
  project-local `node_modules/.bin` may come first, never alone. A `find_node_modules_bin(...)`
  whose result reaches `?` or `.ok_or_else(...)` with no `find_on_path` / `which_in_path` fallback,
  for a tool that has a `Tool { bin: ... }` in `BUNDLES`, is a violation. So is a new `Tool`
  whose only installer lands somewhere no lookup for that `bin` searches.
- **One tool, one resolution.** `:health`'s probe (`primary_formatter_for_path`,
  `primary_spec_for_path`), the formatter's `run_*` and the server spec resolve a binary in the
  same order. A diff that adds a fallback to one of them and not the others is a violation, since
  `:health` then reports found for a tool the editor won't run.
- **A comment saying a package "has no global install" or "must live in `node_modules`" cites
  where that is documented.** The catalog installing it globally contradicts the claim outright.
  A new comment of that shape with no citation, or one that survives a catalog entry installing
  the tool globally, is a violation.
