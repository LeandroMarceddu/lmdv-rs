//! The media preview (V-01–V-10): renderer refs, open/close, zoom, pan,
//! focus, modal shortcuts and the headless scene.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use lmdv_core::{Document, DocumentId, DocumentKey, SourceBuffer, TabId};
use lmdv_render::MediaKind;
use rune::{ElementInfo, Key, KeyEvent, Mods, Runtime, Tx, WheelEvent};
use tempfile::TempDir;

use crate::lightbox::{
    LIGHTBOX_ACTUAL, LIGHTBOX_CLOSE, LIGHTBOX_FIT, LIGHTBOX_KEY, LIGHTBOX_STAGE, LIGHTBOX_VIEWPORT, LIGHTBOX_WIDTH,
    LIGHTBOX_ZOOM_IN, LIGHTBOX_ZOOM_LEVEL, LIGHTBOX_ZOOM_OUT, LightboxMode, PAN_STEP, media_refs,
};
use crate::test_support;
use crate::window::WindowState;

const IMAGE_MARKDOWN: &str = r#"# Media

The picture and the diagram.

![A quiet landscape](./img/test.png)

```mermaid
flowchart LR
    A[Source] --> B{Parse}
    B -->|ok| C[Render]
```
"#;

fn window(temp: &TempDir) -> WindowState {
    let state = WindowState::new(test_support::shared(temp));
    // Headless image decodes wait, so refs and screenshots are deterministic.
    state.shared.set_headless();
    state
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn runtime(state: WindowState) -> Runtime<WindowState> {
    Runtime::new(state, crate::view::view, (1280.0, 900.0), rune::Env::new(true, 2.0))
}

fn keys(elements: &[ElementInfo]) -> Vec<&str> {
    elements.iter().filter_map(|element| element.key.as_deref()).collect()
}

fn focused(elements: &[ElementInfo]) -> Option<String> {
    elements.iter().find(|element| element.focused).and_then(|element| element.key.clone())
}

fn by_key<'a>(elements: &'a [ElementInfo], key: &str) -> &'a ElementInfo {
    elements.iter().find(|element| element.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
}

/// A ready file-backed document rooted at the fixture folder, so relative
/// images resolve without a load job.
fn open_file_document(state: &mut WindowState, name: &str, text: &str) -> (TabId, DocumentId, u64) {
    let path = fixtures().join(name);
    let id = state.shared.docs_mut().allocate_id();
    let doc = Rc::new(Document::ready(id, DocumentKey::File(path), name, SourceBuffer::new(text)));
    let revision = doc.revision();
    state.shared.docs_mut().insert_ready(doc.clone());
    state.shared.store_parsed(id, revision, Ok(Arc::new(lmdv_md::parse_with_revision(text, revision))));
    let tab = state.attach_document(doc);
    (tab, id, revision)
}

fn open_active(state: &mut WindowState, media: lmdv_render::MediaRef) {
    let (tab, doc, revision) = {
        let tab = state.active_tab().expect("a tab");
        (tab.id, tab.document_id().expect("a document"), tab.view.reader.as_ref().map_or(0, |m| m.revision))
    };
    let mut tx = Tx::default();
    state.open_media(tab, doc, revision, media, &mut tx);
}

fn image_ref(state: &WindowState) -> lmdv_render::MediaRef {
    media_refs(state).into_iter().find(|media| media.kind == MediaKind::Image).expect("an image reference")
}

fn diagram_ref(state: &WindowState) -> lmdv_render::MediaRef {
    media_refs(state).into_iter().find(|media| media.kind == MediaKind::Diagram).expect("a diagram reference")
}

fn media_key(media: &lmdv_render::MediaRef) -> String {
    format!("media-{}", media.block)
}

fn plain(key: Key) -> KeyEvent {
    KeyEvent { key, mods: Mods::default() }
}

// ---- opening --------------------------------------------------------------

