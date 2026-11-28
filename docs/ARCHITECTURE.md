# Architecture

This is the condensed, authoritative implementation design. The long-form design with every type sketch, rationale and rejected alternative is [`ARCHITECTURE_DETAIL.md`](ARCHITECTURE_DETAIL.md). The behavior contract is [`BEHAVIOR_SPEC.md`](BEHAVIOR_SPEC.md); the implementation/status matrix is [`FEATURE_PARITY.md`](FEATURE_PARITY.md); decisions are [`DECISIONS.md`](DECISIONS.md).

## 1. One-sentence design

**One event loop, one shared document registry, one command registry, one single-threaded UI state per window; all slow work is a generation-tagged job that reports through a per-window mailbox; Markdown is parsed into a Rune-free `MdDoc`, then rendered by a forked Rune renderer into memoized Rune elements.**

## 2. Decisions up front

| # | Decision |
|---|---|
| D1 | **Vendor the whole pinned Rune workspace** under `vendor/rune/` and depend on it by path. Rune's crates use path dependencies internally; patching the text system means owning the tree. |
| D2 | **Patch Rune for five things the app cannot work around:** inline image + rise in text, native-close veto, optional text-hit helper, optional window theme, deferred Linux client-side frame. Patches are numbered `.patch` files with a re-apply script. |
| D3 | **Fork Rune's Markdown module into `lmdv-md` (parser/model, no Rune) and `lmdv-render` (Rune renderer).** Do not use `rune-kit::markdown` in production; the extensions are parser/renderer changes, not decorations. |
| D4 | **One canonical `Rc<Document>` per file** shared by all tabs/windows; each tab owns only its view state. One **editor lease** per document. |
| D5 | **Commands are data** (`CommandId` + a static `CommandSpec` table); menus, shortcuts and the palette dispatch by ID. |
| D6 | **Per-window worker mailbox** (`Arc<WorkerMailbox>` + `Waker`) and **per-window UI mailbox** (`Rc<UiMailbox>`). Workers never touch app state. |
| D7 | **No file I/O in handlers or views.** Load, save, stat, watch, probe, export and image decode are jobs. A slow FUSE/NFS path must never freeze the UI. |
| D8 | **XDG files + one writer thread** with atomic replace. Do not use `rune::prefs::Prefs` for hot paths (it rewrites synchronously). |
| D9 | **PDF is Typst**; HTML/DOCX/PNG have their own writers over a shared `ExportDoc`. |
| D10 | **Native Linux frame** for v1; draw the app toolbar as content. Client-side decorations are a late optional patch because Rune has no interactive edge resize on Linux. |
| D11 | **One process, one socket we own** for `lmdv` launches, with cwd and activation token. Rune's single instance forwards links but not the sender's cwd. |
| D12 | **Two editor modes:** one `TextEdit` for documents up to ~64 KiB / 1,500 lines; one `TextEdit` per blank-line block above that. The threshold is measured, not guessed. |

## 3. Workspace layout

```text
lmdv-rs/
  Cargo.toml                 # workspace + [patch.crates-io] + lints
  rust-toolchain.toml        # 1.99.1
  PLAN.md  AGENTS.md
  docs/                      # this plan set
  vendor/rune/               # pinned Rune workspace (no local patches)
  assets/fonts/  assets/icons/
  locales/                   # en/it/de/fr/es/zh-CN JSON catalogs
  crates/
    lmdv/                    # bin: CLI, instance socket, app id, window bootstrap
    lmdv-app/                # WindowState, views, commands, routing, overlays, closing
    lmdv-core/               # pure domain: IDs, SourceBuffer, Document, registry, prefs, session
    lmdv-io/                 # XDG store, atomic writes, load/save/probe, watcher, socket, path policy
    lmdv-md/                 # Rune-free parser/model: GFM + extensions + source maps
    lmdv-render/             # Rune renderer for MdDoc + DocTheme + sticky tables + math/images
    lmdv-math/               # RaTeX bridge, metrics, SVG/PNG cache
    lmdv-edit/               # TextEdit sessions, syntax, find/replace, line ops, block mode, drafts
    lmdv-export/             # ExportDoc, HTML, DOCX OOXML, Typst PDF, offscreen PNG
    lmdv-i18n/               # rune-i18n catalogs and typed keys
    lmdv-testkit/            # fixtures, scenes, tree/golden helpers (dev only)
  tests/                     # fixtures, golden, trees, scripts
  scripts/                   # vendor/apply/check, install-md-handler, run-gates
```

