//! P6-I18N-B: the embedded help pages, the HTML-subset parser and the full
//! Help dialog (F1) over the compact shortcut list (`?`).

use rune::{ElementInfo, Env, Key, KeyEvent, Mods, Runtime};
use tempfile::TempDir;

use crate::help::{
    Block, HELP_ABOUT, HELP_ABOUT_VERSION, HELP_BODY, HELP_CLOSE, HELP_KEY, HELP_LICENSE, HELP_LICENSE_URL, HELP_PANEL,
    HELP_REPOSITORY, HELP_REPOSITORY_URL, HELP_SCRIM, HELP_TITLE, HELP_VERSION, HelpPage, Inline, KeyRow, Run, page,
    parse,
};
use crate::test_support;
use crate::window::{HelpView, WindowState};

fn window(temp: &TempDir) -> WindowState {
    WindowState::new(test_support::shared(temp))
}

fn runtime(state: WindowState, size: (f32, f32)) -> Runtime<WindowState> {
    let mut runtime = Runtime::new(state, crate::view::view, size, Env::new(true, 2.0));
    // The AppSpec's hooks: a bare runtime does not run the window's poll or the
    // app's key router by itself.
    runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
    runtime.on_key = Some(crate::app::on_key);
    runtime
}

fn by_key<'a>(elements: &'a [ElementInfo], key: &str) -> &'a ElementInfo {
    elements
        .iter()
        .find(|element| element.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("no element keyed {key}"))
}

fn focused_key(elements: &[ElementInfo]) -> Option<String> {
    elements.iter().find(|element| element.focused).and_then(|element| element.key.clone())
}

