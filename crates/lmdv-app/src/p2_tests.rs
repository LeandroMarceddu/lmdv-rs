//! P2 integration: the real fixtures parsed by `lmdv-md`, rendered by
//! `lmdv-render` in a `rune::Runtime`, and driven through the app's reader,
//! outline, links, `--line` and positions.
//!
//! These are the P2 gate's structural checks: roles, keys, labels and text come
//! out of the element tree, the same data `--tree` prints.

use std::cell::Cell;
use std::io::Write;
use std::rc::Rc;

use lmdv_core::{DocStatus, ScrollAnchor};
use lmdv_render::{FOOTNOTES_KEY, FRONT_MATTER_KEY, TOC_KEY, heading_key};
use rune::{ElementInfo, Tx};
use tempfile::{NamedTempFile, TempDir};

use crate::reader::ARTICLE_KEY;
use crate::test_support;
use crate::view::view;
use crate::window::{Tab, WindowState};

const FEATURES: &str = include_str!("../../../tests/fixtures/test-features.md");
const MATH: &str = include_str!("../../../tests/fixtures/commonmark-math.md");

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(test_support::shared(temp))
}

fn render(state: WindowState) -> Vec<ElementInfo> {
    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.elements()
}

fn runtime(state: WindowState) -> rune::Runtime<WindowState> {
    rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0))
}

fn has_key(elements: &[ElementInfo], key: &str) -> bool {
    elements.iter().any(|element| element.key.as_deref() == Some(key))
}

fn has_text(elements: &[ElementInfo], needle: &str) -> bool {
    elements.iter().any(|element| element.text.as_deref().is_some_and(|text| text.contains(needle)))
}

fn role<'a>(elements: &'a [ElementInfo], role: &str) -> Vec<&'a ElementInfo> {
    elements.iter().filter(|element| element.role == role).collect()
}

#[test]
fn the_features_fixture_renders_headings_tables_code_and_mermaid() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let tab = state.open_synthetic("Features", FEATURES);
    let model = state.tab(tab).and_then(Tab::document).map(|doc| doc.id).expect("doc");
    let doc = state.shared().latest_parsed(model).and_then(|parsed| parsed.result.ok()).expect("parsed");
    assert!(doc.headings.len() >= 12, "the fixture's headings are all in the model");

    let elements = render(state);
    let headings = role(&elements, "heading");
    assert!(
        headings.len() >= doc.headings.len(),
        "every document heading keeps its role: {} drawn, {} in the model",
        headings.len(),
        doc.headings.len()
    );
    for (index, heading) in doc.headings.iter().enumerate() {
        let key = heading_key(index);
        let drawn = &elements
            .iter()
            .find(|element| element.key.as_deref() == Some(key.as_str()))
            .unwrap_or_else(|| panic!("heading {index} ({key}) is missing"));
        assert_eq!(drawn.role, "heading");
        assert_eq!(drawn.text.as_deref(), Some(heading.text.as_str()));
        assert_eq!(drawn.level, Some(u32::from(heading.level)));
    }
    assert!(has_key(&elements, "b0"), "the first block keeps its source key");

    // GFM table header and body cells.
    for cell in ["System", "Role", "Notes", "Legacy PIM", "Akeneo", "Azure Service Bus", "Middleware"] {
        assert!(has_text(&elements, cell), "missing table cell {cell:?}");
    }
    assert!(role(&elements, "table").iter().any(|table| table.label.as_deref().unwrap_or("").contains("System")));

    // Code blocks with their language labels and copy action.
    assert!(has_text(&elements, "export async function openFile(path: string): Promise<void> {"));
    assert!(has_text(&elements, "typescript"));
    assert!(has_text(&elements, "python"));

    // The valid Mermaid diagram draws; the invalid one falls back to its source.
    assert!(has_key(&elements, "diagram"), "the valid diagram is drawn");
    assert!(has_text(&elements, "this is not valid mermaid"), "the invalid diagram keeps its source");
    assert!(has_text(&elements, "Invalid Mermaid diagram"), "and says why");

    // Callout cards, footnote entries and their references.
    let notes: Vec<&str> = role(&elements, "note").iter().filter_map(|note| note.label.as_deref()).collect();
    for title in ["Note", "Tip", "Important", "Warning", "Caution"] {
        assert!(notes.contains(&title), "missing callout {title}: {notes:?}");
    }
    assert!(has_key(&elements, FOOTNOTES_KEY));
    assert!(has_key(&elements, "fn-1"));
    assert!(has_key(&elements, "fn:1"));
    assert!(has_text(&elements, "The first footnote, with emphasis and a link."));

    // The front matter card is a collapsed note.
    let front = elements
        .iter()
        .find(|element| element.key.as_deref() == Some(FRONT_MATTER_KEY))
        .expect("the front-matter card");
    assert_eq!(front.role, "note");
    assert_eq!(front.expanded, Some(false));
}

