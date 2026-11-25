//! The media preview (V-01–V-10): renderer refs, open/close, zoom, pan,
//! focus, modal shortcuts and the headless scene.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lmdv_core::{Document, DocumentId, DocumentKey, SourceBuffer, TabId};
use lmdv_render::MediaKind;
use rune::{ElementInfo, Key, KeyEvent, Mods, Runtime, Tx, WheelEvent};
use tempfile::TempDir;

use crate::lightbox::{
    LIGHTBOX_ACTUAL, LIGHTBOX_CLOSE, LIGHTBOX_COPY, LIGHTBOX_FIT, LIGHTBOX_KEY, LIGHTBOX_MENU, LIGHTBOX_STAGE,
    LIGHTBOX_VIEWPORT, LIGHTBOX_WIDTH, LIGHTBOX_ZOOM_IN, LIGHTBOX_ZOOM_LEVEL, LIGHTBOX_ZOOM_OUT, LightboxMode,
    PAN_STEP, media_refs,
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

/// A paragraph whose only picture is a text-span (inline) image.
const INLINE_MARKDOWN: &str = r#"# Inline

A sentence with ![Inline landscape](./img/test.png) inside it.
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
            || focused == LIGHTBOX_COPY
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

// ---- V-02: the hint -------------------------------------------------------

#[test]
fn the_hint_sits_at_the_media_top_right_corner() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "hint.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let elements = runtime.elements();
    let picture = by_key(&elements, &media_key(&media)).rect;
    let hint = by_key(&elements, &format!("media-hint-{}", media.block)).rect;
    assert!(picture[2] > 100.0 && picture[3] > 40.0, "the picture has a real box: {picture:?}");
    assert!(hint[2] > 0.0 && hint[3] > 0.0, "the hint is laid out: {hint:?}");
    assert!(hint[0] >= picture[0] - 0.5 && hint[1] >= picture[1] - 0.5, "inside the picture: {hint:?} {picture:?}");
    assert!(
        hint[0] + hint[2] <= picture[0] + picture[2] + 0.5 && hint[1] + hint[3] <= picture[1] + picture[3] + 0.5,
        "inside the picture: {hint:?} {picture:?}"
    );
    assert!(
        hint[0] > picture[0] + picture[2] / 2.0,
        "at the top-right corner, not the article column's right edge: {hint:?} {picture:?}"
    );
}

#[test]
fn the_hint_shows_for_hover_and_focus_and_leaves_with_the_pane() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "hint-focus.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    let key = media_key(&media);
    let hint_key = format!("media-hint-{}", media.block);
    let mut viewer = runtime(state);
    viewer.frame(0.0);
    assert!(
        by_key(&viewer.elements(), &hint_key).opacity <= 0.01,
        "the hint starts hidden until the media is hovered or focused"
    );

    let picture = by_key(&viewer.elements(), &key).rect;
    let (x, y) = (picture[0] + picture[2] / 2.0, picture[1] + picture[3] / 2.0);
    viewer.pointer_move(x, y);
    viewer.frame(0.0);
    viewer.frame(0.2);
    assert!(by_key(&viewer.elements(), &hint_key).opacity > 0.5, "hovering the picture reveals its preview hint");

    viewer.pointer_move(0.0, 0.0);
    viewer.frame(0.0);
    viewer.frame(0.2);
    assert!(by_key(&viewer.elements(), &hint_key).opacity < 0.1, "leaving the media hides it");

    // V-02: the media element is the keyboard button; Rune's hover group
    // follows keyboard focus, so the hint lights without being a tab stop.
    viewer.update(|_, tx| tx.focus(key.clone(), true));
    viewer.frame(0.0);
    viewer.frame(0.2);
    let focused_elements = viewer.elements();
    let hint = by_key(&focused_elements, &hint_key);
    assert!(hint.opacity > 0.5, "keyboard focus on the picture shows the same hint");
    assert!(!hint.focusable, "the hint never joins the tab order");

    // "Or leave the pane": the article's clip empties the hint's visible area
    // when the picture scrolls out with it.
    let text = format!("{}![Far](./img/test.png)\n", "A paragraph.\n\n".repeat(60));
    let path = fixtures().join("far.md");
    let mut state = window(&temp);
    open_temp_document(&mut state, &path, &text);
    let media = image_ref(&state);
    let mut offscreen = runtime(state);
    offscreen.frame(0.0);
    let offscreen_elements = offscreen.elements();
    let hint = by_key(&offscreen_elements, &format!("media-hint-{}", media.block));
    assert!(
        hint.visible[2] <= 0.0 || hint.visible[3] <= 0.0,
        "the hint is clipped with the picture that left the pane: {:?}",
        hint.visible
    );
}

