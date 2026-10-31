#!/usr/bin/env bash
# Archives the source behind the FFmpeg libraries the AppImage ships, so a release
# can carry it: BtbN's monthly build assets are pruned after about two years, and
# the licences of FFmpeg (LGPL) and of the libraries compiled into it (LGPL, MPL,
# and the GPL for FFTW inside libavformat) ask us to offer their source.
#
#   packaging/archive-ffmpeg-source.sh --out FILE.tar.xz [--manifest FILE] [--libraries FILE]
#
# The archive holds these trees, each fetched at a pinned commit:
#   ffmpeg-<describe>/        FFmpeg itself
#   FFmpeg-Builds-<commit>/   BtbN's build scripts: scripts.d/ names the repository and the
#                             commit of every library, and holds the patches BtbN applies
#   <id>-<pin>/               every library packaging/ffmpeg_libraries.json lists whose code
#                             is compiled into the shipped libraries (all but the entries
#                             marked "compiled_in": false), at the commit or tag BtbN's
#                             scripts build, with the submodules BtbN's scripts check out
#                             ("submodules" in the registry) and without its .git
#   README.txt                what is where, the licence note and the configure line
# FFmpeg and BtbN's scripts are pinned in packaging/ffmpeg_manifest_linux.json
# (corresponding_source) and the libraries in the registry; a commit is checked
# against its 40-digit pin, a tag against the commit the registry records for it
# ("resolved_commit": a moved tag fails the archive), and a Subversion revision
# (LAME) is exported with svn. A library's "mirror" is tried before its repository
# (libiconv's is the host BtbN's own script clones from, because Savannah's https
# host is often unreachable). FILE.tar.xz.sha256 is written next to the archive.
# Before anything is fetched but the scripts, the registry is checked against
# them (check-ffmpeg-depends.py).
#
# Every network step goes through the retry BtbN's own scripts use (images/base/
# retry-tool.sh): 10 attempts, the first with a 120 s timeout and each further one
# 60 s longer (up to 660 s), and a random pause of 5 to 90 s after a failure. A mirror
# gets only 2 attempts of its own before the repository gets the whole schedule: a
# filtered git:// host would otherwise cost an hour or more before https is tried.
# Each repository gets an origin remote, as BtbN's git-mini-clone does, so that relative
# submodule URLs resolve. Requests to github.com carry ARCHIVE_GITHUB_TOKEN when it
# is set (the release job passes its token), the way actions/checkout does: about a
# hundred anonymous fetches from one runner address are what a rate limit is for.
#
# Environment: ARCHIVE_GITHUB_TOKEN; ARCHIVE_ATTEMPTS (10), ARCHIVE_MIRROR_ATTEMPTS (2),
# ARCHIVE_FETCH_TIMEOUT
# (120 s, the first attempt's), ARCHIVE_TIMEOUT_STEP (60 s), ARCHIVE_PAUSE_MIN (5 s)
# and ARCHIVE_PAUSE_MAX (90 s); ARCHIVE_XZ_OPTS (-T0 -9); ARCHIVE_SVN (svn);
# SOURCE_DATE_EPOCH. For the tests only: ARCHIVE_ALLOW_LOCAL_REPO=1 lets the pins
# name a local path, and --skip-depends-check skips the check.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MANIFEST="$HERE/ffmpeg_manifest_linux.json"
LIBRARIES="$HERE/ffmpeg_libraries.json"
SKIP_DEPENDS=0
OUT=""
ATTEMPTS="${ARCHIVE_ATTEMPTS:-10}"
MIRROR_ATTEMPTS="${ARCHIVE_MIRROR_ATTEMPTS:-2}"
FETCH_TIMEOUT="${ARCHIVE_FETCH_TIMEOUT:-120}"
TIMEOUT_STEP="${ARCHIVE_TIMEOUT_STEP:-60}"
PAUSE_MIN="${ARCHIVE_PAUSE_MIN:-5}"
PAUSE_MAX="${ARCHIVE_PAUSE_MAX:-90}"
XZ_OPTS="${ARCHIVE_XZ_OPTS:--T0 -9}"
SVN="${ARCHIVE_SVN:-svn}"
die() { echo "archive-ffmpeg-source: $*" >&2; exit 1; }
for v in ATTEMPTS MIRROR_ATTEMPTS FETCH_TIMEOUT TIMEOUT_STEP PAUSE_MIN PAUSE_MAX; do
    [[ "${!v}" =~ ^[0-9]+$ ]] || die "ARCHIVE_$v must be a whole number, not '${!v}'"
