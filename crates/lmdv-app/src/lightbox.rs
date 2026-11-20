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
//! V-08 adds the copy/save menu above the zoom bar: a picture's PNG bytes are
//! read on a worker as soon as the preview opens, **Copy as image** (2×) and
//! **Copy as large image** (4×) put them on the clipboard, and **Save as PNG…**
//! asks for a destination and writes atomically through `lmdv-io` on a worker.
//! A raster is copied at its natural pixel size, as V-09 asks; the two entries
//! differ only for vector content. Saving or copying a diagram as PNG needs a
//! scene rasterizer, and SVG needs a serializer, so diagrams keep the zoom-only
//! preview for now (OI-140). A picture that is not PNG is refused with a notice:
//! `lmdv-app` has no decoder to re-encode it.
//!
//! The hover hint (V-02) is a child of the drawn media itself, so it sits at the
//! media's top-right corner rather than the column's, and the article's clip
//! hides it as soon as that corner leaves the pane. The floating find, notice
//! and section bars are drawn above the article, so a hint they cover is not
//! visible either.
//!
//! Inline (text-span) pictures are materialised from the model for the block
//! menu's **Open preview**; opening one by clicking the picture itself needs the
//! renderer to mark the picture's span as a link (see the P6-L follow-up).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use lmdv_core::{Document, DocumentId, DocumentKey, SourceBuffer, TabId};
use lmdv_render::{MediaKind, MediaRef, MediaWire, RenderOptions, RenderWires};
use rune::{
    ClipboardContent, ClipboardImage, Cursor, Cx, Drag, El, FileDialogOptions, Key, KeyEvent, LiveRegion, Phase, Tx,
    Waker, WheelEvent, el, row, text,
};
use rune_kit::image_zoom::math::{clamp_axis, detail_scale};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, Menu, MenuItem, Placement};

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
/// Natural size around the current centre.
pub const LIGHTBOX_ACTUAL: &str = "lightbox-actual";
/// The copy/save button in the title bar (V-08).
pub const LIGHTBOX_COPY: &str = "lightbox-copy";
/// The copy/save menu's surface and rows.
pub const LIGHTBOX_MENU: &str = "lightbox-menu";
/// The PNG header every saveable picture starts with.
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// The decode hint for a picture whose natural size is not known yet (an
/// inline picture): SVG sources rasterize at this size; rasters keep their own.
const INLINE_DECODE_POINTS: f32 = 2048.0;
/// The scripted save directory for headless runs, like `LMDV_EXPORT_DIR`.
const SAVE_SEAM: &str = "LMDV_LIGHTBOX_DIR";

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
    /// Whether the copy/save menu is open (V-08).
    menu_open: Cell<bool>,
    /// The picture's PNG bytes, read on a worker when the preview opens.
    png: Arc<Mutex<PngState>>,
    /// The running Save as PNG… write, if any.
    save: RefCell<Option<ImageSave>>,
}

/// A picture's PNG file, as the worker read it (V-08/V-09).
#[derive(Clone, Debug)]
enum PngState {
    /// The worker is still reading the file.
    Loading,
    /// The file's bytes, ready to copy or write.
    Ready(Arc<[u8]>),
    /// Why the picture can't be copied or saved.
    Unavailable(String),
}

