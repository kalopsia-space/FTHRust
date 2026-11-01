#!/usr/bin/env bash
# Prints the release notes for a tag: its section of CHANGES.md.
#
#   packaging/release-notes.sh TAG [--changes FILE]
#
# TAG is v<version>, <version> or refs/tags/v<version>. The section starts at the
# "## " heading that names the version (## 2.0.0-alpha.3, ## v2.0.0-alpha.3,
# ## [2.0.0-alpha.3] or any of those followed by " (date)" or other text) and
# runs to the next "## " heading. Headings that only start with the version
# (2.0.0-alpha.1 against 2.0.0-alpha.10) do not match. A missing or empty
# section is an error: a release without notes is not published.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHANGES="$ROOT/CHANGES.md"
die() { echo "release-notes: $*" >&2; exit 1; }

tag="${1:-}"
[ -n "$tag" ] || die "usage: release-notes.sh TAG [--changes FILE]"
shift
while [ $# -gt 0 ]; do
    case "$1" in
        --changes) CHANGES="${2:?--changes needs a file}"; shift ;;
        *) die "unknown argument $1" ;;
    esac
    shift
done
[ -f "$CHANGES" ] || die "missing $CHANGES"

version="${tag#refs/tags/}"
version="${version#v}"
[ -n "$version" ] || die "empty version in '$tag'"

notes="$(awk -v v="$version" '
    /^## / {
        if (inside) exit
        h = substr($0, 4)
        sub(/^\[/, "", h)
        sub(/^v/, "", h)
        if (index(h, v) == 1) {
            rest = substr(h, length(v) + 1)
            if (rest == "" || rest ~ /^[] (]/) { inside = 1 }
        }
        next
    }
    inside { print }
' "$CHANGES" | sed -e :a -e '/^[[:space:]]*$/{$d;N;ba' -e '}' | sed '/./,$!d')"

[ -n "$notes" ] || die "$(basename "$CHANGES") has no non-empty '## $version' section: add one before tagging"
printf '%s\n' "$notes"
