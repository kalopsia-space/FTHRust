#!/usr/bin/env bash
# Tests for the packaging scripts. Usage: tools/test-packaging.sh [fetch|gate|version|install|notes|archive|depends|links|ffnotice|desktop|deps|profile|workflow]...
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
    cp "$ROOT/LICENSE" "$d/"
    printf 'notices: [LICENSE](LICENSE), [licenses](licenses/), [manifest](packaging/ffmpeg_manifest_linux.json)\n' > "$d/THIRD_PARTY_NOTICES.md"
    mkdir -p "$d/packaging"; cp "$m" "$d/packaging/"
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

    d="$(fixture gplmoved)"; jq '(.supporting_libraries[] | select(.id == "fftw3") | .in_libraries) = ["libavcodec"]' "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-moved.json"
    gate_expect gate_fails_when_a_gpl_library_is_not_where_the_notices_say 1 "FAIL gpl-libs" "$d" FFMPEG_LIBRARIES_OVERRIDE="$WORK/reg-moved.json"

    d="$(fixture gplgone)"; jq '(.supporting_libraries[] | select(.id == "fftw3") | .signatures) = ["no_such_string_in_any_library"]' "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-gone.json"
    gate_expect gate_fails_when_a_gpl_library_is_not_in_the_libraries 1 "FAIL gpl-libs" "$d" FFMPEG_LIBRARIES_OVERRIDE="$WORK/reg-gone.json"

    d="$(fixture gplextra)"; jq '(.supporting_libraries[] | select(.id == "fftw3") | .signatures) = ["av_version_info"]' "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-extra.json"
    gate_expect gate_fails_when_a_gpl_library_is_also_in_other_libraries 1 "FAIL gpl-libs" "$d" FFMPEG_LIBRARIES_OVERRIDE="$WORK/reg-extra.json"

    # Every signature has to be there: one of FFTW's solver names alone is not enough.
    d="$(fixture gplpart)"; jq '(.supporting_libraries[] | select(.id == "fftw3") | .signatures) += ["no_such_string_in_any_library"]' "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-part.json"
    gate_expect gate_needs_every_signature_of_a_gpl_library 1 "no_such_string_in_any_library) is not in libavformat" "$d" FFMPEG_LIBRARIES_OVERRIDE="$WORK/reg-part.json"

    d="$(fixture gplabsent)"; jq '(.supporting_libraries[] | select(.id == "fftw3") | .in_libraries) = ["libpostproc"]' "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-absent.json"
    gate_expect gate_fails_when_the_library_the_notices_name_is_not_bundled 1 "libpostproc, which the notices say holds it, is not in the package" "$d" FFMPEG_LIBRARIES_OVERRIDE="$WORK/reg-absent.json"

    # A registry the gate cannot use must not let it pass without having checked anything.
    d="$(fixture noreg)"
    gate_expect gate_fails_when_the_registry_is_missing 1 "FAIL gpl-libs: cannot read the GPL libraries from no-such-registry.json" "$d" FFMPEG_LIBRARIES_OVERRIDE="$WORK/no-such-registry.json"
    printf '{ "libraries": [' > "$WORK/reg-broken.json"
    gate_expect gate_fails_when_the_registry_is_not_json 1 "FAIL gpl-libs: cannot read the GPL libraries from reg-broken.json" "$d" FFMPEG_LIBRARIES_OVERRIDE="$WORK/reg-broken.json"
    jq '(.supporting_libraries[] | select(.id == "fftw3")) |= del(.signatures)' "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-nosigs.json"
    gate_expect gate_fails_when_a_gpl_library_lacks_signatures 1 "FAIL gpl-libs: cannot read the GPL libraries from reg-nosigs.json" "$d" FFMPEG_LIBRARIES_OVERRIDE="$WORK/reg-nosigs.json"
    jq '(.supporting_libraries[] | select(.id == "fftw3")) |= del(.gpl)' "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-nogpl-gate.json"
    gate_expect gate_fails_when_the_registry_names_no_gpl_library 1 "FAIL gpl-libs: reg-nogpl-gate.json marks no library" "$d" FFMPEG_LIBRARIES_OVERRIDE="$WORK/reg-nogpl-gate.json"
    jq '(.supporting_libraries[] | select(.id == "fftw3") | .in_libraries) = []' "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-noholder.json"
    gate_expect gate_fails_when_a_gpl_library_names_no_bundled_library 1 "fftw3 has no signatures or no in_libraries" "$d" FFMPEG_LIBRARIES_OVERRIDE="$WORK/reg-noholder.json"
    d="$(fixture nolibs)"; rm "$d"/usr/lib/libav* "$d"/usr/lib/libsw* "$d"/usr/lib/libpostproc* 2>/dev/null
    gate_expect gate_fails_when_no_ffmpeg_library_is_bundled 1 "FAIL gpl-libs: .*no FFmpeg libraries found" "$d"

    d="$(fixture missing)"; rm "$d/THIRD_PARTY_NOTICES.md"
    gate_expect gate_fails_missing_notices 1 "FAIL files" "$d"

    d="$(fixture nomanifest)"; rm -r "$d/packaging"
    gate_expect gate_fails_missing_manifest_copy 1 "FAIL files" "$d"

    d="$(fixture dangling)"; printf 'see [text](licenses/ffmpeg/dav1d.txt)\n' > "$d/THIRD_PARTY_NOTICES.md"
    gate_expect gate_fails_dangling_notice_link 1 "FAIL notice-links" "$d"

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
    if out="$("$ROOT/packaging/check-notice-links.sh" --require-links "$d$p/share/licenses/fthrust/THIRD_PARTY_NOTICES.md" 2>&1)"; then pass install_notice_links_resolve
    else fail install_notice_links_resolve "$out"; fi
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

expect_notes() { # name tag expected-text
    local name="$1" tag="$2" want="$3" got
    if ! got="$("$ROOT/packaging/release-notes.sh" "$tag" --changes "$WORK/CHANGES.md" 2>&1)"; then
        fail "$name" "exit non-zero: $got"; return
    fi
    if [ "$got" = "$want" ]; then pass "$name"; else fail "$name" "got: $got"; fi
}

expect_no_notes() { # name tag message-fragment
    local name="$1" tag="$2" frag="$3" got
    if got="$("$ROOT/packaging/release-notes.sh" "$tag" --changes "$WORK/CHANGES.md" 2>&1)"; then
        fail "$name" "exit 0: $got"; return
    fi
    if grep -q -- "$frag" <<<"$got"; then pass "$name"; else fail "$name" "message: $got"; fi
}

group_notes() {
    cat > "$WORK/CHANGES.md" <<'EOF'
# Changes

## 2.0.0-alpha.10 (2026-12-01)

Ten.

## 2.0.0-alpha.1

One.

### Fixed

- a fix

## [2.0.0] - 2027-01-01

Release.

## v2.1.0

## 2.2.0

Last section.
EOF
    expect_notes notes_section_without_its_neighbours v2.0.0-alpha.1 $'One.\n\n### Fixed\n\n- a fix'
    expect_notes notes_accepts_a_tag_ref refs/tags/v2.0.0-alpha.10 'Ten.'
    expect_notes notes_matches_bracketed_heading 2.0.0 'Release.'
    expect_notes notes_runs_to_end_of_file v2.2.0 'Last section.'
    expect_no_notes notes_empty_section_is_an_error v2.1.0 "no non-empty '## 2.1.0' section"
    expect_no_notes notes_missing_version_is_an_error v9.9.9 "no non-empty '## 9.9.9' section"
    expect_no_notes notes_prefix_of_a_longer_version_is_missing v2.0.0-alpha "no non-empty"
}

