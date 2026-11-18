//! Per-tab Back and Forward history (O-12–O-15).
//!
//! Like a browser's tab, a tab remembers the places it left: another document
//! opened from it, a heading jumped to from the outline, the section line or a
//! `#anchor`, or a `file.md#section` link. The lists live in
//! [`TabView::nav`](crate::window::TabView::nav) — [`lmdv_core::NavHistory`]
//! owns the semantics (a 50-place cap, Forward cleared on a new place,
//! consecutive duplicates under 4 points suppressed) — so a tab switch keeps
//! them and the session stores them (O-15).
//!
//! This module is the app's half: which places exist (a document's key plus the
//! reading anchor), when a place is recorded (a document open, a jump that moves
//! the page at least [`MIN_JUMP_DISTANCE`]), and how a place is reached again (a
//! tab brought forward, a document opened, the anchor restored). A move belongs
//! to the tab it is made in: that tab loses the place it went to and keeps the
//! place it left for Forward; the document it lands on is shown in its own tab
//! when the window has one, else opened. A move to a file waits for its load: it
//! is committed only once the place is on screen, so a failed or overtaken open
//! is not a place, and a file that is gone is dropped with the localized “Not
//! found” notice (O-13).
//!
//! Recording happens in handlers and poll, never in a view; nothing here reads
//! the filesystem (the load job decides whether a file is gone).

use std::path::{Path, PathBuf};

use lmdv_core::{DocStatus, DocumentId, DocumentKey, Place, ScrollAnchor, TabId};
use rune::{PointerButton, Tx};

use crate::reader::{ARTICLE_SCROLL_KEY, ScrollSnapshot};
use crate::window::{Tab, WindowState};

/// Where a heading or section jump lands, in points below the viewport top.
///
/// [`WindowState::jump_to_heading`] asks for exactly this inset, and O-12's
/// “lands within 8 px” rule is measured against it, so the two stay in step.
pub const JUMP_INSET: f32 = 12.0;
/// A jump that would move the page less than this is not a new place (O-12).
pub const MIN_JUMP_DISTANCE: f32 = 8.0;

/// Where a heading jump lands: the heading's top `inset` points below the
/// viewport's top, clamped to the document (R-08/O-12).
///
/// `heading_top` is the last frame's measurement, relative to the viewport top;
/// adding the current offset makes the heading's absolute top, so the formula
/// stays right while a glide is still moving both numbers together.
pub fn heading_destination(scroll: ScrollSnapshot, heading_top: f32, inset: f32) -> f32 {
    (scroll.offset + heading_top - inset).clamp(0.0, scroll.max.max(0.0))
}

/// Pasted documents are keyed `pasted:<id>`, as the original's `docKey` spells
/// them.
pub const PASTED_PREFIX: &str = "pasted:";

/// A Back or Forward move whose document is still loading (O-13).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingTravel {
    /// `-1` is Back, `1` is Forward.
    pub(crate) dir: i8,
    /// The place being travelled to; it comes off its list when it is on screen.
    pub(crate) place: Place,
    /// The place that was left when the move started, for the other list.
    pub(crate) here: Option<Place>,
    /// The tab the move belongs to: its lists change, and a failed move drops
    /// its place there.
    pub(crate) origin: Option<TabId>,
    /// The tab the move opened, when it had to open one.
    pub(crate) tab: Option<TabId>,
    /// The document being opened.
    pub(crate) doc: Option<DocumentId>,
}

/// Back/Forward bookkeeping that is not per tab: the move in flight and the
/// suppression used while a place must not be recorded (a session replay).
#[derive(Debug, Default)]
pub(crate) struct NavigationState {
    /// The move waiting for its document.
    pub(crate) pending: Option<PendingTravel>,
    /// While non-zero, document opens do not record a place: a Back/Forward
    /// move records through its own commit, and a session replay is no
    /// navigation at all.
    pub(crate) suppress: u32,
}

