//! Document images for the reading surface.
//!
//! The renderer asks for a picture by the URL written in the Markdown; this
//! module owns what the app must add: resolution against the document folder
//! under [`lmdv_io::PathPolicy`] (OI-009), one [`rune::ImageCache`] per window
//! (wired to the window's `Waker`), and the missing-image answer that lets the
//! renderer draw its placeholder (M-24, M-25, OI-047).
//!
//! Resolution is cached per `(document folder, source)`, so a frame never
//! repeats the path policy's stat for the same picture. A picture that is still
//! decoding answers `None` for now; [`ImageCacheState::refresh`] reports when
//! one became ready so `poll` rebuilds the view with its real aspect ratio.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};

use lmdv_io::PathPolicy;
use lmdv_render::ImageResolver;
use rune::Image;
use rune::Waker;

use crate::jobs::{ImageProbe, ImageProbeKey, JobHub};

/// The most resolved sources remembered per window.
pub const RESOLVED_CACHE: usize = 512;
/// The most decoded picture handles kept alive (the element tree keeps the
/// pictures it draws alive on its own).
pub const HANDLE_CACHE: usize = 256;

/// `(document folder, source, outside allowed, source is a URL path)`.
type ResolveKey = (PathBuf, String, bool, bool);

/// One answered path-policy probe keeps the flags a waiting view needs to
/// know that it should rebuild and the waker that schedules that poll.
type ProbeWaiter = (Arc<AtomicBool>, Waker);

/// Process-wide image path probes (OI-049).
///
/// `PathPolicy::resolve_asset` stats and canonicalizes files, so the first
/// resolution of a picture must not happen in the view. Only the real entry
/// point enables the worker path ([`enable_worker_probes`]); unit tests and
/// headless scenes (which already block on `Image::wait`) keep the inline
/// probe, so their single frame stays deterministic.
/// Set once by the real entry point; tests and headless runs leave it false so
/// their resolutions stay inline and deterministic.
static USE_WORKER_PROBES: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct ProbeHub {
    jobs: JobHub,
    tx: Option<Sender<ImageProbe>>,
    rx: Option<Receiver<ImageProbe>>,
    /// Every probe's answer, including a `None` for a refused or missing file.
    answered: HashMap<ImageProbeKey, Option<PathBuf>>,
    /// Keys in flight and the views waiting on each of them.
    pending: HashMap<ImageProbeKey, Vec<ProbeWaiter>>,
}

fn probes() -> &'static Mutex<ProbeHub> {
    static PROBES: OnceLock<Mutex<ProbeHub>> = OnceLock::new();
    PROBES.get_or_init(|| Mutex::new(ProbeHub::default()))
}

/// Switches interactive runs to worker-side path probing. Called once, by the
/// real entry point; tests and headless runs never call it.
pub(crate) fn enable_worker_probes() {
    let mut hub = probes().lock().unwrap_or_else(|error| error.into_inner());
    if hub.tx.is_none() {
        let (tx, rx) = channel();
        hub.tx = Some(tx);
        hub.rx = Some(rx);
    }
    USE_WORKER_PROBES.store(true, Ordering::Relaxed);
}

/// The uncached path policy check, used by headless scenes and tests.
fn inline_probe(root: &Path, source: &str, allow_outside: bool) -> Option<PathBuf> {
    PathPolicy::new(root).with_allow_outside(allow_outside).resolve_asset(Path::new(source)).ok().map(|file| file.path)
}

/// Looks up a normalized source, starting one worker probe on a miss.
///
/// A miss answers `None` for this frame; the waiter's flag and waker are
/// recorded so `poll_probes` can make the view rebuild when the answer lands.
fn probe_path(
    root: &Path,
    source: &str,
    allow_outside: bool,
    url_path: bool,
    dirty: Arc<AtomicBool>,
    waker: Waker,
) -> Option<PathBuf> {
    let mut hub = probes().lock().unwrap_or_else(|error| error.into_inner());
    // The channel is created on first use, so a test that opts one cache into
    // worker probing needs no global setup (and cannot change other tests).
    if hub.tx.is_none() {
        let (tx, rx) = channel();
        hub.tx = Some(tx);
        hub.rx = Some(rx);
    }
    let key = ImageProbeKey { root: root.to_path_buf(), source: source.to_owned(), allow_outside, url_path };
    if let Some(path) = hub.answered.get(&key) {
        return path.clone();
    }
    let waiter = (dirty, waker);
    if let Some(waiters) = hub.pending.get_mut(&key) {
        waiters.push(waiter);
        return None;
    }
    // A spawn failure posts an empty answer, so this waiter is always woken.
    if let Some(tx) = hub.tx.clone() {
        hub.jobs.probe_image(tx, key.clone());
    }
    hub.pending.insert(key, vec![waiter]);
    None
}

