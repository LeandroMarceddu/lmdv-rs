//! The section line (O-07–O-10) and the reading progress hairline (R-03).
//!
//! The hairline is a 2-point accent line at the very top of the reading pane:
//! its width is `offset / max`, monotonic, hidden while the document has no more
//! than 24 points of scroll, and it simply is not built in edit mode, on the
//! start page or over an error card. It reads the reader's own scroll report
//! ([`crate::reading::ReadingState::scroll`]); no timer is involved.
//!
//! The section line names the heading being read and the ones it sits under,
//! held at the top of the article while the outline is out of sight. It is
//! `aria-hidden` and pointer-only. Its path comes from the outline's headings
//! (h1–h4, rendered blocks only, the single first h1's title omitted) with the
//! parent being the nearest earlier heading with a smaller level (O-08). Clicking
//! a part goes where an outline row goes: the same scroll request and outline
//! reveal, then the 250 ms hold that keeps the line away while the page lands
//! (O-10).
//!
//! Suppression (O-09): no line while a heading reaches into the top 56 points,
//! while a table header holds the top, while the page runs faster than 2.4
//! points a millisecond, for 120 ms after the last fast frame, or for 250 ms
//! after an outline jump. A new path waits for the page to settle before it is
//! shown. Two rules of the original are approximated and reported as open
//! items: the keyboard's focus ring under the line (Rune has no focus query for
//! the view) and a table whose header is only *about* to stick.
//!
//! Geometry is measured once a frame from the window's last layout: every drawn
//! h1–h6 heading's top and bottom relative to the article viewport, and every
//! top-level table block's. Scrolling re-runs the reader view anyway (its anchor
//! reads the headings), so the line moves with the page without a new timer.

use std::cell::{Cell, RefCell};

use lmdv_md::{Block, MdDoc};
use lmdv_render::heading_key;
use rune::{Cx, El, TextStyle, Tx, el, row, text};
use rune_kit::Kit;

use crate::reader::{ARTICLE_SCROLL_KEY, READER_MAX_WIDTH, ScrollSnapshot, column_padding, has_toc_headings};
use crate::window::WindowState;

/// The section line's root key.
pub const SECTION_LINE_KEY: &str = "section-line";
/// The line's path row.
pub const SECTION_LINE_PATH_KEY: &str = "section-line-path";
/// A part's key: `section-seg-0`, outermost first.
pub const SECTION_SEGMENT_PREFIX: &str = "section-seg-";
/// The invisible 500 ms guard that swallows a double click's second press.
pub const SECTION_SHIELD_KEY: &str = "section-line-shield";
/// The parts' separator (O-08: `A › B › C`).
pub const SECTION_SEPARATOR: char = '\u{203a}';
/// The line's height in points (`--section-line-h`).
pub const SECTION_LINE_HEIGHT: f32 = 30.0;
/// A heading reaching this far into the viewport's top owns it.
pub const SECTION_LINE_BAND: f32 = 56.0;
/// A heading whose bottom is no lower than this has gone.
pub const SECTION_LINE_GONE: f32 = 4.0;
/// How far a focus ring reaches outside its element (the original's 2 px
/// outline at a 2 px offset).
pub const SECTION_FOCUS_RING: f32 = 4.0;
/// Faster than this, in points per millisecond, the page is flying by.
pub const SECTION_FAST_SCROLL: f32 = 2.4;
/// How long the page must be slow before a new path shows.
pub const SECTION_SETTLE_SECONDS: f64 = 0.120;
/// How long after an outline jump the line stays away.
pub const SECTION_GLIDE_HOLD_SECONDS: f64 = 0.250;
/// How long a click's double click second press is swallowed.
pub const SECTION_DOUBLE_SECONDS: f64 = 0.500;
/// The hairline's key.
pub const PROGRESS_KEY: &str = "read-progress";
/// The hairline's filled part.
pub const PROGRESS_FILL_KEY: &str = "read-progress-fill";
/// Documents with no more scroll than this hide the hairline (R-03).
pub const PROGRESS_MIN_SCROLL: f32 = 24.0;
/// Heading measures per frame, as the outline's scroll spy caps them.
const MEASURE_MAX: usize = 500;

/// One part of the path as the frame draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionSegment {
    /// Index into [`MdDoc::headings`].
    pub heading: usize,
    /// Its name, image alt text or “§” (O-08).
    pub text: String,
}

/// One measured heading, relative to the article viewport's top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeasuredHeading {
    /// Index into [`MdDoc::headings`].
    pub index: usize,
    /// Its top edge below the viewport's top (negative when scrolled past).
    pub top: f32,
    /// Its bottom edge likewise.
    pub bottom: f32,
}

/// One measured top-level table block, relative to the article viewport's top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeasuredTable {
    /// Its top edge below the viewport's top.
    pub top: f32,
    /// Its bottom edge likewise.
    pub bottom: f32,
    /// The header row's height, when the frame drew it (0.0 when unmeasured).
    /// A heading jump that lands under a stuck header adds it (L-01).
    pub header: f32,
}