#[test]
fn a_missing_picture_renders_the_placeholder_not_an_error() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Picture", "![A missing picture](./nothing-here.png)\n");
    let elements = render(state);
    let image = elements
        .iter()
        .find(|element| element.role == "image" && element.label.as_deref() == Some("A missing picture"))
        .expect("the missing-image placeholder names its alt");
    assert_eq!(image.label.as_deref(), Some("A missing picture"));
    assert!(!has_text(&elements, "could not be opened"), "a broken picture is not an error card");
}

#[test]
fn an_html_fragment_block_renders_its_allowed_children() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Html", "<div>An <kbd>Enter</kbd> key and <strong>bold</strong> text</div>\n");
    let elements = render(state);
    assert!(has_text(&elements, "An Enter key and bold text"), "the fragment's text is drawn");
    assert!(!has_text(&elements, "<div>"), "the tags themselves are not drawn");
}

#[test]
fn a_toc_marker_builds_a_real_navigation() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Contents", "[TOC]\n\n# One\n\n## Two\n\n### Three\n");
    let elements = render(state);
    let toc = elements
        .iter()
        .find(|element| element.key.as_deref() == Some(TOC_KEY))
        .expect("the generated table of contents");
    assert_eq!(toc.role, "navigation");
    for index in 0..3 {
        assert!(has_key(&elements, &format!("toc-{index}")), "toc entry {index}");
    }
    let link = elements.iter().find(|element| element.key.as_deref() == Some("#one")).expect("the toc link");
    assert_eq!(link.role, "link");
}

#[test]
fn the_math_fixture_draws_formulas_and_caches_them() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Math", MATH);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let elements = runtime.elements();
    let math_blocks = role(&elements, "math");
    assert!(!math_blocks.is_empty(), "display formulas keep their math role");
    for block in &math_blocks {
        assert!(block.label.as_deref().unwrap_or("").starts_with("Math: "));
    }
    assert!(!has_text(&elements, "x^2 + y^2 = z^2"), "a rendered formula is an image, not its TeX");

    // One frame cached pictures; a second frame re-reads them without another
    // raster pass (OI-020).
    let mut cached = 0;
    let mut renders = 0;
    runtime.update(|state, _| {
        cached = state.math.cached();
        renders = state.math.renders();
    });
    assert!(cached > 0, "the math cache retained the formulas");
    runtime.frame(1.0 / 60.0);
    let mut after = 0;
    runtime.update(|state, _| after = state.math.renders());
    assert_eq!(after, renders, "a second frame re-used every formula");
}

#[test]
fn a_broken_formula_falls_back_to_its_tex() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Broken", "Bad $\\frac{1}$ formula.\n");
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let elements = runtime.elements();
    assert!(has_text(&elements, "\\frac{1}"), "the failed formula keeps its TeX visible");
    let mut errors = 0;
    runtime.update(|state, _| errors = state.math.error_count());
    assert!(errors > 0, "the failure is cached (OI-021)");
}

#[test]
fn the_link_wire_is_attached_to_linked_text() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic(
        "Links",
        "Read [the nested note](./nested/deeper.md) and [Anthropic](https://www.anthropic.com).\n",
    );
    let elements = render(state);
    let links = role(&elements, "link");
    assert!(links.len() >= 2, "both links are pressable text: {links:#?}");
    assert!(has_key(&elements, "./nested/deeper.md"));
    assert!(has_key(&elements, "https://www.anthropic.com"));
}

/// A page long enough to scroll at the test viewport height.
fn long_page() -> String {
    let mut source = String::from("# One\n\n");
    for index in 0..32 {
        source.push_str(&format!("Paragraph {index} with enough words to take a line of the reading column.\n\n"));
    }
    source.push_str("## Second section\n\n");
    for index in 0..8 {
        source.push_str(&format!("Closing paragraph {index} after the second heading.\n\n"));
    }
    source
}

