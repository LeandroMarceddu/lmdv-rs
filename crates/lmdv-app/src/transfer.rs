//! Cross-window tab transfer (P4-02b): the same-thread protocol behind
//! “Move to a new window” / “Move tab to window…” and, in P4-02c, the strip's
//! cross-window drop.
//!
//! The original posts a `TabOffer` and waits up to 15 s for `TabAccepted`
//! (`src/main.ts:2742-2782,2784-2850`). lmdv has no webview
//! boundary: every window's inbox is an `Rc<UiMailbox>` on the UI thread, so a
//! reply is reliable and the timeout is not applicable. The only vanished
//! target — a window removed from the registry before the message can be
//! posted — restores the tab locally (`ReturnReason::TargetVanished`). A target
//! whose close guard is running answers `Returned { TargetClosing }` and the
//! source keeps its place at `min(origin_index, len)`.
//!
//! Invariants:
//!
//! * the source detaches the tab but keeps its `Document.subscribers` entry and
//!   edit lease until `Adopted`/`Returned` decides;
//! * the tab carries its [`TabView`](crate::window::TabView) — scroll anchor,
//!   find bar, nav history and editor session — so the target resumes where the
//!   source was;
//! * one tab per document per window still holds (T-22): a clean duplicate
//!   merges into the target's tab, a dirty one is refused and returned.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use lmdv_core::{DocStatus, DocumentId, EditLease, TabId, WindowId};
use lmdv_io::Launch;
use rune::PointerPress;

use crate::shared::UiMsg;
use crate::window::{Mode, Tab, WindowState};
use crate::windows::{WindowKind, WindowOpenRequest, close_title};

/// Correlates one `Adopt` with its `Adopted`/`Returned` reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TransferToken(pub u64);

/// The tab while it is between windows.
///
/// The source builds it after detaching the tab and keeps only the bookkeeping
/// in [`TransferState::pending`]; the tab itself travels in
/// [`TransferMsg::Adopt`] and comes back in [`TransferMsg::Returned`] when the
/// target refuses it.
#[derive(Clone, Debug)]
pub struct Outgoing {
    /// Reply correlation.
    pub token: TransferToken,
    /// The window that detached the tab.
    pub origin: WindowId,
    /// The window asked to adopt it.
    pub target: WindowId,
    /// Where the tab was in the source strip, for a refusal.
    pub origin_index: usize,
    /// Where a drop asks the target to insert it; `None` inserts after the
    /// target's active tab (or replaces an active start page).
    pub insert_at: Option<usize>,
    /// The tab, with its document reference and per-tab view state.
    pub tab: Tab,
}

/// Why a target gave a tab back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReturnReason {
    /// The target's close guard is running; routing there is refused (T-25).
    TargetClosing,
    /// The registry lost the target between validation and posting.
    TargetVanished,
    /// The document is already there and one of the views is dirty (T-22).
    DuplicateDirty,
    /// The message arrived at a window that does not match `target`.
    WrongWindow,
}

/// The transfer protocol. Every variant travels through the same-thread
/// [`UiMailbox`](crate::shared::UiMailbox); `Tab` is not `Send`.
#[derive(Clone, Debug)]
pub enum TransferMsg {
    /// A drag hovers over `target` at a window point (P4-02c); `None` clears.
    Hover {
        /// The drag's token.
        token: TransferToken,
        /// The window the pointer is over.
        target: WindowId,
        /// The pointer in target-window points, or `None` when it left.
        point: Option<[f32; 2]>,
    },
    /// Offer the detached tab to its target.
    Adopt(Box<Outgoing>),
    /// The target took the tab. `merged` means the target already showed the
    /// clean document and the moved view was folded into its tab.
    Adopted {
        /// The offer's token.
        token: TransferToken,
        /// The moved tab was dropped in favour of an existing clean view.
        merged: bool,
    },
    /// The target refused; the tab comes back to the source's old place.
    Returned {
        /// The offer's token.
        token: TransferToken,
        /// The same tab, untouched.
        tab: Box<Tab>,
        /// Why it came back.
        reason: ReturnReason,
    },
}

