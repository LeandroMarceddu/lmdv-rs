//! Formula images for the reading surface.
//!
//! `lmdv-math` parses, lays out and rasterizes a formula; this module owns the
//! per-window pieces the renderer must not: the `MathRenderer`, the
//! `rune::InlineImage` cache and the error cache (OI-020, OI-021).
//!
//! A frame asks for the same formulas it asked for last frame, so a hit must
//! neither re-render nor rebuild an RGBA buffer: the cached `InlineImage` is
//! cloned, which only clones the shared [`rune::Pixels`] handle. A formula that
//! failed last time is answered from the error cache immediately and the
//! renderer draws its TeX in a code chip (M-34).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use lmdv_math::{MathError, MathRenderer, MathRequest};
use lmdv_render::MathResolver;
use rune::Color;
use rune::Pixels;
use rune::text::InlineImage;

/// The most formula images one window keeps. The renderer's own cache is
/// bounded by bytes; this bound keeps the `InlineImage` handles finite too.
pub const MATH_IMAGE_CACHE: usize = 256;
/// The most failed formulas one window remembers.
pub const MATH_ERROR_CACHE: usize = 256;
/// The renderer's own RGBA cache entries per window.
const RENDER_CACHE_ENTRIES: usize = 128;

/// The identity of one rendered formula.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct MathKey {
    tex: String,
    size: u32,
    color: [u8; 4],
    display: bool,
    scale: u32,
}

impl MathKey {
    fn new(tex: &str, size_pt: f32, color: [u8; 4], display: bool, scale: f32) -> Self {
        Self { tex: tex.to_owned(), size: size_pt.to_bits(), color, display, scale: scale.to_bits() }
    }
}

/// One window's formula state.
pub struct MathCache {
    renderer: RefCell<MathRenderer>,
    /// `Some(image)` for a rendered formula, `None` for one that failed.
    images: RefCell<HashMap<MathKey, Option<InlineImage>>>,
    errors: RefCell<HashMap<MathKey, String>>,
    renders: Cell<usize>,
}

impl Default for MathCache {
    fn default() -> Self {
        Self::new()
    }
}

impl MathCache {
    /// A cache with `lmdv-math`'s default RGBA budget.
    pub fn new() -> Self {
        Self {
            renderer: RefCell::new(MathRenderer::new(RENDER_CACHE_ENTRIES)),
            images: RefCell::new(HashMap::new()),
            errors: RefCell::new(HashMap::new()),
            renders: Cell::new(0),
        }
    }

    /// The picture for one formula, or `None` when it cannot be rendered.
    ///
    /// Cached in both directions: a hit returns the same [`InlineImage`] handle
    /// and never clones the RGBA buffer, and a failure returns immediately so
    /// the frame does not re-parse bad TeX.
    pub fn resolve(&self, tex: &str, size_pt: f32, color: [u8; 4], display: bool, scale: f32) -> Option<InlineImage> {
        let scale = if scale.is_finite() { scale.clamp(0.01, 16.0) } else { 1.0 };
        let key = MathKey::new(tex, size_pt, color, display, scale);
        if let Some(cached) = self.images.borrow().get(&key) {
            return cached.clone();
        }
        if let Some(error) = self.errors.borrow().get(&key) {
            let _ = error;
            return None;
        }

        let request = MathRequest { tex: tex.to_owned(), size_pt, color, display, scale };
        let rendered = self.renderer.borrow_mut().render(&request);
        self.renders.set(self.renders.get().saturating_add(1));
        match rendered {
            Ok(math) => {
                let image = inline_image(&math, scale);
                self.insert_image(key, Some(image.clone()));
                Some(image)
            }
            Err(error) => {
                self.insert_error(key, error);
                None
            }
        }
    }

    fn insert_image(&self, key: MathKey, image: Option<InlineImage>) {
        let mut images = self.images.borrow_mut();
        if images.len() >= MATH_IMAGE_CACHE {
            images.clear();
        }
        images.insert(key, image);
    }

    fn insert_error(&self, key: MathKey, error: MathError) {
        let mut errors = self.errors.borrow_mut();
        if errors.len() >= MATH_ERROR_CACHE {
            errors.clear();
        }
        errors.insert(key, error.to_string());
    }

    /// How many formula images are retained.
    pub fn cached(&self) -> usize {
        self.images.borrow().len()
    }

    /// How many failed formulas are retained.
    pub fn error_count(&self) -> usize {
        self.errors.borrow().len()
    }

    /// How many times the rasterizer ran (cache misses).
    pub fn renders(&self) -> usize {
        self.renders.get()
    }

    /// The parser diagnostic for `tex`, when it failed.
    pub fn error_for(&self, tex: &str) -> Option<String> {
        self.errors.borrow().iter().find(|(key, _)| key.tex == tex).map(|(_, message)| message.clone())
    }

    /// Drops every formula, for a theme or size change.
    pub fn clear(&self) {
        self.images.borrow_mut().clear();
        self.errors.borrow_mut().clear();
        self.renderer.borrow_mut().clear();
    }
}

