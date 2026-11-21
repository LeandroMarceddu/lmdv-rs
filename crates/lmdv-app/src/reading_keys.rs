//! Reading keys and their motion (R-06–R-09, A-15, OI-156).
//!
//! The single-key grammar of the reading view, as the original's
//! `handleReadingKey` has it (`src/main.ts:3831-3881`):
//!
//! * `j`/`k` scroll 80 points a press. Presses under 400 ms apart add up from
//!   the previous *target*, not from wherever the glide happens to be, and a
//!   held key's repeats jump instead of gliding (R-06);
//! * `Space`/`Shift+Space` page by 90% of the viewport (R-07). Rune does not
//!   page on Space; it pages `PageUp`/`PageDown`/`Home`/`End` itself, so this
//!   module leaves those to it and never double-scrolls;
//! * `.`/`,` step through the outline's headings on a pinned index, so trailing
//!   headings that cannot reach the top are still visited (R-08);
//! * `g g` goes to the top, `G` to the end; a lone `g` only opens the 500 ms
//!   window (R-09).
//!
//! Every request is a Rune scroll request (`Tx::scroll_to`, `scroll_to_bottom`,
//! `scroll_page`), so it glides on Rune's spring and jumps under reduced motion
//! (A-15). Restores are not reading motion and keep their instant requests:
//! `--line`, a remembered position, Back/Forward and a new document.
//!
//! The pinned heading (O-04) lives here too: a heading jump — an outline click,
//! a section-line part, a `#anchor`, or a `.`/`,` step — pins its heading, and
//! the outline highlights it while the page glides. A wheel, a drag or an
//! instant request releases it at once; a glide releases it on the first scroll
//! report after the page has been quiet for 250 ms (the original's
//! `SETTLE_IDLE_MS`, `src/toc.ts:12`).

use lmdv_md::MdDoc;
use lmdv_render::heading_key;
use rune::{Key, KeyEvent, Tx};

use crate::reader::{ARTICLE_SCROLL_KEY, ScrollSignal, toc_headings};
use crate::window::{Mode, WindowState};

/// Points one `j`/`k` press scrolls (R-06).
pub const KEY_SCROLL: f32 = 80.0;
/// Presses closer together than this add up from the previous target (R-06).
pub const KEY_ACCUMULATE_SECONDS: f64 = 0.400;
/// Wheel notches closer together than this add up from the previous target.
pub const WHEEL_ACCUMULATE_SECONDS: f64 = 0.200;
/// Rune's glide spring (`tok::motion::GLIDE`), in seconds: the smooth single
/// notch.
pub const WHEEL_GLIDE_SECONDS: f32 = rune::tok::motion::GLIDE.duration;
/// The stiffest spring a fast wheel burst glides on, in seconds. Below this the
/// motion reads as a jump rather than a glide.
pub const WHEEL_GLIDE_MIN_SECONDS: f32 = 0.10;
/// How far a fast wheel's glide trails the wheel, in points (under two
/// notches of a 40-point wheel).
pub const WHEEL_LAG: f32 = 72.0;
/// Two `g` presses inside this window go to the top (R-09).
pub const DOUBLE_G_SECONDS: f64 = 0.500;
/// The next heading is the first whose top is past this, below the viewport's
/// top (`src/toc.ts:175`: past the scroll margin a just-reached heading sits at).
///
/// Just inside [`crate::reader::READER_PAD_TOP`] (32, R-02): the first heading
/// sits exactly on the column's top padding, and `.` from the top must still
/// reach it.
pub const STEP_BELOW_TOP: f32 = 24.0;
/// The previous heading is the last one above this (`src/toc.ts:176`).
pub const STEP_ABOVE_TOP: f32 = -4.0;
/// Quiet time after a heading jump's last scroll report before a report counts
/// as the reader's own (the original's `SETTLE_IDLE_MS`).
pub const PIN_IDLE_SECONDS: f64 = 0.250;