/// One open window “Move tab to window…” may offer, in focus order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowTarget {
    /// The window identity to post `Adopt` to.
    pub id: WindowId,
    /// The window's published title.
    pub title: String,
}

/// The tab context menu or the window chooser the view draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabMenu {
    /// The tab the menu acts on.
    pub tab: TabId,
    /// Where the context menu opens, in window points.
    pub point: PointerPress,
    /// `true` lists the other open windows; `false` is the tab's context menu.
    pub chooser: bool,
}

/// The pointer hovering over a target during a cross-window drag (P4-02c).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IncomingHover {
    /// The drag's token.
    pub token: TransferToken,
    /// The pointer in target-window points.
    pub point: [f32; 2],
}

/// Where an adopted tab lands in the target strip: `(index, replaces_start)`.
///
/// A requested slot (a cross-window drop) or the target strip's open incoming
/// gap always inserts, even past the last tab; only a menu move with neither
/// replaces the active start page, and otherwise lands after the active tab.
pub(crate) fn adopt_index(
    insert_at: Option<usize>,
    drop_slot: Option<usize>,
    active: usize,
    len: usize,
    active_is_start: bool,
) -> (usize, bool) {
    match insert_at.or(drop_slot).map(|index| index.min(len)) {
        Some(index) => (index, false),
        None if active_is_start => (active.min(len.saturating_sub(1)), true),
        None => ((active + 1).min(len), false),
    }
}

/// One tab in flight, as the source remembers it.
#[derive(Clone, Debug)]
struct PendingOutgoing {
    target: WindowId,
    origin_index: usize,
    doc: DocumentId,
    tab_id: TabId,
}

/// The window's half of the transfer protocol: tabs it sent, a drag hovering
/// here, and the context menu waiting for a choice.
#[derive(Debug, Default)]
pub(crate) struct TransferState {
    pending: BTreeMap<TransferToken, PendingOutgoing>,
    hover: Option<IncomingHover>,
    menu: Option<TabMenu>,
}

impl TransferState {
    /// Number of tabs this window is waiting to hear about.
    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// The target of one in-flight tab.
    #[cfg(test)]
    pub(crate) fn pending_target(&self, token: TransferToken) -> Option<WindowId> {
        self.pending.get(&token).map(|pending| pending.target)
    }

    /// The first in-flight tab's token.
    #[cfg(test)]
    pub(crate) fn pending_token(&self) -> Option<TransferToken> {
        self.pending.keys().next().copied()
    }

    /// The drag hovering over this window.
    #[cfg(test)]
    pub(crate) fn hover(&self) -> Option<IncomingHover> {
        self.hover
    }

    /// The open tab menu.
    pub(crate) fn menu(&self) -> Option<TabMenu> {
        self.menu
    }
}

impl WindowState {
    /// The open windows “Move tab to window…” offers: every live, non-closing
    /// window except this one, in the same focus order launch routing uses.
    pub fn window_targets(&self) -> Vec<WindowTarget> {
        let windows = self.shared.windows();
        windows
            .focus_order()
            .into_iter()
            .filter(|id| *id != self.id())
            .filter_map(|id| windows.get(id))
            .filter(|entry| !entry.closing)
            .map(|entry| WindowTarget { id: entry.id, title: entry.title.clone() })
            .collect()
    }

    /// The tab menu the view should draw.
    pub(crate) fn tab_menu(&self) -> Option<TabMenu> {
        self.transfer.menu()
    }

    /// Opens the tab's context menu at a point (`on_secondary_press`).
    pub fn show_tab_menu(&mut self, tab: TabId, point: PointerPress) {
        if self.tab(tab).is_none() {
            return;
        }
        self.transfer.menu = Some(TabMenu { tab, point, chooser: false });
    }