#[test]
fn an_image_opens_with_its_name_alt_and_a_fitted_stage() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "lightbox.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    assert_eq!(media.source, "./img/test.png");
    assert_eq!((media.width, media.height), (320.0, 140.0));

    open_active(&mut state, media.clone());
    let lightbox = state.lightbox().expect("the preview opened");
    assert_eq!(lightbox.title, "test.png", "the title bar names the image");
    assert_eq!(lightbox.caption, "A quiet landscape", "the alt text is the caption");
    assert_eq!(lightbox.opener, format!("media-{}", media.block));
    assert_eq!(lightbox.mode(), LightboxMode::Fit);
    lightbox.image.as_ref().expect("the picture handle").wait().expect("the fixture decodes");

    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let elements = runtime.elements();
    let drawn = keys(&elements);
    for key in [
        LIGHTBOX_KEY,
        LIGHTBOX_VIEWPORT,
        LIGHTBOX_STAGE,
        LIGHTBOX_CLOSE,
        LIGHTBOX_ZOOM_OUT,
        LIGHTBOX_ZOOM_LEVEL,
        LIGHTBOX_ZOOM_IN,
        LIGHTBOX_FIT,
        LIGHTBOX_WIDTH,
        LIGHTBOX_ACTUAL,
    ] {
        assert!(drawn.contains(&key), "missing {key}: {drawn:?}");
    }
    assert_eq!(by_key(&elements, LIGHTBOX_KEY).role, "dialog");
    assert_eq!(by_key(&elements, LIGHTBOX_KEY).label.as_deref(), Some("Preview"));
    assert_eq!(by_key(&elements, LIGHTBOX_VIEWPORT).role, "group");
    assert!(
        by_key(&elements, LIGHTBOX_VIEWPORT).label.as_deref().is_some_and(|label| label.contains("Plus and minus"))
    );
    assert_eq!(by_key(&elements, LIGHTBOX_STAGE).role, "img");
    assert_eq!(by_key(&elements, LIGHTBOX_STAGE).label.as_deref(), Some("A quiet landscape"));
    assert_eq!(by_key(&elements, "lightbox-title-text").text.as_deref(), Some("test.png"));
    assert_eq!(by_key(&elements, "lightbox-zoom-level-text").text.as_deref(), Some("100%"));
    assert_eq!(runtime.state.lightbox().expect("open").fit_scale(), 1.0, "a small raster fits at 1:1");
}

#[test]
fn a_broken_image_never_opens() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "broken.md", "![Gone](./missing.png)\n");
    assert!(media_refs(&state).is_empty(), "a missing picture gets no media ref");

    let (tab, doc, revision) = {
        let tab = state.active_tab().expect("tab");
        (tab.id, tab.document_id().expect("doc"), tab.view.reader.as_ref().map_or(0, |m| m.revision))
    };
    let forged = lmdv_render::MediaRef {
        block: "b0".to_owned(),
        index: 0,
        kind: MediaKind::Image,
        source: "./missing.png".to_owned(),
        alt: "Gone".to_owned(),
        width: 10.0,
        height: 10.0,
        line: 0,
        end_line: 1,
    };
    let mut tx = Tx::default();
    state.open_media(tab, doc, revision, forged, &mut tx);
    assert!(state.lightbox().is_none(), "a refused picture stays inert");
}

#[test]
fn a_diagram_opens_from_its_mermaid_source() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Diagram", "```mermaid\nflowchart LR\n    A --> B\n```\n");
    let media = diagram_ref(&state);
    assert!(media.source.contains("flowchart LR"));
    open_active(&mut state, media.clone());
    let lightbox = state.lightbox().expect("the preview opened");
    assert!(lightbox.image.is_none());
    assert_eq!(lightbox.title, "Diagram");
    assert_eq!(lightbox.caption, "Diagram");
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let scale = runtime.state.lightbox().expect("open").scale();
    assert!(scale > 1.0 && scale <= 4.0, "a vector may fill the window up to 4x: {scale}");
    let elements = runtime.elements();
    assert_eq!(by_key(&elements, LIGHTBOX_STAGE).role, "img");
    assert_eq!(by_key(&elements, LIGHTBOX_STAGE).label.as_deref(), Some("Diagram"));
}

