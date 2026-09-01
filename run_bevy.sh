#!/usr/bin/env bash
set -euo pipefail

# Build and launch the Windows game from any working directory. Every project
# path is derived from this script's location, so the command is independent of
# the user name and checkout location on the current machine.

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
GAME_EXE="$REPO_ROOT/dist/windows/SpaceCrusaders.exe"

usage() {
    cat <<'EOF'
Usage: ./run_bevy.sh [--source|--windows|--packaged]

  no option    Build/package the Windows executable, then run it.
  --source     Build and run the current Rust source with Cargo.
  --windows    Build/package the Windows executable, then run it.
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

build_and_run_packaged() {
    "$REPO_ROOT/build_windows.sh"
    run_packaged
}

case "${1:-}" in
    --source)
        run_source
        ;;
    --windows)
        build_and_run_packaged
        ;;
    --packaged)
        run_packaged
        ;;
    --help|-h)
        usage
        ;;
    "")
        build_and_run_packaged
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