archive_cases() {
    local arch="$ROOT/packaging/archive-ffmpeg-source.sh" src="$WORK/srcrepos" m reg out r ffc bsc gc oc tc mc mu rl
    local gitq=(git -c user.name=t -c user.email=t@example.invalid)
    mkdir -p "$src/ffmpeg" "$src/scripts/scripts.d" "$src/gpl" "$src/one" "$src/sub" "$src/sub2" "$src/tag" "$src/mirrored" "$src/multi" "$src/rel"
    echo 'int main(void){return 0;}' > "$src/ffmpeg/main.c"
    echo '#!/bin/sh' > "$src/scripts/scripts.d/50-x.sh"
    echo 'double fft(double x){return x;}' > "$src/gpl/fft.c"
    echo 'int one;' > "$src/one/one.c"
    echo 'int sub;' > "$src/sub/sub.c"
    echo 'int sub2;' > "$src/sub2/sub2.c"
    echo 'int tagged;' > "$src/tag/tagged.c"
    echo 'int mirrored;' > "$src/mirrored/mirrored.c"
    echo 'int multi;' > "$src/multi/multi.c"
    echo 'int rel;' > "$src/rel/rel.c"
    for r in ffmpeg scripts gpl sub sub2 tag mirrored; do
        git -C "$src/$r" init -q
        git -C "$src/$r" config uploadpack.allowAnySHA1InWant true
        git -C "$src/$r" add -A
        "${gitq[@]}" -C "$src/$r" commit -q -m init
    done
    "${gitq[@]}" -C "$src/tag" tag -a v1.2 -m "release 1.2"
    "${gitq[@]}" -C "$src/tag" tag -a libpre-3.1 -m "a tag that starts with the library id"
    # a library with a submodule, which BtbN's scripts fetch with `git submodule update --init --recursive`
    git -C "$src/one" init -q
    git -C "$src/one" config uploadpack.allowAnySHA1InWant true
    "${gitq[@]}" -C "$src/one" -c protocol.file.allow=always submodule add -q "$src/sub" vendor/sub
    git -C "$src/one" add -A
    "${gitq[@]}" -C "$src/one" commit -q -m init
    # a library with two submodules, of which BtbN's script checks out one, none or both
    git -C "$src/multi" init -q
    git -C "$src/multi" config uploadpack.allowAnySHA1InWant true
    "${gitq[@]}" -C "$src/multi" -c protocol.file.allow=always submodule add -q "$src/sub" vendor/a
    "${gitq[@]}" -C "$src/multi" -c protocol.file.allow=always submodule add -q "$src/sub2" vendor/b
    git -C "$src/multi" add -A
    "${gitq[@]}" -C "$src/multi" commit -q -m init
    # a library whose .gitmodules URL is relative (../sub): it resolves against the repository the tree was fetched from
    git -C "$src/rel" init -q
    git -C "$src/rel" config uploadpack.allowAnySHA1InWant true
    printf '[submodule "vendor/sub"]\n\tpath = vendor/sub\n\turl = ../sub\n' > "$src/rel/.gitmodules"
    git -C "$src/rel" add -A
    git -C "$src/rel" update-index --add --cacheinfo "160000,$(git -C "$src/sub" rev-parse HEAD),vendor/sub"
    "${gitq[@]}" -C "$src/rel" commit -q -m init
    ffc="$(git -C "$src/ffmpeg" rev-parse HEAD)"
    bsc="$(git -C "$src/scripts" rev-parse HEAD)"
    gc="$(git -C "$src/gpl" rev-parse HEAD)"
    oc="$(git -C "$src/one" rev-parse HEAD)"
    tc="$(git -C "$src/tag" rev-parse HEAD)"
    mc="$(git -C "$src/mirrored" rev-parse HEAD)"
    mu="$(git -C "$src/multi" rev-parse HEAD)"
    rl="$(git -C "$src/rel" rev-parse HEAD)"
    m="$WORK/source-manifest.json"
    jq --arg a "$src/ffmpeg" --arg b "$ffc" --arg c "$src/scripts" --arg d "$bsc" \
        '.corresponding_source = {ffmpeg: {repo: $a, commit: $b, describe: "n1-test"}, build_scripts: {repo: $c, commit: $d}}' \
        "$ROOT/packaging/ffmpeg_manifest_linux.json" > "$m"
    git clone -q "$src/mirrored" "$src/mirrored-copy"
    git -C "$src/mirrored-copy" config uploadpack.allowAnySHA1InWant true
    # libone: a commit with a submodule, and something BtbN downloads while building; libtag: a tag; libhdr: not compiled in,
    # its repository does not exist; libmirror: the mirror comes first (the repository exists too); libfallback: its mirror is
    # gone, so the repository serves it; libsvn: a Subversion revision; gplx: a GPL library.
    reg="$WORK/source-registry.json"
    jq -n --arg one "$src/one" --arg oc "$oc" --arg tag "$src/tag" --arg tc "$tc" --arg mir "$src/mirrored" --arg mircopy "$src/mirrored-copy" --arg mc "$mc" --arg gpl "$src/gpl" --arg gc "$gc" --arg none "$WORK/no-such-repo" \
        --arg multi "$src/multi" --arg mu "$mu" --arg rel "$src/rel" --arg rl "$rl" \
        'def lib($id; $repo; $pin): {id: $id, name: ($id | ascii_upcase), flags: ["--enable-lib" + $id], license: "MIT", url: ("https://example.org/" + $id), pinned_repo: $repo, pinned_commit: $pin, text: ($id + ".txt")};
         {ignored_options: {}, not_linked: {},
          libraries: [lib("libone"; $one; $oc) + {fetched_at_build: "the crates in its Cargo.lock"}, lib("libtag"; $tag; "v1.2") + {resolved_commit: $tc}, lib("libpre"; $tag; "libpre-3.1") + {resolved_commit: $tc}, lib("libhdr"; $none; "1111111111111111111111111111111111111111") + {compiled_in: false, note: "Headers only."},
                      lib("libmirror"; $mir; $mc) + {mirror: $mircopy}, lib("libfallback"; $mir; $mc) + {mirror: $none}, lib("libsvn"; "https://svn.example.org/trunk/lame"; "r42"),
                      lib("libsome"; $multi; $mu) + {submodules: ["vendor/a"]}, lib("libnone"; $multi; $mu) + {submodules: []}, lib("liball"; $multi; $mu), lib("librel"; $rel; $rl)],
          supporting_libraries: [{id: "gplx", name: "GPLX", license: "GPL-2.0-or-later", url: "https://example.org/gplx", pinned_repo: $gpl, pinned_commit: $gc, text: "gplx.txt", needed_by: ["x"], gpl: true, signatures: ["gplx_sig"], in_libraries: ["libavformat"]}]}' > "$reg"
    out="$WORK/source-out/src.tar.xz"
    local svnlog="$WORK/svn-calls" svn_stub="$WORK/svn-stub"
    printf '#!/bin/sh\necho "$@" >> "%s"\nfor last; do :; done\nmkdir -p "$last"\necho "lame source" > "$last/lame.c"\n' "$svnlog" > "$svn_stub"
    chmod +x "$svn_stub"
    local opts=(--manifest "$m" --libraries "$reg" --skip-depends-check)
    # The other ARCHIVE_* settings (no pauses, three attempts, one second per attempt, a light xz) come from
    # archive_run_cases; the stand-in svn is this case's.
    export ARCHIVE_SVN="$svn_stub"

    if ! r="$("$arch" "${opts[@]}" --out "$out" 2>&1)"; then
        fail archive_builds "$r"; return
    fi
    local listing missing="" f readme
    listing="$(tar -tJf "$out" | sed 's|^\./||')"
    for f in ffmpeg-n1-test/main.c "FFmpeg-Builds-${bsc:0:12}/scripts.d/50-x.sh" "gplx-${gc:0:12}/fft.c" "libone-${oc:0:12}/one.c" "libone-${oc:0:12}/vendor/sub/sub.c" \
             "libtag-v1.2/tagged.c" "libpre-3.1/tagged.c" "libmirror-${mc:0:12}/mirrored.c" "libfallback-${mc:0:12}/mirrored.c" libsvn-r42/lame.c README.txt; do
        grep -qxF "$f" <<<"$listing" || missing="$missing $f"
    done
    if [ -z "$missing" ]; then pass archive_has_the_trees_and_every_library_compiled_in; else fail archive_has_the_trees_and_every_library_compiled_in "missing:$missing"; fi
    # Only the submodules the registry names (libsome: vendor/a; libnone: none; liball: both), as BtbN's scripts check out
    missing=""
    for f in "libsome-${mu:0:12}/vendor/a/sub.c" "libnone-${mu:0:12}/multi.c" "liball-${mu:0:12}/vendor/a/sub.c" "liball-${mu:0:12}/vendor/b/sub2.c"; do
        grep -qxF "$f" <<<"$listing" || missing="$missing $f"
    done
    if grep -q "^libsome-${mu:0:12}/vendor/b/sub2.c" <<<"$listing"; then missing="$missing (libsome has vendor/b)"; fi
    # (the empty directories of the submodule paths are there, as in BtbN's checkout; files in them are not)
    if grep -q "^libnone-${mu:0:12}/vendor/[ab]/." <<<"$listing"; then missing="$missing (libnone has a submodule)"; fi
    if [ -z "$missing" ]; then pass archive_checks_out_only_the_submodules_the_registry_names
    else fail archive_checks_out_only_the_submodules_the_registry_names "$missing"; fi
    # A relative URL in .gitmodules (../sub) needs the origin remote the tree was fetched from
    if grep -qxF "librel-${rl:0:12}/vendor/sub/sub.c" <<<"$listing"; then pass archive_resolves_relative_submodule_urls_against_an_origin
    else fail archive_resolves_relative_submodule_urls_against_an_origin "$(grep librel <<<"$listing" | head -3 | tr '\n' ' ')"; fi
    if grep -E '(^|/)\.git(/|$)' <<<"$listing" >/dev/null; then fail archive_leaves_out_git_data "$(grep -E '(^|/)\.git(/|$)' <<<"$listing" | head -3 | tr '\n' ' ')"; else pass archive_leaves_out_git_data; fi
    if grep -q '^libhdr' <<<"$listing"; then fail archive_leaves_out_libraries_that_are_not_compiled_in "libhdr is in the archive"; else pass archive_leaves_out_libraries_that_are_not_compiled_in; fi
    readme="$(tar -xJOf "$out" ./README.txt)"
    if grep -qF "$ffc" <<<"$readme" && grep -qF -- "--enable-libdav1d" <<<"$readme" && grep -qF "$(jq -r .source.sha256 "$m")" <<<"$readme"; then
        pass archive_readme_names_commits_configuration_and_binary
    else fail archive_readme_names_commits_configuration_and_binary "$readme"; fi
    if grep -qF "commit $gc of $src/gpl" <<<"$readme" && grep -q "compiled into libavformat; GPL code, so its source is part of this archive" <<<"$readme"; then
        pass archive_readme_names_the_gpl_library
    else fail archive_readme_names_the_gpl_library "$readme"; fi
    if grep -qF "tag v1.2 (commit $tc) of $src/tag" <<<"$readme" && grep -qF "commit $oc of $src/one" <<<"$readme" && grep -qF "Subversion revision 42 of https://svn.example.org/trunk/lame" <<<"$readme"; then
        pass archive_readme_names_what_each_library_was_fetched_at
    else fail archive_readme_names_what_each_library_was_fetched_at "$readme"; fi
    if grep -qF "Libraries the build is configured with that contribute no code" <<<"$readme" && grep -qF "LIBHDR (libhdr)" <<<"$readme" && grep -qF "Headers only." <<<"$readme"; then
        pass archive_readme_names_the_libraries_it_leaves_out
    else fail archive_readme_names_the_libraries_it_leaves_out "$readme"; fi
    if grep -qF "submodules: only vendor/a, which is all BtbN's script checks out" <<<"$readme" && grep -qF "submodules: none, BtbN's script checks none out" <<<"$readme"; then
        pass archive_readme_says_which_submodules_were_checked_out
    else fail archive_readme_says_which_submodules_were_checked_out "$readme"; fi
    if grep -qF "download code while they build" <<<"$readme" && grep -qF "the crates in its Cargo.lock" <<<"$readme"; then
        pass archive_readme_names_what_btbns_scripts_download_while_building
    else fail archive_readme_names_what_btbns_scripts_download_while_building "$readme"; fi
    # the mirror comes first, and the repository serves a library whose mirror is gone
    if grep -qF "commit $mc of $src/mirrored, fetched from $src/mirrored-copy" <<<"$readme"; then pass archive_fetches_from_the_mirror_first
    else fail archive_fetches_from_the_mirror_first "$readme"; fi
    if grep -qF "commit $mc of $src/mirrored" <<<"$readme" && ! grep -F "LIBFALLBACK" -A2 <<<"$readme" | grep -q "fetched from"; then pass archive_falls_back_to_the_repository_when_the_mirror_is_gone
    else fail archive_falls_back_to_the_repository_when_the_mirror_is_gone "$readme"; fi
    if ! grep -qF "(LGPL, shared)" <<<"$readme"; then pass archive_readme_does_not_call_the_build_plain_lgpl; else fail archive_readme_does_not_call_the_build_plain_lgpl "$readme"; fi
    if (cd "$WORK/source-out" && sha256sum -c --quiet src.tar.xz.sha256 >/dev/null 2>&1); then pass archive_checksum; else fail archive_checksum "sha256 mismatch"; fi
    if grep -q "export -q --force --non-interactive https://svn.example.org/trunk/lame@42 " "$svnlog"; then pass archive_exports_the_pinned_subversion_revision
    else fail archive_exports_the_pinned_subversion_revision "$(cat "$svnlog" 2>/dev/null)"; fi

    # A Subversion pin needs svn, and says so.
    if r="$(ARCHIVE_SVN="$WORK/no-such-svn" "$arch" "${opts[@]}" --out "$WORK/source-out/nosvn.tar.xz" 2>&1)"; then fail archive_says_when_svn_is_missing "exit 0"
    elif grep -q "libsvn: the registry pins Subversion revision r42, which needs svn" <<<"$r" && [ ! -e "$WORK/source-out/nosvn.tar.xz" ]; then pass archive_says_when_svn_is_missing
    else fail archive_says_when_svn_is_missing "$r"; fi

    jq '.corresponding_source.ffmpeg.commit = "0000000000000000000000000000000000000000"' "$m" > "$WORK/source-bad-commit.json"
    if r="$("$arch" --manifest "$WORK/source-bad-commit.json" --libraries "$reg" --skip-depends-check --out "$WORK/source-out/bad.tar.xz" 2>&1)"; then
        fail archive_unknown_commit_fails "exit 0"
    elif grep -q "could not fetch" <<<"$r" && [ ! -e "$WORK/source-out/bad.tar.xz" ]; then pass archive_unknown_commit_fails
    else fail archive_unknown_commit_fails "$r"; fi

    jq '.libraries[0].pinned_commit = "0000000000000000000000000000000000000000"' "$reg" > "$WORK/source-registry-badlib.json"
    if r="$("$arch" --manifest "$m" --libraries "$WORK/source-registry-badlib.json" --skip-depends-check --out "$WORK/source-out/badlib.tar.xz" 2>&1)"; then
        fail archive_unknown_library_commit_fails "exit 0"
    elif grep -q "libone: could not fetch 0000000000000000000000000000000000000000" <<<"$r" && [ ! -e "$WORK/source-out/badlib.tar.xz" ]; then pass archive_unknown_library_commit_fails
    else fail archive_unknown_library_commit_fails "$r"; fi

    jq '.corresponding_source.ffmpeg.commit = "v8.1"' "$m" > "$WORK/source-tag-pin.json"
    if r="$("$arch" --manifest "$WORK/source-tag-pin.json" --libraries "$reg" --skip-depends-check --out "$WORK/source-out/tag.tar.xz" 2>&1)"; then
        fail archive_rejects_a_movable_ffmpeg_pin "exit 0"
    elif grep -q "40 lowercase hex" <<<"$r"; then pass archive_rejects_a_movable_ffmpeg_pin
    else fail archive_rejects_a_movable_ffmpeg_pin "$r"; fi

    # a library pin is a commit, a tag or a Subversion revision, and nothing git could read as an option
    jq '.libraries[1].pinned_commit = "--upload-pack=touch /tmp/pwned"' "$reg" > "$WORK/source-registry-oddpin.json"
    if r="$("$arch" --manifest "$m" --libraries "$WORK/source-registry-oddpin.json" --skip-depends-check --out "$WORK/source-out/odd.tar.xz" 2>&1)"; then
        fail archive_rejects_a_library_pin_that_is_not_a_commit_tag_or_revision "exit 0"
    elif grep -q "libtag: pin must be a 40-digit commit id, a tag name or a Subversion revision" <<<"$r"; then pass archive_rejects_a_library_pin_that_is_not_a_commit_tag_or_revision
    else fail archive_rejects_a_library_pin_that_is_not_a_commit_tag_or_revision "$r"; fi

    # A tag can be moved: the registry records the commit it names, and a different one (or none) stops the archive
    jq '.libraries[1].resolved_commit = "0123456789012345678901234567890123456789"' "$reg" > "$WORK/source-registry-movedtag.json"
    if r="$("$arch" --manifest "$m" --libraries "$WORK/source-registry-movedtag.json" --skip-depends-check --out "$WORK/source-out/moved.tar.xz" 2>&1)"; then
        fail archive_stops_when_a_tag_has_moved "exit 0"
    elif grep -q "libtag: tag v1.2 names $tc but the registry records 0123456789012345678901234567890123456789: the tag was moved" <<<"$r" && [ ! -e "$WORK/source-out/moved.tar.xz" ]; then pass archive_stops_when_a_tag_has_moved
    else fail archive_stops_when_a_tag_has_moved "$r"; fi
    jq 'del(.libraries[1].resolved_commit)' "$reg" > "$WORK/source-registry-unrecorded.json"
    if r="$("$arch" --manifest "$m" --libraries "$WORK/source-registry-unrecorded.json" --skip-depends-check --out "$WORK/source-out/unrecorded.tar.xz" 2>&1)"; then
        fail archive_needs_the_commit_a_tag_names "exit 0"
    elif grep -q "libtag: the pin v1.2 is a tag, so the registry has to record the commit it names (resolved_commit)" <<<"$r"; then pass archive_needs_the_commit_a_tag_names
    else fail archive_needs_the_commit_a_tag_names "$r"; fi

    # a submodule path the repository does not have fails the archive instead of being skipped
    jq '(.libraries[] | select(.id == "libsome") | .submodules) = ["vendor/zzz"]' "$reg" > "$WORK/source-registry-badsub.json"
    if r="$("$arch" --manifest "$m" --libraries "$WORK/source-registry-badsub.json" --skip-depends-check --out "$WORK/source-out/badsub.tar.xz" 2>&1)"; then
        fail archive_fails_for_a_submodule_path_the_repository_lacks "exit 0"
    elif grep -q "libsome: could not fetch its submodules (vendor/zzz) after 3 attempts" <<<"$r"; then pass archive_fails_for_a_submodule_path_the_repository_lacks
    else fail archive_fails_for_a_submodule_path_the_repository_lacks "$r"; fi

    if r="$(ARCHIVE_ALLOW_LOCAL_REPO= "$arch" --manifest "$m" --libraries "$reg" --skip-depends-check --out "$WORK/source-out/local.tar.xz" 2>&1)"; then
        fail archive_rejects_a_non_https_repo "exit 0"
    elif grep -q "must be https" <<<"$r"; then pass archive_rejects_a_non_https_repo
    else fail archive_rejects_a_non_https_repo "$r"; fi

    jq '(.libraries[] | select(.id == "libmirror") | .mirror) = "http://example.org/mirror.git"' "$reg" > "$WORK/source-registry-badmirror.json"
    if r="$("$arch" --manifest "$m" --libraries "$WORK/source-registry-badmirror.json" --skip-depends-check --out "$WORK/source-out/badmirror.tar.xz" 2>&1)"; then
        fail archive_rejects_a_mirror_that_is_neither_https_nor_git "exit 0"
    elif grep -q "libmirror: repo must be https (or git for a mirror): http://example.org/mirror.git" <<<"$r"; then pass archive_rejects_a_mirror_that_is_neither_https_nor_git
    else fail archive_rejects_a_mirror_that_is_neither_https_nor_git "$r"; fi

    echo '{ not json' > "$WORK/source-registry-broken.json"
    if r="$("$arch" --manifest "$m" --libraries "$WORK/source-registry-broken.json" --skip-depends-check --out "$WORK/source-out/broken.tar.xz" 2>&1)"; then
        fail archive_rejects_a_registry_that_is_not_json "exit 0"
    elif grep -q "is not valid JSON" <<<"$r"; then pass archive_rejects_a_registry_that_is_not_json
    else fail archive_rejects_a_registry_that_is_not_json "$r"; fi
    jq '.libraries |= map(. + {compiled_in: false, note: "x"}) | .supporting_libraries = []' "$reg" > "$WORK/source-registry-none.json"
    if r="$("$arch" --manifest "$m" --libraries "$WORK/source-registry-none.json" --skip-depends-check --out "$WORK/source-out/none.tar.xz" 2>&1)"; then
        fail archive_needs_at_least_one_library "exit 0"
    elif grep -q "lists no library whose code is compiled in" <<<"$r"; then pass archive_needs_at_least_one_library
    else fail archive_needs_at_least_one_library "$r"; fi

    # Without --skip-depends-check the registry is checked against the fetched scripts first; these
    # scripts are nothing like BtbN's, so no archive is made.
    if r="$("$arch" --manifest "$m" --libraries "$reg" --out "$WORK/source-out/nodeps.tar.xz" 2>&1)"; then
        fail archive_checks_the_registry_against_the_scripts "exit 0"
    elif grep -q "does not match BtbN's build scripts" <<<"$r" && [ ! -e "$WORK/source-out/nodeps.tar.xz" ]; then pass archive_checks_the_registry_against_the_scripts
    else fail archive_checks_the_registry_against_the_scripts "$r"; fi

    # Network steps are retried the way BtbN's retry-tool does: here with a git in front of the real one that fails the
    # first fetches, always, or hangs (and logs the credentials it was given), and a timeout and a sleep that log what
    # they were asked for. The count of fetches of a clean run is the baseline.
    local stub="$WORK/archive-git-stub" counter="$WORK/archive-git-fetches" sublog="$WORK/archive-git-submodule" authlog="$WORK/archive-git-auth"
    local tolog="$WORK/archive-timeouts" sleeplog="$WORK/archive-sleeps" real_git real_sleep real_timeout base basic expected
    real_git="$(command -v git)" real_sleep="$(command -v sleep)" real_timeout="$(command -v timeout)"
    mkdir -p "$stub"
    cat > "$stub/git" <<EOF
#!/bin/sh
sub=""; skip=0
for a in "\$@"; do
    if [ \$skip = 1 ]; then skip=0; continue; fi
    case "\$a" in
        -c|-C) skip=1 ;;
        -*) ;;
        *) sub="\$a"; break ;;
    esac
