//! End-to-end tests for the PDF export.
//!
//! The assertions are about the file a reader will print: page count, A4 page
//! size, extracted text, repeated header/footer, image XObjects and the
//! presence of accessible formula descriptions. Poppler and qpdf are optional
//! test tools; a test that needs one skips when it is absent.

use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::process::Command;

use lmdv_export::{
    AssetResolver, ExportDoc, ExportError, ExportOptions, ExportedAsset, NoAssets, PdfFont, PdfOptions, PdfPage,
    export_pdf,
};

const A4_WIDTH_PT: f32 = 595.28;
const A4_HEIGHT_PT: f32 = 841.89;

/// A resolver serving the repository's real PNG for every picture and diagram.
struct TestAssets {
    image: Option<ExportedAsset>,
    diagram: Option<ExportedAsset>,
    calls: Cell<usize>,
}

impl TestAssets {
    fn with_png() -> Self {
        let bytes = fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/img/test.png"))
            .expect("the fixture PNG");
        let asset = ExportedAsset::new("image/png", bytes);
        Self { image: Some(asset.clone()), diagram: Some(asset), calls: Cell::new(0) }
    }
}

impl AssetResolver for TestAssets {
    fn image(&self, _src: &str, _alt: &str) -> Option<ExportedAsset> {
        self.calls.set(self.calls.get() + 1);
        self.image.clone()
    }

    fn diagram(&self, _source: &str) -> Option<ExportedAsset> {
        self.diagram.clone()
    }
}

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures").join(name);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn pdf_with(markdown: &str, export: &ExportOptions, print: &PdfOptions, assets: &dyn AssetResolver) -> Vec<u8> {
    let doc = lmdv_md::parse(markdown);
    let exported = ExportDoc::from_md(&doc, export, assets).expect("an export model");
    export_pdf(&exported, print).expect("a PDF")
}

fn pdf(markdown: &str, print: &PdfOptions) -> Vec<u8> {
    pdf_with(markdown, &ExportOptions::default(), print, &NoAssets)
}

fn print_options() -> PdfOptions {
    PdfOptions { date: Some("Nov 13, 2026".to_owned()), ..PdfOptions::default() }
}

fn write_temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = Path::new("/tmp/opencode");
    fs::create_dir_all(dir).expect("the evidence directory");
    let path = dir.join(name);
    fs::write(&path, bytes).expect("the PDF evidence file");
    path
}

fn have(binary: &str) -> bool {
    Command::new("which").arg(binary).output().map(|output| output.status.success()).unwrap_or(false)
}

