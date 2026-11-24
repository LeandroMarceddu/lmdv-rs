//! SVG rasterization for the app and the DOCX writer.
//!
//! Word can embed an SVG picture, but the plain DrawingML `a:blip` a `.docx`
//! carries needs a raster fallback that every Word version understands. The
//! preview's copy and save actions need PNG bytes too
//! ([`crate::raster_to_png`](crate::raster_to_png) has no SVG decoder). Both go
//! through [`svg_to_png`], which renders an SVG at a requested scale with
//! `resvg` over `usvg` — already compiled for the PDF pipeline (typst-pdf uses
//! `krilla-svg`, which uses both).
//!
//! The raster is sized to the SVG's intrinsic size times the scale, capped at
//! [`MAX_EDGE`] pixels per edge, and drawn on an opaque white background: a
//! clipboard or Word page is white, and a transparent PNG pasted into a dark
//! app would turn a black glyph invisible. The DOCX writer asks for a
//! transparent raster through the crate-internal render function, so existing
//! packages stay byte-identical.
//!
//! No file is read: an `<image href="…">` may only use an embedded data URL.
//! The document resolver had already refused remote URLs; the rasterizer
//! refuses a local path too, before usvg would open it. Text is shaped with
//! Typst's bundled fonts, not the system font database.

use resvg::tiny_skia;
use resvg::usvg;

/// The raster's pixel size cap per edge. A viewport larger than this is scaled
/// down instead of allocating gigabytes.
pub const MAX_EDGE: f32 = 4096.0;
/// The preferred scale over the SVG's intrinsic size for a preview copy
/// (pixels per CSS pixel at 96 dpi).
pub const PREVIEW_SCALE: f32 = 2.0;

/// Rasterizes `svg` as an opaque PNG at `scale` times its intrinsic size.
///
/// Returns `None` when the bytes are not a usable SVG, when the intrinsic size
/// is missing or zero, or when the scale is not positive. The output is sized
/// to the intrinsic size × `scale`, rounded and clamped to [`MAX_EDGE`] per
/// edge; the background is white and opaque.
pub fn svg_to_png(svg: &[u8], scale: f32) -> Option<Vec<u8>> {
    render(svg, scale, Some([255, 255, 255, 255])).map(|(png, _)| png)
}

/// [`svg_to_png`] with the pixel size it produced, for a caller that must
/// reserve a box (the DOCX media writer).
pub fn svg_to_png_sized(svg: &[u8], scale: f32) -> Option<(Vec<u8>, (u32, u32))> {
    render(svg, scale, Some([255, 255, 255, 255]))
}

/// Renders `svg` at `scale`, optionally filling the pixmap first.
///
/// Crate-internal: the DOCX writer keeps a transparent background so a package
/// with an SVG asset has the same bytes it always had.
pub(crate) fn render(svg: &[u8], scale: f32, background: Option<[u8; 4]>) -> Option<(Vec<u8>, (u32, u32))> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let mut options = usvg::Options { font_family: "Libertinus Serif".to_owned(), ..usvg::Options::default() };
    // The exporter reads no files: an `<image href="…">` may only use an
    // embedded data URL. The document resolver had already refused remote
    // URLs; this also refuses a local path before usvg would open it.
    options.image_href_resolver.resolve_string = Box::new(|_href, _options| None);
    load_fallback_fonts(&mut options);
    let tree = usvg::Tree::from_data(svg, &options).ok()?;
    let size = tree.size();
    let (width, height) = (size.width(), size.height());
    if !(width.is_finite() && height.is_finite()) || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let scale = scale.min(MAX_EDGE / width).min(MAX_EDGE / height);
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let pixel_width = (width * scale).round().clamp(1.0, MAX_EDGE) as u32;
    let pixel_height = (height * scale).round().clamp(1.0, MAX_EDGE) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(pixel_width, pixel_height)?;
    if let Some([red, green, blue, alpha]) = background {
        pixmap.fill(tiny_skia::Color::from_rgba8(red, green, blue, alpha));
    }
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let png = pixmap.encode_png().ok()?;
    Some((png, (pixel_width, pixel_height)))
}

