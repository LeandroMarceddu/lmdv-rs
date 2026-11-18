//! Reading positions: capture, restore and the `--line` jump.
//!
//! A position is the nearest heading above the viewport (its GitHub id) plus
//! how far that heading sits above the viewport top, so inserting text above a
//! remembered heading does not move the reopened position (R-12, R-13). With no
//! heading above, the anchor is a raw scroll offset with an empty id, and an
//! at-top viewport forgets the entry (R-14). A remembered id that no longer
//! resolves opens at the top instead (R-15).
//!
//! Capture runs on the UI mailbox's clock: `poll` watches the scroll viewport's
//! reported offset and, 300 ms after the last change, writes the anchor to
//! `AppShared::positions` and queues one `StorePatch::Positions`. Nothing here
//! blocks the frame or touches the filesystem.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use lmdv_core::{DocStatus, DocumentId, ScrollAnchor};
use lmdv_md::MdDoc;
use lmdv_render::heading_key;
use rune::Tx;

use crate::reader::{ARTICLE_SCROLL_KEY, ScrollSignal, ScrollSnapshot, ViewportAnchor};
use crate::window::WindowState;

/// Idle time after the last scroll before the position is remembered (R-12).
pub const POSITION_IDLE_SECONDS: f64 = 0.3;
/// How long a `--line` landing stays highlighted.
pub const LINE_FLASH_SECONDS: f64 = 1.4;
/// How long a heading's landing band stays painted (A-15: 1.2 s).
pub const LANDING_FLASH_SECONDS: f64 = 1.2;
/// How long without any scroll report means the jump had nowhere to go.
pub const LANDING_START_GRACE_SECONDS: f64 = 0.2;
/// How still the glide must be before the heading lights up (three frames).
pub const LANDING_STILL_SECONDS: f64 = 0.05;
/// A viewport within half a point of the top counts as the top (R-14).
const TOP_EPSILON: f32 = 0.5;

/// One `--line` target: the source line and the byte range painted for it.
#[derive(Clone, Debug, PartialEq)]
pub struct LineFlash {
    /// The document the line belongs to.
    pub doc: DocumentId,
    /// Source line, 1-based.
    pub line: usize,
    /// Painted byte range (usually the line's text).
    pub range: Range<usize>,
    /// Virtual clock time at which the highlight expires.
    pub until: f64,
}

/// The heading a jump landed on, painted for a moment (O-10/A-15).
#[derive(Clone, Debug, PartialEq)]
pub struct LandingFlash {
    /// The document the heading belongs to.
    pub doc: DocumentId,
    /// Index into [`lmdv_md::MdDoc::headings`].
    pub heading: usize,
    /// The heading block's source range; the renderer maps it to the drawn text.
    pub range: Range<usize>,
    /// Virtual clock time at which the band expires.
    pub until: f64,
}

/// A heading jump holding the section line until its glide settles (O-09).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LandingWait {
    /// The document the heading belongs to.
    pub(crate) doc: DocumentId,
    /// Index into [`lmdv_md::MdDoc::headings`].
    pub(crate) heading: usize,
    /// The heading block's source range, when the model has one.
    pub(crate) range: Option<Range<usize>>,
    /// Clock time the jump was requested.
    started: f64,
    /// The scroll report count the jump armed at.
    last_report: u64,
    /// The last reported offset.
    last_offset: f32,
    /// Clock time of the last report that moved the page.
    last_change: f64,
    /// The page has reported a movement since the jump.
    moved: bool,
    /// The landing flash already started.
    flashed: bool,
}

/// Per-window reading state.
#[derive(Default)]
pub(crate) struct ReadingState {
    /// The reader viewport's latest report, shared with its `on_scroll_offset`.
    pub(crate) scroll: Rc<Cell<ScrollSnapshot>>,
    /// What caused the latest report, for the heading pin (O-04).
    pub(crate) scroll_signal: Rc<Cell<ScrollSignal>>,
    /// The last report `poll` observed; `None` means no frame has reported yet.
    pub(crate) last: Option<ScrollSnapshot>,
    /// Clock time of the last observed scroll change.
    idle_at: f64,
    /// The current idle period was already stored.
    pub(crate) settled: bool,
    /// A position waiting for the reader to exist (a load, a tab switch).
    pub(crate) pending: Option<ScrollAnchor>,
    /// A restore is in flight; at-top reports are not captured until it lands.
    pub(crate) restoring: bool,
    /// The nearest heading above the viewport, from the last drawn frame.
    pub(crate) viewport_anchor: Cell<Option<ViewportAnchor>>,
    /// Every drawn heading's top relative to the viewport top, from the last
    /// drawn frame; the Back/Forward jump rule measures against it (O-12).
    pub(crate) heading_tops: std::cell::RefCell<Vec<crate::section::MeasuredHeading>>,
    /// The top-level block at the viewport's top, from the last drawn frame;
    /// the reading anchor the keyboard's “Edit from here” uses (OI-098).
    pub(crate) reader_block: Cell<Option<usize>>,
    /// The active `--line` highlight, if any.
    pub(crate) line_flash: Option<LineFlash>,
    /// A heading jump waiting for its glide to settle (O-09/O-10).
    pub(crate) landing: Option<LandingWait>,
    /// The heading landing band, once it lights up (O-10/A-15).
    pub(crate) landing_flash: Option<LandingFlash>,
}

