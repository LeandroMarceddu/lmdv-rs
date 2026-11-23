//! GFM tables (M-06).
//!
//! Columns are sized from their content: at least 56 pt, with wrapped cells up
//! to 360 pt wide, but never narrower than their longest unbreakable word. A
//! table wider than the reading column scrolls sideways as a whole, and its
//! header row sticks to the top of the article viewport when the app asked for
//! sticky headers, so a long table keeps its labels visible. `rune-kit`'s
//! data-table component is deliberately not used: a document table is content,
//! not a grid with sorting and selection state.

use lmdv_md::{Align as MdAlign, Inline, InlineObjectKind, InlineStyle, Table};
use rune::{Align, El, TextStyle, col, row};

use crate::block::Renderer;
use crate::inline::InlineOptions;
use crate::{TableCell, TableLink, TableRef};

/// The narrowest a column may shrink to while fitting the reading column.
const MIN_COLUMN: f32 = 56.0;
/// The widest wrapped content grows to before its cells wrap.
///
/// This caps a column's natural width only: the longest word a column cannot
/// break may be wider, and then the table grows and scrolls (OI-138).
const MAX_COLUMN: f32 = 360.0;
/// Horizontal cell padding on each side.
const PAD_X: f32 = 8.0;
/// The body size's share a code span takes, as `inline.rs` draws it.
const CODE_SIZE: f32 = 0.9;

impl Renderer<'_> {
    /// One table, keyed inside its block wrapper.
    pub(crate) fn table(&self, table: &Table) -> El {
        let columns = table.head.len().max(table.rows.iter().map(Vec::len).max().unwrap_or(0)).max(1);
        let mut widths = vec![MIN_COLUMN; columns];
        let mut minimums = vec![MIN_COLUMN; columns];
        for (column, width) in widths.iter_mut().enumerate() {
            let mut widest = MIN_COLUMN;
            let mut narrowest = MIN_COLUMN;
            if let Some(cell) = table.head.get(column) {
                widest = widest.max(self.cell_width(cell, true));
                narrowest = narrowest.max(self.cell_min_width(cell, true));
            }
            for row in &table.rows {
                if let Some(cell) = row.get(column) {
                    widest = widest.max(self.cell_width(cell, false));
                    narrowest = narrowest.max(self.cell_min_width(cell, false));
                }
            }
            *width = widest.clamp(MIN_COLUMN, MAX_COLUMN);
            // A floor is unbounded above: a word the table cannot break sets a
            // width no cap may shrink below, or its ink would run over the next
            // column (OI-138).
            minimums[column] = narrowest.max(MIN_COLUMN);
        }
        let available = (self.opts.width - 2.0).max(MIN_COLUMN);
        let total: f32 = widths.iter().sum();
        if total > available {
            // Shrink toward the column, but never below a column's longest
            // word (the floor loop below): text must not overlap itself. A
            // table whose minimum widths still do not fit scrolls sideways
            // inside the column (OI-138).
            let minimum_total: f32 = minimums.iter().sum();
            if minimum_total <= available {
                let scale = available / total;
                for width in &mut widths {
                    *width *= scale;
                }
            }
        }
        // `MAX_COLUMN` caps wrapped content, never a word: every column keeps
        // at least its floor even when the table would otherwise fit. Such a
        // table grows past the column, and the scroller below shows it.
        for (width, floor) in widths.iter_mut().zip(&minimums) {
            *width = (*width).max(*floor);
        }
        let drawn_width: f32 = widths.iter().sum();
        let wide = drawn_width > available + 0.5;

        let mut body = col().key("rows").w_full();
        for (index, cells) in table.rows.iter().enumerate() {
            body = body.child(self.table_row(index, cells, &widths, &table.align, false));
        }
        let mut head = self.table_row(0, &table.head, &widths, &table.align, true).key("head");
        if self.opts.sticky_tables {
            head = head.sticky_top(self.opts.sticky_inset);
        }
        let heading_cells = padded_cells(&table.head, columns);
        let table_el = col()
            .key("table")
            .role("table")
            .label(table_heading_label(&heading_cells, &self.theme.labels.table))
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
                let color = if header { self.theme.table_head_text } else { self.theme.text };
                let mut e = self.inline(inline, InlineOptions::new(self.cell_style(header), color).breaks(0, 0));
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
        let style = self.cell_style(header);
        let text = if inline.text.is_empty() { " " } else { inline.text.as_str() };
        rune::text::measure(text, &[], style, None).0 + PAD_X * 2.0 + 2.0
    }

    /// The narrowest a cell may draw without breaking a word, in points.
    ///
    /// A table that has to shrink stops at its longest word rather than letting
    /// the text run over its neighbours; the whole table scrolls instead. The
    /// floor is not capped at [`MAX_COLUMN`]: a word wider than that still has
    /// to fit its cell (OI-138).
    fn cell_min_width(&self, inline: &Inline, header: bool) -> f32 {
        let style = self.cell_style(header);
        let text = if inline.text.is_empty() { " " } else { inline.text.as_str() };
        // The widest unbreakable run in the cell's own face.
        let mut narrowest = rune::text::content_widths(text, &[], style).0 + PAD_X * 2.0 + 2.0;
        // A word a styled run covers can draw wider in that run's face.
        let mut cursor = 0;
        for word in text.split_whitespace() {
            let Some(offset) = text.get(cursor..).and_then(|rest| rest.find(word)) else { continue };
            let start = cursor + offset;
            let end = start + word.len();
            cursor = end;
            let mut width = None;
            for span in &inline.spans {
                if span.range.start >= end || start >= span.range.end {
                    continue;
                }
                let Some(styled) = self.styled_face(span.style, style) else { continue };
                let widest = width.get_or_insert_with(|| rune::text::measure(word, &[], style, None).0);
                *widest = (*widest).max(rune::text::measure(word, &[], styled, None).0);
            }
            if let Some(width) = width {
                narrowest = narrowest.max(width + PAD_X * 2.0 + 2.0);
            }
        }
        narrowest
    }

    /// The face a styled run draws in, when it can be wider than the cell's own
    /// face: code and `<kbd>` are monospace at [`CODE_SIZE`] × the body size,
    /// strong is weight 600, emphasis is italic (`inline.rs`).
    fn styled_face(&self, style: InlineStyle, body: TextStyle) -> Option<TextStyle> {
        Some(match style {
            InlineStyle::Code | InlineStyle::Kbd => TextStyle { size: body.size * CODE_SIZE, ..body.mono() },
            InlineStyle::Strong => TextStyle { weight: 600.0, ..body },
            InlineStyle::Em => body.italic(),
            _ => return None,
        })
    }

    /// The style a cell's body draws in: the header carries the stronger weight.
    fn cell_style(&self, header: bool) -> TextStyle {
        if header { TextStyle { weight: 600.0, ..self.theme.body } } else { self.theme.body }
    }
}

