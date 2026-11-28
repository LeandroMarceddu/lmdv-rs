//! Reading find: matching, prefill, the per-tab bar, `--line` paint and the
//! renderer reveal key (R-10, R-16–R-19, F-16).

use lmdv_md::match_ranges;
use lmdv_render::{HighlightKind, highlight_key};
use rune::{ElementInfo, Key, KeyEvent, Mods, Runtime, Tx};
use tempfile::TempDir;

use crate::reader_find::{
    FindBand, FindReveal, READER_FIND_BAND, ReaderFind, find_in_document, find_reveal, reader_find_prefill,
    refresh_tab, selection_in_article,
};
use crate::section::{SECTION_LINE_HEIGHT, SectionLineFrame, SectionSegment};
use crate::test_support;
use crate::view::{READER_MARKDOWN, scene_reader_find, view};
use crate::window::{Mode, Tab, WindowState};

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(test_support::shared(temp))
}

fn command(letter: char) -> KeyEvent {
    KeyEvent { key: Key::Command(letter), mods: Mods { cmd: true, ctrl: true, ..Mods::default() } }
}

fn plain(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods::default() }
}

fn render(state: WindowState) -> Vec<ElementInfo> {
    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.elements()
}

fn runtime(state: WindowState) -> Runtime<WindowState> {
    let mut runtime = Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
    runtime
}

fn settle(runtime: &mut Runtime<WindowState>, frames: usize) {
    for _ in 0..frames {
        runtime.frame(1.0 / 60.0);
    }
}

fn has_key(elements: &[ElementInfo], key: &str) -> bool {
    elements.iter().any(|element| element.key.as_deref() == Some(key))
}

fn text_at(elements: &[ElementInfo], key: &str) -> Option<String> {
    elements.iter().find(|element| element.key.as_deref() == Some(key)).and_then(|element| element.text.clone())
}

fn active_tab(state: &WindowState) -> &Tab {
    state.active_tab().expect("an active tab")
}

#[test]
fn matching_is_case_insensitive_plain_and_in_document_order() {
    let doc = lmdv_md::parse(
        "# Akeneo\n\nAkeneo and akeneo.\n\n| Field | Value |\n| --- | --- |\n| Akeneo | x |\n\n**Akeneo** works.\n",
    );
    let hits = find_in_document(&doc, "akeneo");
    assert_eq!(hits.len(), 5, "heading, paragraph twice, cell and strong text: {hits:?}");
    for hit in &hits {
        assert_eq!(doc.source[hit.clone()].to_lowercase(), "akeneo", "every hit is the drawn word, not a marker");
    }
    assert!(hits.windows(2).all(|pair| pair[0].start < pair[1].start), "document order");

    assert!(find_in_document(&doc, "Akeneo|akeneo").is_empty(), "no regex");
    assert!(find_in_document(&doc, "a.*o").is_empty(), "no regex");
    let accented = lmdv_md::parse("Beyoncé\n");
    assert!(find_in_document(&accented, "beyonce").is_empty(), "no accent folding");
    assert_eq!(find_in_document(&accented, "BEYONCÉ").len(), 1, "case-insensitive non-ASCII");
    assert!(find_in_document(&doc, "").is_empty(), "an empty query matches nothing");
}

#[test]
fn the_reader_and_the_parser_agree_on_the_same_hits() {
    // OI-024: the app asks `MdDoc::find`; there is only one matching rule.
    let source = "# Alpha\n\nAlpha **alpha**.\n\n```\nalpha\n```\n";
    let doc = lmdv_md::parse(source);
    let expected = doc.find("alpha");
    assert_eq!(find_in_document(&doc, "alpha"), expected);

    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Find", source);
    state.show_reader_find("alpha");
    assert_eq!(active_tab(&state).view.reader_find.matches, expected);
}