done
if [ "\$sub" = submodule ]; then echo "\$@" >> "$sublog"; fi
if [ "\$sub" = fetch ]; then
    n=\$(cat "$counter" 2>/dev/null || echo 0); n=\$((n + 1)); echo \$n > "$counter"
    echo "\${GIT_CONFIG_KEY_0:-}=\${GIT_CONFIG_VALUE_0:-}" >> "$authlog"
    case "\${GIT_STUB:-}" in
        fail) echo "fatal: simulated network failure" >&2; exit 128 ;;
        fail-twice) if [ \$n -le 2 ]; then echo "fatal: simulated network failure" >&2; exit 128; fi ;;
        hang) exec "$real_sleep" 30 ;;
    esac
fi
exec "$real_git" "\$@"
EOF
    printf '#!/bin/sh\necho "$1" >> "%s"\nexec "%s" "$@"\n' "$tolog" "$real_timeout" > "$stub/timeout"
    printf '#!/bin/sh\necho "$1" >> "%s"\nexit 0\n' "$sleeplog" > "$stub/sleep"
    chmod +x "$stub/git" "$stub/timeout" "$stub/sleep"
    archiving() { # stub-mode [env...]: an archive of the test registry with the stubbed git first on PATH
        local mode="$1"
        shift
        rm -f "$counter"
        env PATH="$stub:$PATH" GIT_STUB="$mode" "$@" "$arch" "${opts[@]}" --out "$WORK/source-out/stub.tar.xz"
    }
    fetches() { cat "$counter" 2>/dev/null || echo 0; }

    rm -f "$sublog" "$authlog"
    if r="$(archiving ok 2>&1)"; then base="$(fetches)"; pass archive_counts_its_fetches_in_a_clean_run; else fail archive_counts_its_fetches_in_a_clean_run "$r"; return; fi
    if [ "$(sort -u "$authlog")" = "=" ]; then pass archive_sends_no_credentials_without_a_token
    else fail archive_sends_no_credentials_without_a_token "$(sort -u "$authlog")"; fi
    # git starts a gc or maintenance run in the background after a fetch or clone; it wrote into .git while the script
    # was removing it, which failed a trial run on libplacebo's submodules.
    if grep -q -- "-c gc.auto=0 -c maintenance.auto=false" "$sublog" && grep -q "submodule update --init --recursive --depth 1" "$sublog"; then pass archive_keeps_gits_background_maintenance_off
    else fail archive_keeps_gits_background_maintenance_off "$(cat "$sublog" 2>/dev/null)"; fi
    if r="$(archiving fail-twice 2>&1)" && [ "$(fetches)" = $((base + 2)) ]; then pass archive_retries_a_failing_fetch
    else fail archive_retries_a_failing_fetch "$r ($(fetches) fetches, expected $((base + 2)))"; fi
    if r="$(archiving fail 2>&1)"; then fail archive_gives_up_after_three_attempts "exit 0"
    elif [ "$(fetches)" = 3 ] && grep -q "ffmpeg: could not fetch $ffc from $src/ffmpeg after 3 attempts" <<<"$r"; then pass archive_gives_up_after_three_attempts
    else fail archive_gives_up_after_three_attempts "$r ($(fetches) fetches)"; fi
    if r="$(archiving hang ARCHIVE_FETCH_TIMEOUT=1 2>&1)"; then fail archive_times_out_a_hanging_fetch "exit 0"
    elif [ "$(fetches)" = 3 ] && grep -q "could not fetch" <<<"$r"; then pass archive_times_out_a_hanging_fetch
    else fail archive_times_out_a_hanging_fetch "$r ($(fetches) fetches)"; fi

    # With none of the retry settings changed the schedule is BtbN's: 10 attempts, timeouts of 120 s growing by 60 s to
    # 660 s, and between attempts a random pause of 5 to 90 s (the stand-in sleep records them and returns at once).
    rm -f "$counter" "$tolog" "$sleeplog"
    r="$(env -u ARCHIVE_ATTEMPTS -u ARCHIVE_FETCH_TIMEOUT -u ARCHIVE_TIMEOUT_STEP -u ARCHIVE_PAUSE_MIN -u ARCHIVE_PAUSE_MAX \
        PATH="$stub:$PATH" GIT_STUB=fail "$arch" "${opts[@]}" --out "$WORK/source-out/stub.tar.xz" 2>&1)"
    if [ "$(fetches)" = 10 ] && [ "$(tr '\n' ' ' < "$tolog")" = "120 180 240 300 360 420 480 540 600 660 " ] && grep -q "after 10 attempts" <<<"$r"; then
        pass archive_retries_with_btbns_attempts_and_timeouts
    else fail archive_retries_with_btbns_attempts_and_timeouts "$r (fetches $(fetches), timeouts $(tr '\n' ' ' < "$tolog"))"; fi
    if [ "$(wc -l < "$sleeplog")" = 9 ] && awk '$1 < 5 || $1 > 90 { bad = 1 } END { exit bad }' "$sleeplog" && [ "$(sort -u "$sleeplog" | wc -l)" -ge 2 ]; then
        pass archive_pauses_a_random_while_between_attempts
    else fail archive_pauses_a_random_while_between_attempts "$(tr '\n' ' ' < "$sleeplog")"; fi

    # The token goes to git as an http extraheader for github.com URLs only (in its environment, not its arguments), the
    # way actions/checkout sets it, and its encoded form is masked in a GitHub Actions log.
    rm -f "$authlog"
    basic="$(printf 'x-access-token:tok3n' | base64 -w0)"
    expected="http.https://github.com/.extraheader=AUTHORIZATION: basic $basic"
    r="$(archiving ok ARCHIVE_GITHUB_TOKEN=tok3n GITHUB_ACTIONS=true 2>&1)"
    if [ -s "$authlog" ] && [ "$(sort -u "$authlog")" = "$expected" ]; then pass archive_authenticates_requests_to_github_with_the_token
    else fail archive_authenticates_requests_to_github_with_the_token "$(sort -u "$authlog")"; fi
    if grep -qxF "::add-mask::$basic" <<<"$r" && ! grep -q tok3n <<<"$r"; then pass archive_masks_the_token_and_never_prints_it
    else fail archive_masks_the_token_and_never_prints_it "$r"; fi

    if r="$(ARCHIVE_ATTEMPTS=many "$arch" "${opts[@]}" --out "$WORK/source-out/nan.tar.xz" 2>&1)"; then fail archive_refuses_a_setting_that_is_not_a_number "exit 0"
    elif grep -q "ARCHIVE_ATTEMPTS must be a whole number, not 'many'" <<<"$r"; then pass archive_refuses_a_setting_that_is_not_a_number
    else fail archive_refuses_a_setting_that_is_not_a_number "$r"; fi
}

