//! Link, `<details>` and copy actions for the rendered article.
//!
//! `lmdv-render` turns link runs into Rune links whose destination is exactly
//! what the Markdown wrote: `#slug`, `fn:label`, `fnref:label`,
//! `copy-code:<n>`, `details:<key>`, `media:<src>`, or an ordinary relative or
//! external URL. One [`LinkWire`] per window attaches the handler, and every
//! destination is classified here before anything happens (M-30, M-31, R-15).
//!
//! The rules: fragments resolve through [`MdDoc::resolve_heading`] and land at
//! L-01's 24-point margin with a landing flash, silently when the document has
//! no such target; footnote references and entries scroll inside the article;
//! `http`/`https`/`mailto`/`tel` open externally while any other scheme is
//! ignored without a notice and script/`file:`/`data:` URLs stay refused
//! (L-02); every local spelling — with or without a Markdown extension — is
//! probed on a worker first, so an existing Markdown file opens at the
//! `#section` it names (L-03), an unresolved one says “Not found” and does not
//! navigate (L-08), and an existing non-Markdown file offers **Open** or
//! **Show in folder** instead of opening as text (L-07); pasted text offers
//! **Save as…** (L-09). Network/device spellings are refused with the
//! “another computer” notice unless the document's own share is named
//! (M-30/L-10). A click's placement follows its modifiers and button (L-11):
//! plain in front, Ctrl or middle in a background tab, Ctrl+Shift in a
//! foreground tab, Shift in a new window. Nothing here follows a link on its
//! own.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use lmdv_core::{DocumentId, DocumentKey, TabId};
use lmdv_io::Launch;
use lmdv_md::MdDoc;
use lmdv_render::{DetailsState, LinkWire, heading_key};
use rune::{Mods, PointerButton, Tx};

