#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/pkg-homebrew.sh 0.1.0
# Downloads the source tarball for the given tag, computes its sha256,
# rewrites url + sha256 in the tap clone's Formula/binvim.rb, then
# offers to commit and push.
#
# Expects the tap repo cloned at ../../homebrew/binvim (a sibling to the
# binman/binsql tap clones).

OWNER="bgunnarsson"
REPO="binvim"

if [[ $# -lt 1 ]]; then
    echo "Usage: $0 <version-without-v>  (e.g. $0 0.1.0)" >&2
    exit 1
fi

VERSION="${1#v}"
TAG="v${VERSION}"
TARBALL_URL="https://github.com/${OWNER}/${REPO}/archive/refs/tags/${TAG}.tar.gz"

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
SRC_REPO_DIR="${SCRIPT_DIR}/.."
TAP_DIR="${SCRIPT_DIR}/../../homebrew/${REPO}"

if [[ ! -d "$SRC_REPO_DIR" ]]; then
    echo "Source repo dir not found: $SRC_REPO_DIR" >&2
    exit 1
fi

if [[ ! -d "$TAP_DIR" ]]; then
    echo "Homebrew tap repo not found: $TAP_DIR" >&2
    echo "Clone it first:" >&2
    echo "  mkdir -p ${SCRIPT_DIR}/../../homebrew" >&2
    echo "  git clone git@github.com:${OWNER}/homebrew-${REPO}.git \"$TAP_DIR\"" >&2
    exit 1
fi

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
        echo "No formula found in tap. Bootstrapping from ${REF}"
        mkdir -p "${TAP_DIR}/Formula"
        cp "$REF" "${TAP_DIR}/Formula/binvim.rb"
        FORMULA_FILE="${TAP_DIR}/Formula/binvim.rb"
    else
        echo "Formula file not found in any of:" >&2
        printf '  %s\n' "${CANDIDATES[@]}" >&2
        exit 1
    fi
fi

echo "Releasing binvim ${VERSION}"
echo "  src repo:     ${SRC_REPO_DIR}"
echo "  tap repo:     ${TAP_DIR}"
echo "  formula file: ${FORMULA_FILE}"
echo "  tag:          ${TAG}"
echo "  tarball:      ${TARBALL_URL}"
echo

if ! git -C "$SRC_REPO_DIR" ls-remote --tags origin "$TAG" 2>/dev/null | grep -q "refs/tags/${TAG}\$"; then
    echo "Tag ${TAG} not found on origin. Run scripts/release.sh ${VERSION} first." >&2
    exit 1
fi

TMP_TGZ="$(mktemp)"
trap 'rm -f "$TMP_TGZ"' EXIT

echo "Downloading source tarball..."
DOWNLOADED=0
for i in 1 2 3 4 5; do
    if curl -L -sSf "$TARBALL_URL" -o "$TMP_TGZ"; then
        DOWNLOADED=1
        break
    fi
    echo "  Attempt $i failed, retrying in 5s..."
    sleep 5
done

if [[ "$DOWNLOADED" -ne 1 ]]; then
    echo "Could not download $TARBALL_URL" >&2
    exit 1
fi

SHA256="$(shasum -a 256 "$TMP_TGZ" | awk '{print $1}')"
echo "sha256: ${SHA256}"
echo

perl -pi -e 's|^  url ".*"|  url "'"${TARBALL_URL}"'"|' "$FORMULA_FILE"
perl -pi -e 's|^  sha256 ".*"|  sha256 "'"${SHA256}"'"|' "$FORMULA_FILE"

echo "Updated ${FORMULA_FILE}:"
grep -E '  url "|  sha256 "' "$FORMULA_FILE" || true
echo

cd "$TAP_DIR"
echo "Git status in tap repo:"
git status --short
echo

read -r -p "Commit and push these Homebrew changes? [y/N] " ans
if [[ "$ans" =~ ^[Yy]$ ]]; then
    git add "$FORMULA_FILE"
    git commit -m "binvim ${VERSION}"
    git push
    echo "Pushed updated formula."
else
    echo "Aborted before commit."
fi

cat <<EOF

Next steps:

  brew uninstall binvim        # if installed from this tap
  brew untap ${OWNER}/${REPO} || true
  brew tap ${OWNER}/${REPO}
  brew install ${OWNER}/${REPO}/binvim

EOF
