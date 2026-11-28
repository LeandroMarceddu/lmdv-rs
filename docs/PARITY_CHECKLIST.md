# Parity checklist

Generated from [`BEHAVIOR_SPEC.md`](BEHAVIOR_SPEC.md) and the status tables in
[`audit/`](audit/) by `scripts/gen-parity-checklist.py`. Do not edit by hand:
update the audit, then regenerate. Evidence is cited per row in the audit.

**Total behaviors:** 342

## How to use

- `[ ]` missing, `[~]` partial, `[x]` done or accepted substitute/gap.
- Status and evidence live in `docs/audit/*.md`; this file is generated from them.
  Do not edit it by hand: run `scripts/gen-parity-checklist.py` after an audit.
- A row is done only when its acceptance criterion has a test, golden, tree
  assertion or manual note; the tag after each row names its status.

## Counts

| Area | Prefix | Rows | P0 | P1 | P2 |
|---|---|---:|---:|---:|---:|
| Reading and viewer | R | 30 | 16 | 14 | 0 |
| Tabs and windows | T | 35 | 16 | 17 | 2 |
| Editor | E | 40 | 22 | 16 | 2 |
| Markdown rendering | M | 34 | 19 | 13 | 2 |
| Full-window preview | V | 18 | 2 | 15 | 1 |
| Share, print and export | X | 16 | 5 | 10 | 1 |
| File browser and folder search | F | 22 | 13 | 8 | 1 |
| Outline, section line and history | O | 15 | 7 | 6 | 2 |
| Command palette | C | 13 | 5 | 8 | 0 |
| Links and media | L | 15 | 10 | 4 | 1 |
| System, CLI and file integration | S | 27 | 15 | 6 | 6 |
| Window chrome | W | 10 | 2 | 4 | 4 |
| Drag and drop | D | 8 | 2 | 5 | 1 |
| Help, start page, notices, rating | H | 13 | 3 | 4 | 6 |
| Accessibility | A | 15 | 4 | 11 | 0 |
| Internationalization | I | 11 | 4 | 5 | 2 |
| Persistence | P | 20 | 7 | 9 | 4 |

### Audit status

| Area | Rows | Done | Substitute | Partial | Missing | n/a |
|---|---:|---:|---:|---:|---:|---:|
| Reading and viewer | 30 | 27 | 3 | 0 | 0 | 0 |
| Tabs and windows | 35 | 26 | 7 | 2 | 0 | 0 |
| Editor | 40 | 36 | 2 | 1 | 0 | 1 |
| Markdown rendering | 34 | 31 | 3 | 0 | 0 | 0 |
| Full-window preview | 18 | 18 | 0 | 0 | 0 | 0 |
| Share, print and export | 16 | 14 | 2 | 0 | 0 | 0 |
| File browser and folder search | 22 | 20 | 2 | 0 | 0 | 0 |
| Outline, section line and history | 15 | 12 | 2 | 1 | 0 | 0 |
| Command palette | 13 | 11 | 2 | 0 | 0 | 0 |
| Links and media | 15 | 13 | 0 | 1 | 0 | 1 |
| System, CLI and file integration | 27 | 14 | 6 | 0 | 0 | 7 |
| Window chrome | 10 | 3 | 3 | 0 | 0 | 4 |
| Drag and drop | 8 | 5 | 2 | 1 | 0 | 0 |
| Help, start page, notices, rating | 13 | 12 | 0 | 0 | 0 | 1 |
| Accessibility | 15 | 14 | 1 | 0 | 0 | 0 |
| Internationalization | 11 | 6 | 3 | 1 | 0 | 1 |
| Persistence | 20 | 1 | 16 | 0 | 0 | 3 |
| **All** | **342** | **263** | **54** | **7** | **0** | **18** |

## R — Reading and viewer (30)

