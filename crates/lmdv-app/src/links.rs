//! Link, `<details>` and copy actions for the rendered article.
//!
//! `lmdv-render` turns link runs into Rune links whose destination is exactly
//! what the Markdown wrote: `#slug`, `fn:label`, `fnref:label`,
//! `copy-code:<key>`, `details:<key>`, `media:<src>`, or an ordinary relative or
//! external URL. One [`LinkWire`] per window attaches the handler, and every
//! destination is classified here before anything happens (M-30, M-31, R-15).
//!
//! The rules: fragments resolve through [`MdDoc::resolve_heading`]; footnote
//! references and entries scroll inside the article; `http`/`https`/`mailto`
//! open externally on the explicit click; relative links either open a
//! Markdown tab through the normal load job (at the `#section` they name when
//! they carry one) or, when their spelling does not already end in a Markdown
//! extension, probe the wiki-style spellings on a worker first (L-03);
//! network/device spellings and script URLs are refused with a notice. A
//! click's placement follows its modifiers and button (L-11): plain in front,
//! Ctrl or middle in a background tab, Ctrl+Shift in a foreground tab, Shift in
//! a new window. Nothing here follows a link on its own.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use lmdv_core::{DocumentId, DocumentKey, TabId};
use lmdv_io::Launch;
use lmdv_md::{Block, MdDoc};
use lmdv_render::{DetailsState, LinkWire, heading_key};
use rune::{Mods, PointerButton, Tx};

use crate::commands::MARKDOWN_EXTENSIONS;
use crate::window::{TabView, WindowState};
use crate::windows::{WindowKind, WindowOpenRequest};

/// What one link destination means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// An in-document fragment, without `#`.
    Fragment(String),
    /// A footnote entry, by label (`fn:`).
    Footnote(String),
    /// The inline reference of a footnote, by label (`fnref:`).
    FootnoteRef(String),
    /// A code block's copy action.
    CopyCode(String),
    /// A `<details>`/front-matter card toggle.
    Details(String),
    /// A media placeholder's external open.
    Media(String),
    /// An inline (text-span) picture, by block key and object index.
    InlineImage { block: String, index: usize },
    /// `http`, `https` or `mailto`.
    External(String),
    /// A local path from a relative or absolute destination.
    Local {
        path: PathBuf,
        markdown: bool,
        /// The `#section` the destination names, if any.
        fragment: Option<String>,
    },
    /// A destination the app refuses, with the reason to show.
    Refused(String),
}

/// How a link click places the document it opens (T-12/L-11).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkPlacement {
    /// The normal foreground open: the current window, in front. This is the
    /// app's existing plain-click behavior.
    #[default]
    Current,
    /// Ctrl+click or a middle click: a tab behind the one in front.
    BackgroundTab,
    /// Ctrl+Shift+click: a new tab in front.
    ForegroundTab,
    /// Shift+click: a window of its own.
    NewWindow,
}

impl LinkPlacement {
    /// The placement a click asks for.
    ///
    /// A middle click wins over the keyboard modifiers, as a browser's middle
    /// click does. Shift alone asks for a window; Ctrl+Shift makes it a
    /// foreground tab instead.
    pub fn for_click(mods: Mods, button: PointerButton) -> Self {
        if button == PointerButton::Middle {
            return Self::BackgroundTab;
        }
        if mods.shift && !mods.ctrl {
            Self::NewWindow
        } else if mods.ctrl && mods.shift {
            Self::ForegroundTab
        } else if mods.ctrl {
            Self::BackgroundTab
        } else {
            Self::Current
        }
    }
}

impl TabView {
    /// Toggles a `<details>` or the front-matter card. Returns whether the
    /// state changed.
    pub fn toggle_details(&mut self, key: &str) -> bool {
        if self.details_open.contains(key) {
            self.details_open.remove(key);
            false
        } else if self.details_open.len() < MAX_OPEN_DETAILS {
            self.details_open.insert(key.to_owned());
            true
        } else {
            false
        }
    }
}

