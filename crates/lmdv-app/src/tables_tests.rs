//! Table action bar, preview, filter/sort/freeze and TSV copy tests (P6-T,
//! M-06, X-16, V-11–V-18).

use lmdv_core::DocumentId;
use lmdv_render::{TableCell, TableRef};
use rune::{ElementInfo, Tx};
use rune_kit::SortDir;
use tempfile::TempDir;

use crate::tables::{
    SortKey, TablePreview, TableTarget, copy_tsv, fold, highlight_ranges, natural_widths, plan_widths, sort_order,
};
use crate::test_support;
use crate::window::WindowState;

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(test_support::shared(temp))
}

/// A drawn table with plain cells, for the pure tests.
fn drawn_table(head: &[&str], rows: &[&[&str]]) -> TableRef {
    let columns = head.len().max(rows.iter().map(|row| row.len()).max().unwrap_or(0));
    let pad = |cells: &[&str]| {
        cells
            .iter()
            .map(|cell| TableCell::plain(*cell))
            .chain(std::iter::repeat_with(TableCell::default).take(columns))
            .take(columns)
            .collect::<Vec<_>>()
    };
    TableRef {
        block: "b1".to_owned(),
        index: 1,
        source: 0..1,
        line: 0,
        end_line: rows.len() + 2,
        align: vec![lmdv_md::Align::None; columns],
        head: pad(head),
        rows: rows.iter().map(|row| pad(row)).collect(),
        columns,
    }
}

fn target(table: TableRef) -> TableTarget {
    TableTarget { tab: 1u32, doc: DocumentId(1), revision: 0, table }
}

fn preview_of(table: TableRef, width: f32) -> TablePreview {
    TablePreview::new(target(table), width)
}

/// Lays the state out headlessly and returns every element it drew.
fn render(state: WindowState) -> Vec<ElementInfo> {
    let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.elements()
}

fn keys(elements: &[ElementInfo]) -> Vec<&str> {
    elements.iter().filter_map(|element| element.key.as_deref()).collect()
}

// ---- the action bar -------------------------------------------------------

#[test]
fn every_table_gets_a_copy_and_preview_bar_and_none_without_a_table() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Notes", "# Notes\n\n| A | B |\n| - | - |\n| 1 | 2 |\n");
    let model = state.active_tab().and_then(|tab| tab.view.reader.clone()).expect("the model");
    let table = lmdv_render::table_refs(&model).into_iter().next().expect("one table");

    let elements = render(state);
    let drawn = keys(&elements);
    let copy = format!("table-copy-{}", table.block);
    let open = format!("table-preview-{}", table.block);
    assert!(drawn.iter().any(|key| *key == copy), "the bar's copy button: {drawn:?}");
    assert!(drawn.iter().any(|key| *key == open), "the bar's preview button");
    let copy = elements.iter().find(|element| element.key.as_deref() == Some(copy.as_str())).expect("copy");
    assert!(copy.focusable, "the bar is reachable by keyboard, not only hover");
    assert_eq!(copy.label.as_deref(), Some("Copy table (for Excel)"));
    let open = elements.iter().find(|element| element.key.as_deref() == Some(open.as_str())).expect("preview");
    assert_eq!(open.label.as_deref(), Some("Open preview"));

    let mut start = window(&temp);
    for id in ["table.copy", "table.preview", "table.filter-reset", "table.sort-reset"] {
        let command = crate::commands::lookup_command(id).expect("command");
        assert!(!(command.enabled)(&start), "{id} is disabled with no table");
    }
    crate::view::scene_start(&mut start);
    let elements = render(start);
    let drawn = keys(&elements);
    assert!(!drawn.iter().any(|key| key.starts_with("table-copy-")), "a page with no table draws no bar");
}

// ---- TSV copy (X-16, V-16) ------------------------------------------------

#[test]
fn the_bar_copies_the_whole_table_as_tsv() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Notes", "# Notes\n\n| Name | Qty |\n| - | - |\n| Alpha | 2 |\n| Beta | 10 |\n");
    let model = state.active_tab().and_then(|tab| tab.view.reader.clone()).expect("the model");
    let table = lmdv_render::table_refs(&model).into_iter().next().expect("one table");
    state.select_table(table.clone());

    let board = rune::clipboard::memory();
    let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(true, 2.0));
    runtime.use_clipboard(&board);
    runtime.update(|state: &mut WindowState, tx: &mut Tx| {
        assert!(state.run_command("table.copy", tx));
    });
    runtime.frame(0.0);

    let copied = board.content().expect("the clipboard");
    assert_eq!(copied.text(), "Name\tQty\nAlpha\t2\nBeta\t10");
    let notice = runtime.state.ui.notice.as_ref().expect("a localized notice");
    assert_eq!(notice.text, "Table copied.");
    let flash = runtime.state.tables.flash.borrow().clone();
    assert_eq!(flash.map(|(block, _)| block).as_deref(), Some(table.block.as_str()), "the bar shows its check");
}

