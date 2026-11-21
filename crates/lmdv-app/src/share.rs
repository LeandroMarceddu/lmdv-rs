//! The Share menu, the rich-copy job and the HTML/DOCX/PDF export job (X-01,
//! X-02, X-05–X-09, X-13, X-15).
//!
//! The menu is one `rune_kit::Menu` under the toolbar's Share button: the
//! command table owns what each item does, so the palette, the button and
//! `Ctrl+Shift+E` all dispatch the same ids (ADR-018). The checkbox writes
//! `Prefs::print_break_h1` through the shared store and keeps the menu open
//! (X-06).
//!
//! Rich copy is a background job like an export: the worker builds the
//! `ExportDoc` and its inline-styled clipboard fragment, and posts back the
//! HTML with the document's Markdown source as the plain form. The UI handler
//! performs the clipboard write, so the job never touches the clipboard and no
//! handler touches the filesystem.
//!
//! Export is one background job per request. The UI flushes the active editor,
//! copies the shared buffer's source and hands the worker (`export_io`) that
//! snapshot with the document's revision. The worker reuses a parse already at
//! that revision or parses the snapshot itself, then builds
//! `ExportDoc::from_md` exactly once, writes the file atomically and posts the
//! result back (OI-103). Nothing here reads the reader's theme, size, font or
//! column width: the export pipeline is separate (ADR-015, X-10).
//!
//! While a job runs, the exporting document's tab is pinned: activation,
//! reordering and closing it are refused until the completion or failure
//! clears `ShareState::in_flight` (OI-105). The job still owns a snapshot, so
//! the worker never depends on the tab staying put.
//!
//! The UI only stores paths. File access is confined to the worker: pictures go
//! through [`lmdv_io::PathPolicy`] before their bytes are read, and the
//! destination's parent must already exist and is canonicalized (X-15). PDF
//! fonts are loaded from system faces once per process with `fontdb`; if none
//! is found the exporter keeps Typst's bundled serif and the outcome records the
//! fallback (OI-085).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use lmdv_core::{DocStatus, Document, DocumentId, JobId};
use lmdv_export::{ExportOptions, PdfFont};
use lmdv_md::MdDoc;
use rune::{ClipboardContent, FileDialogOptions, Tx};

use crate::shared::unix_millis;
use crate::window::{NoticeAction, NoticeKind, WindowState};

/// How long the Share button shows its check after a successful export (X-13).
pub const SHARE_FLASH_SECONDS: f64 = 1.2;
/// How long a share notice with an action (Show in folder, Use the print
/// dialog) stays up.
pub const SHARE_NOTICE_SECONDS: f64 = 8.0;
/// The menu's key; item keys are `share-item0` … `share-item5`.
pub const MENU_KEY: &str = "share";
/// The toolbar button's key. `rune_kit::Menu` wraps it as `share-trigger`.
pub const TOOLBAR_KEY: &str = "share";
/// How many items the Share menu lists.
pub const MENU_ITEMS: usize = 6;

/// One row of the Share menu, in the original's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShareItem {
    /// The command the row dispatches.
    pub command: &'static str,
    /// The label's catalog key.
    pub label: &'static str,
    /// The faint hint under the label, a catalog key.
    pub hint: &'static str,
    /// Whether the row is a setting (a checkbox) rather than an action.
    pub checkbox: bool,
}

/// The six rows, in order: rich copy, Print, PDF, the H1 break setting,
/// HTML, DOCX.
pub static SHARE_ITEMS: [ShareItem; MENU_ITEMS] = [
    ShareItem { command: "share.copy-rich", label: "share.copy", hint: "share.copyHint", checkbox: false },
    ShareItem { command: "share.print", label: "share.print", hint: "share.printHint", checkbox: false },
    ShareItem { command: "share.pdf", label: "share.pdf", hint: "share.pdfHint", checkbox: false },
    ShareItem { command: "share.toggle-break-h1", label: "share.breakH1", hint: "share.breakH1Hint", checkbox: true },
    ShareItem { command: "share.export-html", label: "share.html", hint: "share.htmlHint", checkbox: false },
    ShareItem { command: "share.export-docx", label: "share.docx", hint: "share.docxHint", checkbox: false },
];

