//! The table action bar, spreadsheet copy and full-window preview (M-06, X-16,
//! V-11–V-18).
//!
//! The renderer hands every drawn GFM table to [`table_wire`](crate::table_ui::table_wire)
//! as a [`lmdv_render::TableRef`]: its top-level block key, its own source
//! range, its alignment and its drawn cells. This module keeps the window's
//! transient table state — the table the bar last pointed at and the open
//! preview — and implements the two actions without touching the document:
//!
//! * **Copy table (for Excel)** builds an [`ExportTable`](lmdv_export::ExportTable)
//!   from the drawn cells and serializes it with [`lmdv_export::table_tsv`].
//!   The preview's hidden rows travel as the visibility mask, and a sorted
//!   preview reorders the rows first, so the TSV is exactly what is on screen
//!   (V-16, X-16). The header is always kept.
//! * **Open preview** opens the modal reader (`table_ui`). Filtering keeps the
//!   rows containing every whitespace-separated word, accent- and case-folded,
//!   after an 80 ms debounce (V-13); the header cycle is ascending, descending,
//!   original order with stable, typed keys (V-15); the first column freezes
//!   only when the table overflows and it takes at most 35% of the view (V-11).
//!
//! Widths are planned once when the preview opens (V-18) and never recomputed
//! while filtering, so the columns do not jump with every letter typed. The
//! preview is window state, not document state: a tab switch, a reparse of the
//! model it came from, or Escape closes it.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::rc::Rc;

use lmdv_core::{DocumentId, TabId};
use lmdv_export::{ExportRun, ExportTable, RunStyle, table_tsv};
use lmdv_render::TableRef;
use rune::{Key, KeyEvent, TextEdit, TextStyle, Tx};
use rune_kit::SortDir;

use crate::window::WindowState;

/// The preview's filter field key; `Ctrl+F` focuses it (V-13).
pub const TABLE_PREVIEW_FILTER_KEY: &str = "table-preview-query";
/// The preview's scrolling viewport, which takes focus when it opens.
pub const TABLE_PREVIEW_SCROLL: &str = "table-preview-scroll";
/// The preview's full-window overlay root.
pub const TABLE_PREVIEW_KEY: &str = "table-preview";
/// The preview's panel.
pub const TABLE_PREVIEW_PANEL: &str = "table-preview-panel";
/// The preview's close button (V-14).
pub const TABLE_PREVIEW_CLOSE: &str = "table-preview-close";
/// The preview's zoom-in button (V-12).
pub const TABLE_PREVIEW_ZOOM_IN: &str = "table-preview-zoom-in";
/// The preview's zoom-out button (V-12).
pub const TABLE_PREVIEW_ZOOM_OUT: &str = "table-preview-zoom-out";
/// The preview's own copy button (V-16).
pub const TABLE_PREVIEW_COPY: &str = "table-preview-copy";
/// How long the filter waits after the last key (V-13).
pub const FILTER_DEBOUNCE: f64 = 0.080;
/// How long the bar's copy check shows (X-16).
pub const COPY_FLASH_SECONDS: f64 = 1.2;
/// The preview panel's margin around the window.
pub const PREVIEW_MARGIN: f32 = 24.0;
/// The preview panel's own padding.
pub const PREVIEW_PAD: f32 = 14.0;
/// The narrowest a preview column shrinks to.
pub const MIN_COLUMN: f32 = 56.0;
/// The widest a column's natural (one-line) width grows to before its cells
/// wrap. A column's unbreakable floor is not capped ([`minimum_widths`]).
pub const MAX_COLUMN: f32 = 360.0;
/// Horizontal cell padding on each side.
pub const CELL_PAD: f32 = 10.0;
/// The preview's fixed row height, which its virtual list assumes.
pub const ROW_HEIGHT: f32 = 32.0;
/// The preview's header height.
pub const HEADER_HEIGHT: f32 = 34.0;
/// The first column freezes only while it takes at most this share of the view.
pub const FIRST_COLUMN_SHARE: f32 = 0.35;
/// The nominal body text size used to plan column widths.
const MEASURE_SIZE: f32 = 15.0;

/// The table the action bar last pointed at, with the model it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableTarget {
    /// The tab whose article drew the table.
    pub tab: TabId,
    /// The document that tab shows.
    pub doc: DocumentId,
    /// The parsed model's revision when the bar was pressed.
    pub revision: u64,
    /// The table as drawn.
    pub table: TableRef,
}

/// The widest the preview's text zooms (V-12).
pub const MAX_ZOOM: f32 = 2.0;
/// The smallest the preview's text zooms (V-12).
pub const MIN_ZOOM: f32 = 0.5;
/// What `+`/`−`, the preview's zoom buttons and one Ctrl+wheel notch change (V-12).
pub const ZOOM_STEP: f32 = 1.25;
/// The wheel travel, in points, that doubles the preview's zoom (V-12).
const ZOOM_WHEEL_POINTS: f32 = 240.0;