impl WindowState {
    /// The scroll cell the reader view writes into.
    pub(crate) fn scroll_cell(&self) -> Rc<Cell<ScrollSnapshot>> {
        self.reading.scroll.clone()
    }

    /// The report signal cell the reader view writes into (O-04).
    pub(crate) fn scroll_signal_cell(&self) -> Rc<Cell<ScrollSignal>> {
        self.reading.scroll_signal.clone()
    }

    /// This frame's scroll report signal.
    pub(crate) fn scroll_signal(&self) -> ScrollSignal {
        self.reading.scroll_signal.get()
    }

    /// The active tab's parsed model, when it has one.
    pub(crate) fn active_reader(&self) -> Option<Arc<MdDoc>> {
        self.active_tab().and_then(|tab| tab.view.reader.clone())
    }

    /// The scroll viewport's latest report, for the frame's anchor read.
    pub(crate) fn scroll_snapshot(&self) -> ScrollSnapshot {
        self.reading.scroll.get()
    }

    /// Records the nearest heading the frame just drew above the viewport.
    pub(crate) fn set_viewport_anchor(&self, anchor: ViewportAnchor) {
        self.reading.viewport_anchor.set(Some(anchor));
    }

    /// Records the drawn headings' tops for this frame (O-12).
    ///
    /// Only the reading view publishes them; a tab that is editing its preview
    /// left its reader, so the last measurement is dropped instead of measuring
    /// the preview's layout.
    pub(crate) fn set_heading_tops(&self, tops: Vec<crate::section::MeasuredHeading>) {
        *self.reading.heading_tops.borrow_mut() = tops;
    }

    /// The last drawn top of heading `index`, relative to the viewport top.
    pub(crate) fn heading_top(&self, index: usize) -> Option<f32> {
        self.reading.heading_tops.borrow().iter().find(|heading| heading.index == index).map(|heading| heading.top)
    }

    /// The heading id stored for `anchor`, or a raw-offset marker.
    ///
    /// The id is the heading's GitHub slug so an edit above it keeps pointing
    /// at the same section (R-13). A stale index-shaped id (`h7`) is honored
    /// when it names a heading; anything unknown is `None` (R-15).
    pub(crate) fn anchor_key(doc: &MdDoc, id: &str) -> Option<String> {
        if let Some(index) = id.strip_prefix('h').and_then(|digits| digits.parse::<usize>().ok())
            && index < doc.headings.len()
        {
            return Some(heading_key(index));
        }
        doc.resolve_heading(id).map(heading_key)
    }

    /// Schedules a restore of `anchor` once the active tab can scroll.
    ///
    /// Called on every tab switch, load completion and reopen. Every anchor is
    /// queued, the top included: the article's scroll viewport keeps its offset
    /// across tabs because its key is stable, so a top anchor has to move the
    /// viewport back instead of trusting the new tab to start there. Without
    /// it a short or medium page opens at the previous document's offset — the
    /// nearest heading above — and its progress hairline starts part way.
    pub(crate) fn begin_restore(&mut self, anchor: &ScrollAnchor) {
        // The previous tab's last offset must not look like this tab's first
        // report; the new viewport writes its own offset after this.
        self.reading.scroll.set(ScrollSnapshot::default());
        self.reading.scroll_signal.set(ScrollSignal::default());
        self.reading.last = None;
        self.reading.settled = false;
        self.reading.restoring = !anchor.is_top();
        self.reading.viewport_anchor.set(None);
        self.reading.heading_tops.borrow_mut().clear();
        self.reading.reader_block.set(None);
        self.reading.pending = Some(anchor.clone());
        // A new reading place: no key target, `g g` window or pinned heading
        // carries over from the tab or document that was in front (R-06–R-09).
        self.reading_keys.reset();
        // A heading jump of the tab that was in front is not this page's.
        self.reading.landing = None;
        self.reading.landing_flash = None;
        // A new tab in front or a reload: the section line's old path is gone.
        self.section_line.reset();
    }

