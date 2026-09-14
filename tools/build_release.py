#!/usr/bin/env python3
"""Build the release binaries reproducibly, and prove it.

The release profile keeps `debug = 1`, so without help rustc records the absolute path of the
source tree, of the cargo registry and of the toolchain sysroot inside every artifact. Two
checkouts in different directories then produce different bytes, which was measured on this tree
before this script existed. `--remap-path-prefix` rewrites those three roots to fixed names, so the
artifact depends on the source and the toolchain and nothing else about the machine.

`--verify` is the evidence, not the claim: it copies the committed tree into two differently named
directories, builds both, and compares the digests. A reproducibility claim that has not been
measured is worth nothing.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BINARIES = ("carolina", "carolina-node")


def digest(path: Path) -> str:
    sha256 = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            sha256.update(chunk)
    return sha256.hexdigest()


def sysroot() -> str | None:
    try:
        done = subprocess.run(["rustc", "--print", "sysroot"], capture_output=True, text=True)
        return done.stdout.strip() or None
    except FileNotFoundError:
        return None


def build_env(source: Path) -> dict[str, str]:
    env = dict(os.environ)
    cargo_home = Path(env.get("CARGO_HOME") or (Path.home() / ".cargo"))
    remaps = [f"--remap-path-prefix={source}=/carolinadb",
              f"--remap-path-prefix={cargo_home}=/cargo"]
    root = sysroot()
    if root:
        remaps.append(f"--remap-path-prefix={root}=/rust")
    env["RUSTFLAGS"] = " ".join(remaps + ([env["RUSTFLAGS"]] if env.get("RUSTFLAGS") else []))
    # incremental output embeds per-build state; a release build must not use it
    env["CARGO_INCREMENTAL"] = "0"
    env.pop("CARGO_TARGET_DIR", None)
    return env


def build(source: Path) -> dict[str, str]:
    done = subprocess.run(
        ["cargo", "build", "--release", "--locked"],
        cwd=source, env=build_env(source), capture_output=True, text=True,
    )
    if done.returncode != 0:
        print(done.stdout + done.stderr, file=sys.stderr)
        raise SystemExit("build_release: the release build failed")
    out = {}
    for name in BINARIES:
        for candidate in (source / "target" / "release" / name,
                          source / "target" / "release" / f"{name}.exe"):
            if candidate.is_file():
                out[name] = digest(candidate)
                break
        else:
            raise SystemExit(f"build_release: {name} was not produced")
    return out


VERSION = "0.1.0"
# Everything a recipient needs to know what they have and what it is not.
PAPERS = ("LICENSE", "README.md", "CHANGELOG.md", "SECURITY.md", "sbom.json",
          "docs/BUILD.md", "docs/STATUS.md")


def target_triple() -> str:
    done = subprocess.run(["rustc", "-vV"], capture_output=True, text=True)
    for line in done.stdout.splitlines():
        if line.startswith("host: "):
            return line[len("host: "):].strip()
    return "unknown"


def toolchain() -> str:
    done = subprocess.run(["rustc", "--version"], capture_output=True, text=True)
    return done.stdout.strip()


def commit() -> str:
    done = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True)
    return done.stdout.strip() or "unknown"


def modified() -> list[str]:
    """Tracked files that differ from HEAD.

    Untracked files are reported but do not block: cargo does not compile what it does not know
    about, and if something did reference such a file, the tracked file doing the referencing
    would itself be modified and caught here.
    """
    done = subprocess.run(["git", "status", "--porcelain", "--untracked-files=no"],
                          cwd=ROOT, capture_output=True, text=True)
    return [line for line in done.stdout.splitlines() if line.strip()]


def untracked() -> list[str]:
    done = subprocess.run(["git", "ls-files", "--others", "--exclude-standard"],
                          cwd=ROOT, capture_output=True, text=True)
    return [line for line in done.stdout.splitlines() if line.strip()]


def package(out_dir: Path) -> int:
    """Build, then assemble the archive a release actually ships, plus its digests.

    The digest list is written in `sha256sum -c` format so it can be signed detached, by whatever
    key the owner chooses. Nothing here signs anything: this project has no release key, and
    pretending otherwise would be the worst kind of security theatre.
    """
    triple = target_triple()
    changed = modified()
    if changed:
        print("build_release: refusing to package — these tracked files differ from HEAD:",
              file=sys.stderr)
        for line in changed:
            print(f"  {line}", file=sys.stderr)
        print("an artifact whose commit cannot be named is not a release", file=sys.stderr)
        return 2
    loose = untracked()
    if loose:
        shown = ", ".join(loose[:5]) + (" ..." if len(loose) > 5 else "")
        print(f"build_release: note — {len(loose)} untracked file(s) present, not packaged: {shown}")
    digests = build(ROOT)
    out_dir.mkdir(parents=True, exist_ok=True)
    stem = f"carolinadb-{VERSION}-{triple}"
    archive = out_dir / f"{stem}.tar.gz"

    release = ROOT / "target" / "release"
    members: list[tuple[Path, str]] = []
    for name in BINARIES:
        for candidate in (release / name, release / f"{name}.exe"):
            if candidate.is_file():
                members.append((candidate, f"{stem}/bin/{candidate.name}"))
                break
    for paper in PAPERS:
        source = ROOT / paper
        if source.is_file():
            members.append((source, f"{stem}/{Path(paper).name}"))

    # deterministic archive: sorted, no owner names, no mtimes
    with tarfile.open(archive, "w:gz", compresslevel=9, format=tarfile.GNU_FORMAT) as tar:
        for source, arcname in sorted(members, key=lambda m: m[1]):
            info = tar.gettarinfo(str(source), arcname=arcname)
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            info.mtime = 0
            with source.open("rb") as stream:
                tar.addfile(info, stream)

    manifest = {
        "name": "carolinadb",
        "version": VERSION,
        "commit": commit(),
        "target": triple,
        "toolchain": toolchain(),
        "reproducible": "verified by tools/build_release.py --verify",
        "signature": "none — this project has no release key (owner decision)",
        "scope": "research prototype; DEV_LOCAL plaintext loopback profile only; QI-SECURITY NOT_RUN",
        "artifacts": {archive.name: digest(archive)},
        "binaries": digests,
    }
    (out_dir / "MANIFEST.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + chr(10), encoding="utf-8")
    sums = "".join(f"{digest(p)}  {p.name}" + chr(10)
                   for p in sorted(out_dir.iterdir()) if p.name != "SHA256SUMS")
    (out_dir / "SHA256SUMS").write_text(sums, encoding="utf-8")
    print(f"  {archive.name}: {manifest['artifacts'][archive.name]}")
    for name, value in sorted(digests.items()):
        print(f"  {name}: {value}")
    print(f"build_release: packaged {archive} (unsigned)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verify", action="store_true",
                        help="build twice from two differently named copies and compare")
    parser.add_argument("--package", action="store_true",
                        help="build and assemble the release archive, digests and manifest")
    parser.add_argument("--out", default=str(ROOT / "release"))
    args = parser.parse_args()

    if args.package:
        return package(Path(args.out))

    if not args.verify:
        for name, value in sorted(build(ROOT).items()):
            print(f"  {name}: {value}")
        print("build_release: built with path remapping")
        return 0

    export = subprocess.run(["git", "archive", "HEAD"], cwd=ROOT, capture_output=True)
    if export.returncode != 0:
        print("build_release: `git archive HEAD` failed; commit the tree first", file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory(prefix="carolina-rb-") as scratch:
        base = Path(scratch)
        first, second = base / "alpha", base / "a-much-longer-second-path"
        first.mkdir()
        archive = base / "tree.tar"
        archive.write_bytes(export.stdout)
        shutil.unpack_archive(str(archive), str(first), format="tar")
        shutil.copytree(first, second)
        one, two = build(first), build(second)

    ok = True
    for name in BINARIES:
        if one[name] == two[name]:
            print(f"  {name}: IDENTICAL {one[name]}")
        else:
            ok = False
            print(f"  {name}: DIFFERS\n    {one[name]}\n    {two[name]}")
    if not ok:
        print("build_release: FAIL — the release build is not reproducible across paths",
              file=sys.stderr)
        return 1
    print("build_release: PASS (identical artifacts from two differently named source paths)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
