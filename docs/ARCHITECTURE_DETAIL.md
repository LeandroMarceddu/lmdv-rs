# lmdv — architecture and orchestration plan

**Status:** detailed design snapshot. `docs/ARCHITECTURE.md` is the condensed, current summary; this file
keeps the full type sketches, rationale, rejected alternatives and module trees. The phase summary is in
`PLAN.md`; the subagent contract is in `AGENTS.md`; the risk register is in `docs/RISKS_AND_SPIKES.md`.

**Inputs read:** the behavior spec, Rune at
`cca5df6f…` (`vendor/rune/`), `BEHAVIOR_SPEC.md` (342 behavior rows),
`RUNE_CAPABILITY_MAP.md`, and the repository `AGENTS.md`.

**One-sentence design:** one event loop, one shared document registry, one command registry, one
single-threaded UI state per window; all slow work is a generation-tagged job that reports through a
per-window mailbox; Markdown is parsed by a fork of Rune's parser into a Rune-free `MdDoc`, then rendered by
a fork of Rune's renderer into memoized Rune elements.

---

## 0. Decisions up front

| # | Decision | Alternative rejected | Why |
|---|---|---|---|
| D1 | **Vendor the whole Rune workspace** at the pinned rev under `vendor/rune/` and depend on it by path. | Git dependency plus a patch of one crate. | Rune's crates depend on sibling crates by path; patching the text system means owning the whole tree anyway. It also makes the build reproducible offline. |
| D2 | **Patch Rune for the five things the app cannot work around**: inline image/rise in text, native-close veto, a small text hit/selection helper, optional window theme, and (deferred) Linux client-side frame. | App-side hacks. | Inline math, unsaved-close confirmation, and click-to-source are product requirements, not polish. A 100–400 line patch is cheaper than a second text engine. |
| D3 | **Fork `rune-kit`'s Markdown module into two crates**: `lmdv-md` (parser/model, no Rune) and `lmdv-render` (renderer, Rune). Do not use `rune-kit::markdown` in production. | Depend on `rune-kit::markdown` and preprocess. | Footnotes, callouts, front matter, heading IDs, `[TOC]`, HTML subset, math, sticky table headers, and source maps are parser/renderer changes, not decorations. Preprocessing would be a pile of fragile rewrites. |
| D4 | **Shared canonical document buffer** in a process-wide `Rc<RefCell<…>>` registry; each tab owns only its view state. One **editor lease** per document. | One independent buffer per tab (baseline), or `Arc<Mutex>` everywhere. | The user explicitly wants safe sharing. The UI is single-threaded, so `Rc` is correct; a lease avoids cross-window undo/merge. `Arc<Mutex>` would add poisoning and UI stalls for no benefit. |
| D5 | **Commands are data** (`CommandId` + a static `CommandSpec` table); handlers dispatch commands synchronously. Menus, shortcuts and the palette all use the same table. | Ad-hoc closures per control. | The palette and shortcut matrix are large; one table is testable and localizable. |
| D6 | **Per-window worker mailbox, per-job-type threads**. Workers never touch app state; they post `JobMsg` values and call a `Waker`. | `AppHandle::update` for every completion, or shared mutable state. | Dynamic windows (`open_window`) have no `AppHandle<WindowState>`; mailbox + `Waker` works for every window uniformly. |
| D7 | **No file I/O in handlers or views.** Store, load, save, watch, probe, export and image decode are jobs. | Read a small file in a handler. | Rune's own rule, and the Linux desktop must never block on a slow NFS/FUSE path or a spinning disk. |
| D8 | **XDG files + a single writer thread** with atomic replace; no `rune::prefs::Prefs` for hot paths. | `Prefs::set` in a handler. | Rune's `Prefs` writes synchronously and rewrites the file on every set; the store must coalesce and never run on the UI thread. |
| D9 | **PDF is Typst**; HTML/DOCX/PNG have their own writers over a shared `ExportDoc`. | Print the webview (impossible), or one renderer for all. | Print/PDF needs pagination, headers, page numbers and table fitting; Typst is the only offline Rust engine that does it well. |
| D10 | **Native Linux frame** first; draw the app toolbar as content. Custom client-side decorations are a late, optional patch. | Borderless window with hand-written resize. | Rune has no interactive edge resize on Linux; a borderless window would be unusable. Native frame is a documented substitute. |
| D11 | **One process, one socket we own** for `lmdv` launches, with cwd and activation token. | Rune's `single_instance`. | Rune forwards argv but not the sender's cwd, and surfaces only links to the app. Relative paths and `--line` need the sender's cwd. |
| D12 | **One field for small documents; one `TextEdit` per blank-line block for large ones.** Threshold measured, initially 64 KiB / 1,500 lines. | Always one whole-document `TextEdit`. | Rune measures 12 ms per keystroke at 50 KB and 45.7 ms at 200 KB in one field, versus 0.8/2.6 ms per block. The threshold is a measured constant, not a guess. |

### 0.1 Historical note (superseded drafts)

These corrections were applied to the current `docs/ARCHITECTURE.md`, `docs/DECISIONS.md` and `PLAN.md`.
Keep this table for context only; the “current draft” column describes the earlier drafts, not the
current documents.

| Current draft | Correction in this report | Action for the orchestrator |
|---|---|---|
| ADR/architecture: workers wake the UI through `AppHandle::wake` / `rune::Latest`; `Live` appears as a result path. | `Live` is a paint-time binding for a property, **not** a channel. Dynamic windows opened by `open_window` do not have an `AppHandle<WindowState>`. Use the per-window `Waker` from `AppSpec::setup` + `WorkerMailbox` + `poll` (D6). | Replace the worker-result section of `docs/ARCHITECTURE.md`; keep `Live` only for progress/section-line/pan/zoom. |
| Architecture permits small synchronous file IO in a handler if under the frame budget. | No file IO on the UI thread, including `stat`/canonicalize; FUSE, network mounts and AV filters can block for seconds. All reads/writes/probes are `lmdv-io` jobs (D7). | Update the rule and the review checklist; unit tests should fail on `std::fs` in `lmdv-app` (grep/CI). |
| `lmdv-core` contains documents, buffers, prefs, persistence, watcher and search. | Split pure domain (`lmdv-core`) from IO/persistence (`lmdv-io`). This keeps `lmdv-core` testable without threads/FS and gives IO one owner. | Map `lmdv-io` into the current layout as a crate, or as `lmdv-core::io` with the same ownership boundary. |
| `lmdv-markdown` depends on Rune and includes parser + renderer. | Split parser/model (`lmdv-md`, no Rune, `Send`) from renderer (`lmdv-render`). Parse jobs run off-thread; `lmdv-export` can build HTML/DOCX/PDF without linking Rune. | If the name `lmdv-markdown` must stay, make it the renderer crate and put the parser in `lmdv-md`; or feature-gate `render` and keep one package. |
| `Tab.editor: Option<TextEdit>` per tab, with conflict notices when two editors touch one document. | One `Document` + one `EditorLease` (D4). Viewers share the buffer; only the lease holder edits. This removes merge/undo ambiguity. | Decide explicitly; if per-tab editors are kept, specify the merge and stale-editor rules and add race tests. |
| GFM tables use `rune-kit::Table`; sticky header via overlay. | `rune-kit::Table` is a data grid over app-owned rows, not the Markdown table renderer. Use it for the full-window preview; render document tables with the forked renderer + `sticky_top`. | Update §5 of the draft; add the table preview as a separate consumer. |
| DOCX through `docx-rs`. | `docx-rs` has no native OMML math. Either write OOXML directly (this plan) or accept image-only equations. | Update ADR-006; choose custom OOXML + best-effort OMML, or document the math substitute. |
| `--line N` and `[file…]`: current PLAN shows multiple positional files. | The specification uses only the first positional path; unknown flags are ignored; `--` ends flags. Match `BEHAVIOR_SPEC.md §3.15`. | Fix the CLI contract before P1. |
| Close guard is described without a Rune hook. | Native close has no veto at this rev; add `on_close_request` patch (D2). | Add to SP-04/phase P0; otherwise the guard is not implementable. |
| Inline math described as a normal `InlineMath` node. | The node can exist in the model, but drawing it inline requires the text patch; `InlineIcon` is square. | Keep the node; add the patch to P0/SP-01; do not plan a square fallback as parity. |

Everything else in the draft (native frame, Typst, XDG store, no webview, the spec as behavior authority, command
registry, tab strip, headless goldens) is adopted here.

---

## 1. Verified Rune constraints and the consequences

These are facts from the pinned source, not README claims.

| Fact | Where | Consequence |
|---|---|---|
| Handlers are queued and run between frames with `&mut S`. | `crates/rune/src/el/handlers.rs:1` | No re-entrancy from input; nested `RefCell` borrows are the only borrow hazard. |
| A view is `fn(&S, &mut Cx<S>) -> El`; `AppSpec::init` is `fn() -> S`. | `crates/rune/src/tooling.rs:17`, `runtime.rs` | Every window can share one `view`/`poll`/`on_key`; state must be constructed before `open_window`. `App::init` accepts a closure. |
| Multi-window: `App::window(WindowSpec)` for a second long-lived window; `open_window(WindowSpec)` for dynamic windows. Per-window state and retained tree; closing one does not quit the app. | `crates/rune/src/app.rs:345`, `window/spec.rs:330` | One `WindowState` type for every window, each with its own `Rc<AppShared>`. Cross-window requests go through mailboxes, never through `&mut` of another window. |
| `AppSpec::setup` runs once per window runtime and receives a `Waker`; `poll` runs each frame. | `crates/rune/src/tooling.rs:149` | Attach the window's `Waker` to its mailbox in `setup`; drain mailboxes in `poll`. |
| `WindowSpec<S>` requires `S: 'static` but not `Send`; each window's state lives on the UI thread. | `window/spec.rs:227` | `Rc`, `RefCell`, `TextEdit` and `ImageCache` are all legal in `WindowState`; do not put them in worker messages. |
| Rune has **no `Span::image`** at this rev. The only inline visual is `El::inline_icon` / `InlineIcon::image`: a square box tied to the following word. | `crates/rune/src/text/span.rs:11`, `el/pills.rs:57`, `text/pill_chips.rs:6` | Inline math cannot be correct without a text patch. This is the first spike and the highest-risk item. |
| Rune has no text-click byte callback. `on_selection_change` is not called for a bare caret press; `Cx::caret` is only the focused field's caret. | `crates/rune/src/el/text.rs:518`, `runtime/cx.rs:295` | Click-to-source needs a small hit callback, or a double-click handler that reads `Tx::text_selection`. Start with the latter; patch only if it fails. |
| Native close has **no veto**. `CloseRequested` hides the window and then runs `on_close`. | `crates/rune/src/window/lifecycle.rs:342` | Add `App::on_close_request` returning `bool`; otherwise unsaved-close confirmation is impossible. |
| File dialogs on Linux are XDG portal only. | `crates/rune/src/file_dialog.rs:1` | Keep `native-file-dialog`; show a clear notice if the portal is missing; drag/drop and CLI remain usable. |
| Clipboard writes HTML + text on Linux via `arboard`. | `crates/rune/src/clipboard/native.rs` | “Copy as rich text” is a `Tx::copy_rich` call, no browser needed. |
| `AppSpec::on_key` only sees keys the focused control did not consume. | `tooling.rs` docs | Editor shortcut handling must live in the editor's `on_input_key`; global reading keys live in `on_key`. |
| Native menus are macOS/Windows only. | `crates/rune/src/native_menu/mod.rs:1` | Linux uses an in-app menu bar/popup menus; do not plan native menus. |
| `TextEdit` has one selection; no multi-cursor, no gutter, no find/replace. Undo is 128 steps / 1 MiB full snapshots. | `crates/rune/src/edit.rs:38`, `edit/history.rs` | Multi-cursor is a later, explicit feature; find/replace and line ops are app-side. |
| Typing cost scales with the whole field: ~12 ms per keystroke at 50 KB, ~46 ms at 200 KB. One field per block: ~0.8/2.6 ms. | `RUNE_CAPABILITY_MAP.md §2.3`, Rune `examples/long_note.rs` | Adopt the two-mode editor (D12). Do not attempt a single 1 MB text field. |
| `Gpu::headless`, `Renderer`, `Offscreen`, `read`, `write_png` are public. | `crates/rune/src/render.rs:550,1052,1120,1138` | Diagram/doc PNG export can run in an export worker with its own `Runtime`, no subprocess and no window. |
| `rune::task::spawn` exists; `Waker` is `Arc<dyn Fn() + Send + Sync>`. | `crates/rune/src/task.rs`, `runtime/cx.rs:36` | Use plain threads for jobs; the waker is the only UI wake path. |
| Headless tooling: `--snap`, `--tree`, `--script`, `--frames`, `--scene`, `--desktop`, virtual clock. | `crates/rune/src/tooling/args.rs:239` | Visual, structural and interaction tests are first-class; use them as gates. |

### 1.1 The Rune patch set

Keep patches in `vendor/rune/patches/` as numbered `.patch` files plus `UPSTREAM.md`. Re-apply with
`scripts/vendor-rune.sh` and `scripts/apply-rune-patches.sh`.

| Patch | Scope | Acceptance |
|---|---|---|
| `0001-text-inline-object.patch` | Add `Span::image(InlineImage)` and `Span::rise(f32)`; reserve a U+FFFC placeholder; extend the existing inline-box bookkeeping in `text.rs`, `text/span.rs`, `text/pill_chips.rs`, `runtime/paint/content.rs`. | Inline formula baseline within 0.5 pt, correct aspect ratio, wraps whole, selection copy skips it, works in RTL. |
| `0002-close-request-veto.patch` | Add `App::on_close_request(fn(&mut S, &mut Tx) -> bool)` and `WindowSpec::on_close_request`; run before hiding on `CloseRequested`; `Tx::close_window`/`AppHandle::close` remain force-close. | Alt+F4 on a dirty doc leaves the window open and shows the dialog. |
| `0003-text-hit.patch` (only if needed) | Add `El::on_text_hit(fn(&mut S, TextHit, &mut Tx) -> bool)` with the rendered byte under the pointer, or a double-press variant that does not break selection. | Double-click on preview places the editor caret on that word; single click still selects normally. |
| `0004-window-theme.patch` (optional) | `WindowHandle::set_theme(Theme)` to match the app theme on the native frame where winit supports it. | Native frame follows light/dark choice; not a release gate. |
| `0005-linux-frame.patch` (deferred) | Interactive edge resize and optional client-side frame. | Only if the native frame is judged unacceptable; never blocks parity. |

