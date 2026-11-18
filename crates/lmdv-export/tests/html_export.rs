//! End-to-end tests for the self-contained HTML export.

use std::cell::Cell;
use std::fs;
use std::path::Path;

use base64::Engine as _;
use lmdv_export::{AssetResolver, ExportDoc, ExportOptions, ExportWarningKind, ExportedAsset, NoAssets, export_html};

/// The PNG signature plus a few bytes: enough to prove a data URI is embedded
/// and base64-encoded. The exporter does not decode resolver bytes.
fn fake_png() -> ExportedAsset {
    ExportedAsset::new("image/png", b"\x89PNG\r\n\x1a\nlmdv-test".to_vec())
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

fn export(markdown: &str, opts: &ExportOptions, assets: &dyn AssetResolver) -> String {
    let doc = lmdv_md::parse(markdown);
    export_html(&doc, opts, assets).unwrap()
}

fn first_png(html: &str) -> Vec<u8> {
    let marker = "src=\"data:image/png;base64,";
    let start = html.find(marker).expect("a PNG data URI") + marker.len();
    let end = start + html[start..].find('"').expect("a closing quote");
    base64::engine::general_purpose::STANDARD.decode(&html[start..end]).expect("valid base64")
}

#[test]
fn fixture_is_self_contained_semantic_and_deterministic() {
    let source = fixture("test-features.md");
    let doc = lmdv_md::parse(&source);
    let assets = TestAssets::with_images();
    let opts =
        ExportOptions { source_path: Some("/home/jane/docs/test-features.md".into()), ..ExportOptions::default() };
    let html = export_html(&doc, &opts, &assets).unwrap();

    assert!(html.starts_with("<!doctype html>\n<html lang=\"en\">"));
    assert!(html.contains("<article class=\"markdown-body\">"));
    assert!(html.ends_with("</html>\n"));
    assert!(!html.to_ascii_lowercase().contains("<script"));
    assert!(!html.contains("<link "));
    assert!(!html.contains("@import"));
    assert!(!html.contains("src=\"http"));
    assert!(!html.contains("srcset="));
    assert!(!html.contains("copy-code"));
    assert!(!html.contains("data-source-line"));

    // Every picture, formula and diagram is an embedded data URI.
    assert_eq!(html.matches("<img").count(), html.matches("src=\"data:image/").count());
    assert!(html.contains("class=\"math math-inline\""));
    assert!(html.contains("alt=\"E = mc^2\""));
    assert!(html.contains("<figure class=\"diagram\"><img src=\"data:image/png;base64,"));
    assert!(html.contains("<figure class=\"image\"><img src=\"data:image/png;base64,"));

    // Semantic structure.
    let heading_id = &doc.headings[0].id;
    assert!(html.contains(&format!("id=\"{heading_id}\"")));
    assert!(html.contains("<h1 id=\""));
    assert!(html.contains("<table>"));
    assert!(html.contains("<thead>"));
    assert!(html.contains("<tbody>"));
    assert!(html.contains("<th scope=\"col\">"));
    assert!(html.contains("<aside class=\"callout callout-note\" role=\"note\">"));
    assert!(html.contains("<p class=\"callout-title\">Note</p>"));
    assert!(html.contains("<blockquote>"));
    assert!(html.contains("<pre><code class=\"language-typescript\">"));
    assert!(html.contains("class=\"tok-"));
    assert!(html.contains("<hr>"));

    // Tasks, footnotes and their backlink.
    assert!(html.contains("<input type=\"checkbox\" class=\"task-checkbox\" disabled checked aria-label="));
    assert!(html.contains("<section class=\"footnotes\" aria-label=\"Footnotes\">"));
    assert!(html.contains("<sup class=\"footnote-ref\"><a href=\"#fn-1\" id=\"fnref-1\""));
    assert!(html.contains("<li id=\"fn-1\" class=\"footnote-item\">"));
    assert!(html.contains("<a href=\"#fnref-1\" class=\"footnote-backref\""));

    // The original omits the front matter, so the default export does too.
    assert!(!html.contains("class=\"front-matter\""));
    assert!(!html.contains("Jane Doe"));
    assert!(html.contains("<title>test-features</title>"));

    // Determinism: same input, same bytes (asset order is first-use order).
    let again = export_html(&doc, &opts, &assets).unwrap();
    assert_eq!(html, again);
}

#[test]
fn commonmark_math_exports_pngs_with_tex_in_alt() {
    let source = fixture("commonmark-math.md");
    let doc = lmdv_md::parse(&source);
    let html = export_html(&doc, &ExportOptions::default(), &NoAssets).unwrap();

    assert!(!html.to_ascii_lowercase().contains("<script"));
    assert!(!html.contains("src=\"http"));
    assert!(html.matches("class=\"math math-inline\"").count() >= 5);
    assert!(html.matches("class=\"math math-display-image\"").count() >= 5);
    assert!(html.contains("alt=\"E = mc^2\""));
    assert!(html.contains("alt=\"x^2 + y^2 = z^2\""));

    // The bytes after `base64,` really are a PNG.
    assert_eq!(&first_png(&html)[..8], b"\x89PNG\r\n\x1a\n");

    // With math off, the TeX stays readable and no formula raster is embedded.
    let text = export_html(&doc, &ExportOptions { math: false, ..ExportOptions::default() }, &NoAssets).unwrap();
    assert!(text.contains("<code class=\"math-tex\">"));
    assert!(!text.contains("data:image/png;base64,"));
    assert!(!text.contains("math math-inline"));
}

#[test]
fn math_accessible_name_follows_the_export_language() {
    let markdown = "Inline $x^2$ formula.\n";
    let italian = export(markdown, &ExportOptions { lang: "it".into(), ..ExportOptions::default() }, &NoAssets);
    assert!(italian.contains("alt=\"x^2\""), "the TeX stays the alt text");
    assert!(italian.contains("aria-label=\"Formula: x^2\""));

    let english = export(markdown, &ExportOptions::default(), &NoAssets);
    assert!(english.contains("aria-label=\"Math: x^2\""), "English is unchanged");
}

#[test]
fn front_matter_is_opt_in() {
    let source = fixture("test-features.md");
    let opts = ExportOptions { front_matter: true, ..ExportOptions::default() };
    let html = export(source.as_str(), &opts, &NoAssets);

    assert!(html.contains("<details class=\"front-matter\">"));
    assert!(html.contains("<summary>Front matter</summary>"));
    assert!(html.contains("<dt>title</dt><dd>Test document</dd>"));
    assert!(html.contains("<dt>tags</dt><dd>lmdv, markdown, test</dd>"));

    // The original omits the front matter from an export: not even its title
    // leaks into the document metadata unless the app passes it explicitly.
    let secret = "---\ntitle: Secret\n---\n# Body\n";
    let hidden = export(secret, &ExportOptions::default(), &NoAssets);
    assert!(!hidden.contains("Secret"));
    assert!(hidden.contains("<title>Body</title>"));
}

#[test]
fn toc_marker_and_generated_toc_nest() {
    let marker = "# Title\n\n[TOC]\n\n## One\n\n### Two\n\n#### Three\n";
    let html = export(marker, &ExportOptions::default(), &NoAssets);
    assert!(html.contains("<nav class=\"toc\" aria-label=\"Table of contents\">"));
    assert!(html.contains("<a href=\"#one\">One</a>"));
    assert!(html.contains("<a href=\"#two\">Two</a>"));
    assert!(html.contains("<a href=\"#three\">Three</a>"));
    assert!(html.matches("<ul>").count() >= 3); // the toc's nested lists
    // The h1 before the marker is not part of the marker's TOC.
    assert!(!html.contains("<a href=\"#title\">Title</a>"));

    let generated_doc = lmdv_md::parse("# Title\n\n## One\n");
    let generated =
        export_html(&generated_doc, &ExportOptions { include_toc: true, ..Default::default() }, &NoAssets).unwrap();
    assert!(generated.contains("<nav class=\"toc\""));
    assert!(generated.contains(&format!("href=\"#{}\"", generated_doc.headings[0].id)));
    assert!(generated.contains("<a href=\"#one\">One</a>"));

    let off = export("# Title\n\n## One\n", &ExportOptions::default(), &NoAssets);
    assert!(!off.contains("<nav class=\"toc\""));
}

#[test]
fn links_are_absolutized_and_unsafe_destinations_dropped() {
    let markdown = concat!(
        "[nested](./nested/deeper.md) ",
        "[top](#section) ",
        "[web](https://example.com) ",
        "[bad](javascript:alert(1))\n\n",
        "## Section\n",
    );
    let opts = ExportOptions { source_path: Some("/home/jane/docs/notes.md".into()), ..ExportOptions::default() };
    let doc = lmdv_md::parse(markdown);
    let exported = ExportDoc::from_md(&doc, &opts, &NoAssets).unwrap();
    let html = exported.to_html().unwrap();

    assert!(html.contains("href=\"file:///home/jane/docs/nested/deeper.md\""));
    assert!(html.contains("href=\"#section\""));
    assert!(html.contains("href=\"https://example.com\""));
    assert!(!html.contains("javascript:"));
    assert!(exported.warnings.iter().any(|warning| warning.kind == ExportWarningKind::LinkRefused));
}

#[test]
fn network_images_are_never_fetched_and_missing_ones_keep_alt_text() {
    let assets = TestAssets::none();
    let markdown =
        "![remote](https://example.com/a.png)\n\n![gone](./gone.png)\n\n![data](data:image/png;base64,iVBORw0KGgo=)\n";
    let doc = lmdv_md::parse(markdown);
    let exported = ExportDoc::from_md(&doc, &ExportOptions::default(), &assets).unwrap();
    let html = exported.to_html().unwrap();

    assert_eq!(assets.image_calls.get(), 2, "only the local and data images reach the resolver");
    assert!(!html.contains("example.com"));
    assert!(html.contains("<p class=\"missing-image\">remote</p>"));
    assert!(html.contains("<p class=\"missing-image\">gone</p>"));
    // A small data image survives even when the resolver has no bytes for it.
    assert!(html.contains(&format!("src=\"data:image/png;base64,{}\"", "iVBORw0KGgo=")));
    assert!(exported.warnings.iter().any(|warning| warning.kind == ExportWarningKind::ImageRefused));
    assert!(exported.warnings.iter().any(|warning| warning.kind == ExportWarningKind::ImageMissing));
}

#[test]
fn resolver_mime_types_are_restricted_to_images() {
    struct HtmlAssets;
    impl AssetResolver for HtmlAssets {
        fn image(&self, _src: &str, _alt: &str) -> Option<ExportedAsset> {
            Some(ExportedAsset::new("text/html", b"<b>x</b>".to_vec()))
        }
        fn diagram(&self, _source: &str) -> Option<ExportedAsset> {
            None
        }
    }
    let html = export("![x](./x.png)\n", &ExportOptions::default(), &HtmlAssets);
    assert!(!html.contains("text/html"));
    assert!(!html.contains("data:text"));
    assert!(html.contains("<p class=\"missing-image\">x</p>"));
}

#[test]
fn inline_styles_tasks_breaks_and_callouts() {
    let markdown = concat!(
        "**bold** *em* ~~strike~~ `code` <mark>m</mark> <kbd>K</kbd> ",
        "H<sub>2</sub>O x<sup>2</sup> <u>u</u>\n\n",
        "Line one\\\nLine two\n\n",
        "- [x] done\n- [ ] todo\n\n",
        "3. three\n4. four\n\n",
        "> [!WARNING]\n> Careful.\n",
    );
    let html = export(markdown, &ExportOptions::default(), &NoAssets);

    assert!(html.contains("<strong>bold</strong>"));
    assert!(html.contains("<em>em</em>"));
    assert!(html.contains("<s>strike</s>"));
    assert!(html.contains("<code>code</code>"));
    assert!(html.contains("<mark>m</mark>"));
    assert!(html.contains("<kbd>K</kbd>"));
    assert!(html.contains("<sub>2</sub>"));
    assert!(html.contains("<sup>2</sup>"));
    assert!(html.contains("<u>u</u>"));
    assert!(html.contains("Line one<br>\nLine two"));
    assert!(html.contains("<ul class=\"contains-task-list\">"));
    assert!(html.contains("<li class=\"task-list-item\"><input type=\"checkbox\" class=\"task-checkbox\" disabled checked aria-label=\"done\"> done</li>"));
    assert!(html.contains("<li class=\"task-list-item\"><input type=\"checkbox\" class=\"task-checkbox\" disabled aria-label=\"todo\"> todo</li>"));
    assert!(html.contains("<ol start=\"3\">"));
    assert!(html.contains("<aside class=\"callout callout-warning\" role=\"note\">"));
    assert!(html.contains("<p class=\"callout-title\">Warning</p>"));
}

#[test]
fn tables_carry_alignment_classes() {
    let markdown = "| L | C | R |\n|:--|:-:|--:|\n| a | b | c |\n";
    let html = export(markdown, &ExportOptions::default(), &NoAssets);
    assert!(html.contains("<th class=\"align-left\" scope=\"col\">L</th>"));
    assert!(html.contains("<th class=\"align-center\" scope=\"col\">C</th>"));
    assert!(html.contains("<th class=\"align-right\" scope=\"col\">R</th>"));
    assert!(html.contains("<td class=\"align-right\">c</td>"));
}

#[test]
fn footnotes_follow_references_and_backlink_to_the_first() {
    let markdown = "First[^x] and second[^x].\n\n[^x]: The note.\n";
    let html = export(markdown, &ExportOptions::default(), &NoAssets);
    assert!(html.contains("id=\"fnref-1\""));
    assert!(html.contains("id=\"fnref-1-1\""));
    assert!(html.contains("<a href=\"#fnref-1\" class=\"footnote-backref\""));
    assert!(html.contains("<li id=\"fn-1\" class=\"footnote-item\">"));
    assert!(html.contains("<p>The note.</p>"));
}

#[test]
fn html_subset_is_preserved_and_scripts_stay_text() {
    let markdown = concat!(
        "<details>\n<summary>More</summary>\n<p>Body <strong>raw</strong></p>\n</details>\n\n",
        "Inline <kbd>Ctrl</kbd> and <mark>mark</mark>.\n\n",
        "<script>alert(1)</script>\n",
    );
    let html = export(markdown, &ExportOptions::default(), &NoAssets);
    assert!(html.contains("<details>"));
    assert!(html.contains("<summary>More</summary>"));
    assert!(html.contains("<p>Body <strong>raw</strong></p>"));
    assert!(html.contains("<kbd>Ctrl</kbd>"));
    assert!(html.contains("<mark>mark</mark>"));
    assert!(!html.to_ascii_lowercase().contains("<script"));
    assert!(html.contains("&lt;script&gt;") || html.contains("&lt;/script&gt;"));
}

#[test]
fn raw_pictures_collapse_to_one_light_source_and_empty_links_go() {
    let markdown = concat!(
        "<div><picture>",
        "<source media=\"(prefers-color-scheme: dark)\" srcset=\"./dark.png 2x\">",
        "<source media=\"(prefers-color-scheme: light)\" srcset=\"./light.png 2x\">",
        "<img src=\"./fallback.png\" alt=\"Pic\">",
        "</picture></div>\n\n",
        "<div><a href=\"https://example.com\"><img src=\"./dark.png#gh-dark-mode-only\" alt=\"\"></a></div>\n",
    );
    let assets = TestAssets::with_images();
    let html = export(markdown, &ExportOptions::default(), &assets);
    assert_eq!(html.matches("<img").count(), 1);
    assert!(html.contains("<img src=\"data:image/png;base64,"));
    assert!(!html.contains("dark.png"));
    assert!(!html.contains("light.png"));
    assert!(!html.contains("fallback.png"));
    assert!(!html.contains("<picture>"));
    assert!(!html.contains("example.com"), "an empty link around a dark-only picture is removed");
}

#[test]
fn raw_tables_keep_their_cells_and_alignment() {
    let markdown = concat!(
        "<table>\n<thead><tr><th align=\"left\">A</th></tr></thead>\n",
        "<tbody><tr><td align=\"right\" colspan=\"2\">1</td></tr></tbody>\n</table>\n",
    );
    let html = export(markdown, &ExportOptions::default(), &NoAssets);
    assert!(html.contains("<table>"));
    assert!(html.contains("<th class=\"align-left\">A</th>"));
    assert!(html.contains("<td class=\"align-right\" colspan=\"2\">1</td>"));
}

#[test]
fn raw_html_links_follow_the_same_policy() {
    let markdown = "<div><a href=\"./other.md\">Other</a> <a href=\"javascript:alert(1)\">Bad</a></div>\n";
    let opts = ExportOptions { source_path: Some("/home/jane/docs/notes.md".into()), ..ExportOptions::default() };
    let html = export(markdown, &opts, &NoAssets);
    assert!(html.contains("<a href=\"file:///home/jane/docs/other.md\">Other</a>"));
    assert!(!html.contains("javascript:"));
    assert!(html.contains("Bad"));
}

#[test]
fn media_is_a_file_url_or_a_plain_link() {
    let markdown = concat!(
        "<div><video src=\"./clip.mp4\"></video></div>\n\n",
        "<div><audio src=\"https://example.com/a.mp3\"></audio></div>\n",
    );
    let opts = ExportOptions { source_path: Some("/home/jane/docs/notes.md".into()), ..ExportOptions::default() };
    let html = export(markdown, &opts, &NoAssets);
    assert!(html.contains("<video controls preload=\"none\" src=\"file:///home/jane/docs/clip.mp4\">"));
    assert!(html.contains("<span class=\"media\"><video"));
    assert!(html.contains("<a href=\"file:///home/jane/docs/clip.mp4\">video</a>"));
    // A remote track is only a link, never a media element that would fetch it.
    assert!(!html.contains("<audio controls"));
    assert!(html.contains("<span class=\"media\"><a href=\"https://example.com/a.mp3\">audio</a>"));
}

#[test]
fn heading_ids_escaped_text_and_document_metadata() {
    let doc = lmdv_md::parse("# A & B < C\n");
    let opts = ExportOptions { title: Some("Custom <title>".into()), lang: "it".into(), ..ExportOptions::default() };
    let html = export_html(&doc, &opts, &NoAssets).unwrap();
    let id = &doc.headings[0].id;
    assert!(html.contains("<html lang=\"it\">"));
    assert!(html.contains("<title>Custom &lt;title&gt;</title>"));
    assert!(html.contains(&format!("<h1 id=\"{id}\">A &amp; B &lt; C</h1>")));

    let empty = export("", &ExportOptions::default(), &NoAssets);
    assert!(empty.contains("<article class=\"markdown-body\">"));
    assert!(empty.contains("<title>Pasted text</title>"));
}

#[test]
fn math_failures_fall_back_to_tex_and_are_reported() {
    let doc = lmdv_md::parse("Bad formula $\\frac{1}$ here.\n");
    let exported = ExportDoc::from_md(&doc, &ExportOptions::default(), &NoAssets).unwrap();
    let html = exported.to_html().unwrap();
    assert!(html.contains("<code class=\"math-tex\">"));
    assert!(exported.warnings.iter().any(|warning| warning.kind == ExportWarningKind::MathFallback));
}

#[test]
fn repeated_formulas_share_one_asset() {
    let doc = lmdv_md::parse("$x$ and $x$\n");
    let exported = ExportDoc::from_md(&doc, &ExportOptions::default(), &NoAssets).unwrap();
    let math_assets = exported.assets.iter().filter(|asset| asset.mime == "image/png").count();
    assert_eq!(math_assets, 1);
    let html = exported.to_html().unwrap();
    assert_eq!(html.matches("src=\"data:image/png;base64,").count(), 2);
}

#[test]
fn diagrams_fall_back_to_their_source() {
    let source = fixture("test-features.md");
    let doc = lmdv_md::parse(&source);
    let exported = ExportDoc::from_md(&doc, &ExportOptions::default(), &NoAssets).unwrap();
    let html = exported.to_html().unwrap();
    assert!(html.contains("<figure class=\"diagram diagram-source\">"));
    assert!(html.contains("<pre><code class=\"language-mermaid\">"));
    assert!(html.contains("flowchart LR"));
    assert!(exported.warnings.iter().any(|warning| warning.kind == ExportWarningKind::DiagramFallback));
}

#[test]
fn dark_only_pictures_are_removed() {
    let assets = TestAssets::with_images();
    let markdown = "![dark](./dark.png#gh-dark-mode-only)\n\n![light](./light.png#gh-light-mode-only)\n";
    let html = export(markdown, &ExportOptions::default(), &assets);
    assert!(!html.contains("dark.png"));
    assert!(!html.contains("light.png")); // the source is not written, only its bytes
    assert_eq!(html.matches("<img").count(), 1);
}

#[test]
fn document_labels_follow_the_export_language() {
    let markdown = concat!(
        "> [!NOTE]\n> Body.\n\n",
        "> [!WARNING]\n> Careful.\n\n",
        "Text[^1].\n\n",
        "[^1]: The note.\n\n",
        "```mermaid\ngraph TD; A-->B;\n```\n",
    );
    let opts = ExportOptions { lang: "it".into(), ..ExportOptions::default() };
    let html = export(markdown, &opts, &TestAssets::with_images());
    assert!(html.contains("<html lang=\"it\">"));
    assert!(html.contains("<p class=\"callout-title\">Nota</p>"));
    assert!(html.contains("<p class=\"callout-title\">Avviso</p>"));
    assert!(html.contains("<section class=\"footnotes\" aria-label=\"Note a piè di pagina\">"));
    assert!(html.contains("alt=\"Diagramma Mermaid\""));
    assert!(html.contains("aria-label=\"Torna al riferimento 1\">↩</a>"));
    // English is unchanged when the export language is English.
    let english = export(markdown, &ExportOptions::default(), &TestAssets::with_images());
    assert!(english.contains("<p class=\"callout-title\">Note</p>"));
    assert!(english.contains("aria-label=\"Footnotes\""));
    assert!(english.contains("alt=\"Mermaid diagram\""));
    assert!(english.contains("aria-label=\"Back to reference 1\">↩</a>"));
}