/// The formats this slice exports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// One self-contained HTML file (X-07).
    Html,
    /// One OOXML package (X-08).
    Docx,
    /// One A4 PDF (X-05, X-06).
    Pdf,
}

impl ExportFormat {
    /// The save dialog's extension.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Docx => "docx",
            Self::Pdf => "pdf",
        }
    }

    /// The localized filter name's catalog key.
    pub const fn filter_key(self) -> &'static str {
        match self {
            Self::Html => "filter.html",
            Self::Docx => "filter.word",
            Self::Pdf => "filter.pdf",
        }
    }

    /// The save dialog's title catalog key.
    pub const fn dialog_key(self) -> &'static str {
        match self {
            Self::Html => "share.html",
            Self::Docx => "share.docx",
            Self::Pdf => "share.pdf",
        }
    }

    /// The menu command that starts this format.
    pub const fn command(self) -> &'static str {
        match self {
            Self::Html => "share.export-html",
            Self::Docx => "share.export-docx",
            Self::Pdf => "share.pdf",
        }
    }
}

/// What the PDF writer needs from the window: the localized date line and the
/// print setting. Everything else comes from the export job's fixed layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfExport {
    /// The localized medium date in the running header.
    pub date: String,
    /// Start a page before every level-1 heading after the first.
    pub break_on_h1: bool,
}

/// Everything one rich-copy job needs. It is built on the UI thread from a
/// snapshot and moved to the worker; the Markdown source is the plain form of
/// the clipboard content.
#[derive(Clone, Debug)]
pub struct RichCopySpec {
    /// The document the snapshot came from, for the completion check.
    pub doc: DocumentId,
    /// The source the copy must describe, taken after the editor flush.
    pub source: Arc<str>,
    /// The buffer revision `source` came from.
    pub revision: u64,
    /// A parse already at `revision`, when the reader or the cache had one.
    ///
    /// The worker parses `source` when this is missing or older, so a copy
    /// started inside the preview debounce is never stale (OI-103).
    pub model: Option<Arc<MdDoc>>,
    /// The title fallback when the source has no level-1 heading.
    pub title_base: String,
    /// Title, language, source path and content flags.
    pub options: ExportOptions,
}

/// A finished rich copy: the HTML fragment and its plain fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RichCopy {
    /// The document's Markdown source.
    pub markdown: String,
    /// The inline-styled clipboard fragment.
    pub html: String,
}

/// The rich-copy job the window is waiting for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RichCopyJob {
    /// The process-wide job id.
    pub job: JobId,
    /// The document it was started from.
    pub doc: DocumentId,
}

/// Everything one export job needs. It is built on the UI thread from a
/// snapshot taken after the editor flush and moved to the worker.
#[derive(Clone, Debug)]
pub struct ExportSpec {
    /// The document the snapshot came from, for the completion check.
    pub doc: DocumentId,
    /// The source to export, taken after the active editor was flushed.
    pub source: Arc<str>,
    /// The buffer revision `source` came from.
    pub revision: u64,
    /// A parse already at `revision`, when the reader or the cache had one.
    ///
    /// The worker parses `source` when this is missing or older, so an export
    /// started inside the preview debounce is never stale (OI-103).
    pub model: Option<Arc<MdDoc>>,
    /// The title fallback when the source has no level-1 heading.
    pub title_base: String,
    /// Title, language, source path and content flags.
    pub options: ExportOptions,
    /// Which writer to run.
    pub format: ExportFormat,
    /// PDF-only settings.
    pub pdf: Option<PdfExport>,
    /// The chosen destination. It may not exist; its parent must.
    pub path: PathBuf,
    /// Open the written file in the system viewer when the job ends (Print…).
    pub open_viewer: bool,
}

