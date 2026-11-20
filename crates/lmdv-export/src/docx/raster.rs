//! SVG rasterization for the DOCX writer.
//!
//! Word can embed an SVG picture, but the plain DrawingML `a:blip` a `.docx`
//! carries needs a raster fallback that every Word version understands. Rather
//! than writing a second `svgBlip` extension and hoping the reader supports it,
//! the exporter renders the picture to a PNG at twice its intrinsic size.
//!
//! The renderer is `resvg` over `usvg`, already compiled for the PDF pipeline
//! (typst-pdf uses `krilla-svg`, which uses both). It reads no file: the
//! document's resolver supplies the bytes and text inside the SVG is shaped
//! with Typst's bundled fonts, not the system font database. A picture with
//! no usable size, or a renderer that rejects the SVG, returns `None` and the
//! writer falls back to alt text.

use resvg::tiny_skia;
use resvg::usvg;

/// The raster's pixel size cap per edge. A viewport larger than this is scaled
/// down instead of allocating gigabytes.
const MAX_EDGE: f32 = 4096.0;
/// The preferred raster scale over the SVG's intrinsic size, in pixels per
/// CSS pixel (96 dpi). Word prints the raster at that logical size.
const SCALE: f32 = 2.0;

/// Rasterizes `svg` as a PNG and returns its bytes with the pixel size.
pub(crate) fn svg_to_png(svg: &[u8]) -> Option<(Vec<u8>, (u32, u32))> {
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
    let scale = SCALE.min(MAX_EDGE / width).min(MAX_EDGE / height);
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let pixel_width = (width * scale).round().clamp(1.0, MAX_EDGE) as u32;
    let pixel_height = (height * scale).round().clamp(1.0, MAX_EDGE) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(pixel_width, pixel_height)?;
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

    #[test]
    fn a_path_only_svg_becomes_a_png_at_twice_its_size() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
            <rect width="40" height="20" fill="#0969da"/></svg>"##;
        let (png, (width, height)) = svg_to_png(svg).expect("a raster");
        assert_eq!((width, height), (80, 40));
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "not a PNG");
    }

    #[test]
    fn svg_text_is_shaped_with_a_bundled_font() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="40">
            <text x="4" y="24" font-family="sans-serif" font-size="16">Label</text></svg>"##;
        let (png, (width, height)) = svg_to_png(svg).expect("a raster");
        assert_eq!((width, height), (240, 80));
        // The text draws ink: a blank raster would compress to almost nothing.
        assert!(png.len() > 200, "the text did not render ({} bytes)", png.len());
    }

    #[test]
    fn an_unusable_svg_is_refused() {
        assert!(svg_to_png(b"not an svg").is_none());
        let zero = br#"<svg xmlns="http://www.w3.org/2000/svg" width="0" height="0"></svg>"#;
        assert!(svg_to_png(zero).is_none());
    }

    #[test]
    fn a_huge_viewport_is_clamped_instead_of_allocating() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="100">
            <rect width="100000" height="100" fill="#0969da"/></svg>"##;
        let (_, (width, height)) = svg_to_png(svg).expect("a clamped raster");
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
        let with_image = svg_to_png(svg.as_bytes()).expect("a raster");
        let blank = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"></svg>"##;
        let without = svg_to_png(blank).expect("a raster");
        assert_eq!(with_image, without, "the local file must not be read");
    }
}
