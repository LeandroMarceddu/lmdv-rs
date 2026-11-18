//! Share menu, export job, PDF fonts and export-naming tests (P5-04a).

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use lmdv_core::{Document, DocumentKey, ReaderFont, SourceBuffer};
use rune::{ElementInfo, Tx};
use tempfile::TempDir;

use super::*;
use crate::commands::lookup_command;
use crate::share::{PdfExport, RichCopyJob, SHARE_ITEMS, export_title, pdf_export};
use crate::test_support;
use crate::window::{WindowState, title_for_path};

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(test_support::shared(temp))
}

fn wait_for_reader(state: &mut WindowState) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while state.active_tab().is_none_or(|tab| tab.view.reader.is_none()) {
        assert!(Instant::now() < deadline, "the document never parsed");
        let mut tx = Tx::default();
        state.poll(0.0, &mut tx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_export(state: &mut WindowState) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while state.share.in_flight.is_some() {
        assert!(Instant::now() < deadline, "the export job never finished");
        let mut tx = Tx::default();
        state.poll(0.0, &mut tx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn export_target(temp: &TempDir, name: &str) -> PathBuf {
    temp.path().join(name)
}

/// The first menu row whose command is enabled now.
fn first_enabled_index(state: &WindowState) -> Option<usize> {
    SHARE_ITEMS.iter().position(|item| lookup_command(item.command).is_some_and(|command| (command.enabled)(state)))
}

#[test]
fn the_share_menu_needs_a_document() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let mut tx = Tx::default();

    assert!(!state.run_command("share.menu", &mut tx), "a start page has nothing to share");
    assert!(!state.share.menu_open);
    assert!(!(lookup_command("share.menu").expect("command").enabled)(&state));
    assert!(!(lookup_command("share.print").expect("command").enabled)(&state));
    assert!(!(lookup_command("share.copy-rich").expect("command").enabled)(&state));
    assert!(!(lookup_command("share.menu").expect("command").checked.expect("checked"))(&state));
}

#[test]
fn the_toolbar_share_button_is_disabled_without_a_document() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::view::scene_start(&mut state);
    let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(true, 2.0));
    runtime.frame(0.0);
    let share = runtime
        .elements()
        .into_iter()
        .find(|element| element.key.as_deref() == Some(crate::share::TOOLBAR_KEY))
        .expect("the toolbar Share button");
    assert!(!share.focusable, "a start page has nothing to share");
}

#[test]
fn the_menu_lists_the_six_items_in_order_with_rich_copy_enabled() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Notes", "# Notes\n");
    let mut tx = Tx::default();

    assert!(state.run_command("share.menu", &mut tx));
    assert!(state.share.menu_open);
    let commands: Vec<&str> = SHARE_ITEMS.iter().map(|item| item.command).collect();
    assert_eq!(
        commands,
        [
            "share.copy-rich",
            "share.print",
            "share.pdf",
            "share.toggle-break-h1",
            "share.export-html",
            "share.export-docx",
        ]
    );
    assert_eq!(first_enabled_index(&state), Some(0), "rich copy is the first row and is available");
    assert!((lookup_command("share.copy-rich").expect("command").enabled)(&state));
    assert_eq!(lookup_command("share.copy-rich").expect("command").default_keys, &["Ctrl+Shift+C"]);

    assert!(state.run_command("share.menu", &mut tx), "the shortcut toggles it closed");
    assert!(!state.share.menu_open);
}

#[test]
fn the_h1_break_setting_persists_and_keeps_the_menu_open() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Notes", "# Notes\n");
    assert!(state.run_command("share.menu", &mut Tx::default()));
    assert!(!state.shared().prefs().print_break_h1);

    assert!(state.run_command("share.toggle-break-h1", &mut Tx::default()));
    assert!(state.shared().prefs().print_break_h1);
    assert!(state.share.menu_open, "a setting does not close the menu (X-06)");
    let checked = lookup_command("share.toggle-break-h1").expect("command").checked.expect("checked");
    assert!(checked(&state));

    assert!(state.run_command("share.toggle-break-h1", &mut Tx::default()));
    assert!(!state.shared().prefs().print_break_h1);
    assert!(!checked(&state));
}

