//! P6-F: the file browser's tree rules, rooting, collapse persistence, watch
//! rebuild focus, folder search and the pane's structural output.

use std::fs;
use std::path::{Path, PathBuf};

use lmdv_io::store::UiPrefs;
use lmdv_io::tree::{SearchHit, TreeError, TreeListing};
use rune::ElementInfo;
use tempfile::TempDir;

use crate::browser::{SEARCH_DEBOUNCE, collapse_key, search_prefill};
use crate::test_support;
use crate::view::view;
use crate::window::WindowState;

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(test_support::shared(temp))
}

fn write(root: &Path, relative: &str, body: &str) -> PathBuf {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, body).unwrap();
    path
}

/// Roots the pane and applies a listing without a worker round-trip.
fn show(state: &mut WindowState, listing: TreeListing) {
    state.browser.root = Some(listing.root.clone());
    state.browser.root_name = listing.name.clone();
    state.browser.tree_generation += 1;
    let generation = state.browser.tree_generation;
    let root = listing.root.clone();
    assert!(state.finish_tree_listing(generation, root, Ok(listing)), "the first listing changes the rows");
}

fn listing(root: &Path) -> TreeListing {
    lmdv_io::tree::list_tree(root).unwrap()
}

fn row_paths(state: &WindowState) -> Vec<String> {
    state.browser.rows().iter().map(|row| row.name.clone()).collect()
}

fn render(state: WindowState) -> Vec<ElementInfo> {
    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.elements()
}

fn has_key(elements: &[ElementInfo], key: &str) -> bool {
    elements.iter().any(|element| element.key.as_deref() == Some(key))
}

#[test]
fn a_listed_folder_shows_folders_and_markdown_files_with_the_document_highlight() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let root = temp.path().join("notes");
    write(&root, "b.md", "b");
    write(&root, "a.md", "a");
    write(&root, "nested/deeper.md", "deep");
    write(&root, "image.png", "png");
    show(&mut state, listing(&root));
    assert_eq!(
        row_paths(&state),
        ["nested", "deeper.md", "a.md", "b.md"],
        "folders first, then case-insensitive names"
    );

    let deeper = root.join("nested/deeper.md");
    state.open_path(deeper.clone());
    assert!(state.follow_browser_active(), "the active document changed the pane");
    assert_eq!(state.browser.active(), Some(deeper.to_string_lossy().as_ref()));
    assert_eq!(state.browser.focused(), Some(deeper.to_string_lossy().as_ref()));

    state.left.set_open(true);
    let elements = render(state);
    assert!(elements.iter().any(|element| element.key.as_deref() == Some("browser-tree") && element.role == "tree"));
    for name in ["nested", "a.md", "b.md", "deeper.md"] {
        assert!(
            elements.iter().any(|element| element.role == "treeitem" && element.label.as_deref() == Some(name)),
            "the tree draws {name}: {elements:#?}"
        );
    }
    let active = elements
        .iter()
        .find(|element| element.key.as_deref() == Some(deeper.to_string_lossy().as_ref()))
        .expect("the active row is drawn");
    assert!(active.item_selected, "the open document's row is selected");
    assert!(!elements.iter().any(|element| element.label.as_deref() == Some("image.png")));
    assert!(has_key(&elements, "browser-search"));
    assert!(has_key(&elements, "browser-up"));
    assert!(has_key(&elements, "browser-refresh"));
}

#[test]
fn opening_outside_the_root_reroots_and_opening_inside_keeps_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let project = temp.path().join("project");
    let other = temp.path().join("other");
    let inside = write(&project, "a.md", "a");
    let outside = write(&other, "b.md", "b");
    let nested = write(&project, "nested/deeper.md", "deep");

    state.set_browser_root(project.clone());
    state.open_path(inside.clone());
    assert_eq!(state.browser.root(), Some(project.as_path()), "a file inside keeps the root");

    state.open_path(nested.clone());
    assert_eq!(state.browser.root(), Some(project.as_path()), "a nested file keeps the root");

    state.open_path(outside.clone());
    assert_eq!(state.browser.root(), Some(other.as_path()), "a file outside re-roots the pane");

    // Persisted so the next launch shows the same folder (F-07's sibling rule).
    assert_eq!(state.shared().ui().tree_root.as_deref(), Some(other.as_path()));
}

