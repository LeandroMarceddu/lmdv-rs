//! The full-window media preview (V-01–V-10): images and Mermaid diagrams.
//!
//! The renderer hands every drawn picture and every supported diagram to
//! [`media_wire`](WindowState::media_wire) as a [`MediaRef`]. The wire makes the
//! media a keyboard button (`Enter`/`Space`) and adds the shared hover hint;
//! clicking it opens one [`Lightbox`] for the window.
//!
//! The preview is a modal layer over the whole window: a dim backdrop, the
//! media fitted in a clipped stage, and a title bar with the image's name and
//! the alt text. The zoom bar offers `−`, the live level, `+`, **Fit**,
//! **Width** and **100%**; the keys follow V-03 (`+`/`=`, `-`/`_`, `0`, `W`,
//! `1`, arrows, `Esc`) and a plain wheel or `Ctrl`+wheel zooms about the
//! pointer (V-04). A drag pans, a double-click toggles fit and 100%, and a click
//! on the backdrop or the empty stage closes only when press and release land
//! together without a drag (V-05, V-10).
//!
//! V-06 semantics follow the original: `Fit` never upscales a raster past 1:1
//! but may enlarge a diagram to 4×; `Width` fits the window's width from the
//! top; `100%` is the natural size around the current centre. The zoom range is
//! `min(fit, 0.2)`–`8×` of the natural size, every button and key steps by
//! 1.25, and a resize re-fits `Fit` and `Width` while leaving a custom zoom
//! alone.
//!
//! Not implemented here: the copy/save menu and PNG rasterization (V-08/V-09)
//! and the original's pointer-hint avoidance around floating bars (V-02); the
//! hint is anchored inside the media's own corner instead.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use lmdv_core::{Document, DocumentId, DocumentKey, SourceBuffer, TabId};
use lmdv_render::{MediaKind, MediaRef, MediaWire, RenderOptions, RenderWires};
use rune::{Cursor, Cx, Drag, El, Key, KeyEvent, LiveRegion, Phase, Tx, WheelEvent, col, el, row, text};
use rune_kit::image_zoom::math::{clamp_axis, detail_scale};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit};

use crate::icons;
use crate::images::{ImageCacheState, WindowImages};
use crate::math::WindowMath;
use crate::window::WindowState;

/// The preview's modal root.
pub const LIGHTBOX_KEY: &str = "lightbox";
/// The clipped stage viewport that owns focus and the pan/zoom gestures.
pub const LIGHTBOX_VIEWPORT: &str = "lightbox-viewport";
/// The drawn media inside the viewport.
pub const LIGHTBOX_STAGE: &str = "lightbox-stage";
/// The title bar's close button.
pub const LIGHTBOX_CLOSE: &str = "lightbox-close";
/// The zoom bar's controls.
pub const LIGHTBOX_ZOOM_OUT: &str = "lightbox-zoom-out";
/// The live zoom level; pressing it fits.
pub const LIGHTBOX_ZOOM_LEVEL: &str = "lightbox-zoom-level";
/// Zoom in.
pub const LIGHTBOX_ZOOM_IN: &str = "lightbox-zoom-in";
/// Fit to window.
pub const LIGHTBOX_FIT: &str = "lightbox-fit";
/// Fit the window's width.
pub const LIGHTBOX_WIDTH: &str = "lightbox-width";
/// Natural size.
pub const LIGHTBOX_ACTUAL: &str = "lightbox-actual";

/// Breathing room around the stage, in points.
pub const MARGIN: f32 = 40.0;
/// Room the bottom bar takes from the fitted area.
pub const BAR_SPACE: f32 = 64.0;
/// Room the title bar takes from the fitted area.
pub const TITLE_SPACE: f32 = 48.0;
/// The smallest natural-size zoom the keyboard and buttons reach (V-06).
pub const SCALE_FLOOR: f32 = 0.2;
/// The largest zoom, as a multiple of the natural size (V-06).
pub const SCALE_MAX: f32 = 8.0;
/// One button or key step (V-03).
pub const STEP: f32 = 1.25;
/// A pointer must travel this far to count as a drag (V-05).
pub const DRAG_SLOP: f32 = 3.0;
/// One arrow-key pan, in points (V-03).
pub const PAN_STEP: f32 = 40.0;
/// Shift+arrow pan, in points (V-03).
pub const PAN_STEP_FAST: f32 = 160.0;

/// The view a [`Lightbox`] holds, shown pressed in the zoom bar (V-06).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LightboxMode {
    /// Whole content centred; a raster stops at 1:1, a diagram may reach 4×.
    #[default]
    Fit,
    /// The window's width, from the top.
    Width,
    /// The natural size around the current centre.
    Actual,
    /// A zoom or pan the user chose.
    Custom,
}

