#!/usr/bin/env sh
# Verifies that every bundled asset is byte-identical to the reference copy
# (spec section 4: assets are copied from the reference unchanged).
set -eu
cd "$(dirname "$0")/.."
if [ ! -d reference/FTHR_UI/assets ]; then
  echo "skipping: reference/ is not checked out (run tools/fetch-reference.sh)"
  exit 0
fi
status=0
while read -r sum rel; do
  ref="reference/FTHR_UI/assets/${rel#./}"
  if [ ! -f "$ref" ]; then echo "missing in reference: $rel"; status=1; continue; fi
  refsum=$(sha256sum "$ref" | cut -d' ' -f1)
  if [ "$sum" != "$refsum" ]; then echo "differs from reference: $rel"; status=1; fi
done < assets/manifest.sha256
(cd assets && sha256sum -c --quiet manifest.sha256) || status=1
exit $status
