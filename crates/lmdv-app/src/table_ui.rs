//! The table action bar and the full-window preview's view (M-06, X-16,
//! V-11–V-18).
//!
//! The bar is the renderer wire's chrome: it sits pinned at the table block's
//! top-right, fades in while the pointer is over the table or keyboard focus is
//! inside it, and holds **Copy table (for Excel)** and **Open preview**. It is
//! an overlay, so it takes no layout and never moves the table's sticky header
//! or its text.
//!
//! The preview is a modal layer over the window: a real two-axis scroller with
//! a sticky header that scrolls sideways with its columns, a first column held
//! at the left when the table overflows (V-11), a debounced filter with match
//! highlights and a live count (V-13), sortable headers (V-15), and its own
//! copy and close buttons. `Esc` empties the filter first, then closes (V-14).
//!
//! The scroller carries pinned scroll bars (OI-044): the kit's `ScrollBar`
//! tracks are children of the scroll *area*, not of the scrolling content, and
//! their thumbs follow the scroll offset through live values. A scroll moves
//! the grid under a bar that stays glued to the viewport's edge, so the wide
//! table's horizontal position is always visible while its columns scroll.

use std::cell::RefCell;
use std::rc::Rc;

use lmdv_render::{TableRef, TableWire};
use rune::{Cursor, Cx, El, Key, KeyEvent, Link, Span, TextStyle, Tx, col, el, rich, row, text, text_input};
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit, ScrollBar, ScrollBarStyle, SortDir};

use crate::icons;
use crate::tables::{
    CELL_PAD, PREVIEW_MARGIN, PREVIEW_PAD, TABLE_PREVIEW_CLOSE, TABLE_PREVIEW_COPY, TABLE_PREVIEW_FILTER_KEY,
    TABLE_PREVIEW_KEY, TABLE_PREVIEW_PANEL, TABLE_PREVIEW_SCROLL, TABLE_PREVIEW_ZOOM_IN, TABLE_PREVIEW_ZOOM_OUT,
    TablePreview, highlight_ranges,
};
use crate::window::WindowState;

impl WindowState {
    /// The one table wire this frame's article uses (X-16).
    ///
    /// It captures the kit's palette and the window's copy flash, so the bar's
    /// check appears without rebuilding the document; the buttons dispatch the
    /// same `table.copy`/`table.preview` commands as the palette.
    pub(crate) fn table_wire(&self, kit: &Kit) -> TableWire {
        let kit = kit.clone();
        let flash = Rc::clone(&self.tables.flash);
        Rc::new(move |table: El, reference: &TableRef| table_bar(table, reference, &kit, &flash))
    }
}

/// One table's hover/focus action bar around the drawn table (X-16).
fn table_bar(table: El, reference: &TableRef, kit: &Kit, flash: &Rc<RefCell<Option<(String, f64)>>>) -> El {
    let block = reference.block.clone();
    let copied = flash.borrow().as_ref().is_some_and(|(key, _)| key == &block);
    let glyph = if copied { &icons::CHECK } else { &icons::TABLE };
    let copy_ref = reference.clone();
    let copy = IconButton::new(glyph)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .label(lmdv_i18n::t!("render.copyTable"))
        .help(lmdv_i18n::t!("render.copyTable"))
        .on_press_tx(move |state: &mut WindowState, tx: &mut Tx| {
            state.select_table(copy_ref.clone());
            let _ = state.run_command("table.copy", tx);
        })
        .build(kit)
        .key(format!("table-copy-{block}"));
    let preview_ref = reference.clone();
    let preview = IconButton::new(&icons::EXPAND)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .label(lmdv_i18n::t!("zoom.open"))
        .help(lmdv_i18n::t!("zoom.open"))
        .on_press_tx(move |state: &mut WindowState, tx: &mut Tx| {
            state.select_table(preview_ref.clone());
            let _ = state.run_command("table.preview", tx);
        })
        .build(kit)
        .key(format!("table-preview-{block}"));
    let actions = row()
        .key(format!("table-actions-{block}"))
        .gap(2.0)
        .padding(2.0)
        .radius(8.0)
        .fill(kit.c.layer_01)
        .border(1.0, kit.c.border_base)
        .pin(None, Some(4.0), Some(4.0), None)
        .group_opacity(0.0, 1.0)
        .child(copy)
        .child(preview);
    col().key(format!("table-block-{block}")).w_full().hover_group().child(table).child(actions)
}