/// One reading key's meaning (R-06–R-09).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadingKey {
    /// `j`.
    ScrollDown,
    /// `k`.
    ScrollUp,
    /// `Space`.
    PageDown,
    /// `Shift+Space`.
    PageUp,
    /// `.`.
    NextHeading,
    /// `,`.
    PreviousHeading,
    /// The first `g` of `g g`.
    Top,
    /// `G`.
    End,
}

/// The reading key an event is, or `None` when it belongs to something else.
///
/// A command chord never reads: Ctrl/Cmd, Alt and Super are all rejected, so
/// `Ctrl+G` stays go-to-line and the palette's chords keep working. `G` arrives
/// as `Text("G")` on most platforms and as `Text("g")` plus Shift on some, so
/// both spellings are accepted; `.`/`,` with Shift do nothing, as in the
/// original, which upper-cases a shifted character before the match.
pub fn reading_key_of(event: &KeyEvent) -> Option<ReadingKey> {
    let mods = event.mods;
    if mods.cmd || mods.ctrl || mods.alt || mods.logo {
        return None;
    }
    let Key::Text(text) = &event.key else { return None };
    match text.as_str() {
        "j" if !mods.shift => Some(ReadingKey::ScrollDown),
        "k" if !mods.shift => Some(ReadingKey::ScrollUp),
        " " => Some(if mods.shift { ReadingKey::PageUp } else { ReadingKey::PageDown }),
        "." if !mods.shift => Some(ReadingKey::NextHeading),
        "," if !mods.shift => Some(ReadingKey::PreviousHeading),
        "g" if mods.shift => Some(ReadingKey::End),
        "g" => Some(ReadingKey::Top),
        "G" => Some(ReadingKey::End),
        _ => None,
    }
}

/// The `j`/`k` target and when it was asked for (R-06).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct KeyTarget {
    pub(crate) target: f32,
    pub(crate) at: f64,
}

/// A jumped-to heading the outline keeps highlighted (O-04).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeadingPin {
    /// Index into [`MdDoc::headings`].
    pub(crate) index: usize,
    /// The model revision the index belongs to; a reparse drops the pin.
    pub(crate) revision: u64,
    /// A scroll report before this time is the jump's own glide.
    pub(crate) settled_at: f64,
}

/// Per-window reading-key state: the j/k target, the `g` window and the pin.
///
/// Reset on every new reading place — a tab switch, a document open, entering
/// edit mode, a scene reset, Back/Forward (which goes through
/// [`WindowState::begin_restore`](crate::window::WindowState)) — so no key or
/// window outlives the page it belongs to.
#[derive(Debug, Default)]
pub(crate) struct ReadingKeys {
    /// The last `j`/`k` press (R-06).
    pub(crate) target: Option<KeyTarget>,
    /// When the last `g` was pressed (R-09).
    pub(crate) last_g: Option<f64>,
    /// The heading the outline keeps highlighted (O-04).
    pub(crate) pin: Option<HeadingPin>,
    /// When the last notched wheel delta was taken.
    pub(crate) last_wheel: Option<f64>,
    /// The wheel's own speed, in points a second, smoothed over a burst
    /// ([`Self::wheel_spring`]); 0 outside a burst.
    pub(crate) wheel_speed: f32,
    /// The scroll report count the pin was armed at.
    report: u64,
}

impl ReadingKeys {
    /// Forgets everything: a tab switch, an open, edit mode, a scene reset.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Where a `j`/`k` press starts from: the previous target inside 400 ms,
    /// the page's own position otherwise (R-06).
    pub(crate) fn scroll_from(&self, now: f64, offset: f32) -> f32 {
        self.scroll_from_within(now, offset, KEY_ACCUMULATE_SECONDS)
    }

    /// [`Self::scroll_from`] with another accumulation window: a notched wheel
    /// repeats faster than a key press and takes a shorter one.
    pub(crate) fn scroll_from_within(&self, now: f64, offset: f32, window: f64) -> f32 {
        match self.target {
            Some(last) if now >= last.at && now - last.at < window => last.target,
            _ => offset,
        }
    }

    /// Records the target of a `j`/`k` press.
    pub(crate) fn note_scroll(&mut self, target: f32, now: f64) {
        self.target = Some(KeyTarget { target, at: now });
    }