# The cases export ARCHIVE_* settings (no pauses, three attempts, one second for each, a light xz). They are taken out
# again however the cases end: an early return used to leave them in the environment of the groups that run after.
archive_run_cases() {
    export ARCHIVE_PAUSE_MIN=0 ARCHIVE_PAUSE_MAX=0 ARCHIVE_ATTEMPTS=3 ARCHIVE_FETCH_TIMEOUT=120 ARCHIVE_TIMEOUT_STEP=0 \
        ARCHIVE_XZ_OPTS="-T1 -1" ARCHIVE_ALLOW_LOCAL_REPO=1
    archive_cases
    local rc=$?
    unset ARCHIVE_PAUSE_MIN ARCHIVE_PAUSE_MAX ARCHIVE_ATTEMPTS ARCHIVE_FETCH_TIMEOUT ARCHIVE_TIMEOUT_STEP ARCHIVE_XZ_OPTS \
        ARCHIVE_ALLOW_LOCAL_REPO ARCHIVE_SVN ARCHIVE_GITHUB_TOKEN
    return $rc
}

group_archive() {
    # An early return, forced by a root without the archive script: the settings must not outlive it.
    local left
    left="$( WORK="$WORK/early"; mkdir -p "$WORK"; ROOT="$WORK/no-such-root"; archive_run_cases >/dev/null 2>&1; env | grep -c '^ARCHIVE_' )"
    if [ "$left" = 0 ]; then pass archive_cases_leave_no_settings_behind_after_an_early_return
    else fail archive_cases_leave_no_settings_behind_after_an_early_return "$left ARCHIVE_* variables left"; fi
    archive_run_cases
}