/// The frame's section line: its parts, outermost first; empty when hidden.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SectionLineFrame {
    /// The path the frame draws.
    pub segments: Vec<SectionSegment>,
}

impl SectionLineFrame {
    /// Whether the frame draws the line.
    pub fn visible(&self) -> bool {
        !self.segments.is_empty()
    }
}

/// The line's timers, the path it shows and the path waiting to show.
///
/// Views cannot ask for a frame, so the state is written through `Cell`s from
/// the view and read by `poll`, which schedules the wakes that let a settled
/// path appear (see [`crate::reading::POSITION_IDLE_SECONDS`] and
/// [`WindowState::jump_to_heading`]).
#[derive(Debug, Default)]
pub struct SectionLineState {
    /// The last scroll offset observed and when, for the page's speed.
    last: Cell<Option<(f32, f64)>>,
    /// Hidden until this clock time: a fast page (O-09).
    fast_until: Cell<f64>,
    /// Hidden until this clock time: an outline jump's hold (O-09).
    hold_until: Cell<f64>,
    /// Hidden until a heading jump's glide settles (O-09/OI-142).
    hold_open: Cell<bool>,
    /// The path waiting for the page to settle, and when it was first wanted.
    pending: RefCell<Option<Vec<usize>>>,
    pending_at: Cell<f64>,
    /// The path on screen.
    shown: RefCell<Option<Vec<usize>>>,
    /// What the last frame drew.
    frame: RefCell<SectionLineFrame>,
    /// A click's second press is swallowed until this clock time (O-10).
    swallow_until: Cell<f64>,
    /// The last viewer frame's measured tables, for a jump's sticky-header
    /// cover (L-01). Cleared by [`Self::reset`] and every non-viewer frame.
    tables: RefCell<Vec<MeasuredTable>>,
}

impl SectionLineState {
    /// Forgets everything: a tab switch, an edit, a session change.
    pub fn reset(&self) {
        self.last.set(None);
        self.fast_until.set(0.0);
        self.hold_until.set(0.0);
        self.hold_open.set(false);
        *self.pending.borrow_mut() = None;
        self.pending_at.set(0.0);
        *self.shown.borrow_mut() = None;
        *self.frame.borrow_mut() = SectionLineFrame::default();
        self.tables.borrow_mut().clear();
    }

    /// Records the viewer frame's measured tables (L-01).
    pub fn set_tables(&self, tables: Vec<MeasuredTable>) {
        *self.tables.borrow_mut() = tables;
    }

    /// The last viewer frame's measured tables.
    pub fn tables(&self) -> Vec<MeasuredTable> {
        self.tables.borrow().clone()
    }

    /// Records a scroll report and the page's speed (O-09).
    pub fn observe_scroll(&self, scroll: ScrollSnapshot, now: f64) {
        if let Some((offset, at)) = self.last.get() {
            let seconds = now - at;
            if seconds > 0.0 {
                // Points a millisecond, as O-09's 2.4 is written.
                let speed = (scroll.offset - offset).abs() / (seconds as f32 * 1000.0);
                if speed > SECTION_FAST_SCROLL {
                    self.fast_until.set(now + SECTION_SETTLE_SECONDS);
                }
            }
        }
        self.last.set(Some((scroll.offset, now)));
    }

    /// The line steps aside as a heading jump starts (O-09).
    ///
    /// The hold is open-ended: [`Self::release_hold`] ends it when the glide
    /// actually settles (OI-142), not after a fixed 250 ms.
    pub fn hold_until_landing(&self) {
        self.hold_open.set(true);
        *self.pending.borrow_mut() = None;
        *self.shown.borrow_mut() = None;
    }

    /// Whether a heading jump's hold is still open (O-09/OI-142).
    pub fn holding(&self) -> bool {
        self.hold_open.get()
    }

    /// Ends a heading jump's hold: the line may come back after its settle.
    pub fn release_hold(&self) {
        self.hold_open.set(false);
        self.hold_until.set(0.0);
        *self.pending.borrow_mut() = None;
    }

    /// The line hides at once, without a fade: editing, a tab leaving the front.
    pub fn clear(&self) {
        *self.pending.borrow_mut() = None;
        *self.shown.borrow_mut() = None;
        *self.frame.borrow_mut() = SectionLineFrame::default();
        self.last.set(None);
        self.fast_until.set(0.0);
        self.hold_until.set(0.0);
        self.hold_open.set(false);
    }