    /// The spring one notch glides on, from how fast the wheel is turning.
    ///
    /// Rune bounds a glide's speed by `ω × distance left`, so Rune's own 0.32 s
    /// spring settles ~300 points behind a wheel turning at 60 notches a
    /// second: the content visibly trails the hand. A shorter spring raises ω,
    /// and with it the speed the glide may carry, so a burst holds a constant
    /// [`WHEEL_LAG`] behind the wheel instead of falling further behind the
    /// faster it turns. One notch, or a wheel after a pause, keeps Rune's glide
    /// untouched, so the smooth single notch stays smooth.
    ///
    /// Called once per notch; it also records the notch as the gesture's last
    /// ([`Self::last_wheel`]), before [`Self::note_scroll`] moves the target.
    pub(crate) fn wheel_spring(&mut self, now: f64, dy: f32) -> rune::Spring {
        let gap = self.last_wheel.and_then(|at| (now >= at).then_some(now - at));
        self.last_wheel = Some(now);
        let instant = match gap {
            // Two events in the same instant are one burst at the highest rate
            // the clock can tell: a 4 ms floor keeps the spring finite.
            Some(gap) if gap > 0.0 && gap < WHEEL_ACCUMULATE_SECONDS => dy.abs() / gap.max(0.004) as f32,
            _ => 0.0,
        };
        self.wheel_speed = if instant <= 0.0 || !instant.is_finite() {
            0.0
        } else if self.wheel_speed <= 0.0 {
            instant
        } else {
            // Smoothed: a wheel's events are not evenly spaced, and one short
            // gap must not turn a slow gesture into a burst.
            self.wheel_speed * 0.5 + instant * 0.5
        };
        wheel_spring_for(self.wheel_speed)
    }

    /// Records a `g` press; true when it completed `g g` inside 500 ms (R-09).
    ///
    /// A press after the window starts a new one, as the original does: `g`,
    /// a pause, `g` does not jump, and the pause's `g` opens a fresh window.
    pub(crate) fn press_g(&mut self, now: f64) -> bool {
        match self.last_g {
            Some(at) if now >= at && now - at < DOUBLE_G_SECONDS => {
                self.last_g = None;
                true
            }
            _ => {
                self.last_g = Some(now);
                false
            }
        }
    }

    /// Pins heading `index` while its jump lands (O-04).
    pub(crate) fn pin_heading(&mut self, index: usize, revision: u64, now: f64, signal: ScrollSignal) {
        self.pin = Some(HeadingPin { index, revision, settled_at: now + PIN_IDLE_SECONDS });
        self.report = signal.report;
    }

    /// The pinned heading's index, while the pin belongs to this model.
    pub(crate) fn pinned_index(&self, doc: &MdDoc) -> Option<usize> {
        self.pin.filter(|pin| pin.revision == doc.revision && pin.index < doc.headings.len()).map(|pin| pin.index)
    }

    /// Drops the pin; true when one was set.
    pub(crate) fn release_pin(&mut self) -> bool {
        self.pin.take().is_some()
    }

    /// Watches the scroll reports for the moment the pin must go (O-04).
    ///
    /// A wheel, a drag or an instant request is unmistakably the reader's own
    /// (`by_hand`), and any report that arrives after the glide has been quiet
    /// for 250 ms is one too. True when the pin went, so the outline redraws.
    pub(crate) fn observe(&mut self, signal: ScrollSignal, now: f64) -> bool {
        if signal.report == self.report {
            return false;
        }
        self.report = signal.report;
        let Some(pin) = self.pin else { return false };
        if signal.by_hand || now >= pin.settled_at {
            self.pin = None;
            return true;
        }
        if let Some(pin) = self.pin.as_mut() {
            pin.settled_at = now + PIN_IDLE_SECONDS;
        }
        false
    }
}