The root workspace must repeat Rune's `[patch.crates-io]` entries; a dependency's patches do not apply
to the dependent workspace:

```toml
[patch.crates-io]
wgpu-hal = { path = "vendor/rune/vendor/wgpu-hal" }
winit = { path = "vendor/rune/vendor/winit" }
parley_engine = { path = "vendor/rune/vendor/parley_engine" }
accesskit_windows = { path = "vendor/rune/vendor/accesskit_windows" }
```

---

## 2. Workspace and crate layout

```
lmdv-rs/
  Cargo.toml                     # workspace; lints; [patch.crates-io]; no members from vendor/
  rust-toolchain.toml            # 1.99.1
  rustfmt.toml
  clippy.toml
  PLAN.md
  AGENTS.md
  docs/
    ARCHITECTURE.md              # this document, condensed
    FEATURE_PARITY.md            # IDs + status, from BEHAVIOR_SPEC.md
    TESTING.md
    DECISIONS.md                 # D1–D12 and patch log
    RISKS_AND_SPIKES.md
    LINUX_INTEGRATION.md
    STATUS.md                    # living handoff
  vendor/
    rune/                        # full pinned workspace, patched
      UPSTREAM.md
      patches/
  assets/
    fonts/                       # Inter, Source Serif 4, JetBrains Mono
    icons/
  locales/                       # en.json, it.json, de.json, fr.json, es.json, zh-CN.json
  scripts/
    vendor-rune.sh
    apply-rune-patches.sh
    check-vendor.sh
    install-md-handler.sh
    run-gates.sh
  tests/
    fixtures/                    # F-CORE, F-MARKDOWN, … from BEHAVIOR_SPEC.md
    golden/                      # PNG and JSON trees
    scripts/                     # headless input scripts
  crates/
    lmdv/                        # bin only
    lmdv-app/                    # lib: WindowState, view, commands, routing, wiring
    lmdv-core/                   # lib: pure domain types + SourceBuffer + Document
    lmdv-io/                     # lib: paths, store, atomic write, load/save, watcher, instance, open-external
    lmdv-md/                     # lib: parser + MdDoc model + extensions (no Rune)
    lmdv-render/                 # lib: Rune renderer for MdDoc + DocTheme + inline math provider trait
    lmdv-math/                   # lib: RaTeX wrapper, SVG/PNG cache, metrics (no Rune)
    lmdv-edit/                   # lib: editor model on Rune TextEdit, find/replace, line ops, drafts
    lmdv-export/                 # lib: ExportDoc, HTML, DOCX, Typst PDF, offscreen PNG (Rune)
    lmdv-i18n/                   # lib + build.rs: rune-i18n catalogs and typed keys
    lmdv-testkit/                # lib (dev-dependency only): fixtures, scenes, tree/golden helpers
```

### 2.1 Responsibilities

| Crate | Owns | Must not know about |
|---|---|---|
| `lmdv` | `main`, CLI split, app id, single-instance claim, constructing `AppShared`, initial windows, hidden `--render-asset` mode if used. | Any UI layout or domain logic. |
| `lmdv-app` | `WindowState`, `Tab`, view composition, command dispatch, key routing, dialogs/palette/lightbox, multi-window routing, editor/preview wiring, preferences application, session restore. | Parsing internals, file formats, Typst. |
| `lmdv-core` | IDs, `SourceBuffer`, `Document`, `DocumentRegistry`, `Prefs`, `ScrollAnchor`, `Session`, `Draft`, `FindState`, `ExportJob` request types, `LineIndex`, `Eol`, `FileMeta`. | Rune, `std::fs` (types may carry paths, but no I/O). |
| `lmdv-io` | XDG resolution/migrations, store writer, atomic file replacement, read/probe/normalize, watcher, single-instance socket, `xdg-open`, path policy. | Rune, rendering, Markdown. |
| `lmdv-md` | CommonMark + GFM + extensions to `MdDoc`; source maps; anchors; TOC; math/footnote/callout/front-matter scans; HTML subset AST. | Rune, file I/O, themes. |
| `lmdv-render` | `DocTheme`, block/inline rendering, table sticky header, code highlighting, math/inline image placement, images/diagrams, selection/source mapping hooks. | Filesystem, persistence, window/tab semantics. |
| `lmdv-math` | TeX→RaTeX AST→layout→SVG/PNG, metrics/caching, deterministic color/size keys. | Rune UI (except `Image`/`Pixels` are supplied by caller). |
| `lmdv-edit` | `TextEdit`-based editor session, syntax spans, find/replace, line ops, undo journal, large-document block mode, draft snapshots. | Window routing, file writes (returns edits). |
| `lmdv-export` | `ExportDoc`, HTML serializer, DOCX writer, Typst source/compiler, offscreen PNG. | App state; receives a snapshot + asset resolver. |
| `lmdv-i18n` | Catalog codegen, locale negotiation, `t!`-style typed lookup. | UI. |
| `lmdv-testkit` | Fixture loader, headless scene/script runner, golden/tree assertions, deterministic temp dirs. | Production code. |

**i18n.** `lmdv-i18n/build.rs` runs `rune_i18n::codegen::generate` over `locales/*.json`; keys are checked at
compile time. Locale selection uses `rune_i18n::system_languages()` (`LANGUAGE`, `LC_ALL`, `LANG` on Linux)
and `negotiate`, with a user override in prefs. A language change rebuilds the current window immediately;
there is no WebView reload. The six catalogs are ported from the reference 399-key inventory (413 keys now), including help
text; command IDs stay English while titles/keywords are localized.

**Name mapping if the repo keeps the current draft names.** `lmdv-markdown` → `lmdv-render` (renderer) plus
`lmdv-md` (parser); `lmdv-theme` → `lmdv-render/src/theme.rs` plus `lmdv-app/src/theme.rs`; `lmdv-i18n` → a
module in `lmdv-app` with a build script; `lmdv-io` → `lmdv-core::io` with the same no-UI-thread boundary.
Do not maintain two copies of the parser or two theme token sets.

### 2.2 Dependency DAG

```text
lmdv ──► lmdv-app ──► lmdv-render ──► lmdv-md
  │           │            │              │
  │           │            └──► lmdv-math ─┘
  │           ├──► lmdv-edit ──► lmdv-core
  │           ├──► lmdv-export ──► lmdv-md, lmdv-math, rune (offscreen only)
  │           ├──► lmdv-io ──► lmdv-core
  │           ├──► lmdv-i18n
  │           └──► lmdv-core
  └──► lmdv-io, lmdv-core
```

Rules:

- No library depends on `lmdv-app` or `lmdv`.
- `lmdv-md` has **no** Rune dependency, so parse jobs are `Send` and export stays lightweight.
- `lmdv-render` has **no** `lmdv-io`; the app supplies path validation and image resolution.
- `lmdv-export` may depend on Rune for offscreen PNG only; HTML/DOCX/PDF paths must not require a GPU.
- Only `lmdv-testkit` may be a dev-dependency of several crates; production crates never depend on it.
- `vendor/rune` is not a workspace member. `cargo fmt --all` and `cargo clippy --workspace` do not touch it;
  patch compilation is tested by a dedicated `vendor-check` job.

### 2.3 Feature flags

```toml
# lmdv-export
[features]
default = ["pdf"]
pdf = ["dep:typst", "dep:typst-pdf"]
docx = ["dep:zip", "dep:quick-xml"]
png = []           # Rune offscreen only
```

`typst` is the largest dependency in the workspace. Keep it out of `cargo check -p lmdv-app` while iterating;
the full gate builds it. If Typst makes builds unacceptably slow, split PDF into a `lmdv-pdf` helper binary
and keep the same `ExportJob` interface.

---

## 3. Core domain model

### 3.1 IDs and keys

```rust
// lmdv-core/src/ids.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DocumentId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WindowId(pub u64);

pub type TabId = u32;
pub type JobId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PasteId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DocumentKey {
    /// Canonical, absolute, not necessarily existing (a missing file still has a tab).
    File(PathBuf),
    /// Clipboard text with no file yet.
    Untitled(PasteId),
    /// The start page. A tab only, never a Document.
    Start,
}
```

`DocumentId` is allocated once and never reused. `WindowId` is process-monotonic. `TabId` is per-window and
never reused. `JobId` is global.

### 3.2 Source buffer

Use `String`, not a rope. At the sizes lmdv edits (< 5 MB) splice cost is negligible compared with text
shaping; a rope would complicate `TextEdit` synchronization and the parser's `&str` API.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol { Lf, CrLf }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileMeta {
    pub len: u64,
    pub mtime_ns: u64,   // since the Unix epoch; 0 when unknown
    pub dev: u64,        // Linux st_dev; 0 when unknown
    pub ino: u64,        // Linux st_ino; 0 when unknown
}

#[derive(Clone, Debug)]
pub struct SourceBuffer {
    text: String,
    line_starts: Vec<u32>,   // UTF-8 byte offset of each line start; always starts with 0
    revision: u64,
    eol: Eol,
    had_bom: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditDelta {
    pub start: usize,
    pub removed: usize,      // bytes
    pub inserted: usize,     // bytes
    pub first_line: usize,   // line containing start, before the edit
    pub line_delta: isize,   // added lines minus removed lines
}

impl SourceBuffer {
    pub fn text(&self) -> &str;
    pub fn revision(&self) -> u64;
    pub fn line_count(&self) -> usize;
    pub fn line_of(&self, byte: usize) -> usize;
    pub fn line_start(&self, line: usize) -> usize;
    pub fn line_range(&self, line: usize) -> Range<usize>;
    pub fn replace(&mut self, range: Range<usize>, with: &str) -> EditDelta;
    pub fn replace_all(&mut self, text: &str, eol: Eol) -> u64;
    /// LF-normalized; CRLF is only reintroduced when writing.
    pub fn set_eol(&mut self, eol: Eol);
    pub fn snapshot(&self) -> Arc<str>;
}
```

`line_starts` is rebuilt with `memchr` from `first_line` forward; at 200 KB this is < 0.1 ms. Keep
`revision` monotonic even if text becomes identical to an older revision; dirty is decided by hash, not
revision equality.

### 3.3 Document and registry

`Document` is shared and therefore **never handed out as `&mut`**. All mutation goes through `&self` methods
that borrow one interior cell at a time.

```rust
// lmdv-core/src/document.rs
pub struct Document {
    pub id: DocumentId,
    pub key: RefCell<DocumentKey>,
    pub title: RefCell<String>,
    pub buf: RefCell<SourceBuffer>,
    pub status: RefCell<DocStatus>,              // Loading | Ready | Error(String) | Gone
    pub saved: RefCell<SavedBase>,
    pub lease: RefCell<Option<EditLease>>,
    pub subscribers: RefCell<BTreeSet<WindowId>>,
    pub ignore_writes_until: Cell<Option<Instant>>,
    pub last_local_write: Cell<Option<FileMeta>>,
    // The latest good parsed model lives in AppShared::parse, keyed by DocumentId and revision;
    // lmdv-core must not depend on lmdv-md. Parse diagnostics live in MdDoc, not here.
}

pub enum DocStatus { Loading, Ready, Error(String), Gone }

pub struct SavedBase {
    pub revision: u64,
    pub hash: [u8; 32],       // blake3 of LF-normalized text
    pub path: Option<PathBuf>,
    pub meta: FileMeta,       // len, mtime_ns, dev, ino
    pub eol: Eol,
    pub had_bom: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditLease { pub window: WindowId, pub tab: TabId }

impl Document {
    pub fn revision(&self) -> u64;
    pub fn text(&self) -> Ref<'_, SourceBuffer>;
    pub fn snapshot(&self) -> Arc<str>;
    pub fn apply(&self, range: Range<usize>, text: &str) -> EditDelta;
    pub fn replace_all(&self, text: &str, eol: Eol) -> u64;
    pub fn hash(&self) -> [u8; 32];
    pub fn is_dirty(&self) -> bool;
    pub fn acquire_edit(&self, window: WindowId, tab: TabId) -> Result<EditLease, LeaseError>;
    pub fn release_edit(&self, lease: EditLease);
    pub fn lease_holder(&self) -> Option<EditLease>;
    pub fn set_saved(&self, meta: FileMeta, hash: [u8; 32], path: PathBuf);
    pub fn mark_gone(&self);
}
```

`hash` is computed on save, reload and lease release; not on every keystroke. `is_dirty` compares the current
text hash with `saved.hash`; callers may cache the answer for one commit.

Registry:

```rust
pub struct DocumentRegistry {
    docs: HashMap<DocumentId, Rc<Document>>,
    by_path: HashMap<PathBuf, DocumentId>,     // canonical, case-sensitive on Linux
    by_identity: HashMap<(u64, u64), DocumentId>, // (dev, ino) when known
    next_id: u64,
}

impl DocumentRegistry {
    pub fn get(&self, id: DocumentId) -> Option<Rc<Document>>;
    pub fn find_path(&self, path: &Path) -> Option<Rc<Document>>;
    pub fn find_identity(&self, meta: &FileMeta) -> Option<Rc<Document>>;
    pub fn insert_loading(&mut self, key: DocumentKey, title: String) -> Rc<Document>;
    pub fn insert_ready(&mut self, doc: Rc<Document>);
    pub fn rekey(&mut self, id: DocumentId, new_key: DocumentKey);
    pub fn collect_unused(&mut self) -> Vec<DocumentId>; // strong_count == registry + one
}
```

A `Document` may be referenced by many tabs in many windows. It is dropped only when the registry and every
tab release it. A start page has no `Document`.

### 3.4 Editor session and lease

```rust
pub struct EditorSession {
    pub edit: TextEdit,            // one selection, own undo history
    pub doc_revision: u64,         // revision the edit was based on
    pub base_rev: u64,             // last revision known saved when the session started
    pub eol: Eol,
    pub find: FindState,
    pub syntax: SyntaxCache,
    pub last_edit_at: Instant,
    pub block_mode: Option<BlockEditor>, // large-document mode
}
```

Rules:

1. `TabView.mode == Editor` is only legal while `doc.lease == Some(EditLease { this window, this tab })`.
2. `Ctrl+E` asks for the lease. If another window holds it, show a notice with **Go there** and **Take over**
   (take over is explicit; the other window drops to viewer and its session is discarded after a draft is
   flushed).
3. While the lease is held, edits go: `TextEdit` → `doc.apply(delta)` → `revision + 1` → parse debounce.
4. A viewer tab sees `doc.buf` directly; it never owns an `EditorSession`.
5. Closing the lease-holding tab/window releases the lease after the draft is queued. If the doc is dirty and
   this is the last view, the close prompt runs first.
6. Tab transfer moves the `TextEdit` (including its shared undo history) with the lease; the source waits for
   `TabAccepted` before dropping its tab.

This deliberately gives up *simultaneous independent editing* of the same file in two windows. The specification
allowed two CodeMirror buffers and then had to detect conflicts; one canonical buffer plus a lease is safer
and matches the user's explicit sharing requirement. Record it as a `substitute` in `FEATURE_PARITY.md`.

### 3.5 Tabs, views, windows

```rust
pub enum TabSource {
    Start,
    Document(Rc<Document>),
}

pub struct Tab {
    pub id: TabId,
    pub source: TabSource,
    pub view: TabView,
    pub title_cache: String,
    pub minutes: u16,              // reading time after the title
    pub last_shown: Instant,
}

pub struct TabView {
    pub anchor: ScrollAnchor,       // heading id + offset; robust across reloads
    pub mode: Mode,                 // Viewer | Editor
    pub editor: Option<EditorSession>,
    pub nav: NavHistory,            // back/forward, max 50 each
    pub find: Option<FindState>,
    pub toc_filter: String,
    pub tree_root: Option<PathBuf>,
    pub details_open: BTreeSet<String>, // <details> keyed by block path
    pub pending_goto: Option<Goto>, // line or heading to apply after first layout
    pub preview_scroll: f32,        // last editor→preview sync line
    pub scroll_target: f32,         // reading keys accumulate from a target, not the spring position
    pub section_line_hidden_until: Option<Instant>,
}

pub struct WindowState {
    pub id: WindowId,
    pub shared: Rc<AppShared>,

