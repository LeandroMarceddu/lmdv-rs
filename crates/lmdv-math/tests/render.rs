//! Public-API tests for the RaTeX pipeline, metrics and cache.

use lmdv_math::{DEFAULT_CACHE_BYTES, MathError, MathRenderer, MathRequest, RenderedMath};

fn request(tex: &str, display: bool) -> MathRequest {
    MathRequest { tex: tex.to_string(), size_pt: 16.0, color: [0, 0, 0, 255], display, scale: 1.0 }
}

fn assert_pixels_match_size(rendered: &RenderedMath) {
    let expected =
        (rendered.width as usize).checked_mul(rendered.height as usize).and_then(|pixels| pixels.checked_mul(4));
    assert_eq!(Some(rendered.rgba.len()), expected);
}

fn assert_has_ink(rendered: &RenderedMath) {
    let (pixels, _) = rendered.rgba.as_chunks::<4>();
    let inked = pixels.iter().filter(|pixel| pixel[3] > 0).count();
    assert!(inked > 100, "expected visible pixels, got {inked}");
}

fn assert_plausible_aspect(rendered: &RenderedMath) {
    let aspect = rendered.width as f32 / rendered.height as f32;
    assert!((0.2..=12.0).contains(&aspect), "implausible aspect ratio {aspect}");
}

fn assert_metrics_match_canvas(rendered: &RenderedMath, scale: f32) {
    assert!(rendered.ascent_pt > 0.0, "ascent should be positive");
    assert!(rendered.descent_pt >= 0.0);
    let logical_height = rendered.height as f32 / scale;
    let metrics_height = rendered.ascent_pt + rendered.descent_pt;
    let tolerance = 1.0 / scale + 0.01;
    assert!(
        (metrics_height - logical_height).abs() <= tolerance,
        "ascent {}+descent {} = {} does not match height {} (scale {scale})",
        rendered.ascent_pt,
        rendered.descent_pt,
        metrics_height,
        logical_height
    );
}

fn assert_good_render(rendered: &RenderedMath, scale: f32) {
    assert!(rendered.width > 0);
    assert!(rendered.height > 0);
    assert_pixels_match_size(rendered);
    assert_has_ink(rendered);
    assert_plausible_aspect(rendered);
    assert_metrics_match_canvas(rendered, scale);
}

#[test]
fn fraction_and_radical_render_with_baseline_metrics() {
    let mut math = MathRenderer::new(16);
    let rendered = math.render(&request(r"\frac{1}{2}+\sqrt{x}", false)).expect("fraction and radical");

    assert_good_render(&rendered, 1.0);
    assert!(rendered.ascent_pt > rendered.descent_pt, "the fraction rises above the baseline");
}

#[test]
fn einstein_inline_renders() {
    let mut math = MathRenderer::new(16);
    let rendered = math.render(&request("E=mc^2", false)).expect("E=mc^2");

    assert_good_render(&rendered, 1.0);
    assert!(rendered.width > rendered.height, "E=mc^2 is wider than it is tall");
}

#[test]
fn display_integral_renders_with_limits() {
    let mut math = MathRenderer::new(16);
    let rendered = math.render(&request(r"\int_0^\infty e^{-x^2}dx", true)).expect("display integral");

    assert_good_render(&rendered, 1.0);
    assert!(rendered.height as f32 / rendered.width as f32 > 0.3, "limits add vertical extent");
}

