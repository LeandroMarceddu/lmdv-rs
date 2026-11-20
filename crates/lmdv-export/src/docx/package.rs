//! The OOXML package: a deterministic ZIP container and the fixed parts that
//! do not depend on the document ([`[Content_Types].xml`], the package
//! relationships, Word settings and the extended properties).
//!
//! The archive is written by hand instead of through a zip crate so that the
//! bytes depend only on the document: every entry carries the fixed 1980-01-01
//! DOS timestamp and the entry order is the order of [`zip`]. XML parts are
//! deflated with a fixed compression level; pictures stay stored because they
//! are already compressed. Deflating can only make an entry fail to shrink, in
//! which case it is stored, so an in-memory package cannot fail to write.
//!
//! OI-079: this replaced the initial stored-only archive; the package is valid
//! OPC either way, but the XML parts deflate by 60–90% in a typical document.

use std::borrow::Cow;
use std::io::Write as _;

/// One part of the package.
pub(crate) struct Part {
    /// The entry name, with forward slashes (`word/document.xml`).
    pub name: String,
    /// The raw part bytes.
    pub bytes: Vec<u8>,
}

impl Part {
    /// A UTF-8 XML part.
    pub(crate) fn text(name: impl Into<String>, xml: impl Into<String>) -> Self {
        Self { name: name.into(), bytes: xml.into().into_bytes() }
    }
}

/// The DOS time field: midnight.
const DOS_TIME: u16 = 0;
/// The DOS date field: 1980-01-01, the earliest representable zip date.
const DOS_DATE: u16 = 0x0021;
/// The zip "version needed" / "version made by" value for stored and deflated
/// entries (both are ZIP 2.0).
const VERSION: u16 = 20;
/// The general-purpose flag: file names are UTF-8.
const UTF8_FLAG: u16 = 0x0800;
/// The stored compression method.
const STORED: u16 = 0;
/// The deflate compression method.
const DEFLATED: u16 = 8;
/// The fixed deflate level. It is part of the byte-for-byte output contract.
const DEFLATE_LEVEL: u32 = 6;
const LOCAL_HEADER: u32 = 0x0403_4b50;
const CENTRAL_HEADER: u32 = 0x0201_4b50;
const END_OF_CENTRAL: u32 = 0x0605_4b50;

/// Writes `parts` as one deterministic ZIP archive.
///
/// XML and relationship parts are deflated; media bytes are stored. A part
/// whose deflated form is not smaller is stored too, so the method never
/// changes the content. The package is bounded by the 4 GiB ZIP32 limit; a
/// larger archive cannot be produced by the exporter, so sizes and offsets are
/// clamped instead of panicking in the unreachable case.
pub(crate) fn zip(parts: &[Part]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for part in parts {
        let name = part.name.as_bytes();
        let crc = crc32(&part.bytes);
        let data = compressed(&part.name, &part.bytes);
        let method = if matches!(data, Cow::Owned(_)) { DEFLATED } else { STORED };
        let size = clamp_u32(part.bytes.len());
        let packed = clamp_u32(data.len());
        let offset = clamp_u32(out.len());

        push_u32(&mut out, LOCAL_HEADER);
        push_u16(&mut out, VERSION);
        push_u16(&mut out, UTF8_FLAG);
        push_u16(&mut out, method);
        push_u16(&mut out, DOS_TIME);
        push_u16(&mut out, DOS_DATE);
        push_u32(&mut out, crc);
        push_u32(&mut out, packed);
        push_u32(&mut out, size);
        push_u16(&mut out, clamp_u16(name.len()));
        push_u16(&mut out, 0);
        out.extend_from_slice(name);
        out.extend_from_slice(&data);

        push_u32(&mut central, CENTRAL_HEADER);
        push_u16(&mut central, VERSION);
        push_u16(&mut central, VERSION);
        push_u16(&mut central, UTF8_FLAG);
        push_u16(&mut central, method);
        push_u16(&mut central, DOS_TIME);
        push_u16(&mut central, DOS_DATE);
        push_u32(&mut central, crc);
        push_u32(&mut central, packed);
        push_u32(&mut central, size);
        push_u16(&mut central, clamp_u16(name.len()));
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, 0);
        push_u32(&mut central, offset);
        central.extend_from_slice(name);
    }

    let directory_offset = clamp_u32(out.len());
    let directory_size = clamp_u32(central.len());
    out.extend_from_slice(&central);
    push_u32(&mut out, END_OF_CENTRAL);
    push_u16(&mut out, 0);
    push_u16(&mut out, 0);
    push_u16(&mut out, clamp_u16(parts.len()));
    push_u16(&mut out, clamp_u16(parts.len()));
    push_u32(&mut out, directory_size);
    push_u32(&mut out, directory_offset);
    push_u16(&mut out, 0);
    out
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn clamp_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

fn clamp_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// The bytes an entry stores: a deflated XML/relationship part when that is
/// smaller, else the part unchanged.
fn compressed<'a>(name: &str, bytes: &'a [u8]) -> Cow<'a, [u8]> {
    let text_part = name.ends_with(".xml") || name.ends_with(".rels");
    if !text_part || bytes.is_empty() {
        return Cow::Borrowed(bytes);
    }
    match deflate(bytes) {
        Some(packed) if packed.len() < bytes.len() => Cow::Owned(packed),
        _ => Cow::Borrowed(bytes),
    }
}

/// A raw deflate stream at [`DEFLATE_LEVEL`]. `None` when the encoder fails;
/// the entry is then stored, so a package is still written.
fn deflate(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(DEFLATE_LEVEL));
    encoder.write_all(bytes).ok()?;
    encoder.finish().ok()
}