/// Turns `lmdv-math`'s raster into the inline object Rune lays out.
///
/// `width` and `height` are device pixels, so the drawn size divides by the
/// device scale; the ascent and descent are already points at the requested
/// size (patch 0001 places them independently of the image box).
fn inline_image(math: &lmdv_math::RenderedMath, scale: f32) -> InlineImage {
    let pixels = Pixels::new();
    let expected = (math.width as usize).saturating_mul(math.height as usize).saturating_mul(4);
    if math.width > 0 && math.height > 0 && math.rgba.len() >= expected {
        pixels.set(math.width, math.height, math.rgba.clone());
    }
    let width = math.width as f32 / scale;
    let height = math.height as f32 / scale;
    InlineImage::new(pixels, width, height).ascent(math.ascent_pt).descent(math.descent_pt)
}

/// The straight sRGB bytes of a theme colour, as `lmdv-math` expects them.
pub fn color_bytes(color: Color) -> [u8; 4] {
    let hex = color.hex();
    let byte = |at: usize| u8::from_str_radix(hex.get(at..at + 2).unwrap_or("00"), 16).unwrap_or(0);
    [byte(1), byte(3), byte(5), byte(7)]
}

/// The [`MathResolver`] the renderer sees: a window cache plus the theme colour
/// and display scale for this frame.
pub struct WindowMath<'a> {
    cache: &'a MathCache,
    color: [u8; 4],
    scale: f32,
}

impl<'a> WindowMath<'a> {
    /// A resolver drawing in `color` at `scale` device pixels per point.
    pub fn new(cache: &'a MathCache, color: Color, scale: f32) -> Self {
        Self { cache, color: color_bytes(color), scale }
    }
}

impl MathResolver for WindowMath<'_> {
    fn inline(&self, tex: &str, size_pt: f32) -> Option<InlineImage> {
        self.cache.resolve(tex, size_pt, self.color, false, self.scale)
    }

    fn display(&self, tex: &str, size_pt: f32) -> Option<InlineImage> {
        self.cache.resolve(tex, size_pt, self.color, true, self.scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_request_for_the_same_formula_is_a_cache_hit() {
        let cache = MathCache::new();
        let first = cache.resolve(r"x^2 + y^2", 16.0, [0, 0, 0, 255], false, 2.0);
        assert!(first.is_some(), "a valid formula renders");
        assert_eq!(cache.renders(), 1);
        let second = cache.resolve(r"x^2 + y^2", 16.0, [0, 0, 0, 255], false, 2.0);
        assert!(second.is_some());
        assert_eq!(cache.renders(), 1, "the hit did not rasterize again");
        assert_eq!(cache.cached(), 1);
    }

    #[test]
    fn display_and_inline_are_different_entries() {
        let cache = MathCache::new();
        assert!(cache.resolve(r"\sum_i i", 16.0, [0, 0, 0, 255], false, 1.0).is_some());
        assert!(cache.resolve(r"\sum_i i", 16.0, [0, 0, 0, 255], true, 1.0).is_some());
        assert_eq!(cache.cached(), 2);
    }

    #[test]
    fn a_broken_formula_is_cached_as_a_failure() {
        let cache = MathCache::new();
        assert!(cache.resolve(r"\frac{1}", 16.0, [0, 0, 0, 255], false, 1.0).is_none());
        assert_eq!(cache.error_count(), 1);
        assert!(cache.error_for(r"\frac{1}").is_some());
        assert!(cache.resolve(r"\frac{1}", 16.0, [0, 0, 0, 255], false, 1.0).is_none());
        assert_eq!(cache.error_count(), 1, "the failure was answered from its cache");
        assert_eq!(cache.renders(), 1, "no second parse");
    }

    #[test]
    fn scale_and_colour_are_part_of_the_identity() {
        let cache = MathCache::new();
        assert!(cache.resolve("x", 16.0, [0, 0, 0, 255], false, 1.0).is_some());
        assert!(cache.resolve("x", 16.0, [255, 255, 255, 255], false, 1.0).is_some());
        assert!(cache.resolve("x", 16.0, [0, 0, 0, 255], false, 2.0).is_some());
        assert_eq!(cache.cached(), 3);
    }

    #[test]
    fn the_image_keeps_its_baseline_metrics_in_points() {
        let cache = MathCache::new();
        let image = cache.resolve(r"\frac{1}{2}", 20.0, [0, 0, 0, 255], true, 2.0).expect("renders");
        assert!(image.width > 0.0 && image.height > 0.0);
        assert!(image.ascent > 0.0, "the numerator is above the baseline");
        assert!(image.descent > 0.0, "the denominator dips below it");
    }

    #[test]
    fn colours_encode_to_srgb_bytes() {
        assert_eq!(color_bytes(Color::BLACK), [0, 0, 0, 255]);
        assert_eq!(color_bytes(Color::WHITE), [255, 255, 255, 255]);
    }
}