    /// The path to draw now, or `None` (O-07/O-09).
    pub fn settle(&self, want: Option<Vec<usize>>, now: f64) -> Option<Vec<usize>> {
        // A heading jump's glide owns the top until it lands; the landing poll
        // releases it and wakes the view (OI-142), so no timer is armed here.
        if self.hold_open.get() {
            *self.pending.borrow_mut() = None;
            *self.shown.borrow_mut() = None;
            return None;
        }
        let suppress = self.fast_until.get().max(self.hold_until.get());
        if now < suppress {
            // The page is moving fast or a jump is landing: the path waits, and
            // its settle deadline is the suppression's end (not 120 ms beyond it).
            *self.pending.borrow_mut() = want;
            self.pending_at.set(suppress - SECTION_SETTLE_SECONDS);
            *self.shown.borrow_mut() = None;
            return None;
        }
        let Some(want) = want else {
            *self.pending.borrow_mut() = None;
            *self.shown.borrow_mut() = None;
            return None;
        };
        if self.shown.borrow().as_ref() == Some(&want) {
            *self.pending.borrow_mut() = None;
            return Some(want);
        }
        if self.pending.borrow().as_ref() != Some(&want) {
            *self.pending.borrow_mut() = Some(want.clone());
            self.pending_at.set(now);
        }
        if now >= self.pending_at.get() + SECTION_SETTLE_SECONDS {
            *self.pending.borrow_mut() = None;
            *self.shown.borrow_mut() = Some(want.clone());
            Some(want)
        } else {
            *self.shown.borrow_mut() = None;
            None
        }
    }

    /// Records the path this frame drew.
    pub fn publish(&self, frame: SectionLineFrame) {
        *self.frame.borrow_mut() = frame;
    }

    /// The path the last frame drew.
    pub fn frame(&self) -> SectionLineFrame {
        self.frame.borrow().clone()
    }

    /// Whether the line is on screen.
    pub fn visible(&self) -> bool {
        self.frame.borrow().visible()
    }

    /// A part was pressed: for the next 500 ms a second press is swallowed (O-10).
    pub fn pressed(&self, now: f64) {
        self.swallow_until.set(now + SECTION_DOUBLE_SECONDS);
    }

    /// Whether a press at `now` belongs to a double click's second half (O-10).
    pub fn swallowing(&self, now: f64) -> bool {
        now < self.swallow_until.get()
    }

    /// Whether a timer has come due and the view must run to show or hide the line.
    pub fn needs_frame(&self, now: f64) -> bool {
        let suppressed = now < self.fast_until.get() || now < self.hold_until.get();
        (self.fast_until.get() > 0.0 && now >= self.fast_until.get())
            || (self.hold_until.get() > 0.0 && now >= self.hold_until.get())
            || (!suppressed && self.pending.borrow().is_some() && now >= self.pending_at.get() + SECTION_SETTLE_SECONDS)
    }

    /// Clears expired suppression timers; `needs_frame` reports each once.
    pub fn tick(&self, now: f64) {
        if self.fast_until.get() > 0.0 && now >= self.fast_until.get() {
            self.fast_until.set(0.0);
        }
        if self.hold_until.get() > 0.0 && now >= self.hold_until.get() {
            self.hold_until.set(0.0);
        }
    }

    /// The next clock time at which the line's timers need a frame.
    pub fn next_wake(&self, now: f64) -> Option<f64> {
        let fast = self.fast_until.get();
        let hold = self.hold_until.get();
        let suppress = fast.max(hold);
        let pending = self.pending.borrow().is_some().then(|| self.pending_at.get() + SECTION_SETTLE_SECONDS);
        [Some(suppress), pending].into_iter().flatten().filter(|at| *at > now).min_by(f64::total_cmp)
    }
}

impl WindowState {
    /// Drains the section line's due timers; schedules the next one.
    ///
    /// Returns whether the view must run: a suppression ended, or a waiting path
    /// reached its settle time. The wakes themselves are scheduled when the
    /// scroll is observed ([`crate::reading`]) and when an outline jump starts;
    /// this keeps the newest deadline on the runtime's clock.
    pub(crate) fn poll_section_line(&mut self, now: f64, tx: &mut Tx) -> bool {
        let changed = self.section_line.needs_frame(now);
        if changed {
            self.section_line.tick(now);
        }
        if let Some(at) = self.section_line.next_wake(now) {
            tx.wake_at(at);
        }
        changed
    }