    /// Opens the window chooser for `tab`. Without a pointer (the command
    /// table), it opens near the tab strip.
    pub fn show_tab_window_menu(&mut self, tab: TabId, point: Option<PointerPress>) {
        if self.tab(tab).is_none() {
            return;
        }
        let point =
            point.or_else(|| self.transfer.menu.map(|menu| menu.point)).unwrap_or(PointerPress { x: 140.0, y: 72.0 });
        self.transfer.menu = Some(TabMenu { tab, point, chooser: true });
    }

    /// Closes the tab menu (Escape, an outside press, or a choice).
    pub fn close_tab_menu(&mut self) {
        self.transfer.menu = None;
    }

    /// Whether this tab may leave the window right now.
    ///
    /// A loading document has a load job owned by this window's worker; moving
    /// it would strand the document if this window closes first. A save in
    /// flight, an open decision dialog or a running close/quit also keep the
    /// tab in place.
    pub fn can_move_tab(&self, tab: TabId) -> bool {
        let Some(tab) = self.tab(tab) else { return false };
        let Some(document) = tab.document() else { return false };
        if !matches!(*document.status.borrow(), DocStatus::Ready) {
            return false;
        }
        if self.ui.close_requested || self.shared.is_quitting() || self.prompt().is_some() {
            return false;
        }
        if self.editor.save_texts.contains_key(&document.id) {
            return false;
        }
        // OI-105: a document being exported cannot move to another window.
        if self.export_pins(document.id) {
            return false;
        }
        if self.transfer.pending.values().any(|pending| pending.doc == document.id) {
            return false;
        }
        !self.shared.windows().get(self.id()).is_some_and(|entry| entry.closing)
    }

    /// “Move to a new window”: opens a window through the installed opener and
    /// offers the tab to it (P4-02c's menu fallback for tear-out).
    pub(crate) fn move_tab_to_new_window(&mut self, tab: TabId) {
        if !self.can_move_tab(tab) {
            return;
        }
        let position = self.shared.cascade_position(self.id());
        let request = WindowOpenRequest { launch: Launch::default(), kind: WindowKind::Normal, position };
        match Rc::clone(&self.shared).request_new_window(request) {
            Ok(opened) => {
                if let Err(error) = self.move_tab_to_window(tab, opened.id) {
                    self.notice(lmdv_i18n::t!("msg.tearOffFailed", "err" => error));
                }
            }
            Err(error) => self.notice(lmdv_i18n::t!("msg.tearOffFailed", "err" => error)),
        }
    }

    /// Detaches `tab` and offers it to `target`.
    ///
    /// The target decides: clean duplicate → merge, dirty duplicate or closing
    /// → `Returned`. The source keeps the subscriber entry and lease until the
    /// answer arrives, so a refusal is lossless.
    pub(crate) fn move_tab_to_window(&mut self, tab: TabId, target: WindowId) -> Result<(), String> {
        self.move_tab_to_window_at(tab, target, None)
    }

