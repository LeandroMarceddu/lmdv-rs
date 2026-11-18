//! The public surface: a crate outside `lmdv-render` can build a reading scene
//! from `MdDoc` + theme + options without touching internals.

use lmdv_md::MdDoc;
use lmdv_render::{
    AllClosed, DocTheme, FOOTNOTES_KEY, Highlight, HighlightKind, ImageResolver, MathResolver, NoMath, RenderOptions,
    footnote_key, footnote_link, heading_key, highlight_key, render,
};
use rune::text::InlineImage;
use rune_kit::Kit;
use rune_kit::markdown::MarkdownStyle;

struct Images;

impl ImageResolver for Images {
    fn resolve(&self, _src: &str, _alt: &str, _title: &str) -> Option<rune::Image> {
        None
    }
}

struct Formulas;

impl MathResolver for Formulas {
    fn inline(&self, tex: &str, _size_pt: f32) -> Option<InlineImage> {
        (tex == "x").then(|| {
            let pixels = rune::Pixels::new();
            pixels.set(1, 1, vec![0, 0, 0, 255]);
            InlineImage::new(pixels, 8.0, 8.0)
        })
    }

    fn display(&self, _tex: &str, _size_pt: f32) -> Option<InlineImage> {
        None
    }
}

#[test]
fn the_public_api_builds_an_article() {
    let doc = lmdv_md::parse_with_revision("# Title\n\nA $x$ formula and a note.[^1]\n\n[^1]: Body.\n", 3);
    assert_eq!(doc.revision, 3);
    let kit = Kit::neutral(true);
    let theme = DocTheme::from_kit(&kit);
    let style = MarkdownStyle::from_kit(&kit);
    let opts = RenderOptions::new(600.0, &style, &Formulas, &Images).details(&AllClosed);
    render(&doc, &theme, &opts);
    assert_eq!(heading_key(0), "h0");
    assert_eq!(footnote_key("1"), "fn-1");
    assert_eq!(footnote_link("1"), "fn:1");
    assert_eq!(FOOTNOTES_KEY, "footnotes");
}

#[test]
fn the_public_api_takes_source_range_highlights() {
    let doc = lmdv_md::parse("# Title\n\nA paragraph with a match.\n");
    let theme = DocTheme::neutral(false);
    let style = MarkdownStyle::neutral(false);
    let at = doc.source.find("match").expect("the word");
    let opts = RenderOptions::new(600.0, &style, &NoMath, &Images).highlights([
        Highlight::new(at..at + "match".len(), HighlightKind::Find),
        Highlight { range: at..at + "match".len(), kind: HighlightKind::Current },
    ]);
    render(&doc, &theme, &opts);
    assert_eq!(highlight_key(0), "hl-0");
}

#[test]
fn a_document_with_no_content_still_renders() {
    let doc = MdDoc::default();
    let theme = DocTheme::neutral(false);
    let style = MarkdownStyle::neutral(false);
    let opts = RenderOptions::new(400.0, &style, &NoMath, &Images);
    let _ = render(&doc, &theme, &opts);
}