/// One open media preview.
#[derive(Debug)]
pub struct Lightbox {
    /// The tab that drew the media.
    pub tab: TabId,
    /// Its document.
    pub doc: DocumentId,
    /// The model revision the media came from.
    pub revision: u64,
    /// The media as drawn.
    pub media: MediaRef,
    /// The decoded picture, for an image.
    pub image: Option<rune::Image>,
    /// The title bar's name (the image's file name, or “Diagram”).
    pub title: String,
    /// The caption: the alt text, or the localized “Image”/“Diagram”.
    pub caption: String,
    /// The key that opened it; focus returns there when it closes.
    pub opener: String,
    mode: Cell<LightboxMode>,
    /// The absolute scale: 1.0 is the natural size.
    scale: Cell<f32>,
    /// The content centre's offset from the stage centre, in points.
    pan: Cell<[f32; 2]>,
    /// The last stage area `[x, y, w, h]` in window points.
    area: Cell<[f32; 4]>,
    /// The content's natural size in points.
    natural: Cell<[f32; 2]>,
    drag: Option<ViewportDrag>,
    dragged: bool,
    backdrop_dragged: bool,
}

#[derive(Clone, Copy, Debug)]
struct ViewportDrag {
    pan: [f32; 2],
    on_media: bool,
}

impl Lightbox {
    /// A preview for `media`, with the image handle for pictures.
    fn new(
        tab: TabId,
        doc: DocumentId,
        revision: u64,
        media: MediaRef,
        image: Option<rune::Image>,
        opener: String,
    ) -> Self {
        let caption = match media.kind {
            MediaKind::Image if !media.alt.trim().is_empty() => media.alt.trim().to_owned(),
            MediaKind::Image => lmdv_i18n::t!("zoom.image").to_owned(),
            MediaKind::Diagram => lmdv_i18n::t!("zoom.diagram").to_owned(),
        };
        let title = match media.kind {
            MediaKind::Image => image_name(&media.source),
            MediaKind::Diagram => lmdv_i18n::t!("zoom.diagram").to_owned(),
        };
        Self {
            tab,
            doc,
            revision,
            media,
            image,
            title,
            caption,
            opener,
            mode: Cell::new(LightboxMode::Fit),
            scale: Cell::new(1.0),
            pan: Cell::new([0.0, 0.0]),
            area: Cell::new([0.0, 0.0, 1.0, 1.0]),
            natural: Cell::new([1.0, 1.0]),
            drag: None,
            dragged: false,
            backdrop_dragged: false,
        }
    }

    /// The active view.
    pub fn mode(&self) -> LightboxMode {
        self.mode.get()
    }

    /// The current zoom, as a multiple of the natural size.
    pub fn scale(&self) -> f32 {
        self.scale.get()
    }

    /// The content's offset from the stage centre, in points.
    pub fn pan(&self) -> [f32; 2] {
        self.pan.get()
    }

    /// The zoom percentage shown in the bar and announced (`aria-live`).
    pub fn level(&self) -> u32 {
        (self.scale.get() * 100.0).round().clamp(1.0, 99_999.0) as u32
    }

    /// The fitted area `[x, y, w, h]` for a window body `[w, h]`.
    pub fn area_for(viewport: [f32; 2]) -> [f32; 4] {
        let w = (viewport[0] - 2.0 * MARGIN).max(80.0);
        let h = (viewport[1] - 2.0 * MARGIN - BAR_SPACE - TITLE_SPACE).max(80.0);
        let x = (viewport[0] - w) / 2.0;
        let y = TITLE_SPACE + ((viewport[1] - TITLE_SPACE - BAR_SPACE - h) / 2.0).max(0.0);
        [x, y, w, h]
    }

    /// The natural size used for the fit; `media` until the view measures the
    /// real drawing.
    fn natural(&self) -> [f32; 2] {
        let [w, h] = self.natural.get();
        [w.max(1.0), h.max(1.0)]
    }

    /// Fit may enlarge a diagram to 4× and a raster only to 1:1 (V-06).
    fn fit_cap(&self) -> f32 {
        if self.media.kind == MediaKind::Diagram { 4.0 } else { 1.0 }
    }

    /// The fit scale for the current area and natural size.
    pub fn fit_scale(&self) -> f32 {
        let [_, _, w, h] = self.area.get();
        let [nw, nh] = self.natural();
        (w / nw).min(h / nh).min(self.fit_cap()).max(f32::EPSILON)
    }

    /// The width scale: the window's width, capped like Fit (V-06).
    fn width_scale(&self) -> f32 {
        let [_, _, w, _] = self.area.get();
        let [nw, _] = self.natural();
        (w / nw).min(self.fit_cap()).max(f32::EPSILON)
    }

    /// The lower end of the zoom range: `min(fit, 0.2)` (V-06).
    fn min_scale(&self) -> f32 {
        self.fit_scale().min(SCALE_FLOOR)
    }