// ---- V-08/V-09: copy and save --------------------------------------------

fn clipboard_runtime(state: WindowState) -> (Runtime<WindowState>, rune::clipboard::MemoryClipboard) {
    let board = rune::clipboard::memory();
    let mut runtime = runtime(state);
    runtime.use_clipboard(&board);
    runtime.frame(0.0);
    (runtime, board)
}

fn wait_for_png(state: &WindowState) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !state.lightbox().is_some_and(|lightbox| lightbox.png_ready()) {
        assert!(Instant::now() < deadline, "the PNG preload never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A ready file-backed document at any path, so relative pictures resolve.
fn open_temp_document(state: &mut WindowState, path: &Path, text: &str) {
    let id = state.shared.docs_mut().allocate_id();
    let title = path.file_name().and_then(|name| name.to_str()).unwrap_or("note.md");
    let doc = Rc::new(Document::ready(id, DocumentKey::File(path.to_path_buf()), title, SourceBuffer::new(text)));
    let revision = doc.revision();
    state.shared.docs_mut().insert_ready(doc.clone());
    state.shared.store_parsed(id, revision, Ok(Arc::new(lmdv_md::parse_with_revision(text, revision))));
    state.attach_document(doc);
}

fn click_key(runtime: &mut Runtime<WindowState>, key: &str) {
    let (x, y) = runtime.find(key).unwrap_or_else(|| panic!("no {key}"));
    runtime.pointer_move(x, y);
    runtime.pointer_down(x, y);
    runtime.pointer_up(x, y);
    runtime.frame(0.0);
}

#[test]
fn the_copy_menu_lists_paste_sizes_and_save_for_a_picture() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "copy-menu.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    open_active(&mut state, media);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    assert!(runtime.find(LIGHTBOX_COPY).is_some(), "the title bar has the copy button");
    click_key(&mut runtime, LIGHTBOX_COPY);
    assert!(runtime.state.lightbox().expect("open").menu_open(), "the click opened the menu");

    let elements = runtime.elements();
    let menu = by_key(&elements, &format!("{LIGHTBOX_MENU}-menu"));
    assert_eq!(menu.role, "menu");
    for key in [format!("{LIGHTBOX_MENU}-item0"), format!("{LIGHTBOX_MENU}-item1"), format!("{LIGHTBOX_MENU}-item3")] {
        assert!(elements.iter().any(|element| element.key.as_deref() == Some(key.as_str())), "missing {key}");
    }
    let texts: Vec<&str> = elements.iter().filter_map(|element| element.text.as_deref()).collect();
    assert!(texts.contains(&"Copy as image"), "{texts:?}");
    assert!(texts.contains(&"Copy as large image"), "{texts:?}");
    assert!(texts.contains(&"Save as PNG…"), "{texts:?}");
    assert!(texts.contains(&"PNG, 2× for crisp pasting"), "{texts:?}");
    assert!(texts.contains(&"PNG at 4×, for print or slides"), "{texts:?}");

    // The menu keeps the dialog modal: an app shortcut acts on neither.
    let tabs = runtime.state.tabs().len();
    runtime.key(KeyEvent { key: Key::Command('w'), mods: Mods { ctrl: true, cmd: true, ..Mods::default() } });
    runtime.frame(0.0);
    assert!(runtime.state.lightbox().is_some(), "Ctrl+W must not close the preview through the menu");
    assert_eq!(runtime.state.tabs().len(), tabs);
}

#[test]
fn escape_closes_the_copy_menu_before_the_preview() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "copy-escape.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    open_active(&mut state, media);
    let mut runtime = runtime(state);
    runtime.frame(0.0);
    click_key(&mut runtime, LIGHTBOX_COPY);
    assert!(runtime.state.lightbox().expect("open").menu_open());

    runtime.key(plain(Key::Escape));
    runtime.frame(0.0);
    let lightbox = runtime.state.lightbox().expect("the preview stays open");
    assert!(!lightbox.menu_open(), "the first Escape closes the menu");
    assert_eq!(focused(&runtime.elements()).as_deref(), Some(LIGHTBOX_COPY), "focus returns to the copy button");

    runtime.key(plain(Key::Escape));
    runtime.frame(0.0);
    assert!(runtime.state.lightbox().is_none(), "the second Escape closes the preview");
}