#[test]
fn up_moves_to_the_parent_and_a_filesystem_root_cannot_go_up() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let root = temp.path().join("a/b");
    fs::create_dir_all(&root).unwrap();
    state.set_browser_root(root.clone());
    assert!(state.browser_can_go_up());
    assert!(state.browser_up());
    assert_eq!(state.browser.root(), Some(temp.path().join("a").as_path()));

    state.browser.root = Some(PathBuf::from("/"));
    state.browser.root_name = "/".to_owned();
    assert!(!state.browser_can_go_up());
    assert!(!state.browser_up(), "the root stays at /");
    assert_eq!(state.browser.root(), Some(Path::new("/")));
}

#[test]
fn collapsed_folders_survive_rebuilds_and_open_when_their_file_is_shown() {
    let temp = TempDir::new().unwrap();
    let shared = test_support::shared(&temp);
    let mut state = WindowState::new(shared.clone());
    let root = temp.path().join("notes");
    let deeper = write(&root, "nested/deeper.md", "deep");
    let nested = root.join("nested");
    show(&mut state, listing(&root));
    assert!(row_paths(&state).contains(&"deeper.md".to_owned()));

    state.toggle_browser_folder(nested.to_string_lossy().as_ref());
    assert!(!row_paths(&state).contains(&"deeper.md".to_owned()), "the child row is hidden");
    assert!(shared.ui().collapsed_folders.contains(&PathBuf::from(collapse_key(&nested))));

    // A watcher listing that changes elsewhere still honors the collapsed set.
    write(&root, "new.md", "new");
    show(&mut state, listing(&root));
    assert!(!row_paths(&state).contains(&"deeper.md".to_owned()), "a rebuild keeps folders closed");

    // Selecting the file expands and persists the change (F-07).
    state.open_path(deeper.clone());
    assert!(state.follow_browser_active());
    assert!(row_paths(&state).contains(&"deeper.md".to_owned()), "the ancestor opened");
    assert!(!state.browser.collapsed.contains(&collapse_key(&nested)));
    assert!(shared.ui().collapsed_folders.is_empty(), "the persisted set follows");

    // A fresh window reads the same shared UI state.
    let mut second = WindowState::new(shared.clone());
    second.browser.root = Some(root.clone());
    second.browser.tree_generation += 1;
    let generation = second.browser.tree_generation;
    second.finish_tree_listing(generation, root.clone(), Ok(listing(&root)));
    assert!(row_paths(&second).contains(&"deeper.md".to_owned()));
}

#[test]
fn a_rebuild_keeps_the_focused_row_or_its_folder() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let root = temp.path().join("notes");
    let deep = write(&root, "a/b/c.md", "c");
    write(&root, "a/b/keep.md", "keep");
    show(&mut state, listing(&root));
    state.browser.focus = Some(deep.to_string_lossy().into_owned());

    fs::remove_file(&deep).unwrap();
    let changed = listing(&root);
    state.browser.tree_generation += 1;
    let generation = state.browser.tree_generation;
    assert!(state.finish_tree_listing(generation, root.clone(), Ok(changed)));
    assert_eq!(state.browser.focused(), Some(root.join("a/b").to_string_lossy().as_ref()), "the folder takes over");
}

#[test]
fn an_unchanged_listing_does_not_rebuild_or_move_focus() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let root = temp.path().join("notes");
    let deep = write(&root, "nested/deeper.md", "deep");
    show(&mut state, listing(&root));
    state.browser.focus = Some(deep.to_string_lossy().into_owned());
    let epoch = state.browser.row_epoch;

    state.browser.tree_generation += 1;
    let generation = state.browser.tree_generation;
    assert!(!state.finish_tree_listing(generation, root.clone(), Ok(listing(&root))), "same JSON, no rebuild");
    assert_eq!(state.browser.row_epoch, epoch);
    assert_eq!(state.browser.focused(), Some(deep.to_string_lossy().as_ref()));
}

