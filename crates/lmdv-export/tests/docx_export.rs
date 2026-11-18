//! End-to-end tests for the OOXML (`.docx`) export.
//!
//! The package is unzipped in memory and every interesting part is parsed as
//! XML, so the assertions are about the Word document a reader will open, not
//! about string fragments alone.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;
use std::process::Command;

use lmdv_export::{AssetResolver, DocxOptions, ExportDoc, ExportOptions, ExportedAsset, NoAssets, export_docx};
use zip::ZipArchive;

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

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

fn docx(markdown: &str, opts: &ExportOptions, assets: &dyn AssetResolver) -> Vec<u8> {
    let doc = lmdv_md::parse(markdown);
    let exported = ExportDoc::from_md(&doc, opts, assets).expect("an export model");
    export_docx(&exported, &DocxOptions::default()).expect("a DOCX package")
}

/// Every part of the package, by name.
fn unzip(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut archive = ZipArchive::new(Cursor::new(bytes.to_vec())).expect("a readable OOXML package");
    let mut parts = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("an entry");
        let name = entry.name().to_owned();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).expect("entry bytes");
        assert!(parts.insert(name.clone(), data).is_none(), "duplicate part {name}");
    }
    parts
}

fn part(parts: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
    String::from_utf8(parts.get(name).unwrap_or_else(|| panic!("missing part {name}")).clone()).expect("UTF-8 XML")
}

fn document(parts: &BTreeMap<String, Vec<u8>>) -> String {
    part(parts, "word/document.xml")
}

/// Concatenated `w:t` content of a subtree (the visible text a reader sees).
fn note_text<'a, 'input>(node: roxmltree::Node<'a, 'input>) -> String {
    node.descendants().filter(|node| node.has_tag_name((W, "t"))).filter_map(|node| node.text()).collect()
}

fn xml<'a>(text: &'a str) -> roxmltree::Document<'a> {
    roxmltree::Document::parse(text).expect("well-formed XML")
}

/// The required package parts are present and the bytes are deterministic.
#[test]
fn package_parts_are_complete_and_deterministic() {
    let source = fixture("test-features.md");
    let assets = TestAssets::with_png();
    let opts =
        ExportOptions { source_path: Some("/home/jane/docs/test-features.md".into()), ..ExportOptions::default() };
    let first = docx(&source, &opts, &assets);
    let second = docx(&source, &opts, &assets);
    assert_eq!(first, second, "the same document must produce identical bytes");

    let parts = unzip(&first);
    for name in [
        "[Content_Types].xml",
        "_rels/.rels",
        "word/document.xml",
        "word/styles.xml",
        "word/numbering.xml",
        "word/settings.xml",
        "word/footnotes.xml",
        "word/_rels/document.xml.rels",
        "word/_rels/footnotes.xml.rels",
        "docProps/core.xml",
        "docProps/app.xml",
    ] {
        assert!(parts.contains_key(name), "missing {name}");
    }
    for (name, bytes) in &parts {
        if name.ends_with(".xml") || name.ends_with(".rels") {
            let text = String::from_utf8_lossy(bytes);
            assert!(!text.to_ascii_lowercase().contains("<script"), "{name}");
            assert!(!text.to_ascii_lowercase().contains("javascript:"), "{name}");
        }
    }

    let core = part(&parts, "docProps/core.xml");
    assert!(core.contains("<dc:title>test-features</dc:title>"));
    // The front matter is not part of any export.
    let body = document(&parts);
    let doc = xml(&body);
    assert!(doc.descendants().filter(|node| node.has_tag_name((W, "p"))).count() >= 30, "paragraphs");
    assert_eq!(body.matches("<w:tbl>").count(), 8, "one GFM table, two code cells and five callouts");
    assert!(!body.contains("Jane Doe"));
    assert!(!body.contains("status: draft"));

    // All parts parse as XML.
    for (name, bytes) in &parts {
        if name.ends_with(".xml") || name.ends_with(".rels") {
            let _ = xml(&String::from_utf8_lossy(bytes));
        }
    }
}