use crate::commands::MARKDOWN_EXTENSIONS;
use crate::window::{NOTICE_SECONDS, NoticeAction, NoticeKind, TabView, WindowState};
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
    /// `http`, `https`, `mailto` or `tel`.
    External(String),
    /// A local path from a relative or absolute destination.
    Local {
        path: PathBuf,
        markdown: bool,
        /// The `#section` the destination names, if any.
        fragment: Option<String>,
    },
    /// A local link in pasted text: there is no document folder yet, so the
    /// notice offers Save as… instead of a lookup (L-09).
    NeedsFile,
    /// A scheme the app does not follow: nothing happens, with no notice
    /// (L-02).
    Ignored,
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
            // `C:x` is a drive-relative device spelling, not a scheme (M-31).
            if scheme.len() == 1 && scheme.starts_with(|c: char| c.is_ascii_alphabetic()) {
                return LinkTarget::Refused(lmdv_i18n::t!("msg.otherComputer", "name" => dest));
            }
            if matches!(scheme.as_str(), "http" | "https" | "mailto" | "tel") {
                return LinkTarget::External(dest.to_owned());
            }
            if scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            {
                // L-02: a scheme the app does not follow is ignored without a
                // notice. The script and local-file forms stay refused.
                return if matches!(scheme.as_str(), "javascript" | "vbscript" | "data" | "file") {
                    LinkTarget::Refused(lmdv_i18n::t!("link.refusedScheme", "scheme" => scheme))
                } else {
                    LinkTarget::Ignored
                };
            }
        }
        if is_network_spelling(dest) {
            // M-30/L-10: a network path is refused unless the document itself
            // lives on that same share.
            return if self.active_document_root().is_some_and(|root| lmdv_io::on_document_share(&root, Path::new(dest)))
            {
                self.local_target(dest, false)
            } else {
                LinkTarget::Refused(lmdv_i18n::t!("msg.otherComputer", "name" => dest))
            };
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
            // L-02: a scheme the app does not follow does nothing at all.
            LinkTarget::Ignored => {}
            // L-09: pasted text has no folder yet, so the notice offers Save
            // as… instead of a lookup. In-page anchors never reach here.
            LinkTarget::NeedsFile => self.show_pasted_link_notice(),
            // L-03/L-08: every local spelling — with or without a Markdown
            // extension — is probed on a worker before anything opens, so a
            // missing target reports “Not found” instead of an error tab. The
            // one exception is a `#section` in the document already on screen,
            // which jumps without touching the disk.
            LinkTarget::Local { path, fragment, .. } => {
                let path = lmdv_io::absolute_lexical(&path);
                if fragment.is_some() && self.active_path().as_deref() == Some(path.as_path()) {
                    if let Some(fragment) = fragment {
                        self.jump_fragment(&fragment, tx);
                    }
                    return;
                }
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
            // L-06: a real image opens the full-window preview, decode-tested
            // through the image cache; a missing or broken picture falls through
            // to L-07's honest notice instead of opening as text.
            Ok(Some(lmdv_io::LinkResolution::File { path, markdown: false })) => {
                // The link run's key is its destination as written; with a
                // fragment, the renderer kept the whole spelling.
                let opener = match &fragment {
                    Some(fragment) => format!("{dest}#{fragment}"),
                    None => dest.clone(),
                };
                if !(is_image_path(&path) && self.open_image_link(path.clone(), opener, tx)) {
                    self.show_non_markdown_notice(&path);
                }
            }
            // L-05: a link resolving to a directory roots the file pane there
            // and shows it, with no tab.
            Ok(Some(lmdv_io::LinkResolution::Directory(path))) => self.open_folder_link(path),
            // L-08: no navigation. The pane's listing and search are refreshed
            // in case the target was moved or deleted behind the link.
            Ok(None) => {
                self.notice(lmdv_i18n::t!("msg.notFound", "name" => dest));
                self.refresh_browser();
            }
            Err(error) => self.notice(lmdv_i18n::t!("msg.openFailed", "name" => dest, "err" => error)),
        }
        true
    }

    /// L-09's notice for a relative link in pasted text.
    ///
    /// The action names the pasted document, so a tab switch before the press
    /// cannot turn it into Save as on a file.
    fn show_pasted_link_notice(&mut self) {
        let action = self.active_document_id().map(NoticeAction::SaveAs);
        self.show_notice(lmdv_i18n::t!("msg.pastedLink").to_owned(), NoticeKind::Info, action, NOTICE_SECONDS);
    }

    /// L-07's notice for an existing file that is not a Markdown document.
    pub(crate) fn show_non_markdown_notice(&mut self, path: &Path) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let action = if can_open_externally(path) {
            NoticeAction::OpenFile(path.to_path_buf())
        } else {
            // A program or script is never passed to a handler: revealing its
            // folder shows it without running anything.
            NoticeAction::ShowInFolder(path.parent().map_or_else(|| path.to_path_buf(), Path::to_path_buf))
        };
        self.show_notice(
            lmdv_i18n::t!("msg.notMarkdown", "name" => name),
            NoticeKind::Warning,
            Some(action),
            NOTICE_SECONDS,
        );
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

    /// Opens a recent file with a click's placement (T-12).
    ///
    /// Recents, tree rows, search hits and document links share
    /// [`LinkPlacement`]; this maps its four gestures onto the app's existing
    /// open paths instead of a second enum: plain in front, Ctrl or middle in
    /// a background tab, Ctrl+Shift in front, Shift in a new window.
    pub(crate) fn open_recent_placed(&mut self, path: PathBuf, placement: LinkPlacement, tx: &mut Tx) {
        match placement {
            LinkPlacement::Current | LinkPlacement::ForegroundTab => {
                self.open_path(path);
            }
            LinkPlacement::BackgroundTab => {
                self.open_path_background(path);
            }
            LinkPlacement::NewWindow => self.open_link_in_new_window(path),
        }
        // The new tab must draw at its own place on this frame, not at the
        // offset the shared article viewport still carries.
        self.apply_pending_scroll(tx);
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
    ///
    /// L-01: a target the document does not have does nothing — no notice, no
    /// scroll, no history entry.
    fn jump_fragment(&mut self, fragment: &str, tx: &mut Tx) {
        let Some(doc) = self.active_doc().cloned() else {
            // A click before the article exists has nothing to land on, but the
            // reader is told why instead of nothing happening.
            self.notice(lmdv_i18n::t!("msg.docLoading"));
            return;
        };
        if let Some(index) = doc.resolve_heading(fragment) {
            self.jump_to_heading(&heading_key(index), tx);
        }
    }

    /// A relative path against the active document's folder, or a refusal.
    fn local_target(&self, dest: &str, media: bool) -> LinkTarget {
        let Some(root) = self.active_document_root() else {
            // L-09: pasted text has no folder; the notice offers Save as…
            return LinkTarget::NeedsFile;
        };
        let (dest, fragment) = match dest.split_once('#') {
            Some((dest, fragment)) if !fragment.is_empty() => (dest, Some(fragment.to_owned())),
            Some((dest, _)) => (dest, None),
            None => (dest, None),
        };
        // M-24/L-15: a URL query is a fetch hint that has no meaning for a
        // local file, so it never becomes part of the path. Splitting the
        // fragment first keeps `file.md?plain=1#section` landing on `section`.
        // A literal `?` in a file name is written `%3F` and decoded below.
        let dest = dest.split_once('?').map_or(dest, |(path, _)| path);
        if dest.is_empty() {
            return LinkTarget::Refused(lmdv_i18n::t!("link.noTarget").to_owned());
        }
        // L-15: the spelling is a URL path, so `%20` names a space. The
        // fragment was split first, so `%23` stays part of the file name and a
        // literal `#section` still travels separately.
        let decoded = lmdv_md::decode_url_path(dest);
        let path = lmdv_io::absolute_lexical(&root.join(decoded.as_ref()));
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

    /// The source of the code block a copy action names (M-13).
    ///
    /// The renderer numbers every code block in drawing order, so the
    /// destination's key is that number and the model's own
    /// [`MdDoc::code_texts`] answers with exactly the block that was clicked —
    /// top-level, nested, duplicated and raw-HTML blocks alike.
    fn code_text(&self, key: &str) -> Option<String> {
        let index = key.parse::<usize>().ok()?;
        let doc = self.active_doc()?;
        doc.code_texts().get(index).map(|text| text.to_string())
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

/// Whether a destination is written as a network path: `//server/share/…` or
/// `\\server\share\…` (a `///abs` spelling is an ordinary absolute path).
pub(crate) fn is_network_spelling(dest: &str) -> bool {
    (dest.starts_with("//") && !dest.starts_with("///")) || dest.starts_with(r"\\")
}

/// The extensions L-07's notice may hand to the desktop: documents, media,
/// archives and web pages. A program or script is never on this list, so it is
/// only ever revealed in the file manager.
pub const OPENABLE_EXTENSIONS: &[&str] = &[
    // Documents.
    "pdf", "txt", "text", "csv", "tsv", "json", "jsonl", "xml", "yaml", "yml", "toml", "doc", "docx", "odt", "rtf",
    "xls", "xlsx", "ods", "ppt", "pptx", "odp", "epub", //
    // Images.
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif", "tif", "tiff", "ico", "heic", // Audio.
    "mp3", "wav", "ogg", "oga", "flac", "m4a", "opus", "aac", "wma", // Video.
    "mp4", "m4v", "webm", "mkv", "mov", "avi", "wmv", // Archives.
    "zip", "7z", "tar", "gz", "tgz", "bz2", "xz", "zst", "rar", // Web pages.
    "html", "htm", "xhtml",
];

/// The extensions a plain link may open in the media preview (L-06).
///
/// They are the raster and vector pictures the window's image cache decodes;
/// anything else keeps L-07's notice.
pub const IMAGE_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "svgz", "avif", "tif", "tiff", "ico", "heic"];

/// Whether a path names a picture the preview can try to open (L-06).
pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| IMAGE_EXTENSIONS.iter().any(|known| extension.eq_ignore_ascii_case(known)))
}