impl NavigationState {
    /// Suppresses recording until [`Self::unsuppress`].
    pub(crate) fn suppress(&mut self) {
        self.suppress = self.suppress.saturating_add(1);
    }

    /// Ends one suppression.
    pub(crate) fn unsuppress(&mut self) {
        self.suppress = self.suppress.saturating_sub(1);
    }

    /// Whether recording is currently suppressed.
    pub(crate) fn suppressed(&self) -> bool {
        self.suppress > 0
    }
}

impl WindowState {
    // ---- places -----------------------------------------------------------

    /// The document key a place stores: an absolute path, or `pasted:<id>`.
    pub(crate) fn place_key(document: &lmdv_core::Document) -> Option<String> {
        let key = document.key.borrow();
        match &*key {
            DocumentKey::File(path) => Some(path.to_string_lossy().into_owned()),
            DocumentKey::Untitled(paste) => Some(format!("{PASTED_PREFIX}{}", paste.0)),
            DocumentKey::Start => None,
        }
    }

    /// The place a tab shows, when it shows a reading position (O-13).
    ///
    /// The start page, a loading/error card and a document without a parsed
    /// model are not places: Back over one would return to nothing.
    pub(crate) fn current_place(&self) -> Option<Place> {
        let tab = self.active_tab()?;
        let document = tab.document()?;
        if !matches!(*document.status.borrow(), DocStatus::Ready) {
            return None;
        }
        let key = Self::place_key(document)?;
        let model = tab.view.reader.as_ref()?;
        let anchor = self.place_anchor_for(model);
        Some(Place::new(key, anchor))
    }

    /// The reading anchor for the active document right now, from the last
    /// scroll report and the nearest heading the last frame drew (R-12).
    fn place_anchor_for(&self, model: &lmdv_md::MdDoc) -> ScrollAnchor {
        crate::reading::anchor_for(model, self.reading.scroll.get(), self.reading.viewport_anchor.get())
    }

    /// The active tab's document path, when it has one.
    pub(crate) fn active_path(&self) -> Option<PathBuf> {
        let document = self.active_tab()?.document()?;
        document.key.borrow().path().map(Path::to_path_buf)
    }

    /// The document key of a tab, as history stores it.
    fn tab_place_key(&self, tab: TabId) -> Option<String> {
        self.tab(tab).and_then(Tab::document).and_then(|document| Self::place_key(document))
    }

    /// The tab showing the document `place` names, if this window has one.
    fn tab_for_place(&self, place: &Place) -> Option<TabId> {
        if let Some(paste) = place.path.strip_prefix(PASTED_PREFIX) {
            let id = paste.parse::<u64>().ok()?;
            return self.tabs.iter().find(|tab| tab_paste_id(tab) == Some(id)).map(|tab| tab.id);
        }
        let path = Path::new(&place.path);
        self.tabs
            .iter()
            .find(|tab| {
                tab.document().and_then(|doc| doc.key.borrow().path().map(Path::to_path_buf)).as_deref() == Some(path)
            })
            .map(|tab| tab.id)
    }

    // ---- recording (O-12) -------------------------------------------------

    /// The place an open should record, unless this open is itself a move.
    pub(crate) fn open_origin(&self) -> Option<Place> {
        if self.navigation.suppressed() {
            return None;
        }
        self.current_place()
    }

    /// Records `from` as the new place of an opened tab (O-12).
    ///
    /// Opening the document that is already in front is not a place: the tab
    /// did not move. Every other open — a link, the browser, a recent, a
    /// forwarded launch, the palette — is one.
    pub(crate) fn record_open(&mut self, tab: TabId, from: Option<Place>) {
        let Some(from) = from else { return };
        if self.tab_place_key(tab).as_deref() == Some(from.path.as_str()) {
            return;
        }
        if let Some(tab) = self.tabs.iter_mut().find(|candidate| candidate.id == tab) {
            tab.view.nav.record(from);
        }
    }

