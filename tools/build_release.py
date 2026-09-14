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
import os
import shutil
import subprocess
import sys
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


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verify", action="store_true",
                        help="build twice from two differently named copies and compare")
    args = parser.parse_args()

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
