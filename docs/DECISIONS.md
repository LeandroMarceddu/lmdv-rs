# Decisions (ADR log)

Accepted decisions are not relitigated without new evidence. A changed decision gets a new ADR that supersedes the old one; do not rewrite history.

Format: **Status** · **Context** · **Decision** · **Consequences**.

---

## ADR-001 — Native Rune UI, no webview

**Status:** Accepted · 2026-11-12

**Context:** A Tauri 2 + TypeScript stack would be a port, not the requested native rewrite. Rune provides a GPU renderer, text/layout, widgets and native Mermaid.

**Decision:** Build with Rune and `rune-kit`. No HTML/JS application shell and no hidden webview. Math/PDF may use offline Rust engines; they are not a rendering surface for the UI.

**Consequences:** UI behavior is implemented natively; some web-platform conveniences (CSS, DOM, CodeMirror) must be rebuilt. We gain native text quality, no WebView memory cost and one rendering model.

---

## ADR-002 — Vendor the whole pinned Rune workspace and patch it

**Status:** Accepted · 2026-11-12 · supersedes the earlier “vendor only rune-kit” decision

**Context:** Rune is pre-release, `dev` moves, upstream does not accept pull requests, and the app needs five things at the text/window layer. Rune's crates depend on siblings by path; patching the text system means owning the tree. A dependency's `[patch]` does not apply to the dependent workspace.

**Decision:** Vendor the full pinned Rune workspace under `vendor/rune/`, depend on it by path, and repeat Rune's eight `[patch.crates-io]` entries in our root. Numbered local patches are retired: `0002` was superseded by upstream `CloseResponse`, and `0001`/`0003`/`0004`/`0005` by Rune `07bf8c5d3` (#2743). `vendor/rune/patches/` is intentionally empty.

**Consequences:** Reproducible offline builds with no local framework divergence. Rev bumps are plain upstream copies plus goldens; record checksums and a bump runbook.

**Update 2026-11-17:** the vendor pin moved from `d6d04d3` to `cca5df6`; Rune now patches five crates (`accesskit_macos` added) and `tiny-skia-path` is a path dependency of `crates/rune`. Patches 0001/0002 were rebased and the full gate is green. See `docs/STATUS.md` P7-RUNE and OI-164 for the visual/manual re-baseline still due.

---

## ADR-003 — The behavior specification is the source of truth

**Status:** Accepted · 2026-11-12

**Context:** README bullets hide edge cases: math delimiters, watcher conflicts, link resolution, tab routing, draft staleness.

**Decision:** `docs/BEHAVIOR_SPEC.md` is the 342-row authoritative inventory; `docs/PARITY_CHECKLIST.md` tracks implementation. Implement behavior, not TypeScript structure. Source references are recorded for traceability.

**Consequences:** Agents must read the relevant `file:line` before changing subtle behavior. The reference sources are research-only and never imported into the build.

---

## ADR-004 — One canonical Document per file, one editor lease

**Status:** Accepted · 2026-11-12 · supersedes the draft “per-tab editor with conflict notices”

**Context:** Windows/tabs must share documents; the UI is single-threaded; two independent buffers create merge/undo ambiguity.

**Decision:** One `Rc<Document>` per canonical file per process, shared by all tabs/windows. Each tab owns view state only. `Document.lease` grants edit rights to one `(window, tab)` at a time; other windows are live viewers. Takeover is explicit. Tab transfer moves the `TextEdit` and lease. `Document` mutates through `&self` interior cells, never `&mut`.

**Consequences:** No merge hell; one dirty state and one undo history per file. Simultaneous independent editing is intentionally given up; record it as `T-SHARED` substitute. Borrow discipline is critical: clone `Rc`, borrow one cell, drop guards before UI calls.

---

## ADR-005 — Math via RaTeX, with a mandatory inline text patch

**Status:** Proposed — P0 proof required · 2026-11-12

**Context:** Rune has no TeX. RaTeX is pure Rust, KaTeX-compatible (>99.5% syntax, SVG/PNG/PDF, em metrics). Rune's only inline visual is a **square** `InlineIcon` tied to the following word; there is no `Span::image` at the pinned rev.