/// The table's accessible name: its header cells joined by commas.
fn table_heading_label(head: &[TableCell], fallback: &str) -> String {
    let cells: Vec<&str> = head.iter().map(|cell| cell.text.trim()).filter(|cell| !cell.is_empty()).collect();
    if cells.is_empty() { fallback.to_owned() } else { format!("{fallback}: {}", cells.join(", ")) }
}

/// One cell as the app previews and copies it (V-13, V-17).
///
/// Markup disappears because the parser's text already excludes it, a formula
/// keeps its TeX so a spreadsheet paste is useful, a task box draws its glyph and
/// records its state, and every object that draws no glyph contributes a space at
/// most. Whitespace is collapsed in one pass that also keeps link ranges aligned
/// with the collapsed text.
fn cell_from_inline(inline: &Inline) -> TableCell {
    let mut text = String::with_capacity(inline.text.len());
    let mut offsets = vec![0usize; inline.text.len() + 1];
    let mut task = None;
    let mut space = true;
    for (start, character) in inline.text.char_indices() {
        offsets[start] = text.len();
        if character == lmdv_md::OBJECT_CHAR {
            if let Some(math) = inline.math.iter().find(|math| math.range.contains(&start)) {
                push_words(&mut text, &math.tex, &mut space);
            } else if let Some(object) = inline.objects.iter().find(|object| object.range.contains(&start)) {
                match &object.kind {
                    InlineObjectKind::TaskCheckbox { checked } => {
                        task.get_or_insert(*checked);
                        push_char(&mut text, if *checked { '\u{2611}' } else { '\u{2610}' }, &mut space);
                    }
                    InlineObjectKind::LineBreak => push_space(&mut text, &mut space),
                    _ => {}
                }
            }
            continue;
        }
        if character.is_whitespace() {
            push_space(&mut text, &mut space);
        } else {
            text.push(character);
            space = false;
        }
    }
    offsets[inline.text.len()] = text.len();
    text.truncate(text.trim_end().len());
    let len = text.len();
    let links = inline
        .links
        .iter()
        .filter_map(|link| {
            let start = offsets.get(link.range.start).copied().unwrap_or(len).min(len);
            let end = offsets.get(link.range.end).copied().unwrap_or(len).min(len);
            (start < end).then(|| TableLink { range: start..end, dest: link.dest.clone() })
        })
        .collect();
    TableCell { text, links, task }
}

fn push_char(text: &mut String, character: char, space: &mut bool) {
    text.push(character);
    *space = false;
}

fn push_space(text: &mut String, space: &mut bool) {
    if !*space && !text.is_empty() {
        text.push(' ');
    }
    *space = true;
}

/// `words` with every whitespace run collapsed to one space, as a cell draws it.
fn push_words(text: &mut String, words: &str, space: &mut bool) {
    for character in words.chars() {
        if character.is_whitespace() {
            push_space(text, space);
        } else {
            push_char(text, character, space);
        }
    }
}

/// One row's cells as the app copies and previews them, padded to `columns`.
fn padded_cells(cells: &[Inline], columns: usize) -> Vec<TableCell> {
    (0..columns).map(|column| cells.get(column).map_or_else(TableCell::default, cell_from_inline)).collect()
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