    /// The reader pane's section line, when it draws one.
    ///
    /// `width` is the reading viewport's width, so the path uses the article's
    /// own side padding steps and starts where its text does (R-02/O-08).
    pub(crate) fn section_line_overlay(&self, kit: &Kit, width: f32) -> Option<El> {
        let frame = self.section_line.frame();
        if frame.segments.is_empty() {
            return None;
        }
        let c = &kit.c;
        let font = TextStyle::new(12.5, 500.0);
        let (pad_x, _) = column_padding(width);
        let mut path = row()
            .key(SECTION_LINE_PATH_KEY)
            .w_full()
            .max_w(width.min(READER_MAX_WIDTH))
            .min_w(0.0)
            .px((pad_x - 6.0).max(0.0))
            .gap(2.0)
            .items_center();
        let last = frame.segments.len().saturating_sub(1);
        for (index, segment) in frame.segments.into_iter().enumerate() {
            if index > 0 {
                path = path.child(text(SECTION_SEPARATOR.to_string()).font(font).color(c.text_faint).shrink());
            }
            let heading = segment.heading;
            path = path.child(
                row()
                    .key(format!("{SECTION_SEGMENT_PREFIX}{index}"))
                    .h(22.0)
                    .px(6.0)
                    .items_center()
                    // A plain span, not a button: the line is a mouse shortcut,
                    // out of the tab order and hidden from readers (O-10).
                    .role("presentation")
                    .focusable(false)
                    .cursor(rune::Cursor::Pointer)
                    .radius(4.0)
                    .shrink()
                    .min_w(0.0)
                    .hover(c.hover)
                    .on_press_tx(move |state: &mut WindowState, tx: &mut Tx| {
                        section_pressed(state, heading, tx);
                    })
                    // A double click's second press is the one the guard swallows;
                    // there is never a second jump to act on.
                    .on_double_press(|_: &mut WindowState| {})
                    .child(
                        text(segment.text)
                            .font(font)
                            .color(if index == last { c.text_base } else { c.text_muted })
                            .truncate(),
                    ),
            );
        }
        Some(
            row()
                .key(SECTION_LINE_KEY)
                .pin(Some(0.0), Some(0.0), Some(0.0), None)
                .h(SECTION_LINE_HEIGHT)
                .w_full()
                .items_center()
                .justify_center()
                .fill(c.bg_base)
                .border_sides([0.0, 0.0, 1.0, 0.0], c.border_base)
                .border_layout()
                .a11y_hidden()
                .child(path),
        )
    }

    /// The reading progress hairline, when the document scrolls (R-03).
    pub(crate) fn progress_overlay(&self, kit: &Kit) -> Option<El> {
        let width = progress_width(self.scroll_snapshot())?;
        Some(
            row()
                .key(PROGRESS_KEY)
                .pin(Some(0.0), Some(0.0), Some(0.0), None)
                .h(2.0)
                .w_full()
                .a11y_hidden()
                .child(el().key(PROGRESS_FILL_KEY).h(2.0).w_frac(width).fill(kit.c.bg_accent)),
        )
    }

    /// The invisible guard that swallows a double click's second press on the
    /// line's old place (O-10).
    pub(crate) fn section_swallow_shield(&self, now: f64) -> Option<El> {
        if !self.section_line.swallowing(now) {
            return None;
        }
        Some(
            el().key(SECTION_SHIELD_KEY)
                .pin(Some(0.0), Some(0.0), Some(0.0), None)
                .h(SECTION_LINE_HEIGHT)
                .w_full()
                .a11y_hidden()
                .on_press(|_: &mut WindowState| {}),
        )
    }
}

/// The hairline's filled share, or `None` while the document has nowhere to
/// scroll (R-03: hidden at 24 points or less).
pub fn progress_width(scroll: ScrollSnapshot) -> Option<f32> {
    if !scroll.max.is_finite() || scroll.max <= PROGRESS_MIN_SCROLL {
        return None;
    }
    Some((scroll.offset / scroll.max).clamp(0.0, 1.0))
}

fn section_pressed(state: &mut WindowState, heading: usize, tx: &mut Tx) {
    let now = state.ui.last_now;
    // The second press of a double click lands on the text the line was over:
    // it must neither jump again nor select (O-10).
    if state.section_line.swallowing(now) {
        return;
    }
    state.jump_to_heading(&heading_key(heading), tx);
    state.section_line.pressed(now);
    // The guard is drawn for 500 ms; one frame after it ends clears it.
    tx.wake_at(now + SECTION_DOUBLE_SECONDS + 0.02);
}

/// The heading path of `current`, outermost first (O-08).
///
/// Only the outline's headings (h1–h4 that a rendered block owns) form the tree;
/// a skipped level hangs from the nearest earlier smaller one. The document's
/// title — exactly one h1, and the first heading — is left out.
pub fn path_of(doc: &MdDoc, current: Option<usize>) -> Vec<usize> {
    let nodes: Vec<usize> = doc
        .headings
        .iter()
        .enumerate()
        .filter(|(_, heading)| heading.in_toc && heading.block != usize::MAX)
        .map(|(index, _)| index)
        .collect();
    let Some(current) = current else { return Vec::new() };
    let Some(position) = nodes.iter().position(|index| *index == current) else { return Vec::new() };
    let mut parents: Vec<Option<usize>> = vec![None; nodes.len()];
    let mut open: Vec<usize> = Vec::new();
    for (place, index) in nodes.iter().enumerate() {
        let level = doc.headings[*index].level;
        while open.last().is_some_and(|last| doc.headings[nodes[*last]].level >= level) {
            open.pop();
        }
        parents[place] = open.last().copied();
        open.push(place);
    }
    let h1s = nodes.iter().filter(|index| doc.headings[**index].level == 1).count();
    let title = (h1s == 1 && nodes.first().is_some_and(|index| doc.headings[*index].level == 1)).then_some(0);
    let mut path = Vec::new();
    let mut step = Some(position);
    while let Some(place) = step {
        if Some(place) != title {
            path.push(nodes[place]);
        }
        step = parents[place];
    }
    path.reverse();
    path
}