#[test]
fn headings_are_real_word_heading_styles() {
    let markdown = "# One\n\n## Two\n\n### Three\n\n#### Four\n\n##### Five\n\n###### Six\n";
    let parts = unzip(&docx(markdown, &ExportOptions::default(), &NoAssets));
    let body = document(&parts);
    let doc = xml(&body);
    let styles: Vec<String> = doc
        .descendants()
        .filter(|node| node.has_tag_name((W, "pStyle")))
        .filter_map(|node| node.attribute((W, "val")).map(str::to_owned))
        .collect();
    for level in 1..=6 {
        assert!(styles.contains(&format!("Heading{level}")), "Heading{level}");
    }

    let styles_xml = part(&parts, "word/styles.xml");
    for level in 1..=6 {
        assert!(styles_xml.contains(&format!("w:styleId=\"Heading{level}\"")));
        assert!(styles_xml.contains(&format!("w:name w:val=\"heading {level}\"")));
    }
    assert!(styles_xml.contains("<w:outlineLvl w:val=\"0\"/>"));
    assert!(styles_xml.contains("<w:outlineLvl w:val=\"5\"/>"));
}

#[test]
fn gfm_tables_have_a_header_row_shading_and_look() {
    let markdown = "| L | C | R |\n|:--|:-:|--:|\n| a | b | c |\n| d | e | f |\n";
    let parts = unzip(&docx(markdown, &ExportOptions::default(), &NoAssets));
    let body = document(&parts);
    let doc = xml(&body);
    let table = doc.descendants().find(|node| node.has_tag_name((W, "tbl"))).expect("a real w:tbl");
    let look = table.descendants().find(|node| node.has_tag_name((W, "tblLook"))).expect("w:tblLook");
    assert_eq!(look.attribute((W, "val")), Some("04A0"));
    assert_eq!(look.attribute((W, "firstRow")), Some("1"));

    let rows: Vec<_> = table.descendants().filter(|node| node.has_tag_name((W, "tr"))).collect();
    assert_eq!(rows.len(), 3, "one header row plus two body rows");
    assert!(rows[0].descendants().any(|node| node.has_tag_name((W, "tblHeader"))));
    let header_fill: Vec<&str> = rows[0]
        .descendants()
        .filter(|node| node.has_tag_name((W, "shd")))
        .filter_map(|node| node.attribute((W, "fill")))
        .collect();
    assert_eq!(header_fill, vec!["F6F8FA"; 3]);
    // The body rows alternate tone, like the HTML writer.
    assert!(
        rows[2]
            .descendants()
            .any(|node| { node.has_tag_name((W, "shd")) && node.attribute((W, "fill")) == Some("FBFCFD") })
    );

    let alignments: Vec<Option<&str>> = rows[1]
        .descendants()
        .filter(|node| node.has_tag_name((W, "jc")))
        .map(|node| node.attribute((W, "val")))
        .collect();
    // Left and unset need no `w:jc`; only center/right emit one.
    assert_eq!(alignments, vec![Some("center"), Some("right")]);

    let columns = table.descendants().filter(|node| node.has_tag_name((W, "gridCol"))).count();
    assert_eq!(columns, 3);
}

#[test]
fn lists_have_numbering_and_tasks_use_ballot_boxes() {
    let markdown = concat!("- one\n  - nested\n", "- [x] done\n- [ ] todo\n\n", "3. three\n4. four\n",);
    let parts = unzip(&docx(markdown, &ExportOptions::default(), &NoAssets));
    let body = document(&parts);
    assert!(body.contains("\u{2611}"), "checked task glyph");
    assert!(body.contains("\u{2610}"), "unchecked task glyph");

    let numbering = part(&parts, "word/numbering.xml");
    assert!(numbering.contains("w:numFmt w:val=\"bullet\""));
    assert!(numbering.contains("w:numFmt w:val=\"decimal\""));
    assert!(numbering.contains("<w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"3\"/>"));
    assert!(numbering.contains("<w:lvl w:ilvl=\"8\">"));
    // Nested lists use the next level and the next indent stop.
    assert!(body.contains("<w:ilvl w:val=\"1\"/>"));
    assert!(body.contains("<w:ind w:left=\"1440\" w:hanging=\"360\"/>"));

    // Every numId used in the document is defined.
    let doc = xml(&body);
    let mut used: Vec<&str> = doc
        .descendants()
        .filter(|node| node.has_tag_name((W, "numId")))
        .filter_map(|node| node.attribute((W, "val")))
        .collect();
    used.sort_unstable();
    used.dedup();
    assert!(!used.is_empty());
    assert_eq!(body.matches("<w:numPr>").count(), 4, "two bullet items (one nested) and two ordered");
    let numbers = xml(&numbering);
    for id in used {
        assert!(
            numbers.descendants().any(|node| node.has_tag_name((W, "num")) && node.attribute((W, "numId")) == Some(id)),
            "numId {id} is defined"
        );
    }
}