- [x] **R-01** Viewer-first opening (P0 · F-BASIC) — done
- [x] **R-02** Reading column defaults (P0 · F-BASIC) — done
- [x] **R-03** Reading progress hairline (P1 · F-LONG) — done
- [x] **R-04** Reading time (P1 · F-LONG) — done
- [x] **R-05** Reading time placement (P1 · F-BASIC) — done
- [x] **R-06** j / k scroll (P0 · F-LONG) — done
- [x] **R-07** Space / Shift+Space paging (P1 · F-LONG) — substitute
- [x] **R-08** . / , heading steps (P0 · F-HEADINGS) — done
- [x] **R-09** g g / G (P0 · F-LONG) — done
- [x] **R-10** n / N find stepping (P1 · F-FIND) — done
- [x] **R-11** ? keys overlay (P1 · F-BASIC) — done
- [x] **R-12** Per-file reading position (P0 · F-LONG) — done
- [x] **R-13** Position survives edits above (P0 · F-LONG) — done
- [x] **R-14** Top means forgotten (P1 · F-LONG) — done
- [x] **R-15** Missing anchor falls back to top (P1 · F-HEADINGS) — done
- [x] **R-16** Find bar open (P0 · F-FIND) — done
- [x] **R-17** Find prefill from selection (P1 · F-FIND) — done
- [x] **R-18** Find matching semantics (P0 · F-FIND) — done
- [x] **R-19** Find scrolling (P1 · F-TABLE) — substitute
- [x] **R-20** F5 / Ctrl+R (P0 · F-RELOAD) — done
- [x] **R-21** Live reload without edits (P0 · F-RELOAD) — done
- [x] **R-22** Live reload with unsaved edits (P0 · F-CONFLICT) — done
- [x] **R-23** Rename while open (P0 · F-RELOAD) — done
- [x] **R-24** Delete/move-away while open (P0 · F-RELOAD) — done
- [x] **R-25** Ctrl+A scope (P1 · F-BASIC) — done
- [x] **R-26** Selection copy (P1 · F-SELECT) — done
- [x] **R-27** Double-click while reading (P0 · F-BASIC) — done
- [x] **R-28** Drag to scroll (P1 · F-LONG) — done
- [x] **R-29** Empty/error state persistence (P1 · F-ERRORS) — done
- [x] **R-30** Reading settings isolation (P0 · F-PREFS) — substitute

## T — Tabs and windows (35)

- [~] **T-01** Tab strip (P0 · F-BASIC) — partial
- [x] **T-02** Tab names (P0 · F-PASTE) — done
- [~] **T-03** Dirty indicator (P0 · F-EDIT) — partial
- [x] **T-04** New tab (P0 · F-BASIC) — done
- [x] **T-05** Close tab (P0 · F-TABS) — done
- [x] **T-06** Close begin page/window (P0 · F-TABS) — done
- [x] **T-07** Reopen closed tab (P0 · F-TABS) — done
- [x] **T-08** Next/previous tab (P0 · F-TABS) — done
- [x] **T-09** Tab by number (P1 · F-TABS) — substitute
- [x] **T-10** Move tab by keyboard (P1 · F-TABS) — done
- [x] **T-11** One file, one tab per window (P0 · F-TABS) — done
- [x] **T-12** Click-open placement (P0 · F-TABS) — done
- [x] **T-13** Start-page replacement (P0 · F-BASIC) — done
- [x] **T-14** Background insertion order (P1 · F-TABS) — done
- [x] **T-15** Background tab pulse (P2 · F-TABS) — done
- [x] **T-16** Tab hover card (P1 · F-TABS) — done
- [x] **T-17** Overflow tab list (P1 · F-TABS) — substitute
- [x] **T-18** Tab context menu (P1 · F-TABS) — done
- [x] **T-19** Tab drag reorder (P0 · F-DRAG) — done
- [x] **T-20** Tab tear-off (P1 · F-DRAG) — substitute
- [x] **T-21** Drag only tab → window (P1 · F-DRAG) — substitute
- [x] **T-22** Drop tab onto another window (P1 · F-DRAG) — substitute
- [x] **T-23** Close others / right (P1 · F-TABS) — done
- [x] **T-24** Window close question (P0 · F-TABS) — done
- [x] **T-25** Close request routing (P1 · F-TABS) — done
- [x] **T-26** Tab state retention (P0 · F-TABS) — done
- [x] **T-27** Rendered-view cache (P1 · F-TABS) — substitute
- [x] **T-28** State carried across reload (P1 · F-I18N) — substitute
- [x] **T-29** Close-key precedence (P1 · F-PREVIEW) — done
- [x] **T-30** Window close key (P0 · F-TABS) — done
- [x] **T-31** New window (P1 · F-WINDOW) — done
- [x] **T-32** Multi-select open (P1 · F-BASIC) — done
- [x] **T-33** Tabs across windows (P0 · F-WINDOW) — done
- [x] **T-34** Tab overflow and narrow toolbar (P1 · F-NARROW) — done
- [x] **T-35** Reading time/ad ratio in tab (P2 · F-TABS) — done

