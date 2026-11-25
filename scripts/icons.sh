#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

icons=app/src-tauri/icons
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

swiftc -O -o "$work/render-svg" scripts/render-svg.swift
render="$work/render-svg"

# Menu bar template image at 22pt @2x
"$render" "$icons/tray.svg" "$icons/tray.png" 44
"$render" "$icons/icon.svg" "$icons/icon.png" 512

mkdir "$work/icon.iconset"
for size in 16 32 128 256 512; do
    "$render" "$icons/icon.svg" "$work/icon.iconset/icon_${size}x${size}.png" "$size"
    "$render" "$icons/icon.svg" "$work/icon.iconset/icon_${size}x${size}@2x.png" "$((size * 2))"
done
iconutil -c icns -o "$icons/icon.icns" "$work/icon.iconset"