#[test]
fn an_unsupported_diagram_never_reaches_the_lightbox() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Broken", "```mermaid\nnot a diagram {{{\n```\n");
    assert!(media_refs(&state).is_empty());
}

#[test]
fn only_one_preview_opens_per_window() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "both.md", IMAGE_MARKDOWN);
    let image = image_ref(&state);
    let diagram = diagram_ref(&state);
    open_active(&mut state, image.clone());
    open_active(&mut state, diagram);
    assert_eq!(
        state.lightbox().expect("open").media.kind,
        MediaKind::Image,
        "a second open request is ignored while the preview is up"
    );
}

// ---- the wire and the keyboard -------------------------------------------

#[test]
fn media_is_a_focusable_button_and_its_hint_stays_out_of_the_tab_order() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "wire.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let elements = runtime.elements();
    let key = media_key(&media);
    let button = by_key(&elements, &key);
    assert!(button.focusable);
    assert_eq!(button.role, "button");
    assert_eq!(button.label.as_deref(), Some("A quiet landscape — open preview"));
    assert!(elements.iter().any(|element| element.key.as_deref() == Some(&format!("media-hint-{}", media.block))));
    assert!(
        !elements
            .iter()
            .find(|element| element.key.as_deref() == Some(&format!("media-hint-{}", media.block)))
            .expect("hint")
            .focusable,
        "the shared hint is not a tab stop"
    );
    assert!(
        elements
            .iter()
            .any(|element| element.key.as_deref() == Some(&format!("media-{}", diagram_ref(&runtime.state).block)))
    );
}

#[test]
fn a_click_opens_focuses_the_viewport_and_escape_returns_to_the_media() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "click.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    let key = media_key(&media);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let (x, y) = runtime.find(&key).expect("the media button");
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(0.0);
    assert!(runtime.state.lightbox().is_some(), "the click opened the preview");
    assert_eq!(focused(&runtime.elements()).as_deref(), Some(LIGHTBOX_VIEWPORT));

    runtime.key(plain(Key::Escape));
    runtime.frame(0.0);
    assert!(runtime.state.lightbox().is_none());
    assert_eq!(
        focused(&runtime.elements()).as_deref(),
        Some(key.as_str()),
        "focus returns to the media button that opened it"
    );
}

#[test]
fn enter_and_space_open_a_focused_media_element() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "keys.md", IMAGE_MARKDOWN);
    let key = media_key(&image_ref(&state));
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    runtime.update(|_, tx| tx.focus(key.clone(), false));
    runtime.frame(0.0);
    runtime.key(plain(Key::Enter));
    runtime.frame(0.0);
    assert!(runtime.state.lightbox().is_some());
    runtime.key(plain(Key::Escape));
    runtime.frame(0.0);
    assert!(runtime.state.lightbox().is_none());
    runtime.key(plain(Key::Text(" ".to_owned())));
    runtime.frame(0.0);
    assert!(runtime.state.lightbox().is_some(), "Space opens it too");
}

// ---- zoom, pan, wheel -----------------------------------------------------