## E — Editor (40)

- [x] **E-01** Toggle edit mode (P0 · F-EDIT) — done
- [x] **E-02** Split layout (P0 · F-NARROW) — done
- [x] **E-03** Editor entry position (P1 · F-EDIT) — done
- [x] **E-04** Edit from here (P1 · F-EDIT) — done
- [x] **E-05** Live preview (P0 · F-BROKEN) — done
- [x] **E-06** Mermaid reuse while typing (P1 · F-MERMAID) — done
- [x] **E-07** Preview follow / scroll sync (P0 · F-EDIT) — done
- [x] **E-08** Save semantics (P0 · F-EDIT) — done
- [x] **E-09** Line endings (P0 · F-EOL) — done
- [x] **E-10** Save when file changed on disk (P0 · F-CONFLICT) — done
- [x] **E-11** Save as for pasted text (P0 · F-PASTE) — done
- [x] **E-12** Save-as collision (P1 · F-PASTE) — done
- [x] **E-13** Discard guard on leaving edit (P0 · F-EDIT) — done
- [x] **E-14** Discard guard on opening other doc (P0 · F-EDIT) — substitute
- [x] **E-15** Reopening edited file (P0 · F-CONFLICT) — done
- [x] **E-16** Recovery draft writing (P0 · F-DRAFT) — done
- [x] **E-17** Draft offering (P0 · F-DRAFT) — done
- [x] **E-18** Draft restore (P0 · F-DRAFT) — done
- [x] **E-19** Draft quotas (P1 · F-DRAFT) — done
- [x] **E-20** Find/replace panel (P0 · F-FIND) — done
- [x] **E-21** Find focus routing (P1 · F-FIND) — done
- [x] **E-22** Format bar (P0 · F-EDIT) — done
- [x] **E-23** Inline formatting (P0 · F-EDIT) — done
- [x] **E-24** Insert link (P1 · F-EDIT) — done
- [x] **E-25** Headings (P0 · F-EDIT) — done
- [x] **E-26** List/quote toggles (P0 · F-EDIT) — done
- [x] **E-27** Block inserts (P1 · F-EDIT) — done
- [x] **E-28** Slash menu (P1 · F-EDIT) — done
- [~] **E-29** Line commands (P1 · F-EDIT) — partial
- [x] **E-30** Comment line (P1 · F-EDIT) — done
- [x] **E-31** Go to line (P1 · F-EDIT) — done
- [x] **E-32** Spellcheck (P1 · F-EDIT) — substitute
- [x] **E-33** Tab / leaving editor (P0 · F-EDIT) — done
- [x] **E-34** Scroll past end (P2 · F-EDIT) — done
- [x] **E-35** Preview double-click → caret (P1 · F-EDIT) — done
- [x] **E-36** Images into editor (P1 · F-IMAGE) — done
- [x] **E-37** Editor font scaling (P2 · F-PREFS) — done
- [x] **E-38** Editor load failure (P1 · F-BROKEN) — n/a
- [x] **E-39** Pasted text in edit mode (P0 · F-PASTE) — done
- [x] **E-40** Undo per tab (P0 · F-TABS) — done

## M — Markdown rendering (34)

