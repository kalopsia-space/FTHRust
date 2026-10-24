#!/usr/bin/env sh
# Verifies every bundled asset and packaging asset against its manifest, and,
# when reference/ is checked out, that each is byte-identical to the reference
# copy (spec section 4: assets are copied from the reference unchanged).
set -eu
cd "$(dirname "$0")/.."
status=0
(cd assets && sha256sum -c --quiet manifest.sha256) || status=1
(cd packaging && sha256sum -c --quiet manifest.sha256) || status=1
if [ ! -d reference/FTHR_UI/assets ]; then
  echo "reference comparison skipped: reference/ is not checked out (run tools/fetch-reference.sh)"
  exit $status
fi
while read -r sum rel; do
  ref="reference/FTHR_UI/assets/${rel#./}"
  if [ ! -f "$ref" ]; then echo "missing in reference: $rel"; status=1; continue; fi
  refsum=$(sha256sum "$ref" | cut -d' ' -f1)
  if [ "$sum" != "$refsum" ]; then echo "differs from reference: $rel"; status=1; fi
done < assets/manifest.sha256
if ! cmp -s packaging/space.kalopsia.fthrust.png reference/AppDir/fthr-clips.png; then
  echo "differs from reference: packaging/space.kalopsia.fthrust.png"; status=1
fi
exit $status