/// The glide spring for `speed` points a second: Rune's own glide for a slow
/// wheel or a single notch, and a stiffer one — down to
/// [`WHEEL_GLIDE_MIN_SECONDS`] — the faster the wheel turns, so the trailing
/// distance stays near [`WHEEL_LAG`].
pub(crate) fn wheel_spring_for(speed: f32) -> rune::Spring {
    let speed = if speed.is_finite() { speed.max(0.0) } else { 0.0 };
    if speed <= 0.0 {
        return rune::tok::motion::GLIDE;
    }
    let duration = (std::f32::consts::TAU * WHEEL_LAG / speed).clamp(WHEEL_GLIDE_MIN_SECONDS, WHEEL_GLIDE_SECONDS);
    rune::Spring::new(duration, 0.0)
}

impl WindowState {
    /// Whether the reading keys may act: a reading surface is in front and no
    /// dialog, modal or pop-up owns the keyboard.
    ///
    /// A text field is not checked here: Rune gives the focused input the key
    /// before the app's handler, so a `j` typed into the find bar, the browser
    /// search, the outline filter or a settings field never reaches this
    /// module. Editing is a different mode and is rejected outright.
    pub(crate) fn reading_keys_active(&self) -> bool {
        let Some(tab) = self.active_tab() else { return false };
        if tab.view.mode != Mode::Viewer || tab.view.reader.is_none() {
            return false;
        }
        !(self.prompt().is_some()
            || self.help_view().is_some()
            || self.share.menu_open
            || self.palette_is_open()
            || self.settings_is_open()
            || self.table_preview().is_some()
            || self.lightbox.is_some()
            || self.block_menu().is_some()
            || self.browser_menu().is_some()
            || self.tab_menu().is_some())
    }