- [x] **M-01** CommonMark baseline (P0 · F-BASIC) — done
- [x] **M-02** GitHub line breaks (P0 · F-BASIC) — done
- [x] **M-03** GitHub heading anchors (P0 · F-HEADINGS) — done
- [x] **M-04** Legacy anchors (P1 · F-HEADINGS) — done
- [x] **M-05** Reserved ids (P1 · F-HEADINGS) — done
- [x] **M-06** Tables (P0 · F-TABLE) — done
- [x] **M-07** Task lists (P0 · F-TASK) — done
- [x] **M-08** Strikethrough (P0 · F-BASIC) — done
- [x] **M-09** Autolinks (P0 · F-LINKS) — done
- [x] **M-10** Typographer, minimal (P1 · F-BASIC) — done
- [x] **M-11** Footnotes (P1 · F-FOOTNOTE) — done
- [x] **M-12** YAML front matter (P1 · F-FRONT) — done
- [x] **M-13** Syntax highlighting (P0 · F-CODE) — done
- [x] **M-14** Mermaid (P0 · F-MERMAID) — substitute
- [x] **M-15** Inline math $…$ (P0 · F-MATH) — done
- [x] **M-16** Display $$ blocks (P0 · F-MATH) — done
- [x] **M-17** Inline display $$…$$ in a sentence (P0 · F-MATH) — done
- [x] **M-18** Fenced math (P0 · F-MATH) — done
- [x] **M-19** GitHub alerts (P1 · F-ALERTS) — done
- [x] **M-20** [TOC] (P1 · F-TOC) — done
- [x] **M-21** HTML tagfilter (P0 · F-HTML) — done
- [x] **M-22** Unknown tags hidden (P1 · F-HTML) — done
- [x] **M-23** Sanitization (P0 · F-SECURITY) — done
- [x] **M-24** Relative images (P0 · F-IMAGE) — done
- [x] **M-25** Missing images (P1 · F-IMAGE) — done
- [x] **M-26** srcset parsing (P2 · F-IMAGE) — done
- [x] **M-27** Media playback continuity (P1 · F-MEDIA) — substitute
- [x] **M-28** Subtitles (P2 · F-MEDIA) — substitute
- [x] **M-29** GitHub theme images (P1 · F-IMAGE) — done
- [x] **M-30** Network path refusal (P0 · F-SECURITY) — done
- [x] **M-31** Path scope (P0 · F-SECURITY) — done
- [x] **M-32** Source-line map (P1 · F-LINE) — done
- [x] **M-33** Long formula overflow (P1 · F-MATH) — done
- [x] **M-34** Render error containment (P0 · F-BROKEN) — done

## V — Full-window preview (18)

- [x] **V-01** Opening the preview (P0 · F-PREVIEW) — done
- [x] **V-02** Hover affordance (P2 · F-PREVIEW) — done
- [x] **V-03** Zoom keys (P0 · F-PREVIEW) — done
- [x] **V-04** Wheel zoom (P1 · F-PREVIEW) — done
- [x] **V-05** Drag pan (P1 · F-PREVIEW) — done
- [x] **V-06** Fit/Width/100% semantics (P1 · F-PREVIEW) — done
- [x] **V-07** Preview caption/accessibility (P1 · F-A11Y) — done
- [x] **V-08** Preview copy menu (P1 · F-PREVIEW) — done
- [x] **V-09** PNG rasterization (P1 · F-PREVIEW) — done
- [x] **V-10** Preview close semantics (P1 · F-PREVIEW) — done
- [x] **V-11** Table preview layout (P1 · F-TABLE) — done
- [x] **V-12** Table zoom (P1 · F-TABLE) — done
- [x] **V-13** Table filter (P1 · F-TABLE) — done
- [x] **V-14** Table filter clearing (P1 · F-TABLE) — done
- [x] **V-15** Table sort (P1 · F-TABLE) — done
- [x] **V-16** Table copy as shown (P1 · F-TABLE) — done
- [x] **V-17** Table links (P1 · F-TABLE) — done
- [x] **V-18** Table open mode (P1 · F-TABLE) — done

## X — Share, print and export (16)