#[test]
fn matrix_renders() {
    let mut math = MathRenderer::new(16);
    let rendered = math.render(&request(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}", true)).expect("matrix");

    assert_good_render(&rendered, 1.0);
}

#[test]
fn smash_metrics_follow_the_raster_bounds() {
    let mut math = MathRenderer::new(16);
    let mut request = request(r"\smash{\frac{1}{2}}", false);
    request.size_pt = 32.0;
    let rendered = math.render(&request).expect("smashed fraction");

    // `\smash` zeroes the nominal box but the ink still has height. Metrics must
    // follow the display list actually rasterized, not the zeroed root box.
    assert_good_render(&rendered, 1.0);
}

#[test]
fn requested_color_is_applied_and_cached_separately() {
    let mut math = MathRenderer::new(16);
    let mut red = request("x+y", false);
    red.color = [255, 0, 0, 255];
    let mut blue = request("x+y", false);
    blue.color = [0, 0, 255, 255];

    let red = math.render(&red).expect("red formula");
    let blue = math.render(&blue).expect("blue formula");
    assert_eq!(math.cached_len(), 2, "color is part of the cache key");

    let channel_totals = |rendered: &RenderedMath| {
        let mut red = 0u64;
        let mut blue = 0u64;
        let (pixels, _) = rendered.rgba.as_chunks::<4>();
        for pixel in pixels.iter().filter(|pixel| pixel[3] > 32) {
            red += u64::from(pixel[0]);
            blue += u64::from(pixel[2]);
        }
        (red, blue)
    };

    let (red_channel, blue_channel) = channel_totals(&red);
    assert!(red_channel > blue_channel * 8, "red formula has r={red_channel}, b={blue_channel}");
    let (red_channel, blue_channel) = channel_totals(&blue);
    assert!(blue_channel > red_channel * 8, "blue formula has r={red_channel}, b={blue_channel}");
}

#[test]
fn requested_alpha_is_preserved() {
    let mut math = MathRenderer::new(16);
    let mut translucent = request("x+y", false);
    translucent.color = [0, 0, 0, 128];
    let rendered = math.render(&translucent).expect("translucent formula");

    let (pixels, _) = rendered.rgba.as_chunks::<4>();
    let max_alpha = pixels.iter().map(|pixel| pixel[3]).max().unwrap_or(0);
    assert!(max_alpha <= 128, "alpha {max_alpha} exceeded the requested 128");
    assert!(max_alpha > 0, "the formula should still be visible");
}

#[test]
fn hidpi_scale_changes_pixels_but_not_point_metrics() {
    let mut math = MathRenderer::new(16);
    let formula = r"\frac{1}{2}+\sqrt{x}";
    let low = math.render(&request(formula, false)).expect("1x");
    let mut high_request = request(formula, false);
    high_request.scale = 2.0;
    let high = math.render(&high_request).expect("2x");

    assert!((high.width as f32 / low.width as f32 - 2.0).abs() < 0.1, "width {} vs {}", high.width, low.width);
    assert!((high.height as f32 / low.height as f32 - 2.0).abs() < 0.1, "height {} vs {}", high.height, low.height);
    assert!((high.ascent_pt - low.ascent_pt).abs() < 0.01);
    assert!((high.descent_pt - low.descent_pt).abs() < 0.01);
    assert_good_render(&high, 2.0);
}

#[test]
fn invalid_tex_reports_the_parser_message() {
    let mut math = MathRenderer::new(16);
    let error = math.render(&request(r"\frac{1}{", false)).expect_err("unclosed group");

    match error {
        MathError::InvalidTex(message) => {
            assert!(message.contains("ParseError"), "unexpected message: {message}");
        }
        other => panic!("expected InvalidTex, got {other:?}"),
    }
    assert_eq!(math.cached_len(), 0, "errors are not cached");
}

#[test]
fn cache_key_includes_every_input() {
    let mut math = MathRenderer::new(16);
    let base = request("x+1", false);
    math.render(&base).expect("base");

    let mut size = base.clone();
    size.size_pt = 18.0;
    math.render(&size).expect("size");

    let mut color = base.clone();
    color.color = [255, 0, 0, 255];
    math.render(&color).expect("color");

    let mut display = base.clone();
    display.display = true;
    math.render(&display).expect("display");

    let mut scale = base.clone();
    scale.scale = 2.0;
    math.render(&scale).expect("scale");

    assert_eq!(math.cached_len(), 5, "each distinct input keeps its own entry");
    math.render(&base).expect("base again");
    assert_eq!(math.cached_len(), 5);
}

#[test]
fn cache_is_bounded_by_entries() {
    let mut math = MathRenderer::new(2);
    for formula in ["a", "b", "c"] {
        math.render(&request(formula, false)).expect("render");
    }
    assert_eq!(math.cached_len(), 2);
    assert_eq!(DEFAULT_CACHE_BYTES, 32 * 1024 * 1024);
}

#[test]
fn empty_tex_renders_nothing() {
    let mut math = MathRenderer::new(16);
    let rendered = math.render(&request("", false)).expect("empty formula");

    assert_eq!(rendered.width, 0);
    assert_eq!(rendered.height, 0);
    assert_eq!(rendered.ascent_pt, 0.0);
    assert_eq!(rendered.descent_pt, 0.0);
    assert!(rendered.rgba.is_empty());
}
