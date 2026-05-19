#!/usr/bin/env bash
set -euo pipefail

# One script, end-to-end release flow. Replaces the prior
# do-release.sh / release.sh / pkg-homebrew.sh / pkg-web.sh quartet.
#
# Usage: ./scripts/release.sh <version> [--skip-ci-wait] [--yes]
#   <version>       semver string, with or without leading `v` (e.g. 0.4.5 or v0.4.5)
#   --skip-ci-wait  don't block waiting for the GitHub Actions release build
#   --yes           non-interactive: auto-confirm Homebrew + web push prompts
#
# What it does, in order:
#   1. Pre-flight checks (clean tree, on main, CHANGELOG entry exists,
#      tag absent, sibling repos present).
#   2. Local verification (cargo fmt --check, cargo test, cargo clippy,
#      cargo build --release) so a broken commit never gets tagged.
#   3. Bump Cargo.toml + refresh Cargo.lock, commit, push to main.
#   4. Tag v<version>, push tag → triggers .github/workflows/release.yml
#      which builds the per-target binaries + publishes the GitHub Release.
#   5. (Optional) Wait for the release workflow to finish.
#   6. Update the Homebrew tap repo: download the source tarball, compute
#      sha256, rewrite url + sha256 in the formula, commit, push.
#   7. Mirror install.sh into the binvim-web repo (for binvim.dev/install.sh),
#      commit, push.
#   8. Print a short summary with the release URL.
#
# Expected sibling layout:
#   ../homebrew/binvim     — Homebrew tap clone (homebrew-binvim)
#   ../binvim-web          — binvim.dev source repo

# ─── Argument parsing ─────────────────────────────────────────────

VERSION=""
SKIP_CI_WAIT=0
ASSUME_YES=0

for arg in "$@"; do
    case "$arg" in
        --skip-ci-wait) SKIP_CI_WAIT=1 ;;
        --yes|-y)       ASSUME_YES=1 ;;
        -h|--help)
            sed -n '3,30p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        -*)
            echo "Unknown flag: $arg" >&2
            exit 1
            ;;
        *)
            if [[ -n "$VERSION" ]]; then
                echo "Multiple version arguments: $VERSION, $arg" >&2
                exit 1
            fi
            VERSION="$arg"
            ;;
    esac
done

if [[ -z "$VERSION" ]]; then
    echo "Usage: $0 <version> [--skip-ci-wait] [--yes]" >&2
    exit 1
fi

VERSION="${VERSION#v}"
TAG="v${VERSION}"

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
    echo "Bad version: $VERSION (expected semver, e.g. 0.4.5)" >&2
    exit 1
fi

OWNER="bgunnarsson"
REPO="binvim"
TARBALL_URL="https://github.com/${OWNER}/${REPO}/archive/refs/tags/${TAG}.tar.gz"

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
cd "$ROOT"

TAP_DIR="$(cd "${ROOT}/.." 2>/dev/null && pwd)/homebrew/${REPO}"
WEB_DIR="$(cd "${ROOT}/.." 2>/dev/null && pwd)/binvim-web"

# ─── Helpers ──────────────────────────────────────────────────────

step() {
    echo
    echo "==> $*"
}

confirm() {
    # confirm "<question>" — returns 0 on yes, 1 on no. --yes auto-yes.
    if [[ "$ASSUME_YES" -eq 1 ]]; then
        echo "$1 [y/N] y  (auto)"
        return 0
    fi
    local ans
    read -r -p "$1 [y/N] " ans
    [[ "$ans" =~ ^[Yy]$ ]]
}

# ─── 1. Pre-flight ────────────────────────────────────────────────

step "Pre-flight checks"

if ! git diff-index --quiet HEAD --; then
    echo "Working tree is not clean. Commit or stash first." >&2
    exit 1
fi

CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD)"
if [[ "$CURRENT_BRANCH" != "main" ]]; then
    echo "Not on main (current: $CURRENT_BRANCH). Switch to main first." >&2
    exit 1
fi

git fetch --tags origin >/dev/null 2>&1 || true

if git rev-parse "$TAG" >/dev/null 2>&1; then
    echo "Tag $TAG already exists locally. Bump the version." >&2
    exit 1
