//! Localized document labels shared by the HTML, DOCX and PDF writers.
//!
//! The reader's equivalent is `lmdv_render::DocLabels`; the export crate cannot
//! depend on Rune, so it resolves the same catalog keys through this small
//! struct. The locale comes from [`ExportDoc::lang`](crate::ExportDoc::lang),
//! the BCP 47 tag the application fills with the UI language (X-07); an
//! unknown tag falls back to English.

use lmdv_i18n::{Key, text_in};
use lmdv_md::CalloutKind;

const CALLOUTS: [Key; 5] = [
    lmdv_i18n::key("render.calloutNote"),
    lmdv_i18n::key("render.calloutTip"),
    lmdv_i18n::key("render.calloutImportant"),
    lmdv_i18n::key("render.calloutWarning"),
    lmdv_i18n::key("render.calloutCaution"),
];
const FOOTNOTES: Key = lmdv_i18n::key("render.footnotes");
const FOOTNOTE_BACK: Key = lmdv_i18n::key("render.footnoteBack");
const IMAGE: Key = lmdv_i18n::key("render.image");
const MERMAID: Key = lmdv_i18n::key("render.mermaid");
const MATH: Key = lmdv_i18n::key("render.math");
const DETAILS: Key = lmdv_i18n::key("render.details");

/// The document labels an export writer draws itself.
pub(crate) struct ExportLabels {
    /// Default callout titles, in [`CalloutKind`] order.
    pub(crate) callouts: [String; 5],
    /// The generated footnotes section's name.
    pub(crate) footnotes: String,
    /// A footnote backlink's accessible name; `{n}` is the footnote number.
    pub(crate) footnote_back: String,
    /// The alt text of a picture with no alt of its own.
    pub(crate) image: String,
    /// The alt text of a rendered Mermaid diagram.
    pub(crate) mermaid: String,
    /// A rendered formula's accessible name; `{tex}` is the TeX source.
    pub(crate) math: String,
    /// The fallback title of a `<details>` block with no `<summary>`.
    pub(crate) details: String,
}

impl ExportLabels {
    /// Labels for a BCP 47 tag; an empty or unknown tag is English.
    pub(crate) fn for_lang(lang: &str) -> Self {
        let locale = lmdv_i18n::negotiate(std::iter::once(lang));
        Self {
            callouts: CALLOUTS.map(|key| text_in(locale, key).to_owned()),
            footnotes: text_in(locale, FOOTNOTES).to_owned(),
            footnote_back: text_in(locale, FOOTNOTE_BACK).to_owned(),
            image: text_in(locale, IMAGE).to_owned(),
            mermaid: text_in(locale, MERMAID).to_owned(),
            math: text_in(locale, MATH).to_owned(),
            details: text_in(locale, DETAILS).to_owned(),
        }
    }

    /// The default title of `kind`.
    pub(crate) fn callout(&self, kind: CalloutKind) -> &str {
        let index = match kind {
            CalloutKind::Note => 0,
            CalloutKind::Tip => 1,
            CalloutKind::Important => 2,
            CalloutKind::Warning => 3,
            CalloutKind::Caution => 4,
        };
        &self.callouts[index]
    }
}
