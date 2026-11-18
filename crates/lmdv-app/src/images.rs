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
use std::sync::Arc;

use lmdv_io::PathPolicy;
use lmdv_render::ImageResolver;
use rune::Image;
use rune::Waker;

/// The most resolved sources remembered per window.
pub const RESOLVED_CACHE: usize = 512;
/// The most decoded picture handles kept alive (the element tree keeps the
/// pictures it draws alive on its own).
pub const HANDLE_CACHE: usize = 256;

/// One window's image state.
pub struct ImageCacheState {
    cache: rune::ImageCache,
    /// `(document folder, source)` → validated path, or `None` when missing
    /// or refused.
    resolved: RefCell<HashMap<(PathBuf, String), Option<PathBuf>>>,
    /// Decoded handles, so a second frame asks the cache for a live entry.
    handles: RefCell<HashMap<PathBuf, Image>>,
    /// `(ready, failed)` at the last `refresh`.
    settled: Cell<(usize, usize)>,
    /// Block until a picture decodes: headless gates that capture one frame.
    wait: bool,
}

impl ImageCacheState {
    /// A cache decoding on its own threads; `waker` wakes the UI when a
    /// picture lands. `wait` blocks each resolve until the decode finishes and
    /// is only for deterministic headless scenes.
    pub fn new(waker: Waker, wait: bool) -> Self {
        Self {
            cache: rune::ImageCache::new(waker),
            resolved: RefCell::new(HashMap::new()),
            handles: RefCell::new(HashMap::new()),
            settled: Cell::new((0, 0)),
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
        let src = src.split('#').next().unwrap_or(src).trim();
        if src.is_empty() || is_not_a_local_path(src) {
            return None;
        }
        let key = (root.to_path_buf(), src.to_owned());
        let cached = self.resolved.borrow().get(&key).cloned();
        let path = match cached {
            Some(path) => path,
            None => {
                let policy = PathPolicy::new(root);
                let path = policy.resolve_asset(Path::new(src)).ok().map(|file| file.path);
                let mut resolved = self.resolved.borrow_mut();
                if resolved.len() >= RESOLVED_CACHE {
                    resolved.clear();
                }
                resolved.insert(key, path.clone());
                path
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
    pub fn clear(&self) {
        self.resolved.borrow_mut().clear();
        self.handles.borrow_mut().clear();
        self.settled.set((0, 0));
    }
}

/// Whether `src` names something other than a local file: a URL, a data URL,
/// a device path or a Windows drive.
fn is_not_a_local_path(src: &str) -> bool {
    if src.starts_with("//") || src.starts_with(r"\\") {
        return true;
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
    fn resolution_is_cached_per_source() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("a.png"), PNG).unwrap();
        let images = state();
        assert!(images.resolve(temp.path(), "a.png", 32.0, 2.0).is_some());
        assert!(images.resolve(temp.path(), "a.png", 32.0, 2.0).is_some());
        assert_eq!(images.resolved.borrow().len(), 1);
    }
}
