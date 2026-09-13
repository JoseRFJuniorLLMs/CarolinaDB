#!/usr/bin/env python3
"""Run the bounded TLA+ models with a pinned, checksum-verified TLC."""

from __future__ import annotations

import argparse
import hashlib
import subprocess
import sys
import urllib.request
from pathlib import Path


TLC_VERSION = "1.8.0"
TLC_URL = f"https://github.com/tlaplus/tlaplus/releases/download/v{TLC_VERSION}/tla2tools.jar"
TLC_SHA256 = "db131ddb48e7004d823bef4493df7b35694babe37505b9d9fa5685e7a331f1f1"
MODELS = ("FM1_Escrow", "FM2_Decision", "FM3_Migration")


def digest(path: Path) -> str:
    sha256 = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            sha256.update(chunk)
    return sha256.hexdigest()


def obtain_jar(path: Path) -> None:
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        print(f"Downloading TLC {TLC_VERSION} from {TLC_URL}", flush=True)
        with urllib.request.urlopen(TLC_URL, timeout=60) as response:
            path.write_bytes(response.read())
    actual = digest(path)
    if actual != TLC_SHA256:
        raise SystemExit(
            f"TLC checksum mismatch for {path}: expected {TLC_SHA256}, got {actual}"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--jar",
        type=Path,
        default=Path("target/tools") / f"tla2tools-{TLC_VERSION}.jar",
        help="path to the pinned TLC jar (downloaded when absent)",
    )
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    jar = args.jar if args.jar.is_absolute() else root / args.jar
    obtain_jar(jar)

    for model in MODELS:
        print(f"\n=== TLC {model} ===", flush=True)
        command = [
            "java",
            "-XX:+UseParallelGC",
            "-jar",
            str(jar),
            "-workers",
            "1",
            "-cleanup",
            "-metadir",
            str(root / "target" / "tlc" / model),
            "-config",
            str(root / "models" / f"{model}.cfg"),
            str(root / "models" / f"{model}.tla"),
        ]
        result = subprocess.run(command, cwd=root, check=False)
        if result.returncode != 0:
            return result.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
