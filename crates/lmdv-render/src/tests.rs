//! Renderer tests: the real fixtures parsed by `lmdv-md`, rendered into a
//! `rune::Runtime`, then read back from the element tree (roles, labels, keys,
//! text) the way `--tree` does.

use std::cell::Cell;
use std::rc::Rc;

use lmdv_md::MdDoc;
use rune::Cx;
use rune::runtime::ElementInfo;
use rune::text::InlineImage;
use rune_kit::markdown::MarkdownStyle;

use crate::{
    AllClosed, DocLabels, DocTheme, Highlight, HighlightKind, LinkWire, MathResolver, NoImages, NoMath, RenderOptions,
    render,
};

const FEATURES: &str = include_str!("../../../tests/fixtures/test-features.md");
const MATH: &str = include_str!("../../../tests/fixtures/commonmark-math.md");

#[derive(Clone)]
struct Scene {
    doc: MdDoc,
    theme: DocTheme,
    style: MarkdownStyle,
    width: f32,
    open_details: bool,
    with_math: bool,
    with_images: bool,
    link_calls: Option<Rc<Cell<usize>>>,
    highlights: Vec<Highlight>,
}

/// A math resolver that draws every formula as a small red square.
struct SquareMath;

impl MathResolver for SquareMath {
    fn inline(&self, _tex: &str, _size_pt: f32) -> Option<InlineImage> {
        Some(square())
    }

    fn display(&self, _tex: &str, _size_pt: f32) -> Option<InlineImage> {
        Some(square())
    }
}

fn square() -> InlineImage {
    let pixels = rune::Pixels::new();
    pixels.set(2, 2, vec![255; 16]);
    InlineImage::new(pixels, 24.0, 12.0)
}

static SQUARE: SquareMath = SquareMath;
static CLOSED: AllClosed = AllClosed;
static SQUARE_IMAGES: SquareImages = SquareImages;

/// Resolves every picture to an 8 by 4 square, so an inline image draws.
struct SquareImages;

impl crate::ImageResolver for SquareImages {
    fn resolve(&self, _src: &str, _alt: &str, _title: &str) -> Option<rune::Image> {
        static PICTURE: std::sync::OnceLock<std::sync::Arc<rune::Image>> = std::sync::OnceLock::new();
        let picture = PICTURE.get_or_init(|| {
            let cache = rune::ImageCache::new(std::sync::Arc::new(|| {}));
            let handle = cache.rgba(8, 4, vec![0x80; 8 * 4 * 4]);
            handle.wait().expect("the fixture decodes");
            std::sync::Arc::new(handle)
        });
        Some((**picture).clone())
    }
}

struct AllOpen;

impl crate::DetailsState for AllOpen {
    fn is_open(&self, _key: &str) -> bool {
        true
    }
}

static OPEN: AllOpen = AllOpen;

impl Scene {
    fn new(source: &str) -> Self {
        Self {
            doc: lmdv_md::parse(source),
            theme: DocTheme::neutral(false),
            style: MarkdownStyle::neutral(false),
            width: 760.0,
            open_details: false,
            with_math: false,
            with_images: false,
            link_calls: None,
            highlights: Vec::new(),
        }
    }

    fn math(mut self) -> Self {
        self.with_math = true;
        self
    }

    /// Resolves every picture to a small square, so an inline image draws.
    fn images(mut self) -> Self {
        self.with_images = true;
        self
    }

    /// Paints source ranges over the rendered text.
    fn highlights(mut self, highlights: impl IntoIterator<Item = Highlight>) -> Self {
        self.highlights = highlights.into_iter().collect();
        self
    }

    fn details(mut self, open: bool) -> Self {
        self.open_details = open;
        self
    }

    /// The document labels of `id`, without switching the process locale.
    fn localized(mut self, id: &str) -> Self {
        let locale = lmdv_i18n::find_locale(id).unwrap_or_else(|| panic!("unknown locale {id}"));
        self.theme.labels = DocLabels::localized_in(locale);
        self
    }

    fn links(mut self, calls: Rc<Cell<usize>>) -> Self {
        self.link_calls = Some(calls);
        self
    }

    fn runtime(&self) -> rune::Runtime<Scene> {
        fn view(scene: &Scene, _: &mut Cx<Scene>) -> rune::El {
            let math: &dyn MathResolver = if scene.with_math { &SQUARE } else { &NoMath };
            let images: &dyn crate::ImageResolver = if scene.with_images { &SQUARE_IMAGES } else { &NoImages };
            let mut opts = RenderOptions::new(scene.width, &scene.style, math, images);
            opts.details = Some(if scene.open_details { &OPEN } else { &CLOSED });
            opts = opts.highlights(scene.highlights.clone());
            if let Some(calls) = &scene.link_calls {
                let calls = calls.clone();
                let wire: LinkWire = Rc::new(move |e| {
                    calls.set(calls.get() + 1);
                    e
                });
                opts = opts.link_wire(wire);
            }
            render(&scene.doc, &scene.theme, &opts)
        }
        rune::Runtime::new(
            Scene {
                doc: self.doc.clone(),
                theme: self.theme.clone(),
                style: self.style.clone(),
                width: self.width,
                open_details: self.open_details,
                with_math: self.with_math,
                with_images: self.with_images,
                link_calls: self.link_calls.clone(),
                highlights: self.highlights.clone(),
            },
            view,
            (self.width, 4000.0),
            rune::Env::new(true, 1.0),
        )
    }

    fn render(&self) -> ElTree {
        let mut runtime = self.runtime();
        runtime.frame(0.0);
        ElTree { elements: runtime.elements() }
    }
}

struct ElTree {
    elements: Vec<ElementInfo>,
}

impl ElTree {
    fn has_key(&self, key: &str) -> bool {
        self.elements.iter().any(|e| e.key.as_deref() == Some(key))
    }

    fn by_key(&self, key: &str) -> Option<&ElementInfo> {
        self.elements.iter().find(|e| e.key.as_deref() == Some(key))
    }

    fn texts(&self) -> Vec<&str> {
        self.elements.iter().filter_map(|e| e.text.as_deref()).collect()
    }

    fn has_text(&self, needle: &str) -> bool {
        self.texts().iter().any(|text| text.contains(needle))
    }

    fn headings(&self) -> Vec<(&str, Option<String>, u32)> {
        self.elements
            .iter()
            .filter(|e| e.role == "heading")
            .map(|e| (e.text.as_deref().unwrap_or(""), e.key.clone(), e.level.unwrap_or(0)))
            .collect()
    }

    /// The paint-only highlights Rune reports after their text elements.
    fn marks(&self) -> Vec<&ElementInfo> {
        self.elements.iter().filter(|e| e.role == "mark").collect()
    }

    fn mark_labels(&self) -> Vec<&str> {
        self.marks().iter().filter_map(|e| e.label.as_deref()).collect()
    }

