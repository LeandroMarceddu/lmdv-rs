# Complete Behavior Specification

Behavioral specification of the application. It is the authoritative behavior reference; `docs/FEATURE_PARITY.md` is the
implementation/status layer on top of it.

- Line references are to the reference sources; paths are relative to the reference repository root.
- Do not edit behavior rows to match our implementation. If `lmdv` intentionally
  differs, record a `substitute` or `accepted-gap` in `docs/FEATURE_PARITY.md`
  and link it here.
- “P0” means required for a faithful Linux daily-driver; “P1” means important but
  isolatable; “P2” means platform-specific, cosmetic, or safe to defer behind a
  documented substitute.
- “Fixture” names are defined in §9.
- `README.md` and `CHANGELOG.md` are cited only where they state behavior that is
  not directly visible in a single source line.

Version facts: the version is `2.2.0` in `package.json:4` and
`src-tauri/Cargo.toml:3`; `tauri.conf.json:4` reads it from `package.json`.
The running version is shown in help (`F1`) and on the start page
(`src/main.ts:4807-4810`).

---

## 1. Executive inventory

The app is a single-process, multi-window Markdown viewer/editor. Every window
holds browser-like tabs. Every document is watched on disk. Every window draws
its own title bar. The application is offline-only and has no telemetry.

| Area | ID prefix | Rows | What it covers |
|---|---|---:|---|
| Reading and viewer | R | 30 | reading column, keys, progress, reading time, find, reload, positions, copy/selection |
| Tabs and windows | T | 35 | tab strip, tab lifecycle, routing, close/open semantics, tab state, cross-window moves |
| Editor | E | 40 | split view, save model, drafts, format bar, slash menu, find/replace, line commands, images, spellcheck |
| Markdown rendering | M | 34 | CommonMark/GFM, anchors, HTML policy, math, tables, tasks, front matter, TOC, media |
| Full-window preview | V | 18 | image/diagram lightbox, zoom/pan/fit, table reader, filter/sort/copy |
| Share, print, export | X | 16 | rich-text copy, print, PDF, HTML, DOCX, page-break option |
| File browser and folder search | F | 22 | tree rules, watch, keyboard tree, search results, pane sizing |
| Outline, section line, history | O | 15 | outline scroll-spy/filter, section path, Back/Forward |
| Command palette | C | 13 | fuzzy search, modes, key handling, ranking, accessibility |
| Links and media | L | 15 | wiki links, fallbacks, images, video/audio, schemes, dark/light pictures |
| System, CLI, file integration | S | 27 | CLI grammar, single instance, routing, association, recent list, watcher, WebView2 guards |
| Window chrome | W | 10 | themed title bar, caption buttons, Snap zones, cascade, always-on-top |
| Drag and drop | D | 8 | files, tabs, windows, drag-to-scroll |
| Help, start page, notices, rating | H | 13 | home, help, keys overlay, notices, saved feedback, Store card |
| Accessibility | A | 15 | names, roles, live regions, focus, reduced motion |
| Internationalization | I | 11 | six languages, auto-selection, override, number/date formats |
| Persistence | P | 20 | every localStorage/sessionStorage key, eviction and write timing |

**Totals:** 342 behavior rows, 80 “subtle behaviors” bullets, 10 fixture
families, 15 CLI forms, 14 localStorage keys, 3 sessionStorage keys,
399 localized string keys × 6 languages.

---

## 2. Behavior tables

### 2.1 Reading and viewer (R)

The viewer is the default and only automatic mode. Opening a document never
enables editing. `Ctrl+E` is the sole entry into edit mode. Documents are
rendered into a single `#article` inside `#content-pane`; the side panes and
toolbar are outside that scroller.

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| R-01 | Viewer-first opening | Every document opens in viewer mode; the editor is instantiated only on the first `Ctrl+E`. No setting makes edit mode the default. | `README.md:74`, `src/main.ts:129`, `src/main.ts:421-475` | P0 | Open any fixture; assert no editor DOM exists and `#article` is visible. | F-BASIC |
| R-02 | Reading column defaults | Defaults: 16 px, sans, line-height 1.7, max width 760 px, article padding 48/32/96 px. At pane width ≤640 px side padding becomes 20 px; at ≤420 px 14 px and top padding 24 px. | `src/styles/base.css:54-58`, `src/styles/layout.css:181-220` | P0 | Snapshot at 1280 and 480 px; measure column and padding. | F-BASIC |
| R-03 | Reading progress hairline | A 2 px accent line at the very top of `#content-pane`, width = `scrollTop / (scrollHeight - clientHeight)`; hidden when max scroll ≤24 px, when editing, on the start page, or over an error card. | `index.html:74`, `src/main.ts:3742-3752` | P1 | Scroll a long document; width follows monotonically; hidden on a short doc. | F-LONG |
| R-04 | Reading time | Words/min = 220; CJK characters/min = 500; `ceil(words/220 + cjk/500)`, minimum 1. `msg.words` tooltip counts words + CJK characters. CJK regex covers Han, Hiragana, Katakana, Hangul. | `src/main.ts:3766-3784` | P1 | Fixture with 440 words → “2 min read”; tooltip “440 words”. | F-LONG |
| R-05 | Reading time placement | In the tab in front, after its name. With one tab: full form “{n} min read”; with several: “{n} min”. Hidden while editing and while the strip is full; the hover card still shows it. Screen readers receive it as `aria-description`. | `src/tabs/strip.ts:306-343`, `src/locales/en.ts:28-30,259` | P1 | 1-tab and 3-tab windows; shrink window until text disappears but card retains it. | F-BASIC |
| R-06 | `j` / `k` scroll | Scrolls 80 px per press. Rapid presses accumulate from the previous target within 400 ms, not from the in-flight smooth-scroll position. Held key repeats use instant scroll. | `src/main.ts:3831-3845` | P0 | 5 quick `j` presses move ~400 px; held `j` is immediate. | F-LONG |
| R-07 | `Space` / `Shift+Space` paging | Quick-reference and help advertise page down/up. No page-key handler exists in `handleReadingKey`; behavior depends on the native scroller receiving the key (see §3.1). | `index.html:258`, `src/locales/help/en.html:91`, `src/main.ts:3848-3881` | P1 | Verify in a reference build whether Space scrolls `#content-pane`; record exact behavior. | F-LONG |
| R-08 | `.` / `,` heading steps | Jumps to next/previous heading through the outline machinery; glide, outline highlight and landing flash apply. While a jump is pinned, steps use the pinned index, so trailing headings that cannot reach the top still get visited. | `src/main.ts:3858-3861`, `src/toc.ts:159-181` | P0 | From top, `.` visits every heading in order, including the last. | F-HEADINGS |
| R-09 | `g g` / `G` | `G` scrolls to document end; two `g` presses within 500 ms scroll to top. Smooth unless reduced motion. A single `g` does nothing after the 500 ms window. | `src/main.ts:3862-3874` | P0 | `g`,`g` → top; `G` → bottom; single `g` → no move. | F-LONG |
| R-10 | `n` / `N` find stepping | Only acts when the reading find bar has matches; steps forward/back cyclically and scrolls to the current match. With no find bar open, nothing happens. | `src/main.ts:3875-3878`, `src/search.ts:90-96` | P1 | Open find, type term, `n`/`N` cycles and counter changes. | F-FIND |
| R-11 | `?` keys overlay | Toggles a modal `<dialog>` listing Reading, App, Tabs and Editing groups. Works even with no document. `?` again or `Esc` closes; backdrop click closes. While open, most keys are ignored. | `index.html:248-315`, `src/main.ts:3884-3887`, `src/main.ts:4588-4593` | P1 | `?` opens; `?` closes; `Esc` closes; focus stays trapped. | F-BASIC |
| R-12 | Per-file reading position | The position is saved per canonical path in localStorage after 300 ms without scrolling. It is captured as the nearest heading above the viewport + offset, or raw `scrollTop` when no heading is above. | `src/main.ts:3754-3763`, `src/state.ts:35-46`, `src/positions.ts:35-53` | P0 | Scroll halfway, close, reopen: same heading at top. | F-LONG |
| R-13 | Position survives edits above | Because it stores a heading id and offset, inserting text above the remembered heading does not shift the reopened position relative to that heading. | `src/state.ts:35-46`, `src/positions.ts:1-10` | P0 | Remember at “## B”; add 30 lines above; reopen still at B. | F-LONG |
| R-14 | Top means forgotten | If captured anchor has no heading id and offset ≤0, the stored position is deleted. Scrolling back to the top therefore forgets the place. | `src/positions.ts:35-45` | P1 | Scroll to top, reopen: starts at top and entry is gone. | F-LONG |
| R-15 | Missing anchor falls back to top | A remembered heading id that no longer exists is not honored; document opens at top. Legacy pre-1.15.1 heading ids are searched in `data-legacy-id` too. | `src/main.ts:3729-3740`, `src/state.ts:14-33` | P1 | Rename the remembered heading; reopen starts at top. | F-HEADINGS |
| R-16 | Find bar open | `Ctrl+F` opens a floating bar, focuses and selects the input, refreshes matches immediately. While editing, `Ctrl+F` opens CodeMirror’s panel instead (E-20). | `src/search.ts:35-40`, `src/main.ts:4017-4022` | P0 | `Ctrl+F` focuses input; typing updates counter. | F-FIND |
| R-17 | Find prefill from selection | `Ctrl+F` / `Ctrl+Shift+F` use the first line of a non-collapsed selection inside the article, trimmed, capped at 100 chars. The context-menu “Find in document” uses the same query. | `src/main.ts:3114-3120`, `src/main.ts:4017-4022`, `src/main.ts:4641-4647` | P1 | Select “Akeneo”, `Ctrl+F`: input contains it. | F-FIND |
| R-18 | Find matching semantics | Case-insensitive plain substring search across text nodes in document order; no regex, no accent folding, no whole-word option. Count shows `current/total` or `0/0`. Enter/Shift+Enter and buttons wrap. | `src/search.ts:98-128`, `index.html:76-88` | P0 | “akeneo” finds 2 of 3 matches in fixture; counter 1/3. | F-FIND |
| R-19 | Find scrolling | Current match is scrolled to the middle if it is within 60 px of the top edge or 60 px from bottom; the top band expands to clear the find bar and notice bar. A match inside a horizontally scrolled table/code block first centers that inner scroller. | `src/search.ts:130-160` | P1 | Match at far right of wide table becomes visible. | F-TABLE |
| R-20 | F5 / Ctrl+R | Re-reads the current file from disk, preserving reading position. In edit mode it does nothing when the buffer is dirty. On pasted text it does nothing. It never reloads the WebView. | `src/main.ts:3443-3451`, `src/main.ts:4556-4559` | P0 | Modify file externally, F5: new text, same place. | F-RELOAD |
| R-21 | Live reload without edits | A disk change re-reads the file automatically and restores the anchor. Only a different text counts as a change; an mtime-only touch updates the baseline silently. | `src/main.ts:4896-4923`, `src/main.ts:373-383` | P0 | Append paragraph externally: view updates and reading place holds. | F-RELOAD |
| R-22 | Live reload with unsaved edits | A sticky warning “File changed on disk. Saving will overwrite it.” appears with **Reload**. The buffer is never overwritten automatically. Reload explicitly discards edits. | `src/main.ts:385-390`, `src/main.ts:4916-4922` | P0 | Edit, change file externally: warning, buffer intact; Reload shows disk. | F-CONFLICT |
| R-23 | Rename while open | A watcher rename follows the file: current path mutated in place, recents/positions/drafts/history renamed, tab updated, tree re-highlighted, notice “Renamed to …”. Unsaved edits survive. | `src/main.ts:1184-1208` | P0 | Rename file in file manager: title changes, edits survive. | F-RELOAD |
| R-24 | Delete/move-away while open | Text stays on screen, a sticky notice ““name” is no longer on disk…” offers **Save again**. Saving then creates the file again. A later reappearance clears the notice/tree refreshes. | `src/main.ts:1210-1250`, `src-tauri/src/commands.rs:269-297` | P0 | Delete open file: text stays; Save again recreates. | F-RELOAD |
| R-25 | `Ctrl+A` scope | While reading, selects only the article’s children, not toolbar/panes. In a text field/editor it is the field’s native select-all. | `src/main.ts:4659-4664` | P1 | `Ctrl+A`, copy: copied text starts at first heading, no UI labels. | F-BASIC |
| R-26 | Selection copy | `Ctrl+C` on an article selection writes Word-friendly HTML for the selected range plus plain text; fallback to plain text if clipboard HTML fails. Only when not editing and focus is not a typing target. | `src/main.ts:4326-4334`, `src/main.ts:3122-3131`, `src/share.ts:541-577` | P1 | Copy bold phrase; paste into Word keeps bold; Notepad gets text. | F-SELECT |
| R-27 | Double-click while reading | Native word selection; no editor caret. (While editing it changes to “put caret there”, E-28.) | `src/main.ts:4766-4795` | P0 | Double-click word: selected, no mode change. | F-BASIC |
| R-28 | Drag to scroll | Middle button drags anywhere in the pane; left button only when pressed exactly on `#content-pane` (the empty margin). A drag starts after 3 px. A middle click without drag on a link opens it in a new tab. Autoscroll is suppressed. | `src/main.ts:3484-3542` | P1 | Middle-drag moves document; middle-click link opens background tab. | F-LONG |
| R-29 | Empty/error state persistence | Sticky notices survive ordinary preview refreshes; force-hide only on document change, save, reload, explicit dismiss. A notice from a previous document is removed on opening another. | `src/notices.ts:58-102`, `src/main.ts:252-257` | P1 | “Not found” then open a good doc: notice disappears. | F-ERRORS |
| R-30 | Reading settings isolation | Reader font/size/width/line-height apply only on screen; print and all exports force 16 px sans, 1.7 line-height, standard width. | `src/styles/print.css:37-48`, `README.md:203-206` | P0 | Set 22 px serif + full width, print preview stays 16 px sans. | F-PREFS |

### 2.2 Tabs and windows (T)

All windows share one process. Each window has one tab list, one front tab, one
set of panes and one Back/Forward history. A tab carries a source: `start`,
`file`, or `pasted`. A start page is replaced when a document opens there.

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| T-01 | Tab strip | Tabs live in the toolbar where the file name used to be; each tab is as wide as its name down to a minimum, has icon, title, optional reading time, close button/dot. | `src/tabs/strip.ts:92-178`, `src/styles/tabs.css:23-100` | P0 | Two documents show two tabs with icons. | F-BASIC |
| T-02 | Tab names | File tabs show the file name **without extension** on the strip and the full name as accessible label/card. Pasted tabs show their title (or “Pasted text”); start tabs “New tab”. Icons: document, clipboard, home. | `src/tabs/strip.ts:62-72,180-200`, `src/locales/en.ts:17,379` | P0 | `notes.md` renders as “notes”; pasted `# Budget` renders “Budget”. | F-PASTE |
| T-03 | Dirty indicator | Unsaved edits replace the close × with an orange dot. After `Ctrl+S` it becomes a check for 1.4 s; typing again restores the dot immediately. Screen reader hears “Saved”. | `src/tabs/strip.ts:244-258`, `src/main.ts:1063-1079` | P0 | Type → dot; save → check → dot gone. | F-EDIT |
| T-04 | New tab | `Ctrl+T`, the `+` after the last tab, tab menu “New tab”, and palette create a start-page tab. `Ctrl+T` appends at the end; the menu inserts after the tab. | `src/main.ts:1866-1873`, `src/main.ts:2649-2650`, `src/tabs/strip.ts:768-793` | P0 | `Ctrl+T` adds start page at end and brings it front. | F-BASIC |
| T-05 | Close tab | `Ctrl+W` / `Ctrl+F4`, × button, middle click, Delete on a focused tab. Dirty tabs ask Save / Don’t save / Cancel. The neighbor to the right comes forward, else the left; the last document leaves a start page. | `src/main.ts:2495-2523`, `src/main.ts:2527-2551`, `src/tabs/strip.ts:462-474` | P0 | Close middle of 3 tabs: right neighbor front; dirty tab prompts. | F-TABS |
| T-06 | Close begin page/window | `Ctrl+W` on a lone start page closes the window. Last tab of any kind always leaves a start page unless it is itself a start page. | `src/main.ts:2498-2501`, `src/main.ts:2542-2546` | P0 | Start page only + `Ctrl+W` closes window. | F-TABS |
| T-07 | Reopen closed tab | `Ctrl+Shift+T` reopens the most recently closed tab at its old index and reading place, up to 25 closed tabs. If the file/start page is open again, its tab is activated instead. Pasted text comes back as originally pasted, with same id. | `src/main.ts:2567-2588`, `src/tabs/session.ts:84-86,165-169` | P0 | Close tab at index 1, reopen: same index and scroll. | F-TABS |
| T-08 | Next/previous tab | `Ctrl+Tab` / `Ctrl+Shift+Tab`, and `Ctrl+PgDn` / `Ctrl+PgUp`, wrap around; held Ctrl+Tab repeats. | `src/main.ts:2638-2667` | P0 | Cycles forward and wraps last→first. | F-TABS |
| T-09 | Tab by number | `Ctrl+1`…`Ctrl+8` select the nth live tab; `Ctrl+9` the last. Digits are read by physical `Digit` code (AZERTY works); numpad only when Num Lock makes it a digit; fallback keyCode for environments with no code. Editor takes `Ctrl+1`–`Ctrl+3` for headings first. | `src/main.ts:2651-2659`, `src/editor.ts:407-423` | P1 | On AZERTY, Ctrl+& focuses tab 1; editor Ctrl+2 = heading 2. | F-TABS |
| T-10 | Move tab by keyboard | `Ctrl+Shift+PgUp` / `Ctrl+Shift+PgDn` move the front tab left/right one slot, clamped at ends. | `src/main.ts:2604-2613,2645-2648` | P1 | Move tab 2 left; order swaps. | F-TABS |
| T-11 | One file, one tab per window | Opening a file already in a tab activates that tab (and may jump to `--line`/anchor). Opening in background pulses the existing tab instead of duplicating. A file may be open in two different windows. | `src/main.ts:1784-1801,1819-1830`, `src/tabs/session.ts:132-134` | P0 | Double-open file: same tab front, count unchanged. | F-TABS |
| T-12 | Click-open placement | Plain click opens in the tab in front; `Ctrl+click` or middle click opens a background tab; `Ctrl+Shift+click` opens a foreground tab; `Shift+click` a new window. This applies to tree, recents, search hits and document links. | `src/where.ts:5-16`, `src/main.ts:1724-1734` | P0 | Four gestures yield four placements. | F-TABS |
| T-13 | Start-page replacement | A document opened in a window whose front tab is a start page takes that tab’s place (start page marked closing). In background, it does not. Pasted text in front is never replaced; the file gets a new tab. | `src/main.ts:1837-1850`, `src/main.ts:1895-1904` | P0 | Start page + open file: one tab, not two. | F-BASIC |
| T-14 | Background insertion order | Background tabs open after the tab they were opened from, and successive background tabs queue after each other; the queue resets when another tab comes forward. Files opened from outside go at the end. Files dropped on the strip go at the drop index. | `src/main.ts:1814-1851` | P1 | Ctrl+click A then B: order front, A, B. | F-TABS |
| T-15 | Background tab pulse | A tab opened behind lights up briefly and is revealed in the strip, so the eye finds it. | `src/tabs/strip.ts:236-242` | P2 | Background open flashes. | F-TABS |
| T-16 | Tab hover card | After 550 ms on a tab (immediately when moving tab→tab), a floating card shows full file name, folder, and either “Unsaved changes” or reading time. For pasted text it says “Pasted text, in no file yet”; for start “Start page with the recent files”. | `src/tabs/strip.ts:817-856`, `src/locales/en.ts:26-27` | P1 | Hover 1 s: card with path and “4 min read”. | F-TABS |
| T-17 | Overflow tab list | When tabs overflow, a ⌄ button lists every tab (active marked ●, folder as hint) plus “New tab”. Wheel over the strip scrolls horizontally when content overflows and vertical delta dominates. Left/right fade masks appear. | `src/tabs/strip.ts:294-304,488-500,796-815` | P1 | 12 tabs → ⌄ visible; wheel scrolls strip. | F-TABS |
| T-18 | Tab context menu | New tab; Move to a new window (if >1); Copy path / Show in File Explorer (files only); Save as… (pasted only); Close; Close other tabs; Close tabs to the right (if any); Reopen closed tab. | `src/tabs/strip.ts:768-787` | P1 | Every item present/absent correctly per tab kind/position. | F-TABS |
| T-19 | Tab drag reorder | Dragging 5 px starts reorder. The dragged tab translates within strip bounds; a neighbor yields once the dragged tab’s edge passes that neighbor’s midpoint. Drop animates into place; `Esc` cancels. Does not start while PDF/export busy. | `src/tabs/strip.ts:547-686` | P0 | Drag tab 1 past tab 2’s midpoint: order swaps; Esc restores. | F-DRAG |
| T-20 | Tab tear-off | Pulling a tab ≥36 px above/below the strip or past the window side creates a window whose outer top-left puts the grabbed point under the pointer. The new window follows the pointer until mouse-up. | `src/tabs/strip.ts:542-566,635-642`, `src-tauri/src/drag.rs:179-208` | P1 | Pull tab off: new window appears under cursor. | F-DRAG |
| T-21 | Drag only tab → window | Dragging the sole tab starts a native window move; dropping it on another app window’s strip merges. | `src/tabs/strip.ts:552-558`, `src-tauri/src/drag.rs:156-177` | P1 | Drag sole tab onto another window strip: one window remains. | F-DRAG |
| T-22 | Drop tab onto another window | Target shows a ghost at insertion index; accepts unless closing or same file already open with unsaved edits on either side. A same-file clean merge just activates existing tab. Source tab disappears; 15 s timeout restores it if no answer. | `src/main.ts:2784-2850,2742-2782`, `src-tauri/src/drag.rs:56-104` | P1 | Move tab with edits: edits/undo survive; dirty duplicate refused. | F-DRAG |
| T-23 | Close others / right | “Close other tabs” activates the kept tab, then closes others sequentially; a Cancel stops the sequence. “Close tabs to the right” closes from right to left likewise. | `src/main.ts:2553-2565` | P1 | Dirty second tab + Cancel leaves rest open. | F-TABS |
| T-24 | Window close question | Closing a window asks once if any tab has unsaved edits **or is pasted text, changed or not**. Question names files with `Intl.ListFormat`; buttons Save all / Don’t save / Cancel. Save runs per tab and aborts on failure. | `src/main.ts:2412-2438,2496-2523,4844-4881` | P0 | Window with one dirty file + one untouched pasted tab asks naming both. | F-TABS |
| T-25 | Close request routing | While the close question is open, `set_closing(true)` tells Rust to route external launches to another window. A second close request is prevented. Cancel restores routing and keeps window. | `src/main.ts:4843-4881`, `src-tauri/src/commands.rs:192-199` | P1 | Click X twice while prompt open: one prompt. | F-TABS |
| T-26 | Tab state retention | Each tab keeps scroll anchor, rendered fragment (up to 6 recent), scheme at render, editing state, editor snapshot (text/undo/caret/scroll), Back/Forward, find bar, outline filter, sticky notice, error card, `docGone`, tree root, stale-preview flag. | `src/tabs/session.ts:15-37`, `src/main.ts:2071-2133,2137-2221` | P0 | A in tab 1, scroll, edit, find, filter; B; back to A: all restored. | F-TABS |
| T-27 | Rendered-view cache | Only the 6 most recently shown background tabs keep their rendered DOM; older ones drop the fragment and re-render when activated. | `src/main.ts:1737,2056`, `src/tabs/session.ts:171-180` | P1 | Open 8 docs, return to oldest: Mermaid redraws. | F-TABS |
| T-28 | State carried across reload | Language change writes every tab’s transfer to `sessionStorage`, reloads, and rebuilds tabs with scroll, edits/undo, nav, find, outline filter and pasted text. A start page comes back as an empty start page (its rendered state is not carried). | `src/main.ts:2930-2959,4457-4478` | P1 | Change language with dirty tab: tab and edits survive. | F-I18N |
| T-29 | Close-key precedence | `Ctrl+W`/`Ctrl+F4` first closes palette, then preview, then help/keys, then cancels tab drag, then closes the tab. It does not close the window until only a start page remains. | `src/main.ts:2615-2632` | P1 | Ctrl+W with preview open closes preview first. | F-PREVIEW |
| T-30 | Window close key | `Ctrl+Shift+W` requests window close (same unsaved prompt). Held key acts once. | `src/main.ts:3467-3469,4544-4547` | P0 | Ctrl+Shift+W opens the close question. | F-TABS |
| T-31 | New window | `Ctrl+Shift+N` opens an empty window on the start page. New windows cascade +32/+32 from the previous window created within 3 s, else from the current one; maximized source yields maximized child; minimized source uses default size centered. | `src/main.ts:4626-4628`, `src-tauri/src/windowing.rs:20-23,263-379` | P1 | Three Ctrl+Shift+N: three offset windows. | F-WINDOW |
| T-32 | Multi-select open | `Ctrl+O` accepts multiple files; first opens front, rest background in selection order, each a tab. | `src/main.ts:4001-4012` | P1 | Select 3 files: one front, two background. | F-BASIC |
| T-33 | Tabs across windows | Each window has independent tabs, front tab, history and panes. Launch routing picks by tab contents, start-page idleness, then use order (S-03). | `src/main.ts:1722-1737`, `src-tauri/src/windowing.rs:80-111` | P0 | File already in window B opens/activates there. | F-WINDOW |
| T-34 | Tab overflow and narrow toolbar | Under 720 px toolbar separators disappear; under 640 px Open and Help buttons disappear; under 560 px the `+` disappears (Ctrl+T and tab list remain). | `src/styles/tabs.css:440-470` | P1 | Resize to 500 px: buttons hidden, keys still work. | F-NARROW |
| T-35 | Reading time/ad ratio in tab | Tab grows for reading time only if strip has spare room; if any tab is squeezed the time is dropped. A tab in hand or widths frozen by a pointer hides it temporarily. | `src/tabs/strip.ts:306-367` | P2 | Fill tabs: active time disappears, card retains. | F-TABS |

### 2.3 Editor (E)