/// The preview's modal layer, over the whole window (V-11–V-18).
pub(crate) fn preview_overlay(state: &WindowState, cx: &mut Cx<WindowState>, kit: &Kit, layer: &mut Vec<El>) {
    let Some(preview) = state.table_preview() else { return };
    let [width, height] = cx.layout_size("window-body").unwrap_or([1100.0, 760.0]);
    let panel_width = (width - 2.0 * PREVIEW_MARGIN).max(320.0);
    let panel_height = (height - 2.0 * PREVIEW_MARGIN).max(220.0);

    let scrim = el()
        .key("table-preview-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(kit.c.scrim.mix(rune::Color::BLACK, 0.6))
        .a11y_hidden()
        .on_press(|state: &mut WindowState| state.close_table_preview());
    let copied = state.tables.flash.borrow().as_ref().is_some_and(|(key, _)| key == &preview.table.block);
    let panel = preview_panel(preview, kit, panel_width, panel_height, preview.sort.is_some(), copied);
    let centered = el().absolute(0.0, 0.0).w_full().h_full().padding(PREVIEW_MARGIN).center().child(panel);
    let root = el()
        .key(TABLE_PREVIEW_KEY)
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .label(lmdv_i18n::t!("zoom.table"))
        .focusable(true)
        .no_focus_ring()
        .modal_tx(false, |state: &mut WindowState, tx: &mut Tx| state.table_preview_escape(tx))
        .on_key(|state: &mut WindowState, event: &KeyEvent, tx: &mut Tx| state.table_preview_key(event, tx))
        .child(scrim)
        .child(centered);
    layer.push(root);
}

/// The preview's panel: title, filter, count and the grid.
fn preview_panel(preview: &TablePreview, kit: &Kit, width: f32, height: f32, sorted: bool, copied: bool) -> El {
    let c = &kit.c;
    let title = preview_title(preview);
    let panel_title = title.clone();
    let close = IconButton::new(kit.glyph.close)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .label(lmdv_i18n::t!("zoom.close"))
        .help(lmdv_i18n::t!("zoom.close"))
        .on_press(|state: &mut WindowState| state.close_table_preview())
        .build(kit)
        .key(TABLE_PREVIEW_CLOSE);
    let copy = IconButton::new(if copied { &icons::CHECK } else { &icons::TABLE })
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .label(lmdv_i18n::t!("render.copyTable"))
        .help(lmdv_i18n::t!("render.copyTable"))
        .on_press_tx(|state: &mut WindowState, tx: &mut Tx| state.copy_table(tx))
        .build(kit)
        .key(TABLE_PREVIEW_COPY);
    let zoom_out = IconButton::new(&icons::MINUS)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .label(lmdv_i18n::t!("zoom.out"))
        .help(lmdv_i18n::t!("zoom.out"))
        .on_press(|state: &mut WindowState| {
            state.table_preview_zoom(-1);
        })
        .build(kit)
        .key(TABLE_PREVIEW_ZOOM_OUT);
    let zoom_in = IconButton::new(&icons::PLUS)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .label(lmdv_i18n::t!("zoom.in"))
        .help(lmdv_i18n::t!("zoom.in"))
        .on_press(|state: &mut WindowState| {
            state.table_preview_zoom(1);
        })
        .build(kit)
        .key(TABLE_PREVIEW_ZOOM_IN);
    let header = row()
        .key("table-preview-header")
        .w_full()
        .px(16.0)
        .py(10.0)
        .gap(8.0)
        .center_y()
        .child(text(title).key("table-preview-title").font(kit.ty.ui_medium).color(c.text_base).truncate().grow(1.0))
        .child(zoom_out)
        .child(zoom_in)
        .child(copy)
        .child(close);

    let filter = text_input(&preview.query, kit.ty.ui)
        .key(TABLE_PREVIEW_FILTER_KEY)
        .single_line()
        .label(lmdv_i18n::t!("zoom.filter"))
        .placeholder(lmdv_i18n::t!("zoom.filter"), c.text_faint)
        .shortcut_hint("Control+F")
        .w(240.0)
        .on_edit(|state: &mut WindowState| state.active_table_filter_edit())
        .on_input_change(|state: &mut WindowState, tx| state.table_filter_changed(tx))
        .on_input_key(|state: &mut WindowState, event, tx| state.table_filter_key(event, tx));
    let count = text(preview.count_text())
        .key("table-preview-count")
        .font(kit.ty.small)
        .color(c.text_muted)
        .live_region(rune::LiveRegion::Polite);
    let mut filter_row =
        row().key("table-preview-filter").w_full().px(16.0).pb(10.0).gap(10.0).center_y().child(filter).child(count);
    if let Some(empty) = preview.empty_text() {
        filter_row = filter_row
            .child(text(empty).key("table-preview-empty").font(kit.ty.small).color(c.warning.fg).truncate().grow(1.0));
    } else if sorted {
        filter_row = filter_row.child(
            text(lmdv_i18n::t!("zoom.sortHint"))
                .key("table-preview-sorted")
                .font(kit.ty.small)
                .color(c.text_faint)
                .truncate()
                .grow(1.0),
        );
    }

    let mut panel = col()
        .key(TABLE_PREVIEW_PANEL)
        .role("dialog")
        .label(panel_title)
        .w(width)
        .h(height)
        .hit()
        .radius(10.0)
        .fill(c.bg_base)
        .border(1.0, c.border_base)
        .shadow(8.0, 32.0, rune::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.28 })
        .child(header)
        .child(filter_row)
        .child(el().w_full().h(1.0).fill(c.border_muted));
    let grid = preview_grid(preview, kit);
    panel = panel.child(
        col().key("table-preview-body").w_full().grow(1.0).min_h(0.0).px(PREVIEW_PAD).pb(PREVIEW_PAD).child(grid),
    );
    panel
}