/// One open table preview.
///
/// The table's drawn cells are the source of truth: the preview never re-reads
/// the document, so the file and the buffer cannot change under it, and a
/// reparse closes it instead of showing stale rows.
#[derive(Debug)]
pub struct TablePreview {
    /// The tab the table was drawn in.
    pub tab: TabId,
    /// The document the table belongs to.
    pub doc: DocumentId,
    /// The model revision the table came from.
    pub revision: u64,
    /// The table as drawn.
    pub table: TableRef,
    /// Column widths in points at 100%, planned for the view (V-13/V-18).
    pub widths: Vec<f32>,
    /// The natural widths the plan started from, kept for a zoom or a resize.
    natural: Vec<f32>,
    /// The minimum widths the plan started from, kept for a zoom or a resize.
    minimum: Vec<f32>,
    /// The width budget the columns were planned for, so a resize can refit.
    pub available: f32,
    /// The sum of [`Self::widths`] at 100%.
    pub table_width: f32,
    /// The text zoom: 0.5 to 2, 1 at open (V-12).
    pub zoom: f32,
    /// The table is wider than the view and scrolls sideways.
    pub overflow: bool,
    /// The first column is held to the left edge while scrolling (V-11).
    pub frozen: bool,
    /// The filter field's live text.
    pub query: TextEdit,
    /// The filter whose rows the preview shows; empty means none.
    pub applied: String,
    /// When the debounce applies [`Self::query`] (V-13).
    pub due: Option<f64>,
    /// The active sort, or `None` for the document's order (V-15).
    pub sort: Option<(usize, SortDir)>,
    /// Body row indices in the current row order.
    pub order: Vec<usize>,
    /// One flag per body row in document order: false when the filter hid it.
    pub visible: Vec<bool>,
    /// [`Self::order`] without the rows [`Self::visible`] hides.
    pub shown: Vec<usize>,
    /// Each body row's folded text, for the filter (V-13).
    pub row_text: Vec<String>,
}

impl TablePreview {
    /// Plans the preview for `target` in a window body `width` points wide.
    pub fn new(target: TableTarget, width: f32) -> Self {
        let table = target.table;
        let available = preview_available(width);
        let natural = natural_widths(&table);
        let minimum = minimum_widths(&table);
        let widths = plan_widths(&natural, &minimum, available);
        let table_width: f32 = widths.iter().sum();
        let row_text: Vec<String> = table
            .rows
            .iter()
            .map(|row| fold(&row.iter().map(|cell| cell.text.as_str()).collect::<Vec<_>>().join(" ")))
            .collect();
        let order: Vec<usize> = (0..table.rows.len()).collect();
        let mut preview = Self {
            tab: target.tab,
            doc: target.doc,
            revision: target.revision,
            table,
            widths,
            natural,
            minimum,
            available,
            table_width,
            zoom: 1.0,
            overflow: false,
            frozen: false,
            query: TextEdit::default(),
            applied: String::new(),
            due: None,
            sort: None,
            visible: vec![true; order.len()],
            shown: order.clone(),
            order,
            row_text,
        };
        preview.fit();
        preview
    }

    /// Recomputes the sideways scrolling and frozen column from the drawn width.
    fn fit(&mut self) {
        let drawn = self.drawn_width();
        // A table whose minimum widths do not fit draws at its floors and
        // scrolls sideways; one that fits is squeezed or grown by its columns,
        // and only the rounding of a squeezed minimum can still reach the
        // scroller (V-18, OI-194).
        self.overflow = drawn > self.available + 0.5;
        self.frozen = self.overflow
            && self.widths.first().is_some_and(|first| first * self.zoom <= self.available * FIRST_COLUMN_SHARE);
    }

    /// The drawn width of `column` at the current zoom (V-12).
    pub fn column_width(&self, column: usize) -> f32 {
        self.widths.get(column).copied().unwrap_or(MIN_COLUMN) * self.zoom
    }

    /// The whole grid's drawn width at the current zoom.
    pub fn drawn_width(&self) -> f32 {
        self.table_width * self.zoom
    }

    /// The preview's body row height at the current zoom (V-12).
    pub fn row_height(&self) -> f32 {
        ROW_HEIGHT * self.zoom
    }

    /// The preview's header height at the current zoom (V-12).
    pub fn header_height(&self) -> f32 {
        HEADER_HEIGHT * self.zoom
    }

    /// Sets the text zoom, clamped to [`MIN_ZOOM`]..=[`MAX_ZOOM`] (V-12).
    /// True when it changed.
    pub fn set_zoom(&mut self, zoom: f32) -> bool {
        let zoom = if zoom.is_finite() { zoom.clamp(MIN_ZOOM, MAX_ZOOM) } else { self.zoom };
        if (zoom - self.zoom).abs() < 0.001 {
            return false;
        }
        self.zoom = zoom;
        self.fit();
        true
    }

    /// `+`: one [`ZOOM_STEP`] larger (V-12).
    pub fn zoom_in(&mut self) -> bool {
        self.set_zoom(self.zoom * ZOOM_STEP)
    }

    /// `−`: one [`ZOOM_STEP`] smaller (V-12).
    pub fn zoom_out(&mut self) -> bool {
        self.set_zoom(self.zoom / ZOOM_STEP)
    }

    /// Ctrl+wheel: `dy` points of travel scale the zoom exponentially (V-12).
    ///
    /// One [`ZOOM_WHEEL_POINTS`] of travel doubles or halves it, so a trackpad's
    /// small deltas zoom gradually and a mouse notch moves by a fixed ratio.
    pub fn zoom_by_wheel(&mut self, dy: f32) -> bool {
        if !dy.is_finite() || dy == 0.0 {
            return false;
        }
        self.set_zoom(self.zoom * 2f32.powf(-dy / ZOOM_WHEEL_POINTS))
    }

    /// Replans the columns for a new window body width (V-12/V-18).
    ///
    /// Called when the window resizes: the preview keeps its filter and sort and
    /// refits its columns instead of closing (OI-135).
    pub fn refit(&mut self, width: f32) -> bool {
        let available = preview_available(width);
        if (available - self.available).abs() < 0.5 {
            return false;
        }
        self.available = available;
        self.widths = plan_widths(&self.natural, &self.minimum, available);
        self.table_width = self.widths.iter().sum();
        self.fit();
        true
    }

    /// The body row indices the preview shows, in order.
    pub fn shown(&self) -> &[usize] {
        &self.shown
    }

    /// The folded filter words the preview highlights.
    pub fn words(&self) -> Vec<Vec<char>> {
        fold_words(&self.applied)
    }

