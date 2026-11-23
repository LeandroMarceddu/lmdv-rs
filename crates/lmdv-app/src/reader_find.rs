//! The reading find bar and its paint-only highlights (R-10, R-16–R-19).
//!
//! Matching runs over the model rather than the drawn tree: every drawn inline
//! text ([`lmdv_md::Inline::text`]) is searched in reading order and each hit is
//! mapped back through [`lmdv_md::SourceMap`] to the document source, so `**`
//! markers, link destinations and code fences never match while code spans do.
//! Code-block bodies are searched through the same line map the renderer uses
//! for their highlights. Nothing here uses a regex, whole-word matching or
//! accent folding (R-18).
//!
//! The state is per tab ([`ReaderFind`] lives in [`crate::window::TabView`]), so
//! a query, its hits and the current index survive tab switches (T-26). Entering
//! edit mode closes it (E-01); the editor keeps its own find bar (E-20/E-21).
//!
//! Every match is a [`HighlightKind::Find`] paint at its own index, the current
//! match is also a [`HighlightKind::Current`] one right after them, and the
//! `--line` flash is a [`HighlightKind::Line`] entry last. The renderer paints
//! the strongest kind once per segment, so the current hit's painted key is the
//! appended `Current` entry's index (`matches.len() + current`); the reveal uses
//! that with `Tx::scroll_into_view` instead of a line-height estimate
//! (OI-081/OI-084).

use std::cell::Cell;
use std::ops::Range;

use lmdv_core::TabId;
use lmdv_md::{Block, Inline, MdDoc};
use lmdv_render::{Highlight, HighlightKind, highlight_key};
use rune::{Key, KeyEvent, TextEdit, Tx};

use crate::window::{Mode, Tab, WindowState};

/// The viewer find query field's key.
pub const READER_FIND_QUERY_KEY: &str = "reader-find-query";
/// The prefill's character cap (R-17).
pub const READER_FIND_PREFILL_MAX: usize = 100;
/// The top band a revealed match should be clear of, in points (R-19).
///
/// The original computes `max(60, find-bar bottom + 8)`: the floating bar sits
/// 10 points down and is about 38 points tall, so the band is 60. When the
/// section line shows, the bar steps below it and the band grows by the line's
/// height in [`WindowState::find_top_band`]. The notice bar takes flow space
/// below the article here, so it can never cover a match.
pub const READER_FIND_BAND: f32 = 60.0;
/// The distance from the viewport's bottom edge that also centres a match
/// (R-19's fixed 60 points).
pub const READER_FIND_BOTTOM_BAND: f32 = 60.0;
/// How long a reveal waits for the frame after it to measure the match's block.
pub const FIND_PROBE_SECONDS: f64 = 0.5;

/// Where the current match's top-level block sits, relative to the article
/// viewport's top, from the last frame's layout.
///
/// Rune keeps a text highlight's own boxes private (there is no `Cx` query for
/// them; they live in the runtime's nodes), so the app measures the block that
/// owns the hit instead. A block fully inside both bands guarantees the hit is
/// too; a block that reaches into a band may put the hit there.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FindBand {
    /// The block's top below the viewport's top (negative when scrolled past).
    pub top: f32,
    /// The block's bottom likewise.
    pub bottom: f32,
}

/// How a find step reveals its match (R-19).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindReveal {
    /// Reveal minimally: the match is already clear of both bands, so only a
    /// horizontally scrolled table/code block moves.
    IntoView,
    /// Centre the match: it is near the top band or the bottom edge.
    Center,
}

/// R-19's band decision for one measured block.
///
/// `viewport` is the article viewport's height. Without a measurement (the
/// first frame of a document, or a block the renderer has not mounted) the
/// minimal reveal is the safe choice; the caller then probes once for the
/// measured band.
pub fn find_reveal(band: Option<FindBand>, viewport: f32, top_band: f32, bottom_band: f32) -> FindReveal {
    if viewport <= 0.0 {
        return FindReveal::IntoView;
    }
    let Some(band) = band else { return FindReveal::IntoView };
    if !band.top.is_finite() || !band.bottom.is_finite() {
        return FindReveal::IntoView;
    }
    if band.top >= top_band && band.bottom <= viewport - bottom_band {
        FindReveal::IntoView
    } else {
        FindReveal::Center
    }
}

