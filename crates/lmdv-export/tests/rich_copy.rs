//! End-to-end tests for the rich-text clipboard fragment (X-02).

use std::cell::Cell;
use std::fs;
use std::path::Path;

use base64::Engine as _;
use lmdv_export::{
    AssetResolver, ExportDoc, ExportOptions, ExportWarningKind, ExportedAsset, NoAssets, RichOptions, export_rich_html,
};

/// The PNG signature plus a few bytes: enough to prove a data URI is embedded
/// and base64-encoded. The exporter does not decode resolver bytes.
fn fake_png() -> ExportedAsset {
    ExportedAsset::new("image/png", b"\x89PNG\r\n\x1a\nlmdv-rich".to_vec())
}

struct TestAssets {
    image: Option<ExportedAsset>,
    diagram: Option<ExportedAsset>,
    image_calls: Cell<usize>,
    diagram_calls: Cell<usize>,
}

impl TestAssets {
    fn none() -> Self {
        Self { image: None, diagram: None, image_calls: Cell::new(0), diagram_calls: Cell::new(0) }
    }

    fn with_images() -> Self {
        Self { image: Some(fake_png()), diagram: Some(fake_png()), ..Self::none() }
    }
}

impl AssetResolver for TestAssets {
    fn image(&self, _src: &str, _alt: &str) -> Option<ExportedAsset> {
        self.image_calls.set(self.image_calls.get() + 1);
        self.image.clone()
    }

    fn diagram(&self, _source: &str) -> Option<ExportedAsset> {
        self.diagram_calls.set(self.diagram_calls.get() + 1);
        self.diagram.clone()
    }
}

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures").join(name);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn rich(markdown: &str, opts: &ExportOptions, rich_opts: &RichOptions, assets: &dyn AssetResolver) -> String {
    let doc = ExportDoc::from_md(&lmdv_md::parse(markdown), opts, assets).unwrap();
    export_rich_html(&doc, rich_opts)
}

#[test]
fn the_fixture_is_one_inline_styled_fragment() {
    let source = fixture("test-features.md");
    let assets = TestAssets::with_images();
    let opts =
        ExportOptions { source_path: Some("/home/jane/docs/test-features.md".into()), ..ExportOptions::default() };
    let html = rich(&source, &opts, &RichOptions::default(), &assets);

    assert!(html.starts_with("<div role=\"document\""), "{html:.200}");
    assert!(html.ends_with("</div>"));
    assert!(html.contains("max-width:760px"));
    assert!(html.contains("font-size:16px"));
    assert!(html.contains("line-height:1.7"));
    for banned in [
        "<style",
        "<script",
        "class=",
        "<link",
        "@import",
        "@media",
        "copy-code",
        "data-source-line",
        "table-actions",
        "table-scroll",
        "markdown-alert",
        "onclick",
        "<button",
    ] {
        assert!(!html.to_ascii_lowercase().contains(banned), "the fragment still carries {banned}");
    }
    assert!(!html.contains("src=\"http"), "pictures never load from the network");
    assert!(!html.contains("<!doctype"), "a clipboard fragment, not a page");
    assert!(html.contains("style=\"font-family:"), "body text is inline styled");
}

#[test]
fn headings_paragraphs_and_inline_runs_are_semantic_and_styled() {
    let markdown = "# One\n\n## Two\n\n### Three\n\n#### Four\n\n##### Five\n\n###### Six\n\n\
                    **bold** *em* ~~gone~~ `code` [link](https://example.com) \
                    <u>under</u> <mark>marked</mark> <kbd>Ctrl</kbd> H<sub>2</sub>O x<sup>2</sup>\n";
    let html = rich(markdown, &ExportOptions::default(), &RichOptions::default(), &NoAssets);

    for level in 1..=6 {
        assert!(html.contains(&format!("<h{level} ")), "h{level} exists");
        assert!(html.contains("style=\"margin:"), "headings carry inline styles");
        assert!(html.contains(&format!("</h{level}>")));
    }
    assert!(html.contains("<strong style=\"font-weight:600\">bold</strong>"));
    assert!(html.contains("<em style=\"font-style:italic\">em</em>"));
    assert!(html.contains("<s style=\"text-decoration:line-through\">gone</s>"));
    assert!(html.contains("<code style=\"font-family:ui-monospace"));
    assert!(
        html.contains("<a href=\"https://example.com\" style=\"color:#0969da;text-decoration:underline\">link</a>")
    );
    assert!(html.contains("<u style=\"text-decoration:underline\">under</u>"));
    assert!(html.contains("<mark style=\"background-color:#fff8c5"));
    assert!(html.contains("<kbd style="));
    assert!(html.contains("<sub style="));
    assert!(html.contains("<sup style="));
    assert!(html.contains("padding-bottom:7px;border-bottom:1px solid #d1d9e0"), "h1/h2 underline");
}