done
[ "$ATTEMPTS" -ge 1 ] || die "ARCHIVE_ATTEMPTS must be at least 1"
[ "$MIRROR_ATTEMPTS" -ge 1 ] || die "ARCHIVE_MIRROR_ATTEMPTS must be at least 1"
[ "$PAUSE_MAX" -ge "$PAUSE_MIN" ] || die "ARCHIVE_PAUSE_MAX is below ARCHIVE_PAUSE_MIN"
# No prompts for credentials, and no background gc or maintenance: git starts one after a clone or fetch, detached, and
# it writes into .git while this script is removing it (that failed a trial run on libplacebo's many submodules).
export GIT_TERMINAL_PROMPT=0
GIT=(git -c gc.auto=0 -c maintenance.auto=false)
if [ "${ARCHIVE_ALLOW_LOCAL_REPO:-}" = 1 ]; then GIT+=(-c protocol.file.allow=always); fi
# The token goes into git's environment (not its command line), scoped to github.com URLs, so no other host sees it and
# the submodule clones git starts inherit it. The encoded form is masked in a GitHub Actions log like the token itself.
if [ -n "${ARCHIVE_GITHUB_TOKEN:-}" ]; then
    basic="$(printf 'x-access-token:%s' "$ARCHIVE_GITHUB_TOKEN" | base64 -w0)"
    if [ "${GITHUB_ACTIONS:-}" = true ]; then echo "::add-mask::$basic"; fi
    n="${GIT_CONFIG_COUNT:-0}"
    export "GIT_CONFIG_KEY_$n=http.https://github.com/.extraheader" "GIT_CONFIG_VALUE_$n=AUTHORIZATION: basic $basic" "GIT_CONFIG_COUNT=$((n + 1))"
    unset basic n
fi

while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT="${2:?--out needs a file}"; shift ;;
        --manifest) MANIFEST="${2:?--manifest needs a file}"; shift ;;
        --libraries) LIBRARIES="${2:?--libraries needs a file}"; shift ;;
        --skip-depends-check) SKIP_DEPENDS=1 ;;
        *) die "unknown argument $1" ;;
    esac
    shift
done
[ -n "$OUT" ] || die "usage: archive-ffmpeg-source.sh --out FILE.tar.xz [--manifest FILE] [--libraries FILE]"
[ -f "$MANIFEST" ] || die "missing $MANIFEST"
[ -f "$LIBRARIES" ] || die "missing $LIBRARIES"
jq -e . "$LIBRARIES" >/dev/null 2>&1 || die "$LIBRARIES is not valid JSON"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
stage="$work/stage"
mkdir -p "$stage"

field() { jq -er "$1" "$MANIFEST" || die "$MANIFEST has no $1"; }

# retry_n ATTEMPTS DESCRIPTION COMMAND...: BtbN's retry-tool. The command up to ATTEMPTS times, the first under a timeout
# of $FETCH_TIMEOUT seconds and each further one under $TIMEOUT_STEP more, with a random pause of $PAUSE_MIN to $PAUSE_MAX
# seconds after a failure. retry is retry_n with the full $ATTEMPTS.
retry_n() {
    local max="$1" what="$2" attempt=1 limit pause
    shift 2
    while :; do
        limit=$((FETCH_TIMEOUT + (attempt - 1) * TIMEOUT_STEP))
        if timeout "$limit" "$@"; then return 0; fi
        [ $attempt -lt "$max" ] || return 1
        pause=$((PAUSE_MIN + RANDOM % (PAUSE_MAX - PAUSE_MIN + 1)))
        echo "archive-ffmpeg-source: $what failed (attempt $attempt of $max, limit ${limit}s), trying again in ${pause}s" >&2
        sleep "$pause"
        attempt=$((attempt + 1))
    done
}
retry() { retry_n "$ATTEMPTS" "$@"; }