/// A finished export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportOutcome {
    /// The canonical path that was written.
    pub path: PathBuf,
    /// Bytes written.
    pub bytes: u64,
    /// The writer that ran.
    pub format: ExportFormat,
    /// Non-fatal problems (missing pictures, Mermaid fallbacks, refused links).
    pub warnings: Vec<String>,
    /// Which fonts the PDF used, or why it fell back to the bundled serif.
    pub font_note: Option<String>,
}

/// The export job the window is waiting for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportJob {
    /// The process-wide job id.
    pub job: JobId,
    /// The document it was started from.
    pub doc: DocumentId,
    /// The writer that is running.
    pub format: ExportFormat,
    /// Whether Print… opened it in the system viewer.
    pub open_viewer: bool,
}

/// Share-menu and export UI state. It is transient and never persisted.
#[derive(Debug, Default)]
pub struct ShareState {
    /// The menu is open under the Share button.
    pub menu_open: bool,
    /// The export in flight; one at a time (X-05). Its document's tab stays
    /// pinned to the front until the job ends (OI-105).
    pub in_flight: Option<ExportJob>,
    /// The rich copy in flight; one at a time.
    pub copy_in_flight: Option<RichCopyJob>,
    /// The toolbar check's expiry on the virtual clock (X-13).
    pub flash_until: Option<f64>,
    /// The expiry wake has been requested.
    pub flash_armed: bool,
}

impl ShareState {
    /// Whether the Share button draws its successful-export check.
    pub fn flashing(&self) -> bool {
        self.flash_until.is_some()
    }

    /// Whether an export is still running.
    pub fn busy(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Whether a rich copy is still running.
    pub fn copy_busy(&self) -> bool {
        self.copy_in_flight.is_some()
    }
}

impl WindowState {
    /// Opens the Share menu under its button, or closes it (`Ctrl+Shift+E` and
    /// the toolbar button share this).
    pub fn toggle_share_menu(&mut self) {
        if self.share.menu_open {
            self.close_share_menu();
        } else {
            self.open_share_menu();
        }
    }

    /// Opens the Share menu. The menu is unavailable without a document
    /// (X-01); the modal focuses its first enabled row.
    pub fn open_share_menu(&mut self) {
        if self.active_document_id().is_none() {
            return;
        }
        self.ui.block_menu = None;
        self.share.menu_open = true;
    }

    /// Closes the Share menu; focus returns to the Share button through the
    /// menu's modal (X-01).
    pub fn close_share_menu(&mut self) {
        self.share.menu_open = false;
    }

    /// Flips `Prefs::print_break_h1` and persists it (X-06). The caller keeps
    /// the menu open.
    pub fn toggle_print_break_h1(&mut self) {
        let mut prefs = self.shared().prefs().clone();
        prefs.print_break_h1 = !prefs.print_break_h1;
        self.shared().set_prefs(prefs);
    }

    /// Starts a save dialog for `format`, or the scripted/test seam.
    ///
    /// The default name is the source stem, or the pasted title made file-safe
    /// (X-09). Cancel does nothing.
    pub(crate) fn request_export(&mut self, format: ExportFormat, tx: &mut Tx) {
        if self.share.busy() {
            self.notice(lmdv_i18n::t!("msg.exportBusy"));
            return;
        }
        let Some(document) = self.active_tab().and_then(|tab| tab.document()).cloned() else { return };
        let base = export_base(&document);
        let name = format!("{base}.{}", format.extension());
        let pdf = (format == ExportFormat::Pdf).then(|| pdf_export(self));
        if self.shared().is_headless()
            && let Some(dir) = export_seam_dir()
        {
            self.start_export(format, dir.join(name), pdf, false);
            return;
        }
        let directory = document.key.borrow().path().and_then(Path::parent).map(Path::to_path_buf);
        let filter = format.filter_key();
        let title = lmdv_i18n::text(lmdv_i18n::key(format.dialog_key())).to_owned();
        let options = FileDialogOptions { title, directory, file_name: Some(name), ..FileDialogOptions::default() }
            .filter(lmdv_i18n::text(lmdv_i18n::key(filter)), &[format.extension()]);
        tx.save_file(options, move |state: &mut WindowState, result, _| match result {
            Ok(Some(path)) => state.start_export(format, path, pdf, false),
            Ok(None) => {}
            Err(error) => state.notice(lmdv_i18n::t!("msg.exportFailed", "err" => error)),
        });
    }

