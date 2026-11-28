//! Rune-free math rendering for lmdv, backed by [RaTeX](https://github.com/erweixin/RaTeX).
//!
//! The Markdown layer strips the `$`/`$$` delimiters and passes the TeX source
//! here. [`MathRenderer`] parses and lays out the formula, rasterizes it to
//! straight-alpha RGBA, and returns both the pixels and the exact baseline
//! metrics. The app wraps the result in `rune::InlineImage`.
//!
//! # Units
//!
//! - [`MathRequest::size_pt`] is the font size of the surrounding text, in
//!   points. RaTeX lays out in em units, so layout dimensions are multiplied by
//!   this value.
//! - [`MathRequest::scale`] is the device pixel ratio used only for
//!   rasterization. `1.0` on standard displays, `2.0` on HiDPI.
//! - [`RenderedMath::width`] and [`RenderedMath::height`] are **device pixels**.
//! - [`RenderedMath::ascent_pt`] and [`RenderedMath::descent_pt`] are **points at
//!   the requested size**, independent of `scale`. They are the room above and
//!   below the baseline.
//!
//! Because the raster canvas is rounded up to a whole pixel,
//! `ascent_pt + descent_pt` can differ from `height / scale` by less than one
//! device pixel.
//!
//! # Colors and transparency
//!
//! [`MathRequest::color`] is straight (non-premultiplied) sRGB plus alpha, and
//! [`RenderedMath::rgba`] is also straight alpha, rows top to bottom. The raster
//! background is transparent. A `\color{...}` inside the formula overrides the
//! requested color for the affected items, as in KaTeX.
//!
//! # Example
//!
//! ```text
//! let mut math = MathRenderer::new(128);
//! let out = math.render(&MathRequest {
//!     tex: r"\frac{1}{2}+\sqrt{x}".into(),
//!     size_pt: 16.0,
//!     color: [0, 0, 0, 255],
//!     display: false,
//!     scale: 2.0,
//! })?;
//!
//! let pixels = rune::Pixels::new();
//! pixels.set(out.width, out.height, out.rgba);
//! // Rune takes width, ascent and descent directly: the box's height is
//! // `ascent + descent`, with fractional points preserved.
//! let image = rune::InlineImage::new(pixels, out.width as f32 / 2.0, out.ascent_pt, out.descent_pt);
//! ```

#![forbid(unsafe_code)]

mod cache;

use image::ImageFormat;
use ratex_layout::{LayoutOptions, layout, to_display_list, validate_layout};
use ratex_render::{RenderOptions, render_to_png};
use ratex_types::color::Color;
use ratex_types::math_style::MathStyle;
use thiserror::Error;

use cache::{CacheKey, MathCache};

/// Default memory budget for cached RGBA buffers: 32 MiB.
///
/// Use [`MathRenderer::with_memory_budget`] to change it. The default matches
/// the budget reserved for math assets in the architecture notes.
pub const DEFAULT_CACHE_BYTES: usize = 32 * 1024 * 1024;

/// RaTeX's renderer clamps the device pixel ratio to this range; values outside
/// it are rejected here instead of silently changing the pixel density.
const MIN_SCALE: f32 = 0.01;
const MAX_SCALE: f32 = 16.0;

/// One formula to render.
#[derive(Clone, Debug)]
pub struct MathRequest {
    /// TeX source without delimiters (`$`, `$$`, `\(`, `\[`).
    pub tex: String,
    /// Font size of the surrounding text, in points.
    pub size_pt: f32,
    /// Straight (non-premultiplied) sRGB color and alpha.
    pub color: [u8; 4],
    /// Display style for block formulas; text style for inline formulas.
    pub display: bool,
    /// Device pixel ratio for rasterization, normally `1.0` or `2.0`.
    pub scale: f32,
}

/// A rasterized formula plus the metrics needed to place it on a baseline.
#[derive(Clone, Debug)]
pub struct RenderedMath {
    /// Image width in device pixels. Zero for an empty expression.
    pub width: u32,
    /// Image height in device pixels. Zero for an empty expression.
    pub height: u32,
    /// Room above the baseline, in points at the requested font size.
    pub ascent_pt: f32,
    /// Room below the baseline, in points at the requested font size.
    pub descent_pt: f32,
    /// `width * height * 4` straight-alpha RGBA bytes, rows top to bottom.
    ///
    /// Empty when `width` or `height` is zero.
    pub rgba: Vec<u8>,
}

/// Why a formula could not be rendered.
#[derive(Clone, Debug, Error)]
pub enum MathError {
    /// The TeX parser rejected the source. The message is the parser's own
    /// diagnostic, including its source position when available.
    #[error("invalid TeX: {0}")]
    InvalidTex(String),
    /// `size_pt` was NaN, infinite, or not positive.
    #[error("math size must be finite and positive, got {0}")]
    InvalidSize(f32),
    /// `scale` was outside the range RaTeX can rasterize faithfully.
    #[error("device scale must be finite and within 0.01..=16, got {0}")]
    InvalidScale(f32),
    /// Layout succeeded syntactically but exceeded RaTeX's output limits.
    #[error("math layout rejected: {0}")]
    Layout(String),
    /// Rasterization failed (for example, the image would exceed the pixel
    /// limit).
    #[error("math rasterization failed: {0}")]
    Render(String),
    /// The renderer's PNG could not be decoded, or had inconsistent dimensions.
    #[error("math image decode failed: {0}")]
    Decode(String),
}

