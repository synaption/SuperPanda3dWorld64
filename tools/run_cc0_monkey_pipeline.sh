#!/usr/bin/env bash
set -euo pipefail

workspace_dir="/home/bob/mario"
aria_bin="$workspace_dir/tools_local/aria2-root/usr/bin/aria2c"
aria_lib="$workspace_dir/tools_local/aria2-root/usr/lib/x86_64-linux-gnu"
blender_bin="$workspace_dir/tools_local/blender-snap/blender"
download_dir="$workspace_dir/materials_download"
render_dir="$workspace_dir/monkey_material_pngs"

mkdir -p "$download_dir" "$render_dir"
mapfile -d '' torrents < <(find "$workspace_dir/cc0-textures" -name '*.torrent' -print0)

LD_LIBRARY_PATH="$aria_lib" "$aria_bin" \
  --dir="$download_dir" \
  --seed-time=0 \
  --file-allocation=none \
  --continue=true \
  --max-concurrent-downloads=22 \
  --bt-max-peers=80 \
  --summary-interval=60 \
  "${torrents[@]}"

"$blender_bin" --background \
  --python "$workspace_dir/tools/render_material_monkeys.py" -- \
  --input "$download_dir" \
  --output "$render_dir"