    /// Whether the media shows above its fitted size.
    pub fn zoomed(&self) -> bool {
        self.scale.get() > self.fit_scale() + 0.001
    }

    /// Applies the current mode to the measured area and natural size.
    ///
    /// The view calls this every frame, so a resize re-fits `Fit` and `Width`
    /// and clamps a custom pan, exactly as V-06 asks.
    fn settle(&self) {
        let [_, _, _, h] = self.area.get();
        let [_, nh] = self.natural();
        match self.mode.get() {
            LightboxMode::Fit => {
                self.scale.set(self.fit_scale());
                self.pan.set([0.0, 0.0]);
            }
            LightboxMode::Width => {
                let scale = self.width_scale();
                let sh = nh * scale;
                let ty = if sh > h { -(sh - h) / 2.0 } else { 0.0 };
                self.scale.set(scale);
                self.pan.set([0.0, clamp_axis(ty, sh, h)]);
            }
            LightboxMode::Actual => {
                self.scale.set(1.0);
                self.set_pan(self.pan.get());
            }
            LightboxMode::Custom => {
                self.set_pan(self.pan.get());
            }
        }
    }

    /// Clamps and stores a pan; true when it moved.
    fn set_pan(&self, pan: [f32; 2]) -> bool {
        let scale = self.scale.get();
        let [nw, nh] = self.natural();
        let [_, _, w, h] = self.area.get();
        let next = [clamp_axis(pan[0], nw * scale, w), clamp_axis(pan[1], nh * scale, h)];
        let moved = next != self.pan.get();
        self.pan.set(next);
        moved
    }

    /// Scales to `target` about a point measured from the stage centre.
    fn zoom_about(&self, target: f32, anchor: [f32; 2]) {
        let current = self.scale.get().max(f32::EPSILON);
        let next = target.clamp(self.min_scale(), SCALE_MAX);
        let factor = next / current;
        let pan = self.pan.get();
        self.scale.set(next);
        self.mode.set(LightboxMode::Custom);
        self.set_pan([(1.0 - factor) * anchor[0] + factor * pan[0], (1.0 - factor) * anchor[1] + factor * pan[1]]);
    }

    /// `+`/the button: one 1.25 step about the centre (V-03).
    pub fn zoom_in(&mut self) {
        self.zoom_about(self.scale.get() * STEP, [0.0, 0.0]);
    }

    /// `-`/the button.
    pub fn zoom_out(&mut self) {
        self.zoom_about(self.scale.get() / STEP, [0.0, 0.0]);
    }

    /// `0`/Fit: the whole content centred (V-06).
    pub fn fit(&mut self) {
        self.mode.set(LightboxMode::Fit);
        self.settle();
    }

    /// `W`/Width: the window's width from the top.
    pub fn width(&mut self) {
        self.mode.set(LightboxMode::Width);
        self.settle();
    }

    /// `1`/100%: the natural size about the current centre.
    pub fn actual(&mut self) {
        if (self.scale.get() - 1.0).abs() < 0.001 {
            self.mode.set(LightboxMode::Actual);
            self.settle();
        } else {
            self.zoom_about(1.0, [0.0, 0.0]);
            self.mode.set(LightboxMode::Actual);
        }
    }

    /// Arrow keys: pan by `step` points, clamped to the content's edges (V-03).
    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        let pan = self.pan.get();
        if self.set_pan([pan[0] + dx, pan[1] + dy]) {
            self.mode.set(LightboxMode::Custom);
        }
    }

    /// A double-click: 100% about the pointer, or back to Fit (V-06).
    fn toggle_detail(&mut self, anchor: [f32; 2]) {
        if self.mode.get() == LightboxMode::Fit {
            let fit = self.fit_scale();
            self.zoom_about(detail_scale(fit) * fit, anchor);
        } else {
            self.fit();
        }
    }

    /// The media's rect inside the viewport, `[x, y, w, h]` in viewport points.
    fn media_rect(&self) -> [f32; 4] {
        let [_, _, w, h] = self.area.get();
        let [nw, nh] = self.natural();
        let scale = self.scale.get();
        let [sw, sh] = [nw * scale, nh * scale];
        let [px, py] = self.pan.get();
        [(w - sw) / 2.0 + px, (h - sh) / 2.0 + py, sw, sh]
    }

    fn hits_media(&self, point: [f32; 2]) -> bool {
        let [x, y, w, h] = self.media_rect();
        point[0] >= x && point[0] <= x + w && point[1] >= y && point[1] <= y + h
    }

    /// The viewer state a wheel event asks for (V-04): `exp(-dy·0.0015)`,
    /// bounded to 0.5–2 per event, about the pointer.
    fn wheel_zoom(&mut self, dy: f32, anchor: [f32; 2]) {
        let factor = (-dy * 0.0015).exp().clamp(0.5, 2.0);
        self.zoom_about(self.scale.get() * factor, anchor);
    }

    /// A stage press. True when a press-and-release pair on empty space asks to
    /// close (V-10).
    fn viewport_drag(&mut self, drag: &Drag) -> bool {
        let point = [drag.x, drag.y];
        match drag.phase {
            Phase::Down => {
                self.drag = Some(ViewportDrag { pan: self.pan.get(), on_media: self.hits_media(point) });
                self.dragged = false;
                false
            }
            Phase::Move => {
                let Some(press) = self.drag else { return false };
                if drag.dx.hypot(drag.dy) > DRAG_SLOP {
                    self.dragged = true;
                }
                if self.dragged && self.set_pan([press.pan[0] + drag.dx, press.pan[1] + drag.dy]) {
                    self.mode.set(LightboxMode::Custom);
                }
                false
            }
            Phase::Up => {
                let Some(press) = self.drag.take() else { return false };
                if drag.cancelled() {
                    return false;
                }
                let on_media = self.hits_media(point);
                if !self.dragged && drag.clicks == 2 && on_media {
                    let [_, _, w, h] = self.area.get();
                    let anchor = [point[0] - w / 2.0, point[1] - h / 2.0];
                    self.toggle_detail(anchor);
                }
                !self.dragged && !press.on_media && !on_media
            }
        }
    }
}