#[test]
fn the_buttons_and_keys_step_clamp_fit_width_and_actual() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "zoom.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    open_active(&mut state, media);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let lightbox = runtime.state.lightbox.as_mut().expect("open");
    let fit = lightbox.fit_scale();
    assert_eq!(fit, 1.0);

    lightbox.zoom_in();
    assert!((lightbox.scale() / fit - 1.25).abs() < 0.001, "one step is 1.25x");
    lightbox.zoom_out();
    assert!((lightbox.scale() - fit).abs() < 0.001);

    lightbox.width();
    assert_eq!(lightbox.mode(), LightboxMode::Width);
    lightbox.actual();
    assert_eq!(lightbox.mode(), LightboxMode::Actual);
    assert!((lightbox.scale() - 1.0).abs() < 0.001);

    for _ in 0..20 {
        lightbox.zoom_in();
    }
    assert!(lightbox.scale() <= 8.0 + 0.001, "8x ceiling");
    for _ in 0..60 {
        lightbox.zoom_out();
    }
    assert!(lightbox.scale() >= lightbox.fit_scale().min(0.2) - 0.001, "min(fit, 0.2) floor");
    lightbox.fit();
    assert_eq!(lightbox.mode(), LightboxMode::Fit);
    assert!((lightbox.scale() - fit).abs() < 0.001);
    assert_eq!(lightbox.pan(), [0.0, 0.0]);
}

#[test]
fn arrow_keys_pan_by_40_or_160_points_and_clamp() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "pan.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    open_active(&mut state, media);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    for _ in 0..9 {
        runtime.state.lightbox.as_mut().expect("open").zoom_in();
    }
    runtime.frame(0.0);

    runtime.key(plain(Key::Left));
    runtime.frame(0.0);
    assert_eq!(runtime.state.lightbox().expect("open").pan()[0], PAN_STEP);
    runtime.key(plain(Key::Right));
    runtime.key(plain(Key::Right));
    runtime.frame(0.0);
    assert_eq!(runtime.state.lightbox().expect("open").pan()[0], -PAN_STEP);
    runtime.key(KeyEvent { key: Key::Left, mods: Mods { shift: true, ..Mods::default() } });
    runtime.frame(0.0);
    assert_eq!(runtime.state.lightbox().expect("open").pan()[0], 120.0);
    assert_eq!(runtime.state.lightbox().expect("open").mode(), LightboxMode::Custom);
}

#[test]
fn the_wheel_zooms_about_the_pointer_and_never_scrolls_the_article() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "wheel.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    open_active(&mut state, media);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let scroll = runtime.state.scroll_snapshot();
    let viewport = by_key(&runtime.elements(), LIGHTBOX_VIEWPORT).rect;
    // Overflow the stage first: a fitted image centres on each axis, so an
    // anchored zoom is only observable once the content is larger than it.
    for _ in 0..9 {
        runtime.state.lightbox.as_mut().expect("open").zoom_in();
    }
    runtime.frame(0.0);
    let (x, y) = (viewport[0] + viewport[2] * 0.3, viewport[1] + viewport[3] * 0.6);
    let before = runtime.state.lightbox().expect("open").scale();
    let pan = runtime.state.lightbox().expect("open").pan();
    assert!(before > 6.0);

    runtime.wheel_event(WheelEvent::new(0.0, 100.0).at(x, y));
    runtime.frame(0.0);
    let after = runtime.state.lightbox().expect("open").scale();
    assert!(after < before, "down zooms out: {before} -> {after}");
    assert!((after / before - (-100.0 * 0.0015f32).exp()).abs() < 0.01);
    // The content point under the pointer stays under it. Wheel coordinates
    // arrive in the viewport's own points, measured from its centre here.
    let area = crate::lightbox::Lightbox::area_for([1280.0, 900.0]);
    let local = [x - area[0] - area[2] / 2.0, y - area[1] - area[3] / 2.0];
    let body = runtime.state.lightbox().expect("open");
    let new_pan = body.pan();
    let content_before = [(local[0] - pan[0]) / before, (local[1] - pan[1]) / before];
    let content_after = [(local[0] - new_pan[0]) / after, (local[1] - new_pan[1]) / after];
    assert!((content_before[0] - content_after[0]).abs() < 0.05, "{content_before:?} {content_after:?}");
    assert!((content_before[1] - content_after[1]).abs() < 0.05, "{content_before:?} {content_after:?}");
    assert_eq!(runtime.state.scroll_snapshot(), scroll, "the article never moves");

    // Ctrl+wheel runs the same zoom, not the article's page zoom.
    let before = runtime.state.lightbox().expect("open").scale();
    runtime.wheel_event(WheelEvent::new(0.0, 100.0).at(x, y).mods(Mods { ctrl: true, ..Mods::default() }));
    runtime.frame(0.0);
    assert!(runtime.state.lightbox().expect("open").scale() < before);
    assert_eq!(runtime.state.scroll_snapshot(), scroll);
}