#[test]
fn a_html_export_writes_one_self_contained_file() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic(
        "notes",
        "---\ntitle: Hidden\n---\n# Title\n\nBody with $x^2$.\n\n## Section\n\n| A | B |\n| - | - |\n| 1 | 2 |\n",
    );
    let mut prefs = state.shared().prefs().clone();
    prefs.reader.size = 22;
    prefs.reader.font = ReaderFont::Serif;
    state.shared().set_prefs(prefs);
    let target = export_target(&temp, "notes.html");

    state.start_export(ExportFormat::Html, target.clone(), None, false);
    wait_for_export(&mut state);

    let html = std::fs::read_to_string(&target).expect("the exported HTML");
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("<html lang=\"en\">"));
    assert!(html.contains("html { font-size: 16px; }"), "the fixed export layout, not the 22 pt reader");
    assert!(!html.contains("22px"), "the reader's size never leaks into the export (X-10)");
    assert!(!html.contains("Hidden"), "the front matter is omitted");
    assert!(!html.contains("<script"), "the exported page runs no script");
    assert!(state.share.flashing(), "the Share button shows its check (X-13)");
    assert!(state.ui.notice.is_some(), "the success notice names the file");
    let notice = &state.ui.notice.as_ref().expect("notice").text;
    assert!(notice.contains("notes.html"), "{notice}");
}

#[test]
fn pdf_settings_take_the_h1_break_from_prefs_and_the_localized_date() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("notes", "# Notes\n");
    let mut prefs = state.shared().prefs().clone();
    prefs.print_break_h1 = true;
    state.shared().set_prefs(prefs);
    let pdf = pdf_export(&state);
    assert!(pdf.break_on_h1);
    assert!(pdf.date.contains("20"), "the medium date: {}", pdf.date);
}

#[test]
fn a_docx_export_writes_a_non_empty_package() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("notes", "# Title\n\nBody.\n\n- [x] done\n");
    let target = export_target(&temp, "notes.docx");

    state.start_export(ExportFormat::Docx, target.clone(), None, false);
    wait_for_export(&mut state);

    let bytes = std::fs::read(&target).expect("the exported DOCX");
    assert!(bytes.len() > 1_000);
    assert!(bytes.starts_with(b"PK\x03\x04"), "a ZIP package");
}

#[test]
fn a_pdf_export_writes_an_a4_document_with_the_h1_break_from_prefs() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("notes", "# One\n\nBody.\n\n# Two\n\nMore body.\n");
    let mut prefs = state.shared().prefs().clone();
    prefs.print_break_h1 = true;
    state.shared().set_prefs(prefs);
    let target = export_target(&temp, "notes.pdf");
    let pdf = PdfExport { date: medium_date(1_794_614_400, lmdv_i18n::locale()), break_on_h1: true };

    state.start_export(ExportFormat::Pdf, target.clone(), Some(pdf), false);
    wait_for_export(&mut state);

    let bytes = std::fs::read(&target).expect("the exported PDF");
    assert!(bytes.starts_with(b"%PDF-"));
    assert!(bytes.len() > 5_000);
    assert!(bytes.windows(5).any(|window| window == b"%%EOF"));
    // A4 is 595.28 × 841.89 pt; Typst writes the MediaBox it used. The two
    // level-1 headings with `break_on_h1` are two pages (X-06).
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("MediaBox[0 0 595.2756 841.8898]"), "A4 page size");
    assert!(text.contains("/Count 2"), "two H1s, one page each");
    assert!(state.share.flashing());
    assert!(state.ui.notice.as_ref().and_then(|notice| notice.action.clone()).is_some(), "Show in folder");
}