/// A reveal whose post-frame band still has to be checked once.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FindProbe {
    /// The highlight key the reveal asked for.
    pub(crate) key: String,
    /// Frames to let the page and its measurement catch up with the reveal.
    pub(crate) wait: u8,
    /// Clock time after which the probe is dropped unanswered.
    pub(crate) deadline: f64,
}

/// Per-tab reading find state: the bar, its query, the source ranges of the
/// hits in drawn order and the current hit.
#[derive(Clone, Debug, Default)]
pub struct ReaderFind {
    /// The floating bar is showing.
    pub open: bool,
    /// The input's text, as the text field binds it.
    pub query: TextEdit,
    /// Source byte ranges of the hits, in document (drawn) order.
    pub matches: Vec<Range<usize>>,
    /// Index into [`ReaderFind::matches`]; 0 when there are none.
    pub current: usize,
    /// The reader revision the ranges were computed from, so a reparse or
    /// reload unexpectedly landing while the bar is open recomputes them.
    pub(crate) revision: Option<u64>,
    /// The current hit's owning block, as the last frame measured it (R-19).
    pub(crate) band: Cell<Option<FindBand>>,
    /// A reveal waiting for the next frame's measurement (R-19).
    pub(crate) probe: Option<FindProbe>,
}

impl ReaderFind {
    /// Closes the bar and forgets its hits; the query stays for the next open.
    pub fn close(&mut self) {
        self.open = false;
        self.matches.clear();
        self.current = 0;
        self.revision = None;
        self.band.set(None);
        self.probe = None;
    }

    /// `current/total`, or `0/0` with no hits (R-18).
    pub fn counter(&self) -> String {
        if self.matches.is_empty() { "0/0".to_owned() } else { format!("{}/{}", self.current + 1, self.matches.len()) }
    }

    /// The current hit's source range.
    pub fn current_match(&self) -> Option<&Range<usize>> {
        self.matches.get(self.current)
    }

    /// Steps `delta` hits, wrapping at either end. False with no hits.
    pub fn step(&mut self, delta: isize) -> bool {
        let total = self.matches.len();
        if total == 0 {
            return false;
        }
        let current = self.current.min(total - 1) as isize;
        self.current = (current + delta).rem_euclid(total as isize) as usize;
        true
    }

    /// Recomputes the hits for the input's current text, selecting the first.
    fn refresh(&mut self, doc: &MdDoc) {
        self.matches = find_in_document(doc, &self.query.text);
        self.current = 0;
    }

    /// Recomputes the hits but keeps the current index where it can, for a
    /// reparse that landed while the bar was open.
    fn refresh_keep_current(&mut self, doc: &MdDoc) {
        self.matches = find_in_document(doc, &self.query.text);
        if self.current >= self.matches.len() {
            self.current = 0;
        }
    }

    /// Sets the query and recomputes the hits from the first one.
    pub fn set_query(&mut self, doc: &MdDoc, query: &str) {
        if self.query.text != query {
            self.query.replace_text(query.to_owned());
        }
        self.refresh(doc);
    }
}

