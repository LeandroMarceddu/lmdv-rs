//! Share menu, export job, PDF fonts and export-naming tests (P5-04a).

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use lmdv_core::{Document, DocumentKey, ReaderFont, SourceBuffer};
use rune::{ElementInfo, Key, KeyEvent, Mods, Tx};
use tempfile::TempDir;

use super::*;
use crate::commands::lookup_command;
use crate::share::{
    ExportJob, PdfExport, RichCopy, RichCopyJob, SHARE_ITEMS, export_title, is_print_temp, medium_date_at, pdf_export,
    pdf_export_at, print_temp_path, remove_print_temp,
};
use crate::test_support;
use crate::window::{Notice, NoticeAction, NoticeKind, WindowState, title_for_path};

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

    // OI-104: the header date comes from the local-time formatter, not a
    // separate UTC path.
    let at = 1_794_612_600; // 2026-11-13, 23:30 UTC
    assert_eq!(pdf_export_at(&state, at).date, medium_date(at, lmdv_i18n::locale()));
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
    let pdf = PdfExport { date: medium_date_at(1_794_614_400, 0, lmdv_i18n::locale()), break_on_h1: true };

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
    let action = state.ui.notice.as_ref().and_then(|notice| notice.action.clone());
    assert_eq!(
        action,
        Some(NoticeAction::ShowInFolder(std::fs::canonicalize(&target).expect("the written file"))),
        "X-05's Show in folder carries the exported file, not just its folder"
    );
}

#[test]
fn the_pdf_success_notice_reveals_the_exported_file() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("notes", "# Notes\n");
    let target = export_target(&temp, "notes.pdf");
    let pdf = PdfExport { date: "Nov 14, 2026".to_owned(), break_on_h1: false };
    state.start_export(ExportFormat::Pdf, target.clone(), Some(pdf), false);
    wait_for_export(&mut state);

    let revealed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    state.shared().set_external_sink({
        let revealed = revealed.clone();
        Box::new(move |path| revealed.borrow_mut().push(path.to_path_buf()))
    });
    state.run_notice_action();

    let canonical = std::fs::canonicalize(&target).expect("the written file");
    assert_eq!(revealed.borrow().len(), 1, "one reveal request");
    assert_eq!(revealed.borrow()[0], canonical, "Show in folder reveals the file, selecting it (OI-104)");
    assert!(state.ui.notice.is_none(), "the notice closes when its action runs");
}

#[test]
fn a_failed_pdf_export_offers_the_print_dialog() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("notes", "# Notes\n");
    // X-15 refuses a destination whose folder does not exist, so the worker
    // fails exactly like a full disk or a Typst error would (X-05).
    let target = temp.path().join("missing").join("notes.pdf");
    let pdf = PdfExport { date: "Nov 14, 2026".to_owned(), break_on_h1: false };

    state.start_export(ExportFormat::Pdf, target, Some(pdf), false);
    wait_for_export(&mut state);

    let notice = state.ui.notice.as_ref().expect("the failure is shown");
    assert_eq!(notice.kind, NoticeKind::Error);
    let prefix = lmdv_i18n::t!("msg.pdfFailed", "err" => "");
    assert!(notice.text.starts_with(prefix.trim_end()), "{}", notice.text);
    assert_eq!(notice.action, Some(NoticeAction::UsePrintDialog), "X-05's Use the print dialog");
    assert_eq!(notice.action.as_ref().expect("the action").label(), lmdv_i18n::t!("msg.usePrintDialog"));
    assert!(!state.share.flashing(), "a failure shows no success check");
}