fn click(runtime: &mut Runtime<WindowState>, key: &str) {
    let rect = by_key(&runtime.elements(), key).rect;
    let (x, y) = (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
}

fn settle(runtime: &mut Runtime<WindowState>, frames: usize) {
    for frame in 0..frames {
        runtime.frame(frame as f64 / 60.0);
    }
}

fn all_text(page: &HelpPage) -> Vec<String> {
    let inline = |text: &Inline| text.text();
    let mut out = Vec::new();
    for block in &page.blocks {
        match block {
            Block::Heading { text, .. } => out.push(inline(text)),
            Block::Paragraph(text) | Block::Bullet(text) => out.push(inline(text)),
            Block::Keys(rows) => {
                for KeyRow { keys, action } in rows {
                    out.push(inline(keys));
                    out.push(inline(action));
                }
            }
        }
    }
    out
}

fn headings(page: &HelpPage) -> Vec<String> {
    page.blocks
        .iter()
        .filter_map(|block| match block {
            Block::Heading { text, .. } => Some(text.text()),
            _ => None,
        })
        .collect()
}

fn first_inline(html: &str) -> Inline {
    match parse(html).blocks.into_iter().next() {
        Some(Block::Paragraph(inline)) | Some(Block::Bullet(inline)) => inline,
        other => panic!("no inline block in {html:?}: {other:?}"),
    }
}

// ---- the embedded pages ---------------------------------------------------

#[test]
fn every_locale_has_a_parsable_full_page() {
    for locale in lmdv_i18n::locales() {
        let parsed = parse(lmdv_i18n::help_page(locale.id()));
        let heads = headings(&parsed);
        assert!(heads.len() >= 4, "{}: too few sections: {heads:?}", locale.id());
        assert!(
            parsed.blocks.iter().filter(|block| matches!(block, Block::Paragraph(_))).count() > 10,
            "{}: too few paragraphs",
            locale.id()
        );
        let rows = parsed
            .blocks
            .iter()
            .map(|block| match block {
                Block::Keys(rows) => rows.len(),
                _ => 0,
            })
            .sum::<usize>();
        assert!(rows > 10, "{}: too few key rows ({rows})", locale.id());
        assert!(
            all_text(&parsed).iter().all(|text| text.chars().count() < 4_000),
            "{}: a block kept stray markup",
            locale.id()
        );
    }
}

#[test]
fn the_english_page_keeps_its_sections_and_shortcuts() {
    let parsed = parse(lmdv_i18n::help_page("en"));
    let heads = headings(&parsed);
    assert_eq!(heads.first().map(String::as_str), Some("Opening documents"));
    assert!(heads.iter().any(|head| head == "Keyboard shortcuts"), "{heads:?}");
    assert!(heads.iter().any(|head| head == "Desktop integration"), "{heads:?}");

    let all = all_text(&parsed);
    assert!(all.iter().any(|text| text == "F1"), "the F1 row is in the key table");
    assert!(all.iter().any(|text| text.contains("Show this help")));
    assert!(all.iter().any(|text| text.contains("A fast viewer and editor for Markdown documents")));
}

#[test]
fn localized_pages_reach_the_dialog_content() {
    assert_eq!(headings(&parse(lmdv_i18n::help_page("it")))[0], "Aprire i documenti");
    assert_eq!(headings(&parse(lmdv_i18n::help_page("de")))[0], "Dokumente öffnen");
    assert_eq!(headings(&parse(lmdv_i18n::help_page("fr")))[0], "Ouvrir des documents");
    assert_eq!(headings(&parse(lmdv_i18n::help_page("es")))[0], "Abrir documentos");
    assert_eq!(headings(&parse(lmdv_i18n::help_page("zh-CN")))[0], "打开文档");
}

#[test]
fn pages_are_parsed_once_per_locale() {
    let first = page("en");
    let second = page("en");
    assert!(std::sync::Arc::ptr_eq(&first, &second), "the same locale reuses its parse");
    let italian = page("it");
    assert!(!std::sync::Arc::ptr_eq(&first, &italian));
    assert_eq!(page("xx").blocks, first.blocks, "an unknown id falls back to English");
}

// ---- the HTML subset ------------------------------------------------------

#[test]
fn inline_marks_entities_and_line_breaks_become_styled_runs() {
    let html = "<p>Use <strong>bold</strong>, <em>italics</em>, <code>a &amp; b</code> and \
                <kbd>Ctrl</kbd>+<kbd>S</kbd>.<br>Tom &amp; Jerry &mdash; &#39;quoted&#39; &lt;tag&gt;</p>";
    let parsed = parse(html);
    let [Block::Paragraph(inline)] = parsed.blocks.as_slice() else {
        panic!("one paragraph: {:?}", parsed.blocks);
    };
    assert_eq!(inline.text(), "Use bold, italics, a & b and Ctrl+S.\nTom & Jerry — 'quoted' <tag>");
    let run = |text: &str| inline.runs.iter().find(|run| run.text.contains(text)).expect(text);
    assert!(run("bold").bold && !run("bold").italic);
    assert!(run("italics").italic && !run("italics").bold);
    assert!(run("a & b").mono && !run("a & b").keycap);
    assert!(run("Ctrl").keycap && run("Ctrl").mono);
    assert!(run("Tom & Jerry ").text.contains('&'));
}

#[test]
fn lists_links_tables_and_unknown_tags_are_handled() {
    let html = "\
        <!-- a comment --><h3>Sections</h3>\
        <p>Read <a href=\"https://example.com\">the <strong>guide</strong></a> or <span class=\"x\">this</span>.</p>\
        <ul><li>First item</li><li>Second &amp; last</li></ul>\
        <table class=\"help-keys\"><tbody>\
          <tr><td><kbd>Ctrl</kbd>+<kbd>F</kbd></td><td>Find</td></tr>\
          <tr><td><kbd>Esc</kbd></td><td>Close</td></tr>\
        </tbody></table>";
    let parsed = parse(html);
    assert_eq!(headings(&parsed), ["Sections"]);
    assert!(matches!(parsed.blocks[1], Block::Paragraph(_)));
    let Block::Paragraph(paragraph) = &parsed.blocks[1] else { unreachable!() };
    assert_eq!(paragraph.text(), "Read the guide or this.");
    assert!(paragraph.runs.iter().any(|run| run.text == "guide" && run.bold), "the link's bold text is kept");
    assert!(matches!(parsed.blocks[2], Block::Bullet(_)));
    assert_eq!(parsed.blocks[2], Block::Bullet(first_inline("<li>First item</li>")));
    assert_eq!(parsed.blocks[3], Block::Bullet(first_inline("<li>Second &amp; last</li>")));
    let Block::Keys(rows) = &parsed.blocks[4] else { panic!("key table: {:?}", parsed.blocks[4]) };
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].keys.text(), "Ctrl+F");
    assert_eq!(rows[0].action.text(), "Find");
    assert!(rows[1].keys.runs.iter().any(|run: &Run| run.keycap));
}