/// The last path heading whose top has passed the viewport's top (O-07).
pub fn current_heading(doc: &MdDoc, headings: &[MeasuredHeading]) -> Option<usize> {
    headings
        .iter()
        .rev()
        .find(|heading| {
            heading.top <= 0.0
                && doc.headings.get(heading.index).is_some_and(|heading| heading.in_toc && heading.block != usize::MAX)
        })
        .map(|heading| heading.index)
}

/// Whether a heading reaches into the top band and has not fully gone (O-09).
pub fn heading_in_band(headings: &[MeasuredHeading]) -> bool {
    headings.iter().any(|heading| heading.top < SECTION_LINE_BAND && heading.bottom > SECTION_LINE_GONE)
}

/// Whether a table header holds the top under the line, or is about to (O-09).
///
/// The original asks whether a table spans the line's lower edge (`edge =
/// SECTION_LINE_HEIGHT - off`) and its header would cover that edge. A table's
/// top band is its header, so a table whose top is at or above the line's lower
/// edge while its bottom is below it owns the line — the exact header height is
/// not measured (OI-141's remaining approximation).
pub fn table_holds_top(tables: &[MeasuredTable]) -> bool {
    tables.iter().any(|table| table.top <= SECTION_LINE_HEIGHT && table.bottom > SECTION_LINE_HEIGHT)
}

/// The path to show, before the settle timer decides whether it may appear.
pub fn wanted(doc: &MdDoc, headings: &[MeasuredHeading], tables: &[MeasuredTable]) -> Option<Vec<usize>> {
    let current = current_heading(doc, headings)?;
    if heading_in_band(headings) {
        return None;
    }
    if table_holds_top(tables) {
        return None;
    }
    let path = path_of(doc, Some(current));
    (!path.is_empty()).then_some(path)
}

/// Every drawn heading's top and bottom relative to the article viewport's top.
///
/// The reader view publishes this list each frame in the reading mode so the
/// Back/Forward jump rule can tell a jump that would move the page from one that
/// lands where the heading already is (O-12). A frame without a viewport yet
/// measures nothing.
pub(crate) fn measure_headings(cx: &Cx<WindowState>, doc: &MdDoc) -> Vec<MeasuredHeading> {
    let Some(viewport) = cx.window_layout(ARTICLE_SCROLL_KEY) else { return Vec::new() };
    measure_headings_from(cx, doc, viewport[1])
}

/// Whether the keyboard's focus ring is under the line (O-09, OI-141).
///
/// Rune answers `Tx::focused()` in a poll, so the window records the focused key
/// and whether the keyboard put it there (`focus-visible`); this reads that
/// element's last layout. The element must overlap the article horizontally — a
/// focused outline row or toolbar button at the same height is not under the
/// line — and reach into the line's 30 points plus the ring's reach.
fn focus_under_line(state: &WindowState, cx: &Cx<WindowState>, viewport: [f32; 4]) -> bool {
    if !state.ui.focus_visible {
        return false;
    }
    let Some(key) = state.ui.focused_key.as_deref().filter(|key| !key.is_empty() && !key.starts_with('#')) else {
        return false;
    };
    let Some(rect) = cx.window_layout(key) else { return false };
    focus_rect_hits(rect, viewport)
}

/// Whether `rect` reaches the line's band and overlaps the article horizontally.
fn focus_rect_hits(rect: [f32; 4], viewport: [f32; 4]) -> bool {
    if rect[2] <= 0.0 || rect[3] <= 0.0 || !rect[0].is_finite() || !rect[1].is_finite() {
        return false;
    }
    let (left, right) = (viewport[0], viewport[0] + viewport[2]);
    if rect[0] + rect[2] < left + SECTION_FOCUS_RING || rect[0] > right - SECTION_FOCUS_RING {
        return false;
    }
    let top = viewport[1] - SECTION_FOCUS_RING;
    let bottom = viewport[1] + SECTION_LINE_HEIGHT + SECTION_FOCUS_RING;
    rect[1] + rect[3] > top && rect[1] < bottom
}