/// Whether a non-Markdown file may be opened by the desktop's handler (L-07).
///
/// Extensions are compared case-insensitively; a file with no extension, or
/// one whose extension is not on the list (a shell script, a binary), is only
/// revealed.
pub fn can_open_externally(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| OPENABLE_EXTENSIONS.iter().any(|known| extension.eq_ignore_ascii_case(known)))
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

    /// A valid 1x1 PNG, for L-06's decode test.
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00,
        0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D,
        0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

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
            state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("not a Markdown document"))
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
    fn a_recent_row_takes_the_four_click_placements() {
        let temp = TempDir::new().unwrap();
        let a = write(&temp, "a.md");
        let b = write(&temp, "b.md");
        let c = write(&temp, "c.md");
        let d = write(&temp, "d.md");
        let e = write(&temp, "e.md");
        let mut state = window(&temp);
        open_file(&mut state, a.clone());
        let mut tx = Tx::default();

        // Plain: in front.
        state.open_recent_placed(b.clone(), LinkPlacement::Current, &mut tx);
        poll_until(&mut state, "the plain recent open", |state| {
            state.active_path().as_deref() == Some(canonical(&b).as_path())
        });

        // Ctrl: a background tab; the front document stays in front.
        state.open_recent_placed(c.clone(), LinkPlacement::BackgroundTab, &mut tx);
        poll_until(&mut state, "the background recent open", |state| shows(state, &c));
        assert_eq!(
            state.active_path().as_deref(),
            Some(canonical(&b).as_path()),
            "a background recent keeps the front tab"
        );

        // Ctrl+Shift: a foreground tab.
        state.open_recent_placed(d.clone(), LinkPlacement::ForegroundTab, &mut tx);
        poll_until(&mut state, "the foreground recent open", |state| {
            state.active_path().as_deref() == Some(canonical(&d).as_path())
        });
        assert!(shows(&state, &c), "the background tab is still there");

        // Shift: a window of its own, leaving this window where it is.
        state.shared().set_window_opener(recording_opener);
        OPENED.with(|opened| opened.borrow_mut().clear());
        state.open_recent_placed(e.clone(), LinkPlacement::NewWindow, &mut tx);
        let deadline = Instant::now() + Duration::from_secs(5);
        while OPENED.with(|opened| opened.borrow().is_empty()) {
            assert!(Instant::now() < deadline, "the recent's new-window open never happened");
            state.poll(0.0, &mut tx);
            std::thread::sleep(Duration::from_millis(5));
        }
        let opened = OPENED.with(|opened| opened.borrow().clone());
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].launch.path.as_deref(), Some(canonical(&e).as_path()));
        assert_eq!(state.active_path().as_deref(), Some(canonical(&d).as_path()), "this window did not move");
        assert!(!shows(&state, &e), "the new window does not also open a tab here");
    }

    #[test]
    fn a_recent_row_press_passes_the_held_modifiers() {
        let temp = TempDir::new().unwrap();
        let a = write(&temp, "a.md");
        let mut state = window(&temp);
        open_file(&mut state, a.clone());
        // A start page in front shows the recents; A is the newest.
        state.new_start_tab();
        assert!(state.active_tab().is_some_and(|tab| tab.is_start()));

        let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(false, 2.0));
        runtime.poll = Some(|state: &mut WindowState, now, tx| state.poll(now, tx));
        runtime.frame(0.0);
        let (x, y) = runtime.find("recent-0").expect("the recent row");

        // Ctrl+click: a background tab; the start page stays in front.
        runtime.physical_input(rune::PhysicalInput::Modifiers(Mods { ctrl: true, ..Mods::default() }));
        runtime.pointer_move(x, y);
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0 / 60.0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !shows(&runtime.state, &a) {
            assert!(Instant::now() < deadline, "the Ctrl+clicked recent never opened");
            runtime.update(|state, tx| {
                state.poll(0.0, tx);
            });
            runtime.frame(1.0 / 60.0);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            runtime.state.active_tab().is_some_and(|tab| tab.is_start()),
            "Ctrl+click opens the recent behind the start page"
        );

        // Shift+click: a window of its own, through the same opener path the
        // links use; this window keeps the start page.
        runtime.state.shared().set_window_opener(recording_opener);
        OPENED.with(|opened| opened.borrow_mut().clear());
        runtime.physical_input(rune::PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0 / 60.0);
        let opened = OPENED.with(|opened| opened.borrow().clone());
        assert_eq!(opened.len(), 1, "Shift+click asked for a new window");
        assert_eq!(opened[0].launch.path.as_deref(), Some(canonical(&a).as_path()));
        assert!(runtime.state.active_tab().is_some_and(|tab| tab.is_start()), "this window did not move");

        // A plain press replaces the front start page, as every foreground open.
        runtime.physical_input(rune::PhysicalInput::Modifiers(Mods::default()));
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0 / 60.0);
        while runtime.state.active_path().as_deref() != Some(canonical(&a).as_path()) {
            assert!(Instant::now() < deadline, "the plain recent press never came forward");
            runtime.update(|state, tx| {
                state.poll(0.0, tx);
            });
            runtime.frame(1.0 / 60.0);
            std::thread::sleep(Duration::from_millis(5));
        }
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
    fn every_code_block_copies_its_own_text() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic(
            "Note",
            "```rust\nlet top = 1;\n```\n\n> ```rust\n> let quoted = 2;\n> ```\n\n```\nlet top = 1;\n```\n",
        );
        // M-13: the renderer numbers blocks in drawing order, which is the
        // model's own order; duplicates stay separate and each keeps its text.
        assert_eq!(state.code_text("0").as_deref(), Some("let top = 1;"));
        assert_eq!(state.code_text("1").as_deref(), Some("let quoted = 2;"));
        assert_eq!(state.code_text("2").as_deref(), Some("let top = 1;"));
        assert_eq!(state.code_text("3"), None, "there is no fourth block");
        assert_eq!(state.code_text("code"), None, "the old ambiguous key names nothing");

        // The click path copies the block the destination names.
        let board = rune::clipboard::memory();
        let mut runtime = rune::Runtime::new(state, crate::view::view, (1100.0, 760.0), rune::Env::new(true, 2.0));
        runtime.use_clipboard(&board);
        runtime.update(|state: &mut WindowState, tx: &mut Tx| state.open_link("copy-code:1", tx));
        assert_eq!(board.content().expect("the clipboard").text(), "let quoted = 2;");
    }

    #[test]
    fn a_missing_heading_does_nothing() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        let tab = state.open_synthetic("Note", "# One\n");
        let mut tx = Tx::default();
        state.open_link("#gone", &mut tx);
        assert!(state.ui.notice.is_none(), "L-01: a missing target is silent");
        assert!(state.tab(tab).expect("the tab").view.nav.back.is_empty());
        assert!(state.reading.landing_flash.is_none() && state.reading.landing.is_none());
    }

    #[test]
    fn a_media_source_drops_its_query_and_keeps_its_fragment_out_of_the_path() {
        // M-24: video/audio placeholders resolve the file without `?…`, and a
        // `#t=…` fragment never becomes part of the path either.
        let temp = TempDir::new().unwrap();
        let root = canonical(temp.path());
        write(&temp, "media/clip.mp4");
        let note = write(&temp, "note.md");
        let mut state = window(&temp);
        open_file(&mut state, note);

        assert_eq!(state.classify_media("./media/clip.mp4?start=3#t=10"), Some(root.join("media/clip.mp4")));
        assert_eq!(state.classify_media("./media/clip.mp4#t=10"), Some(root.join("media/clip.mp4")));
    }

    #[test]
    fn external_and_unknown_schemes_classify_as_specified() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "# One\n");

        // L-02: the four schemes the desktop follows.
        for url in ["https://example.test/a", "http://example.test/a", "mailto:me@example.test", "tel:+391234567"] {
            assert_eq!(state.classify_link(url), LinkTarget::External(url.to_owned()), "{url} opens externally");
        }
        // Any other scheme is ignored without a notice.
        for url in ["ssh://example.test/x", "obsidian://open?vault=x", "ftp://example.test/x", "sms:+39123"] {
            assert_eq!(state.classify_link(url), LinkTarget::Ignored, "{url} does nothing");
        }
        // Script and local-file forms stay refused for safety.
        for url in ["javascript:alert(1)", "vbscript:msgbox", "data:text/html,x", "file:///etc/passwd"] {
            assert!(matches!(state.classify_link(url), LinkTarget::Refused(_)), "{url} is refused");
        }
        // A drive-relative spelling is a device path, not a scheme (M-31).
        assert!(matches!(state.classify_link("C:/pictures/a.png"), LinkTarget::Refused(_)));
        // An ignored scheme never touches the notice either.
        state.open_link("obsidian://open", &mut Tx::default());
        assert!(state.ui.notice.is_none());
    }

    #[test]
    fn a_network_link_is_refused_with_the_other_computer_notice() {
        let temp = TempDir::new().unwrap();
        let index = write(&temp, "index.md");
        let mut state = window(&temp);
        open_file(&mut state, index);

        let refused = state.classify_link("//server/share/a.md");
        assert_eq!(
            refused,
            LinkTarget::Refused(lmdv_i18n::t!("msg.otherComputer", "name" => "//server/share/a.md")),
            "M-30/L-10: another computer's share is refused"
        );
        assert!(matches!(state.classify_link(r"\\server\share\a.md"), LinkTarget::Refused(_)));
        // An absolute web URL is an ordinary external link.
        assert_eq!(
            state.classify_link("https://example.test/a"),
            LinkTarget::External("https://example.test/a".to_owned())
        );
        state.open_link("//server/share/a.md", &mut Tx::default());
        let notice = state.ui.notice.as_ref().expect("the refusal notice");
        assert!(notice.text.contains("another computer"), "{}", notice.text);
        assert_eq!(state.tabs().len(), 1, "nothing opens");
    }

    #[cfg(unix)]
    #[test]
    fn a_document_on_a_share_may_follow_a_link_on_that_share() {
        // L-10/M-30: the same-share exception is textual. `//tmp/<dir>` is a
        // share spelling for this temporary folder; while the document's key
        // carries it, a link on that share is a normal local link while
        // another share keeps the refusal. (On Windows the canonical
        // `\\?\UNC\server\share\…` key keeps the share too; on Linux the
        // canonical spelling drops the double slash, so the io policy is what
        // enforces the same rule there.)
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        std::fs::write(root.join("a.md"), "# A\n").unwrap();
        std::fs::write(root.join("b.md"), "# B\n").unwrap();
        let spelled = PathBuf::from(format!("//{}", root.to_string_lossy().trim_start_matches('/')));

        let mut state = window(&temp);
        state.open_path(spelled.join("a.md"));
        assert_eq!(state.active_document_root(), Some(spelled.clone()));

        let own = spelled.join("b.md").to_string_lossy().into_owned();
        assert!(
            matches!(state.classify_link(&own), LinkTarget::Local { markdown: true, .. }),
            "the own share is local"
        );
        assert!(matches!(state.classify_link("//elsewhere/share/b.md"), LinkTarget::Refused(_)));
    }

    #[test]
    fn a_non_markdown_file_offers_open_or_show_in_folder() {
        let temp = TempDir::new().unwrap();
        let index = write(&temp, "index.md");
        std::fs::write(temp.path().join("manual.pdf"), b"%PDF-1.4").unwrap();
        std::fs::write(temp.path().join("build.sh"), b"#!/bin/sh\n").unwrap();
        let mut state = window(&temp);
        open_file(&mut state, index.clone());

        let mut tx = Tx::default();
        state.open_link("manual.pdf", &mut tx);
        poll_until(&mut state, "the PDF notice", |state| {
            state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("is not a Markdown"))
        });
        let notice = state.ui.notice.clone().expect("the notice");
        assert_eq!(notice.text, lmdv_i18n::t!("msg.notMarkdown", "name" => "manual.pdf"));
        assert_eq!(
            notice.action,
            Some(NoticeAction::OpenFile(canonical(&temp.path().join("manual.pdf")))),
            "L-07: a whitelisted document offers Open"
        );
        assert!(!shows(&state, &temp.path().join("manual.pdf")), "it never opens as text");

        // Pressing Open hands the file to the desktop's handler.
        let seen = Rc::new(RefCell::new(Vec::new()));
        state.shared().set_external_sink({
            let seen = Rc::clone(&seen);
            Box::new(move |path: &Path| seen.borrow_mut().push(path.to_path_buf()))
        });
        state.run_notice_action();
        assert_eq!(seen.borrow().as_slice(), [canonical(&temp.path().join("manual.pdf"))]);

        state.activate_tab(state.active_tab().expect("the tab").id);
        let mut tx = Tx::default();
        state.open_link("build.sh", &mut tx);
        poll_until(&mut state, "the script notice", |state| {
            state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("build.sh"))
        });
        let notice = state.ui.notice.clone().expect("the notice");
        assert_eq!(
            notice.action,
            Some(NoticeAction::ShowInFolder(canonical(temp.path()))),
            "L-07: a script is only ever revealed, never run"
        );
        assert_eq!(notice.text, lmdv_i18n::t!("msg.notMarkdown", "name" => "build.sh"));
        // The reveal opens the folder, never the script itself.
        seen.borrow_mut().clear();
        state.run_notice_action();
        assert_eq!(seen.borrow().as_slice(), [canonical(temp.path())]);
        assert!(can_open_externally(Path::new("a.pdf")));
        assert!(can_open_externally(Path::new("a.PNG")));
        assert!(!can_open_externally(Path::new("a.sh")), "a program is not on the Open list");
        assert!(!can_open_externally(Path::new("a.py")));
        assert!(!can_open_externally(Path::new("a")));
    }

    #[test]
    fn an_unresolved_markdown_link_reports_not_found_without_opening() {
        let temp = TempDir::new().unwrap();
        let index = write(&temp, "index.md");
        let mut state = window(&temp);
        open_file(&mut state, index);

        // An explicit `.md` spelling is probed too (L-08).
        let mut tx = Tx::default();
        state.open_link("gone.md", &mut tx);
        poll_until(&mut state, "the not-found notice", |state| {
            state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("Not found: gone.md"))
        });
        assert_eq!(state.tabs().len(), 1, "no error tab, no navigation");
    }

    #[test]
    fn a_pasted_documents_relative_link_offers_save_as() {
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Pasted", "# Pasted\n\n[next](next.md)\n\n[top](#pasted)\n");

        let mut tx = Tx::default();
        state.open_link("next.md", &mut tx);
        let notice = state.ui.notice.clone().expect("L-09's notice");
        assert_eq!(notice.text, lmdv_i18n::t!("msg.pastedLink"));
        assert_eq!(notice.action, Some(NoticeAction::SaveAs(state.active_document_id().expect("the pasted document"))));
        assert_eq!(state.tabs().len(), 1);

        // The action asks for the dialog; poll opens it on the frame's Tx.
        state.run_notice_action();
        assert!(state.ui.save_as_requested.is_some(), "the press asks poll for the dialog");
        let mut tx = Tx::default();
        state.poll(0.0, &mut tx);
        assert!(state.ui.save_as_requested.is_none(), "poll consumed the request");

        // An in-page anchor still works in pasted text (L-09).
        state.dismiss_notice();
        state.open_link("#pasted", &mut tx);
        assert!(state.ui.notice.is_none(), "an anchor in pasted text needs no file");
    }

    #[test]
    fn a_percent_encoded_fragment_lands_with_the_landing_wait() {
        // L-01: the id may be percent-encoded; the jump and its flash apply.
        let temp = TempDir::new().unwrap();
        let mut state = window(&temp);
        state.open_synthetic("Note", "# Résumé notes\n\nBody.\n");
        let doc = state.active_doc().expect("parsed");
        assert_eq!(doc.headings[0].id, "résumé-notes");
        assert_eq!(
            state.classify_link("#r%C3%A9sum%C3%A9-notes"),
            LinkTarget::Fragment("r%C3%A9sum%C3%A9-notes".to_owned())
        );
        let mut tx = Tx::default();
        state.open_link("#r%C3%A9sum%C3%A9-notes", &mut tx);
        assert!(state.reading.landing.is_some(), "the landing poll and flash are armed");
        assert!(state.ui.notice.is_none());
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
    fn a_percent_encoded_relative_path_decodes_once_before_joining() {
        // L-15: the Markdown spelling is a URL path.
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(&temp, "my notes.md");
        write(&temp, "sub dir/next.md");
        std::fs::write(root.join("100% done.md"), "# Percent\n").unwrap();
        let index = write(&temp, "index.md");
        let mut state = window(&temp);
        open_file(&mut state, index);

        assert_eq!(
            state.classify_link("my%20notes.md"),
            LinkTarget::Local { path: root.join("my notes.md"), markdown: true, fragment: None }
        );
        assert_eq!(
            state.classify_link("sub%20dir/next.md#part"),
            LinkTarget::Local { path: root.join("sub dir/next.md"), markdown: true, fragment: Some("part".to_owned()) }
        );
        assert_eq!(
            state.classify_link("100%25%20done.md"),
            LinkTarget::Local { path: root.join("100% done.md"), markdown: true, fragment: None }
        );
        assert_eq!(
            state.classify_link("my%20notes.md?plain=1#part"),
            LinkTarget::Local { path: root.join("my notes.md"), markdown: true, fragment: Some("part".to_owned()) },
            "a query is not part of the file and the fragment still travels"
        );
        assert_eq!(
            state.classify_link("100% done.md"),
            LinkTarget::Local { path: root.join("100% done.md"), markdown: true, fragment: None },
            "a stray % stays as written"
        );
        assert_eq!(
            state.classify_link("%FF.md"),
            LinkTarget::Local { path: root.join("\u{FFFD}.md"), markdown: true, fragment: None },
            "invalid UTF-8 is lossy, never a panic"
        );

        // The worker probe receives the decoded spelling and opens the file.
        let mut tx = Tx::default();
        state.open_link("my%20notes.md", &mut tx);
        poll_until(&mut state, "the encoded link following", |state| {
            state.active_path().as_deref() == Some(canonical(&root.join("my notes.md")).as_path())
        });
    }

    #[test]
    fn an_image_link_opens_the_preview_and_a_broken_one_reports_honestly() {
        // L-06: a real image opens the preview, decode-tested through the image
        // cache; a broken or non-image file keeps L-07's notice, and a folder
        // still roots the file pane (L-05).
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let index = write(&temp, "index.md");
        std::fs::write(root.join("my pic.png"), PNG).unwrap();
        std::fs::write(root.join("broken.png"), b"not a png").unwrap();
        std::fs::write(
            root.join("drawing.svg"),
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#0969da"/></svg>"##,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("pics")).unwrap();
        let mut state = window(&temp);
        state.shared.set_headless();
        open_file(&mut state, index);

        let mut tx = Tx::default();
        state.open_link("my%20pic.png", &mut tx);
        poll_until(&mut state, "the image preview", |state| state.lightbox().is_some());
        let lightbox = state.lightbox().expect("the preview");
        assert_eq!(lightbox.media.kind, lmdv_render::MediaKind::Image);
        assert_eq!(lightbox.title, "my pic.png");
        assert_eq!(lightbox.caption, "my pic.png", "no alt text names the file");
        assert_eq!(lightbox.opener, "my%20pic.png", "focus returns to the link run that was clicked");
        assert_eq!(state.tabs().len(), 1, "no document tab opened for the picture");
        state.close_lightbox();

        // An SVG is a picture too: the image cache rasterizes it natively.
        state.open_link("drawing.svg", &mut tx);
        poll_until(&mut state, "the SVG preview", |state| state.lightbox().is_some());
        assert_eq!(state.lightbox().expect("the SVG preview").title, "drawing.svg");
        state.close_lightbox();

        state.open_link("broken.png", &mut tx);
        poll_until(&mut state, "the broken-image notice", |state| {
            state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("not a Markdown"))
        });
        assert!(state.lightbox().is_none(), "a picture that cannot decode never stays open");

        state.dismiss_notice();
        state.open_link("pics", &mut tx);
        poll_until(&mut state, "the folder link", |state| {
            state.browser.root().is_some_and(|folder| folder.ends_with("pics"))
        });
        assert!(state.lightbox().is_none(), "a folder is not a picture");
    }

    #[test]
    fn an_image_link_outside_the_document_folder_opens_the_preview() {
        // L-06 with L-07's scope: a click may leave the document folder, while
        // an embedded picture may not (OI-009). The media copy/save worker
        // follows the same allowance, so the preview is not read-only.
        let temp = TempDir::new().unwrap();
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::create_dir_all(temp.path().join("shared")).unwrap();
        let index = notes.join("index.md");
        std::fs::write(&index, "# Notes\n\n[Picture](../shared/pic.png)\n").unwrap();
        std::fs::write(temp.path().join("shared/pic.png"), PNG).unwrap();
        let mut state = window(&temp);
        state.shared.set_headless();
        open_file(&mut state, index);

        let mut tx = Tx::default();
        state.open_link("../shared/pic.png", &mut tx);
        poll_until(&mut state, "the outside image preview", |state| state.lightbox().is_some());
        let lightbox = state.lightbox().expect("the preview");
        assert_eq!(lightbox.title, "pic.png");
        poll_until(&mut state, "the linked picture's PNG preload", |state| {
            state.lightbox().is_some_and(|lightbox| lightbox.png_ready())
        });
    }

    #[test]
    fn markdown_extensions_are_recognized_case_insensitively() {
        assert!(is_markdown(Path::new("a.md")));
        assert!(is_markdown(Path::new("a.MARKDOWN")));
        assert!(!is_markdown(Path::new("a.png")));
        assert!(!is_markdown(Path::new("a")));
    }
}