Edit mode is a split view: CodeMirror 6 on the left, live preview on the right.
The editor source is plain Markdown at all times. There is no rich-text model.

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| E-01 | Toggle edit mode | `Ctrl+E` or pencil. Button title changes “Edit (Ctrl+E)” ↔ “Back to reading (Ctrl+E)”, icon pencil ↔ open book. Editor fades in. Entering closes reading find bar and section line. | `src/main.ts:421-508`, `src/locales/en.ts:6-7` | P0 | Ctrl+E twice returns to identical reading view. | F-EDIT |
| E-02 | Split layout | Reading panes hidden while editing; two equal columns: editor left, preview right. At shell width <760 px only editor shows; preview returns when widened or reading. | `src/styles/layout.css:105-230`, `src/panes.ts:31-33` | P0 | 1200 px split; 600 px editor only. | F-NARROW |
| E-03 | Editor entry position | With no explicit line, editor scrolls to the first preview block at/above the reading top (the section being read); caret placed there. | `src/main.ts:444-475` | P1 | Read section 3, Ctrl+E: editor at section 3. | F-EDIT |
| E-04 | Edit from here | Right-click menu “Edit from here” enters edit mode at the 0-based source line under the pointer, or moves caret there if already editing. | `src/main.ts:477-484,3293-3298` | P1 | Right-click paragraph → Edit from here: caret in paragraph. | F-EDIT |
| E-05 | Live preview | Renders 200 ms after last keystroke. Fallible work (markdown-it, sanitize, image resolution) happens before DOM commit; on failure old preview stays and dismissible error notice appears. Superseded renders are discarded by generation counter. | `src/main.ts:700-775` | P0 | Type invalid Mermaid: previous preview remains + error notice. | F-BROKEN |
| E-06 | Mermaid reuse while typing | Existing rendered diagrams keyed by source; unchanged diagrams reuse inner HTML so they do not flicker on each preview refresh. | `src/main.ts:722-745` | P1 | Type prose between two diagrams: diagrams do not flash. | F-MERMAID |
| E-07 | Preview follow / scroll sync | Editor scroll drives preview one-way; preview manual scroll is free until editor moves again. Mapping interpolates between source-line anchors. On edits/caret moves, the caret’s whole top-level block is revealed if it fits, else innermost anchored part. Table header cover and section line offsets are compensated. | `src/main.ts:777-914` | P0 | Type at bottom: preview follows; scroll preview manually: stays. | F-EDIT |
| E-08 | Save semantics | `Ctrl+S` or save button. Only explicit save writes (the deliberate task-checkbox write is the sole exception, M-07). Dirty = buffer differs from last saved/baseline. On save: write file, update baseline, remove draft, clear notice, flash check, announce “Saved”. | `src/main.ts:943-968`, `src/editor.ts:335-342,587-598` | P0 | No disk write before Ctrl+S; one write on Ctrl+S. | F-EDIT |
| E-09 | Line endings | Editor normalizes to LF internally; file’s original EOL detected on load (`\r\n` if content contains it). On write, LF is converted back to CRLF when needed. A UTF-8 BOM is stripped on read and never re-added. | `src/editor.ts:67-70,505-537,577-581`, `src-tauri/src/commands.rs:235-249`, `CHANGELOG.md:779-781` | P0 | CRLF fixture saves as CRLF; BOM fixture saves without BOM. | F-EOL |
| E-10 | Save when file changed on disk | `write_markdown` compares mtime; on mismatch returns `conflict`. UI asks “The file changed on disk… Overwrite it with your version?” with **Overwrite** / **Keep theirs**. Overwrite rewrites without guard. | `src/main.ts:918-941`, `src-tauri/src/commands.rs:252-298` | P0 | External write then Ctrl+S: prompt; Keep theirs keeps disk. | F-CONFLICT |
| E-11 | Save as for pasted text | `Ctrl+S` on pasted text always opens Save dialog, even unchanged/read-only. Proposed name from title made file-system safe, `.md`; folder = paste home (pane root) else Documents. After write, tab becomes that file, draft removed, history renamed, file read back. | `src/main.ts:943-949,976-1061` | P0 | Pasted `# Budget` → default `Budget.md`; becomes file tab. | F-PASTE |
| E-12 | Save-as collision | If chosen file is open clean in another tab, that tab is released/merged; if dirty, warning ““name” is open in another tab with unsaved changes…” and save aborts. Canonicalized path used to detect. | `src/main.ts:1003-1013,1042-1049` | P1 | Save pasted over dirty open file: warning, no overwrite. | F-PASTE |
| E-13 | Discard guard on leaving edit | Leaving edit mode with dirty buffer asks “You have unsaved changes. Discard them?” with **Discard** / **Cancel**; on discard, saved version reloaded into preview and draft removed. | `src/main.ts:421-440,587-598` | P0 | Dirty + Ctrl+E: prompt; cancel stays editing. | F-EDIT |
| E-14 | Discard guard on opening other doc | Opening another document (tree, recent, launch, drop) while editing dirty asks the same discard question before switching. Reopening the **same** file is special: see E-15. | `src/main.ts:217-231` | P0 | Dirty + open other: prompt; cancel keeps current. | F-EDIT |
| E-15 | Reopening edited file | Same file reopened from outside/tree while editing does not read disk over the buffer. If disk differs, shows file-changed notice (dirty) or reloads (clean); honours `--line`/anchor by scrolling both panes. | `src/main.ts:343-366` | P0 | Double-click edited file: buffer stays, no preview swap. | F-CONFLICT |
| E-16 | Recovery draft writing | While dirty, draft written to localStorage 1 s after last keystroke; clean buffer removes it. Tab going behind flushes immediately. Base is the file’s LF-normalized current content. | `src/main.ts:602-634`, `src/drafts.ts:50-61` | P0 | Type, wait 1 s: draft exists; save removes. | F-DRAFT |
| E-17 | Draft offering | On opening a file, a stored draft is offered with **Restore**; never auto-applied. Dismissing deletes it. If draft equals disk text, it is silently removed. If another window is editing the file (BroadcastChannel, 250 ms), no offer. | `src/main.ts:668-698,636-666` | P0 | Kill app dirty, reopen: notice Restore; dismiss removes. | F-DRAFT |
| E-18 | Draft restore | Restore enters edit mode and loads draft text into buffer; lastSaved remains the file, so buffer is dirty; editor’s change callbacks update preview/draft. File still untouched until save. | `src/main.ts:681-697`, `src/editor.ts:539-549` | P0 | Restore then check file unchanged on disk; buffer dirty. | F-DRAFT |
| E-19 | Draft quotas | Max 20 drafts, max 2,000,000 characters total across `text`+`base`; evicts oldest. localStorage quota error drops oldest and retries once, then gives up silently. | `src/drafts.ts:13-15,50-61` | P1 | 21 drafts: oldest gone; quota failure does not crash. | F-DRAFT |
| E-20 | Find/replace panel | `Ctrl+F` opens find-only row; `Ctrl+H` opens replace row. Panel is CodeMirror’s; options match case, regexp, whole word; F3/Shift+F3 step; localized strings; query and options survive face switches. | `src/editCommands.ts:16-56`, `src/editor.ts:385-423,556-575` | P0 | Ctrl+F row hides replace; Ctrl+H same query shows replace. | F-FIND |
| E-21 | Find focus routing | `Ctrl+F` while editing always opens editor find, even if focus is in preview/toolbar. `Ctrl+H` only edits when editing; palette’s Find command opens editor panel with query. | `src/main.ts:4017-4022,4099-4102`, `src/main.ts:4645-4652` | P1 | Click preview, Ctrl+F: editor panel opens. | F-FIND |
| E-22 | Format bar | Above editor: heading picker (Text/H1/H2/H3), bold/italic/strike/code/link, bullet/ordered/task/quote, Insert menu, Tools menu, hint “Type / for blocks”. Buttons show pressed state from syntax tree; heading picker shows current level. | `src/formatBar.ts:8-202`, `src/locales/en.ts:292-311` | P0 | Caret in bold text: Bold pressed. | F-EDIT |
| E-23 | Inline formatting | Bold `**`, italic `*`, strike `~~`, code `` ` `` toggle around each selection or word under caret. Surrounding whitespace excluded. Existing outside markers removed; markers inside selection stripped. Italic vs bold distinguished by run parity/count. | `src/editCommands.ts:58-138,397-402` | P0 | Toggle bold twice restores exact text; whitespace stays outside. | F-EDIT |
| E-24 | Insert link | `Ctrl+K`; bare URL selection becomes `[](url)` with caret in label; text selection becomes `[text](url)` with URL selected; no selection inserts `[link text](url)` with “link text” selected. | `src/editCommands.ts:140-166` | P1 | Select URL → Ctrl+K; label empty, caret inside. | F-EDIT |
| E-25 | Headings | `Ctrl+1`/`2`/`3` set ATX heading level on every selected line; applying same level again removes heading. Command “Normal text” removes to paragraph. The toggle edits ATX headings; Setext headings are recognized only for the format bar’s active state. | `src/editCommands.ts:243-258,403-406` | P0 | H2 on H2 line → paragraph. | F-EDIT |
| E-26 | List/quote toggles | Bullet, ordered, task, quote toggle line prefixes on selected non-empty lines (all-empty selection toggles empty lines). Ordered numbering restarts 1..n. Quote toggles `> `. Preserves indentation. | `src/editCommands.ts:195-241,407-410` | P0 | H2 list → bullets; same command removes. | F-EDIT |
| E-27 | Block inserts | Table, Mermaid, math, code block, note/tip/warning callout, divider insert at line start/end with blank-line separation; placeholder is selected. Empty line is replaced; otherwise insert after current line. | `src/editCommands.ts:260-302,411-427` | P1 | `/table` in empty line inserts table and selects “Column”. | F-EDIT |
| E-28 | Slash menu | Typing `/` at line start (optional indentation) opens completions filtered by start of any label/keyword word. Suppressed inside fenced code, indented code, HTML blocks. Choosing a block replaces the `/query` and runs insert. | `src/editor.ts:277-319`, `src/editCommands.ts:11-12` | P1 | `/t` lists Table, Task list, Tip; not “Quote”. | F-EDIT |
| E-29 | Line commands | `Alt+↑`/`Alt+↓` move line; `Shift+Alt+↓` duplicate; `Ctrl+Shift+K` delete; `Ctrl+/` comment; Tools menu exposes all. `Alt+L` select line; `Ctrl+D` next occurrence; `Ctrl+Alt+↑/↓` add cursor; these rely on CodeMirror defaults plus custom line commands. | `src/editCommands.ts:429-438`, `src/editor.ts:407-423`, `README.md:262-267` | P1 | Move/duplicate/delete act on all selected lines. | F-EDIT |
| E-30 | Comment line | Wraps selected lines in `<!-- -->`; inside a fenced code block uses the language’s comment syntax. Second invocation removes it. Shortcut is the key that types `/` (Ctrl+Shift+7 on Italian/German layouts). | `src/editCommands.ts:434-437`, `src/locales/help/en.html:225-230` | P1 | Comment a paragraph; reading hides it; uncomment restores. | F-EDIT |
| E-31 | Go to line | `Ctrl+Alt+G` opens CodeMirror’s go-to-line dialog; accepts line numbers, +N/−N, percentages and `line:col` forms supported by CodeMirror; localized. | `src/editor.ts:230-253,363-383` | P1 | Ctrl+Alt+G “40” puts caret at line 40. | F-EDIT |
| E-32 | Spellcheck | On by default; uses WebView2/Windows dictionary of interface language. Toggle in Tools persists `editor-spell=0`; a language change applies to spelling only on next start. Attribute `spellcheck` on editor content, `autocorrect/autocapitalize` off. | `src/editCommands.ts:352-368`, `src/locales/help/en.html:230-233` | P1 | Off persists; red underlines disappear after reload. | F-EDIT |
| E-33 | Tab / leaving editor | Tab always indents, never toggles focus mode (Ctrl+M binding removed). `Esc` then `Tab` within 2 s leaves editor; with selection/panel/menu open press Esc twice first. | `src/editor.ts:271-275`, `src/locales/help/en.html:235-239` | P0 | Tab indents; Esc,Tab moves focus to preview. | F-EDIT |
| E-34 | Scroll past end | Editor can scroll past last line so preview can follow to document end. | `src/editor.ts:399-400`, `CHANGELOG.md:556-557` | P2 | Scroll to bottom of editor beyond text. | F-EDIT |
| E-35 | Preview double-click → caret | While editing, double-clicking a preview block maps to its source line and selects the word if present. Handles repeated word occurrences via preceding preview text. Word match tries exact whole word, any case, inside longer word. Code spans/links/images/HTML offsets are blanked to keep counts. Caret lands at block’s preview height, editor focused. Interactive elements (links, buttons, inputs, images, summaries, diagrams) keep their own clicks. | `src/editor.ts:677-806`, `src/main.ts:4766-4795` | P1 | Double-click second “budget” in preview selects second source occurrence. | F-EDIT |
| E-36 | Images into editor | Pasting an image saves `assets/pasted-<ms>.<ext>`; dropping an image copies it into `assets/<stem>.<ext>`, uniquified `-1`, `-2`. Inserts `![](assets/…)` with URL-encoded path at cursor. If document is pasted text, asks to save as a file first. Errors notice. | `src/main.ts:510-585`, `src-tauri/src/commands.rs:617-659`, `CHANGELOG.md:689-696` | P1 | Paste PNG: file created and link inserted; pasted doc prompts Save as. | F-IMAGE |
| E-37 | Editor font scaling | Editor font size = reader font size × 0.875 (14 px at default 16 px). Changing Aa size re-measures editor. | `src/styles/base.css:58`, `src/main.ts:4479-4483` | P2 | 22 px reading → 19.25 px editor. | F-PREFS |
| E-38 | Editor load failure | Editor chunk loaded lazily; if import fails, an error notice explains, edit mode does not open, reading unaffected. | `src/main.ts:2273-2303` | P1 | Block editor chunk: notice, no blank pane. | F-BROKEN |
| E-39 | Pasted text in edit mode | Pasted text can be edited; returning to reading keeps buffer text (doc content updated) and draft. No file until Save as. Closing tab with changes asks save. | `src/main.ts:421-433`, `src/main.ts:611-627` | P0 | Ctrl+E, type, Ctrl+E: preview shows typed text, tab dirty. | F-PASTE |
| E-40 | Undo per tab | Each tab’s editor state includes its undo history; moving a tab to another window carries history JSON. Switching tabs restores history. | `src/editor.ts:453-503`, `src/tabs/contract.ts:8-19` | P0 | Type, tab away, back, Ctrl+Z undoes. | F-TABS |

### 2.4 Markdown rendering (M)

The parser is markdown-it 14 with `html`, `linkify`, `typographer`; GFM behavior
is assembled from `markdown-it-anchor`, `markdown-it-task-lists`,
`markdown-it-github-alerts`, `markdown-it-footnote`, front matter and a
custom GitHub-style math layer. Rendering is sanitized with DOMPurify before it
touches the live document.

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| M-01 | CommonMark baseline | CommonMark blocks/inlines, raw HTML enabled, tables and strikethrough from markdown-it/GFM, links linkified. | `src/renderer.ts:58-84` | P0 | F-BASIC renders headings, lists, quotes, code, table, strike. | F-BASIC |
| M-02 | GitHub line breaks | Single newline inside paragraph = space; two trailing spaces or trailing backslash = hard break. Not `breaks:true`. | `src/renderer.ts:58-72`, `samples/commonmark-math.md:7-16` | P0 | Fixture lines render as one paragraph; two-space line breaks. | F-BASIC |
| M-03 | GitHub heading anchors | Lowercase; drop punctuation/symbols except Unicode letters/marks/numbers/connector punctuation, spaces and hyphen; spaces → `-`; accents kept. Duplicates `a`, `a-1`, `a-2`. A title reduced to nothing uses legacy slug. | `src/renderer.ts:29-54,73` | P0 | “## 1. Introduzione” → `#1-introduzione`; “Q&A” → `#qa`; “Attività” → `#attività`. | F-HEADINGS |
| M-04 | Legacy anchors | Every heading also gets `data-legacy-id` with pre-1.15.1 slug if different; old internal links are retargeted when no current element owns the old id; remembered positions and old links still land. | `src/renderer.ts:38-46,89-101,498-511`, `src/state.ts:21-33` | P1 | Open `#q%26a` from old fixture: lands on Q&A. | F-HEADINGS |
| M-05 | Reserved ids | A heading id matching app UI ids (`toolbar`, `links`, `title`, `name`, `timeline`, runtime ids, Mermaid/palette/tree group patterns) becomes `user-content-<id>`; original name kept in `data-anchor`; internal links retargeted. | `src/renderer.ts:513-561` | P1 | Heading “Toolbar” renders, has anchor, outline entry, and `[TOC]` link. | F-HEADINGS |
| M-06 | Tables | GFM tables render; each table is wrapped in `.table-block > .table-scroll`, with hover/focus action bar. Header/cell alignment follows Markdown. | `src/renderer.ts:905-931` | P0 | Fixture table renders 3 columns, aligned. | F-TABLE |
| M-07 | Task lists | `- [ ]` / `- [x]` render as checkboxes. No `<label>` around item text: only the checkbox toggles; text click selects. Checkbox gets `aria-label` from item’s own text (sub-lists and KaTeX MathML excluded), capped 120 chars with ellipsis. | `src/renderer.ts:74-77,832-855` | P0 | Click task text: no file change; click box: file changes. | F-TASK |
| M-08 | Strikethrough | `~~text~~` renders `<s>`. | `src/renderer.ts:58-84` | P0 | Struck text shown. | F-BASIC |
| M-09 | Autolinks | Only explicit scheme (`https:`, `mailto:`…), email, and `www.` addresses linkify. Fuzzy bare-domain linking disabled, so `README.md`, `main.rs`, `file.csv` stay text. `www.` must start an address, not follow `. / \ @ -`. | `src/renderer.ts:103-122` | P0 | `README.md` text; `www.example.com` link; `dev-www.example.com` text. | F-LINKS |
| M-10 | Typographer, minimal | Curly quotes enabled; replacements disabled, so `(c)`, `(tm)`, `--`, `...` stay literal. | `src/renderer.ts:58-72,124-126` | P1 | `--line` in prose is two hyphens, not en dash. | F-BASIC |
| M-11 | Footnotes | `[^1]` refs and `[^1]: …` definitions; collected at document end with backlinks. Definitions may be anywhere. | `src/renderer.ts:79` | P1 | Two footnotes render at end with return links. | F-FOOTNOTE |
| M-12 | YAML front matter | Leading `---` block (closes with an equal/larger dash line or `...`; unclosed runs to EOF) becomes collapsed `<details class="front-matter">` labelled “Front matter”. Summary lists keys joined by ` · `. Click opens a two-column table; array values become chips. Non-flat YAML falls back to `<pre>` verbatim. Block is omitted from print, rich-text copy and DOCX. | `src/renderer.ts:80-84,451-453`, `src/frontmatter.ts:20-99`, `src/styles/print.css:77-80` | P1 | Fixture: collapsed block, click shows tags chips; print has none. | F-FRONT |
| M-13 | Syntax highlighting | highlight.js common languages. Language label shown (empty if no language); copy button copies raw code text and shows check for 1.2 s. Unknown language = plain escaped code with label. | `src/renderer.ts:62-71,857-888` | P0 | TS block colored, label “typescript”, copy works. | F-CODE |
| M-14 | Mermaid | ` ```mermaid ` fences rendered to SVG, bundled locally. Light theme = neutral; dark/black = dark, with edge-label background = page bg and custom fills `#474949`, `#3a3c59`, `#a40000` for dark palette gaps. Invalid diagram: error banner “Invalid Mermaid diagram: …” plus original code block. Diagram naming the asset host is refused before Mermaid lays it out. | `src/renderer.ts:857-888`, `src/mermaid.ts:9-86`, `CHANGELOG.md:76-78` | P0 | Valid flowchart renders; invalid shows banner + code, not blank. | F-MERMAID |
| M-15 | Inline math `$…$` | Opens only when not after `$`, `\` or word char, and next char exists and is not whitespace. Closes at next unescaped `$` where next char is not `$`, not word char, and char before close is not whitespace; content may not be empty. `$`…`$` backtick form ends at first `` `$ `` and may contain `$`. `$ x $` stays text; `$5 and $10` stays text; `\$x\$` text; `<span>$</span>5` avoids opener. | `src/renderer.ts:339-390` | P0 | All cases in `commonmark-math.md` §Inline. | F-MATH |
| M-16 | Display `$$` blocks | A line starting `$$` (not `$$$`, not 4-column indent) opens a block only if a closing `$$` exists before a blank line, a less-indented line, or the end of the list/quote; the closing line must have only whitespace after `$$`. If no closure, the line is prose. Content may start/end on opening/closing lines. Text after a closing `$$` within the paragraph keeps rendering (inline-display span). Blank line inside cancels the block. | `src/renderer.ts:268-308` | P0 | `$$ 100 l’uno` remains prose; multi-line formula renders; text after close survives. | F-MATH |
| M-17 | Inline display `$$…$$` in a sentence | Renders as a block-level `<span class="katex-block">` to avoid `<p>` inside `<p>`; spaces/tabs immediately inside are disallowed; a line break just inside is allowed; two in a row work; text after close stays in same paragraph. | `src/renderer.ts:310-337,240-246`, `src/styles/markdown.css:615-623` | P0 | “The famous $$E=mc^2$$ formula” keeps one paragraph, formula on own line. | F-MATH |
| M-18 | Fenced math | ` ```math ` / `~~~math ` render display math; words after `math` are ignored (“```math Energy”). KaTeX loads on demand only when source contains `$` or a math fence; if it fails, math shows as written. | `src/renderer.ts:196-226,247-253,406-413` | P0 | “```math Fundamental theorem” renders formula. | F-MATH |
| M-19 | GitHub alerts | `> [!NOTE]`, `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]`, `[!CAUTION]` become tinted callouts with GitHub palette in both schemes. | `src/renderer.ts:78`, `src/styles/markdown.css:522-600` | P1 | Five callouts in fixture have distinct colors/icons. | F-ALERTS |
| M-20 | `[TOC]` | A paragraph containing only `[TOC]`, `[[TOC]]` or case variants becomes a nested `<nav class="doc-toc" aria-label="Table of contents">` of h1–h4 headings **after** it, preserving nesting based on levels. It is real document content, printed and exported. | `src/renderer.ts:786-830` | P1 | `[TOC]` before headings yields nested links; print includes it. | F-TOC |
| M-21 | HTML tagfilter | GitHub’s tagfilter: `title`, `textarea`, `style`, `xmp`, `iframe`, `noembed`, `noframes`, `script`, `plaintext`, `noscript` are escaped to show as text when written raw. This prevents one tag from swallowing the page. | `src/renderer.ts:145-161` | P0 | `<title>x</title>` in prose shows literally. | F-HTML |
| M-22 | Unknown tags hidden | Non-HTML custom tags such as `<name>` render as nothing, as on GitHub. `<br>`, `<kbd>`, `<sup>`, `<details>`, `<picture>`, video/audio work. Writing inside backticks or escaping shows the tag. | `src/renderer.ts:149-161`, `src/locales/help/en.html:328` | P1 | `<name>` absent; `` `<name>` `` visible. | F-HTML |
| M-23 | Sanitization | DOMPurify sanitizes rendered HTML; `<style>` forbidden; top-layer/command attributes `popover`, `popovertarget`, `popovertargetaction`, `command`, `commandfor`, `interestfor` removed; app protocol references stripped from attributes and `<style>` text. | `src/renderer.ts:393-437`, `src/reach.ts:53-69` | P0 | Fixture with `<style>`, popover, `asset:` image: none affect app. | F-SECURITY |
| M-24 | Relative images | Relative `src`, `srcset`, `<picture><source>`, video/audio `src`, poster, `<source>`, `<track>` resolve against the document folder via asset protocol. Fragment kept, query dropped. Resolved URLs cached per folder (cache cleared past 500). | `src/renderer.ts:563-716` | P0 | `./img/test.png` loads. | F-IMAGE |
| M-25 | Missing images | A resolved path with no file, a broken load, or a local image in pasted text is marked `.img-missing` and is no longer zoomable; alt text falls back to the source address when no alt. | `src/renderer.ts:684-757` | P1 | Broken image shows placeholder, click does nothing. | F-IMAGE |
| M-26 | `srcset` parsing | HTML-spec-like candidate parser: URL ends at whitespace, trailing commas end candidate, descriptors preserved; supports commas inside URLs. Unresolved candidates stay as written except off-limits network candidates, which are dropped. | `src/renderer.ts:615-648` | P2 | `a,b.png 1x, c.png 2x` resolves both. | F-IMAGE |
| M-27 | Media playback continuity | On preview rerender, new `<video>/<audio>` whose markup matches an old element is replaced by the old element, preserving playback position; old source lines transferred. Fresh copy is released (`remove src`, `load()`). | `src/media.ts:11-75` | P1 | Play video, type in editor: video keeps playing. | F-MEDIA |
| M-28 | Subtitles | A local `<track>` on video/audio causes `crossorigin="anonymous"` on its media element so the asset protocol can serve it across origins. | `src/renderer.ts:667-677` | P2 | Video with local VTT shows subtitles. | F-MEDIA |
| M-29 | GitHub theme images | CSS hides `img[src*="#gh-dark-mode-only"]` on light schemes and `#gh-light-mode-only` on dark. `<picture><source media>` with `prefers-color-scheme` is rewritten to always/never match the app scheme; original media stored in `data-media`; re-applied on theme change and around print. | `src/renderer.ts:759-784`, `src/styles/markdown.css:162-168` | P1 | Light/dark logo swaps with theme, not OS. | F-IMAGE |
| M-30 | Network path refusal | A relative URL that is a network path (`//server/share/…`, `\\server\share\…`, pipes, `\\?\`) is not looked up unless the document itself is on that same share. Such images show missing; such links get “A document cannot open a file on another computer”. App-protocol addresses in text are never loaded. | `src/reach.ts:9-51`, `src/renderer.ts:585-610`, `CHANGELOG.md:86-93` | P0 | Image `//server/share/x.png` shows missing; no lookup. | F-SECURITY |
| M-31 | Path scope | Resolution through Rust refuses rooted paths with their own prefix, paths on another drive/share, drive-relative `C:a.png`, device paths (`\\.\pipe`), and `\\?\` prefixes others than the document’s own; `..` cannot leave the document’s drive/share. | `src-tauri/src/commands.rs:360-394`, `src-tauri/src/commands.rs:1113-1170` | P0 | Link to `D:\x` or `//server` refused while local `../a` allowed. | F-SECURITY |
| M-32 | Source-line map | Every block token gets `data-source-line` (0-based); fences get it on `<code>`, math/HTML blocks get it stamped on first tag. Used for `--line`, preview sync, “Edit from here”, context menu, anchor jumps into `<details>`. | `src/renderer.ts:130-191` | P1 | `--line 40` lands on block containing source line 40. | F-LINE |
| M-33 | Long formula overflow | Display math scrolls sideways rather than widening/breaking the reading column; inline math wraps normally. | `src/styles/markdown.css:608-623` | P1 | Long formula has its own horizontal scrollbar. | F-MATH |
| M-34 | Render error containment | Rendering is transactional: a thrown Markdown/KaTeX/sanitize/image error keeps the previous article and shows “Preview error: …”. A failed Mermaid render only affects that diagram. | `src/main.ts:712-775`, `src/mermaid.ts:61-73` | P0 | Broken fixture never blanks the article. | F-BROKEN |

### 2.5 Full-window preview (V)

Diagrams and images open in a zoomable lightbox. Tables open in the same dialog
but as a different reader: `tableView.ts`, not a picture.

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| V-01 | Opening the preview | Click a diagram or image; click a table’s expand button; `Enter`/`Space` on a focused diagram/image; context menu “Open preview”. Table with no rows/width does not open. Broken image does not open. | `src/lightbox.ts:242-331`, `src/lightbox.ts:170-183` | P0 | Click image opens dialog; broken image does not. | F-PREVIEW |
| V-02 | Hover affordance | A single shared floating “open preview” button appears at the top-right of the hovered/focused zoomable element, hidden if it would overlap the find/notice/section bars or leave the pane. It is hidden from tab order and screen readers; the element itself is the keyboard button. | `src/lightbox.ts:734-795`, `index.html:342-346` | P2 | Hover image shows hint; keyboard focus does not need it. | F-PREVIEW |
| V-03 | Zoom keys | `+`/`=` zoom in 1.25×, `-`/`_` out, `0` Fit, `W` Width, `1` 100%, arrows pan 40 px (Shift 160 px), `Esc` closes. Works with keyboard focus in viewport. | `src/lightbox.ts:557-579` | P0 | All keys change level/position. | F-PREVIEW |
| V-04 | Wheel zoom | Plain wheel (and Ctrl+wheel) zooms around the pointer, factor `exp(-deltaY*0.0015)` clamped 0.5–2 per event; web page zoom suppressed. | `src/lightbox.ts:546-555` | P1 | Wheel over image keeps point under pointer fixed. | F-PREVIEW |
| V-05 | Drag pan | Left-drag pans; >3 px marks it a drag; pointer capture; release outside still ends. | `src/lightbox.ts:520-544` | P1 | Drag image moves it; no accidental close. | F-PREVIEW |
| V-06 | Fit/Width/100% semantics | Fit: whole content centered, diagrams may enlarge to 4×, rasters capped at 1:1. Width: window width from top, diagram capped 4×. 100%: natural size around view center. Zoom range min(`fit`,0.2) to 8. Resize re-fits Fit/Width, leaves custom zoom. Axis that fits is centered; pan clamped to content edges. | `src/lightbox.ts:29-35,387-511` | P1 | Large diagram Fit fills; small raster Fit does not upscale beyond 100%. | F-PREVIEW |
| V-07 | Preview caption/accessibility | Caption = image alt (or “Image”), “Diagram”, “Table”. Stage `aria-label` uses caption; SVG hidden from AT; dialog labelled “Preview”; viewport labelled with instructions; zoom level `aria-live="polite"`. | `index.html:348-392`, `src/lightbox.ts:372-373`, `src/locales/en.ts:195-220` | P1 | Screen reader announces caption and zoom changes. | F-A11Y |
| V-08 | Preview copy menu | Copy PNG 2×, PNG 4×, SVG (diagrams only), Save PNG…, Save SVG… (diagrams only), Copy table (for Excel; tables only). Menu hides irrelevant entries; focus returns to copy button on close; `Esc` closes menu first, then preview. | `src/lightbox.ts:582-637`, `index.html:365-388` | P1 | Diagram menu has SVG; image menu does not; table only copy-table. | F-PREVIEW |
| V-09 | PNG rasterization | SVG rasterized at requested scale on white background, intrinsic natural size; raster images converted to PNG through canvas if not already PNG. | `src/share.ts:615-645`, `src/lightbox.ts:689-721` | P1 | Copied PNG is crisp; transparent diagram gets white background. | F-PREVIEW |
| V-10 | Preview close semantics | Backdrop/empty viewport click closes only if press and release are on the same target and no drag happened. Table text selection ending on empty space does not close. Toolbar close button also closes. Modal dialog blocks app behind. | `src/lightbox.ts:161-168` | P1 | Select table text and release over backdrop: stays open. | F-PREVIEW |
| V-11 | Table preview layout | Full-window table copy in a real scroller; header row sticky; header scrolls sideways with columns; first column freezes only when table overflows, first-column width ≤35% of view, and no cell has row/colspan. | `src/tableView.ts:1-20,285-295`, `src/styles/lightbox.css` | P1 | Wide table: header stays, first column stays. | F-TABLE |
| V-12 | Table zoom | `+`/`−` ×1.25, Ctrl+wheel exponential, range 0.5–2; text is re-laid-out (CSS zoom), so sharp. Width mode fits table to window, never enlarges beyond 1.5×, never below 0.5×; tables that fit by wrapping stay 100%. Resize re-fits Width. | `src/tableView.ts:37-43,222-283` | P1 | Table text sharp at 200%; Width fits wide table. | F-TABLE |
| V-13 | Table filter | `Ctrl+F` focuses filter; debounce 80 ms; keeps rows containing **every** whitespace-separated word; accent/case folded; all matches highlighted; count “n of total rows” (localized plural); no rows message “No row contains “q””. Filter hides rows only; document unchanged. Widths locked while filtering to prevent jumping. | `src/tableView.ts:410-520` | P1 | “alpha beta” keeps rows with both; “0 of 8 rows”. | F-TABLE |
| V-14 | Table filter clearing | `Esc` with a non-empty filter clears it and keeps preview open; second `Esc` closes preview. `Esc` from anywhere in dialog does the same. `Enter`/ArrowDown in filter returns focus to table. | `src/tableView.ts:130-137,308-331` | P1 | Esc clears filter, Esc again closes. | F-TABLE |
| V-15 | Table sort | Click header cycles ascending → descending → original order. Only sortable if one `<tbody>`, ≥2 body rows, header exists, no body cell spans rows/cols, and header cells with links/buttons/inputs are skipped. Sort type per column: numeric if ≥80% filled cells parse as numbers, else date, else text. Decimal mark voted (comma/point), currency/percent/unit stripped, negatives with −/– normalized. Dates ISO or d/m/y; order inferred by values >12, else en-US English = mdy. Empty cells last in both directions; numbers before words. | `src/tableView.ts:522-702` | P1 | Numeric column sorts 2,10,100; dates sort chronologically; third click restores. | F-TABLE |
| V-16 | Table copy as shown | “Copy table (for Excel)” in preview copies visible rows in current order as TSV; hidden filtered rows omitted; cell text whitespace collapsed. | `src/tables.ts:5-12`, `src/lightbox.ts:601-608` | P1 | Filter+sort then copy: TSV matches screen. | F-TABLE |
| V-17 | Table links | Links in the copied table preview are mapped back to the document’s original links; clicking closes preview and dispatches a synthetic click on the original (preserving Ctrl/Shift/middle). Table task checkboxes are disabled in preview. IDs and source lines stripped from copy. | `src/tableView.ts:151-176,376-400` | P1 | Click link in preview: opens target, preview closes. | F-TABLE |
| V-18 | Table open mode | If table’s minimum width + frame exceeds view, opens at 100% and scrolls; if it fits, opens at Width. | `src/tableView.ts:182-199` | P1 | Huge table starts 100%; small table fills width. | F-TABLE |

### 2.6 Share, print and export (X)

Everything that leaves the app is collected in the Share menu (`Ctrl+Shift+E`).
Print and exports always use the standard light paper layout, independent of
reader preferences and theme.

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| X-01 | Share menu | Toolbar Share button or `Ctrl+Shift+E`; disabled with no document. Items: Copy as rich text, Print…, Save as PDF, New page at every heading 1 (checkbox), Export as HTML, Export as DOCX. Opens under button, first item focused, closes on outside click/Esc, arrow-key navigable. | `index.html:156-182`, `src/main.ts:1459-1476,4388-4432` | P0 | Menu lists six items; Esc closes and returns focus. | F-EXPORT |
| X-02 | Copy whole document as rich text | `Ctrl+Shift+C` or menu. Clipboard receives `text/html` + `text/plain` (plain = Markdown source, unsaved buffer while editing). HTML is fully inline-styled light theme; local images data URIs; code blocks single-cell shaded tables with language label; tables bordered with tinted header; alerts colored + emoji; tasks ☑/☐; math MathML only; Mermaid PNG at 2×; links absolutized; dark-only images removed; classes stripped. | `src/share.ts:345-577`, `src/main.ts:1319-1333` | P0 | Paste into Word: formatting, code shading, tables, equations, diagram image. | F-EXPORT |
| X-03 | Copy selection as rich text | `Ctrl+C` on article selection uses the same pipeline for the selected range; plain fallback = selection text. If HTML write fails, plain text only. | `src/main.ts:4326-4334`, `src/share.ts:541-551` | P1 | Copy a bold+link phrase; Word keeps both. | F-SELECT |
| X-04 | Print | `Ctrl+P` or Print…; prints only article, hides toolbar/panes/editor/find/notice/home/help/preview/dialogs/front matter. Forces 16 px sans, light colors, white page. Header title + localized date, footer `page / pages`, A4 margins 20/16/18/16 mm. Code wraps; blocks avoid page breaks; headings avoid break after; wide tables scaled. | `src/main.ts:1335-1416`, `src/styles/print.css:4-164` | P0 | Print preview has only article, header/footer, no UI. | F-EXPORT |
| X-05 | Save as PDF | File picker only; then WebView2 `PrintToPdf` A4 portrait, backgrounds on, native header/footer off, margins 0.63 in fallback. Success notice “PDF saved.” with **Show in folder**; failure notice with **Use the print dialog**. One PDF at a time; `pdfInProgress` blocks tab moves. | `src/main.ts:1418-1450`, `src-tauri/src/commands.rs:866-943` | P0 | PDF A4, header/footer from CSS, no dialog; cancel does nothing. | F-EXPORT |
| X-06 | Page break per H1 | Share checkbox persists `printBreakH1`; when on, every `h1` after the first gets `break-before: page` in print/PDF. Toggling keeps menu open. | `src/main.ts:4417-4424`, `src/styles/print.css:31-35`, `src/prefs.ts:17-18` | P1 | Two H1s + option → 2 pages; off → one. | F-EXPORT |
| X-07 | HTML export | Self-contained `.html`: inline markdown/code/KaTeX CSS plus exported token set; light theme; title; `<html lang="current UI language">`; images data URIs; `srcset`/`<picture>` collapsed to one light source; relative links `file:///`; video/audio as file URLs; dark-only images and their empty links removed; app buttons/anchors stripped; TOC included. | `src/main.ts:1526-1544`, `src/share.ts:11-55,657-684` | P1 | Open exported HTML offline: identical content and images. | F-EXPORT |
| X-08 | DOCX export | Word-oriented HTML through `html-docx-js-typescript`, portrait. CSS in points, simple tables/code/blockquote. Math MathML (native equations), Mermaid PNG, local images embedded, video/audio replaced by filename links, front matter removed, alerts with symbols/colors, tasks ☑/☐, relative links `file:///`. | `src/main.ts:1546-1564`, `src/share.ts:579-613,211-343` | P1 | Word opens DOCX; equations editable, diagram visible. | F-EXPORT |
| X-09 | Export naming | Default file name = source file name minus Markdown extension, or pasted title made Windows-file-safe; fallback “Pasted text”; filters localized (Markdown/HTML/Word/PDF/PNG/SVG). | `src/main.ts:157-160`, `src/docs.ts:277-302` | P1 | `notes.md` → `notes.html`; pasted `# Q1/Q2` → `Q1 Q2.html`. | F-EXPORT |
| X-10 | Export ignores reader settings | Reader size/font/line-height/width/theme do not affect print/PDF/HTML/DOCX/rich copy; standard 16 px sans 1.7 layout. | `src/styles/print.css:37-48`, `src/share.ts:11-55` | P0 | Set 22 px serif, export: 16 px sans. | F-EXPORT |
| X-11 | Print theme swap | Before print/PDF: sets `data-theme=light`, `data-scheme=light`, rerenders Mermaid light, waits for `<picture>` images to decode (≤1.5 s). After: restores theme and rerenders dark if needed. This also runs for prints started elsewhere via `beforeprint`. | `src/main.ts:1355-1403,4728-4738` | P1 | Print in dark theme: diagrams light; screen returns dark. | F-EXPORT |
| X-12 | Wide-table print fitting | Before printing, each top-level table is measured at paper width; if wider, CSS `zoom` scales it down, floor 0.3; below floor, cells get `overflow-wrap:anywhere` and are re-measured. | `src/wide.ts:39-43,212-265` | P1 | Very wide table fits page without cropping. | F-EXPORT |
| X-13 | Feedback | Share button flashes a check 1.2 s after rich/HTML/DOCX/PDF success; errors are error notices with the raw message. | `src/main.ts:1310-1317`, `src/main.ts:1539-1542` | P2 | Successful export shows check; failure shows notice. | F-EXPORT |
| X-14 | Print header text | Header = first `h1`’s text, else file name without extension, else pasted title; plus localized medium date. Injected as literal `@page @top-center` for that print and removed after. | `src/main.ts:1340-1353` | P1 | Header “Test document · Nov 12, 2026”. | F-EXPORT |
| X-15 | Export target rules | Export paths may not exist; parent must exist and is canonicalized; binary export base64-decodes; PDF path must be absolute and parent dirs are created. | `src-tauri/src/commands.rs:310-319,603-615,870-881` | P1 | Export to new filename in existing folder works. | F-EXPORT |
| X-16 | Table copy button | Every table has a hover/focus action bar with **Copy table (for Excel)** and **Open preview**; TSV excludes hidden rows; check feedback 1.2 s. | `src/renderer.ts:905-931`, `src/tables.ts:5-12` | P1 | Hover table: two buttons; copy pastes into spreadsheet cells. | F-TABLE |

### 2.7 File browser and folder search (F)

The left pane is rooted at the document’s folder and lists Markdown files, not
all files. It is both a view of the disk and a keyboard tree widget.

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| F-01 | Root follows document | Opening a document whose folder is outside the current root re-roots the pane there. Opening a file inside the current root keeps the root. Opening from the pane keeps the same root. | `src/main.ts:308-313`, `README.md:300-308` | P0 | Open `nested/deeper.md` from tree: root stays project root. | F-NESTED |
| F-02 | Up arrow | Moves root to the parent folder. Disabled at a drive root (`C:\`) and does nothing there. | `src/main.ts:3716-3723,3559`, `src/main.ts:150` | P0 | At `C:\docs\sub`, up → `C:\docs`; at `C:\` disabled. | F-NESTED |
| F-03 | Depth and size caps | Lists up to 4 levels below root (paths with 5 components are excluded) and at most 2,000 Markdown files globally. A folder with no Markdown descendants is hidden from its parent. | `src-tauri/src/commands.rs:52-67,444-489` | P0 | 6-level fixture shows 4 levels; 2,001st file absent. | F-TREE |
| F-04 | Skipped folders | Any name starting `.` and `node_modules`, `target`, `dist`, `__pycache__`, `venv`, `windows`, `program files`, `program files (x86)`, `programdata`, `appdata`, `$recycle.bin`, `system volume information` are skipped case-insensitively. Root itself may be such a folder. | `src-tauri/src/commands.rs:52-73` | P0 | `.git/x.md`, `node_modules/x.md` absent. | F-TREE |
| F-05 | Only Markdown files | Extensions: `md markdown mdown mkd mkdn mdwn markdn mdtxt`, case-insensitive. Non-Markdown files are not listed (folders are listed only if they contain Markdown). | `src-tauri/src/commands.rs:44-50`, `src/main.ts:145-148` | P0 | `image.png` absent, `README.mkd` present. | F-TREE |
| F-06 | Sort order | Directories before files; each group sorted case-insensitively by name. | `src-tauri/src/commands.rs:462-466` | P1 | `Zdir/` before `afile.md`; `Beta` before `alpha`. | F-TREE |
| F-07 | Collapsed folders | Only collapsed paths stored (lowercased). Survives navigation, watcher rebuilds and restarts. Selecting a file auto-expands all its collapsed ancestors and persists the change. | `src/tree.ts:10-47,78-106,180-194` | P0 | Collapse folder, open its file, restart: folder expanded. | F-TREE |
| F-08 | Watched disk | A recursive folder watch (400 ms debounce) reports creates/removes/renames that fall inside listing rules; content writes do not trigger. On `tree-changed`, the pane re-lists; if the JSON is unchanged it does not rebuild, so scroll and focus survive. | `src-tauri/src/watcher.rs:132-169`, `src/main.ts:3555-3600` | P0 | Add a `.md` in file manager: appears without losing scroll. | F-TREE |
| F-09 | Focus preservation | After a rebuild, the focused row (or its folder’s row) is found by path and re-focused without scrolling. | `src/main.ts:3577-3589` | P1 | Focus a row, rename another file: focus remains. | F-TREE |
| F-10 | Roving tab stop | One row has `tabIndex=0`: active file, or the folder hiding it, else the first row. Focusing any row takes the tab stop. Tab into the tree lands on the active file unless the tree already has focus. | `src/tree.ts:68-73,102-106,198-217` | P1 | Tab from toolbar lands on active file. | F-A11Y |
| F-11 | Tree keyboard | `↑`/`↓` visible rows, `→` opens collapsed folder or first child, `←` closes open folder or goes to parent, `Home`/`End`, `Enter`/`Space` open (with Ctrl/Shift modifiers preserved), `Ctrl+Enter` new tab, `Shift+Enter` new window. Alt/meta modifiers are left to the app. | `src/tree.ts:270-324`, `README.md:313-316` | P0 | Full keyboard traversal works without mouse. | F-A11Y |
| F-12 | File context menu | Right-click a file or folder: Open in new tab (files only), Open in new window (files only), Show in File Explorer, Copy path, Copy as Markdown link (relative to open document, percent-encoded; name without extension). Keyboard invocation positions under the row. | `src/main.ts:3055-3101`, `src/main.ts:4287-4302` | P1 | Menu on folder lacks Open entries; copy link is relative. | F-TREE |
| F-13 | Folder search trigger | Search box above the tree or `Ctrl+Shift+F`; shortcut shows the pane (or peek), focuses and selects box. With a document selection, searches that. Debounced 250 ms. | `src/main.ts:3342-3351,4490-4501`, `src/main.ts:3610-3613` | P0 | Ctrl+Shift+F focuses box; selecting word pre-fills it. | F-SEARCH |
| F-14 | Folder search algorithm | Case-insensitive substring; file-name hits (`line=0`) plus line hits (1-based). Walks same depth/skip rules as tree, at most 200 hits. Preview = line trimmed, centered on first match, ≤160 chars with ellipses; non-Markdown files ignored. | `src-tauri/src/commands.rs:500-587` | P0 | Search “PIM” returns file hit + line snippets. | F-SEARCH |
| F-15 | Search result rendering | Grouped by file (file button with icon + name); each text hit is a button “Line N” with `<mark>` around first match; empty state “No matches”. Clicking file opens without a specific line; clicking snippet opens at that line. | `src/main.ts:3632-3683` | P1 | Results show file header and line rows. | F-SEARCH |
| F-16 | Search result opening | Same placement modifiers as files. New tab carries query into its find bar; current tab navigates then opens find prefilled. New window opens at line (query cannot travel). Pasted text in front never replaced. | `src/main.ts:3685-3703` | P0 | Ctrl+click result: background tab with highlighted term. | F-SEARCH |
| F-17 | Search clear | `Esc` in box clears query, closes results, restores tree, blurs. Clear button same. Enter opens first result. `Esc` with empty box? (Input handler only; focus stays.) | `src/main.ts:4491-4501,3705-3714` | P1 | Esc restores tree. | F-SEARCH |
| F-18 | Pane visibility | `Ctrl+B` or toolbar toggles tree; stored choice `tree-hidden`. Hidden choice wins over auto-fit. Showing a pane focuses it only when invoked by keyboard. | `src/panes.ts:181-230`, `src/main.ts:4276-4277` | P0 | Ctrl+B hides/shows tree and remembers. | F-PANES |
| F-19 | Auto-fold and peek | When the article would get <520 px beside panes, outline folds first, then files (classes `toc-auto`, `tree-auto`). A folded pane can be opened over the text (`*-peek`), closed by Esc, outside click, or choosing an item. Below 760 px edit mode also forces panes out of the grid. | `src/panes.ts:27-33,153-175,181-259` | P0 | 600 px window: both panes hidden; Ctrl+B opens overlay. | F-NARROW |
| F-20 | Pane resizing | Drag inner edge; current width is also the minimum, max 560 px tree / 520 px toc. Keyboard: arrows 16 px, Shift+arrows 64 px, Home default, End max. Double-click resets default. Persisted on release/key. Applied width clamped by available shell width; stored choice preserved. | `src/panes.ts:1-8,90-149,301-344` | P1 | Widen to 400, shrink window: visual narrower, re-widen returns 400. | F-PANES |
| F-21 | Panel text size | Panels setting small/normal/large = 12/13/14.5 px. `Ctrl+wheel` over a panel changes panel size, over document changes reader size. | `src/prefs.ts:80-83`, `src/main.ts:4706-4726`, `src/styles/base.css:169-176` | P1 | Ctrl+wheel over tree changes only tree text. | F-PANES |
| F-22 | Empty folder | If listing fails or has no Markdown, tree shows blank and root name empty; no explicit “empty” message for tree. (Search has “No matches”.) | `src/main.ts:3592-3598` | P2 | Empty folder: blank pane, no crash. | F-TREE |

### 2.8 Outline, section line and history (O)

The outline is built only from h1–h4 with ids. The section line is a separate,
deliberately non-interactive-for-AT path shown when the outline is not visible.

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| O-01 | Outline contents | Right pane lists `h1`–`h4` with an id, in document order, as buttons; indentation by level; title = heading text. Empty outline hides pane and adds `no-headings`. | `src/toc.ts:58-118`, `src/main.ts:266-267` | P0 | Fixture’s h1–h4 all listed; h5/h6 not. | F-HEADINGS |
| O-02 | Outline jump | Click or keyboard jump glides to heading (`scroll-behavior` follows reduced motion), lights landing highlight, pins clicked entry; the place left is recorded for Back; outline peek closes. | `src/toc.ts:93-118`, `src/main.ts:4344-4347` | P0 | Click entry: heading at top, flash, back works. | F-HEADINGS |
| O-03 | Scroll-spy | Active heading = last heading whose top is above activation line. Line is 90 px normally; over the last viewport of scroll it slides linearly down to pane bottom, so the final headings can activate at max scroll. Active entry is scrolled into view in outline. | `src/toc.ts:4-12,45-55,120-157` | P0 | Scroll to bottom: last heading active. | F-HEADINGS |
| O-04 | Pin behavior | A clicked/jumped entry stays active through the smooth scroll and until the user scrolls by wheel/touch or a scroll after settling. Programmatic Back/Forward releases pin. | `src/toc.ts:78-91,133-149`, `src/main.ts:1670` | P1 | Click entry, wait, small scroll: spy resumes. | F-HEADINGS |
| O-05 | Outline filter | Appears only at ≥8 headings. Case-insensitive substring filter as you type; first visible gets keyboard cursor; `↑`/`↓` move among visible; `Enter` jumps; `Esc` clears and blurs; clear button clears and refocuses. `toc.setFilter` hides nonmatching buttons. | `src/main.ts:3799-3816,4349-4370`, `src/toc.ts:212-251` | P1 | 10-heading fixture: filter box; 7: absent. | F-HEADINGS |
| O-06 | Outline visibility | Hidden while editing (`#app.editing #toc-pane`), when no headings, or when user hid it/auto-folded. A peeking outline floats over text. | `src/styles/layout.css:89-103`, `src/panes.ts:153-179` | P0 | Ctrl+E: outline gone; back: returns. | F-EDIT |
| O-07 | Section line conditions | Visible only while reading a document, outline hidden or auto-folded (not peeking), not editing, not `no-headings`, not error/home. It shows when current heading has scrolled out of sight. | `src/sectionLine.ts:212-235`, `src/main.ts:4265-4273` | P1 | Close outline, scroll section out: line appears. | F-SECTION |
| O-08 | Section path | Path = current heading plus ancestors via nearest earlier smaller level; skipped levels attach to nearest smaller. Document title omitted when exactly one h1 and it is the first heading; multiple h1s remain. Rendered `A › B › C`. Headings with no text use image alt or “§”. | `src/sections.ts:1-41`, `src/sectionLine.ts:280-335,394-401` | P1 | “2. Decisions › 2.1 Budget”, title absent. | F-HEADINGS |
| O-09 | Section line suppression | Hidden while any heading reaches 56 px into the top band and has not fully gone; while a table header holds/approaches the top; when a focus-visible element is under the line; while scrolling faster than 2.4 px/ms; 120 ms after the last fast frame; 250 ms after an outline glide. It waits for the page to settle before changing path. | `src/sectionLine.ts:45-69,226-305`, `src/sectionLine.ts:432-439` | P1 | Jump to heading: line gone during glide; reappears after. | F-SECTION |
| O-10 | Section line interaction | Parts are `<span>`s, not links/buttons: not tabbable, `aria-hidden` line. Click a part jumps to that heading exactly like an outline click (records Back, glide, flash). A double-click’s second press is swallowed for 500 ms so it does not select text underneath. | `src/sectionLine.ts:1-26,403-430` | P2 | Click “2. Decisions”: jumps; double-click jumps once. | F-SECTION |
| O-11 | Section line reservation | A jump to a non-heading (footnote, anchor, `--line`) is moved down 30 px when the line will cover it, unless a table header will cover that point. Headings need no reserve because the line steps aside. | `src/sectionLine.ts:160-183`, `src/main.ts:1589-1596` | P2 | Link to footnote lands fully visible under line. | F-SECTION |
| O-12 | Back/Forward triggers | Recorded when following a local link to another document, an outline/section-line jump, a same-document `#anchor`, a `file.md#section` link, and opening another document by any entry point. Not recorded on reload/Back/Forward; same-document jump that lands within 8 px is not recorded. | `src/main.ts:1622-1692,314-322`, `src/main.ts:1634-1641` | P0 | Link to other doc: Alt+Left returns with position. | F-HISTORY |
| O-13 | History semantics | Per tab, the Back list is capped at 50 places; entries are path + scroll anchor; Forward is cleared on a new record and populated as you go back; duplicate consecutive places suppressed (<4 px + same id). A failed/cancelled open is not committed; gone files are dropped with “Not found”. Pasted-text places that are gone are dropped silently. | `src/navHistory.ts:13-21,58-90`, `src/main.ts:1655-1692` | P0 | Back into deleted file: notice, entry removed. | F-HISTORY |
| O-14 | History inputs | `Alt+←`/`Alt+→`, mouse side buttons (3/4), `BrowserBack`/`BrowserForward` keys, and first right-click menu entries when available. Back/Forward do not light landing highlight. | `src/main.ts:4549-4573,3201-3211` | P0 | Side mouse buttons navigate. | F-HISTORY |
| O-15 | History persistence | Each window persists its current tab’s history in `sessionStorage` (`nav-history`) across a language reload; other tabs keep theirs in memory/transfer. | `src/navHistory.ts:1-4,22-56`, `src/main.ts:4974` | P1 | Change language, Back still returns. | F-HISTORY |

### 2.9 Command palette (C)

One fuzzy box reaching commands, document headings, open tabs, folder files and
recent files. Loaded lazily on first open.

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| C-01 | Open keys | `Ctrl+K` or `Ctrl+Shift+P`. In the editor only `Ctrl+Shift+P` (Ctrl+K inserts a link). Not with Alt (AltGr), Meta, or IME composition. Repeating the opening key closes the palette (fresh press only). | `src/main.ts:4041-4052,4599-4603`, `src/palette.ts:761-810` | P0 | Ctrl+K opens; Ctrl+K closes; editor only Shift+P. | F-PALETTE |
| C-02 | What is listed | Commands available now, headings of current doc (h1–h4 + path), other tabs (excluding front, most recently shown first), recent files, files in pane’s folder (up to 2,000, excluding files open in tabs). Groups with caps: start commands 6/tabs 5/files 5; typed commands 6/headings 6/tabs 5/files 8; outline 200. | `src/palette.ts:96-158,523-582`, `src/main.ts:4071-4168` | P0 | Typing “tab” shows commands/tabs; start shows recents. | F-PALETTE |
| C-03 | Fuzzy matching | Accent- and case-insensitive; query words must appear in order; scores word starts/contiguous runs; English command label is a keyword in non-English UI; folder path and heading ancestors are searchable extras. | `src/fuzzy.ts:1-448`, `src/palette.ts:489-503`, `src/appCommands.ts:184-194` | P0 | “attivita” finds “Attività”; “sapdf” finds “Save as PDF”; “print” finds “Stampa” in Italian UI. | F-PALETTE |
| C-04 | Prefix modes | `>` keeps only commands; `#` keeps only headings. A bare `#` shows the outline in document order around current heading (cap 200, centered on current); `#foo` ranks heading matches. | `src/palette.ts:523-544` | P1 | `>` lists commands; `#` lists all headings with depth/current. | F-PALETTE |
| C-05 | Ranking | Score first, then recency/order, then shorter label. Recent commands move to top of command group (max 8 remembered). Start-page recents sorted by last opened. Highlight starts on best group’s top entry, or current heading in outline mode. | `src/palette.ts:376-395,489-521,552-581` | P1 | Recently used command first next time. | F-PALETTE |
| C-06 | Activation keys | `Enter` runs/opens per placement keys: files Ctrl+Enter background tab, Ctrl+Shift+Enter foreground tab, Shift+Enter new window. Commands ignore modifiers (act). Headings jump in current doc. Tabs activate. Middle click on file opens background tab. | `src/palette.ts:714-724,781-810,956-965`, `src/where.ts:13-17` | P0 | Enter opens here; Ctrl+Enter background; Shift+Enter window. | F-PALETTE |
| C-07 | Search rows | In “all” mode, adds “Find “q” in the document” (if doc on screen) and “Search the folder for “q”” (if folder root), after normal groups. Enter runs them; both then use editor find or folder search. | `src/palette.ts:566-581`, `src/main.ts:4099-4104` | P1 | Typing term + Enter on Find highlights it. | F-PALETTE |
| C-08 | Navigation | Up/Down wrap; PageDown next group’s first entry; PageUp start of current group or previous; Home/End not separately handled (text box editing keys only); Tab is swallowed; Esc closes. Pointer movement changes highlight only on real movement. | `src/palette.ts:678-752,781-810,944-955` | P1 | PgDn jumps groups; pointer rest does not steal selection. | F-PALETTE |
| C-09 | Modal key trapping | Every unhandled shortcut is `preventDefault`ed so print/save/zoom/find/tab keys do not act behind the dialog. Text-box Ctrl keys (select all, copy, paste, undo, word movement) allowed. F1 passes to main to open help. AltGr allowed for typing. | `src/palette.ts:764-824` | P1 | Ctrl+P in palette does not print; F1 opens help after closing palette. | F-PALETTE |
| C-10 | Focus/selection restoration | On open, remembers focused element and any non-collapsed page selection. On close, restores focus and selection (if still connected); if focus was on the input it blurs in the same task before running the picked action. Clicking backdrop (press+release outside) closes. Right-click on entries suppressed; paste in input does not create a document. | `src/palette.ts:235-350,906-928` | P1 | Ctrl+K, Esc: prior selection/focus restored. | F-PALETTE |
| C-11 | Command coverage | Commands include open/paste/edit/save/reload/copy path/reveal, tabs/windows, find/folder search/up/back/forward/panes, share/copy rich/print/PDF/HTML/DOCX, page break toggle, pin, five themes, larger/smaller/reset size, reading settings, keys, help, and every editor command while editing (tagged “Editor”). Availability predicates hide impossible actions. | `src/appCommands.ts:196-313` | P0 | Start page lacks Save/Find; editing adds editor commands. | F-PALETTE |
| C-12 | Accessibility | Dialog labelled “Command palette”; input `role=combobox`, `aria-autocomplete=list`, `aria-expanded`, `aria-controls`, `aria-describedby` help; list `role=listbox`; options `role=option`, `aria-selected`, `aria-activedescendant`; groups labelled; optional `aria-checked`; live status announces result count/empty after 300 ms. | `src/palette.ts:857-897,826-842,648-689` | P1 | Screen reader reads count and active option. | F-A11Y |
| C-13 | Loading and closing triggers | Module and appCommands loaded on first open; load failure notice, retry next time. Palette closes on tab switch, document change, share/context menu and drag; does not survive a modal opening while loading. | `src/main.ts:4030-4069,2049,245`, `src/palette.ts:307-350` | P1 | Open palette then switch tab: palette closes. | F-PALETTE |

### 2.10 Links and media (L)

Link handling is centralized in `followLink`, with `whereFrom` deciding
placement and Rust `resolve_doc` deciding whether a file exists.

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| L-01 | In-page anchors | `#id` (percent-encoded or not) finds a heading by id, data-anchor, or legacy id; smooth-scrolls with 24 px top margin plus table-header/section-line compensation; records Back; flashes landing. If no target: no action. | `src/main.ts:1585-1606,1645-1651`, `src/state.ts:21-33` | P0 | `[go](#section)` lands and flashes. | F-LINKS |
| L-02 | External schemes | `http:`, `https:` open default browser; `mailto:`, `tel:` open their apps; other schemes are ignored. External URLs from exported HTML also open externally. | `src/main.ts:3365-3367,4740-4747` | P0 | Click https link: browser; mailto: mail app. | F-LINKS |
| L-03 | Wiki resolution | `[text](other)` resolves, in order: `other` itself if file; then `other.<each md ext>`; then `other/index.<ext>`, `other/README.<ext>`, `other/readme.<ext>`. Anchors stripped before resolution and applied after. This is case-sensitive per filesystem (Windows case-insensitive). | `src-tauri/src/commands.rs:396-422`, `src/main.ts:3359-3373` | P0 | `[x](nested/deeper)` opens `deeper.md`; `[x](folder/)` opens README. | F-LINKS |
| L-04 | Same-document link | A local Markdown link resolving to the file already on screen does not re-read the file (which would overwrite live editor preview); it only jumps to the anchor if present. | `src/main.ts:3395-3401` | P0 | Click `other.md#h` in current doc: no reload, jumps. | F-LINKS |
| L-05 | Folder link | A link resolving to a directory that `list_tree` accepts (it has Markdown) roots the file pane there and shows it (dock or peek). No tab. | `src/main.ts:3423-3430` | P1 | `[x](nested/)` re-roots pane to nested. | F-NESTED |
| L-06 | Image link | A link to a real image opens it in the full-window preview (decode-tested); if decode fails, falls through to non-Markdown notice. | `src/main.ts:3404-3412` | P1 | `[x](img/test.png)` opens preview. | F-IMAGE |
| L-07 | Other file link | A link to an existing non-Markdown/non-image file never opens as text. Notice ““name” is not a Markdown document.” with **Open** for whitelisted document/media/archive extensions (pdf, txt, csv, json, docx, xlsx, images, audio, video, zip, html…), else **Show in folder**. Programs/scripts only reveal in file manager, never execute. | `src/main.ts:3376-3421`, `src-tauri/src/commands.rs:945-967` | P0 | Link to .pdf offers Open; link to .sh only Show in folder. | F-LINKS |
| L-08 | Unresolved link | Notice “Not found: <relative path>”; recents/tree/search refreshed; no navigation. | `src/main.ts:3431,1269-1275` | P0 | Broken link shows Not found. | F-ERRORS |
| L-09 | Pasted text links | Pasted text has no folder: a relative link says “Links to other files work once this text is saved as a file.” with **Save as…** action. In-page anchors still work. | `src/main.ts:3368-3372`, `src/locales/en.ts:388` | P0 | Click relative link in pasted tab: notice + Save as. | F-PASTE |
| L-10 | Network links/media | Relative network paths are refused (M-30). A document on a network share may open files on that same share. Absolute web URLs remain normal external links. | `src/reach.ts:18-24`, `src/main.ts:3386-3389` | P0 | `//server/share/a.md`: “cannot open a file on another computer”. | F-SECURITY |
| L-11 | Link click placement | Ctrl+click background tab, Ctrl+Shift+click foreground tab, Shift+click window, middle click background tab (without drag), plain click in current tab. The active tab check prevents a delayed resolve from acting after switching tabs. | `src/main.ts:3359-3402,4751-4758`, `src/where.ts:7-11` | P0 | Four gestures on a link work as specified. | F-LINKS |
| L-12 | Image maps and SVG links | `<area>` and SVG `<a>`/`xlink:href` are handled; right-click link address reads `href` or `xlink:href`. | `src/main.ts:3434-3436,4751-4758` | P2 | Image-map area opens link. | F-HTML |
| L-13 | Old anchors | `#q%26a` and old slug forms retarget to current GitHub id when no element has the old id; also applies to legacy reading positions. | `src/renderer.ts:498-511` | P1 | Old TOC links still land. | F-HEADINGS |
| L-14 | URL safety | A document cannot navigate the window away: forms/image maps/SVG/Mermaid links are intercepted; http/https/mailto/tel launch externally. A page attempting navigation is blocked. | `src-tauri/src/lib.rs:353-401`, `CHANGELOG.md:386-388` | P0 | Link inside SVG opens browser, app window stays. | F-SECURITY |
| L-15 | URL decoding | Href anchors decode percent escapes safely (`decodeURIComponent` fallback to raw on stray `%`/lone surrogate). Relative file paths decode once before Rust resolution. | `src/main.ts:1573-1580,3359-3373` | P1 | `%20` in filename resolves; stray `%` does not crash. | F-LINKS |

### 2.11 System, CLI and file integration (S)

The app is a single-instance desktop app. `main.rs` only calls
the library's `run()`; all setup is in `lib.rs`.

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| S-01 | CLI grammar | `lmdv [--new-window] [file] [--line N]`; `-l N` and `--line=N` accepted; `--` ends options; unknown `-flags` ignored; only first positional becomes path; later positionals ignored. `--new-window` may appear before/after file (before `--`). | `src-tauri/src/lib.rs:171-204`, `README.md:284-298` | P0 | Fixture run shows exactly the behavior in column 3. | `lmdv -l 40 --unknown a.md b.md` → a.md line 40. |
| S-02 | CLI path handling | Relative paths resolve against the shell’s cwd (also for a second instance); `.`/`..` folded without touching disk; missing file keeps a full absolute path, so the page can name it and tab matching still works. | `src-tauri/src/lib.rs:190-199`, `src-tauri/src/lib.rs:571-590` | P0 | Fixture run shows exactly the behavior in column 3. | `lmdv .\gone\x.md` shows “Not found: x.md”. |
| S-03 | Launch routing | Order: (1) a tab in another window has/soon has the file → activate it; (2) a window idle on a start page → new tab there; (3) window used last → new tab; (4) any ready window → new tab; else (5) new window. A window asking to close is skipped. A start page in front is reused. | `src-tauri/src/windowing.rs:80-111,134-180` | P0 | Fixture run shows exactly the behavior in column 3. | Open A in window 2, launch A: window 2 front. |
| S-04 | Launch during startup | Launches arriving before the first page listens are queued in order; `get_initial_file` consumes the window’s own launch; `show_window` releases queued launches. A burst from Explorer stays in one window, its own file first, no duplicates. | `src-tauri/src/windowing.rs:567-650`, `src-tauri/src/commands.rs:102-147` | P0 | Fixture run shows exactly the behavior in column 3. | 10-file burst from cold start opens 10 tabs in one window. |
| S-05 | `--new-window` | Always a new window, even if the file is already open elsewhere; without a file, an empty window on the start page. Missing file still gets a window and a Not found notice. | `src-tauri/src/windowing.rs:146,173-179,706-752` | P0 | Fixture run shows exactly the behavior in column 3. | Two `--new-window a.md` produce two windows. |
| S-06 | Single instance | Second process forwards `argv` + cwd to the first via Tauri single-instance plugin (registered first). Routing happens on async runtime, not the callback thread (WebView2 deadlock avoidance). | `src-tauri/src/lib.rs:488-496` | P0 | Fixture run shows exactly the behavior in column 3. | Second launch exits; first handles file. |
| S-07 | File associations | Installer associates 8 Markdown extensions as “Markdown document”, role Editor, rank Default. MSIX declares the same. `lmdv` command is installed in `%LOCALAPPDATA%\Microsoft\WindowsApps`; Store build uses app execution alias. | `src-tauri/tauri.conf.json:53-61`, `src-tauri/msix/Package.appxmanifest`, `README.md:295-298` | P0 | Fixture run shows exactly the behavior in column 3. | Double-click `.md` opens app; `lmdv` on PATH. |
| S-08 | New Markdown document | Installer registers a File Explorer “New → Markdown document” template; its name follows Windows display language (“Documento Markdown”, “Markdown-Dokument”…). Store package localizes via resources; Store version always “Markdown document”. | `src-tauri/nsis/hooks.nsh`, `src-tauri/msix/Strings/*/Resources.resw`, `CHANGELOG.md:220-224` | P1 | Fixture run shows exactly the behavior in column 3. | Right-click New shows localized entry creating empty .md. |
| S-09 | Recent items / jump list | Every successfully read file calls `SHAddToRecentDocs(SHARD_PATHW)`, feeding Windows Recent and the taskbar jump list. Non-Windows builds no-op. | `src-tauri/src/commands.rs:321-336` | P2 | Fixture run shows exactly the behavior in column 3. | Opened file appears in Explorer Recent. |
| S-10 | Portable mode | `lmdv-portable.txt` next to exe switches WebView2 user-data folder to `<exe>\data`, changes identifier to `com.example.lmdv.portable` (separate single-instance/config), and fails gracefully to per-user mode if data folder cannot be created. | `src-tauri/src/lib.rs:248-262,468-473`, `README.md:52-70` | P1 | Fixture run shows exactly the behavior in column 3. | Copy exe+marker to USB: settings in `data`, installed copy unaffected. |
| S-11 | Store build detection | `GetCurrentPackageFullName` returning `ERROR_INSUFFICIENT_BUFFER` means MSIX/Store; only then rating UI appears. | `src-tauri/src/commands.rs:849-864` | P2 | Fixture run shows exactly the behavior in column 3. | GitHub build has no rate card. |
| S-12 | Window size defaults | Main window 1200×860 logical, min 480×480, centered, `visible:false` until page calls `show_window`; no native decorations; drag-drop enabled. | `src-tauri/tauri.conf.json:12-26`, `src/main.ts:4987-4988` | P0 | Fixture run shows exactly the behavior in column 3. | Window appears only after first render, min size 480. |
| S-13 | Window placement | New windows cascade 32 px right/down from source, or from the last window created <3 s ago; if cascade leaves the work area it restarts at work-area top-left +32. Maximized source → maximized child. Minimized source → default size, centered. | `src-tauri/src/windowing.rs:20-24,263-379` | P1 | Fixture run shows exactly the behavior in column 3. | Cascade and off-screen reset. |
| S-14 | Window state persistence | Only window `main` remembers position/size/maximized via tauri-plugin-window-state; decorations are excluded so a 1.x saved state cannot restore a native title bar. | `src-tauri/src/lib.rs:498-506` | P1 | Fixture run shows exactly the behavior in column 3. | Move/resize main, restart: restored; other windows not. |
| S-15 | WebView2 missing | On Windows, if WebView2 runtime is absent, a native message box explains and offers to open the Microsoft download page; app exits instead of silently failing. | `src-tauri/src/lib.rs:206-246` | P2 | Fixture run shows exactly the behavior in column 3. | No runtime: explanation + download link. |
| S-16 | Native context menu pruning | WebView2’s context menu is allowed only in text fields/editor and retains Emoji, Undo, Redo, Cut, Copy, Paste, Paste and match style, Select all, spelling suggestions. Other entries and empty separators removed; empty menu suppressed. | `src-tauri/src/lib.rs:327-447` | P2 | Fixture run shows exactly the behavior in column 3. | Right-click document: app menu; right-click editor: native edit/spelling menu. |
| S-17 | Browser accelerator keys off | WebView2 browser keys (Ctrl+G, Ctrl+J, F7, its find/print in dialogs) are disabled; app handles every shortcut. | `src-tauri/src/lib.rs:377-400`, `CHANGELOG.md:349-351` | P1 | Fixture run shows exactly the behavior in column 3. | Ctrl+G does nothing; Ctrl+F opens app find. |
| S-18 | Navigation guard | A window never navigates away from the app. http/https/mailto/tel URLs open externally and navigation is cancelled; any other navigation is cancelled. | `src-tauri/src/lib.rs:345-402` | P0 | Fixture run shows exactly the behavior in column 3. | Clicking form submit does not replace app page. |
| S-19 | Clipboard read | `read_clipboard_text` runs off main thread; retries `OpenClipboard` 10× at 15 ms; reads CF_UNICODETEXT up to first NUL; image = CF_DIB/CF_DIBV5/CF_BITMAP present; returns `clipboard-busy` if all opens fail. | `src-tauri/src/clipboard.rs:20-118` | P1 | Fixture run shows exactly the behavior in column 3. | Copy text: start-page Paste opens it; held clipboard shows busy notice. |
| S-20 | Document watch | Watches each distinct parent folder non-recursively, debounced 300 ms, with file-id cache to pair renames. Emits `file-changed` per watched document touched. Folders watched, not files, to survive atomic saves. | `src-tauri/src/watcher.rs:35-96` | P0 | Fixture run shows exactly the behavior in column 3. | Atomic-save editor updates view. |
| S-21 | Tree watch | Watches pane root recursively, debounced 400 ms; only create/remove/rename of paths that pass `listed_path` emit `tree-changed`; content writes ignored. Windows watch can be dropped silently, so page rearms on window focus. Drive roots not watched. | `src-tauri/src/watcher.rs:132-169`, `src-tauri/src/commands.rs:716-779`, `src/main.ts:4935-4940` | P0 | Fixture run shows exactly the behavior in column 3. | Add file: one tree event; save file: none. |
| S-22 | Rename classification | If watched path exists after events → `changed` (atomic save). If gone, follow a rename chain from old→new; if final target is a file → `renamed`, else `removed`. Same name in another folder is not a rename. Case-insensitive comparison. | `src-tauri/src/watcher.rs:98-130` | P0 | Fixture run shows exactly the behavior in column 3. | Rename open file: tab follows; move out: removed. |
| S-23 | Save conflict check | Compares exact mtime (ms). `expected_modified=0/None` means overwrite. Without `create`, a file renamed away is not recreated under the old name. With `create`, a deleted file is recreated in canonicalized parent. | `src-tauri/src/commands.rs:252-298` | P0 | Fixture run shows exactly the behavior in column 3. | Rename during save: error, not ghost file. |
| S-24 | Path identity | Paths canonicalized with `dunce` (strips `\\?\`); comparisons case-insensitive ASCII (`same_file`); page path comparisons lowercase full strings. This treats Windows paths as case-insensitive and is used for tabs, recents, drafts, positions, watches. | `src-tauri/src/lib.rs:96-99`, `src/paths.ts:1-9` | P0 | Fixture run shows exactly the behavior in column 3. | `A.MD` and `a.md` are one tab. |
| S-25 | Command install | Installer creates `lmdv` in `%LOCALAPPDATA%\Microsoft\WindowsApps` (on PATH); Store registers `lmdv.exe` app execution alias; portable is run by path. | `README.md:295-298` | P2 | Fixture run shows exactly the behavior in column 3. | `lmdv --version`? (no version flag; `lmdv file.md`). |
| S-26 | Offline/privacy | No runtime network access except user-initiated external links/opens; no telemetry, accounts or ads. CSP allows only self, asset protocol, ipc; no remote script/style/font/connect. | `README.md:396-398`, `PRIVACY.md`, `src-tauri/tauri.conf.json:27-42` | P0 | Fixture run shows exactly the behavior in column 3. | Network monitor: idle app makes no connections. |
| S-27 | Windows-only capabilities | PDF export, clipboard read, recent items, store rating, Snap zones, DWM cloak, Windows preferred languages and native menu pruning are `#[cfg(windows)]`; non-Windows builds compile with no-op/stub behavior (PDF returns “only available on Windows”). | `src-tauri/src/commands.rs:866-943`, `src-tauri/src/clipboard.rs:111-118`, `src-tauri/src/drag.rs:1-20` | P2 | Fixture run shows exactly the behavior in column 3. | Linux build still runs viewer/editor; PDF falls back to print dialog. |

### 2.12 Window chrome (W)

Every window has `decorations:false`; the toolbar is the title bar. On Windows
11 native child zones re-create the Snap layouts over Maximize.

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| W-01 | Draggable toolbar | The whole toolbar has `data-tauri-drag-region="deep"`; pressing an empty part moves the window; double-click toggles maximize/restore. Tabs, buttons and inputs keep their clicks. | `src/tabs/chrome.ts:25-26`, `index.html:10-48` | P0 | Fixture run shows exactly the behavior in column 3. | Drag empty toolbar moves window; double-click maximizes. |
| W-02 | Caption buttons | Minimize / Maximize-Restore / Close at right. Close requests window close so unsaved prompt still runs. Buttons are 46 px wide, full toolbar height. | `index.html:43-47`, `src/tabs/chrome.ts:27-34` | P0 | Fixture run shows exactly the behavior in column 3. | Buttons match native placement; X prompts. |
| W-03 | Snap layouts | On Windows 11, native invisible zones over Min/Max/Close answer HTMINBUTTON/HTMAXBUTTON/HTCLOSE; Maximize exposes Snap layouts. Zones are positioned from page-reported CSS rects and hidden while the preview covers the buttons (the page reports `visible:false`). | `src-tauri/src/caption.rs:1-146`, `src/tabs/chrome.ts:73-96` | P2 | Fixture run shows exactly the behavior in column 3. | Hover Maximize shows Snap flyout on Win11. |
| W-04 | Max/Restore state | Max icon/title/aria-label switches to “Restore down” when maximized; re-checked on start and on window resize. | `src/tabs/chrome.ts:49-60,91-94` | P1 | Fixture run shows exactly the behavior in column 3. | Maximize: icon becomes restore. |
| W-05 | Window system menu | Right-click empty toolbar (and tab strip empty end) opens native window menu at pointer: Restore, Move, Size, Minimize, Maximize, Close. | `src/tabs/chrome.ts:36-47`, `src/tabs/strip.ts:476-487`, `src-tauri/src/caption.rs:132-147` | P2 | Fixture run shows exactly the behavior in column 3. | Right-click title bar: native menu. |
| W-06 | Inactive window look | On window blur, a `win-blur` class greys the caption; focus removes it, mimicking Windows. | `src/tabs/chrome.ts:62-64`, `src/styles/tabs.css:528+` | P2 | Fixture run shows exactly the behavior in column 3. | Unfocused window caption grey. |
| W-07 | Always on top | Pin button toggles `setAlwaysOnTop`; button `aria-pressed`; state is per window and not persisted. No shortcut since 2.0. | `src/main.ts:1697-1710`, `README.md:165-166` | P1 | Fixture run shows exactly the behavior in column 3. | Pin keeps window above others; restart unpinned. |
| W-08 | Native chrome hidden behind preview | While preview dialog is open, the page reports `visible:false` to caption zones so an invisible Close cannot sit over a picture. | `src/tabs/chrome.ts:76-87` | P2 | Fixture run shows exactly the behavior in column 3. | Open preview over caption: no invisible close trap. |
| W-09 | Window title | OS title is `“name” — lmdv`, prefixed `• ` when front tab dirty; start page exactly `lmdv`. | `src/main.ts:392-417`, `src/locales/en.ts:14` | P1 | Fixture run shows exactly the behavior in column 3. | Dirty title begins bullet. |
| W-10 | Toolbar layout | Order: tree, tabs, spacer, Open, Edit, Save, separator, Share, Aa, Pin, separator, Help, Outline, caption. Tabs expand to fill and `+` stays visible at overflow. | `index.html:10-48`, `src/tabs/strip.ts:92-125` | P1 | Fixture run shows exactly the behavior in column 3. | Layout matches fixture screenshot. |

### 2.13 Drag and drop (D)

Tauri delivers OS file drops; tab drags are DOM pointer events; cross-window
drags use Rust/DWM on Windows.

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| D-01 | Drop overlay | Dragging files over the window adds `.dropping`; a card “Drop to open” appears. Leaving removes it. | `src/main.ts:4941-4946`, `index.html:153`, `src/locales/en.ts:64` | P1 | Fixture run shows exactly the behavior in column 3. | Drag file over window: overlay. |
| D-02 | File drop | Only Markdown paths are opened; several open one tab each, first front, rest background. Dropped on the tab strip → inserted at drop index (x converted by devicePixelRatio), successive files at following indices. | `src/main.ts:4947-4957` | P0 | Fixture run shows exactly the behavior in column 3. | Drop 3 files on tab 2: tabs inserted there. |
| D-03 | Image drop in editor | If no Markdown among dropped and an image exists and editing, copy image into `assets/` and insert link. Otherwise non-Markdown drop ignored. | `src/main.ts:4950-4959` | P1 | Fixture run shows exactly the behavior in column 3. | Drop PNG while editing inserts image. |
| D-04 | Tab drag | See T-19–T-22: reorder, tear-off, merge, only-tab window drag. | `src/tabs/strip.ts:547-686` | P0 | Fixture run shows exactly the behavior in column 3. | Reorder and tear-off. |
| D-05 | Cross-window ghost | While a tab/window hovers another app window’s strip, a ghost tab opens a place; target receives `tab-hover` at most every 15 ms; moving window is DWM-cloaked over the strip. | `src-tauri/src/drag.rs:252-258,356+`, `src/main.ts:4963-4969` | P1 | Fixture run shows exactly the behavior in column 3. | Ghost follows pointer over other window. |
| D-06 | Drag-to-scroll | Middle-drag anywhere or left-drag empty margin scrolls the article; 3 px threshold; capture only after threshold so clicks stay clicks. | `src/main.ts:3484-3542` | P1 | Fixture run shows exactly the behavior in column 3. | Middle-drag scrolls. |
| D-07 | Window drag by sole tab | Dragging the only tab starts native move loop with Aero Snap; dropping on another strip offers merge. | `src/main.ts:2867-2871`, `src-tauri/src/drag.rs:156-177` | P2 | Fixture run shows exactly the behavior in column 3. | Sole tab drag moves window. |
| D-08 | Drag cancel | `Esc` during tab drag returns it to original slot; pointer cancel also settles. | `src/tabs/strip.ts:674-686`, `src/main.ts:4532-4537` | P1 | Fixture run shows exactly the behavior in column 3. | Esc mid-drag restores order. |

### 2.14 Help, start page, notices, rating (H)

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| H-01 | Start page | Shown when no document is open: app icon, “No document open”, “Drop a Markdown file here, or”, **Open a file…**, **Paste from clipboard**, recent list (if any), three tips, version, “press F1 for help, ? for the keys”. It scrolls if recents are long. | `index.html:98-123`, `src/styles/ui.css:1236+`, `CHANGELOG.md:302-303` | P0 | Fixture run shows exactly the behavior in column 3. | Empty window shows all elements; long recents scroll. |
| H-02 | Recent files | Max 12, newest first, one per path. Row: name, folder, relative time (“now”, “5 minutes ago”, “yesterday”, short date). Click opens per modifiers; middle click opens background tab; dead files silently removed at display. **Clear** empties list. | `src/recents.ts:11-54`, `src/main.ts:3936-3974`, `src/i18n.ts:95-106` | P1 | Fixture run shows exactly the behavior in column 3. | Recents show time; delete file, reopen home: gone. |
| H-03 | Start-page tips | “Double-click any .md file in File Explorer”, “Drag a file onto this window”, “Press Ctrl+O to browse for one”. | `index.html:116-120`, `src/locales/en.ts:60-63` | P2 | Fixture run shows exactly the behavior in column 3. | Text matches. |
| H-04 | Help dialog | `F1` opens modal help, localized, first use loads a ~20 KB page chunk; shows version, icon, license/repo links external; Store build also “Rate on the Store” card. Click backdrop or Close/Esc closes. `F1` again closes (unless preview open, which blocks help). | `index.html:317-340`, `src/main.ts:3922-3927,4579-4587`, `src/i18n.ts:239-260` | P0 | Fixture run shows exactly the behavior in column 3. | F1 shows localized help with version. |
| H-05 | Keys overlay | `?` toggles `<dialog>` with four groups and localized key caps (`Strg`, `Maj`, `Leertaste`…). `?` again or Esc closes. It lists Space paging even though code has no explicit handler. | `index.html:248-315`, `src/i18n.ts:192-236` | P1 | Fixture run shows exactly the behavior in column 3. | Overlay keys localized in German. |
| H-06 | Notice bar | One message at a time at top center of pane; kind icons warning/error/info/success; optional action button; dismiss ×. Sticky notices survive preview refreshes only. Find bar and notice reposition side-by-side or stacked. Dismiss callback may delete draft. | `src/notices.ts:1-116`, `src/bars.ts:1-34`, `index.html:89-96` | P0 | Fixture run shows exactly the behavior in column 3. | Warning with Reload; dismiss; layout no overlap. |
| H-07 | Screen-reader notices | Non-errors go to polite `role=status`; errors to assertive `role=alert`; live regions cleared before writing so the same message twice is announced twice. Palette has its own status inside dialog. | `index.html:395-397`, `src/notices.ts:104-116` | P1 | Fixture run shows exactly the behavior in column 3. | Screen reader announces save/error. |
| H-08 | Micro-feedback | Save: tab dot→check 1.4 s + “Saved” announcement; Share button check 1.2 s; code/table copy check 1.2 s; preview copy button `.done` 1.2 s. | `src/main.ts:1063-1079,1310-1317`, `src/renderer.ts:875-883` | P2 | Fixture run shows exactly the behavior in column 3. | All confirmations visible. |
| H-09 | Error card | Unreadable document shows centered error card text (not notice) in article; tab gets no reading time; home not shown if a prior doc exists, notice instead. | `src/main.ts:3976-3989,324-339` | P1 | Fixture run shows exactly the behavior in column 3. | Permission-denied file: error card, app alive. |
| H-10 | Rate prompt | Store build only. Due after ≥5 opened documents and ≥2 days since first run; card appears 2 s later only in focused window with a doc, not editing, no card already. Shown claims it for 7 days in all windows; **Later** 30 days; **No thanks** forever; **Rate** opens Store dialog, fallback Store page; Esc = Later. Help rating counts as done. | `src/rating.ts:1-99`, `src/main.ts:3889-3920`, `CHANGELOG.md:53-54` | P2 | Fixture run shows exactly the behavior in column 3. | Simulate dates: card once, one window, Later returns after 30 d. |
| H-11 | Quick reference from home | `?` works on start page too (no doc required). | `src/main.ts:4615-4620` | P2 | Fixture run shows exactly the behavior in column 3. | Empty window `?` opens overlay. |
| H-12 | Drop hint | “Drop to open” card follows `.dropping` state. | `index.html:153`, `src/main.ts:4943-4945` | P2 | Fixture run shows exactly the behavior in column 3. | Overlay visible during drag. |
| H-13 | Version display | Version `v2.2.0` in help header and start-page footer; resolved asynchronously from Tauri app version. | `src/main.ts:4807-4810`, `index.html:121,319` | P2 | Fixture run shows exactly the behavior in column 3. | Both show same version. |

### 2.15 Accessibility (A)

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| A-01 | Contrast | All four themes meet WCAG AA for body text, muted text, links, primary buttons, code comments, callout titles, menu hover/edges. | `CHANGELOG.md:261-268`, `src/styles/base.css:1-130` | P1 | Fixture run shows exactly the behavior in column 3. | Contrast checker passes 4.5:1 body / 3:1 large. |
| A-02 | Visible focus | `:focus-visible` 2 px accent outline with 2 px offset everywhere; toolbar, tree, menus, copy buttons, find bar, Aa popover. | `src/styles/base.css:112-116`, `CHANGELOG.md:264-270` | P0 | Fixture run shows exactly the behavior in column 3. | Tab through UI: every stop visible. |
| A-03 | Names from tooltips | `setTitle` splits “Save (Ctrl+S)” into `aria-label="Save"` and `aria-keyshortcuts="Control+S"`; icon-only buttons get real names; glyph buttons “Aa”, “A” get names. | `src/i18n.ts:108-190` | P0 | Fixture run shows exactly the behavior in column 3. | Screen reader says “Save, Control+S”. |
| A-04 | Toolbar and dialogs | Toolbar buttons named; dialogs `aria-label`; caption group “Window”; panes “Files”/“Outline”; resizers role separator with min/max/now; expand button labels. | `index.html:11-47,50-150`, `src/i18n.ts:218-237` | P1 | Fixture run shows exactly the behavior in column 3. | Every control has accessible name. |
| A-05 | Tabs | `role=tablist` “Open documents”; each tab `role=tab`, `aria-selected`, roving tabindex, `aria-controls=shell`, full name label, reading time `aria-description`. Arrow/Home/End/Enter/Space/Delete keys. | `src/tabs/strip.ts:92-118,164-200,510-538` | P1 | Fixture run shows exactly the behavior in column 3. | Screen reader reads tab name + reading time. |
| A-06 | File tree | `role=tree`; rows `treeitem` with `aria-level`, `aria-posinset`, `aria-setsize`, `aria-expanded`, `aria-current`; folder `aria-owns` its group; roving tab stop. | `src/tree.ts:114-178,196-217` | P1 | Fixture run shows exactly the behavior in column 3. | Screen reader announces “Level 2, 3 of 7, collapsed”. |
| A-07 | Outline | Entries are native buttons; active entry class; outline nav labelled “Outline”; filter input labelled. | `index.html:141-150`, `src/toc.ts:106-118` | P1 | Fixture run shows exactly the behavior in column 3. | Active entry announced/visible. |
| A-08 | Menus | Menus `role=menu`, items `menuitem`; Up/Down wrap, Home/End, Esc/Tab close and restore focus; Enter/Space native; hidden/disabled items skipped. | `src/menuKeys.ts:1-47`, `src/contextMenu.ts:130-173` | P0 | Fixture run shows exactly the behavior in column 3. | Arrow keys traverse all menus. |
| A-09 | Dialogs | Native `<dialog>` modal for help, keys, preview, palette. Backdrop click closes help/keys; Esc native except where a filter/menu consumes first. Focus returns to trigger. | `index.html:248,317,348`, `src/main.ts:4503-4509`, `src/lightbox.ts:152-168` | P1 | Fixture run shows exactly the behavior in column 3. | Focus trapped and restored. |
| A-10 | Live regions | Notices polite/assertive; zoom level polite; table filter count polite; palette result count polite; saved/undo/copy announcements. Same-message repeat handled by clearing first. | `src/notices.ts:104-116`, `index.html:359,396-397`, `src/tableView.ts:106` | P1 | Fixture run shows exactly the behavior in column 3. | Screen reader gets updates without focus move. |
| A-11 | Task checkbox names | Checkbox `aria-label` = item’s text (excluding nested lists and KaTeX MathML), max 120 chars, ellipsis. | `src/renderer.ts:832-855` | P1 | Fixture run shows exactly the behavior in column 3. | Checkbox announced “Install installer QA”. |
| A-12 | Zoomable media | Images/diagrams `role=button`, `tabindex=0`, `aria-label="alt — open preview"` / “Diagram — open preview”; Enter/Space opens. Table itself not made a button (screen-reader semantics); expand button does. | `src/lightbox.ts:242-268`, `src/main.ts:170-176` | P1 | Fixture run shows exactly the behavior in column 3. | Tab to image, Enter opens preview. |
| A-13 | Table preview semantics | Region label with full key instructions; filter input labelled and `aria-keyshortcuts=Control+F`; count live; sortable headers get `aria-sort`; header buttons have title “Sort by this column…”. | `src/tableView.ts:79-113,557-568,600-605` | P1 | Fixture run shows exactly the behavior in column 3. | Table region instructions read once. |
| A-14 | Reading keys gated | Single-key commands only when no input/select/contenteditable/CodeMirror has focus, no editor, document visible. Menus/palette/dialogs hold their own keys. | `src/main.ts:3818-3829,4614-4625` | P0 | Fixture run shows exactly the behavior in column 3. | Typing in find bar does not scroll document. |
| A-15 | Reduced motion | `prefers-reduced-motion: reduce`: smooth scrolls become instant, landing highlight becomes static 1.2 s tint, CSS animations quieted. | `src/motion.ts:1-71`, `src/styles/base.css:118-143` | P1 | Fixture run shows exactly the behavior in column 3. | OS animation effects off: no glide/fade. |

### 2.16 Internationalization (I)

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| I-01 | Six languages | English `en`, Italian `it`, German `de`, French `fr`, Spanish `es`, Simplified Chinese `zh-CN`. All 399 keys exist in every dictionary; TypeScript enforces the schema. | `src/i18n.ts:6-18`, `src/locales/*.ts` | P0 | Fixture run shows exactly the behavior in column 3. | Every language file compiles; no missing key. |
| I-02 | Auto selection | If no saved preference: first supported language in Windows’ preferred-languages list (injected as `window.__APP_LANGS__` before scripts); any `zh*` → `zh-CN`; primary tag matched; no match → English. In browser harness falls back to `navigator.languages`. | `src/i18n.ts:34-52`, `src-tauri/src/lib.rs:264-325` | P0 | Fixture run shows exactly the behavior in column 3. | Windows list [fr-FR,en-US] → French. |
| I-03 | Override | Aa → Language: Auto, English, Italiano, Deutsch, Français, Español, 中文. Stored `lang`; choosing Auto removes key. Choice reloads window immediately. | `src/i18n.ts:54-77`, `index.html:234-245`, `src/main.ts:4457-4478` | P0 | Fixture run shows exactly the behavior in column 3. | Pick Deutsch: UI reloads German; Auto returns system. |
| I-04 | Reload carries tabs | Before language reload, all tabs (scroll, editor undo, nav, find, toc filter, pasted text) serialized to sessionStorage. If serialization fails and any tab is dirty/pasted, asks “Changing the language reloads the window. Discard the unsaved changes?”. | `src/main.ts:2930-2959,4462-4477` | P1 | Fixture run shows exactly the behavior in column 3. | Switch language dirty: tab/edits survive. |
| I-05 | WebView2 language mirror | `remember_language` writes preference to `<config>/ui-language`; next start passes `--lang=xx` to WebView2 (English → en-US) so native menus/spellcheck/print preview follow app language, not Windows display. | `src-tauri/src/commands.rs:970-979`, `src-tauri/src/lib.rs:290-316,474-487` | P2 | Fixture run shows exactly the behavior in column 3. | Change language, restart: spellcheck dictionary changes. |
| I-06 | Date/number formatting | All numbers/dates via `Intl` with current language: reading-time word count, recent relative times, print header date, palette result counts, table row counts, table number/date sorting and decimal inference (en/zh-CN default point; others comma). | `src/i18n.ts:85-106`, `src/main.ts:1347,3782`, `src/tableView.ts:413-431,639-682` | P1 | Fixture run shows exactly the behavior in column 3. | Italian: “1.234”; English: “1,234”. |
| I-07 | Key names localized | Displayed shortcuts translate Ctrl/Shift/Space/Esc/PgUp/PgDn (Strg, Umschalt, Maj, Mayús…); keys overlay `<kbd>` caps translated; aria-keyshortcuts remain standard English UI Events values. | `src/i18n.ts:110-236` | P1 | Fixture run shows exactly the behavior in column 3. | German shows “Strg+Umschalt+E”. |
| I-08 | English command names as keywords | In non-English UI every palette command label uses the localized string but the English label is appended as a search keyword. | `src/appCommands.ts:184-194,304-313` | P1 | Fixture run shows exactly the behavior in column 3. | Italian UI: typing “print” finds “Stampa”. |
| I-09 | Help and export language | Help page chosen by current language, lazy chunk. HTML export sets `<html lang="current">`; print header date localized. | `src/i18n.ts:19-29,239-260`, `src/share.ts:671` | P1 | Fixture run shows exactly the behavior in column 3. | German help and `lang="de"` export. |
| I-10 | Localized dialogs and menus | Every menu item, tooltip, notice, confirmation button label, file filter, date and command is in the dictionary; confirmation dialogs use explicit labels instead of Windows Yes/No. | `src/locales/en.ts:1-426`, `src/main.ts:587-598,2484-2493` | P0 | Fixture run shows exactly the behavior in column 3. | German close prompt says “Speichern/Nicht speichern/Abbrechen”. |
| I-11 | Seven locale artifacts | Six UI dictionaries plus six help pages (`src/locales/help/{en,it,de,fr,es,zh-CN}.html`), MSIX language resources for the same six, and Store listing languages. | `src/locales/help/`, `src-tauri/msix/Strings/`, `README.md:241-242` | P2 | Fixture run shows exactly the behavior in column 3. | Each artifact exists and is non-empty. |

### 2.17 Persistence (P)

All page state lives in WebView localStorage or sessionStorage; the only
native-side state is window geometry, the language mirror, and the WebView2
profile/portable data folder.

| ID | Feature | Exact behavior and edge cases | Source refs | Priority | Acceptance criterion | Suggested fixture |
|---|---|---|---|---|---|---|
| P-01 | `reader-prefs` | JSON `{size,font,lineHeight,width,sideSize,printBreakH1}`. Absent key = defaults. Size snapped to nearest step 13,14,15,16,17,18,20,22. Invalid enum falls back. When all values equal defaults, key removed. Written on every change. | `src/prefs.ts:21-73` | P0 | Fixture run shows exactly the behavior in column 3. | Set 20 px serif; reload: persists; reset removes key. |
| P-02 | `theme` | `light\|sepia\|dark\|black`; absent = system (`prefers-color-scheme`), which follows OS live. Changing writes; choosing system removes. Cross-window `storage` sync. | `src/theme.ts:28-70` | P0 | Fixture run shows exactly the behavior in column 3. | Set dark; reload dark; Auto follows OS. |
| P-03 | `lang` | Language code or absent = Auto; changes stored; reload required. Cross-window not live (each window reloads itself). | `src/i18n.ts:30-77` | P0 | Fixture run shows exactly the behavior in column 3. | Set it; new window starts Italian. |
| P-04 | `read-positions` | Map lowercased path → `{id,offset,at}`. Max 100, oldest `at` evicted. At-top entry deleted. Renames move entry. Written after 300 ms idle scroll, tab suspend, document leave. | `src/positions.ts:6-77` | P0 | Fixture run shows exactly the behavior in column 3. | 101 files: oldest position gone. |
| P-05 | `recent-files` | Array `{path,name,dir,openedAt}`, max 12, newest first, one per path (case-insensitive). Written on successful open, rename, remove, clear. Empty → key removed. | `src/recents.ts:11-54` | P0 | Fixture run shows exactly the behavior in column 3. | Open 13 files: 12 newest; clear removes key. |
| P-06 | `drafts` | Map lowercased key → `{text,at,base}`. File key = path; pasted key = `pasted:<id>`. Max 20 entries / 2 MB chars; quota drop-oldest retry. Written 1 s after dirty keystroke, on tab suspend, on paste change; removed on save/discard/clean. | `src/drafts.ts:1-89`, `src/main.ts:602-634` | P0 | Fixture run shows exactly the behavior in column 3. | Kill app dirty; draft survives; save removes. |
| P-07 | `tree-collapsed` | JSON array of lowercased collapsed folder paths only. Written on collapse/expand and auto-reveal. | `src/tree.ts:10-47,180-194` | P1 | Fixture run shows exactly the behavior in column 3. | Collapse persists across restart. |
| P-08 | `tree-width` / `toc-width` | Integer CSS px. Absent = 260/240 default. Clamped to default..560/520 and current room. Written on drag end and keyboard resize; default removes key. | `src/panes.ts:20-24,90-99,301-344` | P1 | Fixture run shows exactly the behavior in column 3. | 400 px width survives restart. |
| P-09 | `tree-hidden` / `toc-hidden` | `"1"` hidden, `"0"` shown (always written on toggle, so a missing key means shown). Auto-fold is never stored separately. | `src/panes.ts:24-25,66-69,225-230` | P1 | Fixture run shows exactly the behavior in column 3. | Hidden pane stays hidden after restart. |
| P-10 | `palette-recent` | JSON array of up to 8 command ids, most recent first; written when a command is run from palette. | `src/palette.ts:151-152,467-485` | P2 | Fixture run shows exactly the behavior in column 3. | Run Print from palette; next open lists it first. |
| P-11 | `editor-spell` | `"0"` = spellcheck off; absent = on. Written by Tools toggle; language change takes effect next start. | `src/editCommands.ts:357-368` | P2 | Fixture run shows exactly the behavior in column 3. | Off survives restart. |
| P-12 | `rate-prompt` | `{firstRun,opens,state:"pending"\|"later"\|"done",askedAt}`. Store build only. Written on first document open, card shown, Later, No thanks. | `src/rating.ts:9-99` | P2 | Fixture run shows exactly the behavior in column 3. | Simulate 5 opens/2 days: due; No thanks never again. |
| P-13 | `nav-history` (session) | Per-window JSON `{back,forward}`, max 50 each. Written after every record/commit/drop/rename; survives language reload but not window close. | `src/navHistory.ts:13-56` | P1 | Fixture run shows exactly the behavior in column 3. | Back history survives Ctrl+R? (not full reload; language reload only). |
| P-14 | `reopen-tabs` (session) | Transient transfer array `{tabs:[TabTransfer…],active}` written immediately before language reload, removed on read. | `src/main.ts:2930-2959` | P1 | Fixture run shows exactly the behavior in column 3. | Reload keeps tabs once. |
| P-15 | `pasted-drafts-offered` (session) | `"1"` after the first window has attempted automatic recovery of pasted drafts; prevents repeat recovery after language reload. | `src/main.ts:1973-2007` | P1 | Fixture run shows exactly the behavior in column 3. | Crash recovery appears once per window session. |
| P-16 | Window geometry file | tauri-plugin-window-state file for window `main` only; position/size/maximized, decorations excluded. | `src-tauri/src/lib.rs:498-506` | P1 | Fixture run shows exactly the behavior in column 3. | Geometry restored after restart. |
| P-17 | WebView2 profile / portable data | All page localStorage/sessionStorage live in the WebView2 user-data folder; portable mode redirects it to `<exe>/data`; identifier changes isolate it. | `src-tauri/src/lib.rs:248-262,468-473`, `README.md:61-64` | P1 | Fixture run shows exactly the behavior in column 3. | Portable settings travel with folder. |
| P-18 | `ui-language` file | One-line preference mirrored to native config dir for next-start WebView2 `--lang`. | `src-tauri/src/commands.rs:970-979` | P2 | Fixture run shows exactly the behavior in column 3. | File content equals preference. |
| P-19 | Draft cleanup points | Removed on explicit save, “Save again”, successful Save as, confirmed discard, closing a tab with discard, closing a pasted tab unchanged, and when draft equals disk. | `src/main.ts:595-597,962,1018-1039,1245,2509-2515`, `src/main.ts:677-680` | P0 | Fixture run shows exactly the behavior in column 3. | After each path, key absent. |
| P-20 | Cross-window sync | `storage` events apply reader prefs, theme and rating state in other windows. Tabs and history are per window; drafts are shared but guarded by BroadcastChannel + 250 ms peer query. | `src/prefs.ts:97-105`, `src/theme.ts:39-42`, `src/rating.ts:84-98`, `src/main.ts:636-666` | P1 | Fixture run shows exactly the behavior in column 3. | Change theme in window A: B updates; tabs do not. |

---

## 3. Detailed behavior notes

These subsections collect the rules a table row flattens: order of precedence,
focus destinations, exact reasons for hiding, and the edge cases that generated
the most code in the reference implementation.

### 3.1 Keyboard handling and focus

**Global dispatch order.** One capture-phase `keydown` listener on `window`
handles window-level keys before anything else: `Esc` cancels a tab drag;
`Ctrl+W`/`Ctrl+F4` closes an overlay or tab; `Ctrl+Shift+W` closes the window;
`BrowserBack`/`BrowserForward` navigate history; `F5`/`Ctrl+R` reloads the file.
A second bubble-phase listener on `document` handles `F1`, dialogs, palette,
menus, tab keys, reading keys, `Ctrl+Shift+N`, `Alt+←/→`, `Ctrl+Shift+F`,
`Ctrl+F`, `Ctrl+H`, `F3`, `Ctrl+A`, `Ctrl+O`, `Ctrl+Shift+E`, `Ctrl+E`,
`Ctrl+S`, `Ctrl+B`, `Ctrl+I`, `Ctrl+P`, `Ctrl+Shift+C`, `Ctrl+=`, `Ctrl+−`,
`Ctrl+0` (`src/main.ts:4529-4704`).

**Modifiers and layouts.** Shortcut letters are matched by the character typed
(`e.key.toLowerCase()`), with the physical `KeyX` code used only when the key
types no ASCII character (Greek/Cyrillic), so `Ctrl+W` stays the W key on AZERTY
and Dvorak (`src/main.ts:3453-3458`). Digits for tab selection are matched by
physical `DigitN`, so `Ctrl+&` on AZERTY is tab 1; numpad digits only count when
Num Lock makes them digits; old environments fall back to `keyCode`
(`src/main.ts:2651-2659`). Almost every `Ctrl` shortcut is rejected when Alt is
also down, because Windows reports AltGr as Ctrl+Alt (`src/main.ts:4630-4704`).
IME composition (`isComposing` or keyCode 229) makes the key the IME’s
(`src/main.ts:3471-3472`).

**Focus behavior.**
- Opening the palette remembers the focused element and page selection, then
  restores both on close; a picked action runs after the dialog closes and after
  the focus is restored, so the action’s own focus target wins
  (`src/palette.ts:235-350,714-724`).
- Context menus opened by pointer take focus themselves (the menu element), not
  the first item, “as Windows menus do”; keyboard-opened menus focus the first
  item. Esc/Tab close and restore focus to the element focused before opening
  (`src/contextMenu.ts:54-128,151-153`).
- A peeking side pane returns focus to the element that had it before the peek
  when closed with Esc/toggle; when closed by choosing an item, focus is released
  to the document; when closed by an outside click, focus follows the click
  (`src/panes.ts:232-259`).
- A folded pane opening from the keyboard focuses its current item: active file,
  roving tree row, active outline entry, first row, then first control
  (`src/panes.ts:261-274`).
- Closing the last tab leaves a start page and focuses the window normally; the
  start page has no autofocus target.
- Help and keys close on backdrop click; the preview closes only when the same
  element receives press and release and no drag happened
  (`src/main.ts:4503-4509`, `src/lightbox.ts:161-168`).
- A table preview’s filter has focus after opening; Enter or ArrowDown returns
  focus to the table view (`src/tableView.ts:325-331`).

**Text-field ownership.** Shortcut handlers treat `input` (text/search/url/
email/tel/password/number), `textarea`, any contenteditable, and anything inside
`.cm-editor` as typing targets. Single-key reading commands, `Ctrl+A`, and some
arrow shortcuts are suppressed there (`src/main.ts:3103-3110,3820-3829`).
`Ctrl+F` is the deliberate exception: it always opens the appropriate find UI,
and in edit mode always opens CodeMirror’s panel (`src/main.ts:4017-4022`).

**`Esc` priority.** In one keystroke: cancel tab drag; close palette (its own
`Esc` handler); close preview; close help/keys; close share/reading/
context menu; close rate card as “Later”; close a pane peek; clear folder search
or outline filter first. The table preview and section line have their own
earlier layers. This order is distributed across modules and is a major parity
risk; see the subtle-behavior list.

### 3.2 Error states

| Situation | What the user sees | What remains |
|---|---|---|
| File missing on open | Sticky? notice “Not found: name”; home shown only if nothing else is on screen. Recent entry removed; tree/search refreshed. | Existing document/error card stays; no blank window. |
| File unreadable (permissions, binary) | Centered error card with raw message; no reading time; notice if another doc is on screen. | App remains usable; tab can be closed. |
| File changed on disk while editing dirty | Sticky warning “File changed on disk. Saving will overwrite it.” + **Reload**. | Buffer untouched; save later asks conflict if mtime differs. |
| File deleted while open | Sticky warning ““name” is no longer on disk…” + **Save again**. Editing still possible. | Text stays; `Ctrl+S` writes it back (create). |
| Save failed | Error notice “Save failed: …”. | Buffer and dirty state stay. |
| Save conflict | Warning dialog “The file changed on disk since you opened it. Overwrite it with your version?” + **Overwrite** / **Keep theirs**. | Keep theirs leaves buffer dirty; overwrite writes. |
| Preview render failed | Error notice “Preview error: …” or “Diagram rendering failed: …”; Mermaid-specific error may show “Invalid Mermaid diagram” banner in place of that diagram. | Last good article stays; editing continues. |
| Editor chunk failed | Error notice “The editor could not be loaded: …”. | Reading works; edit mode does not open. |
| Export/copy failed | Error notice with raw error; PDF offers **Use the print dialog**. | Document unchanged. |
| Reveal in file manager failed | Error notice “Could not open File Explorer: …”. | No other effect. |
| New window failed | Error notice “Could not open a new window: …”. | Current window normal. |
| Language reload with un-carryable dirty tabs | Warning “Changing the language reloads the window. Discard the unsaved changes?” with Discard/Cancel. | Cancel leaves language unchanged. |
| Clipboard busy | Info “Another program is using the clipboard: try again in a moment.” | No document created. |
| Paste empty / image-only | Info “There is no text on the clipboard: copy some Markdown first.” / “The clipboard holds a picture, not text.” | No tab created. |
| Link to other computer | Info “A document cannot open a file on another computer: name”. | No lookup, no document. |
| Link to non-Markdown | Info ““name” is not a Markdown document.” + Open / Show in folder. | No navigation. |
| Unresolved local link | Notice “Not found: rel”. | No navigation. |
| Draft recovered | Info “Unsaved edits from a previous session were recovered.” + **Restore**. | File untouched until restore+save. |
| Pasted draft recovered | Info “Pasted text with unsaved changes was recovered…” (or N form), no question; tabs restored automatically. | Tabs appear with dirty dots. |

Sources: `src/main.ts:324-341,385-390,918-968,1210-1275,1566-1569`,
`src/notices.ts`, `src/lightbox.ts`, `src/mermaid.ts:61-73`,
`src/locales/en.ts:229-278`.

### 3.3 Empty states

- **Home/start page:** whole `#empty-state` with icon, title “No document open”,
  lead, Open, Paste, optional recents, three tips, version and key hint
  (`index.html:98-123`). It scrolls if the recent list is taller than the
  window (`CHANGELOG.md:302-303`).
- **Recent list absent:** section hidden entirely when there are no live entries
  (`src/main.ts:3952`).
- **No headings:** outline pane hidden and `#app.no-headings`; section line
  disabled too (`src/main.ts:267`, `src/sectionLine.ts:215-223`).
- **No folder files:** blank tree, no text message (`src/main.ts:3592-3598`).
- **No folder search matches:** “No matches” row (`src/main.ts:3636-3641`).
- **Empty table filter result:** “No row contains “q”” inside the table sheet
  (`src/tableView.ts:446-447`).
- **Empty find box:** counter “0/0”, no highlights (`src/search.ts:127`).
- **Empty palette:** “Nothing matches “q””, and for a bare `#` with no headings
  the empty message uses `#` (`src/palette.ts:579`).
- **No closed tabs:** Reopen item absent from menus and palette
  (`src/main.ts:2859`, `src/appCommands.ts:224`).
- **No document:** Share disabled, Save disabled, reading time/outline empty
  (`src/main.ts:392-417`).

### 3.4 Persistence keys and write timing

See §7 for the full table. Behavioral principles:

- Missing key means “default”, not “defaults written”. `reader-prefs`, `theme`,
  `lang`, pane widths and panel hidden state remove the key when the value
  returns to default (`src/prefs.ts:65-73`, `src/theme.ts:47-56`,
  `src/i18n.ts:74-77`, `src/panes.ts:96-99`).
- Pane hidden state is the exception: it writes `"0"` as well as `"1"`, so a
  missing key can only mean an older version (`src/panes.ts:225-230`).
- Positions, recents, drafts and collapsed folders store only meaningful
  entries; deleting the last entry removes the whole key
  (`src/positions.ts:26-33`, `src/recents.ts:29-32`, `src/drafts.ts:30-33`).
- Drafts are written 1 s after a keystroke, not per keystroke
  (`src/main.ts:606-609`).
- Positions are written 300 ms after scroll stops (`src/main.ts:3754-3763`).
- Pane widths are written once per drag, on pointer up
  (`src/panes.ts:321-327`).
- Reading history is written on every navigation event, not on a timer
  (`src/navHistory.ts:22-28,58-65`).
- Window geometry is native, not localStorage.

### 3.5 File watching and conflict handling

The watcher design is intentionally folder-based:

- **Document watch:** every distinct parent folder of every tab’s file is watched
  non-recursively. A watch on the file itself would die on an atomic save
  (write temp + rename over target) (`src-tauri/src/watcher.rs:28-35,65-90`).
- **Events:** 300 ms debounce. For each watched document touched by the batch,
  emit one event (`changed`, `renamed` + `to`, or `removed`). A path that still
  exists is always `changed`, whatever the OS event says. A gone path follows a
  rename chain; if the final target is a file, `renamed`; otherwise `removed`
  (`src-tauri/src/watcher.rs:98-130`).
- **Own-write suppression:** before writing, the page marks the lowercased path
  for 1.5 s; watcher echoes are ignored (`src/main.ts:139-142,4907-4908`).
- **Text equality:** even when an event is not suppressed, the page re-reads the
  file and compares text. If identical, only the remembered mtime is updated;
  this fixes OneDrive/antivirus attribute touches and avoids false “changed on
  disk” (`src/main.ts:368-383`).
- **Dirty buffer:** never auto-reloaded. Warning + explicit Reload.
- **Clean buffer:** auto-reload preserving the anchor.
- **F5 / Ctrl+R:** explicit reload; in edit mode only when clean.
- **Save conflict:** mtime check in Rust (`expected_modified`). If mismatch,
  dialog; overwrite passes `None`. A deleted file is only recreated when
  `create=true` (`src-tauri/src/commands.rs:252-298`).
- **Deleted-on-disk document:** stays on screen; `docGone` suppresses writes from
  task ticks; **Save again** re-creates; an Undo of the deletion clears the
  notice (`src/main.ts:1113-1233`).
- **Rename:** current document object path mutated in place so in-flight saves
  target the new path; all path-keyed stores renamed (`src/main.ts:1188-1208`).
- **Tree watch:** recursive, 400 ms, emits only for creates/removes/renames that
  pass listing rules; content writes ignored. Rearmed on window focus because
  Windows can drop a watch (`src-tauri/src/watcher.rs:132-169`,
  `src/main.ts:4935-4940`).
- **Drive roots:** never watched; checked on focus instead
  (`src-tauri/src/commands.rs:716-779`).

### 3.6 Line-ending and encoding preservation

- Read: Rust reads bytes as UTF-8 string, strips a leading BOM
  (`src-tauri/src/commands.rs:227-249`).
- Detect: editor sets `eol = "\r\n"` when the loaded content contains CRLF,
  otherwise `"\n"` (`src/editor.ts:527-537`).
- Edit: CodeMirror always works in LF internally; CRLF never enters the buffer.
- Write: `getContent()` / `snapshotText()` converts every LF to CRLF when the
  original EOL was CRLF (`src/editor.ts:505-509,577-581`).
- BOM: never re-added (`CHANGELOG.md:779-781`).
- Pasted text: `pastedText()` strips BOM, trailing NULs, and normalizes
  CRLF/CR to LF (`src/docs.ts:52-57`).
- Drafts are stored LF-normalized; restoring does not alter the file until save
  (`src/drafts.ts:6-11`, `src/main.ts:622-623`).
- A file whose EOL style changes on disk is re-read and adopts the new EOL
  (`src/editor.ts:527-530`).

### 3.7 Draft and recovery semantics

- **What is a draft:** a snapshot of an unsaved buffer, keyed by lowercased file
  path or `pasted:<id>`, with `{text, at, base}`. `base` is the file text the
  edits started from, used to distinguish stale/redundant drafts
  (`src/drafts.ts:6-11`).
- **When written:** 1 s after the last keystroke while dirty; immediately when a
  tab goes behind; immediately for changed pasted text when leaving edit mode or
  ticking a task (`src/main.ts:602-634,700-710`).
- **When removed:** save, Save again, Save as, confirmed discard, closing a tab
  with discard, closing an unchanged pasted tab, draft equal to disk, dismissing
  the offer (`src/main.ts:595-597,962,1245,2509-2515`, `src/main.ts:677-680`).
- **File draft offer:** on opening a file, if draft exists:
  - skip if another window reports editing that key (BroadcastChannel
    `editing?`/`editing!`, 250 ms timeout);
  - skip if the buffer already holds it;
  - skip if another tab of this window is editing the same file dirty;
  - delete silently if equal to disk;
  - otherwise sticky info notice “Unsaved edits…” + **Restore**. Dismissing
    deletes the draft. Restoring never saves; it loads text into editor.
  (`src/main.ts:668-698`)
- **Pasted-text drafts:** changed pasted text is kept whenever it differs from
  the text as pasted, even in reading mode. At next start, the first window
  (“main” label only) restores **all** changed pasted drafts automatically, once
  per session, after checking other windows; it inserts tabs after the front tab
  and reports “Pasted text … recovered” (`src/main.ts:1967-2030`).
- **Crash behavior:** a crash/forced close loses nothing that was dirty for
  >1 s; the next open offers file drafts, auto-restores pasted drafts. There is
  no autosave to the original file (`README.md:89`, `CHANGELOG.md:450-455`).
- **Quotas:** 20 drafts / 2 M chars; localStorage quota fallback drops oldest
  once (`src/drafts.ts:13-61`).

### 3.8 Reading-position semantics

- Key = lowercased canonical path. File rename moves the entry; case-only rename
  is a no-op (`src/positions.ts:15,61-69`).
- Anchor = `{id, offset}` where `id` is the nearest heading with id at or above
  the pane’s top (within 8 px), and `offset = -distanceAboveTop`. If no heading,
  `{id:"", offset: scrollTop}` (`src/state.ts:35-46`).
- Restore: if `id` exists, set scrollTop so the heading sits the same distance
  above the top as before. If `id` no longer exists, open at top. If no id,
  restore raw offset clamped to max scroll (`src/state.ts:48-60`,
  `src/main.ts:3729-3740`).
- At-top deletion: `id="" && offset<=0` deletes the entry; “scrolling back to the
  top forgets the position” (`src/positions.ts:35-45`).
- Capture points: 300 ms after scrolling stops; when leaving a document; when a
  tab goes behind; before a reload/reopen; before Back. Not while editing.
- Restore points: opening a file; reopen same file; reload; tab resume; Back/
  Forward restore; language reload transfer.
- Precedence: explicit `#anchor` or `--line` beats remembered position; Back/
  Forward explicit place beats all; otherwise remembered position; otherwise
  top (`src/main.ts:277-282`).
- The anchor is re-applied after Mermaid rendering only if the user has not
  moved since (`src/main.ts:295-303`).
- Text-size/width changes capture and restore the same anchor, so reflow does
  not move the reading place (`src/main.ts:1511-1524`).
- A remembered heading from pre-1.15.1 is found by legacy id
  (`src/state.ts:21-33`).

### 3.9 Link resolution

1. Strip fragment at first `#`; decode fragment safely. Nothing before `#` =
   in-page jump.
2. Any `scheme:` match: open only `http`, `https`, `mailto`, `tel` externally;
   others ignored.
3. Relative target with a document folder: Rust `resolve_doc(dir, rel)`.
   - Reject non-absolute/relative `dir` (pasted text has none).
   - Reject paths with a root/prefix of their own (UNC, device, other drive),
     drive-relative paths, and `..` that escapes the volume/share
     (`src-tauri/src/commands.rs:360-394`).
   - Candidates in order: `rel` if file; `rel.<md ext>` for all 8 extensions;
     `rel/index.<ext>`, `rel/README.<ext>`, `rel/readme.<ext>`.
   - Canonicalize and return first existing.
4. If resolved and Markdown: open/navigate, applying anchor. If it is the same
   document, only anchor jump.
5. If resolved and image: decode test then preview.
6. If resolved other file: notice + Open (whitelist) or Show in folder.
7. If not resolved: try `resolve_path(dir, rel)`; if it is a directory accepted
   by `list_tree`, root the file pane there.
8. Else notice “Not found”.
9. Pasted text skips all local resolution and offers Save as.
10. Network-path targets are refused before any disk lookup.

Placement: `whereFrom(e)`/`whereFromKeys(e)` as in T-12 (`src/where.ts:5-16`).
`file.md#section` carries the anchor into the new tab/window; `--line` and
anchors flash the landing (`src/main.ts:167-174,279-292`).

### 3.10 Math delimiter rules (exact)

The implementation replaces the KaTeX plugin’s block/inline rules, keeping its
renderers, to match GitHub’s reading of `$` (`src/renderer.ts:228-254`).

**`$$` block opener** (`mathBlock`, `src/renderer.ts:268-308`):
- Must start at the first non-space of a line (`bMarks + tShift`).
- `$$$` does not open (third char `$`).
- Must not be indented four columns from the block indent (indented code wins).
- The first `$$` after the opener must end a line: the same line (only spaces/
  tabs after it) or a later line before a blank line, a line less indented than
  the block, or the end of the list item/quote. The closing line must also have
  only whitespace after its `$$`.
- An escaped `\$$` is not a closer (odd number of preceding backslashes).
- If no closer: the line is ordinary prose, so `$$ 100 l’uno` never swallows the
  document.
- Content is the opener text after `$$`, the continuation lines, and the closing
  line before `$$`, blank parts filtered and joined by `\n`.

**Inline display `$$…$$`** (`inlineDisplayMath`, `src/renderer.ts:310-337`):
- Allowed anywhere inline, including inside a paragraph and after text.
- Not after `$`, `\`, or when the third char is `$`.
- Closing `$$` must exist before end of inline range; after close must not be
  `$`; content must be non-empty; no space/tab **immediately** inside either end
  (line break is allowed).
- Rendered as `<span class="katex-block">`, CSS `display:block`, so it does not
  split the paragraph with invalid `<p><p>`.
- Text after the closing `$$` remains in the paragraph.

**`$…$` inline** (`inlineMath`, `src/renderer.ts:339-390`):
- Not after `$`, `\`, word char (`\w`: letter/digit/underscore), not at end of
  input, and next char must exist and not be whitespace.
- `$` followed by backtick uses the `` $`…`$ `` form and ends at the first
  `` `$ ``; the backticks stay in the token and the renderer removes them.
- Closing dollar: next unescaped `$`; not immediately after opener; next char
  after close not `$`; char before close not whitespace; char after close not a
  word char.
- If no close, the `$` is literal text, not a pending formula.
- `<span>$</span>` immediately before suppresses math, GitHub’s way to write a
  dollar next to math.

**Fences:** ` ```math ` or `~~~math `, with optional title after `math`; only
when KaTeX is loaded. Long display math scrolls sideways, does not widen the
column (`src/styles/markdown.css:608-623`).

**Dollar examples that must stay text:** `$ x $`, `$5 and $10`, `US$5`,
`5$`, `\$x\$`, an empty `$$ $$`, `$$$$`, `$$ 100`, `$$ 100 l’uno, oppure $$ 50`.
**Examples that must render:** `$x$`, `$$E=mc^2$$`, a closed multi-line `$$`
block, a `math` fence, `` $`\sqrt{2}`$ ``, inline `$$…$$` inside a sentence.

### 3.11 Table behavior

**In the document**
- Every table is wrapped in `.table-block` (actions + horizontal bar) and
  `.table-scroll` (sideways scroller); the table itself is untouched
  (`src/renderer.ts:905-931`).
- A table/diagram whose **min-content** width exceeds the reading column is
  marked `wide` and allowed to use up to the pane width; a table that can fit by
  wrapping stays in the column. Only direct children of the article break out; a
  table inside a list/quote/callout keeps its scroller (`src/wide.ts:1-17,99-191`).
- A table that fits its box gets `fits`; its header row sticks to the pane top
  and leaves with the last row. A table still wider gets `hscroll`; a proxy
  scrollbar sticks to the pane bottom while the table is in sight, and the
  header is translated down with a scroll-driven animation so it scrolls
  sideways with columns (`src/tableScroll.ts:1-23,34-122`).
- Nested tables keep the plain scroller (`src/tableScroll.ts:41-44`).
- Jumps into tables add the header height to the top offset
  (`src/tableScroll.ts:173-206`).
- Print: scroller removed, header wraps, every table printed whole, wide tables
  scaled via `--print-zoom`; below 0.3 cells break long words
  (`src/styles/print.css:128-159`, `src/wide.ts:212-265`).

**Full-window table preview**
- Operates on a **copy**: ids and `data-source-line` removed, task boxes
  disabled, links mapped back to originals (`src/tableView.ts:151-176`).
- Header row sticky within one 2-axis scroller; first column frozen only if
  table overflows, first-column width ≤35% of view, and no `rowspan`/`colspan`
  in first column (`src/tableView.ts:176,285-295`).
- Filter: every word must occur in the row’s concatenated text, case/accent
  folded; matches highlighted; widths locked while filtering; count localized;
  empty message. Clearing filter restores rows (`src/tableView.ts:410-520`).
- Sort: only a single tbody, ≥2 rows, a `<thead>`, no spanning body cells, and
  the last header row’s cell must cover one column and contain no link/button/
  input. Cycle asc → desc → original. Empty cells bottom; numbers before words;
  numeric if ≥80% of filled cells parse; else dates if ≥80%; else text.
  Decimal mark inferred from cells, fallback language (`en`/`zh-CN` point,
  others comma). Date order inferred from values >12, fallback en-US English =
  month/day, else day/month. Year 00–69 → 2000s, 70–99 → 1900s. Interspersed
  text in a numeric column is compared as text, so numbers sort before it
  (`src/tableView.ts:522-702`).
- Copy table (preview): TSV of visible rows in current order
  (`src/tables.ts:5-12`).
- Opening mode: 100% if the table cannot fit at min-content, else Width
  (`src/tableView.ts:182-199`).

### 3.12 Selection and copy semantics

- Reading `Ctrl+A`: selects article contents only (`src/main.ts:4659-4664`).
- Reading `Ctrl+C`: if selection inside article and focus isn’t a typing target,
  intercepts and writes HTML (for the selected range) + plain text; if the HTML
  path throws, writes plain text (`src/main.ts:4326-4334,3122-3131`).
- Context menu on a selection: Copy (rich), Copy as plain text, Find in document
  (reading only), Search the folder. Outside the document only a Copy item for
  a non-empty selection, unless the selection is in the editor
  (`src/main.ts:3173-3236`).
- Editor `Ctrl+C` is native CodeMirror copy; it does not go through rich-text
  conversion.
- Code-block copy copies `<code>.textContent` exactly; table copy copies
  visible rows normalized to single spaces per cell (`src/renderer.ts:875-883`,
  `src/tables.ts:5-12`).
- Rich copy strips all classes and inline-styles everything; plain flavor is
  Markdown source (`src/share.ts:528-577`).
- Selecting text then opening the context menu does **not** lose the selection:
  menu `mousedown` is prevented (`src/contextMenu.ts:140-142`).

### 3.13 Drag-and-drop semantics

- **Files onto window:** Tauri emits `drag-enter`/`drag-over`/`drag-leave`/
  `drag-drop` with paths and physical position. `enter` adds `.dropping`; drop
  filters Markdown paths. Multiple Markdown files open one tab each; first is
  front, rest background. Drop on tab strip: x position converted from physical
  to CSS by `devicePixelRatio`, `strip.indexAtClient(x)` decides index,
  subsequent files use following indices (`src/main.ts:4941-4957`).
- **Non-Markdown drop:** if exactly one image and editing, import to `assets/`
  and insert; otherwise ignored.
- **Tab drag:** threshold 5 px; only left button; close button excluded; press
  already activates tab (browser behavior). Drag is measured in current tab
  slots; neighbor yields at midpoint. Off-strip by 36 px or outside window sides
  tears off. Only tab drags the window instead (`src/tabs/strip.ts:430-686`).
- **Cross-window:** target strip shows ghost; moving window is DWM-cloaked while
  over another strip; drop sends `window-drag-drop`; source offers tab via
  `tab-offer`; target accepts/refuses with token; source removes tab only on
  acceptance; 15 s fallback restores it (`src-tauri/src/drag.rs`,
  `src/main.ts:2742-2850,4963-4969`).
- **Same file merge:** if target already has clean copy, activate it; if either
  copy is dirty, refuse with info and keep tab in source
  (`src/main.ts:2795-2806`).
- **Drag-to-scroll:** middle button anywhere or left on empty pane margin; 3 px
  threshold; capture after threshold; middle click on link after no movement
  opens background tab (`src/main.ts:3484-3542`).
- **Window drag by sole tab:** native move loop with Snap; if another app window's
  strip is under pointer, ghost and merge as above
  (`src-tauri/src/drag.rs:1-20,156-208`).

### 3.14 Multi-window and tab rules

- One process, many windows; each window independent tabs/history/panes.
- A file may be open in different windows; within a window it is unique.
- Launch routing order in S-03. A window with pasted text is **not** “idle on a
  start page” even though it has no file.
- `--new-window` bypasses routing entirely.
- Closing a window asks once for all tabs; closing a tab asks per tab.
- Language change carries tabs within a window via sessionStorage; windows are
  not merged.
- Tear-off and merge move a tab’s scroll, edits/undo, Back/Forward, find bar and
  outline filter. Pasted text carries its text and id. A start page carries
  nothing and is not reopened by Ctrl+Shift+T.
- A window with only a start page can be taken over by an outside launch, and a
  torn-off window on a start page is likewise idle and can be reused (it has
  no pasted text yet).
- `KEEP_VIEWS=6` limits retained rendered fragments per window.
- Window focus order (`focus` list in Rust) drives which window “used last” gets
  a launch; minimized windows are unminimized and focused when routed.

### 3.15 CLI rules

```
lmdv [--new-window] [file] [--line N]
```

- `--new-window` before or after file, before `--`; no file = empty window.
- `--line N`, `-l N`, `--line=N`; non-numeric value ignored (line stays null).
- `--` makes every later argument positional; `--new-window` after `--` is a
  filename.
- Unknown arguments beginning `-` ignored, not treated as file.
- Only the first positional path is used.
- Relative path resolved against the calling process cwd (the shell’s folder for
  a second instance, thanks to single-instance passing cwd).
- `Path::absolute` is used without requiring existence; `.`/`..` normalized.
- Missing file: full path still routed; page shows “Not found: name”.
- Existing file canonicalized with `dunce`; tab matching case-insensitive.
- Relative links inside a file are resolved against the **document folder**,
  never the process cwd (`src-tauri/src/commands.rs:352-394`).
- No `--version`, `--help`, `-h`, `--wait`, or `--new-tab` flag exists; unknown
  flags are silently ignored. `--new-window` is the only routing flag.
- Installer adds `lmdv` to `%LOCALAPPDATA%\Microsoft\WindowsApps`; Store
  build registers alias; portable is invoked by its own path.
Source: `src-tauri/src/lib.rs:171-204,562-611`, `README.md:284-298`.

### 3.16 Theme and reading settings

- **Themes:** Light (warm white `#faf9f5`), Sepia (warm paper), Dark
  (`#262624`), Black (`#000`), Auto (system light/dark). `data-theme` picks
  palette; `data-scheme` drives two-palette consumers (highlight.js, alerts,
  Mermaid, print). Auto listens to OS changes live and rerenders Mermaid.
  Theme switch cross-fades colors for 280 ms (disabled by reduced motion).
  (`src/theme.ts:1-70`, `src/styles/base.css:1-140`.)
- **Text size:** eight steps 13, 14, 15, 16, 17, 18, 20, 22 px. Ctrl+=/Ctrl+−
  step, Ctrl+0 resets to 16; Ctrl+wheel over document/editor steps; over panel
  changes panel size. Step buttons disabled at ends. (`src/prefs.ts:21-83`,
  `src/main.ts:4694-4726`.)
- **Font:** sans (Segoe UI Variable Text/Segoe UI/system) or serif (Georgia/
  Cambria/Times).
- **Line spacing:** compact 1.5, normal 1.7, relaxed 1.9.
- **Column width:** narrow 640, normal 760, wide 920, full none.
- **Panels:** small 12, normal 13, large 14.5 px.
- **Reset text settings:** resets size/font/spacing/width/panels but preserves
  `printBreakH1` (`src/prefs.ts:85-87`).
- **Remembering:** all persisted; a change in one window applies in others via
  `storage` and restores reading anchor (`src/prefs.ts:97-105`).
- **Not applied to print/export:** force 16 px sans 1.7.
- **Editor scale:** reader size × 0.875.
- **Reading position during reflow:** capture anchor, apply, re-mark wide
  blocks, restore (`src/main.ts:1511-1524`).

### 3.17 Language selection and override

- Resolution happens when `i18n.ts` module loads; every module may call `t()` at
  module scope (`src/i18n.ts:1-5,59-67`).
- Preferred-languages list is injected by a Tauri plugin before any page script:
  `window.__APP_LANGS__ = [...]` (`src-tauri/src/lib.rs:318-325`). This is
  Windows’ Settings > Time & language list, not `navigator.language`.
- Resolution: for each tag, `zh*` → `zh-CN`, `en*`/`en` → `en`, any primary
  `it/de/fr/es` → that; first match wins; no match → `en`. The UI language is
  not regional (no `pt-BR` → English unless English appears later).
- Override is a six-language + Auto segmented control in Aa; labels are
  endonyms (English, Italiano, Deutsch, Français, Español, 中文), Auto localized.
- Choosing a language stores `lang` and calls `location.reload()` after trying to
  carry tabs; `remember_language` mirrors it for WebView2 next start.
- Auto removes the key, so a later Windows language change is followed at next
  start/reload.
- HTML root `lang` is set; HTML export carries the same.
- Only the interface, help, menus, notices and native webview UI follow; document
  content is never translated.
- Dates/numbers/plurals use `Intl` with the current language.

### 3.18 Accessibility specifics

- Accessible names are derived from tooltips: the parenthesized shortcut is
  removed to `aria-keyshortcuts`; keys are translated for display but not for
  ARIA (`src/i18n.ts:108-190`).
- Icon-only buttons have names from `data-i18n-title`; glyph-only buttons (Aa,
  text-size A) get names explicitly; the static HTML also carries English
  fallback labels before localization (`index.html`, `src/i18n.ts:218-237`).
- Tab strip: `tablist` + `tab`, roving tab stop, selected state, reading time
  through `aria-description`. Tabs are not buttons; Enter/Space activates.
- Tree: full ARIA tree pattern; folder `aria-owns` its group; groups have unique
  ids; level/posinset/setsize; roving tab stop.
- Menus: `menu`/`menuitem`; no typeahead; arrow wrap; Home/End; Esc/Tab close
  and restore.
- Dialogs: native modal; focus trap; `aria-label` on dialog; backdrop close for
  help/keys only.
- Notices: polite status for info/success/warning, assertive alert for error;
  clear-before-write to repeat identical messages; save/undo/copy announced.
- Palette: combobox pattern with `aria-activedescendant`, listbox, option,
  group labels, live result count.
- Media: images/diagrams are buttons with labels; tables are not buttons.
- Table preview: region with instructions, live filter count, `aria-sort`.
- Section line: intentionally hidden from AT and tab order; equivalent
  navigation via outline, `.`/`,`, palette `#`, screen-reader heading keys.
- Reduced motion honored for scroll behavior, landing highlight, CSS animations.
- Focus is always visible; peeking panes and menus restore focus.

### 3.19 Windows-specific behavior and Linux substitutes

| Windows behavior | Where | Linux substitute / gap |
|---|---|---|
| `.md` associations, Open With, Explorer “New → Markdown document” | `tauri.conf.json`, `nsis/hooks.nsh`, MSIX resources | XDG MIME type + `.desktop` `MimeType=text/markdown;`; `xdg-mime default`; template in `~/Templates` (Desktop Environment dependent). Record as `substitute`. |
| `lmdv` command in WindowsApps, Store execution alias | README/installer | Install launcher script/symlink into `~/.local/bin`; AppImage/Flatpak `Exec` alias. |
| Single-instance mutex | tauri-plugin-single-instance | Unix socket or D-Bus name; forward argv+cwd. |
| Recent items + taskbar jump list (`SHAddToRecentDocs`) | `commands.rs:321-336` | `RecentManager`/`gtk_recent` or `xdg-desktop-portal` recent; no jump list on Linux/Wayland; optional `.desktop` actions. `substitute`. |
| Windows 11 Snap layouts over Maximize | `caption.rs` | Wayland compositor tiling/snap; X11 `_NET_WM` hints. No custom zones; use system decorations or compositor snap. `gap/substitute`. |
| Native title bar zones + DWM cloak for tab drag | `caption.rs`, `drag.rs` | Compositor move/resize and drag gestures; tab merge via in-process/D-Bus protocol, no cloak. `substitute`. |
| `WM_NCHITTEST`/caption hover | `caption.rs` | Not applicable; theme-drawn window controls only if server-side decorations are disabled; otherwise use CSD. `gap`. |
| WebView2 `PrintToPdf` | `commands.rs:866-943` | Typst (planned) or print-to-PDF via CUPS/GTK; must preserve A4, backgrounds, header/footer, H1 breaks. |
| WebView2 spellcheck dictionary | WebView2 | Hunspell/enchant with language matching; suggestions context menu. `substitute`. |
| WebView2 `--lang` for native menus/spellcheck | `lib.rs:474-487` | Locale env (`LANG`, `LANGUAGE`) or toolkit locale; no separate webview. |
| Windows clipboard CF_UNICODETEXT/DIB | `clipboard.rs` | Wayland/X11 clipboard (text + image MIME); busy retry semantics may differ. |
| WebView2 context-menu pruning | `lib.rs:327-447` | Toolkit text context menu; spelling suggestions; document menu is app-drawn. |
| `dunce` canonicalization and drive/UNC rules | `commands.rs`, `paths.ts` | Use `std::fs::canonicalize`; replace drive/share scope with mount/root scope, refuse `//host/share` and device paths; Linux paths are case-sensitive (do **not** lowercase for identity blindly). |
| Case-insensitive path identity | `paths.ts`, `same_file` | Linux case-sensitive: exact path comparison; optionally detect case-insensitive filesystems per mount. This is a behavior change to document. |
| `\` path separators, drive roots, `C:` handling | throughout | POSIX `/`, root `/`, no drive letters; URL paths use `/`. |
| `%APPDATA%` config, `%LOCALAPPDATA%` command | installer, language file | `$XDG_CONFIG_HOME`, `$XDG_DATA_HOME`, `$XDG_CACHE_HOME`. |
| Portable `lmdv-portable.txt` + `data` beside exe | `lib.rs:248-262` | Portable mode as AppImage: detect marker next to AppImage or use `--portable`; data dir beside binary. |
| Windows file locking/atomic-save behavior | watcher design | inotify also benefits from folder watches; atomic rename handling differs (IN_MOVED_TO/FROM). |
| File Explorer “Show in folder” | opener plugin | `xdg-open` on parent + select via D-Bus FileManager1 (`ShowItems`) where available; else open folder. |
| Store rating dialog | `commands.rs:988-1063` | Not applicable; omit or link to a project page. `gap`. |
| Jump list/recent/`SHARD_PATHW` | `commands.rs` | as above. |
| WebView2 zoom/page print guards | main key handlers | Native text layout ignores them; keep documented shortcut behavior. |
| Windows high-DPI devicePixelRatio conversions for drop/tear-off | `main.ts`, `drag.rs` | Wayland fractional scaling; convert logical/physical consistently. |

---

## 4. Subtle behaviors easy to miss

1. **A file “change” is defined by text, not mtime.** OneDrive reparse-point
   touches and antivirus attribute writes only refresh the stored mtime; a
   notice appears only when the text differs (`src/main.ts:368-383`).
2. **Own writes are ignored for 1.5 s by lowercased path.** The watcher echo of
   `Ctrl+S` must not trigger a reload (`src/main.ts:139-142,4907-4908`).
3. **Reloading a deleted document can bring it back.** `write_markdown(create)`
   recreates a file that is gone; plain save never does, so a rename race cannot
   leave a ghost old-name file (`src-tauri/src/commands.rs:269-297`).
4. **The file-changed check is on mtime, not content.** A save over a file whose
   mtime changed but text is identical still asks for overwrite; separate
   `sameOnDisk` text comparison governs reload notices
   (`src/main.ts:373-383`, `src-tauri/src/commands.rs:281-286`).
5. **`Ctrl+F` is never the browser’s find.** The app always intercepts it, even
   with no document, and routes it to reading or editor find
   (`src/main.ts:4645-4648`).
6. **`Ctrl+P` is swallowed without a document** to stop WebView2 printing the
   empty UI; `Ctrl+Shift+P` no longer prints, it opens the palette
   (`src/main.ts:4599-4603,4684-4687`).
7. **`Ctrl+S` is always swallowed**, even while reading or on the start page,
   then does nothing unless editable/pasted (`src/main.ts:4674-4677`).
8. **`Ctrl+B`/`Ctrl+I` change meaning inside the editor** (bold/italic) and
   only toggle panes elsewhere (`src/main.ts:4678-4683`,
   `src/editor.ts:407-423`).
9. **Tab digits are by physical key, not character**, so AZERTY Ctrl+& is tab 1;
   numpad digit only with Num Lock; editor Ctrl+1–3 wins first
   (`src/main.ts:2651-2659`).
10. **A single `g` does nothing after 500 ms**; `gg` is not a browser-style
    “go” prefix with a timeout indicator (`src/main.ts:3865-3874`).
11. **`j`/`k` accumulate from the target, not the smooth position.** Five
    quick presses reach 400 px even if the first animation has barely moved
    (`src/main.ts:3831-3845`).
12. **The outline activation line slides down over the last viewport.** Without
    this, the last short sections can never become active
    (`src/toc.ts:45-55`).
13. **Clicking an outline entry pins the highlight** until the user scrolls
    (wheel/touch input releases immediately), so the spy does not override the
    click while the smooth scroll runs (`src/toc.ts:78-91,133-149`).
14. **`Esc` does not close the palette**; it closes the dialog only if the
    palette handles it, and `Ctrl+W` closes palette before tab
    (`src/palette.ts:799-802`, `src/main.ts:2615-2632`).
15. **Section line disappears during fast scrolling**, not merely when the
    heading is out of view; it re-appears only after 120 ms of slow scrolling
    (`src/sectionLine.ts:45-69,256-277`).
16. **Section line path omits the document title only when there is exactly one
    H1 and it is first.** A document with two H1s keeps both in the path
    (`src/sections.ts:20-31`).
17. **A heading with an id that collides with app UI is renamed
    `user-content-…`**, while the outline/TOC links follow; without this a
    heading “Toolbar” disappears or steals app styles
    (`src/renderer.ts:513-561`, `CHANGELOG.md:380-385`).
18. **DOMPurify drops ids that are document properties** (`links`, `images`,
    `title`, `name`), so the renderer restores heading ids from `data-hid`
    before the reserved-id rename (`src/renderer.ts:466-475`).
19. **Old heading anchors are kept in `data-legacy-id` and old links retarget**
    only if no current element owns the old id
    (`src/renderer.ts:498-511`).
20. **Task text is deliberately not a label.** `markdown-it-task-lists` is
    configured `label:false`, and a separate aria-label names the box; a click
    on text must not change the file (`src/renderer.ts:74-77,832-855`).
21. **Ticking a task in reading mode writes the file immediately**, the only
    exception to explicit-save, and the notice offers **Undo** which writes the
    previous text back. In a deleted file, the tick is reverted in the UI
    (`src/main.ts:1081-1159`).
22. **Changing a task back to the exact text it had no longer counts as
    changed** for pasted text; a pasted tab can be clean even after edits
    (`src/main.ts:163-165,2384-2387`).
23. **Closing an unchanged pasted tab asks nothing and deletes its draft**, but
    closing the **window** always asks for any pasted text, changed or not
    (`src/main.ts:2412-2438,2503-2515`).
24. **Pasted-text crash recovery is automatic, not offered.** It restores all
    changed pasted drafts once per session, only in the `main` window, skips
    ones another window has open, and never asks a question that could steal the
    notice bar from a file draft (`src/main.ts:1967-2030`).
25. **Two windows share drafts but query each other before offering one.** A
    250 ms BroadcastChannel handshake prevents “Restore” for a draft that is
    live in another window (`src/main.ts:636-666`).
26. **The draft’s `base` is the current file text, not the last-saved buffer.**
    A draft can become “equal to disk” after an external save and is deleted
    silently (`src/main.ts:622-623,677-680`).
27. **A reopen of an already-open edited file must not read disk into the
    preview**, or the editor’s baseline moves and a later save could overwrite
    an external change; `keepEditedDoc` handles this (`src/main.ts:343-366`).
28. **Opening a file in another tab while a `--line` is requested queues it
    behind the tab switch**, and the latest queued request wins
    (`src/main.ts:177-214`).
29. **A launch for a file already in a tab queues an “activate” only after the
    page has both listened and shown itself**; earlier launches are queued in
    order, avoiding duplicate tabs during Explorer bursts
    (`src-tauri/src/windowing.rs:148-171,567-650`).
30. **A window with pasted text is not “idle”.** Launch routing will use the
    last-used window, not replace the pasted tab, until that tab closes
    (`src-tauri/src/windowing.rs:76-86,449-455`).
31. **`--new-window` with a missing file still creates a window**, and its page
    shows “Not found”; routing does not reject it (`src-tauri/src/windowing.rs:706-752`).
32. **Relative CLI paths use the shell cwd, not the app install dir**, because
    single-instance forwards `cwd` and parse uses it (`src-tauri/src/lib.rs:490-495,190-199`).
33. **Dragging the only tab moves the whole window**, not a tear-off; dropping it
    on another strip merges windows (`src/tabs/strip.ts:552-558`).
34. **A tab dragged onto a window that already has the same file is merged only
    if both copies are clean**; either dirty copy refuses the move with an info
    notice (`src/main.ts:2795-2806`).
35. **A tab dragged out has a 15 s pending window**; if the new window never
    acknowledges, the tab reappears in the source window
    (`src/main.ts:2742-2782`).
36. **Tearing off a tab positions the new window so the grabbed point stays
    under the pointer**, not at the window’s top-left
    (`src/tabs/strip.ts:635-642`).
37. **Tab strip widths freeze while the pointer stays on the strip after a
    close**, so the next close button does not move under the pointer
    (`src/tabs/strip.ts:260-276`, `src/main.ts:2520`).
38. **The active tab’s reading time is measured before being shown and is the
    first thing dropped** when any tab is squeezed; the hover card keeps it
    (`src/tabs/strip.ts:306-367`).
39. **Section line click swallows the second press of a double-click for
    500 ms**, so a double-click does not select text under the fading line
    (`src/sectionLine.ts:403-430`).
40. **Table preview first-column freeze is conditional on width ≤35% and no cell
    spans**; otherwise no freeze at all (`src/tableView.ts:285-295`).
41. **Table filter widths are locked only when no `<colgroup>` exists**, and
    locking adds a temporary colgroup/table-layout fixed; clearing restores
    (`src/tableView.ts:452-482`).
42. **A number column can be sorted as text** if fewer than 80% of filled cells
    parse; mixed cells sort numbers before words, not by a numeric extraction
    (`src/tableView.ts:615-626`).
43. **Decimal mark is inferred from the column, not the UI language**, unless no
    cell is decisive; en and zh-CN default to point, others comma
    (`src/tableView.ts:642-658`).
44. **Date order is inferred from values over 12**, not only locale; American
    English is the only locale defaulting month-first
    (`src/tableView.ts:671-702`).
45. **A `$$` block needs a closing `$$` that ends a line**, and a blank line
    cancels it; this is the fix that stops `$$ 100` from eating the document
    (`src/renderer.ts:275-308`, `CHANGELOG.md:67-71`).
46. **Inline `$$…$$` renders as a block-level span**, not a `<p>`, preventing
    invalid nested paragraphs and splitting the sentence
    (`src/renderer.ts:240-246`).
47. **`$` immediately after a word character is a dollar**, and a closing `$`
    followed by a word character is not a closer, so `US$5` and `$5 and $10`
    stay text (`src/renderer.ts:339-390`).
48. **`$`…`$` (GitHub’s backtick form) may contain a literal `$`**, because it
    ends at the first `` `$ `` (`src/renderer.ts:377-380`).
49. **Line breaks are CommonMark, not GFM hard breaks**; a single newline is a
    space unless two trailing spaces or a backslash (`src/renderer.ts:58-72`,
    `samples/commonmark-math.md:7-16`).
50. **Typographer’s replacements are disabled**, so `(c)`, `(tm)`, `--`, `...`
    are literal; only quotes curl (`src/renderer.ts:124-126`).
51. **Autolinking is deliberately narrow**: `README.md` and `main.rs` are not
    links, `www.` is, and a `www.` preceded by `. / \ @ -` is not
    (`src/renderer.ts:103-122`).
52. **HTML tagfilter makes dangerous tags visible as text**, rather than
    stripping them, so a sentence mentioning `<title>` is readable and cannot
    swallow the rest of the page (`src/renderer.ts:145-161`).
53. **Unknown tags are hidden, matching GitHub**, but raw HTML is otherwise
    allowed and sanitized (`src/renderer.ts:149-161,429-437`).
54. **A `<track>` causes `crossorigin=anonymous` on its media element** so
    subtitles load from the asset protocol’s different origin
    (`src/renderer.ts:667-677`).
55. **Video/audio elements are swapped by exact markup, not recreated**, so
    playback continues while typing; the fresh copy is released from fetching
    (`src/media.ts:11-75`).
56. **`applyScheme` rewrites `<picture>` media queries**, not just
    `#gh-dark-mode-only`; a `not (prefers-color-scheme: dark)` query still works
    because it becomes `(width < 0px)` (`src/renderer.ts:759-784`).
57. **A network path is refused even as an image**, because looking it up would
    make Windows contact that computer immediately
    (`src/reach.ts:9-24`, `src/renderer.ts:585-610`).
58. **App-protocol strings in attributes and `<style>` text are removed before
    the live DOM**, not after load, so an image cannot start fetching
    (`src/renderer.ts:416-425`, `src/reach.ts:53-69`).
59. **The PDF header is injected as a literal `@page` rule** because CSS
    `string-set` is unimplemented; it is removed after the print
    (`src/main.ts:1340-1353`).
60. **Printing a dark theme briefly rerenders Mermaid light and waits for
    `<picture>` decode**, then restores dark; the same path runs for a
    WebView2-initiated print (`src/main.ts:1355-1403,4728-4738`).
61. **HTML export drops `#gh-dark-mode-only` images and any link left empty**,
    because output is light paper (`src/share.ts:87-95`).
62. **HTML/DOCX export converts asset URLs to `file:///`**, preserving fragments
    and handling UNC host names and folder names containing `#`
    (`src/share.ts:137-174`).
63. **DOCX output converts task lists to ☑/☐ and alerts to colored
    emoji/symbols** because Word drops inputs and SVG
    (`src/share.ts:287-343`).
64. **Rich copy turns code blocks into single-cell tables** so Word/Outlook
    shade them reliably, and hardens newlines/tabs into `<br>`/NBSP
    (`src/share.ts:479-506,435-477`).
65. **The table preview is a copy with ids removed**, so copy/sort/filter never
    mutate the document and do not create duplicate ids
    (`src/tableView.ts:151-176`).
66. **Table preview links dispatch synthetic clicks on the original anchors**,
    preserving modifiers, then close the preview
    (`src/tableView.ts:376-400`).
67. **The table preview filter folds accents per character**, building a map
    from folded offsets to source offsets so highlights stay correct
    (`src/tableView.ts:484-520`).
68. **A filtered table’s column widths are locked** so typing does not make the
    table jump; the lock is a generated `<colgroup>` and fixed layout, restored
    when the filter empties (`src/tableView.ts:452-482`).
69. **Omitting `sizes`/`srcset` in export is deliberate**: asset URLs would win
    over the inlined `src` (`src/share.ts:102-121`).
70. **The window can never navigate away.** Even a form, image map or SVG link
    is intercepted; http/https/mailto/tel open externally
    (`src-tauri/src/lib.rs:353-402`, `CHANGELOG.md:386-388`).
71. **WebView2’s native menu survives only in text fields and the editor**, and
    even there loses Share/Print/Writing direction/Import passwords; spelling
    suggestions stay (`src-tauri/src/lib.rs:327-447`).
72. **Browser accelerator keys are disabled**, so Ctrl+G/J and F7 cannot reach
    Edge UI (`src-tauri/src/lib.rs:377-400`).
73. **Closing a window tells Rust `closing=true`**, so a file opened from outside
    during the prompt goes to another window rather than being lost
    (`src/main.ts:4854-4855`, `src-tauri/src/commands.rs:192-199`).
74. **Window routing is order-sensitive and claimed under one lock**, including
    “opening” reservations, so a burst cannot open the same file twice
    (`src-tauri/src/windowing.rs:127-180`).
75. **A new window created by a tear-off can be born hidden and follow the
    pointer**, with the source window DWM-cloaked over another strip; after a
    failed merge it uncloaks (`src-tauri/src/drag.rs:1-20`).
76. **Window geometry persistence only applies to `main`**, not torn-off
    windows, and excludes decorations so old Windows title bars cannot return
    (`src-tauri/src/lib.rs:498-506`).
77. **Portable mode changes the app identifier**, not just the data dir, so it
    gets a separate single-instance identity and can run beside an installed
    copy (`src-tauri/src/lib.rs:468-473`).
78. **The Store rating card is claimed by writing `askedAt` before showing**, so
    only one window can show it and all windows see it as not due
    (`src/main.ts:3901-3905`, `src/rating.ts:62-67`).
79. **`Esc` on the rating card means Later**, not “never” and not dismissal
    without state (`src/main.ts:4609-4613`).
80. **A language change stores the preference, mirrors it for native WebView2 parts,
    then reloads the whole page; every module re-reads the dictionary at load
    (`src/main.ts:4457-4478`, `src/i18n.ts:59-67`).

---

## 5. Parity definition per area

“Done” is stated as observable behavior, not implementation. A row marked
`substitute` may use a different platform mechanism if the user-visible outcome
is equivalent; `gap` is a behavior that cannot be reproduced and must be
documented as absent.

| Area | Done means (Linux parity gate) | Platform-specific substitutions |
|---|---|---|
| R Reading | Open, read, scroll, find, remember position, reload and copy behave as specified on X11 and Wayland, at 480–4K widths; all reading keys and Ctrl+wheel work; a render error never blanks the article. | None. Space/Shift+Space must be explicitly verified because the app relies on native scrolling (see §3.1 and R-07). |
| T Tabs/windows | Multiple tabs and windows, one file per window, background/foreground placement, close/reopen, all tab state, state carrying, routing of launches, save prompts. | Single-instance transport is a socket/D-Bus substitute. Window state persistence may use the compositor’s own placement instead of a plugin; it must still restore size/position where the compositor allows. |
| E Editor | Split edit mode, explicit save, EOL/BOM, conflict prompt, drafts, format bar, slash menu, all editor shortcuts, find/replace, images, spellcheck toggle. | Native text editor (Rune) replaces CodeMirror; its undo/selection/multi-cursor semantics must match the documented shortcuts. Spellcheck uses Hunspell/enchant (`substitute`). |
| M Markdown | Every fixture in F-MATH/F-HTML/F-TASK/F-FRONT/F-TOC/F-MERMAID/F-CODE renders with the same visible result, including edge cases and security refusals. | Math: RaTeX/Typst-class engine may replace KaTeX (`substitute`); delimiters and visible layout must match. Mermaid may be replaced by another offline diagram engine only if the supported syntax and fallback are documented; otherwise mark `gap` for unsupported diagram types. |
| V Preview | Image/diagram zoom/pan/fit/copy/save; table reader with sticky header/first column, filter, sort, copy-as-shown; accessibility. | SVG rasterization through a Rust rasterizer; table rendering native. No browser APIs required. |
| X Share/export | Rich clipboard, print, PDF, self-contained HTML, DOCX; all output light/standard layout; page header/footer and H1 breaks. | PDF/print via Typst/CUPS (`substitute`); must keep A4, page numbers, header, backgrounds, table fitting. DOCX via a Rust OOXML writer (`substitute`); MathML→OMML conversion required for native equations. |
| F File browser | Rooting, depth/skip/caps, watching, collapsed state, keyboard tree, context menu, folder search, pane resize/fold/peek. | inotify replaces ReadDirectoryChangesW; path rules use POSIX roots/mounts. “Show in File Explorer” → `xdg-open`/FileManager1 (`substitute`). |
| O Outline/history | Scroll-spy, filter threshold, section path, section-line suppression, Back/Forward per tab. | Pointer side-button mapping is toolkit/compositor-specific; provide Alt+←/→ and BrowserBack/Forward equivalents. |
| C Palette | Fuzzy search over commands/headings/tabs/files/recents, prefix modes, placement keys, recent commands, modal key isolation, a11y. | None. |
| L Links/media | Wiki resolution, image/folder/other-file fallbacks, scheme handling, network refusal, local media. | MIME/desktop opening differs; `mailto:`/`tel:` via xdg-open (`substitute`). |
| S System/CLI | Exact CLI grammar and path resolution, single instance, launch routing, association, recent list, watchers, no navigation away, offline. | XDG MIME/desktop, socket single-instance, recent-manager portal, `xdg-open`; no jump list/Snap/MSIX/Store rating (`gap` or `substitute` as annotated). |
| W Window chrome | Draggable themed title bar, caption controls, right-click system menu, always-on-top, correct title, toolbar layout. | On Wayland the compositor owns window position/snap; use client-side decorations only if the compositor allows; otherwise system frame is an acceptable `substitute` if all controls/title remain reachable. Snap layouts are a `gap`. |
| D Drag/drop | File drops, tab reorder, tear-off, cross-window merge, drag-to-scroll, drop overlay. | Cross-window tab merge can be implemented in-process or via a desktop protocol; no DWM cloak needed. Tearing off may require a compositor move loop; fall back to “Move to new window” if impossible, marked `substitute`. |
| H Help/notices/rating | Start page, recents, help, keys overlay, notices, feedback, error cards. | Store rating card omitted on Linux (`gap`); replace with no prompt or a project link if desired. |
| A Accessibility | Every name/role/live region/focus behavior above; keyboard-only operation; reduced motion. | Toolkit/AT-SPI mapping; ensure Orca reads names, roles, states and live regions. |
| I i18n | Six languages, auto-selection from system locale, override, all strings, dates/numbers, help pages, export `lang`. | System locale environment (`LANG`/`LANGUAGE`) replaces Windows preferred-languages list; use `Intl`-equivalent CLDR data. |
| P Persistence | Same keys/semantics (or a documented migration), same eviction, same cross-window sync, crash recovery. | Config/data under XDG dirs; portable mode next to AppImage; native window geometry may be compositor-managed. |

---

## 6. Every keyboard shortcut and CLI flag

### 6.1 Global, document and tab shortcuts

| Keys | Action | Source |
|---|---|---|
| `Ctrl+K` | Command palette (reading; in editor inserts a link) | `src/main.ts:4041-4052`, `src/locales/en.ts:235` |
| `Ctrl+Shift+P` | Command palette (including editor) | `src/main.ts:4041-4052` |
| `Ctrl+O` | Open file(s), each in a tab | `src/main.ts:4665-4667` |
| `Ctrl+V` | Paste clipboard Markdown as a new document (outside editor/fields) | `src/main.ts:1938-1954,4282` |
| `Ctrl+E` | Toggle edit mode | `src/main.ts:4671-4673` |
| `Ctrl+S` | Save, or Save as for pasted text | `src/main.ts:4674-4677` |
| `Ctrl+F` | Find in document; in editor, editor find | `src/main.ts:4645-4648` |
| `Ctrl+Shift+F` | Search the folder | `src/main.ts:4641-4644` |
| `Ctrl+H` | Find and replace in editor (only while editing) | `src/main.ts:4648-4652` |
| `F3` / `Shift+F3` | Next / previous match (editor or reading find) | `src/main.ts:4653-4658` |
| `Ctrl+B` | Toggle file browser (bold in editor) | `src/main.ts:4678-4680` |
| `Ctrl+I` | Toggle outline (italic in editor) | `src/main.ts:4681-4683` |
| `Ctrl+P` | Print / Save as PDF | `src/main.ts:4684-4687` |
| `Ctrl+Shift+C` | Copy whole document as rich text | `src/main.ts:4688-4690` |
| `Ctrl+Shift+E` | Share menu | `src/main.ts:4668-4670` |
| `Ctrl+=` / `Ctrl++` | Larger text | `src/main.ts:4694-4696` |
| `Ctrl+−` | Smaller text | `src/main.ts:4697-4699` |
| `Ctrl+0` | Reset text size | `src/main.ts:4700-4702` |
| `F5` / `Ctrl+R` | Reload file from disk | `src/main.ts:3443-3451,4556-4559` |
| `Ctrl+Shift+N` | New window | `src/main.ts:4626-4628` |
| `Ctrl+Shift+W` | Close window | `src/main.ts:3467-3469,4544-4547` |
| `Ctrl+W` / `Ctrl+F4` | Close palette/preview/help/keys/drag, else tab | `src/main.ts:2615-2632,3463-3465` |
| `Ctrl+T` | New tab | `src/main.ts:2649-2650` |
| `Ctrl+Shift+T` | Reopen last closed tab | `src/main.ts:2649-2650` |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab | `src/main.ts:2642-2644` |
| `Ctrl+PgDn` / `Ctrl+PgUp` | Next / previous tab | `src/main.ts:2645-2648` |
| `Ctrl+1`…`Ctrl+8` | Tab 1…8 (by physical digit key) | `src/main.ts:2651-2659` |
| `Ctrl+9` | Last tab | `src/main.ts:2651-2659` |
| `Ctrl+Shift+PgUp` / `Ctrl+Shift+PgDn` | Move tab left / right | `src/main.ts:2645-2648` |
| `Alt+←` / `Alt+→` | Back / Forward | `src/main.ts:4629-4640` |
| `Shift+F10` / Menu key | Right-click menu | `src/main.ts:4304-4322`, help |
| `F1` | Help | `src/main.ts:4579-4586` |
| `?` | Keys overlay | `src/main.ts:4616-4619` |
| `Esc` | Close preview/menu/find/help/keys/peek; cancel tab drag | multiple, see 3.1 |
| `Ctrl+A` | Select article contents (reading) | `src/main.ts:4659-4664` |

### 6.2 Reading keys

| Keys | Action | Source |
|---|---|---|
| `j` / `k` | Scroll down/up 80 px | `src/main.ts:3851-3857` |
| `Space` / `Shift+Space` | Page down/up (documented; native scroller) | `index.html:258`, `src/locales/help/en.html:91` |
| `.` / `,` | Next / previous heading | `src/main.ts:3858-3861` |
| `g g` | Top (two presses ≤500 ms) | `src/main.ts:3865-3874` |
| `G` | End | `src/main.ts:3862-3864` |
| `n` / `N` | Next / previous find match | `src/main.ts:3875-3878` |
| `?` | Quick reference | `src/main.ts:4616-4619` |
| `F1` | Help | `src/main.ts:4579-4586` |
| `Esc` | Close find bar/preview/help/keys | multiple |
| Middle-drag / left-drag empty margin | Drag to scroll | `src/main.ts:3489-3542` |
| `Ctrl`+wheel over document/editor | Text size | `src/main.ts:4706-4726` |
| `Ctrl`+wheel over panel | Panel text size | `src/main.ts:4721-4723` |
| Double-click word | Select word (reading) | native behavior; `src/main.ts:4766-4769` |

### 6.3 Editor shortcuts

| Keys | Action | Source |
|---|---|---|
| `Ctrl+B` | Bold | `src/editCommands.ts:398` |
| `Ctrl+I` | Italic | `src/editCommands.ts:399` |
| `Ctrl+Shift+X` | Strikethrough | `src/editCommands.ts:400` |
| `Ctrl+Shift+M` | Inline code | `src/editCommands.ts:401` |
| `Ctrl+K` | Insert link | `src/editCommands.ts:402` |
| `Ctrl+1` / `Ctrl+2` / `Ctrl+3` | Heading 1/2/3 (again removes) | `src/editCommands.ts:403-405` |
| `Ctrl+Shift+7` | Comment line on Italian/German layouts (types `/` with Ctrl) | `src/editCommands.ts:434-437`, help |
| `Ctrl+/` | Comment / uncomment line | `src/editCommands.ts:437` |
| `Ctrl+F` | Find (editor panel, find row) | `src/editCommands.ts:55`, `src/editor.ts:416` |
| `Ctrl+H` | Find and replace (replace row) | `src/editCommands.ts:56`, `src/editor.ts:417` |
| `F3` / `Shift+F3` | Next / previous match | `src/main.ts:4653-4658` |
| `Alt+↑` / `Alt+↓` | Move line up/down | `src/editCommands.ts:430-431` |
| `Shift+Alt+↓` | Duplicate line | `src/editCommands.ts:432` |
| `Ctrl+Shift+K` | Delete line | `src/editCommands.ts:433` |
| `Ctrl+D` | Add next occurrence to selection | `README.md:265`, CodeMirror default keymap |
| `Alt+L` | Select line | `README.md:265`, CodeMirror default keymap |
| `Ctrl+Alt+↑` / `Ctrl+Alt+↓` | Add cursor above/below | `README.md:266`, CodeMirror default keymap |
| `Ctrl+Alt+G` | Go to line | `src/editor.ts:363-383`, help |
| `Tab` | Indent (never focus mode) | `src/editor.ts:271-275,419` |
| `Esc`, then `Tab` (≤2 s) | Leave editor with keyboard | `src/locales/help/en.html:235-239` |
| `/` at line start | Block menu | `src/editor.ts:277-319` |
| `Ctrl+Z` / `Ctrl+Shift+Z` / `Ctrl+Y` | Undo/redo (CodeMirror history) | `src/editor.ts:421` |
| `Ctrl+A` | Select all in editor | CodeMirror default |
| `Ctrl+C`/`X`/`V` | Native cut/copy/paste; image paste handled specially | `src/editor.ts:437-449` |
| `Mod-S` | Save (CodeMirror binding also routes to app) | `src/editor.ts:408-414` |
| `Esc` twice (selection/panel/menu open) then Tab | Leave editor | help |

### 6.4 Tab strip keyboard (focus on a tab)

| Keys | Action | Source |
|---|---|---|
| `←` / `→` | Previous / next tab (wraps) | `src/tabs/strip.ts:510-538` |
| `Home` / `End` | First / last tab | same |
| `Enter` / `Space` | Activate tab | same |
| `Delete` | Close tab | same |
| `Middle click` on tab | Close tab | `src/tabs/strip.ts:468-474` |
| Wheel over overflowing strip | Scroll strip horizontally | `src/tabs/strip.ts:488-496` |

### 6.5 File tree keyboard

| Keys | Action | Source |
|---|---|---|
| `↑` / `↓` | Previous / next visible row | `src/tree.ts:281-292` |
| `Home` / `End` | First / last visible row | same |
| `→` | Open collapsed folder; else enter first child | `src/tree.ts:294-300` |
| `←` | Close open folder; else go to parent | `src/tree.ts:301-307` |
| `Enter` / `Space` | Open file/folder (modifiers preserved) | `src/tree.ts:308-315` |
| `Ctrl+Enter` | Open file in new tab | `src/tree.ts:312`, README |
| `Shift+Enter` | Open file in new window | `src/tree.ts:312`, README |

### 6.6 Menus, palette, dialogs, preview and table

| Context | Keys | Action | Source |
|---|---|---|---|
| Any app menu | `↑`/`↓` | Previous/next item (wrap) | `src/menuKeys.ts:19-46` |
| Any app menu | `Home`/`End` | First/last item | same |
| Any app menu | `Enter`/`Space` | Activate focused item (native button) | same |
| Any app menu | `Esc`/`Tab` | Close, restore focus | same |
| Palette | `↑`/`↓` | Move highlight | `src/palette.ts:781-810` |
| Palette | `PageUp`/`PageDown` | Previous/next group (or page) | `src/palette.ts:736-752,788-792` |
| Palette | `Enter` | Run/open (with Ctrl/Shift placement) | `src/palette.ts:793-796` |
| Palette | `Ctrl+Enter` | Open file in background tab | `src/where.ts:13-17` |
| Palette | `Ctrl+Shift+Enter` | Open file in foreground tab | `src/where.ts:13-17` |
| Palette | `Shift+Enter` | Open file in new window | `src/where.ts:13-17` |
| Palette | `>` / `#` | Commands-only / headings-only prefix | `src/palette.ts:523-544` |
| Palette | `Esc` | Close | `src/palette.ts:799-802` |
| Palette | `F1` | Passes through; main opens help | `src/palette.ts:820-822` |
| Preview | `+` / `=` | Zoom in 1.25× | `src/lightbox.ts:567-571` |
| Preview | `−` / `_` | Zoom out | same |
| Preview | `0` | Fit | `src/lightbox.ts:569` |
| Preview | `W` | Fit width | `src/lightbox.ts:570` |
| Preview | `1` | 100% | `src/lightbox.ts:571` |
| Preview | Arrows | Pan 40 px (Shift 160) | `src/lightbox.ts:565-575` |
| Preview | `Esc` | Close (menu first) | `src/lightbox.ts:136-147` |
| Table preview | `Ctrl+F` | Focus filter | `src/tableView.ts:309-315` |
| Table preview | `+`/`=` | Zoom in | `src/tableView.ts:335` |
| Table preview | `−`/`_` | Zoom out | `src/tableView.ts:336` |
| Table preview | `0` or `1` | Actual size 100% | `src/tableView.ts:337` |
| Table preview | `W` | Fit width | `src/tableView.ts:338` |
| Table preview | Arrows/Page/Home/End/Space | Native scrolling | `src/tableView.ts:304-307` |
| Table preview | `Esc` (filter non-empty) | Clear filter | `src/tableView.ts:318-324` |
| Table preview | `Esc` (filter empty) | Close preview | dialog default |
| Help/keys dialog | `Esc` | Close | native `<dialog>` + handlers |
| Table filter | `Enter`/`ArrowDown` | Leave filter, focus table | `src/tableView.ts:325-331` |

### 6.7 Mouse gestures

| Gesture | Action | Source |
|---|---|---|
| Click file/recent/search/link | Open in current tab | `src/where.ts:7-11` |
| `Ctrl`+click or middle click | Open in background tab | same |
| `Ctrl+Shift`+click | Open in foreground tab | same |
| `Shift`+click | Open in new window | same |
| Middle click on tab | Close tab | `src/tabs/strip.ts:468-474` |
| Middle click external link after no drag | Open background tab | `src/main.ts:3525-3541` |
| Middle-drag document | Pan/scroll | `src/main.ts:3489-3542` |
| Left-drag empty margin | Pan/scroll | same |
| Double-click toolbar empty part | Maximize/restore | `src/tabs/chrome.ts:25` |
| Double-click pane edge | Reset pane width | `src/panes.ts:331` |
| Double-click preview (editing) | Place caret at word | `src/main.ts:4766-4795` |
| Wheel on tab strip | Horizontal scroll if overflow | `src/tabs/strip.ts:488-496` |
| `Ctrl`+wheel | Text size (document) or panel size | `src/main.ts:4706-4726` |
| Wheel in preview | Zoom around pointer | `src/lightbox.ts:546-555` |
| `Ctrl`+wheel in table preview | Text zoom | `src/tableView.ts:120-128` |
| Drag table preview with middle button | Pan | `src/tableView.ts:343-372` |

### 6.8 CLI flags and positional arguments

| Form | Behavior | Source |
|---|---|---|
| `lmdv` | Open/start app with a start page (or focus last window if running) | `src-tauri/src/lib.rs:171-204` |
| `lmdv FILE` | Route FILE to a tab/window; missing file shows Not found | `src-tauri/src/windowing.rs:80-111` |
| `lmdv --new-window` | New empty window on start page | `src-tauri/src/lib.rs:186-187,601-602` |
| `lmdv --new-window FILE` | New window for FILE even if open elsewhere | `src-tauri/src/windowing.rs:146,706-752` |
| `lmdv FILE --line N` | Open at 1-based source line N (block at top; editor caret if editing) | `src-tauri/src/lib.rs:187`, `src/main.ts:279-292` |
| `lmdv -l N FILE` | Same as `--line N` | `src-tauri/src/lib.rs:187` |
| `lmdv --line=N FILE` | Same | `src-tauri/src/lib.rs:188` |
| `lmdv -- FILE` | `--` ends options; FILE may begin with `-` | `src-tauri/src/lib.rs:185,604-606` |
| `lmdv --unknown FILE` | Unknown flag ignored; FILE still opened | `src-tauri/src/lib.rs:189` |
| `lmdv FILE1 FILE2` | Only FILE1 used; FILE2 ignored | `src-tauri/src/lib.rs:190-200` |
| `lmdv file.md --new-window` | Same new-window behavior (flag after file allowed) | `src-tauri/src/lib.rs:593-600` |
| `lmdv --new-window --line 40` | Empty new window; line ignored without file | parser (`path=None`), page start |
| `lmdv --version`, `--help`, `-h` | Not implemented; treated as unknown flags (ignored) | absence in parser |
| Portable marker | `lmdv-portable.txt` beside exe selects portable data/identifier (not a CLI flag) | `src-tauri/src/lib.rs:248-262` |
| `lmdv` command install location | `%LOCALAPPDATA%\Microsoft\WindowsApps` / Store alias / portable by path | `README.md:295-298` |

---

## 7. All persisted data

### 7.1 localStorage (shared by all windows, same WebView profile)

| Key | Shape | Cap / eviction | Written when | Removed when | Source |
|---|---|---|---|---|---|
| `reader-prefs` | JSON `{size,font,lineHeight,width,sideSize,printBreakH1}` | enums; size snapped to 8 steps | any Aa change | all values equal defaults | `src/prefs.ts:21-73` |
| `theme` | `"light"\|"sepia"\|"dark"\|"black"` | one value | theme choice | choosing system/auto | `src/theme.ts:28-56` |
| `lang` | `"en"\|"it"\|"de"\|"fr"\|"es"\|"zh-CN"` | one value | language choice | choosing auto | `src/i18n.ts:30-77` |
| `read-positions` | object lowercased path → `{id,offset,at}` | 100, oldest `at` evicted | 300 ms after scroll; leave doc; suspend tab | at-top capture; explicit forget | `src/positions.ts:6-77` |
| `recent-files` | array `{path,name,dir,openedAt}` | 12, newest first, one/path | successful open; rename; remove | clear; delete last entry | `src/recents.ts:11-54` |
| `drafts` | object key → `{text,at,base}` | 20 entries / 2,000,000 chars | dirty buffer 1 s after keystroke; tab suspend; pasted change | save/discard/clean; close unchanged pasted | `src/drafts.ts:13-89`, `src/main.ts:602-634` |
| `tree-collapsed` | array of lowercased collapsed dir paths | no cap | collapse/expand; auto-reveal | removing last entry writes `[]` | `src/tree.ts:10-47,180-194` |
| `tree-width` | integer px | 260–560 (room-clamped) | pane drag end; keyboard resize | width equals default | `src/panes.ts:20-24,90-99` |
| `toc-width` | integer px | 240–520 (room-clamped) | pane drag end; keyboard resize | width equals default | same |
| `tree-hidden` | `"0"`/`"1"` | one | pane toggle | never removed by toggle; missing = shown | `src/panes.ts:225-230` |
| `toc-hidden` | `"0"`/`"1"` | one | pane toggle | same | same |
| `palette-recent` | JSON array of command ids | 8 | command run from palette | never (until replaced) | `src/palette.ts:467-485` |
| `editor-spell` | `"0"` when off | one | spell toggle off | spell toggle on | `src/editCommands.ts:357-368` |
| `rate-prompt` | JSON `{firstRun,opens,state,askedAt}` | one object | first open; card shown; Later; No thanks | never | `src/rating.ts:9-99` |

### 7.2 sessionStorage (per window/tab session)

| Key | Shape | Written when | Removed when | Source |
|---|---|---|---|---|
| `nav-history` | `{back:Place[],forward:Place[]}`; Back capped at 50 | every record/commit/drop/rename | loaded at boot; overwritten | `src/navHistory.ts:13-104` |
| `reopen-tabs` | `{tabs:TabTransfer[],active:number}` | immediately before language reload | read once on startup | `src/main.ts:2930-2959` |
| `pasted-drafts-offered` | `"1"` | first attempt to auto-recover pasted drafts | never in that window session | `src/main.ts:1973-2007` |
| `reopen` (legacy) | removed at boot | — | removed at boot | `src/main.ts:4977` |

### 7.3 Native/WebView data

| Item | Location / shape | Written when | Purpose |
|---|---|---|---|
| Window geometry | tauri-plugin-window-state file (OS config dir), window `main` only | on move/resize/close | restore position/size/maximized; decorations excluded |
| WebView2 profile | Default user-data dir, or `<exe>/data` in portable mode | continuously | localStorage/sessionStorage, spellcheck dictionary, caches |
| `ui-language` | `<app config dir>/ui-language`, one line (`auto` or code) | on startup and language change | next-start `--lang` for WebView2 menus/spellcheck |
| Portable marker | `<exe dir>/lmdv-portable.txt` | user-created | switches data dir + identifier |
| Bundle/Store state | MSIX package identity / NSIS install | install time | Store detection, file associations, `lmdv` command |
| `lmdv-portable.txt` | next to exe | user-created | portable mode; deleting returns per-user |

### 7.4 What is *not* persisted

- Cursor/caret positions (except as part of a tab’s editor snapshot while it
  lives; not across app restarts).
- Open tabs (except transiently across a language reload).
- Back/Forward across a full app restart (only across the language reload).
- Scroll position for pasted, unsaved text (only draft text is persisted).
- Pane visibility caused by auto-fold (`*-auto` is never stored).
- Pin/always-on-top state.
- Store rating “shown” flag is in `rate-prompt`; other per-window UI state is not.
- Preview open state, palette open state, menu open state.
- Search queries (folder search and find bar are per-tab in memory only).

---

## 8. Localization: languages, strings and selection

### 8.1 The six languages

| Code | Endonym (Aa) | Dictionary | Help page | MSIX resources |
|---|---|---|---|---|
| `en` | English | `src/locales/en.ts` | `src/locales/help/en.html` | `src-tauri/msix/Strings/en-US/Resources.resw` |
| `it` | Italiano | `src/locales/it.ts` | `it.html` | `it-IT` |
| `de` | Deutsch | `src/locales/de.ts` | `de.html` | `de-DE` |
| `fr` | Français | `src/locales/fr.ts` | `fr.html` | `fr-FR` |
| `es` | Español | `src/locales/es.ts` | `es.html` | `es-ES` |
| `zh-CN` | 中文 | `src/locales/zh-CN.ts` | `zh-CN.html` | `zh-Hans-CN` |

All six dictionaries contain exactly **399 keys** (verified by counting the
English schema `src/locales/en.ts:4-426`). The English dictionary is the type
schema: a missing/extra key in any other locale is a TypeScript compile error
(`src/i18n.ts:1-18`).

### 8.2 Complete string-key inventory (399 keys)

Every key below is localized in all six dictionaries. Grouping is by key
prefix; count in parentheses. English values are in `src/locales/en.ts` at the
cited lines; the other dictionaries mirror the key set.

- `toolbar.*` (12): `toolbar.tree`, `.open`, `.edit`, `.read`, `.save`, `.share`,
  `.reading`, `.pin`, `.help`, `.toc`, `.windowTitle`, `.saveAs`. Values include
  “Toggle file browser (Ctrl+B)”,
  “Edit (Ctrl+E)”, “Back to reading (Ctrl+E)”, “Save (Ctrl+S)”, “Share…
  (Ctrl+Shift+E)”, “Reading settings”, “Keep on top of other windows”, “Help
  (F1)”, “Toggle outline (Ctrl+I)”, “{name} — lmdv”.
- `tab.*` (15): `tab.list`, `.newTab`, `.newTabHint`, `.close`, `.closeTab`,
  `.closeOthers`, `.closeRight`, `.reopen`, `.moveToWindow`, `.allTabs`,
  `.unsaved`, `.startPage`, `.minutes`, `.pasted`, `.saveAs`.
- `caption.*` (5): `caption.group`, `.minimize`, `.maximize`, `.restore`,
  `.close`.
- `pane.*` (11): `pane.files`, `.up`, `.searchPlaceholder`, `.clear`, `.outline`,
  `.filterPlaceholder`, `.resizeTree`, `.resizeToc`, `.resizeHint`,
  `.noMatches`, `.lineN`.
- `find.*` (4): `find.placeholder`, `.prev`, `.next`, `.close`.
- `notice.*` (1): `notice.dismiss`.
- `empty.*` (11): `empty.title`, `.lead`, `.open`, `.recent`, `.clear`, `.tip1`,
  `.tip2`, `.tip3`, `.hint`, `.paste`, `.pasteHint`.
- `drop.*` (1): `drop.card`.
- `rate.*` (6): `rate.title`, `.text`, `.now`, `.later`, `.never`, `.store`.
- `share.*` (12): `share.copy`, `.copyHint`, `.print`, `.printHint`, `.pdf`,
  `.pdfHint`, `.breakH1`, `.breakH1Hint`, `.html`, `.htmlHint`, `.docx`,
  `.docxHint`.
- `file.*` (8): `file.reveal`, `.copyPath`, `.copyLink`, `.copyLinkHint`,
  `.openNewWindow`, `.ctrlClick`, `.openNewTab`, `.shiftClick`.
- `ctx.*` (15): `ctx.copy`, `.copyPlain`, `.findInDoc`, `.findInFolder`,
  `.openLink`, `.openNewWindow`, `.openNewTab`, `.copyLinkAddress`, `.zoom`,
  `.copyCode`, `.copySectionLink`, `.editHere`, `.copyDoc`, `.back`, `.forward`.
- `key.*` (6): `key.ctrl`, `.shift`, `.space`, `.esc`, `.pgUp`, `.pgDn`.
- `rs.*` (25): smaller, larger, font, sans, serif, spacing, compact, normal,
  relaxed, width, narrow, wide, full, panels, small, large, reset, theme, light,
  sepia, dark, black, auto, language, langAuto.
- `keys.*` (50): title, hint, reading, app, editing, scroll, page, heading,
  topEnd, findMatch, find, textSize, open, edit, save, panels, searchFolder,
  share, copyRich, print, pin, bik, findReplace, moveLine, dupLine, slash,
  reload, newWindow, ctrlClickKeys, ctrlClick, backForward, closeWindow, tabs,
  newTab, closeTab, reopenTab, nextTab, tabN, moveTab, shiftClickKeys,
  shiftClick, contextMenu, deleteLine, comment, gotoLine, leaveEditor,
  dblclickKeys, dblclick, paste, palette.
- `help.*` (2): `help.close`, `.title`.
- `zoom.*` (34): open, dialog, viewport, out, in, fit, fitLabel, copy, copyPng,
  copyPngHint, copyPng4, copyPng4Hint, copySvg, copySvgHint, savePng, saveSvg,
  close, diagramOpen, imageOpen, image, diagram, table, width, widthLabel,
  actual, actualLabel, tableViewport, filter, filterHint, rowsOne, rows,
  rowsOf, noRows, sortHint.
- `msg.*` (66): `msg.editorLoadFailed`, `msg.imagePasteFailed`,
  `msg.imageDropFailed`, `msg.discardQ`, `msg.discardTitle`, `msg.draftRecovered`,
  `msg.restore`, `msg.diagramFailed`, `msg.previewError`, `msg.conflictQ`,
  `msg.conflictTitle`, `msg.overwrite`, `msg.keepTheirs`, `msg.saveFailed`,
  `msg.taskChecked`, `msg.taskUnchecked`, `msg.undo`, `msg.undoFailed`,
  `msg.taskUpdateFailed`, `msg.reloadFailed`, `msg.copyFailed`, `msg.pdfSaved`,
  `msg.showInFolder`, `msg.pdfFailed`, `msg.usePrintDialog`, `msg.exportFailed`,
  `msg.revealFailed`, `msg.notFound`, `msg.fileChanged`, `msg.reload`,
  `msg.minRead`, `msg.words`, `msg.langReloadQ`, `msg.windowFailed`,
  `msg.notMarkdown`, `msg.openFile`, `msg.renamedTo`, `msg.goneFromDisk`,
  `msg.saveAgain`, `msg.discard`, `msg.cancel`, `msg.closeTabQ`,
  `msg.closeWindowQ`, `msg.closeWindowOneQ`, `msg.save`, `msg.saveAll`,
  `msg.dontSave`, `msg.tearOffFailed`, `msg.tabRefusedSameFile`, `msg.saved`,
  `msg.pasteEmpty`, `msg.pasteImage`, `msg.pasteFailed`, `msg.pasteBusy`,
  `msg.pastedOpened`, `msg.pastedLink`, `msg.otherComputer`,
  `msg.imageNeedsFileTitle`, `msg.imageNeedsFileQ`, `msg.saveAsOpenDirty`,
  `msg.pastedRecovered`, `msg.pastedRecoveredN`,
  `msg.closeWindowPastedOneQ`, `msg.closeWindowPastedQ`,
  `msg.closeWindowMixedQ`, `msg.unsavedTextTitle`.
- `filter.*` (6): markdown, html, word, pdf, png, svg.
- `render.*` (5): copy, copyTable, frontMatter, toc, invalidMermaid.
- `fb.*` (8): label, textStyle, text, insert, insertHint, tools, toolsHint,
  slashHint.
- `ins.*` (8): table, diagram, math, codeblock, note, tip, warning, divider.
- `tool.*` (3): find, spell, comment.
- `cmd.*` (30): bold, italic, strike, code, link, h1, h2, h3, paragraph, bullet,
  ordered, task, quote, table, diagram, math, codeblock, note, tip, warning,
  divider, find, moveLineUp, moveLineDown, copyLineDown, deleteLine, spell,
  comment, keys, kwReading.
- `cm.*` (19): CodeMirror UI strings: find, replace, next, previous, all,
  matchCase, regexp, byWord, replaceOne, replaceAll, close, currentMatch,
  onLine, replacedMatches, replacedOnLine, gotoLine, go, completions,
  selectionDeleted.
- `kw.*` (14): extra localized `/`-menu keywords: h1, h2, bullet, ordered, task,
  quote, table, diagram, math, codeblock, note, tip, warning, divider.
- `paste.*` (1): `paste.untitled`.
- `pal.*` (21): title, placeholder, results, none, help, foot, countOne, count,
  gCommands, gHeadings, gTabs, gFiles, gSearch, findInDoc, searchFolder,
  nextTab, prevTab, theme, resetSize, editor, current.

**Total = 399.** The six help pages are additional localized artifacts, not
counted in the dictionary (each is a self-contained HTML fragment with the same
section structure, e.g. `src/locales/help/en.html:1-383`).

### 8.3 How language is chosen, stored and overridden

1. **Saved override** (`localStorage.lang`) wins, except `"auto"`/missing.
2. **Auto:** `window.__APP_LANGS__`, injected before any page script from
   Windows’ preferred-languages list in order; any `zh*` → `zh-CN`; `en*` →
   `en`; `it/de/fr/es` primary → that; first match wins; otherwise `en`
   (`src/i18n.ts:37-52`, `src-tauri/src/lib.rs:264-325`).
3. **Browser fallback:** `navigator.languages` / `navigator.language` when the
   injected list is absent (dev harness) (`src/i18n.ts:37-41`).
4. **Override UI:** Aa → Language; labels endonyms; `auto` removes the key;
   choice triggers `location.reload()`; tabs are carried through
   `sessionStorage` (`src/main.ts:4457-4478`).
5. **Native mirror:** `remember_language` writes `ui-language`; next start passes
   `--lang` to WebView2; English maps to `en-US`; this affects native menus,
   spelling and print preview, not the page (`src-tauri/src/commands.rs:970-979`,
   `src-tauri/src/lib.rs:474-487`).
6. **Per-window behavior:** a language change reloads only the window that
   changed it; other windows keep their loaded language until reloaded (they
   share `lang` but each module resolves at load).
7. **Document content is never translated.** Only UI, help, menus, notices,
   dialogs and exports’ `lang` attribute.

---

## 9. Fixtures

The fixture names used in the tables map to ten families. All are plain
Markdown unless noted; paths are relative to the fixture root.

### 9.1 F-CORE (reading, headings, outline, section line)

`core.md` — a 1,500-word document with:
- exactly one H1 as title;
- H2/H3/H4 headings, one skipped level (H2 → H4), one heading containing
  punctuation (`## 1. Introduction`, `## Q&A`, `## What's new?`,
  `## Attività`), one heading named `Toolbar`, `Title`, `Links`, `Images`;
- a `[TOC]` paragraph after the first section;
- a 12-heading section for the outline filter threshold;
- a short trailing section that only activates at max scroll;
- long paragraphs and a short document variant `short.md`;
- `long.md` with 5,000 words for progress/reading time.
Acceptance targets: R-03/04/08/09/12-15, O-01–O-11, M-03–M-05, M-20.

### 9.2 F-MARKDOWN (rendering conformance)

One file per concern:
- `math.md` — copy of `samples/commonmark-math.md`; every case must match its
  comments, including prices, unclosed `$$`, inline display, backtick math,
  fenced math with title, list/quote blocks, blank-line cancellation.
- `html.md` — `<br>`, `<kbd>`, `<sup>`, `<details>` with table inside,
  `<picture>` light/dark, `srcset`, `<video>/<audio>/<track>`, `<style>`,
  `<script>`, `<title>`, `<iframe>`, `<name>`, `<name>` in backticks;
- `tasks.md` — checked/unchecked, nested list, long item, item with inline
  math, plain item in same list;
- `front.md` — flat YAML, arrays, quoted values, comments, nested map (fallback),
  unclosed front matter;
- `alerts.md` — all five callouts;
- `footnotes.md` — named and numeric, multiple references;
- `code.md` — known/unknown/empty languages, long lines, no trailing newline;
- `mermaid.md` — valid flowchart, invalid diagram, diagram with `asset.localhost`
  label;
- `links.md` — `other`, `other.md`, `folder/`, `folder/README`, `#anchor`,
  `other.md#anchor`, image link, PDF link, `.sh` link, broken link, `mailto:`,
  `tel:`, `www.`, `README.md` in prose;
- `security.md` — `//server/share/a.png`, `\\server\share\a.png`,
  `asset://localhost/...`, `<style>@import ...</style>`, popover/command attrs.
Acceptance: M-01–M-34, L-01–L-15.

### 9.3 F-TABLE

`table.md`:
- 3-column prose table;
- 12-row numeric table with Italian and English amounts, percentages, dates,
  empty cells, mixed text;
- very wide table (>2,000 px min-content);
- table inside blockquote/list/callout;
- nested table;
- table with `<colgroup>`;
- table with a header link and a header without one;
- table with a task box in a cell;
- table wider than A4 for print.
Acceptance: M-06, V-11–V-18, X-12, F-05.

### 9.4 F-EDIT (editing, save, drafts, EOL)

- `edit.md` with one of each construct and a known word repeated twice in one
  paragraph;
- `crlf.md` saved with CRLF, `bom.md` with UTF-8 BOM;
- `draft-target.md` for crash simulation;
- `image-target.md` for paste/drop images;
- `conflict.md` for external modification;
- `long-edit.md` for scroll sync and preview follow;
- `broken-render.md` with invalid Mermaid/KaTeX to test last-good preview.
Acceptance: E-01–E-40, M-34, X-02.

### 9.5 F-TABS (tabs, windows, drag)

- Six files `t1.md` … `t6.md`, two of them dirty-able;
- a pasted-text session with `# Budget` and no heading;
- two windows with overlapping `t3.md`;
- a 15-tab workspace for overflow/list/reading-time suppression;
- a 480 px window for narrow behavior.
Acceptance: T-01–T-35, W-01–W-10, D-04–D-08.

### 9.6 F-TREE (file browser and search)

A tree with:
```
root/
  a.md
  B.MD
  .hidden/x.md
  node_modules/pkg/readme.md
  target/x.md
  dist/x.md
  one/two/three/four/five.md
  2001 files (cap test)
  empty-folder/
  docs/README.md
  docs/index.md
  docs/guide.md
```
Search terms: “PIM”, “akeneo”, “Line”, a term only in a filename, a term in a
file outside depth 4.
Acceptance: F-01–F-22.

### 9.7 F-SYSTEM (CLI, reload, conflict, security, i18n, prefs, a11y)

- `cli.md` with 100 numbered lines;
- a file deleted/renamed while open;
- a file touched without text change;
- a network path fixture;
- six-language UI screenshots;
- every reading preference combination;
- an axe/Orca transcript fixture.
Acceptance: S-01–S-27, R-20–R-24, A-01–A-15, I-01–I-11, P-01–P-20.

### 9.8 F-EXPORT

`export.md` with headings, code, table, callout, task list, footnote, math,
Mermaid, local image, relative links, `[TOC]`, front matter, a 2-page table.
Acceptance: X-01–X-16.

### 9.9 F-PALETTE

A document with headings named like commands (“Print”, “Save”, “Theme”), six
open tabs, 30 folder files, 12 recents, commands with/without shortcuts, editor
commands while editing.
Acceptance: C-01–C-13, A-08.

### 9.10 F-ERRORS / F-BROKEN

- missing file launch;
- unreadable file;
- external change while dirty;
- save conflict;
- preview render throw;
- invalid Mermaid;
- clipboard busy/image-only/empty;
- language reload with dirty tabs;
- deleted file then Save again.
Acceptance: all error rows in §3.2, R-24, T-24, E-05.

### 9.11 Fixture-name aliases

The row-level names map onto the ten families as follows (a name may be used by
more than one family where the scenario overlaps):

| Family | Also named in rows as |
|---|---|
| F-CORE | F-BASIC, F-LONG, F-HEADINGS, F-FIND, F-SECTION, F-TOC, F-LINE |
| F-MARKDOWN | F-MATH, F-CODE, F-MERMAID, F-ALERTS, F-HTML, F-FOOTNOTE, F-FRONT, F-TASK, F-IMAGE, F-LINKS, F-MEDIA, F-SECURITY |
| F-TABLE | F-PREVIEW |
| F-EDIT | F-EOL, F-DRAFT, F-CONFLICT, F-PASTE, F-SELECT |
| F-TABS | F-WINDOW, F-DRAG, F-PASTE |
| F-TREE | F-SEARCH, F-PANES, F-NARROW, F-NESTED |
| F-SYSTEM | F-RELOAD, F-I18N, F-PREFS, F-A11Y, F-PALETTE, F-LINE |
| F-EXPORT | F-SELECT, F-PREVIEW |
| F-PALETTE | F-PALETTE (standalone) |
| F-ERRORS | F-BROKEN, F-CONFLICT, F-RELOAD |

---

## 10. Reference test harness and hooks

The reference source tree ships an end-to-end browser harness at `dev/harness.html`
(`README.md:375-378`). It loads the real `index.html` + `src/main.ts` with a
mocked Tauri backend and files in memory.

- Query options: `?doc=<url>` first document, `?doc=none` start on home,
  `?line=N` first launch line, `?lang=it` force UI language, `?store=1` Store
  build, `?root=Name` nested folder, `?label=doc-1` multi-window identity
  (`dev/harness.html:12-32,72-92`).
- `window.__harness` controls disk (`files`, `modified`, `conflict`), dialogs
  (`answers`, `dialogs`), windows (`win`, `boxes`, `cursor`), events
  (`emit`, `emitTo`, `drop`, `requestClose`), clipboard (`clipboard`,
  `clipboardError`), async calls (`hold`/`release`), and the real app modules
  (`modules`, `editor`) (`dev/harness.html:132-298`).
- `window.__app` test hooks are installed in release builds too:
  `tabs()`, `activate(i)`, `close(i)`, `newTab()`, `openInTab(path, opts)`,
  `move(from,to)`, `transfer(i)`, `tearOff(i)` (`src/main.ts:2961-3006`,
  `src/tabs/contract.ts:137-149`).
- Useful harness patterns:
  - `__harness.load(text)` simulates an external write + activate;
  - `__harness.fileChanged(kind, path, to)` simulates watcher events;
  - `__harness.answer("save"|"dontsave"|"cancel"|label)` answers dialogs;
  - `__harness.paste(text,{image:true})` sends a paste event;
  - `__harness.drop([...],x,y)` simulates file drop at page coordinates.
- The harness overrides `requestAnimationFrame` with a 50 ms timeout fallback so
  scroll sync works in hidden automation tabs (`dev/harness.html:44-70`).

**Suggested lmdv golden scenes** (not implementation, just acceptance targets):
light/sepia/dark/black at 1280×900 and 480×800; home with 12 recents; 15 tabs
with overflow; edit split with format bar and find panel; wide table; table
preview filtered/sorted; preview at Fit/Width/200%; help/keys; six languages;
pasted tab; close prompt mixed; error card; print/PDF first page.