    /// `Print…`: one temporary PDF, then the desktop's viewer (X-04/X-05). No
    /// save dialog is shown.
    pub(crate) fn print_document(&mut self) {
        if self.share.busy() {
            self.notice(lmdv_i18n::t!("msg.exportBusy"));
            return;
        }
        let path = print_temp_path();
        self.start_export(ExportFormat::Pdf, path, Some(pdf_export(self)), true);
    }

    /// Starts the export job for the active document. `path` may not exist; its
    /// parent must (X-15).
    ///
    /// The active editor is flushed first, then the shared buffer is snapshotted
    /// with its revision. A parse already at that revision rides along; otherwise
    /// the worker parses the snapshot itself, so an edit typed just before Share
    /// (inside the 120 ms preview debounce) is exported (OI-103).
    pub(crate) fn start_export(
        &mut self,
        format: ExportFormat,
        path: PathBuf,
        pdf: Option<PdfExport>,
        open_viewer: bool,
    ) {
        if self.share.busy() {
            self.notice(lmdv_i18n::t!("msg.exportBusy"));
            return;
        }
        let Some(document) = self.active_tab().and_then(|tab| tab.document()).cloned() else { return };
        if !matches!(*document.status.borrow(), DocStatus::Ready | DocStatus::Gone) {
            self.notice(lmdv_i18n::t!("msg.exportNotReady"));
            return;
        }
        self.flush_export_text();
        let revision = document.revision();
        let source = document.snapshot();
        let model = self.parsed_for_export(document.id, revision);
        let title_base = export_base(&document);
        let title =
            model.as_ref().and_then(|model| export_title(&document, model)).unwrap_or_else(|| title_base.clone());
        let options = ExportOptions {
            title: Some(title),
            lang: lmdv_i18n::locale_id().to_owned(),
            include_toc: false,
            math: true,
            front_matter: false,
            source_path: document.key.borrow().path().map(Path::to_path_buf),
        };
        let spec = ExportSpec {
            doc: document.id,
            source,
            revision,
            model,
            title_base,
            options,
            format,
            pdf,
            path,
            open_viewer,
        };
        let job = self.shared().jobs().export(self.worker().clone(), spec);
        self.share.flash_until = None;
        self.share.flash_armed = false;
        self.share.in_flight = Some(ExportJob { job, doc: document.id, format, open_viewer });
    }

    /// Starts the rich-copy job for the active document (X-02). The copy is
    /// always the whole document; selection copy is X-03.
    ///
    /// Like an export it flushes and snapshots the current revision first, so the
    /// clipboard describes the buffer the user sees, never the last parsed model
    /// (OI-103).
    pub(crate) fn start_rich_copy(&mut self) {
        if self.share.copy_busy() {
            self.notice(lmdv_i18n::t!("msg.copyBusy"));
            return;
        }
        let Some(document) = self.active_tab().and_then(|tab| tab.document()).cloned() else { return };
        if !matches!(*document.status.borrow(), DocStatus::Ready | DocStatus::Gone) {
            self.notice(lmdv_i18n::t!("msg.copyNotReady"));
            return;
        }
        self.flush_export_text();
        let revision = document.revision();
        // Unsaved edits live in the shared source buffer, so the plain form is
        // the Markdown the user currently sees, not the last saved file.
        let source = document.snapshot();
        let model = self.parsed_for_export(document.id, revision);
        let title_base = export_base(&document);
        let options = ExportOptions {
            title: Some(title_base.clone()),
            lang: lmdv_i18n::locale_id().to_owned(),
            include_toc: false,
            math: true,
            front_matter: false,
            source_path: document.key.borrow().path().map(Path::to_path_buf),
        };
        let spec = RichCopySpec { doc: document.id, source, revision, model, title_base, options };
        self.share.flash_until = None;
        self.share.flash_armed = false;
        let job = self.shared().jobs().copy_rich(self.worker().clone(), spec);
        self.share.copy_in_flight = Some(RichCopyJob { job, doc: document.id });
    }