#[test]
fn a_preview_copies_only_its_shown_rows_in_their_shown_order() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::table_ui::scene_table_preview(&mut state);

    // A filter alone: the hidden rows travel as the visibility mask and the
    // header stays (V-16).
    state.reset_table_sort();
    state.set_table_filter("alpha beta");
    let copied = state.table_preview().expect("preview").copy_tsv();
    let lines: Vec<&str> = copied.lines().collect();
    assert_eq!(lines.len(), 3, "header plus the two matching rows: {copied}");
    assert!(lines[0].starts_with("Name\tOwner\tAmount\tUpdated\tNotes\tReference\tSummary\tStatus"));
    assert!(lines[1].starts_with("Alpha one\tAna\t"), "{copied}");
    assert!(lines[2].starts_with("Alpha beta three\tCid\t"), "{copied}");
    assert!(!copied.contains("Beta two"), "a hidden row never reaches the clipboard");

    // A sort reorders the same visible rows; the header still leads.
    state.cycle_table_sort(0);
    let copied = state.table_preview().expect("preview").copy_tsv();
    let lines: Vec<&str> = copied.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[1].starts_with("Alpha beta three\t"), "ascending order: {copied}");
    assert!(lines[2].starts_with("Alpha one\t"), "{copied}");

    // Copying from the preview writes the clipboard through the same command.
    let board = rune::clipboard::memory();
    let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(true, 2.0));
    runtime.use_clipboard(&board);
    runtime.update(|state: &mut WindowState, tx: &mut Tx| {
        assert!(state.run_command("table.copy", tx));
    });
    runtime.frame(0.0);
    assert_eq!(board.content().expect("clipboard").text(), copied);
}

#[test]
fn an_empty_table_has_nothing_to_copy() {
    let table = drawn_table(&["Name"], &[]);
    assert_eq!(copy_tsv(&table, None, &[true]), "Name");
    let empty = drawn_table(&[], &[]);
    assert_eq!(copy_tsv(&empty, None, &[]), "");
    assert_eq!(copy_tsv(&empty, None, &[]), "");
}

// ---- filter (V-13) --------------------------------------------------------

#[test]
fn the_filter_keeps_rows_with_every_word_accent_and_case_folded() {
    let table = drawn_table(
        &["Name"],
        &[&["Café Alpha"], &["BETA alpha"], &["alpha only"], &["beta only"], &["ÁLPHA BÉTA extra"]],
    );
    let mut preview = preview_of(table, 1000.0);
    preview.query.replace_text("ALPHA beta".to_owned());
    preview.apply_filter();
    let shown: Vec<usize> = preview.shown().to_vec();
    assert_eq!(shown, [1, 4], "both words, case and accents folded");
    assert_eq!(preview.count_text(), "2 of 5 rows");
    assert_eq!(preview.empty_text(), None);

    preview.clear_filter();
    assert_eq!(preview.shown().len(), 5);
    assert!(preview.visible.iter().all(|visible| *visible));

    preview.query.replace_text("nothing".to_owned());
    preview.apply_filter();
    assert!(preview.shown().is_empty());
    assert_eq!(preview.empty_text().as_deref(), Some("No row contains “nothing”"));
    assert_eq!(preview.count_text(), "0 of 5 rows");
}

#[test]
fn filter_matches_map_back_to_the_accented_characters() {
    let words = vec!["alpha".chars().collect::<Vec<_>>(), "beta".chars().collect()];
    let text = "ÁLPHA and BÉTA";
    let ranges = highlight_ranges(text, &words);
    assert_eq!(ranges.len(), 2);
    assert_eq!(&text[ranges[0].clone()], "ÁLPHA");
    assert_eq!(&text[ranges[1].clone()], "BÉTA");
    assert_eq!(fold("Café ÁLPHA"), "cafe alpha");
}