    /// Runs one reading key (R-06–R-09). False when the event is not one or the
    /// reading surface is not in front.
    pub(crate) fn reading_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        let Some(key) = reading_key_of(event) else { return false };
        if !self.reading_keys_active() {
            return false;
        }
        let now = tx.clock();
        match key {
            ReadingKey::ScrollDown => self.scroll_reading(KEY_SCROLL, now, tx),
            ReadingKey::ScrollUp => self.scroll_reading(-KEY_SCROLL, now, tx),
            ReadingKey::PageDown => {
                // R-07: one page is 90% of the viewport, instant and clamped by
                // Rune's `scroll_page`. `PageDown` itself is not bound here: Rune
                // pages its own scroll views, and taking it would scroll twice.
                self.reading_keys.release_pin();
                self.cancel_landing(now, tx);
                tx.scroll_page(ARTICLE_SCROLL_KEY, 1);
            }
            ReadingKey::PageUp => {
                self.reading_keys.release_pin();
                self.cancel_landing(now, tx);
                tx.scroll_page(ARTICLE_SCROLL_KEY, -1);
            }
            ReadingKey::NextHeading => {
                self.step_reading_heading(1, tx);
            }
            ReadingKey::PreviousHeading => {
                self.step_reading_heading(-1, tx);
            }
            ReadingKey::Top => {
                if self.reading_keys.press_g(now) {
                    self.reading_keys.release_pin();
                    self.cancel_landing(now, tx);
                    tx.scroll_to(ARTICLE_SCROLL_KEY, 0.0, 0.0);
                }
            }
            ReadingKey::End => {
                self.reading_keys.release_pin();
                self.cancel_landing(now, tx);
                tx.scroll_to_bottom(ARTICLE_SCROLL_KEY);
            }
        }
        // The original consumes every reading key, even one that found no
        // heading to step to or a lone `g` that only opens the window.
        true
    }

    /// `j`/`k`: 80 points from the target, the previous one inside 400 ms
    /// (R-06), gliding on the first press and jumping on a held repeat.
    fn scroll_reading(&mut self, delta: f32, now: f64, tx: &mut Tx) {
        let scroll = self.reading.scroll.get();
        let from = self.reading_keys.scroll_from(now, scroll.offset);
        // The live max is the clamp; only before the viewport's first report are
        // the extent and height both unknown, and then Rune clamps the request to
        // the content it measures in this same frame.
        let known = scroll.max > 0.0 || scroll.viewport > 0.0;
        let target = if known { (from + delta).clamp(0.0, scroll.max.max(0.0)) } else { (from + delta).max(0.0) };
        self.reading_keys.note_scroll(target, now);
        self.reading_keys.release_pin();
        self.cancel_landing(now, tx);
        if tx.key_repeat() {
            tx.scroll_to_y(ARTICLE_SCROLL_KEY, target);
        } else {
            tx.scroll_to(ARTICLE_SCROLL_KEY, 0.0, target);
        }
    }

    /// A notched mouse wheel over the article, smoothed onto the key glide.
    ///
    /// Rune applies a notched wheel instantly and a trackpad's precise deltas
    /// point by point. This takes only a plain vertical notch (`precise` false,
    /// no modifiers, no horizontal part) on an active reading surface: it
    /// accumulates from the previous wheel target within
    /// [`WHEEL_ACCUMULATE_SECONDS`], clamps to the document and glides there
    /// through `Tx::scroll_to_with`, which reduced motion turns into a jump
    /// (A-15). The spring is Rune's own glide for a single notch and stiffens
    /// with a burst's speed, so a fast wheel does not trail hundreds of points
    /// behind the hand; a slow wheel keeps the smooth glide.
    /// `dy` points content down, so a notch down (negative) moves the offset up.
    ///
    /// The first notch of a gesture releases the O-04 heading pin and forgets
    /// the key target: the wheel takes over, and the glide's own reports are not
    /// the reader's hand (Rune's `by_hand` never sees a taken wheel).
    /// Everything else returns false, so Rune scrolls it natively.
    pub(crate) fn article_wheel(&mut self, event: &rune::WheelEvent, tx: &mut Tx) -> bool {
        if event.precise || event.dx != 0.0 || event.dy == 0.0 || !event.dy.is_finite() {
            return false;
        }
        let mods = event.mods;
        if mods.cmd || mods.ctrl || mods.alt || mods.shift || mods.logo {
            return false;
        }
        if !self.reading_keys_active() {
            return false;
        }
        let now = tx.clock();
        let fresh = self.reading_keys.last_wheel.is_none_or(|at| now < at || now - at >= WHEEL_ACCUMULATE_SECONDS);
        // The glide's spring follows the wheel's own speed (see
        // `ReadingKeys::wheel_spring`); it also records this notch as the
        // gesture's last, so it runs after `fresh` reads the one before.
        let spring = self.reading_keys.wheel_spring(now, event.dy);
        if fresh {
            self.reading_keys.release_pin();
            self.reading_keys.target = None;
            self.cancel_landing(now, tx);
        }
        let scroll = self.reading.scroll.get();
        let from = self.reading_keys.scroll_from_within(now, scroll.offset, WHEEL_ACCUMULATE_SECONDS);
        let target = (from - event.dy).clamp(0.0, scroll.max.max(0.0));
        self.reading_keys.note_scroll(target, now);
        tx.scroll_to_with(ARTICLE_SCROLL_KEY, 0.0, target, spring);
        true
    }

    /// `.`/`,`: the next or previous outline heading (R-08).
    ///
    /// The step is not a place: the original's reading keys only glide, they do
    /// not record Back (`src/toc.ts:24-26`). [`Self::scroll_to_heading`] is the
    /// outline's own jump, so the row stays in view, the section line steps
    /// aside and the heading is pinned while the page lands.
    fn step_reading_heading(&mut self, dir: i8, tx: &mut Tx) {
        let Some(doc) = self.active_reader() else { return };
        let list: Vec<usize> = toc_headings(&doc).map(|(index, _)| index).collect();
        if list.is_empty() {
            return;
        }
        let pinned = self.reading_keys.pinned_index(&doc).filter(|index| list.contains(index));
        let tops: Vec<(usize, f32)> =
            list.iter().filter_map(|index| self.heading_top(*index).map(|top| (*index, top))).collect();
        let anchor = self.reading.viewport_anchor.get().and_then(|anchor| anchor.heading);
        if let Some(target) = step_target(&list, pinned, &tops, anchor, dir) {
            self.scroll_to_heading(&heading_key(target), tx);
        }
    }

    /// The heading the outline highlights while a jump lands (O-04), as its
    /// rendered key. `None` once the reader has scrolled on their own or the
    /// pin's model was reparsed.
    pub(crate) fn pinned_heading(&self, doc: &MdDoc) -> Option<String> {
        let index = self.reading_keys.pinned_index(doc)?;
        toc_headings(doc).any(|(candidate, _)| candidate == index).then(|| heading_key(index))
    }

    /// Feeds this frame's scroll report to the pin (O-04). True when the pin
    /// went, so the outline draws the scroll-spy's heading again.
    pub(crate) fn poll_reading_keys(&mut self, now: f64) -> bool {
        let signal = self.scroll_signal();
        self.reading_keys.observe(signal, now)
    }

    /// Space and Shift+Space for the article itself (R-07).
    ///
    /// Rune's focus machine presses whatever non-input element holds the focus on
    /// Space, and a block clicked for word selection or Shift+F10 is one of those,
    /// so the window's key handler would never see the page keys. This handler sits
    /// on the article viewport: it runs before the focus machine and pages only
    /// while a rendered block owns the focus. A focused link or another live
    /// control keeps its own Space.
    pub(crate) fn article_space(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if !matches!(reading_key_of(event), Some(ReadingKey::PageDown | ReadingKey::PageUp)) {
            return false;
        }
        if !tx.focused().is_none_or(block_key) || !self.reading_keys_active() {
            return false;
        }
        self.reading_key(event, tx)
    }
}