impl DetailsState for TabView {
    fn is_open(&self, key: &str) -> bool {
        self.details_open.contains(key)
    }
}

/// The most cards one tab keeps open; a generated document cannot grow the set
/// without bound.
pub const MAX_OPEN_DETAILS: usize = 128;

impl WindowState {
    /// The one link wire this window's article uses.
    pub(crate) fn link_wire(&self) -> LinkWire {
        self.cached_link_wire().clone()
    }

    /// Classifies a rendered link destination for the active document.
    pub fn classify_link(&self, dest: &str) -> LinkTarget {
        if let Some(fragment) = dest.strip_prefix('#') {
            return if fragment.is_empty() {
                LinkTarget::Refused(lmdv_i18n::t!("link.noTarget").to_owned())
            } else {
                LinkTarget::Fragment(fragment.to_owned())
            };
        }
        if let Some(label) = dest.strip_prefix("fn:") {
            return LinkTarget::Footnote(label.to_owned());
        }
        if let Some(label) = dest.strip_prefix("fnref:") {
            return LinkTarget::FootnoteRef(label.to_owned());
        }
        if let Some(key) = dest.strip_prefix("copy-code:") {
            return LinkTarget::CopyCode(key.to_owned());
        }
        if let Some(key) = dest.strip_prefix("details:") {
            return LinkTarget::Details(key.to_owned());
        }
        if let Some(src) = dest.strip_prefix("media:") {
            return LinkTarget::Media(src.to_owned());
        }
        if let Some(spec) = dest.strip_prefix("image:")
            && let Some((block, index)) = spec.rsplit_once(':')
            && let Ok(index) = index.parse::<usize>()
        {
            return LinkTarget::InlineImage { block: block.to_owned(), index };
        }
        if let Some((scheme, _)) = dest.split_once(':') {
            let scheme = scheme.to_ascii_lowercase();
            if matches!(scheme.as_str(), "http" | "https" | "mailto") {
                return LinkTarget::External(dest.to_owned());
            }
            if scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            {
                return LinkTarget::Refused(lmdv_i18n::t!("link.refusedScheme", "scheme" => scheme));
            }
        }
        if dest.starts_with("//") || dest.starts_with(r"\\") {
            return LinkTarget::Refused(lmdv_i18n::t!("link.networkPath").to_owned());
        }
        self.local_target(dest, false)
    }

    /// Runs one rendered link destination with the click's placement.
    ///
    /// The window records the held modifiers through `WindowSpec::on_modifiers`;
    /// a middle click is reported by Rune in [`Tx::button`], so both decide the
    /// placement here (L-11).
    pub fn open_link(&mut self, dest: &str, tx: &mut Tx) {
        let placement = LinkPlacement::for_click(self.link_modifiers(), tx.button());
        self.open_link_placed(dest, placement, tx);
    }