#[test]
fn the_filter_applies_after_the_debounce() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::table_ui::scene_table_preview(&mut state);
    state.clear_table_filter();
    let preview = state.tables.preview.as_mut().expect("preview");
    preview.query.replace_text("gamma".to_owned());
    preview.due = Some(0.9);
    let mut tx = Tx::default();

    assert!(!state.poll_tables(0.5, &mut tx), "the debounce has not elapsed");
    assert_eq!(state.table_preview().expect("preview").shown().len(), 8);
    assert!(state.poll_tables(0.9, &mut tx), "the filter applies");
    let preview = state.table_preview().expect("preview");
    assert_eq!(preview.applied, "gamma");
    assert_eq!(preview.shown().len(), 2, "the word reaches a note as well as a name");
    let names: Vec<&str> = preview.shown().iter().map(|&row| preview.cell(false, row, 0)).collect();
    assert_eq!(names, ["Beta two", "Gamma four"]);
    assert_eq!(preview.due, None);
}

#[test]
fn widths_stay_locked_while_the_filter_changes() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::table_ui::scene_table_preview(&mut state);
    let before = state.table_preview().expect("preview").widths.clone();
    state.set_table_filter("gamma");
    let after = state.table_preview().expect("preview").widths.clone();
    assert_eq!(before, after, "columns never jump with the filter");
    assert!(state.table_preview().is_some_and(|preview| preview.overflow), "the scene's wide table opens at 100%");
    assert!(state.table_preview().is_some_and(|preview| preview.frozen), "and holds its short first column");
}

// ---- sort (V-15) ----------------------------------------------------------

#[test]
fn a_header_click_cycles_ascending_descending_and_original() {
    let table = drawn_table(&["Qty"], &[&["10"], &["2"], &["100"]]);
    let mut preview = preview_of(table, 1000.0);
    let order = |preview: &TablePreview| preview.shown().to_vec();
    preview.cycle_sort(0);
    assert_eq!(preview.sort.map(|(_, dir)| dir), Some(SortDir::Ascending));
    assert_eq!(order(&preview), [1, 0, 2], "numbers sort by value: 2, 10, 100");
    preview.cycle_sort(0);
    assert_eq!(preview.sort.map(|(_, dir)| dir), Some(SortDir::Descending));
    assert_eq!(order(&preview), [2, 0, 1]);
    preview.cycle_sort(0);
    assert_eq!(preview.sort, None);
    assert_eq!(order(&preview), [0, 1, 2], "the third click restores the document's order");
}

#[test]
fn numeric_columns_strip_currency_units_and_vote_on_the_decimal_mark() {
    let table = drawn_table(&["Price"], &[&["€1.234,50"], &["12,50"], &["−3,00"], &[""]]);
    let order = sort_order(&table, 0, SortDir::Ascending);
    assert_eq!(order, [2, 1, 0, 3], "negative, small, thousands, and the empty cell last");
    let desc = sort_order(&table, 0, SortDir::Descending);
    assert_eq!(desc, [0, 1, 2, 3], "an empty cell stays last either way");

    let english = drawn_table(&["Size"], &[&["1,200"], &["300"], &["2%"]]);
    assert_eq!(sort_order(&english, 0, SortDir::Ascending), [2, 1, 0], "2%, 300, then the grouped 1,200");
}

#[test]
fn date_columns_sort_chronologically_and_infer_the_order() {
    let iso = drawn_table(&["Date"], &[&["2026-03-02"], &["2025-12-31"], &["2026-01-04"]]);
    assert_eq!(sort_order(&iso, 0, SortDir::Ascending), [1, 2, 0]);

    let dmy = drawn_table(&["Date"], &[&["13/01/2026"], &["02/02/2026"], &["28/12/2025"]]);
    assert_eq!(sort_order(&dmy, 0, SortDir::Ascending), [2, 0, 1], "13 in the first place means d/m/y");

    let mdy = drawn_table(&["Date"], &[&["01/13/2026"], &["02/02/2026"]]);
    assert_eq!(sort_order(&mdy, 0, SortDir::Ascending), [0, 1], "13 in the second place means m/d/y");
}