    pub tabs: Vec<Tab>,
    pub active: usize,
    pub closed: Vec<ClosedTab>,     // max 25

    pub left: PaneState,            // file browser
    pub right: PaneState,           // outline
    pub width: f32,                 // last root layout width
    pub ui: UiState,

    pub worker: Arc<WorkerMailbox>,
    pub ui_inbox: Rc<UiMailbox>,
    pub images: OnceCell<ImageCache>,
    pub math: RefCell<MathCache>,
    pub kit: RefCell<KitCache>,

    pub closing: ClosingState,
    pub last_focus: Instant,
}
```

`UiState` holds only transient, window-scoped surfaces:

```rust
pub struct UiState {
    pub palette: Option<PaletteState>,
    pub notice: Option<Notice>,
    pub dialog: Option<DialogState>,          // save conflict, draft restore, close confirm
    pub menu: Option<MenuState>,
    pub lightbox: Option<LightboxState>,
    pub table_preview: Option<TablePreviewState>,
    pub prefs_open: bool,
    pub share_open: bool,
    pub help_open: bool,
    pub keys_help: bool,
    pub focus: FocusArea,
    pub key_sequence: KeySequence,             // "g g", pending timeout
}
```

### 3.6 App-wide shared state

```rust
pub struct AppShared {
    pub dirs: AppDirs,                         // rune::dirs::AppDirs
    pub app_id: &'static str,
    pub prefs: RefCell<Prefs>,
    pub docs: RefCell<DocumentRegistry>,
    pub parse: RefCell<HashMap<DocumentId, Arc<MdDoc>>>, // latest good model per doc
    pub windows: RefCell<WindowRegistry>,
    pub store: StoreHandle,
    pub jobs: JobHub,
    pub watcher: WatcherHandle,
    pub instance: InstanceServer,
    pub next_window: Cell<u64>,
    pub quitting: Cell<bool>,
    pub startup: RefCell<StartupState>,        // initial launch queue, restore plan
}
```

`AppShared` is `Rc`, created on the UI thread before the first window. Worker threads only see the
`Send` services inside it (store sender, job senders, watcher sender, `Arc<WorkerMailbox>`); they never
receive `Rc` values or references into `AppShared`.

### 3.7 Preferences, search, exports

```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub schema: u32,
    pub theme: ThemePref,                 // System | Light | Sepia | Dark | Black
    pub reader: ReaderPrefs,              // size, font, line height, column width, side size
    pub panels: PanelPrefs,               // left_width, right_width, hidden
    pub print_break_h1: bool,
    pub locale: Option<String>,           // None = system
    pub restore_session: bool,
    pub editor: EditorPrefs,              // spellcheck, large_file_mode, format_bar
}

pub struct FindState {
    pub query: String,
    pub replace: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub matches: Vec<FindMatch>,          // source ranges + block index
    pub current: usize,
    pub replace_open: bool,
}

pub struct FindMatch { pub source: Range<usize>, pub block: usize, pub line: usize }

pub struct FolderSearchState {
    pub query: String,
    pub root: PathBuf,
    pub results: Vec<SearchHit>,
    pub active: Option<usize>,
    pub token: u64,                 // invalidates late results
    pub running: bool,
}

pub struct SearchHit {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub snippet: String,
    pub ranges: Vec<Range<usize>>,
}

pub struct ExportRequest {
    pub id: JobId,
    pub doc: DocumentId,
    pub revision: u64,
    pub kind: ExportKind,
    pub target: PathBuf,
    pub options: ExportOptions,
}

pub enum ExportKind {
    Html, Docx, Pdf, Print,
    DiagramPng { source: String, width: f32, scale: u32 },
    ImagePng { path: PathBuf, scale: u32 },
}

pub enum StorePatch {
    Prefs(Prefs),
    Recents(Vec<Recent>),
    Positions(PositionTable),
    Session(SessionSnapshot),
    Ui(UiPrefs),
    Draft(Draft),
    RemoveDraft(String),
}

pub struct StoreHandle { tx: mpsc::Sender<StoreCmd> }
impl StoreHandle {
    pub fn patch(&self, patch: StorePatch);       // non-blocking, coalesced by file
    pub fn flush(&self, timeout: Duration) -> bool; // close/quit only
}
```

### 3.8 Ownership and borrow rules (the part workers must memorize)

1. **`Rc` for UI-shared state, `Arc` only at the worker boundary.** `Rc<Document>` is never sent to a worker.
   A job receives `DocumentId`, `revision`, and an `Arc<str>` snapshot.
2. **Never hold a `Ref`/`RefMut` across a call that could take the same cell.** Clone the `Rc<Document>` in a
   small block, then borrow one field. Example:

```rust
fn toggle_task(s: &mut WindowState, doc: DocumentId, range: Range<usize>) {
    let Some(doc) = s.shared.docs.borrow().get(doc) else { return };
    let (start, end, before) = {
        let buf = doc.buf.borrow();
        let start = range.start;
        let end = range.end;
        let before = buf.text()[start..end].to_owned();
        (start, end, before)
    }; // buf borrow ends here
    let after = if before.contains("[x]") { before.replace("[x]", "[ ]") } else { before.replace("[ ]", "[x]") };
    let delta = doc.apply(start..end, &after);
    s.on_doc_edited(doc.id, delta);
}
```

3. **Handlers do not call `&mut self` helpers while a `RefMut` is alive.** Either the helper takes only the
   borrowed struct (`fn edit(buf: &mut SourceBuffer, …)`) or the borrow is dropped first.
4. **Commands are synchronous.** A command may enqueue a job, but it must finish mutating UI state before it
   returns; there is no “post-command” hook.
5. **Views are pure.** A view may read `doc.buf` and `cx`, and may read live values; it never writes state or
   starts a job. Exceptions: `El::on_layout`/`on_scroll_offset` handlers, which run after layout, not in the
   view.
6. **Document methods return `Rc` clones, not guards.** Public API surface:

```rust
impl WindowState {
    pub fn doc(&self, id: DocumentId) -> Option<Rc<Document>> {
        self.shared.docs.borrow().get(id)
    }
    pub fn active_doc(&self) -> Option<Rc<Document>> {
        match self.tabs.get(self.active)?.source {
            TabSource::Document(ref d) => Some(d.clone()),
            TabSource::Start => None,
        }
    }
}
```

7. **No `unwrap` in UI paths.** A missing document after a close race is normal; return `Option` and show a
   notice.

### 3.9 Which sharing primitive to use

| Primitive | Use it for | Do not use it for |
|---|---|---|
| `Rc<Document>` + `RefCell` fields | Canonical document text, key/path, saved base, lease, per-doc dirty state. One per file per process. | Copying a document per tab; holding a guard across UI calls. |
| `Rc<AppShared>` + `RefCell` | Registries, prefs, parse cache, window summaries. All UI-thread-only. | Anything a worker thread reads. |
| `Rc<OnceCell<ImageCache>>` (per window) | The window's image/math decode cache, created in `setup` with the window's `Waker`. | A process-global cache created before any window exists. |
| `Arc<WorkerMailbox>` | Job completions and watcher events from threads. Contains a `Mutex<VecDeque<JobMsg>>` and a `Mutex<Option<Waker>>`. | UI messages (`UiMsg`), which carry `Rc`/`TextEdit`. |
| `Rc<UiMailbox>` (per window) | Same-thread cross-window commands (`Open`, `ActivateDocument`, `PrefsChanged`, tab offer). | Worker threads. |
| `rune::Live<T>` / `Binding<T>` | Per-frame values that must move without a view rebuild: reading progress, section-line offset, lightbox pan/zoom, a sticky fade. | State the view's structure depends on (active tab, open palette). A `Live` change must not need new elements. |
| `mpsc` channels | Job queues and store writer. Bounded; latest-wins per doc for parse. | Sending UI state or `Rc` values. |
| `Tx::wake_after` | Debounces and timers. Store deadline in state; wake the window. | Busy-polling in `poll`. |

`Live` is not a replacement for app state: a value that selects a different element tree must go through
`RefCell` + a commit. A value that only moves a rectangle (progress, pan, zoom, sticky line) is a `Live`.

---

## 4. Event, command and async architecture

### 4.1 Command registry

```rust
pub type CmdFn = fn(&mut WindowState, &mut Tx);

pub struct CommandSpec {
    pub id: &'static str,                 // stable, English, never localized
    pub title: &'static str,              // i18n key
    pub category: Category,               // File, Edit, View, Tab, Window, Export, Help
    pub keywords: &'static [&'static str],// English + localized synonyms
    pub default_keys: &'static [&'static str], // Chord syntax, e.g. "mod+s", "g g"
    pub run: CmdFn,
    pub enabled: fn(&WindowState) -> bool,
    pub checked: fn(&WindowState) -> bool,
}

pub static COMMANDS: &[CommandSpec] = &[ ... ];

pub fn dispatch(s: &mut WindowState, tx: &mut Tx, id: &str) {
    let Some(cmd) = COMMANDS.iter().find(|c| c.id == id) else { return };
    if !(cmd.enabled)(s) { return; }
    (cmd.run)(s, tx);
    s.ui.note_command(id);
}
```

- Menu items, toolbar buttons, context menus, the palette and shortcuts all dispatch by ID.
- Shortcut strings are parsed once into `Chord`s; `Chord::matches(&KeyEvent)` handles sequences such as
  `g g`. The menu uses `rune::Shortcut` for display; keep the two tables generated from one literal to avoid
  drift.
- `CommandSpec::enabled` is cheap; never probes disk.
- Native menu items are not used on Linux. A `MenuBar`/`Menu` in the toolbar renders `COMMANDS` grouped by
  category; the same items appear in the palette.

### 4.2 Key routing order

Rune calls, in order:

1. Focused text field (`TextEdit`) and its `on_input_key`.
2. `AppSpec::on_key` for unconsumed keys.
3. Window-level shortcuts (registered by Rune) — not used for app commands; keep one path.

`lmdv-app::on_key` therefore implements:

```text
if dialog/palette/lightbox/table-preview/menu is open:
    modal handler; consume or close
else if editor focused:
    editor-specific keys (Ctrl+S/E/F/H, formatting, line ops) are handled by on_input_key;
    on_key only handles global chords that the field deliberately ignores
else if reading keys:
    j/k, . ,, g g, G, n/N, ?, Esc, space/shift-space
else:
    chord table (first match wins) -> dispatch
```

Editor `on_input_key` returns `true` for a handled command and `false` for typing. This is the only place
where the editor's own key semantics can win over global shortcuts (e.g. `Ctrl+B` bolds inside the editor,
toggles the file browser elsewhere).

### 4.3 Per-window mailboxes

```rust
pub struct WorkerMailbox {
    pub id: WindowId,
    jobs: Mutex<VecDeque<JobMsg>>,
    waker: Mutex<Option<Waker>>,
}

pub struct UiMailbox {
    msgs: RefCell<VecDeque<UiMsg>>, // UI thread only
}

impl WorkerMailbox {
    pub fn attach(&self, waker: Waker);       // AppSpec::setup
    pub fn post(&self, msg: JobMsg) { ...; self.wake(); }
    fn wake(&self) { if let Some(w) = &*self.waker.lock()... { w() } }
}
```

`WindowState::poll`:

```rust
fn poll(s: &mut WindowState, now: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    for job in s.worker.drain() { changed |= s.handle_job(job, tx); }
    for msg in s.ui_inbox.drain() { changed |= s.handle_ui(msg, tx); }
    changed |= s.pump_timers(now, tx);
    changed
}
```

- `handle_job` returns whether the view reads anything that changed. A store acknowledgement returns false.
- Async completions use `tx.instant()` so a file reload does not animate from the old layout.
- `poll` never blocks; a job that needs UI feedback sends another message.

### 4.4 Job types and generations

| Job | Payload in | Completion | Stale guard |
|---|---|---|---|
| `Load` | path, doc id, window mailbox | `Loaded { doc, snapshot, meta, eol, bom }` or error | registry still has doc and status Loading |
| `Save` | path, bytes, expected meta, create? | `Saved { doc, meta, hash }` or `SaveConflict`/`SaveFailed` | doc id + request id |
| `ProbeImage` | path, policy | width/height/format or error | request id |
| `Parse` | doc id, revision, `Arc<str>`, options | `ParseDone { revision, Arc<MdDoc> }` | revision equality |
| `Search` | root, query, case, token | `SearchDone { token, hits }` | token/generation |
| `Export` | `ExportRequest` | `ExportEvent` (progress/done/failed) | job id + cancellation token |
| `Watch` | events | `FileEvent { doc, kind, path, meta }` | doc identity/revision at apply time |
| `Store` | patch | `Stored`/`StoreError` | none (coalesced) |

`JobHub` owns one thread per class (parse: 2 workers, io: 2, search: 1, export: 1). Each queue is bounded;
parse jobs are **latest-wins per document** (drop an older parse for the same doc). Do not spawn a thread per
keystroke.

```rust
pub struct JobHub {
    parse: JobQueue<ParseJob>,
    io: JobQueue<IoJob>,
    search: JobQueue<SearchJob>,
    export: JobQueue<ExportJob>,
}