impl WindowState {
    /// The `on_edit` accessor for the reading find query.
    pub(crate) fn active_reader_find_edit(&mut self) -> &mut TextEdit {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            return &mut tab.view.reader_find.query;
        }
        &mut self.editor.scratch
    }

    /// `Ctrl+F` in the reading view: opens the bar, prefills from the article
    /// selection, selects the input and reveals the first hit (R-16/R-17).
    ///
    /// The selection is used exactly as selected (one line, trimmed, no
    /// newlines); a non-collapsed selection that trims to nothing prefills an
    /// empty query instead of leaving the previous query's hits in place.
    pub(crate) fn open_reader_find(&mut self, tx: &mut Tx) {
        let Some(tab) = self.active_tab() else { return };
        if tab.view.mode != Mode::Viewer {
            return;
        }
        let Some(model) = tab.view.reader.clone() else { return };
        let prefill = tx
            .text_selection()
            .filter(|selection| selection.texts().iter().any(|text| selection_in_article(text.path())))
            .map(|selection| reader_find_prefill(selection.text()));
        let Some(tab) = self.active_tab_mut() else { return };
        let find = &mut tab.view.reader_find;
        find.open = true;
        match prefill {
            Some(query) => find.set_query(&model, &query),
            None => find.refresh(&model),
        }
        find.revision = Some(model.revision);
        let length = find.query.text.len();
        find.query.set_selection(0, length);
        tx.focus(READER_FIND_QUERY_KEY, false);
        self.reveal_reader_current(tx);
    }

    /// Shows the bar with `query`, for a headless scene or the folder search
    /// that hands its query over. No focus request.
    pub(crate) fn show_reader_find(&mut self, query: &str) {
        let Some(index) = self.active_tab().filter(|tab| tab.view.mode == Mode::Viewer).map(|_| self.active) else {
            return;
        };
        self.fill_reader_find(index, query);
    }

    /// Fills `tab`'s reading find bar with `query` (F-16): a folder-search
    /// result carries its query into the document it opened.
    ///
    /// The bar opens and its hits are computed against whatever model the tab
    /// has; a load still in flight fills them when its parse lands. Focus and
    /// the reveal happen only when that tab is the one in front, so a Ctrl+click
    /// background tab never takes the keyboard.
    pub(crate) fn prefill_reader_find(&mut self, tab: TabId, query: &str, focus: bool, tx: &mut Tx) -> bool {
        if query.is_empty() {
            return false;
        }
        let Some(index) = self.tabs.iter().position(|found| found.id == tab) else { return false };
        if self.tabs[index].is_start() || self.tabs[index].view.mode != Mode::Viewer {
            return false;
        }
        self.fill_reader_find(index, query);
        if self.active != index {
            return true;
        }
        self.reveal_reader_current_at(index, tx);
        if focus {
            let length = self.tabs[index].view.reader_find.query.text.len();
            self.tabs[index].view.reader_find.query.set_selection(0, length);
            tx.focus(READER_FIND_QUERY_KEY, false);
        }
        true
    }

    /// Fills the bar at `index` and recomputes its hits when the model is there.
    fn fill_reader_find(&mut self, index: usize, query: &str) {
        let Some(tab) = self.tabs.get_mut(index) else { return };
        tab.view.reader_find.open = true;
        if tab.view.reader_find.query.text != query {
            tab.view.reader_find.query.replace_text(query);
        }
        match tab.view.reader.clone() {
            Some(model) => {
                tab.view.reader_find.set_query(&model, query);
                tab.view.reader_find.revision = Some(model.revision);
            }
            None => {
                tab.view.reader_find.matches.clear();
                tab.view.reader_find.current = 0;
                tab.view.reader_find.revision = None;
            }
        }
    }

    /// Closes the bar, drops its hits and returns the keyboard to the article.
    pub(crate) fn close_reader_find(&mut self, tx: &mut Tx) {
        if let Some(tab) = self.active_tab_mut() {
            tab.view.reader_find.close();
        }
        tx.blur();
    }

    /// The query field changed: recompute and reveal the first hit.
    pub(crate) fn reader_find_changed(&mut self, tx: &mut Tx) {
        let Some(model) = self.active_reader() else { return };
        let Some(tab) = self.active_tab_mut() else { return };
        let find = &mut tab.view.reader_find;
        find.refresh(&model);
        find.revision = Some(model.revision);
        self.reveal_reader_current(tx);
    }

    /// `Enter` in the query field, the bar's next button and `n`.
    pub(crate) fn reader_find_next(&mut self, tx: &mut Tx) -> bool {
        let Some(tab) = self.active_tab_mut() else { return false };
        if !tab.view.reader_find.open || !tab.view.reader_find.step(1) {
            return false;
        }
        self.reveal_reader_current(tx);
        true
    }

    /// `Shift+Enter` in the query field, the bar's previous button and `N`.
    pub(crate) fn reader_find_prev(&mut self, tx: &mut Tx) -> bool {
        let Some(tab) = self.active_tab_mut() else { return false };
        if !tab.view.reader_find.open || !tab.view.reader_find.step(-1) {
            return false;
        }
        self.reveal_reader_current(tx);
        true
    }

    /// The query field's keys before the text field: Esc closes, Enter and
    /// Shift+Enter step, F3/Shift+F3 step and `Ctrl+F` reselects the query
    /// (R-16/R-18).
    pub(crate) fn reader_find_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        match event.key {
            Key::Escape => {
                self.close_reader_find(tx);
                return true;
            }
            Key::Enter | Key::Function(3) => {
                if event.mods.shift {
                    self.reader_find_prev(tx);
                } else {
                    self.reader_find_next(tx);
                }
                return true;
            }
            _ => {}
        }
        if event.is_command('f') && !event.mods.alt && !event.mods.shift {
            self.open_reader_find(tx);
            return true;
        }
        false
    }

    /// Reading-view keys that belong to the find bar even when the input does
    /// not hold the focus: Esc closes it and `n`/`N` step it (R-10). Nothing
    /// happens without an open bar.
    pub(crate) fn viewer_context_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if self.active_tab().is_none_or(|tab| tab.view.mode != Mode::Viewer) {
            return false;
        }
        if !self.active_tab().is_some_and(|tab| tab.view.reader_find.open) {
            return false;
        }
        if event.key == Key::Escape {
            self.close_reader_find(tx);
            return true;
        }
        if self.prompt().is_some() || self.help_view().is_some() || self.share.menu_open {
            return false;
        }
        match bare_n(event) {
            Some(true) => {
                self.reader_find_next(tx);
                true
            }
            Some(false) => {
                self.reader_find_prev(tx);
                true
            }
            None => false,
        }
    }

    /// The paint-only highlights for one tab's article, in the order the
    /// renderer keys them: every find hit as [`HighlightKind::Find`], the
    /// current hit as [`HighlightKind::Current`], the `--line` flash
    /// (OI-043/OI-084) and last the heading a jump landed on
    /// ([`HighlightKind::Landing`], O-10/A-15).
    ///
    /// The find hits come first so [`highlight_key(i)`] names hit `i` exactly;
    /// every later entry can never shift a find key.
    pub(crate) fn reader_highlights(&self, tab: &Tab) -> Vec<Highlight> {
        let mut out = Vec::new();
        if tab.view.mode == Mode::Viewer && tab.view.reader_find.open {
            let find = &tab.view.reader_find;
            for range in &find.matches {
                out.push(Highlight::new(range.clone(), HighlightKind::Find));
            }
            if let Some(range) = find.current_match() {
                out.push(Highlight::new(range.clone(), HighlightKind::Current));
            }
        }
        if let Some(flash) = &self.reading.line_flash
            && tab.document_id() == Some(flash.doc)
            && !flash.range.is_empty()
        {
            out.push(Highlight::new(flash.range.clone(), HighlightKind::Line));
        }
        // O-10/A-15: the heading a jump landed on, a static 1.2 s accent band.
        // Its entry comes last, after every find key, so it can never shift one.
        if let Some(flash) = &self.reading.landing_flash
            && tab.view.mode == Mode::Viewer
            && tab.document_id() == Some(flash.doc)
            && !flash.range.is_empty()
        {
            out.push(Highlight::new(flash.range.clone(), HighlightKind::Landing));
        }
        out
    }

    /// The `hl-{index}` key of the current hit, for `Tx::scroll_into_view`.
    ///
    /// [`Self::reader_highlights`] lists every hit as `Find` first and then the
    /// current hit again as `Current`. The renderer's overlap pass keeps one
    /// paint per drawn segment and the stronger `Current` wins, so the painted
    /// key of the current hit is the appended entry's index
    /// (`matches.len() + current`), not the `Find` entry it covers.
    pub(crate) fn reader_find_scroll_key(&self, tab: &Tab) -> Option<String> {
        if tab.view.mode != Mode::Viewer || !tab.view.reader_find.open {
            return None;
        }
        let find = &tab.view.reader_find;
        find.current_match()?;
        Some(highlight_key(find.matches.len() + find.current))
    }

    /// Recomputes an open tab's hits when its reader revision changes under it
    /// (a reload, a task tick, an edit in another window), and answers a reveal
    /// probe once the page has measured it. True on a change.
    pub(crate) fn poll_reader_find(&mut self, now: f64, tx: &mut Tx) -> bool {
        let mut changed = false;
        let Some(tab) = self.active_tab() else { return false };
        if tab.view.mode != Mode::Viewer || !tab.view.reader_find.open {
            return false;
        }
        let Some(model) = tab.view.reader.clone() else { return false };
        if tab.view.reader_find.revision != Some(model.revision) {
            let Some(tab) = self.active_tab_mut() else { return false };
            tab.view.reader_find.refresh_keep_current(&model);
            tab.view.reader_find.revision = Some(model.revision);
            changed = true;
        }
        // R-19: the first reveal could not measure its hit, so the frame after
        // it decides whether the hit still sits in a band and must be centred.
        let Some(probe) = self.active_tab().and_then(|tab| tab.view.reader_find.probe.clone()) else {
            return changed;
        };
        if now >= probe.deadline {
            if let Some(tab) = self.active_tab_mut() {
                tab.view.reader_find.probe = None;
            }
            return changed;
        }
        if probe.wait > 0 {
            if let Some(tab) = self.active_tab_mut()
                && let Some(probe) = tab.view.reader_find.probe.as_mut()
            {
                probe.wait -= 1;
            }
            // One more frame lets the view measure the page the reveal made.
            tx.wake_at(now + 0.05);
            return true;
        }
        let band = self.active_tab().and_then(|tab| tab.view.reader_find.band.get());
        let viewport = self.reading.scroll.get().viewport;
        let top = self.find_top_band();
        if find_reveal(band, viewport, top, READER_FIND_BOTTOM_BAND) == FindReveal::Center {
            tx.scroll_center(probe.key);
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.view.reader_find.probe = None;
        }
        changed
    }

    /// The top band a revealed find hit must clear (R-19).
    ///
    /// [`READER_FIND_BAND`] is the floating bar's 60 points; the bar steps below
    /// the section line while it shows, so the line's height joins the band.
    /// A notice takes flow space below the article and never covers it.
    pub(crate) fn find_top_band(&self) -> f32 {
        READER_FIND_BAND + if self.section_line.visible() { crate::section::SECTION_LINE_HEIGHT } else { 0.0 }
    }

    /// Issues the reveal for the current hit (R-19).
    ///
    /// The hit's block, measured by the last frame, decides between a minimal
    /// `scroll_into_view` (both bands clear: only a horizontally scrolled table
    /// or code block moves) and `scroll_center` (near the top band or the
    /// bottom edge: centre it). A document whose first frame has not measured
    /// the block takes the minimal reveal and arms one probe, answered by
    /// [`Self::poll_reader_find`] after the next frame.
    fn reveal_reader_current(&mut self, tx: &mut Tx) {
        self.reveal_reader_current_at(self.active, tx);
    }

    fn reveal_reader_current_at(&mut self, index: usize, tx: &mut Tx) {
        let Some(tab) = self.tabs.get(index) else { return };
        let Some(key) = self.reader_find_scroll_key(tab) else { return };
        let band = tab.view.reader_find.band.get();
        let viewport = self.reading.scroll.get().viewport;
        let top = self.find_top_band();
        let reveal = find_reveal(band, viewport, top, READER_FIND_BOTTOM_BAND);
        match reveal {
            FindReveal::Center => tx.scroll_center(key),
            FindReveal::IntoView => {
                tx.scroll_into_view(key.clone());
                // Measure once after the reveal: a band taken before a query
                // change belongs to another hit, and a first frame has none.
                let armed = tx.clock();
                if let Some(tab) = self.tabs.get_mut(index) {
                    tab.view.reader_find.probe = Some(FindProbe { key, wait: 1, deadline: armed + FIND_PROBE_SECONDS });
                }
                tx.wake_at(armed + 0.05);
            }
        }
    }
}