#[test]
fn a_folder_link_reroots_the_pane_and_a_plain_file_falls_back() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let project = temp.path().join("project");
    write(&project, "a.md", "a");
    let notes = temp.path().join("notes");
    write(&notes, "note.md", "note");
    let image = write(&project, "image.png", "png");
    state.browser.root = Some(project.clone());
    state.browser.root_name = "project".to_owned();

    state.open_folder_link(notes.clone());
    assert_eq!(state.browser.root(), Some(notes.as_path()), "the folder link roots the pane there");
    assert!(state.left.is_open(), "and shows the pane");
    let generation = state.browser.tree_generation;
    assert!(state.finish_tree_listing(generation, notes.clone(), Ok(listing(&notes))));

    state.browser.root = Some(project.clone());
    state.browser.root_name = "project".to_owned();
    state.open_folder_link(image.clone());
    let generation = state.browser.tree_generation;
    assert!(state.finish_tree_listing(generation, image.clone(), Err(TreeError::NotADirectory(image.clone()))));
    assert_eq!(state.browser.root(), Some(project.as_path()), "a non-folder link returns to the old root");
    assert!(state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("not a Markdown file")));
}

#[test]
fn search_results_group_by_file_and_open_at_the_line() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let root = temp.path().join("notes");
    let pim = write(&root, "PIM.md", "body\n");
    let notes = write(&root, "notes.md", "one\nA pim here\nthree\npim again\n");
    state.browser.root = Some(root.clone());
    state.browser.root_name = "notes".to_owned();
    state.browser_search.replace_text("pim");
    state.browser.search_query = "pim".to_owned();
    state.browser.search_generation += 1;
    let generation = state.browser.search_generation;
    let hits = vec![
        SearchHit { path: pim.clone(), name: "PIM.md".to_owned(), line: 0, preview: String::new() },
        SearchHit { path: notes.clone(), name: "notes.md".to_owned(), line: 2, preview: "A pim here".to_owned() },
        SearchHit { path: notes.clone(), name: "notes.md".to_owned(), line: 4, preview: "pim again".to_owned() },
    ];
    assert!(state.finish_search(generation, root.clone(), "pim".to_owned(), Ok(hits)));
    assert!(state.browser.is_searching());
    let rows = state.browser.results();
    assert_eq!(rows.len(), 4, "two file headers and two line rows: {rows:?}");
    assert_eq!(rows[0].line, 0);
    assert_eq!(rows[0].path, pim);
    assert_eq!(rows[1].line, 0);
    assert_eq!(rows[2].line, 2);
    assert_eq!(rows[2].preview, "A pim here");
    assert_eq!(state.browser.selected_result(), Some(0));

    // Down moves through file headers and line rows; Enter opens the line.
    let mut tx = rune::Tx::default();
    let down = rune::KeyEvent { key: rune::Key::Down, mods: rune::Mods::default() };
    assert!(state.browser_search_key(&down, &mut tx));
    assert_eq!(state.browser.selected_result(), Some(1));
    assert!(state.browser_search_key(&down, &mut tx));
    assert!(state.browser_search_key(&down, &mut tx));
    assert_eq!(state.browser.selected_result(), Some(3), "the second line row of notes.md");
    let enter = rune::KeyEvent { key: rune::Key::Enter, mods: rune::Mods::default() };
    assert!(state.browser_search_key(&enter, &mut tx));
    let tab = state.active_tab().expect("the opened tab");
    assert_eq!(tab.document().and_then(|doc| doc.key.borrow().path().map(Path::to_path_buf)), Some(notes));
    assert_eq!(tab.view.line, Some(4), "the hit's 1-based line");

    // Escape clears the query, the results and the tree's search mode (F-17).
    let escape = rune::KeyEvent { key: rune::Key::Escape, mods: rune::Mods::default() };
    assert!(state.browser_search_key(&escape, &mut tx));
    assert!(!state.browser.is_searching());
    assert!(state.browser.results().is_empty());
    assert!(state.browser_search.text.is_empty());
}

#[test]
fn search_is_debounced_and_an_empty_query_closes_the_results() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let mut tx = rune::Tx::default();
    state.browser_search.replace_text("pim");
    state.browser_search_changed(10.0, &mut tx);
    assert_eq!(state.browser.search_due, Some(10.0 + SEARCH_DEBOUNCE));
    assert!(!state.browser.is_searching(), "nothing is searched until the debounce passes");

    state.browser_search.replace_text("");
    state.browser_search_changed(11.0, &mut tx);
    assert_eq!(state.browser.search_due, None);
    assert!(!state.browser.is_searching());
}

#[test]
fn the_search_prefill_is_the_first_trimmed_selection_line() {
    assert_eq!(search_prefill("  Akeneo  \nsecond line"), "Akeneo");
    assert_eq!(search_prefill("\n\nbelow"), "");
    let long = "x".repeat(150);
    assert_eq!(search_prefill(&long).chars().count(), 100);
}

