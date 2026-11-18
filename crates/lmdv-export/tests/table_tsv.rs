//! End-to-end tests for table TSV copy (X-16).

use lmdv_export::{ExportBlock, ExportDoc, ExportOptions, ExportRun, ExportTable, NoAssets, RunStyle, table_tsv};

fn export_doc(markdown: &str) -> ExportDoc {
    ExportDoc::from_md(&lmdv_md::parse(markdown), &ExportOptions::default(), &NoAssets).unwrap()
}

fn run(text: &str) -> Vec<ExportRun> {
    vec![ExportRun::Text { text: text.to_owned(), style: RunStyle::default(), link: None }]
}

fn row(cells: &[&str]) -> Vec<Vec<ExportRun>> {
    cells.iter().map(|cell| run(cell)).collect()
}

fn table(head: &[&str], rows: &[&[&str]]) -> ExportTable {
    ExportTable {
        align: Vec::new(),
        head: head.iter().map(|cell| run(cell)).collect(),
        rows: rows.iter().map(|cells| row(cells)).collect(),
    }
}

#[test]
fn a_markdown_table_copies_as_header_and_body_rows() {
    let doc = export_doc("Intro.\n\n| Name | Age |\n| :--- | ---: |\n| Ada | 36 |\n| Bob | 41 |\n");
    let index = doc.blocks.iter().position(|block| matches!(block, ExportBlock::Table(_))).unwrap();
    assert_eq!(index, 1);
    assert_eq!(doc.table_tsv(index, None).as_deref(), Some("Name\tAge\nAda\t36\nBob\t41"));
    assert_eq!(doc.table_tsv(0, None), None, "a non-table block has no TSV");
    assert_eq!(doc.table_tsv(99, None), None, "an out-of-range index has no TSV");
}

#[test]
fn alignment_is_presentation_only() {
    let centered = export_doc("| A | B |\n| :-: | --: |\n| 1 | 2 |\n");
    let plain = export_doc("| A | B |\n| --- | --- |\n| 1 | 2 |\n");
    let tsv = |doc: &ExportDoc| {
        let index = doc.blocks.iter().position(|block| matches!(block, ExportBlock::Table(_))).unwrap();
        doc.table_tsv(index, None).unwrap()
    };
    assert_eq!(tsv(&centered), tsv(&plain));
}

#[test]
fn hidden_filter_rows_are_excluded_and_the_header_stays() {
    let table = table(&["A"], &[&["keep"], &["hide"], &["keep too"]]);
    assert_eq!(table_tsv(&table, Some(&[true, false, true])), "A\nkeep\nkeep too");
    assert_eq!(table_tsv(&table, Some(&[false, false, false])), "A", "the header is not filterable");
    assert_eq!(table_tsv(&table, None), "A\nkeep\nhide\nkeep too");
    assert_eq!(table_tsv(&table, Some(&[true])), "A\nkeep\nhide\nkeep too", "a short mask keeps the rest");
}

#[test]
fn tabs_newlines_and_trailing_empty_cells_are_handled() {
    let table = table(&["Head\tone", ""], &[&["x\ny", "", "last", ""]]);
    assert_eq!(table_tsv(&table, None), "Head one\nx y\t\tlast");
}

#[test]
fn empty_tables_and_header_only_tables() {
    assert_eq!(table_tsv(&ExportTable::default(), None), "");
    assert_eq!(table(&["Only"], &[]).to_tsv(), "Only");
    assert_eq!(table(&[], &[&["a", "b"]]).to_tsv(), "a\tb");
    assert_eq!(table(&[], &[]).to_tsv(), "");
}

#[test]
fn cell_markup_becomes_plain_text() {
    let doc = export_doc("| **Bold** | `code` | [link](https://example.com) |\n| --- | --- | --- |\n| a | b | c |\n");
    let index = doc.blocks.iter().position(|block| matches!(block, ExportBlock::Table(_))).unwrap();
    assert_eq!(doc.table_tsv(index, None).as_deref(), Some("Bold\tcode\tlink\na\tb\tc"));
}

#[test]
fn footnotes_images_and_math_keep_a_text_form() {
    let table = ExportTable {
        align: Vec::new(),
        head: vec![run("Head")],
        rows: vec![vec![
            vec![ExportRun::Math { tex: "x^2".to_owned(), display: false, asset: None, link: None }],
            vec![ExportRun::FootnoteRef { label: "a".to_owned(), number: 3, occurrence: 0 }],
            vec![ExportRun::Image {
                image: lmdv_export::ExportImage {
                    src: "pic.png".to_owned(),
                    alt: "pixel".to_owned(),
                    title: None,
                    asset: None,
                },
                link: None,
            }],
        ]],
    };
    assert_eq!(table_tsv(&table, None), "Head\nx^2\t3\tpixel");
}
