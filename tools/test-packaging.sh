#!/usr/bin/env bash
# Tests for the packaging scripts. Usage: tools/test-packaging.sh [fetch|gate|version|install|archive]...
# (no argument runs every group). The fetch group downloads the pinned
# AppImage runtime (~1 MB); the gate group needs the fetched FFmpeg tree,
# patchelf and cc; the archive group needs git.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FETCH="$ROOT/packaging/fetch-third-party.sh"
GATE="$ROOT/packaging/verify-appdir.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
failures=0

pass() { echo "ok $1"; }
fail() { echo "FAIL $1: $2"; failures=$((failures + 1)); }

# A private copy of the manifests that a test may edit.
manifests() {
    local dir="$WORK/manifests-$1"
    mkdir -p "$dir"
    cp "$ROOT"/packaging/*_manifest*.json "$dir/"
    echo "$dir"
}

edit_json() { # file jq-filter
    local tmp="$1.tmp"
    jq "$2" "$1" > "$tmp" && mv "$tmp" "$1"
}

rejects_http_url() {
    local m c out
    m="$(manifests http)"; c="$WORK/cache-http"
    edit_json "$m/appimage_tool_manifest.json" '.runtime.url |= sub("^https"; "http")'
    out="$("$FETCH" --appimagetool --manifest-dir "$m" --cache "$c" 2>&1)"
    if [ $? -eq 0 ]; then fail rejects_http_url "exit 0"; return; fi
    if ! grep -q https <<<"$out"; then fail rejects_http_url "message lacks https: $out"; return; fi
    if [ -n "$(find "$c" -type f 2>/dev/null)" ]; then fail rejects_http_url "cache not empty"; return; fi
    pass rejects_http_url
}

rejects_bad_sha_format() {
    local m c
    m="$(manifests sha)"; c="$WORK/cache-sha"
    edit_json "$m/appimage_tool_manifest.json" '.runtime.sha256 = "abc"'
    local out
    if out="$("$FETCH" --appimagetool --manifest-dir "$m" --cache "$c" 2>&1)"; then
        fail rejects_bad_sha_format "exit 0"; return
    fi
    if ! grep -q "fetch: .*sha256" <<<"$out"; then fail rejects_bad_sha_format "message: $out"; return; fi
    if [ -n "$(find "$c" -type f 2>/dev/null)" ]; then fail rejects_bad_sha_format "downloaded anyway"; return; fi
    pass rejects_bad_sha_format
}

refetches_tampered_cache() {
    local m c want got
    m="$(manifests tamper)"; c="$WORK/cache-tamper"
    # Runtime only: the appimagetool download is 15 MB, so drop it from this copy.
    edit_json "$m/appimage_tool_manifest.json" 'del(.appimagetool)'
    "$FETCH" --appimagetool --manifest-dir "$m" --cache "$c" >/dev/null 2>&1 || { fail refetches_tampered_cache "first fetch failed"; return; }
    printf x >> "$c/runtime-x86_64"
    "$FETCH" --appimagetool --manifest-dir "$m" --cache "$c" >/dev/null 2>&1 || { fail refetches_tampered_cache "second fetch failed"; return; }
    want="$(jq -r .runtime.sha256 "$m/appimage_tool_manifest.json")"
    got="$(sha256sum "$c/runtime-x86_64" | cut -d' ' -f1)"
    [ "$want" = "$got" ] && pass refetches_tampered_cache || fail refetches_tampered_cache "hash $got"
}

rejects_size_mismatch() {
    local m c
    m="$(manifests size)"; c="$WORK/cache-size"
    edit_json "$m/appimage_tool_manifest.json" 'del(.appimagetool) | .runtime.size += 1'
    local out
    if out="$("$FETCH" --appimagetool --manifest-dir "$m" --cache "$c" 2>&1)"; then
        fail rejects_size_mismatch "exit 0"; return
    fi
    if ! grep -q "fetch: .*size" <<<"$out"; then fail rejects_size_mismatch "message: $out"; return; fi
    if [ -n "$(find "$c" -type f 2>/dev/null)" ]; then fail rejects_size_mismatch "file left in cache"; return; fi
    pass rejects_size_mismatch
}

group_fetch() {
    rejects_http_url
    rejects_bad_sha_format
    refetches_tampered_cache
    rejects_size_mismatch
}

# A minimal AppDir that passes the gate: the pinned FFmpeg libs and three
# copies of a tiny program linked against libavutil.
fixture() {
    local d="$WORK/appdir-$1" ff="$ROOT/packaging/.cache/ffmpeg" m="$ROOT/packaging/ffmpeg_manifest_linux.json"
    mkdir -p "$d/usr/bin/assets/fonts" "$d/usr/lib" "$d/licenses"
    printf 'int av_version_info(void);\nint main(void){return av_version_info()==0;}\n' > "$WORK/t.c"
    cc -o "$WORK/t" "$WORK/t.c" -L"$ff/lib" -l:libavutil.so.60 || return 1
    for b in fthrust fthrust-engine fthrust-uploader; do
        cp "$WORK/t" "$d/usr/bin/$b"
        patchelf --force-rpath --set-rpath '$ORIGIN/../lib' "$d/usr/bin/$b"
    done
    while read -r file soname; do
        cp "$ff/lib/$file" "$d/usr/lib/"
        ln -s "$file" "$d/usr/lib/$soname"
    done < <(jq -r '.soname_map | to_entries[] | "\(.key) \(.value)"' "$m")
    printf '#!/bin/sh\n' > "$d/AppRun"; chmod +x "$d/AppRun"
    cp "$ROOT/packaging/space.kalopsia.fthrust.desktop" "$ROOT/packaging/space.kalopsia.fthrust.png" "$d/"
    ln -s space.kalopsia.fthrust.png "$d/.DirIcon"
    cp "$ROOT/LICENSE" "$d/"; echo notices > "$d/THIRD_PARTY_NOTICES.md"
    cp "$ROOT/assets/fonts/Oswald-Regular.ttf" "$d/usr/bin/assets/fonts/"
    echo "$d"
}

gate_expect() { # name expected-exit expected-line dir [env...]
    local name="$1" want="$2" line="$3" dir="$4" out rc
    shift 4
    out="$(env "$@" "$GATE" "$dir" 2>&1)"; rc=$?
    if [ $rc -ne "$want" ]; then fail "$name" "exit $rc: $(tail -3 <<<"$out")"; return; fi
    if ! grep -q -- "$line" <<<"$out"; then fail "$name" "missing '$line' in: $out"; return; fi
    pass "$name"
}

group_gate() {
    local d
    "$FETCH" --ffmpeg >/dev/null || { fail fixture "could not fetch FFmpeg"; return; }
    d="$(fixture pass)" || { fail fixture "could not build the fixture"; return; }
    gate_expect gate_passes_fixture 0 "RESULT: PASS" "$d"

    d="$(fixture lib)"; printf x >> "$d/usr/lib/libavutil.so.60.26.102"
    gate_expect gate_fails_tampered_lib 1 "FAIL ffmpeg-libs" "$d"

    d="$(fixture runpath)"; patchelf --set-rpath '$ORIGIN/../lib' "$d/usr/bin/fthrust-engine"
    gate_expect gate_fails_runpath 1 "FAIL linkage" "$d"

    d="$(fixture flag)"; printf -- '--enable-gpl' >> "$d/usr/bin/fthrust-uploader"
    gate_expect gate_fails_forbidden_flag 1 "FAIL forbidden-flags" "$d"

    d="$(fixture pw)"; touch "$d/usr/lib/libpipewire-0.3.so.0"
    gate_expect gate_fails_bundled_pipewire 1 "FAIL linkage" "$d"

    d="$(fixture config)"; jq '.configuration += " --enable-libfoo"' "$ROOT/packaging/ffmpeg_manifest_linux.json" > "$WORK/manifest-config.json"
    gate_expect gate_fails_config_mismatch 1 "FAIL ffmpeg-config" "$d" FFMPEG_MANIFEST_OVERRIDE="$WORK/manifest-config.json"

    d="$(fixture noconfig)"; jq 'del(.configuration)' "$ROOT/packaging/ffmpeg_manifest_linux.json" > "$WORK/manifest-noconfig.json"
    gate_expect gate_fails_missing_config 1 "FAIL ffmpeg-config" "$d" FFMPEG_MANIFEST_OVERRIDE="$WORK/manifest-noconfig.json"

    d="$(fixture missing)"; rm "$d/THIRD_PARTY_NOTICES.md"
    gate_expect gate_fails_missing_notices 1 "FAIL files" "$d"

    d="$(fixture glibc)"
    gate_expect gate_fails_glibc 1 "FAIL glibc" "$d" MAX_GLIBC_OVERRIDE=2.0

    d="$(fixture symroot)"; printf x >> "$d/usr/lib/libavutil.so.60.26.102"; ln -s "$d" "$WORK/symroot"
    gate_expect gate_follows_symlinked_root 1 "FAIL ffmpeg-libs" "$WORK/symroot"

    d="$(fixture symlib)"; mv "$d/usr/lib" "$WORK/reallib"; ln -s "$WORK/reallib" "$d/usr/lib"
    printf -- '--enable-nonfree' >> "$WORK/reallib/libswscale.so.9.5.102"
    gate_expect gate_follows_symlinked_dir 1 "FAIL forbidden-flags" "$d"

    gate_expect gate_rejects_file_input 1 "is not a directory" "$d/AppRun"
    mkdir -p "$WORK/empty"
    gate_expect gate_fails_empty_dir 1 "no ELF files found" "$WORK/empty"
}

group_version() {
    local cv="$ROOT/tools/check-version.sh" v out
    v="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
    if out="$("$cv" 2>&1)"; then pass version_tree_consistent; else fail version_tree_consistent "$out"; fi
    if out="$("$cv" --tag "refs/tags/v$v" 2>&1)"; then pass version_tag_matches; else fail version_tag_matches "$out"; fi
    if out="$("$cv" --tag refs/tags/v0.0.1 2>&1)"; then fail version_tag_mismatch "exit 0"
    elif grep -q "version mismatch: tag has 0.0.1, Cargo.toml has $v" <<<"$out"; then pass version_tag_mismatch
    else fail version_tag_mismatch "$out"; fi
    printf '#!/bin/sh\necho "fthrust 9.9.9"\n' > "$WORK/fake"; chmod +x "$WORK/fake"
    if out="$("$cv" --binary "$WORK/fake" 2>&1)"; then fail version_binary_mismatch "exit 0"
    elif grep -q "version mismatch: .*fake has 9.9.9, Cargo.toml has $v" <<<"$out"; then pass version_binary_mismatch
    else fail version_binary_mismatch "$out"; fi
}

group_install() {
    local inst="$ROOT/packaging/install.sh" t="$WORK/inst-target/release" d="$WORK/inst-dest" p=/opt/fthrust out
    mkdir -p "$t"
    for b in fthrust fthrust-engine fthrust-uploader; do
        printf '#!/bin/sh\necho %s\n' "$b" > "$t/$b"; chmod +x "$t/$b"
    done
    run_install() { DESTDIR="$d" CARGO_TARGET_DIR="$WORK/inst-target" "$inst" --no-build --prefix "$p" "$@"; }

    if ! out="$(run_install 2>&1)"; then fail install_runs "$out"; return; fi
    pass install_runs
    local missing=""
    for f in bin/fthrust bin/fthrust-engine bin/fthrust-uploader share/fthrust/assets/fonts \
        share/applications/space.kalopsia.fthrust.desktop \
        share/icons/hicolor/512x512/apps/space.kalopsia.fthrust.png \
        share/licenses/fthrust/LICENSE share/licenses/fthrust/licenses; do
        [ -e "$d$p/$f" ] || missing="$missing $f"
    done
    if [ -z "$missing" ]; then pass install_layout; else fail install_layout "missing:$missing"; fi
    if grep -qx "Exec=$p/bin/fthrust" "$d$p/share/applications/space.kalopsia.fthrust.desktop"; then
        pass install_desktop_exec_absolute
    else fail install_desktop_exec_absolute "$(grep Exec= "$d$p/share/applications/space.kalopsia.fthrust.desktop")"; fi

    touch "$d$p/share/fthrust/assets/stale"
    run_install >/dev/null 2>&1
    if [ -e "$d$p/share/fthrust/assets/stale" ] || [ -e "$d$p/share/licenses/fthrust/licenses/licenses" ]; then
        fail install_reinstall_replaces_trees "stale file or nested licences left"
    else pass install_reinstall_replaces_trees; fi

    rm "$t/fthrust-engine"
    if out="$(run_install 2>&1)"; then fail install_rejects_missing_binary "exit 0"
    elif grep -q "fthrust-engine is missing" <<<"$out"; then pass install_rejects_missing_binary
    else fail install_rejects_missing_binary "$out"; fi

    if out="$("$inst" --no-build --prefix relative/dir 2>&1)"; then fail install_rejects_relative_prefix "exit 0"
    else pass install_rejects_relative_prefix; fi

    run_install --uninstall >/dev/null 2>&1
    out="$(find "$d$p" -type f)"
    if [ -z "$out" ]; then pass install_uninstall_removes_files; else fail install_uninstall_removes_files "left: $out"; fi
}

group_archive() {
    local arch="$ROOT/packaging/archive-ffmpeg-source.sh" src="$WORK/srcrepos" m out r ffc bsc
    mkdir -p "$src/ffmpeg" "$src/scripts/scripts.d"
    echo 'int main(void){return 0;}' > "$src/ffmpeg/main.c"
    echo '#!/bin/sh' > "$src/scripts/scripts.d/50-x.sh"
    for r in ffmpeg scripts; do
        git -C "$src/$r" init -q
        git -C "$src/$r" config uploadpack.allowAnySHA1InWant true
        git -C "$src/$r" add -A
        git -C "$src/$r" -c user.name=t -c user.email=t@example.invalid commit -q -m init
    done
    ffc="$(git -C "$src/ffmpeg" rev-parse HEAD)"
    bsc="$(git -C "$src/scripts" rev-parse HEAD)"
    m="$WORK/source-manifest.json"
    jq --arg a "$src/ffmpeg" --arg b "$ffc" --arg c "$src/scripts" --arg d "$bsc" \
        '.corresponding_source = {ffmpeg: {repo: $a, commit: $b, describe: "n1-test"}, build_scripts: {repo: $c, commit: $d}}' \
        "$ROOT/packaging/ffmpeg_manifest_linux.json" > "$m"
    out="$WORK/source-out/src.tar.xz"

    if ! r="$(ARCHIVE_ALLOW_LOCAL_REPO=1 "$arch" --manifest "$m" --out "$out" 2>&1)"; then
        fail archive_builds "$r"; return
    fi
    local listing missing="" f readme
    listing="$(tar -tJf "$out" | sed 's|^\./||')"
    for f in ffmpeg-n1-test/main.c "FFmpeg-Builds-${bsc:0:12}/scripts.d/50-x.sh" README.txt; do
        grep -qxF "$f" <<<"$listing" || missing="$missing $f"
    done
    if [ -z "$missing" ]; then pass archive_has_both_trees_and_readme; else fail archive_has_both_trees_and_readme "missing:$missing"; fi
    readme="$(tar -xJOf "$out" ./README.txt)"
    if grep -qF "$ffc" <<<"$readme" && grep -qF -- "--enable-libdav1d" <<<"$readme" && grep -qF "$(jq -r .source.sha256 "$m")" <<<"$readme"; then
        pass archive_readme_names_commits_configuration_and_binary
    else fail archive_readme_names_commits_configuration_and_binary "$readme"; fi
    if (cd "$WORK/source-out" && sha256sum -c --quiet src.tar.xz.sha256 >/dev/null 2>&1); then pass archive_checksum; else fail archive_checksum "sha256 mismatch"; fi

    jq '.corresponding_source.ffmpeg.commit = "0000000000000000000000000000000000000000"' "$m" > "$WORK/source-bad-commit.json"
    if r="$(ARCHIVE_ALLOW_LOCAL_REPO=1 "$arch" --manifest "$WORK/source-bad-commit.json" --out "$WORK/source-out/bad.tar.xz" 2>&1)"; then
        fail archive_unknown_commit_fails "exit 0"
    elif grep -q "could not fetch" <<<"$r" && [ ! -e "$WORK/source-out/bad.tar.xz" ]; then pass archive_unknown_commit_fails
    else fail archive_unknown_commit_fails "$r"; fi

    jq '.corresponding_source.ffmpeg.commit = "v8.1"' "$m" > "$WORK/source-tag-pin.json"
    if r="$(ARCHIVE_ALLOW_LOCAL_REPO=1 "$arch" --manifest "$WORK/source-tag-pin.json" --out "$WORK/source-out/tag.tar.xz" 2>&1)"; then
        fail archive_rejects_a_movable_pin "exit 0"
    elif grep -q "40 lowercase hex" <<<"$r"; then pass archive_rejects_a_movable_pin
    else fail archive_rejects_a_movable_pin "$r"; fi

    if r="$("$arch" --manifest "$m" --out "$WORK/source-out/local.tar.xz" 2>&1)"; then
        fail archive_rejects_a_non_https_repo "exit 0"
    elif grep -q "must be https" <<<"$r"; then pass archive_rejects_a_non_https_repo
    else fail archive_rejects_a_non_https_repo "$r"; fi
}

groups=("$@")
[ ${#groups[@]} -eq 0 ] && groups=(fetch gate version install archive)
for g in "${groups[@]}"; do
    if ! declare -F "group_$g" >/dev/null; then fail "group_$g" "no such group"; continue; fi
    "group_$g"
done
[ $failures -eq 0 ] && echo "all ok" || echo "$failures failed"
[ $failures -eq 0 ]
