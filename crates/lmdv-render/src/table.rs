//! GFM tables (M-06).
//!
//! Columns are sized from their content: at least 56 pt, at most 360 pt, with
//! wrapping cells. A table wider than the reading column scrolls sideways as a
//! whole, and its header row sticks to the top of the article viewport when the
//! app asked for sticky headers, so a long table keeps its labels visible.
//! `rune-kit`'s data-table component is deliberately not used: a document table
//! is content, not a grid with sorting and selection state.

use lmdv_md::{Align as MdAlign, Inline, Table};
use rune::{Align, El, col, row};

use crate::TableRef;
use crate::block::Renderer;
use crate::inline::InlineOptions;

/// The narrowest a column may shrink to while fitting the reading column.
const MIN_COLUMN: f32 = 56.0;
/// The widest a column grows to before its cells wrap.
const MAX_COLUMN: f32 = 360.0;
/// Horizontal cell padding on each side.
const PAD_X: f32 = 8.0;

impl Renderer<'_> {
    /// One table, keyed inside its block wrapper.
    pub(crate) fn table(&self, table: &Table) -> El {
        let columns = table.head.len().max(table.rows.iter().map(Vec::len).max().unwrap_or(0)).max(1);
        let mut widths = vec![MIN_COLUMN; columns];
        for (column, width) in widths.iter_mut().enumerate() {
            let mut widest = MIN_COLUMN;
            if let Some(cell) = table.head.get(column) {
                widest = widest.max(self.cell_width(cell, true));
            }
            for row in &table.rows {
                if let Some(cell) = row.get(column) {
                    widest = widest.max(self.cell_width(cell, false));
                }
            }
            *width = widest.clamp(MIN_COLUMN, MAX_COLUMN);
        }
        let available = (self.opts.width - 2.0).max(MIN_COLUMN);
        let total: f32 = widths.iter().sum();
        let wide = total > available;
        if wide {
            let scale = (available / total).max(0.0);
            for width in &mut widths {
                *width = (*width * scale).max(MIN_COLUMN);
            }
        }
        let drawn_width: f32 = widths.iter().sum();

        let mut body = col().key("rows").w_full();
        for (index, cells) in table.rows.iter().enumerate() {
            body = body.child(self.table_row(index, cells, &widths, &table.align, false));
        }
        let mut head = self.table_row(0, &table.head, &widths, &table.align, true).key("head");
        if self.opts.sticky_tables {
            head = head.sticky_top(self.opts.sticky_inset);
        }
        let table_el = col()
            .key("table")
            .role("table")
            .label(table_heading_label(&table.head, &self.theme.labels.table))
            .w(drawn_width)
            .radius(self.theme.radius)
            .border(0.5, self.theme.table_border)
            .border_layout()
            .child(head)
            .child(body);
        let drawn = if wide {
            col()
                .key("scroll")
                .w_full()
                .scroll_x()
                .scroll_margin(crate::HORIZONTAL_REVEAL_MARGIN)
                .scroll_edge_fade(10.0)
                .child(table_el)
        } else {
            table_el
        };
        // M-06/X-16: the app's hover action bar and preview read the table as
        // drawn. Without a wire the tree is exactly what it was before.
        let Some(wire) = &self.opts.wires.table else { return drawn };
        let index = self.current_block.get();
        let block = self.doc.block_key(index).map(str::to_owned).unwrap_or_else(|| format!("b{index}"));
        wire(drawn, &table_ref_for(&block, index, table))
    }

    /// One row of cells; the header carries the fill and the stronger weight.
    fn table_row(&self, index: usize, cells: &[Inline], widths: &[f32], aligns: &[MdAlign], header: bool) -> El {
        let row_key = if header { "head-row".to_owned() } else { format!("row-{index}") };
        self.push_scope(row_key.clone());
        let mut row = row().key(row_key).role("row").w_full().items_stretch();
        if header {
            row = row
                .fill(self.theme.table_head_fill)
                .border_sides([0.0, 0.0, 0.5, 0.0], self.theme.table_border)
                .border_layout();
        } else if index > 0 {
            row = row.border_sides([0.5, 0.0, 0.0, 0.0], self.theme.table_border).border_layout();
        }
        for (column, width) in widths.iter().enumerate() {
            let cell_key = format!("c{column}");
            self.push_scope(cell_key.clone());
            let align = aligns.get(column).copied().unwrap_or_default();
            let cell = cells.get(column);
            let mut cell_el = col().key(cell_key).w(*width).px(PAD_X).py(5.0).role("cell");
            if let Some(inline) = cell {
                let style = if header { rune::TextStyle { weight: 600.0, ..self.theme.body } } else { self.theme.body };
                let color = if header { self.theme.table_head_text } else { self.theme.text };
                let mut e = self.inline(inline, InlineOptions::new(style, color).breaks(0, 0));
                e = e.align(match align {
                    MdAlign::None | MdAlign::Left => Align::Start,
                    MdAlign::Center => Align::Center,
                    MdAlign::Right => Align::End,
                });
                cell_el = cell_el.child(e);
            }
            if column + 1 < widths.len() {
                cell_el = cell_el.border_sides([0.0, 0.5, 0.0, 0.0], self.theme.table_border).border_layout();
            }
            self.pop_scope();
            row = row.child(cell_el);
        }
        self.pop_scope();
        row
    }

    /// The content width of a cell, in points, padding included.
    fn cell_width(&self, inline: &Inline, header: bool) -> f32 {
        let style = if header { rune::TextStyle { weight: 600.0, ..self.theme.body } } else { self.theme.body };
        let text = if inline.text.is_empty() { " " } else { inline.text.as_str() };
        rune::text::measure(text, &[], style, None).0 + PAD_X * 2.0 + 2.0
    }
}