    /// [`Self::move_tab_to_window`] with the target strip's insertion slot,
    /// used by P4-02c's cross-window drop. `None` inserts after the target's
    /// active tab (or replaces its active start page).
    pub(crate) fn move_tab_to_window_at(
        &mut self,
        tab: TabId,
        target: WindowId,
        insert_at: Option<usize>,
    ) -> Result<(), String> {
        if target == self.id() {
            return Err(lmdv_i18n::t!("transfer.sameWindow").to_owned());
        }
        if !self.can_move_tab(tab) {
            return Err(lmdv_i18n::t!("transfer.cannotMove").to_owned());
        }
        if self.shared.windows().get(target).is_none() {
            return Err(lmdv_i18n::t!("transfer.targetGone").to_owned());
        }
        let outgoing = self.detach_for_transfer(tab, target, insert_at)?;
        self.transfer.pending.insert(
            outgoing.token,
            PendingOutgoing {
                target: outgoing.target,
                origin_index: outgoing.origin_index,
                doc: outgoing.tab.document_id().unwrap_or(DocumentId(0)),
                tab_id: outgoing.tab.id,
            },
        );
        match self.shared.try_post_window(target, UiMsg::Transfer(TransferMsg::Adopt(Box::new(outgoing)))) {
            Ok(()) => Ok(()),
            Err(message) => {
                // The registry lost the target between the check and the post:
                // this is the only “vanished” path, and it is local (no timeout).
                // The message still owns the tab, so it is restored, not dropped.
                if let UiMsg::Transfer(TransferMsg::Adopt(outgoing)) = message {
                    self.receive_returned(outgoing.token, outgoing.tab, ReturnReason::TargetVanished);
                }
                Err(lmdv_i18n::t!("transfer.targetGone").to_owned())
            }
        }
    }

    /// Removes the tab from the strip without releasing its document: the
    /// subscriber set and lease stay until the target answers.
    fn detach_for_transfer(
        &mut self,
        tab_id: TabId,
        target: WindowId,
        insert_at: Option<usize>,
    ) -> Result<Outgoing, String> {
        let index = self.tabs.iter().position(|tab| tab.id == tab_id).ok_or_else(|| lmdv_i18n::t!("transfer.noTab"))?;
        let Some(document) = self.tabs[index].document().cloned() else {
            return Err(lmdv_i18n::t!("transfer.startPage").to_owned());
        };
        if !matches!(*document.status.borrow(), DocStatus::Ready) {
            return Err(lmdv_i18n::t!("transfer.loading").to_owned());
        }
        if self.editor.save_texts.contains_key(&document.id) {
            return Err(lmdv_i18n::t!("transfer.saving").to_owned());
        }

        // Commit block fields and write drafts while the tab is still ours.
        self.flush_all_drafts();
        let was_active = index == self.active;
        if was_active {
            self.capture_active_position();
            self.save_outline_filter();
            self.reset_scroll_sync();
            self.editor.goto_line = None;
            self.ui.block_menu = None;
            self.share.menu_open = false;
        }

        let tab = self.tabs.remove(index);
        if index < self.active {
            self.active = self.active.saturating_sub(1);
        } else if index == self.active {
            self.active = index.min(self.tabs.len().saturating_sub(1));
        }
        if self.tabs.is_empty() {
            self.push_start_tab();
            self.active = 0;
        }

        // The view's watch token belongs to the tab: stop following here, and
        // the target starts following when it inserts the tab. Subscribers and
        // the lease deliberately stay until the target answers.
        if let Some(path) = tab.source.document().and_then(|doc| doc.key.borrow().path().map(PathBuf::from)) {
            self.shared.unwatch_path(&path);
        }
        self.parse_inflight.remove(&document.id);
        self.sync_editor_panes();
        if was_active {
            let anchor = self.active_tab().map(|tab| tab.view.anchor.clone()).unwrap_or_default();
            self.begin_restore(&anchor);
            self.load_outline_filter();
        }
        self.sync_outline();
        self.sync_editor_titles();
        self.sync_registry();

        Ok(Outgoing {
            token: self.shared.allocate_transfer(),
            origin: self.id(),
            target,
            origin_index: index,
            insert_at,
            tab,
        })
    }

    /// Handles one transfer message addressed to this window.
    pub(crate) fn handle_transfer(&mut self, msg: TransferMsg) -> bool {
        match msg {
            TransferMsg::Hover { token, target, point } => self.handle_hover(token, target, point),
            TransferMsg::Adopt(outgoing) => self.adopt_tab(*outgoing),
            TransferMsg::Adopted { token, merged } => self.finish_outgoing(token, merged),
            TransferMsg::Returned { token, tab, reason } => self.receive_returned(token, *tab, reason),
        }
    }