#[test]
fn a_fragment_link_scrolls_to_its_heading() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Fragments", &long_page());
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.update(|state, tx| state.open_link("#second-section", tx));
    // The fragment jump glides (OI-156): give the spring its travel time.
    for _ in 0..40 {
        runtime.frame(1.0 / 60.0);
    }

    let article = runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some(ARTICLE_KEY))
        .expect("the article viewport");
    assert!(article.scrolled[1] > 0.0, "the article moved to the heading: {}", article.scrolled[1]);
}

#[test]
fn a_details_link_toggles_the_front_matter_card() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Card", "---\ntitle: A page\n---\n\n# Body\n");
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.update(|state, tx| state.open_link("details:front-matter", tx));
    runtime.frame(0.0);

    let card = runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some(FRONT_MATTER_KEY))
        .expect("the front-matter card");
    assert_eq!(card.expanded, Some(true), "the card opened through the link wire");
}

#[test]
fn a_line_target_scrolls_the_article_to_its_block() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Lines", FEATURES);
    state.set_pending_line(90);

    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.update(|state, tx| {
        state.apply_line_target(1.0, tx);
    });
    runtime.frame(0.0);

    let article = runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some(ARTICLE_KEY))
        .expect("the article viewport");
    assert!(article.scrolled[1] > 0.0, "--line moved the article to block: {}", article.scrolled[1]);
}

#[test]
fn a_heading_position_restores_after_the_model_is_on_screen() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Position", &long_page());
    let model = state.active_reader().expect("parsed");
    let anchor = ScrollAnchor { id: model.headings[1].id.clone(), offset: 40.0 };
    state.begin_restore(&anchor);

    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.update(|state, tx| state.apply_pending_scroll(tx));
    runtime.frame(0.0);

    let article = runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some(ARTICLE_KEY))
        .expect("the article viewport");
    assert!(article.scrolled[1] > 0.0, "the remembered heading was restored: {}", article.scrolled[1]);
}

#[test]
fn a_file_load_parses_on_a_job_and_feeds_every_tab() {
    let temp = TempDir::new().unwrap();
    let mut file = NamedTempFile::new_in(temp.path()).unwrap();
    write!(file, "# Welcome\n\nA body paragraph.\n\n## Next section\n\nMore text.\n").unwrap();
    file.flush().unwrap();

    let mut state = window(&temp);
    state.open_path(file.path().to_path_buf());
    wait_for_model(&mut state);

    let tab = state.active_tab().expect("the file tab");
    let model = tab.view.reader.as_ref().expect("the parse landed");
    assert_eq!(model.revision, state.active_tab().and_then(Tab::document).map(|doc| doc.revision()).unwrap());
    assert_eq!(
        model.headings.iter().map(|heading| heading.text.as_str()).collect::<Vec<_>>(),
        ["Welcome", "Next section"]
    );
    assert!(matches!(*tab.document().expect("doc").status.borrow(), DocStatus::Ready));
}

