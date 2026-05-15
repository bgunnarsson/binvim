#!/usr/bin/env bash
# Demo script — exercises common bash constructs.

set -euo pipefail

readonly SCRIPT_NAME="${0##*/}"
readonly DEFAULT_DIR="${HOME}/projects"

log() {
    printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2
}

die() {
    log "fatal: $*"
    exit 1
}

usage() {
    cat <<EOF
Usage: ${SCRIPT_NAME} [-v] [-d DIR] PATTERN

Options:
  -v          verbose output
  -d DIR      search root (default: ${DEFAULT_DIR})
EOF
}

main() {
    local verbose=0
    local search_dir="${DEFAULT_DIR}"

    while getopts ":vd:h" opt; do
        case "${opt}" in
            v) verbose=1 ;;
            d) search_dir="${OPTARG}" ;;
            h) usage; exit 0 ;;
            *) usage; exit 1 ;;
        esac
    done
    shift $((OPTIND - 1))

    [[ $# -lt 1 ]] && die "missing PATTERN"
    local pattern="$1"

    if [[ ! -d "${search_dir}" ]]; then
        die "no such dir: ${search_dir}"
    fi

    (( verbose )) && log "searching ${search_dir} for ${pattern}"

    local -a matches=()
    while IFS= read -r -d '' file; do
        matches+=("${file}")
    done < <(find "${search_dir}" -type f -name "*${pattern}*" -print0)

    log "found ${#matches[@]} matches"
    for f in "${matches[@]}"; do
        echo "${f}"
    done
}

main "$@"