/// Whether a focused key names a rendered block (`b{i}`).
fn block_key(key: &str) -> bool {
    key.strip_prefix('b').is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|digit| digit.is_ascii_digit()))
}

/// The heading `.`/`,` steps to (R-08), as an index into `doc.headings`.
///
/// `list` is the outline's headings in document order and `tops` their last
/// measured tops. While a heading is pinned the step is simply its neighbour,
/// which is what lets the last headings of a document — the ones that can never
/// reach the top — be visited one by one. Without a pin the page decides: the
/// first heading below the top, or the last one above it. Before the first
/// frame has measured anything, the viewport's anchor stands in for the tops.
pub(crate) fn step_target(
    list: &[usize],
    pinned: Option<usize>,
    tops: &[(usize, f32)],
    anchor: Option<usize>,
    dir: i8,
) -> Option<usize> {
    if let Some(current) = pinned.and_then(|index| list.iter().position(|candidate| *candidate == index)) {
        return if dir > 0 { list.get(current + 1).copied() } else { current.checked_sub(1).map(|at| list[at]) };
    }
    if !tops.is_empty() {
        return if dir > 0 {
            tops.iter().find(|(_, top)| *top > STEP_BELOW_TOP).map(|(index, _)| *index)
        } else {
            tops.iter().rev().find(|(_, top)| *top < STEP_ABOVE_TOP).map(|(index, _)| *index)
        };
    }
    if dir > 0 {
        list.iter().copied().find(|index| anchor.is_none_or(|at| *index > at))
    } else {
        list.iter().copied().rev().find(|index| anchor.is_some_and(|at| *index < at))
    }
}

#[cfg(test)]
mod tests {
    use rune::{Key, KeyEvent, Mods};

    use super::*;

    fn key(key: Key) -> KeyEvent {
        KeyEvent { key, mods: Mods::default() }
    }

    fn shifted(key: Key) -> KeyEvent {
        KeyEvent { key, mods: Mods { shift: true, ..Mods::default() } }
    }