Dependency DAG:

```text
lmdv -> lmdv-app -> lmdv-render -> lmdv-md
                 -> lmdv-edit   -> lmdv-core
                 -> lmdv-export -> lmdv-md, lmdv-math, rune (offscreen only)
                 -> lmdv-io     -> lmdv-core
                 -> lmdv-i18n
                 -> lmdv-core
```

Rules:

- No library depends on `lmdv-app` or `lmdv`.
- `lmdv-md` has **no Rune dependency**, so parse jobs are `Send` and export is lightweight.
- `lmdv-render` has **no `lmdv-io`**; the app supplies path policy and image resolution.
- `lmdv-export` may use Rune only for offscreen PNG; HTML/DOCX/PDF must not require a GPU.
- `vendor/rune` is not a workspace member, so `cargo fmt --all`/`clippy --workspace` do not touch it.

## 4. Verified Rune constraints

| Fact | Consequence |
|---|---|
| Handlers are queued and run between frames with `&mut S`. | No re-entrancy from input; nested `RefCell` borrows are the main hazard. |
| `AppSpec::setup` runs once per window and receives a `Waker`; `poll` runs each frame. | Attach the window's `Waker` to its mailbox; drain mailboxes in `poll`. |
| Per-window state is `!Send` and lives on the UI thread. | `Rc`, `RefCell`, `TextEdit`, `ImageCache` are legal in `WindowState`; never send them to workers. |
| No `Span::image` at the pinned rev. The only inline visual is a square `InlineIcon` tied to the following word. | Inline math needs a text patch (`Span::image` + `Span::rise` + inline-box geometry). |
| No text-click byte callback. `Cx::caret` is only the focused field's caret. | Start with a double-click handler reading `Tx::text_selection`; patch only if it fails. |
| Native close: Rune `f4ec14e` runs `on_close_request -> CloseResponse` before hiding. | Return `Defer` while a document is dirty; no local patch. |
| `TextEdit` has one selection, no gutter, no find/replace; undo is 128 full snapshots / 1 MiB. | Build find/replace/line ops; multi-cursor needs a patch; use block mode for large docs. |
| Typing in one field: ~12 ms/keystroke at 50 KB, ~46 ms at 200 KB; per-block: ~0.8/2.6 ms. | Two editor modes (D12) are mandatory. |
| `Gpu::headless`, `Renderer`, `Offscreen`, `write_png` are public. | PNG export/diagram capture can run in an export worker. |
| `rune::task::spawn`, `Waker` are public. | Use plain threads/jobs; the waker is the UI wake path. |
| Headless `--snap/--tree/--script/--trace/--desktop` + virtual clock. | Visual, structural and interaction tests are first-class gates. |
| Custom titlebar is macOS/Windows only; no Linux edge-resize API. | Native frame v1; CSD is a deferred patch. |
| Wayland has no window positions or `screen_point`; always-on-top is a no-op. | Tear-out placement/cross-window drop/always-on-top are X11-only or substitutes. |
| Rune's single instance forwards `scheme://` links, not bare paths/cwd. | Own Unix socket with cwd + `--line` + activation token. |

### Rune patch set