/// Drains worker answers on the UI thread and wakes the views that asked.
///
/// Returns whether an answer landed, so `app::poll` rebuilds at least the
/// polling window even if its waker was detached.
pub(crate) fn poll_probes() -> bool {
    let mut landed = false;
    loop {
        let message = {
            let hub = probes().lock().unwrap_or_else(|error| error.into_inner());
            match hub.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                Some(message) => message,
                None => break,
            }
        };
        let mut hub = probes().lock().unwrap_or_else(|error| error.into_inner());
        if let Some(waiters) = hub.pending.remove(&message.key) {
            for (dirty, waker) in waiters {
                dirty.store(true, Ordering::Relaxed);
                waker();
            }
        }
        hub.answered.insert(message.key, message.path);
        landed = true;
    }
    landed
}

/// Forgets every worker answer, so a document or policy change re-probes and a
/// picture that appeared or disappeared since the first frame is seen again.
fn clear_probes() {
    let mut hub = probes().lock().unwrap_or_else(|error| error.into_inner());
    hub.answered.clear();
    hub.pending.clear();
}

/// One window's image state.
pub struct ImageCacheState {
    cache: rune::ImageCache,
    /// Each [`ResolveKey`] → validated path, or `None` when missing or refused.
    resolved: RefCell<HashMap<ResolveKey, Option<PathBuf>>>,
    /// Decoded handles, so a second frame asks the cache for a live entry.
    handles: RefCell<HashMap<PathBuf, Image>>,
    /// `(ready, failed)` at the last `refresh`.
    settled: Cell<(usize, usize)>,
    /// Set by a worker probe that this view asked for; `refresh` turns it into
    /// a view rebuild, so the resolved path is picked up on the next frame.
    dirty: Arc<AtomicBool>,
    /// The same UI waker the decoder uses, for the path probe's answer.
    waker: Waker,
    /// Whether path policy checks go to a worker. Off in tests and headless
    /// scenes, so a single-frame gate never depends on a probe landing.
    worker_probes: bool,
    /// Block until a picture decodes: headless gates that capture one frame.
    wait: bool,
}

impl ImageCacheState {
    /// A cache decoding on its own threads; `waker` wakes the UI when a
    /// picture lands. `wait` blocks each resolve until the decode finishes and
    /// is only for deterministic headless scenes.
    pub fn new(waker: Waker, wait: bool) -> Self {
        Self {
            cache: rune::ImageCache::new(waker.clone()),
            resolved: RefCell::new(HashMap::new()),
            handles: RefCell::new(HashMap::new()),
            settled: Cell::new((0, 0)),
            dirty: Arc::new(AtomicBool::new(false)),
            waker,
            worker_probes: !wait && USE_WORKER_PROBES.load(Ordering::Relaxed),
            wait,
        }
    }

    /// The picture for `src`, resolved against `root`, or `None` when it is
    /// missing, broke, or was refused by the path policy.
    ///
    /// A picture that is still decoding is returned as a handle: the element
    /// binds its `Pixels` and draws as soon as they land. [`Self::refresh`]
    /// tells `poll` when to rebuild for the real aspect ratio, and when a
    /// decoder failed so the placeholder replaces the empty frame.
    pub fn resolve(&self, root: &Path, src: &str, size_pt: f32, scale: f32) -> Option<Image> {
        self.resolve_with(root, src, size_pt, scale, false, true)
    }

    /// An image a link named (L-06), which may leave the document folder the
    /// way L-07's document links may; system, device and network paths stay
    /// refused. `src` is the resolved filesystem path, not a URL spelling, so
    /// `%`, `#` and `?` in a real file name stay literal.
    pub(crate) fn resolve_linked(&self, root: &Path, src: &str, size_pt: f32, scale: f32) -> Option<Image> {
        self.resolve_with(root, src, size_pt, scale, true, false)
    }

