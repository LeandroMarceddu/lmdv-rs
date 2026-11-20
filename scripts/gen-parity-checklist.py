#!/usr/bin/env python3
"""Generate docs/PARITY_CHECKLIST.md from docs/BEHAVIOR_SPEC.md.

The behavior spec is the authoritative list of original behaviors. This script
turns its numbered rows into a checkbox checklist so implementation status can be
tracked without editing the spec. Re-run it when the spec changes.

Usage: python3 scripts/gen-parity-checklist.py [--check]
  --check  fail if the checklist is out of date (for CI/pre-commit)
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC = ROOT / "docs" / "BEHAVIOR_SPEC.md"
OUT = ROOT / "docs" / "PARITY_CHECKLIST.md"
AUDIT = ROOT / "docs" / "audit"

SECTION_RE = re.compile(r"^### 2\.(\d+) (.+?) \(([A-Z]+)\)\s*$")
ROW_RE = re.compile(r"^\|\s*([A-Z]+-\d+)\s*\|")
STATUS_RE = re.compile(r"^\|\s*([A-Z]+-\d+)\s*\|\s*P[0-2]\s*\|\s*(done|substitute|partial|missing|n/a)\s*\|")

# The audit tables are the status source: the checklist is generated from them.
def audit_statuses() -> dict[str, str]:
    statuses: dict[str, str] = {}
    if AUDIT.is_dir():
        for path in sorted(AUDIT.glob("*.md")):
            for line in path.read_text(encoding="utf-8").splitlines():
                m = STATUS_RE.match(line.strip())
                if m:
                    statuses[m.group(1)] = m.group(2)
    return statuses

MARK = {"done": "[x]", "substitute": "[x]", "partial": "[~]", "missing": "[ ]", "n/a": "[x]"}


def cells(line: str) -> list[str]:
    line = line.strip()
    if line.startswith("|"):
        line = line[1:]
    if line.endswith("|"):
        line = line[:-1]
    # Split on unescaped pipes only: a cell may contain `\|` (e.g. a Rust
    # `Option<A> | Option<B>` or a literal table pipe).
    out: list[str] = []
    cur: list[str] = []
    escaped = False
    for ch in line:
        if escaped:
            cur.append(ch)
            escaped = False
        elif ch == "\\":
            cur.append(ch)
            escaped = True
        elif ch == "|":
            out.append("".join(cur).strip())
            cur = []
        else:
            cur.append(ch)
    out.append("".join(cur).strip())
    return [c.replace("\\|", "|") for c in out]


def main() -> int:
    text = SPEC.read_text(encoding="utf-8")
    lines = text.splitlines()

    sections: list[tuple[str, str, list[tuple[str, str, str, str]]]] = []
    current: tuple[str, str, list[tuple[str, str, str, str]]] | None = None

    for line in lines:
        m = SECTION_RE.match(line)
        if m:
            current = (m.group(3), m.group(2), [])
            sections.append(current)
            continue
        if line.startswith("## ") and not line.startswith("### "):
            current = None
            continue
        if current is None:
            continue
        m = ROW_RE.match(line)
        if not m:
            continue
        row = cells(line)
        if len(row) < 7:
            continue
        rid, feature, _behavior, _refs, priority, _acceptance, fixture = row[:7]
        # Strip markdown emphasis/backticks from the feature for a readable line.
        feature = feature.replace("**", "").replace("`", "")
        sections[-1][2].append((rid, feature, priority, fixture))

    rows = sum(len(s[2]) for s in sections)
    if rows == 0:
        print("error: no rows parsed from", SPEC, file=sys.stderr)
        return 2

    out: list[str] = []
    out.append("# Parity checklist")
    out.append("")
    out.append("Generated from [`BEHAVIOR_SPEC.md`](BEHAVIOR_SPEC.md) and the status tables in")
    out.append("[`audit/`](audit/) by `scripts/gen-parity-checklist.py`. Do not edit by hand:")
    out.append("update the audit, then regenerate. Evidence is cited per row in the audit.")
    out.append("")
    out.append(f"**Total behaviors:** {rows}")
    out.append("")
    out.append("## How to use")
    out.append("")
    out.append("- `[ ]` missing, `[~]` partial, `[x]` done or accepted substitute/gap.")
    out.append("- Status and evidence live in `docs/audit/*.md`; this file is generated from them.")
    out.append("  Do not edit it by hand: run `scripts/gen-parity-checklist.py` after an audit.")
    out.append("- A row is done only when its acceptance criterion has a test, golden, tree")
    out.append("  assertion or manual note; the tag after each row names its status.")
    out.append("")
    out.append("## Counts")
    out.append("")
    out.append("| Area | Prefix | Rows | P0 | P1 | P2 |")
    out.append("|---|---|---:|---:|---:|---:|")
    for prefix, title, items in sections:
        p0 = sum(1 for _, _, p, _ in items if p.startswith("P0"))
        p1 = sum(1 for _, _, p, _ in items if p.startswith("P1"))
        p2 = sum(1 for _, _, p, _ in items if p.startswith("P2"))
        out.append(f"| {title} | {prefix} | {len(items)} | {p0} | {p1} | {p2} |")
    out.append("")
    statuses = audit_statuses()
    out.append("### Audit status")
    out.append("")
    out.append("| Area | Rows | Done | Substitute | Partial | Missing | n/a |")
    out.append("|---|---:|---:|---:|---:|---:|---:|")
    totals = {key: 0 for key in ("done", "substitute", "partial", "missing", "n/a")}
    for prefix, title, items in sections:
        counts = {key: 0 for key in totals}
        for rid, _, _, _ in items:
            status = statuses.get(rid)
            if status in counts:
                counts[status] += 1
        for key in totals:
            totals[key] += counts[key]
        out.append(
            f"| {title} | {len(items)} | {counts['done']} | {counts['substitute']} | "
            f"{counts['partial']} | {counts['missing']} | {counts['n/a']} |"
        )
    out.append(
        f"| **All** | **{rows}** | **{totals['done']}** | **{totals['substitute']}** | "
        f"**{totals['partial']}** | **{totals['missing']}** | **{totals['n/a']}** |"
    )
    out.append("")
    for prefix, title, items in sections:
        out.append(f"## {prefix} — {title} ({len(items)})")
        out.append("")
        for rid, feature, priority, fixture in items:
            fixture = f" · {fixture}" if fixture else ""
            status = statuses.get(rid)
            mark = MARK.get(status or "", "[ ]")
            tag = f" — {status}" if status else ""
            out.append(f"- {mark} **{rid}** {feature} ({priority}{fixture}){tag}")
        out.append("")

    rendered = "\n".join(out) + "\n"
    if "--check" in sys.argv:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != rendered:
            print(f"{OUT} is out of date; run {Path(__file__).name}", file=sys.stderr)
            return 1
        print(f"{OUT} is up to date ({rows} rows)")
        return 0

    OUT.write_text(rendered, encoding="utf-8")
    print(f"wrote {OUT} ({rows} rows in {len(sections)} sections)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
