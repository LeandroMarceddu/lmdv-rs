//! Table action bar, preview, filter/sort/freeze and TSV copy tests (P6-T,
//! M-06, X-16, V-11–V-18).

use lmdv_core::DocumentId;
use lmdv_render::TableRef;
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
            .map(|cell| (*cell).to_owned())
            .chain(std::iter::repeat_n(String::new(), columns))
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
        head: head.clone(),
        rows: vec![cells.clone(), cells],
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
        head: vec!["Name".to_owned(), "Value".to_owned()],
        rows,
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
