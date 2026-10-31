#!/usr/bin/env bash
# Release gate for an AppDir: run it on the tree before packing and on the
# --appimage-extract output after (a raw .AppImage file only exposes the
# runtime's own ELF, which is how the reference gate passed the wrong file).
#
#   packaging/verify-appdir.sh <AppDir>
#
# Prints PASS/FAIL per check and RESULT: PASS|FAIL; exits 0 only on PASS.
# MAX_GLIBC_OVERRIDE replaces the baseline ceiling, FFMPEG_MANIFEST_OVERRIDE the
# FFmpeg manifest and FFMPEG_LIBRARIES_OVERRIDE the library registry (tests only).
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FFM="${FFMPEG_MANIFEST_OVERRIDE:-$HERE/ffmpeg_manifest_linux.json}"
REG="${FFMPEG_LIBRARIES_OVERRIDE:-$HERE/ffmpeg_libraries.json}"
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

# Resolve the root and follow symlinked directories, so a linked tree is
# scanned rather than passed vacuously.
DIR="$(readlink -f "$DIR")"

is_elf() { [ "$(head -c4 "$1" 2>/dev/null | od -An -c | tr -d ' ')" = '177ELF' ]; }
mapfile -t ELVES < <(find -L "$DIR" -type f | while read -r f; do is_elf "$f" && echo "$f"; done)

# 1. files
missing=()
for f in AppRun space.kalopsia.fthrust.desktop space.kalopsia.fthrust.png .DirIcon \
         LICENSE THIRD_PARTY_NOTICES.md licenses packaging/ffmpeg_manifest_linux.json \
         usr/bin/assets/fonts/Oswald-Regular.ttf; do
    [ -e "$DIR/$f" ] || missing+=("$f")
done
for b in "${BINARIES[@]}"; do [ -x "$DIR/usr/bin/$b" ] || missing+=("usr/bin/$b"); done
[ ${#missing[@]} -eq 0 ] && pass files || fail files "missing ${missing[*]}"

# 2. ffmpeg-libs: every bundled FFmpeg file is a pinned, unmodified one.
problems=()
while IFS= read -r -d '' f; do
    [ -L "$f" ] && continue  # soname links, checked below
    name="$(basename "$f")"
    want="$(jq -r --arg n "$name" '.shipped_files_sha256[$n] // empty' "$FFM")"
    if [ -z "$want" ]; then problems+=("unpinned $name"); continue; fi
    [ "$(sha256sum "$f" | cut -d' ' -f1)" = "$want" ] || problems+=("hash $name")
done < <(find -L "$DIR" -type f \( -name 'libav*' -o -name 'libsw*' -o -name 'libpostproc*' \) -print0)
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
pw="$(find -L "$DIR" -name 'libpipewire*' | head -1)"
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

# 6. ffmpeg-config: the configure line recorded in the manifest, which the
# licence notices are generated from, is the one inside every bundled library.
want="$(jq -r .configuration "$FFM")"
problems=()
scanned=0
while IFS= read -r -d '' f; do
    scanned=$((scanned + 1))
    grep -aqF -- "$want" "$f" || problems+=("$(basename "$f")")
done < <(find -L "$DIR" -type f \( -name 'libav*' -o -name 'libsw*' \) -print0)
if [ -z "$want" ] || [ "$want" = null ]; then problems+=("the manifest records no configuration"); fi
[ $scanned -gt 0 ] || problems+=("no FFmpeg libraries found")
[ ${#problems[@]} -eq 0 ] && pass ffmpeg-config || fail ffmpeg-config "${problems[*]}"

# 7. notice-links: the links in THIRD_PARTY_NOTICES.md resolve inside the package.
if out="$("$HERE/check-notice-links.sh" --require-links "$DIR/THIRD_PARTY_NOTICES.md" 2>&1)"; then pass notice-links; else fail notice-links "$(tr '\n' ' ' <<<"$out")"; fi

# 8. gpl-libs: a library the notices declare as GPL ("gpl": true in the registry) is in exactly the bundled
# libraries they name. Each is recognised by strings only it contains (its "signatures": FFTW's own solver names,
# string literals that survive stripping); a mismatch means the licence notices describe a different build than
# the one in the package. A registry that cannot be read, or that names no GPL library, fails the gate (as a
# missing configure line fails gate 6): the check must not pass because it had nothing to check.
problems=()
declare -A reported=()
report() { [ -n "${reported[$1]:-}" ] || { reported[$1]=1; problems+=("$1"); }; }
gpl_rows='(.libraries[], .supporting_libraries[]) | select(.gpl == true) | .id as $id | (.in_libraries | join(" ")) as $in | .signatures[] | [$id, $in, .] | join("\u001f")'
if err="$(jq -r "$gpl_rows" "$REG" 2>&1 >/dev/null)"; then
    rows="$(jq -r "$gpl_rows" "$REG")"
    [ -n "$rows" ] || report "${REG##*/} marks no library \"gpl\": true, so the notices name nothing behind the GPL in libavformat"
else
    report "cannot read the GPL libraries from ${REG##*/}: $(head -n1 <<<"$err")"
    rows=""
fi
LIBS=()
declare -A seen=()
while IFS= read -r -d '' f; do
    real="$(readlink -f "$f")"
    [ -z "${seen[$real]:-}" ] || continue
    seen[$real]=1
    LIBS+=("$f")
done < <(find -L "$DIR" -type f \( -name 'libav*' -o -name 'libsw*' \) -print0)
[ ${#LIBS[@]} -gt 0 ] || report "no FFmpeg libraries found"
while IFS=$'\x1f' read -r id names sig; do
    [ -n "$id" ] || continue
    if [ -z "$names" ] || [ -z "$sig" ]; then report "$id has no signatures or no in_libraries in ${REG##*/}"; continue; fi
    for n in $names; do
        found=0
        for f in "${LIBS[@]}"; do [[ "$(basename "$f")" == "$n".so.* ]] && found=1; done
        [ $found = 1 ] || report "$id: $n, which the notices say holds it, is not in the package"
    done
    for f in "${LIBS[@]}"; do
        base="$(basename "$f")"
        has=0 want=0
        grep -aqF -- "$sig" "$f" && has=1
        for n in $names; do [[ "$base" == "$n".so.* ]] && want=1; done
        [ $has = 1 ] && [ $want = 0 ] && report "$id ($sig) is in $base, which the notices do not name"
        [ $has = 0 ] && [ $want = 1 ] && report "$id ($sig) is not in $base, which the notices say holds it"
    done
done <<<"$rows"
[ ${#problems[@]} -eq 0 ] && pass gpl-libs || fail gpl-libs "${problems[*]}"

[ $failed -eq 0 ] && { echo "RESULT: PASS"; exit 0; }
echo "RESULT: FAIL"
exit 1