/// The file name in an image source, stripped of its query/fragment; the
/// localized “Image” when it has none.
fn image_name(source: &str) -> String {
    let path = source.split(['#', '?']).next().unwrap_or(source);
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| lmdv_i18n::t!("zoom.image").to_owned())
}

impl WindowState {
    /// One media handler per frame: the article's images and diagrams become
    /// focusable buttons that open this window's preview (V-01).
    pub(crate) fn media_wire(&self, kit: &Kit) -> MediaWire {
        let Some(tab) = self.active_tab() else {
            return Rc::new(|element, _| element);
        };
        let tab_id = tab.id;
        let Some(doc) = tab.document_id() else {
            return Rc::new(|element, _| element);
        };
        let revision = tab.view.reader.as_ref().map_or(0, |model| model.revision);
        let kit = kit.clone();
        Rc::new(move |element, media| media_button(element, media, tab_id, doc, revision, &kit))
    }

    /// The open media preview.
    pub fn lightbox(&self) -> Option<&Lightbox> {
        self.lightbox.as_ref()
    }

    /// The media a top-level block draws, for the block context menu's
    /// “Open preview” item (V-01).
    pub(crate) fn media_for_block(&self, block: &str) -> Option<MediaRef> {
        media_refs(self).into_iter().find(|media| media.block == block)
    }

    /// Opens the preview for the media a top-level block draws.
    pub(crate) fn open_media_block(&mut self, block: &str, tx: &mut Tx) {
        let Some(media) = self.media_for_block(block) else { return };
        let Some(tab) = self.active_tab() else { return };
        let (tab_id, Some(doc)) = (tab.id, tab.document_id()) else { return };
        let revision = tab.view.reader.as_ref().map_or(0, |model| model.revision);
        self.open_media(tab_id, doc, revision, media, tx);
    }

    /// Opens the preview for `media` in `tab`, if that tab still shows the
    /// model revision the media was drawn from.
    ///
    /// A picture the path policy refuses or the decoder failed does not open
    /// (V-01); a pending decode opens and draws once its pixels land.
    pub(crate) fn open_media(&mut self, tab: TabId, doc: DocumentId, revision: u64, media: MediaRef, tx: &mut Tx) {
        if self.lightbox.is_some() {
            return;
        }
        let Some(active) = self.active_tab() else { return };
        if active.id != tab
            || active.document_id() != Some(doc)
            || active.view.reader.as_ref().map_or(0, |model| model.revision) != revision
        {
            return;
        }
        let image = match media.kind {
            MediaKind::Image => {
                let Some(document) = active.document().cloned() else { return };
                let Some(root) = self.document_root(&document) else { return };
                let cache = self
                    .image_cell()
                    .get_or_init(|| ImageCacheState::new(self.window_waker(), self.shared.is_headless()));
                let Some(image) = cache.resolve(&root, &media.source, media.width.max(1.0), 2.0) else {
                    return;
                };
                if image.error().is_some() {
                    return;
                }
                Some(image)
            }
            MediaKind::Diagram => {
                if media.source.trim().is_empty() {
                    return;
                }
                None
            }
        };
        let opener = format!("media-{}", media.block);
        self.lightbox = Some(Lightbox::new(tab, doc, revision, media, image, opener));
        tx.focus(LIGHTBOX_VIEWPORT, false);
    }

    /// Closes the preview. Focus returns to the media button that opened it.
    pub(crate) fn close_lightbox(&mut self) {
        self.lightbox = None;
    }