    /// A drag hovering here (P4-02c): the strip opens its incoming gap.
    fn handle_hover(&mut self, token: TransferToken, target: WindowId, point: Option<[f32; 2]>) -> bool {
        if target != self.id() || self.is_closing_target() {
            return false;
        }
        let next = point.map(|point| IncomingHover { token, point });
        if self.transfer.hover == next {
            return false;
        }
        self.transfer.hover = next;
        self.drag.set_incoming(point);
        true
    }

    /// The target's side of `Adopt`.
    fn adopt_tab(&mut self, outgoing: Outgoing) -> bool {
        let Outgoing { token, origin, target, insert_at, mut tab, .. } = outgoing;
        // A cross-window drop inserts at the slot its target strip opened from
        // `on_tear_move`'s hover. The source cannot read another window's
        // strip, so `insert_at: None` means “the open incoming slot” here; a
        // menu move has no hover and falls back to after the active tab.
        let drop_slot = self.transfer.hover.is_some().then(|| self.drag.incoming_slot()).flatten();
        self.clear_incoming();
        if target != self.id() {
            return self.return_tab(origin, token, tab, ReturnReason::WrongWindow);
        }
        if self.is_closing_target() {
            return self.return_tab(origin, token, tab, ReturnReason::TargetClosing);
        }
        let Some(document) = tab.document().cloned() else {
            return self.return_tab(origin, token, tab, ReturnReason::WrongWindow);
        };
        let old_tab_id = tab.id;

        if let Some(existing) = self.index_of_document(document.id) {
            // T-22: the same file is already here. One tab per document per
            // window: merge when both are clean, refuse when either is dirty.
            let dirty = document.is_dirty() || tab.view.dirty || self.tabs[existing].view.dirty;
            if dirty {
                self.notice(lmdv_i18n::t!("msg.tabRefusedSameFile", "name" => close_title(&document)));
                return self.return_tab(origin, token, tab, ReturnReason::DuplicateDirty);
            }
            // The moved view folds into the existing tab. A lease that pointed
            // at the moved tab (a clean editor session) ends with it.
            if let Some(lease) = document.lease_holder()
                && lease.window == origin
                && lease.tab == old_tab_id
            {
                document.release_edit(lease);
            }
            self.switch_to_index(existing);
            drop(tab);
            self.sync_registry();
            self.reply_adopted(origin, token, true);
            return true;
        }

        // A normal adoption gets a fresh per-window tab id; the document and
        // every per-tab view field travel unchanged.
        let new_tab_id = self.allocate_tab();
        tab.id = new_tab_id;
        tab.view.dirty = document.is_dirty();
        tab.refresh_title();
        self.adopt_cached_parse(&mut tab, &document);
        // A requested slot or an open incoming gap inserts there, even past the
        // last tab; only a menu move with neither replaces the active start
        // page. (`Some(len)` is a valid append slot, never a replacement.)
        let (index, replacing_start) = adopt_index(
            insert_at,
            drop_slot,
            self.active,
            self.tabs.len(),
            self.tabs.get(self.active).is_some_and(Tab::is_start),
        );
        if !replacing_start {
            self.capture_active_position();
            self.save_outline_filter();
        }
        if replacing_start {
            self.tabs[index] = tab;
        } else {
            self.tabs.insert(index, tab);
        }
        self.active = index;
        self.editor.goto_line = None;
        self.ui.block_menu = None;
        self.share.menu_open = false;
        self.reset_scroll_sync();
        self.suppress_scroll_sync();

        document.subscribers.borrow_mut().insert(self.id());
        if let Some(lease) = document.lease_holder()
            && lease.window == origin
            && lease.tab == old_tab_id
        {
            // The source held the edit lease: it moves with the tab (T-22).
            document.take_over_edit(self.id(), new_tab_id);
        } else if self.tabs[index].view.mode == Mode::Editor {
            // Defensive: an editor view must own its lease. If a third window
            // took it over meanwhile, this view becomes the live viewer.
            if document.acquire_edit(self.id(), new_tab_id).is_err()
                && let Some(tab) = self.tabs.get_mut(index)
            {
                tab.view.mode = Mode::Viewer;
                tab.view.editor = None;
                tab.view.edit_base = None;
            }
        }
        if let Some(path) = document.key.borrow().path().map(PathBuf::from) {
            self.shared.watch_path(&path);
        }
        let anchor = self.tabs[index].view.anchor.clone();
        self.sync_editor_panes();
        self.begin_restore(&anchor);
        self.load_outline_filter();
        self.sync_outline();
        self.sync_editor_titles();
        self.sync_registry();
        self.request_draft_check(document.id);
        self.reply_adopted(origin, token, false);
        true
    }