#[test]
fn search_results_render_with_no_matches_and_with_grouped_rows() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let root = temp.path().join("notes");
    write(&root, "notes.md", "pim\n");
    state.browser.root = Some(root.clone());
    state.browser.root_name = "notes".to_owned();
    state.browser_search.replace_text("zzz");
    state.browser.search_query = "zzz".to_owned();
    state.browser.search_generation += 1;
    let generation = state.browser.search_generation;
    state.finish_search(generation, root.clone(), "zzz".to_owned(), Ok(Vec::new()));
    state.left.set_open(true);
    let elements = render(state);
    assert!(has_key(&elements, "browser-no-matches"), "the empty state is drawn");

    let mut state = window(&temp);
    state.browser.root = Some(root.clone());
    state.browser.root_name = "notes".to_owned();
    state.browser_search.replace_text("pim");
    state.browser.search_query = "pim".to_owned();
    state.browser.search_generation += 1;
    let generation = state.browser.search_generation;
    let notes = root.join("notes.md");
    state.finish_search(
        generation,
        root,
        "pim".to_owned(),
        Ok(vec![SearchHit { path: notes, name: "notes.md".to_owned(), line: 1, preview: "pim".to_owned() }]),
    );
    state.left.set_open(true);
    let elements = render(state);
    assert!(has_key(&elements, "browser-result-0"), "the file header row");
    assert!(has_key(&elements, "browser-result-1"), "the line row");
    assert!(elements.iter().any(|element| element.text.as_deref() == Some("Line 1")));
}

#[test]
fn startup_restores_the_root_and_collapsed_folders() {
    let temp = TempDir::new().unwrap();
    let shared = test_support::shared(&temp);
    let root = temp.path().join("notes");
    fs::create_dir_all(root.join("nested")).unwrap();
    let ui =
        UiPrefs { tree_root: Some(root.clone()), collapsed_folders: vec![root.join("nested")], ..UiPrefs::default() };
    shared.set_ui(ui);

    let state = WindowState::new(shared);
    assert_eq!(state.browser.root(), Some(root.as_path()));
    assert!(state.browser.collapsed.contains(&collapse_key(&root.join("nested"))));
}

#[test]
fn a_click_on_a_file_row_opens_it_and_keeps_the_root() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let root = temp.path().join("notes");
    let file = write(&root, "nested/deeper.md", "deep");
    show(&mut state, listing(&root));
    state.left.set_open(true);

    let mut tx = rune::Tx::default();
    state.browser_row_pressed(file.to_string_lossy().as_ref(), false, rune::Mods::default(), &mut tx);
    assert_eq!(
        state.active_tab().and_then(|tab| tab.document()).and_then(|doc| doc
            .key
            .borrow()
            .path()
            .map(Path::to_path_buf)),
        Some(file),
        "one click opens the file in a tab"
    );
    assert_eq!(state.browser.root(), Some(root.as_path()), "opening from the pane keeps the root");
    assert!(state.left.is_open(), "the pane stays open");
}

#[test]
fn the_folder_search_shortcut_is_ctrl_shift_f_and_shows_the_pane() {
    let event = rune::KeyEvent {
        key: rune::Key::Command('f'),
        mods: rune::Mods { cmd: true, ctrl: true, shift: true, ..rune::Mods::default() },
    };
    assert_eq!(crate::commands::command_for_key(&event).map(|command| command.id), Some("browser.search"));

    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let mut tx = rune::Tx::default();
    assert!(state.run_command("browser.search", &mut tx));
    assert!(state.left.is_open(), "the shortcut shows the file pane");
}

#[test]
fn a_watch_report_relists_only_the_watched_root() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let root = temp.path().join("notes");
    write(&root, "a.md", "a");
    show(&mut state, listing(&root));
    assert!(!state.browser.tree_pending, "the applied listing settled");

    state.worker().post(crate::jobs::JobMsg::TreeChanged { root: temp.path().join("elsewhere") });
    let mut tx = rune::Tx::default();
    state.poll(0.0, &mut tx);
    assert!(!state.browser.tree_pending, "another root's watcher is ignored");

    state.worker().post(crate::jobs::JobMsg::TreeChanged { root: root.clone() });
    state.poll(0.0, &mut tx);
    assert!(state.browser.tree_pending, "the watched root lists again");
}

