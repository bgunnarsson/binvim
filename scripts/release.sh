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
#      tag absent, sibling repos present, gh CLI available,
#      crates.io credentials available).
#   2. Local verification (cargo fmt --check, cargo test, cargo clippy,
#      cargo build --release) so a broken commit never gets tagged.
#   3. Stamp today's date on the CHANGELOG entry, bump Cargo.toml +
#      refresh Cargo.lock, commit, push to main.
#   3b. Publish to crates.io via `cargo publish --locked`. Runs BEFORE
#       the tag push so a failed publish doesn't leave a dangling tag /
#       GitHub Release / Homebrew bump that points at a non-existent
#       crates.io version. Re-run the script with the same version to
#       retry (the bump commit is already on main, so step 3 becomes
#       a no-op).
#   4. Tag v<version>, push tag → triggers .github/workflows/release.yml
#      which builds the per-target binaries + publishes the GitHub Release.
#   5. (Optional) Wait for the release workflow to finish, then sanity-
#      check the asset count (expects 12 — 4 targets × tar.gz/sha256/bundle).
#   5b. Overwrite the auto-generated Release notes with the curated
#       CHANGELOG section for this version.
#   6. Verify the Homebrew tap repo is clean + current, then download
#      the source tarball, compute sha256, rewrite url + sha256 in
#      the formula, commit, push.
#   7. Verify the binvim-web repo is clean + current, then mirror
#      install.sh in for binvim.dev/install.sh, commit, push.
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

# Stamp today's date on the `## [X.Y.Z]` line in CHANGELOG.md. If the
# header already carries a date (in any form) it's overwritten. No-op
# if the header doesn't exist (caller's pre-flight already enforced
# that it does). Dots in the version are escaped so `0.4.5` doesn't
# accidentally match `0x4x5`.
stamp_changelog_date() {
    local v="$1"
    local v_re="${v//./\\.}"
    local today
    today="$(date +%Y-%m-%d)"
    perl -pi -e 's/^(## \[?'"$v_re"'\]?).*$/$1 - '"$today"'/' CHANGELOG.md
}

# Extract the body of the `## [X.Y.Z]` section — everything between
# that header and the next `## ` header. Header itself is omitted.
# Used to seed the GitHub Release notes from the curated CHANGELOG
# instead of the auto-generated commit-message list the workflow
# would otherwise leave behind.
extract_changelog_section() {
    local v="$1"
    local v_re="${v//./\\.}"
    awk -v ver="$v_re" '
        BEGIN { in_section = 0 }
        /^## / {
            if (in_section) exit
            if ($0 ~ "^## \\[?" ver "\\]?") { in_section = 1; next }
            next
        }
        in_section { print }
    ' CHANGELOG.md
}