    /// [`Self::open_link`] with an explicit placement.
    pub(crate) fn open_link_placed(&mut self, dest: &str, placement: LinkPlacement, tx: &mut Tx) {
        match self.classify_link(dest) {
            LinkTarget::Fragment(fragment) => self.jump_fragment(&fragment, tx),
            // OI-162: a footnote jump glides like a heading when its target is
            // already on screen and stays instant when it is out of view.
            LinkTarget::Footnote(label) => self.jump_to_footnote(&label, false, tx),
            LinkTarget::FootnoteRef(label) => self.jump_to_footnote(&label, true, tx),
            LinkTarget::CopyCode(key) => match self.code_text(&key) {
                Some(code) => {
                    tx.copy(code);
                    self.notice(lmdv_i18n::t!("ctx.copyCode"));
                }
                None => self.notice(lmdv_i18n::t!("link.copyUnavailable")),
            },
            LinkTarget::Details(key) => {
                if let Some(tab) = self.active_tab_mut()
                    && tab.view.toggle_details(&key)
                {
                    tx.instant();
                }
            }
            LinkTarget::Media(src) => match self.classify_media(&src) {
                Some(path) => self.open_external(&path),
                None => self.notice(lmdv_i18n::t!("link.openFailed", "name" => src)),
            },
            LinkTarget::InlineImage { block, index } => self.open_inline_media(&block, index, tx),
            LinkTarget::External(url) => {
                if let Err(error) = tx.open_url(&url) {
                    self.notice(lmdv_i18n::t!("msg.openFailed", "name" => url, "err" => error));
                }
            }
            // L-03: a spelling that already names a Markdown file is the first
            // candidate itself and opens at once, keeping same-document jumps,
            // Back/Forward recording and the editor lease rules on the frame
            // the click happened. Every other local spelling probes the wiki
            // candidates on a worker.
            LinkTarget::Local { path, markdown: true, fragment } => {
                self.place_linked_document(path, fragment, placement, tx);
            }
            LinkTarget::Local { path, markdown: false, fragment } => {
                self.resolve_local_link(path, dest, fragment, placement);
            }
            LinkTarget::Refused(reason) => self.notice(reason),
        }
    }

    /// Starts the wiki-style probe for a local link whose spelling does not end
    /// in a Markdown extension (L-03). The worker posts
    /// [`crate::jobs::JobMsg::LinkResolved`] with this click's context.
    fn resolve_local_link(&mut self, path: PathBuf, dest: &str, fragment: Option<String>, placement: LinkPlacement) {
        let display = dest.split_once('#').map_or(dest, |(path, _)| path).to_owned();
        let Some(root) = self.active_document_root() else { return };
        let Some((tab, doc)) = self.active_tab().and_then(|tab| Some((tab.id, tab.document_id()?))) else { return };
        let path = lmdv_io::absolute_lexical(&path);
        self.shared.jobs().resolve_link(self.worker().clone(), tab, doc, display, fragment, placement, root, path);
    }