/// One Save as PNG… write on a worker.
#[derive(Debug)]
struct ImageSave {
    /// The destination, for the success notice.
    path: PathBuf,
    /// The worker's answer, filled once and drained by `poll_lightbox`.
    outcome: Arc<Mutex<Option<Result<(), String>>>>,
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
            menu_open: Cell::new(false),
            png: Arc::new(Mutex::new(PngState::Loading)),
            save: RefCell::new(None),
        }
    }

    /// Whether the copy/save menu is open (V-08).
    pub fn menu_open(&self) -> bool {
        self.menu_open.get()
    }

    /// Opens the copy/save menu; the copy button owns it.
    pub(crate) fn open_menu(&self) {
        self.menu_open.set(true);
    }

    /// Closes the copy/save menu; focus returns to the copy button.
    pub(crate) fn close_menu(&self) {
        self.menu_open.set(false);
    }

    /// The picture's PNG file state (V-08/V-09).
    fn png(&self) -> PngState {
        self.png.lock().map(|state| state.clone()).unwrap_or_else(|poison| poison.into_inner().clone())
    }

    /// Whether the PNG bytes are ready to copy or save.
    pub fn png_ready(&self) -> bool {
        matches!(self.png(), PngState::Ready(_))
    }

    /// Why the picture cannot be copied or saved, once the worker said so.
    pub fn png_unavailable(&self) -> Option<String> {
        match self.png() {
            PngState::Unavailable(reason) => Some(reason),
            _ => None,
        }
    }

    /// Reads the picture's file on a worker, so Copy and Save never touch a
    /// frame (ADR-017). Only a PNG can be handed to the clipboard or written
    /// again: `lmdv-app` has no decoder to re-encode another format.
    fn load_png(&self, root: PathBuf, source: String, waker: Waker) {
        let slot = self.png.clone();
        let spawned = rune::task::spawn("lmdv-image-png", move || {
            let state = read_png(&root, &source);
            if let Ok(mut slot) = slot.lock() {
                *slot = state;
            }
            waker();
        });
        if spawned.is_err()
            && let Ok(mut slot) = self.png.lock()
        {
            *slot = PngState::Unavailable(lmdv_i18n::t!("msg.imageUnavailable").to_owned());
        }
    }

    /// The Save as PNG… answer once it lands, so `poll_lightbox` can report it.
    fn take_save_outcome(&self) -> Option<Result<PathBuf, String>> {
        let mut save = self.save.borrow_mut();
        let job = save.as_ref()?;
        let answer = job.outcome.lock().ok().and_then(|mut outcome| outcome.take())?;
        let path = job.path.clone();
        *save = None;
        Some(answer.map(|()| path))
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

/// The default name for a saved preview: the picture's stem with `.png`.
fn png_file_name(media: &MediaRef) -> String {
    if media.kind == MediaKind::Diagram {
        return format!("{}.png", lmdv_i18n::t!("zoom.diagram").to_lowercase());
    }
    let name = image_name(&media.source);
    let stem =
        name.strip_suffix(".png").unwrap_or_else(|| name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem));
    format!("{stem}.png")
}

/// Reads a picture's file for the copy/save menu (V-08/V-09), on a worker.
///
/// Only a PNG can be passed on unchanged: this crate has no decoder to turn
/// another raster format into PNG. A network, `data:` or refused path answers
/// [`PngState::Unavailable`] rather than touching the filesystem.
fn read_png(root: &Path, source: &str) -> PngState {
    let unavailable = || PngState::Unavailable(lmdv_i18n::t!("msg.imageUnavailable").to_owned());
    let src = source.split('#').next().unwrap_or(source).trim();
    if src.is_empty() || !is_local_asset(src) {
        return unavailable();
    }
    let policy = lmdv_io::PathPolicy::new(root);
    let Ok((_, bytes)) = policy.read_asset(Path::new(src)) else {
        return unavailable();
    };
    if bytes.starts_with(PNG_SIGNATURE) {
        return PngState::Ready(Arc::from(bytes));
    }
    // V-08/V-09: other rasters are converted on the worker.
    match lmdv_export::raster_to_png(&bytes) {
        Some(png) => PngState::Ready(Arc::from(png)),
        None => unavailable(),
    }
}

/// Whether an image source names a local path rather than a URL, a device or a
/// UNC share (the same rule as the image resolver, OI-009).
fn is_local_asset(src: &str) -> bool {
    if src.starts_with("//") || src.starts_with(r"\\") {
        return false;
    }
    if let Some((scheme, _)) = src.split_once(':') {
        let looks_like_scheme = !scheme.is_empty()
            && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if looks_like_scheme {
            return false;
        }
    }
    true
}

/// Writes PNG bytes atomically on a worker; the parent must exist and may not
/// be a network or device path (X-15's rule for the exports).
fn write_png_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    lmdv_io::path_policy::reject_network_or_device(path).map_err(|error| error.to_string())?;
    let target = lmdv_io::atomic::canonical_target(path).map_err(|error| format!("{}: {error}", path.display()))?;
    lmdv_io::atomic::write_atomic(&target, bytes).map_err(|error| format!("{}: {error}", target.display()))
}

/// The scripted/test seam for the preview's save dialog: `LMDV_LIGHTBOX_DIR`.
/// A headless script cannot answer a dialog, so the save writes
/// `<dir>/<default name>` when this variable is set.
fn save_seam_dir() -> Option<PathBuf> {
    std::env::var_os(SAVE_SEAM).map(PathBuf::from).filter(|dir| !dir.as_os_str().is_empty())
}