/// Publishes the current hit's owning block for R-19's scroll band.
///
/// Runs in the reader view each frame while the bar is open. Rune keeps a text
/// highlight's own boxes private, so the block's rect stands in: it is the
/// smallest piece of geometry the app can measure for a match. The rect is
/// relative to the article viewport's top, as the heading measures are.
pub(crate) fn publish_band(cx: &rune::Cx<WindowState>, model: &MdDoc, tab: &Tab) {
    let find = &tab.view.reader_find;
    let band = find.open.then(|| find.current_match()).flatten().and_then(|range| {
        let index = model.block_index_at(range.start)?;
        let key = model.block_key(index)?;
        let rect = cx.window_layout(key)?;
        let viewport = cx.window_layout(crate::reader::ARTICLE_SCROLL_KEY)?;
        Some(FindBand { top: rect[1] - viewport[1], bottom: rect[1] + rect[3] - viewport[1] })
    });
    find.band.set(band);
}

/// Refreshes a tab's open find bar after a parse landed (`window::drive_parse`).
pub(crate) fn refresh_tab(tab: &mut Tab) {
    if !tab.view.reader_find.open {
        return;
    }
    let Some(model) = tab.view.reader.clone() else { return };
    tab.view.reader_find.refresh_keep_current(&model);
    tab.view.reader_find.revision = Some(model.revision);
}