    /// Issues the pending restore once the article is on screen.
    ///
    /// A position that names a heading waits for the model, because the heading
    /// key does not exist until the renderer has drawn it. An unknown id opens
    /// at the top instead (R-15).
    ///
    /// The old document's offset can be the first report the new tab makes
    /// (the scroll viewport keeps its state by key); that inherited report is
    /// not the reader moving and never cancels the restore. Only Rune's own
    /// hand verdict — a wheel, a drag, a key or an instant request — does.
    pub(crate) fn apply_pending_scroll(&mut self, tx: &mut Tx) {
        let Some(anchor) = self.reading.pending.clone() else { return };
        // The reader moved before the restore could land (the file was still
        // parsing, or the user scrolled on the first frame). Never override a
        // position the user chose: drop the pending restore instead of
        // snapping the article back.
        if self.scroll_signal().by_hand {
            self.reading.pending = None;
            self.reading.restoring = false;
            return;
        }
        let ready = self.active_tab().is_some_and(|tab| {
            tab.document().is_some_and(|doc| matches!(*doc.status.borrow(), DocStatus::Ready))
                && tab.view.reader.is_some()
        });
        if !ready {
            return;
        }
        self.reading.pending = None;
        if anchor.is_top() {
            self.reading.restoring = false;
            // R-14: always move the viewport to the top. A fresh viewport does
            // not necessarily start there when its key was the last document's.
            tx.scroll_to_y(ARTICLE_SCROLL_KEY, 0.0);
            return;
        }
        if anchor.id.is_empty() {
            tx.scroll_to_y(ARTICLE_SCROLL_KEY, anchor.offset);
            return;
        }
        let key = self
            .active_tab()
            .and_then(|tab| tab.view.reader.clone())
            .and_then(|doc| Self::anchor_key(&doc, &anchor.id));
        match key {
            Some(key) => tx.scroll_key_to(key, -anchor.offset),
            // R-15: a remembered heading that is gone opens at the top.
            None => tx.scroll_to_y(ARTICLE_SCROLL_KEY, 0.0),
        }
    }

    /// Stores the active tab's current position in memory and, for a file, in
    /// the position table. Safe to call on tab switch, close and quit.
    pub(crate) fn capture_active_position(&mut self) {
        // The preview's scroll is not a reading position (E-07).
        if self.active_tab().is_some_and(|tab| tab.view.mode == crate::window::Mode::Editor) {
            return;
        }
        // The cell is newer than the last `poll` observation; `last` only says
        // that this tab has reported at least once since the last switch.
        if self.reading.last.is_none() {
            return;
        }
        let snapshot = self.reading.scroll.get();
        if self.reading.restoring {
            if snapshot.is_top() {
                return;
            }
            self.reading.restoring = false;
        }
        let Some(tab) = self.active_tab() else { return };
        let Some(doc) = tab.view.reader.clone() else { return };
        let anchor = anchor_for(&doc, snapshot, self.reading.viewport_anchor.get());
        if tab.view.anchor == anchor {
            return;
        }
        if let Some(tab) = self.active_tab_mut() {
            tab.view.anchor = anchor.clone();
        }
        let path = self.active_tab().and_then(|tab| tab.document()).and_then(|doc| {
            let key = doc.key.borrow();
            key.path().map(std::path::Path::to_path_buf)
        });
        if let Some(path) = path {
            self.shared.remember_position(&path, &anchor);
        }
    }

    /// Recomputes the active tab's reading time when its model changed (R-04).
    ///
    /// Only the reading view keeps it up to date: the editor's preview reparses on
    /// every pause, and the inline time is hidden while editing anyway (R-05).
    /// A document that opens straight into edit mode (or a session's restored
    /// editor) still gets its first count, which the hover card keeps; leaving
    /// the editor asks for the edited model's count.
    pub(crate) fn refresh_reading_stats(&mut self) -> bool {
        let Some(tab) = self.active_tab() else { return false };
        if tab.view.mode == crate::window::Mode::Editor && tab.view.reading_revision.is_some() {
            return false;
        }
        let (Some(model), Some(revision)) = (tab.view.reader.clone(), tab.view.reader_revision) else {
            return false;
        };
        if tab.view.reading_revision == Some(revision) {
            return false;
        }
        let stats = crate::reading_meta::reading_stats(&model);
        let Some(tab) = self.active_tab_mut() else { return false };
        tab.view.reading = Some(stats);
        tab.view.reading_revision = Some(revision);
        true
    }