/// Every inline (text-span) picture in the active model, in reading order.
///
/// The renderer draws a picture inside a paragraph as a text-span image, not an
/// element, so it has no `MediaRef` of its own. The app materialises one from
/// the model for the block menu (and, with the renderer follow-up, for a click
/// on the picture); `index` is the picture object's place in its inline run,
/// the same index the renderer's `image:` link carries.
pub(crate) fn inline_media_refs(state: &WindowState) -> Vec<MediaRef> {
    let Some(model) = state.active_tab().and_then(|tab| tab.view.reader.clone()) else { return Vec::new() };
    let mut found = Vec::new();
    for (block_index, block) in model.blocks.iter().enumerate() {
        let key = model.block_key(block_index).map(str::to_owned).unwrap_or_else(|| format!("b{block_index}"));
        let span = model.block_spans.get(block_index);
        let mut pictures = Vec::new();
        collect_inline_pictures(block, &mut pictures);
        for (index, source, alt) in pictures {
            found.push(MediaRef {
                block: key.clone(),
                index,
                kind: MediaKind::Image,
                source,
                alt,
                width: 0.0,
                height: 0.0,
                line: span.map_or(0, |span| span.line),
                end_line: span.map_or(1, |span| span.end_line),
            });
        }
    }
    found
}

/// The inline pictures one block draws, including nested lists, quotes and
/// table cells; the index is the picture object's place in its own inline run.
fn collect_inline_pictures(block: &lmdv_md::Block, out: &mut Vec<(usize, String, String)>) {
    match block {
        lmdv_md::Block::Para(inline) | lmdv_md::Block::Heading { inline, .. } => inline_pictures(inline, out),
        lmdv_md::Block::Quote { blocks, .. } => {
            for block in blocks {
                collect_inline_pictures(block, out);
            }
        }
        lmdv_md::Block::List { items, .. } => {
            for item in items {
                for block in item {
                    collect_inline_pictures(block, out);
                }
            }
        }
        lmdv_md::Block::Table(table) => {
            for inline in table.head.iter().chain(table.rows.iter().flatten()) {
                inline_pictures(inline, out);
            }
        }
        _ => {}
    }
}

/// The `<img>` objects of one inline run, in source order; the index is the
/// object's place in [`lmdv_md::Inline::objects`], the destination the
/// renderer's inline-picture link carries.
fn inline_pictures(inline: &lmdv_md::Inline, out: &mut Vec<(usize, String, String)>) {
    use lmdv_md::InlineObjectKind;
    for (index, object) in inline.objects.iter().enumerate() {
        let node = match &object.kind {
            InlineObjectKind::Image => object.html.as_ref(),
            InlineObjectKind::Picture => object.html.as_ref().and_then(picture_fallback),
            _ => None,
        };
        let Some(node) = node else { continue };
        let Some(src) = node.attr("src").map(str::trim).filter(|src| !src.is_empty()) else { continue };
        out.push((index, src.to_owned(), node.attr("alt").unwrap_or_default().to_owned()));
    }
}

