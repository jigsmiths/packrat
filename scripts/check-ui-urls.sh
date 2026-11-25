#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if grep -rnE "url\(['\"]?data:" app/ui; then
    echo "The CSP in app/src-tauri/tauri.conf.json blocks data: URLs. Ship the image as a file in app/ui and reference it by path."
    exit 1
fi