    /// Records the place being left before a heading jump (O-12), unless the
    /// jump would move the page less than [`MIN_JUMP_DISTANCE`].
    pub(crate) fn note_jump(&mut self, key: &str) {
        let Some(index) = heading_index(key) else { return };
        if self.jump_moves_less_than(index, MIN_JUMP_DISTANCE) {
            return;
        }
        let Some(here) = self.current_place() else { return };
        if let Some(tab) = self.active_tab_mut() {
            tab.view.nav.record(here);
        }
    }

    /// Whether a jump to heading `index` would move the page less than
    /// `distance` points, from the last frame's geometry.
    ///
    /// Landing inside 8 points of where the heading already is would leave the
    /// reader where they were, so the original does not call that a place. The
    /// destination is the heading's measured top less [`JUMP_INSET`], clamped to
    /// the document, exactly where the scroll request lands.
    fn jump_moves_less_than(&self, index: usize, distance: f32) -> bool {
        let Some(top) = self.heading_top(index) else { return false };
        let scroll = self.reading.scroll.get();
        let dest = heading_destination(scroll, top, JUMP_INSET);
        (dest - scroll.offset).abs() < distance
    }

    // ---- inputs (O-14) ----------------------------------------------------

    /// Back through this tab's places.
    pub fn nav_back(&mut self, tx: &mut Tx) {
        self.travel(-1, tx);
    }

    /// Forward through this tab's places.
    pub fn nav_forward(&mut self, tx: &mut Tx) {
        self.travel(1, tx);
    }

    /// Whether Back has a place to go.
    pub fn nav_can_back(&self) -> bool {
        self.active_tab().is_some_and(|tab| tab.view.nav.can_back())
    }

    /// Whether Forward has a place to go.
    pub fn nav_can_forward(&self) -> bool {
        self.active_tab().is_some_and(|tab| tab.view.nav.can_forward())
    }

    /// The mouse's side buttons: Back and Forward (O-14).
    ///
    /// Rune reports the platform's back/forward buttons through
    /// [`PointerButton`] only for elements that opt in with
    /// `El::pointer_buttons`; the window shell does, so the buttons work over
    /// the whole window like a browser's.
    pub fn nav_pointer_button(&mut self, button: PointerButton, tx: &mut Tx) {
        match button {
            PointerButton::Back => self.nav_back(tx),
            PointerButton::Forward => self.nav_forward(tx),
            _ => {}
        }
    }

    // ---- travel (O-13/O-14) ----------------------------------------------

    /// Goes to the place Back (`dir < 0`) or Forward (`dir > 0`) names.
    ///
    /// The move belongs to the tab it is made in: that tab loses the place from
    /// its list and keeps the place it was showing for the other direction. The
    /// destination document comes forward in its own tab when the window has it,
    /// so a move between documents switches tabs (O-13/requirement 4).
    fn travel(&mut self, dir: i8, tx: &mut Tx) {
        // One move at a time: another place is already being opened.
        if self.navigation.pending.is_some() {
            return;
        }
        let Some(origin) = self.active_tab().map(|tab| tab.id) else { return };
        let Some(place) = self
            .tab(origin)
            .and_then(|tab| if dir < 0 { tab.view.nav.peek_back() } else { tab.view.nav.peek_forward() })
        else {
            return;
        };
        let place = place.clone();
        let here = self.current_place();
        // The same document: the place is reached without a load.
        if here.as_ref().is_some_and(|here| here.path == place.path) {
            self.commit_travel(origin, dir, here, place, tx);
            return;
        }
        // Another tab of this window already shows it.
        if let Some(tab) = self.tab_for_place(&place) {
            self.activate_tab_with_tx(tab, tx);
            self.commit_travel(origin, dir, here, place, tx);
            return;
        }
        // Pasted text has no file to look for: a place without its document is
        // dropped silently (O-13).
        if place.path.starts_with(PASTED_PREFIX) {
            self.drop_place_in(Some(origin), dir);
            return;
        }
        self.open_place(origin, dir, place, here, tx);
    }