- [x] **X-01** Share menu (P0 · F-EXPORT) — done
- [x] **X-02** Copy whole document as rich text (P0 · F-EXPORT) — done
- [x] **X-03** Copy selection as rich text (P1 · F-SELECT) — done
- [x] **X-04** Print (P0 · F-EXPORT) — substitute
- [x] **X-05** Save as PDF (P0 · F-EXPORT) — done
- [x] **X-06** Page break per H1 (P1 · F-EXPORT) — done
- [x] **X-07** HTML export (P1 · F-EXPORT) — done
- [x] **X-08** DOCX export (P1 · F-EXPORT) — done
- [x] **X-09** Export naming (P1 · F-EXPORT) — done
- [x] **X-10** Export ignores reader settings (P0 · F-EXPORT) — done
- [x] **X-11** Print theme swap (P1 · F-EXPORT) — done
- [x] **X-12** Wide-table print fitting (P1 · F-EXPORT) — done
- [x] **X-13** Feedback (P2 · F-EXPORT) — done
- [x] **X-14** Print header text (P1 · F-EXPORT) — done
- [x] **X-15** Export target rules (P1 · F-EXPORT) — substitute
- [x] **X-16** Table copy button (P1 · F-TABLE) — done

## F — File browser and folder search (22)

- [x] **F-01** Root follows document (P0 · F-NESTED) — done
- [x] **F-02** Up arrow (P0 · F-NESTED) — done
- [x] **F-03** Depth and size caps (P0 · F-TREE) — done
- [x] **F-04** Skipped folders (P0 · F-TREE) — done
- [x] **F-05** Only Markdown files (P0 · F-TREE) — done
- [x] **F-06** Sort order (P1 · F-TREE) — done
- [x] **F-07** Collapsed folders (P0 · F-TREE) — done
- [x] **F-08** Watched disk (P0 · F-TREE) — done
- [x] **F-09** Focus preservation (P1 · F-TREE) — done
- [x] **F-10** Roving tab stop (P1 · F-A11Y) — substitute
- [x] **F-11** Tree keyboard (P0 · F-A11Y) — done
- [x] **F-12** File context menu (P1 · F-TREE) — done
- [x] **F-13** Folder search trigger (P0 · F-SEARCH) — done
- [x] **F-14** Folder search algorithm (P0 · F-SEARCH) — done
- [x] **F-15** Search result rendering (P1 · F-SEARCH) — done
- [x] **F-16** Search result opening (P0 · F-SEARCH) — done
- [x] **F-17** Search clear (P1 · F-SEARCH) — done
- [x] **F-18** Pane visibility (P0 · F-PANES) — done
- [x] **F-19** Auto-fold and peek (P0 · F-NARROW) — done
- [x] **F-20** Pane resizing (P1 · F-PANES) — substitute
- [x] **F-21** Panel text size (P1 · F-PANES) — done
- [x] **F-22** Empty folder (P2 · F-TREE) — done

## O — Outline, section line and history (15)

- [x] **O-01** Outline contents (P0 · F-HEADINGS) — done
- [x] **O-02** Outline jump (P0 · F-HEADINGS) — done
- [x] **O-03** Scroll-spy (P0 · F-HEADINGS) — done
- [x] **O-04** Pin behavior (P1 · F-HEADINGS) — done
- [x] **O-05** Outline filter (P1 · F-HEADINGS) — done
- [x] **O-06** Outline visibility (P0 · F-EDIT) — done
- [x] **O-07** Section line conditions (P1 · F-SECTION) — done
- [x] **O-08** Section path (P1 · F-HEADINGS) — done
- [x] **O-09** Section line suppression (P1 · F-SECTION) — substitute
- [x] **O-10** Section line interaction (P2 · F-SECTION) — done
- [x] **O-11** Section line reservation (P2 · F-SECTION) — done
- [x] **O-12** Back/Forward triggers (P0 · F-HISTORY) — done
- [x] **O-13** History semantics (P0 · F-HISTORY) — done
- [~] **O-14** History inputs (P0 · F-HISTORY) — partial
- [x] **O-15** History persistence (P1 · F-HISTORY) — substitute

## C — Command palette (13)