    /// The preview's keys (V-03): `+`/`=`, `-`/`_`, `0`, `W`, `1`, arrows,
    /// `Esc`. All other app shortcuts are swallowed while it is modal.
    pub(crate) fn lightbox_key(&mut self, event: &KeyEvent, _: &mut Tx) -> bool {
        if event.key == Key::Escape {
            self.close_lightbox();
            return true;
        }
        let Some(lightbox) = self.lightbox.as_mut() else { return false };
        let plain = !event.mods.ctrl && !event.mods.cmd && !event.mods.alt && !event.mods.logo;
        if !plain {
            // Modal: the app's Ctrl/Cmd shortcuts must not act through it.
            return event.mods.ctrl || event.mods.cmd || matches!(event.key, Key::Function(_));
        }
        let pan = if event.mods.shift { PAN_STEP_FAST } else { PAN_STEP };
        match &event.key {
            Key::Text(value) if value == "+" || value == "=" => lightbox.zoom_in(),
            Key::Text(value) if value == "-" || value == "_" => lightbox.zoom_out(),
            Key::Text(value) if value == "0" => lightbox.fit(),
            Key::Text(value) if value.eq_ignore_ascii_case("w") => lightbox.width(),
            Key::Text(value) if value == "1" => lightbox.actual(),
            Key::Left => lightbox.pan_by(pan, 0.0),
            Key::Right => lightbox.pan_by(-pan, 0.0),
            Key::Up => lightbox.pan_by(0.0, pan),
            Key::Down => lightbox.pan_by(0.0, -pan),
            Key::Function(_) => return true,
            _ => return false,
        }
        true
    }

    /// Wheel over the stage zooms about the pointer and never reaches the
    /// article (V-04). The root consumes wheel outside the stage too.
    pub(crate) fn lightbox_wheel(&mut self, event: WheelEvent, over_stage: bool) -> bool {
        if let Some(lightbox) = self.lightbox.as_mut()
            && over_stage
        {
            let [_, _, w, h] = lightbox.area.get();
            lightbox.wheel_zoom(event.dy, [event.x - w / 2.0, event.y - h / 2.0]);
        }
        true
    }

    /// A press on the stage: pan, empty-space close, double-click.
    pub(crate) fn lightbox_viewport_drag(&mut self, drag: &Drag, tx: &mut Tx) -> bool {
        if drag.phase == Phase::Down {
            tx.focus(LIGHTBOX_VIEWPORT, false);
        }
        let Some(lightbox) = self.lightbox.as_mut() else { return false };
        let close = lightbox.viewport_drag(drag);
        if close {
            self.lightbox = None;
        }
        true
    }

    /// Backdrop press: closes only when press and release stay put (V-10).
    pub(crate) fn lightbox_backdrop_drag(&mut self, drag: &Drag) -> bool {
        let Some(lightbox) = self.lightbox.as_mut() else { return false };
        match drag.phase {
            Phase::Down => lightbox.backdrop_dragged = false,
            Phase::Move => {
                if drag.dx.hypot(drag.dy) > DRAG_SLOP {
                    lightbox.backdrop_dragged = true;
                }
                return true;
            }
            Phase::Up => {
                if !lightbox.backdrop_dragged && !drag.cancelled() {
                    self.lightbox = None;
                }
                return true;
            }
        }
        true
    }

    /// Closes a preview whose tab, document, revision or picture no longer
    /// matches; also called after an image decode fails.
    pub(crate) fn poll_lightbox(&mut self) -> bool {
        let stale = self.lightbox.as_ref().is_some_and(|lightbox| {
            self.active_tab().is_none_or(|tab| {
                tab.id != lightbox.tab
                    || tab.document_id() != Some(lightbox.doc)
                    || tab.view.reader.as_ref().map_or(0, |model| model.revision) != lightbox.revision
            }) || lightbox.image.as_ref().is_some_and(|image| image.error().is_some())
        });
        if stale {
            self.lightbox = None;
        }
        stale
    }
}