#[test]
fn a_drag_pans_a_zoomed_image_and_does_not_close() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "drag.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    open_active(&mut state, media);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    for _ in 0..9 {
        runtime.state.lightbox.as_mut().expect("open").zoom_in();
    }
    runtime.frame(0.0);
    let (cx, cy) = runtime.find(LIGHTBOX_VIEWPORT).expect("viewport");
    runtime.pointer_move(cx, cy);
    runtime.pointer_down(cx, cy);
    runtime.pointer_move(cx + 60.0, cy + 30.0);
    runtime.pointer_up(cx + 60.0, cy + 30.0);
    runtime.frame(0.0);
    let body = runtime.state.lightbox().expect("still open");
    assert!(body.pan()[0] > 30.0 && body.pan()[1] > 15.0, "the image follows the hand: {:?}", body.pan());

    // Releasing outside the window's stage is still a pan, not a backdrop close.
    runtime.pointer_move(cx, cy);
    runtime.pointer_down(cx, cy);
    runtime.pointer_move(2.0, 2.0);
    runtime.pointer_up(2.0, 2.0);
    runtime.frame(0.0);
    assert!(runtime.state.lightbox().is_some(), "a press on the media never dismisses on release");
}

#[test]
fn a_plain_click_on_empty_stage_or_backdrop_closes_and_a_drag_does_not() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "close.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    open_active(&mut state, media);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let viewport = by_key(&runtime.elements(), LIGHTBOX_VIEWPORT).rect;
    let empty = (viewport[0] + 6.0, viewport[1] + 6.0);
    runtime.pointer_move(empty.0, empty.1);
    runtime.pointer_down(empty.0, empty.1);
    runtime.pointer_up(empty.0, empty.1);
    runtime.frame(0.0);
    assert!(runtime.state.lightbox().is_none(), "a plain click on the empty stage closes");

    // Reopen and drag the backdrop: the release must not dismiss it.
    let media = image_ref(&runtime.state);
    open_active(&mut runtime.state, media);
    runtime.frame(0.0);
    runtime.pointer_move(400.0, 860.0);
    runtime.pointer_down(400.0, 860.0);
    runtime.pointer_move(420.0, 870.0);
    runtime.pointer_up(420.0, 870.0);
    runtime.frame(0.0);
    assert!(runtime.state.lightbox().is_some(), "a backdrop drag keeps the preview open");
}

#[test]
fn a_double_click_toggles_between_fit_and_actual_size() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "double.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    open_active(&mut state, media);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let (cx, cy) = runtime.find(LIGHTBOX_STAGE).expect("the stage");
    runtime.pointer_move(cx, cy);
    for _ in 0..2 {
        runtime.pointer_down(cx, cy);
        runtime.pointer_up(cx, cy);
    }
    runtime.frame(0.0);
    let body = runtime.state.lightbox().expect("open");
    assert!(body.scale() > 1.0, "a double-click on a small image zooms to 200%: {}", body.scale());
    runtime.frame(0.6);
    runtime.pointer_down(cx, cy);
    runtime.pointer_up(cx, cy);
    runtime.pointer_down(cx, cy);
    runtime.pointer_up(cx, cy);
    runtime.frame(0.0);
    assert_eq!(runtime.state.lightbox().expect("open").mode(), LightboxMode::Fit);
}

// ---- window state ---------------------------------------------------------

