#!/usr/bin/env python3
"""Cross-SPEC normative lint for CarolinaDB (REVISAR-02 §13).

Checks, over md/SPEC-*.md and README.md:

  1. forbidden identity aliases          (IdcEpoch, authority_epoch: u64, idcs: Vec<(IdcId, u64)>, ...)
  2. duplicate schema definitions        (a schema block `Name {` outside its owning SPEC)
  3. dangling SPEC references            (SPEC-0NN that does not exist as a file)
  4. dangling section references         (SPEC-0NN §N where N is not a heading in that SPEC)
  5. dangling relative markdown links    ([text](path) whose path does not exist)
  6. product-name residue                (AstraDB / astra-* crates / `astra <cmd>` CLI; astra.* domains are allowed)
  7. formal gate references              (FM-1/FM-2/FM-3 must be defined in SPEC-010)

Exit code 0 = PASS, 1 = FAIL, 2 = invalid invocation. Stdlib only; deterministic output.
"""
from __future__ import annotations

import os
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MD_DIR = ROOT / "md"

# ---------------------------------------------------------------------------
# Configuration (normative ownership; see md/SPEC-OWNERSHIP.md)
# ---------------------------------------------------------------------------

FORBIDDEN_ALIASES = [
    # (regex, human label)
    (re.compile(r"\bIdcEpoch\b"), "IdcEpoch is not a normative type (SPEC-011 §2)"),
    (re.compile(r"\bauthority_epoch\s*:\s*u64\b"), "bare authority_epoch: u64"),
    (re.compile(r"\bidcs\s*:\s*Vec<\(\s*IdcId\s*,\s*u64\s*\)>"), "untyped idcs: Vec<(IdcId, u64)>"),
    (re.compile(r"\bVec<\(\s*IdcId\s*,\s*u64\s*\)>"), "untyped Vec<(IdcId, u64)>"),
    (re.compile(r"\bsource_epoch\s*:\s*u64\b"), "bare source_epoch: u64"),
    (re.compile(r"\bidc_epochs\s*\[\s*\]"), "idc_epochs[]"),
    (re.compile(r"\b(source|target)_idc_epochs\b"), "source/target_idc_epochs"),
]

# Lines that mention a forbidden alias only to prohibit it are allowed when they
# contain one of these markers.
PROHIBITION_MARKERS = re.compile(
    r"not a normative type|MUST NOT appear|forbidden|is not an alias|are forbidden|"
    r"Untyped .* are forbidden|no bare integer|No generic epoch|never a|not acceptable",
    re.IGNORECASE,
)

# Schema block owners: `Name {` (Rust or pseudo-record block) may be *defined*
# only in the owner. Explicit restatements are listed as allowed copies with a
# justification recorded in SPEC-OWNERSHIP.md.
SCHEMA_OWNERS = {
    "FinalReceiptV1": ("SPEC-012", set()),
    "FinalReceipt": ("SPEC-012", set()),
    "AcceptedResultV1": ("SPEC-012", set()),
    "RequestBindingV1": ("SPEC-012", set()),
    "RequestBinding": ("SPEC-012", set()),
    "RequestContentV1": ("SPEC-012", set()),
    "ClientReplyV1": ("SPEC-012", set()),
    "RequestKey": ("SPEC-012", {"SPEC-011"}),      # SPEC-011 §2 restates the taxonomy entry
    "IdcBinding": ("SPEC-011", {"SPEC-001"}),      # SPEC-001 §55 explanatory restatement
    "PlanRef": ("SPEC-011", set()),
    "AuthorityBinding": ("SPEC-011", set()),
    "CompiledBatch": ("SPEC-002", set()),
    "ProtocolMutation": ("SPEC-002", set()),
    "TxnStatusRecord": ("SPEC-002", set()),
    "MigrationRecord": ("SPEC-009", set()),
    "CloseCertificate": ("SPEC-009", set()),
    "OperationPlan": ("SPEC-004", set()),
    "ConsistencyCertificate": ("SPEC-004", set()),
    "ContractIR": ("SPEC-003", set()),
    "EffectIR": ("SPEC-003", set()),
    "InvariantIR": ("SPEC-003", set()),
    "ModuleIR": ("SPEC-003", set()),
    "SemanticCommitV1": ("SPEC-005", set()),
    "CausalContextV1": ("SPEC-005", set()),
    "SessionTokenV1": ("SPEC-005", set()),
    "HolderStateV1": ("SPEC-006", set()),
    "TransferTermsV1": ("SPEC-006", set()),
    "EscrowPlanV1": ("SPEC-006", set()),
    "DecisionCertificate": ("SPEC-008", set()),
    "PublicationCertificate": ("SPEC-008", set()),
    "CompletionCertificate": ("SPEC-008", set()),
    "TxnBegin": ("SPEC-008", set()),
    "SnapshotCut": ("SPEC-008", set()),
    "CertificationRecord": ("SPEC-007", set()),
    "PointReadToken": ("SPEC-007", set()),
    "RangeReadToken": ("SPEC-007", set()),
    "CatalogCommand": ("SPEC-011", set()),
    "AuthorityGrant": ("SPEC-011", set()),
    "QualificationManifest": ("SPEC-010", set()),
    "SnapshotManifestV1": ("SPEC-012", set()),
    "SignedPayloadV1": ("SPEC-013", set()),
    "SecurityPolicy": ("SPEC-013", set()),
    "CredentialRecord": ("SPEC-013", set()),
}
SCHEMA_DEF_RE = re.compile(r"^\s*(?:pub\s+)?(?:struct\s+)?([A-Z][A-Za-z0-9]*)(?:<[^>]*>)?\s*\{\s*(?://.*)?$")

