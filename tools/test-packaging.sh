#!/usr/bin/env bash
# Tests for the packaging scripts. Usage: tools/test-packaging.sh [fetch|gate|version|install|notes|archive|depends|links|ffnotice|desktop|deps|profile]...
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

    d="$(fixture gplgone)"; jq '(.supporting_libraries[] | select(.id == "fftw3") | .signature) = "no_such_string_in_any_library"' "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-gone.json"
    gate_expect gate_fails_when_a_gpl_library_is_not_in_the_libraries 1 "FAIL gpl-libs" "$d" FFMPEG_LIBRARIES_OVERRIDE="$WORK/reg-gone.json"

    d="$(fixture gplextra)"; jq '(.supporting_libraries[] | select(.id == "fftw3") | .signature) = "av_version_info"' "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-extra.json"
    gate_expect gate_fails_when_a_gpl_library_is_also_in_other_libraries 1 "FAIL gpl-libs" "$d" FFMPEG_LIBRARIES_OVERRIDE="$WORK/reg-extra.json"

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

group_archive() {
    local arch="$ROOT/packaging/archive-ffmpeg-source.sh" src="$WORK/srcrepos" m reg out r ffc bsc gc
    mkdir -p "$src/ffmpeg" "$src/scripts/scripts.d" "$src/gpl"
    echo 'int main(void){return 0;}' > "$src/ffmpeg/main.c"
    echo '#!/bin/sh' > "$src/scripts/scripts.d/50-x.sh"
    echo 'double fft(double x){return x;}' > "$src/gpl/fft.c"
    for r in ffmpeg scripts gpl; do
        git -C "$src/$r" init -q
        git -C "$src/$r" config uploadpack.allowAnySHA1InWant true
        git -C "$src/$r" add -A
        git -C "$src/$r" -c user.name=t -c user.email=t@example.invalid commit -q -m init
    done
    ffc="$(git -C "$src/ffmpeg" rev-parse HEAD)"
    bsc="$(git -C "$src/scripts" rev-parse HEAD)"
    gc="$(git -C "$src/gpl" rev-parse HEAD)"
    m="$WORK/source-manifest.json"
    jq --arg a "$src/ffmpeg" --arg b "$ffc" --arg c "$src/scripts" --arg d "$bsc" \
        '.corresponding_source = {ffmpeg: {repo: $a, commit: $b, describe: "n1-test"}, build_scripts: {repo: $c, commit: $d}}' \
        "$ROOT/packaging/ffmpeg_manifest_linux.json" > "$m"
    reg="$WORK/source-registry.json"
    jq -n --arg repo "$src/gpl" --arg c "$gc" \
        '{ignored_options: {}, libraries: [], not_linked: {}, supporting_libraries: [{id: "gplx", name: "GPLX", license: "GPL-2.0-or-later", url: "https://example.org/gplx", pinned_repo: $repo, pinned_commit: $c, text: "gplx.txt", needed_by: ["x"], gpl: true, signature: "gplx_sig", in_libraries: ["libavformat"]}]}' > "$reg"
    out="$WORK/source-out/src.tar.xz"
    local opts=(--manifest "$m" --libraries "$reg" --skip-depends-check)

    if ! r="$(ARCHIVE_ALLOW_LOCAL_REPO=1 "$arch" "${opts[@]}" --out "$out" 2>&1)"; then
        fail archive_builds "$r"; return
    fi
    local listing missing="" f readme
    listing="$(tar -tJf "$out" | sed 's|^\./||')"
    for f in ffmpeg-n1-test/main.c "FFmpeg-Builds-${bsc:0:12}/scripts.d/50-x.sh" "gplx-${gc:0:12}/fft.c" README.txt; do
        grep -qxF "$f" <<<"$listing" || missing="$missing $f"
    done
    if [ -z "$missing" ]; then pass archive_has_the_trees_the_gpl_library_and_readme; else fail archive_has_the_trees_the_gpl_library_and_readme "missing:$missing"; fi
    readme="$(tar -xJOf "$out" ./README.txt)"
    if grep -qF "$ffc" <<<"$readme" && grep -qF -- "--enable-libdav1d" <<<"$readme" && grep -qF "$(jq -r .source.sha256 "$m")" <<<"$readme"; then
        pass archive_readme_names_commits_configuration_and_binary
    else fail archive_readme_names_commits_configuration_and_binary "$readme"; fi
    if grep -qF "GPLX (GPL-2.0-or-later) at commit $gc" <<<"$readme" && grep -q "GPL code, so its source is part of this archive" <<<"$readme"; then
        pass archive_readme_names_the_gpl_library
    else fail archive_readme_names_the_gpl_library "$readme"; fi
    if (cd "$WORK/source-out" && sha256sum -c --quiet src.tar.xz.sha256 >/dev/null 2>&1); then pass archive_checksum; else fail archive_checksum "sha256 mismatch"; fi

    jq '.corresponding_source.ffmpeg.commit = "0000000000000000000000000000000000000000"' "$m" > "$WORK/source-bad-commit.json"
    if r="$(ARCHIVE_ALLOW_LOCAL_REPO=1 "$arch" --manifest "$WORK/source-bad-commit.json" --libraries "$reg" --skip-depends-check --out "$WORK/source-out/bad.tar.xz" 2>&1)"; then
        fail archive_unknown_commit_fails "exit 0"
    elif grep -q "could not fetch" <<<"$r" && [ ! -e "$WORK/source-out/bad.tar.xz" ]; then pass archive_unknown_commit_fails
    else fail archive_unknown_commit_fails "$r"; fi

    jq '.corresponding_source.ffmpeg.commit = "v8.1"' "$m" > "$WORK/source-tag-pin.json"
    if r="$(ARCHIVE_ALLOW_LOCAL_REPO=1 "$arch" --manifest "$WORK/source-tag-pin.json" --libraries "$reg" --skip-depends-check --out "$WORK/source-out/tag.tar.xz" 2>&1)"; then
        fail archive_rejects_a_movable_pin "exit 0"
    elif grep -q "40 lowercase hex" <<<"$r"; then pass archive_rejects_a_movable_pin
    else fail archive_rejects_a_movable_pin "$r"; fi

    # a GPL library has to be pinned to a full commit as well
    jq '.supporting_libraries[0].pinned_commit = "openssl-3.6.3"' "$reg" > "$WORK/source-registry-tag.json"
    if r="$(ARCHIVE_ALLOW_LOCAL_REPO=1 "$arch" --manifest "$m" --libraries "$WORK/source-registry-tag.json" --skip-depends-check --out "$WORK/source-out/gpltag.tar.xz" 2>&1)"; then
        fail archive_rejects_a_gpl_library_pinned_by_tag "exit 0"
    elif grep -q "40 lowercase hex" <<<"$r"; then pass archive_rejects_a_gpl_library_pinned_by_tag
    else fail archive_rejects_a_gpl_library_pinned_by_tag "$r"; fi

    if r="$("$arch" --manifest "$m" --libraries "$reg" --skip-depends-check --out "$WORK/source-out/local.tar.xz" 2>&1)"; then
        fail archive_rejects_a_non_https_repo "exit 0"
    elif grep -q "must be https" <<<"$r"; then pass archive_rejects_a_non_https_repo
    else fail archive_rejects_a_non_https_repo "$r"; fi

    # Without --skip-depends-check the registry is checked against the fetched scripts first; these
    # scripts are nothing like BtbN's, so no archive is made.
    if r="$(ARCHIVE_ALLOW_LOCAL_REPO=1 "$arch" --manifest "$m" --libraries "$reg" --out "$WORK/source-out/nodeps.tar.xz" 2>&1)"; then
        fail archive_checks_the_registry_against_the_scripts "exit 0"
    elif grep -q "does not match BtbN's build scripts" <<<"$r" && [ ! -e "$WORK/source-out/nodeps.tar.xz" ]; then pass archive_checks_the_registry_against_the_scripts
    else fail archive_checks_the_registry_against_the_scripts "$r"; fi
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

    jq '(.supporting_libraries[] | select(.id == "fftw3")) |= del(.signature)' "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-nosig.json"
    if out="$(python3 -I "$gen" --libraries "$WORK/reg-nosig.json" 2>&1)"; then fail ffnotice_gpl_library_needs_a_signature "exit 0"
    elif grep -q "fftw3 is marked gpl but has no signature" <<<"$out"; then pass ffnotice_gpl_library_needs_a_signature
    else fail ffnotice_gpl_library_needs_a_signature "$out"; fi

    printf '{{ffmpeg.licence_summary}}|{{ffmpeg.licence_row}}|{{ffmpeg.configuration_sentence}}|{{ffmpeg.gpl_consequence}}\n<!-- ffmpeg-libraries -->\n' > "$WORK/tpl-licence.md"
    out="$(python3 -I "$gen" --template "$WORK/tpl-licence.md" 2>&1 | head -1)"
    if [ "$(grep -o 'FFTW' <<<"$out" | wc -l)" -ge 3 ] && grep -q 'libavformat' <<<"$out"; then pass ffnotice_licence_wording_names_the_gpl_library
    else fail ffnotice_licence_wording_names_the_gpl_library "$out"; fi
    jq '(.supporting_libraries[] | select(.id == "fftw3")) |= (del(.gpl) | del(.signature) | del(.in_libraries))' "$ROOT/packaging/ffmpeg_libraries.json" > "$WORK/reg-nogpl.json"
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

groups=("$@")
[ ${#groups[@]} -eq 0 ] && groups=(fetch gate version install notes archive depends links ffnotice desktop deps profile)
for g in "${groups[@]}"; do
    if ! declare -F "group_$g" >/dev/null; then fail "group_$g" "no such group"; continue; fi
    "group_$g"
done
[ $failures -eq 0 ] && echo "all ok" || echo "$failures failed"
[ $failures -eq 0 ]