- [x] **C-01** Open keys (P0 · F-PALETTE) — done
- [x] **C-02** What is listed (P0 · F-PALETTE) — done
- [x] **C-03** Fuzzy matching (P0 · F-PALETTE) — done
- [x] **C-04** Prefix modes (P1 · F-PALETTE) — done
- [x] **C-05** Ranking (P1 · F-PALETTE) — done
- [x] **C-06** Activation keys (P0 · F-PALETTE) — done
- [x] **C-07** Search rows (P1 · F-PALETTE) — done
- [x] **C-08** Navigation (P1 · F-PALETTE) — done
- [x] **C-09** Modal key trapping (P1 · F-PALETTE) — done
- [x] **C-10** Focus/selection restoration (P1 · F-PALETTE) — done
- [x] **C-11** Command coverage (P0 · F-PALETTE) — done
- [x] **C-12** Accessibility (P1 · F-A11Y) — substitute
- [x] **C-13** Loading and closing triggers (P1 · F-PALETTE) — substitute

## L — Links and media (15)

- [x] **L-01** In-page anchors (P0 · F-LINKS) — done
- [x] **L-02** External schemes (P0 · F-LINKS) — done
- [x] **L-03** Wiki resolution (P0 · F-LINKS) — done
- [x] **L-04** Same-document link (P0 · F-LINKS) — done
- [x] **L-05** Folder link (P1 · F-NESTED) — done
- [x] **L-06** Image link (P1 · F-IMAGE) — done
- [x] **L-07** Other file link (P0 · F-LINKS) — done
- [x] **L-08** Unresolved link (P0 · F-ERRORS) — done
- [x] **L-09** Pasted text links (P0 · F-PASTE) — done
- [x] **L-10** Network links/media (P0 · F-SECURITY) — done
- [~] **L-11** Link click placement (P0 · F-LINKS) — partial
- [x] **L-12** Image maps and SVG links (P2 · F-HTML) — done
- [x] **L-13** Old anchors (P1 · F-HEADINGS) — done
- [x] **L-14** URL safety (P0 · F-SECURITY) — n/a
- [x] **L-15** URL decoding (P1 · F-LINKS) — done

## S — System, CLI and file integration (27)

- [x] **S-01** CLI grammar (P0 · `lmdv -l 40 --unknown a.md b.md` → a.md line 40.) — done
- [x] **S-02** CLI path handling (P0 · `lmdv .\gone\x.md` shows “Not found: x.md”.) — done
- [x] **S-03** Launch routing (P0 · Open A in window 2, launch A: window 2 front.) — done
- [x] **S-04** Launch during startup (P0 · 10-file burst from cold start opens 10 tabs in one window.) — done
- [x] **S-05** --new-window (P0 · Two `--new-window a.md` produce two windows.) — done
- [x] **S-06** Single instance (P0 · Second launch exits; first handles file.) — done
- [x] **S-07** File associations (P0 · Double-click `.md` opens app; `lmdv` on PATH.) — substitute
- [x] **S-08** New Markdown document (P1 · Right-click New shows localized entry creating empty .md.) — substitute
- [x] **S-09** Recent items / jump list (P2 · Opened file appears in Explorer Recent.) — n/a
- [x] **S-10** Portable mode (P1 · Copy exe+marker to USB: settings in `data`, installed copy unaffected.) — done
- [x] **S-11** Store build detection (P2 · GitHub build has no rate card.) — n/a
- [x] **S-12** Window size defaults (P0 · Window appears only after first render, min size 480.) — done
- [x] **S-13** Window placement (P1 · Cascade and off-screen reset.) — done
- [x] **S-14** Window state persistence (P1 · Move/resize main, restart: restored; other windows not.) — substitute
- [x] **S-15** WebView2 missing (P2 · No runtime: explanation + download link.) — n/a
- [x] **S-16** Native context menu pruning (P2 · Right-click document: app menu; right-click editor: native edit/spelling menu.) — n/a
- [x] **S-17** Browser accelerator keys off (P1 · Ctrl+G does nothing; Ctrl+F opens app find.) — n/a
- [x] **S-18** Navigation guard (P0 · Clicking form submit does not replace app page.) — n/a
- [x] **S-19** Clipboard read (P1 · Copy text: start-page Paste opens it; held clipboard shows busy notice.) — substitute
- [x] **S-20** Document watch (P0 · Atomic-save editor updates view.) — done
- [x] **S-21** Tree watch (P0 · Add file: one tree event; save file: none.) — done
- [x] **S-22** Rename classification (P0 · Rename open file: tab follows; move out: removed.) — done
- [x] **S-23** Save conflict check (P0 · Rename during save: error, not ghost file.) — done
- [x] **S-24** Path identity (P0 · `A.MD` and `a.md` are one tab.) — substitute
- [x] **S-25** Command install (P2 · `lmdv --version`? (no version flag; `lmdv file.md`).) — substitute
- [x] **S-26** Offline/privacy (P0 · Network monitor: idle app makes no connections.) — done
- [x] **S-27** Windows-only capabilities (P2 · Linux build still runs viewer/editor; PDF falls back to print dialog.) — n/a