/// A parsing, layout, raster and cache pipeline for math formulas.
///
/// The cache is least-recently-used and bounded by both an entry count and a
/// byte budget; see [`DEFAULT_CACHE_BYTES`]. Errors are not cached.
pub struct MathRenderer {
    cache: MathCache,
    #[cfg(test)]
    renders: usize,
}

impl MathRenderer {
    /// Creates a renderer caching at most `cache_entries` results, within
    /// [`DEFAULT_CACHE_BYTES`]. `0` disables caching.
    pub fn new(cache_entries: usize) -> Self {
        Self::with_memory_budget(cache_entries, DEFAULT_CACHE_BYTES)
    }

    /// Creates a renderer with an explicit cache byte budget.
    ///
    /// The budget covers the retained RGBA buffers and TeX sources
    /// approximately; it does not include RaTeX's own process-wide font cache.
    /// `0` for either limit disables caching.
    pub fn with_memory_budget(cache_entries: usize, max_bytes: usize) -> Self {
        Self {
            cache: MathCache::new(cache_entries, max_bytes),
            #[cfg(test)]
            renders: 0,
        }
    }

    /// Renders `request`, returning a cached result when all inputs match.
    ///
    /// The common failure is malformed TeX ([`MathError::InvalidTex`]); it is
    /// reported, never panicked. Empty input renders as a zero-sized,
    /// metric-free result so the caller can place nothing without special
    /// cases.
    pub fn render(&mut self, request: &MathRequest) -> Result<RenderedMath, MathError> {
        validate_request(request)?;

        let key = CacheKey::new(request);
        if let Some(hit) = self.cache.get(&key) {
            return Ok(hit);
        }

        #[cfg(test)]
        {
            self.renders += 1;
        }

        let rendered = render_uncached(request)?;
        self.cache.insert(key, rendered.clone());
        Ok(rendered)
    }

    /// Number of formulas currently retained.
    pub fn cached_len(&self) -> usize {
        self.cache.len()
    }

    /// Drops every cached formula while keeping the configured limits.
    pub fn clear(&mut self) {
        self.cache.clear();
    }
}

fn validate_request(request: &MathRequest) -> Result<(), MathError> {
    if !request.size_pt.is_finite() || request.size_pt <= 0.0 {
        return Err(MathError::InvalidSize(request.size_pt));
    }
    if !request.scale.is_finite() || !(MIN_SCALE..=MAX_SCALE).contains(&request.scale) {
        return Err(MathError::InvalidScale(request.scale));
    }
    Ok(())
}

fn render_uncached(request: &MathRequest) -> Result<RenderedMath, MathError> {
    let nodes = ratex_parser::parse(&request.tex).map_err(|error| MathError::InvalidTex(error.to_string()))?;

    let options = LayoutOptions {
        style: if request.display { MathStyle::Display } else { MathStyle::Text },
        color: to_ratex_color(request.color),
        ..LayoutOptions::default()
    };
    let tree = layout(&nodes, &options);
    validate_layout(&tree).map_err(MathError::Layout)?;

    let display_list = to_display_list(&tree);
    let size_pt = f64::from(request.size_pt);
    let total_em = display_list.height + display_list.depth;

    // An empty expression (or one that lays out to nothing) has no ink and no
    // advance. Avoid the renderer's 1x1 transparent placeholder so callers can
    // skip drawing it entirely.
    if display_list.width <= 0.0 && total_em <= 0.0 {
        return Ok(RenderedMath { width: 0, height: 0, ascent_pt: 0.0, descent_pt: 0.0, rgba: Vec::new() });
    }

    let render_options = RenderOptions {
        font_size: request.size_pt,
        // The layout box already encloses the formula. Padding would shift the
        // image inside its box and make baseline placement depend on it.
        padding: 0.0,
        background_color: Color::new(0.0, 0.0, 0.0, 0.0),
        // With the `embed-fonts` feature this is unused; keep it empty so the
        // crate never reaches for a user font directory.
        font_dir: String::new(),
        device_pixel_ratio: request.scale,
    };
    let png = render_to_png(&display_list, &render_options).map_err(MathError::Render)?;
    let image = image::load_from_memory_with_format(&png, ImageFormat::Png)
        .map_err(|error| MathError::Decode(error.to_string()))?
        .into_rgba8();
    let (width, height) = image.dimensions();
    let rgba = image.into_raw();

    let expected = (width as usize).checked_mul(height as usize).and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(rgba.len()) {
        return Err(MathError::Decode(format!(
            "decoded {}x{} image has {} bytes, expected width*height*4",
            width,
            height,
            rgba.len()
        )));
    }

    Ok(RenderedMath {
        width,
        height,
        ascent_pt: (display_list.height.max(0.0) * size_pt) as f32,
        descent_pt: (display_list.depth.max(0.0) * size_pt) as f32,
        rgba,
    })
}

fn to_ratex_color(color: [u8; 4]) -> Color {
    Color::new(
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        f32::from(color[3]) / 255.0,
    )
}

#[cfg(test)]
mod tests;
