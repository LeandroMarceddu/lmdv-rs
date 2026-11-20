#!/usr/bin/env python3
"""Generate docs/PARITY_AUDIT.md from the per-area audit tables in docs/audit/.

The audit tables are the status source: each row is
`| ID | P0..P2 | done|substitute|partial|missing|n/a | evidence |`.
`docs/PARITY_CHECKLIST.md` is generated from the same tables by
`scripts/gen-parity-checklist.py`, so both views agree.
"""

from __future__ import annotations

import re
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC = ROOT / "docs" / "BEHAVIOR_SPEC.md"
AUDIT = ROOT / "docs" / "audit"
OUT = ROOT / "docs" / "PARITY_AUDIT.md"

AREA_RE = re.compile(r"^### 2\.\d+ (.+?) \(([A-Z]+)\)\s*$")
ROW_RE = re.compile(r"^\| ([A-Z]+-\d+) \| (P[0-2]) \| (done|substitute|partial|missing|n/a) \| (.*?) \|$")

STATUSES = ("done", "substitute", "partial", "missing", "n/a")


def main() -> int:
    areas = []
    for line in SPEC.read_text(encoding="utf-8").splitlines():
        m = AREA_RE.match(line)
        if m:
            areas.append((m.group(2), m.group(1)))

    statuses: dict[str, str] = {}
    per_area: dict[str, Counter[str]] = defaultdict(Counter)
    p0: list[tuple[str, str, str]] = []
    for path in sorted(AUDIT.glob("*.md")):
        for line in path.read_text(encoding="utf-8").splitlines():
            m = ROW_RE.match(line.strip())
            if not m:
                continue
            rid, priority, status, evidence = m.groups()
            statuses[rid] = status
            per_area[rid.split("-")[0]][status] += 1
            if priority == "P0" and status in ("partial", "missing"):
                p0.append((rid, status, evidence))

    totals = Counter(statuses.values())
    out: list[str] = []
    out.append("# Parity audit")
    out.append("")
    out.append("Status of every behavior in [`BEHAVIOR_SPEC.md`](BEHAVIOR_SPEC.md), checked against the")
    out.append("checkout on 2026-11-20. The detailed tables and evidence live in [`audit/`](audit/):")
    out.append("")
    out.append("- [`audit/reading-outline-preview.md`](audit/reading-outline-preview.md) — R, O, V (63 rows)")
    out.append("- [`audit/tabs-windows-browser-palette.md`](audit/tabs-windows-browser-palette.md) — T, W, D, F, C (88)")
    out.append("- [`audit/editor-markdown-links.md`](audit/editor-markdown-links.md) — E, M, L (89)")
    out.append("- [`audit/export-system-a11y-i18n-help-persistence.md`](audit/export-system-a11y-i18n-help-persistence.md) — X, S, A, I, H, P (102)")
    out.append("")
    out.append("**Statuses:** `done` meets the acceptance criterion with a named test/scene/fixture;")
    out.append("`substitute` is an accepted different mechanism (linked from `FEATURE_PARITY` or an OI);")
    out.append("`partial` is implemented with a named clause missing; `missing` has no implementation;")
    out.append("`n/a` is platform-specific and not applicable on Linux.")
    out.append("")
    out.append("`PARITY_CHECKLIST.md` is generated from these tables, so the checkbox list always")
    out.append("matches this audit.")
    out.append("")
    out.append("## Totals")
    out.append("")
    out.append(
        f"**{sum(totals.values())} rows:** {totals['done']} done, {totals['substitute']} substitute, "
        f"{totals['partial']} partial, {totals['missing']} missing, {totals['n/a']} n/a."
    )
    out.append("")
    out.append("| Area | Rows | Done | Substitute | Partial | Missing | n/a |")
    out.append("|---|---:|---:|---:|---:|---:|---:|")
    for prefix, title in areas:
        counts = per_area[prefix]
        rows = sum(counts.values())
        out.append(
            f"| {title} | {rows} | {counts['done']} | {counts['substitute']} | "
            f"{counts['partial']} | {counts['missing']} | {counts['n/a']} |"
        )
    out.append(
        f"| **All** | **{sum(totals.values())}** | **{totals['done']}** | **{totals['substitute']}** | "
        f"**{totals['partial']}** | **{totals['missing']}** | **{totals['n/a']}** |"
    )
    out.append("")
    out.append(f"## P0 gaps ({len(p0)})")
    out.append("")
    out.append("Priority-0 rows that are not yet done. The full evidence and the precise missing")
    out.append("clause are in the area files.")
    out.append("")
    by_area: dict[str, list[tuple[str, str, str]]] = defaultdict(list)
    for rid, status, evidence in p0:
        by_area[rid.split("-")[0]].append((rid, status, evidence))
    for prefix, title in areas:
        items = by_area.get(prefix)
        if not items:
            continue
        out.append(f"### {prefix} — {title} ({len(items)})")
        out.append("")
        for rid, status, evidence in items:
            text = re.sub(r"\s+", " ", evidence).strip()
            if len(text) > 200:
                text = text[:197].rsplit(" ", 1)[0] + "…"
            out.append(f"- **{rid}** ({status}) — {text}")
        out.append("")
    out.append("## How this is maintained")
    out.append("")
    out.append("1. Re-run the audit for the affected areas after a batch (read the spec rows, the")
    out.append("   implementation and the tests; do not mark `done` without evidence).")
    out.append("2. Update the area table; run `scripts/gen-parity-audit.py` and")
    out.append("   `scripts/gen-parity-checklist.py`. `--check` enforces the checklist in the gate.")
    out.append("3. Add a row to `OPEN_ITEMS.md` for a gap that needs a decision or a substitute")
    out.append("   acceptance; the audit keeps the per-behavior status.")
    OUT.write_text("\n".join(out) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({sum(totals.values())} rows, {len(p0)} P0 gaps)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