#[test]
fn stray_text_and_unclosed_blocks_still_reach_the_page() {
    let parsed = parse("Intro text<p>No close");
    assert_eq!(parsed.blocks.len(), 2);
    assert_eq!(parsed.blocks[0], Block::Paragraph(first_inline("<p>Intro text</p>")));
    assert_eq!(parsed.blocks[1], Block::Paragraph(first_inline("<p>No close</p>")));

    let parsed = parse("<p>a</p><h3>b");
    assert_eq!(headings(&parsed), ["b"]);
}

// ---- the dialog -----------------------------------------------------------

#[test]
fn the_full_help_shows_the_repository_license_and_version() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::view::scene_help_full(&mut state);
    let mut runtime = runtime(state, (1100.0, 760.0));
    runtime.frame(0.0);

    let elements = runtime.elements();
    let about = by_key(&elements, HELP_ABOUT);
    assert_eq!(about.role, "group", "the About row is one group");
    let repository = by_key(&elements, HELP_REPOSITORY);
    assert_eq!(repository.role, "link", "the repository is a link");
    assert_eq!(repository.label.as_deref(), Some("Repository"));
    let license = by_key(&elements, HELP_LICENSE);
    assert_eq!(license.role, "link", "the license is a link");
    assert_eq!(license.label.as_deref(), Some("License (MIT)"));
    let version = by_key(&elements, HELP_ABOUT_VERSION);
    assert_eq!(version.text.as_deref(), Some(concat!("Version ", env!("CARGO_PKG_VERSION"))));
    assert_eq!(by_key(&elements, HELP_VERSION).text.as_deref(), Some(concat!("Version ", env!("CARGO_PKG_VERSION"))));
    assert_eq!(HELP_REPOSITORY_URL, "https://github.com/LeandroMarceddu/lmdv-rs");
    assert_eq!(HELP_LICENSE_URL, "https://github.com/LeandroMarceddu/lmdv-rs/blob/main/LICENSE");

    // Both links go through the app's external-open path; headless runs record
    // the request instead of opening a browser.
    click(&mut runtime, HELP_REPOSITORY);
    runtime.frame(1.0 / 60.0);
    assert_eq!(runtime.take_url_requests(), [HELP_REPOSITORY_URL], "the repository URL is opened");
    click(&mut runtime, HELP_LICENSE);
    runtime.frame(2.0 / 60.0);
    assert_eq!(runtime.take_url_requests(), [HELP_LICENSE_URL.to_string()], "the license URL is opened");
}

#[test]
fn the_about_labels_are_localized_in_all_six_catalogs() {
    let expected = [
        ("en", "Repository", "License"),
        ("it", "Repository", "Licenza"),
        ("de", "Repository", "Lizenz"),
        ("fr", "Dépôt", "Licence"),
        ("es", "Repositorio", "Licencia"),
        ("zh-CN", "代码仓库", "许可证"),
    ];
    assert_eq!(lmdv_i18n::locales().len(), expected.len());
    for (index, (id, repository, license)) in expected.into_iter().enumerate() {
        assert_eq!(lmdv_i18n::locales()[index].id(), id);
        assert_eq!(lmdv_i18n::text_in(index, lmdv_i18n::key("help.repository")), repository, "{id}");
        assert_eq!(lmdv_i18n::text_in(index, lmdv_i18n::key("help.license")), license, "{id}");
    }
}