fn wait_for_model(state: &mut WindowState) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.active_tab().is_none_or(|tab| tab.view.reader.is_none()) {
        assert!(std::time::Instant::now() < deadline, "the parse job never reported");
        let mut tx = Tx::default();
        state.poll(0.0, &mut tx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn an_edit_requests_a_new_parse_and_keeps_the_last_good_model_meanwhile() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Edit", "# First\n");
    let doc = state.active_tab().and_then(Tab::document).cloned().expect("doc");
    let before = state.active_reader().expect("the first model");
    let revision = doc.revision();

    doc.apply(0..7, "# Second");
    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    let during = state.active_reader().expect("the last good model is still drawn");
    assert_eq!(during.revision, revision, "the old model stays until the new parse lands");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.active_reader().is_some_and(|model| model.revision != doc.revision()) {
        assert!(std::time::Instant::now() < deadline, "the edit's parse never reported");
        let mut tx = Tx::default();
        state.poll(0.0, &mut tx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let after = state.active_reader().expect("the new model");
    assert_eq!(after.headings[0].text, "Second");
    assert_eq!(after.revision, doc.revision());
    assert_ne!(before.revision, after.revision);
}

#[test]
fn an_oversized_document_keeps_the_source_preview_without_parsing() {
    let temp = TempDir::new().unwrap();
    let shared = test_support::shared(&temp);
    let mut state = WindowState::new(shared.clone());
    let text = "x".repeat(crate::reader::PARSE_MAX_BYTES + 1);
    let id = shared.docs_mut().allocate_id();
    let doc = Rc::new(lmdv_core::Document::ready(
        id,
        lmdv_core::DocumentKey::Untitled(shared.allocate_paste()),
        "Huge",
        lmdv_core::SourceBuffer::new(&text),
    ));
    shared.docs_mut().insert_ready(doc.clone());
    state.attach_document(doc);

    let mut tx = Tx::default();
    state.poll(0.0, &mut tx);
    let tab = state.active_tab().expect("the tab");
    assert!(tab.view.reader.is_none(), "an oversized document never reaches the parser");
    assert!(tab.view.reader_error.as_deref().unwrap_or("").contains("too large"));
    assert!(state.worker().is_empty(), "no parse job was queued");
}

#[test]
fn a_parse_result_is_cached_latest_wins_per_document() {
    use std::sync::Arc;

    let temp = TempDir::new().unwrap();
    let shared = test_support::shared(&temp);
    let id = lmdv_core::DocumentId(7);

    let newer = Arc::new(lmdv_md::parse_with_revision("# Newer\n", 5));
    assert!(shared.store_parsed(id, 5, Ok(newer)));
    let stale = Arc::new(lmdv_md::parse_with_revision("# Stale\n", 4));
    assert!(!shared.store_parsed(id, 4, Ok(stale)), "an older revision is dropped");
    assert_eq!(shared.latest_parsed(id).map(|parsed| parsed.revision), Some(5));

    assert!(!shared.store_parsed(id, 5, Err("broken".to_owned())), "a failure never replaces a good model");
    assert!(shared.parsed(id, 5).is_some_and(|result| result.is_ok()));
}

#[test]
fn an_image_becomes_ready_on_a_later_frame() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    std::fs::write(root.join("dot.png"), PNG).unwrap();
    let images = crate::images::ImageCacheState::new(crate::images::quiet_waker(), true);
    let image = images.resolve(root, "dot.png", 32.0, 2.0).expect("the path resolves");
    image.wait().expect("the picture decodes");
    let ready = Rc::new(Cell::new(images.refresh()));
    assert!(ready.get(), "the ready count changed");

    // The same document drawn again gets a live handle without a new decode.
    assert!(images.resolve(root, "dot.png", 32.0, 2.0).is_some());
    assert_eq!(images.known(), 1);
}

/// A valid 1x1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49,
    0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[test]
fn readme_position_round_trips_exactly() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let readme = state.open_synthetic("README", include_str!("../../../README.md"));
    let other = state.open_synthetic("Other", &long_page());
    state.activate_tab(readme);
    let mut runtime = runtime(state);
    runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
    runtime.frame(0.0);
    for _ in 0..5 {
        runtime.wheel_event(rune::WheelEvent::new(0.0, -120.0).at(500.0, 400.0).precise(false));
        for _ in 0..5 {
            runtime.frame(1.0 / 60.0);
        }
    }
    for _ in 0..30 {
        runtime.frame(1.0 / 60.0);
    }
    runtime.update(|state, tx| {
        state.poll(10.0, tx);
        state.poll(10.4, tx);
    });
    let at = runtime.state.reading.scroll.get();
    let anchor = runtime.state.tab(readme).unwrap().view.anchor.clone();

    runtime.update(|state, tx| {
        state.activate_tab_with_tx(other, tx);
    });
    for _ in 0..10 {
        runtime.frame(1.0 / 60.0);
    }
    runtime.update(|state, tx| {
        state.activate_tab_with_tx(readme, tx);
    });
    for _ in 0..30 {
        runtime.frame(1.0 / 60.0);
    }
    let back = runtime.state.reading.scroll.get();
    assert!((back.offset - at.offset).abs() < 2.0, "reopened at the same place: {at:?} vs {back:?}");
    assert_eq!(runtime.state.tab(readme).unwrap().view.anchor.id, anchor.id);
}

#[test]
fn a_second_view_of_the_same_document_starts_at_its_own_top() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("Long", &long_page());
    let doc = state.tab(first).unwrap().document().unwrap().clone();
    let second = state.attach_document(doc);
    state.activate_tab(first);
    let mut runtime = runtime(state);
    runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
    runtime.frame(0.0);
    runtime.update(|_, tx| tx.scroll_to_y("article-scroll", 600.0));
    runtime.frame(0.0);
    runtime.update(|state, tx| {
        state.poll(0.0, tx);
        state.poll(0.4, tx);
    });
    assert_eq!(runtime.state.reading.scroll.get().offset, 600.0);

    runtime.update(|state, _| {
        state.activate_tab(second);
    });
    runtime.frame(1.0 / 60.0);
    assert_eq!(
        runtime.state.reading.scroll.get().offset,
        0.0,
        "the second view has its own top even though the viewport key is shared"
    );
}