## W — Window chrome (10)

- [x] **W-01** Draggable toolbar (P0 · Drag empty toolbar moves window; double-click maximizes.) — substitute
- [x] **W-02** Caption buttons (P0 · Buttons match native placement; X prompts.) — substitute
- [x] **W-03** Snap layouts (P2 · Hover Maximize shows Snap flyout on Win11.) — n/a
- [x] **W-04** Max/Restore state (P1 · Maximize: icon becomes restore.) — substitute
- [x] **W-05** Window system menu (P2 · Right-click title bar: native menu.) — n/a
- [x] **W-06** Inactive window look (P2 · Unfocused window caption grey.) — n/a
- [x] **W-07** Always on top (P1 · Pin keeps window above others; restart unpinned.) — done
- [x] **W-08** Native chrome hidden behind preview (P2 · Open preview over caption: no invisible close trap.) — n/a
- [x] **W-09** Window title (P1 · Dirty title begins bullet.) — done
- [x] **W-10** Toolbar layout (P1 · Layout matches fixture screenshot.) — done

## D — Drag and drop (8)

- [x] **D-01** Drop overlay (P1 · Drag file over window: overlay.) — done
- [x] **D-02** File drop (P0 · Drop 3 files on tab 2: tabs inserted there.) — done
- [x] **D-03** Image drop in editor (P1 · Drop PNG while editing inserts image.) — done
- [~] **D-04** Tab drag (P0 · Reorder and tear-off.) — partial
- [x] **D-05** Cross-window ghost (P1 · Ghost follows pointer over other window.) — substitute
- [x] **D-06** Drag-to-scroll (P1 · Middle-drag scrolls.) — done
- [x] **D-07** Window drag by sole tab (P2 · Sole tab drag moves window.) — substitute
- [x] **D-08** Drag cancel (P1 · Esc mid-drag restores order.) — done

## H — Help, start page, notices, rating (13)

- [x] **H-01** Start page (P0 · Empty window shows all elements; long recents scroll.) — done
- [x] **H-02** Recent files (P1 · Recents show time; delete file, reopen home: gone.) — done
- [x] **H-03** Start-page tips (P2 · Text matches.) — done
- [x] **H-04** Help dialog (P0 · F1 shows localized help with version.) — done
- [x] **H-05** Keys overlay (P1 · Overlay keys localized in German.) — done
- [x] **H-06** Notice bar (P0 · Warning with Reload; dismiss; layout no overlap.) — done
- [x] **H-07** Screen-reader notices (P1 · Screen reader announces save/error.) — done
- [x] **H-08** Micro-feedback (P2 · All confirmations visible.) — done
- [x] **H-09** Error card (P1 · Permission-denied file: error card, app alive.) — done
- [x] **H-10** Rate prompt (P2 · Simulate dates: card once, one window, Later returns after 30 d.) — n/a
- [x] **H-11** Quick reference from home (P2 · Empty window `?` opens overlay.) — done
- [x] **H-12** Drop hint (P2 · Overlay visible during drag.) — done
- [x] **H-13** Version display (P2 · Both show same version.) — done

## A — Accessibility (15)

