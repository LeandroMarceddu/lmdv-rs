//! Images pasted or dropped into a document's own `assets/` folder (E-36).
//!
//! The original saves a pasted picture as `assets/pasted-<ms>.<ext>` and copies
//! a dropped one as `assets/<stem>.<ext>`, adding `-1`, `-2` … when the name is
//! taken. The app only needs the folder-relative Markdown destination; the
//! encoding lives in [`crate::assets::markdown_destination`] so the view never
//! builds a path by hand.
//!
//! Reading and writing stay here: `lmdv-app` has no `std::fs`.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The folder, beside the document, where image drops and pastes land.
pub const ASSETS_DIR: &str = "assets";

/// The largest picture accepted from a paste or a drop: 32 MiB.
pub const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

/// The image extensions a paste or a drop keeps, lower-case, without the dot.
///
/// These are the formats the reader decodes; anything else is refused rather
/// than copied next to the document, where it would only draw the missing-image
/// placeholder.
pub const IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"];

/// Why an image could not be saved into `assets/`.
#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    /// The source file has no image extension this app can draw.
    #[error("not an image this app can draw: {name}")]
    NotImage {
        /// The file name, for the notice.
        name: String,
    },
    /// The source file is larger than [`MAX_IMAGE_BYTES`].
    #[error("the image is too large ({bytes} bytes)")]
    TooLarge {
        /// The source's size in bytes.
        bytes: u64,
    },
    /// The `assets` folder or the file could not be read or written.
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// A picture saved into the document's `assets/` folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedAsset {
    /// The file's absolute path.
    pub path: PathBuf,
    /// The path relative to the document folder, with `/` separators, ready for
    /// a Markdown destination (`assets/pasted-1700000000000.png`).
    pub relative: String,
}

/// Saves pasted PNG bytes as `assets/pasted-<ms>.png` (E-36).
///
/// `now_ms` is the paste's millisecond timestamp; when that name already exists
/// (two pastes in one millisecond) the next free millisecond is used so a
/// picture is never overwritten. The write is atomic (temp + rename).
pub fn save_pasted_image(doc_dir: &Path, png: &[u8], now_ms: u64) -> Result<SavedAsset, AssetError> {
    if png.len() as u64 > MAX_IMAGE_BYTES {
        return Err(AssetError::TooLarge { bytes: png.len() as u64 });
    }
    let folder = doc_dir.join(ASSETS_DIR);
    fs::create_dir_all(&folder)?;
    let mut stamp = now_ms;
    let (path, name) = loop {
        let name = format!("pasted-{stamp}.png");
        let path = folder.join(&name);
        match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                use std::io::Write;
                if let Err(error) = file.write_all(png).and_then(|()| file.sync_all()) {
                    let _ = fs::remove_file(&path);
                    return Err(error.into());
                }
                break (path, name);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                stamp = stamp.saturating_add(1);
            }
            Err(error) => return Err(error.into()),
        }
    };
    Ok(SavedAsset { path, relative: format!("{ASSETS_DIR}/{name}") })
}

/// Copies a dropped image into `assets/<stem>.<ext>`, uniquified with `-1`,
/// `-2` … (E-36).
///
/// The extension is lower-cased; [`IMAGE_EXTENSIONS`] is the whole allow-list,
/// so dropping a `.exe` or a `.md` copies nothing.
pub fn save_dropped_image(doc_dir: &Path, source: &Path) -> Result<SavedAsset, AssetError> {
    let name = source.file_name().and_then(OsStr::to_str).unwrap_or("image");
    let Some(extension) = image_extension(source) else {
        return Err(AssetError::NotImage { name: name.to_owned() });
    };
    let bytes = fs::read(source)?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(AssetError::TooLarge { bytes: bytes.len() as u64 });
    }
    let folder = doc_dir.join(ASSETS_DIR);
    fs::create_dir_all(&folder)?;
    let stem = source.file_stem().and_then(OsStr::to_str).unwrap_or("image");
    let mut candidate = format!("{stem}.{extension}");
    let mut suffix = 0u32;
    loop {
        let path = folder.join(&candidate);
        match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                use std::io::Write;
                if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
                    let _ = fs::remove_file(&path);
                    return Err(error.into());
                }
                return Ok(SavedAsset { path, relative: format!("{ASSETS_DIR}/{candidate}") });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                suffix = suffix.saturating_add(1);
                candidate = format!("{stem}-{suffix}.{extension}");
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// An image path's lower-cased extension, when [`IMAGE_EXTENSIONS`] accepts it.
pub fn image_extension(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    IMAGE_EXTENSIONS.contains(&extension.as_str()).then_some(extension)
}