/// Loads the bundled faces an SVG's text can use. `typst_assets::fonts()` is a
/// fixed list: the four Libertinus Serif upright faces come first and the four
/// DejaVu Sans Mono faces last. Only those two families are loaded, and the
/// generic families point at them, so text that names an unavailable family
/// still renders instead of disappearing.
fn load_fallback_fonts(options: &mut usvg::Options) {
    for (index, font) in typst_assets::fonts().enumerate() {
        if matches!(index, 0..=3) || index >= 12 {
            options.fontdb_mut().load_font_data(font.to_vec());
        }
    }
    let database = options.fontdb_mut();
    database.set_serif_family("Libertinus Serif");
    database.set_sans_serif_family("Libertinus Serif");
    database.set_monospace_family("DejaVu Sans Mono");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory(png).expect("a PNG").to_rgba8()
    }

    #[test]
    fn a_rect_svg_is_sized_to_its_intrinsic_size_times_the_scale() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
            <rect width="40" height="20" fill="#0969da"/></svg>"##;
        let (png, (width, height)) = svg_to_png_sized(svg, 2.0).expect("a raster");
        assert_eq!((width, height), (80, 40));
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "not a PNG");
    }

    #[test]
    fn the_background_is_opaque_white() {
        // A small blue rect in the corner leaves the rest of the canvas clear.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
            <rect width="5" height="5" fill="#0969da"/></svg>"##;
        let png = svg_to_png(svg, 1.0).expect("a raster");
        let image = decode(&png);
        assert_eq!((image.width(), image.height()), (20, 10));
        assert_eq!(image.get_pixel(19, 9).0, [255, 255, 255, 255], "the empty corner is white and opaque");
    }

    #[test]
    fn svg_text_is_shaped_with_a_bundled_font() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="40">
            <text x="4" y="24" font-family="sans-serif" font-size="16">Label</text></svg>"##;
        let (png, (width, height)) = svg_to_png_sized(svg, 2.0).expect("a raster");
        assert_eq!((width, height), (240, 80));
        // The text draws ink: a blank raster would compress to almost nothing.
        assert!(png.len() > 200, "the text did not render ({} bytes)", png.len());
    }

    #[test]
    fn an_unusable_svg_or_scale_is_refused() {
        assert!(svg_to_png(b"not an svg", 2.0).is_none());
        let zero = br#"<svg xmlns="http://www.w3.org/2000/svg" width="0" height="0"></svg>"#;
        assert!(svg_to_png(zero, 2.0).is_none());
        let good = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"></svg>"##;
        assert!(svg_to_png(good, 0.0).is_none(), "a zero scale");
        assert!(svg_to_png(good, f32::NAN).is_none(), "a NaN scale");
        assert!(svg_to_png(good, -1.0).is_none(), "a negative scale");
    }

    #[test]
    fn a_huge_viewport_is_clamped_instead_of_allocating() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="100">
            <rect width="100000" height="100" fill="#0969da"/></svg>"##;
        let (_, (width, height)) = svg_to_png_sized(svg, 2.0).expect("a clamped raster");
        assert_eq!(width, MAX_EDGE as u32);
        assert!(height <= MAX_EDGE as u32);
    }

    #[test]
    fn a_local_image_reference_is_never_read() {
        // Point the picture at a real file: if usvg opened it, the raster would
        // contain its pixels and differ from the blank raster.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/img/test.png");
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
                <image href="{path}" width="20" height="20"/></svg>"##
        );
        let with_image = svg_to_png(svg.as_bytes(), 1.0).expect("a raster");
        let blank = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"></svg>"##;
        let without = svg_to_png(blank, 1.0).expect("a raster");
        assert_eq!(with_image, without, "the local file must not be read");
    }
}
