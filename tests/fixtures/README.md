# Fixtures

These are reference samples, adapted for lmdv (MIT; personal project).

- `test-features.md` — the primary reading/rendering fixture.
- `commonmark-math.md` — GitHub math delimiter cases.
- `nested/`, `img/` — relative links and images.
- `stress/developer-guide.md` — a real 951 KiB, 10.5k-line, table-heavy
  developer guide (423 headings, 51 tables, 186 code fences) used to reproduce
  scroll latency and frame-cost reports. Do not trim it: the point is its size.

Additional fixture families (F-CORE, F-TABLE, F-EDIT, F-TABS, F-TREE,
F-SYSTEM, F-EXPORT, F-PALETTE, F-ERRORS) are specified in
`docs/BEHAVIOR_SPEC.md` §9 and will be added in P2. Do not edit these fixtures
to make an implementation pass; add a new fixture and document the behavior.