#[test]
fn the_full_scene_exposes_the_dialog_and_its_section_headings() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::view::scene_help_full(&mut state);
    let mut runtime = runtime(state, (1100.0, 760.0));
    runtime.frame(0.0);

    let elements = runtime.elements();
    let root = by_key(&elements, HELP_KEY);
    assert_eq!(root.role, "dialog", "one dialog node names the modal");
    assert_eq!(root.label.as_deref(), Some("Help"));
    let panel = by_key(&elements, HELP_PANEL);
    assert_eq!(panel.role, "group", "the visual panel is not a second dialog");
    assert_eq!(by_key(&elements, HELP_TITLE).level, Some(1));
    let sections: Vec<&ElementInfo> = elements
        .iter()
        .filter(|element| element.key.as_deref().is_some_and(|key| key.starts_with("help-section-")))
        .collect();
    assert!(sections.len() >= 8, "the guide's sections are exposed: {sections:?}");
    assert!(sections.iter().all(|section| section.role == "heading" && section.level == Some(3)));
    assert!(
        sections.iter().any(|section| section.text.as_deref() == Some("Opening documents")),
        "the first section's name is in the tree: {:?}",
        sections.iter().map(|section| section.text.as_deref()).collect::<Vec<_>>()
    );
    assert!(by_key(&elements, HELP_BODY).rect[3] > 0.0);
    let before = by_key(&elements, "help-section-2").rect[1];
    runtime.update(|_, tx| tx.scroll_to_instant(HELP_BODY, 0.0, 120.0));
    runtime.frame(1.0 / 60.0);
    let elements = runtime.elements();
    let after = by_key(&elements, "help-section-2").rect[1];
    assert!((before - after - 120.0).abs() < 2.0, "the page scrolls in its body: {before} -> {after}");
    assert!(elements.iter().all(|element| element.rect[0] + element.rect[2] <= 1100.0));
}

#[test]
fn the_full_page_fits_a_480_pixel_window() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::view::scene_help_full(&mut state);
    let mut runtime = runtime(state, (480.0, 760.0));
    runtime.frame(0.0);

    let elements = runtime.elements();
    let panel = by_key(&elements, HELP_PANEL);
    assert!(panel.rect[0] >= 0.0, "{:?}", panel.rect);
    assert!(panel.rect[0] + panel.rect[2] <= 480.0, "the panel overflows a 480 pt window: {:?}", panel.rect);
    let body = by_key(&elements, HELP_BODY);
    assert!(body.rect[0] + body.rect[2] <= 480.0, "the body overflows: {:?}", body.rect);
    let offenders: Vec<_> = elements
        .iter()
        .filter(|element| element.path.contains(HELP_PANEL))
        .filter(|element| element.visible[2] > 0.0 && element.visible[3] > 0.0)
        .filter(|element| element.visible[0] + element.visible[2] > 480.5)
        .map(|element| (element.key.clone(), element.rect, element.visible, element.role))
        .collect();
    assert!(
        !offenders.is_empty() || elements.iter().any(|element| element.path.contains(HELP_PANEL)),
        "the panel is in the tree"
    );
    assert!(offenders.is_empty(), "drawn past the right edge: {offenders:?}");
    assert!(elements.iter().any(|element| element.text.as_deref() == Some("Opening documents")));
    // H-04's About row fits too, with both links drawn and the panel on screen.
    let about = by_key(&elements, HELP_ABOUT);
    assert!(about.rect[0] >= 0.0 && about.rect[0] + about.rect[2] <= 480.5, "{:?}", about.rect);
    assert!(by_key(&elements, HELP_REPOSITORY).rect[2] > 0.0, "the repository link draws");
    assert!(by_key(&elements, HELP_LICENSE).rect[2] > 0.0, "the license link draws");
    assert!(panel.rect[1] >= 0.0 && panel.rect[1] + panel.rect[3] <= 760.5, "the panel fits: {:?}", panel.rect);
    assert!(by_key(&elements, HELP_BODY).rect[3] > 100.0, "the guide still has room to scroll");
}

#[test]
fn the_page_scrolls_from_the_keyboard() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::view::scene_help_full(&mut state);
    let mut runtime = runtime(state, (1100.0, 760.0));
    runtime.frame(0.0);
    assert_eq!(focused_key(&runtime.elements()).as_deref(), Some(HELP_BODY), "the page takes the keyboard");

    let before = by_key(&runtime.elements(), "help-section-2").rect[1];
    runtime.key(KeyEvent { key: Key::PageDown, mods: Mods::default() });
    settle(&mut runtime, 3);
    let after = by_key(&runtime.elements(), "help-section-2").rect[1];
    assert!(after < before - 300.0, "Page Down pages the guide: {before} -> {after}");

    runtime.key(KeyEvent { key: Key::Home, mods: Mods::default() });
    settle(&mut runtime, 3);
    assert!(by_key(&runtime.elements(), "help-section-2").rect[1] > after + 300.0, "Home returns to the top");
}

