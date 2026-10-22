#!/usr/bin/env bash
# Fetch the FTHR Clips reference checkout at the pinned commit into reference/.
set -euo pipefail
PIN=aa0dc9b
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
if [ ! -d reference/.git ]; then
    git clone https://github.com/FTHR-Community/FTHR-Clips reference
fi
if ! git -C reference cat-file -e "$PIN^{commit}" 2>/dev/null; then
    # GitHub does not serve abbreviated SHAs by name; fetch all refs instead.
    git -C reference fetch origin
fi
git -C reference checkout --detach "$PIN"
HEAD_SHA="$(git -C reference rev-parse HEAD)"
case "$HEAD_SHA" in
    "$PIN"*) echo "reference at $PIN" ;;
    *) echo "reference HEAD is $HEAD_SHA, expected $PIN" >&2; exit 1 ;;
esac