#[test]
fn a_local_picture_is_embedded_through_the_path_policy() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("docs");
    std::fs::create_dir_all(&root).unwrap();
    let mut file = std::fs::File::create(root.join("notes.md")).unwrap();
    write!(file, "# Picture\n\n![pixel](pic.png)\n\nBody.\n").unwrap();
    file.flush().unwrap();
    std::fs::write(root.join("pic.png"), b"\x89PNG\r\n\x1a\nnot-a-real-image").unwrap();
    // Outside the document folder: the policy must refuse it.
    std::fs::write(temp.path().join("secret.png"), b"\x89PNG\r\n\x1a\nsecret").unwrap();

    let mut state = window(&temp);
    state.open_path(root.join("notes.md"));
    wait_for_reader(&mut state);
    let target = export_target(&temp, "picture.html");

    state.start_export(ExportFormat::Html, target.clone(), None, false);
    wait_for_export(&mut state);

    let html = std::fs::read_to_string(&target).expect("the exported HTML");
    assert!(html.contains("data:image/png;base64,"), "the picture became a data URI");
    assert!(!html.contains("secret"), "a picture outside the document folder was refused");
}

#[test]
fn a_missing_parent_folder_is_an_error_not_a_created_folder() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("notes", "# Notes\n");
    let target = temp.path().join("missing").join("notes.html");

    state.start_export(ExportFormat::Html, target.clone(), None, false);
    wait_for_export(&mut state);

    assert!(!target.exists(), "X-15 does not create the destination folder");
    assert!(!state.share.flashing());
    let notice = &state.ui.notice.as_ref().expect("the failure is shown").text;
    assert!(notice.contains("Export failed"), "{notice}");
}

#[test]
fn export_names_follow_the_source_stem_and_the_pasted_title() {
    let temp = TempDir::new().unwrap();
    let shared = test_support::shared(&temp);
    let id = shared.docs_mut().allocate_id();
    let path = PathBuf::from("/tmp/lmdv-tests/notes.md");
    let file_doc =
        Document::ready(id, DocumentKey::File(path.clone()), title_for_path(&path), SourceBuffer::new("text"));
    assert_eq!(export_base(&file_doc), "notes");

    let mut state = WindowState::new(shared);
    state.open_synthetic("Q1/Q2", "# Q1/Q2\n");
    let pasted = state.active_tab().and_then(|tab| tab.document()).cloned().expect("pasted document");
    assert_eq!(export_base(&pasted), "Q1 Q2");
    assert_eq!(file_safe_name("CON"), "con_");
    assert_eq!(file_safe_name("  a  b.  "), "a b");
    assert_eq!(file_safe_name("bad/name:with*chars?"), "bad name with chars");
}

#[test]
fn export_title_prefers_the_first_h1_then_the_source_stem() {
    let temp = TempDir::new().unwrap();
    let shared = test_support::shared(&temp);
    let id = shared.docs_mut().allocate_id();
    let path = PathBuf::from("/tmp/lmdv-tests/notes.md");
    let doc = Document::ready(id, DocumentKey::File(path.clone()), title_for_path(&path), SourceBuffer::new("text"));

    let with_h1 = lmdv_md::parse("# Hello world\n\nBody\n");
    assert_eq!(export_title(&doc, &with_h1).as_deref(), Some("Hello world"));

    let no_h1 = lmdv_md::parse("Body only\n");
    assert_eq!(export_title(&doc, &no_h1).as_deref(), Some("notes"));
}

#[test]
fn the_pdf_fonts_are_cached_and_come_from_the_system() {
    let fonts = pdf_fonts();
    assert!(std::ptr::eq(fonts, pdf_fonts()), "the faces are loaded once per process");
    assert!(!fonts.note.is_empty());
    if fonts.fonts.is_empty() {
        assert!(fonts.note.contains("bundled"), "a documented fallback: {}", fonts.note);
    } else {
        assert!(fonts.fonts.iter().any(|font| !font.bytes.is_empty()));
        if !fonts.body_family.is_empty() {
            let candidates = ["Noto Sans", "DejaVu Sans", "Cantarell", "Liberation Sans"];
            assert!(candidates.contains(&fonts.body_family.as_str()), "unexpected body face {}", fonts.body_family);
        }
    }
}

#[test]
fn the_medium_date_is_localized() {
    let at = 1_794_614_400; // 2026-11-14, 00:00 UTC
    assert_eq!(medium_date(at, lmdv_i18n::find_locale("en").unwrap()), "Nov 14, 2026");
    assert_eq!(medium_date(at, lmdv_i18n::find_locale("it").unwrap()), "14 nov 2026");
    assert_eq!(medium_date(at, lmdv_i18n::find_locale("de").unwrap()), "14.11.2026");
    assert_eq!(medium_date(at, lmdv_i18n::find_locale("zh-CN").unwrap()), "2026年11月14日");
}