# Product-name residue. The permanent protocol codename `astra` is allowed in
# hash domains (astra.xxx.vN), URI SANs (astra://), the wire magic ASTR and
# file extensions (.astr/.astj).
RESIDUE = [
    (re.compile(r"\bAstraDB\b"), "AstraDB used as product name"),
    (re.compile(r"\bastra-(core|storage|runtime|server|cli)\b"), "astra-* crate name"),
    (re.compile(r"(^|[\s`])astra (storage|journal|txn|checkpoint|explain|compile|check|plan|graph|rights|migration|qualify|simulate|replay|minimize|report)\b"), "`astra <cmd>` CLI name"),
    (re.compile(r"\bAstra (Storage|Semantic|Runtime|B\+Tree|storage)\b"), "Astra product phrase"),
]
RESIDUE_ALLOW = re.compile(r"historical|codename|survives only|permanent protocol", re.IGNORECASE)

SPEC_REF_RE = re.compile(r"\bSPEC-(\d{3})\b")
# SPEC-002 §180, SPEC-001 §§7–17, SPEC-002 §§44, 59–71, 100–109
SECTION_REF_RE = re.compile(r"\bSPEC-(\d{3})\s*(§§?)\s*([0-9][0-9.,–\-\s]*[0-9])")
LINK_RE = re.compile(r"\[[^\]]*\]\(([^)#]+)(#[^)]*)?\)")
HEADING_RE = re.compile(r"^#{1,6}\s*(?:Part\s+[IVXLC]+\s+—\s+)?(\d+)(?:\.(\d+))*[.\s—–-]")
FM_REF_RE = re.compile(r"\bFM-(\d)\b")


@dataclass
class Report:
    errors: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    checked_files: int = 0

    def err(self, path: Path, line: int, msg: str) -> None:
        self.errors.append(f"{path.relative_to(ROOT).as_posix()}:{line}: {msg}")

    def warn(self, path: Path, line: int, msg: str) -> None:
        self.warnings.append(f"{path.relative_to(ROOT).as_posix()}:{line}: {msg}")


def spec_id_of(path: Path) -> str | None:
    m = re.match(r"(SPEC-\d{3})\.md$", path.name)
    return m.group(1) if m else None


def load_headings(path: Path) -> set[int]:
    nums: set[int] = set()
    in_code = False
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.strip().startswith("```"):
            in_code = not in_code
            continue
        if in_code:
            continue
        m = HEADING_RE.match(line)
        if m:
            nums.add(int(m.group(1)))
    return nums


def expand_section_list(text: str) -> list[int]:
    out: list[int] = []
    for part in re.split(r"[,\s]+", text.strip()):
        if not part:
            continue
        rng = re.split(r"[–\-]", part)
        try:
            if len(rng) == 2:
                a, b = int(rng[0].split(".")[0]), int(rng[1].split(".")[0])
                out.extend([a, b])
            else:
                out.append(int(part.split(".")[0]))
        except ValueError:
            continue
    return out