#[cfg(unix)]
#[test]
fn a_symlinked_root_keeps_its_spelling_when_a_listed_file_opens() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let target = temp.path().join("real-notes");
    write(&target, "a.md", "a");
    let link = temp.path().join("linked-notes");
    symlink(&target, &link).unwrap();

    let mut state = window(&temp);
    let listing = listing(&link);
    assert_ne!(listing.root, link, "the listing canonicalizes its root");
    state.browser.root = Some(link.clone());
    state.browser.root_name = "linked-notes".to_owned();
    state.browser.tree_generation += 1;
    let generation = state.browser.tree_generation;
    assert!(state.finish_tree_listing(generation, listing.root.clone(), Ok(listing)));

    state.open_path(link.join("a.md"));
    assert!(state.follow_browser_active());
    assert_eq!(state.browser.root(), Some(link.as_path()), "opening a listed file keeps the pane's spelling");
    assert_eq!(state.browser.active(), Some(link.join("a.md").to_string_lossy().as_ref()));
}

#[test]
fn a_failed_folder_link_with_no_previous_root_returns_to_the_empty_state() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let file = write(temp.path(), "image.png", "png");
    assert!(state.browser.root().is_none());

    state.open_folder_link(file.clone());
    assert_eq!(state.browser.root(), Some(file.as_path()));
    let generation = state.browser.tree_generation;
    assert!(state.finish_tree_listing(generation, file.clone(), Err(TreeError::NotADirectory(file.clone()))));
    assert!(state.browser.root().is_none(), "no folder open again");
    assert!(state.shared().ui().tree_root.is_none(), "the bad root is not persisted");
    assert!(state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("not a Markdown file")));
}

/* ---- F-12: the file and folder context menu ---- */

fn menu_for(state: &mut WindowState, path: &Path, dir: bool) -> Vec<crate::browser::BrowserMenuEntry> {
    state.show_browser_menu(path.to_path_buf(), dir, rune::PointerPress { x: 40.0, y: 80.0 });
    let menu = state.browser_menu().expect("the menu opened");
    crate::browser::browser_menu_entries(menu)
}

fn labels(entries: &[crate::browser::BrowserMenuEntry]) -> Vec<&'static str> {
    entries.iter().map(|entry| entry.label).collect()
}

#[test]
fn a_files_menu_lists_open_reveal_copy_and_a_relative_link() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let file = temp.path().join("notes/deeper.md");
    let entries = menu_for(&mut state, &file, false);
    assert_eq!(
        labels(&entries),
        ["Open in a new tab", "Open in a new window", "Show in folder", "Copy path", "Copy as Markdown link"]
    );
    assert!(entries.iter().all(|entry| entry.enabled), "every file row is enabled");
    assert!(entries[2].separator_before, "the Open rows stand apart");
    assert!(!entries[0].separator_before && !entries[1].separator_before);
    assert_eq!(entries[0].hint, Some("Ctrl+click"));
    assert_eq!(entries[1].hint, Some("Shift+click"));
    assert_eq!(entries[4].description, Some("Relative to the open document"));
}

#[test]
fn a_folders_menu_has_no_open_rows() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let folder = temp.path().join("notes");
    let entries = menu_for(&mut state, &folder, true);
    assert_eq!(labels(&entries), ["Show in folder", "Copy path"]);
    assert!(entries.iter().all(|entry| entry.enabled));
    assert!(!entries[0].separator_before, "the folder menu is two rows");

    // The filesystem root has no parent to reveal the item in.
    let entries = menu_for(&mut state, Path::new("/"), true);
    assert!(!entries[0].enabled, "Show in folder needs a parent folder");
    assert!(entries[1].enabled, "Copy path still copies /");
}

#[test]
fn the_menu_renders_as_a08_menu_with_its_rows() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let file = temp.path().join("notes/deeper.md");
    state.show_browser_menu(file, false, rune::PointerPress { x: 40.0, y: 80.0 });
    let elements = render(state);
    assert!(
        elements.iter().any(|element| element.role == "menu" && element.key.as_deref() == Some("browser-menu-menu")),
        "the surface is a menu"
    );
    let items: Vec<&ElementInfo> = elements.iter().filter(|element| element.role == "menuitem").collect();
    assert_eq!(items.len(), 5, "five file rows: {items:#?}");
    for label in ["Open in a new tab", "Open in a new window", "Show in folder", "Copy path", "Copy as Markdown link"] {
        let item = elements
            .iter()
            .find(|element| element.role == "menuitem" && element.label.as_deref() == Some(label))
            .unwrap_or_else(|| panic!("{label} is drawn"));
        assert!(item.focusable, "an enabled item can take the keyboard: {label}");
    }
    assert!(
        elements.iter().any(|element| element.label.as_deref() == Some("Open in a new tab")
            && element.key.as_deref() == Some("browser-menu-item0")),
        "the first row keeps the key the opening focus asks for: {items:#?}"
    );
}