#[test]
fn the_menu_keyboard_moves_focus_and_presses_the_h1_setting() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("notes", "# Notes\n");
    state.open_share_menu();
    let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(true, 2.0));
    runtime.frame(0.0);
    let focused = |runtime: &rune::Runtime<WindowState>| -> Option<String> {
        runtime.elements().into_iter().find(|element| element.focused).and_then(|element| element.key)
    };
    assert_eq!(focused(&runtime).as_deref(), Some("share-item0"), "the first enabled row takes focus");
    // The first arrow reveals the hidden highlight; the next three reach the
    // checkbox (Rune's menu machine).
    for _ in 0..4 {
        runtime.key(rune::KeyEvent { key: rune::Key::Down, mods: rune::Mods::default() });
        runtime.frame(0.0);
    }
    assert_eq!(focused(&runtime).as_deref(), Some("share-item3"));
    runtime.key(rune::KeyEvent { key: rune::Key::Enter, mods: rune::Mods::default() });
    runtime.frame(0.0);
    assert!(runtime.state.shared().prefs().print_break_h1, "Return chose the checkbox");
    assert!(runtime.state.share.menu_open, "a setting keeps the menu open");
    // One more Down reaches Export as HTML; Return closes the menu before the
    // command asks for a save path.
    runtime.key(rune::KeyEvent { key: rune::Key::Down, mods: rune::Mods::default() });
    runtime.frame(0.0);
    assert_eq!(focused(&runtime).as_deref(), Some("share-item4"));
    runtime.key(rune::KeyEvent { key: rune::Key::Enter, mods: rune::Mods::default() });
    runtime.frame(0.0);
    assert!(!runtime.state.share.menu_open, "Return activated Export as HTML");
}

#[test]
fn escape_closes_the_menu_and_returns_focus_to_share() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("notes", "# Notes\n");
    state.open_share_menu();
    let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(true, 2.0));
    runtime.frame(0.0);
    runtime.key(rune::KeyEvent { key: rune::Key::Escape, mods: rune::Mods::default() });
    runtime.frame(0.0);
    assert!(!runtime.state.share.menu_open);
    let focused = runtime.elements().into_iter().find(|element| element.focused).and_then(|element| element.key);
    assert_eq!(focused.as_deref(), Some(crate::share::TOOLBAR_KEY), "focus returns to the Share button");
}

#[test]
fn the_share_scene_draws_the_open_menu() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::view::scene_share(&mut state);
    assert!(state.share.menu_open);
    assert!(state.active_tab().is_some_and(|tab| tab.document().is_some()));

    let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(true, 2.0));
    runtime.frame(0.0);
    let elements: Vec<ElementInfo> = runtime.elements();
    assert!(elements.iter().any(|element| element.role == "menu"), "the menu surface");
    let labels: Vec<&str> = elements.iter().filter_map(|element| element.label.as_deref()).collect();
    for expected in [
        "Copy as rich text",
        "Print…",
        "Save as PDF",
        "New page at every heading 1",
        "Export as HTML",
        "Export as DOCX",
    ] {
        assert!(
            labels.iter().any(|label| *label == expected || label.starts_with(expected)),
            "missing {expected}: {labels:?}"
        );
    }
    assert!(elements.iter().any(|element| element.role == "menuitemcheckbox"), "the H1 setting is a checkbox");
    let copy =
        elements.iter().find(|element| element.key.as_deref() == Some("share-item0")).expect("the rich-copy row");
    assert!(copy.focusable, "rich copy is enabled in the scene (X-02)");
    assert!(!labels.iter().any(|label| label.contains("P5-04b")), "no placeholder label remains: {labels:?}");
    assert!(
        elements.iter().any(|element| element.key.as_deref() == Some(crate::share::TOOLBAR_KEY)),
        "the Share button is in the toolbar"
    );
}