# The dependency check on small synthetic BtbN scripts: the FFTW case, where a library is pulled in by another
# one's ffbuild_depends and no configure option names it.
group_depends() {
    local chk="$ROOT/packaging/check-ffmpeg-depends.py" d="$WORK/depends" out
    mkdir -p "$d/scripts.d/45-grp"
    cat > "$d/scripts.d/50-foo.sh" <<'EOF'
#!/bin/bash
SCRIPT_REPO="https://example.org/foo.git"
SCRIPT_COMMIT="1111111111111111111111111111111111111111"
ffbuild_depends() {
    echo base
    echo bar
    echo grp
}
ffbuild_configure() {
    echo --enable-libfoo
}
EOF
    cat > "$d/scripts.d/40-bar.sh" <<'EOF'
#!/bin/bash
SCRIPT_REPO="https://example.org/bar.git"
SCRIPT_COMMIT="2222222222222222222222222222222222222222"
ffbuild_depends() {
    echo base
}
EOF
    printf '#!/bin/bash\nffbuild_dummy() { :; }\n' > "$d/scripts.d/15-base.sh"
    cat > "$d/scripts.d/45-grp/50-baz.sh" <<'EOF'
#!/bin/bash
SCRIPT_REPO="https://example.org/baz.git"
SCRIPT_REV="77"
EOF
    jq -n '{configuration: "--enable-version3 --enable-libfoo"}' > "$d/manifest.json"
    registry() { # extra-library-json not-linked-json -> registry file
        jq -n --argjson extra "$1" --argjson nl "$2" \
            '{ignored_options: {"--enable-version3": "x"}, not_linked: $nl,
              libraries: [{id: "foo", name: "foo", flags: ["--enable-libfoo"], license: "MIT", url: "https://example.org/foo", pinned_repo: "https://example.org/foo.git", pinned_commit: "1111111111111111111111111111111111111111", text: "foo.txt"}],
              supporting_libraries: $extra}' > "$d/registry.json"
    }
    local bar='{"id": "bar", "name": "bar", "license": "MIT", "url": "https://example.org/bar", "pinned_repo": "https://example.org/bar.git", "pinned_commit": "2222222222222222222222222222222222222222", "text": "bar.txt", "needed_by": ["foo"]}'
    local baz='{"id": "baz", "name": "baz", "license": "MIT", "url": "https://example.org/baz", "pinned_repo": "https://example.org/baz.git", "pinned_commit": "r77", "text": "baz.txt", "needed_by": ["foo"]}'
    run() { python3 -I "$chk" --manifest "$d/manifest.json" --libraries "$d/registry.json" --scripts "$d" 2>&1; }

    registry "[$bar, $baz]" '{}'
    if out="$(run)"; then pass depends_accepts_a_registry_that_covers_the_closure; else fail depends_accepts_a_registry_that_covers_the_closure "$out"; fi

    registry "[$baz]" '{}'
    if out="$(run)"; then fail depends_finds_a_transitive_library_the_registry_lacks "exit 0"
    elif grep -q "bar (https://example.org/bar) is built for --enable-libfoo" <<<"$out"; then pass depends_finds_a_transitive_library_the_registry_lacks
    else fail depends_finds_a_transitive_library_the_registry_lacks "$out"; fi

    registry "[$baz]" '{"bar": "build time only"}'
    if out="$(run)"; then pass depends_accepts_a_library_recorded_as_not_linked; else fail depends_accepts_a_library_recorded_as_not_linked "$out"; fi

    registry "[$bar]" '{}'
    if out="$(run)"; then fail depends_follows_directory_groups "exit 0"
    elif grep -q "baz (https://example.org/baz) is built for --enable-libfoo > foo > grp" <<<"$out"; then pass depends_follows_directory_groups
    else fail depends_follows_directory_groups "$out"; fi

    registry "[$bar, $baz]" '{"ghost": "never needed"}'
    if out="$(run)"; then fail depends_rejects_a_stale_not_linked_entry "exit 0"
    elif grep -q "not_linked names" <<<"$out"; then pass depends_rejects_a_stale_not_linked_entry
    else fail depends_rejects_a_stale_not_linked_entry "$out"; fi

    registry "[$bar, $baz]" '{}'
    jq '.libraries[0].pinned_commit = "3333333333333333333333333333333333333333"' "$d/registry.json" > "$d/registry-drift.json"
    if out="$(python3 -I "$chk" --manifest "$d/manifest.json" --libraries "$d/registry-drift.json" --scripts "$d" 2>&1)"; then fail depends_catches_a_pin_that_drifted "exit 0"
    elif grep -q "foo: the registry pins 3333" <<<"$out" && grep -q "builds 1111" <<<"$out"; then pass depends_catches_a_pin_that_drifted
    else fail depends_catches_a_pin_that_drifted "$out"; fi

    jq -n '{configuration: "--enable-version3 --enable-libfoo --enable-libnothing --enable-libmissing --enable-libabsent"}' > "$d/manifest-many.json"
    if out="$(python3 -I "$chk" --manifest "$d/manifest-many.json" --libraries "$d/registry.json" --scripts "$d" 2>&1)"; then fail depends_refuses_scripts_that_do_not_look_like_btbns "exit 0"
    elif grep -q "have a provider script" <<<"$out"; then pass depends_refuses_scripts_that_do_not_look_like_btbns
    else fail depends_refuses_scripts_that_do_not_look_like_btbns "$out"; fi

    # A supporting library whose script no enabled option reaches any more: dropping an FFmpeg option has to fail
    # here, in the pull request, and not later at the release gate.
    printf '#!/bin/bash\nSCRIPT_REPO="https://example.org/qux.git"\nSCRIPT_COMMIT="4444444444444444444444444444444444444444"\n' > "$d/scripts.d/50-qux.sh"
    local qux='{"id": "qux", "name": "qux", "license": "MIT", "url": "https://example.org/qux", "pinned_repo": "https://example.org/qux.git", "pinned_commit": "4444444444444444444444444444444444444444", "text": "qux.txt", "needed_by": ["foo"]}'
    registry "[$bar, $baz, $qux]" '{}'
    if out="$(run)"; then fail depends_rejects_a_supporting_library_nothing_needs "exit 0"
    elif grep -q "qux is in supporting_libraries but no enabled option needs it" <<<"$out" && grep -q "licenses/ffmpeg/qux.txt" <<<"$out"; then pass depends_rejects_a_supporting_library_nothing_needs
    else fail depends_rejects_a_supporting_library_nothing_needs "$out"; fi
    registry "[$bar, $baz]" '{}'
    if out="$(run)"; then pass depends_ignores_a_script_nothing_needs_that_the_registry_lacks; else fail depends_ignores_a_script_nothing_needs_that_the_registry_lacks "$out"; fi

    # BtbN's final script depends on whole directories, so a script's siblings are built with it even when it
    # declares no dependency on them (libpng, next to libaribb24).
    mkdir -p "$d/scripts.d/45-pair"
    printf '#!/bin/bash\nSCRIPT_REPO="https://example.org/main.git"\nSCRIPT_COMMIT="5555555555555555555555555555555555555555"\nffbuild_configure() {\n    echo --enable-libmain\n}\n' > "$d/scripts.d/45-pair/50-main.sh"
    printf '#!/bin/bash\nSCRIPT_REPO="https://example.org/sib.git"\nSCRIPT_COMMIT="6666666666666666666666666666666666666666"\n' > "$d/scripts.d/45-pair/25-sib.sh"
    jq -n '{configuration: "--enable-libmain"}' > "$d/manifest-pair.json"
    pair_registry() { # supporting-json
        jq -n --argjson extra "$1" \
            '{ignored_options: {}, not_linked: {},
              libraries: [{id: "main", name: "main", flags: ["--enable-libmain"], license: "MIT", url: "https://example.org/main", pinned_repo: "https://example.org/main.git", pinned_commit: "5555555555555555555555555555555555555555", text: "main.txt"}],
              supporting_libraries: $extra}' > "$d/registry-pair.json"
    }
    local sib='{"id": "sib", "name": "sib", "license": "MIT", "url": "https://example.org/sib", "pinned_repo": "https://example.org/sib.git", "pinned_commit": "6666666666666666666666666666666666666666", "text": "sib.txt", "needed_by": ["main"]}'
    pair_registry "[$sib]"
    if out="$(python3 -I "$chk" --manifest "$d/manifest-pair.json" --libraries "$d/registry-pair.json" --scripts "$d" 2>&1)"; then pass depends_counts_the_siblings_of_a_group_member
    else fail depends_counts_the_siblings_of_a_group_member "$out"; fi
    pair_registry "[]"
    if out="$(python3 -I "$chk" --manifest "$d/manifest-pair.json" --libraries "$d/registry-pair.json" --scripts "$d" 2>&1)"; then fail depends_requires_the_siblings_of_a_group_member "exit 0"
    elif grep -q "sib (https://example.org/sib) is built for --enable-libmain > pair" <<<"$out"; then pass depends_requires_the_siblings_of_a_group_member
    else fail depends_requires_the_siblings_of_a_group_member "$out"; fi

    # Every repository a script builds counts, not only SCRIPT_REPO (libiconv clones gnulib, opencl builds the ICD loader).
    printf '#!/bin/bash\nSCRIPT_REPO="https://example.org/twin.git"\nSCRIPT_COMMIT="7777777777777777777777777777777777777777"\nSCRIPT_REPO2="https://example.org/twin-loader.git"\nSCRIPT_COMMIT2="8888888888888888888888888888888888888888"\nffbuild_configure() {\n    echo --enable-libtwin\n}\n' > "$d/scripts.d/50-twin.sh"
    jq -n '{configuration: "--enable-libtwin"}' > "$d/manifest-twin.json"
    twin_registry() { # supporting-json not-linked-json
        jq -n --argjson extra "$1" --argjson nl "$2" \
            '{ignored_options: {}, not_linked: $nl,
              libraries: [{id: "twin", name: "twin", flags: ["--enable-libtwin"], license: "MIT", url: "https://example.org/twin", pinned_repo: "https://example.org/twin.git", pinned_commit: "7777777777777777777777777777777777777777", text: "twin.txt"}],
              supporting_libraries: $extra}' > "$d/registry-twin.json"
    }
    local loader='{"id": "twin-loader", "name": "twin loader", "license": "MIT", "url": "https://example.org/twin-loader", "pinned_repo": "https://example.org/twin-loader.git", "pinned_commit": "8888888888888888888888888888888888888888", "text": "twin-loader.txt", "needed_by": ["twin"]}'
    twin_run() { python3 -I "$chk" --manifest "$d/manifest-twin.json" --libraries "$d/registry-twin.json" --scripts "$d" 2>&1; }
    twin_registry "[]" '{}'
    if out="$(twin_run)"; then fail depends_checks_a_scripts_second_repository "exit 0"
    elif grep -q "twin (https://example.org/twin-loader) is built for --enable-libtwin" <<<"$out"; then pass depends_checks_a_scripts_second_repository
    else fail depends_checks_a_scripts_second_repository "$out"; fi
    twin_registry "[$loader]" '{}'
    if out="$(twin_run)"; then pass depends_accepts_a_second_repository_in_the_registry; else fail depends_accepts_a_second_repository_in_the_registry "$out"; fi
    twin_registry "[]" '{"https://example.org/twin-loader.git": "build time only"}'
    if out="$(twin_run)"; then pass depends_accepts_a_second_repository_recorded_as_not_linked; else fail depends_accepts_a_second_repository_recorded_as_not_linked "$out"; fi

    # Without --scripts the check fetches BtbN's scripts at the commit the manifest pins. The fetch is retried and
    # has a timeout; here against a local repository, with a git in front of the real one that fails or hangs.
    local repo="$WORK/btbn-repo" sha stub="$WORK/git-stub" counter="$WORK/git-fetches" real_git
    real_git="$(command -v git)"
    mkdir -p "$repo" "$stub"
    cp -r "$d/scripts.d" "$repo/"
    git -C "$repo" init -q
    git -C "$repo" config uploadpack.allowAnySHA1InWant true
    git -C "$repo" add -A
    git -C "$repo" -c user.name=t -c user.email=t@example.invalid commit -q -m scripts
    sha="$(git -C "$repo" rev-parse HEAD)"
    cat > "$stub/git" <<EOF
#!/bin/sh
if [ "\$1" = fetch ]; then
    n=\$(cat "$counter" 2>/dev/null || echo 0); n=\$((n + 1)); echo \$n > "$counter"
    case "\${GIT_STUB:-}" in
        fail) echo "fatal: simulated network failure" >&2; exit 128 ;;
        fail-twice) if [ \$n -le 2 ]; then echo "fatal: simulated network failure" >&2; exit 128; fi ;;
        hang) exec sleep 30 ;;
    esac