pub struct ParseJob {
    pub doc: DocumentId,
    pub revision: u64,
    pub source: Arc<str>,
    pub mailbox: Arc<WorkerMailbox>,
}
```

### 4.5 Cross-window operations

No handler reaches into another window's `&mut WindowState`. It posts a `UiMsg` to the target:

```rust
pub enum UiMsg {
    ActivateDocument(DocumentId),
    Open(OpenRequest),
    PrefsChanged(Prefs),
    OfferTab(TabOffer),
    TabAccepted { token: u64, ok: bool },
    FocusWindow,
    CloseWindow,
}
```

`send_ui(shared, target, msg)` pushes into `target.ui_inbox` and calls `target.worker.wake()`. The target's
`poll` handles it next frame. For a new window, build `WindowState` with the tab already inside and call
`open_window`; the target posts `TabAccepted` from `on_ready`.

**Routing algorithm** (port of `src-tauri/src/windowing.rs`):

```text
route(launch, --new-window):
  if new_window: build new window; done
  if launch has file:
     1. window with a tab for that canonical doc -> ActivateDocument + focus
     2. window that is idle on a start page (no pasted text, not closing) -> Open there
     3. most-recently-focused open window -> Open there
     4. any ready window -> Open there
     5. build new window
  else:
     most-recently-focused open window -> focus; else build new
```

A file that does not exist still routes by absolute path; the target shows “Not found: name”. A burst of
launches is serialized on the UI thread; each launch claims its doc in the registry, so no duplicate tabs.

**Broadcast rules for a shared document.** A change that other windows can see is not complete until every
subscriber knows:

| Change | Origin | Broadcast |
|---|---|---|
| `doc.apply` from the lease holder | editor handler | `DocChanged { doc, revision, kind: Edited }` to every subscriber except self; each drops its parsed model if older and rebuilds viewers. |
| Save success | IO completion | `DocChanged { kind: Saved, meta }`; update dirty dots, clear draft notices, update watcher suppression. |
| Reload / rename / delete | watcher or explicit | `DocChanged { kind: Reloaded/Renamed/Gone }`; tabs update title/path/anchor. |
| ParseDone | parse worker completion | store `AppShared.parse[doc] = model`; `DocParsed { doc, revision }` to every subscriber; each view uses the shared model. |
| Lease acquired/released | command | `LeaseChanged`; other windows switch edit controls/banner. |

`Document.subscribers` is maintained by tab open/close/transfer. If a subscriber is itself the origin, it
applies directly and is excluded from the broadcast. A broadcast to a closed window is dropped by the
registry lookup; no error.

### 4.6 Timers and debouncing

Use `tx.wake_after` and store deadlines in `WindowState`/`Document`; do not busy-poll.

| Work | Delay | Notes |
|---|---|---|
| Live preview parse | 120 ms after last edit | keep last good `MdDoc` |
| Draft write | 1 s after last edit | matching the specification; also on tab switch/close |
| Reading position | 300 ms after scroll stops | plus tab switch/close |
| Session/tab snapshot | 500 ms after tab change | coalesced by store |
| Preview scroll sync | 80 ms throttle | editor→preview |
| Section-line reveal | 120 ms after slow scroll | hide during fast scroll |
| Own-write suppression | 1.5 s per path | watcher echoes |
| `g` sequence timeout | 500 ms | `g g` |
| Search/export | no timer; explicit | cancellable |

`pump_timers(now, tx)` runs in `poll`; `tx.wake_after` is set when the deadline is scheduled, and again if
the deadline moves.

---

## 5. Markdown pipeline

### 5.1 Stage diagram

```text
Document buffer (LF)
   │  snapshot Arc<str>
   ▼
lmdv-md::parse (worker)
   ├─ front-matter scan/parse (flat YAML, specified semantics)
   ├─ math scan → placeholder pass (same byte length, line count preserved)
   ├─ pulldown-cmark (CommonMark + GFM tables/tasks/strike/footnotes)
   ├─ html5ever fragment parse for allowed raw HTML
   ├─ callout detection on block quotes
   ├─ heading text extraction → GitHub slug + legacy slug
   ├─ inline source maps (rendered byte → source byte)
   └─ footnote/TOC/diagnostic collection
   ▼
Arc<MdDoc> (Send, no Rune)
   │
   ▼
lmdv-render::render (UI)
   ├─ DocTheme + width + font
   ├─ memo("b{i}", revision/theme, || block)
   ├─ images via ImageResolver (ImageCache)
   ├─ math via MathResolver (RaTeX → ImageCache.svg)
   ├─ mermaid via rune_mermaid (Drawing/render)
   └─ El tree with keys, links, selection maps, source-line hooks
```

### 5.2 Why fork

`rune-kit::markdown` at this rev:

- enables only strike/tables/tasks (`parser.rs:466`);
- drops raw HTML entirely;
- has no footnotes, front matter, heading IDs, TOC, callouts, or math;
- renders tables as a single element, so a sticky header cannot be attached to the page viewport;
- lifts inline images to blocks;
- has no extension hooks at the block model level.

The fork is bounded: parser ~1.5k lines, renderer ~2k lines. Copy the upstream files into the new crates with
a provenance header naming the Rune rev, then change them. Keep `UPSTREAM.md` with the exact file list and a
diff summary. Do not copy the streaming cache initially; a full parse per debounce is enough for viewer and
preview. Revisit incremental parsing only after the perf gate.

### 5.3 `MdDoc` model

```rust
pub struct MdDoc {
    pub source: Arc<str>,
    pub revision: u64,
    pub blocks: Vec<Block>,
    pub block_spans: Vec<BlockSpan>,
    pub headings: Vec<Heading>,
    pub footnotes: Vec<Footnote>,
    pub front_matter: Option<FrontMatter>,
    pub diagnostics: Vec<Diagnostic>,
    pub line_starts: Arc<[u32]>,
}

pub struct BlockSpan {
    pub key: String,             // "b{index}" (stable within this doc revision)
    pub source: Range<usize>,
    pub line: usize,
    pub end_line: usize,
}

pub struct Heading {
    pub level: u8,               // 1..=6
    pub id: String,              // GitHub slug, deduplicated
    pub legacy_id: Option<String>,
    pub text: String,            // plain text, no markers
    pub block: usize,
    pub line: usize,
    pub in_toc: bool,            // h1..h4
}

pub enum Block {
    Para(Inline),
    Heading { heading: usize, inline: Inline },
    Code { lang: Option<String>, text: String, source: Range<usize> },
    List { ordered: bool, start: u64, tight: bool, items: Vec<Vec<Block>> },
    Quote { blocks: Vec<Block>, callout: Option<Callout> },
    Table(Table),
    Rule,
    Mermaid { source: String, line: usize },
    MathBlock { tex: String, line: usize },
    HtmlBlock(HtmlNode),
    Image(ImageBlock),
    FootnoteDefs,                // rendered as the generated footnotes section
}

pub struct Inline {
    pub text: String,            // what is drawn; placeholders U+FFFC for math/objects
    pub spans: Vec<InlineSpan>,  // style runs
    pub links: Vec<InlineLink>,
    pub math: Vec<InlineMath>,   // range in text + tex
    pub objects: Vec<InlineObject>, // html <img>/<br>/<kbd>/<sub>…
    pub source_map: SourceMap,   // rendered byte -> source byte, monotone segments
}

pub struct Table {
    pub align: Vec<Align>,
    pub head: Vec<Inline>,
    pub rows: Vec<Vec<Inline>>,
    pub source: Range<usize>,
    pub line: usize,
}

pub struct FrontMatter {
    pub raw: String,
    pub entries: Vec<(String, FmValue)>, // flat only; nested/multiline -> invalid
    pub valid: bool,
}
```

`BlockSpan.key` is used for scroll-into-view and source mapping. For headings also give the Rune element a
key `h{heading_index}` so outline jumps can target the heading itself, not the block wrapper.

### 5.4 Extension algorithms

**Front matter.** If source starts with `---` (allowing a BOM already stripped) and a closing `---`/`...`
line exists before any other block, parse the flat YAML subset exactly as `src/frontmatter.ts` does:
`key: value`, `key: [a, b]`, `key:` plus indented `- item`, quoted strings, comments. Nested maps, block
scalars and odd syntax mark `valid=false` and store raw. Replace the front-matter lines with blank lines in
the parse input so every later source offset and line number is unchanged. Render a collapsed
`<details>`-like card at the top with key chips; the card is not a document table and is skipped by table
preview/export-as-table.

**Math.** Port the exact GitHub delimiter rules from `src/renderer.ts`:
`$$` block only if closed before a blank line and nothing follows the closing `$$` on its line; inline `$$`
inside a paragraph; `$…$` only with no space inside, no word char adjacently, escaped dollars ignored;
`$`\`…\`$` supported; ` ```math ` with a title treated as math. Implement a scanner that recognizes fenced
code, inline code, HTML comments and escapes, then rewrites math spans into same-length placeholders:

- replace the first **three** bytes of the math span with U+FFFC (a valid 3-byte UTF-8 character), so the
  transformed source has exactly the same byte length;
- replace all remaining non-newline bytes with ASCII spaces;
- keep newlines so line numbers are preserved;
- record `MathSpan { source, tex, display, line }` and map the U+FFFC's rendered offset to `source.start`.

A candidate math span shorter than 3 bytes (never a real `$…$`) is left as text. The scanner must run
outside fenced code, inline code and HTML comments, and must respect escaped dollars.

Because the byte length is unchanged, pulldown-cmark's offsets, block spans and heading lines remain exact.
The parser sees a normal paragraph with an object char; `Inline.math` records its range. The renderer asks
`MathResolver` for an image. If the formula fails, show the raw TeX in a code chip and a diagnostic; never
drop the paragraph.

**Callouts.** On a `BlockQuote`, inspect the first inline's plain text for `[!NOTE]`, `[!TIP]`,
`[!IMPORTANT]`, `[!WARNING]`, `[!CAUTION]` (case-insensitive), optionally followed by a title on the same
line. Remove the marker and leading space; parse the rest of the quote normally. Render a tinted card with
the localized default title, an icon, and the body blocks. Nested lists/quotes work because the body is a
normal block list. `role("note")`, label = title.

**Heading anchors.** Compute from the heading's plain text exactly like `headingSlug`: lowercase; keep
Unicode `L`, `M`, `N`, `Pc`, space, hyphen; spaces → `-`; drop everything else; dedupe with `-1`, `-2`; if
empty, fall back to the legacy slug. Also compute `legacy_id = percent_encode(trim.lower.replace(/\s+/-/))`
for old reading positions. If an id collides with an app-reserved id, prefix `user-content-` and keep the
original in `data-anchor` semantics in `Heading` (we do not have DOM, so store both).

**TOC.** A paragraph whose entire plain text matches `\[\[?toc\]?\]` (case-insensitive) becomes a
`TocPlaceholder` marker. The renderer builds a nested nav from the `headings` after that block, with the top
level equal to the minimum level among them; each link is `Link::new(range, "toc:<id>")`. The nav is a real
document block: it prints and exports.

**Footnotes.** Enable `ENABLE_FOOTNOTES`; map references to superscript text with a `Link` id `fn:<label>`,
and collect definitions into `footnotes`. At the end, append a rule and a footnotes section: each entry has a
backlink to the first reference. If a reference has no definition, render the literal text and a diagnostic.
Duplicate definitions: last wins, diagnostic.

**HTML subset.** Pre-scan for GitHub's tagfilter set and escape those tags to text. Parse each raw HTML
block/inline with `html5ever` into a temporary tree; convert to `HtmlNode` with an allowlist:
`a, abbr, b, blockquote, br, code, del, details, div, em, i, img, ins, kbd, li, mark, ol, p, picture, pre,
s, source, span, strong, sub, summary, sup, table, tbody, td, tfoot, th, thead, tr, u, ul, video, audio,
track`.
Attributes: `href, src, alt, title, width, height, media, type, start, colspan, rowspan, align, poster,
controls, loop, muted, preload, kind, srclang, label, class` (class only for
`gh-dark-mode-only`/`gh-light-mode-only`), `id` (namespaced). Drop `style`, event handlers, `popover`,
`command*`, `interestfor`, `srcset` unless parsed by our own candidate parser. For unknown tags, keep their
children as text; for `script/style/iframe/object/embed/noscript`, drop the element and its content.
`<details>` state lives in `TabView.details_open` keyed by block path + element index. `<video>/<audio>` on
Linux render a poster/audio card with an “Open externally” action, not native playback (Rune has no Linux
video backend in this rev).

**Autolinks.** Apply a small `linkify` pass to plain text runs (not code/math/HTML): `http://`, `https://`,
`www.` with the specified boundary rules (not inside a path or longer host), plus `<mail@x>` from pulldown.
Split the inline into text + link spans and preserve the source map. Bare domains that look like filenames
(`README.md`, `main.rs`) stay text.

**Tables.** Keep pulldown's alignment; build `Table` with rich `Inline` cells. Renderer: outer horizontal
scroll viewport, header row as a sibling with `sticky_top(0)`, fixed column widths computed from a content
heuristic (min 56 pt, max 360 pt, total capped to window width; wrap cells). Header/body share the same
widths. Add a header fill and `stuck_shadow` divider. A table wider than the window scrolls; the app adds a
pinned horizontal scrollbar at the bottom of the pane when a wide table is in view (phase 6). Do not use
`rune-kit::Table` for document tables; use it for the full-window table preview, where virtualization and
sorting are already solved.

**Images.** Block-level (as the specification). Parser stores `src`, `alt`, `title`, optional `srcset`/`picture`
candidates. The renderer resolves through `ImageResolver`, which enforces the path policy and returns an
`Image` or a missing-state. `ImageCache.file(path, draw_width, scale)` does decoding off-thread; until ready,
draw the broken-image glyph + alt exactly like Rune's Markdown. `picture` chooses a source by theme;
`srcset` picks 2x on scale ≥ 1.5. Remote URLs are never fetched: show alt + URL and a “Copy link” action.

### 5.5 Source mapping, scroll spy, click-to-source

- `SourceMap` maps each rendered inline byte to a source byte, with monotone segments; block-level maps cover
  code and tables.
- Editor→preview sync: on editor scroll/caret, map the first visible line to `block_spans[index].key`, then
  `Tx::scroll_key_to(key, 0)` (throttled 80 ms). Do not call on every keystroke.
