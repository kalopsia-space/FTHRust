#!/usr/bin/env bash
# Archives the source of the FFmpeg build the AppImage ships, so a release can
# carry it: BtbN's monthly build assets are pruned after about two years, and
# the LGPL asks us to offer the source of the libraries we distribute.
#
#   packaging/archive-ffmpeg-source.sh --out FILE.tar.xz [--manifest FILE]
#
# The archive holds two trees, each fetched at the commit that
# packaging/ffmpeg_manifest_linux.json pins (corresponding_source) and checked
# against it: FFmpeg itself (ffmpeg-<describe>/) and BtbN's build scripts
# (FFmpeg-Builds-<commit>/, whose scripts.d/ names the upstream repository and
# commit of every library built into the shared libraries), plus a README.txt
# with the configure line. FILE.tar.xz.sha256 is written next to it.
# ARCHIVE_ALLOW_LOCAL_REPO=1 lets the manifest name a local path (tests only).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MANIFEST="$HERE/ffmpeg_manifest_linux.json"
OUT=""
die() { echo "archive-ffmpeg-source: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT="${2:?--out needs a file}"; shift ;;
        --manifest) MANIFEST="${2:?--manifest needs a file}"; shift ;;
        *) die "unknown argument $1" ;;
    esac
    shift
done
[ -n "$OUT" ] || die "usage: archive-ffmpeg-source.sh --out FILE.tar.xz [--manifest FILE]"
[ -f "$MANIFEST" ] || die "missing $MANIFEST"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
stage="$work/stage"
mkdir -p "$stage"

field() { jq -er "$1" "$MANIFEST" || die "$MANIFEST has no $1"; }

# fetch NAME REPO COMMIT PREFIX: the tree at COMMIT, extracted into $stage/PREFIX.
fetch() {
    local name="$1" repo="$2" commit="$3" prefix="$4" got
    [[ "$commit" =~ ^[0-9a-f]{40}$ ]] || die "$name: commit must be 40 lowercase hex digits: $commit"
    [[ "$repo" == https://* ]] || [ "${ARCHIVE_ALLOW_LOCAL_REPO:-}" = 1 ] || die "$name: repo must be https: $repo"
    git init -q "$work/$name"
    git -C "$work/$name" fetch -q --depth 1 "$repo" "$commit" || die "$name: could not fetch $commit from $repo"
    got="$(git -C "$work/$name" rev-parse FETCH_HEAD)"
    [ "$got" = "$commit" ] || die "$name: fetched $got, expected $commit"
    git -C "$work/$name" archive --format=tar --prefix="$prefix/" "$commit" | tar -x -C "$stage"
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

# A stable timestamp: the FFmpeg commit's, unless the caller fixes one.
mtime="${SOURCE_DATE_EPOCH:-$(git -C "$work/ffmpeg" log -1 --format=%ct "$ff_commit")}"

{
    cat <<EOF
Source of the FFmpeg libraries shipped in FTHRust Clips
=======================================================

FTHRust Clips bundles the unmodified shared libraries of BtbN's FFmpeg build
$(field .version) (LGPL, shared):

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
      scripts.d/ holds one script per library built into the shared libraries;
      each names the upstream repository and the commit or tag it builds.

FFmpeg is licensed under the LGPL (version 3 or later for this build; see
$ff_dir/COPYING.LGPLv3). The external libraries compiled into it keep their own
licences, listed with their texts in THIRD_PARTY_NOTICES.md and licenses/ffmpeg/
of the FTHRust release.

The build was configured with:

$(field .configuration)
EOF
} > "$stage/README.txt"

mkdir -p "$(dirname "$OUT")"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$mtime" -C "$stage" -cf - . | xz -T0 -9 > "$OUT.part"
mv "$OUT.part" "$OUT"
(cd "$(dirname "$OUT")" && sha256sum "$(basename "$OUT")" > "$(basename "$OUT").sha256")
echo "wrote $OUT ($(stat -c %s "$OUT") bytes)"