/// Whether a selection text's path is inside the reader's article (R-17).
///
/// Rune reports a selection inside a text field as a text selection too; the
/// prefill comes from the article only, so a selection in the find bar, the
/// outline filter or a browser field is ignored. The article viewport's key is
/// in every path below it.
pub(crate) fn selection_in_article(path: &str) -> bool {
    path.split('/').any(|segment| segment == crate::reader::ARTICLE_SCROLL_KEY)
}

/// The selection as the reading find prefill (R-17).
///
/// The selection is used exactly as selected: no word expansion, no case
/// folding, no whitespace rewriting inside the line. It is trimmed at both
/// ends, reduced to its first line — a selection across several drawn texts
/// has no newlines left — and capped at [`READER_FIND_PREFILL_MAX`] characters.
/// A selection that trims to nothing yields an empty query.
pub fn reader_find_prefill(text: &str) -> String {
    text.trim().lines().next().unwrap_or_default().trim().chars().take(READER_FIND_PREFILL_MAX).collect()
}

/// Every case-insensitive occurrence of `query` in the document's drawn text,
/// as source byte ranges in drawing order (R-18).
///
/// The query is lowercased once; inline text is searched through its source
/// map so Markdown markers never match. Code spans are ordinary inline text;
/// code-block bodies use the renderer's verbatim line map. Generated or
/// unmapped surfaces (front-matter values, the TOC labels, formula TeX and
/// Mermaid source) are not searched (OI-127).
pub fn find_in_document(doc: &MdDoc, query: &str) -> Vec<Range<usize>> {
    let needle = query.to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for block in &doc.blocks {
        collect_block(doc, block, &needle, &mut out);
    }
    out
}

