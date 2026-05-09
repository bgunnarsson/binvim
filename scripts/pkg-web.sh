#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/pkg-web.sh
# Mirrors install.sh into the binvim-web repo so it can be served at
# binvim.dev/install.sh. Prompts before committing + pushing.
#
# Expects the web repo cloned at ../../binvim-web (a sibling to this repo).

REPO="binvim-web"

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
SRC_REPO_DIR="${SCRIPT_DIR}/.."
WEB_DIR="${SCRIPT_DIR}/../../${REPO}"

SRC_INSTALL="${SRC_REPO_DIR}/install.sh"
DST_INSTALL="${WEB_DIR}/install.sh"

if [[ ! -f "$SRC_INSTALL" ]]; then
    echo "Source install.sh not found: $SRC_INSTALL" >&2
    exit 1
fi

if [[ ! -d "$WEB_DIR" ]]; then
    echo "binvim-web repo not found: $WEB_DIR" >&2
    echo "Clone it first:" >&2
    echo "  git clone git@github.com:bgunnarsson/${REPO}.git \"$WEB_DIR\"" >&2
    exit 1
fi

echo "Mirroring install.sh"
echo "  src: ${SRC_INSTALL}"
echo "  dst: ${DST_INSTALL}"
echo

if [[ -f "$DST_INSTALL" ]] && cmp -s "$SRC_INSTALL" "$DST_INSTALL"; then
    echo "install.sh is already up-to-date in ${REPO}, nothing to do."
    exit 0
fi

cp "$SRC_INSTALL" "$DST_INSTALL"
chmod +x "$DST_INSTALL"

cd "$WEB_DIR"
echo "Git status in ${REPO}:"
git status --short
echo

read -r -p "Commit and push install.sh to ${REPO}? [y/N] " ans
if [[ "$ans" =~ ^[Yy]$ ]]; then
    git add install.sh
    git commit -m "Update install.sh"
    git push
    echo "Pushed. Should be live at binvim.dev/install.sh once the deploy completes."
else
    echo "Aborted before commit."
fi