    /// `n of total rows`, or the plain row count without a filter (V-13).
    pub fn count_text(&self) -> String {
        let total = lmdv_i18n::number(self.table.rows.len() as i64);
        if self.applied.trim().is_empty() {
            let n = lmdv_i18n::number(self.shown.len() as i64);
            if self.shown.len() == 1 {
                lmdv_i18n::t!("zoom.rowsOne", "n" => n)
            } else {
                lmdv_i18n::t!("zoom.rows", "n" => n)
            }
        } else {
            lmdv_i18n::t!(
                "zoom.rowsOf",
                "n" => lmdv_i18n::number(self.shown.len() as i64),
                "total" => total
            )
        }
    }

    /// The no-rows message while a filter hides every row (V-13).
    pub fn empty_text(&self) -> Option<String> {
        if self.applied.trim().is_empty() || !self.shown.is_empty() {
            return None;
        }
        Some(lmdv_i18n::t!("zoom.noRows", "q" => self.applied.trim()))
    }

    /// Whether `column`'s header can sort (V-15).
    ///
    /// The preview shows the drawn header text, so a header link or control is
    /// not present to keep its own click; the remaining conditions are the
    /// original's: one body, at least two rows, and a header cell for the
    /// column. GFM cells never span rows or columns.
    pub fn sortable(&self, column: usize) -> bool {
        column < self.table.columns && column < self.table.head.len() && self.table.rows.len() >= 2
    }

    /// One cell's drawn text, header or body.
    pub fn cell(&self, header: bool, row: usize, column: usize) -> &str {
        &self.cell_data(header, row, column).text
    }

    /// One cell as drawn, with its links and task state (V-17).
    pub fn cell_data(&self, header: bool, row: usize, column: usize) -> &lmdv_render::TableCell {
        static EMPTY: lmdv_render::TableCell =
            lmdv_render::TableCell { text: String::new(), links: Vec::new(), task: None };
        let cells = if header { Some(&self.table.head) } else { self.table.rows.get(row) };
        cells.and_then(|cells| cells.get(column)).unwrap_or(&EMPTY)
    }

    /// Applies [`Self::query`] now, debounce or not.
    pub fn apply_filter(&mut self) {
        self.due = None;
        self.applied = self.query.text.clone();
        let words = fold_words(&self.applied);
        if words.is_empty() {
            self.visible.fill(true);
        } else {
            for (index, text) in self.row_text.iter().enumerate() {
                self.visible[index] = words.iter().all(|word| contains_folded(text, word));
            }
        }
        self.rebuild_shown();
    }

    /// Clears the filter and shows every row.
    pub fn clear_filter(&mut self) {
        self.query.replace_text(String::new());
        self.apply_filter();
    }

    /// Sets the active sort and rebuilds the row order (V-15).
    pub fn set_sort(&mut self, column: usize, dir: SortDir) {
        if !self.sortable(column) {
            return;
        }
        self.sort = Some((column, dir));
        self.order = sort_order(&self.table, column, dir);
        self.rebuild_shown();
    }

    /// A header click: ascending, then descending, then the document's order.
    pub fn cycle_sort(&mut self, column: usize) {
        if !self.sortable(column) {
            return;
        }
        match self.sort {
            Some((sorted, SortDir::Ascending)) if sorted == column => {
                self.set_sort(column, SortDir::Descending);
            }
            Some((sorted, SortDir::Descending)) if sorted == column => self.reset_sort(),
            _ => self.set_sort(column, SortDir::Ascending),
        }
    }

    /// Back to the document's row order (the third header click).
    pub fn reset_sort(&mut self) {
        self.sort = None;
        self.order = (0..self.table.rows.len()).collect();
        self.rebuild_shown();
    }

    /// The TSV this preview copies: its visible rows, in the shown order (V-16).
    pub fn copy_tsv(&self) -> String {
        if self.sort.is_some() {
            copy_tsv(&self.table, Some(&self.shown), &self.visible)
        } else {
            copy_tsv(&self.table, None, &self.visible)
        }
    }

    fn rebuild_shown(&mut self) {
        self.shown = self.order.iter().copied().filter(|&row| self.visible.get(row).copied().unwrap_or(true)).collect();
    }
}

/// The window's transient table state; never persisted.
#[derive(Debug)]
pub struct TableState {
    /// The table the bar last pointed at.
    pub target: Option<TableTarget>,
    /// The open preview.
    pub preview: Option<TablePreview>,
    /// The bar's copy check: the block key and the time it disappears.
    ///
    /// The wire draws from this cell, so a successful copy needs no rebuild of
    /// the document to show its check (X-16).
    pub flash: Rc<RefCell<Option<(String, f64)>>>,
    /// The flash expiry wake has been requested.
    pub flash_armed: bool,
    /// The last laid-out window body, so the preview can plan its widths.
    pub viewport: Rc<Cell<[f32; 2]>>,
}

impl Default for TableState {
    fn default() -> Self {
        Self {
            target: None,
            preview: None,
            flash: Rc::new(RefCell::new(None)),
            flash_armed: false,
            viewport: Rc::new(Cell::new([1100.0, 760.0])),
        }
    }
}

impl WindowState {
    /// Remembers the table the action bar was pressed on (M-06, X-16).
    pub(crate) fn select_table(&mut self, table: TableRef) {
        let Some(tab) = self.active_tab() else { return };
        let Some(doc) = tab.document() else { return };
        let revision = tab.view.reader.as_ref().map_or(0, |model| model.revision);
        self.tables.target = Some(TableTarget { tab: tab.id, doc: doc.id, revision, table });
    }

    /// The current target, while its tab, document and model revision still match.
    pub fn table_target(&self) -> Option<&TableTarget> {
        let target = self.tables.target.as_ref()?;
        let tab = self.active_tab()?;
        if tab.id != target.tab || tab.document_id() != Some(target.doc) {
            return None;
        }
        let revision = tab.view.reader.as_ref().map_or(0, |model| model.revision);
        (revision == target.revision).then_some(target)
    }