fn collect_block(doc: &MdDoc, block: &Block, needle: &str, out: &mut Vec<Range<usize>>) {
    match block {
        Block::Para(inline) | Block::Heading { inline, .. } => collect_inline(inline, needle, out),
        Block::Code { text, source, .. } => {
            for (drawn, from) in code_segments(doc.source.as_ref(), text, source) {
                for hit in match_ranges(&text[drawn], needle) {
                    out.push(from.start + hit.start..from.start + hit.end);
                }
            }
        }
        Block::List { items, .. } => {
            for item in items {
                for block in item {
                    collect_block(doc, block, needle, out);
                }
            }
        }
        Block::Quote { blocks, .. } => {
            for block in blocks {
                collect_block(doc, block, needle, out);
            }
        }
        Block::Table(table) => {
            for cell in &table.head {
                collect_inline(cell, needle, out);
            }
            for row in &table.rows {
                for cell in row {
                    collect_inline(cell, needle, out);
                }
            }
        }
        Block::FootnoteDefs => {
            for footnote in &doc.footnotes {
                for block in &footnote.blocks {
                    collect_block(doc, block, needle, out);
                }
            }
        }
        Block::Rule
        | Block::Mermaid { .. }
        | Block::MathBlock { .. }
        | Block::HtmlBlock(_)
        | Block::Image(_)
        | Block::Toc => {}
    }
}