    /// Watches the scroll viewport and stores the anchor after 300 ms of rest.
    ///
    /// The first report after a switch starts a new idle period; the timer only
    /// asks for one wake, so an idle window draws nothing.
    pub(crate) fn poll_reading(&mut self, now: f64, tx: &mut Tx) -> bool {
        if self.active_tab().is_none_or(|tab| tab.view.reader.is_none() || tab.view.mode == crate::window::Mode::Editor)
        {
            return false;
        }
        let snapshot = self.reading.scroll.get();
        match self.reading.last {
            Some(previous) if previous == snapshot => {}
            _ => {
                self.reading.last = Some(snapshot);
                self.reading.idle_at = now;
                self.reading.settled = false;
                if self.reading.restoring && !snapshot.is_top() {
                    self.reading.restoring = false;
                }
                tx.wake_at(now + POSITION_IDLE_SECONDS);
                // The section line waits 120 ms after the last fast frame (O-09);
                // this frame lets it appear once the page has stopped.
                tx.wake_at(now + crate::section::SECTION_SETTLE_SECONDS + 0.02);
                return false;
            }
        }
        if self.reading.settled {
            return false;
        }
        if self.reading.restoring && snapshot.is_top() {
            return false;
        }
        if now >= self.reading.idle_at + POSITION_IDLE_SECONDS {
            self.reading.settled = true;
            self.capture_active_position();
        }
        false
    }

    /// Consumes the active tab's `--line` target: lands on its block.
    ///
    /// `line_starts[line - 1]` maps the requested line to its first byte, and
    /// `MdDoc::block_index_at` to the top-level block that owns it. The paint
    /// range is kept in [`LineFlash`] for the find/`--line` highlight work
    /// (OI-043): the renderer has no paint-only highlight API yet.
    pub(crate) fn apply_line_target(&mut self, now: f64, tx: &mut Tx) -> bool {
        let Some(tab) = self.active_tab() else { return false };
        let Some(requested) = tab.view.line else { return false };
        let Some(reader) = tab.view.reader.clone() else { return false };
        let doc = tab.document_id();
        let line = requested.max(1).min(reader.line_count().max(1));
        let key = crate::reader::block_key_at_line(&reader, line);
        let range = crate::reader::line_range(&reader, line).unwrap_or(0..0);
        if let Some(tab) = self.active_tab_mut() {
            tab.view.line = None;
        }
        if let Some(key) = key {
            tx.scroll_key_to(key, 24.0);
        }
        if let Some(doc) = doc {
            self.reading.line_flash = Some(LineFlash { doc, line, range, until: now + LINE_FLASH_SECONDS });
            tx.wake_at(now + LINE_FLASH_SECONDS);
        }
        true
    }

    /// Clears an expired `--line` highlight. True when the view changed.
    pub(crate) fn expire_line_flash(&mut self, now: f64) -> bool {
        let expired = self.reading.line_flash.as_ref().is_some_and(|flash| now >= flash.until);
        if expired {
            self.reading.line_flash = None;
        }
        expired
    }

    /// Arms a heading jump: the section line stays away and the heading lights
    /// up only once the glide has settled (O-09/O-10, A-15).
    ///
    /// Called by `WindowState::scroll_to_heading` as it issues the scroll
    /// request. The heading block's source range is what the renderer paints; a
    /// heading without a rendered block (a footnote definition) has none and
    /// only holds the line. Returns whether a wait was armed: a jump without a
    /// document has nothing to wait for.
    pub(crate) fn expect_landing(&mut self, heading: usize, now: f64) -> bool {
        let Some(doc) = self.active_tab().and_then(|tab| tab.view.reader.clone()) else { return false };
        let Some(document) = self.active_document_id() else { return false };
        let range = doc
            .headings
            .get(heading)
            .and_then(|heading| doc.block_spans.get(heading.block))
            .map(|span| span.source.clone());
        let scroll = self.reading.scroll.get();
        self.reading.landing = Some(LandingWait {
            doc: document,
            heading,
            range,
            started: now,
            last_report: self.scroll_signal().report,
            last_offset: scroll.offset,
            last_change: now,
            moved: false,
            flashed: false,
        });
        self.reading.landing_flash = None;
        true
    }

    /// The reader took the page over while a heading jump was landing: forget
    /// the flash and let the line return after its settle (O-04/OI-142).
    ///
    /// A key, a page or a taken wheel is the reader's own motion, not the
    /// jump's glide.
    pub(crate) fn cancel_landing(&mut self, now: f64, tx: &mut Tx) {
        if self.reading.landing.is_none() {
            return;
        }
        self.reading.landing = None;
        self.reading.landing_flash = None;
        self.section_line.release_hold();
        tx.wake_at(now + crate::section::SECTION_SETTLE_SECONDS + 0.02);
    }