fn clipboard_runtime(state: WindowState) -> (rune::Runtime<WindowState>, rune::clipboard::MemoryClipboard) {
    let board = rune::clipboard::memory();
    let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(true, 2.0));
    runtime.use_clipboard(&board);
    runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
    runtime.frame(0.0);
    (runtime, board)
}

fn wait_for_clipboard(runtime: &mut rune::Runtime<WindowState>, board: &rune::clipboard::MemoryClipboard) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while board.content().is_none() {
        assert!(Instant::now() < deadline, "the rich copy never reached the clipboard");
        runtime.frame(0.0);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn rich_copy_puts_markdown_and_html_on_the_clipboard() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let markdown = "# Notes\n\n**Bold** and $x^2$.\n\n- [x] done\n";
    state.open_synthetic("Notes", markdown);
    let (mut runtime, board) = clipboard_runtime(state);

    runtime.update(|state: &mut WindowState, tx: &mut Tx| {
        assert!(state.run_command("share.copy-rich", tx), "the command is enabled");
    });
    wait_for_clipboard(&mut runtime, &board);

    let copied = board.content().expect("clipboard content");
    assert_eq!(copied.text(), markdown, "plain form is the Markdown source");
    let html = copied.html().expect("the HTML form");
    assert!(html.starts_with("<div role=\"document\" aria-label=\"Notes\""), "{html:.200}");
    assert!(html.contains("<strong style=\"font-weight:600\">Bold</strong>"));
    assert!(html.contains("data:image/png;base64,"), "math is embedded");
    assert!(html.contains('\u{2611}'), "tasks are visible symbols");
    assert!(!html.contains("<style"), "inline styles only");

    assert!(runtime.state.share.flashing(), "the Share button flashes (X-13)");
    assert!(!runtime.state.share.copy_busy());
    let notice = runtime.state.ui.notice.as_ref().expect("a localized success notice");
    assert_eq!(notice.text, "Copied the document as rich text.");
}

#[test]
fn rich_copy_resolves_local_pictures_through_the_path_policy() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("docs");
    std::fs::create_dir_all(&root).unwrap();
    let mut file = std::fs::File::create(root.join("notes.md")).unwrap();
    write!(file, "# Picture\n\n![pixel](pic.png)\n").unwrap();
    file.flush().unwrap();
    std::fs::write(root.join("pic.png"), b"\x89PNG\r\n\x1a\nnot-a-real-image").unwrap();
    std::fs::write(temp.path().join("secret.png"), b"\x89PNG\r\n\x1a\nsecret").unwrap();

    let mut state = window(&temp);
    state.open_path(root.join("notes.md"));
    wait_for_reader(&mut state);
    let (mut runtime, board) = clipboard_runtime(state);
    runtime.update(|state: &mut WindowState, tx: &mut Tx| {
        let _ = state.run_command("share.copy-rich", tx);
    });
    wait_for_clipboard(&mut runtime, &board);

    let html = board.content().and_then(|content| content.html().map(str::to_owned)).expect("HTML form");
    assert!(html.contains("data:image/png;base64,"), "the local picture is embedded");
    assert!(!html.contains("secret"), "a picture outside the document folder was refused");
}

#[test]
fn a_failed_rich_copy_shows_an_error_notice_and_no_flash() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Notes", "# Notes\n");
    let doc = state.active_document_id().expect("document");
    state.share.copy_in_flight = Some(RichCopyJob { job: 41, doc });
    let mut tx = Tx::default();

    assert!(state.handle_rich_copied(41, doc, Err("the worker stopped".to_owned()), &mut tx));
    assert!(!state.share.copy_busy());
    assert!(!state.share.flashing());
    let notice = state.ui.notice.as_ref().expect("the failure is shown");
    assert!(notice.text.contains("Copy failed: the worker stopped"), "{}", notice.text);
    assert!(notice.action.is_none());
    assert!(
        !state.handle_rich_copied(
            41,
            doc,
            Ok(crate::share::RichCopy { markdown: String::new(), html: String::new() }),
            &mut tx,
        ),
        "a duplicate completion changes nothing"
    );
}

#[test]
fn scenes_list_share_once() {
    assert_eq!(crate::SCENES.iter().filter(|(name, _)| *name == "share").count(), 1);
}
