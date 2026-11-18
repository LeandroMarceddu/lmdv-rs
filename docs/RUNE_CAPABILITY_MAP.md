# Rune capability map for lmdv (native Markdown viewer)

**Audited source:** Rune rev `d6d04d3e64180ba34ed4a976bb5fec9ff6769ad0` (2026-11-12),
`rust-toolchain.toml` = 1.99.1. **Current vendor pin:** `cca5df6fafb4721d96fe105f126ebfd2ec351bb6`
(2026-11-17). `docs/RUNE_PIN.md` has the pin and vendor instructions. This file is a research
snapshot from the audited rev; re-check rows touched by the bump (text metrics, focus, a11y, wheel)
before relying on them. `docs/ARCHITECTURE.md` and `docs/DECISIONS.md` are the current design.

**Method.** Static reading of the checkout: every claim below was read in source, not taken from README prose.
No build was run, so type-level details (generic bounds, exact `impl` availability) are as-written but not
compile-verified. RaTeX API facts come from docs.rs for `ratex-*` 0.1.15 (linked inline). Where a README claim
disagrees with code, the code wins and the mismatch is listed in §13.

**Effort scale** (for one experienced Rust engineer): **S** ≤ 1 day · **M** 2–5 days · **L** 1–3 weeks.
**Risk** is delivery risk given Rune is pre-1.0 (`rune`/`rune-kit` version `0.0.1`) and pinned: high = likely to
need framework patches and re-application on every rev bump; medium = contained patch or app-side work; low = public API.

**Top-line verdict**

| Area | Verdict | Effort | Risk |
|---|---|---|---|
| Markdown view | Strong, production-grade for a viewer; streaming is best-in-class | S to adopt | low |
| Markdown spec gaps (footnotes, front matter, callouts, HTML, anchors, sticky tables, inline images) | Real gaps; some are parser changes | S–L each | medium |
| Editing | Solid `TextEdit` + IME + undo + rich marks; **no** multi-cursor, find/replace, Linux spellcheck, gutter; typing cost grows with document size | M–L | medium–high |
| Math | **Nothing**; RaTeX can be embedded, but inline math needs a Rune patch for baseline/aspect | M (block) / L (inline) | high |
| Mermaid | Far broader than the README says; no zoom/pan or export built in | S–M | low–medium |
| Shell / windows | Tab strip with reorder + tear-out + cross-window handoff; no custom titlebar on Linux | S–M | medium |
| Platform | Fonts/fallback/a11y/i18n/prefs/clipboard good; drag-out absent on Linux; always-on-top no-op on Wayland | S–L | medium–high |
| Headless tooling | Excellent: snap/tree/script/trace/html + virtual clock | S | low |
| Export | PNG via offscreen; static HTML via CLI only; **no PDF/SVG/print** | S (PNG), L (PDF/SVG) | high for parity |

---

## 1. Markdown parsing and rendering

Primary API: `rune_kit::markdown` (feature `markdown`) — `Markdown`, `MarkdownStyle`, `Block`, `Inline`,
`Table`, `parse`, `find`, `Case`, `Highlight`, `parse_cached` (`crates/rune-kit/src/markdown.rs:52-81`).
Feature wiring: `markdown = ["dep:pulldown-cmark"]` (`crates/rune-kit/Cargo.toml:19-21`), `mermaid` implies it
(`:22-23`).

### 1.1 Parser: exactly three GFM extensions

```rust
// crates/rune-kit/src/markdown/parser.rs:466
let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
```

Nothing else is enabled. Concretely:

| Feature | State | Evidence |
|---|---|---|
| CommonMark core (headings, setext, emphasis, lists, quotes, rules, indented/fenced code, links, reference links) | Yes | `Parser::new_ext` at `parser.rs:466`; block walk `parser.rs:471-668` |
| GFM tables | Yes, with per-column alignment | `parser.rs:544-556`; `Table` at `markdown.rs:134-143` |
| GFM task lists | Yes, interactive | `parser.rs:649-658`; `render.rs:541-590` |
| GFM strikethrough | Yes | `Event::Strikethrough` arm `parser.rs:570`; drawn with `El::strike` `render.rs:333-336` |
| GFM autolinks | **Partial, custom**: bare `https://`, `http://`, `www.` in plain text; bare e-mails are *not* linked, `<mail@x>` is | `markdown/autolink.rs:1-5`, `markdown.rs:250-258` |
| GFM footnotes | **No** | not in `Options` (`parser.rs:466`) |
| GFM tagfilter | n/a — no HTML is rendered | see below |
| GitHub alerts / callouts `> [!NOTE]` | **Not in `Markdown`**; only in the separate `rune_kit::notes::Note` wrapper (§1.4) | `crates/rune-kit/src/notes.rs:1-6, 133-167` |
| Front matter (YAML/TOML `---`) | **No**: parsed as content (the `---` lines become thematic breaks, the YAML a paragraph) | no metadata option at `parser.rs:466`; the test document with `--- title: … ---` is only used to compare stream vs whole parses (`tests.rs:207`) |
| Footnotes/definition lists/smart punctuation/sub/superscript/math | **No** | `parser.rs:466` |
| Raw HTML / inline HTML | **Dropped entirely**: no `Event::Html`/`InlineHtml` arm, so both hit the catch-all `_ => {}` | `parser.rs:667`; start tags still set a stream tail (`parser.rs:472,488`) |
| Images | **Block-level only** (see §1.5) | `parser.rs:317-347`, `markdown.rs:126-129` |
| Heading anchors/ids/slugs | **None** | headings only get `role("heading")` + `level` (`render.rs:597`) |
| TOC | **None built in**, but `parse()` is public (`parser.rs:440`) so the app can build one from `Block::Heading` |