#[test]
fn marks_and_link_targets_are_not_searchable_but_code_is() {
    let doc = lmdv_md::parse("[**Akeneo**](https://example.com/secret)\n\n```rust\nlet akeneo = 1;\n```\n");
    let hits = find_in_document(&doc, "akeneo");
    assert_eq!(hits.len(), 2, "the link text and the code body: {hits:?}");
    assert_eq!(&doc.source[hits[0].clone()], "Akeneo");
    assert_eq!(&doc.source[hits[1].clone()], "akeneo");
    assert!(find_in_document(&doc, "secret").is_empty(), "a link destination is not drawn text");
}

#[test]
fn non_ascii_lowercasing_keeps_byte_offsets() {
    // `İ` lowercases to two characters in the folded string; the returned range
    // still covers the original character exactly.
    assert_eq!(match_ranges("İstanbul", "i̇stanbul"), vec![0..9]);
    assert_eq!(match_ranges("AKENEO akeneo", "akeneo"), vec![0..6, 7..13]);
    assert!(match_ranges("abc", "").is_empty());
}

#[test]
fn prefill_takes_the_first_line_trimmed_and_capped() {
    assert_eq!(reader_find_prefill("  Akeneo  \nsecond line"), "Akeneo");
    assert_eq!(reader_find_prefill("\n\n  two words \n"), "two words");
    assert_eq!(reader_find_prefill("   "), "");
    assert_eq!(reader_find_prefill("  ken  "), "ken", "a partial word is not expanded");
    let long = "x".repeat(140);
    assert_eq!(reader_find_prefill(&long).chars().count(), 100);
}

#[test]
fn the_counter_reads_current_of_total_and_zero_of_zero() {
    let mut find = ReaderFind::default();
    assert_eq!(find.counter(), "0/0");

    find.matches = vec![1..2, 5..6];
    assert_eq!(find.counter(), "1/2");
    assert!(find.step(1));
    assert_eq!(find.counter(), "2/2");
    assert!(find.step(1), "next wraps");
    assert_eq!(find.current, 0);
    assert!(find.step(-1), "previous wraps");
    assert_eq!(find.current, 1);

    find.close();
    assert_eq!(find.counter(), "0/0");
    assert!(!find.step(1), "nothing to step after close");
}

#[test]
fn the_bar_opens_with_hits_and_keeps_its_query_per_tab() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("One", "# Alpha\n\nAlpha beta.\n");
    state.show_reader_find("alpha");
    assert!(active_tab(&state).view.reader_find.open);
    assert_eq!(active_tab(&state).view.reader_find.query.text, "alpha");
    assert_eq!(active_tab(&state).view.reader_find.matches.len(), 2);
    assert_eq!(active_tab(&state).view.reader_find.counter(), "1/2");

    let second = state.open_synthetic("Two", "Beta only.\n");
    state.show_reader_find("beta");
    assert_eq!(active_tab(&state).view.reader_find.query.text, "beta");
    assert_eq!(active_tab(&state).view.reader_find.matches.len(), 1);

    assert!(state.activate_tab(first));
    assert!(active_tab(&state).view.reader_find.open);
    assert_eq!(active_tab(&state).view.reader_find.query.text, "alpha", "the first tab's bar comes back");
    assert_eq!(active_tab(&state).view.reader_find.matches.len(), 2);
    assert_eq!(active_tab(&state).view.reader_find.counter(), "1/2");

    assert!(state.activate_tab(second));
    assert_eq!(active_tab(&state).view.reader_find.query.text, "beta");
}

#[test]
fn zero_hits_show_zero_of_zero_and_paint_nothing() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("None", "Alpha\n");
    state.show_reader_find("zzz");
    assert_eq!(active_tab(&state).view.reader_find.counter(), "0/0");
    assert!(active_tab(&state).view.reader_find.matches.is_empty());
    assert!(state.reader_highlights(active_tab(&state)).is_empty());
}