fi
exec "$real_git" "\$@"
EOF
    chmod +x "$stub/git"
    jq -n --arg r "$repo" --arg c "$sha" \
        '{configuration: "--enable-version3 --enable-libfoo", corresponding_source: {build_scripts: {repo: $r, commit: $c}}}' > "$d/manifest-fetch.json"
    registry "[$bar, $baz]" '{}'
    fetching() { # stub-mode [env...]: the check without --scripts, with the stubbed git first on PATH
        local mode="$1"
        shift
        rm -f "$counter"
        env PATH="$stub:$PATH" GIT_STUB="$mode" FFMPEG_DEPENDS_FETCH_DELAY=0 "$@" \
            python3 -I "$chk" --manifest "$d/manifest-fetch.json" --libraries "$d/registry.json" 2>&1
    }
    fetches() { cat "$counter" 2>/dev/null || echo 0; }

    if out="$(fetching ok)" && [ "$(fetches)" = 1 ]; then pass depends_fetches_the_scripts_at_the_pinned_commit
    else fail depends_fetches_the_scripts_at_the_pinned_commit "$out ($(fetches) fetches)"; fi
    if out="$(fetching fail-twice)" && [ "$(fetches)" = 3 ]; then pass depends_retries_a_failing_fetch
    else fail depends_retries_a_failing_fetch "$out ($(fetches) fetches)"; fi
    if out="$(fetching fail)"; then fail depends_gives_up_after_three_attempts "exit 0"
    elif [ "$(fetches)" = 3 ] && grep -q "after 3 attempts" <<<"$out" && grep -q "simulated network failure" <<<"$out"; then pass depends_gives_up_after_three_attempts
    else fail depends_gives_up_after_three_attempts "$out ($(fetches) fetches)"; fi
    if out="$(fetching hang FFMPEG_DEPENDS_FETCH_TIMEOUT=1)"; then fail depends_times_out_a_hanging_fetch "exit 0"
    elif [ "$(fetches)" = 3 ] && grep -q "timed out after 1 s" <<<"$out"; then pass depends_times_out_a_hanging_fetch
    else fail depends_times_out_a_hanging_fetch "$out ($(fetches) fetches)"; fi
    jq '.corresponding_source.build_scripts.commit = "main"' "$d/manifest-fetch.json" > "$d/manifest-fetch-tag.json"
    if out="$(python3 -I "$chk" --manifest "$d/manifest-fetch-tag.json" --libraries "$d/registry.json" 2>&1)"; then fail depends_refuses_a_build_scripts_pin_that_is_not_a_commit "exit 0"
    elif grep -q "40 lowercase hex" <<<"$out"; then pass depends_refuses_a_build_scripts_pin_that_is_not_a_commit
    else fail depends_refuses_a_build_scripts_pin_that_is_not_a_commit "$out"; fi

    # The source archive fetches the submodules the registry names, so where a script checks any out (libjxl: only
    # third_party/highway) the registry has to say the same; a script that checks none out leaves the registry free.
    printf '#!/bin/bash\nSCRIPT_REPO="https://example.org/subs.git"\nSCRIPT_COMMIT="9999999999999999999999999999999999999999"\nffbuild_dockerdl() {\n    default_dl .\n    echo "git submodule update --init --recursive --depth 1 --recommend-shallow third_party/one third_party/two"\n}\nffbuild_configure() {\n    echo --enable-libsubs\n}\n' > "$d/scripts.d/50-subs.sh"
    printf '#!/bin/bash\nSCRIPT_REPO="https://example.org/allsubs.git"\nSCRIPT_COMMIT="8888888888888888888888888888888888888888"\nffbuild_dockerdl() {\n    default_dl .\n    echo "git submodule update --init --recursive --depth=1"\n}\nffbuild_configure() {\n    echo --enable-liballsubs\n}\n' > "$d/scripts.d/50-allsubs.sh"
    jq -n '{configuration: "--enable-libsubs --enable-liballsubs"}' > "$d/manifest-subs.json"
    subs_registry() { # submodules-json-for-subs (or "absent") submodules-json-for-allsubs (or "absent") extra-for-foo
        jq -n --arg a "$1" --arg b "$2" \
            '{ignored_options: {}, not_linked: {}, supporting_libraries: [],
              libraries: [
                ({id: "subs", name: "subs", flags: ["--enable-libsubs"], license: "MIT", url: "https://example.org/subs", pinned_repo: "https://example.org/subs.git", pinned_commit: "9999999999999999999999999999999999999999", text: "subs.txt"} + (if $a == "absent" then {} else {submodules: ($a | fromjson)} end)),
                ({id: "allsubs", name: "allsubs", flags: ["--enable-liballsubs"], license: "MIT", url: "https://example.org/allsubs", pinned_repo: "https://example.org/allsubs.git", pinned_commit: "8888888888888888888888888888888888888888", text: "allsubs.txt"} + (if $b == "absent" then {} else {submodules: ($b | fromjson)} end))]}' > "$d/registry-subs.json"
    }
    subs_run() { python3 -I "$chk" --manifest "$d/manifest-subs.json" --libraries "$d/registry-subs.json" --scripts "$d" 2>&1; }
    subs_registry '["third_party/one","third_party/two"]' absent
    if out="$(subs_run)"; then pass depends_accepts_the_submodules_a_script_checks_out; else fail depends_accepts_the_submodules_a_script_checks_out "$out"; fi
    subs_registry absent absent
    if out="$(subs_run)"; then fail depends_requires_the_submodule_list_where_a_script_names_paths "exit 0"
    elif grep -q "subs: BtbN's script checks out third_party/one, third_party/two but the registry's submodules says all" <<<"$out"; then pass depends_requires_the_submodule_list_where_a_script_names_paths
    else fail depends_requires_the_submodule_list_where_a_script_names_paths "$out"; fi
    subs_registry '["third_party/one"]' absent
    if out="$(subs_run)"; then fail depends_rejects_a_submodule_list_that_differs "exit 0"
    elif grep -q "subs: BtbN's script checks out third_party/one, third_party/two but the registry's submodules says third_party/one:" <<<"$out"; then pass depends_rejects_a_submodule_list_that_differs
    else fail depends_rejects_a_submodule_list_that_differs "$out"; fi
    subs_registry '["third_party/one","third_party/two"]' '["x"]'
    if out="$(subs_run)"; then fail depends_rejects_a_list_where_a_script_checks_out_all_submodules "exit 0"
    elif grep -q "allsubs: BtbN's script checks out all submodules but the registry's submodules says x:" <<<"$out"; then pass depends_rejects_a_list_where_a_script_checks_out_all_submodules
    else fail depends_rejects_a_list_where_a_script_checks_out_all_submodules "$out"; fi
    # a script that checks none out says nothing about the registry (foo's script has no submodule line)
    registry "[$bar, $baz]" '{}'
    jq '(.libraries[0]) += {submodules: []}' "$d/registry.json" > "$d/registry-nosubs.json"
    if out="$(python3 -I "$chk" --manifest "$d/manifest.json" --libraries "$d/registry-nosubs.json" --scripts "$d" 2>&1)"; then pass depends_leaves_the_registry_free_where_a_script_checks_out_no_submodule
    else fail depends_leaves_the_registry_free_where_a_script_checks_out_no_submodule "$out"; fi
}

group_links() {
    local d="$WORK/links" out
    chk() { "$ROOT/packaging/check-notice-links.sh" --require-links "$@"; }
    if out="$(chk "$ROOT/THIRD_PARTY_NOTICES.md" 2>&1)"; then pass links_repo_notices_resolve; else fail links_repo_notices_resolve "$out"; fi

    # The tarball used to put the notices at its top with licenses/ only under share/.
    mkdir -p "$d/top"; cp "$ROOT/THIRD_PARTY_NOTICES.md" "$ROOT/LICENSE" "$d/top/"
    if out="$(chk "$d/top/THIRD_PARTY_NOTICES.md" 2>&1)"; then fail links_dangle_without_licenses_dir "exit 0"
    elif grep -q "licenses/" <<<"$out"; then pass links_dangle_without_licenses_dir
    else fail links_dangle_without_licenses_dir "$out"; fi
    cp -r "$ROOT/licenses" "$d/top/licenses"
    if out="$(chk "$d/top/THIRD_PARTY_NOTICES.md" 2>&1)"; then fail links_still_dangle_without_manifest "exit 0"
    elif grep -q "packaging/ffmpeg_manifest_linux.json" <<<"$out"; then pass links_still_dangle_without_manifest
    else fail links_still_dangle_without_manifest "$out"; fi
    mkdir -p "$d/top/packaging"; cp "$ROOT/packaging/ffmpeg_manifest_linux.json" "$d/top/packaging/"
    if out="$(chk "$d/top/THIRD_PARTY_NOTICES.md" 2>&1)"; then pass links_resolve_with_licenses_and_manifest; else fail links_resolve_with_licenses_and_manifest "$out"; fi

    mkdir -p "$d/misc"
    printf '[web](https://example.org/x) [mail](mailto:a@b.c) [anchor](#top) [file](here.txt#part)\n## Rust crates\n[nope](missing.txt)\n' > "$d/misc/THIRD_PARTY_NOTICES.md"
    touch "$d/misc/here.txt"
    if out="$(chk "$d/misc/THIRD_PARTY_NOTICES.md" 2>&1)"; then pass links_ignore_urls_anchors_and_licence_text; else fail links_ignore_urls_anchors_and_licence_text "$out"; fi
    printf 'no links here\n' > "$d/misc/THIRD_PARTY_NOTICES.md"
    if out="$("$ROOT/packaging/check-notice-links.sh" "$d/misc/THIRD_PARTY_NOTICES.md" 2>&1)"; then pass links_none_is_fine_by_default; else fail links_none_is_fine_by_default "$out"; fi
    if out="$(chk "$d/misc/THIRD_PARTY_NOTICES.md" 2>&1)"; then fail links_none_fails_when_required "exit 0"
    elif grep -q "no relative links" <<<"$out"; then pass links_none_fails_when_required
    else fail links_none_fails_when_required "$out"; fi
    printf '[a](one.txt) [b](two.txt)\n' > "$d/misc/THIRD_PARTY_NOTICES.md"
    out="$(chk "$d/misc/THIRD_PARTY_NOTICES.md" 2>&1)" && { fail links_report_every_dangling_target "exit 0"; return; }
    if grep -q "one.txt" <<<"$out" && grep -q "two.txt" <<<"$out"; then pass links_report_every_dangling_target; else fail links_report_every_dangling_target "$out"; fi
}