/// The top-level tables' edges relative to the article viewport's top.
///
/// The header row's height is measured from the drawn table (under the app's
/// `TableWire` wrapper, `b7/table-block-b7/table/head`, or `…/scroll/table/head`
/// when the table scrolls sideways) so a jump that lands under a stuck header
/// can clear it (L-01).
pub(crate) fn measure_tables(cx: &Cx<WindowState>, doc: &MdDoc, viewport_top: f32) -> Vec<MeasuredTable> {
    let mut tables = Vec::new();
    for (index, block) in doc.blocks.iter().enumerate() {
        if !matches!(block, Block::Table(_)) {
            continue;
        }
        let Some(key) = doc.block_key(index) else { continue };
        let Some(rect) = cx.window_layout(key) else { continue };
        if !rect[3].is_finite() || rect[3] <= 0.0 {
            continue;
        }
        let header = crate::editor::scroll_sync::table_head_paths(key)
            .iter()
            .find_map(|path| cx.window_layout(path))
            .filter(|rect| rect[3].is_finite() && rect[3] > 0.0)
            .map_or(0.0, |rect| rect[3]);
        tables.push(MeasuredTable { top: rect[1] - viewport_top, bottom: rect[1] + rect[3] - viewport_top, header });
    }
    tables
}

fn measure_headings_from(cx: &Cx<WindowState>, doc: &MdDoc, viewport_top: f32) -> Vec<MeasuredHeading> {
    let mut headings = Vec::new();
    for (index, heading) in doc.headings.iter().enumerate().take(MEASURE_MAX) {
        if heading.block == usize::MAX {
            continue;
        }
        let Some(rect) = cx.window_layout(&heading_key(index)) else { continue };
        if !rect[3].is_finite() || rect[3] <= 0.0 {
            continue;
        }
        headings.push(MeasuredHeading { index, top: rect[1] - viewport_top, bottom: rect[1] + rect[3] - viewport_top });
    }
    headings
}