- [x] **A-01** Contrast (P1 · Contrast checker passes 4.5:1 body / 3:1 large.) — done
- [x] **A-02** Visible focus (P0 · Tab through UI: every stop visible.) — done
- [x] **A-03** Names from tooltips (P0 · Screen reader says “Save, Control+S”.) — done
- [x] **A-04** Toolbar and dialogs (P1 · Every control has accessible name.) — done
- [x] **A-05** Tabs (P1 · Screen reader reads tab name + reading time.) — done
- [x] **A-06** File tree (P1 · Screen reader announces “Level 2, 3 of 7, collapsed”.) — substitute
- [x] **A-07** Outline (P1 · Active entry announced/visible.) — done
- [x] **A-08** Menus (P0 · Arrow keys traverse all menus.) — done
- [x] **A-09** Dialogs (P1 · Focus trapped and restored.) — done
- [x] **A-10** Live regions (P1 · Screen reader gets updates without focus move.) — done
- [x] **A-11** Task checkbox names (P1 · Checkbox announced “Install installer QA”.) — done
- [x] **A-12** Zoomable media (P1 · Tab to image, Enter opens preview.) — done
- [x] **A-13** Table preview semantics (P1 · Table region instructions read once.) — done
- [x] **A-14** Reading keys gated (P0 · Typing in find bar does not scroll document.) — done
- [x] **A-15** Reduced motion (P1 · OS animation effects off: no glide/fade.) — done

## I — Internationalization (11)

- [x] **I-01** Six languages (P0 · Every language file compiles; no missing key.) — done
- [x] **I-02** Auto selection (P0 · Windows list [fr-FR,en-US] → French.) — substitute
- [x] **I-03** Override (P0 · Pick Deutsch: UI reloads German; Auto returns system.) — substitute
- [x] **I-04** Reload carries tabs (P1 · Switch language dirty: tab/edits survive.) — substitute
- [x] **I-05** WebView2 language mirror (P2 · Change language, restart: spellcheck dictionary changes.) — n/a
- [x] **I-06** Date/number formatting (P1 · Italian: “1.234”; English: “1,234”.) — done
- [x] **I-07** Key names localized (P1 · German shows “Strg+Umschalt+E”.) — done
- [x] **I-08** English command names as keywords (P1 · Italian UI: typing “print” finds “Stampa”.) — done
- [x] **I-09** Help and export language (P1 · German help and `lang="de"` export.) — done
- [~] **I-10** Localized dialogs and menus (P0 · German close prompt says “Speichern/Nicht speichern/Abbrechen”.) — partial
- [x] **I-11** Seven locale artifacts (P2 · Each artifact exists and is non-empty.) — done

## P — Persistence (20)

- [x] **P-01** reader-prefs (P0 · Set 20 px serif; reload: persists; reset removes key.) — substitute
- [x] **P-02** theme (P0 · Set dark; reload dark; Auto follows OS.) — substitute
- [x] **P-03** lang (P0 · Set it; new window starts Italian.) — substitute
- [x] **P-04** read-positions (P0 · 101 files: oldest position gone.) — substitute
- [x] **P-05** recent-files (P0 · Open 13 files: 12 newest; clear removes key.) — done
- [x] **P-06** drafts (P0 · Kill app dirty; draft survives; save removes.) — substitute
- [x] **P-07** tree-collapsed (P1 · Collapse persists across restart.) — substitute
- [x] **P-08** tree-width / toc-width (P1 · 400 px width survives restart.) — substitute
- [x] **P-09** tree-hidden / toc-hidden (P1 · Hidden pane stays hidden after restart.) — substitute
- [x] **P-10** palette-recent (P2 · Run Print from palette; next open lists it first.) — substitute
- [x] **P-11** editor-spell (P2 · Off survives restart.) — substitute
- [x] **P-12** rate-prompt (P2 · Simulate 5 opens/2 days: due; No thanks never again.) — n/a
- [x] **P-13** nav-history (session) (P1 · Back history survives Ctrl+R? (not full reload; language reload only).) — substitute
- [x] **P-14** reopen-tabs (session) (P1 · Reload keeps tabs once.) — substitute
- [x] **P-15** pasted-drafts-offered (session) (P1 · Crash recovery appears once per window session.) — n/a
- [x] **P-16** Window geometry file (P1 · Geometry restored after restart.) — substitute
- [x] **P-17** WebView2 profile / portable data (P1 · Portable settings travel with folder.) — substitute
- [x] **P-18** ui-language file (P2 · File content equals preference.) — n/a
- [x] **P-19** Draft cleanup points (P0 · After each path, key absent.) — substitute
- [x] **P-20** Cross-window sync (P1 · Change theme in window A: B updates; tabs do not.) — substitute

