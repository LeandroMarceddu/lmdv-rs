//! Inline images in laid-out text: the room their box reserves, the baseline they sit on, the
//! line height they ask for, the whole box a line wraps as, and the rise that moves them.
use rune::text::{InlineImage, OBJECT_REPLACEMENT, Span, TextSystem};
use rune::{Pixels, TextStyle};

/// `w` by `h` opaque white pixels.
fn pixels(w: u32, h: u32) -> Pixels {
    let pixels = Pixels::new();
    pixels.set(w, h, vec![255; (w as usize) * (h as usize) * 4]);
    pixels
}

/// The one inline box a layout's lines hold, with the baseline of the line it sits on.
fn only_box(layout: &parley::Layout<rune::text::Ink>) -> (parley::PositionedInlineBox, f32, usize) {
    let mut found = None;
    for (i, line) in layout.lines().enumerate() {
        for item in line.items() {
            if let parley::PositionedLayoutItem::InlineBox(b) = item {
                assert!(found.is_none(), "one inline box in this layout");
                found = Some((b, line.metrics().baseline, i));
            }
        }
    }
    found.expect("an inline box")
}

/// The text of `s` with a U+FFFC where the image goes.
fn placeholder(s: &str) -> String {
    s.replace('#', &OBJECT_REPLACEMENT.to_string())
}

#[test]
fn an_image_reserves_its_width_and_sits_on_the_baseline() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(20.0, 400.0).lh(24.0);
    let image = InlineImage::new(pixels(40, 12), 40.0, 12.0).ascent(10.0).descent(2.0);
    let content = placeholder("x#y");
    let at = content.find(OBJECT_REPLACEMENT).unwrap();
    let spans = vec![Span::new(at, at + 3).image(image)];
    let layout = text.layout_rich(&content, &spans, style, 1.0, None);
    let plain = text.layout_rich("xy", &[], style, 1.0, None);

    let (b, baseline, _) = only_box(&layout);
    assert_eq!((b.width, b.height), (40.0, 12.0), "the room the line reserved: {}", b.id);
    assert_eq!(b.baseline, Some(10.0), "the baseline inside the box is its ascent");
    assert!((b.y + 10.0 - baseline).abs() < 1e-3, "the box's baseline is the text's: {} vs {baseline}", b.y + 10.0);

    // The picture's width is all the line gained: the placeholder itself takes no room.
    let advance = layout.lines().next().unwrap().metrics().advance;
    let plain_advance = plain.lines().next().unwrap().metrics().advance;
    assert!((advance - plain_advance - 40.0).abs() < 0.5, "{advance} over {plain_advance}");
    assert_eq!(layout.len(), 1, "it fits on one line");
}

#[test]
fn a_deep_image_grows_the_line_and_a_rise_moves_it() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(12.0, 400.0).lh(10.0);
    let at = 0;
    // 18 points above the baseline and 6 below is taller than the 10 point line.
    let deep = InlineImage::new(pixels(20, 24), 20.0, 24.0).ascent(18.0).descent(6.0);
    let content = placeholder("#");
    let layout = text.layout_rich(&content, &[Span::new(at, at + 3).image(deep)], style, 1.0, None);
    let (b, baseline, _) = only_box(&layout);
    assert!((b.y + 18.0 - baseline).abs() < 1e-3, "on the baseline");
    assert!(layout.height() >= 24.0, "the line makes room for the depth: {}", layout.height());
    assert!(b.y.abs() < 1e-3, "the deep image starts at the line's top: {}", b.y);

    // A rise lifts the whole box: its baseline moves 5 points up, and what it keeps below the
    // baseline shrinks by the 5 it rose (6 less 5 is 1).
    let raised = InlineImage::new(pixels(20, 24), 20.0, 24.0).ascent(18.0).descent(6.0);
    let layout = text.layout_rich(&content, &[Span::new(at, at + 3).image(raised).rise(5.0)], style, 1.0, None);
    let (b, baseline, _) = only_box(&layout);
    assert!((b.y + 18.0 + 5.0 - baseline).abs() < 1e-3, "the image's baseline is 5 above the text's");
    assert!((baseline - 18.0 - 5.0).abs() < 1e-3, "the line's own baseline rose with it: {baseline}");
    assert!((layout.height() - baseline - 1.0).abs() < 1e-3, "its descent rose too: {}", layout.height());
}

