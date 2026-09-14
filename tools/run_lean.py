#!/usr/bin/env python3
"""Check the Lean proofs and audit the axioms every theorem depends on.

`lake build` alone is not a verdict: Lean reports `sorry` as a warning, not an error, so a file
full of holes still "builds". This runner therefore fails on three separate grounds — a non-zero
build, the string `sorry` anywhere in the sources, and any theorem whose axiom set contains
`sorryAx`. It also refuses a silent pass: `Carolina/Check.lean` must actually print an axiom line
for every theorem listed there.

The toolchain is pinned by `lean/lean-toolchain`; install it with elan
(https://elan.lean-lang.org). Unlike `tools/run_tlc.py`, nothing is downloaded here.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LEAN_DIR = ROOT / "lean"
AXIOM_LINE = re.compile(r"'([^']+)' depends on axioms: \[([^\]]*)\]")
# Lean's own foundations. Anything else — above all `sorryAx` — means the proof is not closed.
ALLOWED_AXIOMS = {"propext", "Quot.sound", "Classical.choice"}


def lake() -> str:
    candidate = Path.home() / ".elan" / "bin" / "lake"
    return str(candidate) if candidate.exists() else "lake"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clean", action="store_true", help="rebuild from scratch")
    args = parser.parse_args()

    if not LEAN_DIR.is_dir():
        print("run_lean: no lean/ directory", file=sys.stderr)
        return 2

    holes = []
    for source in sorted(LEAN_DIR.rglob("*.lean")):
        text = source.read_text(encoding="utf-8")
        for number, line in enumerate(text.splitlines(), 1):
            # the word, not a substring of `sorryAx` inside this file's own prose
            if re.search(r"\bsorry\b", line) and "run_lean" not in line and not line.lstrip().startswith("--"):
                if source.name != "Check.lean":
                    holes.append(f"{source.relative_to(ROOT)}:{number}: {line.strip()}")
    if holes:
        print("run_lean: FAIL — `sorry` in the sources:", file=sys.stderr)
        for hole in holes:
            print(f"  {hole}", file=sys.stderr)
        return 1

    env = dict(os.environ)
    env["PATH"] = f"{Path.home() / '.elan' / 'bin'}{os.pathsep}{env.get('PATH', '')}"
    if args.clean:
        subprocess.run([lake(), "clean"], cwd=LEAN_DIR, env=env, check=False)
    try:
        done = subprocess.run(
            [lake(), "build"], cwd=LEAN_DIR, env=env, capture_output=True, text=True
        )
    except FileNotFoundError:
        print("run_lean: lake not found; install the pinned toolchain with elan", file=sys.stderr)
        return 2
    output = done.stdout + done.stderr
    if done.returncode != 0:
        print(output, file=sys.stderr)
        print("run_lean: FAIL — the proofs do not build", file=sys.stderr)
        return 1

    audited = AXIOM_LINE.findall(output)
    if not audited:
        print(output, file=sys.stderr)
        print("run_lean: FAIL — no axiom audit was printed; did Check.lean stop listing?", file=sys.stderr)
        return 1

    bad = []
    for name, axioms in audited:
        used = {a.strip() for a in axioms.split(",") if a.strip()}
        extra = used - ALLOWED_AXIOMS
        if extra:
            bad.append(f"{name} depends on {sorted(extra)}")
    if bad:
        print("run_lean: FAIL — unexpected axioms:", file=sys.stderr)
        for entry in bad:
            print(f"  {entry}", file=sys.stderr)
        return 1

    for name, axioms in audited:
        print(f"  {name}: [{axioms}]")
    print(f"run_lean: PASS ({len(audited)} theorems, no sorry, axioms within {sorted(ALLOWED_AXIOMS)})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
