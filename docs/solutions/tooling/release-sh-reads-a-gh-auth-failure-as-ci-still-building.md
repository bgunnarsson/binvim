---
title: release.sh checks only that gh is on PATH and reads every failing gh call as "CI still building", so a stale GITHUB_TOKEN ends a release at exit 0 with its GitHub steps skipped
date: 2026-09-26
category: tooling
module: scripts/release.sh
paths:
  - scripts/release.sh
tags: [release, gh, github-token, silent-failure]
symptoms:
  - "Could not find a release.yml run for v0.7.2 after ~60s. Skipping wait."
  - "Release v0.7.2 not yet published (CI still building?). Skipping notes update."
  - "binvim-v0.7.2-x86_64-pc-windows-msvc.zip.sha256 is not on Release v0.7.2 (CI still building?)."
  - release.sh exits 0, the tag and crates.io publish are done, but the Release keeps its auto-generated notes and bucket/binvim.json still names the previous version
root_cause: gh prefers GH_TOKEN / GITHUB_TOKEN over its keyring login, a stale GITHUB_TOKEN in the shell made every gh call fail with HTTP 401, and release.sh's gh pre-flight is `command -v gh` while each gh step discards stderr and treats failure as "not built yet"
related:
  - docs/solutions/integration/a-scoop-bucket-is-read-from-bucket-or-the-root-never-another-folder.md
---

## Problem

Cutting 0.7.2 with `scripts/release.sh 0.7.2 --yes` ran fmt, tests, the bump
commit, `cargo publish`, the tag push, the Homebrew tap and the binvim-web
mirror, and exited 0. The three steps that go through `gh` did nothing: the CI
wait, the CHANGELOG-to-Release-notes push and the Scoop manifest bump. Each
printed a message blaming a slow CI run.

## What didn't work

Reading the messages at face value. "CI still building?" is what the script
says for any failing `gh` call, so waiting and re-running `--notes-only` in the
same shell would have failed the same way.

## Root cause

`gh` takes a token from `GH_TOKEN` or `GITHUB_TOKEN` before the credential
stored by `gh auth login`. The shell running the release had a stale
`GITHUB_TOKEN`, so every `gh` call got HTTP 401. Unsetting it made the same
commands work. That is verified; where the stale token came from was not
traced. The exact 401 text wasn't captured, because the script sends it to
`/dev/null`.

The script turns the 401 into a skip:

- The pre-flight (`scripts/release.sh:408`) checks `command -v gh` only, so
  missing auth passes, while crates.io auth is checked with a real API call.
- The CI wait (`:593`) runs `gh run list … 2>/dev/null || true`, so an empty
  `RUN_ID` looks like "run not registered yet" and the wait is skipped (`:599`).
- The asset check (`:618`) runs `… 2>/dev/null || echo 0` and only warns.
- `push_release_notes soft` (`:216`) reads a failed `gh release view` as
  "not yet published" and returns 0.
- `update_scoop_manifest` (`:257`) reads a failed `gh release download` as a
  missing sidecar. Its caller (`:638`) swallows the failure with `|| echo`.

## Fix

The release was finished by hand. With `GITHUB_TOKEN` unset, the implementer
watched release.yml run 36234512609 to completion, then ran
`scripts/release.sh 0.7.2 --notes-only` and `--scoop-only`. That produced
`cbd664d` ("Scoop manifest points at binvim 0.7.2").

Then the pre-flight was fixed. Both `command -v gh` sites now call
`require_gh` (`3d04d50`, `d2f9ec8`), which also runs
`gh api "repos/${OWNER}/${REPO}"` and exits before the bump commit,
`cargo publish` or the tag push if that call fails. It started out as
`gh api user`, but review caught that `GET /user` rejects a GitHub App
installation token (an Actions `GITHUB_TOKEN`). A repository-scoped call
accepts either kind of token and still gets a 401 for a bad one.

One gap is still open: `gh api` goes to `GH_HOST` when it's set. With an
authenticated enterprise host in `GH_HOST`, the probe passes, and the later
`gh run` / `gh release` calls fail after publishing because the `github.com`
origin doesn't match `GH_HOST`.

## Prevention

- Before a release, run `gh auth status` in the shell that will run the script,
  and unset `GITHUB_TOKEN` / `GH_TOKEN` unless they hold the token you mean to
  use.
- During a run, "Could not find a release.yml run … Skipping wait" right after
  the tag push means `gh` can't reach GitHub, not that CI is slow. Stop and
  check `gh auth status` before trusting the steps after it.
- A diff to `scripts/release.sh` that adds a `gh` call must not discard its
  stderr and fold a non-zero exit into a "not published yet" / "CI still
  building" branch. It has to tell an auth or network failure apart from a
  missing resource, or the pre-flight has to prove auth first.
- `require_gh` in `scripts/release.sh` must make an authenticated API call,
  and must run before the bump commit, `cargo publish` and the tag push. A diff
  that cuts it back to `command -v gh`, moves a call to it below a publish
  step, or makes the probe `gh api user` breaks this.