    fn resolve_with(
        &self,
        root: &Path,
        src: &str,
        size_pt: f32,
        scale: f32,
        allow_outside: bool,
        url_path: bool,
    ) -> Option<Image> {
        // M-24: a query (`?raw=1`) is a fetch hint and a fragment
        // (`#gh-dark-mode-only`) is a theme hint; neither is part of the file
        // on disk, so both are dropped before resolution. The renderer keeps
        // reading the fragment from the source as written. L-15: the relative
        // spelling is a URL path, so `%20` names a space. A linked image
        // already arrives as a decoded filesystem path.
        let source = if url_path {
            let (path, _fragment) = lmdv_md::url_path_and_fragment(src);
            lmdv_md::decode_url_path(path.trim())
        } else {
            std::borrow::Cow::Borrowed(src.trim())
        };
        let src = source.as_ref();
        if src.is_empty() || (url_path && is_not_a_local_path(root, src)) {
            return None;
        }
        let key = (root.to_path_buf(), src.to_owned(), allow_outside, url_path);
        let cached = self.resolved.borrow().get(&key).cloned();
        let path = match cached {
            Some(path) => path,
            None if self.wait || !self.worker_probes => {
                // Headless scenes and tests need the answer in this frame, so
                // the stat stays inline (headless then blocks on `Image::wait`).
                let path = inline_probe(root, src, allow_outside);
                let mut resolved = self.resolved.borrow_mut();
                if resolved.len() >= RESOLVED_CACHE {
                    resolved.clear();
                }
                resolved.insert(key, path.clone());
                path
            }
            None => {
                // Interactive windows never stat in the view. A miss here is
                // "not answered yet": do not cache it, or the rebuild after the
                // worker answers would keep drawing the placeholder.
                let path = probe_path(root, src, allow_outside, url_path, Arc::clone(&self.dirty), self.waker.clone())?;
                let mut resolved = self.resolved.borrow_mut();
                if resolved.len() >= RESOLVED_CACHE {
                    resolved.clear();
                }
                resolved.insert(key, Some(path.clone()));
                Some(path)
            }
        };
        let path = path?;
        let image = self.cache.file(&path, size_pt, scale);
        if self.wait {
            let _ = image.wait();
        }
        if image.error().is_some() {
            return None;
        }
        let mut handles = self.handles.borrow_mut();
        if handles.len() >= HANDLE_CACHE && !handles.contains_key(&path) {
            handles.clear();
        }
        handles.insert(path, image.clone());
        Some(image)
    }

    /// Whether a picture became ready or failed since the last call; the view
    /// must rebuild to draw it at its real size or swap in the placeholder.
    pub fn refresh(&self) -> bool {
        // OI-049: a path policy answer landed while the view was drawing. The
        // next frame resolves again (now from the worker's cache) and binds the
        // picture instead of holding the placeholder.
        if self.dirty.swap(false, Ordering::Relaxed) {
            return true;
        }
        let handles = self.handles.borrow();
        let ready = handles.values().filter(|image| image.is_ready()).count();
        let failed = handles.values().filter(|image| image.error().is_some()).count();
        if (ready, failed) == self.settled.get() {
            return false;
        }
        self.settled.set((ready, failed));
        true
    }

    /// How many pictures are known, ready or decoding.
    pub fn known(&self) -> usize {
        self.handles.borrow().len()
    }

    /// How many pictures are ready to draw.
    pub fn ready(&self) -> usize {
        self.handles.borrow().values().filter(|image| image.is_ready()).count()
    }

    /// Forgets every path and handle, for a document or policy change.
    ///
    /// The worker answers go too: a picture that appeared, disappeared or
    /// moved since the first frame must be re-probed under the current policy.
    pub fn clear(&self) {
        self.resolved.borrow_mut().clear();
        self.handles.borrow_mut().clear();
        self.settled.set((0, 0));
        self.dirty.store(false, Ordering::Relaxed);
        clear_probes();
    }
}

/// Whether `src` names something other than a local file: a URL, a data URL,
/// a device path or a Windows drive. A network spelling is local only on the
/// document's own share (M-30/L-10).
fn is_not_a_local_path(root: &Path, src: &str) -> bool {
    if crate::links::is_network_spelling(src) {
        return !lmdv_io::on_document_share(root, Path::new(src));
    }
    if let Some((scheme, _)) = src.split_once(':') {
        let looks_like_scheme = !scheme.is_empty()
            && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if looks_like_scheme {
            // `C:` is a Windows drive, not a scheme; the path policy refuses it
            // too, but rejecting it here keeps the URL check honest.
            return true;
        }
    }
    false
}

/// The [`ImageResolver`] the renderer sees for the active document.
///
/// The window's [`ImageCacheState`] is created on the first picture, so a
/// window that never draws one starts no decoder threads.
pub struct WindowImages<'a> {
    cell: &'a std::cell::OnceCell<ImageCacheState>,
    waker: Waker,
    wait: bool,
    root: Option<PathBuf>,
    size_pt: f32,
    scale: f32,
}