    /// The open preview.
    pub fn table_preview(&self) -> Option<&TablePreview> {
        self.tables.preview.as_ref()
    }

    /// The `on_edit` accessor for the preview's filter field.
    pub(crate) fn active_table_filter_edit(&mut self) -> &mut TextEdit {
        if let Some(preview) = self.tables.preview.as_mut() {
            return &mut preview.query;
        }
        &mut self.editor.scratch
    }

    /// Opens the full-window preview of the last table the bar pointed at.
    ///
    /// A table with no rows or no width does not open (V-01); the bar's own
    /// button is only drawn for a drawn table, so the check mainly guards a
    /// stale target.
    pub(crate) fn open_table_preview(&mut self, tx: &mut Tx) {
        if self.tables.preview.is_some() {
            self.close_table_preview();
            return;
        }
        let Some(target) = self.table_target().cloned() else {
            self.notice(lmdv_i18n::t!("table.noTarget"));
            return;
        };
        if target.table.columns == 0 || target.table.rows.is_empty() {
            self.notice(lmdv_i18n::t!("table.noRows"));
            return;
        }
        let width = self.tables.viewport.get()[0].max(360.0);
        self.tables.preview = Some(TablePreview::new(target, width));
        tx.focus(TABLE_PREVIEW_SCROLL, false);
    }

    /// Closes the preview; the filter and sort are forgotten with it.
    pub(crate) fn close_table_preview(&mut self) {
        self.tables.preview = None;
    }

    /// Escape in the preview: a non-empty filter clears first, then Escape
    /// closes it (V-14).
    pub(crate) fn table_preview_escape(&mut self, tx: &mut Tx) {
        let Some(preview) = self.tables.preview.as_mut() else { return };
        if preview.query.text.trim().is_empty() && preview.applied.trim().is_empty() {
            self.close_table_preview();
            return;
        }
        preview.clear_filter();
        tx.instant();
    }