#[test]
fn hyperlinks_use_relationships_and_internal_anchors() {
    let markdown = concat!(
        "[nested](./nested/deeper.md) ",
        "[web](https://example.com) ",
        "[back](#section) ",
        "[bad](javascript:alert(1))\n\n",
        "## Section\n",
    );
    let opts = ExportOptions { source_path: Some("/home/jane/docs/notes.md".into()), ..ExportOptions::default() };
    let parts = unzip(&docx(markdown, &opts, &NoAssets));
    let body = document(&parts);
    assert!(!body.to_ascii_lowercase().contains("javascript:"));
    assert!(body.contains("<w:hyperlink r:id=\""));
    assert!(body.contains("<w:hyperlink w:anchor=\""));

    let rels = part(&parts, "word/_rels/document.xml.rels");
    assert!(rels.contains("Target=\"file:///home/jane/docs/nested/deeper.md\" TargetMode=\"External\""));
    assert!(rels.contains("Target=\"https://example.com\" TargetMode=\"External\""));

    // Relationship ids are unique.
    let rels_doc = xml(&rels);
    let mut ids: Vec<&str> = rels_doc
        .descendants()
        .filter(|node| node.has_tag_name((RELS, "Relationship")))
        .filter_map(|node| node.attribute("Id"))
        .collect();
    let total = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), total);

    // The internal link points at a bookmark on the heading.
    let doc = xml(&body);
    let bookmark = doc
        .descendants()
        .find(|node| node.has_tag_name((W, "bookmarkStart")))
        .and_then(|node| node.attribute((W, "name")))
        .expect("a heading bookmark");
    assert!(body.contains(&format!("w:anchor=\"{bookmark}\"")));
}

#[test]
fn images_are_embedded_media_with_relationships() {
    let markdown = "![Test image](./img/test.png)\n";
    let parts = unzip(&docx(markdown, &ExportOptions::default(), &TestAssets::with_png()));
    let body = document(&parts);
    let doc = xml(&body);
    assert!(doc.descendants().any(|node| node.has_tag_name((W, "drawing"))));
    assert!(doc.descendants().any(|node| node.has_tag_name((WP, "inline"))));
    let blip = doc.descendants().find(|node| node.has_tag_name((A, "blip"))).expect("an a:blip");
    let embed = blip.attribute((R, "embed")).expect("r:embed");
    let doc_pr = doc.descendants().find(|node| node.has_tag_name((WP, "docPr"))).expect("wp:docPr");
    assert_eq!(doc_pr.attribute("descr"), Some("Test image"));

    let rels = part(&parts, "word/_rels/document.xml.rels");
    let rels_doc = xml(&rels);
    let image = rels_doc
        .descendants()
        .find(|node| {
            node.has_tag_name((RELS, "Relationship"))
                && node.attribute("Id") == Some(embed)
                && node.attribute("Type").is_some_and(|kind| kind.ends_with("/image"))
        })
        .expect("the image relationship");
    let target = image.attribute("Target").expect("a target");
    assert!(target.starts_with("media/image"));
    let media = parts.get(&format!("word/{target}")).expect("the media part");
    let expected = fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/img/test.png"))
        .expect("the fixture PNG");
    assert_eq!(media, &expected);

    let content_types = part(&parts, "[Content_Types].xml");
    assert!(content_types.contains("Extension=\"png\" ContentType=\"image/png\""));
    assert!(content_types.contains("Extension=\"jpeg\" ContentType=\"image/jpeg\""));
}

#[test]
fn math_is_the_embedded_png_with_tex_in_the_alt_text() {
    let source = fixture("commonmark-math.md");
    let parts = unzip(&docx(&source, &ExportOptions::default(), &NoAssets));
    let body = document(&parts);
    let doc = xml(&body);
    let descr: Vec<&str> = doc
        .descendants()
        .filter(|node| node.has_tag_name((WP, "docPr")))
        .filter_map(|node| node.attribute("descr"))
        .collect();
    assert!(descr.contains(&"E = mc^2"), "{descr:?}");
    assert!(descr.contains(&"x^2 + y^2 = z^2"), "{descr:?}");

    let media: Vec<&Vec<u8>> =
        parts.iter().filter(|(name, _)| name.starts_with("word/media/")).map(|(_, bytes)| bytes).collect();
    assert!(!media.is_empty());
    assert!(media.iter().all(|bytes| bytes.starts_with(b"\x89PNG\r\n\x1a\n")));

    // With math disabled the TeX stays readable and no formula is embedded.
    let text_only = unzip(&docx(&source, &ExportOptions { math: false, ..Default::default() }, &NoAssets));
    assert!(!text_only.keys().any(|name| name.starts_with("word/media/")));
    assert!(document(&text_only).contains("E = mc^2"));
}

