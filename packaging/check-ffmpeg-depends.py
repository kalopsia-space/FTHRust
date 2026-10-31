#!/usr/bin/env python3
"""Checks that packaging/ffmpeg_libraries.json accounts for every library BtbN's build scripts pull in.

    packaging/check-ffmpeg-depends.py [--manifest FILE] [--libraries FILE] [--scripts DIR]

The configure line only names the libraries FFmpeg is configured with. BtbN's scripts also build the libraries
those need (`ffbuild_depends`), and some of them end up inside the shared libraries: FFTW, which is GPL, came in
through Chromaprint without any configure option. This starts from the options the manifest enables, finds the
script that provides each (`ffbuild_configure` echoes it), follows `ffbuild_depends` transitively, and requires
every script in that closure that builds an upstream project to be either

  * a library in the registry (same upstream repository), or
  * named in the registry's "not_linked" with the reason none of it is in the shipped libraries.

It also fails when the commit a registry entry pins is not the one BtbN's script builds. Scripts are read from
--scripts (BtbN's scripts.d, or the repository containing it), or fetched at the commit the manifest pins
(corresponding_source.build_scripts) when --scripts is not given. `ffbuild_depends` is read as written, without
evaluating its conditions, so a dependency for another platform is part of the closure: list it in "not_linked".
What this cannot see is a library linked in by a script without declaring it.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))


def norm_repo(url):
    return url.strip().rstrip("/").removesuffix(".git")


def find_scripts_dir(path):
    for cand in (os.path.join(path, "scripts.d"), path):
        if os.path.isdir(cand) and any(f.endswith(".sh") for _, _, fs in os.walk(cand) for f in fs):
            return cand
    sys.exit(f"check-ffmpeg-depends: no scripts found under {path}")


def fetch_scripts(manifest, workdir):
    bs = manifest["corresponding_source"]["build_scripts"]
    repo, commit = bs["repo"], bs["commit"]
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        sys.exit(f"check-ffmpeg-depends: build_scripts commit must be 40 lowercase hex digits: {commit}")
    run = lambda *a: subprocess.run(a, cwd=workdir, check=True, capture_output=True, text=True)
    run("git", "init", "-q", ".")
    try:
        run("git", "fetch", "-q", "--depth", "1", repo, commit)
    except subprocess.CalledProcessError as e:
        sys.exit(f"check-ffmpeg-depends: could not fetch {commit} from {repo}: {e.stderr.strip()}")
    got = run("git", "rev-parse", "FETCH_HEAD").stdout.strip()
    if got != commit:
        sys.exit(f"check-ffmpeg-depends: fetched {got}, expected {commit}")
    run("git", "checkout", "-q", "FETCH_HEAD")
    return find_scripts_dir(workdir)


def parse_scripts(scripts_dir):
    """Returns (scripts by name, groups): a sub-directory of scripts.d (45-fonts, 47-vulkan, 45-x11) is a group
    that `ffbuild_depends` can name to depend on every script in it."""
    scripts = {}
    groups = {}
    for root, _, files in os.walk(scripts_dir):
        for f in sorted(files):
            if not f.endswith(".sh"):
                continue
            text = open(os.path.join(root, f), encoding="utf-8", errors="replace").read()
            name = re.sub(r"^\d+-", "", f[:-3])
            if os.path.abspath(root) != os.path.abspath(scripts_dir):
                group = re.sub(r"^\d+-", "", os.path.relpath(root, scripts_dir).split(os.sep)[0])
                groups.setdefault(group, set()).add(name)
            m = re.search(r"ffbuild_depends\(\)\s*\{(.*?)\n\}", text, re.S)
            depends = re.findall(r"echo\s+([A-Za-z0-9_.+-]+)", m.group(1)) if m else []
            m = re.search(r"ffbuild_configure\(\)\s*\{(.*?)\n\}", text, re.S)
            flags = re.findall(r"--enable-[A-Za-z0-9_-]+", m.group(1)) if m else []
            repo = re.search(r'^SCRIPT_REPO="?([^"\n]*)', text, re.M)
            commit = re.search(r'^SCRIPT_COMMIT="?([^"\n]*)', text, re.M)
            tag = re.search(r'^SCRIPT_TAGFILTER="?([^"\n]*)', text, re.M)
            rev = re.search(r'^SCRIPT_REV="?([^"\n]*)', text, re.M)
            pin = (commit and commit.group(1)) or (tag and tag.group(1)) or (rev and f"r{rev.group(1)}") or ""
            scripts.setdefault(name, []).append(
                {"path": os.path.relpath(os.path.join(root, f), scripts_dir), "depends": depends, "flags": flags,
                 "repo": norm_repo(repo.group(1)) if repo else "", "pin": pin}
            )
    return scripts, groups


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", default=os.path.join(HERE, "ffmpeg_manifest_linux.json"))
    ap.add_argument("--libraries", default=os.path.join(HERE, "ffmpeg_libraries.json"))
    ap.add_argument("--scripts")
    args = ap.parse_args()

    manifest = json.load(open(args.manifest))
    registry = json.load(open(args.libraries))
    with tempfile.TemporaryDirectory() as tmp:
        scripts_dir = find_scripts_dir(args.scripts) if args.scripts else fetch_scripts(manifest, tmp)
        scripts, groups = parse_scripts(scripts_dir)

    ignored = set(registry.get("ignored_options", {}))
    enabled = [o for o in manifest["configuration"].split() if o.startswith("--enable-") and o not in ignored]
    providers = {}
    for name, infos in scripts.items():
        for i in infos:
            for flag in i["flags"]:
                providers.setdefault(flag, set()).add(name)

    errors = []
    missing_provider = [o for o in enabled if o not in providers]
    if len(missing_provider) * 5 > len(enabled):
        errors.append(
            f"only {len(enabled) - len(missing_provider)} of {len(enabled)} enabled options have a provider script "
            f"(missing: {' '.join(missing_provider)}): is {scripts_dir if args.scripts else 'the fetched tree'} BtbN's scripts.d?"
        )

    closure = {}  # script name -> the option chain that needs it

    def visit(name, why):
        if name in closure:
            return
        closure[name] = why
        for member in sorted(groups.get(name, ())):
            visit(member, f"{why} > {name}")
        for info in scripts.get(name, []):
            for dep in info["depends"]:
                visit(dep, f"{why} > {name}")

    for option in enabled:
        for name in sorted(providers.get(option, ())):
            visit(name, option)

    entries = registry["libraries"] + registry["supporting_libraries"]
    by_repo = {}
    for e in entries:
        by_repo.setdefault(norm_repo(e["pinned_repo"]), []).append(e)
    not_linked = registry.get("not_linked", {})

    covered = linked_away = 0
    for name in sorted(closure):
        infos = scripts.get(name, [])
        repos = {i["repo"] for i in infos if i["repo"]}
        if not infos and name in groups:
            continue  # a directory of scripts: its members are in the closure themselves
        if not infos:
            errors.append(f"{name}: needed by {closure[name]}, but there is no such script in BtbN's scripts.d")
        elif not repos:
            continue  # a grouping script (base, fonts, x11, vulkan): its dependencies are followed
        elif repos & set(by_repo):
            covered += 1
        elif name in not_linked:
            linked_away += 1
        else:
            errors.append(
                f"{name} ({', '.join(sorted(repos))}) is built for {closure[name]} but packaging/ffmpeg_libraries.json "
                f"neither lists it nor says in \"not_linked\" why none of it is in the shipped libraries"
            )

    for repo, es in sorted(by_repo.items()):
        pins = {i["pin"] for infos in scripts.values() for i in infos if i["repo"] == repo}
        if not pins:
            errors.append(f"{es[0]['id']}: no BtbN script builds {repo} any more (renamed or dropped?)")
            continue
        for e in es:
            if e["pinned_commit"] not in pins:
                errors.append(
                    f"{e['id']}: the registry pins {e['pinned_commit'] or '(nothing)'} but BtbN's script builds "
                    f"{', '.join(sorted(pins))}"
                )
    stale = sorted(n for n in not_linked if n not in closure)
    for n in stale:
        errors.append(f"not_linked names {n}, which no enabled option needs: remove it")

    if errors:
        print("check-ffmpeg-depends:\n  " + "\n  ".join(errors), file=sys.stderr)
        sys.exit(1)
    print(
        f"ffmpeg dependencies covered: {len(closure)} scripts in the closure, {covered} upstream projects in the registry, "
        f"{linked_away} recorded as not linked"
    )


if __name__ == "__main__":
    main()