#[test]
fn the_print_dialog_action_asks_for_a_print_and_refuses_while_one_runs() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("notes", "# Notes\n");
    let doc = state.active_document_id().expect("the document");
    state.ui.notice = Some(Notice {
        text: String::new(),
        deadline: 0.0,
        action: Some(NoticeAction::UsePrintDialog),
        kind: NoticeKind::Error,
        seq: 1,
    });

    // One export at a time (X-05): a second request is refused with the busy
    // notice and leaves the first job alone, so no viewer is ever started here.
    state.share.in_flight = Some(ExportJob { job: 9, doc, format: ExportFormat::Pdf, open_viewer: true });
    state.run_notice_action();

    assert_eq!(
        state.ui.notice.as_ref().map(|notice| notice.text.as_str()),
        Some(lmdv_i18n::t!("msg.exportBusy")),
        "the action reached Print…, which refuses while an export runs"
    );
    assert_eq!(state.share.in_flight.as_ref().map(|job| job.job), Some(9), "the running job is untouched");
}

#[test]
fn a_printed_pdf_is_removed_but_the_users_export_is_kept() {
    let temp = TempDir::new().unwrap();
    let exported = export_target(&temp, "notes.pdf");
    std::fs::write(&exported, b"%PDF-").unwrap();
    assert!(!remove_print_temp(&exported).expect("the export is left alone"));
    assert!(exported.exists(), "a file the user saved is never removed");

    let printed = print_temp_path();
    std::fs::write(&printed, b"%PDF-").unwrap();
    assert!(is_print_temp(&printed), "Print…'s own temporary: {}", printed.display());
    assert!(remove_print_temp(&printed).expect("the temp is removed"));
    assert!(!printed.exists(), "the print temporary is gone after the viewer had it");
    assert!(remove_print_temp(&printed).expect("removing it again is not an error"));
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
fn one_export_policy_covers_html_docx_and_pdf() {
    // X-15's spec lets PDF create its parent folders; this app deliberately
    // uses the stricter HTML/DOCX rule for every format (see `export_io`'s
    // header). A save dialog never asks for a missing folder, so the only
    // arguments are the headless seam and Print…'s temp, and neither should
    // create a tree behind the user's back.
    let temp = TempDir::new().unwrap();
    for name in ["notes.html", "notes.docx", "notes.pdf"] {
        // A new file in an existing folder is accepted for every format.
        let target = temp.path().join(name);
        let written = crate::export_io::write_export(&target, b"payload")
            .unwrap_or_else(|error| panic!("{name} in an existing folder: {error}"));
        assert_eq!(std::fs::read(&written).unwrap(), b"payload");
        std::fs::remove_file(&written).unwrap();

        // A missing parent is refused, and remains missing.
        let missing = temp.path().join("missing").join(name);
        let error = crate::export_io::write_export(&missing, b"payload").expect_err(name);
        assert!(error.contains("cannot be used"), "{name}: {error}");
        assert!(!missing.exists(), "{name}: no file");
        assert!(!temp.path().join("missing").exists(), "{name}: X-15's stricter rule never creates the folder");
    }
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
    assert_eq!(medium_date_at(at, 0, lmdv_i18n::find_locale("en").unwrap()), "Nov 14, 2026");
    assert_eq!(medium_date_at(at, 0, lmdv_i18n::find_locale("it").unwrap()), "14 nov 2026");
    assert_eq!(medium_date_at(at, 0, lmdv_i18n::find_locale("de").unwrap()), "14.11.2026");
    assert_eq!(medium_date_at(at, 0, lmdv_i18n::find_locale("zh-CN").unwrap()), "2026年11月14日");
}

#[test]
fn the_medium_date_follows_the_local_day_across_midnight() {
    let en = lmdv_i18n::find_locale("en").unwrap();
    let it = lmdv_i18n::find_locale("it").unwrap();
    // 2026-11-13, 23:30 UTC: +00:30 is already the 14th locally, while UTC and
    // a western zone still say the 13th (X-14, OI-104).
    let at = 1_794_612_600;
    assert_eq!(medium_date_at(at, 0, en), "Nov 13, 2026");
    assert_eq!(medium_date_at(at, 30 * 60, en), "Nov 14, 2026");
    assert_eq!(medium_date_at(at, 30 * 60, it), "14 nov 2026");
    assert_eq!(medium_date_at(at, -4 * 3_600, en), "Nov 13, 2026", "a western zone is still on the 13th");

    // The same instant at UTC midnight names the previous day in the Americas.
    let midnight = 1_794_614_400;
    assert_eq!(medium_date_at(midnight, 0, en), "Nov 14, 2026");
    assert_eq!(medium_date_at(midnight, -4 * 3_600, en), "Nov 13, 2026");
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

/// Rune delivers a `copy_then`/`copy_rich_then` ack on a later frame, after
/// the clipboard write itself (OI-108).
fn wait_for_notice(runtime: &mut rune::Runtime<WindowState>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while runtime.state.ui.notice.is_none() {
        assert!(Instant::now() < deadline, "no clipboard notice arrived");
        runtime.frame(0.0);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_share_flash(runtime: &mut rune::Runtime<WindowState>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !runtime.state.share.flashing() {
        assert!(Instant::now() < deadline, "the clipboard ack never flashed the Share check");
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

    // OI-108: the check and the notice are earned by the clipboard's ack, not
    // by the export job finishing. Both land on a frame after the write.
    wait_for_share_flash(&mut runtime);
    wait_for_notice(&mut runtime);
    assert!(!runtime.state.share.copy_busy());
    let notice = runtime.state.ui.notice.as_ref().expect("a localized success notice");
    assert_eq!(notice.kind, NoticeKind::Success);
    assert_eq!(notice.text, "Copied the document as rich text.");
    eprintln!("clipboard-ack success notice: {}", notice.text);
}

#[test]
fn a_selection_copy_writes_the_selected_blocks_as_rich_text() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let markdown = "# Title\n\nOpening **bold** and [link](https://example.com).\n";
    state.open_synthetic("Notes", markdown);
    let (mut runtime, board) = clipboard_runtime(state);
    runtime.on_key = Some(crate::app::on_key);
    runtime.frame(0.0);

    // “Opening bold and link” inside the paragraph; the heading and the
    // final period stay out of the plain form.
    runtime.update(|_, tx| tx.select_text("b1-text-0", 0..21));
    runtime.frame(0.0);
    runtime.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ctrl: true, ..Mods::default() } });
    wait_for_clipboard(&mut runtime, &board);

    let copied = board.content().expect("clipboard content");
    assert_eq!(copied.text(), "Opening bold and link", "R-26: the plain form is the selection as drawn");
    let html = copied.html().expect("the HTML form");
    assert!(html.starts_with("<div role=\"document\" style="), "X-03: no document label: {html:.120}");
    assert!(html.contains("<strong style=\"font-weight:600\">bold</strong>"), "{html}");
    assert!(
        html.contains("<a href=\"https://example.com\" style=\"color:#0969da;text-decoration:underline\">link</a>"),
        "{html}"
    );
    assert!(html.contains("Opening"), "the selected block is copied");
    assert!(!html.contains("Title"), "the unselected heading is not");
    assert!(!html.contains("<style") && !html.contains("<script"), "inline styles only");

    runtime.frame(0.0); // let the `copy_rich_then` ack land
    assert!(!runtime.state.share.copy_busy());
    assert!(!runtime.state.share.flashing(), "Ctrl+C is not the Share action (X-13)");
    assert!(runtime.state.ui.notice.is_none(), "an ordinary copy is quiet");
}

#[test]
fn copy_without_a_selection_does_not_start_a_rich_job() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Notes", "# Title\n\nOpening paragraph.\n");
    let (mut runtime, board) = clipboard_runtime(state);
    runtime.on_key = Some(crate::app::on_key);
    runtime.frame(0.0);

    // R-26: only a selection copies as rich text; Cmd/Ctrl+C with none is
    // Rune's own path and must not start a job.
    runtime.key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ctrl: true, ..Mods::default() } });
    runtime.frame(0.0);
    assert_eq!(board.content(), None, "nothing was selected and nothing was copied");
    assert!(!runtime.state.share.copy_busy(), "no copy job was started");
}

