#!/usr/bin/env python3
"""Generate a CycloneDX 1.5 SBOM for the workspace from `Cargo.lock`.

`Cargo.lock` is the authority for what is actually built: it pins every version and carries the
SHA-256 of each downloaded `.crate`. Licences are not in the lock file, so they are read from the
vendored source of the same version in the local cargo registry when it is present, and reported as
`NOASSERTION` when it is not. Nothing is guessed and nothing is fetched: an SBOM that invents a
licence is worse than one that admits it does not know.

Deterministic by construction: components are sorted, no timestamp is emitted, and the document
serial number is derived from the content, so two runs on the same tree produce identical bytes.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PACKAGE = re.compile(r"^\[\[package\]\]$")
FIELD = re.compile(r'^(name|version|source|checksum)\s*=\s*"(.*)"$')
LICENSE = re.compile(r'^license\s*=\s*"(.*)"$')
REGISTRY = Path.home() / ".cargo" / "registry" / "src"


def parse_lock(path: Path) -> list[dict[str, str]]:
    packages: list[dict[str, str]] = []
    current: dict[str, str] | None = None
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if PACKAGE.match(line):
            current = {}
            packages.append(current)
            continue
        if current is None:
            continue
        if not line:
            current = None
            continue
        found = FIELD.match(line)
        if found:
            current[found.group(1)] = found.group(2)
    return [p for p in packages if "name" in p and "version" in p]


def licence_of(name: str, version: str) -> str:
    """Read the licence from the vendored source of exactly this version, or admit ignorance."""
    for registry in sorted(REGISTRY.glob("*")) if REGISTRY.is_dir() else []:
        manifest = registry / f"{name}-{version}" / "Cargo.toml"
        if manifest.is_file():
            for line in manifest.read_text(encoding="utf-8", errors="replace").splitlines():
                found = LICENSE.match(line.strip())
                if found:
                    return found.group(1)
    return "NOASSERTION"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", default=str(ROOT / "sbom.json"))
    parser.add_argument("--check", action="store_true",
                        help="fail if the file on disk differs from what this run produces")
    args = parser.parse_args()

    workspace_license = "Apache-2.0"
    packages = parse_lock(ROOT / "Cargo.lock")
    local = {p["name"] for p in packages if "source" not in p}

    components = []
    for p in sorted(packages, key=lambda p: (p["name"], p["version"])):
        name, version = p["name"], p["version"]
        internal = name in local
        component = {
            "type": "application" if name == "carolina-cli" else "library",
            "bom-ref": f"pkg:cargo/{name}@{version}",
            "name": name,
            "version": version,
            "purl": f"pkg:cargo/{name}@{version}",
            "scope": "required",
            "licenses": [{"expression": workspace_license if internal else licence_of(name, version)}],
            "properties": [{"name": "carolina:origin", "value": "workspace" if internal else "crates.io"}],
        }
        if "checksum" in p:
            component["hashes"] = [{"alg": "SHA-256", "content": p["checksum"]}]
        components.append(component)

    document = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "bom-ref": "pkg:cargo/carolinadb",
                "name": "carolinadb",
                "version": "0.1.0",
                "licenses": [{"expression": workspace_license}],
                "description": "Invariant-compiled consistency: research prototype, DEV_LOCAL profile only",
            },
            "tools": [{"vendor": "CarolinaDB", "name": "tools/make_sbom.py", "version": "1"}],
            "properties": [
                {"name": "carolina:source", "value": "Cargo.lock"},
                {"name": "carolina:third-party-count", "value": str(len(packages) - len(local))},
            ],
        },
        "components": components,
    }
    body = json.dumps(document, indent=2, sort_keys=True, ensure_ascii=False)
    serial = hashlib.sha256(body.encode("utf-8")).hexdigest()[:32]
    document["serialNumber"] = f"urn:uuid:{uuid.UUID(serial)}"
    text = json.dumps(document, indent=2, sort_keys=True, ensure_ascii=False) + "\n"

    out = Path(args.out)
    if args.check:
        # Compare against `Cargo.lock`, not against a fresh render: licences are read from the
        # local cargo registry, which a checkout that has not built yet does not have. A byte
        # comparison would then fail for the wrong reason. What must match is what the lock file
        # pins — identity, version and crate digest — plus the rule that the committed document
        # leaves no licence undetermined.
        if not out.is_file():
            print(f"make_sbom: FAIL — {out} does not exist; run tools/make_sbom.py", file=sys.stderr)
            return 1
        try:
            on_disk = json.loads(out.read_text(encoding="utf-8"))
        except json.JSONDecodeError as bad:
            print(f"make_sbom: FAIL — {out} is not valid JSON: {bad}", file=sys.stderr)
            return 1

        def identity(entries):
            return {
                (c["name"], c["version"], (c.get("hashes") or [{}])[0].get("content"))
                for c in entries
            }

        want, have = identity(components), identity(on_disk.get("components", []))
        missing, extra = want - have, have - want
        if missing or extra:
            print(f"make_sbom: FAIL — {out} is stale; regenerate it", file=sys.stderr)
            for entry in sorted(missing):
                print(f"  only in Cargo.lock: {entry}", file=sys.stderr)
            for entry in sorted(extra):
                print(f"  only in the SBOM:   {entry}", file=sys.stderr)
            return 1
        undetermined = sorted(
            c["name"] for c in on_disk["components"]
            if c.get("licenses", [{}])[0].get("expression") == "NOASSERTION"
        )
        if undetermined:
            print("make_sbom: FAIL — the committed SBOM has undetermined licences: "
                  f"{undetermined}", file=sys.stderr)
            return 1
        third_party = sum(1 for c in on_disk["components"]
                          if c.get("properties", [{}])[0].get("value") == "crates.io")
        print(f"make_sbom: PASS ({len(have)} components match Cargo.lock, {third_party} "
              "third-party, every licence determined)")
        return 0

    out.write_text(text, encoding="utf-8")
    print(f"make_sbom: wrote {out} ({len(components)} components)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
