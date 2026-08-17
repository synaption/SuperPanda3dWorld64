#!/usr/bin/env bash
set -euo pipefail

# Launch the game from any working directory. By default, refresh the packaged
# Windows build and then run it. The source and existing-package paths remain
# available as explicit options.

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
GAME_EXE="$REPO_ROOT/dist/windows/SuperBevyWorld64.exe"

usage() {
    cat <<'EOF'
Usage: ./run_bevy.sh [--source|--packaged]

  no option    Build/package the Windows executable, then run it.
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

build_and_run_packaged() {
    "$REPO_ROOT/build_windows.sh"
    run_packaged
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
        build_and_run_packaged
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