#[test]
fn a_selection_with_no_export_block_falls_back_to_plain_text() {
    // Defensive X-03 path: a source range that touches no export block (an
    // empty range, a separator) builds no fragment, and the clipboard carries
    // the selection as plain text instead.
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Notes", "# Title\n\nOpening paragraph.\n");
    let document = state.active_tab().and_then(|tab| tab.document()).cloned().expect("a document");
    let spec = crate::share::RichCopySpec {
        doc: document.id,
        source: document.snapshot(),
        revision: document.revision(),
        model: None,
        title_base: "Notes".to_owned(),
        options: lmdv_export::ExportOptions { lang: "en".to_owned(), ..lmdv_export::ExportOptions::default() },
        selection: Some(crate::share::SelectionCopy { plain: "No block".to_owned(), source: 0..0 }),
    };
    let copy = crate::export_io::run_rich_copy(&spec).expect("the worker");
    assert!(copy.html.is_empty(), "no export block: no HTML form");
    assert_eq!(copy.markdown, "No block", "the plain selection rides along");

    let (mut runtime, board) = clipboard_runtime(state);
    runtime.update(|state: &mut WindowState, tx: &mut Tx| {
        state.share.copy_in_flight = Some(RichCopyJob { job: 77, doc: document.id, selection: true });
        assert!(state.handle_rich_copied(77, document.id, Ok(copy.clone()), tx));
    });
    runtime.frame(0.0);
    let copied = board.content().expect("the fallback reached the clipboard");
    assert!(copied.html().is_none(), "plain only");
    assert_eq!(copied.text(), "No block");
    runtime.frame(0.0); // the `copy_then` ack lands on a later frame
    assert!(runtime.state.ui.notice.is_none(), "the fallback is quiet");
}