**Decision:** Use RaTeX for parsing/layout/rendering; cache by `(tex, size, color, scale)`; block math as images. For inline math, implement patch 0001 (non-square inline image with ascent/descent + `Span::rise`). If P0 cannot achieve baseline/aspect within 0.5 pt at 1×/2×, fall back to block math and record the inline gap — do not ship a square, misaligned formula as parity.

**Consequences:** GitHub-compatible syntax without JS. The inline path is a Rune-core patch; it is the highest-risk item and the first thing P0 proves. Keep a `MathRenderer` trait so Typst math or another engine can replace RaTeX.

---

## ADR-006 — PDF via Typst; DOCX via OOXML (image math first)

**Status:** Proposed — P5 spike · 2026-11-12

**Context:** Rune draws windows, not paper. The baseline uses WebView2 `PrintToPdf`. `--html` is a first-paint scene export, not paginated/semantic; raw-pixel math/diagrams are dropped.

**Decision:** Build a shared `ExportDoc` from `MdDoc`. PDF via Typst (page model, header/date, page numbers, optional H1 page breaks, table fitting, embedded math/diagram assets). DOCX via a direct OOXML writer (or `docx-rs` plus our own math); math is an image first, best-effort OMML later. HTML is our own semantic writer. Print on Linux = PDF + system viewer/print.

**Consequences:** Export is independent of reader theme/size/column. Typst is the largest dependency; it may become a helper binary if build time hurts. Native Word equations are a known possible gap.

---

## ADR-007 — Linux v1 uses the native window frame

**Status:** Accepted for v1 · 2026-11-12

**Context:** Rune's custom titlebar is macOS/Windows only; borderless Linux has no interactive edge resize. The specified chrome is a themed draggable titlebar with caption buttons.

**Decision:** Ship the native Linux frame and draw the app toolbar/tab strip below it. Do not block parity on client-side decorations. A Rune CSD patch is optional after the core app works.

**Consequences:** One visual difference from the specified chrome. W-01/02/03/05/06/08 are `substitute`/`accepted-gap` on Linux. Revisit only if the user rejects the native frame.

---

## ADR-008 — Linux spellcheck via Hunspell

**Status:** Accepted · 2026-11-12

**Context:** Rune's `spell` module is Windows/macOS only; Linux returns no misspellings.

**Decision:** Implement Linux spellcheck in `lmdv-edit` with a Hunspell-compatible crate (`zspell`/`spellbook`), system/user dictionaries, underline spans and a suggestion menu. Keep Rune's native path for other platforms.

**Consequences:** We own dictionary discovery, language selection and suggestion UI on Linux. It is independent of the editor core and can land in P3/P6.

---

## ADR-009 — Own the single-instance socket

**Status:** Accepted · 2026-11-12 · supersedes “use Rune's single_instance”

**Context:** Rune's single-instance forwards `scheme://` links to `App::on_open_url`, but not the sender's cwd or bare file paths. `lmdv file.md --line 40` must resolve relative paths against the caller's cwd and route into the running app.

**Decision:** First instance claims a lock + Unix socket at `$XDG_RUNTIME_DIR/lmdv-<id>.sock` (dir 0700, socket 0600, `SO_PEERCRED` uid check, 1 MiB JSON, versioned). Later instances send `{cwd, paths, line, new_window, activation}` and exit. Stale sockets are removed only if owned by the uid and no live process answers. The listener only forwards a launch to the UI.

**Consequences:** Full control over cwd, `--line`, multi-file and activation. We own stale-socket races and protocol validation; test kill -9, burst launches and portable ids.

---

## ADR-010 — XDG files with a single writer thread

**Status:** Accepted · 2026-11-12 · supersedes “use rune::Prefs for everything”

**Context:** The baseline uses localStorage; Rune has no document/session store. `rune::Prefs` rewrites synchronously and is unsuitable for hot paths.