#[test]
fn hard_breaks_and_thematic_rules_are_kept() {
    let html = rich("line one\\\nline two\n\n---\n", &ExportOptions::default(), &RichOptions::default(), &NoAssets);
    assert!(html.contains("line one<br>line two"), "{html}");
    assert!(html.contains("<hr style=\"height:0;margin:24px 0;border:0;border-top:1px solid #d1d9e0\">"));
}

#[test]
fn tables_are_bordered_with_a_tinted_header_and_alignment() {
    let html = rich(
        "| Left | Center | Right |\n| :-- | :-: | --: |\n| a | b | c |\n",
        &ExportOptions::default(),
        &RichOptions::default(),
        &NoAssets,
    );
    assert!(html.contains("<table cellspacing=\"0\""));
    assert!(html.contains("border:1px solid #d1d9e0"));
    assert!(
        html.contains("<th scope=\"col\" style=\"padding:6px 13px;border:1px solid #d1d9e0;background-color:#f6f8fa")
    );
    assert!(html.contains("text-align:left"));
    assert!(html.contains("text-align:center"));
    assert!(html.contains("text-align:right"));
    assert!(html.contains("<td style=\"padding:6px 13px;border:1px solid #d1d9e0"));
}

#[test]
fn code_blocks_are_shaded_single_cell_tables_with_a_language_label() {
    let html = rich(
        "```rust\nfn main() {\n\tprintln!(\"hi <&>\");\n}\n```\n",
        &ExportOptions::default(),
        &RichOptions::default(),
        &NoAssets,
    );
    assert!(html.contains("<table cellspacing=\"0\" cellpadding=\"0\""));
    assert!(html.contains(">rust</td></tr>"), "the language label row");
    assert!(html.contains("&nbsp;&nbsp;&nbsp;&nbsp;"), "the tab indented with NBSPs");
    assert!(html.contains("println"), "the code text is kept");
    assert!(html.contains("&quot;hi &lt;&amp;&gt;&quot;"), "code text escaped");
    assert!(html.contains("<br>"), "newlines are paste-safe breaks");
    assert!(!html.contains("<pre"), "the pre wrapper is gone");
    assert!(!html.contains("<code"), "the code wrapper is gone");
    // The Rust token colours are inline too.
    assert!(html.contains("color:#cf222e"), "keyword colour");
}

#[test]
fn task_items_use_visible_ballot_boxes() {
    let html = rich("- [x] done\n- [ ] todo\n", &ExportOptions::default(), &RichOptions::default(), &NoAssets);
    assert!(html.contains('\u{2611}'), "checked: {html}");
    assert!(html.contains('\u{2610}'), "unchecked: {html}");
    assert!(html.contains("list-style:none"));
    assert!(!html.contains("<input"), "Word drops disabled inputs; the copy carries symbols");
}

#[test]
fn callouts_have_their_colour_symbol_and_localized_title() {
    let html = rich(
        "> [!NOTE]\n> Body.\n\n> [!TIP] Heads up\n> More.\n",
        &ExportOptions::default(),
        &RichOptions::default(),
        &NoAssets,
    );
    assert!(html.contains("<aside role=\"note\""));
    assert!(html.contains("border-left:3px solid #0969da"));
    assert!(html.contains("background-color:#ddf4ff"));
    assert!(html.contains("\u{2139}\u{fe0f} Note"));
    assert!(html.contains("border-left:3px solid #1a7f37"));
    assert!(html.contains("\u{1f4a1} Heads up"));

    let italian = rich(
        "> [!WARNING]\n> Attento.\n",
        &ExportOptions::default(),
        &RichOptions { lang: "it".to_owned(), ..RichOptions::default() },
        &NoAssets,
    );
    assert!(italian.contains("Avviso"), "{italian}");
}