#[test]
fn callouts_code_and_rules_carry_their_visuals() {
    let markdown = concat!(
        "> [!WARNING]\n> Careful.\n\n",
        "> [!NOTE]\n> Custom title\n",
        "> [!NOTE] My title\n> Body.\n\n",
        "```rust\nfn main() {\n\tlet x =  1;\n}\n```\n\n",
        "---\n",
    );
    let parts = unzip(&docx(markdown, &ExportOptions::default(), &NoAssets));
    let body = document(&parts);
    assert!(body.contains("w:fill=\"FAF7F2\""), "warning tint");
    assert!(body.contains("w:color=\"9A6700\""), "warning border/title color");
    assert!(body.contains("w:fill=\"F3F8FD\""), "note tint");
    assert!(body.contains("My title"));
    assert!(body.contains("\u{26A0}"), "warning symbol");
    assert!(body.contains("w:fill=\"F6F8FA\""), "code shading");
    assert!(body.contains("<w:tab/>"), "a preserved tab");
    assert!(body.contains("let x =  1;"), "two spaces survive");
    assert!(body.contains("<w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"D1D9E0\"/>"));
    assert!(body.contains("fn main() {"));
}

#[test]
fn adjacent_tables_are_separated_so_word_does_not_merge_them() {
    let markdown =
        concat!("```a\n1\n```\n\n```b\n2\n```\n\n", "| A |\n|---|\n| 1 |\n\n", "> [!NOTE]\n> A callout table.\n",);
    let parts = unzip(&docx(markdown, &ExportOptions::default(), &NoAssets));
    let body = document(&parts);
    assert!(!body.contains("</w:tbl><w:tbl>"), "Word would merge adjacent tables");
    assert!(body.contains("</w:tbl><w:p>"), "a separator paragraph follows a table");
}

#[test]
fn text_and_attributes_are_escaped_and_control_characters_dropped() {
    // A code span keeps the raw punctuation, so the writer itself is tested
    // (prose goes through the parser's smart punctuation and tag filter).
    let markdown = "# A & B < C\n\nRaw `a & b < c \"d\"` and \u{1}after.\n";
    let parts = unzip(&docx(markdown, &ExportOptions::default(), &NoAssets));
    let body = document(&parts);
    assert!(body.contains("A &amp; B &lt; C"));
    assert!(body.contains("a &amp; b &lt; c &quot;d&quot;"));
    assert!(!body.contains('\u{1}'), "XML 1.0 control characters are removed");
    assert!(body.contains("after."));
    let _ = xml(&body);
}

#[test]
fn empty_and_front_matter_only_documents_are_valid() {
    for markdown in ["", "---\ntitle: Only metadata\n---\n"] {
        let parts = unzip(&docx(markdown, &ExportOptions::default(), &NoAssets));
        let body = document(&parts);
        let doc = xml(&body);
        assert!(doc.descendants().any(|node| node.has_tag_name((W, "sectPr"))));
        let _ = xml(&part(&parts, "word/styles.xml"));
        let _ = xml(&part(&parts, "word/numbering.xml"));
        assert!(!body.contains("Only metadata"));
    }
}

#[test]
fn html_details_and_media_degrade_to_text_and_links() {
    let markdown = concat!(
        "<details>\n<summary>More</summary>\n<p>Body <strong>raw</strong></p>\n</details>\n\n",
        "<div><video src=\"./clip.mp4\"></video></div>\n",
    );
    let opts = ExportOptions { source_path: Some("/home/jane/docs/notes.md".into()), ..ExportOptions::default() };
    let parts = unzip(&docx(markdown, &opts, &NoAssets));
    let body = document(&parts);
    assert!(body.contains("More"));
    assert!(body.contains("Body "));
    assert!(body.contains("raw"));
    assert!(body.contains("clip.mp4"));
    let rels = part(&parts, "word/_rels/document.xml.rels");
    assert!(rels.contains("Target=\"file:///home/jane/docs/clip.mp4\""));
}