/// Whether a path is an image a drop may copy into `assets/`.
pub fn is_supported_image(path: &Path) -> bool {
    image_extension(path).is_some()
}

/// Percent-encodes a relative destination for a Markdown image link (E-36).
///
/// `/`, `.`, `-`, `_` and `~` stay literal; a space, `%`, `#`, `?`, `(`, `)`
/// and every non-ASCII byte take `%XX`, so `assets/my picture.png` becomes
/// `assets/my%20picture.png`.
pub fn markdown_destination(relative: &str) -> String {
    const LITERAL: &[u8] = b"-._~/";
    let mut out = String::with_capacity(relative.len() + 8);
    for &byte in relative.as_bytes() {
        if byte.is_ascii_alphanumeric() || LITERAL.contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0').to_ascii_uppercase());
            out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0').to_ascii_uppercase());
        }
    }
    out
}

/// The Markdown a saved picture inserts at the caret: `![](assets/…)`.
pub fn image_markdown(saved: &SavedAsset) -> String {
    format!("![]({})", markdown_destination(&saved.relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> tempfile::TempDir {
        tempfile::TempDir::new().expect("temp dir")
    }

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 0, 0, 0];

    #[test]
    fn a_paste_lands_in_assets_with_its_millisecond_name() {
        let temp = temp();
        let saved = save_pasted_image(temp.path(), PNG, 1_700_000_000_123).expect("the paste saves");
        assert_eq!(saved.relative, "assets/pasted-1700000000123.png");
        assert_eq!(saved.path, temp.path().join("assets/pasted-1700000000123.png"));
        assert_eq!(fs::read(&saved.path).unwrap(), PNG);
        assert_eq!(image_markdown(&saved), "![](assets/pasted-1700000000123.png)");

        // A second paste in the same millisecond takes the next free stamp.
        let second = save_pasted_image(temp.path(), PNG, 1_700_000_000_123).expect("a second paste");
        assert_eq!(second.relative, "assets/pasted-1700000000124.png");
    }

    #[test]
    fn a_drop_keeps_its_stem_and_uniquifies_1_2() {
        let temp = temp();
        let source = temp.path().join("My Picture.PNG");
        fs::write(&source, PNG).unwrap();
        let first = save_dropped_image(temp.path(), &source).expect("the drop");
        assert_eq!(first.relative, "assets/My Picture.png", "the stem survives, the extension is lower-case");
        let second = save_dropped_image(temp.path(), &source).expect("the second drop");
        assert_eq!(second.relative, "assets/My Picture-1.png");
        let third = save_dropped_image(temp.path(), &source).expect("the third drop");
        assert_eq!(third.relative, "assets/My Picture-2.png");
        assert_eq!(fs::read(&third.path).unwrap(), PNG);

        let markdown = image_markdown(&first);
        assert_eq!(markdown, "![](assets/My%20Picture.png)", "the destination is percent-encoded");
    }

    #[test]
    fn a_non_image_drop_is_refused_and_writes_nothing() {
        let temp = temp();
        let source = temp.path().join("notes.md");
        fs::write(&source, b"# not an image").unwrap();
        let error = save_dropped_image(temp.path(), &source).expect_err("a Markdown file is not an image");
        assert!(matches!(error, AssetError::NotImage { .. }), "{error:?}");
        assert!(!temp.path().join("assets").exists(), "no folder is made for a refusal");

        let huge = temp.path().join("huge.png");
        fs::write(&huge, vec![0u8; MAX_IMAGE_BYTES as usize + 1]).unwrap();
        let error = save_dropped_image(temp.path(), &huge).expect_err("too large");
        assert!(matches!(error, AssetError::TooLarge { .. }), "{error:?}");
        assert!(!temp.path().join("assets/huge.png").exists());
    }

    #[test]
    fn only_the_reader_s_own_image_types_pass() {
        for name in ["a.png", "a.JPG", "a.jpeg", "a.gif", "a.webp", "a.svg", "a.bmp"] {
            assert!(is_supported_image(Path::new(name)), "{name}");
        }
        for name in ["a.avif", "a.md", "a", "a.png.exe"] {
            assert!(!is_supported_image(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn the_destination_keeps_path_separators_and_escapes_the_rest() {
        assert_eq!(markdown_destination("assets/a-b_c.png"), "assets/a-b_c.png");
        assert_eq!(markdown_destination("assets/a b(c)#d?.png"), "assets/a%20b%28c%29%23d%3F.png");
        assert_eq!(markdown_destination("assets/à.png"), "assets/%C3%A0.png");
    }
}