#[test]
fn the_toolbar_help_opens_the_full_guide_and_closing_returns_focus() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("One", "# One\n");
    let mut runtime = runtime(state, (1100.0, 760.0));
    runtime.frame(0.0);

    click(&mut runtime, "help");
    runtime.frame(1.0 / 60.0);
    assert_eq!(runtime.state.help_view(), Some(HelpView::Full));
    assert!(focused_key(&runtime.elements()).is_some(), "the dialog takes the keyboard");
    assert_eq!(by_key(&runtime.elements(), HELP_TITLE).role, "heading");

    runtime.update(|state, _| state.close_help());
    runtime.frame(2.0 / 60.0);
    assert_eq!(runtime.state.help_view(), None);
    assert_eq!(
        focused_key(&runtime.elements()).as_deref(),
        Some("help"),
        "closing hands focus back to the toolbar button that opened it"
    );
}

#[test]
fn escape_question_mark_and_the_scrim_close_the_full_guide() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("One", "# One\n");
    state.open_help(HelpView::Full);
    let mut runtime = runtime(state, (1100.0, 760.0));
    runtime.frame(0.0);

    let escape = KeyEvent { key: Key::Escape, mods: Mods::default() };
    runtime.key(escape);
    settle(&mut runtime, 2);
    assert_eq!(runtime.state.help_view(), None, "Escape closes the guide");

    runtime.update(|state, _| state.open_help(HelpView::Full));
    settle(&mut runtime, 3);
    assert_eq!(runtime.state.help_view(), Some(HelpView::Full), "the guide reopened");
    let question = KeyEvent { key: Key::Text("?".to_owned()), mods: Mods { shift: true, ..Mods::default() } };
    runtime.key(question);
    settle(&mut runtime, 2);
    assert_eq!(runtime.state.help_view(), None, "`?` closes the guide too");

    runtime.update(|state, _| state.open_help(HelpView::Full));
    settle(&mut runtime, 3);
    runtime.pointer_move(4.0, 4.0);
    runtime.pointer_down(4.0, 4.0);
    runtime.pointer_up(4.0, 4.0);
    settle(&mut runtime, 2);
    assert_eq!(runtime.state.help_view(), None, "the scrim closes the modal");
}

#[test]
fn the_close_button_closes_the_full_guide_and_the_keys_stay_compact() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_help(HelpView::Full);
    let mut runtime = runtime(state, (1100.0, 760.0));
    runtime.frame(0.0);
    click(&mut runtime, HELP_CLOSE);
    settle(&mut runtime, 2);
    assert_eq!(runtime.state.help_view(), None);

    runtime.update(|state, tx| {
        assert!(state.run_command("help.keys", tx));
    });
    settle(&mut runtime, 20);
    assert_eq!(runtime.state.help_view(), Some(HelpView::Shortcuts));
    let elements = runtime.elements();
    assert_eq!(by_key(&elements, "help-dialog").role, "dialog");
    assert!(
        elements.iter().all(|element| element.key.as_deref() != Some("help-full-dialog")),
        "the full page is gone: {:?}",
        elements.iter().filter_map(|element| element.key.as_deref()).collect::<Vec<_>>()
    );
}

// Keep the scrim constant exercised by the tests above (and documented for
// the orchestrator's tree checks).
#[test]
fn the_modal_has_a_keyed_scrim() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_help(HelpView::Full);
    let mut runtime = runtime(state, (1100.0, 760.0));
    runtime.frame(0.0);
    assert!(runtime.elements().iter().any(|element| element.key.as_deref() == Some(HELP_KEY)));
    assert!(runtime.elements().iter().any(|element| element.key.as_deref() == Some(HELP_SCRIM)));
}