    #[test]
    fn the_reading_grammar_matches_the_original() {
        assert_eq!(reading_key_of(&key(Key::Text("j".to_owned()))), Some(ReadingKey::ScrollDown));
        assert_eq!(reading_key_of(&key(Key::Text("k".to_owned()))), Some(ReadingKey::ScrollUp));
        assert_eq!(reading_key_of(&key(Key::Text(".".to_owned()))), Some(ReadingKey::NextHeading));
        assert_eq!(reading_key_of(&key(Key::Text(",".to_owned()))), Some(ReadingKey::PreviousHeading));
        assert_eq!(reading_key_of(&key(Key::Text("g".to_owned()))), Some(ReadingKey::Top));
        assert_eq!(reading_key_of(&key(Key::Text("G".to_owned()))), Some(ReadingKey::End));
        assert_eq!(reading_key_of(&shifted(Key::Text("g".to_owned()))), Some(ReadingKey::End), "Shift+g as g+shift");
        assert_eq!(reading_key_of(&key(Key::Text(" ".to_owned()))), Some(ReadingKey::PageDown));
        assert_eq!(reading_key_of(&shifted(Key::Text(" ".to_owned()))), Some(ReadingKey::PageUp));

        assert_eq!(reading_key_of(&shifted(Key::Text("j".to_owned()))), None, "Shift+J is not j");
        assert_eq!(reading_key_of(&shifted(Key::Text(".".to_owned()))), None, "the original upper-cases it");
        assert_eq!(reading_key_of(&key(Key::Text("x".to_owned()))), None);
        assert_eq!(reading_key_of(&key(Key::PageDown)), None, "Rune pages PageDown itself");
        let command = KeyEvent { key: Key::Command('g'), mods: Mods { cmd: true, ctrl: true, ..Mods::default() } };
        assert_eq!(reading_key_of(&command), None);
        let ctrl_space = KeyEvent { key: Key::Text(" ".to_owned()), mods: Mods { ctrl: true, ..Mods::default() } };
        assert_eq!(reading_key_of(&ctrl_space), None);
    }

    #[test]
    fn presses_under_four_hundred_ms_add_up_from_the_previous_target() {
        let mut keys = ReadingKeys::default();
        assert_eq!(keys.scroll_from(0.0, 40.0), 40.0, "the first press starts at the page");
        keys.note_scroll(120.0, 0.0);
        assert_eq!(keys.scroll_from(0.2, 55.0), 120.0, "still inside the window: the target, not the glide");
        assert_eq!(keys.scroll_from(0.4, 55.0), 55.0, "400 ms later the page is the start again");
        keys.note_scroll(200.0, 1.0);
        assert_eq!(keys.scroll_from(1.399, 70.0), 200.0);
        assert_eq!(keys.scroll_from(1.401, 70.0), 70.0);
    }

    #[test]
    fn a_single_notch_keeps_rune_glide_and_a_burst_stiffens_it() {
        let mut keys = ReadingKeys::default();
        let first = keys.wheel_spring(1.0, -53.0);
        assert_eq!(first.duration, WHEEL_GLIDE_SECONDS, "a lone notch keeps Rune's glide smooth");

        // 53 points every 16.7 ms is about 3180 points a second.
        let mut now = 1.0;
        let mut burst = first;
        for _ in 0..12 {
            now += 1.0 / 60.0;
            burst = keys.wheel_spring(now, -53.0);
        }
        let lag = std::f32::consts::TAU * WHEEL_LAG / burst.duration;
        assert!((lag - 3180.0).abs() < 400.0, "a 60-notch burst holds its lag: {burst:?} carries {lag}");
        assert!(burst.duration < WHEEL_GLIDE_SECONDS, "the burst glides stiffer than the notch: {burst:?}");

        // A pause is a new gesture: soft again, and no speed left over.
        let soft = keys.wheel_spring(now + WHEEL_ACCUMULATE_SECONDS + 0.1, -53.0);
        assert_eq!(soft.duration, WHEEL_GLIDE_SECONDS);
        assert_eq!(keys.wheel_speed, 0.0);
    }

    #[test]
    fn the_spring_never_softens_past_rune_and_never_jumps() {
        assert_eq!(wheel_spring_for(0.0).duration, WHEEL_GLIDE_SECONDS);
        assert_eq!(wheel_spring_for(-10.0).duration, WHEEL_GLIDE_SECONDS, "a nonsense speed keeps the glide");
        assert_eq!(wheel_spring_for(f32::NAN).duration, WHEEL_GLIDE_SECONDS);
        assert_eq!(wheel_spring_for(1.0).duration, WHEEL_GLIDE_SECONDS, "a slow wheel is never softer than Rune's");
        assert_eq!(wheel_spring_for(1e6).duration, WHEEL_GLIDE_MIN_SECONDS, "a very fast one still glides");
        assert_eq!(wheel_spring_for(1e6).bounce, 0.0, "critically damped, as Rune's glide");
        let crossover = std::f32::consts::TAU * WHEEL_LAG / WHEEL_GLIDE_SECONDS;
        assert!((wheel_spring_for(crossover).duration - WHEEL_GLIDE_SECONDS).abs() < 1e-5);
    }