/// The preview panel's title: the table's header cells.
fn preview_title(preview: &TablePreview) -> String {
    let head = preview.table.head.iter().map(|cell| cell.text.as_str()).collect::<Vec<_>>().join(", ");
    format!("{}: {}", lmdv_i18n::t!("zoom.table"), head)
}

/// `style` at the preview's text zoom (V-12).
///
/// The font size and line height scale, so the text is laid out again at the
/// larger size and stays sharp; a transform would blur it.
fn scaled(style: TextStyle, zoom: f32) -> TextStyle {
    TextStyle {
        size: style.size * zoom,
        line_height: if style.line_height > 0.0 { style.line_height * zoom } else { 0.0 },
        ..style
    }
}

/// The scrolling grid: sticky header, virtual rows and the frozen first column.
fn preview_grid(preview: &TablePreview, kit: &Kit) -> El {
    let c = &kit.c;
    let columns = preview.table.columns.max(1);
    let width = preview.drawn_width().max(1.0);
    let label = preview_title(preview);
    let mut header = row()
        .key("table-preview-head")
        .role("row")
        .w(width)
        .h(preview.header_height())
        .items_stretch()
        .sticky_top(0.0)
        .fill(c.layer_02);
    for column in 0..columns {
        header = header.child(header_cell(preview, kit, column));
    }
    let mut content = col().key("table-preview-grid").role("grid").label(label).w(width).child(header);
    let words = Rc::new(preview.words());
    let rows = preview.shown.len();
    let row_height = preview.row_height();
    // OI-044: the bars are pinned to the viewport edges. `always_visible`
    // keeps them attached whenever the axis overflows (a wide table always
    // has a horizontal bar), and the kit drives both thumbs from live scroll
    // values, so scrolling commits no view and never moves the bars.
    let bar = ScrollBarStyle::from_kit(kit).always_visible().width(8.0).hover_width(12.0).track_inset(6.0);
    let (vertical, horizontal) = (bar, bar.horizontal());
    let kit = kit.clone();
    content = content.child(col().key("table-preview-rows").w(width).growth(rune::GrowthSpec::default()).virtual_rows(
        rows,
        row_height,
        |state: &WindowState, index| {
            state
                .table_preview()
                .and_then(|preview| preview.shown().get(index))
                .map_or_else(|| format!("x{index}").into(), |row| format!("r{row}").into())
        },
        move |state: &WindowState, index| preview_row(state, &kit, &words, index),
    ));
    col()
        .key(TABLE_PREVIEW_SCROLL)
        .role("region")
        .label(format!("{}. {}", preview_title(preview), lmdv_i18n::t!("zoom.tableViewport")))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .focusable(true)
        .no_focus_ring()
        .scroll_xy()
        .scroll_edge_fade(12.0)
        // V-12: Ctrl+wheel zooms the text exponentially; a plain wheel keeps
        // Rune's scrolling.
        .on_wheel(|state: &mut WindowState, event: rune::WheelEvent, tx: &mut Tx| state.table_preview_wheel(&event, tx))
        .child(content)
        .scroll_bars_xy_styled(vertical, horizontal)
}

