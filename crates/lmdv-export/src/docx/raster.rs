//! SVG rasterization for the DOCX writer.
//!
//! Word can embed an SVG picture, but the plain DrawingML `a:blip` a `.docx`
//! carries needs a raster fallback that every Word version understands. Rather
//! than writing a second `svgBlip` extension and hoping the reader supports it,
//! the exporter renders the picture to a PNG at twice its intrinsic size
//! through [`crate::raster`], the same renderer the preview uses.
//!
//! The DOCX raster keeps a transparent background: a package written before the
//! preview copy existed keeps its exact bytes. The rasterizer reads no file; an
//! SVG with no usable size, or one the renderer rejects, returns `None` and the
//! writer falls back to alt text.

/// The preferred raster scale over the SVG's intrinsic size, in pixels per
/// CSS pixel (96 dpi). Word prints the raster at that logical size.
const SCALE: f32 = 2.0;

/// Rasterizes `svg` as a transparent PNG and returns its bytes with the pixel
/// size.
pub(crate) fn svg_to_png(svg: &[u8]) -> Option<(Vec<u8>, (u32, u32))> {
    crate::raster::render(svg, SCALE, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::MAX_EDGE;

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