    #[test]
    fn a_second_g_inside_half_a_second_goes_top() {
        let mut keys = ReadingKeys::default();
        assert!(!keys.press_g(1.0));
        assert!(keys.press_g(1.499));
        assert_eq!(keys.last_g, None, "a completed g g starts over");
        assert!(!keys.press_g(2.0));
        assert!(!keys.press_g(2.5), "the window expired; this press opens a new one");
        assert!(keys.press_g(2.7));
    }

    #[test]
    fn a_pin_releases_on_a_hand_scroll_or_after_the_glide_settles() {
        let doc = lmdv_md::parse("# One\n\n## Two\n");
        let mut keys = ReadingKeys::default();
        keys.pin_heading(1, doc.revision, 10.0, ScrollSignal { report: 4, by_hand: false });
        assert_eq!(keys.pinned_index(&doc), Some(1));
        assert!(!keys.observe(ScrollSignal { report: 4, by_hand: true }, 10.01), "no new report");
        assert!(!keys.observe(ScrollSignal { report: 5, by_hand: false }, 10.2), "the glide's own report");
        assert_eq!(keys.pinned_index(&doc), Some(1));
        assert!(keys.observe(ScrollSignal { report: 6, by_hand: true }, 10.3), "a wheel releases at once");
        assert_eq!(keys.pinned_index(&doc), None);

        keys.pin_heading(0, doc.revision, 20.0, ScrollSignal { report: 9, by_hand: false });
        assert!(!keys.observe(ScrollSignal { report: 10, by_hand: false }, 20.2));
        assert!(keys.observe(ScrollSignal { report: 11, by_hand: false }, 20.5), "quiet for 250 ms: the reader's");
        assert_eq!(keys.pinned_index(&doc), None);
    }

    #[test]
    fn a_reparse_drops_the_pin() {
        let doc = lmdv_md::parse("# One\n\n## Two\n");
        let mut keys = ReadingKeys::default();
        keys.pin_heading(0, doc.revision, 0.0, ScrollSignal::default());
        let newer = lmdv_md::parse_with_revision("# One\n\n## Two\n", 7);
        assert_eq!(keys.pinned_index(&doc), Some(0));
        assert_eq!(keys.pinned_index(&newer), None);
    }

    #[test]
    fn the_step_walks_the_outline_and_stops_at_its_ends() {
        let list = [0usize, 2, 5];
        let tops = [(0, 40.0), (2, 400.0), (5, 1200.0)];
        assert_eq!(step_target(&list, None, &tops, None, 1), Some(0), "from the top: the first heading");
        assert_eq!(step_target(&list, None, &tops, None, -1), None, "nothing above");
        assert_eq!(step_target(&list, Some(0), &tops, None, 1), Some(2));
        assert_eq!(step_target(&list, Some(2), &tops, None, 1), Some(5));
        assert_eq!(step_target(&list, Some(5), &tops, None, 1), None, "past the last");
        assert_eq!(step_target(&list, Some(5), &tops, None, -1), Some(2));

        let scrolled = [(0, -900.0), (2, -300.0), (5, -5.0)];
        assert_eq!(step_target(&list, None, &scrolled, None, 1), None, "all above the top");
        assert_eq!(step_target(&list, None, &scrolled, None, -1), Some(5), "the last one above the top");

        let below = [(0, -900.0), (2, 400.0), (5, 1200.0)];
        assert_eq!(step_target(&list, None, &below, None, 1), Some(2));

        assert_eq!(step_target(&list, None, &[], Some(2), 1), Some(5), "the anchor stands in before a frame");
        assert_eq!(step_target(&list, None, &[], Some(2), -1), Some(0));
        assert_eq!(step_target(&list, None, &[], None, 1), Some(0));
        assert_eq!(step_target(&list, None, &[], None, -1), None);
        assert_eq!(step_target(&list, Some(1), &[], Some(2), 1), Some(5), "a stale pin falls back to the page");
    }
}
