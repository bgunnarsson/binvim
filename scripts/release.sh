#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/release.sh 0.1.0
# Bumps Cargo.toml + Cargo.lock to the new version, commits, tags, pushes.
# CI builds the Linux musl tarballs and publishes the GitHub Release.

VERSION="${1:-}"

if [[ -z "$VERSION" ]]; then
    echo "Usage: $0 <version>  (e.g. $0 0.1.0)" >&2
    exit 1
fi

VERSION="${VERSION#v}"
TAG="v${VERSION}"

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
    echo "Bad version: $VERSION (expected semver, e.g. 0.1.0)" >&2
    exit 1
fi

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

if ! git diff-index --quiet HEAD --; then
    echo "Working tree is not clean. Commit or stash first." >&2
    exit 1
fi

CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD)"
if [[ "$CURRENT_BRANCH" != "main" ]]; then
    echo "Not on main (current: $CURRENT_BRANCH). Switch to main first." >&2
    exit 1
fi

git fetch --tags >/dev/null 2>&1 || true
if git rev-parse "$TAG" >/dev/null 2>&1; then
    echo "Tag $TAG already exists locally. Bump the version." >&2
    exit 1
fi

CURRENT_VERSION="$(perl -ne 'print $1 if /^version\s*=\s*"([^"]+)"/' Cargo.toml | head -n1)"
echo "==> Cargo.toml: ${CURRENT_VERSION} -> ${VERSION}"

if [[ "$CURRENT_VERSION" != "$VERSION" ]]; then
    perl -pi -e 's|^version = ".*"|version = "'"${VERSION}"'"|' Cargo.toml

    echo "==> Refreshing Cargo.lock"
    cargo check --quiet

    git add Cargo.toml Cargo.lock
    git commit -m "Bump version to ${VERSION}"
    git push origin main
else
    echo "Cargo.toml already at ${VERSION}, skipping bump."
fi

echo "==> Tagging ${TAG}"
git tag -a "$TAG" -m "binvim ${VERSION}"

echo "==> Pushing tag ${TAG}"
git push origin "$TAG"

cat <<EOF

==> Tag ${TAG} pushed. CI will build Linux tarballs and publish the GitHub Release.
    Watch with:
      gh run watch \$(gh run list --workflow=release.yml --limit=1 --json databaseId --jq='.[0].databaseId')

EOF