# check_repo NAME REPO KIND: REPO is an https URL (or, when KIND is "mirror", a git:// one: what arrives is checked against
# the commit id anyway). A local path is accepted only for the tests; any other scheme never is.
check_repo() {
    local hint=""
    case "$2" in
        https://*) return 0 ;;
        git://*) if [ "$3" = mirror ]; then return 0; fi ;;
        *://*) ;;
        *) if [ "${ARCHIVE_ALLOW_LOCAL_REPO:-}" = 1 ]; then return 0; fi ;;
    esac
    if [ "$3" = mirror ]; then hint=" (or git for a mirror)"; fi
    die "$1: repo must be https$hint: $2"
}

# git_fetch DIR NAME PIN REPO [MIRROR]: PIN (a commit id or a tag), shallow, into the new repository DIR, from MIRROR if
# there is one (MIRROR_ATTEMPTS attempts: a mirror that is filtered or slow must not cost the whole schedule) and else, or
# if that fails, from REPO (the full schedule). The repository gets an origin remote, as BtbN's git-mini-clone does,
# so that a relative submodule URL (../x.git) resolves against the place the tree came from. Sets fetched_from.
git_fetch() {
    local dir="$1" name="$2" pin="$3" repo="$4" mirror="${5:-}" ref candidate kind tries origin=0
    "${GIT[@]}" init -q "$dir"
    if [[ "$pin" =~ ^[0-9a-f]{40}$ ]]; then ref="$pin"; else ref="refs/tags/$pin:refs/tags/$pin"; fi
    for kind in mirror repo; do
        if [ "$kind" = mirror ]; then candidate="$mirror" tries="$MIRROR_ATTEMPTS"; else candidate="$repo" tries="$ATTEMPTS"; fi
        [ -n "$candidate" ] || continue
        check_repo "$name" "$candidate" "$kind"
        if [ $origin = 1 ]; then
            "${GIT[@]}" -C "$dir" remote set-url origin "$candidate"
        else
            "${GIT[@]}" -C "$dir" remote add origin "$candidate"
            origin=1
        fi
        if retry_n "$tries" "fetching $name from $candidate" "${GIT[@]}" -C "$dir" fetch -q --depth 1 origin "$ref"; then
            fetched_from="$candidate"
            return 0
        fi
    done
    return 1
}

# fetch NAME REPO COMMIT PREFIX: the tree at COMMIT (checked against the pin), extracted into $stage/PREFIX.
fetch() {
    local name="$1" repo="$2" commit="$3" prefix="$4" got
    [[ "$commit" =~ ^[0-9a-f]{40}$ ]] || die "$name: commit must be 40 lowercase hex digits: $commit"
    git_fetch "$work/$name" "$name" "$commit" "$repo" || die "$name: could not fetch $commit from $repo after $ATTEMPTS attempts"
    got="$("${GIT[@]}" -C "$work/$name" rev-parse FETCH_HEAD)"
    [ "$got" = "$commit" ] || die "$name: fetched $got, expected $commit"
    "${GIT[@]}" -C "$work/$name" archive --format=tar --prefix="$prefix/" "$commit" | tar -x -C "$stage"
}

