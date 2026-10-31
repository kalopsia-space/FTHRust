#!/usr/bin/env bash
# Tests for tools/check-upload-isolation.sh: it must fail, loudly, whenever
# it cannot show that the app's dependency trees are free of upload network
# code, not only when it finds some. A stub `cargo` stands in for
# `cargo tree`. Usage: tools/test-check-upload-isolation.sh
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHECK="$ROOT/tools/check-upload-isolation.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
failures=0

pass() { echo "ok $1"; }
fail() { echo "FAIL $1: $2"; failures=$((failures + 1)); }

GOOD='fthrust-core v2.0.0-alpha.2 (/src/crates/fthrust-core)
serde v1.0.228
fthrust-media v2.0.0-alpha.2 (/src/crates/fthrust-media)
fthrust-engine v2.0.0-alpha.2 (/src/crates/fthrust-engine)
fthrust-ui v2.0.0-alpha.2 (/src/crates/fthrust-ui)
egui v0.37.0'

# stub NAME STATUS STDERR < TREE: a directory whose `cargo` answers
# `cargo tree` with the tree on stdin, STDERR on stderr and exit status STATUS.
stub() {
    local dir="$WORK/$1"
    mkdir -p "$dir"
    cat > "$dir/tree"
    printf '%s' "$3" > "$dir/err"
    printf '%s' "$2" > "$dir/status"
    cat > "$dir/cargo" <<'EOF'
#!/usr/bin/env bash
here="$(dirname "${BASH_SOURCE[0]}")"
[ "$1" = tree ] || { echo "stub cargo: unexpected $*" >&2; exit 99; }
cat "$here/err" >&2
cat "$here/tree"
exit "$(cat "$here/status")"
EOF
    chmod +x "$dir/cargo"
    echo "$dir"
}

# check DIR: runs the script with DIR's cargo first on the PATH; sets out, code.
check() {
    out="$(PATH="$1:$PATH" "$CHECK" 2>&1)"
    code=$?
}

passes_a_clean_tree() {
    check "$(stub clean 0 '' <<<"$GOOD")"
    [ "$code" -eq 0 ] || { fail passes_a_clean_tree "exit $code: $out"; return; }
    grep -q 'upload isolation ok' <<<"$out" || { fail passes_a_clean_tree "no verdict: $out"; return; }
    pass passes_a_clean_tree
}

fails_when_cargo_tree_fails() {
    check "$(stub broken 101 'error: could not find `Cargo.toml`' <<<'')"
    [ "$code" -ne 0 ] || { fail fails_when_cargo_tree_fails "exit 0: $out"; return; }
    if grep -q 'upload isolation ok' <<<"$out"; then
        fail fails_when_cargo_tree_fails "claims ok: $out"; return
    fi
    grep -q 'could not find' <<<"$out" || { fail fails_when_cargo_tree_fails "cargo's error is hidden: $out"; return; }
    pass fails_when_cargo_tree_fails
}

fails_when_cargo_tree_fails_after_printing() {
    # A tree cut short by a failure must not count, even if it looks clean.
    check "$(stub cut 101 'error: failed to download' <<<"$GOOD")"
    [ "$code" -ne 0 ] || { fail fails_when_cargo_tree_fails_after_printing "exit 0: $out"; return; }
    if grep -q 'upload isolation ok' <<<"$out"; then
        fail fails_when_cargo_tree_fails_after_printing "claims ok: $out"; return
    fi
    pass fails_when_cargo_tree_fails_after_printing
}

fails_when_cargo_is_missing() {
    # A PATH with the script's tools and no cargo.
    local tool bin="$WORK/no-cargo"
    mkdir -p "$bin"
    for tool in dirname awk sort grep sed wc tr cat; do
        ln -sf "$(command -v "$tool")" "$bin/$tool"
    done
    out="$(PATH="$bin" "$(command -v bash)" "$CHECK" 2>&1)"
    code=$?
    [ "$code" -ne 0 ] || { fail fails_when_cargo_is_missing "exit 0: $out"; return; }
    if grep -q 'upload isolation ok' <<<"$out"; then
        fail fails_when_cargo_is_missing "claims ok: $out"; return
    fi
    pass fails_when_cargo_is_missing
}

fails_on_an_empty_tree() {
    check "$(stub empty 0 '' <<<'')"
    [ "$code" -ne 0 ] || { fail fails_on_an_empty_tree "exit 0: $out"; return; }
    if grep -q 'upload isolation ok' <<<"$out"; then
        fail fails_on_an_empty_tree "claims ok: $out"; return
    fi
    pass fails_on_an_empty_tree
}

fails_when_an_app_crate_is_missing() {
    # Not the tree of the app at all: nothing in it can be called isolated.
    check "$(stub other 0 '' <<<$'serde v1.0.228\nlibc v0.2.170')"
    [ "$code" -ne 0 ] || { fail fails_when_an_app_crate_is_missing "exit 0: $out"; return; }
    grep -q 'fthrust-' <<<"$out" || { fail fails_when_an_app_crate_is_missing "does not say what is missing: $out"; return; }
    pass fails_when_an_app_crate_is_missing
}

rejects_every_network_crate() {
    local crate bad=0
    for crate in reqwest hyper hyper-util hyper-rustls h2 rustls rustls-webpki tokio-rustls \
        webpki-roots native-tls openssl openssl-sys ureq russh russh-sftp keyring keyring-core \
        zbus-secret-service-keyring-store secret-service oo7 fthrust-uploader; do
        check "$(stub "bad-$crate" 0 '' <<<"$GOOD"$'\n'"$crate v1.0.0")"
        if [ "$code" -eq 0 ] || ! grep -qx "  $crate" <<<"$out" || grep -q 'upload isolation ok' <<<"$out"; then
            echo "  $crate was let through: exit $code, $out"
            bad=1
        fi
    done
    [ $bad -eq 0 ] && pass rejects_every_network_crate || fail rejects_every_network_crate "see above"
}

leaves_look_alikes_alone() {
    # Crates that only start like a forbidden one.
    check "$(stub alike 0 '' <<<"$GOOD"$'\nhyperfine v1.0.0\nh264-reader v0.8.0\nkeyringer v1\nreqwester v1\nrustlsish v1')"
    [ "$code" -eq 0 ] && pass leaves_look_alikes_alone || fail leaves_look_alikes_alone "exit $code: $out"
}

passes_on_this_repository() {
    out="$("$CHECK" 2>&1)"
    code=$?
    [ "$code" -eq 0 ] && grep -q 'upload isolation ok' <<<"$out" \
        && pass passes_on_this_repository || fail passes_on_this_repository "exit $code: $out"
}

passes_a_clean_tree
fails_when_cargo_tree_fails
fails_when_cargo_tree_fails_after_printing
fails_when_cargo_is_missing
fails_on_an_empty_tree
fails_when_an_app_crate_is_missing
rejects_every_network_crate
leaves_look_alikes_alone
passes_on_this_repository

if [ $failures -ne 0 ]; then
    echo "$failures failed"
    exit 1
fi
echo "all passed"
