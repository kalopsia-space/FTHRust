#!/usr/bin/env python3
"""Prints the FFmpeg library section of THIRD_PARTY_NOTICES.md.

    packaging/gen-ffmpeg-notice.py [--manifest FILE] [--libraries FILE] [--licenses DIR]
                                   [--template FILE]

Without --template it prints the "External libraries built into FFmpeg" section.
With --template (packaging/notices/header.md) it prints that file instead, with
{{ffmpeg.<field>}} placeholders filled from the manifest and the marker line
<!-- ffmpeg-libraries --> replaced by the section, so no version, hash or commit
in the notices is typed by hand.

The section is generated from the configure line the FFmpeg manifest records
(packaging/ffmpeg_manifest_linux.json; the release gate checks that every
bundled library contains exactly that line) and the registry in
packaging/ffmpeg_libraries.json, which says what each option is. It exits 1,
with the reasons on stderr, when the two disagree: a library the build enables
that the registry does not know, a registry entry the build no longer enables,
or a licence text that is missing.
"""
import argparse
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
# Options that disable parts of FFmpeg that are not third-party libraries.
PLAIN_DISABLES = {"--disable-debug", "--disable-static", "--disable-schannel"}


def short(url):
    return url.split("://", 1)[-1].rstrip("/")


def short_pin(commit):
    """A 40-digit commit id is shown as its first 12; a tag or a Subversion revision (openssl-3.6.3, r6531) whole."""
    return commit[:12] if re.fullmatch(r"[0-9a-f]{40}", commit) else commit