#[test]
fn switching_to_a_short_page_resets_the_previous_document_offset() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("Long", &long_page());
    let readme = state.open_synthetic("README", include_str!("../../../README.md"));
    state.activate_tab(first);
    let mut runtime = runtime(state);
    runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
    runtime.frame(0.0);
    runtime.update(|_, tx| tx.scroll_to_y("article-scroll", 600.0));
    runtime.frame(0.0);
    runtime.update(|state, tx| {
        state.poll(0.0, tx);
        state.poll(0.4, tx);
    });
    assert_eq!(runtime.state.reading.scroll.get().offset, 600.0);

    // The tab strip's handler path issues the restore in the same transaction.
    runtime.update(|state, tx| {
        state.activate_tab_with_tx(readme, tx);
    });
    runtime.frame(1.0 / 60.0);
    let opened = runtime.state.reading.scroll.get();
    assert_eq!(opened.offset, 0.0, "README opens at its own top, not the previous document's offset");
    assert_eq!(
        crate::section::progress_width(opened),
        Some(0.0),
        "the hairline starts empty, it does not flash part way"
    );
    for _ in 0..8 {
        runtime.frame(1.0 / 60.0);
        assert_eq!(runtime.state.reading.scroll.get().offset, 0.0);
    }

    // Switching back restores the long page's own place.
    runtime.update(|state, tx| {
        state.activate_tab_with_tx(first, tx);
    });
    runtime.frame(1.0 / 60.0);
    assert!(
        (runtime.state.reading.scroll.get().offset - 600.0).abs() < 2.0,
        "the long page returns to its reading place"
    );

    // Even a switch without a Tx (the poll-driven path) must reset the top.
    runtime.update(|state, _| {
        state.activate_tab(readme);
    });
    runtime.frame(1.0 / 60.0);
    assert_eq!(runtime.state.reading.scroll.get().offset, 0.0);
}

#[test]
fn a_remembered_heading_wins_over_the_inherited_offset() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("Long", &long_page());
    let second = state.open_synthetic("README", include_str!("../../../README.md"));
    state.activate_tab(first);
    let mut runtime = runtime(state);
    runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
    runtime.frame(0.0);
    runtime.update(|_, tx| tx.scroll_to_y("article-scroll", 600.0));
    runtime.frame(0.0);
    runtime.update(|state, tx| {
        state.poll(0.0, tx);
        state.poll(0.4, tx);
    });
    assert_eq!(runtime.state.reading.scroll.get().offset, 600.0);

    let anchor = {
        let doc = runtime.state.tab(second).and_then(|tab| tab.view.reader.clone()).expect("parsed");
        ScrollAnchor { id: doc.headings[1].id.clone(), offset: 40.0 }
    };
    runtime.update(|state, _| {
        if let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == second) {
            tab.view.anchor = anchor;
        }
    });
    runtime.update(|state, tx| {
        state.activate_tab_with_tx(second, tx);
    });
    runtime.frame(1.0 / 60.0);

    let elements = runtime.elements();
    let article = elements
        .iter()
        .find(|element| !element.exiting && element.key.as_deref() == Some(crate::reader::ARTICLE_SCROLL_KEY))
        .expect("the article viewport");
    let heading = elements
        .iter()
        .find(|element| !element.exiting && element.key.as_deref() == Some(heading_key(1).as_str()))
        .expect("the remembered heading");
    let above = article.rect[1] - heading.rect[1];
    assert!(
        (above - 40.0).abs() < 2.0,
        "the heading sits 40 pt above the viewport, not at the inherited offset: {above}"
    );
}