No local Rune patches remain. Rune `07bf8c5d3` (#2743) provides
`InlineImage::{rise,label}` with fractional point metrics, a11y label
substitution, inline-image highlight boxes in `range_boxes(..., false)`, and
static Mermaid SVG (`render_svg`, `Drawing::to_svg`, `rune_kit::mermaid::svg`).
lmdv attaches images with `El::inline_image` and exposes `MathImage`; the
upstream feature tests replace the old patch-proof suite.

Root `[patch.crates-io]` must repeat Rune's eight vendored crates:

```toml
[patch.crates-io]
wgpu-hal = { path = "vendor/rune/vendor/wgpu-hal" }
naga = { path = "vendor/rune/vendor/naga" }
winit = { path = "vendor/rune/vendor/winit" }
sctk-adwaita = { path = "vendor/rune/vendor/sctk-adwaita" }
parley_engine = { path = "vendor/rune/vendor/parley_engine" }
accesskit_windows = { path = "vendor/rune/vendor/accesskit_windows" }
accesskit_atspi_common = { path = "vendor/rune/vendor/accesskit_atspi_common" }
accesskit_macos = { path = "vendor/rune/vendor/accesskit_macos" }
```

## 5. Core domain model

```rust
pub struct DocumentId(pub u64);
pub struct WindowId(pub u64);
pub type TabId = u32;
pub type JobId = u64;

pub enum DocumentKey { File(PathBuf), Untitled(PasteId), Start }
pub enum Eol { Lf, CrLf }
pub enum DocStatus { Loading, Ready, Error(String), Gone }

pub struct SourceBuffer {
    text: String,            // LF-normalized, UTF-8
    line_starts: Vec<u32>,
    revision: u64,
    eol: Eol,
    had_bom: bool,
}

pub struct Document {
    pub id: DocumentId,
    pub key: RefCell<DocumentKey>,
    pub title: RefCell<String>,
    pub buf: RefCell<SourceBuffer>,
    pub status: RefCell<DocStatus>,
    pub saved: RefCell<SavedBase>,       // revision, hash, path, FileMeta, Eol, BOM
    pub lease: RefCell<Option<EditLease>>,
    pub subscribers: RefCell<BTreeSet<WindowId>>,
    pub ignore_writes_until: Cell<Option<Instant>>,
    // parsed model lives in AppShared.parse, keyed by DocumentId + revision
}

pub struct DocumentRegistry {
    docs: HashMap<DocumentId, Rc<Document>>,
    by_path: HashMap<PathBuf, DocumentId>,          // canonical, case-sensitive on Linux
    by_identity: HashMap<(u64, u64), DocumentId>,   // (dev, ino) when known
}
```

Rules:

- **One `Document` per file per process.** Many tabs/windows reference it; it is dropped only when the registry and every tab release it.
- `Document` is never handed out as `&mut`; mutate one interior cell at a time through `&self` methods.
- Dirty is `current hash != saved.hash`; hash on save/reload/lease release, not every keystroke.
- The start page is a tab source, never a `Document`.
- `SourceBuffer` uses `String`, not a rope: <5 MB, and text shaping dominates splice cost. `line_starts` is rebuilt from `first_line` with `memchr`.

### Editor session and lease

```rust
pub struct EditorSession {
    pub edit: TextEdit,
    pub doc_revision: u64,
    pub base_rev: u64,
    pub eol: Eol,
    pub find: FindState,
    pub syntax: SyntaxCache,
    pub last_edit_at: Instant,
    pub block_mode: Option<BlockEditor>,
}
```

1. `TabView.mode == Editor` is legal only while this tab holds `doc.lease`.
2. `Ctrl+E` asks for the lease. If another window holds it, show **Go there** / **Take over**; takeover is explicit.
3. Edits go `TextEdit` → `doc.apply(delta)` → `revision + 1` → 120 ms parse debounce.
4. A viewer tab reads `doc.buf`; it never owns an `EditorSession`.
5. Closing the lease holder flushes a draft, releases the lease, then prompts if the document is dirty and this is the last view.
6. Tab transfer moves the `TextEdit` and lease with the tab; the source waits for `TabAccepted`.

This intentionally gives up simultaneous independent editing of one file (the specification allowed two CodeMirror buffers and then detected conflicts). One canonical buffer + lease is safer. Record as `substitute` (`T-SHARED`).

### Tabs, windows, shared state

```rust
pub struct Tab { id, source: TabSource, view: TabView, title_cache, minutes, last_shown }
pub enum TabSource { Start, Document(Rc<Document>) }

pub struct TabView {
    anchor: ScrollAnchor, mode: Mode, editor: Option<EditorSession>,
    nav: NavHistory, find: Option<FindState>, toc_filter: String,
    tree_root: Option<PathBuf>, details_open: BTreeSet<String>,
    pending_goto: Option<Goto>, preview_scroll: f32,
    scroll_target: f32, section_line_hidden_until: Option<Instant>,
}

pub struct WindowState {
    id: WindowId, shared: Rc<AppShared>,
    tabs: Vec<Tab>, active: usize, closed: Vec<ClosedTab>,
    left: PaneState, right: PaneState, width: f32, ui: UiState,
    worker: Arc<WorkerMailbox>, ui_inbox: Rc<UiMailbox>,
    images: OnceCell<ImageCache>, math: RefCell<MathCache>, kit: RefCell<KitCache>,
    closing: ClosingState, last_focus: Instant,
}

pub struct AppShared {
    dirs: AppDirs, app_id: &'static str,
    prefs: RefCell<Prefs>, docs: RefCell<DocumentRegistry>,
    parse: RefCell<HashMap<DocumentId, Arc<MdDoc>>>,
    windows: RefCell<WindowRegistry>,
    store: StoreHandle, jobs: JobHub, watcher: WatcherHandle, instance: InstanceServer,
    next_window: Cell<u64>, quitting: Cell<bool>, startup: RefCell<StartupState>,
}
```

`AppShared` is `Rc`, created on the UI thread before the first window. Workers get only `Send` services (store sender, job senders, `Arc<WorkerMailbox>`), never `Rc` values.

## 6. Ownership and borrow rules

1. **`Rc` for UI-shared state, `Arc` only at the worker boundary.** A job receives `DocumentId`, `revision`, and an `Arc<str>` snapshot.
2. **Never hold a `Ref`/`RefMut` across a call that could take the same cell.** Clone the `Rc<Document>` in a small block, then borrow one field.
3. **Handlers do not call `&mut self` helpers while a `RefMut` is alive.**
4. **Commands are synchronous.** A command may enqueue a job, but it finishes mutating UI state before returning.
5. **Views are pure.** A view reads state and `cx`; it never writes state or starts a job.
6. **No `unwrap` in UI paths.** A missing document after a close race is normal; return `Option` and show a notice.
7. **No `std::fs` in `lmdv-app`.** Enforce with a grep/CI check.

## 7. Events, commands, jobs, timers

- `CommandSpec { id, title_key, category, keywords, default_keys, run, enabled, checked }`; one table for menus, toolbar, context menus, palette and shortcuts.
- Key routing order: focused `TextEdit::on_input_key` → `AppSpec::on_key` → window-level shortcuts (not used for app commands). Editor keys (`Ctrl+B` etc.) win inside the editor; global chords handle the rest.
- `WorkerMailbox`: `Mutex<VecDeque<JobMsg>>` + `Mutex<Option<Waker>>`; `post` pushes and wakes. `WindowState::poll` drains worker and UI mailboxes and pumps timers; returns whether the view changed.
- `UiMailbox`: same-thread cross-window messages only (`Open`, `ActivateDocument`, `PrefsChanged`, `OfferTab`, `TabAccepted`, `FocusWindow`, `CloseWindow`).
- `rune::Live<T>` is only for per-frame values that must move without rebuilding the tree: reading progress, section-line offset, lightbox pan/zoom, sticky fade. It is not state.
- Job types: `Load`, `Save`, `ProbeImage`, `Parse`, `Search`, `Export`, `Watch`, `Store`. Each has a stale guard (doc identity/revision/token/job id).
- `JobHub`: 2 parse workers, 2 IO, 1 search, 1 export. Parse jobs are latest-wins per document. Never spawn a thread per keystroke.
- Timers: preview 120 ms, draft 1 s, position 300 ms, session 500 ms, scroll sync 80 ms, section line 120 ms, own-write suppression 1.5 s, `g g` 500 ms.

## 8. Markdown pipeline

```text
Document buffer (LF) -> snapshot Arc<str>
  -> lmdv-md::parse (worker)
     front matter scan, math scan/placeholder, pulldown-cmark
     (CommonMark + GFM tables/tasks/strike/footnotes),
     html5ever subset, callout detection, headings/slugs,
     source maps, footnote/TOC/diagnostics
  -> Arc<MdDoc> (Send, no Rune)
  -> lmdv-render::render (UI)
     DocTheme + width + font, memo("b{i}", revision/theme),
     images via ImageResolver, math via MathResolver,
     mermaid via rune_mermaid, El tree with keys/source hooks
```

Why fork: the upstream parser enables only strike/tables/tasks, drops raw HTML, has no footnotes/front matter/anchors/TOC/callouts/math, renders tables as one element (no sticky header), lifts inline images to blocks, and has no block-level extension hooks. The fork is bounded: parser ~1.5k lines, renderer ~2k lines. Do not copy the streaming cache initially; full parse per 120 ms debounce is enough. Revisit incremental parsing only after the perf gate.

Extensions:

| Feature | Approach |
|---|---|
| Footnotes | `pulldown-cmark` footnote events → `Block::Footnote` / reference spans + backlinks. |
| Callouts | Detect `> [!KIND]` at quote start; per-kind icon/color/title; body is Markdown. |
| Front matter | Strip leading `---` block; flat YAML → table/chips; nested/parse-fail → raw fallback; omit from print/copy/DOCX. |
| Heading anchors | GitHub slugger + legacy ids + reserved-id `user-content-` handling; stable keys. |
| `[TOC]` | Build from headings after the marker; real content (printed/exported). |
| HTML subset | GitHub tagfilter first; html5ever fragment; allowlist mapped to native elements; unknown tags hidden; no CSS engine. |
| Math | `lmdv-md` scans and replaces with same-byte-length placeholders; `lmdv-math` renders; inline needs patches 0001 + 0003. |
| Sticky table | Fork table renderer to emit a sticky header sibling; wide-table proxy scrollbar/header translation in `lmdv-render`. |
| Source maps | Every block/inline range maps rendered bytes back to source; used by outline, find, scroll-spy, `--line`, click-to-source, copy-section. |
| Mermaid | `rune-mermaid` 17 families; unsupported → code block. `render_drawing` for lightboxes. |
| Images | `ImageCache` (PNG/JPEG/WebP/SVG), relative resolver, `srcset`/`picture`, light/dark variants, missing-image placeholder. |
| Code | `rune-highlights` 80 lexers; first 128 KiB / 32k ranges; copy button. |

## 9. Editor

**Two modes, one model.** `EditorSession` always owns a `TextEdit`; rendering switches:

- **Single-field** (default while `len <= 64 KiB && lines <= 1500`): one `text_input`, full selection/undo/IME.
- **Block** (large): one `TextEdit` per blank-line-separated block in a virtualized column; per-block history + a global journal so `Ctrl+Z` spans blocks. Selection is within a block (documented substitute). Auto-switch above the threshold; a pref can force it.
- Tune the threshold with `long_note`-style benchmarks; lower it if typing p95 exceeds 16 ms at 32 KiB.

Layout: source left, live preview right, 8 pt draggable divider, format bar above the source; below 760 pt show the source alone with a preview toggle. Preview parse keeps the last good `MdDoc` while pending. Scroll sync is one-way editor→preview (throttled) plus preview double-click→caret.

Build:

- Syntax spans from `rune_highlights` (debounced 120 ms; visible window above 128 KiB).
- Find/replace with literal + optional regex/whole-word/case; paint-only highlights; one undo group for replace-all.
- Format bar, `/` menu, line commands, comment toggle (`<!-- -->` or fenced language), go-to-line, `Ctrl+D` selects next occurrence (single selection in v1).
- Multi-cursor staged: Phase 3 selects next occurrence; Phase 7 adds a Rune patch for caret rects and a `MultiCursor` model; otherwise documented gap.
- Spellcheck: `lmdv-edit::Spell` worker with `zspell`/`spellbook`, `/usr/share/hunspell` + optional bundled dictionaries; underline spans + editor context menu; hide if no dictionary.
- Drafts: 1 s after edit, tab switch, close, quit; LF-normalized; 20 / 2 MB; restore/discard/compare; never auto-save to the file.
- Save: explicit; `expected_meta` conflict check; `Save As` updates key/registry/recents/positions/draft atomically; strip BOM, never re-add; preserve final newline; CRLF only on write.
- Own-write echo: `(dev, ino, mtime, size)` suppression 1.5 s; watcher events re-read and compare text.

## 10. Multi-window and tabs

- One `Document` per file; a window has at most one tab per document; two windows may show the same document (one lease holder edits, the other is a live viewer).
- Tab lifecycle: open activates existing; new tab after active; replace an untouched start page; close asks only when this is the last view of a dirty doc; last tab → start page; reopen stack 25; `Ctrl+Tab` cycles; reorder; tear-out; cross-window drop.
- Routing: existing doc → idle start-page window → most-recently-focused → any ready → new. `--new-window` bypasses. A file that does not exist still routes by path and shows “Not found”.
- Window close patched to ask once for all dirty docs. `Ctrl+Shift+W` same. `Ctrl+Shift+N` new window, cascaded 32 pt.
- Cross-window tab drag: X11 uses screen points + `TabOffer`/`TabAccepted`; Wayland falls back to **Move tab to window…** / **Move to new window** (`substitute`).
- Session restore is an intentional extension: store windows/tabs/anchors/mode/nav/panes; do not store editor text (drafts handle that). `T-SHARED` is a substitute for independent per-window buffers.

## 11. Persistence

Use `rune::dirs::AppDirs::new("md.lmdv")` (XDG on Linux; `RUNE_DIRS_ROOT` for tests).

| Data | Path | Format |
|---|---|---|
| Prefs | `$XDG_CONFIG_HOME/md.lmdv/prefs.json` | `Prefs`, pretty JSON |
| Recents | `$XDG_DATA_HOME/md.lmdv/recents.json` | array, max 12 |
| Positions | `$XDG_DATA_HOME/md.lmdv/positions.json` | map, max 100 |
| UI state | `$XDG_DATA_HOME/md.lmdv/ui.json` | panes, collapsed folders, palette recents |
| Session | `$XDG_DATA_HOME/md.lmdv/session.json` | windows/tabs/anchors/placements |
| Drafts | `$XDG_DATA_HOME/md.lmdv/drafts/<blake3(key)>.json` | one per draft, max 20/2 MB |
| Math cache | `$XDG_CACHE_HOME/md.lmdv/math/` | optional SVG/PNG |
| Logs | `$XDG_STATE_HOME/md.lmdv/logs/` | crash log only |
| Socket | `$XDG_RUNTIME_DIR/lmdv/<instance-id>.sock` | 0600, dir 0700 |

- Every file has `schema` + `app`; `#[serde(default)]`; unknown fields ignored; `.bad-<timestamp>` quarantine on parse failure; pure migration functions.
- Atomic write: temp in the same dir → `write_all` → `sync_all` → `rename` → fsync dir.
- One store thread coalesces patches (max one write per file per 300 ms). `flush` only on close/quit.
- Portable: `lmdv-portable.txt` beside the executable → `<exe>/data/`, distinct instance id; `LMDV_HOME` override for tests.
- Crash: drafts are at most 1 s old; restore file drafts on open (offer, not auto), pasted drafts automatically once per launch in the first window.

### 11.1 P1-01 implementation notes (`lmdv-io`)

The shipped `lmdv-io` API is the contract the app builds against. It differs from the sketch above in these documented ways:

- `StorePatch::Draft` is a struct variant: `Draft { key: String, draft: Draft }`, because a `Draft` does not carry its key.
- `Recent`, `PositionTable`, `UiPrefs`, `SessionSnapshot`, `WindowSession`, `TabSession`, `ClosedTabSession` and `WindowPlacement` live in `lmdv-io` (they describe persisted state, not the pure `lmdv-core` graph).
- State files use an envelope `{"schema":1,"app":"lmdv","data":…}`; `load_*` clamps/rejects caps.
- The instance wire request is `{"v":1,"launch":{…},"activationToken":…}`; the ack is `{"v":1,"ok":true,"error":null}`.
- `parse_args` takes full argv including the program name and accepts `String`/`OsString`/`&str`; it always stores an absolute lexical `cwd`.
- A leading `//host/share` is preserved by lexical normalization so the path policy can reject it.
- `read_document`/`probe`/`save_document` also reject network/device/system paths, not only assets.
- `AppDirs::resolve()` never fails: it falls back to a private temp root; `ensure()` returns the I/O error.
- `TreeEvent` is a unit `Changed` (re-list), matching the specification.

The app must wire the store/watchers/instance, apply `PathPolicy` to links/assets, map `WriteError::Conflict`/`Deleted` to dialogs, and call `flush` on close/quit.

## 12. Security

- **HTML:** GitHub tagfilter first; allowlist tags/attributes; drop `on*`, `style`, `srcdoc`, `formaction`, `popover`, `command*`, `interestfor`, `ping`; `data:` only small `data:image/*`; no CSS engine; sanitize exports again.
- **Paths:** canonicalize on a worker; embedded assets confined to the document root by default; reject `/proc`, `/sys`, `/dev`, sockets, FIFOs, non-regular files; reject `//host/share` and device paths before canonicalization; symlinks only if resolved under root.
- **Links:** `http/https/mailto` open externally on explicit action; `file:`/`javascript:`/`vbscript:` ignored; relative Markdown links may open any local Markdown file; non-Markdown asks before external open.
- **Images:** 32 MiB / 64 MP caps; SVG external refs/scripts rejected; bounded rasterization; broken image placeholder, not an error card.
- **Huge content:** open cap 32 MiB; >8 MiB warning + viewer; table render cap 64 columns/10k rows (preview virtualizes); parse cap 5 MB / 200k blocks; else plain viewer + notice.
- **Watcher:** watch folders not files; 300 ms debounce; classify by `stat`; always re-read + compare text; own-write suppression; dirty docs never auto-reload; generation guard.
- **Socket:** 1 MiB max, versioned JSON, `SO_PEERCRED` uid check, stale socket removed only if owned by uid and no live process answers; listener only forwards a launch.
- **Writes:** atomic replace, preserve mode, no symlink following; never write on open/scroll/render/watch; task ticks are the only viewer write; saving never recreates a deleted file unless **Save again**.

## 13. Testing and gates

Layers: unit (core/buffer/EOL/path/slug/math/table/find/line ops), golden parser (`MdDoc`), golden render (tree JSON + PNG at 1x/2x), a11y (roles/names/focus/live regions), interaction scripts, export (HTML exact/normalized, DOCX unzip+XML, PDF text, PNG dims), security corpus, perf, manual Wayland/X11/NVIDIA/IME/multi-window/MIME.

Fixture families: F-CORE, F-MARKDOWN, F-TABLE, F-EDIT, F-TABS, F-TREE, F-SYSTEM, F-EXPORT, F-PALETTE, F-ERRORS, plus `commonmark-math.md`. Every fixture maps to parity IDs.

Headless scenes: viewer light/sepia/dark/black, callout, math, table, frontmatter, TOC, footnotes, HTML, mermaid; editor split/find/formatbar/large; palette/share/help/keys/lightbox/table-preview; dirty-close/save-conflict/draft-restore/missing-file/render-error. Use `--size 1280x900 --scale 2` for goldens.

Performance budgets (p95, not median): launch <350 ms warm; 1 MB open <250 ms; parse 500 KB <80 ms; typing 64 KB single field <16 ms; 1 MB block mode <16 ms; scroll 2k blocks <8 ms CPU; idle zero frames/timers; HTML 1 MB <1 s; PDF 50 pages <5 s; PNG diagram 2x <1.5 s.

## 14. Orchestration

- **One file, one owner.** Orchestrator owns shared manifests, docs, scripts and app wiring; workers own bounded crates/modules; reviewers are read-only.
- Every task gets a **context capsule**: current phase, gate, what exists, frozen signatures, spec `file:line`, known failures, do-not-touch, next.
- Workers must provide code + tests + evidence; the orchestrator reads the diff and reruns every gate. A worker's “it passes” is not evidence.
- Re-prompt with exact failing command/output/expected/actual and a file scope; one defect per bullet; if it fails twice, narrow or take over.
- Parallelize only after an interface freeze: P1 core/IO/app shell; P2 parser/renderer/math; P3 editor/file-safety; P5 HTML/DOCX/PDF. Never parallelize a shared manifest or domain type.
- A phase never advances on a red gate. Update `docs/STATUS.md`, the parity checklist and OpenChamber at every handoff.

## 15. Phase plan and first tasks

| Phase | Goal | Gate |
|---|---|---|
| P0 Bootstrap + Rune proof | Workspace, vendor, patches, proof app, snapshot harness | `cargo check --workspace`; inline math baseline; close-veto test; `--snap`; vendor checksum |
| P1 Core + IO + viewer shell | Domain, registry, store, socket, windows/tabs, open file, plain Markdown | 3 tabs; second-instance routing; position survives; dirty close veto; `--tree` |
| P2 Markdown parity | `MdDoc`, extensions, renderer, math, mermaid, images, outline, source maps | F-MARKDOWN/F-TABLE/F-MATH; goldens/tree; math baseline; sticky table; TOC |
| P3 Editor + file safety | Single-field editor, syntax, find/replace, line ops, save/EOL/conflict, drafts, watcher, task ticks | E-01–E-20, P-04–P-06; flows; typing p95; no file IO in handlers |
| P4 Multi-window + session | Shared registry, tab transfer/tear-out, routing, reopen, prefs/title sync, restore | Two windows same doc; move tab; close prompts; routing matrix; X11 drag; Wayland fallback |
| P5 Export | HTML, DOCX, PDF/print, PNG/SVG, table copy | X-01–X-16; self-contained HTML; DOCX opens; PDF pages/header; 2x PNG |
| P6 Parity extras | File browser, folder search, table preview, lightbox, palette, menus, history, help, media, i18n, a11y | F/O/C/V/A/I/H rows; keyboard matrix; Orca |
| P7 Hardening + packaging | Fuzz/security, large files, perf, Wayland/X11, MIME/desktop, full audit | Security corpus; budgets; manual matrix; parity no untriaged rows; install script |

First five tasks:

1. **P0-01** vendor Rune, root manifests, `[patch]`, `UPSTREAM.md`, patch `Span::image`/close veto, proof app.
2. **P0-02** `lmdv-core` IDs, `SourceBuffer`, `Document`, `DocumentRegistry`, tests.
3. **P1-01** `lmdv-io`: XDG paths, store, atomic write, load/save, socket, CLI parser.
4. **P1-02** `lmdv-app` skeleton: `AppShared`, `WindowState`, mailboxes, commands, shell, tabs, start page.
5. **P1-03** viewer integration: Markdown base, outline/anchors, positions, headless scenes; run P1 gate.

## 16. Open design questions

1. Inline math patch: implement in P0 or fall back to block-only? (The plan requires the patch for parity.)
2. DOCX: custom OOXML + best-effort OMML, or image-only equations? (C recommends OOXML; accept image fallback initially.)
3. PDF: in-process Typst vs a helper binary? (Start in-process; split if build time hurts.)
4. Linux titlebar: native frame accepted? (Default yes.)
5. Cross-window tab drag: X11-only implementation or no cross-window drop anywhere? (Default X11 + Wayland menu fallback.)
6. Multi-cursor: patch Rune or ship a documented gap? (Default gap until P7.)
7. Editor threshold: 64 KiB / 1,500 lines; tune with benchmarks.
8. Session restore is an extension beyond the specification; keep it on by default.

See `docs/RISKS_AND_SPIKES.md` for the spike specs and `docs/STATUS.md` for what is currently blocking.
