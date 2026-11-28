//! X11 tab tear-out (T-20, P4-02c): a live-following window and the
//! cross-window drop, on top of P4-02b's transfer protocol.
//!
//! Rune's strip reports a torn-out tab on release as
//! [`TabTearOut`](rune_kit::shell::TabTearOut) with the pointer in physical
//! desktop pixels (`vendor/rune/crates/rune-kit/src/shell/tab_strip/tab.rs:579-593`).
//! `screen_point` is `None` headless and on Wayland, where the system hides
//! window positions (`vendor/rune/crates/rune/src/el/handlers.rs:170-176`), so
//! the strip gets no `TearOutSpec` there at all and the tab menu stays the
//! substitute (`docs/ARCHITECTURE_DETAIL.md` §7.2).
//!
//! Where screen points exist:
//!
//! * [`WindowState::handle_tear_move`] remembers where the press landed in the
//!   tab and posts a [`TransferMsg::Hover`] to the window under the pointer, so
//!   its strip opens the incoming gap (`TabDragState::set_incoming`);
//! * [`WindowState::handle_tear_out`] hit-tests the release against the cached
//!   X11 placements of the other windows. Over another window, the tab travels
//!   through `move_tab_to_window_at`; over none, a new window opens with its
//!   frame at [`tear_out_position`] and adopts the tab.
//!
//! OI-125: the new window maps on the first torn-out move and follows the
//! pointer, and docking the tab again closes it without losing the tab. The
//! tab itself still travels on release: a live window opens with a start page
//! and the dragged tab adopts it (or another window) on mouse-up. This keeps
//! the transfer protocol lossless — there is no way to ask another window to
//! send a tab back mid-drag. T-21 (dragging a lone tab to move its window)
//! has no Rune `drag_window` API, so [`tear_out_spec`] keeps
//! `TearOutSpec::lone` false and the **Move to a new window** menu covers it.

use std::rc::Rc;

use lmdv_core::WindowId;
use lmdv_io::Launch;
use rune::WindowPlacement;
use rune_kit::shell::{TabTearMove, TabTearOut, TearOutSpec};

use crate::shared::UiMsg;
use crate::transfer::{TransferMsg, TransferToken};
use crate::window::{Tab, WindowState, parse_tab_key};
use crate::windows::{WindowKind, WindowOpenRequest};

/// The strip's tear-out setting. Only where the platform reports global screen
/// points: Wayland and headless keep `None`, so a drag can never detach a tab
/// there and the tab context menu remains the substitute (T-20/T-21).
///
/// `lone` stays false: a lone tab is a window move in the original (T-21), and
/// Rune has no hook for that without a patch.
pub(crate) fn tear_out_spec(screen_points: bool) -> Option<TearOutSpec> {
    screen_points.then(TearOutSpec::default)
}

/// The physical frame top-left for a window made by tearing a tab out.
///
/// The window's content top-left is the release point minus the point in the
/// tab where the pointer grabbed it (`grab`, in points at the source's scale);
/// the frame's top-left then lies out by the source's decoration inset
/// (`content_position - position`). Without a source placement there is no
/// scale or decoration to apply, so the grab is taken as physical pixels. The
/// result is clamped to the source display so a drop near an edge opens the
/// window on that display rather than across it (`frame_size` is the new
/// window's physical frame size).
pub(crate) fn tear_out_position(
    screen_point: (i32, i32),
    grab: [f32; 2],
    source: Option<WindowPlacement>,
    frame_size: (u32, u32),
) -> (i32, i32) {
    let (scale, decoration) = match source {
        Some(placement) => (
            positive_scale(placement.scale),
            (
                placement.content_position.0.saturating_sub(placement.position.0),
                placement.content_position.1.saturating_sub(placement.position.1),
            ),
        ),
        None => (1.0, (0, 0)),
    };
    let x = screen_point.0.saturating_sub(scaled(grab[0], scale)).saturating_sub(decoration.0);
    let y = screen_point.1.saturating_sub(scaled(grab[1], scale)).saturating_sub(decoration.1);
    match source.and_then(|placement| placement.display) {
        Some((display, size)) => {
            (clamp_axis(x, display.0, size.0, frame_size.0), clamp_axis(y, display.1, size.1, frame_size.1))
        }
        None => (x, y),
    }
}