# fetch_library ID REPO PIN MIRROR SUBMODULES RESOLVED: a library's tree into $stage/<id>-<pin>/, without .git.
# SUBMODULES is "all", "none" or "paths:A B" (what BtbN's script checks out; see "submodules" in the registry) and
# RESOLVED the commit a tag pin must name. Sets lib_dir (that directory's name), lib_from (where the tree came from, for
# the README) and lib_submodules (a README line about the submodules, or nothing).
fetch_library() {
    local id="$1" repo="$2" pin="$3" mirror="$4" submodules="$5" resolved="$6" tree="$work/lib-$1" commit paths
    lib_submodules=""
    if [[ "$pin" =~ ^r[0-9]+$ ]]; then
        command -v "$SVN" >/dev/null 2>&1 || die "$id: the registry pins Subversion revision $pin, which needs svn (apt-get install subversion; ARCHIVE_SVN names another)"
        check_repo "$id" "$repo" ""
        lib_dir="$id-$pin"
        retry "exporting $id $pin from $repo" "$SVN" export -q --force --non-interactive "$repo@${pin#r}" "$stage/$lib_dir" \
            || die "$id: could not export $pin from $repo after $ATTEMPTS attempts"
        lib_from="Subversion revision ${pin#r} of $repo"
        return
    fi
    [[ "$pin" =~ ^[0-9a-f]{40}$ || "$pin" =~ ^[A-Za-z0-9][A-Za-z0-9._+/-]*$ ]] \
        || die "$id: pin must be a 40-digit commit id, a tag name or a Subversion revision (rNNN): $pin"
    git_fetch "$tree" "$id" "$pin" "$repo" "$mirror" \
        || die "$id: could not fetch $pin from ${mirror:+$mirror after $MIRROR_ATTEMPTS attempts or from }$repo after $ATTEMPTS attempts"
    if [[ "$pin" =~ ^[0-9a-f]{40}$ ]]; then
        commit="$("${GIT[@]}" -C "$tree" rev-parse FETCH_HEAD)"
        [ "$commit" = "$pin" ] || die "$id: fetched $commit, expected $pin"
        lib_dir="$id-${pin:0:12}"
        lib_from="commit $pin of $repo"
    else
        commit="$("${GIT[@]}" -C "$tree" rev-parse "refs/tags/$pin^{commit}")"
        # A tag can be moved. The registry records the commit it named when it was pinned, and a different one stops the
        # archive instead of silently changing the source that ships.
        [ -n "$resolved" ] || die "$id: the pin $pin is a tag, so the registry has to record the commit it names (resolved_commit)"
        [ "$commit" = "$resolved" ] \
            || die "$id: tag $pin names $commit but the registry records $resolved: the tag was moved, or the pin changed; check what BtbN's script builds, then update resolved_commit"
        lib_dir="$id-${pin//\//_}"
        case "$pin" in "$id"-*) lib_dir="${pin//\//_}" ;; esac # openssl-3.6.3, not openssl-openssl-3.6.3
        lib_from="tag $pin (commit $commit) of $repo"
    fi
    if [ "$fetched_from" != "$repo" ]; then lib_from="$lib_from, fetched from $fetched_from"; fi
    "${GIT[@]}" -C "$tree" checkout -q "$commit"
    if [ -f "$tree/.gitmodules" ]; then
        case "$submodules" in
            all)
                retry "fetching the submodules of $id" "${GIT[@]}" -C "$tree" submodule update --init --recursive --depth 1 --quiet \
                    || die "$id: could not fetch its submodules after $ATTEMPTS attempts"
                ;;
            paths:*)
                paths="${submodules#paths:}"
                # shellcheck disable=SC2086 # a list of paths, none with a space
                retry "fetching the submodules of $id" "${GIT[@]}" -C "$tree" submodule update --init --recursive --depth 1 --quiet -- $paths \
                    || die "$id: could not fetch its submodules ($paths) after $ATTEMPTS attempts"
                lib_submodules="submodules: only $paths, which is all BtbN's script checks out"
                ;;
            none) lib_submodules="submodules: none, BtbN's script checks none out" ;;
            *) die "$id: unknown submodule policy '$submodules'" ;;
        esac
    fi
    find "$tree" -name .git -prune -exec rm -rf {} +
    mv "$tree" "$stage/$lib_dir"
}

ff_repo="$(field .corresponding_source.ffmpeg.repo)"
ff_commit="$(field .corresponding_source.ffmpeg.commit)"
ff_describe="$(field .corresponding_source.ffmpeg.describe)"
bs_repo="$(field .corresponding_source.build_scripts.repo)"
bs_commit="$(field .corresponding_source.build_scripts.commit)"
ff_dir="ffmpeg-$ff_describe"
bs_dir="FFmpeg-Builds-${bs_commit:0:12}"

fetch ffmpeg "$ff_repo" "$ff_commit" "$ff_dir"
fetch build-scripts "$bs_repo" "$bs_commit" "$bs_dir"

# The registry has to account for every library those scripts pull in, or the archive (and the release) stops here.
if [ $SKIP_DEPENDS = 0 ]; then
    python3 -I "$HERE/check-ffmpeg-depends.py" --manifest "$MANIFEST" --libraries "$LIBRARIES" --scripts "$stage/$bs_dir" \
        || die "the library registry does not match BtbN's build scripts"
fi