#[test]
fn entering_edit_mode_closes_the_reading_bar_and_ctrl_f_is_the_editor_panel() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Find", "# Alpha\n\nAlpha.\n");
    state.show_reader_find("alpha");
    assert!(active_tab(&state).view.reader_find.open);

    let mut tx = Tx::default();
    assert!(state.enter_editor(None, &mut tx));
    assert_eq!(active_tab(&state).view.mode, Mode::Editor);
    assert!(!active_tab(&state).view.reader_find.open, "the reading bar closes on entry");
    assert!(active_tab(&state).view.reader_find.matches.is_empty());

    assert!(state.run_command("edit.find", &mut tx));
    assert!(active_tab(&state).view.find_bar.open, "Ctrl+F opens the editor's panel");

    state.leave_editor(&mut tx);
    assert_eq!(active_tab(&state).view.mode, Mode::Viewer);
    assert!(!active_tab(&state).view.reader_find.open, "returning to reading does not reopen it");
}

#[test]
fn n_and_shift_n_step_only_while_the_bar_is_open() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Steps", "alpha alpha alpha\n");
    let mut tx = Tx::default();

    assert!(!state.viewer_context_key(&plain(Key::Text("n".into())), &mut tx), "R-10: closed means nothing");

    state.show_reader_find("alpha");
    assert_eq!(active_tab(&state).view.reader_find.current, 0);
    assert!(state.viewer_context_key(&plain(Key::Text("n".into())), &mut tx));
    assert_eq!(active_tab(&state).view.reader_find.current, 1);
    assert!(state.viewer_context_key(&plain(Key::Text("N".into())), &mut tx));
    assert_eq!(active_tab(&state).view.reader_find.current, 0);
    assert!(state.viewer_context_key(&plain(Key::Text("N".into())), &mut tx));
    assert_eq!(active_tab(&state).view.reader_find.current, 2, "N wraps backwards");

    // A modified key is never `n`.
    assert!(!state.viewer_context_key(&command('n'), &mut tx));
}

#[test]
fn the_line_flash_paints_as_a_line_highlight_until_it_expires() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Line", "# One\n\nfirst\n\n## Two\n\nsecond\n");
    state.set_pending_line(7);
    let mut tx = Tx::default();
    assert!(state.apply_line_target(2.0, &mut tx));
    let flash = state.reading.line_flash.clone().expect("the line is painted");
    assert_eq!(&"# One\n\nfirst\n\n## Two\n\nsecond\n"[flash.range.clone()], "second");

    let highlights = state.reader_highlights(active_tab(&state));
    assert_eq!(highlights.len(), 1);
    assert_eq!(highlights[0].kind, HighlightKind::Line);
    assert_eq!(highlights[0].range, flash.range);

    assert!(state.expire_line_flash(flash.until + 0.1));
    assert!(state.reader_highlights(active_tab(&state)).is_empty(), "the flash clears its paint");
}

#[test]
fn every_hit_is_find_the_current_is_also_current_and_the_line_is_last() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Paint", "# Alpha\n\nAlpha and alpha.\n");
    state.show_reader_find("alpha");
    state.set_pending_line(1);
    let mut tx = Tx::default();
    assert!(state.apply_line_target(0.0, &mut tx));

    let tab = active_tab(&state);
    let find = &tab.view.reader_find;
    let highlights = state.reader_highlights(tab);
    let total = find.matches.len();
    assert_eq!(highlights.len(), total + 2, "every hit, the current again and the line");
    for (index, range) in find.matches.iter().enumerate() {
        assert_eq!(highlights[index].kind, HighlightKind::Find);
        assert_eq!(&highlights[index].range, range);
    }
    assert_eq!(highlights[total].kind, HighlightKind::Current);
    assert_eq!(highlights[total].range, find.matches[find.current]);
    assert_eq!(highlights[total + 1].kind, HighlightKind::Line);
}