/// Whether the physical desktop `point` lies inside the window's frame.
///
/// `WindowPlacement::position` is the outer top-left and `content_size` the
/// content's size; the decorations repeat on the far edges, so the frame's
/// extent is the content's plus the same inset on each side.
pub(crate) fn placement_contains(placement: WindowPlacement, point: (i32, i32)) -> bool {
    let (x, y, width, height) = frame_extent(placement);
    point.0 >= x && point.1 >= y && point.0 < x.saturating_add(width) && point.1 < y.saturating_add(height)
}

/// A physical desktop point in a window's points, for the target strip's
/// incoming gap. Content-relative, like every window point Rune hands a view.
pub(crate) fn point_in_window(placement: WindowPlacement, point: (i32, i32)) -> [f32; 2] {
    let scale = positive_scale(placement.scale) as f32;
    [(point.0 - placement.content_position.0) as f32 / scale, (point.1 - placement.content_position.1) as f32 / scale]
}

/// The frame rectangle a hit test uses: `(x, y, width, height)` in physical
/// pixels. The outer top-left, then the content extent plus the decoration
/// inset on both far edges.
fn frame_extent(placement: WindowPlacement) -> (i32, i32, i32, i32) {
    let inset_x = placement.content_position.0.saturating_sub(placement.position.0).max(0);
    let inset_y = placement.content_position.1.saturating_sub(placement.position.1).max(0);
    let content_width = i32::try_from(placement.content_size.0).unwrap_or(i32::MAX);
    let content_height = i32::try_from(placement.content_size.1).unwrap_or(i32::MAX);
    let width = content_width.saturating_add(inset_x.saturating_mul(2));
    let height = content_height.saturating_add(inset_y.saturating_mul(2));
    (placement.position.0, placement.position.1, width, height)
}

/// Clamps one frame axis into a display, keeping the whole new window on it
/// when the window fits and the frame's top-left at the display origin when it
/// does not.
fn clamp_axis(value: i32, display_origin: i32, display_len: u32, frame_len: u32) -> i32 {
    let display_len = i32::try_from(display_len).unwrap_or(i32::MAX);
    let frame_len = i32::try_from(frame_len).unwrap_or(i32::MAX);
    let room = display_len.saturating_sub(frame_len).max(0);
    value.clamp(display_origin, display_origin.saturating_add(room))
}

/// The physical frame size a new window opens at: the app's default content
/// size at the source's scale. Only the clamp uses it.
fn default_frame_size(scale: f64) -> (u32, u32) {
    let (width, height) = crate::app::WINDOW_SIZE;
    let scale = positive_scale(scale);
    ((f64::from(width) * scale).round().max(1.0) as u32, (f64::from(height) * scale).round().max(1.0) as u32)
}

fn positive_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 { scale } else { 1.0 }
}

fn scaled(points: f32, scale: f64) -> i32 {
    (f64::from(points) * scale).round() as i32
}

/// The window a torn tab last hovered over, with its drag token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TearHover {
    target: WindowId,
    token: TransferToken,
}

/// One window's half of a tear-out: the press's grab point and the hover it
/// last sent. One gesture at a time, like [`TabDragState`](rune_kit::shell::TabDragState).
#[derive(Debug, Default)]
pub(crate) struct TearState {
    /// Where the press landed in the tab, in points (`TabTearMove::grab`). The
    /// release reports no grab, so the last move keeps it for placement.
    grab: Option<[f32; 2]>,
    hover: Option<TearHover>,
    /// The window that follows the pointer once the tab is torn out (OI-125).
    live: Option<WindowId>,
}