    /// The preview overlay's keys while focus is on the table or its chrome:
    /// `Ctrl+F` reaches the filter, Escape clears or closes (V-13/V-14).
    pub(crate) fn table_preview_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if event.key == Key::Escape {
            self.table_preview_escape(tx);
            return true;
        }
        let plain = !event.mods.ctrl && !event.mods.cmd && !event.mods.alt && !event.mods.logo;
        if plain && let Key::Text(text) = &event.key {
            match text.as_str() {
                "+" | "=" => {
                    self.table_preview_zoom(1);
                    return true;
                }
                "-" => {
                    self.table_preview_zoom(-1);
                    return true;
                }
                "1" => {
                    self.table_preview_actual_size();
                    return true;
                }
                _ => {}
            }
        }
        if event.is_command('f') && !event.mods.alt && !event.mods.shift {
            let length = self.tables.preview.as_ref().map_or(0, |preview| preview.query.text.len());
            if let Some(preview) = self.tables.preview.as_mut() {
                preview.query.set_selection(0, length);
            }
            tx.focus(TABLE_PREVIEW_FILTER_KEY, false);
            return true;
        }
        // The preview is modal: the app's shortcuts must not act through it
        // (Ctrl+W would close the tab under the dialog, F1 would open Help
        // behind it). Keys the preview owns were handled above.
        event.mods.ctrl || event.mods.cmd || matches!(event.key, Key::Function(_))
    }

    /// The filter field changed: apply it 80 ms after the last key (V-13).
    pub(crate) fn table_filter_changed(&mut self, tx: &mut Tx) {
        let Some(preview) = self.tables.preview.as_mut() else { return };
        let due = self.ui.last_now + FILTER_DEBOUNCE;
        preview.due = Some(due);
        tx.wake_at(due);
    }

    /// The filter field's keys: Escape clears or closes; Return and Down return
    /// focus to the table (V-14).
    pub(crate) fn table_filter_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if event.key == Key::Escape {
            self.table_preview_escape(tx);
            return true;
        }
        let plain = !event.mods.ctrl && !event.mods.cmd && !event.mods.alt && !event.mods.logo;
        if plain && matches!(event.key, Key::Enter | Key::Down) {
            tx.focus(TABLE_PREVIEW_SCROLL, false);
            return true;
        }
        if event.is_command('f') && !event.mods.alt && !event.mods.shift {
            let length = self.tables.preview.as_ref().map_or(0, |preview| preview.query.text.len());
            if let Some(preview) = self.tables.preview.as_mut() {
                preview.query.set_selection(0, length);
            }
            return true;
        }
        false
    }

    /// Sets the preview filter and applies it at once (scenes and tests).
    pub(crate) fn set_table_filter(&mut self, query: &str) {
        let Some(preview) = self.tables.preview.as_mut() else { return };
        preview.query.replace_text(query.to_owned());
        preview.apply_filter();
    }

    /// Clears the preview filter (`table.filter-reset`).
    pub(crate) fn clear_table_filter(&mut self) {
        if let Some(preview) = self.tables.preview.as_mut() {
            preview.clear_filter();
        }
    }

    /// A header click in the preview: ascending, descending, original (V-15).
    pub(crate) fn cycle_table_sort(&mut self, column: usize) {
        if let Some(preview) = self.tables.preview.as_mut() {
            preview.cycle_sort(column);
        }
    }

    /// Restores the document's row order (`table.sort-reset`).
    pub(crate) fn reset_table_sort(&mut self) {
        if let Some(preview) = self.tables.preview.as_mut() {
            preview.reset_sort();
        }
    }

    /// `+`/`−` and the preview's zoom buttons: one [`ZOOM_STEP`] at a time
    /// (V-12). True when the zoom changed.
    pub(crate) fn table_preview_zoom(&mut self, step: i8) -> bool {
        let Some(preview) = self.tables.preview.as_mut() else { return false };
        if step > 0 { preview.zoom_in() } else { preview.zoom_out() }
    }

    /// `1` and the actual-size control: back to 100% (V-12).
    pub(crate) fn table_preview_actual_size(&mut self) -> bool {
        self.tables.preview.as_mut().is_some_and(|preview| preview.set_zoom(1.0))
    }

    /// Ctrl+wheel over the preview: an exponential zoom (V-12).
    ///
    /// A plain wheel keeps Rune's scrolling; the modified one is taken here and
    /// never scrolls the grid.
    pub(crate) fn table_preview_wheel(&mut self, event: &rune::WheelEvent, _tx: &mut Tx) -> bool {
        if !(event.mods.ctrl || event.mods.cmd) || event.dy == 0.0 || !event.dy.is_finite() {
            return false;
        }
        self.tables.preview.as_mut().is_some_and(|preview| preview.zoom_by_wheel(event.dy))
    }

    /// A link inside the preview: close it and follow the destination (V-17).
    ///
    /// The link is the document's own, so the app's ordinary handler opens it
    /// exactly as a click on the article would; the original dispatches a
    /// synthetic click on the source for the same result.
    pub(crate) fn preview_table_link(&mut self, dest: &str, tx: &mut Tx) {
        self.close_table_preview();
        self.open_link(dest, tx);
    }

    /// Copies the table the bar points at, or the preview's visible rows in
    /// their shown order, as TSV for a spreadsheet (V-16, X-16).
    pub(crate) fn copy_table(&mut self, tx: &mut Tx) {
        let copied = if let Some(preview) = self.tables.preview.as_ref() {
            let tsv = preview.copy_tsv();
            Some((preview.table.block.clone(), tsv))
        } else {
            self.table_target().map(|target| {
                let visible = vec![true; target.table.rows.len()];
                (target.table.block.clone(), copy_tsv(&target.table, None, &visible))
            })
        };
        let Some((block, tsv)) = copied else {
            self.notice(lmdv_i18n::t!("table.noTarget"));
            return;
        };
        if tsv.is_empty() {
            self.notice(lmdv_i18n::t!("table.nothingToCopy"));
            return;
        }
        tx.copy(tsv);
        self.notice(lmdv_i18n::t!("msg.tableCopied"));
        let until = self.ui.last_now + COPY_FLASH_SECONDS;
        *self.tables.flash.borrow_mut() = Some((block, until));
        self.tables.flash_armed = false;
        tx.wake_at(until);
    }

    /// Applies a due filter, closes a preview whose tab or model moved on, and
    /// expires the bar's copy check (X-16, V-13).
    pub(crate) fn poll_tables(&mut self, now: f64, tx: &mut Tx) -> bool {
        let mut changed = false;
        let stale = self.tables.preview.as_ref().is_some_and(|preview| {
            let Some(tab) = self.active_tab() else { return true };
            let revision = tab.view.reader.as_ref().map_or(0, |model| model.revision);
            tab.id != preview.tab || tab.document_id() != Some(preview.doc) || revision != preview.revision
        });
        if stale {
            self.tables.preview = None;
            changed = true;
        }
        if let Some(preview) = self.tables.preview.as_mut() {
            // OI-135: a window resize refits the columns; the filter and sort
            // stay, so the preview never has to be reopened.
            let width = self.tables.viewport.get()[0].max(360.0);
            if preview.refit(width) {
                changed = true;
            }
            if preview.due.is_some_and(|due| now >= due) {
                preview.apply_filter();
                changed = true;
            }
        }
        let flash = self.tables.flash.borrow().clone();
        if let Some((_, until)) = flash {
            if now >= until {
                *self.tables.flash.borrow_mut() = None;
                self.tables.flash_armed = false;
                changed = true;
            } else if !self.tables.flash_armed {
                self.tables.flash_armed = true;
                tx.wake_at(until);
            }
        }
        changed
    }
}

// ---- copy -----------------------------------------------------------------

/// The export model of one drawn table (X-16).
pub fn export_table(table: &TableRef) -> ExportTable {
    fn run(text: &str) -> Vec<ExportRun> {
        vec![ExportRun::Text { text: text.to_owned(), style: RunStyle::default(), link: None }]
    }
    ExportTable {
        align: table.align.clone(),
        head: table.head.iter().map(|cell| run(&cell.text)).collect(),
        rows: table.rows.iter().map(|row| row.iter().map(|cell| run(&cell.text)).collect()).collect(),
    }
}

/// The TSV of `table`, hiding the rows `visible` masks false (V-16).
///
/// `order` reorders the body rows first; the mask then travels to
/// [`lmdv_export::table_tsv`] in the unsorted case, and a sorted copy already
/// carries only the shown rows. The header is always kept, both by the
/// original rule and by `table_tsv`.
pub fn copy_tsv(table: &TableRef, order: Option<&[usize]>, visible: &[bool]) -> String {
    let export = export_table(table);
    match order {
        None => table_tsv(&export, Some(visible)),
        Some(order) => {
            let rows = order.iter().filter_map(|&row| export.rows.get(row).cloned()).collect::<Vec<_>>();
            table_tsv(&ExportTable { align: export.align, head: export.head, rows }, None)
        }
    }
}

// ---- widths ---------------------------------------------------------------

/// The width the preview's grid may use inside a window body `width` wide.
///
/// The preview's own margins, its panel padding and the vertical scrollbar are
/// taken out; `TablePreview::refit` plans for the same number (V-18).
pub fn preview_available(width: f32) -> f32 {
    (width - 2.0 * PREVIEW_MARGIN - 2.0 * PREVIEW_PAD - 18.0).max(MIN_COLUMN)
}

