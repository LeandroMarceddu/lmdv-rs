//! The tab hover card (T-16) and the tab strip's pointer-frozen reading time
//! (T-35).
//!
//! After 550 ms over a tab, a floating card names the document in full, its
//! folder (or “Pasted text, in no file yet” / “Start page with the recent
//! files”) and either “Unsaved changes” or the reading time and word count.
//! Moving from one tab straight to the next keeps the card already up, so it
//! follows at once, as the original's `scheduleCard` does
//! (`src/tabs/strip.ts:817-856`, ported). The card is a
//! layer popover anchored under its tab: it takes no layout, so hovering never
//! changes a tab's width.
//!
//! The card hides on leaving the tab, on a tab switch, while a menu, the
//! palette, a dialog or a preview is open, during a drag, and in edit mode.
//! The strip's inline reading time (R-05) is hidden while a tab is in hand or
//! while a pointer close froze the widths (T-35); the card keeps the time.
//!
//! Reading stats are computed for the tab in front (R-04). A background tab
//! that has never been in front has no count yet; showing its card computes
//! one from the model it already adopted, once.

use lmdv_core::DocumentKey;
use rune::{El, Tx, col, text};
use rune_kit::shell::TabDragPhase;
use rune_kit::{Kit, Placement, popover};

use crate::window::{Mode, Tab, TabSource, WindowState, parse_tab_key, tab_key};

/// How long the pointer must rest on a tab before its card appears (T-16).
pub const TAB_CARD_DELAY: f64 = 0.55;
/// Points between a tab's bottom and its card (the original's 6).
pub const TAB_CARD_GAP: f32 = 6.0;
/// A card follows the pointer from tab to tab inside this many seconds after
/// leaving the last one, as crossing the strip's gaps is one gesture (T-16).
pub const TAB_CARD_WARM: f64 = 0.4;
/// The card's widest line; a long path truncates inside it.
pub const TAB_CARD_MAX_WIDTH: f32 = 360.0;
/// The card layer element's key.
pub const TAB_CARD_KEY: &str = "tab-card";

/// One tab's card text, so the content rules are testable without a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabCardContent {
    /// The full document name.
    pub name: String,
    /// The folder, or what a pasted/start tab says instead.
    pub location: String,
    /// “Unsaved changes”, the reading time and words, or nothing (a start tab
    /// or a document still loading).
    pub meta: Option<String>,
}

/// The pointer's tab and the card it shows (T-16).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TabCardState {
    /// The tab the pointer is in, when it is in one.
    pub(crate) hovered: Option<lmdv_core::TabId>,
    /// The tab whose card is up.
    pub(crate) shown: Option<lmdv_core::TabId>,
    /// Clock time at which a resting pointer's card appears.
    pub(crate) due: Option<f64>,
    /// Until then, moving onto another tab keeps the card at once.
    pub(crate) warm_until: Option<f64>,
}

impl TabCardState {
    /// The tab whose card is up.
    pub fn shown(&self) -> Option<lmdv_core::TabId> {
        self.shown
    }

    /// The tab the pointer is in.
    pub fn hovered(&self) -> Option<lmdv_core::TabId> {
        self.hovered
    }
}

/// What a tab's card says (T-16).
pub fn card_content(tab: &Tab) -> TabCardContent {
    let meta = if tab.view.dirty {
        Some(lmdv_i18n::t!("tab.unsaved").to_owned())
    } else {
        tab.view.reading.map(|stats| stats.description())
    };
    let location = match &tab.source {
        TabSource::Start => lmdv_i18n::t!("tab.startPage").to_owned(),
        TabSource::Document(doc) => match &*doc.key.borrow() {
            DocumentKey::File(path) => {
                path.parent().map(|folder| folder.display().to_string()).unwrap_or_else(|| path.display().to_string())
            }
            DocumentKey::Untitled(_) => lmdv_i18n::t!("tab.pasted").to_owned(),
            DocumentKey::Start => lmdv_i18n::t!("tab.startPage").to_owned(),
        },
    };
    TabCardContent { name: tab.full_title(), location, meta }
}

impl WindowState {
    /// The pointer entered (`inside`) or left a tab, from the strip's
    /// `on_hover` (T-16).
    pub(crate) fn tab_hover_changed(&mut self, id: &str, inside: bool, tx: &mut Tx) {
        let Some(tab) = parse_tab_key(id).filter(|tab| self.tab(*tab).is_some()) else { return };
        if inside {
            if self.tab_card_blocked() {
                self.hide_tab_card();
                return;
            }
            self.tab_card.hovered = Some(tab);
            let now = tx.clock();
            if self.tab_card.shown.is_some() || self.tab_card.warm_until.is_some_and(|until| now <= until) {
                // From one tab to the next the card follows at once (T-16).
                self.tab_card.warm_until = None;
                self.show_tab_card(tab);
            } else {
                let due = now + TAB_CARD_DELAY;
                self.tab_card.due = Some(due);
                tx.wake_at(due);
            }
            return;
        }
        if self.tab_card.hovered == Some(tab) {
            self.tab_card.hovered = None;
            self.tab_card.due = None;
            let was_shown = self.tab_card.shown.take().is_some();
            self.tab_card.warm_until = was_shown.then_some(tx.clock() + TAB_CARD_WARM);
        }
    }

