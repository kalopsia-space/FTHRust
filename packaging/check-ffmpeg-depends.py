#!/usr/bin/env python3
"""Checks that packaging/ffmpeg_libraries.json accounts for every library BtbN's build scripts pull in.

    packaging/check-ffmpeg-depends.py [--manifest FILE] [--libraries FILE] [--scripts DIR]

The configure line only names the libraries FFmpeg is configured with. BtbN's scripts also build the libraries
those need (`ffbuild_depends`), and some of them end up inside the shared libraries: FFTW, which is GPL, came in
through Chromaprint without any configure option. This starts from the options the manifest enables, finds the
script that provides each (`ffbuild_configure` echoes it), follows `ffbuild_depends` transitively, and requires
every upstream repository such a script builds (SCRIPT_REPO, SCRIPT_REPO2, ...) to be either

  * a library in the registry (same repository), or
  * named in the registry's "not_linked", by script name or repository URL, with the reason none of it is in the
    shipped libraries.

It fails the other way round too: a registry pin that is not what BtbN's script builds, a supporting library that
no enabled option needs any more (so dropping an FFmpeg option fails in the pull request that does it, not at the
release gate), and a "not_linked" entry that is no longer needed. Scripts are read from --scripts (BtbN's
scripts.d, or the repository containing it), or fetched at the commit the manifest pins
(corresponding_source.build_scripts) when --scripts is not given; the fetch is retried and has a timeout
(FFMPEG_DEPENDS_FETCH_TIMEOUT seconds, default 120, and FFMPEG_DEPENDS_FETCH_DELAY seconds of back-off, default 5).
`ffbuild_depends` is read as written, without evaluating its conditions, so a dependency for another platform is
part of the closure: list it in "not_linked". What this cannot see is a library linked in by a script without
declaring it.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
FETCH_ATTEMPTS = 3


def norm_repo(url):
    return url.strip().rstrip("/").removesuffix(".git")


def find_scripts_dir(path):
    for cand in (os.path.join(path, "scripts.d"), path):
        if os.path.isdir(cand) and any(f.endswith(".sh") for _, _, fs in os.walk(cand) for f in fs):
            return cand
    sys.exit(f"check-ffmpeg-depends: no scripts found under {path}")


def fetch_scripts(manifest, workdir):
    """Shallow-fetches BtbN's repository at the pinned commit into workdir; returns its scripts.d."""
    bs = manifest["corresponding_source"]["build_scripts"]
    repo, commit = bs["repo"], bs["commit"]
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        sys.exit(f"check-ffmpeg-depends: build_scripts commit must be 40 lowercase hex digits: {commit}")
    timeout = float(os.environ.get("FFMPEG_DEPENDS_FETCH_TIMEOUT", "120"))
    base_delay = float(os.environ.get("FFMPEG_DEPENDS_FETCH_DELAY", "5"))

    def run(*a):
        return subprocess.run(a, cwd=workdir, check=True, capture_output=True, text=True, timeout=timeout)

    run("git", "init", "-q", ".")
    last = ""
    for attempt in range(1, FETCH_ATTEMPTS + 1):
        try:
            run("git", "fetch", "-q", "--depth", "1", repo, commit)
            break
        except subprocess.TimeoutExpired:
            last = f"timed out after {timeout:g} s"
        except subprocess.CalledProcessError as e:
            last = (e.stderr or "").strip() or f"git exited with {e.returncode}"
        if attempt < FETCH_ATTEMPTS:
            time.sleep(base_delay * attempt * attempt)  # 1x, then 4x the base delay
    else:
        sys.exit(f"check-ffmpeg-depends: could not fetch {commit} from {repo} after {FETCH_ATTEMPTS} attempts: {last}")
    got = run("git", "rev-parse", "FETCH_HEAD").stdout.strip()
    if got != commit:
        sys.exit(f"check-ffmpeg-depends: fetched {got}, expected {commit}")
    run("git", "checkout", "-q", "FETCH_HEAD")
    return find_scripts_dir(workdir)