/// The `<img>` a `<picture>` falls back to.
fn picture_fallback(node: &lmdv_md::HtmlNode) -> Option<&lmdv_md::HtmlNode> {
    node.children
        .iter()
        .filter_map(|child| match child {
            lmdv_md::HtmlChild::Node(node) => Some(node),
            lmdv_md::HtmlChild::Text(_) => None,
        })
        .find(|node| node.attr("src").is_some())
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
    ///
    /// A block image or diagram is the renderer's own reference. A paragraph
    /// that holds an inline (text-span) picture has no `MediaRef` from the
    /// renderer yet, so the first picture in the block is materialised from the
    /// model: the same preview opens, with the same zoom, pan and menu.
    pub(crate) fn media_for_block(&self, block: &str) -> Option<MediaRef> {
        self.block_media(block).or_else(|| self.inline_media_first(block))
    }

    /// The renderer's own reference for a block image or diagram.
    fn block_media(&self, block: &str) -> Option<MediaRef> {
        media_refs(self).into_iter().find(|media| media.block == block)
    }

    /// The first inline picture a block draws, for the block menu.
    fn inline_media_first(&self, block: &str) -> Option<MediaRef> {
        inline_media_refs(self).into_iter().find(|media| media.block == block)
    }

    /// The inline picture at `index` in `block`, materialised from the model.
    fn inline_media(&self, block: &str, index: usize) -> Option<MediaRef> {
        inline_media_refs(self).into_iter().find(|media| media.block == block && media.index == index)
    }

    /// Opens the preview for the media a top-level block draws.
    pub(crate) fn open_media_block(&mut self, block: &str, tx: &mut Tx) {
        let Some(tab) = self.active_tab() else { return };
        let (tab_id, Some(doc)) = (tab.id, tab.document_id()) else { return };
        let revision = tab.view.reader.as_ref().map_or(0, |model| model.revision);
        if let Some(media) = self.block_media(block) {
            self.open_media(tab_id, doc, revision, media, tx);
        } else if let Some(media) = self.inline_media_first(block) {
            // The block element, not a media button, is where focus returns:
            // the picture itself lives inside the paragraph's text.
            self.open_inline_media(block, media.index, tx);
        }
    }

    /// Opens the inline picture `index` in `block` (V-01 for a text-span image).
    ///
    /// The block menu reaches this preview through [`Self::media_for_block`].
    /// A future renderer follow-up marks the picture's span as an `image:` link
    /// when the media wire is set; this is the handler behind that destination.
    pub(crate) fn open_inline_media(&mut self, block: &str, index: usize, tx: &mut Tx) {
        let Some(media) = self.inline_media(block, index) else { return };
        let Some(tab) = self.active_tab() else { return };
        let (tab_id, Some(doc)) = (tab.id, tab.document_id()) else { return };
        let revision = tab.view.reader.as_ref().map_or(0, |model| model.revision);
        self.open_media_as(tab_id, doc, revision, media, block.to_owned(), tx);
    }

    /// Opens the preview for `media` in `tab`, if that tab still shows the
    /// model revision the media was drawn from.
    ///
    /// A picture the path policy refuses or the decoder failed does not open
    /// (V-01); a pending decode opens and draws once its pixels land.
    pub(crate) fn open_media(&mut self, tab: TabId, doc: DocumentId, revision: u64, media: MediaRef, tx: &mut Tx) {
        let opener = format!("media-{}", media.block);
        self.open_media_as(tab, doc, revision, media, opener, tx);
    }

    /// [`Self::open_media`] with the element focus returns to when the preview
    /// closes (a block image's own button, or an inline picture's paragraph).
    fn open_media_as(
        &mut self,
        tab: TabId,
        doc: DocumentId,
        revision: u64,
        media: MediaRef,
        opener: String,
        tx: &mut Tx,
    ) {
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
        let mut root = None;
        let image = match media.kind {
            MediaKind::Image => {
                let Some(document) = active.document().cloned() else { return };
                let Some(folder) = self.document_root(&document) else { return };
                let size = if media.width >= 2.0 { media.width.max(1.0) } else { INLINE_DECODE_POINTS };
                let cache = self
                    .image_cell()
                    .get_or_init(|| ImageCacheState::new(self.window_waker(), self.shared.is_headless()));
                let Some(image) = cache.resolve(&folder, &media.source, size, 2.0) else {
                    return;
                };
                if image.error().is_some() {
                    return;
                }
                root = Some(folder);
                Some(image)
            }
            MediaKind::Diagram => {
                if media.source.trim().is_empty() {
                    return;
                }
                None
            }
        };
        let lightbox = Lightbox::new(tab, doc, revision, media, image, opener);
        if let Some(root) = root {
            lightbox.load_png(root, lightbox.media.source.clone(), self.window_waker());
        }
        self.lightbox = Some(lightbox);
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
            // V-08: Escape closes the copy menu first, then the preview.
            if self.lightbox.as_ref().is_some_and(Lightbox::menu_open) {
                self.close_lightbox_menu();
                return true;
            }
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

    /// Opens the copy/save menu (V-08).
    pub(crate) fn open_lightbox_menu(&mut self) {
        if let Some(lightbox) = self.lightbox.as_ref() {
            lightbox.open_menu();
        }
    }

    /// Closes the copy/save menu; the preview stays open and the copy button
    /// takes focus back.
    pub(crate) fn close_lightbox_menu(&mut self) {
        if let Some(lightbox) = self.lightbox.as_ref() {
            lightbox.close_menu();
        }
    }

    /// **Copy as image** (2×) and **Copy as large image** (4×): one PNG of the
    /// previewed picture on the clipboard (V-08/V-09).
    ///
    /// A raster is copied at its natural pixel size either way, as the original
    /// does; `scale` is the vector rasterization factor the diagrams will use
    /// once they can be rasterized. The bytes were read when the preview opened,
    /// so this handler only writes the clipboard.
    pub(crate) fn copy_lightbox_png(&mut self, _scale: u32, tx: &mut Tx) {
        let Some(lightbox) = self.lightbox.as_ref() else { return };
        match lightbox.png() {
            PngState::Ready(bytes) => {
                let Some(image) = ClipboardImage::from_png(bytes) else {
                    self.notice(lmdv_i18n::t!("msg.imageUnavailable"));
                    return;
                };
                tx.copy_rich(ClipboardContent::new("").with_image(image));
                self.notice(lmdv_i18n::t!("msg.imageCopied"));
            }
            PngState::Loading => self.notice(lmdv_i18n::t!("msg.imagePreparing")),
            PngState::Unavailable(reason) => self.notice(reason),
        }
    }

    /// **Save as PNG…**: asks for a destination, then writes the picture on a
    /// worker (V-08). The default name is the picture's stem plus `.png`; a
    /// headless script saves into `LMDV_LIGHTBOX_DIR` without a dialog.
    pub(crate) fn request_lightbox_save(&mut self, tx: &mut Tx) {
        let Some(lightbox) = self.lightbox.as_ref() else { return };
        let name = png_file_name(&lightbox.media);
        if self.shared().is_headless()
            && let Some(dir) = save_seam_dir()
        {
            self.start_lightbox_save(dir.join(name));
            return;
        }
        let directory = self
            .active_tab()
            .and_then(|tab| tab.document().cloned())
            .and_then(|document| document.key.borrow().path().and_then(Path::parent).map(Path::to_path_buf));
        let options = FileDialogOptions {
            title: lmdv_i18n::t!("zoom.savePng").to_owned(),
            directory,
            file_name: Some(name),
            ..FileDialogOptions::default()
        }
        .filter(lmdv_i18n::t!("filter.png"), &["png"]);
        tx.save_file(options, |state: &mut WindowState, result, _| match result {
            Ok(Some(path)) => state.start_lightbox_save(path),
            Ok(None) => {}
            Err(error) => state.notice(lmdv_i18n::t!("msg.saveFailed", "err" => error)),
        });
    }

    /// Writes the previewed picture's PNG bytes to `path` on a worker; the
    /// result reaches `poll_lightbox` through the lightbox's save slot.
    pub(crate) fn start_lightbox_save(&mut self, path: PathBuf) {
        let Some(lightbox) = self.lightbox.as_ref() else { return };
        let bytes = match lightbox.png() {
            PngState::Ready(bytes) => bytes,
            PngState::Loading => {
                self.notice(lmdv_i18n::t!("msg.imagePreparing"));
                return;
            }
            PngState::Unavailable(reason) => {
                self.notice(reason);
                return;
            }
        };
        let outcome = Arc::new(Mutex::new(None));
        let worker_outcome = outcome.clone();
        let worker_path = path.clone();
        let waker = self.window_waker();
        let spawned = rune::task::spawn("lmdv-image-save", move || {
            let result = write_png_atomically(&worker_path, &bytes);
            if let Ok(mut slot) = worker_outcome.lock() {
                *slot = Some(result);
            }
            waker();
        });
        let outcome = if spawned.is_ok() {
            outcome
        } else {
            Arc::new(Mutex::new(Some(Err("the save worker could not start".to_owned()))))
        };
        lightbox.save.replace(Some(ImageSave { path, outcome }));
    }

    /// Closes a preview whose tab, document, revision or picture no longer
    /// matches; also called after an image decode fails. A finished Save as
    /// PNG… is reported here, since it lands outside a handler.
    pub(crate) fn poll_lightbox(&mut self) -> bool {
        let mut changed = false;
        if let Some(result) = self.lightbox.as_ref().and_then(Lightbox::take_save_outcome) {
            match result {
                Ok(path) => self.notice(lmdv_i18n::t!("msg.imageSaved", "name" => path.display().to_string())),
                Err(error) => self.notice(lmdv_i18n::t!("msg.saveFailed", "err" => error)),
            }
            changed = true;
        }
        let stale = self.lightbox.as_ref().is_some_and(|lightbox| {
            self.active_tab().is_none_or(|tab| {
                tab.id != lightbox.tab
                    || tab.document_id() != Some(lightbox.doc)
                    || tab.view.reader.as_ref().map_or(0, |model| model.revision) != lightbox.revision
            }) || lightbox.image.as_ref().is_some_and(|image| image.error().is_some())
        });
        if stale {
            self.lightbox = None;
            changed = true;
        }
        changed
    }
}

/// Wraps one drawn media element in the open handler and hover hint.
///
/// The hint is a child of the media itself, so it sits at the picture's or
/// diagram's top-right corner rather than the article column's, and the pane's
/// clip hides it as soon as that corner scrolls out (V-02).
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
    let press_media = media.clone();
    let key_media = media.clone();
    let hint_media = media.clone();
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
    element
        .key(key)
        .role("button")
        .label(open_label)
        .focusable(true)
        .cursor(Cursor::Pointer)
        .hover_group()
        .child(hint.pin(None, Some(6.0), Some(6.0), None).group_opacity(0.0, 1.0))
        .on_press_tx(move |state: &mut WindowState, tx: &mut Tx| {
            state.open_media(tab, doc, revision, press_media.clone(), tx);
        })
        .on_key(move |state: &mut WindowState, event: &KeyEvent, tx: &mut Tx| {
            let activate = event.key == Key::Enter || matches!(&event.key, Key::Text(value) if value == " ");
            if activate {
                state.open_media(tab, doc, revision, key_media.clone(), tx);
            }
            activate
        })
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
    let (copy, mut menu_layer) = copy_menu(lightbox, kit);
    let mut title_bar = row()
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
        );
    if let Some(copy) = copy {
        title_bar = title_bar.child(copy);
    }
    let title_bar = title_bar.child(close);

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
    // The menu mounts above the dialog: its scrim and surface sit after the
    // preview, so the rows take the pointer and Escape before the preview does.
    layer.append(&mut menu_layer);
}