def upstream(entry):
    repo = entry.get("pinned_repo", "").removesuffix(".git")
    commit = entry.get("pinned_commit", "")
    pin = f" `{short_pin(commit)}`" if commit else ""
    if repo and repo != entry["url"] and not repo.startswith("https://git.code.sf.net"):
        return f"[{short(entry['url'])}]({entry['url']}), built from [{short(repo)}]({repo}){pin}"
    return f"[{short(entry['url'])}]({entry['url']}){pin}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", default=os.path.join(HERE, "ffmpeg_manifest_linux.json"))
    ap.add_argument("--libraries", default=os.path.join(HERE, "ffmpeg_libraries.json"))
    ap.add_argument("--licenses", default=os.path.join(ROOT, "licenses", "ffmpeg"))
    ap.add_argument("--template")
    args = ap.parse_args()

    manifest = json.load(open(args.manifest))
    registry = json.load(open(args.libraries))
    config = manifest.get("configuration")
    if not config:
        sys.exit("gen-ffmpeg-notice: the manifest records no configuration")
    options = config.split()
    enabled = [o for o in options if o.startswith("--enable-")]
    disabled = [o for o in options if o.startswith("--disable-") and o not in PLAIN_DISABLES]

    libs = registry["libraries"]
    supporting = registry["supporting_libraries"]
    ignored = registry["ignored_options"]
    covered = {f for e in libs for f in e["flags"]} | set(ignored)

    problems = []
    for o in enabled:
        if o not in covered:
            problems.append(f"the build enables {o}, which packaging/ffmpeg_libraries.json does not describe: add the library and its licence text")
    for f in sorted(covered):
        if f not in enabled:
            problems.append(f"packaging/ffmpeg_libraries.json describes {f}, which the build no longer enables: remove it")
    for e in libs + supporting:
        path = os.path.join(args.licenses, e["text"])
        if not os.path.isfile(path) or os.path.getsize(path) < 100:
            problems.append(f"licence text missing for {e['id']}: {path}")
    ids = [e["id"] for e in libs + supporting]
    problems += [f"duplicate library id {i}" for i in sorted({i for i in ids if ids.count(i) > 1})]
    on_disk = {f for f in os.listdir(args.licenses) if f.endswith(".txt")} if os.path.isdir(args.licenses) else set()
    for f in sorted(on_disk - {e["text"] for e in libs + supporting}):
        problems.append(f"licenses/ffmpeg/{f} belongs to no library in the registry")
    if problems:
        print("gen-ffmpeg-notice: " + "\n gen-ffmpeg-notice: ".join(problems), file=sys.stderr)
        sys.exit(1)

    out = []
    w = out.append
    w("### External libraries built into FFmpeg\n")
    w(
        "BtbN's FFmpeg compiles the libraries below into the shared libraries it ships. "
        "This list is generated from the build's configure line, which is `configuration` in "
        "[`packaging/ffmpeg_manifest_linux.json`](packaging/ffmpeg_manifest_linux.json); the release gate "
        "checks that every bundled library contains exactly that line, so the list follows the pinned build. "
        "\"Pinned\" is the upstream commit or tag that BtbN's build scripts "
        f"(commit `{manifest['corresponding_source']['build_scripts']['commit'][:12]}` of "
        "[BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds), archived with every release) use for the "
        "library; its licence is identified from the licence file at that commit, reproduced in "
        "[`licenses/ffmpeg/`](licenses/ffmpeg/). Some of the libraries are loaded from the user's system when "
        "FFmpeg runs rather than compiled in; they are listed because the build is configured with them. "
        "Where a project's own licence file says more than one thing, the text keeps all of it.\n"
    )
    w("| Option | Library | Licence | Upstream (pinned) | Licence text |")
    w("|---|---|---|---|---|")
    for e in sorted(libs, key=lambda e: e["name"].lower()):
        flags = " ".join(f"`{f}`" for f in e["flags"])
        w(f"| {flags} | {e['name']} | {e['license']} | {upstream(e)} | [{e['text']}](licenses/ffmpeg/{e['text']}) |")
    w("")
    notes = [e for e in libs if e.get("note")]
    if notes:
        for e in sorted(notes, key=lambda e: e["name"].lower()):
            w(f"- **{e['name']}**: {e['note']}")
        w("")
    w("Libraries that BtbN's build scripts also build, for the libraries above:\n")
    w("| Library | Licence | Built for | Upstream (pinned) | Licence text |")
    w("|---|---|---|---|---|")
    for e in sorted(supporting, key=lambda e: e["name"].lower()):
        w(f"| {e['name']} | {e['license']} | {', '.join(e['needed_by'])} | {upstream(e)} | [{e['text']}](licenses/ffmpeg/{e['text']}) |")
    w("")
    w("Options that add no third-party code:\n")
    for o in sorted(ignored):
        w(f"- `{o}`: {ignored[o]}")
    w("")
    if disabled:
        w(
            "The configure line also disables these libraries and options (the GPL and non-free libraries among "
            "them), so nothing from them is bundled: "
            + ", ".join(f"`{o}`" for o in disabled)
            + ".\n"
        )
    w(
        "Some of these projects bundle code from further projects (for example shaderc builds glslang and "
        "SPIRV-Tools, and libjxl builds highway and skcms); their notices are in the upstream sources named "
        "above, not repeated here. Each GitHub release also carries `FTHRust-ffmpeg-source-*.tar.xz`, the FFmpeg "
        "commit and BtbN's build scripts that the bundled libraries were built from. This is a technical "
        "inventory, not legal advice.\n"
    )
    section = "\n".join(out)
    if not args.template:
        sys.stdout.write(section)
        return

    src = manifest["source"]
    cs = manifest["corresponding_source"]
    fields = {
        "describe": cs["ffmpeg"]["describe"],
        "release_tag": src["release_tag"],
        "asset": src["asset"],
        "sha256": src["sha256"],
        "commit": cs["ffmpeg"]["commit"],
        "scripts_commit": cs["build_scripts"]["commit"],
        "shipped_files": ", ".join(f"`{n}`" for n in sorted(manifest["soname_map"].values())),
    }
    text = open(args.template).read()
    marker = "<!-- ffmpeg-libraries -->"
    if text.count(marker) != 1:
        sys.exit(f"gen-ffmpeg-notice: {args.template} must hold the line {marker} exactly once")
    text = text.replace(marker, section.rstrip("\n"))
    for k, v in fields.items():
        text = text.replace("{{ffmpeg." + k + "}}", v)
    left = sorted(set(re.findall(r"\{\{ffmpeg\.[^}]*\}\}", text)))
    if left:
        sys.exit("gen-ffmpeg-notice: unknown placeholders in the template: " + ", ".join(left))
    sys.stdout.write(text)


if __name__ == "__main__":
    main()