group_ffnotice() {
    local gen="$ROOT/packaging/gen-ffmpeg-notice.py" m="$ROOT/packaging/ffmpeg_manifest_linux.json" out rows want
    if ! out="$(python3 -I "$gen" 2>&1)"; then fail ffnotice_generates "$out"; return; fi
    rows="$(grep -c '^| `--enable-' <<<"$out")"
    want="$(jq -r '.configuration' "$m" | tr ' ' '\n' | grep -c '^--enable-')"
    # every enabled option is a table row, bar the three that add no code and
    # the second option of the one library that has two (OpenCORE AMR)
    if [ "$rows" -ge $((want - 4)) ] && [ "$rows" -le "$want" ]; then pass ffnotice_lists_every_enabled_library
    else fail ffnotice_lists_every_enabled_library "$rows rows for $want enabled options"; fi

    jq '.configuration += " --enable-libnewthing"' "$m" > "$WORK/ff-new.json"
    if out="$(python3 -I "$gen" --manifest "$WORK/ff-new.json" 2>&1)"; then fail ffnotice_rejects_an_unlisted_library "exit 0"
    elif grep -q -- "--enable-libnewthing" <<<"$out"; then pass ffnotice_rejects_an_unlisted_library
    else fail ffnotice_rejects_an_unlisted_library "$out"; fi

    jq '.configuration |= sub(" --enable-libdav1d"; "")' "$m" > "$WORK/ff-gone.json"
    if out="$(python3 -I "$gen" --manifest "$WORK/ff-gone.json" 2>&1)"; then fail ffnotice_rejects_a_stale_entry "exit 0"
    elif grep -q -- "no longer enables" <<<"$out" && grep -q -- "--enable-libdav1d" <<<"$out"; then pass ffnotice_rejects_a_stale_entry
    else fail ffnotice_rejects_a_stale_entry "$out"; fi

    mkdir -p "$WORK/ff-lic"; cp "$ROOT"/licenses/ffmpeg/*.txt "$WORK/ff-lic/"; rm "$WORK/ff-lic/dav1d.txt"
    if out="$(python3 -I "$gen" --licenses "$WORK/ff-lic" 2>&1)"; then fail ffnotice_rejects_a_missing_licence_text "exit 0"
    elif grep -q "licence text missing for dav1d" <<<"$out"; then pass ffnotice_rejects_a_missing_licence_text
    else fail ffnotice_rejects_a_missing_licence_text "$out"; fi

    cp "$ROOT"/licenses/ffmpeg/*.txt "$WORK/ff-lic/"; echo stray text that belongs to nothing at all, long enough to count as a licence text file for this check > "$WORK/ff-lic/stray.txt"
    if out="$(python3 -I "$gen" --licenses "$WORK/ff-lic" 2>&1)"; then fail ffnotice_rejects_an_orphan_text "exit 0"
    elif grep -q "stray.txt belongs to no library" <<<"$out"; then pass ffnotice_rejects_an_orphan_text
    else fail ffnotice_rejects_an_orphan_text "$out"; fi

    out="$(python3 -I "$gen" 2>&1)"
    # only a 40-digit commit id is shortened; a tag or a Subversion revision is shown whole
    if grep -qF '`openssl-3.6.3`' <<<"$out" && grep -qF '`r6531`' <<<"$out" && grep -qF '`c150ba6c9b9b`' <<<"$out" && ! grep -qF 'openssl-3.6.`' <<<"$out"; then
        pass ffnotice_abbreviates_only_commit_ids
    else fail ffnotice_abbreviates_only_commit_ids "$(grep -o 'openssl-3[^|]*' <<<"$out" | head -2)"; fi
    if grep -q '\*\*GPL-2.0-or-later\*\*' <<<"$out" && grep -q 'GPL code in the shipped libraries' <<<"$out" && grep -qF 'fftw3.txt' <<<"$out"; then pass ffnotice_marks_the_gpl_library
    else fail ffnotice_marks_the_gpl_library "no GPL section"; fi

    jq '(.supporting_libraries[] | select(.id == "fftw3")) |= del(.signatures)' "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-nosig.json"
    if out="$(python3 -I "$gen" --libraries "$WORK/reg-nosig.json" 2>&1)"; then fail ffnotice_gpl_library_needs_signatures "exit 0"
    elif grep -q "fftw3 is marked gpl but has no signatures" <<<"$out"; then pass ffnotice_gpl_library_needs_signatures
    else fail ffnotice_gpl_library_needs_signatures "$out"; fi
    local bad
    for bad in '.signatures = []' '.signatures = [""]' '.signatures = ["two words"]' '.signatures = "dft-nop"' '.in_libraries = []'; do
        jq "(.supporting_libraries[] | select(.id == \"fftw3\")) |= ($bad)" "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-badsig.json"
        if out="$(python3 -I "$gen" --libraries "$WORK/reg-badsig.json" 2>&1)"; then fail "ffnotice_rejects_unusable_gpl_evidence ($bad)" "exit 0"
        elif grep -q "fftw3 is marked gpl but has no signatures and in_libraries" <<<"$out"; then pass "ffnotice_rejects_unusable_gpl_evidence ($bad)"
        else fail "ffnotice_rejects_unusable_gpl_evidence ($bad)" "$out"; fi
    done

    # A GPL entry (or any supporting library) without needed_by used to stop the generator with a KeyError.
    for bad in '(.supporting_libraries[] | select(.id == "fftw3")) |= del(.needed_by)' '(.supporting_libraries[] | select(.id == "libogg")) |= del(.needed_by)' '(.supporting_libraries[] | select(.id == "libogg") | .needed_by) = []'; do
        jq "$bad" "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-noneeded.json"
        if out="$(python3 -I "$gen" --libraries "$WORK/reg-noneeded.json" 2>&1)"; then fail "ffnotice_supporting_libraries_say_what_they_are_for ($bad)" "exit 0"
        elif grep -q "has no needed_by" <<<"$out" && ! grep -q Traceback <<<"$out"; then pass "ffnotice_supporting_libraries_say_what_they_are_for ($bad)"
        else fail "ffnotice_supporting_libraries_say_what_they_are_for ($bad)" "$out"; fi
    done

    # compiled_in and mirror steer the source archive, so the generator checks their shape.
    registry_rejects() { # name jq-filter expected-fragment
        jq "$2" "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-reject.json"
        if out="$(python3 -I "$gen" --libraries "$WORK/reg-reject.json" 2>&1)"; then fail "$1" "exit 0"
        elif grep -q -- "$3" <<<"$out" && ! grep -q Traceback <<<"$out"; then pass "$1"
        else fail "$1" "$out"; fi
    }
    registry_rejects ffnotice_compiled_in_false_needs_a_note '(.libraries[] | select(.id == "amf")) |= del(.note)' "amf is marked compiled_in false but has no note"
    registry_rejects ffnotice_compiled_in_must_be_a_boolean '(.libraries[] | select(.id == "amf") | .compiled_in) = "no"' "amf: compiled_in must be true or false"
    registry_rejects ffnotice_a_gpl_library_must_be_compiled_in '(.supporting_libraries[] | select(.id == "fftw3")) |= (. + {compiled_in: false, note: "x"})' "fftw3 is marked gpl but not compiled in"
    registry_rejects ffnotice_mirror_must_be_https_or_git '(.libraries[] | select(.id == "iconv") | .mirror) = "http://example.org/x.git"' "iconv: mirror must be an https:// or git:// URL"
    registry_rejects ffnotice_fetched_at_build_must_say_what '(.libraries[] | select(.id == "shaderc") | .fetched_at_build) = " "' "shaderc: fetched_at_build must say what BtbN's script downloads while building"
    registry_rejects ffnotice_submodules_are_a_list_of_relative_paths '(.libraries[] | select(.id == "libjxl") | .submodules) = ["../outside"]' "libjxl: submodules must be a list of relative paths"
    registry_rejects ffnotice_submodules_are_not_a_string '(.libraries[] | select(.id == "libjxl") | .submodules) = "third_party/highway"' "libjxl: submodules must be a list of relative paths"
    registry_rejects ffnotice_a_tag_pin_needs_the_commit_it_names 'del(.libraries[] | select(.id == "openssl") | .resolved_commit)' "openssl: pinned_commit openssl-3.6.3 is a tag, so resolved_commit has to be the 40-digit commit it names"
    registry_rejects ffnotice_the_commit_a_tag_names_is_a_full_commit_id '(.libraries[] | select(.id == "openssl") | .resolved_commit) = "aae016bfd52f"' "openssl: pinned_commit openssl-3.6.3 is a tag, so resolved_commit has to be the 40-digit commit it names"
    registry_rejects ffnotice_resolved_commit_is_for_tags_only '(.libraries[] | select(.id == "zlib")) += {resolved_commit: "aae016bfd52fcad2bc9657c2c782cfdf73b1ed5f"}' "zlib: resolved_commit is for tag pins"
    # The notices name what the archive does not hold (shaderc's and rav1e's downloads), taken from the registry
    out="$(python3 -I "$gen" --template "$ROOT/packaging/notices/header.md" 2>&1)"
    if grep -q "It does not hold what BtbN's scripts download while building: for rav1e, the Rust crates" <<<"$out" && grep -q "for shaderc, its third-party projects" <<<"$out"; then
        pass ffnotice_names_what_the_archive_does_not_hold
    else fail ffnotice_names_what_the_archive_does_not_hold "$(grep -c 'It does not hold' <<<"$out")"; fi
    jq 'del(.libraries[] | select(.id == "shaderc") | .fetched_at_build) | del(.libraries[] | select(.id == "rav1e") | .fetched_at_build)' "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-nodownloads.json"
    out="$(python3 -I "$gen" --libraries "$WORK/reg-nodownloads.json" --template "$ROOT/packaging/notices/header.md" 2>&1)"
    if ! grep -q "It does not hold" <<<"$out"; then pass ffnotice_says_nothing_about_downloads_when_there_are_none
    else fail ffnotice_says_nothing_about_downloads_when_there_are_none "$out"; fi
    out="$(jq -r '[(.libraries[], .supporting_libraries[]) | select(has("fetched_at_build")) | .id] | sort | join(" ")' "$ROOT/packaging/ffmpeg_libraries.json")"
    if [ "$out" = "rav1e shaderc" ]; then pass ffnotice_registry_names_what_btbns_scripts_download_while_building
    else fail ffnotice_registry_names_what_btbns_scripts_download_while_building "$out"; fi
    # The libraries the source archive leaves out are the ones whose code the shipped libraries do not contain
    # (headers, loaded from the system at run time, SDL which the build only enables). A new entry here is a decision
    # to review, so it has to be made in this list too.
    out="$(jq -r '[(.libraries[], .supporting_libraries[]) | select(.compiled_in == false) | .id] | sort | join(" ")' "$ROOT/packaging/ffmpeg_libraries.json")"
    if [ "$out" = "amf libdrm libva libx11 libxcb opencl-headers sdl2 vulkan-headers" ]; then pass ffnotice_registry_leaves_out_only_what_is_not_compiled_in
    else fail ffnotice_registry_leaves_out_only_what_is_not_compiled_in "$out"; fi

    printf '{{ffmpeg.licence_summary}}|{{ffmpeg.licence_row}}|{{ffmpeg.configuration_sentence}}|{{ffmpeg.gpl_consequence}}\n<!-- ffmpeg-libraries -->\n' > "$WORK/tpl-licence.md"
    out="$(python3 -I "$gen" --template "$WORK/tpl-licence.md" 2>&1 | head -1)"
    if [ "$(grep -o 'FFTW' <<<"$out" | wc -l)" -ge 3 ] && grep -q 'libavformat' <<<"$out"; then pass ffnotice_licence_wording_names_the_gpl_library
    else fail ffnotice_licence_wording_names_the_gpl_library "$out"; fi
    jq '(.supporting_libraries[] | select(.id == "fftw3")) |= (del(.gpl) | del(.signatures) | del(.in_libraries))' "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-nogpl.json"
    out="$(python3 -I "$gen" --libraries "$WORK/reg-nogpl.json" --template "$WORK/tpl-licence.md" 2>&1 | head -1)"
    if grep -q '^LGPL v3 or later|\*\*LGPL v3 or later\*\* (`--enable-version3`' <<<"$out" && ! grep -q 'GPL code' <<<"$out" && grep -q 'The configuration has no GPL or non-free library.' <<<"$out"; then pass ffnotice_without_a_gpl_library_says_lgpl
    else fail ffnotice_without_a_gpl_library_says_lgpl "$out"; fi

    printf 'ffmpeg {{ffmpeg.describe}} sha {{ffmpeg.sha256}}\n<!-- ffmpeg-libraries -->\ntail\n' > "$WORK/tpl.md"
    out="$(python3 -I "$gen" --template "$WORK/tpl.md" 2>&1)"
    if grep -q "ffmpeg $(jq -r .version "$m") sha $(jq -r .source.sha256 "$m")" <<<"$out" && grep -q "^### External libraries built into FFmpeg" <<<"$out" && [ "$(tail -1 <<<"$out")" = tail ]; then
        pass ffnotice_template_fills_placeholders_and_section
    else fail ffnotice_template_fills_placeholders_and_section "$(head -3 <<<"$out")"; fi
    printf '{{ffmpeg.bogus}}\n<!-- ffmpeg-libraries -->\n' > "$WORK/tpl-bad.md"
    if out="$(python3 -I "$gen" --template "$WORK/tpl-bad.md" 2>&1)"; then fail ffnotice_rejects_unknown_placeholders "exit 0"
    elif grep -q "unknown placeholders" <<<"$out"; then pass ffnotice_rejects_unknown_placeholders
    else fail ffnotice_rejects_unknown_placeholders "$out"; fi
    # the committed notices carry the generated section, filled from the manifest
    if grep -qF "$(jq -r .source.sha256 "$m")" "$ROOT/THIRD_PARTY_NOTICES.md" && grep -q '^### External libraries built into FFmpeg' "$ROOT/THIRD_PARTY_NOTICES.md"; then pass ffnotice_committed_notices_have_the_section
    else fail ffnotice_committed_notices_have_the_section "run tools/gen-notices.sh"; fi
}

group_deps() {
    local chk="$ROOT/tools/check-workspace-deps.sh" w="$WORK/ws" out
    if out="$("$chk" 2>&1)"; then pass deps_every_workspace_dependency_is_used; else fail deps_every_workspace_dependency_is_used "$out"; fi
    mkdir -p "$w/crates/a"
    printf '[workspace]\nmembers = ["crates/*"]\n\n[workspace.dependencies]\nserde = "1"\nunused-thing = "2"\nlocal = { version = "1", features = ["x"] }\n\n[profile.dev]\ndebug = 0\n' > "$w/Cargo.toml"
    printf '[package]\nname = "a"\n\n[dependencies]\nserde.workspace = true\nlocal = { workspace = true, features = ["y"] }\n' > "$w/crates/a/Cargo.toml"
    if out="$("$chk" "$w" 2>&1)"; then fail deps_flags_an_unused_entry "exit 0"
    elif grep -q "unused-thing" <<<"$out" && ! grep -qE "serde|local" <<<"$out"; then pass deps_flags_an_unused_entry
    else fail deps_flags_an_unused_entry "$out"; fi
}

# The release profile: fat LTO and one codegen unit (a third off the binaries),
# no panic = "abort" while catch_unwind carries the engine, export, finalize
# and upload paths.
group_profile() {
    local rel
    rel="$(sed -n '/^\[profile.release\]/,/^\[/p' "$ROOT/Cargo.toml")"
    if grep -qx 'lto = "fat"' <<<"$rel" && grep -qx 'codegen-units = 1' <<<"$rel" && grep -qx 'strip = "debuginfo"' <<<"$rel"; then pass profile_release_is_fat_lto_one_codegen_unit
    else fail profile_release_is_fat_lto_one_codegen_unit "[profile.release] is: $(tr '\n' ' ' <<<"$rel")"; fi
    if grep -rq 'catch_unwind' "$ROOT"/crates/*/src && grep -qE '^panic *= *"abort"' <<<"$rel"; then
        fail profile_release_keeps_unwinding "panic = \"abort\" while catch_unwind is in use"
    else pass profile_release_keeps_unwinding; fi
}