/// The title bar's copy/save button and its menu (V-08).
///
/// A diagram is left without the menu until it can be rasterized: it has no
/// PNG and its SVG needs a serializer (OI-140). A picture always gets the menu;
/// its rows report the reason when the file is not a PNG.
fn copy_menu(lightbox: &Lightbox, kit: &Kit) -> (Option<El>, Vec<El>) {
    if lightbox.media.kind != MediaKind::Image {
        return (None, Vec::new());
    }
    let button = IconButton::new(kit.glyph.copy)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .expanded(lightbox.menu_open())
        .label(lmdv_i18n::t!("zoom.copy"))
        .help(lmdv_i18n::t!("zoom.copy"))
        .on_press_tx(|state: &mut WindowState, _: &mut Tx| state.open_lightbox_menu())
        .build(kit)
        .key(LIGHTBOX_COPY);
    let mut menu = Menu::new(LIGHTBOX_MENU)
        .trigger(button)
        .open(lightbox.menu_open())
        .min_width(280.0)
        .placement(Placement::BottomEnd)
        .on_close(|state: &mut WindowState| state.close_lightbox_menu());
    menu = menu.item(
        MenuItem::new(lmdv_i18n::t!("zoom.copyPng")).description(lmdv_i18n::t!("zoom.copyPngHint")).on_select_tx(
            |state: &mut WindowState, tx: &mut Tx| {
                state.close_lightbox_menu();
                state.copy_lightbox_png(2, tx);
            },
        ),
    );
    menu = menu.item(
        MenuItem::new(lmdv_i18n::t!("zoom.copyPng4")).description(lmdv_i18n::t!("zoom.copyPng4Hint")).on_select_tx(
            |state: &mut WindowState, tx: &mut Tx| {
                state.close_lightbox_menu();
                state.copy_lightbox_png(4, tx);
            },
        ),
    );
    menu = menu.separator().item(MenuItem::new(lmdv_i18n::t!("zoom.savePng")).on_select_tx(
        |state: &mut WindowState, tx: &mut Tx| {
            state.close_lightbox_menu();
            state.request_lightbox_save(tx);
        },
    ));
    let mut layer = Vec::new();
    let trigger = menu.build(kit, &mut layer);
    (Some(trigger), layer)
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

A picture written inside a sentence ![Inline landscape](./img/test.png) opens
the same preview from the block's context menu while the renderer's click
wiring is still a follow-up.
"#;

/// The preview's dim scrim.
fn scrim_color() -> rune::Color {
    rune::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.55 }
}