#[test]
fn a_markdown_link_is_relative_to_the_document_and_percent_encoded() {
    use crate::browser::{markdown_link, relative_path};

    let dir = Path::new("/home/me/notes");
    assert_eq!(
        markdown_link(Some(dir), Path::new("/home/me/notes/sub/My Note.md"), "My Note.md"),
        "[My Note](sub/My%20Note.md)"
    );
    assert_eq!(
        markdown_link(Some(Path::new("/home/me/notes/sub")), Path::new("/home/me/notes/a.markdown"), "a.markdown"),
        "[a](../a.markdown)"
    );
    assert_eq!(
        markdown_link(None, Path::new("/home/me/notes/Caf\u{e9} #1.md"), "Caf\u{e9} #1.md"),
        "[Caf\u{e9} #1](/home/me/notes/Caf%C3%A9%20%231.md)",
        "pasted text has no folder: the path stays whole, and # never reads as a fragment"
    );
    assert_eq!(
        markdown_link(Some(dir), Path::new("/home/me/notes/a.B%20b.MD"), "a.B%20b.MD"),
        "[a.B%20b](a.B%2520b.MD)",
        "a percent sign is encoded, an upper-case extension is dropped from the label"
    );
    assert_eq!(relative_path(Path::new("/a/B"), Path::new("/a/b/c.md")), "c.md", "prefixes compare without case");
    assert_eq!(relative_path(Path::new("/a/b"), Path::new("/a/c/d.md")), "../c/d.md");
    assert_eq!(relative_path(Path::new("/a/b"), Path::new("/x/y.md")), "/x/y.md", "nothing shared stays absolute");
}

#[test]
fn the_menus_open_rows_use_the_background_tab_and_new_window_paths() {
    let temp = TempDir::new().unwrap();
    let shared = test_support::shared(&temp);
    let root = temp.path().join("notes");
    let first = write(&root, "first.md", "first");
    let second = write(&root, "second.md", "second");
    let mut state = WindowState::new(shared.clone());
    state.open_path(first.clone());
    let mut tx = rune::Tx::default();

    // Open in a new tab is the original's background open: the tab joins the
    // strip, the document in front stays.
    state.show_browser_menu(second.clone(), false, rune::PointerPress { x: 40.0, y: 80.0 });
    state.run_browser_menu_action(crate::browser::BrowserMenuAction::OpenTab, &mut tx);
    assert!(state.browser_menu().is_none(), "a choice closes the menu");
    assert_eq!(state.tabs().len(), 2);
    assert_eq!(state.active_path(), Some(first.clone()), "the background tab did not take the front");
    assert!(state.tabs().iter().any(
        |tab| tab.document().and_then(|doc| doc.key.borrow().path().map(Path::to_path_buf)) == Some(second.clone())
    ));

    // Open in a new window asks the installed opener with the row's launch.
    shared.set_window_opener(recording_opener);
    OPENED.with(|opened| opened.borrow_mut().clear());
    state.show_browser_menu(second.clone(), false, rune::PointerPress { x: 40.0, y: 80.0 });
    state.run_browser_menu_action(crate::browser::BrowserMenuAction::OpenWindow, &mut tx);
    let opened = recorded_windows();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].launch.path.as_deref(), Some(second.as_path()));
    assert_eq!(opened[0].launch.line, None);
}

#[test]
fn copy_path_and_copy_link_land_on_the_clipboard() {
    use crate::browser::BrowserMenuAction;

    let temp = TempDir::new().unwrap();
    let root = temp.path().join("notes");
    let file = write(&root, "My Note.md", "note");
    write(&root, "index.md", "index");
    let mut state = window(&temp);
    state.open_path(root.join("index.md"));
    let board = rune::clipboard::memory();
    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(true, 2.0));
    runtime.use_clipboard(&board);
    runtime.frame(0.0);

    runtime.update(|state: &mut WindowState, tx: &mut rune::Tx| {
        state.show_browser_menu(file.clone(), false, rune::PointerPress { x: 40.0, y: 80.0 });
        state.run_browser_menu_action(BrowserMenuAction::CopyPath, tx);
    });
    assert_eq!(wait_for_clipboard(&mut runtime, &board), file.to_string_lossy());

    runtime.update(|state: &mut WindowState, tx: &mut rune::Tx| {
        state.show_browser_menu(file.clone(), false, rune::PointerPress { x: 40.0, y: 80.0 });
        state.run_browser_menu_action(BrowserMenuAction::CopyLink, tx);
    });
    assert_eq!(wait_for_clipboard(&mut runtime, &board), "[My Note](My%20Note.md)");
}