def parse_scripts(scripts_dir):
    """Returns (scripts by name, groups). A script is a list of the files of that name, each with its upstream
    repositories [(repo, pin)] for SCRIPT_REPO, SCRIPT_REPO2, ...; a sub-directory of scripts.d (45-fonts,
    47-vulkan, 45-x11) is a group that `ffbuild_depends` can name to depend on every script in it."""
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
            repos = []
            for n in re.findall(r'^SCRIPT_REPO(\d*)="?[^"\n]*', text, re.M):
                grab = lambda var: (re.search(rf'^{var}{n}="?([^"\n]*)', text, re.M) or [None, ""])[1]
                pin = grab("SCRIPT_COMMIT") or grab("SCRIPT_TAGFILTER") or (f"r{grab('SCRIPT_REV')}" if grab("SCRIPT_REV") else "")
                repos.append((norm_repo(grab("SCRIPT_REPO")), pin))
            scripts.setdefault(name, []).append(
                {"path": os.path.relpath(os.path.join(root, f), scripts_dir), "depends": depends, "flags": flags, "repos": repos}
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
    group_of = {member: group for group, members in groups.items() for member in members}

    def visit(name, why):
        if name in closure:
            return
        closure[name] = why
        # BtbN's final script depends on whole directories, so a script in one brings its siblings (libpng in
        # 50-aribb24, built before libaribb24 with no ffbuild_depends line saying so).
        if name in group_of:
            visit(group_of[name], why)
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
    not_linked = {(norm_repo(k) if "://" in k else k): v for k, v in registry.get("not_linked", {}).items()}
    used_not_linked = set()

    covered = linked_away = 0
    closure_repos = set()
    for name in sorted(closure):
        infos = scripts.get(name, [])
        repos = [r for i in infos for r, _ in i["repos"] if r]
        closure_repos.update(repos)
        if not infos and name in groups:
            continue  # a directory of scripts: its members are in the closure themselves
        if not infos:
            errors.append(f"{name}: needed by {closure[name]}, but there is no such script in BtbN's scripts.d")
            continue
        for repo in sorted(set(repos)):
            if repo in by_repo:
                covered += 1
            elif name in not_linked:
                used_not_linked.add(name)
                linked_away += 1
            elif repo in not_linked:
                used_not_linked.add(repo)
                linked_away += 1
            else:
                errors.append(
                    f"{name} ({repo}) is built for {closure[name]} but packaging/ffmpeg_libraries.json neither lists it "
                    f"nor says in \"not_linked\" (by script name or repository) why none of it is in the shipped libraries"
                )

    pins = {}
    scripts_of_repo = {}
    for sname, infos in scripts.items():
        for i in infos:
            for repo, pin in i["repos"]:
                pins.setdefault(repo, set()).add(pin)
                scripts_of_repo.setdefault(repo, set()).add(sname)
    for repo, es in sorted(by_repo.items()):
        if repo not in pins:
            errors.append(f"{es[0]['id']}: no BtbN script builds {repo} any more (renamed or dropped?)")
            continue
        for e in es:
            if e["pinned_commit"] not in pins[repo]:
                errors.append(
                    f"{e['id']}: the registry pins {e['pinned_commit'] or '(nothing)'} but BtbN's script builds "
                    f"{', '.join(sorted(p for p in pins[repo] if p))}"
                )
    for e in registry["supporting_libraries"]:
        repo = norm_repo(e["pinned_repo"])
        if repo in pins and not (scripts_of_repo[repo] & set(closure)):
            errors.append(
                f"{e['id']} is in supporting_libraries but no enabled option needs it (no script in the dependency "
                f"closure builds {repo}): remove it and licenses/ffmpeg/{e['text']}"
            )
    for key in sorted(set(not_linked) - used_not_linked):
        errors.append(f"not_linked names {key}, which no enabled option needs: remove it")

    if errors:
        print("check-ffmpeg-depends:\n  " + "\n  ".join(errors), file=sys.stderr)
        sys.exit(1)
    print(
        f"ffmpeg dependencies covered: {len(closure)} scripts in the closure, {covered} upstream projects in the registry, "
        f"{linked_away} recorded as not linked"
    )


if __name__ == "__main__":
    main()