    /// Commits an active editor's field text into the shared buffer when the
    /// two differ. Every input path already calls `editor_changed`, so this is a
    /// belt-and-braces flush for a key that lands in the same frame as Share
    /// (OI-103); a viewer tab has no editor and needs nothing.
    fn flush_export_text(&mut self) {
        let Some(stale) = self.active_tab().and_then(|tab| {
            let editor = tab.view.editor.as_ref()?;
            let document = tab.document()?;
            Some(editor.text() != document.text().text())
        }) else {
            return;
        };
        if stale {
            self.editor_changed();
        }
    }

    /// The parse a job may hand to a worker: the active tab's model when it is
    /// already at `revision`, else the shared cache's, else `None` so the
    /// worker parses the snapshot.
    fn parsed_for_export(&self, doc: DocumentId, revision: u64) -> Option<Arc<MdDoc>> {
        let reader = self.active_tab().and_then(|tab| tab.view.reader.clone());
        reader
            .filter(|model| model.revision == revision)
            .or_else(|| self.shared().parsed(doc, revision).and_then(|parsed| parsed.ok()))
    }

    /// Handles a rich-copy completion: writes both clipboard forms from the UI
    /// handler, flashes the Share check and shows a localized notice.
    pub(crate) fn handle_rich_copied(
        &mut self,
        job: JobId,
        doc: DocumentId,
        result: Result<RichCopy, String>,
        tx: &mut Tx,
    ) -> bool {
        let Some(pending) = self.share.copy_in_flight.as_ref() else { return false };
        if pending.job != job || pending.doc != doc {
            return false;
        }
        self.share.copy_in_flight = None;
        match result {
            Ok(copy) => {
                self.share.flash_until = Some(self.ui.last_now + SHARE_FLASH_SECONDS);
                self.share.flash_armed = false;
                tx.copy_rich(ClipboardContent::new(copy.markdown).with_html(copy.html));
                self.show_share_notice(lmdv_i18n::t!("msg.copiedRich"), None, NoticeKind::Success);
                true
            }
            Err(error) => {
                self.show_share_notice(lmdv_i18n::t!("msg.copyFailed", "err" => error), None, NoticeKind::Error);
                true
            }
        }
    }

