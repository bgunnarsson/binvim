#!/usr/bin/env sh
# binvim Linux installer.
#
#   curl -fsSL https://binvim.dev/install.sh | sh
#
# Optional environment overrides:
#   BINVIM_VERSION=v0.1.0       pin to a specific tag (default: latest release)
#   BINVIM_INSTALL_DIR=/opt/bin override install directory (default: $HOME/.local/bin)
#   BINVIM_NO_ALIAS=1           skip adding 'alias bim=binvim' to your shell profile
#
# macOS users: use Homebrew instead — `brew install bgunnarsson/binvim/binvim`.

set -eu

REPO="bgunnarsson/binvim"
INSTALL_DIR="${BINVIM_INSTALL_DIR:-$HOME/.local/bin}"

err() { printf 'error: %s\n' "$1" >&2; exit 1; }
info() { printf '==> %s\n' "$1"; }

need() {
    command -v "$1" >/dev/null 2>&1 || err "missing required tool: $1"
}

configure_alias() {
    if [ "${BINVIM_NO_ALIAS:-}" = "1" ]; then
        return
    fi

    rc=""
    line='alias bim=binvim'
    case "${SHELL:-}" in
        */zsh)  rc="$HOME/.zshrc" ;;
        */bash) rc="$HOME/.bashrc" ;;
        */fish)
            rc="$HOME/.config/fish/config.fish"
            line='alias bim binvim'
            ;;
        *)
            printf '\n'
            printf 'note: shell %s not recognised — add this to your shell profile manually:\n\n' "${SHELL:-unknown}"
            printf '          alias bim=binvim\n\n'
            return
            ;;
    esac

    if [ -f "$rc" ] && grep -qE '^[[:space:]]*alias[[:space:]]+bim([[:space:]]|=)' "$rc"; then
        info "'bim' alias already present in $rc"
        return
    fi

    mkdir -p "$(dirname "$rc")"
    {
        printf '\n# binvim alias (added by install.sh)\n'
        printf '%s\n' "$line"
    } >> "$rc"
    info "added '$line' to $rc — start a new shell or 'source $rc' to use it"
}

need curl
need tar
need uname

uname_s=$(uname -s 2>/dev/null || echo unknown)
case "$uname_s" in
    Linux) ;;
    Darwin)
        err "macOS detected — install with Homebrew:
    brew install bgunnarsson/binvim/binvim" ;;
    *) err "unsupported OS: $uname_s" ;;
esac

uname_m=$(uname -m 2>/dev/null || echo unknown)
case "$uname_m" in
    x86_64|amd64)        target="x86_64-unknown-linux-musl" ;;
    aarch64|arm64)       target="aarch64-unknown-linux-musl" ;;
    *) err "unsupported architecture: $uname_m" ;;
esac

if [ -z "${BINVIM_VERSION:-}" ]; then
    info "resolving latest release"
    BINVIM_VERSION=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)
    [ -n "$BINVIM_VERSION" ] || err "could not resolve latest release; set BINVIM_VERSION explicitly"
fi

archive="binvim-${BINVIM_VERSION}-${target}.tar.gz"
url="https://github.com/${REPO}/releases/download/${BINVIM_VERSION}/${archive}"
sha_url="${url}.sha256"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

info "downloading ${archive}"
curl -fsSL "$url"     -o "$tmp/$archive" || err "download failed: $url"
curl -fsSL "$sha_url" -o "$tmp/$archive.sha256" || err "checksum download failed: $sha_url"

info "verifying checksum"
(
    cd "$tmp"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -c "$archive.sha256" >/dev/null 2>&1 || err "checksum verification failed"
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 -c "$archive.sha256" >/dev/null 2>&1 || err "checksum verification failed"
    else
        err "no sha256 tool available (need sha256sum or shasum)"
    fi
)

info "extracting"
tar -C "$tmp" -xzf "$tmp/$archive"

mkdir -p "$INSTALL_DIR"
mv "$tmp/binvim" "$INSTALL_DIR/binvim"
chmod +x "$INSTALL_DIR/binvim"

info "installed binvim ${BINVIM_VERSION} → $INSTALL_DIR/binvim"

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        printf '\n'
        printf 'note: %s is not on your PATH.\n' "$INSTALL_DIR"
        printf '      add this to your shell init (e.g. ~/.bashrc or ~/.zshrc):\n\n'
        printf '          export PATH="%s:$PATH"\n\n' "$INSTALL_DIR"
        ;;
esac

configure_alias
