//! Link and media URL policy for exports.
//!
//! The export is a static file: a relative Markdown link has to become a
//! `file:///` URL or it would resolve against the exported file's folder
//! instead of the document's. The rules mirror the app's link policy: local
//! paths become file URLs, `http`/`https`/`mailto` links stay, and script or
//! data URLs are refused. Nothing here reads the filesystem; a path is only
//! joined and normalized lexically.

use std::path::{Component, Path, PathBuf};

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

/// Characters percent-encoded in a file URL's path. `/` and `:` stay literal.
const PATH_ENCODE: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'%')
    .add(b'\\')
    .add(b'^')
    .add(b'|')
    .add(b'[')
    .add(b']');

/// Characters percent-encoded in a fragment.
const FRAGMENT_ENCODE: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'<').add(b'>').add(b'`');

/// Resolves `href` for a static export.
///
/// `None` means the destination is empty or unsafe and the caller must emit the
/// text without a link. Relative paths are only turned into file URLs when the
/// document's parent folder is absolute; otherwise they stay relative (the
/// exported file and the document then normally share a folder, which is the
/// best a lexical resolver can do).
pub(crate) fn resolve(href: &str, source_path: Option<&Path>) -> Option<String> {
    let href = href.trim();
    if href.is_empty() || href.chars().any(|c| c.is_control()) {
        return None;
    }
    if href.starts_with('#') {
        return Some(href.to_owned());
    }
    match scheme(href).map(str::to_ascii_lowercase).as_deref() {
        Some("http" | "https" | "mailto") => Some(href.to_owned()),
        Some(_) => None,
        None => local(href, source_path),
    }
}

/// The URL scheme of `href`, when it has one. Shared with the asset policy.
pub(crate) fn scheme(href: &str) -> Option<&str> {
    let colon = href.find(':')?;
    if colon == 0 {
        return None;
    }
    let candidate = &href[..colon];
    let mut chars = candidate.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    // Windows drive letters (`C:\…`) are paths, not schemes, on Linux too.
    if candidate.len() == 1 {
        return None;
    }
    Some(candidate)
}

/// A local path, made absolute against the document folder when possible.
fn local(href: &str, source_path: Option<&Path>) -> Option<String> {
    let (path, fragment) = split_fragment(href);
    if path.is_empty() {
        return Some(href.to_owned());
    }
    let decoded = percent_encoding::percent_decode_str(path).decode_utf8_lossy();
    let joined = if Path::new(decoded.as_ref()).is_absolute() {
        PathBuf::from(decoded.as_ref())
    } else {
        match source_path.and_then(Path::parent) {
            Some(parent) if parent.is_absolute() => parent.join(decoded.as_ref()),
            _ => return Some(href.to_owned()),
        }
    };
    let mut out = file_url(&normalize(&joined));
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(&utf8_percent_encode(fragment, FRAGMENT_ENCODE).to_string());
    }
    Some(out)
}

/// Splits `href` at its first `#`, keeping the fragment without the hash.
fn split_fragment(href: &str) -> (&str, Option<&str>) {
    match href.find('#') {
        Some(index) => (&href[..index], Some(&href[index + 1..])),
        None => (href, None),
    }
}

/// Removes `.` and `..` components without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // At the root, `..` is the root; `pop` already leaves it there.
                let _ = out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The `file:///` URL for an absolute path.
fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = if text.starts_with('/') { text.into_owned() } else { format!("/{text}") };
    format!("file://{}", utf8_percent_encode(&text, PATH_ENCODE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_links_become_file_urls() {
        let source = Path::new("/home/jane/docs/notes.md");
        assert_eq!(
            resolve("./nested/deeper.md", Some(source)).as_deref(),
            Some("file:///home/jane/docs/nested/deeper.md")
        );
        assert_eq!(
            resolve("../shared/a b.md#top", Some(source)).as_deref(),
            Some("file:///home/jane/shared/a%20b.md#top")
        );
        assert_eq!(resolve("/etc/hosts", Some(source)).as_deref(), Some("file:///etc/hosts"));
    }

    #[test]
    fn fragments_and_external_links_stay() {
        assert_eq!(resolve("#section", None).as_deref(), Some("#section"));
        assert_eq!(resolve("https://example.com/a", None).as_deref(), Some("https://example.com/a"));
        assert_eq!(resolve("mailto:a@example.com", None).as_deref(), Some("mailto:a@example.com"));
    }

    #[test]
    fn script_and_unknown_schemes_are_refused() {
        assert_eq!(resolve("javascript:alert(1)", None), None);
        assert_eq!(resolve("vbscript:msgbox(1)", None), None);
        assert_eq!(resolve("data:text/html,<script>", None), None);
        assert_eq!(resolve("file:///etc/passwd", None), None);
        assert_eq!(resolve("obsidian://open?vault=x", None), None);
        assert_eq!(resolve("", None), None);
    }

    #[test]
    fn relative_links_stay_when_the_folder_is_unknown() {
        assert_eq!(resolve("./a.md", None).as_deref(), Some("./a.md"));
        assert_eq!(resolve("notes.md", Some(Path::new("notes.md"))).as_deref(), Some("notes.md"));
    }
}
