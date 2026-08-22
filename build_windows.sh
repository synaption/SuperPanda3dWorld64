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
    "$DIST_DIR/assets/impostors" \
    "$DIST_DIR/assets/mario"

cp "$REPO_ROOT/target/$WINDOWS_TARGET/release/super-bevy-world-64.exe" \
    "$DIST_DIR/SuperBevyWorld64.exe"
cp "$REPO_ROOT/assets/bevy/castle.glb" "$REPO_ROOT/assets/bevy/water.png" \
    "$DIST_DIR/assets/bevy/"
cp \
    "$REPO_ROOT/assets/actors/tree.glb" \
    "$REPO_ROOT/assets/actors/warp_pipe.glb" \
    "$REPO_ROOT/assets/actors/slime.glb" \
    "$REPO_ROOT/assets/actors/ant.glb" \
    "$DIST_DIR/assets/actors/"
cp "$REPO_ROOT/assets/hero/hero.glb" "$DIST_DIR/assets/hero/"
# The weapon models, read out of `weapon::Weapon::spec` rather than listed, for
# the same reason the samples below are read out of the sound tables: the two
# cannot drift if there is only one of them.
#
# They drifted once already. The target pistol was added to the game and not to
# this list, and a packaged build has no gun in it and says nothing -- Bevy logs
# the failed load to a stderr that a `windows_subsystem = "windows"` build has
# nobody attached to, exactly as with the impostor sheets below. What it looks
# like from the outside is a weapon that does not exist.
while read -r model; do
    [[ -n "$model" ]] || continue
    mkdir -p "$DIST_DIR/assets/$(dirname "$model")"
    cp "$REPO_ROOT/assets/$model" "$DIST_DIR/assets/$model"
done < <(grep -o '"[A-Za-z0-9_/]*\.glb#[A-Za-z0-9]*"' "$REPO_ROOT/src/weapon.rs" \
    | tr -d '"' | cut -d'#' -f1 | sort -u)
cp "$REPO_ROOT/assets/mario/mario.glb" "$DIST_DIR/assets/mario/"
# The impostor sheets. Leaving these out does not fail and does not crash: the
# game starts, and every enemy past `enemy_draw` is drawn as nothing at all,
# because the sprite that should stand in for it needs an atlas that is not
# there. What that looks like from the outside is enemies popping into existence
# as you walk towards them, and the only word about it goes to a stderr that a
# `windows_subsystem = "windows"` build has nobody attached to.
cp "$REPO_ROOT/assets/impostors/"*.png "$REPO_ROOT/assets/impostors/"*.json \
    "$DIST_DIR/assets/impostors/"

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