- Preview→editor: a double-click on selectable text gives `Tx::text_selection()` with the selected word's
  rendered range; map it through the block's `SourceMap`; set `TextEdit` caret/selection and switch to edit
  mode if the tab is already editing. If Rune does not deliver the double-click selection reliably, add the
  `0003-text-hit.patch`.
- Scroll spy: each heading wrapper gets `on_layout` writing its y into `TabView`/window scratch; the article
  viewport's `on_scroll_offset` computes the active heading at the activation line (90 pt, sliding to the
  bottom over the last viewport). If the active heading changes, mutate state (one rebuild); otherwise
  `tx.live_only()`. Cache y values in a `Vec<f32>` keyed by heading index; do not read all `Cx::layout`
  every frame.
- Anchors: `Heading.id` is the source of truth. `#id` links resolve through a `HashMap<String, usize>` in
  `MdDoc`. A link to a heading calls `scroll_key_to("h{i}", 0)` and flashes the heading via a short
  `TextHighlight`/live color.
- Reading position: store `ScrollAnchor { id: heading_id, offset: f32 }`, where `offset` is the heading's top
  minus the viewport top at capture (negative if above). Restore with `scroll_key_to("h{i}", -offset)`; if the
  heading is gone, fall back to absolute offset.

### 5.6 Caching

| Cache | Key | Bound |
|---|---|---|
| Parsed `MdDoc` | `(source_hash, options_version)` | 2 entries / 16 MiB per process (latest doc + previous) |
| Rendered block | `memo("b{i}", hash(revision, theme, width_bucket, callbacks))` | Rune memo, dropped when block disappears |
| Syntax spans | `(text_hash, theme)` | 8 entries |
| Mermaid | `(src_hash, theme, width_bucket)` | 16 scenes (Rune already has one) |
| Math SVG | `(tex_hash, display, size_bits, color_bits)` | 256 entries / 32 MiB |
| Math raster | `ImageCache` content hash | Rune's image cache, per-window |
| Image probe | `(path, mtime, size)` | 512 entries |
| Link resolution | `(dir, href)` | 512 entries, only hits |

Never cache `El` outside `memo`; `El` handlers capture state and go stale.

### 5.7 Parsing failure behavior

- Parse worker wraps the whole parse in `catch_unwind`; a panic returns `Err(Diagnostic)`.
- The app keeps the last good `Arc<MdDoc>` and shows a dismissible notice: “Preview couldn't update; showing
  the last good version.” This matches the specification's “never blank the pane”.
- Mermaid/math failures are local: fall back to source code/TeX with a diagnostic, not a whole-document error.
- The parser never executes HTML or touches the filesystem.

### 5.8 Math and diagram assets

**Math (`lmdv-math`).**

```rust
pub struct MathImage {
    pub image: rune::Image,     // supplied by the window's ImageCache
    pub width: f32,             // points at the requested font size
    pub height: f32,
    pub depth: f32,             // descent below the baseline
}

pub trait MathResolver {
    fn inline(&self, tex: &str, size_pt: f32, color: Color, scale: f32) -> MathImage;
    fn display(&self, tex: &str, max_width: f32, color: Color, scale: f32) -> MathImage;
}
```

- Pipeline: `ratex_parser::parse` → `ratex_layout::layout(…, LayoutOptions { style, color, .. })` →
  `to_display_list` → `ratex_svg::render_to_svg` (`standalone` + `embed-fonts`) → `ImageCache::svg(bytes,
  width_pt, scale)`.
- Metrics are in em. `LayoutBox { width, height, depth }` gives exact width/ascent/descent; multiply by the
  resolved point size. The inline image's `depth` drives `Span::image`'s baseline; display images are laid
  out with the same width/height and a max-width scale.
- Cache key: `(blake3(tex), display, size_bits, color_bits, scale_bits)`. Keep SVG strings in an LRU
  (32 MiB) and rasters in the window's `ImageCache`.
- Parse errors render the TeX in a code chip with a `Diagnostic`; they never abort the block.
- Accessibility: `role("math")`, label `"Math: <tex>"`, description for display.
- For DOCX, the same `lmdv-math` layer exposes the `ParseNode` tree to the OMML converter; if a construct is
  unsupported, use the PNG fallback and record it in the export report.

**Mermaid.**

- `rune_mermaid::render_with_hover` for the document card, `render_drawing` for the lightbox. Theme from
  `DocTheme`; cache by `(src_hash, theme, width_bucket)`.
- Unknown/too-large diagrams fall back to a code block with the source, as Rune does.
- PNG export uses `rune::render::{Gpu::headless, Renderer, Offscreen, OFFSCREEN_FORMAT, write_png}` with a
  tiny `Runtime<DiagramState>` on the export worker; the view builds the `Drawing` El. There is no SVG export
  from Mermaid at this rev; treat SVG as optional and document the gap.

### 5.9 Export pipeline

One `ExportDoc` feeds all formats; it is pure data and can be built from `MdDoc` + assets without Rune.

```rust
pub struct ExportDoc {
    pub title: String,
    pub lang: &'static str,
    pub blocks: Vec<ExportBlock>,
    pub footnotes: Vec<ExportFootnote>,
    pub assets: Vec<ExportAsset>,       // images/math/diagrams, by id
    pub source_path: Option<PathBuf>,
}

pub enum ExportBlock {
    Para(Vec<ExportRun>),
    Heading { level: u8, id: String, runs: Vec<ExportRun> },
    List { ordered: bool, start: u64, items: Vec<Vec<ExportBlock>> },
    Quote { callout: Option<CalloutKind>, blocks: Vec<ExportBlock> },
    Code { lang: Option<String>, text: String },
    Table { head: Vec<Vec<ExportRun>>, rows: Vec<Vec<Vec<ExportRun>>> },
    Math { id: AssetId, display: bool, tex: String },
    Image { id: AssetId, alt: String, title: String },
    Diagram { id: AssetId, source: String },
    Rule,
}

pub enum ExportRun {
    Text { text: String, style: RunStyle },
    Link { text: String, href: String },
    Code { text: String },
    Math { id: AssetId },
}
```

- **HTML**: one self-contained file, light theme CSS, inline `<style>`, base64 assets, RaTeX inline SVG,
  Mermaid PNG, anchors/footnotes/TOC, no JS, sanitized hrefs. Print CSS mirrors the PDF page rules. This is
  *our* serializer over `ExportDoc`, not Rune's `--html` scene exporter (which cannot emit math/diagram
  pixels and has no semantic table/code tags).
- **DOCX**: OOXML zip written directly (`[Content_Types].xml`, `_rels`, `word/document.xml`, styles,
  `word/media/*`). Headings map to Heading1–6, tables to `<w:tbl>`, code to monospace shaded paragraphs,
  images to drawing relationships, callouts to a shaded one-cell table. Math: translate the RaTeX AST to
  OMML for the supported subset; unsupported formulas become a PNG with alt text. Validate with LibreOffice
  and Word; record fidelity per construct.
- **PDF/print**: generate Typst source from `ExportDoc` (escaped text, headings, lists, tables, code, images,
  math via `#image(svg)`, Mermaid via PNG), compile in memory with `typst` + `typst-pdf`, A4, margins,
  title/date header, page numbers, optional page break at H1. Print = write a temp PDF under `cache/print`
  and open it with `xdg-open`/portal OpenURI.
- **PNG** (diagram/image/preview): offscreen Rune runtime at 2x/4x, crop to the element's laid-out size,
  `write_png`; never screenshot the window.
- Export jobs are cancellable and report `ExportEvent::{Progress, Done, Failed}`; only one file write per
  target, atomic; a failed export leaves no partial file (write temp + rename).

---

## 6. Editor design

### 6.1 Two modes, one model

`EditorSession` always owns a `TextEdit`; the *rendering* switches:

- **Single-field mode** (default): one `text_input` for the whole document. Full selection across blocks,
  TextEdit undo, IME. Use while `text.len() <= 64 KiB && lines <= 1_500`.
- **Block mode** (large): one `TextEdit` per blank-line-separated block, inside a virtualized column. Each
  block has its own TextEdit history; `EditorSession` keeps a global journal so `Ctrl+Z` still undoes across
  blocks. Selection is within a block (documented substitute). Switch to block mode automatically above the
  threshold; a pref can force it.
- The threshold is a constant in `lmdv-edit`; tune with `long_note`-style benchmarks on the target machine.
  If typing in single-field mode exceeds 16 ms p95 at 32 KiB, lower the threshold.

Canonical text is always LF. `EditorSession` never owns CRLF; `Eol` lives in the document/saved base. On
commit, apply the edit delta to `doc.buf`; on save, convert LF → CRLF if `Eol::CrLf`.

**Edit-mode layout.** A horizontal split with the source pane on the left and the live preview on the
right, a draggable 8 pt divider (double-click resets 50/50), and the format bar above the source. The
preview is the same article renderer as viewer mode, fed the latest good `MdDoc`; while a parse is pending
it keeps the previous one and shows a subtle “updating” dot, never a blank pane. Below 760 pt, show the
source alone with a toolbar toggle to the preview; the divider width is persisted in `Prefs.panels`.
The editor's caret scroll and the preview's scroll viewport are independent; sync is one-way from the
visible source line to the preview block (throttled), plus preview double-click back to the caret.

### 6.2 Single-field wiring

```rust
text_input(&session.edit, editor_style)
    .key("editor")
    .w_full()
    .spans(syntax_spans(&session, &theme))
    .on_edit(|s: &mut WindowState| s.active_editor_mut())
    .on_input_key(|s, ev, tx| s.editor_key(ev, tx))
    .on_input_change(|s, tx| s.commit_editor_change(tx))
    .on_submit(|s, _| s.insert_editor("\n"));
```

- Put the field inside `col().key("editor-scroll").scroll(...)`, with the field full width. Test caret
  auto-scroll on real Rune; if the field does not keep the caret visible, wrap with `auto_grow` or add a
  small `scroll_into_view` based on `Cx::caret`.
- `on_input_change` gets the new text; apply `doc.apply` from the previous text. Avoid diffing with a full
  LCS: `TextEdit` can expose only the new string; compute a common-prefix/suffix delta in O(n), which is
  enough for insert/backspace/paste. For block mode, the block's range bounds the splice.
- `on_selection_change` stores the caret/selection for preview sync and find-next.
- `on_submit` inserts `\n` (plain Return is a paragraph break, as specified).
- `Tab` leaves the editor? Spec “Esc, then Tab”. Rune fields consume Tab? Implement `Esc` blur; then Tab
  moves focus. The exact behavior is in the shortcut matrix; test it.

### 6.3 Syntax highlighting

`rune_highlights::highlight(Some("markdown"), text)` returns token ranges in 0.1–0.6 ms/1000 lines for the
first 128 KiB. Map tokens to `Kit`'s `SyntaxTheme`, shift ranges by the editor's start offset, and pass as
`El::spans`. For text >128 KiB, highlight the visible window only (based on editor scroll offset) and shift;
the rest stays plain. Recompute on a 120 ms debounce, not per keystroke.

### 6.4 Find and replace

- `lmdv-edit::find(text, query, opts) -> Vec<Range<usize>>`, literal by default; optional regex behind a
  toggle (`regex` crate) and whole-word/case flags.
- Highlight all matches with `TextHighlight` (paint-only, no reshape); current match uses a stronger color.
- Enter/Shift+Enter or `n/N` move `current`; set `TextEdit` selection to the match; `scroll_into_view` the
  active match key.
- Replace: one `TextEdit::undo_group` around all replacements from the end backwards; preserve selection.
- The find bar is per tab (`TabView.find`); it survives tab switches, not restarts (matching the specification).
- Viewer find uses `lmdv-md::find` over the rendered inline text, so `**` markers never match. It returns
  source ranges and block keys; renderer paints `TextHighlight`s.

### 6.5 Line and formatting commands

All commands edit source text only; the file stays plain Markdown.

- `toggle_wrap(marker)`: detect surrounding marker run; wrap/unwrap selection; preserve selection.
- `set_heading(level)`: strip existing `#{1,6} `, add prefix; keep indentation.
- `toggle_line_prefix`: bullet/ordered/task/quote; handle multi-line selections with the minimum indent;
  ordered lists renumber.
- `move_line_up/down`, `duplicate_line`, `delete_line`, `select_line`: use `TextEdit::line_range` and
  `replace`; one undo group. Preserve final newline.
- `toggle_comment`: wrap the selected/current lines in `<!-- … -->` (or the fenced code language's comment if
  the caret is inside a fence); uncomment if already wrapped.
- `insert_link`: `[selection](url)`; if no selection, placeholders.
- Format bar and `/` menu dispatch the same command IDs.
- Slash menu: at line start after `/`, show a filtered popup over the editor; insert templates (table,
  mermaid, math, code, callout, heading). Keyboard: Up/Down/Enter/Esc.

### 6.6 Multi-cursor

Do not fake it. The pinned `TextEdit` has exactly one anchor/focus pair and no public offset→rect API.

Staged plan:

1. **Phase 3:** single selection. `Ctrl+D` selects the next occurrence (replacing the selection); `Alt+L`
   selects the line; `Ctrl+Alt+↑/↓` are disabled with a notice. This preserves every other editing behavior.
2. **Phase 7:** add a Rune patch exposing `Cx::caret_rects()` or `TextEdit` secondary ranges, then
   `lmdv-edit::MultiCursor`:
   - `Vec<Cursor { anchor, focus }>`, primary first;
   - `on_input_key` applies insert/delete to all ranges as one `undo_group`, computing offsets from last to
     first;
   - draw secondary carets as 1 pt `el()`s positioned by the patched rects;
   - IME composes only at the primary; other cursors insert the committed text.
3. If the patch is rejected, ship a **documented gap**: multi-cursor commands are absent. Do not ship
   “invisible” multi-cursor edits.

### 6.7 Spellcheck

- Rune's Linux checker is a stub. Define `lmdv-edit::Spell` with a worker and `zspell` (Hunspell-compatible)
  or `spellbook`; load dictionaries from `/usr/share/hunspell` plus optional bundled en/it/de/fr/es files.
- Check the visible text on a 300 ms debounce; return misspelled byte ranges; paint with `Span::underline` or
  a dedicated squiggle color. Suggestions go into the editor context menu.
- Toggle is `editor.spellcheck`; if no dictionary is available, hide the toggle and record `gap` in parity.
- Do not block typing on the checker.

### 6.8 Drafts, save, line endings, conflicts