fn pdfinfo(path: &Path) -> Option<String> {
    if !have("pdfinfo") {
        return None;
    }
    let output = Command::new("pdfinfo").arg(path).output().expect("pdfinfo runs");
    assert!(output.status.success(), "pdfinfo failed: {}", String::from_utf8_lossy(&output.stderr));
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The XMP metadata packet `pdfinfo -meta` prints (PDF/UA identification).
fn pdf_metadata(path: &Path) -> Option<String> {
    if !have("pdfinfo") {
        return None;
    }
    let output = Command::new("pdfinfo").arg("-meta").arg(path).output().expect("pdfinfo runs");
    assert!(output.status.success(), "pdfinfo -meta failed: {}", String::from_utf8_lossy(&output.stderr));
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn page_count(path: &Path) -> Option<usize> {
    let info = pdfinfo(path)?;
    info.lines().find_map(|line| line.strip_prefix("Pages:")).and_then(|value| value.trim().parse().ok())
}

fn page_size_is_a4(info: &str) -> bool {
    if info.contains("(A4)") {
        return true;
    }
    let Some(line) = info.lines().find(|line| line.starts_with("Page size:")) else { return false };
    let numbers: Vec<f32> = line.split_whitespace().filter_map(|token| token.parse::<f32>().ok()).collect();
    numbers.len() >= 2 && (numbers[0] - A4_WIDTH_PT).abs() < 1.5 && (numbers[1] - A4_HEIGHT_PT).abs() < 1.5
}

fn pdf_text(path: &Path) -> Option<String> {
    if !have("pdftotext") {
        return None;
    }
    let output = Command::new("pdftotext").arg("-layout").arg(path).arg("-").output().expect("pdftotext runs");
    assert!(output.status.success(), "pdftotext failed: {}", String::from_utf8_lossy(&output.stderr));
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn qdf(path: &Path) -> Option<String> {
    if !have("qpdf") {
        return None;
    }
    let output = Command::new("qpdf")
        .args(["--qdf", "--object-streams=disable"])
        .arg(path)
        .arg("-")
        .output()
        .expect("qpdf runs");
    assert!(output.status.success(), "qpdf failed: {}", String::from_utf8_lossy(&output.stderr));
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn assert_qpdf_check(path: &Path) {
    if !have("qpdf") {
        return;
    }
    let output = Command::new("qpdf").arg("--check").arg(path).output().expect("qpdf --check runs");
    assert!(
        output.status.success(),
        "qpdf --check failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_valid_pdf(bytes: &[u8], path: &Path) {
    assert!(bytes.starts_with(b"%PDF"), "missing the PDF signature");
    assert!(bytes.len() > 1024, "the PDF is only {} bytes", bytes.len());
    assert_qpdf_check(path);
}

#[test]
fn empty_document_is_a_valid_pdf() {
    let bytes = export_pdf(&ExportDoc::default(), &print_options()).expect("an empty PDF");
    let path = write_temp("lmdv-p5-03-empty.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);

    if let Some(info) = pdfinfo(&path) {
        assert!(page_size_is_a4(&info), "not A4:\n{info}");
        assert_eq!(page_count(&path), Some(1), "an empty document prints one page");
    }
}

#[test]
fn front_matter_only_document_is_a_valid_pdf() {
    let markdown = "---\ntitle: Front only\nauthor: Jane\n---\n";
    let exported = ExportDoc::from_md(
        &lmdv_md::parse(markdown),
        &ExportOptions { front_matter: true, ..ExportOptions::default() },
        &NoAssets,
    )
    .expect("an export model");
    let bytes = export_pdf(&exported, &print_options()).expect("a front-matter-only PDF");
    let path = write_temp("lmdv-p5-03-front-matter.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);
    if let Some(pages) = page_count(&path) {
        assert_eq!(pages, 1, "a front-matter-only document still prints one page");
    }

    if let Some(text) = pdf_text(&path) {
        assert!(!text.trim().is_empty(), "the page still has a header and footer");
        assert!(!text.contains("author: Jane"), "print never includes the front matter:\n{text}");
    }
}

#[test]
fn test_features_pdf_is_a4_with_header_footer_and_article_text() {
    let assets = TestAssets::with_png();
    let source = fixture("test-features.md");
    let bytes = pdf_with(&source, &ExportOptions::default(), &print_options(), &assets);
    let path = write_temp("lmdv-p5-03-test-features-check.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);
    if !have("pdfinfo") || !have("pdftotext") {
        return;
    }

    let info = pdfinfo(&path).expect("pdfinfo is required for this assertion");
    assert!(page_size_is_a4(&info), "not A4:\n{info}");
    let pages = page_count(&path).expect("a page count");
    assert!(pages >= 2, "the feature fixture should span several pages, got {pages}");

    let text = pdf_text(&path).expect("pdftotext is required for this assertion");
    for expected in
        ["Test document", "Basic formatting", "Nov 13, 2026", "Akeneo", "Valid Mermaid diagram", "Alert callouts"]
    {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
    assert!(
        text.contains(&format!("1 / {pages}")) || text.contains(&format!("1/{pages}")),
        "missing the page footer in:\n{text}"
    );
    if let Some(qdf) = qdf(&path) {
        assert!(qdf.contains("/S /URI"), "external links should be PDF link annotations");
        assert!(qdf.contains("/URI (https://www.anthropic.com)"), "the external link destination is missing");
    }
}

#[test]
fn the_pdf_declares_pdf_ua_conformance() {
    let bytes = pdf("# Accessible\n\nThe body text.\n", &PdfOptions::default());
    let path = write_temp("lmdv-oi088-pdf-ua.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);

    if let Some(info) = pdfinfo(&path) {
        let tagged =
            info.lines().find(|line| line.starts_with("Tagged:")).is_some_and(|line| line.trim_end().ends_with("yes"));
        assert!(tagged, "the PDF is not tagged:\n{info}");
        assert!(info.lines().any(|line| line.starts_with("Title:") && line["Title:".len()..].trim() != ""), "{info}");
    }
    if let Some(metadata) = pdf_metadata(&path) {
        assert!(metadata.contains("pdfuaid:part>1"), "missing PDF/UA identification:\n{metadata}");
        assert!(metadata.contains("<dc:language>"), "missing the document language:\n{metadata}");
    }
    if let Some(qdf) = qdf(&path) {
        assert!(
            qdf.contains("/Lang (en)") || qdf.contains("/Lang(en)"),
            "missing the document language in the PDF catalog"
        );
        for marker in ["/MarkInfo", "/StructTreeRoot", "/DisplayDocTitle"] {
            assert!(qdf.contains(marker), "missing {marker} in the PDF structure");
        }
    }
}

#[test]
fn an_untitled_document_still_has_a_metadata_title() {
    let bytes = export_pdf(&ExportDoc::default(), &PdfOptions::default()).expect("an empty document prints");
    let path = write_temp("lmdv-oi088-untitled.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);
    if let Some(info) = pdfinfo(&path) {
        // PDF/UA requires a title; the localized “Untitled” label stands in.
        assert!(
            info.lines().any(|line| line.starts_with("Title:") && line["Title:".len()..].trim() == "Untitled"),
            "{info}"
        );
    }
}

#[test]
fn a_picture_without_alt_text_still_exports_a_pdf_ua_document() {
    let markdown = "![](./no-alt.png)\n\nInline ![](./inline.png) too.\n";
    let bytes = pdf_with(markdown, &ExportOptions::default(), &PdfOptions::default(), &TestAssets::with_png());
    let path = write_temp("lmdv-oi088-empty-alt.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);
    if let Some(metadata) = pdf_metadata(&path) {
        assert!(metadata.contains("pdfuaid:part>1"), "the PDF is not PDF/UA:\n{metadata}");
    }
}

/// A resolver that serves one SVG picture, for the vector-picture path.
struct SvgAssets;

impl AssetResolver for SvgAssets {
    fn image(&self, _src: &str, _alt: &str) -> Option<ExportedAsset> {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="24">
            <rect width="48" height="24" fill="#0969da"/></svg>"##;
        Some(ExportedAsset::new("image/svg+xml", svg.as_bytes().to_vec()))
    }

    fn diagram(&self, _source: &str) -> Option<ExportedAsset> {
        self.image("", "")
    }
}

#[test]
fn an_svg_picture_is_an_image_in_a_pdf_ua_document() {
    let bytes = pdf_with("![A blue box](./art.svg)\n", &ExportOptions::default(), &PdfOptions::default(), &SvgAssets);
    let path = write_temp("lmdv-oi088-svg.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);
    if let Some(metadata) = pdf_metadata(&path) {
        assert!(metadata.contains("pdfuaid:part>1"), "the PDF is not PDF/UA:\n{metadata}");
    }
}

#[test]
fn short_code_blocks_and_tables_are_not_split_across_pages() {
    let code: String = (1..=8).map(|line| format!("code{line:02}\n")).collect();
    let table = "| HeadA | HeadB |\n|---|---|\n| RowA | RowB |\n| RowC | RowD |\n";
    for paragraphs in [28, 29, 30, 33, 34, 35, 36] {
        let filler: String = (1..=paragraphs)
            .map(|index| format!("Filler paragraph {index} has enough words to take up some room on the page."))
            .collect::<Vec<_>>()
            .join("\n\n");
        let markdown = format!("{filler}\n\n```text\n{code}```\n\n{table}");
        let bytes = pdf(&markdown, &PdfOptions::default());
        let path = write_temp(&format!("lmdv-oi088-keep-{paragraphs}.pdf"), &bytes);
        assert_valid_pdf(&bytes, &path);
        let Some(pages) = bbox_pages(&path) else { return };
        let page_of = |text: &str| {
            pages
                .iter()
                .position(|page| page.iter().any(|word| word.text == text))
                .unwrap_or_else(|| panic!("{text} is missing from the PDF"))
        };
        let code_page = page_of("code01");
        for line in 2..=8 {
            assert_eq!(
                page_of(&format!("code{line:02}")),
                code_page,
                "the code block split after {paragraphs} paragraphs"
            );
        }
        assert_eq!(page_of("HeadA"), page_of("RowD"), "the table split across pages after {paragraphs} paragraphs");
    }
}

#[test]
fn commonmark_math_spans_pages_and_keeps_tex_descriptions() {
    let source = fixture("commonmark-math.md");
    let bytes = pdf(&source, &print_options());
    let path = write_temp("lmdv-p5-03-commonmark-math-check.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);
    if !have("pdfinfo") || !have("pdftotext") {
        return;
    }

    let pages = page_count(&path).expect("pdfinfo is required for this assertion");
    assert!(pages >= 2, "the math fixture should span pages, got {pages}");

    let text = pdf_text(&path).expect("pdftotext is required for this assertion");
    for expected in ["Math and line breaks, as on GitHub", "Display math"] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }

    // A formula is a picture, not text: its TeX must live in the tagged PDF's
    // alternative description instead of the extractable text layer.
    if let Some(qdf) = qdf(&path) {
        assert!(qdf.contains("/Alt"), "the tagged PDF should carry the formula descriptions");
        assert!(qdf.contains("x^2 + y^2 = z^2"), "the display TeX should be an image description");
        assert!(qdf.contains("E = mc"), "the inline TeX should be an image description");
    }
}

#[test]
fn every_h1_after_the_first_starts_a_page() {
    let markdown = "# First\n\nSome text.\n\n# Second\n\nMore text.\n";
    let plain = pdf(markdown, &PdfOptions::default());
    let plain_path = write_temp("lmdv-p5-03-break-h1-off", &plain);

    let broken = pdf(markdown, &PdfOptions { break_on_h1: true, ..PdfOptions::default() });
    let broken_path = write_temp("lmdv-p5-03-break-h1-on", &broken);

    match (page_count(&plain_path), page_count(&broken_path)) {
        (Some(without), Some(with)) => {
            assert_eq!(without, 1, "without the option the document stays on one page");
            assert_eq!(with, 2, "the second h1 must start a new page");
        }
        _ => {
            // No pdfinfo: the option must still change the bytes.
            assert_ne!(plain, broken);
        }
    }
}

#[test]
fn pictures_become_image_xobjects() {
    let assets = TestAssets::with_png();
    let markdown = "![A test picture](./img/test.png)\n";
    let bytes = pdf_with(markdown, &ExportOptions::default(), &PdfOptions::default(), &assets);
    let path = write_temp("lmdv-p5-03-image.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);

    if let Some(qdf) = qdf(&path) {
        assert!(qdf.contains("/Subtype /Image"), "the picture is not an image XObject");
        assert!(qdf.contains("/Alt"), "the picture should keep its accessible description");
    }
}

#[test]
fn supplied_fonts_are_embedded_and_used() {
    let family = "Liberation Sans";
    let directory = Path::new("/usr/share/fonts/liberation");
    let regular = directory.join("LiberationSans-Regular.ttf");
    let bold = directory.join("LiberationSans-Bold.ttf");
    if !regular.exists() {
        return;
    }
    let mut fonts = vec![PdfFont::new(family, fs::read(&regular).expect("the regular face"))];
    if bold.exists() {
        fonts.push(PdfFont::new(family, fs::read(&bold).expect("the bold face")));
    }
    let options = PdfOptions { fonts, body_font: family.to_owned(), ..PdfOptions::default() };
    let source = "# Fonts\n\nText in a supplied family.\n";
    let with_fonts = pdf(source, &options);
    let default = pdf(source, &PdfOptions::default());
    assert_ne!(default, with_fonts, "the supplied family should change the layout");

    let path = write_temp("lmdv-p5-03-fonts.pdf", &with_fonts);
    if have("pdffonts") {
        let output = Command::new("pdffonts").arg(&path).output().expect("pdffonts runs");
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("LiberationSans"), "the subset font is missing:\n{text}");
    }
}

#[test]
fn output_is_deterministic_and_driven_only_by_print_options() {
    let source = "# One\n\nA paragraph with **strong** text.\n\n## Two\n\n- a\n- b\n";
    let first = pdf(source, &print_options());
    let second = pdf(source, &print_options());
    assert_eq!(first, second, "the same document and options must be byte-identical");

    let larger = pdf(source, &PdfOptions { body_size_pt: 18.0, ..print_options() });
    assert_ne!(first, larger, "print typography is a PDF option, not a reader setting");

    assert_eq!(PdfOptions::default().page, PdfPage::A4);
    assert_eq!(PdfOptions::default().page.top_mm, 20.0);
    assert_eq!(PdfOptions::default().page.bottom_mm, 18.0);
    assert_eq!(PdfOptions::default().page.left_mm, 16.0);
    assert_eq!(PdfOptions::default().page.right_mm, 16.0);
}

#[test]
fn toc_and_footnotes_render_as_text() {
    let markdown = "[TOC]\n\n# First\n\nA note.[^n]\n\n## Nested\n\n- one\n- [x] done\n\n[^n]: The footnote text.\n";
    let bytes = pdf(markdown, &PdfOptions::default());
    let path = write_temp("lmdv-p5-03-toc-footnotes.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);

    if let Some(text) = pdf_text(&path) {
        for expected in ["First", "Nested", "The footnote text."] {
            assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
        }
        assert!(text.contains('1'), "the footnote reference number is missing");
    }
    if let Some(qdf) = qdf(&path) {
        assert!(qdf.contains("/Dest (h-"), "TOC entries should link to their headings");
    }
}

#[test]
fn diagrams_and_missing_pictures_fall_back_to_text() {
    let markdown = "```mermaid\nflowchart LR\n    A --> B\n```\n\n![Missing picture](./nope.png)\n";
    let bytes = pdf(markdown, &PdfOptions::default());
    let path = write_temp("lmdv-p5-03-fallbacks.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);

    if let Some(text) = pdf_text(&path) {
        assert!(text.contains("flowchart LR"), "the diagram source should be kept:\n{text}");
        assert!(text.contains("Missing picture"), "the missing picture alt should be kept:\n{text}");
    }
}

#[test]
fn every_inline_style_and_block_kind_compiles() {
    let markdown = concat!(
        "# Styles\n\n",
        "Text **bold**, *emphasis*, ~~strike~~, `code`, <mark>mark</mark>, <kbd>Ctrl</kbd>, ",
        "<sup>sup</sup>, <sub>sub</sub>, <u>under</u> and [link](https://example.com).\n",
        "Hard break  \nnext line.\n\n",
        "---\n\n",
        "> A quote.\n\n",
        "> [!NOTE]\n> A callout.\n\n",
        "| Left | Center | Right |\n|:-----|:------:|------:|\n| a | b | c |\n\n",
        "- item\n  - nested\n- [ ] open task\n- [x] done task\n\n",
        "3. three\n4. four\n",
    );
    let bytes = pdf(markdown, &PdfOptions::default());
    let path = write_temp("lmdv-p5-03-styles.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);

    if let Some(text) = pdf_text(&path) {
        for expected in ["Styles", "Ctrl", "A callout.", "nested", "three", "next line."] {
            assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
        }
    }
}

#[test]
fn long_code_lines_wrap_inside_the_page() {
    let long = "x".repeat(220);
    let markdown = format!("```text\n{long}\n```\n");
    let bytes = pdf(&markdown, &PdfOptions::default());
    let path = write_temp("lmdv-p5-03-long-code.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);

    if let Some(text) = pdf_text(&path) {
        // The zero-width break opportunities are visible to pdftotext, but no
        // source character may be lost.
        assert!(text.matches('x').count() >= 220, "wrapping must not drop code characters:\n{text}");
    }
}

#[test]
fn invalid_print_options_are_rejected() {
    let error = export_pdf(&ExportDoc::default(), &PdfOptions { body_size_pt: 100.0, ..PdfOptions::default() })
        .expect_err("a 100 pt body is not printable");
    assert!(matches!(error, ExportError::Pdf(_)), "got {error:?}");

    let error = export_pdf(
        &ExportDoc::default(),
        &PdfOptions { page: PdfPage { left_mm: 120.0, right_mm: 120.0, ..PdfPage::A4 }, ..PdfOptions::default() },
    )
    .expect_err("margins wider than the page are not printable");
    assert!(matches!(error, ExportError::Pdf(_)), "got {error:?}");
}

#[test]
fn typst_markup_in_document_text_stays_text() {
    let markdown =
        "# Title with \\ backslash and #hash\n\nBody with #set page(width: 1mm), [brackets] and a \\ backslash.\n";
    let bytes = pdf(markdown, &PdfOptions::default());
    let path = write_temp("lmdv-p5-03-escaping.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);

    if let Some(text) = pdf_text(&path) {
        for expected in ["Title with \\ backslash and #hash", "#set page(width: 1mm)", "[brackets]"] {
            assert!(text.contains(expected), "missing literal {expected:?} in:\n{text}");
        }
    }
    if let Some(info) = pdfinfo(&path) {
        assert!(page_size_is_a4(&info), "document text must not be able to change the page:\n{info}");
        assert!(info.contains("Title with"), "the title metadata is present:\n{info}");
    }
}

#[test]
fn writes_the_two_evidence_pdfs_when_requested() {
    let Ok(directory) = std::env::var("LMDV_PDF_EVIDENCE") else { return };
    fs::create_dir_all(&directory).expect("the evidence directory");

    let assets = TestAssets::with_png();
    let features = pdf_with(&fixture("test-features.md"), &ExportOptions::default(), &print_options(), &assets);
    fs::write(Path::new(&directory).join("lmdv-p5-03-test-features.pdf"), features).expect("the features PDF");

    let math = pdf(&fixture("commonmark-math.md"), &print_options());
    fs::write(Path::new(&directory).join("lmdv-p5-03-commonmark-math.pdf"), math).expect("the math PDF");
}

#[test]
fn document_labels_follow_the_export_language() {
    let markdown = "> [!NOTE]\n> A callout.\n\n> [!WARNING]\n> Careful.\n";
    let opts = ExportOptions { lang: "it".into(), ..ExportOptions::default() };
    let bytes = pdf_with(markdown, &opts, &PdfOptions::default(), &NoAssets);
    let path = write_temp("lmdv-p6-01b-labels-it.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);
    if let Some(text) = pdf_text(&path) {
        assert!(text.contains("Nota"), "Italian callout title missing: {text}");
        assert!(text.contains("Avviso"), "Italian warning title missing: {text}");
        assert!(!text.contains("Note"), "English callout title still present: {text}");
    }
}

// ---- P6-PDF-T: X-12 wide-table fitting -------------------------------

/// A word from `pdftotext -bbox`, with the page it appeared on.
#[derive(Debug)]
struct BboxWord {
    x_min: f32,
    y_min: f32,
    x_max: f32,
    y_max: f32,
    text: String,
}

/// The printable left/right edges of the default A4 page, in points.
const PRINT_LEFT_PT: f32 = 16.0 * 72.0 / 25.4;
const PRINT_RIGHT_PT: f32 = A4_WIDTH_PT - PRINT_LEFT_PT;

fn bbox_pages(path: &Path) -> Option<Vec<Vec<BboxWord>>> {
    if !have("pdftotext") {
        return None;
    }
    let output = Command::new("pdftotext").arg("-bbox").arg(path).arg("-").output().expect("pdftotext runs");
    assert!(output.status.success(), "pdftotext -bbox failed: {}", String::from_utf8_lossy(&output.stderr));
    let text = String::from_utf8_lossy(&output.stdout);
    let mut pages = Vec::new();
    let mut page: Vec<BboxWord> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("<page ") {
            page = Vec::new();
        } else if line.starts_with("</page>") {
            pages.push(std::mem::take(&mut page));
        } else if let Some(rest) = line.strip_prefix("<word ")
            && let Some((attributes, tail)) = rest.split_once('>')
            && let Some((content, _)) = tail.rsplit_once("</word>")
        {
            let attribute = |name: &str| -> Option<f32> {
                let start = attributes.find(name)? + name.len() + 1;
                attributes[start..].split('"').next()?.parse().ok()
            };
            let (Some(x_min), Some(y_min), Some(x_max), Some(y_max)) =
                (attribute("xMin="), attribute("yMin="), attribute("xMax="), attribute("yMax="))
            else {
                continue;
            };
            page.push(BboxWord { x_min, y_min, x_max, y_max, text: content.to_owned() });
        }
    }
    Some(pages)
}

/// Removes the invisible break opportunities the fitting fallback inserts.
fn without_soft_breaks(text: &str) -> String {
    text.chars().filter(|character| *character != '\u{200b}').collect()
}

/// A document with every wide-table shape: prose columns, many short columns,
/// a long unbreakable token, a table below the scale floor, a tall table that
/// must repeat its header across pages, a table nested in a quote, and one
/// ordinary table.
fn wide_table_markdown() -> String {
    let prose_row = (1..=12)
        .map(|column| format!("column {column} has several words of body text"))
        .collect::<Vec<_>>()
        .join(" | ");
    let prose_head = (1..=12).map(|column| format!("Header {column}")).collect::<Vec<_>>().join(" | ");
    let prose_sep = ["---"; 12].join(" | ");
    let prose = format!("| {prose_head} |\n| {prose_sep} |\n| {prose_row} |\n| {prose_row} |\n");

    let many_head = (1..=20).map(|column| format!("H{column}")).collect::<Vec<_>>().join(" | ");
    let many_sep = ["---"; 20].join(" | ");
    let many_row = (1..=20).map(|column| format!("v{column}")).collect::<Vec<_>>().join(" | ");
    let many = format!("| {many_head} |\n| {many_sep} |\n| {many_row} |\n");

    let token = "x".repeat(180);
    let long = format!("| Kind | Value |\n| --- | --- |\n| key | {token} |\n");

    let daemon = "z".repeat(160);
    let daemon_head = (1..=8).map(|column| format!("D{column}")).collect::<Vec<_>>().join(" | ");
    let daemon_sep = ["---"; 8].join(" | ");
    let daemon_row = (1..=8)
        .map(|column| if column == 8 { format!("`{}`", "q".repeat(140)) } else { daemon.clone() })
        .collect::<Vec<_>>()
        .join(" | ");
    let fallback = format!("| {daemon_head} |\n| {daemon_sep} |\n| {daemon_row} |\n");

    let tall_head = (1..=12).map(|column| format!("T{column}")).collect::<Vec<_>>().join(" | ");
    let tall_sep = ["---"; 12].join(" | ");
    let mut tall = format!("| {tall_head} |\n| {tall_sep} |\n");
    for row in 0..40 {
        let cells = (1..=12).map(|column| format!("row {row} column {column} text")).collect::<Vec<_>>().join(" | ");
        tall.push_str(&format!("| {cells} |\n"));
    }

    let quoted = format!("> {prose}").replace('\n', "\n> ");

    format!(
        "# Wide tables\n\n{prose}\n\n{many}\n\n{long}\n\n{fallback}\n\n{tall}\n\n{quoted}\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n"
    )
}

#[test]
fn wide_tables_fit_the_printable_width() {
    let markdown = wide_table_markdown();
    let first = pdf(&markdown, &print_options());
    let second = pdf(&markdown, &print_options());
    assert_eq!(first, second, "wide-table fitting must stay deterministic");

    let path = write_temp("p6-pdf-wide.pdf", &first);
    assert_valid_pdf(&first, &path);

    if let Some(info) = pdfinfo(&path) {
        assert!(page_size_is_a4(&info), "not A4:\n{info}");
        let tagged = info.lines().find(|line| line.starts_with("Tagged:")).map(|line| line.ends_with("yes"));
        assert_eq!(tagged, Some(true), "tagged output was lost:\n{info}");
        assert!(page_count(&path).unwrap_or(0) >= 2, "the tall table should span pages");
    }

    let Some(pages) = bbox_pages(&path) else { return };
    assert!(!pages.is_empty(), "pdftotext reported no pages");
    for (index, page) in pages.iter().enumerate() {
        assert!(!page.is_empty(), "page {} has no words", index + 1);
        for word in page {
            assert!(
                word.x_min >= PRINT_LEFT_PT - 1.0,
                "page {}: {:?} starts at {:.2} pt, left of the {PRINT_LEFT_PT:.2} pt margin",
                index + 1,
                word.text,
                word.x_min
            );
            assert!(
                word.x_max <= PRINT_RIGHT_PT + 1.0,
                "page {}: {:?} ends at {:.2} pt, right of the {PRINT_RIGHT_PT:.2} pt margin",
                index + 1,
                word.text,
                word.x_max
            );
        }
    }

    let Some(text) = pdf_text(&path) else { return };
    let text = without_soft_breaks(&text);
    assert!(text.contains("Wide tables · Nov 13, 2026"), "the running header was lost:\n{text}");
    let total_pages = pages.len();
    assert!(
        text.contains(&format!("1 / {total_pages}")) || text.contains(&format!("1/{total_pages}")),
        "the page footer was lost:\n{text}"
    );
    for expected in ["Header", "Kind", "key", "D1", "T1", "T12"] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
    assert!(text.matches("column").count() >= 12, "the prose table text is incomplete:\n{text}");
    assert!(text.matches("row 0").count() >= 1, "the tall table text is incomplete:\n{text}");
    // No text is cropped: every daemon character is present, and the long code
    // token behind the fallback still has all of its characters.
    assert!(
        text.chars().filter(|character| *character == 'z').count() >= 7 * 160,
        "the unbreakable cells lost characters:\n{text}"
    );
    assert!(
        text.chars().filter(|character| *character == 'q').count() >= 140,
        "the long code token lost characters:\n{text}"
    );

    // The prose table is actually scaled, not merely squeezed by wrapping:
    // its header is visibly smaller than the 12 pt plain-table header.
    let scaled_header = pages[0].iter().find(|word| word.text == "Header").expect("the prose table header");
    let scaled_height = scaled_header.y_max - scaled_header.y_min;
    assert!((10.0..13.0).contains(&scaled_height), "header height {scaled_height:.2} pt is not a scaled 12 pt face");

    // The 40-row table repeats its header on every page it covers.
    let header_pages = pages.iter().filter(|page| page.iter().any(|word| word.text == "T1")).count();
    assert!(header_pages >= 2, "the wide table header did not repeat across pages");

    if let Some(qdf) = qdf(&path) {
        assert!(qdf.matches("/S /Table").count() >= 6, "the tables should stay tagged structures");
    }
}

#[test]
fn wide_tables_with_math_and_pictures_stay_inside_the_margins() {
    let head = (1..=12).map(|column| format!("Header {column} with words")).collect::<Vec<_>>().join(" | ");
    let separator = ["---"; 12].join(" | ");
    let body = (1..=12)
        .map(|column| match column {
            1 => "$x^2 + y^2 = z^2$".to_owned(),
            2 => "![A test picture](./img/test.png)".to_owned(),
            _ => format!("column {column} has several words of body text"),
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let markdown = format!("# Media table\n\n| {head} |\n| {separator} |\n| {body} |\n");
    let assets = TestAssets::with_png();
    let bytes = pdf_with(&markdown, &ExportOptions::default(), &print_options(), &assets);
    let path = write_temp("p6-pdf-wide-media.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);

    let Some(pages) = bbox_pages(&path) else { return };
    for (index, page) in pages.iter().enumerate() {
        for word in page {
            assert!(
                word.x_min >= PRINT_LEFT_PT - 1.0 && word.x_max <= PRINT_RIGHT_PT + 1.0,
                "page {}: {:?} spans {:.2}..{:.2} pt outside the print margins",
                index + 1,
                word.text,
                word.x_min,
                word.x_max
            );
        }
    }
    if let Some(qdf) = qdf(&path) {
        assert!(qdf.contains("/Subtype /Image"), "the inline picture should still be embedded");
        assert!(qdf.contains("x^2 + y^2 = z^2"), "the formula description should survive scaling");
    }
}

#[test]
fn narrow_tables_keep_the_plain_print_layout() {
    let markdown = "# Normal\n\n| Left | Center | Right |\n|:-----|:------:|------:|\n| a | b | c |\n";
    let bytes = pdf(markdown, &print_options());
    let path = write_temp("p6-pdf-normal-table.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);

    if let Some(text) = pdf_text(&path) {
        for expected in ["Left", "Center", "Right", "a", "b", "c"] {
            assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
        }
    }

    if let Some(pages) = bbox_pages(&path) {
        let left = pages.iter().flat_map(|page| page.iter()).find(|word| word.text == "Left").expect("the header cell");
        // The ordinary path keeps the 5 pt cell inset and the 12 pt body size;
        // a fitted table would move and shrink this word.
        assert!((left.x_min - (PRINT_LEFT_PT + 5.0)).abs() < 0.2, "Left starts at {:.2}", left.x_min);
        assert!(left.y_max - left.y_min > 13.0, "the ordinary table was scaled down");
    }
}

// ---- P6-PDF-H: structured raw HTML (OI-087) ---------------------------

fn export_fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Asserts that every expected string appears, in this order, in `text`.
fn assert_in_order(text: &str, expected: &[&str]) {
    let mut cursor = 0;
    for item in expected {
        let Some(index) = text[cursor..].find(item) else {
            panic!("missing {item:?} after byte {cursor} in:\n{text}");
        };
        cursor += index + item.len();
    }
}

#[test]
fn raw_html_blocks_keep_their_structure_in_the_pdf() {
    let markdown = export_fixture("raw-html.md");
    let opts = ExportOptions {
        title: Some("Raw HTML fixture".into()),
        source_path: Some("/home/jane/docs/notes.md".into()),
        ..ExportOptions::default()
    };
    let bytes = pdf_with(&markdown, &opts, &print_options(), &NoAssets);
    let again = pdf_with(&markdown, &opts, &print_options(), &NoAssets);
    assert_eq!(bytes, again, "raw HTML structure must stay deterministic");

    let path = write_temp("p6-pdf-html.pdf", &bytes);
    assert_valid_pdf(&bytes, &path);

    if let Some(info) = pdfinfo(&path) {
        assert!(page_size_is_a4(&info), "not A4:\n{info}");
        let tagged = info.lines().find(|line| line.starts_with("Tagged:")).map(|line| line.ends_with("yes"));
        assert_eq!(tagged, Some(true), "tagged output was lost:\n{info}");
        assert!(page_count(&path).unwrap_or(0) >= 1, "the fixture should print at least one page");
    }

    if let Some(pages) = bbox_pages(&path) {
        assert!(!pages.is_empty(), "pdftotext reported no pages");
        for (index, page) in pages.iter().enumerate() {
            assert!(!page.is_empty(), "page {} has no words", index + 1);
            for word in page {
                assert!(
                    word.x_min >= PRINT_LEFT_PT - 1.0 && word.x_max <= PRINT_RIGHT_PT + 1.0,
                    "page {}: {:?} spans {:.2}..{:.2} pt outside the print margins",
                    index + 1,
                    word.text,
                    word.x_min,
                    word.x_max
                );
            }
        }
    }

    let Some(text) = pdf_text(&path) else { return };
    let text = without_soft_breaks(&text);
    // The whole fixture is checked in reading order: table cells and header
    // rows, nested list items, details title/body, code, quote, media names
    // and finally an unsupported tag's surviving text.
    assert_in_order(
        &text,
        &[
            "Item name",
            "Unit count",
            "Total cost",
            "Alpha row",
            "Spanning value",
            "Beta row",
            "Twenty two",
            "Thirty three",
            "Plain column",
            "Second column",
            "Plain one",
            "Plain two",
            "Rowspan label",
            "First value",
            "Second value",
            "First bullet",
            "Second bullet",
            "Nested third",
            "Nested fourth",
            "More information",
            "Body text with inline code.",
            "Only a summary",
            "let answer = 42;",
            "Quoted paragraph",
            "Video",
            "clip.mp4",
            "audio",
            "song.mp3",
            "remote.mp3",
            "Fallback paragraph",
        ],
    );

    let nested = text.find("Nested third").expect("the nested ordered item");
    let number = text[..nested].split_whitespace().last().unwrap_or_default();
    assert_eq!(number, "3.", "the nested ordered list must start at 3:\n{text}");

    // A media file is a local file link; a remote source is only its name and
    // never a URL in the PDF.
    assert!(!text.contains("example.com"), "a remote media URL leaked into the text layer:\n{text}");
    assert!(!text.to_ascii_lowercase().contains("https://"), "no URL may reach the PDF text:\n{text}");

    if let Some(qdf) = qdf(&path) {
        assert!(
            qdf.matches("/S /Table").count() >= 3,
            "the three raw tables should stay tagged structures (found {})",
            qdf.matches("/S /Table").count()
        );
        assert!(qdf.contains("/URI (file:///home/jane/docs/media/clip.mp4)"), "the local video should be a file link");
        assert!(qdf.contains("/URI (file:///home/jane/docs/media/song.mp3)"), "the local audio should be a file link");
        assert!(!qdf.contains("example.com"), "a remote media URL must never reach the PDF");
    }
}
