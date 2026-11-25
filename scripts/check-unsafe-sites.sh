#!/usr/bin/env bash
# Fails when the files or counts of #[expect(unsafe_code)] differ from unsafe-sites.txt.
set -euo pipefail
cd "$(dirname "$0")/.."

found=$(grep -rc --include='*.rs' unsafe_code crates app/src-tauri/src | grep -v ':0$' | sort)
if ! diff -u unsafe-sites.txt <(echo "$found"); then
    echo "unsafe_code sites changed. Look for a safe objc2 API first, as #30 did."
    echo "If unsafe is still needed, update unsafe-sites.txt so the review shows it."
    exit 1
fi