#[test]
fn closing_drops_the_hits_and_their_paint() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Close", "alpha\n");
    state.show_reader_find("alpha");
    let mut tx = Tx::default();
    state.close_reader_find(&mut tx);
    assert!(!active_tab(&state).view.reader_find.open);
    assert!(active_tab(&state).view.reader_find.matches.is_empty());
    assert!(state.reader_highlights(active_tab(&state)).is_empty());
    assert_eq!(active_tab(&state).view.reader_find.query.text, "alpha", "the query stays for the next open");
}

#[test]
fn a_parse_that_lands_refreshes_an_open_bar() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Refresh", "alpha\n");
    state.show_reader_find("alpha");
    assert_eq!(active_tab(&state).view.reader_find.matches.len(), 1);

    // Simulate a reparsed model with the same query and a second hit.
    let tab = state.active_tab_mut().expect("tab");
    let revision = tab.view.reader.as_ref().map(|doc| doc.revision).unwrap_or(0);
    tab.view.reader = Some(std::sync::Arc::new(lmdv_md::parse_with_revision("alpha alpha\n", revision + 1)));
    tab.view.reader_revision = Some(revision + 1);
    refresh_tab(tab);
    assert_eq!(tab.view.reader_find.matches.len(), 2);
    assert_eq!(tab.view.reader_find.revision, Some(revision + 1));
}

#[test]
fn a_folder_search_query_prefills_the_opened_tabs_bar() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("One", "alpha\n");
    let second = state.open_synthetic("Two", "beta beta\n");
    state.activate_tab(first);

    let mut tx = Tx::default();
    assert!(state.prefill_reader_find(second, "beta", false, &mut tx));
    let tab = state.tab(second).expect("the background tab");
    assert!(tab.view.reader_find.open);
    assert_eq!(tab.view.reader_find.query.text, "beta");
    assert_eq!(tab.view.reader_find.matches.len(), 2);
    assert_eq!(state.active_tab().map(|tab| tab.id), Some(first), "a background carry keeps the front tab");

    assert!(state.prefill_reader_find(first, "alpha", true, &mut tx));
    assert!(active_tab(&state).view.reader_find.open);
    assert_eq!(active_tab(&state).view.reader_find.query.text, "alpha");
}

#[test]
fn the_runtime_opens_the_bar_with_ctrl_f_and_focuses_the_query() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Find", "# Alpha\n\nAlpha beta.\n");
    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);

    runtime.update(|state, tx| state.open_reader_find(tx));
    runtime.frame(0.0);
    runtime.frame(0.0);
    assert!(active_tab(&runtime.state).view.reader_find.open);
    let elements = runtime.elements();
    assert!(has_key(&elements, "reader-find-bar"), "the floating bar is drawn");
    assert!(has_key(&elements, "reader-find-query"));
    assert!(has_key(&elements, "reader-find-count"));
    assert!(has_key(&elements, "reader-find-prev"));
    assert!(has_key(&elements, "reader-find-next"));
    assert!(has_key(&elements, "reader-find-close"));
    let query =
        elements.iter().find(|element| element.key.as_deref() == Some("reader-find-query")).expect("the query field");
    assert_eq!(query.role, "textbox");
    assert!(query.focused, "R-16 focuses the input");
    assert_eq!(text_at(&elements, "reader-find-count").as_deref(), Some("0/0"));
}

#[test]
fn the_scene_shows_the_bar_and_its_count() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    scene_reader_find(&mut state);
    let elements = render(state);
    assert!(has_key(&elements, "reader-find-bar"));
    assert!(has_key(&elements, "reader-find-query"));
    let count = text_at(&elements, "reader-find-count").expect("the counter");
    assert!(count != "0/0", "the scene's query has hits: {count}");
}