#[test]
fn a_tab_switch_closes_the_preview_and_coming_back_keeps_it_closed() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "switch.md", IMAGE_MARKDOWN);
    let tab = state.active_tab().expect("tab").id;
    let media = image_ref(&state);
    open_active(&mut state, media);
    assert!(state.lightbox().is_some());
    state.new_start_tab();
    assert!(state.lightbox().is_none(), "opening a tab in front closes it");
    state.activate_tab(tab);
    assert!(state.lightbox().is_none(), "coming back does not reopen it");
}

#[test]
fn a_reparse_of_the_model_closes_the_preview() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "stale.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    open_active(&mut state, media);
    assert!(state.lightbox().is_some());
    let next = state.active_tab().and_then(|tab| tab.view.reader.as_ref().map(|model| model.revision)).unwrap_or(0) + 1;
    let model = Arc::new(lmdv_md::parse_with_revision(IMAGE_MARKDOWN, next));
    if let Some(tab) = state.active_tab_mut() {
        tab.view.reader = Some(model);
    }
    assert!(state.poll_lightbox(), "the model moved on");
    assert!(state.lightbox().is_none());
}

#[test]
fn ctrl_w_is_swallowed_and_tab_focus_stays_inside_the_modal() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "modal.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    open_active(&mut state, media);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let tabs = runtime.state.tabs().len();
    runtime.key(KeyEvent { key: Key::Command('w'), mods: Mods { ctrl: true, cmd: true, ..Mods::default() } });
    runtime.frame(0.0);
    assert!(runtime.state.lightbox().is_some(), "Ctrl+W must not act through the preview");
    assert_eq!(runtime.state.tabs().len(), tabs, "the tab was not closed");

    runtime.key(plain(Key::Tab));
    runtime.frame(0.0);
    let focused = focused(&runtime.elements()).unwrap_or_default();
    assert!(
        focused == LIGHTBOX_ZOOM_OUT
            || focused == LIGHTBOX_ZOOM_LEVEL
            || focused == LIGHTBOX_ZOOM_IN
            || focused == LIGHTBOX_FIT
            || focused == LIGHTBOX_WIDTH
            || focused == LIGHTBOX_ACTUAL
            || focused == LIGHTBOX_CLOSE
            || focused == LIGHTBOX_VIEWPORT,
        "Tab stays inside the dialog: {focused}"
    );
}

#[test]
fn the_scene_opens_the_fixture_image_and_both_media_are_wired() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    crate::lightbox::scene_lightbox(&mut state);
    assert!(!state.tabs().is_empty());
    let lightbox = state.lightbox().expect("the scene opens the picture");
    assert_eq!(lightbox.media.kind, MediaKind::Image);
    assert_eq!(lightbox.caption, "A quiet landscape");
    assert_eq!(lightbox.title, "test.png");
    assert!(state.active_tab().and_then(|tab| tab.view.reader.as_ref()).is_some(), "the scene page is parsed");
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let elements = runtime.elements();
    assert!(elements.iter().any(|element| element.key.as_deref() == Some(LIGHTBOX_KEY)));
    let diagram = media_refs(&runtime.state)
        .into_iter()
        .find(|media| media.kind == MediaKind::Diagram)
        .expect("the scene page also wires its Mermaid diagram");
    assert!(diagram.source.contains("flowchart LR"));
}

#[test]
fn the_block_menu_offers_open_preview_for_media() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "menu.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let button = by_key(&runtime.elements(), &media_key(&media)).rect;
    let (x, y) = (button[0] + button[2] / 2.0, button[1] + button[3] / 2.0);
    runtime.secondary_down(x, y);
    runtime.secondary_up(x, y);
    runtime.frame(1.0 / 60.0);
    runtime.frame(1.0 / 60.0);
    assert!(runtime.state.block_menu().is_some(), "the media block's menu opened");
    let (x, y) = runtime.find("block-menu-item0").expect("Open preview is the first item");
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(1.0 / 60.0);
    assert!(runtime.state.lightbox().is_some(), "the menu item opened the preview");
    assert_eq!(runtime.state.lightbox().expect("open").media.block, media.block);
}