The parser's public output is stable and small: `Block::{Para, Heading(u8, Inline), Code, List, Quote, Rule,
Table, Diff, Mermaid, Image}` (`markdown.rs:113-132`). `Inline` carries `text`, byte-range `spans`
(`Style::{Code,CodePad,Strong,Em,Strike,Link}`) and `links` (`markdown.rs:91-110`). That makes an app-side
outline, front-matter strip or stats pass easy: call `parse_cached(src)` (`markdown/cache.rs:48-51`) instead of
re-parsing.

**Front-matter workaround (S):** split the file before rendering (`src.strip_prefix("---")…` or a proper
`serde_yaml`/`toml` pass), keep the metadata in the document model, render only the body. This is also what
`--html` and print/export would need.

### 1.2 Rendering model, cost, streaming, keys

- `Markdown::new(src)` → builder → `.build(&Kit)` or `.render(&MarkdownStyle)` (`markdown.rs:222-240, 421-449`).
- **Streaming is a first-class design.** `streaming(true)` (`markdown.rs:286-292`) parses a partial document by
  patching only the tail (`markdown/stream.rs:1-47`), and the parse cache grows a stream from its last
  top-level block instead of re-parsing (`markdown/cache.rs:55-90`). Docs: `markdown.rs:14-27`.
- **Per-block identity:** top-level blocks are keyed `b{index}` by default, or app keys through
  `block_keys([(source_byte_offset, key)])` (`markdown.rs:259-274`, application at `render.rs:405-424`).
  List items are `i{index}`, their inner blocks `b{index}`. This is what keeps a streamed reply from
  remounting. **The app must recompute byte offsets after edits** (`markdown.rs:260-266`).
- **Re-render cost:** `render()` memoises the whole element under a digest of source/style/keys/highlights
  when no callbacks are set (`markdown.rs:427-445`); parse cache is 16 MiB (`cache.rs:6-7`) and highlights,
  table widths and diagram layouts each have their own caches (`syntax.rs:271-334`, `table.rs:162-166`,
  `rune-mermaid/src/scenes.rs:1-46`).
- Word-by-word reveal for incoming text: `reveal(seconds)` (`markdown.rs:276-282`), `El::reveal`
  (`crates/rune/src/el/text.rs:372-378`).
- Bidi: per-block direction with `dir_tentative` (`markdown.rs:346-356`; `markdown/bidi.rs`).

### 1.3 Which GFM and app-specific constructs need work

| Need | Rune today | Recommendation | Effort | Risk |
|---|---|---|---|---|
| Footnotes | absent | Preprocess source: extract `[^id]:` definitions, append a numbered section, map references to links. Keeps `Markdown` untouched but loses exact footnote placement at the reference site. Better: vendor the parser and handle `Tag::FootnoteReference`/`TagEnd::FootnoteDefinition` into a new `Block::Footnote` | M | medium |
| Front matter | absent | Strip before render; keep in model (also used for title/sidebar) | S | low |
| Callout types (NOTE/TIP/WARNING/CAUTION/IMPORTANT) | `notes::Note` renders every `> [!x]` with one info-blue style, uppercased label, no per-kind icon/colour (`notes.rs:547-565`) | Fork/extend `notes.rs` for per-kind paint (it already parses the label, `notes.rs:133-167`); or pre-split and draw your own card, then feed each body to `Markdown` | S–M | low |
| Raw HTML subset | dropped, including `<img>`, `<br>`, `<details>`, `<sub>` | No HTML layout exists in Rune. Options: (a) post-process source into Markdown equivalents (`<br>`→two spaces/newline, `<img>`→`![]()`, `<sub>`/`<sup>`→Unicode where possible); (b) vendor the parser and map a whitelist to elements/fonts (no general HTML/CSS); (c) show HTML as a code block. **Full HTML is not achievable** without an HTML/CSS engine | M–L | medium |
| Heading anchors / TOC / deep links | none | Build a TOC from `parse()`; jump with `Tx::scroll_into_view(block_key)` (`crates/rune/src/el/tx/scroll.rs:18-27`) or `Runtime::find(key)`; assign stable keys with `block_keys`. For HTML export, keys starting with a letter become `id=` (`runtime/web/export.rs:117, 124-129`) | S | low |
| Sticky table header while the page scrolls | Markdown table is a single element; its header is a child of the frame, so `El::sticky_top` cannot reach it (`table.rs:247-368`) | Fork `markdown/table.rs` to emit the header as a sibling that is `sticky_top(0)` inside the page viewport; or leave it (kit `Table` is sticky but is a data grid, not Markdown) | M | medium |
| Inline images | an image in a paragraph is **lifted to its own block**, splitting the paragraph; inside a link/emphasis it becomes alt text (`parser.rs:317-347`, `markdown.rs:126-129`) | Acceptable for a reader (remark/GFM sites style `img{display:block}`). For true inline images, an app-side inline image is only possible via `El::inline_icon` (§3.2) or a Rune patch | S to accept / L to change | low |
| Table styling | Very rich: outline/frame/body card, row rules, header fill/text/weight/rule, cell padding/font, tabular figures, flush-left, full width, min column, wide scroll with a custom bar, `table_like(&TableStyle)` to match kit tables, `table_card(&Kit)` | Use `MarkdownStyle` builders; no work needed | S | low |
| Theming | `MarkdownStyle` (~60 fields) from `Kit` (`style.rs:167-268`) or `neutral(dark)` (`:270`), every part overridable: text/muted/link/code/quote/rule/headings/code-block/shell/diff/syntax/selection/highlight/task/diagram/table | Use kit tokens; a serif reading face is `Family::Named(...)` with bundled bytes | S | low |

**Selection/copy** is unusually complete: `selectable(true)` makes the entire document one selection group across
paragraphs, list items, quotes, code and table cells (`markdown.rs:319-333`, `render.rs:331-370`); Cmd+A selects
all, Cmd+C copies "as drawn" with list markers, a tab between cells, no fences/`**` (`render.rs:67-90, 367-368`;
`crates/rune/src/runtime/selection/copy_join.rs:1-58`);
`copy_source(true)` copies the Markdown source by whole blocks instead (`markdown.rs:335-345`,
`render.rs:124-136`). Links stay clickable inside a selection. `El::selection_copy` allows an app override
(`crates/rune/src/el/text.rs:503-510`).

**Find/highlight** is purpose-built for a viewer: `markdown::find(src, query, Case)` returns source UTF-8 byte
ranges per block, crossing inline markers but not block boundaries (`markdown/find.rs:30-56`,
`find.rs:38-46`); `Case::Folded` does accent/case folding (`find.rs:10-30`, `rune-kit/src/search.rs:1-56`);
`Markdown::highlights(ranges)` paints backgrounds/rings **without reshaping or reflowing** and gives each match
a key for `Tx::scroll_into_view` (`markdown.rs:229-252`, `markdown/highlight.rs:1-83`). This is exactly a find
bar's engine.

### 1.4 Callouts via `rune_kit::notes` (separate from `Markdown`)

`Note::new(src).build(&kit)` / `render_note(key, src, &kit)` splits a document into plain-Markdown chunks and
structured pieces: `> [!note]` callouts, ` ```facts `, ` ```compare `, ` ```steps `, ` ```size `, and code
fences with `title=`/`{1,3-4}` line marks (`crates/rune-kit/src/notes.rs:1-17, 72-190`). Callout bodies are
Markdown. Only "note" is special-cased (label omitted); every other label renders identically
(`notes.rs:547-565`). Feature-gated on `markdown` (`crates/rune-kit/src/lib.rs:36-37, 42-43`).

**Cost/limit:** `Note` splits on top-level structure with a hand-written line scanner (`notes.rs:104-191`);
it does not nest inside lists/quotes. It is a substitute for GitHub alerts, not full parity.

### 1.5 Images

- `Markdown::images(|src| Option<Image>)` supplies decoded pictures by source string (`markdown/image.rs:36-45`).
- Draw is `w_full().max_w(pixel_width).pixels(...)`, radius 4, 24 pt margins, `Cursor::ZoomIn`, optional
  `.help(title)` (`render.rs:708`, `image.rs:56-74`).
- No image means Chromium's broken-image glyph + alt text (`image.rs:15-33, 76-86`) — a deliberate browser match.
- No fetch, no cache ownership: the app decodes (`ImageCache::file/svg/load/load_mime`, `image.rs:196-245`) and
  wakes the view.
- `image_frame(|el, &ImageFrame| …)` wraps the picture for click-to-zoom/keying (`image.rs:47-54`).
- **Math and diagrams cannot ride this path**: raw-pixel images have no `source_url`, so `--html` cannot emit
  them as `<img>` (`image/pixels.rs:74-82`); PNG export is possible (§8).

---

## 2. Editing

Primary types: `rune::TextEdit` (`crates/rune/src/edit.rs:38-86`) and `rune::text_input(&edit, style)`
(`crates/rune/src/el/input.rs:57-101`); richer field builders in `rune_kit::TextInput` / `TextArea`
(`crates/rune-kit/src/text_input.rs:1-140`).

### 2.1 What exists

| Capability | Detail | Evidence |
|---|---|---|
| Model | `text: String`, `anchor`/`focus` byte offsets, `focused`; marks/blocks private with accessors | `edit.rs:38-56, 333-357` |
| Construction | `TextEdit::new`, `from_parts(text, marks, blocks)`, `from_html`, `replace_text` (wipes history), `insert`, `set_caret`, `set_selection` | `edit.rs:173, 205, 305, 470, 650, 535, 548` |
| IME | preedit + clauses + cursor, `finish_composition`, `cancel_composition`; composition is never part of `text` until committed | `edit.rs:407-468`; `edit/ime.rs` |
| IME for custom editors | draw a caret with `.ime_cursor()`, receive `App::on_ime` | `el/layout.rs:612-621`; `app.rs:319-327` |
| Undo/redo | Cmd/Ctrl-Z, Shift-Z; typing runs coalesced; `undo_group`, `clear_history`; **128 steps / 1 MiB**, steps store full text before+after | `edit/history.rs:1-9, 240-310`; README `crates/rune/README.md:1289-1296` |
| Selection | **One** anchor/focus pair only — no multi-cursor support anywhere | `edit.rs:38-42` (fields), no other caret API |
| Rich marks | `Mark::{Bold, Italic, Underline, Strike, Code, Link(url), Pill(id)}` as sorted byte ranges | `edit/model.rs:14-40` |
| Blocks | `Block::{Paragraph, Bullet(0..=3), Numbered, Quote, Code}` per `\n` line; soft breaks (`<br>`) | `edit/model.rs:98-128`; `edit.rs:386-401` |
| Rich editing opt-in | `.rich()` enables Cmd-B/I/U, Shift+Return soft break, rich paste; `allow_marks/allow_blocks` restrict to what the storage supports (Markdown has no underline) | `edit.rs:220-270` |
| Per-frame decoration overlay | `.spans(spans)` / `.decorations(spans)` paint arbitrary `rune::Span` byte ranges over the editor without mutating `TextEdit`; offsets shift through IME preedit and undo | `el/text.rs:69-89`; kit forwarders `text_input.rs:43-67` |
| Find/highlight in editor | `Span`/`TextHighlight` paint only; no find/replace engine in `TextEdit` | `text/span.rs:1-64, 283-330`; `markdown/find.rs` is Markdown-only |
| Clipboard | plain, HTML, RTF, image; focused rich field copies `selected_html()` beside text; `on_input_paste` sees all forms | `clipboard/tx.rs:19-70`; `clipboard/content.rs:25-160`; `el/input.rs:206-211` |
| Spellcheck | Windows `ISpellChecker`, macOS `NSSpellChecker`; **Linux is a stub returning no errors** | `crates/rune/src/spell.rs:1-3, 24-33` |
| Secure fields | bullets, no undo/copy/IME, zeroised buffers, optional reveal | `el/input.rs:144-187`; `edit/secure.rs` |
| Auto-grow | `auto_grow(field, min_lines, max_lines)` inside a scroll viewport | `el/input.rs:103-112` |
| Pills | atomic inline chips (`@file`, mentions) with icons/colour, caret steps over, undo-safe | `edit.rs:558-643`; `el/pills.rs:1-52` |

### 2.2 Gaps and their exact workarounds

| Gap | Workaround | Effort | Risk |
|---|---|---|---|
| **No multi-cursor** | Implement in the app: keep N `(anchor,focus)` and merge edits yourself — but `TextEdit` exposes no way to render secondary carets, so you must draw them as absolutely-positioned `el()` carets from `Cx::caret`-style geometry (that API is for the focused field, `runtime/cx.rs:295`) or patch `TextEdit`/paint | L | high |
| **No find/replace in `TextEdit`** | Use `rune_kit::search::Fold::find` (`search.rs:21-56`) or plain `str::match_indices` over `edit.text`; render matches with `.spans(..)`; replace with `edit.set_selection(a, b); edit.insert(text)` under a single `undo_group`; add next/prev by caret. (`TextEdit` has no public range-replace; only `insert`/`replace_text`/`replace_with_pill`.) | S–M | low |
| **No Linux spellcheck** | Bring your own engine (`spellbook`/Hunspell via `nuspell` FFI, or a pure-Rust checker) and paint squiggles with `.spans(..)`; wire suggestions into your own context menu. There is no hook to Rune's checker on Linux (`spell.rs:24-33`) | M | medium |
| **No gutter/line numbers** | Build a left column of numbers in a row beside the field. With one `TextEdit` per paragraph, `TextEdit::paragraphs()`/`line_range` (`edit.rs:386-401, 655-672`) give a hard-line gutter directly. Wrapped-line numbering needs your own layout pass: `TextSystem::default()` (`text.rs:256`) + `layout_rich(text, &[], style, scale, Some(width))` (`rich_layout.rs:62-96`) and iterate Parley `Layout::lines()`; it is extra shaping per frame and must match the field's width/scale | M | medium |
| **Typing cost scales with document size** | See §2.3. Use one field per block/paragraph (`long_note --blocks`), or patch `TextSystem::layout_blocks` to cache per paragraph | M–L | high (patch) |
| **Undo depth on long documents** | 128 steps / 1 MiB, each step a full snapshot (`history.rs:8-9, 30-35`). For a 200 KB note only ~2 steps survive (research/27 table). Patch history to store deltas, or keep one `TextEdit` per block so each has its own history | M | medium |
| **No inline math/objects other than pills** | See §3 | L | high |
| **Line numbers/selection are app-side** | `El::selectable` + `selection_group` and `TextSelection` let you build a code-view-like selection for read-only text; for editable text, `TextEdit` already owns it | S | low |

### 2.3 Measured performance (the hard limit)

`crates/rune/examples/long_note.rs` + `research/27-long-note-editing.md:1-120` are the authoritative numbers:

| Case (median ms, M4 Pro) | 5 KB | 50 KB | 200 KB |
|---|---|---|---|
| Type one character, one field | 2.2 | **12.0** | **45.7** |
| Return / Backspace / paste 2 KB | ~2.0–2.4 | ~12.0–12.5 | ~46–48 |
| Scroll frame | 0.10 | 0.10 | 0.09 |
| Down arrow | 0.12 | 0.19 | 0.41 |
| Type one character, **one field per block** | 0.31 | **0.82** | **2.6** |
| Heap after 200 typed chars, MB | 10.5 | 18.8 | 45.6 |

Cause: `TextSystem::layout_blocks` caches under the field's **whole text**, so any edit reshapes the whole string
(~0.22 ms/KB) (`research/27:34-44`). Memory ~90 B heap per byte of text while two layouts are alive.
**Design implication for lmdv:** never put a whole 50 KB+ document in one `TextEdit`. Use block-per-field (with
`El::virtualize` for very long documents — `crates/rune/src/el/virtual_rows.rs:24-60`), or budget 12–46 ms per
keystroke.

### 2.4 Reading vs editing: choose one of two architectures

1. **Preview renderer for view mode** (`Markdown`) + **block editor** for edit mode (one `TextEdit` per block,
   styled with `SyntaxTheme` spans and `Block` attributes). Cheapest, fastest, and matches the specification's
   "viewer first, edit is explicit".
2. **Custom single-canvas editor** on `TextSystem` + `Shape`/glyph runs, reusing `TextEdit` as the model only.
   Maximum control (gutter, multi-cursor, inline math) but re-implements caret/selection/IME hit-testing;
   Rune's own paragraph-layout refactor is described as "a large change" (`research/27:106-113`). Not recommended
   before parity pressure demands it.

---

## 3. Math

### 3.1 What Rune has: nothing

A repo-wide search for `latex`, `katex`, `mathml`, `\btex\b`, `$$` in `crates/` finds only language aliases
(`rune-kit/src/syntax/paths.rs:39,63,86,88` — `.tex` highlighting) and shader variable names. There is:

- no math parser, no math font metrics, no MathML;
- no inline-element primitive in text other than pills/inline icons (`text/span.rs:1-64`, `el/pills.rs:78-118`);
- no public text-baseline measurement API for arbitrary boxes.

`$x$` and `$$x$$` render as ordinary text/paragraphs.

### 3.2 What can be embedded, exactly

RaTeX 0.1.15 (MIT, pure Rust, KaTeX-compatible) exposes precisely what a native app needs:

- `ratex_parser` → AST; `ratex_layout::layout(...) -> LayoutBox { width, height, depth, … }` in **em units**
  (ascent/descent, so baseline alignment is computable) —
  <https://docs.rs/ratex-layout/latest/ratex_layout/layout_box/struct.LayoutBox.html>.
- `ratex_layout::to_display_list(...)` → `ratex_types::display_item::DisplayList`, whose items are
  `GlyphPath { x,y,scale,font,char_code,color }`, `Line`, `Rect`, `Path { commands, fill, color }`
  (<https://docs.rs/ratex-types/latest/ratex_types/display_item/enum.DisplayItem.html>).
- `ratex_svg::render_to_svg(&DisplayList, &SvgOptions) -> String`; SVG coordinates are em units, y down,
  **baseline at y = height**, scaled by `SvgOptions::font_size` + padding; feature `standalone` + `embed_glyphs`
  (`embed-fonts`) yields self-contained SVG with KaTeX glyphs
  (<https://docs.rs/ratex-svg/latest/ratex_svg/>).
- `ratex_render::render_to_png(&DisplayList, &RenderOptions) -> Vec<u8>`
  (<https://docs.rs/ratex-render/latest/ratex_render/>).

Two ways into Rune, and the second is the better one:

1. **`ImageCache::svg(bytes, size_pt, scale) -> Image`** (feature `svg`, `crates/rune/src/image.rs:196-206`).
   Decoding/rasterisation happens on Rune's image worker, so the first frames draw nothing until the `Image` is
   ready; the app must keep its own `ImageCache` (`ImageCache::new(waker)`, `image.rs:160-180`, waker from
   `AppSpec::setup`) and handle `Image::is_ready`/`error`.
2. **Synchronous `Pixels`**: rasterise with `ratex_render::render_to_png` (or your own `resvg`) in the app, then
   `rune::Pixels::new()` + `.set(w, h, rgba)` (`crates/rune/src/image/pixels.rs:50, 90-100`) and draw with
   `el().pixels(&pixels)`. Ready the frame it is made, any size/aspect, theme colour baked in, and the app owns
   the LRU. This avoids the async image path entirely and is what lmdv should use for math.

### 3.3 Three integration levels

| Level | How | Quality | Effort | Risk |
|---|---|---|---|---|
| **Block math (display)** | `ratex_layout::layout` → `to_display_list` → `render_to_svg` (embed fonts) → `ImageCache::svg(bytes, em_px, scale)` → `el().pixels(&img)` with `margin`/`max_w`. Height from `LayoutBox::total_height() * size`, depth from `depth`. Cache SVG/raster per (tex, size, theme colour). | Good; vector-quality raster at 2×, themed by choosing colour per formula | **M** | low–medium |
| **Inline math, patched inline box** | Patch Rune's inline box to carry `(width, height, ascent)` instead of a square: `text/pill_chips.rs:6-40` (square `px`), `runtime/paint/content.rs:452-460` ("An image fits the square"), `text.rs:604-620` (lead width). Then `BareIcon`/`InlineIcon::image` places a non-square bitmap on the baseline. | Good; baseline exact from RaTeX metrics | **M** patch + M app | **high** (three files, rev re-apply) |
| **Inline math, no patch** | `El::inline_icon(at, InlineIcon::image(pixels).size(h))`: the box is **square** and the image is fitted inside it, centred (`pill_chips.rs:30`, `paint/content.rs:452-460`), so a 4:1 formula at `size=16` draws 16×4 with the line 16 tall and the formula vertically centred on the word — legible but small and not baseline-exact. Setting `size=width` makes the line 4× too tall. | Poor for wide formulas | S | low |
| **Native vector, no raster** | Convert `DisplayItem::{Line,Rect,Path}` to `Shape::fill_path/stroke_path` (`crates/rune/src/shape.rs:57-130, 251-330`) and `GlyphPath` outlines via `ratex-font` + `ab_glyph` outline curves. Draw math inside a custom paragraph layout, not `text_input`. | Best (crisp at any zoom, themeable) | L | high |

**Recommendation:** build an `lmdv-math` layer that owns `(tex → DisplayList → SVG string → Image)` with an LRU
keyed by `(tex, size_bits, color, scale)`; use block images immediately. For inline, prototype level 2 in a small
Rune fork (keep the patch as a `.patch` in `vendor/rune/patches/` and re-apply on rev bumps). Do **not** start by patching
text layout for inline math; prove parity need first with level 3's square inline icon.

**Fonts:** RaTeX embeds KaTeX metrics/fonts through `ratex-katex-fonts`; Rune's own fonts are irrelevant to math
glyph shapes, but the *size* must match the surrounding text (pass the resolved point size).

**Baseline measurement for surrounding text:** `rune::text::measure`, `advance`, `content_widths`,
`TextSystem::layout_rich` and `Rune::Cx::layout` are public (`text/measure.rs`, `text/rich_layout.rs:62`,
`runtime/layout_read.rs:239-290`). A custom line builder can therefore mix text and formula images with exact
ascent/descent.

---

## 4. Mermaid

### 4.1 Supported set (much larger than the README claims)

`crates/rune-mermaid/src/lib.rs` parses and draws **17 diagram families** (`Diagram` enum, `lib.rs:286-305`;
`KINDS`, `lib.rs:326-334`):

flowchart (`graph`/`flowchart`, all directions, all v11 `@{shape:…}` shapes, subgraphs, classDef/style/linkStyle,
edge ids), sequence (participants/actors/boxes/rect bands/activations/notes/frames/autonumber/create/destroy),
state (`stateDiagram[-v2]`, composite, `--` regions, choice/fork/join, notes), class (members, stereotypes,
namespaces, UML relations), ER (attribute tables, crow's foot, aliases), gantt (date/axis formats, sections,
durations, `after`/`until`, excludes), pie (`showData`), mindmap (shapes, icons read and dropped), timeline,
journey, gitGraph (branches/merges/cherry-pick/tags/types), quadrant, xychart-beta (bars/lines, horizontal),
sankey-beta, block-beta (columns/spans/groups/arrows), requirementDiagram, C4 (all five kinds, nested boundaries,
Rel/ BiRel/RelIndex).

Leniency is explicit: unknown statements are skipped, unclosed blocks close at the end, unknown diagram types fall
back to source (`lib.rs:61-65`). Hard caps: **400 items**, nesting **8** (`lib.rs:312-315`), and an unknown type,
unreadable source or too-large diagram returns `None` (`lib.rs:63-66`).

The kit README's "flowcharts, sequence and state diagrams" (`crates/rune-kit/README.md:331-333`) is stale.

### 4.2 API

| Function | Result | Evidence |
|---|---|---|
| `rune_mermaid::render(src, &DiagramStyle)` | `Option<El>` card | `lib.rs:219-222` |
| `render_with_hover(src, &style, Hover, color)` | hover highlights nodes/edges | `lib.rs:226-229` |
| `render_streaming` / `render_streamed` | growing / settle-in-one-glide | `lib.rs:234-246` |
| `render_drawing(src, &style, Hover, color) -> Option<Drawing>` | `Drawing { el, size:[f32;2], surface, surface_border }` for a lightbox that frames/scales itself | `lib.rs:248-266` |
| `supported(src)` / `may_draw(src)` | parse check / "header names a type we draw" | `lib.rs:282-284, 320-324` |
| `rune_kit::mermaid::render(src, &MarkdownStyle)` | kit-themed card | `crates/rune-kit/src/mermaid.rs:29-33` |
| `rune_kit::mermaid::drawing(src, &style, max_width)` | `Drawing`, laid out for a viewer width, with `role("figure")` + label + description = source | `mermaid.rs:37-50` |

In Markdown, ` ```mermaid ` is detected by the parser (`parser.rs:520-528`), drawn by `Renderer::mermaid`
(`render.rs:513-538`), streamed per complete line, and wrapped by `diagram_frame` (`markdown.rs:365-377`).
Themes: `MarkdownStyle::diagram(node_fill, node_border, line)` and `diagram_surface(fill, border, subgraph)`
(`style.rs:486-500`), plus `DiagramStyle` for standalone use (`lib.rs:149-215`). Layout is cached (16 scenes,
keyed by source+style+hover; `scenes.rs:1-46`).