/// One header cell: sortable by click or Enter, with its sort direction.
fn header_cell(preview: &TablePreview, kit: &Kit, column: usize) -> El {
    let c = &kit.c;
    let zoom = preview.zoom;
    let width = preview.column_width(column);
    let title = preview.cell(true, 0, column).to_owned();
    let sorted = preview.sort.and_then(|(sorted, dir)| (sorted == column).then_some(dir));
    let mut cell = row()
        .key(format!("table-preview-head-{column}"))
        .role("columnheader")
        .label(title.clone())
        .sorted(sorted.map(|dir| dir == SortDir::Ascending))
        .w(width)
        .h_full()
        .px(CELL_PAD * zoom)
        .center_y()
        .clip_soft(0.0);
    if preview.frozen && column == 0 {
        cell = cell.sticky_left(0.0).fill(c.layer_02);
    }
    if preview.sortable(column) {
        cell = cell
            .focusable(true)
            .cursor(Cursor::Pointer)
            .hover(c.hover)
            .help(lmdv_i18n::t!("zoom.sortHint"))
            .on_press(move |state: &mut WindowState| state.cycle_table_sort(column))
            .on_key(move |state: &mut WindowState, event: &KeyEvent, _: &mut Tx| {
                let activate = event.key == Key::Enter || matches!(&event.key, Key::Text(text) if text == " ");
                if activate {
                    state.cycle_table_sort(column);
                }
                activate
            });
    }
    let mut inner = row().gap(4.0 * zoom).center_y().child(
        text(title)
            .font(scaled(kit.ty.text_small_medium, zoom))
            .color(if sorted.is_some() { c.text_base } else { c.text_muted })
            .truncate()
            .shrink()
            .min_w(0.0),
    );
    if let Some(dir) = sorted {
        let glyph = if dir == SortDir::Ascending { "↑" } else { "↓" };
        inner =
            inner.child(text(glyph).key("table-preview-sort").font(scaled(kit.ty.small, zoom)).color(c.text_accent));
    }
    cell.child(inner)
}

/// One body row of the virtual list.
fn preview_row(state: &WindowState, kit: &Kit, words: &Rc<Vec<Vec<char>>>, index: usize) -> El {
    let Some(preview) = state.table_preview() else { return el().h(0.0) };
    let Some(&source) = preview.shown().get(index) else { return el().h(0.0) };
    let mut row = row()
        .key(format!("table-preview-row-{source}"))
        .role("row")
        .w(preview.drawn_width().max(1.0))
        .h(preview.row_height())
        .items_stretch()
        .border_sides([0.0, 0.0, 0.5, 0.0], kit.c.border_weak)
        .border_layout();
    for column in 0..preview.table.columns.max(1) {
        row = row.child(body_cell(preview, kit, words, source, column));
    }
    row
}

/// One body cell, with the filter's matches painted behind their text and its
/// links followable (V-13, V-17).
fn body_cell(preview: &TablePreview, kit: &Kit, words: &[Vec<char>], source: usize, column: usize) -> El {
    let c = &kit.c;
    let zoom = preview.zoom;
    let width = preview.column_width(column);
    let cell_data = preview.cell_data(false, source, column);
    let value = cell_data.text.as_str();
    let ranges = highlight_ranges(value, words);
    let mut spans = Vec::with_capacity(ranges.len() + cell_data.links.len());
    for range in ranges {
        spans.push(
            Span::new(range.start, range.end).bg(kit.alpha(c.text_accent, if kit.dark { 0.38 } else { 0.24 }), 2.0),
        );
    }
    for link in &cell_data.links {
        spans.push(Span::new(link.range.start, link.range.end).color(c.text_accent).underline(c.text_accent));
    }
    let mut content = if spans.is_empty() { text(value.to_owned()) } else { rich(value.to_owned(), spans) };
    content = content.font(scaled(kit.ty.text_small, zoom)).color(c.text_base).truncate().shrink().min_w(0.0);
    // A link keeps its own click: the preview closes and the app follows the
    // destination exactly as it would from the article (V-17).
    if !cell_data.links.is_empty() {
        for link in &cell_data.links {
            content = content.link(Link::new(link.range.clone(), link.dest.clone()));
        }
        content =
            content.on_link(|state: &mut WindowState, dest: &str, tx: &mut Tx| state.preview_table_link(dest, tx));
    }
    let mut cell = row()
        .key(format!("table-preview-c{column}"))
        .role("gridcell")
        .w(width)
        .h_full()
        .px(CELL_PAD * zoom)
        .center_y()
        .clip_soft(0.0);
    match preview.table.align.get(column).copied().unwrap_or(lmdv_md::Align::None) {
        lmdv_md::Align::Right => cell = cell.justify_end(),
        lmdv_md::Align::Center => cell = cell.justify_center(),
        lmdv_md::Align::None | lmdv_md::Align::Left => {}
    }
    if preview.frozen && column == 0 {
        cell = cell.sticky_left(0.0).fill(c.bg_base);
    }
    if column + 1 < preview.table.columns {
        cell = cell.border_sides([0.0, 0.5, 0.0, 0.0], c.border_weak).border_layout();
    }
    cell.child(content)
}

