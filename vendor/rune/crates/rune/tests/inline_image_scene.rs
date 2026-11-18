//! An inline image in a live scene: the picture is drawn at the box the text reserved, at its
//! own size, and selection and copy see the placeholder it stands for.
mod support;

use rune::text::{InlineImage, OBJECT_REPLACEMENT};
use rune::{Cx, El, Env, Pixels, Runtime, Span, TextStyle, col, rich};
use support::{click, command, find, glyphs};

/// `w` by `h` opaque white pixels.
fn pixels(w: u32, h: u32) -> Pixels {
    let pixels = Pixels::new();
    pixels.set(w, h, vec![255; (w as usize) * (h as usize) * 4]);
    pixels
}

struct S {
    content: String,
    at: usize,
    image: InlineImage,
    /// The em's placeholder is not copied, as `El::uncopied` over it makes a formula disappear
    /// from a copy.
    uncopied: bool,
}

impl S {
    fn new(uncopied: bool) -> S {
        let content = format!("before {OBJECT_REPLACEMENT} after");
        let at = content.find(OBJECT_REPLACEMENT).unwrap();
        S { content, at, image: InlineImage::new(pixels(40, 12), 40.0, 12.0).ascent(10.0).descent(2.0), uncopied }
    }
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let span = Span::new(s.at, s.at + OBJECT_REPLACEMENT.len_utf8()).image(s.image.clone());
    let mut text = rich(s.content.clone(), vec![span]).key("line").font(TextStyle::new(16.0, 400.0).lh(22.0)).w(300.0);
    if s.uncopied {
        text = text.uncopied(s.at..s.at + OBJECT_REPLACEMENT.len_utf8());
    }
    col().w_full().h_full().padding(10.0).child(text.selectable(true))
}

fn runtime(uncopied: bool) -> Runtime<S> {
    let mut rt = Runtime::new(S::new(uncopied), view, (500.0, 160.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

/// The rects of the images drawn.
fn pictures(rt: &Runtime<S>) -> Vec<[f32; 4]> {
    rt.instances.iter().filter(|i| i.params[1] == rune::render::KIND_IMAGE).map(|i| i.rect).collect()
}

/// Selects the whole text: a word double-click first, then select-all.
fn select_all(rt: &mut Runtime<S>) {
    let g = glyphs(rt, "line");
    click(rt, g[0]);
    click(rt, g[0]);
    command(rt, 'a');
}

#[test]
fn the_picture_draws_at_the_size_the_text_reserved() {
    let rt = runtime(false);
    let e = find(&rt, "line");
    let pictures = pictures(&rt);
    assert_eq!(pictures.len(), 1, "one image drawn: {pictures:?}");
    let r = pictures[0];
    // 40 by 12 points at 2x, the aspect the image asked for.
    assert!((r[2] - 80.0).abs() < 0.5 && (r[3] - 24.0).abs() < 1e-3, "drawn at its own size: {r:?}");
    // The element's rect is in points; instances are device pixels.
    let (x0, y0) = (e.rect[0] * 2.0, e.rect[1] * 2.0);
    let (w, h) = (e.rect[2] * 2.0, e.rect[3] * 2.0);
    assert!(r[0] >= x0 && r[0] + r[2] <= x0 + w + 0.5, "inside the text's width: {r:?} in {x0}..{}", x0 + w);
    assert!(r[1] >= y0 && r[1] + r[3] <= y0 + h + 0.5, "inside its line: {r:?} in {y0}..{}", y0 + h);
    // The placeholder itself draws no glyph: the letters alone are inked.
    let letters = "beforeafter".chars().count();
    assert_eq!(glyphs(&rt, "line").len(), letters, "no tofu for the placeholder");
    assert!(rt.instances.iter().all(|i| i.params[1] != rune::render::KIND_GLYPH || i.rect[2] > 0.5));
}

#[test]
fn copy_takes_the_placeholder_unless_it_is_uncopied() {
    let mut rt = runtime(false);
    select_all(&mut rt);
    let with = format!("before {OBJECT_REPLACEMENT} after");
    assert_eq!(rt.selected_text().as_deref(), Some(with.as_str()), "the placeholder copies as U+FFFC");

    let mut rt = runtime(true);
    select_all(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("before  after"), "uncopied leaves the text around it");
}