**Decision:** Use `rune::dirs::AppDirs` for XDG/portable resolution, but own the state files. One store thread coalesces patches (max one write/file/300 ms), atomic temp+rename+fsync, schema/migrations, `.bad-<timestamp>` quarantine. Caps: 12 recents, 100 positions, 20 drafts/2 MB, 25 closed tabs, 50 nav entries.

**Consequences:** Inspectable, crash-safe state; no UI-thread I/O. Drafts are at most 1 s old. Portable mode uses `<exe>/data` with a marker and a distinct instance id.

---

## ADR-011 — Licensing ignored, provenance kept

**Status:** Accepted · 2026-11-12

**Context:** Personal tool; Rune and the reference sources are MIT. The user explicitly does not want licensing to constrain completeness.

**Decision:** Copy/port freely. Keep Rune's `UPSTREAM.md` provenance and copyright notices in copied source.

**Consequences:** If the app is ever published, re-evaluate attribution/MIT obligations.

---

## ADR-012 — Orchestrator/worker model with one-file ownership

**Status:** Accepted · 2026-11-12

**Context:** The user wants the main agent to orchestrate subagents, validate output, and re-prompt or integrate.

**Decision:** The orchestrator freezes interfaces, owns shared manifests/docs/scripts/wiring, spawns bounded workers, reads every diff and reruns every gate. Research/review are read-only. **One file, one owner.** Workers get a context capsule and exact files; a shared-file change is reported as a patch for the orchestrator.

**Consequences:** Less overlap, more upfront decomposition, verified work. A worker's “it passes” is not evidence.

---

## ADR-013 — Golden fixtures and headless snapshots are the regression contract

**Status:** Accepted · 2026-11-12

**Context:** The app is visual and behavior-heavy; manual checking will not scale.

**Decision:** Adopt the ten fixture families from `BEHAVIOR_SPEC.md` §9 plus the reference samples. Use `--snap` for PNGs with tolerance, `--tree` for roles/labels/focus, `--script` for interactions, parser/renderer goldens for `MdDoc`/scene JSON. Pixel comparisons tolerate GPU/AA differences; semantic regressions do not.

**Consequences:** Visual changes need a golden update with a reason. Tests run headlessly; GUI-only tests are opt-in.

---

## ADR-014 — Native Mermaid, code-fence fallback

**Status:** Accepted · 2026-11-12

**Context:** `rune-mermaid` supports 17 diagram families (more than the kit README says) but not every Mermaid v11 type. No zoom/pan or SVG export is built in.

**Decision:** Use `rune-mermaid` for supported types; unsupported source renders as a code block with a notice. Build a `Drawing`-based pan/zoom lightbox. PNG export via offscreen Rune; SVG export is a later vendor/serializer spike.

**Consequences:** Offline, themed, crisp diagrams with no JS. A small set of exotic types is a documented gap.

---

## ADR-015 — Export is a separate rendering pipeline

**Status:** Accepted · 2026-11-12

**Context:** Reader settings (theme, size, column width) must not affect print/export; Rune has no page model.

**Decision:** Build an export-specific `ExportDoc`/style from `MdDoc`. Never screenshot the reader for PDF/HTML/DOCX. Render math/diagrams at export resolution. Print = PDF + system viewer/print.

**Consequences:** More work to keep visual parity, but deterministic, theme-independent output and correct pagination.

---

## ADR-016 — Two editor modes with a measured threshold

**Status:** Accepted · 2026-11-12

**Context:** One `TextEdit` for a whole document costs ~12 ms/keystroke at 50 KB and ~46 ms at 200 KB; undo is 128 full snapshots / 1 MiB. Per-block fields cost ~0.8/2.6 ms.

**Decision:** One field while `len <= 64 KiB && lines <= 1500`; one `TextEdit` per blank-line block above that, with per-block history plus a global journal so `Ctrl+Z` spans blocks. Auto-switch; a pref can force block mode. Tune the threshold with `long_note`-style benchmarks.

**Consequences:** Small docs keep full selection/undo/IME simplicity; large docs stay under the typing budget. Cross-block selection in block mode is a documented substitute.

