#!/usr/bin/env bash
set -euo pipefail

# Launch the game from any working directory. On Windows-compatible Bash
# environments (including WSL), prefer the packaged executable and its bundled
# assets. On a native Unix host, build and run the current source with Cargo.

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
GAME_EXE="$REPO_ROOT/dist/windows/SuperBevyWorld64.exe"

usage() {
    cat <<'EOF'
Usage: ./run_bevy.sh [--source|--packaged]

  no option    Use the Windows package under Git Bash/WSL; otherwise Cargo.
  --source     Build and run the current Rust source with Cargo.
  --packaged   Run the packaged Windows executable.
EOF
}

run_source() {
    if ! command -v cargo >/dev/null 2>&1; then
        echo "error: Cargo is required to run the Bevy game from source" >&2
        exit 1
    fi
    cd -- "$REPO_ROOT"
    exec cargo run --release
}

run_packaged() {
    if [[ ! -f "$GAME_EXE" ]]; then
        echo "error: packaged game not found at:" >&2
        echo "  $GAME_EXE" >&2
        echo "build it with ./build_windows.sh" >&2
        exit 1
    fi
    cd -- "$(dirname -- "$GAME_EXE")"
    exec "$GAME_EXE"
}

prefer_packaged() {
    if [[ -f "$GAME_EXE" ]]; then
        run_packaged
    fi
    echo "Packaged Windows game not found; falling back to Cargo." >&2
    run_source
}

case "${1:-}" in
    --source)
        run_source
        ;;
    --packaged)
        run_packaged
        ;;
    --help|-h)
        usage
        ;;
    "")
        case "$(uname -s)" in
            MINGW*|MSYS*|CYGWIN*) prefer_packaged ;;
            Linux*)
                if [[ -r /proc/sys/kernel/osrelease ]] &&
                    grep -qi microsoft /proc/sys/kernel/osrelease; then
                    prefer_packaged
                else
                    run_source
                fi
                ;;
            *) run_source ;;
        esac
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