#[test]
fn the_toc_is_a_linked_heading_list() {
    let markdown = "# Title\n\n[TOC]\n\n## One\n\n### Two\n";
    let parts = unzip(&docx(markdown, &ExportOptions::default(), &NoAssets));
    let body = document(&parts);
    assert!(body.contains("Table of contents"));
    assert!(body.contains(">One</w:t>"));
    assert!(body.contains(">Two</w:t>"));
    assert!(body.contains("<w:hyperlink w:anchor=\"_lmdv2\" w:history=\"1\">"));
    // The h1 before the marker is not in the TOC.
    assert!(!body.contains(">Title</w:t></w:r></w:hyperlink>"));
}

#[test]
fn footnotes_use_a_real_part_with_references_and_backlinks() {
    let markdown = concat!(
        "First[^x] and again[^x]. Another[^y].\n\n",
        "[^x]: The *note* with [a link](https://example.com).\n",
        "[^y]: Second note.\n",
    );
    let opts = ExportOptions::default();
    let first = docx(markdown, &opts, &NoAssets);
    let second = docx(markdown, &opts, &NoAssets);
    assert_eq!(first, second, "a document with footnotes must be deterministic");

    let parts = unzip(&first);
    let body = document(&parts);
    let notes = part(&parts, "word/footnotes.xml");

    // The old plain end list is gone.
    assert!(!body.contains(">Footnotes</w:t>"), "no end-of-document label");
    assert!(!body.contains('\u{21A9}'), "no plain backlink in the body");
    assert!(!notes.contains(">Footnotes</w:t>"), "no heading in the notes part");

    // The part is declared and related to the main document.
    let content_types = part(&parts, "[Content_Types].xml");
    assert!(content_types.contains("PartName=\"/word/footnotes.xml\""));
    assert!(content_types.contains("wordprocessingml.footnotes+xml"));
    let rels = part(&parts, "word/_rels/document.xml.rels");
    assert!(
        rels.contains(
            "Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes\" \
             Target=\"footnotes.xml\""
        ),
        "{rels}"
    );

    // Three references: the two `[^x]` share id 1, `[^y]` is 2.
    let doc = xml(&body);
    let references: Vec<(String, Option<String>)> = doc
        .descendants()
        .filter(|node| node.has_tag_name((W, "r")))
        .filter_map(|run| {
            let reference = run.descendants().find(|node| node.has_tag_name((W, "footnoteReference")))?;
            let id = reference.attribute((W, "id")).map(str::to_owned);
            let style = run
                .descendants()
                .find(|node| node.has_tag_name((W, "rStyle")))
                .and_then(|node| node.attribute((W, "val")))
                .map(str::to_owned);
            Some((id?, style))
        })
        .collect();
    assert_eq!(
        references,
        vec![
            ("1".to_owned(), Some("FootnoteReference".to_owned())),
            ("1".to_owned(), Some("FootnoteReference".to_owned())),
            ("2".to_owned(), Some("FootnoteReference".to_owned())),
        ]
    );
    // The visible superscript survives as direct formatting too.
    assert!(body.contains("<w:rStyle w:val=\"FootnoteReference\"/><w:vertAlign w:val=\"superscript\"/>"));

    // Only the first reference is bookmarked, and only once per note.
    let starts: Vec<&str> = doc
        .descendants()
        .filter(|node| node.has_tag_name((W, "bookmarkStart")))
        .filter_map(|node| node.attribute((W, "name")))
        .collect();
    assert_eq!(starts, vec!["_lmdvfn1", "_lmdvfn2"]);

    // The notes part: separator, continuation separator, then ids 1 and 2.
    let notes_doc = xml(&notes);
    let entries: Vec<_> = notes_doc.descendants().filter(|node| node.has_tag_name((W, "footnote"))).collect();
    let ids: Vec<&str> = entries.iter().map(|entry| entry.attribute((W, "id")).expect("an id")).collect();
    assert_eq!(ids, vec!["-1", "0", "1", "2"]);
    assert_eq!(entries[0].attribute((W, "type")), Some("separator"));
    assert!(entries[0].descendants().any(|node| node.has_tag_name((W, "separator"))));
    assert_eq!(entries[1].attribute((W, "type")), Some("continuationSeparator"));
    assert!(entries[1].descendants().any(|node| node.has_tag_name((W, "continuationSeparator"))));

    // The first note carries its automatic number, its formatted text and an
    // external link relationship of its own.
    let note = entries[2];
    assert!(note.descendants().any(|node| node.has_tag_name((W, "footnoteRef"))));
    assert!(
        note.descendants()
            .any(|node| { node.has_tag_name((W, "pStyle")) && node.attribute((W, "val")) == Some("FootnoteText") })
    );
    assert_eq!(note_text(note).trim(), "The note with a link. \u{21A9}");
    assert!(note.descendants().any(|node| node.has_tag_name((W, "i"))), "the emphasis survives");
    let note_link = note
        .descendants()
        .find(|node| node.has_tag_name((W, "hyperlink")) && node.attribute((R, "id")).is_some())
        .expect("the note's link");
    let link_id = note_link.attribute((R, "id")).expect("a relationship id");
    // The note is its own OPC part, so its external link is declared on the
    // footnotes relationship file, never on the document's.
    let note_rels = part(&parts, "word/_rels/footnotes.xml.rels");
    let rels_doc = xml(&note_rels);
    assert!(
        rels_doc.descendants().any(|node| node.has_tag_name((RELS, "Relationship"))
            && node.attribute("Id") == Some(link_id)
            && node.attribute("Target") == Some("https://example.com")),
        "the note link is a footnotes relationship"
    );
    assert!(!rels.contains("https://example.com"), "not on document.xml.rels");

    // Each note backlinks to the bookmark around its first reference.
    for (entry, anchor, number) in [(entries[2], "_lmdvfn1", 1), (entries[3], "_lmdvfn2", 2)] {
        let back = entry
            .descendants()
            .find(|node| node.has_tag_name((W, "hyperlink")) && node.attribute((W, "anchor")) == Some(anchor))
            .unwrap_or_else(|| panic!("the backlink of note {number}"));
        let tooltip = back.attribute((W, "tooltip")).expect("the localized tooltip");
        assert_eq!(tooltip, format!("Back to reference {number}"));
        assert!(
            back.descendants()
                .any(|node| node.has_tag_name((W, "t")) && node.text().is_some_and(|text| text.contains('\u{21A9}'))),
            "the backlink arrow of note {number}"
        );
    }
    // The backlink targets exist in the body.
    for anchor in ["_lmdvfn1", "_lmdvfn2"] {
        assert!(
            doc.descendants()
                .any(|node| node.has_tag_name((W, "bookmarkStart")) && node.attribute((W, "name")) == Some(anchor)),
            "the bookmark {anchor} exists"
        );
    }

    // The separator/continuation ids and the note ids do not collide with the
    // section's decimal numbering, and the styles make references superscript.
    assert!(body.contains("<w:footnotePr><w:numFmt w:val=\"decimal\"/><w:numStart w:val=\"1\"/>"));
    let styles = part(&parts, "word/styles.xml");
    assert!(styles.contains("w:styleId=\"FootnoteText\""));
    assert!(styles.contains("w:styleId=\"FootnoteReference\""));
    assert!(styles.contains("<w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr></w:style>"));
    assert!(styles.contains("<w:sz w:val=\"18\"/>"));
}