# Verify a sibling repo is on a clean working tree and current with
# origin (fast-forwardable). Aborts the release if not. The Homebrew
# tap and binvim-web both get this treatment so a stale clone or
# half-finished local edit can't get committed during the release.
ensure_sibling_clean_and_current() {
    local dir="$1"
    local label="$2"
    (
        cd "$dir"
        if ! git diff-index --quiet HEAD --; then
            echo "  ${label} has uncommitted changes — commit/stash before re-running:" >&2
            git status --short | sed 's/^/      /' >&2
            return 1
        fi
        if ! git fetch origin >/dev/null 2>&1; then
            echo "  ${label}: git fetch failed" >&2
            return 1
        fi
        if ! git pull --ff-only origin "$(git rev-parse --abbrev-ref HEAD)" >/dev/null 2>&1; then
            echo "  ${label} can't fast-forward — diverged or has unpushed commits: $dir" >&2
            return 1
        fi
    )
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

# `cargo publish` needs a crates.io token. Accept either the
# CARGO_REGISTRY_TOKEN env var (CI-friendly) or a previously-run
# `cargo login` that left a token in ~/.cargo/credentials.toml.
# Fail loudly here rather than at step 3b — we don't want to push
# the bump commit to main and then blow up on the publish step.
if [[ -z "${CARGO_REGISTRY_TOKEN:-}" ]] \
    && [[ ! -f "${CARGO_HOME:-$HOME/.cargo}/credentials.toml" ]] \
    && [[ ! -f "${CARGO_HOME:-$HOME/.cargo}/credentials" ]]; then
    echo "No crates.io credentials found." >&2
    echo "  Set CARGO_REGISTRY_TOKEN=<token> or run \`cargo login\` first." >&2
    echo "  Get a token at https://crates.io/me." >&2
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

# ─── 3. Stamp CHANGELOG date + bump + commit + push ───────────────

step "Stamp CHANGELOG date, bump Cargo.toml + Cargo.lock"

stamp_changelog_date "$VERSION"
echo "  CHANGELOG: ## [${VERSION}] dated $(date +%Y-%m-%d)"

CURRENT_VERSION="$(perl -ne 'print $1 if /^version\s*=\s*"([^"]+)"/' Cargo.toml | head -n1)"
echo "  Cargo.toml: ${CURRENT_VERSION} -> ${VERSION}"

if [[ "$CURRENT_VERSION" != "$VERSION" ]]; then
    perl -pi -e 's|^version = ".*"|version = "'"${VERSION}"'"|' Cargo.toml
    cargo check --quiet  # refresh Cargo.lock
    git add Cargo.toml Cargo.lock CHANGELOG.md
    git commit -m "Bump version to ${VERSION}"
    git push origin main
elif ! git diff --quiet -- CHANGELOG.md; then
    # Cargo already at the requested version (e.g. user manually
    # bumped earlier) but the CHANGELOG date still needed stamping.
    git add CHANGELOG.md
    git commit -m "CHANGELOG: stamp ${VERSION} date"
    git push origin main
else
    echo "  Already at ${VERSION}, no changes to commit."
fi

# ─── 3b. Publish to crates.io ─────────────────────────────────────

step "Publish to crates.io"

# `cargo publish --locked` re-resolves against Cargo.lock so the
# published crate uses the exact dep set we tested in step 2. It
# also runs `cargo package` + a verification build, which catches
# include/exclude misconfiguration and missing files before the
# tarball uploads. The `--allow-dirty` is intentionally NOT passed:
# the bump commit is already in step 3, the working tree is clean,
# we want the upload to be reproducible from main@HEAD.
#
# Re-running after a failed publish: step 3 will say "Already at
# ${VERSION}, no changes to commit" and skip; we'll land here
# again with a clean tree and another shot at the publish.
cargo publish --locked
echo "  Published binvim ${VERSION} to crates.io."
echo "  cargo install binvim  # available within a minute or two"

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

    # Sanity-check the published Release. The build matrix has 3 targets
    # (x86_64 musl, aarch64 musl, x86_64-pc-windows-msvc) and each emits
    # 3 files (archive + .sha256 + .bundle) — 9 assets total. A short
    # count means the workflow's final upload step half-succeeded; loud
    # enough to investigate, not fatal because the Homebrew + web push
    # paths don't depend on these artifacts (they use the source tarball
    # + install.sh respectively).
    ASSETS="$(gh release view "$TAG" --json assets --jq '.assets | length' 2>/dev/null || echo 0)"
    echo "  Release ${TAG} has ${ASSETS} assets attached."
    if [[ "$ASSETS" -lt 9 ]]; then
        echo "  WARNING: expected 9 (3 targets × archive + sha256 + bundle), got ${ASSETS}." >&2
        echo "  Inspect: gh release view ${TAG}" >&2
    fi
fi

# ─── 5b. Push CHANGELOG section as release notes ──────────────────

step "Push CHANGELOG section as GitHub Release notes"

# The workflow publishes the Release with `generate_release_notes:
# true`, which produces a commit-message dump. Overwrite that with
# the curated CHANGELOG section so the user-facing page actually
# reads like release notes.
NOTES_TMP="$(mktemp)"
extract_changelog_section "$VERSION" > "$NOTES_TMP"
if [[ ! -s "$NOTES_TMP" ]]; then
    echo "  CHANGELOG section empty — leaving auto-generated notes in place."
elif gh release view "$TAG" >/dev/null 2>&1; then
    if gh release edit "$TAG" --notes-file "$NOTES_TMP" >/dev/null 2>&1; then
        echo "  Updated release notes from CHANGELOG."
    else
        echo "  WARNING: gh release edit failed. Edit manually:" >&2
        echo "    gh release edit ${TAG}" >&2
    fi
else
    echo "  Release ${TAG} not yet published (CI still building?). Skipping notes update." >&2
    echo "  Update later with: gh release edit ${TAG} --notes-file <(awk ...)" >&2
fi
rm -f "$NOTES_TMP"

# ─── 6. Update Homebrew tap ───────────────────────────────────────

step "Update Homebrew tap"

ensure_sibling_clean_and_current "$TAP_DIR" "Homebrew tap" || exit 1

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

ensure_sibling_clean_and_current "$WEB_DIR" "binvim-web" || exit 1

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
  crates.io:       https://crates.io/crates/${REPO}/${VERSION}
  Tap:             https://github.com/${OWNER}/homebrew-${REPO}
  install.sh:      https://binvim.dev/install.sh

  Try:
    brew upgrade ${REPO}                          # if already tapped
    brew install ${OWNER}/${REPO}/${REPO}         # fresh install
    cargo install --locked ${REPO}                # from crates.io
    curl -fsSL https://binvim.dev/install.sh | sh # curl path

EOF