group_desktop() {
    local name title
    name="$(sed -n 's/^Name=//p' "$ROOT/packaging/space.kalopsia.fthrust.desktop")"
    title="$(sed -n 's/.*\.with_title("\(.*\)").*/\1/p' "$ROOT/crates/fthrust-ui/src/main.rs" | head -1)"
    if [ -z "$title" ]; then fail desktop_name_is_the_window_title "no window title found in crates/fthrust-ui/src/main.rs"; return; fi
    # "FTHR Clips" is the original app's name; two launchers with it would be indistinguishable.
    if [ "$name" = "$title" ] && [ "$name" != "FTHR Clips" ]; then pass desktop_name_is_the_window_title
    else fail desktop_name_is_the_window_title "launcher Name=$name, window title $title"; fi
}

# The release workflow's shape, read as text (a real run is the real test): the source archive is a job of its own that
# publish waits for, it sends the job's token to the archive script, and the package lists match the Dockerfile's.
group_workflow() {
    local wf="$ROOT/.github/workflows/release.yml" appimage srcjob publish wf_pkgs df_pkgs
    appimage="$(sed -n '/^  appimage:/,/^  source:/p' "$wf")"
    srcjob="$(sed -n '/^  source:/,/^  publish:/p' "$wf")"
    publish="$(sed -n '/^  publish:/,$p' "$wf")"
    if grep -q 'archive-ffmpeg-source.sh' <<<"$srcjob" && ! grep -q 'archive-ffmpeg-source.sh' <<<"$appimage"; then pass workflow_source_archive_is_a_job_of_its_own
    else fail workflow_source_archive_is_a_job_of_its_own "the archive script is not only in the source job"; fi
    if grep -q 'needs: \[notes, ci\]' <<<"$srcjob" && ! grep -q 'appimage' <<<"$(grep 'needs:' <<<"$srcjob")"; then pass workflow_source_job_does_not_wait_for_the_build
    else fail workflow_source_job_does_not_wait_for_the_build "$(grep 'needs:' <<<"$srcjob")"; fi
    if grep -q 'needs: \[notes, appimage, source\]' <<<"$publish" && grep -q 'fthrust-source-' <<<"$publish" && grep -q 'fthrust-appimage-' <<<"$publish"; then pass workflow_publish_waits_for_the_build_and_the_source
    else fail workflow_publish_waits_for_the_build_and_the_source "$(grep 'needs:' <<<"$publish")"; fi
    if grep -qF 'ARCHIVE_GITHUB_TOKEN: ${{ github.token }}' <<<"$srcjob"; then pass workflow_source_job_sends_the_job_token_to_the_archive_script
    else fail workflow_source_job_sends_the_job_token_to_the_archive_script "no ARCHIVE_GITHUB_TOKEN in the source job"; fi
    # the job's token is handed to the archive script by hand; the checkout has no reason to leave it in .git/config
    if grep -A4 'actions/checkout@' <<<"$srcjob" | grep -qx '          persist-credentials: false'; then pass workflow_source_checkout_keeps_no_credentials
    else fail workflow_source_checkout_keeps_no_credentials "no persist-credentials: false on the source job's checkout"; fi
    if grep -qx '          compression-level: 0' <<<"$srcjob"; then pass workflow_uploads_the_xz_without_compressing_it_again
    else fail workflow_uploads_the_xz_without_compressing_it_again "no compression-level: 0 in the source job"; fi
    if grep -q 'apt-get update && sudo apt-get install -y --no-install-recommends subversion' <<<"$srcjob"; then pass workflow_source_job_updates_apt_before_installing_svn
    else fail workflow_source_job_updates_apt_before_installing_svn "$(grep -n 'apt-get' <<<"$srcjob")"; fi
    if grep -qx '    timeout-minutes: 30' <<<"$publish"; then pass workflow_publish_has_half_an_hour
    else fail workflow_publish_has_half_an_hour "$(grep 'timeout' <<<"$publish")"; fi
    # "Keep in step with packaging/Dockerfile": the same packages, subversion included
    wf_pkgs="$(grep -m1 'apt-get install -y --no-install-recommends' <<<"$appimage" | sed 's/.*--no-install-recommends //' | tr ' ' '\n' | sed '/^$/d' | sort)"
    df_pkgs="$(awk '/^RUN apt-get update/ { on = 1 } on { print } on && !/\\$/ { exit }' "$ROOT/packaging/Dockerfile" | sed -e 's/.*--no-install-recommends//' -e 's/&& rm.*//' -e 's/\\//g' | tr -s ' \n' '\n\n' | sed '/^$/d' | sort)"
    if [ -n "$wf_pkgs" ] && [ "$wf_pkgs" = "$df_pkgs" ] && grep -qx subversion <<<"$wf_pkgs"; then pass workflow_package_list_matches_the_dockerfile
    else fail workflow_package_list_matches_the_dockerfile "workflow: $(tr '\n' ' ' <<<"$wf_pkgs") / Dockerfile: $(tr '\n' ' ' <<<"$df_pkgs")"; fi
}

groups=("$@")
[ ${#groups[@]} -eq 0 ] && groups=(fetch gate version install notes archive depends links ffnotice desktop deps profile workflow)
for g in "${groups[@]}"; do
    if ! declare -F "group_$g" >/dev/null; then fail "group_$g" "no such group"; continue; fi
    "group_$g"
done
[ $failures -eq 0 ] && echo "all ok" || echo "$failures failed"
[ $failures -eq 0 ]
