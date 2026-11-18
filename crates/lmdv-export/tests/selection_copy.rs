//! End-to-end tests for selection copy (X-03): rich fragments for a top-level
//! block range and the source-range mapping the application uses.

use std::cell::Cell;

use lmdv_export::{
    AssetResolver, ExportBlock, ExportDoc, ExportOptions, ExportedAsset, NoAssets, RichOptions, blocks_for_source,
    export_rich_fragment, export_rich_html,
};

/// The same fake PNG as the whole-document rich-copy tests: enough to prove the
/// data URI is embedded and to compare the bytes.
fn fake_png() -> ExportedAsset {
    ExportedAsset::new("image/png", b"\x89PNG\r\n\x1a\nlmdv-selection".to_vec())
}

#[derive(Default)]
struct TestAssets {
    image_calls: Cell<usize>,
}

impl AssetResolver for TestAssets {
    fn image(&self, _src: &str, _alt: &str) -> Option<ExportedAsset> {
        self.image_calls.set(self.image_calls.get() + 1);
        Some(fake_png())
    }

    fn diagram(&self, _source: &str) -> Option<ExportedAsset> {
        None
    }
}

fn export_doc(markdown: &str) -> ExportDoc {
    ExportDoc::from_md(&lmdv_md::parse(markdown), &ExportOptions::default(), &NoAssets).unwrap()
}

fn export_doc_with_assets(markdown: &str, assets: &dyn AssetResolver) -> ExportDoc {
    ExportDoc::from_md(&lmdv_md::parse(markdown), &ExportOptions::default(), assets).unwrap()
}

/// A range from runtime values, so a deliberately reversed span does not read
/// as a typo to the reversed-empty-range lint.
fn span(start: usize, end: usize) -> std::ops::Range<usize> {
    start..end
}

#[test]
fn a_fragment_holds_only_the_selected_blocks() {
    let markdown = "# Title\n\nOpening **bold** and [link](https://example.com).\n\n## Section\n\n\
                    - [x] done\n- [ ] todo\n\n```rust\nfn main() {}\n```\n\n\
                    > [!NOTE]\n> Careful.\n\n---\n";
    let doc = export_doc(markdown);
    assert_eq!(doc.blocks.len(), 7, "heading, para, heading, list, code, callout, rule");

    // Paragraph + section heading + task list: the same markup as the whole
    // document copy, but none of the surrounding blocks.
    let html = export_rich_fragment(&doc, 1..4, &RichOptions::default());
    assert!(html.starts_with("<div role=\"document\" style="), "no title, no aria-label: {html:.120}");
    assert!(html.ends_with("</div>"));
    assert!(html.contains("Opening"));
    assert!(html.contains("<strong style=\"font-weight:600\">bold</strong>"));
    assert!(
        html.contains("<a href=\"https://example.com\" style=\"color:#0969da;text-decoration:underline\">link</a>")
    );
    assert!(html.contains("<h2 "));
    assert!(html.contains(">Section</h2>"));
    assert!(html.contains('\u{2611}'), "checked task: {html}");
    assert!(html.contains('\u{2610}'), "unchecked task");
    assert!(!html.contains("Title"), "the title block is outside the range");
    assert!(!html.contains("fn main"), "the code block is outside the range");
    assert!(!html.contains("Careful"), "the callout is outside the range");
    assert!(!html.contains("<hr"), "the rule is outside the range");
    for banned in ["<style", "<script", "class=", "<link", "@media", "@import", "<input", "<button", "onclick"] {
        assert!(!html.to_ascii_lowercase().contains(banned), "the fragment still carries {banned}");
    }
}

#[test]
fn images_math_and_inline_styles_survive_a_selection() {
    let assets = TestAssets::default();
    let markdown = "Before.\n\nSelected ![pixel](pic.png) with **bold** $x^2$ \
                    and [link](https://example.com).\n\nAfter.\n";
    let doc = export_doc_with_assets(markdown, &assets);
    let html = export_rich_fragment(&doc, 1..2, &RichOptions::default());

    assert!(html.contains("<img src=\"data:image/png;base64,"), "the picture is embedded: {html}");
    assert!(html.contains("alt=\"pixel\""));
    assert!(html.contains("alt=\"x^2\""), "the formula keeps its TeX alt");
    assert!(html.contains("src=\"data:image/png;base64,"));
    assert!(html.contains("<strong style=\"font-weight:600\">bold</strong>"));
    assert!(html.contains("href=\"https://example.com\""));
    assert!(!html.contains("Before"));
    assert!(!html.contains("After"));
}