#[test]
fn the_current_hit_key_is_revealed_in_the_article() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Long", READER_MARKDOWN);
    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);

    // The query's only hit is the last heading of the document, far below the
    // first viewport. The first frame paints the highlight under `hl-1` (the
    // Current entry after the Find entry), and the next change reveals it.
    runtime.update(|state, _| state.show_reader_find("Seven point one"));
    runtime.frame(0.0);
    runtime.update(|state, tx| state.reader_find_changed(tx));
    runtime.frame(0.0);
    runtime.frame(0.0);

    let tab = active_tab(&runtime.state);
    assert_eq!(tab.view.reader_find.matches.len(), 1);
    let article = runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some(crate::reader::ARTICLE_KEY))
        .expect("the article column");
    assert!(
        article.scrolled[1] > 0.0,
        "the current hit's highlight key was revealed (scrolled {})",
        article.scrolled[1]
    );

    let highlights = runtime.state.reader_highlights(tab);
    let expected_key = highlight_key(highlights.len() - 1);
    assert_eq!(runtime.state.reader_find_scroll_key(tab), Some(expected_key), "the current entry's paint key");
    assert_eq!(highlights.len(), 2, "one Find entry and the Current duplicate");
    assert_eq!(highlights[0].kind, HighlightKind::Find);
    assert_eq!(highlights[1].kind, HighlightKind::Current);
    assert_eq!(highlights[0].range, highlights[1].range);
}

#[test]
fn the_prefill_only_takes_a_selection_inside_the_article() {
    assert!(selection_in_article(
        "kit-root/shell/split/main-column/main-row/main-pane/window-body/content-pane/reader-pane/article-scroll/article-row/article/doc/b0/h0"
    ));
    assert!(!selection_in_article("kit-root/shell/reader-find-bar/reader-find-query/input"), "the find bar");
    assert!(!selection_in_article("kit-root/shell/split/right/outline-filter/input"), "the outline filter");
    assert!(!selection_in_article("kit-root/shell/browser-search/input"), "the folder search");
}

#[test]
fn ctrl_f_prefills_from_a_non_collapsed_article_selection() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Quiet", READER_MARKDOWN);
    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.update(|_, tx| tx.select_text("h0", 0..14));
    runtime.frame(0.0);
    runtime.update(|state, tx| state.open_reader_find(tx));
    runtime.frame(0.0);
    let find = &active_tab(&runtime.state).view.reader_find;
    assert_eq!(find.query.text, "The quiet page", "the selection becomes the query (R-17)");
    assert_eq!(find.query.range(), (0, 14), "the input is selected, so typing replaces it");
}

#[test]
fn ctrl_f_prefills_a_partial_word_exactly_without_expanding_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Partial", "# Akeneo\n\nAkeneo body.\n");
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.update(|_, tx| tx.select_text("h0", 1..4));
    runtime.frame(0.0);
    runtime.update(|state, tx| state.open_reader_find(tx));
    runtime.frame(0.0);
    let find = &active_tab(&runtime.state).view.reader_find;
    assert_eq!(find.query.text, "ken", "no word expansion: the selected bytes are the query");
    assert_eq!(find.query.range(), (0, 3), "the whole prefill is selected");
    assert_eq!(find.matches.len(), 2, "both drawn occurrences of the partial word are found");
    let query = runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some("reader-find-query"))
        .expect("the query field");
    assert!(query.focused, "R-16/R-17: the field keeps the keyboard");
}

#[test]
fn a_selection_across_a_code_span_prefills_its_drawn_text() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Code", "Before **bold** and `code` after.\n");
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.update(|_, tx| tx.select_text("b0-text-0", 0..27));
    runtime.frame(0.0);
    runtime.update(|state, tx| state.open_reader_find(tx));
    runtime.frame(0.0);
    let find = &active_tab(&runtime.state).view.reader_find;
    assert_eq!(find.query.text, "Before bold and code after.", "the drawn text, never the markers");
    assert!(!find.query.text.contains('\n'), "one line");
    assert_eq!(find.matches.len(), 1, "the prefill finds the paragraph it came from");
}

