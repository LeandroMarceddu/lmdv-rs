//! Worker-only file access for the Share exports and the rich copy.
//!
//! Everything here runs inside a job spawned by [`crate::jobs::JobHub`], never
//! in a handler or a view (ADR-017). The UI hands in a snapshot [`ExportSpec`]
//! or [`crate::share::RichCopySpec`]; this module builds the export model once,
//! writes the chosen format atomically and, for Print…, opens the result. The
//! rich copy returns the clipboard HTML and the Markdown fallback without
//! touching the clipboard.
//!
//! Picture bytes are read only after [`lmdv_io::PathPolicy`] validated them
//! against the document folder; the destination's parent must exist and is
//! canonicalized (X-15). Network and `data:` sources never reach the filesystem.

use std::path::{Path, PathBuf};

use lmdv_export::{AssetResolver, DocxOptions, ExportDoc, ExportedAsset, PdfOptions, RichOptions, export_rich_html};
use lmdv_io::PathPolicy;

use crate::share::{ExportFormat, ExportOutcome, ExportSpec, pdf_fonts};

/// Builds the export model once and writes the chosen format off the UI thread.
pub(crate) fn run_export(spec: &ExportSpec) -> Result<ExportOutcome, String> {
    let assets = ExportAssets::for_source(spec.options.source_path.as_deref());
    let doc = ExportDoc::from_md(&spec.model, &spec.options, &assets).map_err(|error| error.to_string())?;
    let (bytes, font_note) = match spec.format {
        ExportFormat::Html => (doc.to_html().map_err(|error| error.to_string())?.into_bytes(), None),
        ExportFormat::Docx => (doc.to_docx(&DocxOptions::default()).map_err(|error| error.to_string())?, None),
        ExportFormat::Pdf => {
            let fonts = pdf_fonts();
            let options = PdfOptions {
                date: spec.pdf.as_ref().map(|pdf| pdf.date.clone()),
                break_on_h1: spec.pdf.as_ref().is_some_and(|pdf| pdf.break_on_h1),
                body_font: fonts.body_family.clone(),
                code_font: fonts.code_family.clone(),
                fonts: fonts.fonts.clone(),
                ..PdfOptions::default()
            };
            (doc.to_pdf(&options).map_err(|error| error.to_string())?, Some(fonts.note.clone()))
        }
    };
    let path = write_export(&spec.path, &bytes)?;
    if spec.open_viewer {
        open_external_path(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    let warnings = doc.warnings.iter().map(|warning| warning.message.clone()).collect();
    Ok(ExportOutcome { path, bytes: bytes.len() as u64, format: spec.format, warnings, font_note })
}

/// Builds the whole-document rich clipboard fragment off the UI thread.
///
/// The plain form is the Markdown source captured on the UI thread; the HTML
/// is the inline-styled export model, resolved through the same asset policy as
/// the file exports.
pub(crate) fn run_rich_copy(spec: &crate::share::RichCopySpec) -> Result<crate::share::RichCopy, String> {
    let assets = ExportAssets::for_source(spec.options.source_path.as_deref());
    let doc = ExportDoc::from_md(&spec.model, &spec.options, &assets).map_err(|error| error.to_string())?;
    let options = RichOptions {
        lang: spec.options.lang.clone(),
        source_path: spec.options.source_path.clone(),
        title: Some(doc.title.clone()),
    };
    let html = export_rich_html(&doc, &options);
    Ok(crate::share::RichCopy { markdown: spec.markdown.clone(), html })
}

/// Writes `bytes` to `path` atomically. The parent folder must exist; a target
/// inside it may not.
fn write_export(path: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    lmdv_io::path_policy::reject_network_or_device(path).map_err(|error| error.to_string())?;
    let target = lmdv_io::atomic::canonical_target(path)
        .map_err(|error| format!("{} cannot be used: {error}", path.display()))?;
    lmdv_io::atomic::write_atomic(&target, bytes).map_err(|error| format!("{}: {error}", target.display()))?;
    Ok(target)
}

/// Opens `path` with the desktop's handler. Called from a worker only, never
/// from a handler or a view.
pub(crate) fn open_external_path(path: &Path) -> Result<(), String> {
    lmdv_io::open_external(path).map_err(|error| error.to_string())
}

/// Resolves a local picture through the document's path policy and returns its
/// bytes and MIME type. Network URLs, `data:` URIs and pictures outside the
/// document folder are refused; Mermaid stays a source block (OI-106).
struct ExportAssets {
    root: Option<PathBuf>,
}

impl ExportAssets {
    /// Asserts pictures against `source_path`'s folder. A document with no file
    /// (pasted text) embeds no local pictures.
    fn for_source(source_path: Option<&Path>) -> Self {
        Self {
            root: source_path
                .and_then(Path::parent)
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(Path::to_path_buf),
        }
    }

    fn picture(&self, src: &str) -> Option<ExportedAsset> {
        let src = src.trim();
        if src.is_empty() || src.get(..5).is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:")) {
            return None;
        }
        let root = self.root.as_deref()?;
        let local = src.strip_prefix("file://").unwrap_or(src);
        let local = local.strip_prefix("localhost/").map(|rest| format!("/{rest}")).unwrap_or_else(|| local.to_owned());
        let policy = PathPolicy::new(root);
        let (resolved, bytes) = policy.read_asset(Path::new(&local)).ok()?;
        if bytes.is_empty() {
            return None;
        }
        Some(ExportedAsset::new(mime_for(&bytes, &resolved.path), bytes))
    }
}

impl AssetResolver for ExportAssets {
    fn image(&self, src: &str, _alt: &str) -> Option<ExportedAsset> {
        self.picture(src)
    }

    fn diagram(&self, _source: &str) -> Option<ExportedAsset> {
        // No Mermaid raster in this slice: the exporter keeps the source as a
        // code block and records a `DiagramFallback` warning (OI-076/OI-106).
        None
    }
}

/// The image MIME type for an embedded picture: the extension first, then the
/// file's magic bytes for extensionless pictures.
fn mime_for(bytes: &[u8], path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("bmp") => "image/bmp",
        Some("svg") => "image/svg+xml",
        _ if bytes.starts_with(b"\x89PNG") => "image/png",
        _ if bytes.starts_with(b"\xff\xd8") => "image/jpeg",
        _ if bytes.starts_with(b"GIF8") => "image/gif",
        _ if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") => "image/webp",
        _ if bytes.starts_with(b"BM") => "image/bmp",
        _ if bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml") => "image/svg+xml",
        _ => "image/png",
    }
}