---

## ADR-017 — No file I/O in handlers or views

**Status:** Accepted · 2026-11-12

**Context:** FUSE/NFS/AV filters can block for seconds; Rune's own warning is 8 ms. The baseline had async commands.

**Decision:** All load/save/stat/probe/watch/export/image-decode work is a job in `lmdv-io`/workers. Handlers only enqueue and mutate UI state; views are pure. Enforce with a grep/CI check for `std::fs` in `lmdv-app`.

**Consequences:** No UI freezes on slow storage. More job types and stale guards; every completion carries doc id/revision/token.

---

## ADR-018 — Commands are data

**Status:** Accepted · 2026-11-12

**Context:** The palette, menus, toolbar, context menus and shortcut matrix are large; ad-hoc closures drift.

**Decision:** One static `CommandSpec` table (`id`, title key, category, keywords, default keys, run, enabled, checked). Everything dispatches by ID. Shortcut strings are parsed into chords once; sequences like `g g` are supported.

**Consequences:** Testable command coverage, one source for the palette and keymap, easy localization. Native menus are not used on Linux; the in-app menu renders the same table.

---

## ADR-019 — Fixed CLI grammar

**Status:** Accepted · 2026-11-12

**Context:** The parser accepts `lmdv [--new-window] [file] [--line N]`, `-l`, `--line=`, `--`, ignores unknown flags and uses only the first positional path.

**Decision:** Match it exactly. Relative paths resolve against the sender's cwd; missing files retain an absolute path and route; only the first positional path is used. `--new-window` may appear before/after the file and before `--`. Do not invent `--version`/`--help`/`--wait` in v1.

**Consequences:** Scripts and habits carry over. The launcher may absolutize paths before forwarding; the binary still accepts bare paths.

---

## ADR-020 — Patch native close for unsaved-close guards

**Status:** Accepted · 2026-11-12

**Context:** Rune hides the window before `on_close`; there is no veto.

**Decision:** Patch `App::on_close_request -> bool`/`WindowSpec::on_close_request`; run before hiding; keep `Tx::close_window`/`AppHandle::close` force-close.

**Consequences:** Alt+F4/WM close can show Save/Discard/Cancel. Programmatic close still works for tests and quit.

**Update 2026-11-25:** upstream Rune `f4ec14e` now has `App::on_close_request -> CloseResponse::{Close, Defer}` with the same semantics. Patch 0002 is retired; the app returns `Defer` from its hook.

---

## ADR-021 — Session restore is an intentional extension

**Status:** Accepted · 2026-11-12

**Context:** The specification does not restore tabs across a full restart (only window geometry). The user asks for persistence.

**Decision:** Restore windows/tabs/anchors/mode/nav/panes on clean start when `prefs.restore_session` and no file argument. Do not restore editor text from session; use drafts. A CLI file wins for the first window. Crash restores tabs and marks the session recovered.

**Consequences:** Better daily-driver feel; a new failure surface (stale paths, deleted files). Missing paths become “Not found” tabs or are dropped with a notice.

---

## ADR-022 — Wayland substitutes for X11-only shell features

**Status:** Accepted · 2026-11-12

**Context:** Wayland hides window positions; `screen_point`/`set_position` are no-ops; always-on-top is a no-op; cross-window pointer capture is unreliable.

**Decision:** Tear-out creates a window compositor-placed (not under the pointer); cross-window tab drop uses **Move tab to window…** / **Move to new window**; always-on-top is disabled with a notice. X11 keeps the richer behavior.

**Consequences:** No data loss; documented substitutes. Do not fake global coordinates on Wayland.

---

## ADR-023 — Shared document substitute for independent per-window buffers

**Status:** Accepted · 2026-11-12

**Context:** The specification allows the same file in two windows with independent CodeMirror buffers and conflict detection. The user explicitly wants safe sharing.

**Decision:** One canonical buffer + editor lease (ADR-004). Record `T-SHARED` as a substitute in `FEATURE_PARITY.md`.

**Consequences:** No cross-window merge/undo races; only one editor at a time. Takeover is explicit and draft-safe.

