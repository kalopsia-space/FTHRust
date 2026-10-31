#!/usr/bin/env bash
# The main app, the engine and everything they share must not link upload
# network code (spec 6.5): HTTP, TLS, SSH and the keyring live only in
# fthrust-uploader. This checks the normal dependency tree of every
# workspace crate but the uploader.
#
# It fails when it finds such a crate, and also when it cannot check: a
# `cargo tree` that fails or prints nothing, or a tree that does not list
# the app's own crates, is not a pass. (tools/test-check-upload-isolation.sh
# tests that.)
set -euo pipefail
cd "$(dirname "$0")/.."

# Forbidden crates and their families (hyper-util, rustls-webpki, russh-sftp,
# keyring-core, ...), and the uploader itself.
DENY='^(reqwest|hyper|h2|rustls|tokio-rustls|webpki|native-tls|openssl|ureq|russh|keyring|zbus-secret-service-keyring-store|secret-service|oo7|fthrust-uploader)($|-)'
# Crates the tree has to list, or it is not the tree of the app.
EXPECTED=(fthrust-core fthrust-engine fthrust-media fthrust-ui)

# No `|| true` and no hidden stderr: a failure here ends the script, with
# cargo's own message.
tree=$(cargo tree --locked --workspace --exclude fthrust-uploader -e normal --prefix none)
names=$(awk '{ print $1 }' <<<"$tree" | sort -u)

if [ -z "$names" ]; then
  echo "upload isolation check failed: cargo tree printed nothing" >&2
  exit 1
fi
for crate in "${EXPECTED[@]}"; do
  if ! grep -qx -- "$crate" <<<"$names"; then
    echo "upload isolation check failed: cargo tree does not list $crate" >&2
    exit 1
  fi
done

found=$(awk -v deny="$DENY" '$0 ~ deny' <<<"$names")
if [ -n "$found" ]; then
  echo "upload code reached the app:" >&2
  sed 's/^/  /' <<<"$found" >&2
  exit 1
fi
echo "upload isolation ok ($(wc -l <<<"$names" | tr -d ' ') crates checked)"
