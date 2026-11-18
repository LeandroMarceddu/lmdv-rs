//! The native close-request veto (Rune patch 0002): a window's native close is asked for
//! before anything hides, and the app can keep the window open. Programmatic closes are
//! already the app's decision and never ask.
//!
//! A headless run has no window manager to deliver `WindowEvent::CloseRequested`, so
//! `Runtime::close_request` runs the same gate the native host runs in `Host::close`, with the
//! app's hook read back through `AppHandle::close_request_hook`. `Host::close` routes the
//! native event through it; `Host::close_now` (a handle, `Tx::close_window`, quitting) skips it.

use rune::{App, AppSpec, Cx, El, Env, Runtime, Tx, WindowSpec, el, text};

#[derive(Default)]
struct State {
    dirty: bool,
    asked: u32,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    el().key("root").child(text(if s.dirty { "dirty" } else { "saved" }))
}

/// A save prompt: keep the window open while the document has unsaved edits.
fn ask_to_save(s: &mut State, _: &mut Tx) -> bool {
    s.asked += 1;
    s.dirty
}

fn runtime(app: App<State>) -> Runtime<State> {
    let mut rt = app.headless_runtime((320.0, 240.0), Env::new(false, 1.0));
    rt.frame(0.0);
    rt
}

#[test]
fn a_dirty_document_vetoes_the_close_and_stays_usable() {
    let app =
        App::new(AppSpec::new("close request", (320.0, 240.0), State::default, view)).on_close_request(ask_to_save);
    let hook = app.handle().close_request_hook();
    assert!(hook.is_some(), "App::on_close_request keeps its hook");
    let mut rt = runtime(app);
    rt.state.dirty = true;

    assert!(rt.close_request(hook), "a dirty document keeps the window open");
    assert_eq!(rt.state.asked, 1, "the app is asked once");
    assert!(rt.state.dirty, "a veto changes nothing");
    rt.frame(1.0 / 60.0);
    assert!(
        rt.elements().iter().any(|e| e.key.as_deref() == Some("root")),
        "the window still builds and draws its state after a veto"
    );

    // The user saved or discarded: the same close now goes ahead.
    rt.state.dirty = false;
    assert!(!rt.close_request(hook), "a clean document closes");
    assert_eq!(rt.state.asked, 2, "each close request asks once");
}

#[test]
fn a_window_spec_sets_its_own_close_request_hook() {
    let spec = WindowSpec::new("settings", AppSpec::new("settings", (240.0, 160.0), State::default, view))
        .on_close_request(ask_to_save);
    let hook = spec.app.handle().close_request_hook();
    assert!(hook.is_some(), "WindowSpec::on_close_request keeps its hook");
    let mut rt = spec.app.headless_runtime((240.0, 160.0), Env::new(false, 1.0));
    rt.frame(0.0);
    rt.state.dirty = true;
    assert!(rt.close_request(hook), "this window's hook vetoes");
    rt.state.dirty = false;
    assert!(!rt.close_request(hook), "and lets the close through when clean");
}

#[test]
fn programmatic_closes_never_ask() {
    let app =
        App::new(AppSpec::new("close request", (320.0, 240.0), State::default, view)).on_close_request(ask_to_save);
    let handle = app.handle();
    let hook = handle.close_request_hook();
    assert!(hook.is_some(), "the hook is set");
    let mut rt = runtime(app);
    rt.state.dirty = true;

    // The app's own close is already its decision: `Tx::close_window` queues a shell request
    // and asks nothing. `AppHandle::close` is the same request from another thread.
    rt.update(|_, tx| tx.close_window());
    assert_eq!(rt.state.asked, 0, "Tx::close_window skips the close-request hook");
    handle.close();
    assert_eq!(rt.state.asked, 0, "AppHandle::close skips the close-request hook");

    // Only the native request asks.
    assert!(rt.close_request(hook), "the native close is the one that asks");
    assert_eq!(rt.state.asked, 1);
}
