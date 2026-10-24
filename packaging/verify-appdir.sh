#!/usr/bin/env bash
# Release gate for an AppDir: run it on the tree before packing and on the
# --appimage-extract output after (a raw .AppImage file only exposes the
# runtime's own ELF, which is how the reference gate passed the wrong file).
#
#   packaging/verify-appdir.sh <AppDir>
#
# Prints PASS/FAIL per check and RESULT: PASS|FAIL; exits 0 only on PASS.
# MAX_GLIBC_OVERRIDE replaces the baseline ceiling (tests only).
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FFM="$HERE/ffmpeg_manifest_linux.json"
MAX_GLIBC="${MAX_GLIBC_OVERRIDE:-$(jq -r .max_glibc "$HERE/linux_release_baseline.json")}"
BINARIES=(fthrust fthrust-engine fthrust-uploader)
DIR="${1:-}"
failed=0

pass() { echo "PASS $1"; }
fail() { echo "FAIL $1: $2"; failed=1; }

if [ -z "$DIR" ] || [ ! -d "$DIR" ]; then
    fail input "${DIR:-(none)} is not a directory (extract the AppImage first)"
    echo "RESULT: FAIL"
    exit 1
fi

is_elf() { [ "$(head -c4 "$1" 2>/dev/null | od -An -c | tr -d ' ')" = '177ELF' ]; }
mapfile -t ELVES < <(find "$DIR" -type f | while read -r f; do is_elf "$f" && echo "$f"; done)

# 1. files
missing=()
for f in AppRun space.kalopsia.fthrust.desktop space.kalopsia.fthrust.png .DirIcon \
         LICENSE THIRD_PARTY_NOTICES.md licenses usr/bin/assets/fonts/Oswald-Regular.ttf; do
    [ -e "$DIR/$f" ] || missing+=("$f")
done
for b in "${BINARIES[@]}"; do [ -x "$DIR/usr/bin/$b" ] || missing+=("usr/bin/$b"); done
[ ${#missing[@]} -eq 0 ] && pass files || fail files "missing ${missing[*]}"

# 2. ffmpeg-libs: every bundled FFmpeg file is a pinned, unmodified one.
problems=()
while IFS= read -r -d '' f; do
    name="$(basename "$f")"
    want="$(jq -r --arg n "$name" '.shipped_files_sha256[$n] // empty' "$FFM")"
    if [ -z "$want" ]; then problems+=("unpinned $name"); continue; fi
    [ "$(sha256sum "$f" | cut -d' ' -f1)" = "$want" ] || problems+=("hash $name")
done < <(find "$DIR" -type f \( -name 'libav*' -o -name 'libsw*' -o -name 'libpostproc*' \) -print0)
while read -r file soname; do
    [ -f "$DIR/usr/lib/$file" ] || problems+=("missing $file")
    [ "$(readlink "$DIR/usr/lib/$soname" 2>/dev/null)" = "$file" ] || problems+=("soname link $soname")
done < <(jq -r '.soname_map | to_entries[] | "\(.key) \(.value)"' "$FFM")
[ ${#problems[@]} -eq 0 ] && pass ffmpeg-libs || fail ffmpeg-libs "${problems[*]}"

# 3. forbidden-flags
problems=()
mapfile -t FLAGS < <(jq -r '.forbidden_flags[]' "$FFM")
for f in "${ELVES[@]}"; do
    for flag in "${FLAGS[@]}"; do
        grep -aqF -- "$flag" "$f" && problems+=("${f#"$DIR"/} has $flag")
    done
done
[ ${#problems[@]} -eq 0 ] && pass forbidden-flags || fail forbidden-flags "${problems[*]}"

# 4. linkage: DT_RPATH (transitive, so the libraries' own dependencies also
# resolve from usr/lib) and only pinned FFmpeg sonames; PipeWire stays on the host.
problems=()
mapfile -t SONAMES < <(jq -r '.soname_map[]' "$FFM")
for b in "${BINARIES[@]}"; do
    f="$DIR/usr/bin/$b"
    [ -f "$f" ] || continue
    dyn="$(readelf -d "$f" 2>/dev/null)"
    rpath="$(sed -n 's/.*(RPATH).*\[\(.*\)\]/\1/p' <<<"$dyn")"
    [ "$rpath" = '$ORIGIN/../lib' ] || problems+=("$b RPATH '${rpath}'")
    grep -q '(RUNPATH)' <<<"$dyn" && problems+=("$b has RUNPATH")
    while read -r need; do
        [[ "$need" =~ ^lib(av|sw|postproc) ]] || continue
        printf '%s\n' "${SONAMES[@]}" | grep -qxF "$need" || problems+=("$b needs $need")
    done < <(sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' <<<"$dyn")
done
pw="$(find "$DIR" -name 'libpipewire*' | head -1)"
[ -z "$pw" ] || problems+=("bundled ${pw#"$DIR"/}")
[ ${#problems[@]} -eq 0 ] && pass linkage || fail linkage "${problems[*]}"

# 5. glibc ceiling
if [ ${#ELVES[@]} -eq 0 ]; then
    fail glibc "no ELF files found in $DIR"
else
    worst=0 worst_file=""
    for f in "${ELVES[@]}"; do
        v="$(readelf --version-info --wide "$f" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)"
        [ -n "$v" ] || continue
        if [ "$(printf '%s\n%s\n' "$worst" "$v" | sort -V | tail -1)" = "$v" ] && [ "$v" != "$worst" ]; then
            worst="$v" worst_file="${f#"$DIR"/}"
        fi
    done
    if [ "$(printf '%s\n%s\n' "$worst" "$MAX_GLIBC" | sort -V | tail -1)" = "$MAX_GLIBC" ]; then
        pass "glibc (max GLIBC_$worst <= $MAX_GLIBC)"
    else
        fail glibc "$worst_file needs GLIBC_$worst, ceiling $MAX_GLIBC"
    fi
fi

[ $failed -eq 0 ] && { echo "RESULT: PASS"; exit 0; }
echo "RESULT: FAIL"
exit 1