#[test]
fn text_sorts_naturally_and_numbers_come_before_words() {
    let names = drawn_table(&["File"], &[&["item 10"], &["item 2"], &["item 1"]]);
    assert_eq!(sort_order(&names, 0, SortDir::Ascending), [2, 1, 0]);

    // A column of 4 filled cells: 3 numbers reach the 80% threshold, so the
    // odd word becomes text and sorts after the numbers.
    let mixed = drawn_table(&["Value"], &[&["9"], &["10"], &["11"], &["n/a"], &[""]]);
    assert_eq!(sort_order(&mixed, 0, SortDir::Ascending), [0, 1, 2, 3, 4]);
    assert!(!matches!(SortKey::Number(1.0), SortKey::Text(_)));
}

#[test]
fn a_stable_sort_keeps_equal_rows_in_document_order() {
    let table = drawn_table(&["Group"], &[&["b"], &["a"], &["a"], &["b"]]);
    assert_eq!(sort_order(&table, 0, SortDir::Ascending), [1, 2, 0, 3]);
}

#[test]
fn a_single_row_or_headless_table_does_not_sort() {
    let one = preview_of(drawn_table(&["A"], &[&["x"]]), 1000.0);
    assert!(!one.sortable(0), "one body row cannot be reordered");
    assert!(!one.sortable(1), "there is no second column");
    let empty = preview_of(drawn_table(&["A"], &[]), 1000.0);
    assert!(!empty.sortable(0), "a header alone has no rows to sort");
}

// ---- widths and freeze (V-11, V-18) --------------------------------------

#[test]
fn widths_fit_the_view_and_cap_the_growth() {
    assert_eq!(plan_widths(&[100.0, 100.0, 100.0], &[56.0; 3], 1000.0), [150.0, 150.0, 150.0], "never above 1.5×");
    assert_eq!(plan_widths(&[400.0, 400.0, 400.0], &[56.0; 3], 600.0), [200.0, 200.0, 200.0], "shrunk to fit");
    assert_eq!(
        plan_widths(&[400.0, 400.0], &[300.0, 300.0], 100.0),
        [400.0, 400.0],
        "below the minimum: 100% and scroll"
    );
}

#[test]
fn a_wide_preview_freezes_its_first_column_only_when_it_is_narrow() {
    // A 300-point window leaves 206 points: four minimum-width columns no
    // longer fit, so the preview opens at 100% and scrolls (V-18).
    let wide = drawn_table(
        &["A", "Notes", "Summary", "More"],
        &[&["x", &"n".repeat(200), &"s".repeat(200), &"m".repeat(200)]],
    );
    let preview = preview_of(wide, 300.0);
    assert!(preview.overflow, "the minimum widths no longer fit the view");
    assert!(preview.frozen, "a short first column holds while the rest scrolls");
    assert!(natural_widths(&preview.table).iter().all(|width| *width <= 360.0));

    let long_first = drawn_table(
        &["A very long first column name", "B", "C", "D"],
        &[&[&"z".repeat(200), &"b".repeat(200), &"c".repeat(200), &"d".repeat(200)]],
    );
    let preview = preview_of(long_first, 300.0);
    assert!(preview.overflow);
    assert!(!preview.frozen, "a first column over 35% of the view stays in the scroller");
}

// ---- opening and closing (V-01, V-14) ------------------------------------

#[test]
fn a_table_with_no_rows_does_not_open_a_preview() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Head only", "# Notes\n\nA page with a header-only table.\n");
    let table = drawn_table(&["A", "B"], &[]);
    assert!(table.rows.is_empty());
    state.select_table(table);

    let mut tx = Tx::default();
    assert!(!(crate::commands::lookup_command("table.preview").expect("command").enabled)(&state));
    state.open_table_preview(&mut tx);
    assert!(state.table_preview().is_none());
    assert!(state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("no rows")));

    let mut none = window(&temp);
    none.open_table_preview(&mut tx);
    assert!(none.table_preview().is_none());
    assert!(none.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("action bar")));
}

#[test]
fn escape_clears_the_filter_before_it_closes_the_preview() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::table_ui::scene_table_preview(&mut state);
    let mut tx = Tx::default();

    state.table_preview_escape(&mut tx);
    let preview = state.table_preview().expect("the preview stays open");
    assert_eq!(preview.query.text, "");
    assert_eq!(preview.applied, "");
    assert_eq!(preview.shown().len(), 8);
    assert!(state.table_preview().is_some());

    state.table_preview_escape(&mut tx);
    assert!(state.table_preview().is_none(), "the second Escape closes");
}

