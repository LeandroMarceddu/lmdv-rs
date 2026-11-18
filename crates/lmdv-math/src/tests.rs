use super::*;

fn request(tex: &str) -> MathRequest {
    MathRequest { tex: tex.to_string(), size_pt: 16.0, color: [0, 0, 0, 255], display: false, scale: 1.0 }
}

#[test]
fn cache_hit_does_not_render_again() {
    let mut renderer = MathRenderer::new(4);
    let request = request(r"\frac{1}{2}");

    let first = renderer.render(&request).expect("first render");
    let second = renderer.render(&request).expect("cached render");

    assert_eq!(renderer.renders, 1, "a cache hit must not re-parse or re-rasterize");
    assert_eq!(renderer.cached_len(), 1);
    assert_eq!(first.width, second.width);
    assert_eq!(first.height, second.height);
    assert_eq!(first.ascent_pt, second.ascent_pt);
    assert_eq!(first.descent_pt, second.descent_pt);
    assert_eq!(first.rgba, second.rgba);
}

#[test]
fn cache_evicts_the_least_recently_used_entry() {
    let mut renderer = MathRenderer::new(2);
    let (a, b, c) = (request("a"), request("b"), request("c"));

    renderer.render(&a).expect("a");
    renderer.render(&b).expect("b");
    renderer.render(&a).expect("a from cache");
    assert_eq!(renderer.renders, 2);
    assert_eq!(renderer.cached_len(), 2);

    renderer.render(&c).expect("c evicts b");
    assert_eq!(renderer.renders, 3);
    assert_eq!(renderer.cached_len(), 2);

    renderer.render(&b).expect("b is rendered again");
    assert_eq!(renderer.renders, 4, "b was the least recently used entry");
    assert_eq!(renderer.cached_len(), 2);
}

#[test]
fn clear_drops_entries_without_changing_limits() {
    let mut renderer = MathRenderer::new(4);
    let request = request("x");
    renderer.render(&request).expect("render");
    assert_eq!(renderer.cached_len(), 1);

    renderer.clear();
    assert_eq!(renderer.cached_len(), 0);

    renderer.render(&request).expect("render after clear");
    assert_eq!(renderer.renders, 2);
    assert_eq!(renderer.cached_len(), 1);
}

#[test]
fn entry_and_memory_budgets_both_bound_the_cache() {
    let mut no_entries = MathRenderer::new(0);
    no_entries.render(&request("x")).expect("render");
    assert_eq!(no_entries.cached_len(), 0);

    let mut no_bytes = MathRenderer::with_memory_budget(16, 0);
    no_bytes.render(&request("x")).expect("render");
    assert_eq!(no_bytes.cached_len(), 0);

    let mut tiny = MathRenderer::with_memory_budget(16, 1);
    tiny.render(&request("x")).expect("render");
    assert_eq!(tiny.cached_len(), 0, "a result larger than the budget is not retained");
}

#[test]
fn empty_tex_is_zero_sized_and_cached() {
    let mut renderer = MathRenderer::new(4);
    let rendered = renderer.render(&request("")).expect("empty math");

    assert_eq!(rendered.width, 0);
    assert_eq!(rendered.height, 0);
    assert_eq!(rendered.ascent_pt, 0.0);
    assert_eq!(rendered.descent_pt, 0.0);
    assert!(rendered.rgba.is_empty());
    assert_eq!(renderer.cached_len(), 1);
}

#[test]
fn invalid_sizes_and_scales_are_rejected_before_rendering() {
    let mut renderer = MathRenderer::new(4);

    let mut invalid_size = request("x");
    invalid_size.size_pt = f32::NAN;
    assert!(matches!(
        renderer.render(&invalid_size),
        Err(MathError::InvalidSize(value)) if value.is_nan()
    ));

    let mut zero_size = request("x");
    zero_size.size_pt = 0.0;
    assert!(matches!(renderer.render(&zero_size), Err(MathError::InvalidSize(0.0))));

    let mut invalid_scale = request("x");
    invalid_scale.scale = f32::INFINITY;
    assert!(matches!(
        renderer.render(&invalid_scale),
        Err(MathError::InvalidScale(value)) if value.is_infinite()
    ));

    let mut tiny_scale = request("x");
    tiny_scale.scale = 0.001;
    assert!(matches!(renderer.render(&tiny_scale), Err(MathError::InvalidScale(_))));
    assert_eq!(renderer.renders, 0);
}