    /// Handles a job completion. A completion for another request changes
    /// nothing.
    pub(crate) fn handle_exported(
        &mut self,
        job: JobId,
        doc: DocumentId,
        format: ExportFormat,
        result: Result<ExportOutcome, String>,
    ) -> bool {
        let Some(pending) = self.share.in_flight.as_ref() else { return false };
        if pending.job != job || pending.doc != doc || pending.format != format {
            return false;
        }
        let open_viewer = pending.open_viewer;
        self.share.in_flight = None;
        match result {
            Ok(outcome) => {
                self.share.flash_until = Some(self.ui.last_now + SHARE_FLASH_SECONDS);
                self.share.flash_armed = false;
                match format {
                    ExportFormat::Pdf if open_viewer => self.show_share_notice(
                        format!("{} {}", lmdv_i18n::t!("msg.pdfSaved"), outcome.path.display()),
                        None,
                        NoticeKind::Success,
                    ),
                    ExportFormat::Pdf => {
                        let action =
                            outcome.path.parent().map(|parent| NoticeAction::ShowInFolder(parent.to_path_buf()));
                        self.show_share_notice(lmdv_i18n::t!("msg.pdfSaved"), action, NoticeKind::Success);
                    }
                    ExportFormat::Html | ExportFormat::Docx => self.show_share_notice(
                        format!("{} {}", lmdv_i18n::t!("msg.saved"), outcome.path.display()),
                        None,
                        NoticeKind::Success,
                    ),
                }
                true
            }
            Err(error) => {
                let text = if format == ExportFormat::Pdf {
                    lmdv_i18n::t!("msg.pdfFailed", "err" => error)
                } else {
                    lmdv_i18n::t!("msg.exportFailed", "err" => error)
                };
                let action = (format == ExportFormat::Pdf).then_some(NoticeAction::UsePrintDialog);
                self.show_share_notice(text, action, NoticeKind::Error);
                true
            }
        }
    }

    /// A hidden system open failed (Show in folder, Print…'s viewer).
    pub(crate) fn handle_opened(&mut self, path: PathBuf, result: Result<(), String>) -> bool {
        match result {
            Ok(()) => false,
            Err(error) => {
                self.notice_kind(
                    lmdv_i18n::t!("msg.openFailed", "name" => path.display(), "err" => error),
                    NoticeKind::Error,
                );
                true
            }
        }
    }

    fn show_share_notice(&mut self, text: impl Into<String>, action: Option<NoticeAction>, kind: NoticeKind) {
        self.show_notice(text.into(), kind, action, SHARE_NOTICE_SECONDS);
    }

    /// Keeps the export/menu state consistent with the tabs: a menu with no
    /// document closes, and the flash expiry wakes the loop once.
    pub(crate) fn poll_share(&mut self, now: f64, tx: &mut Tx) -> bool {
        let mut changed = false;
        if self.share.menu_open && self.active_document_id().is_none() {
            self.share.menu_open = false;
            changed = true;
        }
        if let Some(until) = self.share.flash_until {
            if now >= until {
                self.share.flash_until = None;
                self.share.flash_armed = false;
                changed = true;
            } else if !self.share.flash_armed {
                self.share.flash_armed = true;
                tx.wake_at(until);
            }
        }
        changed
    }

    /// The document an export in flight pins to the front of this window
    /// (OI-105). `None` when no export runs.
    pub fn export_pinned_document(&self) -> Option<DocumentId> {
        self.share.in_flight.as_ref().map(|job| job.doc)
    }

    /// Whether an export in flight keeps the tab showing `doc` in place.
    pub fn export_pins(&self, doc: DocumentId) -> bool {
        self.export_pinned_document() == Some(doc)
    }

    /// Refuses a tab operation that would leave an exporting document and
    /// explains why. Returns `true` when the caller must stop (OI-105).
    ///
    /// The job owns a snapshot, so the refusal is about the promised UI state,
    /// not about the worker's data. The completion or failure clears
    /// `ShareState::in_flight` and every operation works again.
    pub(crate) fn export_holds_tab(&mut self) -> bool {
        if self.share.in_flight.is_some() {
            self.notice(lmdv_i18n::t!("msg.exportBusy"));
            true
        } else {
            false
        }
    }
}

/// The PDF settings for a print/export, including the localized medium date.
pub(crate) fn pdf_export(state: &WindowState) -> PdfExport {
    PdfExport {
        date: medium_date(unix_millis() / 1_000, lmdv_i18n::locale()),
        break_on_h1: state.shared().prefs().print_break_h1,
    }
}

/// The title an export uses: the first level-1 heading, else the source stem or
/// pasted title (X-14). Returns `None` only when the caller has no title at all.
pub fn export_title(document: &Document, model: &MdDoc) -> Option<String> {
    Some(export_title_text(model, &export_base(document)))
}

/// [`export_title`] from an already parsed model and a prepared fallback.
///
/// The export worker calls this after it has chosen the model, so the title and
/// the body always come from the same revision (OI-103).
pub(crate) fn export_title_text(model: &MdDoc, base: &str) -> String {
    model
        .headings
        .iter()
        .find(|heading| heading.level == 1)
        .map(|heading| heading.text.trim())
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| base.to_owned())
}