#[test]
fn switching_tabs_or_closing_the_tab_closes_the_preview() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::table_ui::scene_table_preview(&mut state);
    let first = state.active_tab().map(|tab| tab.id).expect("tab");
    let second = state.open_synthetic("Other", "# Other\n");
    assert!(state.activate_tab(second));
    assert!(state.table_preview().is_none(), "a tab switch closes the preview");
    assert!(state.table_target().is_none(), "and forgets its table");

    assert!(state.activate_tab(first));
    assert!(state.table_preview().is_none(), "the preview does not come back with the tab");
    assert!(state.close_tab(first) == crate::window::CloseOutcome::Closed);
    assert!(state.table_preview().is_none());
}

#[test]
fn the_preview_commands_reset_filter_and_sort() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::table_ui::scene_table_preview(&mut state);
    let mut tx = Tx::default();
    let filter = crate::commands::lookup_command("table.filter-reset").expect("command");
    let sort = crate::commands::lookup_command("table.sort-reset").expect("command");
    assert!((filter.enabled)(&state));
    assert!((sort.enabled)(&state));

    assert!(state.run_command("table.sort-reset", &mut tx));
    let preview = state.table_preview().expect("preview");
    assert_eq!(preview.sort, None);
    assert_eq!(preview.shown().to_vec(), [0, 2], "the filter still hides the other rows");
    assert!(state.run_command("table.filter-reset", &mut tx));
    let preview = state.table_preview().expect("preview");
    assert_eq!(preview.shown().len(), 8);
    assert!(!(filter.enabled)(&state));
    assert!(!(sort.enabled)(&state));
}

// ---- the preview tree -----------------------------------------------------

#[test]
fn the_scene_draws_the_preview_grid_its_count_and_its_sort() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::table_ui::scene_table_preview(&mut state);
    assert_eq!(crate::SCENES.iter().filter(|(name, _)| *name == "table-preview").count(), 1);

    let elements = render(state);
    let drawn = keys(&elements);
    for key in [
        "table-preview",
        "table-preview-panel",
        "table-preview-query",
        "table-preview-count",
        "table-preview-scroll",
        "table-preview-grid",
        "table-preview-head-0",
        "table-preview-copy",
        "table-preview-close",
    ] {
        assert!(drawn.contains(&key), "missing {key}: {drawn:?}");
    }
    let panel = elements.iter().find(|element| element.key.as_deref() == Some("table-preview-panel")).expect("panel");
    assert_eq!(panel.role, "dialog");
    assert!(panel.label.as_deref().is_some_and(|label| label.starts_with("Table: Name")));
    let count = elements.iter().find(|element| element.key.as_deref() == Some("table-preview-count")).expect("count");
    assert_eq!(count.text.as_deref(), Some("2 of 8 rows"));
    let grid = elements.iter().find(|element| element.key.as_deref() == Some("table-preview-grid")).expect("grid");
    assert_eq!(grid.role, "grid");
    let headers: Vec<&str> = elements
        .iter()
        .filter(|element| element.role == "columnheader")
        .filter_map(|element| element.label.as_deref())
        .collect();
    assert_eq!(headers, ["Name", "Owner", "Amount", "Updated", "Notes", "Reference", "Summary", "Status"]);
    let name = elements.iter().find(|element| element.key.as_deref() == Some("table-preview-head-0")).expect("Name");
    assert!(name.focusable, "a sortable header takes the keyboard");
    let rows = elements
        .iter()
        .filter(|element| {
            element.role == "row"
                && element.key.as_deref().is_some_and(|key| {
                    key.strip_prefix('r')
                        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
                })
        })
        .count();
    assert_eq!(rows, 2, "only the two matching rows are mounted");
}

#[test]
fn clicking_outside_the_panel_closes_the_preview() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::table_ui::scene_table_preview(&mut state);
    let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(true, 2.0));
    runtime.frame(0.0);
    assert!(runtime.state.table_preview().is_some());

    runtime.pointer_move(5.0, 5.0);
    runtime.pointer_down(5.0, 5.0);
    runtime.pointer_up(5.0, 5.0);
    runtime.frame(0.0);
    assert!(runtime.state.table_preview().is_none(), "the backdrop closes the preview");
}

#[test]
fn a_filter_with_no_matches_shows_the_message_and_the_count() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::table_ui::scene_table_preview(&mut state);
    state.set_table_filter("zzz");
    let elements = render(state);
    let empty = elements.iter().find(|element| element.key.as_deref() == Some("table-preview-empty")).expect("message");
    assert_eq!(empty.text.as_deref(), Some("No row contains “zzz”"));
    let count = elements.iter().find(|element| element.key.as_deref() == Some("table-preview-count")).expect("count");
    assert_eq!(count.text.as_deref(), Some("0 of 8 rows"));
}