#[test]
fn ctrl_shift_c_still_copies_the_whole_document() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let markdown = "# Title\n\nOpening paragraph.\n";
    state.open_synthetic("Notes", markdown);
    let (mut runtime, board) = clipboard_runtime(state);
    runtime.on_key = Some(crate::app::on_key);
    runtime.frame(0.0);
    runtime.update(|_, tx| tx.select_text("b1-text-0", 0..7));
    runtime.frame(0.0);
    runtime
        .key(KeyEvent { key: Key::Command('c'), mods: Mods { cmd: true, ctrl: true, shift: true, ..Mods::default() } });
    wait_for_clipboard(&mut runtime, &board);
    let copied = board.content().expect("clipboard content");
    assert_eq!(copied.text(), markdown, "Ctrl+Shift+C keeps the whole-document rich copy (X-02)");
    assert!(
        copied.html().is_some_and(|html| html.contains("aria-label=\"Title\"")),
        "named after the document's first h1"
    );
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
fn a_mermaid_source_becomes_a_safe_static_svg() {
    let svg = crate::export_io::diagram_svg("flowchart LR\n    A[Start] --> B[Done]").expect("the diagram draws");
    let svg = String::from_utf8(svg).expect("UTF-8 SVG");
    assert!(svg.starts_with("<svg "), "{svg:.120}");
    assert!(svg.contains("<path "), "the canvas is paths");
    assert!(svg.contains(">Start</text>"), "the labels are text");
    for forbidden in ["<script", "<image", "href=", "url(", "@import"] {
        assert!(!svg.contains(forbidden), "the export SVG carries {forbidden}");
    }
    assert!(crate::export_io::diagram_svg("this is not a diagram").is_none());
}

