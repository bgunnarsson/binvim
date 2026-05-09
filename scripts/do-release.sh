#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/do-release.sh 0.1.0
#
# 1. Bumps Cargo.toml + Cargo.lock, commits, tags, pushes.
#    CI then builds the Linux musl tarballs and publishes the GitHub Release.
# 2. Downloads the source tarball, sha256s it, rewrites the tap formula,
#    commits and pushes to the tap repo.

VERSION="${1:-}"

if [[ -z "$VERSION" ]]; then
    echo "Usage: $0 <version>  (e.g. $0 0.1.0)" >&2
    exit 1
fi

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"

echo "===> Step 1: Bump version, tag, push, kick off Linux build CI"
"${SCRIPT_DIR}/release.sh" "$VERSION"

echo
echo "===> Step 2: Update Homebrew tap"
"${SCRIPT_DIR}/pkg-homebrew.sh" "$VERSION"

echo
echo "===> Step 3: Mirror install.sh to binvim-web"
"${SCRIPT_DIR}/pkg-web.sh"

echo
echo "===> Release ${VERSION#v} complete."