#[test]
fn a_selection_across_an_inline_image_drops_the_object_marker() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("notes");
    std::fs::create_dir_all(&root).unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/img/test.png");
    std::fs::copy(fixture, root.join("dot.png")).unwrap();
    std::fs::write(root.join("note.md"), "Before ![dot](./dot.png) after.\n").unwrap();
    let mut state = window(&temp);
    state.shared().set_headless();
    state.open_path(root.join("note.md"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.active_tab().and_then(|tab| tab.view.reader.as_ref()).is_none() && std::time::Instant::now() < deadline
    {
        state.poll(0.0, &mut Tx::default());
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(state.active_tab().and_then(|tab| tab.view.reader.as_ref()).is_some(), "the page parsed");
    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.update(|_, tx| tx.select_text("b0-text-0", 0..100));
    runtime.frame(0.0);
    runtime.update(|state, tx| state.open_reader_find(tx));
    runtime.frame(0.0);
    let query = active_tab(&runtime.state).view.reader_find.query.text.clone();
    assert_eq!(query, "Before  after.", "the exact selected text: the object is uncopied, not replaced");
    assert!(!query.contains('\u{fffc}'), "the object placeholder is not a find query: {query:?}");
    assert!(!query.contains('\n'), "one line: {query:?}");
    assert_eq!(query.split_whitespace().collect::<Vec<_>>(), ["Before", "after."], "the text on both sides");
}

#[test]
fn a_selection_that_trims_to_nothing_opens_an_empty_query() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Gap", "alpha  beta\n");
    state.show_reader_find("alpha");
    assert_eq!(active_tab(&state).view.reader_find.matches.len(), 1);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    // The two spaces between the words: a real, non-collapsed selection that
    // trims to nothing.
    runtime.update(|_, tx| tx.select_text("b0-text-0", 5..7));
    runtime.frame(0.0);
    runtime.update(|state, tx| state.open_reader_find(tx));
    runtime.frame(0.0);
    let find = &active_tab(&runtime.state).view.reader_find;
    assert!(find.open);
    assert_eq!(find.query.text, "", "the whitespace selection is the query, not the last query");
    assert!(find.matches.is_empty(), "an empty query has no hits");
    assert_eq!(find.counter(), "0/0");
    assert_eq!(find.query.range(), (0, 0));
    let query = runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some("reader-find-query"))
        .expect("the query field");
    assert!(query.focused, "the field keeps the keyboard");
}

// ---- R-17: the block menu's Find in document -----------------------------

#[test]
fn the_block_menus_find_in_document_uses_the_selection_prefill_and_focuses_the_query() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Quiet", READER_MARKDOWN);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.update(|_, tx| tx.select_text("h0", 0..14));
    runtime.frame(0.0);
    runtime.update(|state, tx| {
        // The right-click that opened the menu, then its Find in document row.
        state.show_block_menu(
            &lmdv_render::BlockRef {
                key: "b0".to_owned(),
                line: 0,
                end_line: 1,
                code: false,
                text: "The quiet page".to_owned(),
                texts: Vec::new(),
            },
            rune::PointerPress { x: 40.0, y: 60.0 },
        );
        assert!(state.block_menu().is_some());
        state.find_in_document_from_block_menu(tx);
    });
    runtime.frame(0.0);

    let find = &active_tab(&runtime.state).view.reader_find;
    assert!(find.open, "R-17: Find in document opens the reading bar");
    assert_eq!(find.query.text, "The quiet page", "the same query Ctrl+F would prefill");
    assert_eq!(find.query.range(), (0, 14), "the query is selected for typing");
    assert_eq!(find.matches.len(), 1, "the hit is already revealed");
    assert!(runtime.state.block_menu().is_none(), "the menu closes behind the bar");
    let query = runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some("reader-find-query"))
        .expect("the query field");
    assert!(query.focused, "the field takes the keyboard");
}

// ---- R-19: the scroll band ----------------------------------------------