    /// Opens a file place, suppressing the open's own record: the move commits
    /// itself once the document is on screen.
    fn open_place(&mut self, origin: TabId, dir: i8, place: Place, here: Option<Place>, tx: &mut Tx) {
        self.navigation.suppress();
        let tab = self.open_path(PathBuf::from(&place.path));
        self.navigation.unsuppress();
        let doc = self.tab(tab).and_then(Tab::document_id);
        self.navigation.pending = Some(PendingTravel { dir, place, here, origin: Some(origin), tab: Some(tab), doc });
        // A document another window already read needs no load job of its own:
        // the place is on screen at once.
        let ready = self
            .tab(tab)
            .and_then(Tab::document)
            .is_some_and(|document| matches!(*document.status.borrow(), DocStatus::Ready));
        if ready && let Some(doc) = doc {
            self.finish_travel(doc, true, None, tx);
        }
    }

    /// Completes a move once its document loaded: commit when the place is on
    /// screen, drop it when the file is gone or the reader moved on (O-13).
    pub(crate) fn finish_travel(
        &mut self,
        doc: DocumentId,
        loaded: bool,
        error: Option<&lmdv_io::ReadError>,
        tx: &mut Tx,
    ) {
        let Some(pending) = self.navigation.pending.clone() else { return };
        if pending.doc != Some(doc) {
            return;
        }
        self.navigation.pending = None;
        let Some(tab) = pending.tab else { return };
        if !loaded {
            self.drop_failed_travel(pending, error, tx);
            return;
        }
        // Another document came first: the move was overtaken, so it is not
        // committed. Its tab still remembers the place.
        if self.active_tab().map(|active| active.id) != Some(tab) {
            if let Some(tab) = self.tabs.iter_mut().find(|candidate| candidate.id == tab) {
                tab.view.anchor = pending.place.pos.clone();
            }
            return;
        }
        let Some(origin) = pending.origin else { return };
        self.commit_travel(origin, pending.dir, pending.here, pending.place, tx);
    }

    /// A Back/Forward move whose file is gone: drop the place, close the tab the
    /// move opened, say “Not found” and stay where the reader was (O-13).
    fn drop_failed_travel(&mut self, pending: PendingTravel, error: Option<&lmdv_io::ReadError>, tx: &mut Tx) {
        self.drop_place_in(pending.origin, pending.dir);
        let origin = pending.origin;
        if let Some(tab) = pending.tab
            && self.tab(tab).is_some()
        {
            // The error card the open made is not a place to stay on, and not
            // one Ctrl+Shift+T should reopen.
            self.close_tab_without_history(tab);
        }
        if let Some(origin) = origin
            && self.tab(origin).is_some()
        {
            self.activate_tab_with_tx(origin, tx);
        }
        let name = Path::new(&pending.place.path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| pending.place.path.clone());
        match error {
            Some(lmdv_io::ReadError::NotFound(_)) => {
                self.notice(lmdv_i18n::t!("msg.notFound", "name" => name));
            }
            Some(error) => self.notice(lmdv_i18n::t!("msg.openFailed", "name" => name, "err" => error)),
            None => {}
        }
    }

    /// Takes `here` into `tab`'s other list and puts `place` on screen, without
    /// a landing highlight and without recording the move (O-14).
    ///
    /// The journey follows the reader: when the place is in another tab, that tab
    /// continues it — the updated lists are copied there, so Forward on the
    /// landing tab returns to the place just left, and a later switch back keeps
    /// the trail. The tab the move was made in keeps its own (already updated)
    /// lists.
    fn commit_travel(&mut self, tab: TabId, dir: i8, here: Option<Place>, place: Place, tx: &mut Tx) {
        let updated = self.tabs.iter_mut().find(|candidate| candidate.id == tab).map(|tab| {
            if dir < 0 {
                tab.view.nav.commit_back(here);
            } else {
                tab.view.nav.commit_forward(here);
            }
            tab.view.nav.clone()
        });
        let Some(updated) = updated else { return };
        // The destination tab keeps the place it was brought to, so a later
        // switch to it lands there; the restore below shows it now.
        if let Some(active) = self.active_tab_mut() {
            if active.id != tab {
                active.view.nav = updated;
            }
            active.view.anchor = place.pos.clone();
        }
        self.restore_place(&place, tx);
    }