- Draft write: 1 s after the last edit, on tab switch, on close, and on app quit; LF-normalized; stores
  `{ key, base_hash, base_text? , text, eol, saved_at }`. Cap 20 entries / 2 MB; evict oldest.
- On open: if a draft exists and `draft.text != disk_text`, show a notice/dialog: **Restore**, **Discard**,
  **Compare**. A draft whose `text == disk_text` is deleted silently (specified behavior #26).
- Save is explicit (`Ctrl+S`); no autosave to the file. `Save As` updates the document key, registry,
  recents, positions and draft key atomically (store patch after the IO save succeeds).
- Line endings: detect CRLF if any CRLF is present, else LF; edit LF; write CRLF if detected. Strip a UTF-8
  BOM on read and **never re-add it** (as specified). Preserve the presence/absence of a final newline.
- Conflict: save carries `expected_meta`. The IO worker re-stats immediately before rename; if
  `(dev, ino, len, mtime_ns)` differs, return `SaveConflict` and do not write. UI offers **Overwrite**,
  **Save As**, **Cancel**; Overwrite retries with `expected: None`. A deleted file is only recreated when the
  user chose **Save again** (`create: true`).
- Own-write echo: after a successful save, record `(dev, ino, mtime, size)` and `ignore_writes_until`; watcher
  events matching it update the base quietly.
- F5/Ctrl+R: explicit reload; in edit mode only when clean; if dirty, show the conflict notice.
- Reading `--line`/heading after load: queue `Goto` on the tab, apply after first layout/parse.

### 6.9 Editor performance guardrails

- No full-document `text_input` above the threshold. No syntax highlight above 128 KiB. No parse per
  keystroke; debounce 120 ms. No `doc.snapshot()` per frame; only on parse/save.
- `doc.apply` computes the line index incrementally from `first_line`; no full rebuild on every key.
- Block mode reconciles blocks by text, not by parser output, so a slow parse cannot move the caret.
- Measure `commit_ms` with `RUNE_COMMIT_BUDGET_MS=8`; a typing commit above 16 ms p95 fails the gate.

---

## 7. Multi-window and tab semantics

### 7.1 One document, many views

- A file has exactly one `Document` per process (canonical path / dev+ino).
- A window has at most one tab for a document; opening it again activates that tab.
- Two windows may show the same document; both see the same buffer and dirty state.
- Only one window/tab holds the editor lease; the other is a live viewer.
- A tab transfer preserves the same `Rc<Document>` and, if present, the `TextEdit` and lease.

### 7.2 Tab lifecycle

| Action | Behavior |
|---|---|
| `Ctrl+T` | New start-page tab, in front. If the active tab is an untouched start page, reuse it. |
| Open file | If doc already in this window, activate; else insert after active (foreground unless Ctrl/Middle background); replace an active start page. |
| Close tab | If it is the last view of a dirty doc → Save/Discard/Cancel. If other views exist → close this view silently (buffer survives). A clean start page closes immediately. |
| Last tab closed | Show a start page; do not close the window. |
| `Ctrl+Shift+T` | Reopen last closed tab at its index; reuse `Rc<Document>` if alive, else reload path and anchor. Pasted text comes back with its text. Start pages are not remembered. |
| `Ctrl+Tab` / `Ctrl+PageDown` | Cycle tabs. |
| Reorder | Rune `TabStrip::on_reorder`; update `tabs`. |
| Tear out | Rune `on_tear_out`/`on_tear_move`; build a new window with the tab state; source hides it until accepted. |
| Cross-window drop | Target registry hit-test by screen point; target shows `set_incoming` slot; on release, `TabOffer`; source waits `TabAccepted`. |
| Window close | Patch close request; ask once for all dirty documents in that window; then force close. |
| `Ctrl+Shift+W` | Close window (same prompt). |
| `Ctrl+Shift+N` | New window with a start page, cascaded 32 pt from the current window. |
| `--new-window` | Always a new window, even if the file is open elsewhere. |

Cross-window tab drag is implemented for X11 where global screen points are available. On Wayland, absolute
window positions are hidden and pointer capture across toplevels is unreliable; fall back to the
**Move tab to window…** menu and **Move to new window**. Record the fallback as `substitute`.

### 7.3 Window routing and focus

- `WindowRegistry` tracks each window's tabs, active doc, idle-start-page status, pasted count, closing flag,
  ready flag and last focus time.
- `on_focus(true)` moves the window to the front of the focus list; `on_close` marks closing and removes it.
- A launch for a doc open in any window activates the first window in focus order that holds it, then focuses
  that window.
- A launch without a file brings the most recently focused window forward; if none, new window.
- A window with only a start page is “idle” unless it was created by tearing out a tab (the specified rule).
  Track `WindowKind::Normal | TornOffStart`.
- New windows cascade; use `WindowOptions::position` when available.

### 7.4 Title, theme and prefs

- `Tx::set_title` on active tab changes; title = file name + dirty dot + “ — lmdv”; start page = “lmdv”.
- `AppShared.prefs` is canonical. A change updates all windows via `UiMsg::PrefsChanged`; each window
  invalidates its `Kit` cache and redraws. No per-window preference divergence.
- `ThemePref::System` resolves per window from `Cx::system_dark`; a system change arrives as a window theme
  event; only that window rebuilds.
- Native frame color is not app-controlled without patch `0004`; the in-app toolbar and content always match.
- `Ctrl+=`/`Ctrl+-`/`Ctrl+0` and `Ctrl+wheel` change the shared reader size; all windows follow.
- Panel widths are global (`Prefs.panels`) or per window? The specification stores global. Use global for consistency; the
  `PaneState` in each window is initialized from prefs and writes back on drag end.

### 7.5 Session restore and crash recovery

The specification did not restore tabs across a full restart. The user explicitly asks for sessions, so this is an
intentional extension:

- On clean quit/close, `session.json` has every window's placement, tab sources, active tab, anchors, mode,
  nav history and pane state. Restore on next launch if `prefs.restore_session` and no file argument was given.
- Do not restore `EditorSession` text from `session.json`; restore the file and apply a draft through the
  normal draft prompt. Pasted text lives in drafts and is auto-restored once per launch.
- If the previous exit was a crash (`clean: false` in the session file), still restore tabs but mark the
  session “recovered”; drafts are offered as usual.
- A file argument on the command line always wins over session restore for the first window; additional
  windows restore only if `--restore-session` is passed (optional flag, not in the fixed grammar).

### 7.6 P4 implementation capsule (verified against pinned Rune)

**Verdict:** runtime document windows are supported: `rune::open_window(WindowSpec<S>) -> WindowHandle`
(`vendor/rune/crates/rune/src/window/spec.rs:469-483`). Use it from a handler/poll on the UI thread.

**Rules and traps**

- Use `open_window`, not `App::window`: the latter marks the window secondary, hides it on close and lets the
  app end with the primary (`app.rs:340-355`). `open_window` windows keep the app alive and are destroyed on close.
- `WindowSpec::from_app(key, app).state(prebuilt_state)`; build `WindowState` *before* opening, with no live
  `RefCell` borrow held. Each window has its own `S`, `Runtime`, `Tx`, retained tree and poll/key routing; `S`
  only needs `'static`, so `Rc<AppShared>`/`Rc<Document>` are fine.
- Window keys are unique among all live hosts. Use `window-{WindowId.0}`.
- `open_window` is inert headlessly (`spec.rs:476-482`); two-window gates need a test factory seam plus
  `--window KEY` / `--desktop` (`docs/TESTING.md`).
- `AppHandle::update` closures are `Send`, so they cannot capture `Tab`/`Rc<Document>`; cross-window tab
  transfer goes through the same-thread `Rc<UiMailbox>` and `UiMsg`.
- The last window's `on_quit` is skipped because its host is dropped first (`window.rs:299-322`); do final
  clean/session work in `on_close` as well.
- `Tx::quit` bypasses `on_close_request`; aggregate the quit guard before calling it.
- Wayland: no `WindowHandle::placement`/`set_position`, no `screen_point`, no always-on-top; focus is compositor
  policy unless an `AppHandle::activate(token)` token is forwarded. X11 supports placement and above.
- Only `Tx::set_title` changes a window title (no cross-window title API); each window sets its own in `poll`.
- Native menus are unsupported on Linux; tray/global shortcuts are process-level and must be registered once.

**P4-01 slice — new window, shared registry, routing**

- `WindowEntry` grows: key, native `WindowHandle`, `AppHandle<WindowState>`, per-window `Waker`, documents set,
  `only_start`, `WindowKind`, `closing`, focus stamp, cached placement, dirty names.
- `WakeSet: WindowId -> Waker`; `UiMsg` wakes one window, launches/watch events wake all (today both inboxes
  hold a single waker slot, `shared.rs:453-486,526-562`).
- Opener closure in `AppShared` capturing `Weak<AppShared>`; routing order: `--new-window` → window already
  showing the doc → in-flight claim → first idle normal window → most recently focused → any → new window.
  Clear claims when the doc appears in the target.
- Files: new `crates/lmdv-app/src/windows.rs` (registry, routing matrix, cascade, handles), `shared.rs`,
  `app.rs` (`run_windows` factory + opener + `--window`/`--list-windows`), `window.rs` (new-window command,
  `handle_ui` focus/close, `sync_registry`), `commands.rs` (`Ctrl+Shift+N`, `Ctrl+Shift+W`).
- Tests: routing matrix and two `WindowState`s on one `AppShared` (the existing two-window test pattern), an
  opener seam that records `NewWindow` instead of calling Rune in unit tests, and headless factory gates.

**P4-02 slices**

- **Close/quit:** prompt only when this window holds a doc's last view (`subscribers.len()==1`) plus pasted
  tabs; `closing` skips routing; one aggregate `Ctrl+Q` dialog, save each doc once, then quit.
- **Transfer:** `Outgoing`/`TransferMsg::{Hover,Adopt,Adopted,Returned}`; source detaches without releasing
  subscribers/lease; target validates closing, duplicate+dirty (reject), duplicate+clean (merge/activate), else
  inserts and takes the lease; menu “Move to new window” / “Move tab to window…” is the Wayland substitute.
- **X11 tear-out:** `TabStrip::tear_out` + `on_tear_out`/`on_tear_move`; start with release-only placement, then
  spike live follow + `TabDragState::hand_off` + reattach-close. Wayland: compositor-placed new window or menu.
  T-21 (sole-tab window drag) has no Rune API; patch or accept a substitute.
- **Session:** capture placement/tabs/anchors/active/window kind; restore via the `run_windows` factory only when
  `prefs.restore_session`, no CLI path and not headless; never store editor text (drafts do); skip under
  `--snap`/`--tree`.

**Open product decisions:** close prompt for a doc still open elsewhere (recommend last-view only); one aggregate
quit dialog; T-21 patch vs substitute; live tear-out follow vs release-only; Wayland tear-out/always-on-top;
session restore defaults; single process (recommended) vs process-per-window; closed windows are not reusable.

---

## 8. Persistence

### 8.1 Locations

Use `rune::dirs::AppDirs::new("md.lmdv")` (XDG on Linux), and `RUNE_DIRS_ROOT` for tests.

| Data | Path | Format |
|---|---|---|
| Preferences | `$XDG_CONFIG_HOME/md.lmdv/prefs.json` | `Prefs`, pretty JSON |
| Recent files | `$XDG_DATA_HOME/md.lmdv/recents.json` | array, max 12 |
| Reading positions | `$XDG_DATA_HOME/md.lmdv/positions.json` | map, max 100 |
| UI state | `$XDG_DATA_HOME/md.lmdv/ui.json` | pane widths/hidden, collapsed folders, palette recents |
| Session | `$XDG_DATA_HOME/md.lmdv/session.json` | windows/tabs/anchors/placements |
| Drafts | `$XDG_DATA_HOME/md.lmdv/drafts/<blake3(key)>.json` | one file per draft, max 20/2 MB |
| Math SVG | `$XDG_CACHE_HOME/md.lmdv/math/<hash>.svg` | optional; memory cache is primary |
| Logs | `$XDG_STATE_HOME/md.lmdv/logs/` | crash log only, no routine writes |
| Window places | inside `session.json` | `WindowPlacement` from Rune |
| Single-instance socket | `$XDG_RUNTIME_DIR/lmdv/<instance-id>.sock` | mode 0600, dir 0700; portable id differs |

Portable mode: `lmdv-portable.txt` next to the executable switches the data root to `<exe dir>/data/` and a
distinct instance id (hash of the executable directory), so a portable copy can run beside an installed one.
`LMDV_HOME` overrides the root for tests/advanced users.

### 8.2 Atomic writes and the store thread

```rust
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.{}.{}.tmp",
        path.file_name().unwrap().to_string_lossy(), std::process::id(), NEXT.fetch_add(1, Relaxed)));
    let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    // fsync the directory on Linux
    File::open(dir)?.sync_all()?;
    Ok(())
}
```

The store thread owns every state file. `StoreHandle::patch` sends a patch; the writer coalesces per file and
writes at most once per 300 ms. `flush(timeout)` is called from `on_close`/`on_quit` only (not a frame), and
returns whether it completed. If it times out, drafts are already written on a 1 s debounce, so data loss is
bounded.

Never call `rune::prefs::Prefs::set` in a handler. Never read a state file in a view.

### 8.3 Schema and migration

Every file:

```json
{ "schema": 1, "app": "lmdv", "...": "..." }
```

- `#[serde(default)]` on every struct field; unknown fields are ignored (forward compatible).
- On load failure: rename to `<file>.bad-<timestamp>`, start from defaults, show a notice only if the file
  existed and was non-empty.
- Migrations are pure functions `fn migrate_v1_to_v2(Value) -> Value`; keep tests for each. Unknown future
  `schema` → read-only defaults, do not overwrite until the user changes something.
- Paths are stored canonical where possible; positions/recents use the same path spelling as `DocumentKey`.
- Draft keys are lowercased paths? Linux is case-sensitive; use the canonical path as-is, but enforce one
  draft per `(dev, ino)` when known. (The reference lowercased because Windows.)

### 8.4 Write timing

Match the specification where it matters:

- prefs: on change (coalesced).
- positions: 300 ms after scroll stops, tab switch, window close.
- drafts: 1 s after keystroke, tab switch, close, quit.
- recents: on successful open/save-as/rename/remove.
- session: 500 ms after tab/window changes; on close.
- pane widths: on drag end only.
- window geometry: on close/quit, from `WindowHandle::placement()`.

### 8.5 Crash recovery

- A draft is written before the crash window (1 s debounce); on startup, scan drafts.
- For file drafts, attach them to the document when it opens; do not auto-apply.
- For pasted drafts (no file), restore tabs automatically once per launch, in the first window, unless another
  window already has the draft; mark them dirty so close asks.
- A `clean` flag in `session.json` is set false while running and true on clean quit; crash restore uses it.
- Never write a user file as part of recovery.

---

## 9. Security and robustness

### 9.1 Threat model

Markdown is untrusted input from the internet, AI assistants, or other users. The app must not execute it,
fetch it, or let it escape the document root for embedded assets. The app is offline; links only open
externally on an explicit user action.

### 9.2 HTML policy

- Tagfilter first (GitHub's list) so `<script>`, `<style>`, `<title>`, `<iframe>`, `<xmp>`, `<noembed>`,
  `<noframes>`, `<plaintext>`, `<noscript>` show as text or are dropped.
- Allowlist tags/attributes from §5.4; drop all `on*`, `style`, `srcdoc`, `formaction`, `popover`,
  `command*`, `interestfor`, `ping`, `data:` URLs except small `data:image/(png|jpeg|webp|gif);base64`.
- URLs: `http/https/mailto` open externally; `file:`, `javascript:`, `vbscript:`, `data:text/html` are shown
  as text/ignored; relative links resolve against the document folder.
- Do not render `<style>` or CSS; no external stylesheet.
- HTML exported to a browser is sanitized again by construction; the export writer uses the same allowlist.

### 9.3 Path policy

```rust
pub struct PathPolicy {
    pub doc_root: PathBuf,          // canonical folder of the document
    pub allow_outside: bool,        // user opt-in, default false
    pub max_bytes: u64,             // 32 MiB
    pub max_pixels: u64,            // 64 MP
    pub max_svg_dim: u32,           // 4096
}
```

- Canonicalize with `fs::canonicalize` on the worker; reject paths that leave `doc_root` unless
  `allow_outside`. Reject `/proc`, `/sys`, `/dev`, sockets, FIFOs, and non-regular files.
- Reject network/UNC-style paths and `//host/share` before canonicalization.
- For link *navigation* (user clicked a link), allow any local Markdown file; the path is shown in the tab and
  the file browser re-roots. Do not silently embed/read non-Markdown files; ask before opening externally.
- For embedded assets, restrict to the document root by default. This prevents `![x](/etc/passwd)` from
  reading arbitrary files.
- Symlinks are followed only if the resolved path stays under the root.
- Images: check size/dimensions before decode; SVG reject external references, scripts, and huge dimensions;
  rasterize at a bounded size. A decode failure is a broken-image placeholder, not an error card.
- Exports include only assets that passed the policy; list omitted assets in the export log/notice.

### 9.4 Huge content

- Open reads cap at 32 MiB; over 8 MiB show a “large document” warning and default to viewer mode.
- Table render cap: 64 columns, 10k rows in the document; beyond, show the first 500 + “Open preview”
  (preview virtualizes all rows).
- Parse/render caps: 5 MB source, 200k blocks; beyond, plain-text viewer with a notice.
- Image decode cap as above; animated images show first frame in documents, animate only in lightbox.
- Editor threshold from §6.1; beyond block mode, offer a “read-only large file” mode if block mode still
  cannot meet the typing budget.
- Never freeze the UI: every cap has a notice and an escape hatch.

### 9.5 Watcher races

- Watch folders, not files (atomic saves replace inodes).
- Debounce 300 ms; classify by `stat` after the batch, not by event kind.
- Always re-read and compare text before calling a change real; mtime-only touches update the base quietly.
- Own writes: suppress by `(path, dev, ino, mtime, size)` for 1.5 s; a save completion also updates base.
- Rename chains: follow `[old, new]` pairs; if the final path is a file, rekey the document and its stores;
  if gone, mark `Gone` and show “Save again”.
- A document with dirty edits is never auto-reloaded.
- Tree watch: recursive, 400 ms, only create/remove/rename events under listing rules; rearm on focus.
- Generation guard: a reload result applies only if the doc is still `Loading`/clean and its identity matches.

### 9.6 Single-instance socket

- `$XDG_RUNTIME_DIR/lmdv/<instance-id>.sock` (the installed id, or a portable id derived from the exe
  folder), directory 0700, socket 0600; fallback to a private `/tmp/lmdv-$uid` directory with
  ownership/mode checks.
- A lock file (`flock`) serializes claim/takeover.
- Message: `u32` length + JSON, max 1 MiB; version, cwd, paths, line, new_window, activation token.
- `SO_PEERCRED` must match the effective uid; reject otherwise.
- A stale socket is removed only if it is a socket owned by the same uid and no live process answers.
- The listener only forwards a `Launch` to the UI; it never opens files or runs commands itself.
- On send, wait for an ack (2 s); if the running app is quitting, let the new process claim after release.
- Focus the target window; pass `XDG_ACTIVATION_TOKEN` when present.

### 9.7 File writes

- Atomic replace; preserve permissions/mode; no symlink following for writes (write the canonical target).
- Never write on open, scroll, task render, or watcher event.
- Task checkbox ticks are the only viewer write; they apply a precise source edit, save immediately, and offer
  **Undo** that writes the previous text back. If the doc is dirty, apply to the buffer and leave it dirty
  instead of writing.
- Saving never creates a deleted file unless the user chose **Save again**.

---

## 10. Testing and acceptance gates

### 10.1 Layers

| Layer | What | Where |
|---|---|---|
| Unit | IDs, buffer/line index, EOL/BOM, prefs clamps, path policy, front matter, slugging, math scanner, table model, find, line ops. | each crate `src/*_tests.rs` / `tests/` |
| Golden parser | `MdDoc` serialized (or a compact projection) for every fixture; source maps and headings. | `lmdv-md/tests/golden` |
| Golden render | Rune element tree JSON and PNG at 1x/2x for scenes; pixel compare with tolerance. | `lmdv-testkit`, `tests/golden` |
| A11y | Roles, names, levels, focus order, live regions; `--tree` JSON assertions. | `lmdv-testkit/tree` |
| Interaction | Headless scripts: open, edit, save, find, palette, tabs, close prompts, routing. | `tests/scripts` |
| Export | HTML exact/normalized; DOCX unzip + XML; PDF text extraction via `pdftotext` if present; PNG dimensions/pixels. | `lmdv-export/tests` |
| Security | XSS/HTML payload corpus, path traversal, image bombs, socket fuzz, huge tables. | `tests/security` |
| Perf | Parse, typing, commit, scroll, export, launch. | `benches/`, `--frames`, `--profile perf` |
| Manual | Wayland/X11, NVIDIA, scaling, IME, multi-window, MIME. | checklist below |

### 10.2 Fixture corpus

Adopt the ten families from `BEHAVIOR_SPEC.md §9` under `tests/fixtures/`: `F-CORE`, `F-MARKDOWN`,
`F-TABLE`, `F-EDIT`, `F-TABS`, `F-TREE`, `F-SYSTEM`, `F-EXPORT`, `F-PALETTE`, `F-ERRORS`, plus
`commonmark-math.md` from the reference samples. Every fixture maps to parity IDs; a fixture without an ID is
not a gate.

### 10.3 Headless scenes

Define `AppSpec::scenes` for deterministic screenshots:

```text
viewer-light, viewer-sepia, viewer-dark, viewer-black
viewer-callout, viewer-math, viewer-table, viewer-frontmatter
viewer-toc, viewer-footnotes, viewer-html, viewer-mermaid
editor-split, editor-find, editor-formatbar, editor-large
palette, share, help, keys, lightbox, table-preview
dirty-close, save-conflict, draft-restore, missing-file, render-error
```

Scene functions set `WindowState` prefs and insert fixture documents synchronously through the testkit; they
do not run the async open path. Use `--size 1280x900 --scale 2 --theme light` for goldens.

### 10.4 Structural and accessibility assertions

- Every heading has `role=heading`, correct `level`, non-empty name.
- The tab strip is a `tablist`; each tab is a `tab` with `selected`; close button has a name.
- The outline is a `tree`/`treeitem` with levels and expanded state.
- The file browser is a `tree`, directories expanded/collapsed, keyboard navigation.
- Find bar: input name, count live region, next/prev names.
- Notices are `status`/`alert` live regions.
- Modal dialogs are `dialog` with focus trapped; Esc closes; focus returns.
- The article is `article`/`document`; links have `href`; images have alt; math has a TeX label.

### 10.5 Performance gates (release/perf profile)

| Metric | Budget | How |
|---|---|---|
| Launch to first frame (warm) | < 350 ms | `--frames` timestamp / logs |
| Open a 1 MB file to readable | < 250 ms | script + marks |
| Parse 500 KB | < 80 ms | criterion |
| Typing commit, 64 KB single field | < 16 ms p95 | `long_note`-style bench |
| Typing commit, 1 MB block mode | < 16 ms p95 | block bench |
| Scroll frame, 2k blocks | < 8 ms CPU | `--frames` |
| Idle | zero frames, zero timers | `poll` returns false |
| HTML export 1 MB | < 1 s | bench |
| PDF 50 pages | < 5 s | bench |
| PNG diagram 2x | < 1.5 s | bench |

A gate fails on p95, not median. Keep outliers in the report.

### 10.6 Manual smoke matrix

| Axis | Values |
|---|---|
| Session | Wayland (Hyprland/sway), X11 |
| GPU | NVIDIA proprietary, Mesa |
| Scale | 100%, 150%, 200% |
| Theme | light, sepia, dark, black, system |
| Width | 480, 760, 1024, 1600, 4K |
| Input | US, Italian, German, French, Spanish, Chinese IME |
| Files | CRLF, BOM, no final newline, missing, read-only, deleted, renamed, 1 MB |
| Windows | 1, 3, same doc in two, drag tab out, close last |
| CLI | `lmdv file.md`, `--line`, `--new-window`, second launch, missing file |
| Export | HTML in browser, DOCX in LibreOffice/Word, PDF in viewer, print |
| A11y | Orca: headings, outline, tabs, find, notices, dialog focus |

---

## 11. Orchestration model

### 11.1 Roles and ownership

| Role | May edit | Purpose |
|---|---|---|
| Orchestrator | Shared files, manifests, docs, integration | Freeze interfaces, decompose, run gates, re-prompt, update parity/status. |
| Research | Nothing | Read-only facts with `file:line`; no recommendations without evidence. |
| Worker | Only its owned files/globs | One bounded task, tests, acceptance commands, report. |
| Reviewer | Nothing | Red-team diff against parity IDs, edge cases, security. |

**One file, one owner.** Shared files (`Cargo.toml`, root manifests, `AGENTS.md`, `PLAN.md`, all `docs/`,
`scripts/`, app wiring `lmdv-app/src/lib.rs`, `lmdv/src/main.rs`) are orchestrator-owned unless a task
explicitly transfers ownership. A worker that needs a shared change reports the exact patch; the orchestrator
applies it.

### 11.2 Reusable substrate contract

Copy this into every worker prompt (fill `<...>`):

```text
Objective: <one sentence, testable>.

Read first: AGENTS.md, docs/STATUS.md, PLAN.md, docs/ARCHITECTURE.md,
docs/FEATURE_PARITY.md rows <IDs>, docs/DECISIONS.md, <relevant reference files>.

Context / interface:
- <types and signatures the worker must implement against; paste them, do not paraphrase>
- <spec behavior with file:line>
- <Rune API with crates/rune path:line>

Owned files: <exact paths/globs. Nothing else may be edited.>
Do not touch: shared files, other crates, vendor/rune except the assigned patch,
vendor/rune except the assigned patch, user files outside the repo.

Constraints:
- Rune pinned rev cca5df6…; no webview; Linux first; no CARGO_TARGET_DIR.
- No file I/O in handlers/views; no unwrap in UI paths; no TODO without a parity ID.
- Follow docs/ARCHITECTURE.md ownership and handler rules.
- Keep the public API exactly as frozen; if it must change, stop and report.

Deliverables:
- Code + unit/integration tests.
- A short implementation note: decisions, deviations, and any parity row to mark substitute/gap.
- Evidence: commands run and their results.

Acceptance (run all):
- cargo fmt --all -- --check
- cargo check -p <crate>
- cargo test -p <crate>
- cargo clippy -p <crate> --all-targets
- <headless scene / tree / script / golden / benchmark>

Report:
- Files changed (one line each why)
- Commands run + exact result summary
- Screenshots/tree paths
- Known limitations / follow-ups
- Anything the orchestrator must wire or decide
```

### 11.3 Context capsule (one page, per task)

Before spawning, the orchestrator writes or pastes a capsule:

```text
Capsule: <task id>
Current phase: <P#>. Gate: <commands>.
What exists: <3–6 bullets with paths and public types>.
Interface freeze: <verbatim signatures>.
Spec behavior: <feature IDs + reference file:line>.
Known failures: <exact failing test/command>.
Do not: <scope exclusions>.
Next after this: <one line>.
```

Do not make a worker rediscover the architecture by reading 200 KB of planning docs. Give it the capsule and
the exact files.

### 11.4 Parallelization rules

- One worker per crate/module boundary. Never two workers in the same file.
- Parallelize only after an interface freeze:
  - P1: `lmdv-core` / `lmdv-io` / `lmdv-app` shell can run in parallel if `Document`/`WindowState`/`JobMsg`
    are frozen first.
  - P2: `lmdv-md` parser and `lmdv-render` can run in parallel after `MdDoc`/`Block`/`Inline` are frozen;
    `lmdv-math` is independent.
  - P3: editor and save/draft/watcher can run in parallel after `EditorSession`/`StorePatch` are frozen.
  - P5: HTML, DOCX, PDF workers can run in parallel after `ExportDoc` is frozen.
- Never parallelize a change to `Cargo.toml`, `lmdv-app/src/lib.rs` wiring, or shared domain types.
- A worker that discovers an API gap stops and reports; it does not unilaterally widen a frozen interface.
- Do not spawn a worker to redo a task another worker is still running.

### 11.5 Validation loop (orchestrator)

1. **Read the diff**, not the summary. Look for unrelated edits, debug prints, swallowed errors, `unwrap`,
   `todo!`, large deletions, and accidental changes outside owned files.
2. **Run the acceptance commands yourself** from the repo root. A worker's “passing” claim is not evidence.
3. **Check behavior against the parity IDs.** Read the spec `file:line` when subtle.
4. **Classify:** accept → integrate; minor defect → re-prompt the same worker; design flaw → revise the task or
   take it over; repeated failure → split or implement yourself.
5. **Wire it:** add modules, re-exports, feature flags, scenes, fixtures, i18n keys, docs/status, parity row.
6. **Record evidence:** commit/status entry with commands and results.

### 11.6 Re-prompt protocol (exact)

Never “please fix it”. Send one bullet per defect:

```text
The change fails acceptance:
- `cargo test -p lmdv-md frontmatter::tests::nested` → panics at src/frontmatter.rs:88:
  index out of bounds. Expected: raw fallback with valid=false (F-FRONT-03).
  Actual: panic on `entries[0]`.
- `--scene viewer-frontmatter --tree` shows no `details` role; expected a collapsed group named
  “Document details”.
- Scope: fix only `crates/lmdv-md/src/frontmatter.rs` and its tests. Do not restructure the parser.
- Re-run: `cargo test -p lmdv-md` and `cargo run -p lmdv -- --scene viewer-frontmatter --tree /tmp/fm.json`;
  paste both results.
```

If the same task fails twice, narrow the scope, pair the worker with a reviewer, or take it over.

### 11.7 Integration/wiring checklist

- Add the module to the crate's `lib.rs` and re-export the frozen public types.
- Add the dependency/feature to the workspace manifest (orchestrator only); run `cargo check --workspace`.
- Register commands in `COMMANDS`; add shortcut/chord; add menu/palette visibility; add i18n keys.
- Add the scene/fixture/script; add golden/tree assertions.
- Add the parity row status (`done`/`substitute`/`gap`) with reason and evidence path.
- Add a `docs/STATUS.md` line: what works, what fails, next 3 tasks.
- Run `scripts/run-gates.sh` (fmt, check, test, clippy, headless scene, tree) and keep the output path.

### 11.8 Phase plan and stop/go gates

Dependencies are strict; a phase may overlap research/spikes but implementation starts only when its inputs
are frozen and the previous gate is green.

| Phase | Goal | Workers (parallel) | Depends | Stop/go gate |
|---|---|---|---|---|
| **P0** Bootstrap + Rune proof | Workspace, vendor, patches, a visible fixture, headless snapshot harness. | orchestrator + rune patch worker + 1 app proof worker | — | `cargo check --workspace`; inline math baseline test; close-veto test; `--snap` of a fixture; vendor checksum script. |
| **P1** Core + IO + viewer shell | Domain types, registry, store, single instance, window/tabs/start page, open file, plain Markdown view. | core, io, app shell (3) | P0 | Open 3 files in tabs; launch second instance routes to the right window; position survives restart; dirty close veto; `--tree` scene. |
| **P2** Markdown parity | `MdDoc`, extensions, renderer, math, mermaid, images, outline/scroll spy, source maps. | parser, renderer, math (3) | P1 | Every F-MARKDOWN/F-TABLE/F-MATH fixture renders; golden/tree; math baseline; sticky table; TOC; click/double-click to source; parse/typing budgets. |
| **P3** Editor + file safety | Single-field editor, syntax, find/replace, line ops, save/EOL/BOM/conflict, drafts, watcher, task ticks. | editor, file-safety (2) | P2 | E-01–E-20, P-04–P-06; save/reload/conflict/draft flows; typing p95; no file I/O in handlers. |
| **P4** Multi-window + session | Shared doc registry behavior, tab transfer/tear-out, routing, reopen, prefs/title sync, session restore. | multiwindow (1) + reviewer | P1/P3 | Two windows same doc; move tab; close prompts; routing matrix; restore; X11 drag; Wayland fallback. |
| **P5** Export | HTML, DOCX, PDF/print, PNG/SVG, table copy. | html, docx, pdf, png (4) | P2/P3 | X-01–X-16; self-contained HTML; DOCX opens; PDF pages/header; 2x PNG; no network. |
| **P6** Parity extras | File browser, folder search, table preview, lightbox, palette, context menus, back/forward, help, media substitute, i18n, a11y. | 4–6 by module | P2/P3 | F-*, O-*, C-*, V-*, A-*, I-*, H-* rows; keyboard matrix; Orca pass. |
| **P7** Hardening + packaging | Fuzz/security, large files, perf, Wayland/X11, MIME/desktop, full parity audit. | 2 + reviewer | all | Security corpus; perf budgets; manual matrix; parity has no un-triaged rows; install script works. |

Do not advance on “the worker says it works”. The orchestrator runs the gate and records the command output.

### 11.9 First tasks, in dependency order

| # | Task | Owner | Produces |
|---|---|---|---|
| 1 | `P0-01` vendor Rune, root manifests, `[patch]`, `vendor/UPSTREAM.md`, patch `Span::image`/close veto, proof app | rune worker + orchestrator | buildable workspace + `--snap` proof |
| 2 | `P0-02` `lmdv-core` IDs, `SourceBuffer`, `Document`, `DocumentRegistry`, tests | core worker | frozen domain API |
| 3 | `P1-01` `lmdv-io`: XDG paths, store writer, atomic write, load/save, socket, launch parser | io worker | open/save/launch primitives |
| 4 | `P1-02` `lmdv-app` skeleton: `AppShared`, `WindowState`, mailboxes, commands, shell, tabs, start page | app worker | visible window, tabs, open file |
| 5 | `P1-03` viewer integration + outline/anchors/scenes + run the P1 gate | app/render worker | first end-to-end viewer |

The next five after the gate: parser model, renderer base, math service, editor, file-safety.

### 11.10 Context handoff (what a fresh agent reads)

1. `AGENTS.md`
2. `docs/STATUS.md` — current phase, exact task, failing commands, next 3
3. `PLAN.md` — phase and gate
4. `docs/FEATURE_PARITY.md` — IDs and statuses
5. `docs/ARCHITECTURE.md` — this document, condensed
6. `docs/DECISIONS.md` — D1–D12, patch log
7. `docs/TESTING.md`, `docs/RISKS_AND_SPIKES.md`
8. `vendor/rune/` only for the specific Rune `file:line` in the task capsule

`docs/STATUS.md` is updated at the end of every session; chat history is not a handoff.

---

## 12. Top ten architecture risks

| # | Risk | Likelihood / impact | Mitigation | Exit criterion |
|---|---|---|---|---|
| 1 | **Inline math needs a Rune text patch**; `Span::image` does not exist at the pinned rev. | High / High | P0 spike; implement the inline-box patch first; fall back to `InlineIcon` only for a demo, never ship it as parity. | Formula baseline within 0.5 pt; aspect correct; wraps; RTL; selection skips it. |
| 2 | **Editor performance at document size**: whole-field typing is 12–46 ms at 50–200 KB. | High / High | Two-mode editor; threshold measured; block mode with global undo journal; no parse per keystroke; benchmark gate. | 16 ms p95 at 64 KB single field; 16 ms p95 at 1 MB block mode. |
| 3 | **Rune is pre-release and vendored**; a rev bump re-applies patches 0001/0002 and can break APIs. | High / Medium | Pin rev; patches as `.patch` files; vendor checksum; golden/tree tests on every bump; never track `dev`. | Bump runbook passes on a clean checkout. |
| 4 | **Shared document buffers across windows** create lease/undo/close races. | Medium / High | One canonical `Rc<Document>`; lease state machine; explicit takeover; close prompts; tests for two windows, tab transfer, rename/delete. | All T-* multi-window scenarios pass; no lost edit in a scripted race. |
| 5 | **Markdown parity fork drifts** from upstream and accumulates bugs. | Medium / High | Fork only the module; provenance/diff log; fixture corpus from the specification; one owner; no ad-hoc parser changes outside `lmdv-md`. | Every F-MARKDOWN fixture golden; fuzz no panic. |
| 6 | **Native close has no veto**; unsaved-close confirmation is impossible unpatched. | Medium / High | Patch `on_close_request`; keep programmatic close force-close; test native Alt+F4 and window-manager close. | Dirty window stays open; Save/Discard/Cancel all work. |
| 7 | **Linux launch/focus**: Rune's single instance lacks cwd/paths; Wayland activation is restricted. | Medium / Medium | Own socket with cwd + activation token; route on UI thread; test second process; X11 and Wayland focus. | `lmdv file.md` in another shell opens/focuses the right tab in < 100 ms. |
| 8 | **Export beyond Rune**: PDF/DOCX/math/pagination. | High / High (for X rows) | Shared `ExportDoc`; Typst for PDF; OOXML writer; RaTeX images + best-effort OMML; PNG via offscreen Rune; EPUB not required. | X-01–X-16 pass; DOCX opens in LibreOffice/Word; PDF pages/header correct. |
| 9 | **Cross-window tab drag on Wayland** has no global coordinates/capture. | High / Medium | Implement tear-out to a new window (screen point works on X11); use `TabOffer`/`TabAccepted`; provide “Move to window…” and “Move to new window” on Wayland. | X11 drag passes; Wayland falls back with no data loss. |
| 10 | **Security/robustness**: HTML, paths, image bombs, watcher races, huge docs. | Medium / High | Allowlist + path policy + size caps + generation guards + security corpus + fuzz; no network. | Corpus and fuzz pass; no file outside root read by a document. |

Watchlist (not top ten, but track): Typst build time/binary size; DOCX math fidelity; Linux video/audio
(no Rune backend — use external-player substitute); always-on-top on Wayland (no-op); custom frame/resize;
spellcheck dictionaries; multi-cursor.

---

## Appendix A — module trees

### `lmdv-core`

```text
src/
  lib.rs
  ids.rs
  buffer.rs          SourceBuffer, LineIndex, EditDelta
  document.rs        Document, SavedBase, DocStatus, EditLease
  registry.rs        DocumentRegistry
  tab.rs             TabSource, Tab, TabView, Mode, NavHistory, ClosedTab
  prefs.rs           Prefs, ReaderPrefs, PanelPrefs, ThemePref
  find.rs            FindState, FindMatch
  session.rs         WindowSession, TabSession, ScrollAnchor
  draft.rs           Draft
  export.rs          ExportRequest, ExportKind, ExportOptions
  error.rs           UserError/Notice
```

`WindowState` may live in `lmdv-app` rather than `lmdv-core` because it references `AppShared`, `PaneState`,
`KitCache`; keep `lmdv-core` free of Rune.

### `lmdv-app`

```text
src/
  lib.rs
  shared.rs          AppShared, StartupState
  window.rs          WindowState, WindowState::new, view entry
  view/
    shell.rs         toolbar, panes, tab strip, status
    article.rs       viewer/preview composition, scroll viewport, section line
    editor.rs        editor layout, find bar, format bar
    start.rs         start page, recents, paste
    outline.rs       outline tree, filter, scroll spy
    tree.rs          file browser
    overlays.rs      palette, dialogs, lightbox, table preview, menus
    notices.rs
  commands/
    registry.rs      COMMANDS
    file.rs tab.rs view.rs edit.rs export.rs window.rs help.rs
  routing.rs         launch routing, window registry
  jobs.rs            JobHub, handle_job, timers
  closing.rs         close-request state machine
  editor.rs          editor session binding, key routing, sync
  theme.rs           Kit cache, DocTheme mapping
  a11y.rs
  scenes.rs          headless scenes
```

### `lmdv-md`

```text
src/
  lib.rs
  model.rs           MdDoc, Block, Inline, Table, Heading, …
  parser.rs          pulldown-cmark walk + builders
  frontmatter.rs
  math.rs            scanner + placeholder rewrite
  anchors.rs         slugs
  callout.rs
  footnotes.rs
  html.rs            tagfilter + html5ever + allowlist -> HtmlNode
  autolink.rs
  source_map.rs
  find.rs
  tests.rs
```

### `lmdv-render`

```text
src/
  lib.rs
  theme.rs           DocTheme, callout/table paints
  render.rs          block dispatch, memo keys
  inline.rs          spans, links, math placeholder, objects
  table.rs           sticky header, column widths, scroll
  code.rs            rune-kit syntax + copy button
  image.rs           image resolver, picture/srcset, missing
  math.rs            MathResolver trait + layout
  mermaid.rs         rune_mermaid card/drawing
  html.rs            HtmlNode -> El
  footnotes.rs toc.rs callout.rs
```

### `lmdv-edit`

```text
src/
  lib.rs
  session.rs         EditorSession, commit/delta/take-over
  keymap.rs          editor on_input_key -> commands
  format.rs          wrap/heading/list/line/comment
  find.rs            find/replace
  syntax.rs          rune_highlights -> Span
  block.rs           large-document block editor + journal
  spell.rs           optional Linux checker
  tests.rs
```

### `lmdv-io`

```text
src/
  lib.rs
  dirs.rs            portable/XDG resolution
  store.rs           StoreHandle, writer, migrations
  atomic.rs
  read.rs            load, normalize, probe, EOL/BOM
  write.rs           save, conflict, permissions
  watch.rs           doc + tree watchers
  instance.rs        Unix socket protocol
  path_policy.rs
  open_external.rs   xdg-open/gio
  launch.rs          CLI parse
```

---

## Appendix B — parity ID seed

Use the IDs from `BEHAVIOR_SPEC.md` directly; do not renumber. Area prefixes: `R`, `T`, `E`, `M`, `V`,
`X`, `F`, `O`, `C`, `L`, `S`, `W`, `D`, `H`, `A`, `I`, `P`.

New implementation-specific rows (mark `substitute`/`gap`):

- `T-SHARED` shared canonical buffer + edit lease (substitute for independent window buffers).
- `W-CSD` native Linux frame / no client-side decorated resize (substitute).
- `D-CROSS` cross-window tab drag X11; Wayland menu fallback (substitute).
- `L-MEDIA` Linux video/audio opens externally (substitute).
- `E-MULTI` multi-cursor absent until patched (gap or done in P7).
- `E-SPELL` Linux spellcheck engine optional (substitute/gap).
- `X-SVG` diagram SVG export optional; PNG is the gate (gap).
- `S-RECENT` no Windows jump list; optional XDG recent-manager integration (gap).
- `H-RATE` no Store rating prompt (gap).

---

## Appendix C — first-week execution checklist

1. `scripts/vendor-rune.sh` copies the pinned workspace; `git rev-parse` recorded in `UPSTREAM.md`; checksum
   script passes.
2. `scripts/apply-rune-patches.sh` applies close-veto and inline-object patches; the proof scene renders
   `$E = mc^2$` inline and a dirty-close dialog.
3. `cargo check --workspace` + `cargo test -p lmdv-core` pass.
4. `lmdv file.md` opens; `lmdv file.md` from another shell routes; `--line 40` lands.
5. `docs/STATUS.md` says P1 exactly, with the next three tasks and the failing commands (if any).

If any of 1–4 fails, do not start P2. The rest of the plan assumes a stable build and a proven text patch.