    /// Watches a heading jump's glide and lights the heading once it lands.
    ///
    /// The flash starts when the page has been still for
    /// [`LANDING_STILL_SECONDS`] (the original's three still frames), or at
    /// once for an instant jump such as reduced motion's (A-15). The section
    /// line is released at the same threshold the outline's pin uses
    /// ([`crate::reading_keys::PIN_IDLE_SECONDS`]) after the last movement, so
    /// it stays away through a slow glide instead of a fixed 250 ms. A jump
    /// with nowhere to go settles after [`LANDING_START_GRACE_SECONDS`].
    /// True when the view changed (a flash appeared, ended or the line woke).
    pub(crate) fn poll_landing(&mut self, now: f64, tx: &mut Tx) -> bool {
        let mut changed = self.expire_landing_flash(now);
        let Some(wait) = self.reading.landing.clone() else { return changed };
        let mut wait = wait;
        let signal = self.scroll_signal();
        let offset = self.reading.scroll.get().offset;
        let mut instant = false;
        if signal.report != wait.last_report {
            let was_moved = wait.moved;
            let delta = (offset - wait.last_offset).abs();
            wait.last_report = signal.report;
            if delta > 0.5 {
                wait.moved = true;
                wait.last_offset = offset;
                wait.last_change = now;
            }
            // A wheel or a drag that moved nothing, and an instant request's
            // first report (reduced motion, a jump with no distance to glide),
            // mean the page is already where it is going.
            if signal.by_hand && (!was_moved || delta <= 0.5) {
                instant = true;
            }
        }
        let flash_due = instant
            || (!wait.moved && now >= wait.started + LANDING_START_GRACE_SECONDS)
            || (wait.moved && now >= wait.last_change + LANDING_STILL_SECONDS);
        let hold_due = instant
            || (!wait.moved && now >= wait.started + LANDING_START_GRACE_SECONDS)
            || (wait.moved && now >= wait.last_change + crate::section::SECTION_GLIDE_HOLD_SECONDS);
        if flash_due
            && !wait.flashed
            && let Some(range) = wait.range.clone().filter(|range| !range.is_empty())
        {
            wait.flashed = true;
            self.reading.landing_flash =
                Some(LandingFlash { doc: wait.doc, heading: wait.heading, range, until: now + LANDING_FLASH_SECONDS });
            tx.wake_at(now + LANDING_FLASH_SECONDS);
            changed = true;
        }
        if hold_due {
            self.section_line.release_hold();
            self.reading.landing = None;
            // The line may now appear after its own 120 ms settle (O-09).
            tx.wake_at(now + crate::section::SECTION_SETTLE_SECONDS + 0.02);
            changed = true;
        } else {
            let flash_at = if wait.moved {
                wait.last_change + LANDING_STILL_SECONDS
            } else {
                wait.started + LANDING_START_GRACE_SECONDS
            };
            let hold_at = if wait.moved {
                wait.last_change + crate::section::SECTION_GLIDE_HOLD_SECONDS
            } else {
                wait.started + LANDING_START_GRACE_SECONDS
            };
            if !wait.flashed {
                tx.wake_at(flash_at.max(now) + 0.001);
            }
            tx.wake_at(hold_at.max(now) + 0.001);
            self.reading.landing = Some(wait);
        }
        changed
    }

    /// Clears an expired landing band. True when the view changed.
    pub(crate) fn expire_landing_flash(&mut self, now: f64) -> bool {
        let expired = self.reading.landing_flash.as_ref().is_some_and(|flash| now >= flash.until);
        if expired {
            self.reading.landing_flash = None;
        }
        expired
    }
}

/// The stored anchor for one viewport report.
///
/// At the top the anchor is empty (R-14); below it, the nearest heading above
/// the viewport becomes the anchor id and the distance becomes its offset
/// (R-12). Without a heading the report is a raw offset, which older position
/// files already contain.
pub(crate) fn anchor_for(doc: &MdDoc, snapshot: ScrollSnapshot, viewport: Option<ViewportAnchor>) -> ScrollAnchor {
    if snapshot.offset <= TOP_EPSILON {
        return ScrollAnchor::top();
    }
    if let Some(viewport) = viewport.filter(|viewport| !viewport.top)
        && let Some(heading) = viewport.heading.and_then(|index| doc.headings.get(index))
        && !heading.id.is_empty()
    {
        return ScrollAnchor { id: heading.id.clone(), offset: viewport.offset.max(0.0) };
    }
    ScrollAnchor { id: String::new(), offset: snapshot.offset }
}