/// The table's accessible name: its header cells joined by commas.
fn table_heading_label(head: &[Inline], fallback: &str) -> String {
    let cells: Vec<&str> = head.iter().map(|cell| cell.text.trim()).filter(|cell| !cell.is_empty()).collect();
    if cells.is_empty() { fallback.to_owned() } else { format!("{fallback}: {}", cells.join(", ")) }
}

/// One row's cells as the app copies and previews them, padded to `columns`.
fn padded_cells(cells: &[Inline], columns: usize) -> Vec<String> {
    (0..columns).map(|column| cells.get(column).map_or_else(String::new, cell_text)).collect()
}

/// The [`TableRef`] for `table`, drawn inside the top-level block `block`/`index`.
pub(crate) fn table_ref_for(block: &str, index: usize, table: &Table) -> TableRef {
    let columns = table.head.len().max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
    TableRef {
        block: block.to_owned(),
        index,
        source: table.source.clone(),
        line: table.line,
        end_line: table.line + table.rows.len() + 2,
        align: table.align.clone(),
        head: padded_cells(&table.head, columns),
        rows: table.rows.iter().map(|row| padded_cells(row, columns)).collect(),
        columns,
    }
}

/// Every table the article draws, in reading order (M-06, X-16).
///
/// The list is what [`RenderWires::table`](crate::RenderWires::table) reports
/// for the same document, so a headless scene or a test can name a table
/// without rendering it: tables nested in lists, quotes and footnote
/// definitions are included, each naming its top-level block.
pub fn table_refs(doc: &lmdv_md::MdDoc) -> Vec<TableRef> {
    let mut out = Vec::new();
    for (index, block) in doc.blocks.iter().enumerate() {
        let key = doc.block_key(index).map(str::to_owned).unwrap_or_else(|| format!("b{index}"));
        collect_tables(doc, block, index, &key, &mut out);
    }
    out
}

/// Collects the tables of one block, nested blocks included.
fn collect_tables(doc: &lmdv_md::MdDoc, block: &lmdv_md::Block, index: usize, key: &str, out: &mut Vec<TableRef>) {
    use lmdv_md::Block;
    match block {
        Block::Table(table) => out.push(table_ref_for(key, index, table)),
        Block::List { items, .. } => {
            for item in items {
                for block in item {
                    collect_tables(doc, block, index, key, out);
                }
            }
        }
        Block::Quote { blocks, .. } => {
            for block in blocks {
                collect_tables(doc, block, index, key, out);
            }
        }
        Block::FootnoteDefs => {
            for footnote in &doc.footnotes {
                for block in &footnote.blocks {
                    collect_tables(doc, block, index, key, out);
                }
            }
        }
        _ => {}
    }
}

/// A cell's drawn text with whitespace collapsed (X-16, V-13).
///
/// Markup disappears because the parser's text already excludes it, a formula
/// keeps its TeX so a spreadsheet paste is useful, and every object that draws
/// no glyph (a task box, a picture, a hard break) contributes a space at most.
fn cell_text(inline: &Inline) -> String {
    let mut out = String::with_capacity(inline.text.len());
    for (start, character) in inline.text.char_indices() {
        if character != lmdv_md::OBJECT_CHAR {
            out.push(character);
            continue;
        }
        if let Some(math) = inline.math.iter().find(|math| math.range.contains(&start)) {
            out.push_str(&math.tex);
        } else if inline
            .objects
            .iter()
            .any(|object| matches!(object.kind, lmdv_md::InlineObjectKind::LineBreak) && object.range.start == start)
        {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