---

## ADR-024 — P1 viewer uses `rune-kit::markdown` temporarily

**Status:** Accepted for P1 · 2026-11-13

**Context:** P1 needs a real Markdown reading surface before the `lmdv-md` parser and `lmdv-render` fork are ready. ADR-003 says not to use the kit renderer in production because it lacks footnotes/callouts/front matter/anchors/TOC/HTML/math/sticky tables.

**Decision:** P1 renders the active document with `rune-kit::markdown` behind a small `ReaderView` boundary, with the temporary `pulldown-cmark` scan for headings/outline/`--line`. P2 replaces that boundary with `lmdv-md` + `lmdv-render`; the kit renderer and the direct `pulldown-cmark` dependency are removed from `lmdv-app`.

**Consequences:** The app has a usable reader earlier. No behavior is considered parity-complete until P2 swaps the renderer; the P1 reader is explicitly a temporary slice.

---

## ADR-025 — Product name is lmdv; no project homepage

**Status:** Accepted · 2026-11-16 · homepage clause superseded by ADR-027

**Context:** The locale catalogs and help pages still carried a legacy product name, and packaging metadata was waiting on a public project URL. The user decided that the finished app must not use that name and that no homepage will be made or provided.

**Decision:** Every app-visible string uses **lmdv**: all six locale catalogs, the window-title template, help pages and packaging metadata. The legacy name is scrubbed from the repository; only a single README attribution line may name it. Do not add `<url type="homepage">` or create a project site; `url=` in packaging recipes is only a release-source base. Keep the AppStream `url-homepage-missing` warning as an accepted gap.

**Consequences:** Branding is consistent with the native app identity. A distribution channel that requires a homepage may be unavailable; the release workflow can ship without one. Future locale/help ports must not reintroduce the legacy name.


---

## ADR-026 — AppStream identity is metainfo-only; release checksums are signed

**Status:** Accepted · 2026-11-24

**Context:** AppStream rejects the two-part component id `md.lmdv` (`cid-desktopapp-is-not-rdns`). A full rename would change the runtime/desktop identity and moving the `md.lmdv` data root would require a migration. Releases had no integrity signature.

**Decision:** Use `io.github.leandromarceddu.lmdv` as the AppStream component id and `io.github.leandromarceddu` as the developer id, with `<launchable type="desktop-id">lmdv.desktop</launchable>`. Keep the runtime id `lmdv`, the desktop/icon names and the `md.lmdv` data root unchanged. Sign `SHA256SUMS` with a dedicated Ed25519 release key (fingerprint `DD029B151E6A78B4D2357B17FEFE77EDE1F2E862`, expires 2028-11-24), publish the public half and `SHA256SUMS.asc`, and keep the private key/passphrase only in GitHub Actions secrets. X-15 keeps one stricter export rule: the parent must exist for HTML, DOCX and PDF; folders are never created.

**Consequences:** The AppStream warning is gone with no migration. A full runtime/data rename is a separate decision. Release checksums now detect tampering as well as transfer corruption; Windows Authenticode and AppImage signatures still need platform tooling. The first signed tag must be observed before calling a release signed.


---

## ADR-027 — Project homepage on GitHub Pages

**Status:** Accepted · 2026-11-24 · supersedes ADR-025's “no homepage” clause

**Context:** AppStream reported `url-homepage-missing`, and the user asked for a project site. The app itself remains native, offline and webview-free; a static download page does not change that.

**Decision:** Publish the static site in `site/` with GitHub Actions Pages at <https://leandromarceddu.github.io/lmdv-rs/>. The page is plain HTML/CSS, uses the app screenshots and icon, and offers release-page fallbacks plus direct latest-asset links from the GitHub API. Add `<url type="homepage">` to the AppStream metadata and set the repository homepage. Do not add analytics, tracking or an app webview.

**Consequences:** `appstreamcli validate` is clean, and a channel that requires a homepage has one. The site is a separate deployment surface: update it with releases and do not treat it as the application. The homepage URL is now a stable public identity.