#[test]
fn footnote_text_is_escaped_like_the_body() {
    let markdown = "Text[^x].\n\n[^x]: Code `a & b < c \"d\"` stays escaped.\n";
    let parts = unzip(&docx(markdown, &ExportOptions::default(), &NoAssets));
    let notes = part(&parts, "word/footnotes.xml");
    assert!(notes.contains("a &amp; b &lt; c &quot;d&quot;"));
    assert!(!notes.contains("a & b < c"));
    let _ = xml(&notes);
}

#[test]
fn a_document_without_notes_has_no_footnotes_part() {
    for markdown in ["", "# Title\n\nBody without notes.\n"] {
        let parts = unzip(&docx(markdown, &ExportOptions::default(), &NoAssets));
        assert!(!parts.contains_key("word/footnotes.xml"), "no part for {markdown:?}");
        let content_types = part(&parts, "[Content_Types].xml");
        assert!(!content_types.contains("/word/footnotes.xml"));
        let rels = part(&parts, "word/_rels/document.xml.rels");
        assert!(!rels.contains("relationships/footnotes"));
        let body = document(&parts);
        assert!(!body.contains("w:footnoteReference"));
        assert!(!body.contains("w:footnotePr"));
        for (name, bytes) in &parts {
            if name.ends_with(".xml") || name.ends_with(".rels") {
                let _ = xml(&String::from_utf8_lossy(bytes));
            }
        }
    }
}