#[test]
fn the_preview_is_modal_and_swallows_app_shortcuts() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::table_ui::scene_table_preview(&mut state);
    let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.key(rune::KeyEvent { key: rune::Key::Function(1), mods: rune::Mods::default() });
    runtime.frame(0.0);
    assert!(runtime.state.help_view().is_none(), "F1 must not open Help behind the preview");
    assert!(runtime.state.table_preview().is_some(), "and must not close the preview");
}

#[test]
fn the_first_column_sticks_when_the_preview_scrolls_sideways() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Wide", "# Wide\n\nA page with a wide table.\n");
    let columns = 12usize;
    let head: Vec<String> = (0..columns).map(|column| format!("C{column}")).collect();
    let mut cells: Vec<String> = (0..columns).map(|column| format!("value {column} that is long")).collect();
    cells[0] = "v0".to_owned();
    let table = TableRef {
        block: "b1".to_owned(),
        index: 1,
        source: 0..1,
        line: 0,
        end_line: 3,
        align: vec![lmdv_md::Align::None; columns],
        head: head.iter().map(|cell| TableCell::plain(cell.as_str())).collect(),
        rows: vec![
            cells.iter().map(|cell| TableCell::plain(cell.as_str())).collect(),
            cells.iter().map(|cell| TableCell::plain(cell.as_str())).collect(),
        ],
        columns,
    };
    state.tables.viewport.set([500.0, 760.0]);
    state.select_table(table);
    let mut tx = Tx::default();
    state.open_table_preview(&mut tx);
    assert!(state.table_preview().is_some_and(|preview| preview.overflow && preview.frozen));

    let mut runtime = rune::Runtime::new(state, crate::view::view, (500.0, 760.0), rune::Env::new(true, 1.0));
    runtime.frame(0.0);
    runtime.update(|_: &mut WindowState, tx: &mut Tx| {
        tx.scroll_into_view("table-preview-head-11");
    });
    for _ in 0..4 {
        runtime.frame(0.0);
    }
    let elements = runtime.elements();
    let first = elements
        .iter()
        .find(|element| element.key.as_deref() == Some("table-preview-head-0"))
        .expect("the first header cell");
    assert!(first.stuck > 0.5, "the first column holds at the left edge: stuck {}", first.stuck);
    let last = elements
        .iter()
        .find(|element| element.key.as_deref() == Some("table-preview-head-11"))
        .expect("the last header cell");
    assert!(last.stuck.abs() < 0.5, "the scrolled column moves");
}

#[test]
fn the_header_sticks_to_the_top_while_the_rows_scroll() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Long", "# Long\n\nA page with a long table.\n");
    let rows: Vec<Vec<String>> = (0..40).map(|row| vec![format!("row {row}"), format!("value {row}")]).collect();
    let table = TableRef {
        block: "b1".to_owned(),
        index: 1,
        source: 0..1,
        line: 0,
        end_line: 42,
        align: vec![lmdv_md::Align::None; 2],
        head: vec![TableCell::plain("Name"), TableCell::plain("Value")],
        rows: rows.iter().map(|row| row.iter().map(|cell| TableCell::plain(cell.as_str())).collect()).collect(),
        columns: 2,
    };
    state.select_table(table);
    let mut tx = Tx::default();
    state.open_table_preview(&mut tx);

    let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(true, 1.0));
    runtime.frame(0.0);
    runtime.update(|_: &mut WindowState, tx: &mut Tx| tx.scroll_to_y("table-preview-scroll", 300.0));
    for _ in 0..4 {
        runtime.frame(0.0);
    }
    let elements = runtime.elements();
    let header =
        elements.iter().find(|element| element.key.as_deref() == Some("table-preview-head")).expect("the header row");
    assert!(header.stuck > 0.5, "the header holds at the viewport top: stuck {}", header.stuck);
    let first = elements
        .iter()
        .find(|element| element.key.as_deref() == Some("table-preview-head-0"))
        .expect("the first column header");
    assert!(first.rect[1] >= header.rect[1] - 0.5, "the header and its cells stay together");
}

// ---- zoom (V-12) ----------------------------------------------------------