fi
if git ls-remote --tags origin "$TAG" 2>/dev/null | grep -q "refs/tags/${TAG}\$"; then
    echo "Tag $TAG already exists on origin. Bump the version." >&2
    exit 1
fi

# CHANGELOG must mention the version — keeps us from cutting a release
# without notes. Match either `## [X.Y.Z]` or `## X.Y.Z` for tolerance.
if ! grep -Eq "^## \[?${VERSION}\]?" CHANGELOG.md; then
    echo "CHANGELOG.md has no entry for ${VERSION}. Add one before releasing." >&2
    exit 1
fi

if [[ ! -d "$TAP_DIR" ]]; then
    echo "Homebrew tap repo not found: $TAP_DIR" >&2
    echo "Clone it first:" >&2
    echo "  mkdir -p $(dirname "$TAP_DIR")" >&2
    echo "  git clone git@github.com:${OWNER}/homebrew-${REPO}.git \"$TAP_DIR\"" >&2
    exit 1
fi
if [[ ! -d "$WEB_DIR" ]]; then
    echo "binvim-web repo not found: $WEB_DIR" >&2
    echo "Clone it first:" >&2
    echo "  git clone git@github.com:${OWNER}/binvim-web.git \"$WEB_DIR\"" >&2
    exit 1
fi

if ! command -v gh >/dev/null 2>&1; then
    echo "gh CLI not found. Install with: brew install gh" >&2
    exit 1
fi

echo "  branch:       main"
echo "  version:      ${VERSION}  (tag ${TAG})"
echo "  tap repo:     ${TAP_DIR}"
echo "  web repo:     ${WEB_DIR}"
echo "  tarball:      ${TARBALL_URL}"

# ─── 2. Local verification ────────────────────────────────────────

step "Local verification — fmt / test / clippy / build"

cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -A warnings
cargo build --release --locked

# ─── 3. Bump + commit + push ──────────────────────────────────────

step "Bump Cargo.toml + Cargo.lock"