#[test]
fn math_is_the_png_data_uri_with_tex_in_the_alt() {
    let markdown = "Inline $x^2$ and display:\n\n$$\\int_0^1 x\\,dx$$\n";
    let html = rich(markdown, &ExportOptions::default(), &RichOptions::default(), &NoAssets);
    assert!(html.contains("src=\"data:image/png;base64,"));
    assert!(html.contains("alt=\"x^2\""));
    assert!(html.contains("alt=\"\\int_0^1 x\\,dx\""));
    assert!(html.contains("<div style=\"margin:16px 0;text-align:center\">"), "display math is centred");
    assert!(html.contains("vertical-align:"), "inline math carries its baseline alignment");
    assert!(html.contains("display:inline-block"));
    assert_eq!(html.matches("data:image/png;base64,").count(), 2);
}

#[test]
fn images_are_embedded_and_mermaid_falls_back_to_its_source() {
    let markdown = "![pixel](pic.png)\n\n```mermaid\ngraph TD\n  A --> B\n```\n";
    let html = rich(markdown, &ExportOptions::default(), &RichOptions::default(), &TestAssets::with_images());
    assert!(html.contains("<img src=\"data:image/png;base64,"));
    assert!(html.contains("alt=\"pixel\""));
    assert!(html.contains("max-width:100%;height:auto;border-radius:6px"));

    let source = rich(markdown, &ExportOptions::default(), &RichOptions::default(), &NoAssets);
    assert!(source.contains(">mermaid</td>"), "the diagram source keeps its language label");
    if !source.contains("graph TD") || !source.contains("A --&gt; B") {
        panic!("{source}");
    }
    assert!(!source.contains("<img"), "no picture and no SVG");
}

#[test]
fn links_are_absolutized_and_unsafe_destinations_are_dropped() {
    let markdown = "[other](notes/other.md) [web](https://example.com/a) [bad](javascript:alert(1))\n";

    let opts = ExportOptions { source_path: Some("/home/jane/docs/notes.md".into()), ..ExportOptions::default() };
    let html = rich(markdown, &opts, &RichOptions::default(), &NoAssets);
    assert!(html.contains("href=\"file:///home/jane/docs/notes/other.md\""), "{html}");
    assert!(html.contains("href=\"https://example.com/a\""));
    assert!(!html.contains("javascript:"));

    // The rich options can supply the folder when the model was built without it.
    let html = rich(
        markdown,
        &ExportOptions::default(),
        &RichOptions { source_path: Some("/home/jane/docs/notes.md".into()), ..RichOptions::default() },
        &NoAssets,
    );
    assert!(html.contains("href=\"file:///home/jane/docs/notes/other.md\""), "{html}");
}

#[test]
fn toc_and_footnotes_keep_their_bookmarks() {
    let html = rich(
        "[TOC]\n\n# One\n\n## Two\n\nText[^a] here.\n\n[^a]: The note.\n",
        &ExportOptions::default(),
        &RichOptions::default(),
        &NoAssets,
    );
    assert!(html.contains("<nav aria-label=\"Table of contents\""));
    assert!(html.contains("href=\"#one\""));
    assert!(html.contains("href=\"#two\""));
    assert!(html.contains("<h1 id=\"one\""));
    assert!(html.contains("href=\"#fn-1\""));
    assert!(html.contains("id=\"fn-1\""));
    assert!(html.contains("href=\"#fnref-1\""));
    assert!(html.contains("Table of contents"));
}

#[test]
fn front_matter_is_omitted_and_the_output_is_deterministic() {
    let opts = ExportOptions { front_matter: true, ..ExportOptions::default() };
    let markdown = "---\ntitle: Hidden\n---\n# Visible\n\nBody.\n";
    let first = rich(markdown, &opts, &RichOptions::default(), &NoAssets);
    let second = rich(markdown, &opts, &RichOptions::default(), &NoAssets);
    assert_eq!(first, second);
    assert!(first.contains("Visible"));
    assert!(!first.contains("Hidden"));
    assert!(!first.contains("front-matter"));
}