impl TearState {
    /// The window following the pointer, for tests.
    #[cfg(test)]
    pub(crate) fn live_window(&self) -> Option<WindowId> {
        self.live
    }
}

impl WindowState {
    /// The strip's tear-out setting for this process: some only on X11.
    pub(crate) fn tear_out_spec(&self) -> Option<TearOutSpec> {
        tear_out_spec(self.shared().screen_positions())
    }

    /// The pointer moved a torn-out tab (`TabStrip::on_tear_move`).
    ///
    /// Remembers the grab point and shows the window under the pointer its
    /// incoming gap. The message carries only a copy point: the target's strip
    /// rebuilds on its own next frame. A reattaching move (`torn` false) clears
    /// the hover, as does leaving every window. A vanished target is a dropped
    /// message, never a block.
    pub(crate) fn handle_tear_move(&mut self, tear: TabTearMove) -> bool {
        if !tear.torn {
            // OI-125: docking back into the strip closes the floating window.
            // It holds only a start page until the release, so no tab is lost.
            let closed = self.close_live_tear_window();
            self.tear.grab = None;
            return self.clear_tear_hover() || closed;
        }
        self.tear.grab = Some(tear.grab);
        let Some(screen) = tear.screen_point else { return self.clear_tear_hover() };
        let source = self.shared.window_placement(self.id());
        if let Some(live) = self.tear.live {
            let scale = source.map_or(1.0, |placement| positive_scale(placement.scale));
            let position = tear_out_position(screen, tear.grab, source, default_frame_size(scale));
            self.shared.move_window(live, position);
        } else if self.can_start_live_tear(&tear) {
            let scale = source.map_or(1.0, |placement| positive_scale(placement.scale));
            let position = tear_out_position(screen, tear.grab, source, default_frame_size(scale));
            let kind = self.tear_window_kind(&tear);
            let request = WindowOpenRequest { launch: Launch::default(), kind, position: Some(position) };
            if let Ok(opened) = Rc::clone(&self.shared).request_new_window(request) {
                self.tear.live = Some(opened.id);
                // The kit stops drawing the dragged tab; the floating window
                // stands in for it until it docks or the button comes up.
                self.drag.hand_off();
            }
        }
        let Some(target) = self.shared.window_at(screen, Some(self.id())) else {
            return self.clear_tear_hover();
        };
        let Some(placement) = self.shared.window_placement(target) else {
            return self.clear_tear_hover();
        };
        let token = match self.tear.hover {
            Some(hover) if hover.target == target => hover.token,
            _ => {
                // The pointer entered a new window: clear the previous gap.
                self.clear_tear_hover();
                self.shared.allocate_transfer()
            }
        };
        let hover = TearHover { target, token };
        let changed = self.tear.hover != Some(hover);
        self.tear.hover = Some(hover);
        let point = point_in_window(placement, screen);
        self.shared.post_window(target, UiMsg::Transfer(TransferMsg::Hover { token, target, point: Some(point) }));
        changed
    }

