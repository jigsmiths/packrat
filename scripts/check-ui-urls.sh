#!/usr/bin/env bash
# The CSP in app/src-tauri/tauri.conf.json only loads images from the app itself, so a data: URL renders nothing.
set -euo pipefail
cd "$(dirname "$0")/.."

if grep -rnE "url\(['\"]?data:" app/ui; then
    echo "The CSP blocks data: URLs. Ship the image as a file in app/ui and reference it by path."
    exit 1
fi