#[test]
fn a_block_clear_of_both_bands_reveals_without_centring() {
    let viewport = 700.0;
    let safe = FindBand { top: 200.0, bottom: 480.0 };
    assert_eq!(find_reveal(Some(safe), viewport, READER_FIND_BAND, 60.0), FindReveal::IntoView);
    // R-19's edges: inside 60 points of the top band or the bottom edge centres.
    let top = FindBand { top: 40.0, bottom: 300.0 };
    assert_eq!(find_reveal(Some(top), viewport, READER_FIND_BAND, 60.0), FindReveal::Center);
    let bottom = FindBand { top: 300.0, bottom: 660.0 };
    assert_eq!(find_reveal(Some(bottom), viewport, READER_FIND_BAND, 60.0), FindReveal::Center);
    // A block that spans a band may put the hit there: centre.
    let tall = FindBand { top: -100.0, bottom: 1200.0 };
    assert_eq!(find_reveal(Some(tall), viewport, READER_FIND_BAND, 60.0), FindReveal::Center);
    // No measurement or no viewport yet: the safe minimal reveal.
    assert_eq!(find_reveal(None, viewport, READER_FIND_BAND, 60.0), FindReveal::IntoView);
    assert_eq!(find_reveal(Some(safe), 0.0, READER_FIND_BAND, 60.0), FindReveal::IntoView);
    // With the section line on screen the top band grows by its height.
    assert_eq!(find_reveal(Some(top), viewport, READER_FIND_BAND + 30.0, 60.0), FindReveal::Center);
}

#[test]
fn the_top_band_grows_with_the_section_line_and_not_the_notice() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Bands", "# One\n\nText\n");
    assert_eq!(state.find_top_band(), READER_FIND_BAND);
    state
        .section_line
        .publish(SectionLineFrame { segments: vec![SectionSegment { heading: 0, text: "One".to_owned() }] });
    assert_eq!(state.find_top_band(), READER_FIND_BAND + SECTION_LINE_HEIGHT);
    // A notice takes flow space below the article, so it never covers a match.
    state.notice("Something happened");
    assert_eq!(state.find_top_band(), READER_FIND_BAND + SECTION_LINE_HEIGHT);
    state.section_line.reset();
}

#[test]
fn a_revealed_hit_near_the_bottom_edge_is_centred() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Long", READER_MARKDOWN);
    let mut runtime = runtime(state);
    settle(&mut runtime, 4);
    runtime.update(|state, _| state.show_reader_find("Seven point one"));
    runtime.frame(1.0 / 60.0);
    runtime.update(|state, tx| state.reader_find_changed(tx));
    settle(&mut runtime, 150);

    let tab = active_tab(&runtime.state);
    assert_eq!(tab.view.reader_find.matches.len(), 1);
    assert!(tab.view.reader_find.probe.is_none(), "the probe answered");
    let elements = runtime.elements();
    let viewport = elements
        .iter()
        .find(|element| element.key.as_deref() == Some(crate::reader::ARTICLE_SCROLL_KEY))
        .expect("the article viewport")
        .rect;
    let mark = elements.iter().find(|element| element.key.as_deref() == Some("hl-1")).expect("the current hit's paint");
    let centre = mark.rect[1] + mark.rect[3] / 2.0;
    let view_centre = viewport[1] + viewport[3] / 2.0;
    assert!(
        (centre - view_centre).abs() < 40.0,
        "R-19 centres a hit near an edge: paint centre {centre}, viewport centre {view_centre}"
    );
}

#[test]
fn a_question_mark_typed_in_the_find_field_does_not_open_help() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Quiet", READER_MARKDOWN);
    let mut runtime = runtime(state);
    settle(&mut runtime, 4);
    runtime.update(|state, tx| state.open_reader_find(tx));
    runtime.frame(1.0 / 60.0);

    let question = KeyEvent { key: Key::Text("?".to_owned()), mods: Mods { shift: true, ..Mods::default() } };
    runtime.key(question);
    runtime.frame(1.0 / 60.0);
    runtime.frame(1.0 / 60.0);
    let find = &active_tab(&runtime.state).view.reader_find;
    assert!(runtime.state.help_view().is_none(), "R-11: a text field keeps `?`");
    assert_eq!(find.query.text, "?", "the field typed the character");
}