/// Wraps one drawn media element in the open handler and hover hint.
fn media_button(element: El, media: &MediaRef, tab: TabId, doc: DocumentId, revision: u64, kit: &Kit) -> El {
    let open_label = match media.kind {
        MediaKind::Image => {
            let alt = if media.alt.trim().is_empty() {
                lmdv_i18n::t!("zoom.image").to_owned()
            } else {
                media.alt.trim().to_owned()
            };
            lmdv_i18n::t!("zoom.imageOpen", "alt" => alt)
        }
        MediaKind::Diagram => lmdv_i18n::t!("zoom.diagramOpen").to_owned(),
    };
    let key = format!("media-{}", media.block);
    let hint_key = format!("media-hint-{}", media.block);
    let wrap_key = format!("media-box-{}", media.block);
    let press_media = media.clone();
    let key_media = media.clone();
    let hint_media = media.clone();
    let main = element
        .key(key)
        .role("button")
        .label(open_label)
        .focusable(true)
        .cursor(Cursor::Pointer)
        .on_press_tx(move |state: &mut WindowState, tx: &mut Tx| {
            state.open_media(tab, doc, revision, press_media.clone(), tx);
        })
        .on_key(move |state: &mut WindowState, event: &KeyEvent, tx: &mut Tx| {
            let activate = event.key == Key::Enter || matches!(&event.key, Key::Text(value) if value == " ");
            if activate {
                state.open_media(tab, doc, revision, key_media.clone(), tx);
            }
            activate
        });
    let hint = IconButton::new(&icons::EXPAND)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .label(lmdv_i18n::t!("zoom.open"))
        .help(lmdv_i18n::t!("zoom.open"))
        .on_press_tx(move |state: &mut WindowState, tx: &mut Tx| {
            state.open_media(tab, doc, revision, hint_media.clone(), tx);
        })
        .build(kit)
        .key(hint_key)
        .focusable(false)
        .a11y_hidden();
    col()
        .key(wrap_key)
        .items_start()
        .hover_group()
        .child(main)
        .child(hint.pin(None, Some(8.0), Some(8.0), None).group_opacity(0.0, 1.0))
}

/// The preview's modal layer, over the whole window.
pub(crate) fn overlay(state: &WindowState, cx: &mut Cx<WindowState>, kit: &Kit, layer: &mut Vec<El>) {
    let Some(lightbox) = state.lightbox() else { return };
    let [width, height] =
        cx.layout_size("kit-root").or_else(|| cx.layout_size("window-body")).unwrap_or([1100.0, 760.0]);
    let area = Lightbox::area_for([width, height]);
    lightbox.area.set(area);

    // Measure the content. A diagram re-renders at the window's width, as the
    // original does, so a wide flowchart is readable instead of column-sized.
    let mut diagram = None;
    let natural = match lightbox.media.kind {
        MediaKind::Image => lightbox
            .image
            .as_ref()
            .and_then(rune::Image::size)
            .map(|[w, h]| [w as f32, h as f32])
            .unwrap_or([lightbox.media.width.max(1.0), lightbox.media.height.max(1.0)]),
        MediaKind::Diagram => {
            let look = state.current_appearance(cx.system_dark());
            let max_width = (width - 2.0 * MARGIN).max(200.0);
            match rune_kit::mermaid::drawing(&lightbox.media.source, &look.style, max_width) {
                Some(drawing) => {
                    let size = drawing.size;
                    diagram = Some(drawing.el);
                    size
                }
                None => [lightbox.media.width.max(1.0), lightbox.media.height.max(1.0)],
            }
        }
    };
    lightbox.natural.set(natural);
    lightbox.settle();
    let scale = lightbox.scale();
    let pan = lightbox.pan();
    let [ax, ay, aw, ah] = area;
    let [nw, nh] = natural;
    let [sw, sh] = [(nw * scale).max(1.0), (nh * scale).max(1.0)];
    let left = (aw - sw) / 2.0 + pan[0];
    let top = (ah - sh) / 2.0 + pan[1];

    let mut stage =
        el().key(LIGHTBOX_STAGE).absolute(left, top).size(sw, sh).role("img").label(lightbox.caption.clone());
    match lightbox.media.kind {
        MediaKind::Image => {
            let mut picture = el().key("lightbox-image").w_full().h_full().a11y_hidden();
            if let Some(image) = &lightbox.image
                && image.is_ready()
            {
                picture = picture.pixels(image.pixels());
            }
            stage = stage.child(picture);
        }
        MediaKind::Diagram => {
            if let Some(diagram) = diagram {
                stage = stage.child(
                    el().absolute((sw - nw) / 2.0, (sh - nh) / 2.0)
                        .size(nw, nh)
                        .child(diagram.scale(scale).a11y_hidden()),
                );
            }
        }
    }

    let cursor = if lightbox.dragged {
        Cursor::Grabbing
    } else if lightbox.zoomed() {
        Cursor::Grab
    } else {
        Cursor::Default
    };
    let viewport = el()
        .key(LIGHTBOX_VIEWPORT)
        .absolute(ax, ay)
        .size(aw, ah)
        .clip_soft(0.0)
        .focusable(true)
        .no_focus_ring()
        .role("group")
        .label(lmdv_i18n::t!("zoom.viewport"))
        .cursor(cursor)
        .on_wheel(|state: &mut WindowState, event, _: &mut Tx| state.lightbox_wheel(event, true))
        .on_key(|state: &mut WindowState, event: &KeyEvent, tx: &mut Tx| state.lightbox_key(event, tx))
        .on_drag(|state: &mut WindowState, drag, tx: &mut Tx| {
            state.lightbox_viewport_drag(drag, tx);
        })
        .child(stage);

    let scrim =
        el().key("lightbox-scrim").absolute(0.0, 0.0).w_full().h_full().fill(scrim_color()).a11y_hidden().on_drag(
            |state: &mut WindowState, drag, _: &mut Tx| {
                state.lightbox_backdrop_drag(drag);
            },
        );

    let close = IconButton::new(kit.glyph.close)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .label(lmdv_i18n::t!("zoom.close"))
        .help(lmdv_i18n::t!("zoom.close"))
        .on_press(|state: &mut WindowState| state.close_lightbox())
        .build(kit)
        .key(LIGHTBOX_CLOSE);
    let title_bar = row()
        .key("lightbox-title")
        .pin(Some(16.0), Some(12.0), Some(16.0), None)
        .h(40.0)
        .px(14.0)
        .gap(10.0)
        .center_y()
        .radius(8.0)
        .fill(kit.c.layer_01)
        .border(1.0, kit.c.border_base)
        .child(
            text(lightbox.title.clone())
                .key("lightbox-title-text")
                .font(kit.ty.ui_medium)
                .color(kit.c.text_base)
                .truncate()
                .grow(1.0),
        )
        .child(
            text(lightbox.caption.clone())
                .key("lightbox-caption")
                .font(kit.ty.small)
                .color(kit.c.text_muted)
                .truncate(),
        )
        .child(close);

    let bar = zoom_bar(lightbox, kit);
    let bottom =
        row().key("lightbox-bar-layer").pin(Some(0.0), None, Some(0.0), Some(12.0)).justify_center().child(bar);

    let root = el()
        .key(LIGHTBOX_KEY)
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .role("dialog")
        .label(lmdv_i18n::t!("zoom.dialog"))
        .modal_tx(false, |state: &mut WindowState, _: &mut Tx| state.close_lightbox())
        .focus_return(lightbox.opener.clone())
        .on_key(|state: &mut WindowState, event: &KeyEvent, tx: &mut Tx| state.lightbox_key(event, tx))
        .on_wheel(|state: &mut WindowState, event, _: &mut Tx| state.lightbox_wheel(event, false))
        .child(scrim)
        .child(viewport)
        .child(title_bar)
        .child(bottom);
    layer.push(root);
}