/// Each column's natural width in points: its widest cell, clamped (V-18).
pub fn natural_widths(table: &TableRef) -> Vec<f32> {
    let head = TextStyle::new(MEASURE_SIZE, 600.0);
    let body = TextStyle::new(MEASURE_SIZE, 400.0);
    (0..table.columns.max(1))
        .map(|column| {
            let mut widest = MIN_COLUMN;
            if let Some(cell) = table.head.get(column) {
                widest = widest.max(measure_cell(&cell.text, head));
            }
            for row in &table.rows {
                if let Some(cell) = row.get(column) {
                    widest = widest.max(measure_cell(&cell.text, body));
                }
            }
            widest.clamp(MIN_COLUMN, MAX_COLUMN)
        })
        .collect()
}

fn measure_cell(text: &str, style: TextStyle) -> f32 {
    let text = if text.trim().is_empty() { " " } else { text };
    rune::text::measure(text, &[], style, None).0 + CELL_PAD * 2.0 + 2.0
}

/// Each column's minimum width in points: the widest run its cells cannot
/// break, which wrapping cannot split (V-18, OI-194).
///
/// This follows the reading table's floor (`lmdv-render`'s
/// `cell_min_width`): the CSS min-content width in the cell's own face, at
/// least [`MIN_COLUMN`] but not capped at [`MAX_COLUMN`]. A token wider than
/// the cap therefore grows its column; the grid scrolls instead of clipping
/// the token. The wire carries drawn cell text, not inline styles, and the
/// preview draws every cell in one face, so there is no styled run to measure.
pub fn minimum_widths(table: &TableRef) -> Vec<f32> {
    let head = TextStyle::new(MEASURE_SIZE, 600.0);
    let body = TextStyle::new(MEASURE_SIZE, 400.0);
    (0..table.columns.max(1))
        .map(|column| {
            let mut widest = MIN_COLUMN;
            if let Some(cell) = table.head.get(column) {
                widest = widest.max(min_content_cell(&cell.text, head));
            }
            for row in &table.rows {
                if let Some(cell) = row.get(column) {
                    widest = widest.max(min_content_cell(&cell.text, body));
                }
            }
            widest
        })
        .collect()
}

/// The width of a cell's widest unbreakable run, padding included.
fn min_content_cell(text: &str, style: TextStyle) -> f32 {
    let text = if text.trim().is_empty() { " " } else { text };
    rune::text::content_widths(text, &[], style).0 + CELL_PAD * 2.0 + 2.0
}

/// Plans the preview's columns for `available` points (V-18).
///
/// A table whose minimum widths do not fit draws at its floors and scrolls;
/// one that fits is scaled to the view — shrunk to fit when it would overflow
/// at its natural widths, and grown at most 1.5× when it is narrow. The floors
/// are applied last and are unbounded above (OI-194): a token wider than
/// [`MAX_COLUMN`] still fits its cell, so the grid grows and scrolls rather
/// than truncating it.
pub fn plan_widths(natural: &[f32], minimum: &[f32], available: f32) -> Vec<f32> {
    let available = available.max(MIN_COLUMN);
    let min_total: f32 = minimum.iter().sum();
    let max_total: f32 = natural.iter().sum::<f32>().max(1.0);
    let mut widths = if min_total > available {
        natural.to_vec()
    } else if max_total > available {
        let factor = available / max_total;
        natural.iter().zip(minimum).map(|(width, floor)| (width * factor).max(*floor)).collect()
    } else {
        let factor = (available / max_total).min(1.5);
        natural.iter().map(|width| width * factor).collect()
    };
    for (width, floor) in widths.iter_mut().zip(minimum) {
        *width = (*width).max(*floor);
    }
    widths
}

// ---- folding --------------------------------------------------------------

/// `text` in lower case with the accents Latin letters carry removed (V-13).
///
/// This mirrors the original's `normalize("NFD")` plus a mark strip for the
/// precomposed Latin letters; letters NFD does not decompose (ø, æ, ß …) stay
/// as they are, as they do there.
pub fn fold(text: &str) -> String {
    text.chars().filter_map(fold_char).collect()
}

pub(crate) fn fold_char(character: char) -> Option<char> {
    let lower = character.to_lowercase().next().unwrap_or(character);
    Some(match lower {
        '\u{0300}'..='\u{036f}' => return None,
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' => 'h',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' | 'ŧ' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    })
}

/// The filter's words, folded to characters (V-13).
pub fn fold_words(query: &str) -> Vec<Vec<char>> {
    query
        .split_whitespace()
        .map(|word| fold(word).chars().collect::<Vec<_>>())
        .filter(|word| !word.is_empty())
        .collect()
}

fn contains_folded(text: &str, word: &[char]) -> bool {
    if word.is_empty() {
        return true;
    }
    let chars: Vec<char> = text.chars().collect();
    chars.windows(word.len()).any(|window| window == word)
}

/// The display byte ranges of every word occurrence in `text` (V-13).
///
/// Folding maps one original character to at most one folded character, so a
/// folded match maps back to the exact characters that carried it.
pub fn highlight_ranges(text: &str, words: &[Vec<char>]) -> Vec<std::ops::Range<usize>> {
    if words.is_empty() || text.is_empty() {
        return Vec::new();
    }
    let mut folded = Vec::new();
    let mut from = Vec::new();
    let mut end = Vec::new();
    for (start, character) in text.char_indices() {
        let Some(folded_char) = fold_char(character) else { continue };
        folded.push(folded_char);
        from.push(start);
        end.push(start + character.len_utf8());
    }
    let mut ranges = Vec::new();
    for word in words {
        if word.is_empty() || word.len() > folded.len() {
            continue;
        }
        for at in 0..=folded.len() - word.len() {
            if folded[at..at + word.len()] == word[..] {
                ranges.push(from[at]..end[at + word.len() - 1]);
            }
        }
    }
    ranges.sort_by_key(|range| range.start);
    ranges.dedup();
    ranges
}