CURRENT_VERSION="$(perl -ne 'print $1 if /^version\s*=\s*"([^"]+)"/' Cargo.toml | head -n1)"
echo "  Cargo.toml: ${CURRENT_VERSION} -> ${VERSION}"

if [[ "$CURRENT_VERSION" != "$VERSION" ]]; then
    perl -pi -e 's|^version = ".*"|version = "'"${VERSION}"'"|' Cargo.toml
    cargo check --quiet  # refresh Cargo.lock
    git add Cargo.toml Cargo.lock
    git commit -m "Bump version to ${VERSION}"
    git push origin main
else
    echo "  Already at ${VERSION}, skipping bump commit."
fi

# ─── 4. Tag + push ────────────────────────────────────────────────

step "Tag ${TAG} and push"

git tag -a "$TAG" -m "binvim ${VERSION}"
git push origin "$TAG"
echo "  Pushed ${TAG}. Release workflow is now building."

# ─── 5. Wait for CI (optional) ────────────────────────────────────

if [[ "$SKIP_CI_WAIT" -eq 1 ]]; then
    step "Skipping CI wait (--skip-ci-wait)"
else
    step "Waiting for release workflow to complete"
    # The release workflow triggers off the tag push. Give GitHub a moment
    # to register the run, then watch it.
    sleep 5
    RUN_ID="$(gh run list --workflow=release.yml --limit=1 --json databaseId --jq='.[0].databaseId' 2>/dev/null || true)"
    if [[ -z "$RUN_ID" ]]; then
        echo "  Could not find release.yml run. Skipping wait." >&2
    else
        echo "  Watching run ${RUN_ID}..."
        if ! gh run watch "$RUN_ID" --exit-status; then
            echo "  Release workflow failed. Inspect with: gh run view ${RUN_ID}" >&2
            echo "  Skipping Homebrew + web push — fix CI and re-run those steps manually." >&2
            exit 1
        fi
    fi
fi

# ─── 6. Update Homebrew tap ───────────────────────────────────────

step "Update Homebrew tap"

CANDIDATES=(
    "${TAP_DIR}/binvim.rb"
    "${TAP_DIR}/Formula/binvim.rb"
    "${TAP_DIR}/HomebrewFormula/binvim.rb"
)
FORMULA_FILE=""
for f in "${CANDIDATES[@]}"; do
    if [[ -f "$f" ]]; then
        FORMULA_FILE="$f"
        break
    fi
done

if [[ -z "$FORMULA_FILE" ]]; then
    REF="${SCRIPT_DIR}/homebrew/binvim.rb"
    if [[ -f "$REF" ]]; then
        echo "  No formula found in tap. Bootstrapping from ${REF}"
        mkdir -p "${TAP_DIR}/Formula"
        cp "$REF" "${TAP_DIR}/Formula/binvim.rb"
        FORMULA_FILE="${TAP_DIR}/Formula/binvim.rb"
    else
        echo "Formula file not found in any of:" >&2
        printf '  %s\n' "${CANDIDATES[@]}" >&2
        exit 1
    fi
fi
echo "  formula: ${FORMULA_FILE}"

TMP_TGZ="$(mktemp)"
trap 'rm -f "$TMP_TGZ"' EXIT

echo "  downloading source tarball..."
DOWNLOADED=0
for i in 1 2 3 4 5; do
    if curl -L -sSf "$TARBALL_URL" -o "$TMP_TGZ"; then
        DOWNLOADED=1
        break
    fi
    echo "    attempt $i failed, retrying in 5s..."
    sleep 5
done
if [[ "$DOWNLOADED" -ne 1 ]]; then
    echo "Could not download $TARBALL_URL" >&2
    exit 1
fi

SHA256="$(shasum -a 256 "$TMP_TGZ" | awk '{print $1}')"
echo "  sha256:  ${SHA256}"

perl -pi -e 's|^  url ".*"|  url "'"${TARBALL_URL}"'"|' "$FORMULA_FILE"
perl -pi -e 's|^  sha256 ".*"|  sha256 "'"${SHA256}"'"|' "$FORMULA_FILE"

(
    cd "$TAP_DIR"
    echo "  status:"
    git status --short | sed 's/^/    /'
    if git diff --quiet && git diff --cached --quiet; then
        echo "  Formula already at this URL + sha. Nothing to commit."
    else
        if confirm "Commit and push Homebrew formula update?"; then
            git add "$FORMULA_FILE"
            git commit -m "binvim ${VERSION}"
            git push
            echo "  Pushed."
        else
            echo "  Skipped Homebrew commit." >&2
        fi
    fi
)

# ─── 7. Mirror install.sh → binvim-web ────────────────────────────

step "Mirror install.sh → binvim-web"

SRC_INSTALL="${ROOT}/install.sh"
DST_INSTALL="${WEB_DIR}/install.sh"

if [[ ! -f "$SRC_INSTALL" ]]; then
    echo "Source install.sh not found: $SRC_INSTALL" >&2
    exit 1
fi

if [[ -f "$DST_INSTALL" ]] && cmp -s "$SRC_INSTALL" "$DST_INSTALL"; then
    echo "  install.sh already up-to-date in binvim-web, nothing to do."
else
    cp "$SRC_INSTALL" "$DST_INSTALL"
    chmod +x "$DST_INSTALL"
    (
        cd "$WEB_DIR"
        echo "  status:"
        git status --short | sed 's/^/    /'
        if confirm "Commit and push install.sh to binvim-web?"; then
            git add install.sh
            git commit -m "Update install.sh for binvim ${VERSION}"
            git push
            echo "  Pushed. Live at binvim.dev/install.sh once deploy completes."
        else
            echo "  Skipped binvim-web commit." >&2
        fi
    )
fi

# ─── 8. Summary ───────────────────────────────────────────────────

step "Release ${VERSION} complete"

cat <<EOF

  GitHub Release:  https://github.com/${OWNER}/${REPO}/releases/tag/${TAG}
  Tap:             https://github.com/${OWNER}/homebrew-${REPO}
  install.sh:      https://binvim.dev/install.sh

  Try:
    brew upgrade ${REPO}                          # if already tapped
    brew install ${OWNER}/${REPO}/${REPO}         # fresh install
    curl -fsSL https://binvim.dev/install.sh | sh # curl path

EOF