    /// The button came up on a torn-out tab (`TabStrip::on_tear_out`).
    ///
    /// Over another window, the tab is offered there through the P4-02b
    /// protocol; otherwise a new window opens at the release point and adopts
    /// it. No screen point (Wayland/headless) leaves the tab in place.
    pub(crate) fn handle_tear_out(&mut self, tear: TabTearOut) -> bool {
        let Some(tab) = parse_tab_key(&tear.id) else { return false };
        let grab = self.tear.grab.take().unwrap_or([0.0; 2]);
        // `TearOutSpec::lone` stays false: T-21's lone-tab window drag has no
        // Rune API, so the strip never reports its only tab and a synthetic
        // release is refused here too (the menu substitutes).
        if self.tabs.len() <= 1 || !self.can_move_tab(tab) {
            self.close_live_tear_window();
            return false;
        }
        let Some(screen) = tear.screen_point else {
            self.clear_tear_hover();
            self.close_live_tear_window();
            return false;
        };
        // Once per release, not per move: each native placement read is a
        // round-trip, and the WM may have moved a window since it was focused.
        self.shared.refresh_window_placements();
        let source = self.shared.window_placement(self.id());
        let hovered = self.tear.hover.map(|hover| hover.target);
        let live = self.tear.live.take();
        // OI-120: a start page tears out into a non-reusable window.
        let kind = if self.tab(tab).is_some_and(Tab::is_start) { WindowKind::TornOffStart } else { WindowKind::Normal };
        let target = self.shared.window_at(screen, Some(self.id()));
        if target.is_none() && live.is_none() {
            let scale = source.map_or(1.0, |placement| positive_scale(placement.scale));
            let position = tear_out_position(screen, grab, source, default_frame_size(scale));
            let request = WindowOpenRequest { launch: Launch::default(), kind, position: Some(position) };
            match Rc::clone(&self.shared).request_new_window(request) {
                Ok(opened) => {
                    if let Err(error) = self.move_tab_to_window_at(tab, opened.id, None) {
                        self.notice(lmdv_i18n::t!("msg.tearOffFailed", "err" => error));
                    }
                }
                Err(error) => self.notice(lmdv_i18n::t!("msg.tearOffFailed", "err" => error)),
            }
            return true;
        }
        // The floating window under the pointer is the destination; any other
        // live window is closed and the tab is offered where the button came up.
        if let (Some(other), Some(live)) = (target, live)
            && other != live
        {
            self.shared.close_window(live);
        }
        let Some(target) = target.or(live) else { return true };
        if hovered.is_some_and(|previous| previous != target) {
            // The pointer ended over a different window than the last hover:
            // close that strip's gap before offering the tab.
            self.clear_tear_hover();
        }
        match self.move_tab_to_window_at(tab, target, None) {
            Ok(()) => {
                // The target clears its incoming gap when it adopts.
                self.tear.hover = None;
                true
            }
            Err(error) => {
                self.clear_tear_hover();
                self.notice(error);
                true
            }
        }
    }

    /// Whether a floating window may start for this gesture: the tab is still
    /// here, it is not the strip's only one (T-21 has no native API), and it is
    /// movable right now.
    fn can_start_live_tear(&self, tear: &TabTearMove) -> bool {
        if self.tabs.len() <= 1 {
            return false;
        }
        parse_tab_key(&tear.id).and_then(|tab| self.tab(tab)).is_some_and(|tab| self.can_move_tab(tab.id))
    }

    /// A start page tears out into a `TornOffStart` window, which launch
    /// routing may not reuse as an idle target (OI-120).
    fn tear_window_kind(&self, tear: &TabTearMove) -> WindowKind {
        if parse_tab_key(&tear.id).and_then(|tab| self.tab(tab)).is_some_and(Tab::is_start) {
            WindowKind::TornOffStart
        } else {
            WindowKind::Normal
        }
    }

    /// Closes the window that followed the pointer. It holds only a start
    /// page until release, so a dock-back loses no tab (OI-125).
    pub(crate) fn close_live_tear_window(&mut self) -> bool {
        let Some(live) = self.tear.live.take() else { return false };
        self.shared.close_window(live);
        true
    }

    /// Clears the target's incoming gap (`Hover { point: None }`). Returns
    /// whether a window was hovered.
    pub(crate) fn clear_tear_hover(&mut self) -> bool {
        let Some(hover) = self.tear.hover.take() else { return false };
        self.shared.post_window(
            hover.target,
            UiMsg::Transfer(TransferMsg::Hover { token: hover.token, target: hover.target, point: None }),
        );
        true
    }

    /// Clears a hover left by a tear that ended without a release (Escape, a
    /// lost pointer): the strip reports only the release, so `poll` notices the
    /// gesture is gone and closes the target's gap.
    pub(crate) fn clear_stale_tear_hover(&mut self) -> bool {
        if self.drag.torn_out() {
            return false;
        }
        let hover = self.tear.hover.is_some() && self.clear_tear_hover();
        let live = self.close_live_tear_window();
        hover || live
    }
}
