#!/usr/bin/env bash
set -euo pipefail

# Build and package Super Bevy World 64 for 64-bit Windows from Linux/WSL.
#
# This used to download an official Rust compiler and both standard libraries
# into target/, because the distro's Rust 1.75 and the official Windows
# rust-std of the same version had different internal identities and would not
# link against each other. Bevy 0.19 needs Rust 1.95, which is past anything a
# distro here ships, so the build already runs on a rustup toolchain -- and one
# toolchain providing both the host and the Windows target is exactly what that
# apparatus was faking. It is gone; `rustup target add` is the whole of it now.

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
WINDOWS_TARGET="x86_64-pc-windows-gnu"
DIST_DIR="$REPO_ROOT/dist/windows"
ZIP_PATH="$REPO_ROOT/dist/SuperBevyWorld64-windows-x64.zip"

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "error: required command '$1' was not found" >&2
        exit 1
    fi
}

require_command cargo
require_command rustup
require_command python3
# The GNU target links with MinGW's linker rather than one Rust ships.
require_command x86_64-w64-mingw32-gcc

if ! rustup target list --installed | grep -qx "$WINDOWS_TARGET"; then
    echo "Installing the $WINDOWS_TARGET standard library" >&2
    rustup target add "$WINDOWS_TARGET"
fi

echo "Regenerating Bevy castle assets"
python3 "$REPO_ROOT/tools/convert_level.py"

echo "Building $WINDOWS_TARGET release executable"
cargo build \
    --manifest-path "$REPO_ROOT/Cargo.toml" \
    --release --locked --target "$WINDOWS_TARGET" \
    --config "target.$WINDOWS_TARGET.linker=\"x86_64-w64-mingw32-gcc\""

echo "Packaging Windows build"
mkdir -p \
    "$DIST_DIR/assets/actors" \
    "$DIST_DIR/assets/bevy" \
    "$DIST_DIR/assets/hero" \
    "$DIST_DIR/assets/mario"

cp "$REPO_ROOT/target/$WINDOWS_TARGET/release/super-bevy-world-64.exe" \
    "$DIST_DIR/SuperBevyWorld64.exe"
cp "$REPO_ROOT/assets/bevy/castle.glb" "$REPO_ROOT/assets/bevy/water.png" \
    "$DIST_DIR/assets/bevy/"
cp \
    "$REPO_ROOT/assets/actors/tree.glb" \
    "$REPO_ROOT/assets/actors/warp_pipe.glb" \
    "$REPO_ROOT/assets/actors/goomba.glb" \
    "$REPO_ROOT/assets/actors/scuttlebug.glb" \
    "$DIST_DIR/assets/actors/"
cp "$REPO_ROOT/assets/hero/hero.glb" "$DIST_DIR/assets/hero/"
cp "$REPO_ROOT/assets/mario/mario.glb" "$DIST_DIR/assets/mario/"

# Only the samples the sound tables actually name, read out of the tables
# themselves so the two cannot drift: the sound directories hold thousands of
# files and the game plays a couple of dozen.
while read -r sample; do
    [[ -n "$sample" ]] || continue
    mkdir -p "$DIST_DIR/assets/sounds/$(dirname "$sample")"
    cp "$REPO_ROOT/assets/sounds/$sample" "$DIST_DIR/assets/sounds/$sample"
done < <(grep -o '"[A-Za-z0-9_/]*\.wav"' "$REPO_ROOT/src/audio.rs" | tr -d '"' | sort -u)

rm -f "$ZIP_PATH"
(
    cd "$REPO_ROOT/dist"
    python3 -m zipfile -c "$(basename "$ZIP_PATH")" windows
)

echo
echo "Windows build complete:"
echo "  $DIST_DIR/SuperBevyWorld64.exe"
echo "  $ZIP_PATH"