#[test]
fn the_title_becomes_the_fragments_accessible_name() {
    let html = rich(
        "# Notes\n",
        &ExportOptions::default(),
        &RichOptions { title: Some("Notes \"today\"".to_owned()), ..RichOptions::default() },
        &NoAssets,
    );
    assert!(html.starts_with("<div role=\"document\" aria-label=\"Notes &quot;today&quot;\""));
}

#[test]
fn an_empty_document_is_an_empty_wrapper() {
    let html = rich("", &ExportOptions::default(), &RichOptions::default(), &NoAssets);
    assert!(html.starts_with("<div role=\"document\""));
    assert!(html.ends_with("></div>"), "no body blocks: {html}");
    // A model with no title has no accessible name either.
    let bare = export_rich_html(&ExportDoc::default(), &RichOptions::default());
    assert!(bare.starts_with("<div role=\"document\" style="), "{bare}");
    assert!(!bare.contains("aria-label"));
    assert!(bare.ends_with("></div>"));
}

#[test]
fn network_pictures_never_reach_the_fragment() {
    let assets = TestAssets::with_images();
    let opts = ExportOptions { source_path: Some("/home/jane/docs/notes.md".into()), ..ExportOptions::default() };
    let html = rich("![remote](https://example.com/a.png)\n", &opts, &RichOptions::default(), &assets);
    assert!(!html.contains("src=\"https"));
    assert!(html.contains("remote"), "the alt text remains");
    assert_eq!(assets.image_calls.get(), 0, "a network source is never resolved");
}

#[test]
fn unresolved_pictures_and_unknown_html_degrade_to_text() {
    let assets = TestAssets::none();
    let doc = ExportDoc::from_md(
        &lmdv_md::parse("![missing](gone.png)\n\n<details><summary>More</summary>Body</details>\n"),
        &ExportOptions::default(),
        &assets,
    )
    .unwrap();
    assert!(doc.warnings.iter().any(|warning| warning.kind == ExportWarningKind::ImageMissing));
    let html = export_rich_html(&doc, &RichOptions::default());
    assert!(html.contains("missing"), "{html}");
    // Word understands neither details nor summary: both become visible blocks.
    assert!(!html.contains("<details"));
    assert!(!html.contains("<summary"));
    assert!(html.contains("More"));
    assert!(html.contains("Body"));
}

#[test]
fn encoded_png_bytes_are_the_original_asset() {
    let assets = TestAssets::with_images();
    let html = rich("![pixel](pic.png)\n", &ExportOptions::default(), &RichOptions::default(), &assets);
    let marker = "src=\"data:image/png;base64,";
    let start = html.find(marker).expect("a PNG data URI") + marker.len();
    let end = start + html[start..].find('"').expect("a closing quote");
    let bytes = base64::engine::general_purpose::STANDARD.decode(&html[start..end]).expect("valid base64");
    assert_eq!(bytes, fake_png().bytes);
}

/// Writes the clipboard fragments as inspectable pages when
/// `LMDV_RICH_EVIDENCE` is set; ordinary test runs stay free of side effects.
#[test]
fn evidence_fragments_are_written_on_request() {
    if std::env::var_os("LMDV_RICH_EVIDENCE").is_none() {
        return;
    }
    let dir = Path::new("/tmp/opencode/p5-04b");
    fs::create_dir_all(dir).expect("/tmp/opencode/p5-04b");
    let assets = TestAssets::with_images();
    for (name, fixture_name) in
        [("rich-test-features.html", "test-features.md"), ("rich-commonmark-math.html", "commonmark-math.md")]
    {
        let markdown = fixture(fixture_name);
        let opts = ExportOptions {
            source_path: Some(Path::new("/home/jane/docs").join(fixture_name)),
            include_toc: true,
            ..ExportOptions::default()
        };
        let fragment = rich(&markdown, &opts, &RichOptions::default(), &assets);
        let page = format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{fixture_name}</title></head>\
             <body style=\"margin:24px;background-color:#ffffff\">{fragment}</body></html>\n"
        );
        fs::write(dir.join(name), page).expect("the evidence page");
    }
}