/// The name an export starts with: the source stem, else the pasted title made
/// file-safe, else the localized “Pasted text” (X-09).
pub fn export_base(document: &Document) -> String {
    if let Some(path) = document.key.borrow().path()
        && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
    {
        let stem = stem.trim();
        if !stem.is_empty() {
            return stem.to_owned();
        }
    }
    let title = document.title.borrow();
    let safe = file_safe_name(&title);
    if !safe.is_empty() {
        return safe;
    }
    lmdv_i18n::t!("paste.untitled").to_owned()
}

/// A Windows-file-safe name: no path separators or reserved characters, no
/// trailing dot or space, 80 characters, and no reserved device base name. The
/// original's rule (`docs.ts:286`).
pub fn file_safe_name(title: &str) -> String {
    let tidy = |text: &str| text.trim_end_matches(['.', ' ']).to_owned();
    let cleaned: String = title
        .chars()
        .map(|ch| {
            if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\u{0}'..='\u{1f}' | '\u{7f}') {
                ' '
            } else {
                ch
            }
        })
        .collect();
    let mut name = tidy(&cleaned.split_whitespace().collect::<Vec<_>>().join(" "));
    if name.chars().count() > 80 {
        name = name.chars().take(80).collect();
        name = tidy(&name);
    }
    while name.len() > 200 {
        name.pop();
        name = tidy(&name);
    }
    let base = name.split('.').next().unwrap_or("").trim_end_matches(' ').to_ascii_lowercase();
    let device = matches!(base.as_str(), "con" | "prn" | "aux" | "nul" | "conin$" | "conout$")
        || (base.len() == 4
            && (base.starts_with("com") || base.starts_with("lpt"))
            && base.as_bytes()[3].is_ascii_digit());
    if device {
        let rest = name.get(base.len()..).unwrap_or("");
        return format!("{base}_{rest}");
    }
    name
}

/// A unique temporary PDF for `Print…`. The file is left in place for the
/// viewer; the OS temp sweep owns it.
pub fn print_temp_path() -> PathBuf {
    std::env::temp_dir().join(format!("lmdv-print-{}-{}.pdf", std::process::id(), unix_millis()))
}

/// The scripted/test seam for a native save dialog: `LMDV_EXPORT_DIR`.
///
/// A headless script cannot answer a dialog, so when this variable is set the
/// export writes `<dir>/<default name>` without opening one. Desktop launches
/// never set it.
fn export_seam_dir() -> Option<PathBuf> {
    std::env::var_os("LMDV_EXPORT_DIR").map(PathBuf::from).filter(|dir| !dir.as_os_str().is_empty())
}

// ---- PDF fonts (OI-085) --------------------------------------------------

/// The system faces the PDF uses, loaded and cached once per process.
#[derive(Clone, Debug)]
pub struct ExportFonts {
    /// The body family the exporter asks Typst for.
    pub body_family: String,
    /// The monospace family.
    pub code_family: String,
    /// Both faces' encoded bytes.
    pub fonts: Vec<PdfFont>,
    /// Which faces were loaded, or why the bundled serif is used.
    pub note: String,
}

static PDF_FONTS: OnceLock<ExportFonts> = OnceLock::new();

/// The cached PDF faces. The first call scans system fonts in the export
/// worker; every later export clones the bytes.
pub fn pdf_fonts() -> &'static ExportFonts {
    PDF_FONTS.get_or_init(load_pdf_fonts)
}