/// `−`, the live level, `+`, then Fit / Width / 100%.
fn zoom_bar(lightbox: &Lightbox, kit: &Kit) -> El {
    let scale = lightbox.scale();
    let mode = lightbox.mode();
    let min = lightbox.min_scale();
    let level_text = format!("{}%", lightbox.level());
    let level = Button::new(lmdv_i18n::t!("zoom.fit"))
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .help(lmdv_i18n::t!("zoom.fit"))
        .label_with(move |font, ink| {
            text(level_text.clone())
                .key("lightbox-zoom-level-text")
                .font(font)
                .color(ink)
                .live_region(LiveRegion::Polite)
        })
        .on_press_tx(|state: &mut WindowState, _: &mut Tx| {
            if let Some(lightbox) = state.lightbox.as_mut() {
                lightbox.fit();
            }
        })
        .build(kit)
        .key(LIGHTBOX_ZOOM_LEVEL)
        .min_w(58.0);
    let divider = || el().size(1.0, 16.0).mx(2.0).fill(kit.c.border_base).a11y_hidden();
    let mode_button = |key: &'static str, title: String, label: String, on: bool, action: fn(&mut Lightbox)| {
        Button::new(label)
            .variant(ButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .help(title)
            .on_press_tx(move |state: &mut WindowState, _: &mut Tx| {
                if let Some(lightbox) = state.lightbox.as_mut() {
                    action(lightbox);
                }
            })
            .build(kit)
            .key(key)
            .checked(on)
            .min_w(52.0)
    };
    row()
        .key("lightbox-bar")
        .padding(2.0)
        .gap(2.0)
        .center_y()
        .radius(8.0)
        .fill(kit.c.layer_01)
        .border(1.0, kit.c.border_base)
        .shadow(8.0, 24.0, rune::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.28 })
        .child(
            IconButton::new(kit.glyph.dash)
                .variant(IconButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .label(lmdv_i18n::t!("zoom.out"))
                .help(lmdv_i18n::t!("zoom.out"))
                .disabled(scale <= min + 0.001)
                .on_press_tx(|state: &mut WindowState, _: &mut Tx| {
                    if let Some(lightbox) = state.lightbox.as_mut() {
                        lightbox.zoom_out();
                    }
                })
                .build(kit)
                .key(LIGHTBOX_ZOOM_OUT),
        )
        .child(level)
        .child(
            IconButton::new(&icons::PLUS)
                .variant(IconButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .label(lmdv_i18n::t!("zoom.in"))
                .help(lmdv_i18n::t!("zoom.in"))
                .disabled(scale >= SCALE_MAX - 0.001)
                .on_press_tx(|state: &mut WindowState, _: &mut Tx| {
                    if let Some(lightbox) = state.lightbox.as_mut() {
                        lightbox.zoom_in();
                    }
                })
                .build(kit)
                .key(LIGHTBOX_ZOOM_IN),
        )
        .child(divider())
        .child(mode_button(
            LIGHTBOX_FIT,
            lmdv_i18n::t!("zoom.fit").to_owned(),
            lmdv_i18n::t!("zoom.fitLabel").to_owned(),
            mode == LightboxMode::Fit,
            |lightbox| lightbox.fit(),
        ))
        .child(mode_button(
            LIGHTBOX_WIDTH,
            lmdv_i18n::t!("zoom.width").to_owned(),
            lmdv_i18n::t!("zoom.widthLabel").to_owned(),
            mode == LightboxMode::Width,
            |lightbox| lightbox.width(),
        ))
        .child(mode_button(
            LIGHTBOX_ACTUAL,
            lmdv_i18n::t!("zoom.actual").to_owned(),
            lmdv_i18n::t!("zoom.actualLabel").to_owned(),
            mode == LightboxMode::Actual,
            |lightbox| lightbox.actual(),
        ))
}

// ---- the headless scene ---------------------------------------------------

/// Every media reference the active reader draws, in reading order.
///
/// It renders the article once with a recording media wire, so scenes and tests
/// can open exactly what the view draws without duplicating renderer markup.
pub(crate) fn media_refs(state: &WindowState) -> Vec<MediaRef> {
    let Some(tab) = state.active_tab() else { return Vec::new() };
    let Some(document) = tab.document() else { return Vec::new() };
    let Some(model) = tab.view.reader.clone() else { return Vec::new() };
    let root = state.document_root(document);
    let look = state.current_appearance(false);
    let images = WindowImages::new(
        state.image_cell(),
        state.window_waker(),
        state.shared().is_headless(),
        root,
        look.theme.image_max_width,
        1.0,
    );
    let math = WindowMath::new(&state.math, look.theme.text, 1.0);
    let found: Rc<std::cell::RefCell<Vec<MediaRef>>> = Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = found.clone();
    let wire: MediaWire = Rc::new(move |element, media: &MediaRef| {
        sink.borrow_mut().push(media.clone());
        element
    });
    let opts = RenderOptions {
        width: crate::reader::READER_MAX_WIDTH,
        style: &look.style,
        math: &math,
        images: &images,
        wires: RenderWires { media: Some(wire), ..RenderWires::default() },
        details: None,
        selectable: true,
        sticky_tables: true,
        sticky_inset: 0.0,
        scale: 1.0,
        highlights: Vec::new(),
    };
    lmdv_render::render(&model, &look.theme, &opts);
    found.take()
}

/// The `lightbox` scene: a page with the fixture picture and a Mermaid diagram,
/// with the picture's preview open (V-01–V-10).
pub fn scene_lightbox(state: &mut WindowState) {
    state.reset_to_start_page();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/lightbox-scene.md");
    let id = state.shared.docs_mut().allocate_id();
    let doc =
        Rc::new(Document::ready(id, DocumentKey::File(path), "Media", SourceBuffer::new(LIGHTBOX_SCENE_MARKDOWN)));
    let revision = doc.revision();
    state.shared.docs_mut().insert_ready(doc.clone());
    state.shared.store_parsed(
        id,
        revision,
        Ok(std::sync::Arc::new(lmdv_md::parse_with_revision(LIGHTBOX_SCENE_MARKDOWN, revision))),
    );
    state.attach_document(doc);
    state.left.set_open(false);
    state.right.set_open(false);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.outline_hidden = true;
    }
    state.sync_outline();
    let Some(tab) = state.active_tab() else { return };
    let tab_id = tab.id;
    let Some(doc_id) = tab.document_id() else { return };
    let media = media_refs(state).into_iter().find(|media| media.kind == MediaKind::Image);
    if let Some(media) = media {
        let mut tx = Tx::default();
        let revision = state.active_tab().and_then(|tab| tab.view.reader.clone()).map_or(0, |model| model.revision);
        state.open_media(tab_id, doc_id, revision, media, &mut tx);
    }
    state.ui.notice = None;
    state.ui.help = None;
}

/// The scene's page: the fixture picture, a wide diagram and the prose around
/// them.
pub const LIGHTBOX_SCENE_MARKDOWN: &str = r#"# Media preview

The picture and the diagram below both open in the full-window preview. Click
one, or focus it and press Enter or Space.

![A quiet landscape](./img/test.png)

The drawing is a Mermaid flowchart; it re-renders for the preview at the
window's width.

```mermaid
flowchart LR
    A[Source] --> B{Parse}
    B -->|ok| C[Render]
    B -->|error| D[Notice]
    C --> E[Preview]
```
"#;

/// The preview's dim scrim.
fn scrim_color() -> rune::Color {
    rune::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.55 }
}