    /// Every element except the paint-only marks: the tree a reader sees,
    /// including rects, text and colours.
    fn snapshot(&self) -> String {
        self.elements
            .iter()
            .filter(|e| e.role != "mark")
            .map(|e| {
                format!(
                    "{}|{:?}|{}|{:?}|{:?}|{:?}|{:?}|{:?}",
                    e.path, e.key, e.role, e.label, e.text, e.rect, e.fill, e.text_color
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[test]
fn features_renders_headings_paragraphs_tables_and_code() {
    let scene = Scene::new(FEATURES);
    let tree = scene.render();
    let headings = tree.headings();
    assert_eq!(headings[0].0, "Test document — lmdv");
    assert_eq!(headings[0].1.as_deref(), Some("h0"));
    assert_eq!(headings[1].0, "Basic formatting");
    assert_eq!(headings[1].1.as_deref(), Some("h1"));
    assert_eq!(headings[1].2, 2);
    assert_eq!(headings.len(), scene.doc.headings.len());

    assert!(tree.has_text("This document exercises every feature of the viewer."));
    assert!(tree.has_text("Text in bold, italic, strikethrough, inline code and an external link."));

    // GFM table cells and the header.
    for cell in ["System", "Role", "Notes", "Legacy PIM", "Akeneo", "Azure Service Bus", "Middleware"] {
        assert!(tree.has_text(cell), "missing table cell {cell:?}");
    }
    assert!(tree.elements.iter().any(|e| e.role == "table" && e.label.as_deref().unwrap_or("").contains("System")));

    // Code blocks keep their source, language label and copy action.
    assert!(tree.has_text("export async function openFile(path: string): Promise<void> {"));
    assert!(tree.has_text("typescript"));
    assert!(tree.has_text("python"));
    assert!(tree.has_key("copy-code:code"));
}

#[test]
fn every_top_level_block_has_its_source_key() {
    let scene = Scene::new(FEATURES);
    let tree = scene.render();
    for index in 0..scene.doc.blocks.len() {
        let key = scene.doc.block_key(index).expect("a key per block");
        assert!(tree.has_key(key), "block {index} ({key}) is not in the tree");
    }
    for heading in 0..scene.doc.headings.len() {
        let key = crate::heading_key(heading);
        assert!(tree.has_key(&key), "heading key {key} is missing");
    }
}

#[test]
fn math_falls_back_to_tex_without_a_resolver() {
    let scene = Scene::new(FEATURES);
    let tree = scene.render();
    assert!(tree.has_text("E = mc^2"), "an inline formula shows its TeX");
    assert!(tree.has_text("\\sqrt{a^2 + b^2}"));
    assert!(tree.has_text("\\int_{-\\infty}^{\\infty} e^{-x^2}\\,dx = \\sqrt{\\pi}"));
    let math: Vec<_> = tree.elements.iter().filter(|e| e.role == "math").collect();
    assert_eq!(math.len(), 3, "three display formulas");
    for block in math {
        assert!(block.label.as_deref().unwrap_or("").starts_with("Math: "));
    }
}

#[test]
fn callouts_are_labelled_cards_with_their_bodies() {
    let scene = Scene::new(FEATURES);
    let tree = scene.render();
    let notes: Vec<_> = tree.elements.iter().filter(|e| e.role == "note").collect();
    let labels: Vec<_> = notes.iter().filter_map(|e| e.label.as_deref()).collect();
    for title in ["Note", "Tip", "Important", "Warning", "Caution"] {
        assert!(labels.contains(&title), "missing callout {title}: {labels:?}");
    }
    assert!(tree.has_text("Useful information that users should know, even when skimming."));
    assert!(tree.has_text("Advises about risks or negative outcomes of certain actions."));
}

#[test]
fn footnotes_get_entries_and_reference_links() {
    let scene = Scene::new(FEATURES);
    let tree = scene.render();
    assert!(tree.has_key("fn-1"));
    assert!(tree.has_key("fn-note"));
    assert!(tree.has_key(crate::FOOTNOTES_KEY));
    assert!(tree.has_key("fn:1"), "the inline reference is a link");
    assert!(tree.has_key("fn:note"));
    assert!(tree.has_key("fnref:1"));
    assert!(tree.has_text("The first footnote, with emphasis and a link."));
    assert!(tree.has_text("Named footnotes work too."));
    let first = tree.by_key("fn-1").expect("the first footnote's entry");
    assert_eq!(first.role, "listitem");
    assert_eq!(first.label.as_deref(), Some("Footnote 1"));
}

#[test]
fn document_labels_localize_the_accessibility_tree() {
    let italian = Scene::new(FEATURES).math().localized("it");
    let tree = italian.render();
    let first = tree.by_key("fn-1").expect("the first footnote's entry");
    assert_eq!(first.label.as_deref(), Some("Nota a piè di pagina 1"));
    let math: Vec<_> = tree.elements.iter().filter(|e| e.role == "math").collect();
    assert_eq!(math.len(), 3, "three display formulas");
    for block in &math {
        let label = block.label.as_deref().unwrap_or("");
        assert!(label.starts_with("Formula: "), "the formula's TeX stays after the label: {label}");
    }
    assert!(
        math.iter().any(|e| e.label.as_deref().is_some_and(|l| l.contains("\\int_{-\\infty}^{\\infty} e^{-x^2}\\,dx"))),
        "the TeX is preserved in the accessible name"
    );

    let chinese = Scene::new(FEATURES).localized("zh-CN").render();
    let first = chinese.by_key("fn-1").expect("the first footnote's entry");
    assert_eq!(first.label.as_deref(), Some("脚注 1"));

    // The English defaults are byte-identical to the labels of earlier releases.
    let english = Scene::new(FEATURES).math().localized("en");
    let tree = english.render();
    assert_eq!(tree.by_key("fn-1").and_then(|e| e.label.as_deref()), Some("Footnote 1"));
    assert!(
        tree.elements
            .iter()
            .filter(|e| e.role == "math")
            .all(|e| { e.label.as_deref().is_some_and(|l| l.starts_with("Math: ")) })
    );
}

#[test]
fn mermaid_draws_or_falls_back_to_its_source() {
    let scene = Scene::new(FEATURES);
    let tree = scene.render();
    // The valid diagram draws through rune-mermaid.
    let valid = tree.by_key("b12").expect("the valid diagram's block");
    assert_eq!(valid.role, "group");
    assert!(tree.has_key("diagram"), "the diagram itself is drawn");
    let diagram = tree.by_key("diagram").expect("diagram");
    assert_eq!(diagram.role, "figure");
    // The invalid one keeps its source and a notice.
    assert!(tree.has_text("This is not valid") || tree.has_text("this is not valid mermaid {{{"));
    assert!(tree.has_text("flowchart LR"), "the invalid source stays visible");
}

#[test]
fn display_math_in_a_paragraph_gets_its_own_line() {
    let scene = Scene::new(MATH);
    let tree = scene.render();
    // Block 55 is the paragraph holding `$$E=mc^2$$` inline-display.
    assert!(tree.by_key("b55").is_some(), "the paragraph block");
    let text = tree
        .elements
        .iter()
        .find(|e| e.text.as_deref().is_some_and(|t| t.contains("The famous")) && e.key.as_deref() == Some("text"))
        .expect("paragraph text");
    assert!(text.text.as_deref().unwrap_or("").contains('\n'), "the display formula has its own line");
}

/// OI-037: the accessibility tree reads the formula's TeX, never the U+FFFC object marker,
/// in the text runs a screen reader hears and in the names it builds from them.
#[test]
fn inline_math_speaks_its_tex_in_the_accessibility_tree() {
    let scene = Scene::new(MATH).math();
    let mut runtime = scene.runtime();
    runtime.frame(0.0);
    let tree = runtime.accessibility_tree();
    let mut spoken = Vec::new();
    for (_, node) in &tree.nodes {
        if let Some(value) = node.value() {
            assert!(!value.contains('\u{fffc}'), "{:?} value holds the object marker: {value:?}", node.role());
            spoken.push(value.to_owned());
        }
        if let Some(label) = node.label() {
            assert!(!label.contains('\u{fffc}'), "{:?} label holds the object marker: {label:?}", node.role());
        }
    }
    assert!(
        spoken.iter().any(|value| value.contains("Math: E = mc^2")),
        "the formula is spoken by its TeX: {spoken:?}"
    );
    let json = runtime.accessibility_json().to_string();
    assert!(!json.contains('\u{fffc}'), "no name in the tree holds the object marker: {json}");
}

#[test]
fn front_matter_is_a_collapsed_card_by_default() {
    let scene = Scene::new("---\ntitle: A page\ntags: [one, two]\n---\n\n# Body\n");
    let tree = scene.render();
    let card = tree.by_key(crate::FRONT_MATTER_KEY).expect("front-matter card");
    assert_eq!(card.role, "note");
    assert_eq!(card.label.as_deref(), Some("Front matter"));
    assert_eq!(card.expanded, Some(false));
    assert!(!tree.has_text("A page"), "closed by default");
}

#[test]
fn toc_lists_the_headings_after_it() {
    let scene = Scene::new("[TOC]\n\n# One\n\n## Two\n\n### Three\n");
    let tree = scene.render();
    assert!(tree.has_key(crate::TOC_KEY));
    let nav = tree.by_key(crate::TOC_KEY).expect("toc nav");
    assert_eq!(nav.role, "navigation");
    assert_eq!(nav.label.as_deref(), Some("Table of contents"));
    assert!(tree.has_key("toc-0"));
    assert!(tree.has_key("toc-1"));
    assert!(tree.has_key("toc-2"));
    assert!(tree.has_text("One"));
    assert!(tree.has_text("Three"));
    assert!(tree.has_key("#one"), "each entry links to its anchor");
}

#[test]
fn html_subset_renders_kbd_sup_br_and_media_placeholders() {
    let scene = Scene::new(
        "Press <kbd>Ctrl</kbd> then <br> go and x<sup>2</sup>.\n\n<video src=\"./clip.mp4\" controls></video>\n",
    );
    let tree = scene.render();
    assert!(tree.has_text("Press Ctrl then"));
    assert!(tree.has_text("video: ./clip.mp4"));
    assert!(tree.has_key("media:./clip.mp4"), "the placeholder opens the file externally");

    let italian = Scene::new("<div><video src=\"./clip.mp4\" controls></video></div>\n").localized("it").render();
    assert!(italian.has_text("Apri all'esterno"), "the media action follows the locale");
    assert!(!italian.has_text("Open externally"));
}

#[test]
fn a_task_list_renders_checkboxes_with_item_labels() {
    let scene = Scene::new("- [x] done item\n- [ ] open item\n");
    let tree = scene.render();
    let boxes: Vec<_> = tree.elements.iter().filter(|e| e.role == "checkbox").collect();
    assert_eq!(boxes.len(), 2);
    assert_eq!(boxes[0].label.as_deref(), Some("done item"));
    assert_eq!(boxes[1].label.as_deref(), Some("open item"));
    let list = tree.by_key("list").expect("list container");
    assert_eq!(list.role, "list");
    let items: Vec<_> = tree.elements.iter().filter(|e| e.role == "listitem").collect();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].level, None);
}

#[test]
fn a_second_commit_keeps_every_key() {
    let scene = Scene::new(FEATURES);
    let mut runtime = scene.runtime();
    runtime.frame(0.0);
    let first: Vec<_> = runtime.elements().into_iter().filter_map(|e| e.key).collect();
    runtime.invalidate();
    runtime.frame(1.0 / 60.0);
    let second: Vec<_> = runtime.elements().into_iter().filter_map(|e| e.key).collect();
    assert_eq!(first, second, "memoised blocks keep the keys they were built with");
    assert!(first.iter().any(|key| key == "b0"));
    assert!(first.iter().any(|key| key == "h0"));
}

#[test]
fn an_empty_document_renders_an_empty_article() {
    let scene = Scene::new("");
    let tree = scene.render();
    assert!(tree.by_key(crate::DOC_KEY).is_some());
    assert!(tree.headings().is_empty());
}

#[test]
fn an_open_front_matter_card_shows_the_entries() {
    let scene = Scene::new("---\ntitle: A page\ntags: [one, two]\n---\n\n# Body\n").details(true);
    let tree = scene.render();
    let card = tree.by_key(crate::FRONT_MATTER_KEY).expect("front-matter card");
    assert_eq!(card.expanded, Some(true));
    assert!(tree.has_text("title"));
    assert!(tree.has_text("A page"));
    assert!(tree.has_text("one"));
    assert!(tree.has_text("two"));
}

#[test]
fn a_math_resolver_draws_images_instead_of_tex() {
    let scene = Scene::new("Inline $E = mc^2$ and \n\n$$\\sqrt{2}$$\n").math();
    let tree = scene.render();
    assert!(!tree.has_text("E = mc^2"), "the formula is an image, not its TeX");
    assert!(!tree.has_text("\\sqrt{2}"));
    assert!(tree.elements.iter().any(|e| e.role == "math"));
}

#[test]
fn a_missing_image_keeps_its_alt_text_and_source() {
    let scene = Scene::new(FEATURES);
    let tree = scene.render();
    let missing = tree
        .elements
        .iter()
        .find(|e| e.role == "image" && e.label.as_deref() == Some("Test image"))
        .expect("the missing image's placeholder names its alt");
    assert_eq!(missing.label.as_deref(), Some("Test image"));
}

#[test]
fn a_link_wire_is_invoked_for_each_linked_text() {
    let calls = Rc::new(Cell::new(0));
    let scene = Scene::new(FEATURES).links(calls.clone());
    let tree = scene.render();
    assert!(calls.get() > 0, "the app's wiring ran");
    assert!(tree.has_key("https://www.anthropic.com"));
    assert!(tree.has_key("./nested/deeper.md"));
    let links: Vec<_> = tree.elements.iter().filter(|e| e.role == "link").collect();
    assert!(links.iter().any(|e| e.key.as_deref() == Some("fn:1")));
}

#[test]
fn an_inline_picture_is_a_preview_link_when_the_link_wire_is_set() {
    let calls = Rc::new(Cell::new(0));
    let scene = Scene::new("Before ![Alt](./pic.png) after\n").links(calls).images();
    let tree = scene.render();
    let link = tree
        .elements
        .iter()
        .find(|element| element.key.as_deref().is_some_and(|key| key.starts_with("image:")))
        .expect("the drawn picture is a preview link");
    assert_eq!(link.key.as_deref(), Some("image:b0:0"), "block key and object index");
    assert_eq!(link.label.as_deref(), Some("Alt"), "the link is named by the alt text");
}

#[test]
fn an_open_html_details_card_shows_its_body() {
    let scene = Scene::new("<details><summary>More</summary>Hidden body</details>\n").details(true);
    let tree = scene.render();
    let card = tree.by_key("details").expect("the details card");
    assert_eq!(card.role, "note");
    assert_eq!(card.expanded, Some(true));
    assert!(tree.has_text("Hidden body"));
}

#[test]
fn a_tables_header_row_is_keyed_and_sticky() {
    let scene = Scene::new("| a | b |\n| - | - |\n| 1 | 2 |\n");
    let tree = scene.render();
    let head = tree.by_key("head").expect("the header row");
    assert_eq!(head.role, "row");
    let table = tree.by_key("table").expect("the table");
    assert_eq!(table.role, "table");
}

#[test]
fn every_paragraph_text_reaches_the_tree() {
    for source in [FEATURES, MATH] {
        let scene = Scene::new(source);
        let tree = scene.render();
        let mut checked = 0;
        for block in &scene.doc.blocks {
            let lmdv_md::Block::Para(inline) = block else { continue };
            let needle = inline.text.split(lmdv_md::OBJECT_CHAR).next().unwrap_or_default().trim();
            if needle.is_empty() {
                continue;
            }
            checked += 1;
            assert!(tree.has_text(needle), "paragraph text is missing from the tree: {needle:?}");
        }
        assert!(checked > 3, "the fixture has paragraphs to check");
    }
}

#[test]
fn nested_list_and_quote_text_is_drawn() {
    let tree = Scene::new(MATH).render();
    for phrase in ["First item", "Second item", "A quote", "and the quote goes on.", "lmdv shows the formula"] {
        assert!(tree.has_text(phrase), "missing {phrase:?}");
    }
}

#[test]
fn no_object_placeholder_reaches_the_drawn_text() {
    for source in [FEATURES, MATH] {
        let scene = Scene::new(source);
        let tree = scene.render();
        for text in tree.texts() {
            assert!(!text.contains(lmdv_md::OBJECT_CHAR), "a placeholder is drawn: {text:?}");
        }
    }
}

#[test]
fn a_picture_chooses_its_source_by_scheme() {
    use std::cell::RefCell;

    struct Recording(RefCell<Vec<String>>);
    impl crate::ImageResolver for Recording {
        fn resolve(&self, src: &str, _alt: &str, _title: &str) -> Option<rune::Image> {
            self.0.borrow_mut().push(src.to_owned());
            None
        }
    }

    let source = "<picture>\n<source media=\"(prefers-color-scheme: dark)\" srcset=\"dark.png 1x\">\n<img src=\"light.png\" alt=\"Logo\">\n</picture>\n";
    let doc = lmdv_md::parse(source);
    let style = MarkdownStyle::neutral(true);
    let recording = Recording(RefCell::new(Vec::new()));
    let theme = DocTheme::neutral(true);
    let opts = RenderOptions::new(600.0, &style, &NoMath, &recording);
    render(&doc, &theme, &opts);
    assert_eq!(recording.0.borrow().as_slice(), ["dark.png"]);

    let light = Recording(RefCell::new(Vec::new()));
    let theme = DocTheme::neutral(false);
    let opts = RenderOptions::new(600.0, &style, &NoMath, &light);
    render(&doc, &theme, &opts);
    assert_eq!(light.0.borrow().as_slice(), ["light.png"]);
}

#[test]
fn scheme_only_images_hide_in_the_other_theme() {
    let source = "![dark logo](./dark.png#gh-dark-mode-only)\n";
    let light = Scene::new(source).render();
    assert!(!light.has_text("dark logo"), "hidden in the light theme");
    assert!(light.by_key("image").is_some(), "the block keeps its key");

    let mut scene = Scene::new(source);
    scene.theme = DocTheme::neutral(true);
    let dark = scene.render();
    assert!(dark.has_text("dark logo"), "shown in the dark theme");
}

#[test]
fn an_inline_picture_draws_once() {
    let tree = Scene::new("<picture><source srcset=\"a.png 1x\"><img src=\"a.png\" alt=\"Logo\"></picture>\n").render();
    let texts: Vec<&str> = tree.texts();
    assert_eq!(texts.iter().filter(|text| text.contains("Logo")).count(), 1, "{texts:?}");
}

#[test]
fn a_srcset_picks_the_candidate_for_the_scale() {
    use lmdv_md::SrcSetCandidate;

    let candidates = [
        SrcSetCandidate { url: "one.png".into(), descriptor: Some("1x".into()) },
        SrcSetCandidate { url: "two.png".into(), descriptor: Some("2x".into()) },
    ];
    assert_eq!(crate::block::best_candidate(&candidates, 1.0).unwrap().url, "one.png");
    assert_eq!(crate::block::best_candidate(&candidates, 2.0).unwrap().url, "two.png");
    assert_eq!(crate::block::best_candidate(&candidates, 1.5).unwrap().url, "two.png");
    assert_eq!(crate::block::best_candidate(&[], 2.0).map(|c| c.url.as_str()), None);
}

#[test]
fn the_task_control_wire_wraps_the_real_checkbox_and_carries_its_source() {
    use std::cell::RefCell;

    use crate::{RenderWires, TaskControlWire, TaskRef};

    struct Wired {
        doc: MdDoc,
        theme: DocTheme,
        style: MarkdownStyle,
        wire: TaskControlWire,
    }

    let source = "- [ ] first\n- [x] second\n  - [ ] nested child\n";
    let doc = lmdv_md::parse(source);
    let seen: Rc<RefCell<Vec<TaskRef>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let wire: TaskControlWire = Rc::new(move |element, task: &TaskRef| {
        sink.borrow_mut().push(task.clone());
        element.key(format!("task-control-{}", task.source.start))
    });
    let scene =
        Wired { doc, theme: DocTheme::neutral(false), style: MarkdownStyle::neutral(false), wire: wire.clone() };
    let mut runtime = rune::Runtime::new(
        scene,
        |scene: &Wired, _: &mut Cx<Wired>| {
            let mut opts = RenderOptions::new(760.0, &scene.style, &NoMath, &NoImages);
            opts.wires = RenderWires { task_control: Some(scene.wire.clone()), ..RenderWires::default() };
            render(&scene.doc, &scene.theme, &opts)
        },
        (760.0, 1200.0),
        rune::Env::new(true, 1.0),
    );
    runtime.frame(0.0);

    let seen = seen.borrow();
    assert_eq!(seen.len(), 3, "one control per task item");
    for task in seen.iter() {
        assert_eq!(task.source.len(), 3, "the marker range came from the source map");
        assert!(
            matches!(&runtime.state.doc.source[task.source.clone()], "[ ]" | "[x]"),
            "the range points at a task marker: {:?}",
            &runtime.state.doc.source[task.source.clone()]
        );
        assert!(!task.label.is_empty());
        assert_eq!(task.block, "b0");
    }
    let boxes: Vec<ElementInfo> = runtime.elements().into_iter().filter(|element| element.role == "checkbox").collect();
    assert_eq!(boxes.len(), 3, "no transparent overlay adds a second checkbox");
    assert!(boxes.iter().all(|element| element.focusable));
    assert!(boxes.iter().all(|element| element.key.as_deref().is_some_and(|key| key.starts_with("task-control-"))));
    assert!(boxes.iter().any(|element| element.label.as_deref() == Some("nested child")));
}

#[test]
fn the_block_wire_sees_every_blocks_key_line_and_drawn_text() {
    use std::cell::RefCell;

    use crate::{BlockRef, BlockWire, RenderWires};

    struct Wired {
        doc: MdDoc,
        theme: DocTheme,
        style: MarkdownStyle,
        wire: BlockWire,
    }

    let doc = lmdv_md::parse("First paragraph.\n\n- one budget\n- two budget\n\n```rust\nfn main() {}\n```\n");
    let seen: Rc<RefCell<Vec<BlockRef>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let wire: BlockWire = Rc::new(move |element, block: &BlockRef| {
        sink.borrow_mut().push(block.clone());
        element
    });
    let scene = Wired { doc, theme: DocTheme::neutral(false), style: MarkdownStyle::neutral(false), wire };
    let mut runtime = rune::Runtime::new(
        scene,
        |scene: &Wired, _: &mut Cx<Wired>| {
            let mut opts = RenderOptions::new(760.0, &scene.style, &NoMath, &NoImages);
            opts.wires = RenderWires { block: Some(scene.wire.clone()), ..RenderWires::default() };
            render(&scene.doc, &scene.theme, &opts)
        },
        (760.0, 4000.0),
        rune::Env::new(true, 1.0),
    );
    runtime.frame(0.0);

    let seen = seen.borrow();
    assert_eq!(seen.len(), runtime.state.doc.blocks.len(), "one ref per top-level block");
    let first = &seen[0];
    assert_eq!(first.key, "b0");
    assert_eq!((first.line, first.end_line), (0, 1));
    assert_eq!(first.text.trim_end(), "First paragraph.");
    assert_eq!(first.texts.len(), 1);
    assert_eq!(first.texts[0].key.as_deref(), Some("text"));
    assert_eq!(&first.text[first.texts[0].range.clone()], "First paragraph.");

    // A list: one drawn text per item, each carrying the item key the app can
    // match against `SelectedText::path()`.
    let list = &seen[1];
    assert_eq!(list.key, "b1");
    assert_eq!((list.line, list.end_line), (2, 5));
    let items: Vec<String> = list.texts.iter().filter_map(|text| text.path.first().cloned()).collect();
    assert_eq!(items, ["i0", "i1"]);
    let second = &list.texts[1];
    assert_eq!(&list.text[second.range.clone()], "two budget");

    // A fence: `code` marks the source as written, and its body is the text.
    let code = &seen[2];
    assert!(code.code && code.key == "b2");
    assert_eq!((code.line, code.end_line), (5, 8));
    assert!(code.text.trim_end().ends_with("fn main() {}"), "{:?}", code.text);
}

// ---------------------------------------------------------------------------
// Paint-only highlights (OI-043): source ranges mapped through `Inline::source_map`
// to drawn text, painted without touching selection, copy or line breaks.
// ---------------------------------------------------------------------------

#[test]
fn without_highlights_the_tree_is_byte_identical() {
    for source in [FEATURES, MATH] {
        let plain = Scene::new(source).render();
        let empty = Scene::new(source).highlights(Vec::new()).render();
        assert_eq!(plain.snapshot(), empty.snapshot(), "an empty highlight list changes nothing");
        assert!(plain.marks().is_empty() && empty.marks().is_empty());
        assert_eq!(plain.elements.len(), empty.elements.len());
    }
}

#[test]
fn highlights_paint_each_range_in_a_block() {
    let source = "alpha beta gamma delta.\n\nSecond paragraph.\n";
    let gamma = source.find("gamma").unwrap();
    let tree = Scene::new(source)
        .highlights([
            Highlight::new(0..5, HighlightKind::Find),
            Highlight::new(gamma..gamma + "gamma".len(), HighlightKind::Line),
        ])
        .render();
    assert_eq!(tree.mark_labels(), ["alpha", "gamma"]);
    let marks = tree.marks();
    let theme = DocTheme::neutral(false);
    assert!(marks.iter().all(|mark| mark.rect[2] > 0.0 && mark.rect[3] > 0.0), "each paint has a visible box");
    assert_eq!(marks[0].fill, theme.highlight_find);
    assert_eq!(marks[1].fill, theme.highlight_line);
    assert_eq!(marks[0].key.as_deref(), Some("hl-0"));
    assert_eq!(marks[1].key.as_deref(), Some("hl-1"));
}

#[test]
fn a_landing_highlight_paints_a_heading_and_yields_to_a_find_match() {
    let source = "# Alpha\n\nText\n";
    let tree = Scene::new(source).highlights([Highlight::new(0..7, HighlightKind::Landing)]).render();
    assert_eq!(tree.mark_labels(), ["Alpha"], "the heading block maps to its drawn text");
    let mark = tree.marks()[0];
    assert!(mark.rect[2] > 0.0 && mark.rect[3] > 0.0, "the band has a visible box");
    assert_eq!(mark.fill, DocTheme::neutral(false).highlight_landing);

    // A find match over the same text is the stronger paint.
    let tree = Scene::new(source)
        .highlights([Highlight::new(0..7, HighlightKind::Landing), Highlight::new(2..7, HighlightKind::Find)])
        .render();
    assert_eq!(tree.marks()[0].fill, DocTheme::neutral(false).highlight_find);
}

#[test]
fn highlights_never_move_text_or_change_what_copies() {
    let source = "alpha beta gamma delta.\n\nSecond paragraph.\n";
    let plain = Scene::new(source).render();
    let painted =
        Scene::new(source).highlights([Highlight::new(0..source.find("delta").unwrap(), HighlightKind::Find)]).render();
    let texts = |tree: &ElTree| {
        tree.elements
            .iter()
            .filter(|e| e.role != "mark")
            .map(|e| (e.path.clone(), e.text.clone(), e.rect))
            .collect::<Vec<_>>()
    };
    assert_eq!(texts(&plain), texts(&painted), "paint-only: no text or layout moved");
    assert_eq!(plain.snapshot(), painted.snapshot());
}

#[test]
fn a_highlight_crossing_blocks_is_split_per_block() {
    let source = "alpha beta gamma delta.\n\nSecond paragraph.\n";
    let start = source.find("delta").unwrap();
    let end = source.find("paragraph").unwrap() + "paragraph".len();
    let tree = Scene::new(source).highlights([Highlight::new(start..end, HighlightKind::Find)]).render();
    assert_eq!(tree.mark_labels(), ["delta.", "Second paragraph"]);
}

#[test]
fn a_highlight_inside_a_link_or_a_code_span_paints_only_drawn_text() {
    let source = "Read [the docs](https://example.com/page) and `cargo test` now.\n";
    let docs = source.find("docs").unwrap();
    let tree = Scene::new(source).highlights([Highlight::new(docs..docs + "docs".len(), HighlightKind::Find)]).render();
    assert_eq!(tree.mark_labels(), ["docs"], "the link's text is drawn; its URL is not");

    // Only the URL: it draws no text, so the link's label stands in for it (OI-127).
    let url = source.find("example.com").unwrap();
    let tree =
        Scene::new(source).highlights([Highlight::new(url..url + "example.com".len(), HighlightKind::Find)]).render();
    assert_eq!(tree.mark_labels(), ["the docs"], "a highlight over the destination paints the label");

    // A code span including its backticks still paints the code text alone.
    let open = source.find('`').unwrap();
    let close = source[open + 1..].find('`').unwrap() + open + 1;
    let tree = Scene::new(source).highlights([Highlight::new(open..close + 1, HighlightKind::Find)]).render();
    assert_eq!(tree.mark_labels(), ["cargo test"]);
}

#[test]
fn a_highlight_in_a_table_cell_or_a_list_item_paints_that_text() {
    let table = "| Name | Value |\n| --- | --- |\n| alpha | one |\n";
    let at = table.find("alpha").unwrap();
    let tree = Scene::new(table).highlights([Highlight::new(at..at + "alpha".len(), HighlightKind::Current)]).render();
    assert_eq!(tree.mark_labels(), ["alpha"]);
    assert!(tree.marks()[0].path.contains("c0"), "the mark is in the first column: {}", tree.marks()[0].path);

    let list = "- first item\n- second item\n";
    let at = list.find("second").unwrap();
    let tree =
        Scene::new(list).highlights([Highlight::new(at..at + "second item".len(), HighlightKind::Find)]).render();
    assert_eq!(tree.mark_labels(), ["second item"]);
}

#[test]
fn a_highlight_over_a_formula_paints_its_drawn_picture() {
    let source = "Before $x+y$ after.\n";
    let open = source.find('$').unwrap();
    let close = source[open + 1..].find('$').unwrap() + open + 1;

    // The whole formula maps to the object's drawn box: patch 0004 paints the
    // picture where the placeholder takes no width (OI-127).
    let formula = Scene::new(source).math().highlights([Highlight::new(open..close + 1, HighlightKind::Find)]).render();
    let marks = formula.marks();
    assert_eq!(marks.len(), 1, "one paint over the formula: {:?}", formula.mark_labels());
    assert!(
        (marks[0].rect[2] - 24.0).abs() < 0.6 && (marks[0].rect[3] - 12.0).abs() < 0.6,
        "the picture's own box, 24 by 12: {:?}",
        marks[0].rect
    );

    // The TeX after the opening delimiter is unmapped by the parser, so the
    // renderer maps it to the same object.
    let tex = Scene::new(source).math().highlights([Highlight::new(open + 1..close, HighlightKind::Find)]).render();
    assert_eq!(tex.marks().len(), 1, "a range over the TeX paints the formula");
    assert!(tex.marks()[0].rect[2] > 1.0, "and paints the picture, not an empty glyph box");

    // The prose before it stops at the object: a formula is never part of a
    // highlight that ends where its placeholder begins.
    let before = Scene::new(source).math().highlights([Highlight::new(0..open, HighlightKind::Find)]).render();
    assert_eq!(before.mark_labels(), ["Before "]);

    // A range that reaches the prose on both sides paints it, spanning the object.
    let end = source.find("after").unwrap() + "after.".len();
    let around = Scene::new(source).math().highlights([Highlight::new(0..end, HighlightKind::Find)]).render();
    let labels = around.mark_labels();
    assert_eq!(labels.len(), 1, "{labels:?}");
    assert!(labels[0].starts_with("Before "), "{:?}", labels[0]);
    assert!(labels[0].ends_with(" after."), "{:?}", labels[0]);
}

#[test]
fn a_highlight_over_an_inline_image_paints_its_drawn_box() {
    let source = "A ![diagram](./pic.png) here.\n";
    let from = source.find("![").unwrap();
    let to = source.find(')').unwrap() + 1;

    // A resolved picture draws where the placeholder takes no width; a range
    // over any of the image's syntax paints the picture's box (OI-127).
    let tree = Scene::new(source).images().highlights([Highlight::new(from..to, HighlightKind::Find)]).render();
    let marks = tree.marks();
    assert_eq!(marks.len(), 1, "one paint over the picture: {:?}", tree.mark_labels());
    assert!(
        (marks[0].rect[2] - 80.0).abs() < 0.6 && (marks[0].rect[3] - 40.0).abs() < 0.6,
        "the picture's own box, 80 by 40: {:?}",
        marks[0].rect
    );

    // Without a resolver the alt text is drawn in the object's place, so the
    // whole image's source bytes paint that fallback text.
    let tree = Scene::new(source).highlights([Highlight::new(from..to, HighlightKind::Find)]).render();
    assert_eq!(tree.mark_labels(), ["diagram"]);
}

#[test]
fn a_highlight_over_front_matter_paints_the_key_value_or_item() {
    let source = "---\ntitle: A quiet page\ntags:\n  - alpha\n  - beta\n---\n\n# A quiet page\n\nBody.\n";

    // A value in the open card.
    let value = source.find("A quiet page").unwrap();
    let tree =
        Scene::new(source).details(true).highlights([Highlight::new(value..value + 12, HighlightKind::Line)]).render();
    assert!(tree.mark_labels().contains(&"A quiet page"), "the value paints: {:?}", tree.mark_labels());

    // The key paints in the card and in the closed summary; the summary is the
    // only drawn copy while the card is closed.
    let key = source.find("title").unwrap();
    let open =
        Scene::new(source).details(true).highlights([Highlight::new(key..key + 5, HighlightKind::Find)]).render();
    assert_eq!(open.mark_labels().iter().filter(|label| **label == "title").count(), 2, "{:?}", open.mark_labels());
    let closed = Scene::new(source).highlights([Highlight::new(key..key + 5, HighlightKind::Find)]).render();
    assert_eq!(closed.mark_labels(), ["title"], "the collapsed card's summary carries the key");

    // A list item paints its own chip, not the whole list.
    let alpha = source.find("alpha").unwrap();
    let tree =
        Scene::new(source).details(true).highlights([Highlight::new(alpha..alpha + 5, HighlightKind::Find)]).render();
    assert_eq!(tree.mark_labels(), ["alpha"]);

    // An invalid block draws verbatim; its bytes paint the raw text.
    let raw = "---\n{not flat}\n---\n\nBody.\n";
    let at = raw.find("{not flat}").unwrap();
    let tree = Scene::new(raw).details(true).highlights([Highlight::new(at..at + 10, HighlightKind::Line)]).render();
    assert_eq!(tree.mark_labels(), ["{not flat}"]);
}

#[test]
fn a_highlight_over_a_heading_also_paints_its_generated_toc_label() {
    let source = "# Intro\n\n[TOC]\n\n## Second section\n\nBody.\n";
    let heading = source.find("Second section").unwrap();
    let tree = Scene::new(source).highlights([Highlight::new(heading..heading + 14, HighlightKind::Landing)]).render();
    let labels = tree.mark_labels();
    assert_eq!(labels.iter().filter(|label| **label == "Second section").count(), 2, "heading and TOC: {labels:?}");
    assert!(tree.has_key("toc-1"), "the generated row is keyed");

    // A find hit is counted once and revealed at the heading, so its earlier
    // TOC copy is not painted as well.
    let find = Scene::new(source).highlights([Highlight::new(heading..heading + 14, HighlightKind::Find)]).render();
    assert_eq!(
        find.mark_labels().iter().filter(|label| **label == "Second section").count(),
        1,
        "find paints the heading only: {:?}",
        find.mark_labels()
    );
}

#[test]
fn overlapping_highlights_paint_the_strongest_kind_once() {
    let source = "one two three\n";
    let theme = DocTheme::neutral(false);
    let identical = Scene::new(source)
        .highlights([Highlight::new(0..7, HighlightKind::Find), Highlight::new(0..7, HighlightKind::Current)])
        .render();
    assert_eq!(identical.mark_labels(), ["one two"]);
    assert_eq!(identical.marks().len(), 1, "the current kind paints once");
    assert_eq!(identical.marks()[0].fill, theme.highlight_current);

    // A partial overlap splits at the boundaries: find before, current inside.
    let partial = Scene::new(source)
        .highlights([Highlight::new(0..7, HighlightKind::Find), Highlight::new(4..7, HighlightKind::Current)])
        .render();
    assert_eq!(partial.mark_labels(), ["one ", "two"]);
    let marks = partial.marks();
    assert_eq!(marks[0].fill, theme.highlight_find);
    assert_eq!(marks[1].fill, theme.highlight_current);

    // Two touching matches stay two paints, so each keeps its own scroll key.
    let adjacent = Scene::new("abc def\n")
        .highlights([Highlight::new(0..3, HighlightKind::Find), Highlight::new(3..7, HighlightKind::Find)])
        .render();
    let keys: Vec<_> = adjacent.marks().iter().filter_map(|mark| mark.key.clone()).collect();
    assert_eq!(keys, ["hl-0", "hl-1"]);
    assert_eq!(adjacent.mark_labels(), ["abc", " def"]);
}

#[test]
fn empty_and_out_of_range_highlights_are_ignored() {
    let source = "alpha beta.\n";
    let tree = Scene::new(source)
        .highlights([
            Highlight::new(3..3, HighlightKind::Find),
            Highlight::new(source.len() + 10..source.len() + 20, HighlightKind::Current),
            Highlight::new(usize::MAX - 2..usize::MAX, HighlightKind::Line),
        ])
        .render();
    assert!(tree.marks().is_empty());
    assert_eq!(tree.snapshot(), Scene::new(source).render().snapshot());
}

#[test]
fn a_highlight_paints_inside_fenced_and_indented_code() {
    let fenced = "before\n\n```rust\nfn main() {\n    let x = 1;\n}\n```\n";
    let at = fenced.find("let x = 1;").unwrap();
    let tree =
        Scene::new(fenced).highlights([Highlight::new(at..at + "let x = 1;".len(), HighlightKind::Find)]).render();
    assert_eq!(tree.mark_labels(), ["let x = 1;"]);

    let indented = "    plain\n    code\n";
    let at = indented.find("code").unwrap();
    let tree = Scene::new(indented).highlights([Highlight::new(at..at + "code".len(), HighlightKind::Line)]).render();
    assert_eq!(tree.mark_labels(), ["code"]);

    // Four spaces make an indented block even when its first line starts with
    // backticks, so it must not be mistaken for a fence.
    let fence_like = "    ```\n    text\n";
    let at = fence_like.find("text").unwrap();
    let tree = Scene::new(fence_like).highlights([Highlight::new(at..at + "text".len(), HighlightKind::Find)]).render();
    assert_eq!(tree.mark_labels(), ["text"]);
}

#[test]
fn a_changed_highlight_repaints_through_the_block_memo() {
    use std::cell::RefCell;

    struct Live {
        doc: MdDoc,
        theme: DocTheme,
        style: MarkdownStyle,
        highlights: Rc<RefCell<Vec<Highlight>>>,
    }

    let source = "alpha beta.\n\nsecond paragraph.\n";
    let highlights = Rc::new(RefCell::new(vec![Highlight::new(0..5, HighlightKind::Find)]));
    let live = Live {
        doc: lmdv_md::parse(source),
        theme: DocTheme::neutral(false),
        style: MarkdownStyle::neutral(false),
        highlights: highlights.clone(),
    };
    let mut runtime = rune::Runtime::new(
        live,
        |live: &Live, _: &mut Cx<Live>| {
            let opts =
                RenderOptions::new(760.0, &live.style, &NoMath, &NoImages).highlights(live.highlights.borrow().clone());
            render(&live.doc, &live.theme, &opts)
        },
        (760.0, 1200.0),
        rune::Env::new(true, 1.0),
    );
    let marks =
        |runtime: &rune::Runtime<Live>| runtime.elements().into_iter().filter(|e| e.role == "mark").collect::<Vec<_>>();
    runtime.frame(0.0);
    assert_eq!(marks(&runtime).len(), 1);
    assert_eq!(marks(&runtime)[0].label.as_deref(), Some("alpha"));
    assert!(runtime.find(&crate::highlight_key(0)).is_some(), "the current match is a scroll target");

    // A new match in the second block: the memo version sees it and repaints,
    // while the first block's memo stays reusable.
    let second = source.find("second").unwrap();
    *highlights.borrow_mut() = vec![Highlight::new(second..second + "second".len(), HighlightKind::Current)];
    runtime.invalidate();
    runtime.frame(0.0);
    let painted = marks(&runtime);
    assert_eq!(painted.len(), 1);
    assert_eq!(painted[0].label.as_deref(), Some("second"));
    assert_eq!(painted[0].fill, runtime.state.theme.highlight_current);

    // The same highlights a frame later reuse the memo and paint once.
    runtime.invalidate();
    runtime.frame(0.0);
    let again = marks(&runtime);
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].label.as_deref(), Some("second"));
}

// ---------------------------------------------------------------------------
// Table wire (M-06, X-16): every drawn table hands the app its key, source
// range, alignment and drawn cells; without a wire the tree is unchanged.
// ---------------------------------------------------------------------------

#[test]
fn table_cells_keep_their_links_and_drawn_text() {
    let source = "| Name | Site |\n| :--- | ---: |\n| Docs | [site](https://example.com/x) |\n";
    let doc = lmdv_md::parse(source);
    let table = crate::table_refs(&doc).into_iter().next().expect("one table");
    assert_eq!(table.head[0].text, "Name");
    assert_eq!(table.head[1].text, "Site");
    let cell = &table.rows[0][1];
    assert_eq!(cell.text, "site", "the link's label is the cell's drawn text");
    assert_eq!(cell.links.len(), 1);
    assert_eq!(&cell.text[cell.links[0].range.clone()], "site");
    assert_eq!(cell.links[0].dest, "https://example.com/x");
    assert!(table.rows[0][0].links.is_empty() && table.rows[0][0].task.is_none());
}

#[test]
fn table_cells_collapse_whitespace_and_keep_their_link_offsets() {
    let source = "| A |\n| - |\n|  [the   docs](page.md)  |\n";
    let doc = lmdv_md::parse(source);
    let table = crate::table_refs(&doc).into_iter().next().expect("one table");
    let cell = &table.rows[0][0];
    assert_eq!(cell.text, "the docs", "runs of whitespace collapse");
    assert_eq!(&cell.text[cell.links[0].range.clone()], "the docs");
    assert_eq!(cell.links[0].dest, "page.md");
}

#[test]
fn a_table_keeps_its_formula_text_in_a_cell() {
    let source = "| A |\n| - |\n| $x^2$ |\n";
    let doc = lmdv_md::parse(source);
    let table = crate::table_refs(&doc).into_iter().next().expect("one table");
    assert_eq!(table.rows[0][0].text, "x^2");
    assert!(table.rows[0][0].links.is_empty());
}

#[test]
fn a_table_cell_shows_a_task_glyph_and_keeps_its_state() {
    use lmdv_md::{Inline, InlineObject, InlineObjectKind, Table};

    let mut cell = Inline { text: format!("{} item", lmdv_md::OBJECT_CHAR), ..Inline::default() };
    cell.objects.push(InlineObject { range: 0..3, kind: InlineObjectKind::TaskCheckbox { checked: true }, html: None });
    let table = Table { head: vec![cell], rows: Vec::new(), ..Table::default() };
    let drawn = crate::table::table_ref_for("b0", 0, &table);
    assert_eq!(drawn.head[0].text, "☑ item", "the preview shows the disabled box as a glyph");
    assert_eq!(drawn.head[0].task, Some(true), "and knows its state, so it stays inert");
}

#[test]
fn the_table_wire_sees_every_table_and_changes_no_tree() {
    use std::cell::RefCell;

    use crate::{RenderWires, TableRef, TableWire};

    struct Wired {
        doc: MdDoc,
        theme: DocTheme,
        style: MarkdownStyle,
        wire: Option<TableWire>,
    }

    let source = "| Name | Qty | Notes |\n| :--- | ---: | --- |\n| Alpha | 2 | $x^2$ |\n| Beta | 10 | **bold** text |\n\n> | In | Quote |\n> | - | - |\n> | one | two |\n";
    let seen: Rc<RefCell<Vec<TableRef>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let wire: TableWire = Rc::new(move |element, table: &TableRef| {
        sink.borrow_mut().push(table.clone());
        element
    });
    let make = |wire: Option<TableWire>| Wired {
        doc: lmdv_md::parse(source),
        theme: DocTheme::neutral(false),
        style: MarkdownStyle::neutral(false),
        wire,
    };
    let runtime = |scene: Wired| {
        let mut runtime = rune::Runtime::new(
            scene,
            |scene: &Wired, _: &mut Cx<Wired>| {
                let mut opts = RenderOptions::new(760.0, &scene.style, &NoMath, &NoImages);
                if let Some(wire) = &scene.wire {
                    opts.wires = RenderWires { table: Some(wire.clone()), ..RenderWires::default() };
                }
                render(&scene.doc, &scene.theme, &opts)
            },
            (760.0, 1200.0),
            rune::Env::new(true, 1.0),
        );
        runtime.frame(0.0);
        runtime
    };

    let plain = runtime(make(None));
    let wired = runtime(make(Some(wire)));
    let elements = |runtime: &rune::Runtime<Wired>| runtime.elements();
    assert_eq!(
        ElTree { elements: elements(&plain) }.snapshot(),
        ElTree { elements: elements(&wired) }.snapshot(),
        "a table wire that returns its element leaves the tree byte-identical"
    );

    let seen = seen.borrow();
    assert_eq!(seen.len(), 2, "one ref per drawn table, nested ones included");
    let top = &seen[0];
    assert_eq!(top.block, "b0");
    assert_eq!(top.index, 0);
    assert_eq!(top.columns, 3);
    assert_eq!(top.align, [lmdv_md::Align::Left, lmdv_md::Align::Right, lmdv_md::Align::None]);
    fn cell_texts(cells: &[crate::TableCell]) -> Vec<&str> {
        cells.iter().map(|cell| cell.text.as_str()).collect()
    }
    assert_eq!(cell_texts(&top.head), ["Name", "Qty", "Notes"]);
    assert_eq!(top.rows.len(), 2);
    assert_eq!(cell_texts(&top.rows[0]), ["Alpha", "2", "x^2"], "a formula keeps its TeX");
    assert_eq!(cell_texts(&top.rows[1]), ["Beta", "10", "bold text"], "markup disappears");
    assert_eq!(
        &source[top.source.clone()],
        source[..source.find("\n\n>").unwrap()].to_owned() + "\n",
        "the table's own source range, trailing newline included"
    );
    assert_eq!((top.line, top.end_line), (0, 4));

    assert_eq!(crate::table_refs(&plain.state.doc), *seen, "the public helper reports the same tables as the wire");

    let quoted = &seen[1];
    assert_eq!(quoted.block, "b1", "the nested table names its top-level block");
    assert_eq!(quoted.index, 1);
    assert_eq!(cell_texts(&quoted.head), ["In", "Quote"]);
    assert_eq!(cell_texts(&quoted.rows[0]), ["one", "two"]);
    assert_eq!(quoted.rows.len(), 1);
    assert!(quoted.source.start > top.source.end, "each table keeps its own source range");

    // A wire that adds chrome changes only the wired tree.
    let marker: TableWire = Rc::new(|element, _| rune::col().key("table-bar").child(element));
    let marked = runtime(make(Some(marker)));
    let tree = ElTree { elements: elements(&marked) };
    assert!(tree.has_key("table-bar"));
    assert!(!ElTree { elements: elements(&plain) }.has_key("table-bar"));
}

// ---------------------------------------------------------------------------
// Media wire (V-01–V-10): every drawn image/diagram hands the app its block,
// source, alt and natural size; without a wire the tree is unchanged.
// ---------------------------------------------------------------------------

#[test]
fn the_media_wire_sees_images_and_diagrams_and_changes_no_tree() {
    use std::cell::RefCell;

    use crate::{MediaKind, MediaRef, MediaWire, RenderWires};

    struct Wired {
        doc: MdDoc,
        theme: DocTheme,
        style: MarkdownStyle,
        images: WiredImages,
        wire: Option<MediaWire>,
    }

    #[derive(Clone)]
    struct WiredImages(std::sync::Arc<rune::Image>);

    impl crate::ImageResolver for WiredImages {
        fn resolve(&self, _src: &str, _alt: &str, _title: &str) -> Option<rune::Image> {
            Some((*self.0).clone())
        }
    }

    let cache = rune::ImageCache::new(std::sync::Arc::new(|| {}));
    let picture = cache.rgba(4, 2, vec![0x80; 4 * 2 * 4]);
    picture.wait().expect("the fixture decodes");
    let source = "![Alt text](./picture.png)\n\n```mermaid\nflowchart LR\n    A --> B\n```\n";
    let seen: Rc<RefCell<Vec<MediaRef>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let wire: MediaWire = Rc::new(move |element, media: &MediaRef| {
        sink.borrow_mut().push(media.clone());
        element
    });
    let make = |wire: Option<MediaWire>| Wired {
        doc: lmdv_md::parse(source),
        theme: DocTheme::neutral(false),
        style: MarkdownStyle::neutral(false),
        images: WiredImages(std::sync::Arc::new(picture.clone())),
        wire,
    };
    let runtime = |scene: Wired| {
        let mut runtime = rune::Runtime::new(
            scene,
            |scene: &Wired, _: &mut Cx<Wired>| {
                let mut opts = RenderOptions::new(760.0, &scene.style, &NoMath, &scene.images);
                if let Some(wire) = &scene.wire {
                    opts.wires = RenderWires { media: Some(wire.clone()), ..RenderWires::default() };
                }
                render(&scene.doc, &scene.theme, &opts)
            },
            (760.0, 1200.0),
            rune::Env::new(true, 1.0),
        );
        runtime.frame(0.0);
        runtime
    };

    let plain = runtime(make(None));
    let wired = runtime(make(Some(wire)));
    let elements = |runtime: &rune::Runtime<Wired>| runtime.elements();
    assert_eq!(
        ElTree { elements: elements(&plain) }.snapshot(),
        ElTree { elements: elements(&wired) }.snapshot(),
        "a media wire that returns its element leaves the tree byte-identical"
    );

    let seen = seen.borrow();
    assert_eq!(seen.len(), 2, "one ref per drawn image and diagram");
    let image = &seen[0];
    assert_eq!(image.kind, MediaKind::Image);
    assert_eq!(image.block, "b0");
    assert_eq!(image.index, 0);
    assert_eq!(image.source, "./picture.png");
    assert_eq!(image.alt, "Alt text");
    assert_eq!((image.width, image.height), (4.0, 2.0), "natural pixels, not the drawn size");
    assert_eq!((image.line, image.end_line), (0, 1));

    let diagram = &seen[1];
    assert_eq!(diagram.kind, MediaKind::Diagram);
    assert_eq!(diagram.block, "b1");
    assert_eq!(diagram.index, 1);
    assert!(diagram.source.contains("flowchart LR"), "the diagram carries its Mermaid source");
    assert_eq!(diagram.alt, "");
    // OI-140: the app re-measures at the window width when the preview opens,
    // so the renderer does not lay the diagram out a second time.
    assert_eq!((diagram.width, diagram.height), (0.0, 0.0), "no second layout: {diagram:?}");
    assert_eq!((diagram.line, diagram.end_line), (2, 6));

    let marker: MediaWire = Rc::new(|element, _| rune::col().key("media-chrome").child(element));
    let marked = runtime(make(Some(marker)));
    let tree = ElTree { elements: elements(&marked) };
    assert!(tree.has_key("media-chrome"));
    assert!(!ElTree { elements: elements(&plain) }.has_key("media-chrome"));
}

#[test]
fn missing_images_and_unsupported_diagrams_never_reach_the_media_wire() {
    use std::cell::RefCell;

    use crate::{MediaRef, MediaWire};

    struct Sink {
        doc: MdDoc,
        theme: DocTheme,
        style: MarkdownStyle,
        wire: MediaWire,
    }

    let source = "![Missing](./gone.png)\n\n```mermaid\nnot a diagram {{{\n```\n";
    let seen: Rc<RefCell<Vec<MediaRef>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let wire: MediaWire = Rc::new(move |element, media: &MediaRef| {
        sink.borrow_mut().push(media.clone());
        element
    });
    let scene = Sink {
        doc: lmdv_md::parse(source),
        theme: DocTheme::neutral(false),
        style: MarkdownStyle::neutral(false),
        wire,
    };
    let mut runtime = rune::Runtime::new(
        scene,
        |scene: &Sink, _: &mut Cx<Sink>| {
            let opts = RenderOptions::new(760.0, &scene.style, &NoMath, &NoImages).media_wire(scene.wire.clone());
            render(&scene.doc, &scene.theme, &opts)
        },
        (760.0, 900.0),
        rune::Env::new(true, 1.0),
    );
    runtime.frame(0.0);
    assert!(seen.borrow().is_empty(), "a broken image and an unsupported diagram are inert");
}