    /// Dismisses the card and any pending delay; returns whether anything was
    /// showing or waiting.
    pub fn hide_tab_card(&mut self) -> bool {
        let had = self.tab_card.shown.is_some() || self.tab_card.due.is_some();
        self.tab_card.hovered = None;
        self.tab_card.shown = None;
        self.tab_card.due = None;
        self.tab_card.warm_until = None;
        had
    }

    /// The card shown, for the view and tests.
    pub fn tab_card(&self) -> &TabCardState {
        &self.tab_card
    }

    /// Runs a due card and hides one the window's state no longer allows.
    pub(crate) fn poll_tab_card(&mut self, now: f64) -> bool {
        if self.tab_card_blocked() {
            return self.hide_tab_card();
        }
        let mut changed = false;
        if self.tab_card.warm_until.is_some_and(|until| now > until) {
            self.tab_card.warm_until = None;
        }
        if let Some(due) = self.tab_card.due
            && now >= due
        {
            self.tab_card.due = None;
            if let Some(tab) = self.tab_card.hovered {
                changed |= self.show_tab_card(tab);
            }
        }
        if let Some(tab) = self.tab_card.shown
            && self.tab(tab).is_none()
        {
            self.tab_card.shown = None;
            changed = true;
        }
        changed
    }

    /// Shows `tab`'s card at once, without a pointer: the headless `tab-card`
    /// scene and tests use it.
    pub(crate) fn show_tab_card_now(&mut self, tab: lmdv_core::TabId) -> bool {
        self.tab_card.hovered = Some(tab);
        self.tab_card.due = None;
        self.show_tab_card(tab)
    }

    /// Shows `tab`'s card, computing its reading time once when the tab has
    /// never been in front.
    fn show_tab_card(&mut self, tab: lmdv_core::TabId) -> bool {
        let Some(index) = self.tabs.iter().position(|candidate| candidate.id == tab) else { return false };
        {
            let tab = &mut self.tabs[index];
            if tab.view.reading.is_none()
                && let (Some(model), Some(revision)) = (tab.view.reader.clone(), tab.view.reader_revision)
            {
                tab.view.reading = Some(crate::reading_meta::reading_stats(&model));
                tab.view.reading_revision = Some(revision);
            }
        }
        self.tab_card.shown = Some(tab);
        true
    }

    /// Whether something modal or a drag owns the strip right now (T-16).
    fn tab_card_blocked(&self) -> bool {
        // A tab in hand, or a press on the strip that could become one.
        if !matches!(self.drag.phase(), TabDragPhase::Idle) {
            return true;
        }
        if self.transfer.menu().is_some()
            || self.browser.menu.is_some()
            || self.ui.block_menu.is_some()
            || self.share.menu_open
            || self.palette_is_open()
            || self.tab_list_is_open()
            || self.help_view().is_some()
            || self.prompt().is_some()
            || self.tables.preview.is_some()
            || self.lightbox.is_some()
        {
            return true;
        }
        // Editing owns the tab; the card's time is not needed then.
        self.active_tab().is_some_and(|tab| tab.view.mode == Mode::Editor)
    }
}

/// The floating card under its tab (T-16).
pub(crate) fn card(state: &WindowState, kit: &Kit, layer: &mut Vec<El>) {
    let Some(id) = state.tab_card.shown else { return };
    let Some(tab) = state.tab(id) else { return };
    if state.tab_card_blocked() {
        // `poll_tab_card` hides it first; this is the same frame's safety net.
        return;
    }
    let c = &kit.c;
    let content = card_content(tab);
    let unsaved = tab.view.dirty;
    let mut body = col()
        .key(TAB_CARD_KEY)
        .padding(10.0)
        .gap(2.0)
        .min_w(180.0)
        .max_w(TAB_CARD_MAX_WIDTH)
        // The tab's accessible description already carries the time (R-05);
        // the card is its visual form and must not be read twice.
        .a11y_hidden()
        .child(text(content.name).key("tab-card-name").font(kit.ty.ui_medium).color(c.text_base).truncate());
    body =
        body.child(text(content.location).key("tab-card-location").font(kit.ty.small).color(c.text_faint).truncate());
    if let Some(meta) = content.meta {
        body = body.child(
            text(meta)
                .key("tab-card-meta")
                .font(kit.ty.small)
                .color(if unsaved { c.warning.fg } else { c.text_muted })
                .truncate(),
        );
    }
    let surface = popover::raised(kit, body, kit.radius.md);
    // The strip's slot for this tab, `t-tab-7` (`rune-kit`'s `tab::build`).
    let anchor = format!("t-{}", tab_key(id));
    layer.push(popover::anchored(kit, surface, &anchor, Placement::BottomStart, TAB_CARD_GAP));
}