    /// Restores a place's anchor in the active tab (O-13).
    ///
    /// The top of the document is a place of its own (R-14): a top place scrolls
    /// there at once, while a heading place waits for the model through the
    /// normal restore path.
    pub(crate) fn restore_place(&mut self, place: &Place, tx: &mut Tx) {
        if place.pos.is_top() {
            self.reading.pending = None;
            self.reading.restoring = false;
            self.reading.last = None;
            self.reading.settled = false;
            self.reading.viewport_anchor.set(None);
            self.reading.heading_tops.borrow_mut().clear();
            self.reading.scroll.set(ScrollSnapshot::default());
            self.reading.landing = None;
            self.reading.landing_flash = None;
            self.section_line.reset();
            tx.scroll_to_y(ARTICLE_SCROLL_KEY, 0.0);
            return;
        }
        self.begin_restore(&place.pos);
        self.apply_pending_scroll(tx);
    }

    /// Forgets a pending move whose tab or destination was closed.
    pub(crate) fn cancel_travel_for(&mut self, tab: TabId) {
        if self
            .navigation
            .pending
            .as_ref()
            .is_some_and(|pending| pending.tab == Some(tab) || pending.origin == Some(tab))
        {
            self.navigation.pending = None;
        }
    }

    /// Drops the place a tab's list would go to.
    fn drop_place_in(&mut self, tab: Option<TabId>, dir: i8) {
        if let Some(tab) = tab.and_then(|tab| self.tabs.iter_mut().find(|candidate| candidate.id == tab)) {
            tab.view.nav.drop_place(dir);
        }
    }

    /// Makes every tab's history follow a renamed file (O-15).
    pub(crate) fn rename_history_paths(&mut self, from: &Path, to: &Path) {
        let from = from.to_string_lossy();
        let to = to.to_string_lossy();
        for tab in &mut self.tabs {
            tab.view.nav.rename_path(&from, &to);
        }
    }

    // ---- pending fragment (O-12) -----------------------------------------

    /// Applies a `file.md#section` link's heading once its document is parsed.
    ///
    /// A fragment that names nothing keeps the remembered position, as the
    /// original does. The jump is part of the open, so it records no place of
    /// its own.
    pub(crate) fn apply_pending_fragment(&mut self, tx: &mut Tx) -> bool {
        let Some(tab) = self.active_tab() else { return false };
        let Some(fragment) = tab.view.fragment.clone() else { return false };
        let Some(model) = tab.view.reader.clone() else { return false };
        if !tab.document().is_some_and(|document| matches!(*document.status.borrow(), DocStatus::Ready)) {
            return false;
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.view.fragment = None;
        }
        let Some(index) = model.resolve_heading(&fragment) else { return false };
        self.scroll_to_heading(&lmdv_render::heading_key(index), tx);
        true
    }
}

/// The heading index a rendered key names (`h{i}`).
fn heading_index(key: &str) -> Option<usize> {
    key.strip_prefix('h').and_then(|digits| digits.parse::<usize>().ok())
}

/// The pasted id a tab shows, when it is pasted text.
fn tab_paste_id(tab: &Tab) -> Option<u64> {
    match &*tab.document()?.key.borrow() {
        DocumentKey::Untitled(paste) => Some(paste.0),
        DocumentKey::File(_) | DocumentKey::Start => None,
    }
}