#[test]
fn shift_f10_opens_the_focused_rows_menu_under_it_and_escape_returns_focus() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let root = temp.path().join("notes");
    let file = write(&root, "nested/deeper.md", "deep");
    show(&mut state, listing(&root));
    state.browser.focus = Some(file.to_string_lossy().into_owned());
    state.left.set_open(true);

    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.update(|_: &mut WindowState, tx: &mut rune::Tx| tx.focus(crate::browser::BROWSER_TREE_KEY, false));
    runtime.frame(0.0);

    runtime.key(rune::KeyEvent {
        key: rune::Key::Function(10),
        mods: rune::Mods { shift: true, ..rune::Mods::default() },
    });
    runtime.frame(0.0);
    runtime.frame(0.0);
    assert!(runtime.state.browser_menu().is_some(), "Shift+F10 opens the focused row's menu");

    let path = file.to_string_lossy().into_owned();
    let (rect, _) = runtime.presented(&path).expect("the focused row is drawn");
    let point = runtime.state.browser_menu().expect("the menu").point();
    assert!((point.x - (rect[0] + 12.0)).abs() < 0.6, "the menu starts inside the row: {point:?} {rect:?}");
    assert!((point.y - (rect[1] + rect[3] + 2.0)).abs() < 0.6, "and below it: {point:?} {rect:?}");

    runtime.key(rune::KeyEvent { key: rune::Key::Escape, mods: rune::Mods::default() });
    runtime.frame(0.0);
    runtime.frame(0.0);
    assert!(runtime.state.browser_menu().is_none(), "Escape closes it");
    assert_eq!(focused_key(&runtime).as_deref(), Some(crate::browser::BROWSER_TREE_KEY));
}

#[test]
fn tab_also_closes_the_menu_and_returns_focus() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let root = temp.path().join("notes");
    write(&root, "a.md", "a");
    show(&mut state, listing(&root));
    state.browser.focus = Some(root.join("a.md").to_string_lossy().into_owned());
    state.left.set_open(true);

    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.update(|_: &mut WindowState, tx: &mut rune::Tx| tx.focus(crate::browser::BROWSER_TREE_KEY, false));
    runtime.frame(0.0);
    runtime.key(rune::KeyEvent {
        key: rune::Key::Function(10),
        mods: rune::Mods { shift: true, ..rune::Mods::default() },
    });
    runtime.frame(0.0);
    runtime.frame(0.0);
    assert!(runtime.state.browser_menu().is_some());

    runtime.key(rune::KeyEvent { key: rune::Key::Tab, mods: rune::Mods::default() });
    runtime.frame(0.0);
    runtime.frame(0.0);
    assert!(runtime.state.browser_menu().is_none(), "Tab closes the menu (A-08)");
    assert_eq!(focused_key(&runtime).as_deref(), Some(crate::browser::BROWSER_TREE_KEY));
}