    /// The source's side of `Adopted`: release the bookkeeping and make the
    /// target the focus.
    fn finish_outgoing(&mut self, token: TransferToken, _merged: bool) -> bool {
        let Some(pending) = self.transfer.pending.remove(&token) else { return false };
        if let Some(document) = self.shared.docs().get(pending.doc) {
            // If the document came back here while the tab was away, this
            // window's new view keeps its subscriber entry.
            if self.index_of_document(pending.doc).is_none() {
                document.subscribers.borrow_mut().remove(&self.id());
                if document.subscribers.borrow().is_empty() {
                    self.shared.forget_parsed(pending.doc);
                }
            }
            // A stale lease (the target took over, or the merge released it)
            // is ignored by `release_edit`; this only cleans up if neither ran.
            document.release_edit(EditLease { window: self.id(), tab: pending.tab_id });
        }
        self.shared.docs_mut().collect_unused();
        self.shared.focus_window(pending.target);
        self.sync_registry();
        true
    }

    /// Reinserts a refused tab at its old place, keeping its text and lease.
    fn receive_returned(&mut self, token: TransferToken, mut tab: Tab, _reason: ReturnReason) -> bool {
        let Some(pending) = self.transfer.pending.remove(&token) else { return false };
        self.clear_incoming();
        let Some(document) = tab.document().cloned() else { return false };

        // The user may have reopened the document here while the tab was away.
        // One tab per document wins; the shared buffer keeps the text. The
        // returned view's editor session is not reused, so its lease ends.
        if let Some(index) = self.index_of_document(document.id) {
            if let Some(lease) = document.lease_holder()
                && lease.window == self.id()
                && lease.tab == pending.tab_id
            {
                document.release_edit(lease);
            }
            self.switch_to_index(index);
            drop(tab);
            self.sync_registry();
            return true;
        }

        self.adopt_cached_parse(&mut tab, &document);
        let replace_start = self.tabs.len() == 1 && self.tabs[0].is_start();
        if replace_start {
            self.tabs[0] = tab;
            self.active = 0;
        } else {
            let index = pending.origin_index.min(self.tabs.len());
            self.tabs.insert(index, tab);
            if index <= self.active {
                self.active = self.active.saturating_add(1);
            }
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        document.subscribers.borrow_mut().insert(self.id());
        if let Some(path) = document.key.borrow().path().map(PathBuf::from) {
            self.shared.watch_path(&path);
        }
        self.sync_editor_panes();
        if replace_start {
            let anchor = self.tabs[self.active].view.anchor.clone();
            self.begin_restore(&anchor);
            self.load_outline_filter();
        }
        self.sync_outline();
        self.sync_editor_titles();
        self.sync_registry();
        true
    }

    /// Sends a refused tab home. If the origin itself vanished, the tab is
    /// dropped but its shared buffer survives and any stale lease is released.
    fn return_tab(&mut self, origin: WindowId, token: TransferToken, tab: Tab, reason: ReturnReason) -> bool {
        let lease = tab
            .document()
            .and_then(|document| document.lease_holder().filter(|lease| lease.window == origin && lease.tab == tab.id));
        match self
            .shared
            .try_post_window(origin, UiMsg::Transfer(TransferMsg::Returned { token, tab: Box::new(tab), reason }))
        {
            Ok(()) => true,
            Err(message) => {
                if let UiMsg::Transfer(TransferMsg::Returned { tab, .. }) = message
                    && let Some(document) = tab.document().cloned()
                {
                    if let Some(lease) = lease {
                        document.release_edit(lease);
                    }
                    if document.subscribers.borrow().is_empty() {
                        self.shared.forget_parsed(document.id);
                    }
                }
                self.shared.docs_mut().collect_unused();
                true
            }
        }
    }

    fn reply_adopted(&self, origin: WindowId, token: TransferToken, merged: bool) {
        let _ = self.shared.post_window(origin, UiMsg::Transfer(TransferMsg::Adopted { token, merged }));
    }

    /// Drops a target's incoming drag highlight.
    fn clear_incoming(&mut self) -> bool {
        let changed = self.transfer.hover.take().is_some();
        self.drag.set_incoming(None);
        changed
    }

    /// Whether this window is closing, quitting, or already unregistered, so
    /// it must not take a tab (T-25's closing window refuses routing).
    fn is_closing_target(&self) -> bool {
        self.ui.close_requested
            || self.shared.is_quitting()
            || !self.shared.windows().get(self.id()).is_some_and(|entry| !entry.closing)
    }

    /// Reaps in-flight tabs whose target disappeared without answering.
    ///
    /// The tab went with the target's state; this only releases the source's
    /// subscriber entry and any lease that pointed at the lost tab, so the
    /// document's remaining views see the truth (the in-process equivalent of
    /// the original's 15 s timeout, without the timeout).
    pub(crate) fn reap_transfers(&mut self) -> bool {
        if self.transfer.pending.is_empty() {
            return false;
        }
        let targets: Vec<WindowId> = self.shared.windows().iter().map(|entry| entry.id).collect();
        let dead: Vec<TransferToken> = self
            .transfer
            .pending
            .iter()
            .filter(|(_, pending)| !targets.contains(&pending.target))
            .map(|(token, _)| *token)
            .collect();
        if dead.is_empty() {
            return false;
        }
        for token in dead {
            if let Some(pending) = self.transfer.pending.remove(&token)
                && let Some(document) = self.shared.docs().get(pending.doc)
            {
                if self.index_of_document(pending.doc).is_none() {
                    document.subscribers.borrow_mut().remove(&self.id());
                }
                document.release_edit(EditLease { window: self.id(), tab: pending.tab_id });
                if document.subscribers.borrow().is_empty() {
                    self.shared.forget_parsed(pending.doc);
                }
            }
        }
        self.shared.docs_mut().collect_unused();
        self.sync_registry();
        true
    }

    /// Window teardown: every in-flight tab loses this window's subscriber
    /// entry. The tab itself belongs to its target by now (or is in its inbox).
    pub(crate) fn forget_pending_views(&mut self) {
        let pending = std::mem::take(&mut self.transfer.pending);
        for pending in pending.values() {
            if let Some(document) = self.shared.docs().get(pending.doc) {
                if self.index_of_document(pending.doc).is_none() {
                    document.subscribers.borrow_mut().remove(&self.id());
                }
                document.release_edit(EditLease { window: self.id(), tab: pending.tab_id });
                if document.subscribers.borrow().is_empty() {
                    self.shared.forget_parsed(pending.doc);
                }
            }
        }
        self.transfer.hover = None;
        self.transfer.menu = None;
        // A torn tab leaving with this window must not leave a gap open in
        // another strip; the next move would clear it, but there is no next
        // move once this window is gone.
        self.clear_tear_hover();
    }
}
