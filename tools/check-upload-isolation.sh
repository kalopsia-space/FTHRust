#!/usr/bin/env bash
# The main app and the engine must not link upload network code (spec 6.5):
# reqwest, russh and the keyring live only in fthrust-uploader.
set -euo pipefail
cd "$(dirname "$0")/.."
found=$(cargo tree -p fthrust-ui -p fthrust-engine -e normal --prefix none 2>/dev/null \
  | awk '{print $1}' | grep -E '^(reqwest|russh|russh-sftp|keyring-core|zbus-secret-service-keyring-store)$' | sort -u || true)
if [ -n "$found" ]; then
  echo "upload code reached the app: $found"
  exit 1
fi
echo "upload isolation ok"