    /// Accepts a [`crate::jobs::JobMsg::LinkResolved`] completion.
    ///
    /// The click's tab and document must still be the ones in front: a resolve
    /// that lands after the user switched away changes nothing (L-11). That is
    /// the only guard; a link followed in the tab it was clicked from acts
    /// normally, including a background placement.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn finish_link_resolve(
        &mut self,
        tab: TabId,
        doc: DocumentId,
        dest: String,
        fragment: Option<String>,
        placement: LinkPlacement,
        result: Result<Option<lmdv_io::LinkResolution>, String>,
        tx: &mut Tx,
    ) -> bool {
        let still_active =
            self.active_tab().is_some_and(|current| current.id == tab && current.document_id() == Some(doc));
        if !still_active {
            return false;
        }
        match result {
            Ok(Some(lmdv_io::LinkResolution::File { path, markdown: true })) => {
                self.place_linked_document(path, fragment, placement, tx);
            }
            // L-07 is not part of the wiki resolver: an existing non-Markdown
            // file takes the same folder attempt as before and reports
            // "not a Markdown file" when the listing refuses it.
            Ok(Some(lmdv_io::LinkResolution::File { path, markdown: false })) => self.open_folder_link(path),
            // L-05: a link resolving to a directory roots the file pane there
            // and shows it, with no tab.
            Ok(Some(lmdv_io::LinkResolution::Directory(path))) => self.open_folder_link(path),
            Ok(None) | Err(_) => self.notice(lmdv_i18n::t!("msg.notFound", "name" => dest)),
        }
        true
    }

    /// Applies a click placement to an opened Markdown document.
    ///
    /// A link to the document on screen moves within it instead of reloading
    /// the file (M-30), whatever the placement: the same place rule as an
    /// outline jump applies, so the place left goes on Back and a landing under
    /// 8 points does not (O-12).
    fn place_linked_document(
        &mut self,
        path: PathBuf,
        fragment: Option<String>,
        placement: LinkPlacement,
        tx: &mut Tx,
    ) {
        let path = lmdv_io::absolute_lexical(&path);
        if fragment.is_some() && self.active_path().as_deref() == Some(path.as_path()) {
            if let Some(fragment) = fragment {
                self.jump_fragment(&fragment, tx);
            }
            return;
        }
        match placement {
            LinkPlacement::Current | LinkPlacement::ForegroundTab => {
                self.open_linked_document(path, fragment, tx);
            }
            LinkPlacement::BackgroundTab => {
                let previous = self.active_tab().map(|tab| tab.id);
                let opened = self.open_path(path);
                if let Some(fragment) = fragment
                    && let Some(tab) = self.tabs.iter_mut().find(|candidate| candidate.id == opened)
                {
                    tab.view.fragment = Some(fragment);
                }
                // The document in front stays there; the click leaves it with
                // its place and keeps its own viewport (T-12/T-14).
                if let Some(index) =
                    previous.and_then(|previous| self.tabs.iter().position(|candidate| candidate.id == previous))
                {
                    self.switch_to_index(index);
                }
                self.apply_pending_scroll(tx);
            }
            LinkPlacement::NewWindow => self.open_link_in_new_window(path),
        }
    }

    /// Opens a link target in a window of its own (Shift+click, L-11).
    ///
    /// A `#section` cannot travel: [`Launch`] carries the path and line only, so
    /// a new window opens the document at its remembered/first place.
    fn open_link_in_new_window(&mut self, path: PathBuf) {
        let position = self.shared.cascade_position(self.id());
        let request = WindowOpenRequest {
            launch: Launch { path: Some(path), ..Launch::default() },
            kind: WindowKind::Normal,
            position,
        };
        if let Err(error) = Rc::clone(&self.shared).request_new_window(request) {
            self.notice(lmdv_i18n::t!("msg.openWindowFailed", "err" => error));
        }
    }

    /// Opens a Markdown link's document in front, at its `#section` when it
    /// names one. The same-document move already happened in
    /// [`Self::place_linked_document`].
    fn open_linked_document(&mut self, path: PathBuf, fragment: Option<String>, tx: &mut Tx) {
        let path = lmdv_io::absolute_lexical(&path);
        let tab = self.open_path(path);
        if let Some(fragment) = fragment
            && let Some(tab) = self.tabs.iter_mut().find(|candidate| candidate.id == tab)
        {
            tab.view.fragment = Some(fragment);
        }
        // A document that is already parsed draws on this frame: issue its
        // restore now instead of letting the shared viewport keep the old
        // document's offset for one frame.
        self.apply_pending_scroll(tx);
    }

    /// Scrolls to a heading by its GitHub id, legacy id or anchor name (R-15).
    fn jump_fragment(&mut self, fragment: &str, tx: &mut Tx) {
        let Some(doc) = self.active_doc().cloned() else {
            self.notice(lmdv_i18n::t!("msg.docLoading"));
            return;
        };
        match doc.resolve_heading(fragment) {
            Some(index) => self.jump_to_heading(&heading_key(index), tx),
            None => self.notice(lmdv_i18n::t!("link.noHeading")),
        }
    }

    /// A relative path against the active document's folder, or a refusal.
    fn local_target(&self, dest: &str, media: bool) -> LinkTarget {
        let Some(root) = self.active_document_root() else {
            return LinkTarget::Refused(lmdv_i18n::t!("link.needsFile").to_owned());
        };
        let (dest, fragment) = match dest.split_once('#') {
            Some((dest, fragment)) if !fragment.is_empty() => (dest, Some(fragment.to_owned())),
            Some((dest, _)) => (dest, None),
            None => (dest, None),
        };
        if dest.is_empty() {
            return LinkTarget::Refused(lmdv_i18n::t!("link.noTarget").to_owned());
        }
        let path = lmdv_io::absolute_lexical(&root.join(dest));
        let markdown = !media && is_markdown(&path);
        LinkTarget::Local { path, markdown, fragment: if media { None } else { fragment } }
    }

    /// The validated path of a media placeholder's source, when it is local.
    fn classify_media(&self, src: &str) -> Option<PathBuf> {
        match self.local_target(src, true) {
            LinkTarget::Local { path, .. } => Some(path),
            _ => None,
        }
    }

    /// The source of the code block a copy action names.
    ///
    /// The renderer keys every code block `code`, so the destination does not
    /// identify its block. When the document has exactly one top-level code
    /// block the source is unambiguous; otherwise the app declines instead of
    /// copying the wrong block (a block-keyed destination is a renderer
    /// follow-up).
    fn code_text(&self, _key: &str) -> Option<String> {
        let doc = self.active_doc()?;
        let mut found: Option<&Block> = None;
        for block in &doc.blocks {
            if matches!(block, Block::Code { .. }) {
                if found.is_some() {
                    return None;
                }
                found = Some(block);
            }
        }
        match found {
            Some(Block::Code { text, .. }) => Some(text.clone()),
            _ => None,
        }
    }

    /// Opens a local file with the desktop's handler, on an explicit click only.
    fn open_external(&mut self, path: &Path) {
        if !path.is_absolute() {
            self.notice(lmdv_i18n::t!("link.notLocal", "name" => path.display()));
            return;
        }
        match std::process::Command::new("xdg-open").arg(path).spawn() {
            Ok(_) => {}
            Err(error) => self.notice(lmdv_i18n::t!("msg.openFailed", "name" => path.display(), "err" => error)),
        }
    }

    fn active_doc(&self) -> Option<&std::sync::Arc<MdDoc>> {
        self.active_tab().and_then(|tab| tab.view.reader.as_ref())
    }

    /// The active document's folder, when it is a file.
    pub(crate) fn active_document_root(&self) -> Option<PathBuf> {
        let doc = self.active_tab().and_then(|tab| tab.document())?;
        let key = doc.key.borrow();
        match &*key {
            DocumentKey::File(path) => path.parent().map(Path::to_path_buf),
            DocumentKey::Untitled(_) | DocumentKey::Start => None,
        }
    }
}