impl<'a> WindowImages<'a> {
    /// A resolver rooted at `root` (the document's folder), sizing SVG
    /// rasterization for `size_pt` at `scale`. `wait` is only true for a
    /// deterministic headless gate.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cell: &'a std::cell::OnceCell<ImageCacheState>,
        waker: Waker,
        wait: bool,
        root: Option<PathBuf>,
        size_pt: f32,
        scale: f32,
    ) -> Self {
        Self { cell, waker, wait, root, size_pt: size_pt.max(1.0), scale: scale.max(0.01) }
    }
}

impl ImageResolver for WindowImages<'_> {
    fn resolve(&self, src: &str, _alt: &str, _title: &str) -> Option<Image> {
        let root = self.root.as_deref()?;
        let state = self.cell.get_or_init(|| ImageCacheState::new(self.waker.clone(), self.wait));
        state.resolve(root, src, self.size_pt, self.scale)
    }
}

/// A no-op waker for tests and headless scenes that never install one.
pub fn quiet_waker() -> Waker {
    Arc::new(|| {})
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    use super::*;

    fn state() -> ImageCacheState {
        ImageCacheState::new(quiet_waker(), false)
    }

    /// A valid 1x1 PNG.
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00,
        0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D,
        0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn a_local_picture_resolves_and_decodes() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::write(root.join("a.png"), PNG).unwrap();
        let images = state();
        let image = images.resolve(root, "a.png", 32.0, 2.0).expect("a picture handle");
        image.wait().expect("decodes");
        assert_eq!(image.size(), Some([1, 1]));
        assert!(images.refresh(), "the decode changed the ready count");
        assert!(!images.refresh(), "nothing changed since the first count");
        assert_eq!(images.ready(), 1);
    }

    #[test]
    fn a_missing_picture_is_a_placeholder_not_an_error() {
        let temp = TempDir::new().unwrap();
        let images = state();
        assert!(images.resolve(temp.path(), "none.png", 32.0, 2.0).is_none());
        assert_eq!(images.known(), 0);
    }

    #[test]
    fn paths_outside_the_document_folder_are_refused() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("docs");
        fs::create_dir_all(&root).unwrap();
        fs::write(temp.path().join("secret.png"), PNG).unwrap();
        let images = state();
        assert!(images.resolve(&root, "../secret.png", 32.0, 2.0).is_none());
        assert!(images.resolve(&root, temp.path().join("secret.png").to_str().unwrap(), 32.0, 2.0).is_none());
    }

    #[test]
    fn urls_data_urls_and_device_paths_never_touch_the_filesystem() {
        let temp = TempDir::new().unwrap();
        let images = state();
        for src in [
            "https://example.test/a.png",
            "data:image/png;base64,AAAA",
            "file:///etc/passwd",
            "//host/share/a.png",
            r"\\server\share\a.png",
            "C:/pictures/a.png",
            "/dev/null",
            "javascript:alert(1)",
        ] {
            assert!(images.resolve(temp.path(), src, 32.0, 2.0).is_none(), "{src} must be refused");
        }
    }

    #[test]
    fn a_source_fragment_is_not_part_of_the_path() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("dark.png"), PNG).unwrap();
        let images = state();
        assert!(images.resolve(temp.path(), "dark.png#gh-dark-mode-only", 32.0, 2.0).is_some());
    }

    #[test]
    fn a_source_query_is_dropped_and_the_fragment_stays_a_hint() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("pic.png"), PNG).unwrap();
        let images = state();
        for src in ["pic.png?raw=1", "pic.png?raw=1#gh-light-mode-only", "pic.png#gh-dark-mode-only"] {
            assert!(images.resolve(temp.path(), src, 32.0, 2.0).is_some(), "{src} resolves to pic.png");
        }
        assert!(images.resolve(temp.path(), "gone.png?raw=1", 32.0, 2.0).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_network_source_resolves_only_on_the_documents_own_share() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("pic.png"), PNG).unwrap();
        let spelled = PathBuf::from(format!("//{}", temp.path().to_string_lossy().trim_start_matches('/')));
        let images = state();
        assert!(
            images.resolve(&spelled, spelled.join("pic.png").to_str().unwrap(), 32.0, 2.0).is_some(),
            "the document's own share is reached (L-10)"
        );
        assert!(
            images.resolve(&spelled, "//elsewhere/share/pic.png", 32.0, 2.0).is_none(),
            "another share stays missing"
        );
        assert!(images.resolve(temp.path(), "//elsewhere/share/pic.png", 32.0, 2.0).is_none());
    }

    #[test]
    fn a_percent_encoded_source_resolves_once() {
        // L-15: a Markdown URL path names the file, so `%20` is a space.
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("my pic.png"), PNG).unwrap();
        fs::write(temp.path().join("100%.png"), PNG).unwrap();
        let images = state();
        assert!(images.resolve(temp.path(), "my%20pic.png", 32.0, 2.0).is_some());
        assert!(images.resolve(temp.path(), "my pic.png", 32.0, 2.0).is_some());
        assert!(images.resolve(temp.path(), "100%25.png", 32.0, 2.0).is_some(), "one layer: %25 is a literal %");
        assert!(images.resolve(temp.path(), "100%25.png?raw=1#gh-dark-mode-only", 32.0, 2.0).is_some());
        assert!(images.resolve(temp.path(), "%FF.png", 32.0, 2.0).is_none(), "a broken escape is lossy, not a panic");
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_filesystem_path_is_not_decoded_as_a_url() {
        // L-06: the link resolver hands over the real path, so `%`, `#` and
        // `?` in the name are literal rather than URL syntax.
        let temp = TempDir::new().unwrap();
        for name in ["100%25.png", "hash#1.png", "what?.png"] {
            fs::write(temp.path().join(name), PNG).unwrap();
        }
        let images = state();
        for name in ["100%25.png", "hash#1.png", "what?.png"] {
            let path = temp.path().join(name);
            assert!(
                images.resolve_linked(temp.path(), path.to_str().unwrap(), 32.0, 2.0).is_some(),
                "{name} stays a literal file name"
            );
        }
    }

    #[test]
    fn resolution_is_cached_per_source() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("a.png"), PNG).unwrap();
        let images = state();
        assert!(images.resolve(temp.path(), "a.png", 32.0, 2.0).is_some());
        assert!(images.resolve(temp.path(), "a.png", 32.0, 2.0).is_some());
        assert_eq!(images.resolved.borrow().len(), 1);
    }

    /// Waits for *this view's* dirty flag; `poll_probes` is process-wide and
    /// another test may drain an unrelated answer first.
    fn wait_for_refresh(images: &mut ImageCacheState) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !images.refresh() {
            assert!(Instant::now() < deadline, "the probe never woke this view");
            poll_probes();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_worker_probe_answers_off_the_view_and_is_reused_then_invalidated() {
        let temp = TempDir::new().unwrap();
        let picture = temp.path().join("a.png");
        fs::write(&picture, PNG).unwrap();
        let mut images = state();
        images.worker_probes = true;

        // The first view frame does not stat: it asks for a probe and draws no
        // picture yet. A miss is deliberately not cached locally.
        assert!(images.resolve(temp.path(), "a.png", 32.0, 2.0).is_none());
        assert!(images.resolved.borrow().is_empty(), "an unanswered probe is not a cached miss");

        wait_for_refresh(&mut images);
        let image = images.resolve(temp.path(), "a.png", 32.0, 2.0).expect("the worker's path");
        image.wait().expect("decodes");
        assert_eq!(images.resolved.borrow().len(), 1);

        // A second view with the same document folder reuses the worker answer
        // without spawning another probe.
        let mut second = state();
        second.worker_probes = true;
        assert!(second.resolve(temp.path(), "a.png", 32.0, 2.0).is_some());

        // A document/policy change drops the worker answers, so a picture that
        // appeared since the first frame is picked up on the next one.
        images.clear();
        fs::write(&picture, PNG).unwrap();
        assert!(images.resolve(temp.path(), "a.png", 32.0, 2.0).is_none(), "clearing forces a fresh probe");
        wait_for_refresh(&mut images);
        assert!(images.resolve(temp.path(), "a.png", 32.0, 2.0).is_some());
    }

    #[test]
    fn a_worker_probe_keeps_the_path_policy() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("docs");
        fs::create_dir_all(&root).unwrap();
        // The probe hub is process-wide; make these keys unique so a parallel
        // test cannot have already cached the same refusal.
        let id = temp.path().file_name().unwrap().to_string_lossy();
        let local = format!("../secret-{id}.png");
        let url = format!("https://example-{id}.test/a.png");
        let file = format!("file:///etc/passwd-{id}");
        fs::write(temp.path().join(format!("secret-{id}.png")), PNG).unwrap();
        let mut images = state();
        images.worker_probes = true;

        for src in [&local, &url, &file] {
            assert!(images.resolve(&root, src, 32.0, 2.0).is_none(), "{src} must be refused");
        }
        wait_for_refresh(&mut images);
        for src in [&local, &url, &file] {
            assert!(images.resolve(&root, src, 32.0, 2.0).is_none(), "{src} stays refused on the cached answer");
        }
    }
}