// ---- the headless scene ---------------------------------------------------

/// The `table-preview` scene: a wide table with its grammar preview open,
/// filtered to `alpha beta` and sorted by its first column.
pub fn scene_table_preview(state: &mut WindowState) {
    state.reset_to_start_page();
    state.open_synthetic("Tables", TABLE_SCENE_MARKDOWN);
    state.left.set_open(false);
    state.right.set_open(false);
    if let Some(tab) = state.active_tab_mut() {
        tab.view.outline_hidden = true;
    }
    state.sync_outline();
    let Some(model) = state.active_tab().and_then(|tab| tab.view.reader.clone()) else { return };
    let Some(table) = lmdv_render::table_refs(&model).into_iter().next() else { return };
    let mut tx = Tx::default();
    state.select_table(table);
    state.open_table_preview(&mut tx);
    state.set_table_filter("alpha beta");
    state.cycle_table_sort(0);
    // V-12: the scene shows a 125% text zoom over the wide, filtered table.
    state.table_preview_zoom(1);
    state.ui.notice = None;
    state.ui.help = None;
}

/// The scene's page: a wide table whose first column is short and whose notes
/// push the table past the window, so the preview overflows and freezes it.
pub const TABLE_SCENE_MARKDOWN: &str = r#"# Tables

A page about tables. Every table has a hover action bar with **Copy table (for
Excel)** and **Open preview**; the preview filters, sorts and holds its header.

| Name | Owner | Amount | Updated | Notes | Reference | Summary | Status |
| :--- | :--- | ---: | :--- | :--- | :--- | :--- | :--- |
| Alpha one | Ana | 1,200 | 2026-01-04 | Alpha beta notes: a long explanation that makes this table wider than the preview on a laptop screen. | REF-2026-0001-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU | SUMMARY-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU-SIGMA-RHO | Open |
| Beta two | Bea | 10 | 2026-02-11 | Beta gamma only; this row has no match for the first word. | REF-2026-0002-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU | SUMMARY-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU-SIGMA-RHO | Closed |
| Alpha beta three | Cid | 100 | 2026-03-02 | Alpha beta again, with a second matching row and a considerably longer note text. | REF-2026-0003-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU | SUMMARY-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU-SIGMA-RHO | Open |
| Gamma four | Dee | 80 | 2026-04-19 | Gamma delta notes stay out of the filter. | REF-2026-0004-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU | SUMMARY-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU-SIGMA-RHO | Open |
| Delta five | Eve | 1,050 | 2026-05-23 | More prose to push the natural width past the window, one more time. | REF-2026-0005-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU | SUMMARY-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU-SIGMA-RHO | Closed |
| Epsilon six | Fay | 7 | 2026-06-08 | Another long note, making the horizontal scroll real and visible. | REF-2026-0006-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU | SUMMARY-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU-SIGMA-RHO | Open |
| Zeta seven | Gus | 300 | 2026-07-14 | Nothing to see here beyond the width of the table. | REF-2026-0007-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU | SUMMARY-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU-SIGMA-RHO | Closed |
| Eta eight | Hal | 42 | 2026-08-30 | The last row, still wide enough to overflow the preview. | REF-2026-0008-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU | SUMMARY-OMEGA-LAMBDA-PSI-CHI-PHI-UPSILON-TAU-SIGMA-RHO | Open |
"#;