#[test]
fn a_diagram_gets_the_copy_and_save_menu_and_png_rasters() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    state.open_synthetic("Diagram", "```mermaid\nflowchart LR\n    A[Source] --> B[Done]\n```\n");
    let media = diagram_ref(&state);
    open_active(&mut state, media);
    let board = rune::clipboard::memory();
    let mut runtime = runtime(state);
    runtime.use_clipboard(&board);
    runtime.frame(0.0);
    assert!(runtime.find(LIGHTBOX_COPY).is_some(), "a diagram now has the copy/save menu (OI-140/OI-181)");

    let deadline = Instant::now() + Duration::from_secs(20);
    while !runtime.state.lightbox().is_some_and(|lightbox| lightbox.png_ready()) {
        assert!(Instant::now() < deadline, "the diagram rasters never finished");
        std::thread::sleep(Duration::from_millis(5));
        runtime.frame(0.0);
    }
    let lightbox = runtime.state.lightbox().expect("the preview stays open");
    let two = lightbox.png_bytes().expect("the 2× raster");
    let four = lightbox.png_bytes_at(4).expect("the 4× raster");
    assert!(two.starts_with(b"\x89PNG\r\n\x1a\n"), "the 2× raster is a PNG");
    assert!(four.starts_with(b"\x89PNG\r\n\x1a\n"), "the 4× raster is a PNG");
    let two_width = u32::from_be_bytes(two[16..20].try_into().unwrap());
    let four_width = u32::from_be_bytes(four[16..20].try_into().unwrap());
    assert!(two_width > 0, "the raster has pixels");
    assert!(four_width > two_width, "4× is wider than 2×: {four_width} vs {two_width}");
    assert!(lightbox.png_unavailable().is_none());

    // Copy as large image puts the 4× PNG on the clipboard.
    runtime.update(|state: &mut WindowState, tx: &mut Tx| state.copy_lightbox_png(4, tx));
    let copied = board.content().expect("clipboard content");
    let image = copied.image().expect("a diagram image on the clipboard");
    assert_eq!(image.size().0, four_width, "the clipboard carries the 4× raster");
    assert_eq!(runtime.state.ui.notice.as_ref().expect("a notice").text, "Image copied to the clipboard.");

    // Save as PNG writes the 2× raster atomically.
    let path = temp.path().join("diagram.png");
    runtime.update(|state: &mut WindowState, _: &mut Tx| state.start_lightbox_save(path.clone()));
    let deadline = Instant::now() + Duration::from_secs(20);
    while !runtime.state.poll_lightbox() {
        assert!(Instant::now() < deadline, "the diagram save never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
    let written = std::fs::read(&path).expect("the saved diagram");
    assert!(written.starts_with(b"\x89PNG\r\n\x1a\n"), "the saved diagram is a PNG");
    assert!(written.len() > 500, "the saved diagram has real ink: {} bytes", written.len());
}

#[test]
fn copy_as_image_puts_the_picture_png_on_the_clipboard() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "copy-png.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    open_active(&mut state, media);
    wait_for_png(&state);
    let (mut runtime, board) = clipboard_runtime(state);
    runtime.update(|state: &mut WindowState, tx: &mut Tx| state.copy_lightbox_png(2, tx));

    let copied = board.content().expect("clipboard content");
    let image = copied.image().expect("an image on the clipboard");
    let source = std::fs::read(fixtures().join("img/test.png")).unwrap();
    assert_eq!(image.png(), source.as_slice(), "the picture's own PNG bytes");
    assert_eq!(image.size(), (320, 140));
    assert_eq!(runtime.state.ui.notice.as_ref().expect("a notice").text, "Image copied to the clipboard.");
}

