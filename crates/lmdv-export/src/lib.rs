//! Self-contained export from the Rune-free [`lmdv_md::MdDoc`] model.
//!
//! [`ExportDoc::from_md`] resolves pictures, formulas and diagrams once into a
//! format-neutral document. [`ExportDoc::to_html`] writes a single HTML file,
//! [`ExportDoc::to_docx`] an OOXML package and [`ExportDoc::to_pdf`] a
//! print-ready A4 PDF with a page model. [`export_html`] is the one-call HTML
//! convenience for the app; [`export_rich_html`] is the inline-styled clipboard
//! fragment for the Share menu's rich copy (X-02), [`export_rich_fragment`] the
//! same fragment for a selected top-level block range (X-03), and
//! [`table_tsv`] the spreadsheet copy of a table (X-16). [`raster::svg_to_png`]
//! turns an SVG picture into a PNG for the preview's copy and save actions.
//!
//! The exporter performs no file I/O and no network access. The application
//! supplies picture bytes through [`AssetResolver`] and font bytes through
//! [`PdfOptions::fonts`]; relative links become `file:///` URLs only when
//! [`ExportOptions::source_path`] is absolute.
//!
//! ADR-015: this is a separate rendering pipeline. Reader theme, size and column
//! width never reach the output; the article layout is fixed, light and
//! print-safe.
//!
//! # Example
//!
//! ```no_run
//! use lmdv_export::{AssetResolver, ExportedAsset, ExportOptions, export_html};
//!
//! struct Images;
//! impl AssetResolver for Images {
//!     fn image(&self, _src: &str, _alt: &str) -> Option<ExportedAsset> {
//!         None
//!     }
//!     fn diagram(&self, _source: &str) -> Option<ExportedAsset> {
//!         None
//!     }
//! }
//!
//! let doc = lmdv_md::parse("# Hello\n\nText with $x^2$.\n");
//! let html = export_html(&doc, &ExportOptions::default(), &Images).unwrap();
//! assert!(html.starts_with("<!doctype html>"));
//! ```

#![forbid(unsafe_code)]

mod build;
mod docx;
mod html;
mod labels;
mod link;
mod model;
mod pdf;
pub mod raster;
mod rich;
mod tsv;

pub use docx::{DocxOptions, DocxPage, export_docx};
pub use model::*;
pub use pdf::{PdfFont, PdfOptions, PdfPage, export_pdf};
pub use rich::{RichOptions, export_rich_fragment, export_rich_html};
pub use tsv::table_tsv;

/// Re-encodes a raster picture (PNG, JPEG, GIF, WebP, BMP or AVIF) as PNG.
///
/// The preview's copy/save actions need PNG bytes; the app has no image
/// decoder of its own, and this crate already reads pictures for the exporters.
/// An SVG goes through [`raster::svg_to_png`] instead. AVIF uses the pure-Rust
/// decoder in [`raster::avif_to_png`] (OI-183). Call it on a worker. `None`
/// when the bytes do not decode.
pub fn raster_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    use image::ImageEncoder;
    let image = match image::load_from_memory(bytes) {
        Ok(image) => image.to_rgba8(),
        Err(_) => return raster::avif_to_png(bytes).map(|(png, _)| png),
    };
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(image.as_raw(), image.width(), image.height(), image::ExtendedColorType::Rgba8)
        .ok()?;
    Some(png)
}

/// Writes `doc` as one standalone, self-contained HTML file.
///
/// This is `ExportDoc::from_md(...)?` followed by `to_html()`; use the two steps
/// when a PDF or DOCX export will reuse the resolved assets.
pub fn export_html(
    doc: &lmdv_md::MdDoc,
    opts: &ExportOptions,
    assets: &dyn AssetResolver,
) -> Result<String, ExportError> {
    ExportDoc::from_md(doc, opts, assets)?.to_html()
}