def lint_file(path: Path, specs: dict[str, set[int]], report: Report) -> None:
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()
    me = spec_id_of(path)
    is_config_doc = path.name == "SPEC-OWNERSHIP.md"
    in_code = False
    report.checked_files += 1
    for no, line in enumerate(lines, start=1):
        stripped = line.strip()
        if stripped.startswith("```"):
            in_code = not in_code
            continue
        if is_config_doc:
            # the ownership matrix enumerates forbidden names on purpose; only check its links/refs
            for m in SPEC_REF_RE.finditer(line):
                ref = f"SPEC-{m.group(1)}"
                if ref not in specs:
                    report.err(path, no, f"dangling reference to {ref}")
            for m in LINK_RE.finditer(line):
                target = m.group(1).strip()
                if re.match(r"^[a-z]+://", target):
                    continue
                if not (path.parent / target).resolve().exists():
                    report.err(path, no, f"dangling link `{target}`")
            continue

        # 1. forbidden aliases
        for rx, label in FORBIDDEN_ALIASES:
            if rx.search(line) and not PROHIBITION_MARKERS.search(line):
                report.err(path, no, f"forbidden alias: {label}")

        # 2. duplicate schema definitions (only inside code/pseudo blocks)
        if in_code and me:
            m = SCHEMA_DEF_RE.match(line)
            if m:
                name = m.group(1)
                if name in SCHEMA_OWNERS:
                    owner, copies = SCHEMA_OWNERS[name]
                    if me != owner and me not in copies:
                        report.err(path, no, f"schema `{name}` defined outside owner {owner}")

        # 3./4. SPEC references and section references
        for m in SPEC_REF_RE.finditer(line):
            ref = f"SPEC-{m.group(1)}"
            if ref not in specs:
                report.err(path, no, f"dangling reference to {ref}")
        for m in SECTION_REF_RE.finditer(line):
            ref = f"SPEC-{m.group(1)}"
            if ref not in specs:
                continue
            for sec in expand_section_list(m.group(3)):
                if sec not in specs[ref]:
                    report.err(path, no, f"{ref} has no section §{sec}")

        # 5. relative links
        if not in_code:
            for m in LINK_RE.finditer(line):
                target = m.group(1).strip()
                if re.match(r"^[a-z]+://", target) or target.startswith("mailto:"):
                    continue
                candidate = (path.parent / target).resolve()
                if not candidate.exists():
                    report.err(path, no, f"dangling link `{target}`")

        # 6. product-name residue
        for rx, label in RESIDUE:
            if rx.search(line) and not RESIDUE_ALLOW.search(line):
                report.err(path, no, f"naming residue: {label}")

        # 7. FM gates
        for m in FM_REF_RE.finditer(line):
            gate = int(m.group(1))
            if gate not in (1, 2, 3):
                report.err(path, no, f"unknown formal gate FM-{gate}")


def check_fm_definitions(report: Report) -> None:
    spec010 = MD_DIR / "SPEC-010.md"
    text = spec010.read_text(encoding="utf-8")
    for gate in (1, 2, 3):
        if not re.search(rf"\|\s*FM-{gate}\s+—", text):
            report.err(spec010, 1, f"FM-{gate} gate is not defined in SPEC-010 §16")


def main(argv: list[str]) -> int:
    try:
        sys.stdout.reconfigure(encoding="utf-8")
    except Exception:  # pragma: no cover
        pass
    if len(argv) > 1 and argv[1] not in ("--verbose", "-v"):
        print(__doc__)
        return 2
    verbose = len(argv) > 1
    spec_files = sorted(p for p in MD_DIR.glob("SPEC-*.md") if spec_id_of(p))
    specs = {spec_id_of(p): load_headings(p) for p in spec_files if spec_id_of(p)}
    report = Report()
    for p in spec_files + [ROOT / "README.md", MD_DIR / "SPEC-OWNERSHIP.md"]:
        lint_file(p, specs, report)
    check_fm_definitions(report)

    for w in report.warnings:
        print(f"WARN {w}")
    for e in report.errors:
        print(f"FAIL {e}")
    status = "PASS" if not report.errors else "FAIL"
    print(f"spec_lint: {status} ({report.checked_files} files, {len(report.errors)} errors, {len(report.warnings)} warnings)")
    if verbose:
        for sid, heads in sorted(specs.items()):
            print(f"  {sid}: {len(heads)} numbered sections")
    return 0 if not report.errors else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