#[test]
fn save_as_png_writes_the_picture_through_the_worker() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "save-png.md", IMAGE_MARKDOWN);
    let media = image_ref(&state);
    open_active(&mut state, media);
    wait_for_png(&state);

    let dir = temp.path().join("out");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("landscape.png");
    state.start_lightbox_save(path.clone());
    let deadline = Instant::now() + Duration::from_secs(10);
    while !state.poll_lightbox() {
        assert!(Instant::now() < deadline, "the save worker never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
    let source = std::fs::read(fixtures().join("img/test.png")).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), source, "the picture was written byte for byte");
    let notice = state.ui.notice.as_ref().expect("a success notice");
    assert!(notice.text.starts_with("Image saved to"), "{}", notice.text);
}

#[test]
fn a_picture_that_is_not_png_is_converted_for_copy_and_save() {
    let temp = TempDir::new().unwrap();
    let doc = temp.path().join("notes.md");
    std::fs::write(&doc, "# Inline\n\nA sentence with ![Pixels](./inline.gif) inside it.\n").unwrap();
    // A 1×1 GIF: the reader decodes it and V-09 converts it to PNG.
    std::fs::write(
        temp.path().join("inline.gif"),
        [
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff,
            0xff, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00,
            0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
        ],
    )
    .unwrap();
    let mut state = window(&temp);
    let text = std::fs::read_to_string(&doc).unwrap();
    open_temp_document(&mut state, &doc, &text);
    let media = crate::lightbox::inline_media_refs(&state).into_iter().next().expect("the inline picture");
    let mut tx = Tx::default();
    state.open_inline_media(&media.block, media.index, &mut tx);
    assert!(state.lightbox().is_some(), "the reader can show a GIF");
    wait_for_png(&state);

    // V-09: the GIF was decoded and re-encoded as a real PNG, ready for both
    // the clipboard and a save, at its own natural size.
    let (mut runtime, board) = clipboard_runtime(state);
    runtime.update(|state: &mut WindowState, tx: &mut Tx| state.copy_lightbox_png(2, tx));
    let copied = board.content().expect("clipboard content");
    let image = copied.image().expect("an image on the clipboard");
    assert_eq!(image.size(), (1, 1));
    assert!(image.png().starts_with(b"\x89PNG\r\n\x1a\n"), "the GIF was converted to PNG");

    let path = temp.path().join("converted.png");
    runtime.update(|state: &mut WindowState, _: &mut Tx| state.start_lightbox_save(path.clone()));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !runtime.state.poll_lightbox() {
        assert!(Instant::now() < deadline, "the converted save never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
    let written = std::fs::read(&path).expect("the saved file");
    assert!(written.starts_with(b"\x89PNG\r\n\x1a\n"), "Save as PNG writes the converted PNG");
}

#[test]
fn a_picture_source_with_a_query_and_escapes_is_copied_from_its_file() {
    // V-08/V-09: the copy/save worker resolves the same URL path the reader
    // did, so a fetch hint and percent escapes never become the file name.
    let temp = TempDir::new().unwrap();
    let doc = temp.path().join("query.md");
    std::fs::write(&doc, "# Query\n\n![Pixels](./my%20pic.png?raw=1)\n").unwrap();
    std::fs::copy(fixtures().join("img/test.png"), temp.path().join("my pic.png")).unwrap();
    let text = std::fs::read_to_string(&doc).unwrap();
    let mut state = window(&temp);
    open_temp_document(&mut state, &doc, &text);
    let media = image_ref(&state);
    assert_eq!(media.source, "./my%20pic.png?raw=1");
    open_active(&mut state, media);
    wait_for_png(&state);

    let (mut runtime, board) = clipboard_runtime(state);
    runtime.update(|state: &mut WindowState, tx: &mut Tx| state.copy_lightbox_png(4, tx));
    let copied = board.content().expect("clipboard content");
    let image = copied.image().expect("an image on the clipboard");
    let source = std::fs::read(fixtures().join("img/test.png")).unwrap();
    assert_eq!(image.png(), source.as_slice(), "the file behind the decoded URL path");
}

#[test]
fn an_svg_picture_is_rasterized_for_copy_and_save() {
    // OI-196/V-09: an SVG file has no PNG bytes of its own, so the copy worker
    // rasterizes it through `lmdv_export::raster::svg_to_png` at twice its
    // intrinsic size on white. The bytes then take the ordinary PNG path.
    let temp = TempDir::new().unwrap();
    let doc = temp.path().join("svg.md");
    std::fs::write(&doc, "# SVG\n\n![Drawing](./drawing.svg)\n").unwrap();
    std::fs::write(
        temp.path().join("drawing.svg"),
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#0969da"/></svg>"##,
    )
    .unwrap();
    let text = std::fs::read_to_string(&doc).unwrap();
    let mut state = window(&temp);
    open_temp_document(&mut state, &doc, &text);
    let media = image_ref(&state);
    open_active(&mut state, media);
    assert!(state.lightbox().is_some(), "the SVG itself opens in the preview");
    wait_for_png(&state);

    let lightbox = state.lightbox().expect("the preview");
    assert!(lightbox.png_unavailable().is_none(), "the vector picture became a PNG");
    let bytes = lightbox.png_bytes().expect("ready raster bytes");
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "a real PNG");
    assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 40, "2× intrinsic width");
    assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 20, "2× intrinsic height");

    // The ordinary copy path hands the rasterized bytes to the clipboard.
    let (mut runtime, board) = clipboard_runtime(state);
    runtime.update(|state: &mut WindowState, tx: &mut Tx| state.copy_lightbox_png(2, tx));
    let copied = board.content().expect("clipboard content");
    let image = copied.image().expect("an image on the clipboard");
    assert!(image.png().starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(image.png().len(), bytes.len(), "the same raster the worker made");
}

