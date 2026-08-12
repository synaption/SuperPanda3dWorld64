#!/usr/bin/env bash
set -euo pipefail

# Build and package Super Bevy World 64 for 64-bit Windows from Linux/WSL.
# The distro Rust compiler can have a different internal identity from the
# official Windows standard library even when both say "1.75.0", so this script
# installs a matching official compiler and both standard libraries under
# target/.windows-toolchain. Nothing is installed system-wide.

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../.." && pwd)"
RUST_VERSION="1.75.0"
HOST_TARGET="x86_64-unknown-linux-gnu"
WINDOWS_TARGET="x86_64-pc-windows-gnu"
TOOLCHAIN_DIR="$SCRIPT_DIR/target/.windows-toolchain/$RUST_VERSION"
DOWNLOAD_DIR="$SCRIPT_DIR/target/.windows-downloads/$RUST_VERSION"
UNPACK_DIR="$SCRIPT_DIR/target/.windows-unpack/$RUST_VERSION"
DIST_DIR="$SCRIPT_DIR/dist/windows"
ZIP_PATH="$SCRIPT_DIR/dist/SuperBevyWorld64-windows-x64.zip"

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "error: required command '$1' was not found" >&2
        exit 1
    fi
}

require_command cargo
require_command curl
require_command python3
require_command tar
require_command x86_64-w64-mingw32-gcc

download_component() {
    local component="$1"
    local target="$2"
    local archive="$DOWNLOAD_DIR/${component}-${RUST_VERSION}-${target}.tar.xz"
    local url="https://static.rust-lang.org/dist/${component}-${RUST_VERSION}-${target}.tar.xz"

    if [[ ! -s "$archive" ]]; then
        echo "Downloading $component $RUST_VERSION for $target" >&2
        curl -fL --retry 3 --output "$archive.part" "$url"
        mv "$archive.part" "$archive"
    fi
    printf '%s\n' "$archive"
}

install_component() {
    local component="$1"
    local target="$2"
    local archive
    local source_dir="$UNPACK_DIR/${component}-${target}"
    archive="$(download_component "$component" "$target")"

    mkdir -p "$source_dir"
    tar -xJf "$archive" -C "$source_dir"
    "$source_dir/${component}-${RUST_VERSION}-${target}/install.sh" \
        --prefix="$TOOLCHAIN_DIR" --disable-ldconfig
}

mkdir -p "$DOWNLOAD_DIR" "$UNPACK_DIR" "$TOOLCHAIN_DIR"

if [[ ! -x "$TOOLCHAIN_DIR/bin/rustc" ]] || \
   [[ ! -d "$TOOLCHAIN_DIR/lib/rustlib/$HOST_TARGET" ]] || \
   [[ ! -d "$TOOLCHAIN_DIR/lib/rustlib/$WINDOWS_TARGET" ]]; then
    install_component rustc "$HOST_TARGET"
    install_component rust-std "$HOST_TARGET"
    install_component rust-std "$WINDOWS_TARGET"
fi

echo "Regenerating Bevy castle assets"
python3 "$SCRIPT_DIR/tools/convert_level.py"

echo "Building $WINDOWS_TARGET release executable"
RUSTC="$TOOLCHAIN_DIR/bin/rustc" cargo build \
    --manifest-path "$SCRIPT_DIR/Cargo.toml" \
    --release --locked --target "$WINDOWS_TARGET" \
    --config "target.$WINDOWS_TARGET.linker=\"x86_64-w64-mingw32-gcc\""

echo "Packaging Windows build"
mkdir -p \
    "$DIST_DIR/assets/actors" \
    "$DIST_DIR/assets/bevy" \
    "$DIST_DIR/assets/hero" \
    "$DIST_DIR/assets/mario"

cp "$SCRIPT_DIR/target/$WINDOWS_TARGET/release/super-bevy-world-64.exe" \
    "$DIST_DIR/SuperBevyWorld64.exe"
cp "$REPO_ROOT/assets/bevy/castle.glb" "$DIST_DIR/assets/bevy/"
cp \
    "$REPO_ROOT/assets/actors/tree.glb" \
    "$REPO_ROOT/assets/actors/warp_pipe.glb" \
    "$REPO_ROOT/assets/actors/goomba.glb" \
    "$REPO_ROOT/assets/actors/scuttlebug.glb" \
    "$DIST_DIR/assets/actors/"
cp "$REPO_ROOT/assets/hero/hero.glb" "$DIST_DIR/assets/hero/"
cp "$REPO_ROOT/assets/mario/mario.glb" "$DIST_DIR/assets/mario/"

rm -f "$ZIP_PATH"
(
    cd "$SCRIPT_DIR/dist"
    python3 -m zipfile -c "$(basename "$ZIP_PATH")" windows
)

echo
echo "Windows build complete:"
echo "  $DIST_DIR/SuperBevyWorld64.exe"
echo "  $ZIP_PATH"
