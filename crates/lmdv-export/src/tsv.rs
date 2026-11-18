//! Tab-separated table copy (X-16).
//!
//! The reader's table action bar copies the visible table into a spreadsheet.
//! The export model already holds the semantic cells, so the writer emits one
//! tab-separated line per row: the header first when the table has one, then
//! the body rows, with an optional visibility mask for rows the reader's
//! filter hid.
//!
//! Spreadsheet paste has no escape syntax for a tab or a newline inside a
//! cell, so both become one space. Trailing empty cells are dropped (pasting
//! `a<TAB><TAB>` would otherwise create phantom spreadsheet columns); interior
//! empty cells are kept so columns stay aligned. The output has no trailing
//! newline, which would paste as an extra empty row.
//!
//! Alignment is presentation only and never reaches the TSV.

use lmdv_md::{HtmlChild, HtmlNode, HtmlTag};

use crate::model::{ExportRun, ExportTable};

/// Serializes a table as tab-separated values for spreadsheet paste.
///
/// `visible` optionally filters the body rows, one flag per entry in
/// [`ExportTable::rows`] (X-16's hidden filter rows). A missing trailing flag
/// means visible, so a mask shorter than the table never drops rows silently.
/// The header row is always kept.
///
/// An empty table yields an empty string; a table without body rows yields just
/// its header. The result never ends with a newline.
pub fn table_tsv(table: &ExportTable, visible: Option<&[bool]>) -> String {
    let mut lines = Vec::with_capacity(table.rows.len() + 1);
    if !table.head.is_empty() {
        lines.push(row_tsv(&table.head));
    }
    for (index, row) in table.rows.iter().enumerate() {
        if visible.is_some_and(|mask| !mask.get(index).copied().unwrap_or(true)) {
            continue;
        }
        lines.push(row_tsv(row));
    }
    // A wholly empty last row would otherwise leave a trailing newline, which
    // pastes as an extra empty spreadsheet row.
    let end = lines.iter().rposition(|line| !line.is_empty()).map_or(0, |last| last + 1);
    lines.truncate(end);
    lines.join("\n")
}

impl ExportTable {
    /// This table as tab-separated values, with every body row.
    pub fn to_tsv(&self) -> String {
        table_tsv(self, None)
    }
}

/// One row: its cells separated by tabs, trailing empty cells dropped.
fn row_tsv(cells: &[Vec<ExportRun>]) -> String {
    let mut fields: Vec<String> = cells.iter().map(|runs| cell_tsv(runs)).collect();
    while fields.last().is_some_and(|field| field.trim().is_empty()) {
        fields.pop();
    }
    fields.join("\t")
}

/// The plain text of one cell, with paste-hostile whitespace flattened.
///
/// Markup disappears: a style only changes how a run would be drawn, a link
/// keeps its text, a formula keeps its TeX (the copy target cannot display the
/// raster) and a picture keeps its alt text.
fn cell_tsv(runs: &[ExportRun]) -> String {
    let mut out = String::new();
    for run in runs {
        match run {
            ExportRun::Text { text, .. } => out.push_str(text),
            ExportRun::Math { tex, .. } => out.push_str(tex),
            ExportRun::Image { image, .. } => {
                if image.alt.trim().is_empty() {
                    out.push_str(image.src.trim());
                } else {
                    out.push_str(image.alt.trim());
                }
            }
            ExportRun::Break => out.push(' '),
            ExportRun::FootnoteRef { number, .. } => out.push_str(&number.to_string()),
            ExportRun::Html(node) => html_text(&mut out, node),
        }
    }
    flatten_whitespace(&out)
}

/// Replaces the characters a TSV cell cannot carry with spaces.
fn flatten_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\t' | '\n' | '\r' => out.push(' '),
            '\0' => {}
            other => out.push(other),
        }
    }
    out
}

/// The text content of a preserved HTML subtree (its pictures keep their alt
/// text, a line break becomes a space).
fn html_text(out: &mut String, node: &HtmlNode) {
    match node.tag {
        HtmlTag::Img => {
            if let Some(alt) = node.attr("alt") {
                out.push_str(alt);
            }
        }
        HtmlTag::Br => out.push(' '),
        HtmlTag::Source | HtmlTag::Track => {}
        _ => {
            for child in &node.children {
                match child {
                    HtmlChild::Text(text) => out.push_str(text),
                    HtmlChild::Node(child) => html_text(out, child),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RunStyle;

    fn text_run(text: &str) -> Vec<ExportRun> {
        vec![ExportRun::Text { text: text.to_owned(), style: RunStyle::default(), link: None }]
    }

    fn table(head: &[&str], rows: &[&[&str]]) -> ExportTable {
        ExportTable {
            align: Vec::new(),
            head: head.iter().map(|cell| text_run(cell)).collect(),
            rows: rows.iter().map(|row| row.iter().map(|cell| text_run(cell)).collect()).collect(),
        }
    }

    #[test]
    fn rows_are_tab_separated_with_a_header_first() {
        let table = table(&["A", "B"], &[&["1", "2"], &["3", "4"]]);
        assert_eq!(table_tsv(&table, None), "A\tB\n1\t2\n3\t4");
    }

    #[test]
    fn inner_tabs_and_newlines_become_spaces() {
        let table = table(&["A\tB"], &[&["x\ny", "z\rw"]]);
        assert_eq!(table_tsv(&table, None), "A B\nx y\tz w");
    }

    #[test]
    fn trailing_empty_cells_are_dropped_but_interior_ones_stay() {
        let table = table(&["A", "", ""], &[&["1", "", "2", ""]]);
        assert_eq!(table_tsv(&table, None), "A\n1\t\t2");
    }

    #[test]
    fn the_visible_mask_filters_body_rows_only() {
        let table = table(&["A"], &[&["keep"], &["hide"], &["keep too"]]);
        assert_eq!(table_tsv(&table, Some(&[true, false, true])), "A\nkeep\nkeep too");
        assert_eq!(table_tsv(&table, Some(&[true])), "A\nkeep\nhide\nkeep too", "a short mask keeps rows");
        assert_eq!(table_tsv(&table, Some(&[false, false, false])), "A");
    }

    #[test]
    fn an_empty_table_is_empty_and_a_header_alone_is_kept() {
        assert_eq!(table_tsv(&ExportTable::default(), None), "");
        let header = table(&["Only"], &[]);
        assert_eq!(header.to_tsv(), "Only");
        let body_only = table(&[], &[&["a", "b"]]);
        assert_eq!(table_tsv(&body_only, None), "a\tb");
    }

    #[test]
    fn a_trailing_empty_row_does_not_leave_a_newline() {
        let table = table(&["A"], &[&["one"], &[""], &[""]]);
        assert_eq!(table_tsv(&table, None), "A\none");
    }

    #[test]
    fn markup_disappears_and_math_keeps_its_tex() {
        let table = ExportTable {
            align: Vec::new(),
            head: vec![text_run("Head")],
            rows: vec![vec![
                vec![ExportRun::Text {
                    text: "bold".to_owned(),
                    style: RunStyle { strong: true, ..RunStyle::default() },
                    link: None,
                }],
                vec![ExportRun::Math { tex: "x^2".to_owned(), display: false, asset: None, link: None }],
            ]],
        };
        assert_eq!(table_tsv(&table, None), "Head\nbold\tx^2");
    }
}