#[test]
fn the_preview_zooms_in_steps_and_by_ctrl_wheel() {
    use crate::tables::{MAX_ZOOM, MIN_ZOOM, ROW_HEIGHT, ZOOM_STEP};

    let table = drawn_table(&["Name"], &[&["alpha"], &["beta"]]);
    let mut preview = preview_of(table, 1000.0);
    let planned = preview.widths.clone();
    let table_width = preview.table_width;

    assert!(preview.zoom_in());
    assert!((preview.zoom - ZOOM_STEP).abs() < 0.001);
    assert!((preview.drawn_width() - table_width * ZOOM_STEP).abs() < 0.01, "the grid scales with the text");
    assert!((preview.row_height() - ROW_HEIGHT * ZOOM_STEP).abs() < 0.01, "rows grow with the text");
    assert!(preview.column_width(0) > planned[0], "columns grow with the text");

    for _ in 0..10 {
        preview.zoom_in();
    }
    assert!((preview.zoom - MAX_ZOOM).abs() < 0.001, "never past 200%");
    for _ in 0..10 {
        preview.zoom_out();
    }
    assert!((preview.zoom - MIN_ZOOM).abs() < 0.001, "never below 50%");

    // Ctrl+wheel is exponential: 240 points of travel doubles or halves.
    preview.set_zoom(MIN_ZOOM);
    assert!(preview.zoom_by_wheel(-240.0));
    assert!((preview.zoom - 1.0).abs() < 0.001, "one turn doubles: {}", preview.zoom);
    assert!(preview.zoom_by_wheel(240.0));
    assert!((preview.zoom - MIN_ZOOM).abs() < 0.001, "and back");

    assert_eq!(preview.widths, planned, "the planned 100% columns are kept; only drawing scales");
    assert!(!preview.zoom_by_wheel(0.0), "no travel, no change");
}

#[test]
fn the_preview_zoom_keys_and_buttons_work_through_the_window() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::table_ui::scene_table_preview(&mut state);
    assert!(state.table_preview_actual_size(), "the scene's zoom resets for the key test");
    assert!((state.table_preview().expect("preview").zoom - 1.0).abs() < 0.001);

    let key = |text: &str| rune::KeyEvent { key: rune::Key::Text(text.to_owned()), mods: rune::Mods::default() };
    let mut tx = Tx::default();
    assert!(state.table_preview_key(&key("+"), &mut tx));
    assert!((state.table_preview().expect("preview").zoom - 1.25).abs() < 0.001);
    assert!(state.table_preview_key(&key("-"), &mut tx));
    assert!((state.table_preview().expect("preview").zoom - 1.0).abs() < 0.001);
    assert!(state.table_preview_key(&key("1"), &mut tx), "actual size is a no-op at 100%");
    assert!(state.table_preview_zoom(1));
    assert!((state.table_preview().expect("preview").zoom - 1.25).abs() < 0.001);
    assert!(state.table_preview_actual_size());
    assert!((state.table_preview().expect("preview").zoom - 1.0).abs() < 0.001);

    // A plain wheel over the preview is Rune's scroll, not a zoom.
    let plain = rune::WheelEvent::default();
    assert!(!state.table_preview_wheel(&plain, &mut tx));
    let mut ctrl = rune::WheelEvent::default();
    ctrl.dy = -120.0;
    ctrl.mods.ctrl = true;
    assert!(state.table_preview_wheel(&ctrl, &mut tx), "Ctrl+wheel zooms");
    assert!(state.table_preview().expect("preview").zoom > 1.0);
}

#[test]
fn a_resize_refits_the_columns_without_reopening() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Fitting", "# Fitting\n\nA page with a table.\n");
    let table = drawn_table(
        &["A", "B", "C", "D"],
        &[
            &["alpha beta gamma", "one two three", "four five six", "seven eight nine"],
            &["beta gamma", "two three", "five six", "eight nine"],
        ],
    );
    state.select_table(table);
    let mut tx = Tx::default();
    state.open_table_preview(&mut tx);
    state.set_table_filter("alpha");
    state.cycle_table_sort(0);
    let before = state.table_preview().expect("preview").widths.clone();
    assert!(
        (state.table_preview().expect("preview").available - crate::tables::preview_available(1100.0)).abs() < 0.01
    );

    state.tables.viewport.set([700.0, 760.0]);
    assert!(state.poll_tables(0.0, &mut tx), "the resize replans the columns");
    let preview = state.table_preview().expect("the preview stays open");
    assert!((preview.available - crate::tables::preview_available(700.0)).abs() < 0.01);
    assert_ne!(preview.widths, before, "the columns fit the new width");
    assert_eq!(preview.query.text, "alpha", "the filter survives");
    assert!(preview.sort.is_some(), "the sort survives");

    // The same width again is not a change.
    assert!(!state.poll_tables(0.0, &mut tx));
}

