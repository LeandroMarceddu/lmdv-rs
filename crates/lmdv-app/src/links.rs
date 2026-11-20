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
//! open externally on the explicit click; relative Markdown links open in a tab
//! through the normal load job, at the `#section` they name when they carry one;
//! network/device spellings and script URLs are refused with a notice. Nothing
//! here follows a link on its own.

use std::path::{Path, PathBuf};

use lmdv_core::DocumentKey;
use lmdv_md::{Block, MdDoc};
use lmdv_render::{DetailsState, LinkWire, heading_key};
use rune::Tx;

use crate::commands::MARKDOWN_EXTENSIONS;
use crate::window::{TabView, WindowState};

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

    /// Runs one rendered link destination.
    pub fn open_link(&mut self, dest: &str, tx: &mut Tx) {
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
            LinkTarget::Local { path, markdown: true, fragment } => {
                self.open_linked_document(path, fragment, tx);
            }
            // L-05: a link to a folder roots the file pane there and shows it,
            // with no tab. The listing decides whether the target really is a
            // folder with Markdown; a plain file falls back to the notice M-31
            // wants before opening something externally.
            LinkTarget::Local { path, markdown: false, .. } => self.open_folder_link(path),
            LinkTarget::Refused(reason) => self.notice(reason),
        }
    }

    /// Opens a Markdown link's document, at its `#section` when it names one.
    ///
    /// A link to the document on screen moves within it instead of reloading
    /// the file (M-30): the same place rule as an outline jump applies, so the
    /// place left goes on Back and a landing under 8 points does not (O-12).
    fn open_linked_document(&mut self, path: PathBuf, fragment: Option<String>, tx: &mut Tx) {
        let path = lmdv_io::absolute_lexical(&path);
        if fragment.is_some() && self.active_path().as_deref() == Some(path.as_path()) {
            if let Some(fragment) = fragment {
                self.jump_fragment(&fragment, tx);
            }
            return;
        }
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
    use tempfile::TempDir;

    use super::*;
    use crate::test_support;

    fn window(temp: &TempDir) -> WindowState {
        WindowState::new(test_support::shared(temp))
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

        let mut state = window(&temp);
        state.open_path(file.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while state.active_tab().and_then(|tab| tab.view.reader.as_ref()).is_none()
            && std::time::Instant::now() < deadline
        {
            let mut tx = Tx::default();
            state.poll(0.0, &mut tx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let target = state.classify_link("next.markdown");
        assert_eq!(target, LinkTarget::Local { path: root.join("next.markdown"), markdown: true, fragment: None });

        assert_eq!(
            state.classify_link("drawing.png"),
            LinkTarget::Local { path: root.join("drawing.png"), markdown: false, fragment: None }
        );
        state.open_link("drawing.png", &mut Tx::default());
        assert_eq!(
            state.browser.root(),
            Some(root.join("drawing.png").as_path()),
            "a non-Markdown link is tried as a folder, never handed to the desktop"
        );
        let target = root.join("drawing.png");
        let generation = state.browser.tree_generation;
        state.finish_tree_listing(generation, target.clone(), Err(lmdv_io::TreeError::NotADirectory(target)));
        assert!(
            state.ui.notice.as_ref().is_some_and(|notice| notice.text.contains("not a Markdown file")),
            "a file that is not a folder says so and goes back"
        );
        assert_eq!(state.browser.root(), Some(root.as_path()));
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