#[test]
fn ranges_are_clamped_and_empty_ones_are_the_empty_wrapper() {
    let doc = export_doc("# One\n\nTwo.\n");
    let empty = export_rich_fragment(&doc, 1..1, &RichOptions::default());
    assert!(empty.starts_with("<div role=\"document\" style="));
    assert!(empty.ends_with("></div>"), "an empty wrapper: {empty}");

    for range in [span(5, 9), span(2, 2), span(1, 0)] {
        assert_eq!(export_rich_fragment(&doc, range.clone(), &RichOptions::default()), empty, "{range:?}");
    }
    // A range past the end clamps instead of panicking.
    let last = export_rich_fragment(&doc, 1..usize::MAX, &RichOptions::default());
    assert!(last.contains("Two."));
    assert!(!last.contains("One"));
    // A reversed range with a valid start is empty at that start.
    assert_eq!(
        export_rich_fragment(&doc, span(2, 1), &RichOptions::default()),
        export_rich_fragment(&doc, 2..2, &RichOptions::default())
    );
}

#[test]
fn the_document_title_is_not_a_selection_label() {
    let doc = export_doc("# Notes\n\nBody.\n");
    let selection = export_rich_fragment(&doc, 1..2, &RichOptions::default());
    assert!(!selection.contains("aria-label"), "{selection}");

    let named = export_rich_fragment(
        &doc,
        1..2,
        &RichOptions { title: Some("Quoted \"notes\"".to_owned()), ..RichOptions::default() },
    );
    assert!(named.starts_with("<div role=\"document\" aria-label=\"Quoted &quot;notes&quot;\""), "{named}");

    // The whole-document copy still names itself after the document.
    let whole = export_rich_html(&doc, &RichOptions::default());
    assert!(whole.contains("aria-label=\"Notes\""), "{whole:.120}");
}

#[test]
fn source_ranges_map_to_the_blocks_they_touch() {
    let markdown = "# One\n\nFirst para with **bold**.\n\nSecond para.\n";
    let doc = export_doc(markdown);
    let first = markdown.find("First").unwrap();
    let second = markdown.find("Second").unwrap();

    assert_eq!(blocks_for_source(&doc, first..first + 5), 1..2, "inside one paragraph");
    assert_eq!(blocks_for_source(&doc, first..second + 6), 1..3, "both paragraphs");
    assert_eq!(blocks_for_source(&doc, 0..first + 1), 0..2, "the heading and the first paragraph");
    // A selection ending exactly where the next block starts does not include
    // that next block.
    assert_eq!(blocks_for_source(&doc, first..second), 1..2);
    // No selected bytes and no block at the offset: nothing to copy.
    assert_eq!(blocks_for_source(&doc, 0..0), 0..0);
    assert_eq!(blocks_for_source(&doc, markdown.len()..markdown.len() + 10), 0..0);

    assert!(doc.block_source(0).is_some_and(|source| source.start == 0), "the heading starts at 0");
    assert!(doc.block_source(1).is_some_and(|source| source.contains(&first)));
    assert_eq!(doc.block_source(99), None);
}

#[test]
fn front_matter_and_separators_have_no_blocks() {
    let markdown = "---\ntitle: Hidden\n---\n\n# One\n\nBody.\n";
    let doc = export_doc(markdown);
    let front = 0..markdown.find("---\n\n#").unwrap();
    assert_eq!(blocks_for_source(&doc, front), 0..0, "a selection in the front matter");
}

#[test]
fn an_inserted_toc_shifts_block_indices_but_not_source_mapping() {
    let markdown = "# One\n\nBody.\n";
    let opts = ExportOptions { include_toc: true, ..ExportOptions::default() };
    let doc = ExportDoc::from_md(&lmdv_md::parse(markdown), &opts, &NoAssets).unwrap();
    assert_eq!(doc.blocks.len(), 3);
    assert!(matches!(doc.blocks[0], ExportBlock::Toc { .. }), "the opt-in TOC is inserted first");
    assert_eq!(doc.block_source(0), None, "a generated TOC has no source");

    let body = markdown.find("Body").unwrap();
    assert_eq!(blocks_for_source(&doc, body..body + 4), 2..3, "the body is export block 2, not 1");
}

#[test]
fn a_model_without_spans_has_no_source_mapping() {
    let doc = ExportDoc { blocks: vec![ExportBlock::Rule], ..ExportDoc::default() };
    assert_eq!(blocks_for_source(&doc, 0..10), 0..0);
    assert_eq!(doc.block_source(0), None);
}