// ---- links in the preview (V-17) -----------------------------------------

#[test]
fn a_link_in_the_preview_closes_it_and_follows_the_destination() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Links", "# Links\n\n## Target\n\n| A |\n| - |\n| [jump](#target) |\n");
    let model = state.active_tab().and_then(|tab| tab.view.reader.clone()).expect("the model");
    let table = lmdv_render::table_refs(&model).into_iter().next().expect("one table");
    state.select_table(table);
    let mut tx = Tx::default();
    state.open_table_preview(&mut tx);

    let cell = state.table_preview().expect("preview").cell_data(false, 0, 0).clone();
    assert_eq!(cell.text, "jump");
    assert_eq!(cell.links.len(), 1);
    assert_eq!(cell.links[0].dest, "#target");

    state.preview_table_link("#target", &mut tx);
    assert!(state.table_preview().is_none(), "the preview closes before the link acts");
    assert!(state.reading.landing.is_some(), "the fragment jump ran as an article link would");
}

// ---- the reading column (OI-138) -----------------------------------------

#[test]
fn a_table_wider_than_the_reading_column_fits_the_column() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let long = "word".repeat(40);
    let source = format!("| A | B | C | D |\n| - | - | - | - |\n| {long} | {long} | {long} | {long} |\n");
    state.open_synthetic("Wide", &source);
    let elements = render(state);
    let scroll = elements
        .iter()
        .find(|element| element.key.as_deref() == Some("scroll"))
        .expect("a table that cannot wrap scrolls inside the column");
    // R-02's 760 pt column keeps 48 pt a side: the scroller is the column's,
    // not the 1100 pt window's.
    assert!(scroll.rect[2] <= 664.5, "the scroller fits the reading column: {} wide", scroll.rect[2]);
    let table = elements.iter().find(|element| element.key.as_deref() == Some("table")).expect("the article's table");
    assert!(
        table.rect[2] > scroll.rect[2] + 0.5,
        "the table overflows that column and scrolls: {} in {}",
        table.rect[2],
        scroll.rect[2]
    );
    // Its longest word must not run over the next column.
    assert!(table.rect[2] >= 4.0 * 360.0 - 1.0, "unbreakable columns keep their width: {}", table.rect[2]);
}

#[test]
fn the_plus_and_minus_keys_zoom_the_open_preview() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::table_ui::scene_table_preview(&mut state);
    let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(true, 2.0));
    runtime.frame(0.0);
    let before = runtime.state.table_preview().expect("preview").zoom;
    runtime.key(rune::KeyEvent { key: rune::Key::Text("+".to_owned()), mods: rune::Mods::default() });
    runtime.frame(0.0);
    let zoomed = runtime.state.table_preview().expect("preview").zoom;
    assert!((zoomed - before * 1.25).abs() < 0.001, "the key reaches the preview: {before} -> {zoomed}");
    runtime.key(rune::KeyEvent { key: rune::Key::Text("-".to_owned()), mods: rune::Mods::default() });
    runtime.frame(0.0);
    assert!((runtime.state.table_preview().expect("preview").zoom - before).abs() < 0.001);
}

#[test]
fn clicking_a_link_in_a_preview_cell_follows_it_and_closes_the_preview() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Links", "# Links\n\n## Target\n\n| A | B |\n| - | - |\n| [jump](#target) | plain |\n");
    let model = state.active_tab().and_then(|tab| tab.view.reader.clone()).expect("the model");
    let table = lmdv_render::table_refs(&model).into_iter().next().expect("one table");
    state.select_table(table);
    let mut tx = Tx::default();
    state.open_table_preview(&mut tx);
    let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(true, 2.0));
    runtime.frame(0.0);
    let cell = runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some("table-preview-c0") && element.role == "gridcell")
        .expect("the link's cell");
    let (x, y) = (cell.rect[0] + 12.0, cell.rect[1] + cell.rect[3] * 0.5);
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(0.0);
    assert!(runtime.state.table_preview().is_none(), "the preview closes on its link");
    assert!(runtime.state.reading.landing.is_some(), "and the destination was followed");
}