#[test]
fn footnote_text_reuses_the_body_picture_and_math_strategy() {
    // The definition body is indented onto its own line: a same-line definition
    // is consumed as a link reference by the math pre-pass, so `$…$` stays
    // literal there. The writer is what this test covers.
    let markdown = "Text[^m].\n\n[^m]:\n    Inline $x^2$ and ![pic](./img/test.png).\n";

    // With assets, the note embeds the same media as the body would.
    let parts = unzip(&docx(markdown, &ExportOptions::default(), &TestAssets::with_png()));
    let notes = part(&parts, "word/footnotes.xml");
    let notes_doc = xml(&notes);
    assert_eq!(notes_doc.descendants().filter(|node| node.has_tag_name((W, "drawing"))).count(), 2);
    let descr: Vec<&str> = notes_doc
        .descendants()
        .filter(|node| node.has_tag_name((WP, "docPr")))
        .filter_map(|node| node.attribute("descr"))
        .collect();
    assert_eq!(descr, vec!["x^2", "pic"]);
    let media: Vec<&String> = parts.keys().filter(|name| name.starts_with("word/media/")).collect();
    assert_eq!(media.len(), 2, "one formula and one picture: {media:?}\n{notes}");
    // The note's two drawings are declared on the footnotes relationship file;
    // the body has none, so the document relationship file carries no image.
    let note_rels = part(&parts, "word/_rels/footnotes.xml.rels");
    assert_eq!(note_rels.matches("relationships/image").count(), 2);
    let document_rels = part(&parts, "word/_rels/document.xml.rels");
    assert_eq!(document_rels.matches("relationships/image").count(), 0);

    // Without math or pictures the note keeps the TeX and alt text.
    let text_only = unzip(&docx(markdown, &ExportOptions { math: false, ..ExportOptions::default() }, &NoAssets));
    let notes = part(&text_only, "word/footnotes.xml");
    assert!(!notes.contains("<w:drawing>"));
    assert!(notes.contains("x^2"));
    assert!(notes.contains("pic"));
}

#[test]
fn a_note_referencing_another_note_keeps_the_inner_number_as_text() {
    let markdown = "Outer[^a].\n\n[^a]: See also[^b].\n[^b]: Inner note.\n";
    let parts = unzip(&docx(markdown, &ExportOptions::default(), &NoAssets));
    let body = document(&parts);
    let notes = part(&parts, "word/footnotes.xml");
    let notes_doc = xml(&notes);
    // Both notes are defined; the inner reference is superscript text, not a
    // nested `w:footnoteReference` (Word has no nested notes).
    assert_eq!(notes_doc.descendants().filter(|node| node.has_tag_name((W, "footnote"))).count(), 4);
    assert_eq!(body.matches("<w:footnoteReference").count(), 1);
    assert_eq!(notes.matches("<w:footnoteReference").count(), 0);
    let outer = notes_doc
        .descendants()
        .find(|node| node.has_tag_name((W, "footnote")) && node.attribute((W, "id")) == Some("1"))
        .expect("the outer note");
    assert!(note_text(outer).contains("See also2."));
}

#[test]
fn source_path_controls_the_title_and_a_heading_title_is_not_repeated() {
    let doc = lmdv_md::parse("# Same title\n\nBody.\n");
    let exported = ExportDoc::from_md(&doc, &ExportOptions::default(), &NoAssets).unwrap();
    let parts = unzip(&export_docx(&exported, &DocxOptions::default()).unwrap());
    let body = document(&parts);
    assert_eq!(body.matches("Same title").count(), 1, "the title is not duplicated");
    let core = part(&parts, "docProps/core.xml");
    assert!(core.contains("<dc:title>Same title</dc:title>"));
}