/// The standard CRC-32 used by ZIP (IEEE 802.3, reflected, polynomial
/// `0xEDB88320`). Table-free: a document has a handful of parts and the code
/// stays small.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// `[Content_Types].xml` with the defaults for every raster type the writer
/// embeds, plus the document-part overrides. `footnotes` adds the override for
/// `word/footnotes.xml`, which a document without notes does not carry.
pub(crate) fn content_types(footnotes: bool) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\n\
         <Default Extension=\"png\" ContentType=\"image/png\"/>\n\
         <Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>\n\
         <Default Extension=\"gif\" ContentType=\"image/gif\"/>\n\
         <Default Extension=\"bmp\" ContentType=\"image/bmp\"/>\n\
         <Default Extension=\"webp\" ContentType=\"image/webp\"/>\n",
    );
    for (part, kind) in [
        ("/word/document.xml", "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"),
        ("/word/styles.xml", "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"),
        ("/word/numbering.xml", "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"),
        ("/word/settings.xml", "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"),
        ("/docProps/core.xml", "application/vnd.openxmlformats-package.core-properties+xml"),
        ("/docProps/app.xml", "application/vnd.openxmlformats-officedocument.extended-properties+xml"),
    ] {
        xml.push_str(&format!("<Override PartName=\"{part}\" ContentType=\"{kind}\"/>\n"));
    }
    if footnotes {
        xml.push_str(
            "<Override PartName=\"/word/footnotes.xml\" \
             ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml\"/>\n",
        );
    }
    xml.push_str("</Types>\n");
    xml
}

/// `_rels/.rels`: the main document and the two property parts.
pub(crate) fn root_rels() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
     <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n\
     <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\n\
     <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>\n\
     <Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties\" Target=\"docProps/app.xml\"/>\n\
     </Relationships>\n"
        .to_owned()
}

/// `word/settings.xml`: default tab stop, compatibility mode, zoom and, when
/// the body has a table-of-contents field, the open-time field update that
/// turns its cached entries into a page-numbered TOC.
pub(crate) fn settings(update_fields: bool) -> String {
    let update = if update_fields { "<w:updateFields w:val=\"true\"/>\n" } else { "" };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\n\
         <w:zoom w:percent=\"100\"/>\n\
         <w:defaultTabStop w:val=\"720\"/>\n\
         <w:characterSpacingControl w:val=\"doNotCompress\"/>\n\
         {update}\
         <w:compat>\n\
         <w:compatSetting w:name=\"compatibilityMode\" w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"15\"/>\n\
         </w:compat>\n\
         </w:settings>\n"
    )
}

/// `docProps/app.xml`: the application name, no links or security flags.
pub(crate) fn app() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
     <Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\" \
     xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\">\n\
     <Application>lmdv</Application>\n\
     <DocSecurity>0</DocSecurity>\n\
     <ScaleCrop>false</ScaleCrop>\n\
     <LinksUpToDate>false</LinksUpToDate>\n\
     <SharedDoc>false</SharedDoc>\n\
     <HyperlinksChanged>false</HyperlinksChanged>\n\
     <AppVersion>1.0</AppVersion>\n\
     </Properties>\n"
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};

    use super::*;

    fn parts() -> Vec<Part> {
        vec![
            Part::text("[Content_Types].xml", content_types(false)),
            Part::text("word/document.xml", "<w:document/>"),
            Part { name: "word/media/image1.png".to_owned(), bytes: vec![0x89, b'P', b'N', b'G'] },
        ]
    }

    #[test]
    fn crc32_matches_the_standard_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn the_archive_is_readable_and_keeps_every_part() {
        let bytes = zip(&parts());
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("a readable ZIP");
        assert_eq!(archive.len(), 3);
        let mut document = String::new();
        archive.by_name("word/document.xml").expect("document part").read_to_string(&mut document).unwrap();
        assert_eq!(document, "<w:document/>");
        let mut media = Vec::new();
        archive.by_name("word/media/image1.png").expect("media part").read_to_end(&mut media).unwrap();
        assert_eq!(media, vec![0x89, b'P', b'N', b'G']);
    }

    #[test]
    fn timestamps_and_order_are_fixed() {
        let first = zip(&parts());
        let second = zip(&parts());
        assert_eq!(first, second);
        // The first local header: signature, version, flags, method, time, date.
        assert_eq!(&first[0..4], &LOCAL_HEADER.to_le_bytes());
        assert_eq!(u16::from_le_bytes([first[10], first[11]]), DOS_TIME);
        assert_eq!(u16::from_le_bytes([first[12], first[13]]), DOS_DATE);
        let names: Vec<String> = {
            let mut archive = zip::ZipArchive::new(Cursor::new(first)).unwrap();
            (0..archive.len()).map(|index| archive.by_index(index).unwrap().name().to_owned()).collect()
        };
        assert_eq!(names[0], "[Content_Types].xml");
        assert_eq!(names[1], "word/document.xml");
    }

    #[test]
    fn content_types_declare_every_embedded_raster() {
        let xml = content_types(false);
        for extension in ["png", "jpeg", "gif", "bmp", "webp"] {
            assert!(xml.contains(&format!("Extension=\"{extension}\"")), "{extension}");
        }
        assert!(xml.contains("PartName=\"/word/document.xml\""));
        assert!(xml.contains("PartName=\"/word/numbering.xml\""));
        assert!(!xml.contains("/word/footnotes.xml"));
        let with_notes = content_types(true);
        assert!(with_notes.contains("PartName=\"/word/footnotes.xml\""));
        assert!(with_notes.contains("wordprocessingml.footnotes+xml"));
    }
}