# Every library whose code is compiled into the shipped libraries.
lib_readme="" downloads="" count=0
while IFS=$'\x1f' read -r id name license repo pin mirror gpl_in fetched submodules resolved; do
    [ -n "$id" ] || continue
    fetch_library "$id" "$repo" "$pin" "$mirror" "$submodules" "$resolved" </dev/null
    count=$((count + 1))
    echo "archived $lib_dir ($count)" >&2
    lib_readme+="  $lib_dir/
      $name ($license)
      $lib_from
"
    if [ -n "$lib_submodules" ]; then
        lib_readme+="      $lib_submodules
"
    fi
    if [ -n "$gpl_in" ]; then
        lib_readme+="      compiled into $gpl_in; GPL code, so its source is part of this archive.
"
    fi
    if [ -n "$fetched" ]; then
        downloads+="  $lib_dir/
      $fetched
"
    fi
done < <(jq -r '(.libraries[], .supporting_libraries[]) | select(.compiled_in != false) | [.id, .name, .license, .pinned_repo, .pinned_commit, (.mirror // ""), (if .gpl == true then (.in_libraries | join(" and ")) else "" end), (.fetched_at_build // ""), (if has("submodules") then (if (.submodules | length) == 0 then "none" else "paths:" + (.submodules | join(" ")) end) else "all" end), (.resolved_commit // "")] | join("\u001f")' "$LIBRARIES")
[ $count -gt 0 ] || die "$LIBRARIES lists no library whose code is compiled in"

# The ones the registry lists whose code is not inside the shipped libraries are named, not archived.
left_out=""
while IFS=$'\x1f' read -r id name repo pin note; do
    [ -n "$id" ] || continue
    left_out+="  $name ($id): $repo at $pin
      $note
"
done < <(jq -r '(.libraries[], .supporting_libraries[]) | select(.compiled_in == false) | [.id, .name, .pinned_repo, .pinned_commit, .note] | join("\u001f")' "$LIBRARIES")

# A stable timestamp: the FFmpeg commit's, unless the caller fixes one.
mtime="${SOURCE_DATE_EPOCH:-$("${GIT[@]}" -C "$work/ffmpeg" log -1 --format=%ct "$ff_commit")}"

{
    cat <<EOF
Source of the FFmpeg libraries shipped in FTHRust Clips
=======================================================

FTHRust Clips bundles the unmodified shared libraries of BtbN's FFmpeg build
$(field .version):

  asset    $(field .source.asset)
  from     $(field .source.url)
  sha256   $(field .source.sha256)

This archive is the corresponding source:

  $ff_dir/
      FFmpeg at commit $ff_commit
      ($ff_repo)
  $bs_dir/
      BtbN's build scripts at commit $bs_commit
      ($bs_repo)
      scripts.d/ holds one script per library built into the shared libraries and
      the patches BtbN applies; each script names the upstream repository and the
      commit or tag it builds.

and the libraries compiled into the shared libraries, each at the commit or tag
BtbN's scripts build, with the submodules their scripts check out:

${lib_readme}
EOF
    if [ -n "$downloads" ]; then
        cat <<EOF
BtbN's scripts also download code while they build, which the trees above do not
hold; each tree names what (a DEPS file, a Cargo.lock) in its own sources:

${downloads}
EOF
    fi
    if [ -n "$left_out" ]; then
        cat <<EOF
Libraries the build is configured with that contribute no code to the shared
libraries (headers only, loaded from the system at run time, or unused) are not
in this archive:

${left_out}
EOF
    fi
    cat <<EOF
FFmpeg's own code is licensed under the LGPL (version 3 or later for this build;
see $ff_dir/COPYING.LGPLv3). The external libraries compiled into it keep their
own licences, listed with their texts in THIRD_PARTY_NOTICES.md and
licenses/ffmpeg/ of the FTHRust release; where that list marks one as GPL, the
shared library it is compiled into is distributed under the GPL.

The build was configured with:

$(field .configuration)
EOF
} > "$stage/README.txt"

mkdir -p "$(dirname "$OUT")"
# shellcheck disable=SC2086 # XZ_OPTS is a list of options
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$mtime" -C "$stage" -cf - . | xz $XZ_OPTS > "$OUT.part"
mv "$OUT.part" "$OUT"
(cd "$(dirname "$OUT")" && sha256sum "$(basename "$OUT")" > "$(basename "$OUT").sha256")
echo "wrote $OUT ($(stat -c %s "$OUT") bytes, $count libraries)"