#[test]
fn an_image_wraps_whole_to_the_next_line() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(16.0, 400.0).lh(20.0);
    let image = InlineImage::new(pixels(50, 10), 50.0, 10.0);
    let content = placeholder("word # tail");
    let at = content.find(OBJECT_REPLACEMENT).unwrap();
    let spans = vec![Span::new(at, at + 3).image(image)];
    // "word " fits in 80 points, the 50 point image after it does not.
    let layout = text.layout_rich(&content, &spans, style, 1.0, Some(80.0));
    assert!(layout.len() >= 2, "the image moved the text after it down: {} lines", layout.len());
    let (b, _, line_index) = only_box(&layout);
    assert_eq!(line_index, 1, "the whole image is on the second line");
    let line = layout.get(1).unwrap();
    let m = line.metrics();
    assert!((b.x - (m.inline_min_coord + m.offset)).abs() < 0.5, "it starts the line: {} at {}", b.x, m.offset);
    assert_eq!(b.width, 50.0, "it did not shrink or split");
}

#[test]
fn an_image_draws_at_its_placeholder_after_the_text_before_it() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(16.0, 400.0).lh(22.0);
    let image = InlineImage::new(pixels(30, 10), 30.0, 10.0);
    let content = placeholder("ab#cd");
    let spans = vec![Span::new(0, content.len()).image(image)];
    let layout = text.layout_rich(&content, &spans, style, 1.0, None);

    let line = layout.lines().next().unwrap();
    let mut before = 0.0;
    let mut found = false;
    for item in line.items() {
        match item {
            parley::PositionedLayoutItem::GlyphRun(run) => before = run.offset() + run.advance(),
            parley::PositionedLayoutItem::InlineBox(b) => {
                assert!((b.x - before).abs() < 0.5, "the box starts where the placeholder does: {} vs {before}", b.x);
                found = true;
            }
        }
    }
    assert!(found, "the image's box");
    assert!(before > 5.0, "after the two letters: {before}");
}

#[test]
fn an_image_span_without_a_placeholder_draws_nothing() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(16.0, 400.0).lh(22.0);
    let image = InlineImage::new(pixels(30, 10), 30.0, 10.0);
    let layout = text.layout_rich("plain", &[Span::new(0, 5).image(image)], style, 1.0, None);
    let plain = text.layout_rich("plain", &[], style, 1.0, None);
    assert_eq!(layout.inline_boxes().count(), 0, "no placeholder, no box");
    assert_eq!(layout.full_width(), plain.full_width(), "and no room taken");
}

#[test]
fn a_rise_on_plain_text_shifts_its_run() {
    let mut text = TextSystem::default();
    let style = TextStyle::new(20.0, 400.0).lh(24.0);
    let content = "raised text";
    let plain = text.layout_rich(content, &[], style, 1.0, None);
    let raised = text.layout_rich(content, &[Span::new(0, 6).rise(6.0)], style, 1.0, None);

    // The line's baseline sits below the raised run by the shift, while the run after it sits on
    // the line's baseline.
    let line = plain.lines().next().unwrap();
    let plain_shift = line
        .items()
        .find_map(|i| match i {
            parley::PositionedLayoutItem::GlyphRun(r) => Some(r.baseline() - line.metrics().baseline),
            _ => None,
        })
        .expect("a glyph run");
    assert!(plain_shift.abs() < 1e-3, "unraised text sits on the baseline: {plain_shift}");

    let line = raised.lines().next().unwrap();
    let mut shifts: Vec<f32> = line
        .items()
        .filter_map(|i| match i {
            parley::PositionedLayoutItem::GlyphRun(r) => Some(r.baseline() - line.metrics().baseline),
            _ => None,
        })
        .collect();
    shifts.sort_by(f32::total_cmp);
    shifts.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
    assert_eq!(shifts.len(), 2, "the raised run and the rest: {shifts:?}");
    assert!((shifts[0] + 6.0).abs() < 1e-3, "the raised run is 6 points above the line's baseline: {shifts:?}");
    assert!(shifts[1].abs() < 1e-3, "the text after it keeps the baseline: {shifts:?}");
    assert!(raised.height() >= 30.0, "the line grows above the rise: {}", raised.height());
}
