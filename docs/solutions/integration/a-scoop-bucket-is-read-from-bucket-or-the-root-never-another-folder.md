---
title: A Scoop bucket's manifests are read from its bucket/ folder or its root, never another folder, and fixing a manifest's content proves nothing about whether Scoop sees it
date: 2026-09-23
category: integration
module: bucket/, scripts/release.sh
paths:
  - bucket/**
  - scripts/release.sh
  - docs/install.md
tags: [scoop, bucket, windows, manifest, release, distribution, Find-BucketDirectory]
symptoms:
  - "Couldn't find manifest for 'binvim'"
  - "scoop install binvim finds nothing after scoop bucket add binvim https://github.com/bgunnarsson/binvim"
  - "release.sh bumps scoop/binvim.json every release and scoop update never moves"
root_cause: "the manifest shipped at scoop/binvim.json from 2026-05-20 to 0.7.0, and Scoop's Find-BucketDirectory resolves a bucket to <bucket>/bucket when that folder exists, else the repo root, so no folder named scoop/ is ever searched"
related:
  - docs/solutions/integration/a-catalog-installer-must-put-the-tool-on-path-and-exit-zero-with-nothing-to-do.md
---

## Problem

Tidying the repo root turned up `scoop/binvim.json`, documented in `docs/install.md` as the
manifest for `scoop bucket add binvim https://github.com/bgunnarsson/binvim`. Scoop's
source says it never looks in that folder, so a user following the documented two commands
has had nothing to install since the manifest was added (`2a3bf50`, 2026-05-20).

## What didn't work

- **`c695db6` fixed the manifest's content.** It found that `autoupdate` only rewrites a
  manifest when a bucket maintainer runs checkver, so it committed the version and hash by hand
  ("every Scoop user was pinned to 0.4.7"). The reasoning was about Scoop's semantics, read
  from its docs; nobody installed from the bucket to see which version arrived. The version
  moved, and Scoop still couldn't see the file.
- **`release.sh` step 5c rewrote the manifest for every release** from 0.6.4 on, and each
  commit said "Scoop manifest points at binvim X". A manifest's content being right, with
  a commit saying so, reads like evidence that the channel works.
- **The Windows on-device checklist** (`docs/windows.md`, "On-device verification") covers
  `install.ps1` end to end but has no Scoop row, so the 0.6 verification pass never reached it.

## Root cause

Verified from Scoop's `lib/buckets.ps1`:

```powershell
$bucket = "$bucketsdir\$Name"
if ((Test-Path "$bucket\bucket") -and !$Root) {
    $bucket = "$bucket\bucket"
}
```

A bucket is the cloned repo. Manifests are read from its `bucket/` folder when it exists,
otherwise from the root. `scoop/` matches neither.

Not observed: an install on a Windows machine, before or after the fix. That the documented
flow failed for users is inferred from the source, not from a report or a run.

## Fix

`76328fe` renames `scoop/` to `bucket/` and points `SCOOP_MANIFEST` in `scripts/release.sh`
and the link in `docs/install.md` at `bucket/binvim.json`. A bucket already added picks the
folder up on its next `scoop update`, because the lookup above runs on every manifest read.

## Prevention

- **The Scoop manifest lives at `bucket/binvim.json`.** A diff that moves it anywhere but
  `bucket/` or the repo root, or that points `SCOOP_MANIFEST` in `scripts/release.sh` at
  another folder, is a violation.
- **A fix to a distribution channel is checked by installing through that channel,** not by
  reading the package manager's docs. A commit to `bucket/binvim.json`'s fields that says
  what Scoop users now get, with no install run behind it (`scoop bucket add` from the repo URL,
  then `scoop install binvim` and `binvim` launching), states an unverified claim as fact.
- **An install path `docs/install.md` documents has a row in an on-device checklist.** A new
  install method added to `docs/install.md` without a matching row in `docs/windows.md`'s
  on-device list (or the macOS/Linux equivalent) is a violation.