/// Computes and publishes this frame's line from the last layout.
///
/// Called by the reader view; a viewing tab with the outline out of sight and
/// headings gets a path once the page has settled (O-07/O-09). `headings` is the
/// frame's measurement, which the reader view already took for O-12 and
/// publishes for the Back/Forward jump rule.
pub(crate) fn compute(
    state: &WindowState,
    cx: &Cx<WindowState>,
    doc: &MdDoc,
    headings: &[MeasuredHeading],
) -> SectionLineFrame {
    let now = state.ui.last_now;
    state.section_line.observe_scroll(state.scroll_snapshot(), now);
    // L-01: this frame's tables are measured for the jump's sticky-header
    // cover whether or not the line itself is on screen.
    let Some(viewport) = cx.window_layout(ARTICLE_SCROLL_KEY) else {
        state.section_line.set_tables(Vec::new());
        return SectionLineFrame::default();
    };
    let tables = measure_tables(cx, doc, viewport[1]);
    state.section_line.set_tables(tables.clone());
    // The outline out of sight: hidden, or folded away without peeking over the
    // text. Never while editing (this view is the reading view), nor without
    // headings (O-07).
    let enabled = !state.right.is_open() && !state.right.is_peeking() && has_toc_headings(doc);
    if !enabled {
        state.section_line.clear();
        return SectionLineFrame::default();
    }
    // O-09: an element with the keyboard's focus ring under the line owns it.
    let clear_of_focus = !focus_under_line(state, cx, viewport);
    let want = wanted(doc, headings, &tables).filter(|_| clear_of_focus);
    let segments = state
        .section_line
        .settle(want, now)
        .map(|path| {
            path.into_iter()
                .map(|heading| SectionSegment { heading, text: crate::reading_meta::heading_line(doc, heading) })
                .collect()
        })
        .unwrap_or_default();
    let frame = SectionLineFrame { segments };
    state.section_line.publish(frame.clone());
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(tops: &[(usize, f32, f32)]) -> Vec<MeasuredHeading> {
        tops.iter().map(|(index, top, bottom)| MeasuredHeading { index: *index, top: *top, bottom: *bottom }).collect()
    }

    #[test]
    fn the_path_leaves_out_a_single_first_h1() {
        let doc = lmdv_md::parse("# Brief\n\n## 2. Decisions\n\n### 2.1 Budget\n\n### 2.2 Staff\n");
        assert_eq!(path_of(&doc, Some(0)), Vec::<usize>::new(), "the title alone names no section");
        assert_eq!(path_of(&doc, Some(1)), vec![1]);
        assert_eq!(path_of(&doc, Some(2)), vec![1, 2]);
        assert_eq!(path_of(&doc, Some(3)), vec![1, 3], "a sibling replaces the branch");
    }

    #[test]
    fn skipped_levels_hang_from_the_nearest_smaller_heading() {
        let doc = lmdv_md::parse("# Title\n\n### Deep\n\n#### Deeper\n\n## Middle\n");
        assert_eq!(path_of(&doc, Some(1)), vec![1], "an h3 under an h1 hangs from it");
        assert_eq!(path_of(&doc, Some(2)), vec![1, 2]);
        assert_eq!(path_of(&doc, Some(3)), vec![3], "a new h2 is a root of its own");
    }

    #[test]
    fn several_h1s_stay_in_the_path() {
        let doc = lmdv_md::parse("# One\n\n## A\n\n# Two\n\n## B\n");
        assert_eq!(path_of(&doc, Some(1)), vec![0, 1]);
        assert_eq!(path_of(&doc, Some(3)), vec![2, 3]);
    }

    #[test]
    fn h5_h6_and_footnote_definitions_never_form_the_path() {
        let doc = lmdv_md::parse("# Title\n\n## Section\n\n##### Note\n\nA note.[^1]\n\n[^1]: # Inside\n");
        assert_eq!(path_of(&doc, Some(2)), Vec::<usize>::new(), "the h5 is not in the outline");
        let inside = doc.headings.iter().position(|heading| heading.block == usize::MAX).expect("a definition heading");
        assert_eq!(path_of(&doc, Some(inside)), Vec::<usize>::new());
        assert_eq!(path_of(&doc, None), Vec::<usize>::new());
        assert_eq!(path_of(&doc, Some(1)), vec![1]);
    }

    #[test]
    fn the_line_appears_once_the_section_has_gone_and_no_heading_is_in_the_band() {
        let doc = lmdv_md::parse("# Title\n\n## A\n\nText\n\n## B\n");
        // A has gone 120 above; B is far below: the path of A shows.
        let heads = measured(&[(0, -500.0, -470.0), (1, -120.0, -90.0), (2, 300.0, 330.0)]);
        assert_eq!(wanted(&doc, &heads, &[]), Some(vec![1]));

        // A is only 20 above and still reaches into the 56-point band: none.
        let heads = measured(&[(0, -500.0, -470.0), (1, -20.0, 10.0), (2, 300.0, 330.0)]);
        assert_eq!(wanted(&doc, &heads, &[]), None);

        // A heading arriving from below, its top in the band and its bottom visible: none.
        let heads = measured(&[(0, -500.0, -470.0), (1, -120.0, -90.0), (2, 30.0, 60.0)]);
        assert_eq!(wanted(&doc, &heads, &[]), None);

        // Before the first section (the title only) nothing is named.
        let heads = measured(&[(0, -120.0, -90.0), (1, 300.0, 330.0)]);
        assert_eq!(wanted(&doc, &heads, &[]), None);
    }

    #[test]
    fn a_table_header_holding_the_top_hides_the_line() {
        let doc = lmdv_md::parse("# Title\n\n## A\n\nText\n\n| H | I |\n| --- | --- |\n| x | y |\n");
        let heads = measured(&[(0, -500.0, -470.0), (1, -120.0, -90.0)]);
        let holding = [MeasuredTable { top: -40.0, bottom: 200.0, header: 0.0 }];
        assert_eq!(wanted(&doc, &heads, &holding), None);
        let gone = [MeasuredTable { top: -300.0, bottom: -40.0, header: 0.0 }];
        assert_eq!(wanted(&doc, &heads, &gone), Some(vec![1]), "a table scrolled past does not hide it");
        let below = [MeasuredTable { top: 200.0, bottom: 400.0, header: 0.0 }];
        assert_eq!(wanted(&doc, &heads, &below), Some(vec![1]));
    }

    #[test]
    fn a_table_about_to_stick_also_hides_the_line() {
        // OI-141: the original hides the line while a table spans its lower
        // edge, not only once its top has passed the viewport's top.
        let about = [MeasuredTable { top: 18.0, bottom: 300.0, header: 0.0 }];
        assert!(table_holds_top(&about), "the table's header reaches the line's lower edge");
        let past = [MeasuredTable { top: -2.0, bottom: 300.0, header: 0.0 }];
        assert!(table_holds_top(&past));
        let below = [MeasuredTable { top: SECTION_LINE_HEIGHT + 1.0, bottom: 300.0, header: 0.0 }];
        assert!(!table_holds_top(&below), "clear of the line's edge");
        let gone = [MeasuredTable { top: -300.0, bottom: SECTION_LINE_HEIGHT, header: 0.0 }];
        assert!(!table_holds_top(&gone), "a table scrolled past does not hide it");
    }

    #[test]
    fn the_focus_ring_band_uses_the_line_and_the_ring() {
        let viewport = [100.0, 40.0, 760.0, 700.0];
        let inside = [180.0, 30.0, 300.0, 24.0];
        let above = [180.0, 8.0, 300.0, 20.0];
        let beside = [1000.0, 40.0, 300.0, 24.0];
        assert!(focus_rect_hits(inside, viewport));
        assert!(!focus_rect_hits(above, viewport), "clear above the ring");
        assert!(!focus_rect_hits(beside, viewport), "another pane at the same height");
    }

    #[test]
    fn a_new_path_waits_for_the_page_to_settle() {
        let line = SectionLineState::default();
        assert_eq!(line.settle(Some(vec![1]), 0.0), None, "not at the first frame");
        assert_eq!(line.settle(Some(vec![1]), 0.119), None);
        assert_eq!(line.settle(Some(vec![1]), 0.12), Some(vec![1]));
        assert_eq!(line.settle(Some(vec![2]), 0.13), None, "another path hides at once");
        assert_eq!(line.settle(Some(vec![2]), 0.24), None);
        assert_eq!(line.settle(Some(vec![2]), 0.25), Some(vec![2]));
        assert_eq!(line.settle(None, 0.26), None, "no path hides at once");
    }

    #[test]
    fn fast_scrolling_and_its_settle_hide_the_line() {
        let line = SectionLineState::default();
        line.observe_scroll(ScrollSnapshot { offset: 0.0, ..ScrollSnapshot::default() }, 0.0);
        line.observe_scroll(ScrollSnapshot { offset: 100.0, ..ScrollSnapshot::default() }, 0.01);
        assert_eq!(line.settle(Some(vec![1]), 0.01), None);
        assert_eq!(line.settle(Some(vec![1]), 0.11), None, "still inside the 120 ms settle");
        assert_eq!(line.settle(Some(vec![1]), 0.13), Some(vec![1]), "the page has gone slow");
        line.tick(0.13);
        assert!(!line.needs_frame(0.13));

        let slow = SectionLineState::default();
        slow.observe_scroll(ScrollSnapshot { offset: 0.0, ..ScrollSnapshot::default() }, 0.0);
        slow.observe_scroll(ScrollSnapshot { offset: 10.0, ..ScrollSnapshot::default() }, 0.1);
        assert_eq!(slow.settle(Some(vec![1]), 0.1), None);
        assert_eq!(slow.settle(Some(vec![1]), 0.22), Some(vec![1]), "1 px/ms is not fast");
    }

    #[test]
    fn a_heading_jump_holds_the_line_until_it_lands() {
        let line = SectionLineState::default();
        assert_eq!(line.settle(Some(vec![1]), 0.0), None);
        assert_eq!(line.settle(Some(vec![1]), 0.12), Some(vec![1]));
        line.hold_until_landing();
        assert_eq!(line.settle(Some(vec![2]), 1.0), None);
        assert_eq!(line.settle(Some(vec![2]), 1.25), None, "open-ended: no fixed 250 ms");
        assert_eq!(line.settle(Some(vec![2]), 5.0), None, "still away while the glide runs");
        line.release_hold();
        assert_eq!(line.settle(Some(vec![2]), 5.0), None, "the line waits for its settle");
        assert_eq!(line.settle(Some(vec![2]), 5.12), Some(vec![2]), "the glide has landed");
    }

    #[test]
    fn a_second_press_is_swallowed_for_500_ms() {
        let line = SectionLineState::default();
        assert!(!line.swallowing(0.0));
        line.pressed(1.0);
        assert!(line.swallowing(1.1));
        assert!(line.swallowing(1.499));
        assert!(!line.swallowing(1.5));
    }

    #[test]
    fn a_reset_forgets_the_path_and_every_timer() {
        let line = SectionLineState::default();
        line.observe_scroll(ScrollSnapshot { offset: 0.0, ..ScrollSnapshot::default() }, 0.0);
        line.observe_scroll(ScrollSnapshot { offset: 100.0, ..ScrollSnapshot::default() }, 0.01);
        assert_eq!(line.settle(Some(vec![1]), 0.05), None, "the fast page suppresses it");
        assert_eq!(line.settle(Some(vec![1]), 0.2), Some(vec![1]), "the suppression's end is the settle's end");
        line.publish(SectionLineFrame { segments: vec![SectionSegment { heading: 1, text: "One".to_owned() }] });
        assert!(line.visible());
        line.reset();
        assert!(!line.visible());
        assert!(!line.swallowing(0.2));
        assert!(!line.needs_frame(1.0));
        assert_eq!(line.next_wake(1.0), None);
        assert_eq!(line.settle(Some(vec![1]), 1.0), None, "the settle wait starts over");
    }

    #[test]
    fn the_hairline_hides_on_a_short_document_and_follows_the_offset() {
        assert_eq!(progress_width(ScrollSnapshot { offset: 10.0, max: 24.0, viewport: 700.0 }), None);
        assert_eq!(progress_width(ScrollSnapshot { offset: 10.0, max: 0.0, viewport: 700.0 }), None);
        assert_eq!(progress_width(ScrollSnapshot { offset: 0.0, max: 1000.0, viewport: 700.0 }), Some(0.0));
        assert_eq!(progress_width(ScrollSnapshot { offset: 500.0, max: 1000.0, viewport: 700.0 }), Some(0.5));
        assert_eq!(progress_width(ScrollSnapshot { offset: 2000.0, max: 1000.0, viewport: 700.0 }), Some(1.0));
        let mut previous = 0.0;
        for offset in [100.0, 250.0, 500.0, 900.0] {
            let width = progress_width(ScrollSnapshot { offset, max: 1000.0, viewport: 700.0 }).expect("shown");
            assert!(width >= previous, "the width never goes back");
            previous = width;
        }
    }
}