#[test]
fn rich_copy_embeds_a_mermaid_diagram_as_an_image() {
    // OI-106/X-02: the rich clipboard fragment carries a drawing, not the
    // Mermaid syntax the fragment used to fall back to.
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let markdown = "# Diagram\n\n```mermaid\nflowchart LR\n    A[Source] --> B[Done]\n```\n";
    state.open_synthetic("Diagram", markdown);
    let (mut runtime, board) = clipboard_runtime(state);
    runtime.update(|state: &mut WindowState, tx: &mut Tx| {
        assert!(state.run_command("share.copy-rich", tx), "rich copy is available");
    });
    wait_for_clipboard(&mut runtime, &board);

    let html = board.content().and_then(|content| content.html().map(str::to_owned)).expect("HTML form");
    assert!(html.contains("data:image/svg+xml;base64,"), "the diagram is an embedded image: {html:.400}");
    assert!(html.contains("alt=\"Mermaid diagram\""), "the picture keeps the localized alt text");
    assert!(!html.contains("flowchart LR"), "the diagram source is not in the rich copy");
    assert!(!runtime.state.share.copy_busy());
}

#[test]
fn a_docx_export_embeds_a_mermaid_diagram_as_a_png() {
    // OI-106/X-08: DOCX rasterizes the static SVG through the same media path
    // as an SVG picture, so Word gets a PNG drawing instead of a code block.
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Diagram", "# Diagram\n\n```mermaid\nflowchart LR\n    A[Source] --> B[Done]\n```\n");
    wait_for_reader(&mut state);
    let target = export_target(&temp, "diagram.docx");
    state.start_export(ExportFormat::Docx, target.clone(), None, false);
    wait_for_export(&mut state);

    let bytes = std::fs::read(&target).expect("the exported DOCX");
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("word/media/image1.png"), "the diagram is a PNG media part");
    assert!(
        bytes.windows(8).any(|window| window == b"\x89PNG\r\n\x1a\n"),
        "the stored media entry is a real PNG (pictures are stored, not deflated)"
    );
}

#[test]
fn a_failed_rich_copy_shows_an_error_notice_and_no_flash() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Notes", "# Notes\n");
    let doc = state.active_document_id().expect("document");
    state.share.copy_in_flight = Some(RichCopyJob { job: 41, doc, selection: false });
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
fn a_rejected_clipboard_write_shows_an_error_notice_and_no_flash() {
    // OI-108: a job that finished is not proof the OS write landed. The memory
    // clipboard can refuse a write exactly as a locked OS clipboard would.
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Notes", "# Notes\n\nBody.\n");
    let (mut runtime, board) = clipboard_runtime(state);
    board.fail_writes(Some("clipboard busy"));

    runtime.update(|state: &mut WindowState, tx: &mut Tx| {
        assert!(state.run_command("share.copy-rich", tx), "the command is enabled");
    });
    wait_for_notice(&mut runtime);

    let notice = runtime.state.ui.notice.as_ref().expect("the failure is shown");
    assert_eq!(notice.kind, NoticeKind::Error);
    assert!(notice.text.starts_with("Copy failed:"), "{}", notice.text);
    assert!(notice.text.contains("clipboard busy"), "{}", notice.text);
    eprintln!("clipboard-ack failure notice: {}", notice.text);
    assert!(!runtime.state.share.flashing(), "a refused write earns no success check");
    assert!(!runtime.state.share.copy_busy());
    assert!(board.content().is_none(), "a refused write leaves the clipboard untouched");
    board.fail_writes(None);
}

#[test]
fn a_superseded_clipboard_write_shows_an_error_notice() {
    // Two writes asked in one handler: Rune supersedes the first callback with
    // `CopyError::Superseded`. The selection copy is quiet when it succeeds, so
    // the only notice left is the superseded one (OI-108).
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Notes", "# Notes\n");
    let doc = state.active_document_id().expect("the document");
    let (mut runtime, board) = clipboard_runtime(state);

    runtime.update(|state: &mut WindowState, tx: &mut Tx| {
        state.share.copy_in_flight = Some(RichCopyJob { job: 81, doc, selection: true });
        assert!(state.handle_rich_copied(
            81,
            doc,
            Ok(RichCopy { markdown: "first".to_owned(), html: "<b>first</b>".to_owned() }),
            tx
        ));
        state.share.copy_in_flight = Some(RichCopyJob { job: 82, doc, selection: true });
        assert!(state.handle_rich_copied(
            82,
            doc,
            Ok(RichCopy { markdown: "second".to_owned(), html: "<b>second</b>".to_owned() }),
            tx
        ));
    });
    wait_for_notice(&mut runtime);

    let notice = runtime.state.ui.notice.as_ref().expect("the superseded write is reported");
    assert_eq!(notice.kind, NoticeKind::Error);
    assert!(notice.text.contains("superseded"), "{}", notice.text);
    eprintln!("clipboard-ack superseded notice: {}", notice.text);
    assert!(!runtime.state.share.flashing(), "no success check for a superseded write");
    assert_eq!(board.content().expect("the newer write landed").text(), "second");
}