### 4.3 Zoom/pan, export

- **Zoom/pan: none built in.** `El::scale(k)` is draw-only ("layout keeps the unscaled size", `el/visuals.rs:364-368`),
  so a zoom viewer must size a container to `size*k` and put the scaled drawing in a `scroll_xy` viewport;
  wheel/magnify/rotate gestures are available to handlers (`tooling/args.rs` script gestures; `canvas_input`
  example). `rune_kit::image_zoom` is **image-only** (`image_zoom/mod.rs:1-20`; it drives `Image` pixels) and
  cannot pan an arbitrary `El`. The gallery fakes zoom with `.scale(3.0)` (`examples/kit-gallery/src/stories/markdown.rs:509-511`).
  **Effort M** for a proper pan/zoom lightbox using `Drawing.size`.
- **Export: none.** No `to_svg`/`write_svg` anywhere in `rune-mermaid` or the kit; `Scene`/`scene()` are
  `pub(crate)` (`lib.rs:371-415`; `canvas.rs:285-330`). Options:
  - **PNG**: render the diagram `El` in a headless runtime and `Offscreen`/`Renderer`/`write_png`
    (`crates/rune/src/render.rs:1052-1140`) — app-side, M.
  - **SVG**: vendor `rune-mermaid` and add a serializer over the scene's `Op::{Fill,Stroke}` + `Label{at,size,font,color}`
    (`canvas.rs:87-102`). Labels' baselines are shaped by Rune, so text export is approximate; M–L.
  - Running mermaid.js/Chromium is excluded by the "no browser dependency" constraint.