#[cfg(test)]
mod tests {
    use lmdv_core::{DocumentKey, ScrollAnchor};
    use lmdv_io::store::PositionTable;
    use rune::Tx;
    use std::path::PathBuf;
    use tempfile::TempDir;

    use super::*;
    use crate::reader::ScrollSnapshot;
    use crate::test_support;

    fn window(temp: &TempDir) -> WindowState {
        WindowState::new(test_support::shared(temp))
    }

    #[test]
    fn a_resting_viewport_is_remembered_after_three_hundred_ms() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let tab = state.open_synthetic("Note", "# Note\n\nBody\n");
        state.reading.scroll.set(ScrollSnapshot { offset: 240.0, max: 1200.0, viewport: 700.0 });

        let mut tx = Tx::default();
        state.poll_reading(0.0, &mut tx);
        state.poll_reading(0.2, &mut tx);
        assert_eq!(state.tab(tab).map(|tab| tab.view.anchor.offset), Some(0.0), "not yet idle");

        state.poll_reading(0.31, &mut tx);
        assert_eq!(state.tab(tab).map(|tab| tab.view.anchor.offset), Some(240.0));
    }

    #[test]
    fn at_top_forgets_the_anchor() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let tab = state.open_synthetic("Note", "# Note\n");
        state.reading.scroll.set(ScrollSnapshot { offset: 180.0, ..ScrollSnapshot::default() });
        let mut tx = Tx::default();
        state.poll_reading(0.0, &mut tx);
        state.poll_reading(1.0, &mut tx);
        state.poll_reading(2.0, &mut tx);
        assert!(!state.tab(tab).map(|tab| tab.view.anchor.is_top()).unwrap_or(false));

        state.reading.scroll.set(ScrollSnapshot::default());
        state.poll_reading(3.0, &mut tx);
        state.poll_reading(4.0, &mut tx);
        assert!(state.tab(tab).map(|tab| tab.view.anchor.is_top()).unwrap_or(false));
    }

    #[test]
    fn a_file_position_is_queued_in_the_shared_table() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        let path = PathBuf::from("/tmp/lmdv-app-position/note.md");
        let key = DocumentKey::File(path.clone());
        let id = shared.docs_mut().allocate_id();
        let doc =
            std::rc::Rc::new(lmdv_core::Document::ready(id, key, "note.md", lmdv_core::SourceBuffer::new("# Note\n")));
        shared.docs_mut().insert_ready(doc.clone());
        shared.store_parsed(id, 0, Ok(std::sync::Arc::new(lmdv_md::parse("# Note\n"))));
        let tab = state.attach_document(doc);

        state.reading.scroll.set(ScrollSnapshot { offset: 96.0, ..ScrollSnapshot::default() });
        let mut tx = Tx::default();
        state.poll_reading(0.0, &mut tx);
        state.poll_reading(0.5, &mut tx);
        assert_eq!(state.tab(tab).map(|tab| tab.view.anchor.offset), Some(96.0));

        let positions: PositionTable = shared.positions().clone();
        assert_eq!(positions.get(&path).map(|anchor| anchor.offset), Some(96.0));
    }

    #[test]
    fn an_opened_file_starts_from_its_remembered_position() {
        let temp = TempDir::new().unwrap();
        let shared = test_support::shared(&temp);
        let mut state = WindowState::new(shared.clone());
        let path = PathBuf::from("/tmp/lmdv-app-position/restored.md");
        let id = shared.docs_mut().allocate_id();
        let doc = std::rc::Rc::new(lmdv_core::Document::ready(
            id,
            DocumentKey::File(path.clone()),
            "restored.md",
            lmdv_core::SourceBuffer::new("# Restored\n\nBody\n"),
        ));
        shared.docs_mut().insert_ready(doc.clone());
        shared.store_parsed(id, 0, Ok(std::sync::Arc::new(lmdv_md::parse("# Restored\n\nBody\n"))));
        shared.remember_position(&path, &ScrollAnchor { id: String::new(), offset: 120.0 });

        let tab = state.attach_document(doc);
        assert_eq!(state.tab(tab).map(|tab| tab.view.anchor.offset), Some(120.0));
        assert_eq!(state.reading.pending.as_ref().map(|anchor| anchor.offset), Some(120.0));
    }

    #[test]
    fn a_line_target_lands_on_its_block_and_flashes() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let tab = state.open_synthetic("Long", "# One\n\nfirst\n\n## Two\n\nsecond\n");
        state.set_pending_line(7);
        let mut tx = Tx::default();
        assert!(state.apply_line_target(2.0, &mut tx));
        assert_eq!(state.tab(tab).and_then(|tab| tab.view.line), None, "the target is consumed");
        let flash = state.reading.line_flash.clone().expect("the line is painted");
        assert_eq!(flash.line, 7);
        assert_eq!(&"# One\n\nfirst\n\n## Two\n\nsecond\n"[flash.range], "second");
        assert_eq!(flash.doc, state.active_document_id().expect("a document"));
        assert!(flash.until > 2.0);

        assert!(state.expire_line_flash(flash.until + 0.1));
        assert!(state.reading.line_flash.is_none());
    }

    #[test]
    fn a_heading_jump_flashes_after_the_glide_settles_not_before() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "# One\n\nBody\n\n## Two\n\nMore\n");
        let mut tx = Tx::default();
        let doc = state.active_reader().expect("a model");
        let target = doc.headings.iter().position(|heading| heading.text == "Two").expect("the h2");
        state.scroll_to_heading(&lmdv_render::heading_key(target), &mut tx);
        assert!(state.reading.landing.is_some(), "the jump waits for its glide");
        assert!(state.reading.landing_flash.is_none(), "nothing is painted before it lands");

        // The glide reports moving frames.
        state.reading.scroll.set(ScrollSnapshot { offset: 40.0, max: 1000.0, viewport: 700.0 });
        state.reading.scroll_signal.set(ScrollSignal { report: 1, by_hand: false });
        assert!(!state.poll_landing(0.02, &mut tx));
        assert!(state.reading.landing_flash.is_none());
        state.reading.scroll.set(ScrollSnapshot { offset: 120.0, max: 1000.0, viewport: 700.0 });
        state.reading.scroll_signal.set(ScrollSignal { report: 2, by_hand: false });
        assert!(!state.poll_landing(0.10, &mut tx));

        // Three still frames later (50 ms) the heading lights up.
        assert!(!state.poll_landing(0.13, &mut tx), "still inside the still window");
        assert!(state.poll_landing(0.16, &mut tx));
        let flash = state.reading.landing_flash.clone().expect("the landing band");
        assert_eq!(flash.heading, target);
        assert!((flash.until - (0.16 + LANDING_FLASH_SECONDS)).abs() < 0.001);
        assert!(state.reading.landing.is_some(), "the line's hold is still on");

        // The section line's hold ends after the outline pin's settle, not 50 ms.
        assert!(!state.poll_landing(0.30, &mut tx), "the hold still runs");
        assert!(state.poll_landing(0.36, &mut tx), "the glide has landed");
        assert!(state.reading.landing.is_none());
        assert!(state.reading.landing_flash.is_some(), "the band outlives the hold");

        // 1.2 s after it lit up, the band clears.
        assert!(!state.poll_landing(flash.until - 0.01, &mut tx));
        assert!(state.poll_landing(flash.until + 0.01, &mut tx));
        assert!(state.reading.landing_flash.is_none());
    }

    #[test]
    fn an_instant_jump_lights_up_at_once() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "# One\n\nBody\n\n## Two\n\nMore\n");
        let mut tx = Tx::default();
        let doc = state.active_reader().expect("a model");
        let target = doc.headings.iter().position(|heading| heading.text == "Two").expect("the h2");
        state.scroll_to_heading(&lmdv_render::heading_key(target), &mut tx);
        // Reduced motion's cut: one by-hand report, the whole distance.
        state.reading.scroll.set(ScrollSnapshot { offset: 480.0, max: 1000.0, viewport: 700.0 });
        state.reading.scroll_signal.set(ScrollSignal { report: 1, by_hand: true });
        assert!(state.poll_landing(0.016, &mut tx));
        assert!(state.reading.landing_flash.is_some(), "a reduced-motion jump is already home");
        assert!(state.reading.landing.is_none(), "and the line may come back");
    }

    #[test]
    fn a_jump_with_nowhere_to_go_settles_after_its_grace() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "# One\n\nBody\n\n## Two\n\nMore\n");
        let mut tx = Tx::default();
        let doc = state.active_reader().expect("a model");
        let target = doc.headings.iter().position(|heading| heading.text == "Two").expect("the h2");
        state.scroll_to_heading(&lmdv_render::heading_key(target), &mut tx);
        assert!(!state.poll_landing(0.05, &mut tx), "no report yet");
        assert!(state.poll_landing(0.25, &mut tx), "the start grace released it");
        assert!(state.reading.landing_flash.is_some());
        assert!(state.reading.landing.is_none());
    }

    #[test]
    fn restore_requests_clamp_to_a_ready_document() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "# Note\n\nBody\n");
        let anchor = ScrollAnchor { id: String::new(), offset: 320.0 };
        state.begin_restore(&anchor);
        assert!(state.reading.pending.is_some());
        state.apply_pending_scroll(&mut Tx::default());
        assert!(state.reading.pending.is_none(), "the active tab is a ready synthetic document");
    }

    #[test]
    fn a_position_below_a_heading_stores_the_heading_id() {
        let doc = lmdv_md::parse("# Top\n\nText\n\n## Second\n\nMore\n");
        let viewport = ViewportAnchor { heading: Some(1), offset: 64.0, top: false };
        let anchor = anchor_for(&doc, ScrollSnapshot { offset: 500.0, ..ScrollSnapshot::default() }, Some(viewport));
        assert_eq!(anchor.id, doc.headings[1].id, "the GitHub slug, not the index");
        assert_eq!(anchor.offset, 64.0);
        assert!(!anchor.is_top());

        let raw = anchor_for(&doc, ScrollSnapshot { offset: 500.0, ..ScrollSnapshot::default() }, None);
        assert_eq!(raw, ScrollAnchor { id: String::new(), offset: 500.0 }, "no heading above is a raw offset");

        let top = anchor_for(&doc, ScrollSnapshot::default(), Some(viewport));
        assert!(top.is_top(), "the top forgets the entry (R-14)");
    }

    #[test]
    fn a_remembered_heading_id_resolves_to_its_rendered_key() {
        let doc = lmdv_md::parse("# Top\n\n## Second\n\n### Third\n");
        let key = WindowState::anchor_key(&doc, &doc.headings[2].id).expect("the slug resolves");
        assert_eq!(key, "h2");
        assert_eq!(WindowState::anchor_key(&doc, "h1").as_deref(), Some("h1"), "index ids still work");
        assert_eq!(WindowState::anchor_key(&doc, "gone"), None, "R-15 falls back to the top");
    }

    #[test]
    fn a_heading_anchor_waits_for_the_model_then_restores() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "# Top\n\n## Second\n\nMore\n");
        let doc = state.active_reader().expect("parsed");
        let anchor = ScrollAnchor { id: doc.headings[1].id.clone(), offset: 40.0 };
        state.begin_restore(&anchor);
        assert!(state.reading.pending.is_some());
        state.apply_pending_scroll(&mut Tx::default());
        assert!(state.reading.pending.is_none(), "the heading model was on screen");
    }

    #[test]
    fn a_scrolled_viewport_cancels_a_pending_restore() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "# Top\n\n## Second\n\nMore\n");
        let doc = state.active_reader().expect("parsed");
        let anchor = ScrollAnchor { id: doc.headings[1].id.clone(), offset: 40.0 };
        state.begin_restore(&anchor);
        // The user (or a wheel) moved before the model was ready. Rune's report
        // says a hand moved it; that is what cancels the restore.
        state.reading.scroll.set(ScrollSnapshot { offset: 240.0, max: 1200.0, viewport: 700.0 });
        state.reading.scroll_signal.set(ScrollSignal { report: 1, by_hand: true });
        state.apply_pending_scroll(&mut Tx::default());
        assert!(state.reading.pending.is_none(), "a user scroll wins over the old place");
        assert!(!state.reading.restoring);
    }

    #[test]
    fn an_inherited_offset_does_not_cancel_a_pending_restore() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "# Top\n\n## Second\n\nMore\n");
        let doc = state.active_reader().expect("parsed");
        let anchor = ScrollAnchor { id: doc.headings[1].id.clone(), offset: 40.0 };
        state.begin_restore(&anchor);
        // The new tab's first report can be the previous document's offset:
        // the article viewport keeps its state by key. That is layout, not a
        // hand, so the restore must still land.
        state.reading.scroll.set(ScrollSnapshot { offset: 240.0, max: 1200.0, viewport: 700.0 });
        state.reading.scroll_signal.set(ScrollSignal { report: 1, by_hand: false });
        state.apply_pending_scroll(&mut Tx::default());
        assert!(state.reading.pending.is_none(), "the inherited report is not a user scroll");
    }

    #[test]
    fn a_top_restore_is_a_request_not_a_trusted_fresh_viewport() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "# Top\n\nBody\n");
        state.begin_restore(&ScrollAnchor::top());
        // A tab without a remembered place still has to reset a viewport that
        // kept the last document's offset.
        state.reading.scroll.set(ScrollSnapshot { offset: 240.0, max: 1200.0, viewport: 700.0 });
        state.apply_pending_scroll(&mut Tx::default());
        assert!(state.reading.pending.is_none(), "the top is applied as soon as the reader is ready");
        assert!(!state.reading.restoring);
    }
}