thread_local! {
    static OPENED: std::cell::RefCell<Vec<crate::windows::WindowOpenRequest>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

fn recording_opener(
    _: &std::rc::Rc<crate::shared::AppShared>,
    request: crate::windows::WindowOpenRequest,
) -> Result<crate::windows::OpenedWindow, String> {
    OPENED.with(|opened| opened.borrow_mut().push(request));
    Ok(crate::windows::OpenedWindow { id: lmdv_core::WindowId(0) })
}

fn recorded_windows() -> Vec<crate::windows::WindowOpenRequest> {
    OPENED.with(|opened| opened.borrow().clone())
}

fn focused_key(runtime: &rune::Runtime<WindowState>) -> Option<String> {
    runtime.elements().into_iter().find(|element| element.focused).and_then(|element| element.key)
}

fn wait_for_clipboard(runtime: &mut rune::Runtime<WindowState>, board: &rune::clipboard::MemoryClipboard) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while board.text().is_none() {
        assert!(std::time::Instant::now() < deadline, "the copy never reached the clipboard");
        runtime.frame(0.0);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    board.text().map(|text| text.to_string()).unwrap_or_default()
}

#[test]
fn the_menus_keys_move_wrap_and_skip_disabled_rows() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let root = temp.path().join("notes");
    let file = write(&root, "a.md", "a");
    show(&mut state, listing(&root));
    state.browser.focus = Some(file.to_string_lossy().into_owned());
    state.left.set_open(true);

    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.update(|state: &mut WindowState, tx: &mut rune::Tx| {
        state.show_browser_menu_keyboard();
        state.focus_browser_menu(tx, true);
    });
    runtime.frame(0.0);
    runtime.frame(0.0);
    assert_eq!(focused_key(&runtime).as_deref(), Some("browser-menu-item0"), "the first row opens focused");

    fn key(runtime: &mut rune::Runtime<WindowState>, key: rune::Key) {
        runtime.key(rune::KeyEvent { key, mods: rune::Mods::default() });
        runtime.frame(0.0);
    }
    key(&mut runtime, rune::Key::Down);
    assert_eq!(focused_key(&runtime).as_deref(), Some("browser-menu-item1"));
    key(&mut runtime, rune::Key::Down);
    assert_eq!(focused_key(&runtime).as_deref(), Some("browser-menu-item3"), "the separator is not a stop");
    key(&mut runtime, rune::Key::End);
    assert_eq!(focused_key(&runtime).as_deref(), Some("browser-menu-item5"));
    key(&mut runtime, rune::Key::Down);
    assert_eq!(focused_key(&runtime).as_deref(), Some("browser-menu-item0"), "Down wraps at the end");
    key(&mut runtime, rune::Key::Up);
    assert_eq!(focused_key(&runtime).as_deref(), Some("browser-menu-item5"), "Up wraps at the start");
    key(&mut runtime, rune::Key::Home);
    assert_eq!(focused_key(&runtime).as_deref(), Some("browser-menu-item0"));

    // Enter presses the focused row natively: the background open runs.
    let before = runtime.state.tabs().len();
    key(&mut runtime, rune::Key::Enter);
    runtime.frame(0.0);
    assert!(runtime.state.browser_menu().is_none(), "a chosen row closes the menu");
    assert_eq!(runtime.state.tabs().len(), before, "the document was already open: no duplicate tab");
}

#[test]
fn a_disabled_first_row_gives_the_keyboard_to_the_next_one() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.show_browser_menu(PathBuf::from("/"), true, rune::PointerPress { x: 40.0, y: 80.0 });
    let entries = crate::browser::browser_menu_entries(state.browser_menu().expect("the menu"));
    assert_eq!(
        crate::browser::browser_menu_first_item(&entries).as_deref(),
        Some("browser-menu-item1"),
        "Show in folder is disabled at /; Copy path takes the focus"
    );
    state.close_browser_menu();
}

#[test]
fn the_menu_key_opens_it_too_and_an_outside_press_closes_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let root = temp.path().join("notes");
    let file = write(&root, "a.md", "a");
    show(&mut state, listing(&root));
    state.browser.focus = Some(file.to_string_lossy().into_owned());
    state.left.set_open(true);

    let mut runtime = rune::Runtime::new(state, view, (1100.0, 760.0), rune::Env::new(false, 2.0));
    runtime.frame(0.0);
    runtime.update(|_: &mut WindowState, tx: &mut rune::Tx| tx.focus(crate::browser::BROWSER_TREE_KEY, false));
    runtime.frame(0.0);

    // The Menu key is the same A-08 opening as Shift+F10.
    runtime.key(rune::KeyEvent { key: rune::Key::ContextMenu, mods: rune::Mods::default() });
    runtime.frame(0.0);
    runtime.frame(0.0);
    assert!(runtime.state.browser_menu().is_some(), "the Menu key opens the row's menu");

    // A press outside closes it without choosing anything.
    runtime.pointer_down(900.0, 700.0);
    runtime.frame(0.0);
    runtime.pointer_up(900.0, 700.0);
    runtime.frame(0.0);
    runtime.frame(0.0);
    assert!(runtime.state.browser_menu().is_none(), "an outside press closes it");
    assert_eq!(runtime.state.tabs().len(), 1, "and runs nothing");
}