---

## 5. Shell and windows

### 5.1 Multi-window

- `App::window(WindowSpec)` returns `(AppHandle, WindowHandle)`; `WindowSpec` carries key/app/options and
  per-window chrome/titlebar/focus/IME/playground (`crates/rune/src/app.rs:345-352`;
  `window/spec.rs:227-267`). `run_windows`, `run_windows_with`, `run_windows_with_args`
  (`tooling.rs:198-226`).
- `AppHandle` can wake/show/hide/focus/close/minimize/quit, set position, update state on any thread
  (`app.rs:643-780`). Cross-window `update` is the supported way to move a document between tabs/windows.
- Per-window state is separate (`AppHandle<T>` per window), so a **document registry** must live in shared
  `Arc<Mutex<…>>` state or be duplicated; there is no built-in shared app state.
- **Wayland has no window positions.** winit's Wayland backend returns `NotSupportedError` for
  `outer_position`/`inner_position` and makes `set_outer_position` a no-op
  (`vendor/winit/src/platform_impl/linux/wayland/window/mod.rs:263-275`). So `AppHandle::set_position`,
  `WindowOptions::position` and `Drag::screen_point` (`el/handlers.rs:165-176`, documented `None` "where the
  system hides window positions (Wayland)") cannot place a window on Wayland; the compositor decides. X11 works.
- Headless multi-window compositing exists for tests: `--desktop BACKDROP.png` (`tooling/args.rs:52-55`,
  README `crates/rune/README.md:2518-2545`).

### 5.2 Tab strip (reorder / tear-out)

`rune_kit::shell::TabStrip` (`crates/rune-kit/src/shell/tab_strip.rs:1-6, 324-490`) is exactly a browser tab
bar:

- `TabItem::new(id, title)` with subtitle, avatar, icon, status (busy/attention/unread/failed), pinned,
  closable, edit-in-place, decorate (`tab_strip.rs:167-235`).
- `on_select/on_close/on_double_click/on_hover/on_reorder` (`:449-471`).
- `tear_out(TearOutSpec)` + `on_tear_out(TabTearOut{id,index,offset,window_point,screen_point})` and
  `on_tear_move(TabTearMove{id,index,torn,grab,window_point,screen_point})` (`:387-395, 469-481`;
  `drag.rs:113-140`). `screen_point` is "in physical pixels, for placing the new window under the pointer"
  (`tab_strip.rs:120-122`) — the intended tear-out-to-new-window path.
- `TabDragState`: `hand_off()` (the strip stops drawing the tab; the new window owns it), `set_incoming(point)`,
  `incoming_slot()` to show a drop gap in another window (`drag.rs:194-232`); `cancel()`.
- The drag is a state machine owned by app state, so the app must create/move the new window during
  `on_tear_move` (`drag.rs:1-6`).

**Gap:** Rune has no cross-window pointer-capture API. The app must follow the pointer itself (the new window's
events, `Drag::screen_point`, or its own hit-testing against window frames), and there is no test that exercises
the handoff into a real second window (the strip's tests cover tear-out within one strip,
`crates/rune-kit/src/shell/tab_strip/tests.rs:1-60`). `screen_point` is `None` on Wayland, so the new window
cannot even be placed under the pointer there (`el/handlers.rs:170-176`). Cross-window tab drag is therefore
**M** app work on X11 and partly **blocked** on Wayland.

### 5.3 Window chrome

- `Titlebar::new(height).traffic_lights(x,y)` replaces the native title bar **on macOS and Windows only**;
  "Elsewhere the native frame stays and ChromeInsets is zero" (`window/titlebar.rs:15-31`), enforced by
  `custom_frame()` returning false off macOS/Windows (`window/chrome.rs:56-64`) and `place_chrome` returning 0
  (`chrome.rs:9-24`).
- `.window_drag()` on any element moves the window on every platform (double-click zooms/minimises)
  (`el/layout.rs:590-600`); `shell::WindowTitlebar` builds the band + tab strip + insets
  (`crates/rune-kit/src/shell/titlebar.rs:1-40`); `shell::Page` gives settings/lock-screen pages the same chrome.
- **Borderless on Linux is possible** (`WindowOptions::borderless` → `with_decorations(!borderless)`,
  `window/spec.rs:74-76, 153`) but Rune exposes **no interactive edge resize**: there is no
  `drag_resize_window` anywhere; only programmatic `Tx::resize_window`/`resize_window_instant`
  (`el/tx.rs:458-465`). Under Wayland winit provides no server-side decorations when disabled, so a borderless
  Linux window would be unresizable without a patch.
- **Always-on-top:** `WindowOptions::always_on_top` (`window/spec.rs:77, 97`) and
  `WindowHandle::set_always_on_top` (`window/handle.rs:132-136`). X11 maps it to `_NET_WM_STATE_ABOVE`
  (`vendor/winit/src/platform_impl/linux/x11/window.rs:1073-1083`); **Wayland's implementation is an empty
  function** (`vendor/winit/.../wayland/window/mod.rs:430-431`). So always-on-top is X11-only in practice.
- Window-level extras: `Chrome` (macOS full-size content / traffic lights / drag regions,
  `tooling.rs:63-90`), maximize/fullscreen, `fit_content_height`, `float_on_open`, `accessibility` tree,
  `titlebar_inset`/`chrome_insets` in `Cx` (`runtime/cx.rs:142-150`).

### 5.4 File dialogs, drops, drag-out, single instance

| Feature | State | Evidence |
|---|---|---|
| Native Open/Save/Folder | Feature `native-file-dialog`; Linux is **XDG desktop portal over zbus only** (no GTK fallback), errors "no XDG desktop portal FileChooser is available" | `crates/rune/Cargo.toml:20`; `file_dialog/portal.rs:1-8, 35`; `file_dialog.rs:249-266` |
| Async API | `Tx::open_file/save_file/open_folder(options, on_answer)` after the handler returns; filters, start dir, default name | `file_dialog.rs:31-39, 249-266` |
| Test dialogs | `FileDialogs::scripted([ScriptedFileDialog{…}])`, script `file-dialog`/`file-dialog-cancel` | `file_dialog.rs:44-62, 81-90` |
| Drop files in | winit drop events on all platforms; `.on_drop_files`, `.on_drop_hover`, `.on_drop_rejected_hover`, `.accepts_extensions` | `window/file_drop.rs:1-40`; `examples/drop-files/src/main.rs:30-60` |
| Drag out to other apps | **macOS and Windows only**; "Wayland and X11 don't start one yet" — `drag_out` is ignored there | `drag_out/mod.rs:12-18`; `SUPPORTED` `:27`; `DragItem` `:54-170` |
| Single instance | **Yes on Linux**: first process binds `$XDG_RUNTIME_DIR/rune-instance-<id>.sock`, later launches forward argv/links to `App::on_open_url` and exit | `open_url/instance.rs:1-46`; `single_instance`/`release_instance` |
| Open URL | `Tx::open_url` only `http/https/mailto`, via `xdg-open` on Linux; `App::on_open_url` receives incoming links | `open_url.rs:1-40`; `app.rs:492-500` |
| Tray | Linux StatusNotifierItem + dbusmenu over zbus (needs `dbus`, tray host; GNOME needs an extension) | `tray.rs:1-12` |
| Global shortcuts | X11 works; Wayland via GlobalShortcuts portal with `dbus`; `Tray`/`ShortcutError::Unsupported` without | `crates/rune/Cargo.toml:41-49`; `shortcuts/mod.rs:110-135` |

**Linux parity implication:** tear-out to another app (exporting a note as a file by dragging) and drag-out of
images are **not available**. You must offer Save/Export instead. Drag *in* works.

**Effort/risk:** shell kit adoption S–M (low); multi-window document registry + tear-out handoff M (medium);
custom Linux chrome M plus a Rune patch (high on Wayland); drag-out L, effectively blocked.

---

## 6. Platform

### 6.1 Fonts

- App fonts: `AppSpec::fonts: &[&[u8]]` + `AppSpec.families: (sans, mono)`; default Inter built in unless
  `embedded-fonts` is off (`tooling.rs:31-38`; `crates/rune/Cargo.toml:9-12`).
- `Family::{Sans, Mono, Named("…"), SystemMono}` (`tok.rs:26-38`). `Named` accepts a CSS-style family list and
  loads an *installed* family by name on first use, falling back to sans.
- **No `Serif` slot.** A serif reading face = bundle a font and use `Family::Named("Literata")`, or resolve an
  installed family by name. Same for display faces.
- System fallback is lazy, per missing character: fontconfig on Linux, DirectWrite on Windows, CoreText on Apple
  (`text/system_fonts.rs:1-16`). Linux CJK/script fallback list includes Noto Sans CJK / Source Han / PingFang /
  Hiragino (`system_fonts.rs:128-141`); symbol/emoji families are DejaVu Sans, Noto Sans Symbols, DejaVu Sans Mono,
  Noto Color Emoji on Linux (`system_fonts.rs:38-50`). So CJK and colour emoji work without bundling.
- Text quality: Parley 0.12 with ICU dictionaries, full bidi, `trak` tracking, optical sizing, synthetic
  bold/italic (`crates/rune/Cargo.toml:70-90`; `text/`).
- Text is laid out by Rune; there is **no font-picker or font-list API** for apps: `text::system_fonts` is a
  private module and its `installed_family_files` is `pub(super)` (`crates/rune/src/text.rs:50`;
  `text/system_fonts.rs:398`). A settings font picker must query fontconfig itself (`fontique`, `fc-list`) or
  ship a curated list of family names and let `Family::Named` fail soft to sans.

### 6.2 Accessibility

- Default feature (`crates/rune/Cargo.toml:8, 58-59`); AccessKit platform adapters: UIA, NSAccessibility,
  **AT-SPI on Linux** (`a11y/unix.rs:1-6`). Nothing is built until a client asks (`a11y/mod.rs:1-6`).
- Roles/labels/values/states/actions are set with `El::role/label/level/expanded/checked/…`
  (`el/a11y.rs:250-300`); headings and landmarks are patched into accesskit on Windows
  (`Cargo.toml:66-77`).
- Deterministic introspection: `Runtime::accessibility_tree()` / `accessibility_json()`
  (`a11y/tree.rs:264-280`), `Runtime::accessibility_action` (`a11y/actions.rs:121`), `--tree` writes it under
  `"accessibility"` (`tooling/trace.rs:383`).
- **Limits:** virtualised rows expose mounted rows only (README `crates/rune/README.md:2410-2412`); a canvas-drawn
  editor (our own layout) must report text/caret/selection manually or keep a `text_input` for AT.

### 6.3 i18n

`rune-i18n` is a separate crate (published? `publish = false`, `crates/rune-i18n/Cargo.toml:8`): build-time JSON
catalogs → one blob+span table embedded with `include_str!`; compile-time checked keys; `I18n::text`,
`set_locale`, `plural`, CLDR number/list formatting; system language detection and negotiation
(`crates/rune-i18n/src/lib.rs:1-18`; `i18n.rs:26-120`; `system.rs:1-20`). It is message-only; UI mirroring is
Rune's `Direction` (`el/direction.rs:56`) and bidi text. **RTL UI is not automatic** — the shell lays out LTR
unless each row gets `.dir(Rtl)`.

### 6.4 Clipboard

Per-platform forms (README table `crates/rune/README.md:1381-1388`):

| Platform | Copy | Paste | Image |
|---|---|---|---|
| Linux X11/Wayland | text + `text/html` (no RTF) | `text/html`, else text | `image/png` via arboard, re-encoded |
| macOS | text + HTML + RTF in one change | HTML, else RTF→HTML, else text | PNG/TIFF |
| Windows | text + CF_HTML | CF_HTML, else text | PNG/DIBV5 |

APIs: `Tx::copy`, `copy_rich(ClipboardContent)`, `read_clipboard`, `request_paste`, `copy_secret(text, ttl)`
(`clipboard/tx.rs:19-70`); `ClipboardContent::{text,html,rtf,image}` (`clipboard/content.rs:25-160`);
`on_input_paste` / root `.on_paste` (`el/input.rs:206`; README `1336-1341`). Wayland needs
`wayland-data-control` (enabled in `Cargo.toml:114,118`).

**Limit:** on Linux "write one form" means files > image > HTML+text > text (`clipboard/tx.rs:26-31`); rich copy
is text+HTML, so pasting into LibreOffice keeps bold/italics but not RTF fidelity.

### 6.5 Persistence

- `rune::prefs::Prefs`: JSON at `$XDG_CONFIG_HOME/<app-id>/prefs.json` (or
  `~/Library/Application Support/<id>`, `%APPDATA%\<id>`), atomic temp-file + rename, `.bad` quarantine,
  `memory()` for tests, `RUNE_PREFS_DIR` override (`crates/rune/src/prefs.rs:1-17, 60-110`).
- `rune::dirs::AppDirs`: config/data/cache/state/logs/home/downloads with `RUNE_DIRS_ROOT` for isolated runs
  (`crates/rune/src/dirs.rs:1-40`).
- **No document/session store**: tabs, scroll positions, undo history, recents, per-file state are app-owned.
  Rune only gives the KV store and the directories.

### 6.6 Linux build and runtime

- Build deps: `libxkbcommon` + `libwayland-client` dev packages (README `crates/rune/README.md:2426-2436`).
- Runtime: `libxkbcommon-x11` for any X11 window; otherwise startup panics with
  `Library libxkbcommon-x11.so could not be loaded` (README `:2440-2446`).
- Window backend: winit 0.30.13 with `x11`, `wayland`, `wayland-dlopen`,
  `wayland-csd-adwaita-notitle` (`crates/rune/Cargo.toml:70-74`). Wayland CSD comes from winit's Adwaita
  decorations; Rune never draws its own frame there (§5.3).
- Overlays: layer-shell (Wayland) and X11 overlay paths (`platform/layer_shell.rs`, `platform/x11/overlay.rs`);
  `set_app_id` sets Wayland `app_id` / X11 `WM_CLASS` for `.desktop` matching (`tooling.rs:110-120`).
- GPU: wgpu 30 with Vulkan on Linux (`crates/rune/Cargo.toml:151-154`); software Vulkan (lavapipe) is the
  documented fallback (`README:2437-2444`). **No NVIDIA-specific code**; the risk register calls out Wayland/X11
  swapchain hangs among the reasons not to own a Vulkan backend (`research/34-own-gpu-plan.md:171`) — that is a
  general driver caveat, not something Rune works around.
- Threading: renderer main-thread, image decode on a worker, clipboard worker, AT-SPI/D-Bus thread, tray thread.

**Effort/risk:** platform plumbing is mostly free (S, low). Linux-specific polish — font picker, RTL UI
mirroring, global shortcuts/tray, spellcheck — is S–M each, medium risk. Drag-out and Wayland window placement
are the only platform items with no clean path (see §11).

---

## 7. Headless and agent tooling

This is the strongest part of Rune for an agent-built app. Help text: `crates/rune/src/tooling/args.rs:239-305`;
implementation: `tooling/headless.rs`, `tooling/record.rs`, `tooling/trace.rs`, `tooling/virtual_desktop.rs`.

| Command | What it does | Evidence |
|---|---|---|
| `--snap out.png [--scene NAME] [--at T] [--size WxH] [--scale N] [--theme dark\|light] [--crop KEY]` | offscreen PNG at a virtual time; no window, no display, no XKB | `args.rs:239-260`; README `2448-2468` |
| `--tree out.json [--at T]` | every element: key, path, role, label, text, presented rect, target, opacity, fill/text colour, hover/press, moving, caret, selection, stuck, plus `accessibility`, popup menu, file dialog, preedit, tooltip | `tooling/trace.rs:293-390` |
| `--script FILE.json [--frames N] [--start S] [--out DIR] [--video f.mp4] [--every N] [--strip s.png]` | timestamped synthetic input on a fixed virtual clock | `args.rs:262-280`; script events in README `2470-2515` |
| Script events | `move/down/up/tap/drag` at `x,y` or `key`/key-path; `key`/`type`/`key-up`; `wheel`/`magnify`/`rotate`/`smart-magnify`; `resize`; `shortcut`; `menu-pick`/`menu-cancel`; `file-dialog`/`file-dialog-cancel`; `drag_files`/`drop_files`; `paste`; `mark`; `reopen`/`open_url`/`focus` | README `2470-2515`; `tooling/script.rs` |
| `--trace out.jsonl [--trace-all] [--native]` | motion-lint JSONL of per-frame rects/opacity; keyed elements only unless `--trace-all` | `args.rs:23-31, 184-190` |
| `--html out.html [--html-widths 360,640] [--html-template f]` | static HTML+CSS of the laid-out scene | `tooling/html.rs:1-16`; §8 |
| `--desktop BACKDROP.png [--window KEY]` | composite all windows with frames, focus and z-order | README `2518-2545`; `tooling/virtual_desktop.rs` |
| `--list-scenes`, `--list-windows`, `--playground`, `--variant` | enumerate/snapshot storybook scenes | `args.rs:57-68` |
| `rune::tooling::headless_run()` | app reads script path/events, `scale/fps/size/start/frames/end`, `mark(name)` from `init` on | README `2563-2582`; `tooling/headless_run.rs` |
| `rune::motion_gate` | records a real `Runtime` frame-by-frame and fails on visible pops/jumps/overshoot | README `2583-2590` |

**Determinism:** `--scale` defaults to 2× (`tooling/headless.rs:39, 82`); headless runs step a fixed `dt`; `Runtime::frame(dt)` advances animation time and runs the view
(`runtime/frame.rs:343-352`), and the runtime's logical clock is driven by the host frame (`frame.rs:386-392`).
`--at mark:NAME+0.5` resolves script marks (`args.rs:45-47`). `--snap` needs a GPU (lavapipe fine) but no display.

**Golden tests:** JSON element trees with `RUNE_BLESS=1` (`crates/rune-kit/tests/markdown_fixture.rs:1-10,
142-156`; `tests/a11y/golden/`). Pixel comparison exists internally through `rune-ref`'s reference rasteriser
(e.g. `crates/rune-kit/tests/scroll_bar_fade.rs:1-6`; `crates/rune/src/text/pixel_compare.rs`), but there is no
app-facing pixel-golden runner; `scripts/kit-diff.py` compares against Chromium from an OpenCode checkout and is a
maintainer tool (`crates/rune-kit/README.md:403-420`). For lmdv, use `--tree` as the structural golden and a
small tolerance-based PNG comparator of your own.

**Headless limitations to plan around:**

- No OS file dialog: answer with script `file-dialog` or the run blocks in the tree (`file_dialog.rs:1-8`).
- No real window manager: always-on-top, CSD, drag-out, tray and portal dialogs are not exercised
  (`window/spec.rs:77` still records the flag; X11/Wayland behaviour is host-side).
- One virtual display unless the script declares `displays` (`tooling/virtual_desktop.rs`).
- `--trace` keyed-only by default; unkeyed paths are unstable for assertions.
- Images from raw pixels are not exported to HTML (§8), so math/diagram goldens are PNG-only.

---

## 8. Export (print/PDF/SVG/PNG/HTML/DOCX)

| Format | Support | Evidence |
|---|---|---|
| **PNG** | App-side and total: `Gpu::headless()`, `Renderer`, `Offscreen`, `Runtime::frame`, `renderer.prepare/draw`, `Offscreen::read`, `render::write_png(path,w,h,rgba)`; demonstrated in `crates/rune/examples/long_note.rs:145-160, 260-280` | `render.rs:1052-1140, 1138` |
| **HTML** | CLI `--html OUT.html`, per width, static flexbox CSS from the laid-out tree; templates with `<!-- rune:css -->` / `<!-- rune:body -->` | `tooling/html.rs:1-80`; `runtime/web/export.rs:1-175` |
| **PDF / print** | **None.** No print pipeline, page model, or PDF writer; `grep -i pdf crates/` finds only a macOS drop MIME and a menu fixture | whole tree |
| **SVG** | **None as an export.** `Shape` → inline `<svg>` exists inside `--html` (`runtime/web/export.rs:141`, `runtime/web/export/css.rs`), but `ExportNode`/`export_nodes`/`export` are `pub(crate)` — no app API | `runtime/web/export.rs:20-52`; `tooling/html.rs:10` |
| **DOCX/Markdown** | Markdown is the source format; DOCX none | — |

**HTML export specifics (important for parity):**

- It is the *laid-out scene*, not Markdown: text becomes `<span>/<p>` with per-element CSS, headings become
  `h1–h6` (`runtime/web/export.rs:105-118`), links become `<a href>`, list roles become `ul/li`, `figure`,
  `main/nav/header/footer/section/article/aside` (`runtime/web/a11y.rs:107-121`); **tables, code and blockquotes
  have no semantic tag mapping** (roles `table`/`cell`/`code`/`blockquote` are absent from `container_tag`), so a
  Markdown table exports as styled `div`s.
- Images export as `<img src>` only when `Pixels::source_url()`/`source_set()` is known, i.e. loaded through
  `ImageCache::url/file` (`image/pixels.rs:68-82`; `runtime/web/export.rs:100-135`). **Math/diagram rasters and
  raw RGBA images cannot be exported**, and there is no data-URI fallback.
- Anchor ids come from element keys (`runtime/web/export.rs:117, 124-129`); Markdown blocks keyed `b{i}` qualify
  but are index-based, not slugs.
- **In-app export** requires launching yourself as a subprocess with `--html` (the function is `pub(crate)`), or
  patching `tooling/html.rs`/`ExportNode` to be public.

**Display-list access:** `Runtime::instances` is a public flat `Vec<Instance>` (`crates/rune/src/runtime.rs:502`;
`Instance` is rect/radii/fill/border/params/uv/clip/fade, `crates/rune/src/render.rs:212-233`) with primitive
kinds `KIND_RECT…KIND_GLYPH…KIND_LAYER` (`render.rs:42-87`), and `Runtime::elements()` gives laid-out rects and
colours (`runtime/elements.rs:204-260`). But glyphs are atlas UV quads and layer records are renderer state, so
this is **not** a display list you can serialise to SVG text/shapes. Use it only for hit-testing/analysis.

**Recommended export strategy for lmdv**

1. **PNG** per document/page: render a print-sized headless runtime at 2× and write PNG. Reuse for Mermaid
   diagram export and for golden tests. (S)
2. **HTML**: either shell out to `lmdv --html` (S) or expose Rune's exporter with a small patch (M), then improve
   semantics in your own post-processor (tables/code blocks/anchors). No fidelity guarantee against the reference web
   export.
3. **PDF**: separate pipeline. Two realistic options, both "offline Rust engines" (allowed by the project rules):
   Typst (`typst` + `typst-pdf`, typeset from the document model, best quality, but a second layout engine), or
   print the static HTML with an external tool (adds a user-installed dependency, not offline-first). **Effort L,
   risk high for pixel parity.** Do not expect Rune to grow a print pipeline.

---

## 9. Cargo features and dependency setup

### 9.1 `rune` features (default = `["accessibility", "dbus", "embedded-fonts", "metal-backend", "ogg-audio", "svg"]`, `crates/rune/Cargo.toml:8`)

| Feature | What it buys | Recommendation for lmdv |
|---|---|---|
| `accessibility` (`:58`) | AccessKit UIA/NSAccessibility/**AT-SPI** adapters. `--tree` works without it | **keep** |
| `dbus` (`:46`) | system light/dark, tray, Wayland global shortcuts, portal file dialogs. Without it: windows draw dark, tray/portal unsupported | **keep** |
| `embedded-fonts` (`:9-12`) | Inter Regular/Medium/SemiBold (~1.25 MB) fallback when the app registers none | **drop** if shipping own fonts |
| `svg` (`:34`) | `resvg` rasterisation for `ImageCache::svg` and scalable icons — **required for the math path** | **keep** |
| `native-file-dialog` (`:20`) | rfd (mac/win) + XDG portal (Linux, needs `dbus`) | **keep** |
| `avif` (`:39`) | rav1d; building on x86 needs `nasm` on PATH | drop unless AVIF images matter |
| `ogg-audio` (`:29-32`) | Ogg/Opus/WebM audio on Apple only (AVFoundation gap) | drop on Linux |
| `metal-backend` (`:53`) | Apple-only native Metal backend; no effect on Linux (deps are Apple-target gated) | harmless; explicitly drop for clarity |
| `hot` (`:21`) | live Rust patching via `dx 0.7.10`; dev only | dev-only feature, never release |
| `frame-profile` (`:16`) | headless bench phase clocks | dev/bench only |
| `webgl`, `alloc-sites` | web/measuring | drop |

### 9.2 `rune-kit` features

`markdown = ["dep:pulldown-cmark"]` and `mermaid = ["markdown", "dep:rune-mermaid"]`
(`crates/rune-kit/Cargo.toml:19-25`). Use both. `frame-profile` forwards to `rune`.
`rune-kit` depends on `rune` with `default-features = false` (`Cargo.toml:8`), so **the app must enable its own
`rune` features**; kit components compile without `accessibility`, but then no OS a11y adapters exist.

### 9.3 Pin and patch (critical wiring)

- Pin `rune`, `rune-kit`, `rune-mermaid`, `rune-i18n` to rev `cca5df6fafb4721d96fe105f126ebfd2ec351bb6`
  (2026-11-17), toolchain `1.99.1` (`rust-toolchain.toml`), edition 2024.
- Rune's workspace patches five crates: `wgpu-hal`, `winit`, `parley_engine`, `accesskit_windows`,
  `accesskit_macos` (with `vendor/*.patch`). **A dependency's `[patch]` does not apply to the dependent
  workspace**, so lmdv vendors those five dirs and repeats the `[patch.crates-io]` block, or it will build
  against stock crates and lose fixes (bidi L1, text shaping cache, Wayland/Win32 window fixes, a11y).
  `tiny-skia-path` is a path dependency of `crates/rune` itself, not a root patch.
- Keep the `Cargo.lock`; Rune depends on pre-release-ish ranges: `wgpu ~30.0.1`, `winit 0.30.13`, `taffy 0.15`,
  `parley 0.12`, `accesskit 0.25.2`, `resvg 0.45`, `pulldown-cmark 0.13` (`crates/rune/Cargo.toml:60-130`;
  `crates/rune-kit/Cargo.toml:16`).
- Patching Rune itself: keep each patch as a numbered `.patch` + rationale, and
  `scripts/apply-rune-patches.sh` makes a rev bump mechanical. The current local set is:
  1. `0001-text-inline-object` — non-square inline image box for math (§3).
  2. `0002-close-request-veto` — native close asks before hiding.
  Both rebased onto `cca5df6`; run `scripts/test-rune-patches.sh` after a vendor change.
- **Do not** `[patch]` `rune` to a local copy and also consume it from crates.io; pick one source for all four
  Rune crates (git rev) and patch with path overrides in the same table.

### 9.4 Suggested dependency block

```toml
[workspace.dependencies]
rune = { git = "https://github.com/anomalyco/rune", rev = "cca5df6fafb4721d96fe105f126ebfd2ec351bb6",
         default-features = false,
         features = ["accessibility", "dbus", "svg", "native-file-dialog", "tooling"] }
rune-kit = { git = "…", rev = "…", features = ["markdown", "mermaid"] }
rune-mermaid = { git = "…", rev = "…" }
rune-i18n = { git = "…", rev = "…" }          # optional; codegen in build.rs
rune-theme-opencode = { git = "…", rev = "…" } # only if you want the ported OpenCode theme
ratex-parser = "0.1.15"
ratex-layout = "0.1.15"
ratex-svg = { version = "0.1.15", features = ["standalone", "embed-fonts"] }
# PDF, later: typst + typst-pdf (separate renderer)

[patch.crates-io]
wgpu-hal = { path = "vendor/wgpu-hal" }
winit = { path = "vendor/winit" }
parley_engine = { path = "vendor/parley_engine" }
accesskit_windows = { path = "vendor/accesskit_windows" }
```

Linux build deps: `libxkbcommon libxkbcommon-x11 wayland mesa-vulkan-drivers` (Arch), per README
`crates/rune/README.md:2426-2446`.

---

## 10. Build vs reuse recommendation

**Reuse as-is (depend, do not fork):**

- `rune` core: element/layout/renderer/text, windowing, clipboard, prefs/dirs, file dialogs, single instance,
  accessibility, tray, shortcuts. Re-implementing any of this is months.
- `rune-kit`: shell (`SplitView`/`Sidebar`/`Toolbar`/`Page`/`WindowTitlebar`), `TabStrip`, `Tree`, `Table`,
  `TextInput`/`TextArea`, `Menu`/`Dialog`/`Toast`/`Popover`, `Markdown`, `notes`, `syntax`, `search`,
  `image_zoom`, `diff`, `line_selection`.
- `rune-highlights`: 80 lexers/211 aliases from Pierre's Highlights, 0.1–0.6 ms per 1000 lines
  (`crates/rune-highlights/src/lib.rs:1-22`). Do not write a highlighter.
- `rune-i18n` for message catalogs if lmdv ships localized UI.

**Vendor/patch (small, bounded):**

| Patch | Why | Size | Risk |
|---|---|---|---|
| `rune-kit` Markdown parser: front matter (or do app-side), footnotes, callouts, heading ids, optional sticky header | Only place the parse happens | ~300–600 LOC | medium |
| `rune` inline box: `(w,h,ascent,descent)` instead of square | Inline math parity | ~100 LOC in 3 files | high (text) |
| `rune-mermaid`: `Scene` → SVG | Diagram export parity | ~300 LOC | medium |
| `rune` Linux window: `drag_resize_window` edges / optional custom frame | Custom chrome parity | ~200 LOC | medium |
| `rune` `TextEdit`: delta undo + per-paragraph layout | Long-document typing parity | large | high |

**Write yourself:**

- `lmdv-core`: document model (source, dirty state, undo, reading position, line endings), tab/window registry,
  recents, session restore, file watching/reload prompts.
- Editor surface: block-per-paragraph `TextEdit`s (recommended) or a custom layout editor; gutter; find/replace;
  spellcheck on Linux; Markdown formatting commands; task-checkbox write-back.
- Math service (RaTeX + cache + theme colour + baseline metrics).
- Outline/TOC, link resolution relative to the document, image loading/cache policy.
- Export service: PNG via offscreen, HTML (subprocess/patched exporter), PDF via Typst if required.
- Mermaid lightbox zoom/pan and PNG export (reuse `Drawing`).

**Do not write from scratch:** renderer, text shaping, accessibility, windowing, tab strip, code highlighter,
Mermaid engine, Markdown parser (fork it instead).

**Architecture note for parity:** the specification's viewer and editor can share the `Markdown` renderer for view mode and
block editors for edit mode, with `block_keys` mapping source offsets ↔ rendered blocks. That mapping is also the
anchor for TOC, find, selection restore and per-block editing.

---

## 11. Blockers with no clean Rune path

1. **Inline math with correct baseline and aspect.** No public inline-object API; the only inline visual is a
   square icon (`text/pill_chips.rs:30`, `runtime/paint/content.rs:452-460`). Clean parity requires a Rune patch
   to the inline box model. Everything else (block math, numbers, units) can be done app-side.
2. **PDF / print / DOCX.** Nothing exists; `--html` is a first-paint export, not paginated, and it is `pub(crate)`.
   A separate Typst (or similar) pipeline is mandatory, and its layout will never match the on-screen renderer
   exactly.
3. **Vector (SVG) export of the document or diagrams.** `Shape` can be serialized internally (`runtime/web/export.rs:141`)
   but no public display list or SVG writer exists; `rune-mermaid`'s scene is private. Requires vendoring and
   writing a serializer, with approximate text positioning.
4. **Linux custom titlebar + resizable borderless window.** Rune deliberately keeps the native frame off
   macOS/Windows (`window/titlebar.rs:15-31`, `window/chrome.rs:56-64`) and exposes no interactive edge resize
   (`el/tx.rs:458-465`). Parity with a client-side-decorated app needs a Rune patch.
5. **Drag-out / tear-off to other applications on Wayland/X11.** `drag_out` is compiled but unsupported
   (`drag_out/mod.rs:12-18, 27`). Wayland DnD source + XDND is a large platform project; offer "Save/Export"
   instead. **Tab tear-out to a new window is also degraded on Wayland**: no window positions or
   `screen_point` (`vendor/winit/.../wayland/window/mod.rs:263-275`; `el/handlers.rs:170-176`), so the new
   window cannot be placed under the pointer.
6. **Full raw HTML in Markdown.** No HTML/CSS engine and no plan for one; a whitelist subset is the ceiling.
7. **Multi-cursor editing and Linux spellcheck.** Neither exists; both are app-level or patches, and the spell
   checker has no platform hook to reuse on Linux (`spell.rs:24-33`).

## 12. Pleasant surprises

1. **Streaming is designed in, not bolted on.** Tail-only reparse, stable block keys, memoised parse, streaming
   syntax colouring, streaming Mermaid with per-node enter animations (`markdown/stream.rs:1-47`,
   `markdown.rs:14-27, 259-274`, `render.rs:452-464, 513-538`). This is ideal for an editor preview pane.
2. **Find that maps rendered text back to source bytes**, with accent/case folding and paint-only highlights
   (`markdown/find.rs:30-56`; `search.rs:1-56`; `markdown.rs:229-252`). A find bar is a thin UI over it.
3. **Browser-grade copy semantics for read-only Markdown**: one selection across blocks, list markers, table tabs,
   no fences/`**`; or source copy by block (`render.rs:104-136`, `markdown.rs:322-345`).
4. **Mermaid is 17 diagram families, not 3** — including C4, sankey, block, requirement, git, gantt, xychart —
   lenient parsing, hover highlighting, `render_drawing` for a lightbox, and a 16-scene cache
   (`rune-mermaid/src/lib.rs:1-65, 219-324`; `scenes.rs:1-46`).
5. **Agent tooling is exceptional**: `--snap/--tree/--script/--trace/--html`, a fixed virtual clock, `--desktop`
   multi-window composites, keycaps/pointer overlays, and `headless_run()` so the app can read its own script
   (`tooling/args.rs:239-305`; `tooling/trace.rs:293-390`; README `2448-2580`). Golden JSON trees with
   `RUNE_BLESS=1` (`markdown_fixture.rs:1-10`).
6. **Shell kit covers a real desktop app**: split view, sidebar with peek/reorder/virtual rows, toolbar, page,
   window titlebar band, and a tab strip with reorder/tear-out/cross-window handoff
   (`crates/rune-kit/src/shell/*`, `tab_strip/drag.rs:1-232`).
7. **Single instance on Linux is already implemented** with a per-user Unix socket and launch forwarding to
   `App::on_open_url` (`open_url/instance.rs:1-46`) — exactly what a file-handler `.desktop` entry needs.
8. **`Prefs` + `AppDirs`** give atomic JSON settings and XDG-correct folders, with test isolation env vars
   (`prefs.rs:1-17`; `dirs.rs:1-40`), so lmdv's settings/theme/recent-files layer starts from working plumbing.
9. **Rich clipboard out of the box**, including HTML+text on Linux, images, and secret-copy with expiry
   (`clipboard/tx.rs:19-70`; `clipboard/content.rs:25-160`).
10. **`El::inline_icon` can embed an image inline in a text run** (`el/pills.rs:78-118`) — square, but enough to
    prototype inline math before committing to a patch.
11. **Text quality work is already done**: Parley + ICU dictionaries, bidi, optical sizing, `trak`, synthetic
    bold/italic, system fallback for CJK/emoji (`crates/rune/Cargo.toml:70-90`; `text/`).
12. **`Cx::caret`, `El::ime_cursor`, `App::on_ime`** support an app-drawn editor with a real input method
    (`runtime/cx.rs:295`; `el/layout.rs:612-621`; `app.rs:319-327`).

## 13. Where the README/docs mislead (verified against code)

| Claim | Reality | Evidence |
|---|---|---|
| `crates/rune-kit/README.md:331-333`: Mermaid supports "flowcharts, sequence diagrams and state diagrams" | 17 families | `rune-mermaid/src/lib.rs:286-334` |
| `crates/rune-kit/README.md:286-292`: "renders CommonMark plus GFM tables" | Also strikethrough + task lists; **raw HTML is silently dropped**, footnotes/front matter absent | `parser.rs:466, 667` |
| `crates/rune/README.md:2940`: "No AccessKit … Scrolling is vertical only" under "Differences from PLAN.md" | Accessibility is a **default feature** with AT-SPI/UIA/NSAccessibility and `--tree` support; horizontal scrolling exists (`El::scroll_x`, `scroll_xy`) | `crates/rune/Cargo.toml:8,58`; `a11y/unix.rs`; `el/scroll.rs:54-80` |
| `crates/rune-kit/src/markdown.rs:5-6`: code fences colour "rust, js, ts, tsx, json, python, go, swift, sh, sql, html, css and yaml" | 80 lexers / 211 names via `rune-highlights`, first 128 KiB / 32k ranges | `syntax.rs:1-30, 122-141` |
| `crates/rune-kit/README.md:333-334`: "anything outside the supported subset … shows as its plain code block" | True; unknown type/no readable statement/ >400 items/ >8 nesting returns `None` and falls back | `rune-mermaid/src/lib.rs:61-65, 312-315` |

## 14. Quick API index (where to start reading)

| Need | Read |
|---|---|
| Markdown builder | `crates/rune-kit/src/markdown.rs:222-449` |
| Markdown parse → blocks | `crates/rune-kit/src/markdown/parser.rs:440-668` |
| Markdown block render | `crates/rune-kit/src/markdown/render.rs:381-738` |
| Markdown style | `crates/rune-kit/src/markdown/style.rs:167-500` |
| Markdown tables | `crates/rune-kit/src/markdown/table.rs:1-499` |
| Markdown find/highlight | `crates/rune-kit/src/markdown/find.rs`, `highlight.rs` |
| Structured notes/callouts | `crates/rune-kit/src/notes.rs:58-190, 547-565` |
| Text edit model | `crates/rune/src/edit.rs`, `edit/model.rs`, `edit/history.rs`, `edit/ime.rs` |
| Text field el | `crates/rune/src/el/input.rs`, `crates/rune/src/el/text.rs` |
| Spans/highlights/pills | `crates/rune/src/text/span.rs`, `crates/rune/src/el/pills.rs` |
| Mermaid | `crates/rune-mermaid/src/lib.rs`, `crates/rune-kit/src/mermaid.rs` |
| Shell/tabs | `crates/rune-kit/src/shell/*`, `shell/tab_strip/*` |
| Windows/chrome | `crates/rune/src/window/spec.rs`, `titlebar.rs`, `chrome.rs`, `app.rs:643-780` |
| Clipboard | `crates/rune/src/clipboard*.rs`, `clipboard/*` |
| Headless tooling | `crates/rune/src/tooling/*` |
| Images/SVG | `crates/rune/src/image.rs`, `image/pixels.rs` |
| Fonts | `crates/rune/src/text/system_fonts.rs`, `tok.rs:26-38` |
| Prefs/dirs | `crates/rune/src/prefs.rs`, `dirs.rs` |
| i18n | `crates/rune-i18n/src/*` |