fn collect_inline(inline: &Inline, needle: &str, out: &mut Vec<Range<usize>>) {
    for hit in match_ranges(&inline.text, needle) {
        if let Some(source) = inline.source_map.source_range(hit)
            && !source.is_empty()
        {
            out.push(source);
        }
    }
}

/// The source and drawn byte ranges of each line of a code block's text.
///
/// Mirrors the renderer's `code_segments` (`lmdv-render`): the drawn text is
/// the block body after the opening fence (or with indentation dropped for an
/// indented block), found line by line inside the block's own source span.
fn code_segments(source: &str, code: &str, span: &Range<usize>) -> Vec<(Range<usize>, Range<usize>)> {
    let Some(block) = source.get(span.clone()) else { return Vec::new() };
    let indent = block.len() - block.trim_start_matches(' ').len();
    let fenced = indent <= 3 && {
        let rest = &block[indent..];
        rest.starts_with("```") || rest.starts_with("~~~")
    };
    let mut cursor = if fenced { block.find('\n').map_or(block.len(), |at| at + 1) } else { 0 };
    let mut drawn = 0usize;
    let mut segments = Vec::new();
    for line in code.split('\n') {
        if !line.is_empty()
            && let Some(at) = block[cursor..].find(line)
        {
            let start = span.start + cursor + at;
            segments.push((drawn..drawn + line.len(), start..start + line.len()));
            cursor += at + line.len();
        }
        drawn += line.len() + 1;
    }
    segments
}

/// Case-insensitive, non-overlapping byte ranges of `needle_lower` in `text`.
///
/// Unicode lowercasing can change a character's byte length, so the lowered
/// text keeps the original byte bounds of every lowered byte; a hit is mapped
/// back to whole original characters. Empty queries match nothing.
pub fn match_ranges(text: &str, needle_lower: &str) -> Vec<Range<usize>> {
    if needle_lower.is_empty() || text.is_empty() {
        return Vec::new();
    }
    let mut lower = String::with_capacity(text.len());
    let mut starts = Vec::with_capacity(text.len());
    let mut ends = Vec::with_capacity(text.len());
    for (index, ch) in text.char_indices() {
        let before = lower.len();
        for lowered in ch.to_lowercase() {
            lower.push(lowered);
        }
        let end = index + ch.len_utf8();
        for _ in before..lower.len() {
            starts.push(index);
            ends.push(end);
        }
    }
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find(needle_lower) {
        let start = from + at;
        let end = start + needle_lower.len();
        if let (Some(&source_start), Some(&source_end)) = (starts.get(start), ends.get(end.saturating_sub(1))) {
            out.push(source_start..source_end);
        }
        from = end;
        if from >= lower.len() {
            break;
        }
    }
    out
}

/// `n` steps forward, `N` (or Shift+n) backward, with no command modifier.
fn bare_n(event: &KeyEvent) -> Option<bool> {
    if event.mods.cmd || event.mods.ctrl || event.mods.alt || event.mods.logo {
        return None;
    }
    match &event.key {
        Key::Text(text) if text.eq_ignore_ascii_case("n") => Some(text != "N" && !event.mods.shift),
        _ => None,
    }
}