#[test]
fn document_settings_are_print_safe_and_a4() {
    let parts = unzip(&docx("# Title\n", &ExportOptions::default(), &NoAssets));
    let body = document(&parts);
    assert!(body.contains("<w:pgSz w:w=\"11906\" w:h=\"16838\"/>"));
    assert!(body.contains("w:top=\"1440\""));
    let styles = part(&parts, "word/styles.xml");
    assert!(styles.contains("<w:rFonts w:ascii=\"Calibri\""));
    assert!(styles.contains("<w:sz w:val=\"22\"/>"));
    // Reader preferences never reach the export: a custom option is the only
    // way to change it.
    let doc = lmdv_md::parse("# Title\n");
    let exported = ExportDoc::from_md(&doc, &ExportOptions::default(), &NoAssets).unwrap();
    let big = DocxOptions { body_size_half_points: 28, ..DocxOptions::default() };
    let custom = unzip(&export_docx(&exported, &big).unwrap());
    assert!(part(&custom, "word/styles.xml").contains("<w:sz w:val=\"28\"/>"));
}

/// If LibreOffice is installed, the package must convert to PDF; a missing
/// binary skips the check instead of failing the suite.
#[test]
fn libreoffice_opens_the_package_when_installed() {
    let available = Command::new("soffice").arg("--version").output().is_ok();
    if !available {
        return;
    }
    let dir = std::env::temp_dir().join(format!("lmdv-docx-smoke-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("a temp folder");
    let source = fixture("test-features.md");
    let path = dir.join("smoke.docx");
    fs::write(&path, docx(&source, &ExportOptions::default(), &TestAssets::with_png())).expect("the DOCX");
    let output =
        Command::new("soffice").args(["--headless", "--convert-to", "pdf", "--outdir"]).arg(&dir).arg(&path).output();
    if let Ok(output) = output {
        assert!(output.status.success(), "soffice failed: {}", String::from_utf8_lossy(&output.stderr));
        assert!(dir.join("smoke.pdf").exists(), "soffice produced a PDF");
    }
    let _ = fs::remove_dir_all(&dir);
}

/// Writes the two evidence files when `LMDV_DOCX_EVIDENCE` is set; ordinary
/// test runs stay free of side effects.
#[test]
fn evidence_documents_are_written_on_request() {
    if std::env::var_os("LMDV_DOCX_EVIDENCE").is_none() {
        return;
    }
    let dir = Path::new("/tmp/opencode");
    fs::create_dir_all(dir).expect("/tmp/opencode");
    let cases = [
        ("lmdv-p5-02-test-features.docx", "test-features.md", TestAssets::with_png()),
        ("lmdv-p5-02-commonmark-math.docx", "commonmark-math.md", TestAssets::with_png()),
        ("p6x-test-features.docx", "test-features.md", TestAssets::with_png()),
    ];
    for (name, fixture_name, assets) in cases {
        let markdown = fixture(fixture_name);
        let opts = ExportOptions {
            source_path: Some(Path::new("/home/jane/docs").join(fixture_name)),
            ..ExportOptions::default()
        };
        let bytes = docx(&markdown, &opts, &assets);
        fs::write(dir.join(name), bytes).expect("the evidence file");
    }

    // P6-X1: a purpose-built notes document — a repeated reference, a
    // formatted definition with code/link, and inline math in a note.
    let notes = concat!(
        "# Footnote evidence\n\n",
        "A repeated reference[^a] and its second occurrence[^a]. A second note[^b].\n\n",
        "[^a]:\n    A *formatted* note with `inline code` and a [link](https://commonmark.org).\n",
        "[^b]:\n    Inline math $E = mc^2$ stays a PNG with its TeX as alt text.\n",
    );
    fs::write(dir.join("p6-x1-footnotes.docx"), docx(notes, &ExportOptions::default(), &NoAssets))
        .expect("the footnotes evidence file");
}

#[test]
fn document_labels_follow_the_export_language() {
    let markdown = concat!(
        "> [!CAUTION]\n> Careful.\n\n",
        "Text[^1].\n\n",
        "[^1]: The note.\n\n",
        "```mermaid\ngraph TD; A-->B;\n```\n",
    );
    let opts = ExportOptions { lang: "it".into(), ..ExportOptions::default() };
    let parts = unzip(&docx(markdown, &opts, &TestAssets::with_png()));
    let body = document(&parts);
    assert!(body.contains("Attenzione"), "Italian caution title");
    let notes = part(&parts, "word/footnotes.xml");
    assert!(notes.contains("w:tooltip=\"Torna al riferimento 1\""), "Italian backlink tooltip");
    assert!(notes.contains("The note."));
    assert!(!body.contains("Note a piè di pagina"), "no end-of-document list");
    assert!(body.contains("descr=\"Diagramma Mermaid\""), "Italian diagram alt");
    assert!(!body.contains("Mermaid diagram"));
}