// ---- sorting (V-15) -------------------------------------------------------

/// One column's sort key: an empty cell, a number (or date) or text.
#[derive(Clone, Debug, PartialEq)]
pub enum SortKey {
    /// No cell text.
    Empty,
    /// A parsed number, or a date as days since 1970-01-01.
    Number(f64),
    /// Text that did not parse in a numeric or date column, or any text column.
    Text(String),
}

/// The body rows in `column`'s order (V-15).
pub fn sort_order(table: &TableRef, column: usize, dir: SortDir) -> Vec<usize> {
    let keys = column_keys(table, column, lmdv_i18n::locale_id());
    let mut order: Vec<usize> = (0..table.rows.len()).collect();
    order.sort_by(|&a, &b| compare_for(&keys[a], &keys[b], dir));
    order
}

/// The directed comparison: only values reverse. Empty cells stay last and
/// numbers stay before words in both directions (V-15).
fn compare_for(a: &SortKey, b: &SortKey, dir: SortDir) -> Ordering {
    match (a, b) {
        (SortKey::Empty, _)
        | (_, SortKey::Empty)
        | (SortKey::Number(_), SortKey::Text(_))
        | (SortKey::Text(_), SortKey::Number(_)) => compare_keys(a, b),
        _ => dir.apply(compare_keys(a, b)),
    }
}

/// Whether a column reads as numbers or dates, then builds one key per row.
fn column_keys(table: &TableRef, column: usize, lang: &str) -> Vec<SortKey> {
    let raw: Vec<&str> = table.rows.iter().map(|row| row.get(column).map_or("", |cell| cell.text.as_str())).collect();
    let filled: Vec<&str> = raw.iter().copied().filter(|cell| !cell.trim().is_empty()).collect();
    let need = ((filled.len() as f64) * 0.8).ceil().max(1.0) as usize;
    let decimal = decimal_mark(&filled, lang);
    let numbers: Vec<Option<f64>> =
        raw.iter().map(|cell| if cell.trim().is_empty() { None } else { parse_number(cell, decimal) }).collect();
    if numbers.iter().filter(|number| number.is_some()).count() >= need {
        return raw
            .iter()
            .zip(numbers)
            .map(|(cell, number)| match number {
                _ if cell.trim().is_empty() => SortKey::Empty,
                Some(value) => SortKey::Number(value),
                None => SortKey::Text((*cell).to_owned()),
            })
            .collect();
    }
    let order = date_order(&filled, lang);
    let dates: Vec<Option<i64>> =
        raw.iter().map(|cell| if cell.trim().is_empty() { None } else { parse_date(cell, order) }).collect();
    if dates.iter().filter(|date| date.is_some()).count() >= need {
        return raw
            .iter()
            .zip(dates)
            .map(|(cell, date)| match date {
                _ if cell.trim().is_empty() => SortKey::Empty,
                Some(days) => SortKey::Number(days as f64),
                None => SortKey::Text((*cell).to_owned()),
            })
            .collect();
    }
    raw.iter()
        .map(|cell| if cell.trim().is_empty() { SortKey::Empty } else { SortKey::Text((*cell).to_owned()) })
        .collect()
}