const BODY_FAMILIES: [&str; 4] = ["Noto Sans", "DejaVu Sans", "Cantarell", "Liberation Sans"];
const CODE_FAMILIES: [&str; 3] = ["DejaVu Sans Mono", "Noto Sans Mono", "Liberation Mono"];

fn load_pdf_fonts() -> ExportFonts {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    let body = pick_face(&database, &BODY_FAMILIES);
    let code = pick_face(&database, &CODE_FAMILIES);
    let mut fonts = Vec::new();
    let mut names = Vec::new();
    for face in [body.as_ref(), code.as_ref()].into_iter().flatten() {
        if !fonts.iter().any(|font: &PdfFont| font.family == face.family) {
            fonts.push(PdfFont::new(face.family.clone(), face.bytes.clone()));
        }
        if !names.contains(&face.family) {
            names.push(face.family.clone());
        }
    }
    let (body_family, code_family) = match (&body, &code) {
        (Some(body), Some(code)) => (body.family.clone(), code.family.clone()),
        (Some(body), None) => (body.family.clone(), String::new()),
        (None, Some(code)) => (String::new(), code.family.clone()),
        (None, None) => (String::new(), String::new()),
    };
    let note = match (body, code) {
        (Some(body), Some(code)) => format!("{} + {}", body.family, code.family),
        (Some(body), None) => format!("{} (no system mono face; bundled DejaVu Sans Mono)", body.family),
        (None, Some(_code)) => format!("bundled serif body (no system sans face); code {code_family}"),
        (None, None) => "the bundled serif body and mono (no system sans or mono face)".to_owned(),
    };
    ExportFonts { body_family, code_family, fonts, note }
}

struct LoadedFace {
    family: String,
    bytes: Vec<u8>,
}

/// The first regular face of the first installed candidate family.
fn pick_face(database: &fontdb::Database, candidates: &[&str]) -> Option<LoadedFace> {
    for name in candidates {
        let query = fontdb::Query {
            families: &[fontdb::Family::Name(name)],
            weight: fontdb::Weight::NORMAL,
            stretch: fontdb::Stretch::Normal,
            style: fontdb::Style::Normal,
        };
        let Some(id) = database.query(&query) else { continue };
        let Some(face) = database.face(id) else { continue };
        let family = face.families.first().map(|(family, _)| family.clone()).unwrap_or_else(|| (*name).to_owned());
        let Some(bytes) = database.with_face_data(id, |data, _index| data.to_vec()) else { continue };
        if !bytes.is_empty() {
            return Some(LoadedFace { family, bytes });
        }
    }
    None
}

// ---- localized medium date (X-14) ---------------------------------------

/// A localized `dateStyle: "medium"` date for the PDF header.
///
/// The app has no time-zone formatter yet (OI-033); the calendar date is UTC.
/// The six catalogs' month names are inlined here, matching their `Intl`
/// medium forms: `Nov 14, 2026`, `14 nov 2026`, `14.11.2026`, `14 nov. 2026`,
/// `14 nov 2026`, `2026年11月14日`.
pub fn medium_date(unix_secs: u64, locale: usize) -> String {
    let (year, month, day) = civil_from_days((unix_secs / 86_400) as i64);
    let id = lmdv_i18n::locales().get(locale).map(|locale| locale.id()).unwrap_or("en");
    match id {
        "it" => format!(
            "{day} {} {year}",
            ["gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic"][month - 1]
        ),
        "de" => format!("{day:02}.{month:02}.{year}"),
        "fr" => format!(
            "{day} {} {year}",
            ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."]
                [month - 1]
        ),
        "es" => format!(
            "{day} {} {year}",
            ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic"][month - 1]
        ),
        "zh-CN" => format!("{year}年{month}月{day}日"),
        _ => format!(
            "{} {day}, {year}",
            ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][month - 1]
        ),
    }
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, usize, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as usize;
    (if month <= 2 { year + 1 } else { year }, month, day)
}