#[test]
fn an_edit_typed_just_before_share_is_exported() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("notes", "# First\n\nOld body.\n");
    wait_for_reader(&mut state);
    let mut tx = Tx::default();
    assert!(state.enter_editor(None, &mut tx), "edit mode");
    // Type at the end and do not let the 120 ms preview debounce run: the
    // reader still holds the model parsed before the edit (OI-103).
    let end = state.active_editor_edit().text.len();
    state.active_editor_edit().set_caret(end);
    state.active_editor_edit().insert("\n\nEdited just before Share.\n");
    let document = state.active_tab().and_then(|tab| tab.document()).cloned().expect("the document");
    assert!(!document.text().text().contains("Edited just before Share."), "the field text is not in the buffer yet");
    let stale = state.active_tab().and_then(|tab| tab.view.reader.clone()).expect("the pre-edit model");
    assert!(!stale.source.contains("Edited just before Share."), "the reader is still on the old model");
    let target = export_target(&temp, "fresh.html");

    state.start_export(ExportFormat::Html, target.clone(), None, false);
    wait_for_export(&mut state);

    let html = std::fs::read_to_string(&target).expect("the exported HTML");
    assert!(html.contains("Edited just before Share."), "the export parsed the current revision: {html}");
    assert!(html.contains("Old body."), "the older text is still there");
}

#[test]
fn a_rich_copy_takes_a_revision_typed_just_before_it() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Notes", "# Notes\n\nOriginal.\n");
    wait_for_reader(&mut state);
    let mut tx = Tx::default();
    assert!(state.enter_editor(None, &mut tx), "edit mode");
    let end = state.active_editor_edit().text.len();
    state.active_editor_edit().set_caret(end);
    state.active_editor_edit().insert("\n\nEdited for the clipboard.\n");
    let document = state.active_tab().and_then(|tab| tab.document()).cloned().expect("the document");
    assert!(!document.text().text().contains("Edited for the clipboard."), "the field text is not in the buffer yet");
    let stale = state.active_tab().and_then(|tab| tab.view.reader.clone()).expect("the pre-edit model");
    assert!(!stale.source.contains("Edited for the clipboard."), "the reader is still on the old model");
    let (mut runtime, board) = clipboard_runtime(state);

    runtime.update(|state: &mut WindowState, tx: &mut Tx| {
        assert!(state.run_command("share.copy-rich", tx), "the command is enabled");
    });
    wait_for_clipboard(&mut runtime, &board);

    let copied = board.content().expect("clipboard content");
    assert!(copied.text().contains("Edited for the clipboard."), "the plain form is the edited buffer");
    let html = copied.html().expect("the HTML form");
    assert!(html.contains("Edited for the clipboard."), "the HTML used the same revision: {html}");
}

