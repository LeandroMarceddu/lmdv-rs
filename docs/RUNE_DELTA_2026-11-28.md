# Rune delta and bump — 2026-11-28

Supersedes [`RUNE_DELTA_2026-11-25.md`](RUNE_DELTA_2026-11-25.md). Two
read-only probes compared the current `dev` head with our pin; this file is the
merged result and the record of the second port.

| | |
|---|---|
| Previous pin | `f4ec14ed32256ae0c36e73476825f9245437de5b` (2026-11-25) |
| New pin | `af6b1fdd9537a3f278f9c85e0f0893f1e773302a` (2026-11-28 06:28 UTC) |
| Commits | **230**, no merges; 1,272 files, +146,360 / −10,754 |
| Toolchain | unchanged: Rust 1.99.1, edition 2024, resolver 3 |
| `[patch.crates-io]` | unchanged: the same eight vendored crates |
| Patch count | four (0001, 0003, 0004, 0005); 0002 remains retired upstream |

## Patch verdicts

| Patch | Verdict | Work | Evidence |
|---|---|---|---|
| 0001 inline objects | re-port, reduced | 3–5 h | `Span` still has no `image`/`rise`/label; `El::pill_inline_image` rounds to device px and has no label. `IMAGE_BOX` moved to `1 << 58` because upstream took `1 << 59` for `PILL_BARE`; `layout_attr_key()` extended with image/rise so paragraph-key reuse cannot go stale |
| 0003 a11y labels | re-port, reduced | 0.5–1.5 h | Only two context hunks (`node.rs` `has_text` comment from `El::a11y_action`, `tests.rs` append point). Keep span labels; do not add a sibling `Role::Image` for span images |
| 0004 highlight box | unchanged | 0 h | `git apply --check` clean; upstream `range_boxes` still handles only `PILL_BOX` |
| 0005 Mermaid SVG | unchanged | 0–0.5 h | Zero commits touched its files since the pin; no serializer upstream. The old “applies only after 0001–0004” note was wrong and removed |

All four patches applied to a pristine formatted `af6b1fdd` reproduce
`vendor/rune` byte-for-byte. Patch proof: 7 inline-layout, 2 inline-scene,
2 a11y, 1 highlight, 9 Mermaid SVG tests pass.

## Memory fix (OI-199)

- **#2396** (`1e56b55d8`): `TextSystem::rest` now evicts to the *last* commit
  (the two-commit grace remains only while active); one `RuntimeCore::rest`
  compacts scenes, paint state, spare nodes, memo buckets, a11y state and
  `rune-kit` thread-local caches.
- **#2403** (`110572c64`): table cells shape into uncached owned layouts;
  `Inh` shrinks from ~440 B to ~84 B with a cold side table.
- **#2407** (`b67f8d509`): `malloc_trim(0)` runs off-thread at rest on
  Linux/glibc, guarded by a `mallinfo2` probe, coalesced with a 30 s cooldown.
  Opt out with `App::trim_heap_at_rest(false)` or `RUNE_MALLOC_TRIM=0`.

Verified with the issue's fixture (`developer-guide.md`, 951 KB) in a release
build, window-script close at 6 s:

| Backend | Loaded | After close | After rest |
|---|---:|---:|---:|
| Wayland | ~594 MB | ~594 MB | ~225 MB |
| X11 | ~607 MB | ~625 MB | ~258 MB |

A simple-file app sits at ~220 MB, so the large document returns to the normal
baseline after about 10 s idle. No app-side trim was shipped; the experimental
one was reverted. The active peak is unchanged (no imperative trim API was
added upstream).

## Other changes worth knowing

- `Tx::pick_files` and `FileHandle` enable multi-file Open (OI-002/T-32).
- `El::current` / `Current` closes the last `aria-current` gap for A-06.
- `WindowHandle::activate(token)` helps OI-117; `Tx::hovered()` and
  `PointerEvent::w/h` help V-02. None were bundled into this bump.
- No new `BrowserBack`/`BrowserForward` key, no multi-cursor API, no drag-cancel
  callback: C-01, O-14, E-29 and D-08 remain app-side or accepted gaps.
- `AppSpec::poll` can now run follow-up polls in the same frame (#2406); lmdv's
  `poll` is a thin wrapper over `WindowState::poll` and passes the gate.
- Expect golden/tree drift from ordered-list numbering (#2404), positioned text
  (#2405), `text_line` bounds (#2506), Linux grayscale outline gamma (#2727)
  and font-fallback changes. The full gate passed without rebaselining.
- Vendored patch *contents* changed for `winit`, `sctk-adwaita`,
  `accesskit_atspi_common` and `accesskit_macos`; the vendored copies were
  refreshed with the tree.

## Recommendation

Keep this pin until another upstream wave justifies a bump. If one comes, the
four patches are now cheap to re-port but still mandatory; the memory
acceptance gate is the five-row RSS table above.