#[test]
fn an_inline_picture_opens_with_the_same_preview_from_its_block() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "inline.md", INLINE_MARKDOWN);
    assert!(media_refs(&state).is_empty(), "the renderer hands out no block media for this page");
    let media = crate::lightbox::inline_media_refs(&state)
        .into_iter()
        .next()
        .expect("the inline picture is materialised from the model");
    assert_eq!(media.source, "./img/test.png");
    assert_eq!(media.alt, "Inline landscape");
    assert!(state.media_for_block(&media.block).is_some(), "the block menu finds it");

    let mut tx = Tx::default();
    state.open_media_block(&media.block, &mut tx);
    let lightbox = state.lightbox().expect("the inline picture opens the preview");
    assert_eq!(lightbox.media.block, media.block);
    assert_eq!(lightbox.caption, "Inline landscape");
    assert_eq!(lightbox.title, "test.png");
    assert_eq!(lightbox.opener, media.block, "focus returns to the paragraph");
    lightbox.image.as_ref().expect("the picture handle").wait().expect("the fixture decodes");

    let mut runtime = runtime(state);
    runtime.frame(0.0);
    let elements = runtime.elements();
    assert_eq!(by_key(&elements, LIGHTBOX_STAGE).label.as_deref(), Some("Inline landscape"));
    for key in [LIGHTBOX_ZOOM_OUT, LIGHTBOX_ZOOM_LEVEL, LIGHTBOX_ZOOM_IN, LIGHTBOX_FIT, LIGHTBOX_WIDTH] {
        assert!(elements.iter().any(|element| element.key.as_deref() == Some(key)), "missing {key}");
    }
    assert!(runtime.find(LIGHTBOX_COPY).is_some(), "the inline picture gets the same menu");
}

#[test]
fn the_inline_open_path_used_by_the_renderer_link_opens_the_indexed_picture() {
    let temp = TempDir::new().unwrap();
    let mut state = window(&temp);
    open_file_document(&mut state, "inline-index.md", INLINE_MARKDOWN);
    let media = crate::lightbox::inline_media_refs(&state).into_iter().next().expect("a picture");
    assert_eq!(media.index, 0, "the first picture of its block");

    let mut tx = Tx::default();
    state.open_inline_media(&media.block, 0, &mut tx);
    assert!(state.lightbox().is_some());
    state.close_lightbox();

    state.open_inline_media(&media.block, 9, &mut tx);
    assert!(state.lightbox().is_none(), "an index that names no picture opens nothing");
}
