# Open items and follow-ups

This is the **implementation follow-up register**. It is not the behavior spec (`docs/BEHAVIOR_SPEC.md`) and not the parity checklist (`docs/PARITY_CHECKLIST.md`); it records the concrete limitations, deviations and follow-ups reported by workers or found in review, and what we decided to do about each.

**Rule:** a worker report is not accepted until every “known limitation / follow-up” in it is either
1. fixed in the same task,
2. added here with a phase and owner, or
3. accepted here as a documented substitute/gap (with the user's approval if it affects P0/P1).

At a phase gate, no item scheduled for that phase may remain `open`. `accepted` items are not silently forgotten; they are linked from `docs/FEATURE_PARITY.md`.

## 1. Register

| ID | Source | Limitation / follow-up | Decision | Phase | Status |
|---|---|---|---|---|---|
| OI-001 | P1-02 app | Startup reads prefs/positions/recents on the UI thread before the first frame, not as a job | Move to a startup IO job once the parsing job exists; keep deterministic scenes | P2/P3 | open |
| OI-002 | P1-02 app | Rune's native file dialog is single-select; the specified `Ctrl+O` multi-select is T-32 | Accepted gap until Rune exposes multi-select; document in parity | P6 | accepted-gap |
| OI-003 | P1-02 app | Tab strip sits below the toolbar rather than inside it | Visual polish; not a behavior block | P6 | scheduled |
| OI-004 | P1-02 app | Recents show stale paths until opened; relative times were added in P1-03 | Filter with `probe`/`existing_paths` on a timer | P2/P6 | open |
| OI-005 | P1-02 app | Opening the same file through two symlink spellings can briefly create two docs | Registry identity refreshed after each atomic save; still no pre-insert canonical probe | P3 | open |
| OI-006 | P1-02 app | Dirty native close shows a notice, not Save/Don't/Cancel | Tab/window/native close now use Save / Don't save / Cancel; window names all dirty docs | P3 | fixed |
| OI-007 | P1-01 IO | Corrupt state is quarantined but load functions do not return a notice; future-schema policy undecided | Add `StoreIssue`/notice and a read-only future-schema policy | P3 | open |
| OI-008 | P1-01 IO | `DocWatcher`/`TreeWatcher` exist but are not wired to window mailboxes, own-write suppression or focus rearm | Watcher thread + sequence-tagged inbox; 1.5 s own-write suppression; reload/rename/delete wired | P3 | fixed |
| OI-009 | P1-01 IO | `PathPolicy` is not yet applied to document links/assets | Applied in the P2 image/link resolver | P2 | fixed |
| OI-010 | P1-01 IO | No >8 MiB large-document warning; open cap is 32 MiB | Add warning and viewer-mode default | P3 | open |
| OI-011 | P1-01 IO | Save conflict/Deleted are not mapped to UI dialogs; `create=true` only for Save again is not wired | Overwrite/Keep theirs, Save again/Cancel; `create=true` only for Save again | P3 | fixed |
| OI-012 | P1-01 IO | Portable data root that cannot be created does not fall back to XDG | Add fallback + notice | P4 | open |
| OI-013 | P1-03 viewer | Temporary `rune-kit::markdown` reader and direct `pulldown-cmark` in `lmdv-app` | Replaced with `lmdv-render` over `MdDoc`; dependency removed | P2 | fixed |
| OI-014 | P1-03 viewer | Positions are raw scroll offsets, not heading ids | Heading-id anchors from `MdDoc`/layout now written | P2 | fixed |
| OI-015 | P1-03 viewer | Parsing runs on the UI thread per tab revision | Worker parse job + shared `Arc<MdDoc>` cache | P2 | fixed |
| OI-016 | P1-03 viewer | Outline lists only top-level h1–h4; block-image splits lack stable keys | Outline from `MdDoc.headings`; block keys from spans | P2 | fixed |
| OI-017 | P1-03 viewer | Scroll-spy lacks the end-of-document activation slide and live-value optimization; outline rows use list semantics, not ARIA buttons | P6-READ-B: the activation line slides through the last viewport; rows carry a button role (the kit list wrapper remains, OI-163). | P6 | fixed |
| OI-018 | P1-03 viewer | No links, task toggles, math, footnotes, callouts, sticky tables, TOC marker, section line, progress or reading time | All done: links/math/footnotes/callouts/tables/TOC/task ticks (P2–P3) and section line/progress/reading time (P6-S) | P6 | fixed |
| OI-019 | P1-03 viewer | Reader theme/typography prefs not wired | P6-PREF settings + full Sepia/Black palettes (chrome, article, Mermaid, dialogs) and E-37 editor scaling (0.875×, live) | P6 | fixed |
| OI-020 | P2-02 math | Cache hits clone the RGBA buffer; app should cache `Pixels`/`InlineImage` per formula | Per-window `MathCache` caches `InlineImage` by tex/size/color/display/scale | P2 | fixed |
| OI-021 | P2-02 math | Math errors are not cached; caller should cache an error chip per formula | Errors cached; raw TeX fallback | P2 | fixed |
| OI-022 | P2-02 math | Fallback glyph metrics for CJK/emoji inside `\text{}` are RaTeX defaults | Accept for now; feed host fallback metrics if it becomes visible | P6 | accepted-gap |
| OI-023 | P2-02 math | No display max-width, SVG, PDF or DOCX/OMML here | Later export layers | P5 | scheduled |
| OI-024 | P2-01 parser | `find` is not implemented in the parser; left to renderer/app | Implement source-range find in the renderer/app using `MdDoc` | P2 | open |
| OI-025 | P2-01 parser | Source maps can be off at entity/escape-heavy boundaries | Add entity mapping + a fixture; normal text/emphasis/code/links/math are exact | P2/P6 | open |
| OI-026 | P2-01 parser | `<details open>` initial state is not kept; inline `<picture>` sources are not collected into an `ImageBlock` | Renderer-side handling / optional parser follow-up | P2 | open |
| OI-027 | P2-01 parser | Reserved app ids are a static list; app UI ids could collide later | Extend `heading::is_reserved_id` when the app adds ids | P6 | scheduled |
| OI-028 | P2-01 parser | Footnote definitions register headings with `block = usize::MAX` | Guarded in `toc_headings` and tested | P2 | fixed |
| OI-029 | P2-01 parser | An unreferenced footnote definition can surface a second footnote it references (rare) | Accept; document if a fixture hits it | P6 | accepted-gap |
| OI-030 | P2-01 parser | `HtmlBlock` roots are `Fragment`; `block_index_at` returns `None` in gaps | Fragment renders; gaps are not clickable | P2 | fixed |
| OI-031 | P2-01 parser | Math sentinels where U+FFFC would overwrite a newline | Renderer handles widened placeholders; `commonmark-math` fixture covers it | P2 | fixed |
| OI-032 | P6-01 i18n | Strings/help still carry a legacy product name | Decision: no legacy product name in app-visible strings or help; all six locale catalogs, window title and help pages now use lmdv, and the name appears only in the README attribution line. | P6 | fixed |
| OI-033 | P6-01 i18n | `rune-i18n` has no date/relative-time formatting | P6-I18N-A: start-page relative times use `rel.justNow` plus CLDR plurals for minutes/hours/days; `medium_date` stays the localized export date. | P6 | fixed |
| OI-034 | P6-01 i18n | No CLDR plural pairs in the 409-key inventory, so `PLURALS` is empty | P6-I18N-A added five plural bases (`editor.lines/blocks`, `rel.minutes/hours/days`) in all six catalogs; `PLURALS` is non-empty. | P6 | fixed |
| OI-035 | P6-01 i18n | Help HTML fragments are copied but not integrated into a full help dialog, and still contain Windows/Store-only passages | P6-I18N-A removed File Explorer/Store-only passages and rewrote the Windows section for Linux; P6-I18N-B embeds all six pages and renders them in the F1 full-help dialog. H-04 license/repo links remain OI-165. | P6 | fixed |
| OI-036 | Patch 0001 | Copy is not auto-skipped for inline images; the app must call `El::uncopied` over the placeholder | Renderer marks math objects `uncopied` | P2 | fixed |
| OI-037 | Patch 0001 | Accessibility exposes the U+FFFC placeholder as text | Give math an accessible label/skip the placeholder | P2/P6 | open |
| OI-038 | Patch 0001 | Root re-exports (`rune::InlineImage`) are not added; use `rune::text::InlineImage` | Accepted; no need for a root re-export yet | — | accepted |
| OI-039 | Patch 0001 | The macOS-only smoothing test `pipelines_never_share_masks` fails on Linux | Unrelated to lmdv; not part of the gate | — | accepted |
| OI-040 | P2-01 parser | `stacker` added to grow deep recursion for 200-level nesting | Accepted; keeps malformed input safe | — | accepted |
| OI-041 | P2-03 renderer | Inline `<picture>` loses its `<source>` children and is emitted twice; inline scheme switching impossible (block-level `<picture>` works) | Needs parser model support or a single resolved object | P2/P6 | open |
| OI-042 | P2-03 renderer | A `<details>` body after a blank line is a separate top-level block; the card only owns HTML inside one block | Parser/model block-range association or renderer-side association | P2/P6 | open |
| OI-043 | P2-03 renderer | No paint-only highlight API for find/`--line`; needs source-range → rendered-range mapping through `Inline::source_map` | P6-R `RenderOptions::highlights` + `HighlightKind::{Line,Find,Current}` + `highlight_key`, paint-only and memo-safe | P6 | fixed |
| OI-044 | P2-03 renderer | Table action bar, copy-as-table and pinned scrollbar are app/UI affordances not in the renderer | P6-T action bar + copy/preview done; the specified pinned scrollbar remains | P6 | open |
| OI-045 | P2-03 renderer | Invalid-Mermaid banner shows the localized generic notice, not the exact parser error | `rune_kit::mermaid` returns `Option<El>` without the error | P6 | accepted-gap |
| OI-046 | P2-03 renderer | Video/audio are placeholder cards; playback continuity and `<track>` subtitles remain gaps | Rune has no Linux media backend | P5/P6 | accepted-gap |
| OI-047 | P2-03 renderer | Image loading/`Image::wait` polish and broken-image behavior need app integration | Per-window `ImageCache` + policy + missing placeholder wired | P2 | fixed |

| OI-048 | P2 integration | Renderer emits `copy-code:code` for every block; app copies only when a document has one top-level code block | Renderer should emit `copy-code:b{i}` | P3 | open |
| OI-049 | P2 integration | First image resolution does a `PathPolicy` stat/canonicalize in the view; later hits cache | Add a `ProbeImage` job/cache in P3 | P3 | open |
| OI-050 | P2 integration | `data:` images are unsupported | Decide safe data-image policy (small images only) | P6 | scheduled |
| OI-051 | P2 integration | Non-Markdown local links refuse with a notice; no Open externally? confirmation | P6 link-open confirmation | P6 | open |
| OI-052 | P2 integration | Headless gates parse inline and use `Image::wait` so one frame shows the article; interactive windows use workers | Documented in `docs/TESTING.md` | P2 | fixed |
| OI-053 | P2 integration | Task checkboxes are read-only in P2 | Viewer ticks write immediately with Undo; edit mode stays read-only | P3 | fixed |

| OI-054 | P2 gate | Performance budgets for P2 (parse 500 KB, scroll 2k blocks) are not measured yet | Add a parse/scroll bench in P3/P7 | P7 | open |
| OI-055 | P2 gate | Tests render the fixtures, but no committed PNG/tree golden corpus exists yet | Assemble `tests/golden`/`tests/trees` from the P2 scenes | P3/P6 | open |
| OI-056 | P2 gate | Preview click/double-click → source is editor work, not reader work | E-35 double-click and E-04 Edit from here across single-field and block mode; block wire carries text/line refs | P3 | fixed |

| OI-057 | P3-01 editor | Block-mode fields are not bound in the app; large docs still use one `TextEdit`, and typing does not auto-split/merge blocks | P3-04a virtualized fields, journal undo, split/merge on Enter/Backspace/Delete; Tab indents | P3 | fixed |
| OI-058 | P3-01 editor | Block journal stores full before/after block text; huge blocks can be expensive; no block-size cap | P7-B measured: 1 MiB single-block field p95 2.39 ms, linear; cap ~256 KiB per field and skip unnecessary relayout | P7 | open |
| OI-059 | P3-01 editor | Syntax window starting after a fence opener loses that fence's language | P6 syntax polish | P6 | open |
| OI-060 | P3-01 editor | `toggle_comment` no-ops for unknown fence languages and indented code blocks | P6 comment coverage | P6 | open |
| OI-061 | P3-01 editor | Format-bar active state and slash-menu filtering are app-side heuristics | P3-03 implemented them; syntax-tree parity and Setext detection remain | P6 | open |
| OI-062 | P3-01 editor | Regex find skips zero-length matches | Accepted; document in find options | P6 | accepted-gap |
| OI-063 | P3-01 editor | `goto_line` columns count Rust chars, not UTF-16 units | Minor CodeMirror difference; accepted | P6 | accepted-gap |
| OI-064 | P3-01 editor | Typing/block-mode performance not measured (SP-03) | P7-B `typing_bench`: 64 KiB single-field p95 0.031 ms; 1 MiB block p95 0.166 ms; undo depth finding; 64 KiB threshold kept | P7 | fixed |

| OI-065 | P5-01 export | HTML export drops video/audio `<track>` subtitles | Same gap as OI-046; revisit with a media backend | P5/P6 | accepted-gap |
| OI-066 | P5-01 export | HTML, DOCX and PDF export math is a 2× PNG with TeX in `alt`/`descr`/PDF `/Alt`; no MathML (rich copy, X-02) or OMML (DOCX, X-08) | Add MathML to the HTML/rich-copy serializer and OMML to DOCX; PNG is the fallback | P5-04/P6 | open |
| OI-067 | P5-01 export | Callout default titles, TOC label and footnote labels are English; no catalog keys | P6-01b added 10 keys to all six catalogs; reader and HTML/DOCX/PDF use them | P6 | fixed |
| OI-068 | P5-01 export | Export performance is not benchmarked; a large document re-renders every formula and picture into the asset table | PDF cold export ~10 s on `test-features.md`; benchmark and reuse math/image caches | P7 | open |
| OI-069 | P5-01 export | The app has no Share menu, export job or `lmdv-export` dependency yet | P5-04a wired the Share menu, HTML/DOCX/PDF jobs, save dialogs, flash/notices and Print | P5 | fixed |
| OI-070 | P5-01 export | Explicit `file:` links are refused; relative links become `file:///` only when `source_path` is absolute | Accepted security choice; document in parity/help | P5 | accepted-gap |

| OI-071 | P3-02 app | Save As has no collision/diamond handling and basic naming only (E-11/E-12) | Add overwrite confirmation and diamond naming | P6 | open |
| OI-072 | P3-02 app | Opening another document while editing does not prompt (E-14); the editing tab keeps its session | Prompt or document the accepted behavior | P6 | open |
| OI-073 | P3-02 app | Drafts for deleted/unreadable files are not offered; watcher setup errors are recorded but not shown | Surface watcher notices and stale-draft recovery | P6 | open |
| OI-074 | P3-02 app | Dirty caches are per-window until the P4 shared-registry sync | P4-01 publishes dirty ids/titles in the registry; other windows are not woken and tab dots stay per-window | P4 | open |
| OI-075 | P3-02 app | Scroll sync is caret→preview only; no interpolated bidirectional sync (E-07) | P3-04c2 one-way editor→preview with interpolation, caret reveal, sticky-header cover, entry settling; manual preview scroll stays free | P3 | fixed |

| OI-076 | P5-02 DOCX | SVG/AVIF assets are not embedded (alt text / Mermaid source fallback) | Rasterize or add an SVG+PNG fallback in the export job | P5 | open |
| OI-077 | P5-02 DOCX | Footnotes are superscript numbers plus a plain end list; no `word/footnotes.xml` or working backlink | P6-X real footnotes part, content type/rels, styles, first-reference bookmarks and backlinks; repeated refs share ids | P6 | fixed |
| OI-078 | P5-02 DOCX | TOC is a linked heading list, not a Word `TOC` field | Optional field-based TOC later | P6 | open |
| OI-079 | P5-02 DOCX | DOCX package is uncompressed (stored ZIP); valid/deterministic but larger | Revisit if export size matters; deflate needs a fallible path | P7 | open |
| OI-080 | P5-02 DOCX | Quoted list runs inside a block quote keep the body color (paragraph-mark color gap) | Minor style polish | P6 | open |

| OI-081 | P3-03 app | Find match scrolling approximates wrapped lines by the 20 pt line height | P6-R highlight keys + `Tx::scroll_into_view(highlight_key)` now used | P6 | fixed |
| OI-082 | P3-03 app | Slash menu is an inline panel, not caret-anchored; inserting takes two undo steps | Anchor the panel to the caret and group the replace+insert | P6 | open |
| OI-083 | P3-03 app | Go-to-line errors, "Select line" and some command titles lack `lmdv-i18n` keys | P6-I18N-A keyed the go-to-line errors, “Select line”, and the remaining command titles in all six catalogs. | P6 | fixed |
| OI-084 | P3-03 app | `Ctrl+F`/`Ctrl+H` do nothing in viewer mode; reader find is still missing | P6-V reader find bar with model matching, per-tab state, highlights and F-16 carry | P6 | fixed |
| OI-085 | P5-03 PDF | Default PDF font is bundled Libertinus Serif; the app must supply a proportional sans and mono font | P5-04a loads Noto Sans + DejaVu Sans Mono via `fontdb`, cached per process, with a fallback note | P5 | fixed |
| OI-086 | P5-03 PDF | No X-12 wide-table fitting; a pathological unbreakable cell can overflow | P6-PDF-T min-content measurement, 0.3-floor scaling and break-anywhere fallback; bbox proof | P6 | fixed |
| OI-087 | P5-03 PDF | Raw HTML tables/lists/details flatten to inline text; video/audio are placeholders | P6-PDF-H structured tables (spans/headers), lists/start, details cards, quotes/code, file links for media; remote URLs never printed | P6 | fixed |
| OI-088 | P5-03 PDF | Code/tables may split across pages; long code tokens use U+200B break opportunities; no PDF/UA validation | Page-break rules, cleaner code wrapping, PDF/UA gate | P6/P7 | open |

| OI-089 | P3-04a app | Typing into a block's trailing blank line (or pasting several paragraphs into one field) can leave the in-memory block layout without a blank-line boundary until reload | P3 polish canonical re-partition on commit (edited field ± neighbour, one journal step) | P3 | fixed |
| OI-090 | P3-04a app | Structural undo (split/merge) restores text and focus but the caret lands at the merged field's start/end | `caret_block` + caret anchors restore split/merge undo/redo and focus | P3 | fixed |

| OI-091 | P3-04b app | `TaskWire` hands the item row, not the checkbox control; the app overlays a transparent 22×15 hit area mirroring renderer geometry, and the real checkbox has no keyboard press | `TaskControlWire` wraps the real checkbox; overlay removed; press + Space/Enter work | P3 | fixed |
| OI-092 | P3-04b app | `TaskRef` has only list-local `item`; nested lists repeat indexes, so the app reconstructs labels and refuses ambiguous markers | `TaskRef::source` filled from the inline source map; app locates by it | P3 | fixed |
| OI-093 | P3-04b app | Task notices/prompts have English fallbacks not in the six catalogs; buffer-only ticks (dirty/pasted) have no Undo | P6-I18N-A keyed every task notice; buffer-only ticks still have no Undo. | P6 | open |
| OI-094 | P3-04b app | A second tick while a task save is in flight is refused rather than queued/coalesced | Decide queue vs coalesce; keep `expected_meta` meaningful | P6 | open |

| OI-095 | P3-04c1 app | Double-click on block padding (outside text) does nothing; two keyless text elements in one block can map to the first repeated word; E-35's preview-height caret is not applied | Add a padding→line fallback, better segment identities, and viewport placement with E-07 | P6 | open |
| OI-096 | P3-04c1 app | The block context menu replaces Rune's built-in text Copy menu | Whole-document rich copy done; export-side `export_rich_fragment` + `blocks_for_source` ready (P5-04c); app selection/menu wiring still open | P5/P6 | open |
| OI-097 | P3-04c1 app | The block wire copies each block's drawn text per frame in both modes | Edit-mode-only wire / `Rc<str>` when the reader virtualizes | P7 | open |
| OI-098 | P3-04c1 app | Keyboard access is Shift+F10 on a clicked block; no shortcut-bound Edit from here; word boundaries cover main combining-mark ranges only | P6-READ-B: `F2` / palette `Edit from here` uses the block at the reading anchor; word boundaries use a sorted combining-mark table across major scripts. Historic/new marks may remain absent. | P6 | fixed |

| OI-099 | P6-01b i18n | Residual English a11y labels: `Open externally` (media), `Footnote {n}`, `Math: {tex}`, HTML backlink aria “Back to reference” | P6-01c added/wired `render.openExternally`, `render.footnoteN`, `render.math`, `render.footnoteBack` in all six catalogs | P6 | fixed |

| OI-100 | P3-04c2 app | Single-field sync counts visual lines so wrapped prose maps ahead; block mode scales row height across the block's source lines | Exact rendered-line mapping (needs OI-043/081) and a per-block visual-line API | P6 | open |
| OI-101 | P3-04c2 app | Sticky-header cover measured only around the editor top/caret; overlapping tables use the max; front matter and section-line reserves are approximations | Measure all headers or add a geometry wire; front-matter interpolation | P6 | open |
| OI-102 | P3-04c2 app | Block mode re-runs the view once per visible-row crossing to measure the first row | Profile; make `on_visible_rows` bookkeeping-only with live measurement if it janks | P7 | open |

| OI-103 | P5-04a app | Export uses the last parsed `MdDoc`; edits inside the 120 ms preview debounce or an unparsed revision are excluded | Flush/parse before export or export from the editor buffer | P6 | open |
| OI-104 | P5-04a app | PDF date is UTC; “Show in folder” opens the folder without selecting the file; Print leaves its temp PDF behind | Local-time formatter (OI-033), file-selection helper, temp cleanup | P6 | open |
| OI-105 | P5-04a app | Tab switching is allowed during an export (the specification blocks tab moves); the job snapshots the model | Decide whether to block or keep snapshot semantics | P6 | open |
| OI-106 | P5-04a app | Mermaid is exported as its source because the job has no offscreen diagram renderer | Reuse the reader's Mermaid raster in the export job | P5-04b | open |
| OI-107 | P5-04a app | With `print_break_h1` already true, the Share menu may focus the checked setting instead of Print | Rune popup convention; revisit if it confuses | P6 | accepted-gap |

| OI-108 | P5-04b app | Rune clipboard writes have no failure callback; the check flash reflects a successful job, not proof the OS write landed | Accept as a Rune limitation or add a clipboard ack if exposed | P6 | accepted-gap |
| OI-109 | P5-04b i18n | Rich-copy success reuses `share.copy`; no dedicated “copied rich text” catalog key | P6-I18N-A added `msg.copiedRich` in all six catalogs. | P6 | fixed |
| OI-110 | P5-04b app | Table TSV copy (X-16) and the hover action bar are still missing | P6-T bar + Copy table (visible rows) + preview; TSV excludes filter-hidden rows | P6 | fixed |

| OI-111 | P3 polish | Normalization checks the edited field ± one neighbour; a freshly typed unclosed fence spanning further blocks may wait for a later commit/reload | Extend the canonical scan for open fences | P6 | open |
| OI-112 | P3 polish | Two nested task items with identical labels at the same list index can share a tree key (write path uses `source`) | Stable key format if tree-driven automation needs it | P6 | open |

| OI-113 | P4-01 app | Tab transfer protocol (`OfferTab`/`TabAccepted`/`Adopt`), cross-window drop and the Move tab menu are not implemented | P4-02b in-process protocol + menu; drag/drop and tear-out remain OI-114 | P4 | fixed |
| OI-114 | P4-01 app | X11 tear-out and Wayland substitutes/placement are not wired; T-21 sole-tab window drag has no Rune API | P4-02c release-only X11 tear-out + cross-window drop; Wayland/menu substitute; live follow and T-21 remain OI-125/126 | P4 | fixed |
| OI-115 | P4-01 app | Aggregate quit/close across windows, last-view-only prompts and pasted-tab rules are not implemented | P4-02a last-view prompts, pasted protection, closing flag, one aggregate `Ctrl+Q` | P4 | fixed |
| OI-116 | P4-01 app | Session capture/restore (windows, tabs, anchors, placement, kind) is not implemented | P4-02d session fragment/debounce, factory restore, clean/crash flag, pasted drafts | P4 | fixed |
| OI-117 | P4-01 app | Activation tokens are dropped (Wayland focus); always-on-top and placement refresh are not wired | Forward `LaunchRequest.activation_token` to `AppHandle::activate`; W-07 pin on X11 | P4 | open |
| OI-118 | P4-01 i18n | “New window” command/notices, tab-move menu strings and the dropped-pasted notice have no catalog keys | P6-I18N-A keyed the new-window notices, “Move tab to window…”, the group label, “No other windows are open.” and the dropped-pasted notice. | P6 | fixed |

| OI-119 | P4-02b app | Tab context menu is partial: no Close others/right (T-23), reopen, reveal or copy path (T-18) | Full menu with per-kind items, Close others/right through the sequential save prompts, reveal/copy path, Save as… and reopen | P4/P6 | fixed |
| OI-120 | P4-02b app | Loading documents and start pages cannot be transferred; `WindowKind::TornOffStart` is not created | Decide load-job ownership; start-page tear-out later | P4 | open |

| OI-121 | P4-02d app | `TabSession` cannot carry nav history, find bar, outline filter, open details or pane state; `anchor_id` is unused and the closed-tab stack is memory-only (not restored across sessions) | Extend the session schema in a later pass; T-07 reopen itself is now implemented | P6 | open |
| OI-122 | P4-02d app | Wayland cannot capture maximized; stored scale is not persisted; no off-screen/clamp logic for restored frames | Clamp restored frames to a visible display and seed scale from the first placement read | P6 | open |
| OI-123 | P4-02d app | A file editor tab with a stale draft equal to disk stays in the viewer; a file restored into two windows can start duplicate loads/recent writes | Re-apply stored mode after a silent draft drop; dedupe restore loads | P6 | open |
| OI-124 | P4-02d app | There is no explicit `--restore-session` flag from ARCHITECTURE_DETAIL §7.5 | Add the optional flag and grammar tests | P6 | open |

| OI-125 | P4-02c app | Tear-out is release-only: the window does not follow the pointer, and `TabDragState::hand_off`/reattach-close are not used | Live-follow spike; reattach closes the torn window without losing the tab | P6 | open |
| OI-126 | P4-02c app | T-21 (dragging a lone tab moves the window) has no Rune `drag_window` API; `lone=false` and the menu substitute | Patch Rune with `WindowHandle::drag_window` or accept the menu substitute | P6 | open |

| OI-127 | P6-R renderer | Highlights over a zero-width object (formula/image) paint nothing; TeX after the opening delimiter, link URLs, front-matter values and generated TOC labels are unmapped | P6-READ-B confirmed the blocker: Rune paints `TextHighlight`s from shaped glyph boxes and the patched inline object is zero-width while the image draws separately. A fix needs a renderer band behind the object or a runtime highlight-box query; the current behavior is pinned by a test. | P6 | open |

| OI-128 | P6-F app | File browser F-12 context menu, F-18 hidden-state persistence and F-19–F-21 pane behaviors are not implemented | P6-M F-12 menu done; F-18 hidden-state and pane shortcuts remain | P6 | open |
| OI-129 | P6-F app | `ui.json` holds one global tree root/collapsed set (last writer wins); collapsed changes are not broadcast; per-window watchers can hit inotify limits | Decide per-window vs shared browser state; surface watcher-limit notices | P6 | open |
| OI-130 | P6-F app | Folder-search placement does not carry the query into a viewer find bar (viewer find is OI-084) | Wire once viewer find lands | P6 | open |
| OI-131 | P6-F app | F-10 uses one roving focus stop with `aria-activedescendant`; tree rows are not individually focusable | Accepted kit substitute; revisit if screen readers need row focus | P6 | accepted-gap |

| OI-132 | P6-V app | R-19 is approximate (no 60 px centre decision or notice band; horizontal reveal is Rune’s); `--line` on a task-checkbox object paints nothing (OI-127 family) | P6-READ-B: 60 pt top/bottom bands, +30 pt while the section line shows, centre-vs-reveal decision from the owning block, one post-reveal probe, 40 pt horizontal scroller margin. Residual block proxy in OI-163; task-checkbox object paint remains OI-127. | P6 | fixed |
| OI-133 | P6-V app | Reader find does not search front-matter values, generated TOC labels, callout titles, math TeX, Mermaid source or raw-HTML text; matches can span styled runs; F-16 leaves focus in folder search | Keep as the documented surface; focus polish if wanted | P6 | accepted-gap |
| OI-134 | P6-PDF-T export | Below the 0.3 floor, break-anywhere inserts U+200B; inline HTML cells can over-shrink; images in below-floor tables may exceed their cell; nested insets are hard-coded; measuring adds a layout pass | Keep as documented; revisit if text-layer copy matters | P6 | accepted-gap |

| OI-135 | P6-T app | Table preview lacks V-12 zoom (+/−, Ctrl+wheel, 0.5–2) and V-17 cell link mapping/task glyphs; resize does not refit columns until reopen | Add zoom/refit and link-aware cells | P6 | open |
| OI-136 | P6-T app | Preview uses a purpose-built grid (kit Table cannot freeze a column); the opacity-0 bar still hit-tests over the header; widths are planned at open; text sort is not `Intl.Collator` (date ties mdy for en, NFD folding limited) | Documented substitutes; polish if needed | P6 | accepted-gap |
| OI-137 | P6-T i18n | Three table notices have no catalog keys; command titles remain English | P6-I18N-A keyed the three table notices and the remaining table command titles. | P6 | fixed |
| OI-138 | P6-T app | ReaderView passes the article viewport width to the renderer, so a table wider than the 760 pt column but narrower than the window overflows instead of scrolling | Pass the column width to renderer tables (pre-existing, exposed by P6-T) | P6 | open |

| OI-139 | P6-PDF-H export | `<details>` bodies after a blank line split at the HTML-block boundary; `<figure>` is not in the parser allowlist; span tables use equal 1fr columns and collapse per-column alignment | Parser/model limits; revisit with HTML subset work | P6 | accepted-gap |

| OI-140 | P6-L app | Lightbox gaps: V-08 copy/save PNG/SVG menu, V-09 rasterization, inline (text-span) images not wired, V-02 hint placement/off-pane hiding, native pinch, Mermaid laid out twice when the media wire is active | Wire inline media and add the copy/save/raster path in a later pass | P6 | open |

| OI-141 | P6-S app | Section line cannot see focus-visible elements under it (Rune has no per-frame focus query); table “about to stick” uses the block top, not the header height | P6-READ-B: poll-time `Tx::focused()`/`focus_visible()` tracking covers keyed focusable elements and ignores non-visible focus; the table uses its top band as the header because the exact header height is not measured. | P6 | open |
| OI-142 | P6-S app | O-10 Back/glide/heading flash are absent (no history yet); no floating tab hover card (T-16) or pointer-frozen width detection (T-35); word count includes TOC/front matter/footnote bodies and TOC duplicates | P6-READ-B: landing flash (static 1.2 s accent band; fade residual in OI-163) and an open-ended section-line hold released after the glide is still for 250 ms. Count edges remain. | P6 | fixed |

| OI-143 | P7-SEC parser | Inline math scan recomputes `paragraph_end` for every `$`: 139 KB / 10k formula lines takes ~17.5 s | Cached the paragraph boundary across the monotone inline scan: 14.61 s → **46 ms**, 20k lines 94 ms; all formulas still found. | P7 | fixed |
| OI-144 | P7-SEC parser | Duplicate heading slugging restarts the suffix counter per duplicate: 4,000 duplicates (~32 KB) ~3 s | Per-base next-suffix map for GitHub and legacy ids: 2.73 s → **21 ms**; 8k duplicates 47 ms; occupied suffixes still skipped. | P7 | fixed |
| OI-145 | P7-SEC parser | `SourceMap::source_range` can return an endpoint past `source.len()` for typographic quote expansion (`a'` → `a’`) | Endpoint clamps to the next segment or the inline's private source end; quote-span regressions added; production caller audited. | P7 | fixed |
| OI-146 | P7-SEC parser | `srcset` candidate URLs bypass `sanitize_url` (downstream resolution refuses them) | Every candidate now passes the same URL policy as `src`, preserving safe candidates/descriptors and `<picture><source>`; security corpus checks it. | P7 | fixed |

| OI-147 | P6-C palette | Middle-click and modifier-click on rows are impossible (Rune's press path drops the pointer button); selection restore is implemented but not runtime-tested | `El::on_middle_press` on file rows opens a background tab; focus return and page-selection restore have runtime tests. Headings/commands/tabs intentionally ignore middle press. | P6 | fixed |
| OI-148 | P6-C palette | No `aria-autocomplete`/`aria-controls` API; footer is a plain sentence not kbd-marked; start-mode recents depend on the real store; headings capped at 2,000; no full NFKD table; page step uses the previous frame height | P6-I18N-A: footer uses `Keybind` keycaps, heading cap raised to 20,000 and page step measures the list area. Rune still lacks `aria-autocomplete`/`aria-controls`; full NFKD and real-store recents remain. | P6 | open |
| OI-149 | P6-C palette | Command coverage is limited to the current command table; paste/copy path/reveal/pin/themes/reader size/reading settings/Back-Forward are not commands yet | P6-H added Back/Forward; P6-CMD added paste, copy path, reveal, pin, five themes, reading settings, help, next/previous tab and browser up | P6 | fixed |

| OI-150 | P6-H app | BrowserBack/Forward keys are not exposed by pinned Rune (patch needed); Alt+←/→ is consumed by focused text fields | Rune patch or accept Alt+arrows outside fields + mouse buttons | P6 | accepted-gap |
| OI-151 | P6-H app | 8 px rule uses the last frame’s heading tops; pasted places resolve only within the window; tab menu Back/Forward act on the active tab; session clones each tab’s 50-entry lists per frame | One-frame-stale edge, background-tab menu, and lazy publish optimization | P6 | open |

| OI-152 | P6-PREF app | Settings strings lack catalog keys; reset also clears `print_break_h1` (the earlier reset kept it); spellcheck row is disabled; editor font does not follow reader size (E-37); live locale is skipped under `cfg(test)` | P6-I18N-A keyed every settings string; reset now keeps `print_break_h1`; E-37 verified intact (0.875× live). Spellcheck engine and the test-only locale skip remain. | P6 | open |
| OI-153 | P7-PKG packaging | AppStream warns the `md.lmdv` id is not rDNS; no LICENSE file despite MIT; the AppImage and NSIS installer have not been through a signed end-to-end release; installer still has an old fallback icon | Pick a 3-part rDNS identity, add LICENSE, fix the installer icon, run end-to-end release packaging | P7 | open |

| OI-154 | P6-PREF app | Palette polish: native/floating frame colour stays OS-themed; Sepia hljs hexes are approximated by the derived syntax theme; neutral elevation shadows; `--active-line`/`--landing` have no app role | Accepted substitutes; revisit with a Rune frame patch | P6 | accepted-gap |

| OI-155 | P7 cross-platform | Windows is compile-checked, not runtime-supported: no single-instance routing, file identity falls back to mtime+len, no POSIX modes, `xdg-open` used for external actions | The user verified `cargo check --workspace --all-targets` on Windows, and CI repeats the compile check on `windows-latest` (`.github/workflows/ci.yml`). Runtime gaps stay open: add cmd/start, Windows identity and state ACLs if Windows becomes a runtime target | P7 | open |
| OI-156 | P6-V app | Smooth scrolling is not implemented: heading/outline/anchor jumps and restores use instant `scroll_to_y`/`scroll_key_to`; R-06's 400 ms target accumulation and R-09's `g g`/`G` have no key handlers, so there is no glide to suppress under A-15 | P6-READ-A: `j`/`k` accumulate from the target and use `Tx::key_repeat`; `g g`/`G`, `.`/`,`, Space paging and heading/anchor jumps use Rune's glide; reduced motion jumps; restores stay instant. Evidence `/tmp/opencode/p6read/`. | P6 | fixed |
| OI-157 | P7-PKG packaging | AppStream expects a project homepage; by decision none will be made or provided | Accepted gap (ADR-025): keep the `url-homepage-missing` warning; a channel that requires a homepage may not be available, while the release workflow can ship without one | P7 | accepted-gap |
| OI-158 | P6-CMD tabs | T-17 overflow tab list is not implemented: no ⌄ list, no left/right fade masks; the strip shrinks/scrolls instead | ⌄ all-tabs menu when the measured strip overflows (active ●, folder hint, New tab); kit fades/scroll kept. Scene `tab-overflow`. Residual wheel/one-frame behavior in OI-166. | P6 | fixed |
| OI-159 | P6-CMD tabs | Tab keyboard/placement gaps: Ctrl+PgDn/PgUp (T-08), Ctrl+1…9 (T-09), Ctrl+Shift+PgUp/PgDn move (T-10), and the tab menu's New tab appends instead of inserting after the tab (T-04) | Ctrl+PgDn/PgUp wrap, Ctrl+1–9 (physical digits), Ctrl+Shift+PgUp/PgDn move, and menu New tab inserts after the tab; Ctrl+T still appends. Residual keyCode fallback in OI-166. | P6 | fixed |
| OI-160 | P6-CMD i18n | New P6-CMD strings are English-only (`Path copied…`, the Wayland pin refusal, “window is not on screen yet”); pasted-title parsing is a documented subset; `file.reveal` still says “Show in File Explorer” on Linux | P6-I18N-A keyed the P6-CMD strings and changed `file.reveal` to “Show in folder”; P6-I18N-B added setext headings and the common inline forms to the pasted-title parser. | P6 | fixed |
| OI-161 | P6-READ-A app | Space over a focused link or live control activates that control instead of paging the article (R-07); PageDown still pages and a focused article block pages | Decision needed: keep the accessibility-first focus behavior or match the web scroller; document the chosen substitute | P6 | open |
| OI-162 | P6-READ-A app | Footnote `fn:`/`fnref:` jumps remain instant; heading, outline and `#anchor` jumps glide | Route footnote jumps through the same measured glide when their target is on screen | P6 | open |
| OI-163 | P6-READ-B app/render | R-19 uses the owning block as the hit proxy (a large block spanning a band can centre a mid-screen hit; horizontal reveal is a 40 pt margin, not exact centring), the landing flash is a static 1.2 s band with no fade, and the outline keeps the kit list wrapper under its button role | Accepted visual/geometry substitutes unless exact parity is wanted; revisit with a highlight-box geometry wire and a Rune color animation | P6 | open |
| OI-164 | P7-RUNE app | The `cca5df6` bump changes exact text origins, 1/64 pt metrics and a11y leaf names; targeted tests and one inspected `reader` scene passed, but the full scene/tree corpus, X11/Wayland wheel distance, middle-click and tear-out behavior were not re-baselined | User verified the post-bump visual/manual checks; marked complete. Keep the scene corpus in the release gate. | P7 | fixed |
| OI-165 | P6-I18N-B help | H-04's license and repository links are not exposed in the full Help dialog (no catalog URLs); the Store rate card is Windows-only | Add an About/links section only if a public repository or license URL exists; ADR-025 rules out a project homepage | P6 | open |
| OI-166 | P6-TAB app | T-17's plain vertical wheel over the horizontal strip cannot be translated to horizontal without the kit's live x offset; the ⌄ appears one frame after overflow and does not close on resize; T-09 has no `keyCode` fallback for backends reporting neither a Command digit nor a physical key | Rune kit/backend limits; accept or add a kit API later. Fades, horizontal/Shift wheel, physical digits and clamped moves are implemented. | P6 | open |
| OI-167 | P6-READ app | A tab switch, a cached/recent open or a closed-tab return inherits the previous document's scroll offset: the article viewport is keyed only by `article-scroll`, so a short/medium page opens at the nearest heading above and its progress hairline starts part way; a nonzero inherited report also cancelled a pending heading restore | `begin_restore` queues the top too and `apply_pending_scroll` moves the viewport there; only Rune's hand report (wheel/drag/key/instant), not the inherited layout report, cancels a heading restore. Recents, error retry, Open, links, browser, tab strip/keys and reopen/close flush the restore in the same transaction. Regression tests for a short page after 600 pt, a second view, a remembered heading and a top restore. | P6 | fixed |
| OI-168 | P6-READ app/io | The document watcher counted the app's own read as a change: Linux reports a load as `Access(Open(Any))`, `touched` accepted every event kind, and every load/reload emitted `Changed`, reloaded the file and re-applied the tab anchor. Scrolling up from “Known packaging findings” snapped back to the bottom every few seconds; the outline and progress hairline flickered with each restore | `DocWatcher` ignores all `Access` events (open/read/close); content changes still arrive as `Create`/`Modify`/`Remove` (a write's access events accompany its `Modify`). Tests: a live watcher reads the watched document and sees no event, then writes it and sees `Changed`; a pure test covers read/open/close versus a data write. Root cause confirmed in `/tmp/opencode/live-race/trace.log`; after the fix a wheel-up stays put in `/tmp/opencode/live-fixed/up6s.png`. | P6 | fixed |

## 2. Resolved actions (examples)

These came directly from worker “known limitations” and were acted on:

| Item | Action | Commit |
|---|---|---|
| Patch 0001 image placed at `span.start` instead of the first placeholder; phantom box for placeholder-less spans | Reworked and regenerated the patch; added two tests | `f6c1287` |
| Rune single-instance forwards links but not paths/cwd | Own socket wired into `lmdv-app`; second launch opens a tab with `--line`; headless tooling skips the claim | `9206c8f` |
| `StorePatch::Draft` could not key a draft without a key | Struct variant `Draft { key, draft }` accepted and documented | `09e0377`, `docs/ARCHITECTURE.md` §11.1 |
| Temporary `rune-kit` reader violates ADR-003 if left | ADR-024 records it; P2 replaces the `ReaderView` boundary | `187d034` |
| Math cache clones RGBA per hit | App-side `Pixels`/`InlineImage` cache scheduled as OI-020 | — |
| IO instance protocol/CLI/envelope differ from the sketch | Documented in `docs/ARCHITECTURE.md` §11.1 and tests | `09e0377` |
| i18n lockfile adds `serde_json` and drops stale `lmdv-md` deps | Inspected and committed | `722a9db` |
| Parser added model variants/fields (`Toc`, `anchor`, `first_ref`, objects) | Accepted as additive; passed to the renderer worker as the frozen model | `8bd75cd` |

## 3. How this register is maintained

- **On every worker report:** the orchestrator reads the “Known limitations / follow-ups” section, validates each claim, and adds any unhandled item here before committing the work.
- **At every phase gate:** print/filter this table. No item scheduled for the phase may remain `open`; `accepted` items are re-checked with the user.
- **When fixing:** change `Status` to `fixed` and link the commit/test; when accepting, write the reason and the parity ID.
- **Do not delete rows.** A fixed/accepted item stays for history; a changed decision gets a new row or an ADR.
- **User impact:** any accepted gap that affects P0/P1 needs the user's explicit approval before the phase closes.

## 4. Current count

- **Open (84):** OI-001, 004, 005, 007, 010, 012, 024, 025, 026, 033, 035, 037, 041, 042, 044, 048, 049, 051, 054, 055, 058, 059, 060, 061, 066, 068, 071, 072, 073, 074, 076, 078, 079, 080, 082, 083, 088, 093, 094, 095, 096, 097, 098, 100, 101, 102, 103, 104, 105, 106, 109, 111, 112, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 132, 135, 137, 138, 140, 141, 142, 143, 144, 145, 146, 148, 149, 151, 152, 153, 155
- **Scheduled (6):** OI-003, 017, 023, 027, 032, 050
- **Accepted (23):** OI-002, 022, 029, 034, 038, 039, 040, 045, 046, 062, 063, 065, 070, 107, 108, 131, 133, 134, 136, 139, 147, 150, 154
- **Fixed:** 42 rows, kept for history.

P3, P4 (with substitutes) and P5 are closed; P6 polish and P7 audit remain. Accepted gaps are re-checked at the next gate.