/// Empty cells last either way, numbers before words, numbers by value and
/// text naturally and accent-insensitively.
fn compare_keys(a: &SortKey, b: &SortKey) -> Ordering {
    match (a, b) {
        (SortKey::Empty, SortKey::Empty) => Ordering::Equal,
        (SortKey::Empty, _) => Ordering::Greater,
        (_, SortKey::Empty) => Ordering::Less,
        (SortKey::Number(x), SortKey::Number(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (SortKey::Number(_), SortKey::Text(_)) => Ordering::Less,
        (SortKey::Text(_), SortKey::Number(_)) => Ordering::Greater,
        (SortKey::Text(x), SortKey::Text(y)) => natural_compare(x, y),
    }
}

/// A case- and accent-folded comparison that reads digit runs numerically.
fn natural_compare(a: &str, b: &str) -> Ordering {
    let a: Vec<char> = fold(a).chars().collect();
    let b: Vec<char> = fold(b).chars().collect();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (x, next_a) = number_run(&a, i);
            let (y, next_b) = number_run(&b, j);
            if x != y {
                return x.cmp(&y);
            }
            i = next_a;
            j = next_b;
        } else {
            if a[i] != b[j] {
                return a[i].cmp(&b[j]);
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn number_run(chars: &[char], start: usize) -> (u128, usize) {
    let mut value = 0u128;
    let mut at = start;
    while let Some(digit) = chars.get(at).and_then(|character| character.to_digit(10)) {
        value = value.saturating_mul(10).saturating_add(u128::from(digit));
        at += 1;
    }
    (value, at)
}

/// The column's decimal mark, voted by the cells that say it without doubt
/// (V-15); the language decides a tie.
fn decimal_mark(cells: &[&str], lang: &str) -> char {
    let mut comma = 0usize;
    let mut point = 0usize;
    for cell in cells {
        let bare = bare_number(cell);
        if !bare.chars().all(|character| character.is_ascii_digit() || matches!(character, '+' | '-' | '.' | ','))
            || !bare.chars().any(|character| character.is_ascii_digit())
        {
            continue;
        }
        let last_comma = bare.rfind(',');
        let last_point = bare.rfind('.');
        match (last_comma, last_point) {
            (Some(c), Some(p)) => {
                if c > p {
                    comma += 1;
                } else {
                    point += 1;
                }
            }
            (Some(_), None) => {
                if !grouped_number(&bare, ',') {
                    comma += 1;
                }
            }
            (None, Some(_)) => {
                if !grouped_number(&bare, '.') {
                    point += 1;
                }
            }
            (None, None) => {}
        }
    }
    if comma != point {
        if comma > point { ',' } else { '.' }
    } else if lang == "en" || lang == "zh-CN" {
        '.'
    } else {
        ','
    }
}

/// `1,234` (or `1.234`) with groups of exactly three: a thousands separator.
fn grouped_number(text: &str, separator: char) -> bool {
    let rest = text.trim_start_matches(['+', '-']);
    let groups: Vec<&str> = rest.split(separator).collect();
    if groups.len() < 2 {
        return false;
    }
    let first = groups[0];
    if !(1..=3).contains(&first.len()) || !first.chars().all(|character| character.is_ascii_digit()) {
        return false;
    }
    groups[1..].iter().all(|group| group.len() == 3 && group.chars().all(|character| character.is_ascii_digit()))
}

/// A number cell with its currency, percent sign, unit and spaces removed.
fn bare_number(text: &str) -> String {
    let mut out: String = text.chars().filter(|character| !character.is_whitespace()).collect();
    for currency in ['€', '$', '£', '¥'] {
        if out.starts_with(currency) {
            out.remove(0);
            break;
        }
        if out.ends_with(currency) {
            out.pop();
            break;
        }
    }
    if out.ends_with('%') {
        out.pop();
    }
    let trimmed = out.trim_end_matches(|character: char| character.is_ascii_alphabetic()).len();
    if trimmed < out.len()
        && out.len() - trimmed <= 3
        && out[..trimmed].chars().last().is_some_and(|c| c.is_ascii_digit())
    {
        out.truncate(trimmed);
    }
    if let Some(rest) = out.strip_prefix(['−', '–']) {
        out = format!("-{rest}");
    }
    out
}

fn parse_number(text: &str, decimal: char) -> Option<f64> {
    let bare = bare_number(text);
    let (negative, rest) = match bare.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, bare.strip_prefix('+').unwrap_or(&bare)),
    };
    let thousands = if decimal == ',' { '.' } else { ',' };
    let (integer, fraction) = match rest.split_once(decimal) {
        Some((integer, fraction)) => {
            if fraction.contains(decimal) || fraction.contains(thousands) || fraction.is_empty() {
                return None;
            }
            (integer, Some(fraction))
        }
        None => (rest, None),
    };
    if integer.is_empty() {
        return None;
    }
    let digits = if integer.contains(thousands) {
        if !grouped_number(integer, thousands) {
            return None;
        }
        integer.replace(thousands, "")
    } else {
        if !integer.chars().all(|character| character.is_ascii_digit()) {
            return None;
        }
        integer.to_owned()
    };
    let mut literal = String::with_capacity(digits.len() + 8);
    if negative {
        literal.push('-');
    }
    literal.push_str(&digits);
    if let Some(fraction) = fraction {
        if !fraction.chars().all(|character| character.is_ascii_digit()) {
            return None;
        }
        literal.push('.');
        literal.push_str(fraction);
    }
    literal.parse::<f64>().ok().filter(|value| value.is_finite())
}

/// `dmy` unless the cells prove `mdy` or the interface is English (V-15).
fn date_order(cells: &[&str], lang: &str) -> bool {
    let mut dmy = 0usize;
    let mut mdy = 0usize;
    for cell in cells {
        let Some((first, second)) = date_parts(cell) else { continue };
        if first > 12 {
            dmy += 1;
        } else if second > 12 {
            mdy += 1;
        }
    }
    if dmy != mdy { dmy > mdy } else { !lang.starts_with("en") }
}

/// The first two numbers of a `d/m/y` (or `m/d/y`) cell.
fn date_parts(text: &str) -> Option<(u32, u32)> {
    let text = text.trim();
    let mut parts = [0u32; 3];
    let mut count = 0usize;
    let mut digits = String::new();
    for character in text.chars() {
        match character {
            '0'..='9' => digits.push(character),
            '/' | '.' | '-' => {
                if count >= 2 || digits.is_empty() {
                    return None;
                }
                parts[count] = digits.parse().ok()?;
                count += 1;
                digits.clear();
            }
            ' ' => break,
            _ => return None,
        }
    }
    if count != 2 || digits.is_empty() {
        return None;
    }
    parts[2] = digits.parse().ok()?;
    Some((parts[0], parts[1]))
}

/// A date as days since 1970-01-01: ISO first, then `d/m/y` in `order` (V-15).
fn parse_date(text: &str, dmy: bool) -> Option<i64> {
    let text = text.trim();
    if let Some((year, month, day)) = iso_date(text) {
        return civil_days(year, month, day);
    }
    let (first, second) = date_parts(text)?;
    let year = text.rsplit(['/', '.', '-']).next()?.parse::<i64>().ok()?;
    let year = if year < 100 { if year < 70 { 2000 + year } else { 1900 + year } } else { year };
    let (day, month) = if dmy { (first, second) } else { (second, first) };
    civil_days(year, month, day)
}

fn iso_date(text: &str) -> Option<(i64, u32, u32)> {
    let (date, time) = text.split_once(['T', ' ']).map_or((text, None), |(date, time)| (date, Some(time)));
    if let Some(time) = time {
        let mut parts = time.split(':');
        let hours = parts.next()?.parse::<u32>().ok()?;
        let minutes = parts.next()?.parse::<u32>().ok()?;
        if hours > 23 || minutes > 59 {
            return None;
        }
        if let Some(seconds) = parts.next()
            && (seconds.len() != 2 || seconds.parse::<u32>().ok()? > 59)
        {
            return None;
        }
        if parts.next().is_some() {
            return None;
        }
    }
    let (year, rest) = date.split_once('-')?;
    let (month, day) = rest.split_once('-')?;
    if year.len() != 4 {
        return None;
    }
    Some((year.parse().ok()?, month.parse().ok()?, day.parse().ok()?))
}

/// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant).
fn civil_days(year: i64, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = i64::from(month);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}