#[test]
fn an_export_pins_its_document_until_the_job_ends() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let first = state.open_synthetic("First", "# First\n");
    let second = state.open_synthetic("Second", "# Second\n");
    assert!(state.activate_tab(first));
    let doc = state.active_document_id().expect("the first document");
    state.share.in_flight = Some(ExportJob { job: 77, doc, format: ExportFormat::Html, open_viewer: false });

    assert!(!state.activate_tab(second), "a tab switch away from the exporting document is refused");
    assert_eq!(state.active_tab().map(|tab| tab.id), Some(first));
    assert!(state.ui.notice.is_some(), "the refusal is visible");

    let order: Vec<_> = state.tabs.iter().map(|tab| tab.id).collect();
    assert!(!state.move_active_tab(1), "the pinned tab cannot move");
    assert_eq!(state.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(), order, "the strip is unchanged");

    assert_eq!(state.close_tab(first), crate::window::CloseOutcome::Blocked);
    assert!(state.tab(first).is_some(), "the tab is still open");

    // Opening a file or a new tab while the job runs must not leave the
    // exporting document either; the new tab joins in the background (OI-105).
    let third = state.open_synthetic("Third", "# Third\n");
    assert_eq!(state.active_tab().map(|tab| tab.id), Some(first), "an opened tab joins in the background");
    assert!(state.tab(third).is_some(), "the opened tab is in the strip");
    let blank = state.new_start_tab();
    assert_eq!(state.active_tab().map(|tab| tab.id), Some(first), "a new start page joins in the background");
    assert!(state.tab(blank).is_some(), "the start page is in the strip");

    // The worker failed (or finished): the pin is gone and everything works.
    assert!(state.handle_exported(77, doc, ExportFormat::Html, Err("the worker stopped".to_owned())));
    assert!(!state.export_pins(doc));
    assert!(state.activate_tab(second), "switching works again");
    assert_eq!(state.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(), vec![first, second, third, blank]);
    assert_eq!(state.close_tab(first), crate::window::CloseOutcome::Closed);
}

/// OI-068: a large document's export cost stays visible.
///
/// Ignored by default (it writes hundreds of KB and runs a cold PDF). Run it
/// explicitly and read the timings:
///
/// ```text
/// cargo test -p lmdv-app large_export_benchmark -- --ignored --nocapture
/// ```
#[test]
#[ignore = "performance measurement; run explicitly with --ignored --nocapture"]
fn large_export_benchmark() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    let mut source = String::with_capacity(256 * 1024);
    source.push_str("# Large export\n\n");
    for index in 0..800 {
        source.push_str(&format!(
            "## Section {index}\n\nParagraph {index} with **bold**, *emphasis*, `code`, [a link](other.md) and a plain sentence that wraps at the export column width.\n\n- first item\n- second item\n\n"
        ));
        if index % 16 == 0 {
            source.push_str(&format!("$$\nx_{index}^2 + y_{index}^2 = z_{index}^2\n$$\n\n"));
        }
    }
    state.open_synthetic("large", &source);
    wait_for_reader(&mut state);
    eprintln!("large export benchmark: {} bytes, 800 sections", source.len());

    for (format, name) in
        [(ExportFormat::Html, "large.html"), (ExportFormat::Docx, "large.docx"), (ExportFormat::Pdf, "large.pdf")]
    {
        let target = export_target(&temp, name);
        let pdf = (format == ExportFormat::Pdf).then(|| pdf_export(&state));
        let started = Instant::now();
        state.start_export(format, target.clone(), pdf, false);
        wait_for_export(&mut state);
        let elapsed = started.elapsed();
        let bytes = std::fs::metadata(&target).map(|meta| meta.len()).unwrap_or(0);
        let elapsed_ms = elapsed.as_secs_f64() * 1_000.0;
        eprintln!("{format:?}: {elapsed_ms:.2} ms, {bytes} bytes");
        assert!(bytes > 0, "{format:?} wrote a file");
        // A deliberately generous ceiling: the point is to make a regression
        // visible, not to pin one machine's speed.
        let budget_ms = if format == ExportFormat::Pdf { 60_000.0 } else { 10_000.0 };
        assert!(elapsed_ms < budget_ms, "{format:?} took {elapsed_ms:.0} ms (budget {budget_ms:.0} ms)");
    }
}

#[test]
fn scenes_list_share_once() {
    assert_eq!(crate::SCENES.iter().filter(|(name, _)| *name == "share").count(), 1);
}
