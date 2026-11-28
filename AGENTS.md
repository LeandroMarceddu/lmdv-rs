# lmdv-rs — Agent Operating Manual

This is the first file every agent (and every future, context-pruned orchestrator) reads. It is the durable contract for planning, delegation, validation and handoff.

**Project:** a native Linux-first Markdown viewer/editor — working name `lmdv` — in Rust with [Rune](https://github.com/anomalyco/rune).

**Status:** implementation in progress: P0–P5 complete; P6/P7 in progress. Do not start a new phase without the user's explicit go-ahead. See `docs/STATUS.md`.

---

## 1. Non-negotiable constraints

1. **Native, not a webview.** Rune's renderer/text/layout. No HTML/JS application shell. Math/PDF may use offline Rust engines; they must not become a browser dependency.
2. **Linux first, not Linux only.** Wayland and X11 on CachyOS/Arch are the primary target. Windows must keep compiling and launching cleanly; macOS may follow later. Full Windows/macOS parity is not required for v1.
3. **Completeness over shortcuts.** Do not silently drop a behavior. Every row in `docs/BEHAVIOR_SPEC.md` is in scope; a substitute/gap needs the user's explicit acceptance and a row in `docs/FEATURE_PARITY.md`.
4. **Personal project; licensing is not a constraint.** Port/copy freely; keep Rune provenance in `vendor/rune/`.
5. **The behavior spec is the contract.** Cite `docs/BEHAVIOR_SPEC.md` rows and the `file:line` references they record. README/CHANGELOG state intent; the implementation states edge cases.
6. **Rune is pinned and vendored.** Rev in `docs/RUNE_PIN.md`; the full workspace lives in `vendor/rune/`. There are currently no local patches; upstream #2743 provides the formerly patched features.
7. **Do not set `CARGO_TARGET_DIR`.** Build in the repository `target/`.
8. **No telemetry, no runtime network calls** except user-initiated external links/opens.
9. **Never delete/rewrite user files outside the repository.** No “cleanup” of unrelated files.
10. **No destructive git operations.** No force-push, no history rewrite, unless the user explicitly asks for one.

---

## 2. Read order for a fresh agent

1. `AGENTS.md`
2. `docs/STATUS.md` — exact current phase, failing commands, next 3 tasks
3. `PLAN.md` — phase P0–P7 and the current gate
4. `docs/FEATURE_PARITY.md` — how each area maps to Rune + gaps
5. `docs/PARITY_CHECKLIST.md` — the 342-row status list
6. `docs/OPEN_ITEMS.md` — worker-reported limitations and follow-ups, with phase/owner
7. `docs/ARCHITECTURE.md` — condensed authoritative design
8. `docs/DECISIONS.md` — D1–D12 and ADRs
9. `docs/TESTING.md` — fixtures, scenes, gates
10. `docs/RISKS_AND_SPIKES.md`
11. `docs/LINUX_INTEGRATION.md`, `docs/RUNE_CAPABILITY_MAP.md`, `docs/RUNE_PIN.md`, `docs/BEHAVIOR_SPEC.md` as needed
12. `docs/ARCHITECTURE_DETAIL.md` only for the specific task's types/rationale
13. `vendor/rune/` only for the specific Rune `file:line` in the task capsule

If a document contradicts the checkout, the checkout wins for facts; update the doc. If it contradicts the user, the user wins.

---

## 3. Repository map

```text
lmdv-rs/
  AGENTS.md  PLAN.md  README.md
  docs/
    STATUS.md                living handoff
    BEHAVIOR_SPEC.md         authoritative 342-row app behavior
    PARITY_CHECKLIST.md      generated checkboxes
    FEATURE_PARITY.md        Rune mapping + phase/gaps
    ARCHITECTURE.md          condensed design
    ARCHITECTURE_DETAIL.md   long-form design
    RUNE_CAPABILITY_MAP.md   verified Rune APIs/gaps
    RUNE_PIN.md              Rune pin read by the vendor scripts
    DECISIONS.md             D1–D12 + ADR log
    TESTING.md               fixtures/scenes/gates
    RISKS_AND_SPIKES.md
    LINUX_INTEGRATION.md     MIME/desktop/socket/packaging
  scripts/
    install-md-handler.sh    user-level .md handler (tested)
    uninstall-md-handler.sh
    gen-parity-checklist.py
    vendor-rune.sh           (planned P0)
    apply-rune-patches.sh    (planned P0)
    check-vendor.sh          (planned P0)
    run-gates.sh             (planned P0)
  vendor/rune/               full pinned Rune workspace + patches/ (P0)
  crates/
    lmdv/ lmdv-app/ lmdv-core/ lmdv-io/ lmdv-md/ lmdv-render/
    lmdv-math/ lmdv-edit/ lmdv-export/ lmdv-i18n/ lmdv-testkit/
  tests/                     fixtures, golden, trees, scripts
  assets/fonts/ assets/icons/
  locales/
```

Crate responsibilities and the dependency DAG are in `docs/ARCHITECTURE.md` §3.

---

## 4. Build, test and run

Toolchain: Rust **1.99.1** (pinned by Rune). Linux deps:

```bash
sudo pacman -S --needed libxkbcommon libxkbcommon-x11 wayland mesa-vulkan-drivers
rustup toolchain install 1.99.1
rustup component add rustfmt clippy --toolchain 1.99.1
```

Once the workspace exists:

```bash
cargo build --workspace
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
scripts/fmt-check.sh

cargo run -p lmdv -- path/to/file.md
cargo run -p lmdv -- --new-window path/to/file.md --line 40

cargo run -p lmdv -- --snap /tmp/opencode/snap.png --scene viewer-light --size 1280x900 --scale 2
cargo run -p lmdv -- --tree /tmp/opencode/tree.json
cargo run -p lmdv -- --script tests/scripts/reading.json --frames 60 --out /tmp/opencode/frames
```

Rules:

- Run the narrowest meaningful check while iterating; the full gate before reporting done.
- **Workers run targeted tests only.** Use `cargo test -p <crate> <filter>` for the tests the change touches. Do not run `cargo test --workspace`, `scripts/run-gates.sh`, or a whole-crate suite; those are orchestrator-only because they freeze the user's machine. The orchestrator runs the full gate. `scripts/test-rune-patches.sh` builds `vendor/rune/target`, so run it only after a vendor bump or patch edit.
- Headless `--snap` is the visual gate; compare with tolerance, not exact pixels.
- `--tree` is the structural/a11y gate; assert roles, labels, focus, selection.
- GUI-only tests require `RUNE_GUI_TESTS=1` and a desktop session; not the default gate.
- A flaky test is a bug; record it in `docs/STATUS.md`; do not retry until green.
- **No `std::fs` in `lmdv-app`.** All I/O goes through `lmdv-io` jobs.

---

## 5. Orchestration model

The orchestrator is not the default implementer. It freezes interfaces, owns shared files, integrates and validates. Workers own bounded crates/modules.

### Roles

| Role | May edit | Purpose |
|---|---|---|
| Orchestrator | Shared manifests/docs/scripts, app wiring, any file it takes over | Plan, freeze, decompose, integrate, run gates |
| Research | Nothing | Read-only facts with `file:line` |
| Worker | Only its owned files/globs | One bounded task + tests + evidence |
| Reviewer | Nothing | Red-team diff against parity IDs/edge cases/security |

### One file, one owner

- Never two workers in the same file.
- Shared files (`Cargo.toml`, root manifests, `AGENTS.md`, `PLAN.md`, all `docs/`, `scripts/`, `lmdv-app/src/lib.rs`, `lmdv/src/main.rs`) are orchestrator-owned unless a task explicitly transfers ownership.
- A worker needing a shared change reports the exact patch; the orchestrator applies it.
- Parallelize only after an interface freeze: P1 core/IO/app shell; P2 parser/renderer/math; P3 editor/file-safety; P5 HTML/DOCX/PDF. Never parallelize shared domain types/manifests.

### Worker task contract (copy into every prompt)

```text
Objective: <one sentence, testable>.

Read first: AGENTS.md, docs/STATUS.md, PLAN.md, docs/ARCHITECTURE.md,
docs/FEATURE_PARITY.md rows <IDs>, docs/DECISIONS.md, <Rune reference files>.

Context / interface:
- <verbatim frozen types/signatures the worker implements against>
- <spec behavior: file:line>
- <Rune API: vendor/rune path:line>

Owned files: <exact paths/globs. Nothing else may be edited.>
Do not touch: shared files, other crates, vendor/rune except the assigned patch, user files outside the repo.

Constraints:
- Rune pinned rev cca5df6; no webview; Linux first; no CARGO_TARGET_DIR.
- No file I/O in handlers/views; no unwrap in UI paths; no TODO without a parity ID.
- Run targeted tests only; never the workspace suite, a whole-crate suite, or `scripts/run-gates.sh`.
- Follow docs/ARCHITECTURE.md ownership and handler rules.
- Keep the public API exactly as frozen; if it must change, stop and report.

Deliverables:
- Code + unit/integration tests.
- Short implementation note: decisions, deviations, parity substitute/gap.
- Evidence: commands run and their results.

Acceptance (run all):
- cargo fmt -p <crate> -- --check   (or scripts/fmt-check.sh)
- cargo check -p <crate>
- cargo test -p <crate> <targeted filter>   (only the affected tests)
- cargo clippy -p <crate> --all-targets
- <headless scene / tree / script / golden / benchmark>
- Do not run `cargo test --workspace`, `scripts/run-gates.sh`, or a whole-crate suite; the orchestrator runs the full gate.

Report:
- Files changed (one line each why)
- Commands run + exact result summary
- Screenshots/tree paths
- Known limitations / follow-ups
- Anything the orchestrator must wire or decide
```

### Context capsule (one page per task)

Before spawning, write:

```text
Capsule: <task id>
Current phase: <P#>. Gate: <commands>.
What exists: <3–6 bullets with paths + public types>.
Interface freeze: <verbatim signatures>.
Spec behavior: <feature IDs + reference file:line>.
Known failures: <exact failing test/command>.
Do not: <scope exclusions>.
Next after this: <one line>.
```

Do not make a worker rediscover 200 KB of docs. Give it the capsule and exact files.

### Validation loop

1. **Read the diff**, not the summary: unrelated edits, debug prints, swallowed errors, `unwrap`, `todo!`, large deletions.
2. **Run the acceptance commands yourself.** A worker's “passing” is not evidence.
3. **Check the parity ID** against `BEHAVIOR_SPEC.md` when subtle.
4. **Classify:** accept → integrate; minor defect → re-prompt; design flaw → revise/take over; repeated failure → split or implement.
5. **Wire it:** modules, re-exports, features, commands, scenes, fixtures, i18n, docs/status, parity row.
6. **Record evidence** in `docs/STATUS.md` and the checklist.

### Re-prompt protocol

Never “please fix it.” One bullet per defect:

```text
The change fails acceptance:
- `cargo test -p lmdv-md frontmatter::tests::nested` → panics at src/frontmatter.rs:88:
  index out of bounds. Expected: raw fallback with valid=false (M-12).
  Actual: panic on entries[0].
- `--scene viewer-frontmatter --tree` shows no details role; expected a collapsed group.
- Scope: fix only crates/lmdv-md/src/frontmatter.rs and its tests.
- Re-run: <commands>; paste both results.
```

If the same task fails twice, narrow the scope, pair with a reviewer, or take over.

### Definition of done

- Behavior matches the spec or is a documented substitute/gap.
- Tests cover the behavior and edge cases.
- fmt/check/test/clippy pass for touched crates.
- Headless snapshot/tree/script passes when visual/structural.
- No new warnings; no TODO without a parity ID.
- Docs/status/checklist updated; OpenChamber todo ticked.
- The orchestrator read the diff and ran the checks.

---

## 6. Context handoff

At the end of every session, update `docs/STATUS.md` with:

- Current phase and exact task.
- What works now (commands to see it).
- What is broken/known-failing (failing command).
- Next 3 concrete tasks.
- Decisions changed and where recorded.
- Blocked subagent/open question.

Do not rely on chat history. If it is not in `docs/`, it did not happen.

Update OpenChamber Project knowledge too: plan step, notes, phase todos. If the panel and repo disagree, the repo wins for detail; fix the panel.

---

## 7. Key product rules

- Documents always open in **viewer** mode; edit is explicit (`Ctrl+E`).
- Files change only on explicit save. Line endings (CRLF/LF) and BOM policy are preserved; BOM is never re-added.
- Every document remembers its reading position; reopening returns there.
- Unsaved edits are never silently overwritten by a disk change: warn and offer reload/save.
- One tab per document per window; opening an already-open file brings its tab forward.
- Tabs keep their own scroll, undo, find bar and file-browser folder.
- Reading settings never affect print/export.
- Links resolve relative to the document; never follow network/device paths.
- Math delimiters follow GitHub, including `$5 and $10` staying text.
- Task checkboxes are live in viewer mode; ticking writes the file with Undo (the only viewer write).
- Closing a tab/window with unsaved changes always asks.
- The app works offline; no telemetry.
- Linux v1 uses the native window frame; themed CSD is a deferred substitute.
- Wayland has no window positions: tear-out placement/cross-window drop/always-on-top are X11-only or menu substitutes.

---

## 8. Current known unknowns

See `docs/RISKS_AND_SPIKES.md`.

- Inline math needs Rune patch 0001; P0 proves baseline/aspect.
- Native close veto needs patch 0002.
- Whole-document editor performance forces two editor modes.
- Cross-window tab drag has no clean Wayland path.
- PDF/DOCX fidelity beyond Rune needs Typst/OOXML.
- Multi-cursor and Linux spellcheck are staged; gaps need acceptance.
- Rune pre-release: vendor bumps are plain upstream copies with no local patches; rerun goldens.