/// Whether a path names a Markdown document the app can open in a tab.
pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| MARKDOWN_EXTENSIONS.iter().any(|known| extension.eq_ignore_ascii_case(known)))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::time::{Duration, Instant};

    use lmdv_core::{DocStatus, WindowId};
    use tempfile::TempDir;

    use super::*;
    use crate::jobs::JobMsg;
    use crate::shared::AppShared;
    use crate::test_support;
    use crate::window::Tab;
    use crate::windows::OpenedWindow;

    fn window(temp: &TempDir) -> WindowState {
        WindowState::new(test_support::shared(temp))
    }

    fn poll_until(state: &mut WindowState, what: &str, mut done: impl FnMut(&WindowState) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut tx = Tx::default();
        while !done(state) {
            assert!(Instant::now() < deadline, "{what} never happened");
            state.poll(0.0, &mut tx);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn ready(state: &WindowState) -> bool {
        state.active_tab().and_then(Tab::document).is_some_and(|doc| matches!(*doc.status.borrow(), DocStatus::Ready))
    }

    fn open_file(state: &mut WindowState, path: PathBuf) {
        state.open_path(path);
        poll_until(state, "the document load", ready);
    }

    fn write(temp: &TempDir, rel: &str) -> PathBuf {
        let path = temp.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("# {rel}\n")).unwrap();
        path
    }

    fn canonical(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap()
    }

    fn shows(state: &WindowState, path: &Path) -> bool {
        let path = canonical(path);
        state.tabs().iter().any(|tab| {
            tab.document().and_then(|doc| doc.key.borrow().path().map(Path::to_path_buf)) == Some(path.clone())
        })
    }

    #[test]
    fn destinations_classify_without_touching_the_disk() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "# One\n\n## Two\n");

        assert_eq!(state.classify_link("#two"), LinkTarget::Fragment("two".to_owned()));
        assert_eq!(state.classify_link("fn:1"), LinkTarget::Footnote("1".to_owned()));
        assert_eq!(state.classify_link("fnref:note"), LinkTarget::FootnoteRef("note".to_owned()));
        assert_eq!(state.classify_link("copy-code:code"), LinkTarget::CopyCode("code".to_owned()));
        assert_eq!(state.classify_link("details:front-matter"), LinkTarget::Details("front-matter".to_owned()));
        assert_eq!(
            state.classify_link("https://example.test/a"),
            LinkTarget::External("https://example.test/a".to_owned())
        );
        assert_eq!(state.classify_link("media:./clip.mp4"), LinkTarget::Media("./clip.mp4".to_owned()));
        assert_eq!(state.classify_link("image:b3:1"), LinkTarget::InlineImage { block: "b3".to_owned(), index: 1 });
        assert!(matches!(state.classify_link("javascript:alert(1)"), LinkTarget::Refused(_)));
        assert!(matches!(state.classify_link("file:///etc/passwd"), LinkTarget::Refused(_)));
        assert!(matches!(state.classify_link("//host/share/a.md"), LinkTarget::Refused(_)));
    }

    #[test]
    fn a_relative_markdown_link_resolves_inside_the_document_folder() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("notes");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("index.md");
        std::fs::write(&file, "# Index\n\n[next](next.markdown)\n").unwrap();
        let drawing = root.join("drawing.png");
        std::fs::write(&drawing, b"png").unwrap();

        let mut state = window(&temp);
        open_file(&mut state, file.clone());

        let target = state.classify_link("next.markdown");
        assert_eq!(target, LinkTarget::Local { path: root.join("next.markdown"), markdown: true, fragment: None });

        assert_eq!(
            state.classify_link("drawing.png"),
            LinkTarget::Local { path: drawing.clone(), markdown: false, fragment: None }
        );
        state.open_link("drawing.png", &mut Tx::default());
        poll_until(&mut state, "the non-Markdown folder attempt", |state| {
            state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("not a Markdown file"))
        });
        assert!(state.browser.root().is_some(), "a file that is not a folder says so and goes back");
    }

    #[test]
    fn an_unresolved_link_reports_not_found() {
        let temp = TempDir::new().unwrap();
        let index = write(&temp, "index.md");
        let mut state = window(&temp);
        open_file(&mut state, index);

        state.open_link("gone", &mut Tx::default());
        poll_until(&mut state, "the not-found notice", |state| {
            state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("Not found"))
        });
        assert_eq!(state.tabs().len(), 1, "nothing opened");
    }

    #[test]
    fn wiki_links_probe_extensions_and_folder_indexes() {
        let temp = TempDir::new().unwrap();
        let index = write(&temp, "index.md");
        let note = write(&temp, "note.md");
        let deeper = write(&temp, "nested/deeper.md");
        let readme = write(&temp, "guide/README.md");
        let mut state = window(&temp);
        open_file(&mut state, index);
        let index_tab = state.active_tab().expect("the index tab").id;

        let mut tx = Tx::default();
        state.open_link("note", &mut tx);
        poll_until(&mut state, "`note` resolving to `note.md`", |state| {
            state.active_path().as_deref() == Some(canonical(&note).as_path())
        });

        state.activate_tab(index_tab);
        state.open_link("nested/deeper", &mut tx);
        poll_until(&mut state, "`nested/deeper` resolving to `deeper.md`", |state| {
            state.active_path().as_deref() == Some(canonical(&deeper).as_path())
        });

        // Links resolve from the document that holds them, so go back to the
        // root document before following the folder spelling.
        state.activate_tab(index_tab);
        state.open_link("guide/", &mut tx);
        poll_until(&mut state, "`guide/` resolving to its README", |state| {
            state.active_path().as_deref() == Some(canonical(&readme).as_path())
        });
    }

    #[test]
    fn a_link_anchor_is_stripped_before_resolution_and_applied_after() {
        let temp = TempDir::new().unwrap();
        let index = write(&temp, "index.md");
        let next = temp.path().join("next.md");
        std::fs::write(&next, "# Next\n\n## Part\n\ntext\n").unwrap();
        let mut state = window(&temp);
        open_file(&mut state, index);

        let mut tx = Tx::default();
        state.open_link("next#part", &mut tx);
        poll_until(&mut state, "the anchored document", |state| {
            state.active_path().as_deref() == Some(canonical(&next).as_path())
                && state.active_tab().is_some_and(|tab| tab.view.reader.is_some() && tab.view.fragment.is_none())
        });
        let model = state.active_tab().and_then(|tab| tab.view.reader.clone()).expect("the model");
        assert_eq!(model.resolve_heading("part").map(|index| model.headings[index].text.as_str()), Some("Part"));
    }

    #[test]
    fn link_placements_follow_modifiers_and_the_button() {
        let temp = TempDir::new().unwrap();
        let a = write(&temp, "a.md");
        let b = write(&temp, "b.md");
        let c = write(&temp, "c.md");
        let d = write(&temp, "d.md");
        let e = write(&temp, "e.md");
        let mut state = window(&temp);
        open_file(&mut state, a.clone());

        let plain = Mods::default();
        let ctrl = Mods { ctrl: true, ..Mods::default() };
        let ctrl_shift = Mods { ctrl: true, shift: true, ..Mods::default() };
        let shift = Mods { shift: true, ..Mods::default() };

        assert_eq!(LinkPlacement::for_click(plain, PointerButton::Primary), LinkPlacement::Current);
        assert_eq!(LinkPlacement::for_click(ctrl, PointerButton::Primary), LinkPlacement::BackgroundTab);
        assert_eq!(LinkPlacement::for_click(ctrl_shift, PointerButton::Primary), LinkPlacement::ForegroundTab);
        assert_eq!(LinkPlacement::for_click(shift, PointerButton::Primary), LinkPlacement::NewWindow);
        assert_eq!(
            LinkPlacement::for_click(Mods::default(), PointerButton::Middle),
            LinkPlacement::BackgroundTab,
            "a middle click opens behind without any keyboard modifier"
        );

        let mut tx = Tx::default();
        state.set_link_modifiers(ctrl);
        state.open_link("b", &mut tx);
        poll_until(&mut state, "the background tab", |state| shows(state, &b));
        assert_eq!(state.active_path().as_deref(), Some(canonical(&a).as_path()), "Ctrl+click keeps the front tab");

        state.set_link_modifiers(ctrl_shift);
        state.open_link("c", &mut tx);
        poll_until(&mut state, "the foreground tab", |state| {
            state.active_path().as_deref() == Some(canonical(&c).as_path())
        });
        assert!(shows(&state, &b), "the background tab is still there");

        state.set_link_modifiers(plain);
        state.open_link("d", &mut tx);
        poll_until(&mut state, "the plain open", |state| {
            state.active_path().as_deref() == Some(canonical(&d).as_path())
        });

        state.shared().set_window_opener(recording_opener);
        OPENED.with(|opened| opened.borrow_mut().clear());
        state.set_link_modifiers(shift);
        state.open_link("e", &mut tx);
        let deadline = Instant::now() + Duration::from_secs(5);
        while OPENED.with(|opened| opened.borrow().is_empty()) {
            assert!(Instant::now() < deadline, "the new-window open never happened");
            state.poll(0.0, &mut tx);
            std::thread::sleep(Duration::from_millis(5));
        }
        let opened = OPENED.with(|opened| opened.borrow().clone());
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].launch.path.as_deref(), Some(canonical(&e).as_path()));
        assert_eq!(state.active_path().as_deref(), Some(canonical(&d).as_path()), "Shift+click leaves this window");
        assert!(!shows(&state, &e), "the new window does not also open a tab here");
    }

    #[test]
    fn a_delayed_resolve_does_not_act_on_a_tab_the_user_left() {
        let temp = TempDir::new().unwrap();
        let a = write(&temp, "a.md");
        let b = write(&temp, "b.md");
        let c = write(&temp, "c.md");
        let mut state = window(&temp);
        open_file(&mut state, a.clone());
        let first = state.active_tab().expect("the first tab").id;
        open_file(&mut state, c.clone());
        assert_eq!(state.active_path().as_deref(), Some(canonical(&c).as_path()));
        let stale_tab = state.active_tab().expect("the tab that clicked").id;
        let stale_doc = state.active_tab().and_then(Tab::document_id).expect("the document that clicked");

        // The probe was started in C; its completion lands after the user
        // switched to A and must change nothing.
        assert!(state.activate_tab(first));
        let mut tx = Tx::default();
        state.worker().post(JobMsg::LinkResolved {
            job: 1,
            tab: stale_tab,
            doc: stale_doc,
            dest: "b".to_owned(),
            fragment: None,
            placement: LinkPlacement::Current,
            result: Ok(Some(lmdv_io::LinkResolution::File { path: canonical(&b), markdown: true })),
        });
        state.poll(0.0, &mut tx);
        assert!(!shows(&state, &b), "a resolve from the tab the user left changes nothing");
        assert_eq!(state.tabs().len(), 2);

        // The same completion from the tab in front does open the target.
        state.open_link("b", &mut tx);
        poll_until(&mut state, "the link followed from the front tab", |state| shows(state, &b));
    }

    thread_local! {
        static OPENED: RefCell<Vec<WindowOpenRequest>> = const { RefCell::new(Vec::new()) };
    }

    fn recording_opener(_: &Rc<AppShared>, request: WindowOpenRequest) -> Result<OpenedWindow, String> {
        OPENED.with(|opened| opened.borrow_mut().push(request.clone()));
        Ok(OpenedWindow { id: WindowId(999) })
    }

    #[test]
    fn a_details_link_toggles_the_card() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "<details><summary>More</summary>Body</details>\n");
        state.open_link("details:details:b0:0", &mut Tx::default());
        let tab = state.active_tab().expect("a tab");
        assert!(tab.view.details_open.iter().any(|key| key.starts_with("details:")));
    }

    #[test]
    fn a_copy_action_copies_the_single_code_block() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "```rust\nlet x = 1;\n```\n");
        assert_eq!(state.code_text("code").as_deref(), Some("let x = 1;"));
        state.open_synthetic("Two", "```\na\n```\n\n```\nb\n```\n");
        assert_eq!(state.code_text("code"), None, "two blocks are ambiguous");
    }

    #[test]
    fn a_missing_heading_is_reported_not_followed() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "# One\n");
        state.open_link("#gone", &mut Tx::default());
        assert!(state.ui.notice.is_some());
    }

    #[test]
    fn a_fragment_resolves_legacy_and_anchor_names() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "# Résumé notes\n");
        let doc = state.active_doc().expect("parsed");
        let id = doc.headings[0].id.clone();
        assert_eq!(state.classify_link(&format!("#{id}")), LinkTarget::Fragment(id));
    }

    #[test]
    fn markdown_extensions_are_recognized_case_insensitively() {
        assert!(is_markdown(Path::new("a.md")));
        assert!(is_markdown(Path::new("a.MARKDOWN")));
        assert!(!is_markdown(Path::new("a.png")));
        assert!(!is_markdown(Path::new("a")));
    }
}
