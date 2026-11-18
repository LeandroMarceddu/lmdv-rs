//! Markdown folder listings and folder-wide search (F-03–F-06, F-14).
//!
//! The file browser shows Markdown files, never every file. A listing is the
//! folder tree flattened in display order: directories before files, each
//! group sorted case-insensitively, at most [`MAX_TREE_FILES`] files and
//! [`MAX_TREE_DEPTH`] path components. A folder with no Markdown below it is
//! left out, hidden/dot folders and build/system folders are skipped, and only
//! the [`MARKDOWN_EXTENSIONS`] are files. The flattening is what the app keeps:
//! folder open/closed state belongs to the UI, so the same listing compares
//! equal across watcher refreshes (F-08) and JSON round-trips unchanged.
//!
//! Folder search walks the same rules, reads Markdown files on the worker and
//! reports a file-name hit (`line == 0`) plus one hit per matching line
//! (1-based), at most [`MAX_SEARCH_HITS`], with a short centred preview.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The extensions the file browser and folder search treat as Markdown.
pub const MARKDOWN_EXTENSIONS: [&str; 8] = ["md", "markdown", "mdown", "mkd", "mkdn", "mdwn", "markdn", "mdtxt"];

/// Folder names never listed, matched case-insensitively.
pub const SKIP_FOLDERS: [&str; 12] = [
    "node_modules",
    "target",
    "dist",
    "__pycache__",
    "venv",
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "appdata",
    "$recycle.bin",
    "system volume information",
];

/// The deepest listed path, in components below the root (F-03).
pub const MAX_TREE_DEPTH: usize = 4;

/// The most Markdown files one listing holds (F-03).
pub const MAX_TREE_FILES: usize = 2000;

/// The most hits a folder search returns (F-14).
pub const MAX_SEARCH_HITS: usize = 200;

/// The longest folder-search preview, in characters, ellipses included.
pub const SEARCH_PREVIEW_CHARS: usize = 160;

/// How many characters of a matching line show before the match.
pub const SEARCH_PREVIEW_LEAD: usize = 60;

/// One row of a listing: a folder or a Markdown file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeEntry {
    /// The absolute path, also the row's identity.
    pub path: PathBuf,
    /// The file or folder name alone.
    pub name: String,
    /// Its nesting level: 0 for the root's children.
    pub depth: u32,
    /// A folder row; files carry their Markdown name and no children.
    pub dir: bool,
}

/// A folder listing in display order: folders before files, each group sorted
/// case-insensitively by name (F-06).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeListing {
    /// The canonical folder this listing describes.
    pub root: PathBuf,
    /// The folder's own name, or its full path at a filesystem root.
    pub name: String,
    /// Every visible row, depth first, in display order.
    pub rows: Vec<TreeEntry>,
}

impl TreeListing {
    /// The Markdown files in the listing, in display order.
    pub fn files(&self) -> impl Iterator<Item = &TreeEntry> {
        self.rows.iter().filter(|row| !row.dir)
    }

    /// The number of Markdown files.
    pub fn file_count(&self) -> usize {
        self.files().count()
    }

    /// The folder's root row, when the listing has one at the top level.
    pub fn dir(&self, path: &Path) -> Option<&TreeEntry> {
        self.rows.iter().find(|row| row.dir && row.path == path)
    }
}

/// One folder-search match.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// The Markdown file the hit belongs to.
    pub path: PathBuf,
    /// Its file name.
    pub name: String,
    /// 1-based line number; 0 means the file name itself matched.
    pub line: u32,
    /// The trimmed line around the first match; empty for a file-name hit.
    pub preview: String,
}

/// Why a folder could not be listed or searched.
#[derive(Debug, thiserror::Error)]
pub enum TreeError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("not a folder: {0}")]
    NotADirectory(PathBuf),
}

impl TreeError {
    /// The missing-folder case, as a caller that may keep stale rows cares.
    pub fn is_not_found(&self) -> bool {
        match self {
            Self::Io { source, .. } => source.kind() == io::ErrorKind::NotFound,
            Self::NotADirectory(_) => false,
        }
    }
}

/// Whether `name` has one of the [`MARKDOWN_EXTENSIONS`], case-insensitively.
pub fn is_markdown_name(name: &str) -> bool {
    let Some((stem, extension)) = name.rsplit_once('.') else { return false };
    !stem.is_empty() && MARKDOWN_EXTENSIONS.iter().any(|known| extension.eq_ignore_ascii_case(known))
}

/// Whether a folder is hidden, build output or a system folder (F-04).
pub fn is_skipped_folder(name: &str) -> bool {
    name.starts_with('.') || SKIP_FOLDERS.iter().any(|known| name.eq_ignore_ascii_case(known))
}

/// Lists the Markdown folder tree under `root`.
///
/// The root itself may be a hidden or system folder, and may hold no Markdown:
/// an empty folder is a successful empty listing (the pane then says so). A
/// missing root, a path that is not a folder, or an unreadable root is an
/// error. Unreadable descendants are skipped with their folder, as the original
/// does.
pub fn list_tree(root: &Path) -> Result<TreeListing, TreeError> {
    let canonical = fs::canonicalize(root).map_err(|source| TreeError::Io { path: root.to_path_buf(), source })?;
    let meta = fs::metadata(&canonical).map_err(|source| TreeError::Io { path: canonical.clone(), source })?;
    if !meta.is_dir() {
        return Err(TreeError::NotADirectory(canonical));
    }
    let name = folder_name(&canonical);
    let mut rows = Vec::new();
    let mut count = 0usize;
    scan_dir(&canonical, 0, &mut count, &mut rows);
    Ok(TreeListing { root: canonical, name, rows })
}

/// The folder's display name: its last component, or the whole path at `/`.
pub fn folder_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

fn scan_dir(dir: &Path, depth: usize, count: &mut usize, rows: &mut Vec<TreeEntry>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut entries: Vec<(PathBuf, String, bool)> = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let dir = entry_is_dir(&entry, &path);
        entries.push((path, name, dir));
    }
    // Folders first, then files; each group case-insensitive by name. Sorting
    // before the walk makes the 2,000-file cap deterministic.
    entries.sort_by(|a, b| {
        b.2.cmp(&a.2).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())).then_with(|| a.1.cmp(&b.1))
    });

    for (path, name, is_dir) in entries {
        if *count >= MAX_TREE_FILES {
            return;
        }
        if is_dir {
            // The root may be skipped itself; its children are not exempt.
            if depth + 1 >= MAX_TREE_DEPTH || is_skipped_folder(&name) {
                continue;
            }
            let before = rows.len();
            rows.push(TreeEntry { path: path.clone(), name, depth: depth as u32, dir: true });
            let before_children = rows.len();
            scan_dir(&path, depth + 1, count, rows);
            if rows.len() == before_children {
                // No Markdown below it: the folder is not listed (F-03).
                rows.truncate(before);
            }
        } else {
            if !is_markdown_name(&name) {
                continue;
            }
            *count += 1;
            rows.push(TreeEntry { path, name, depth: depth as u32, dir: false });
        }
    }
}

/// Searches every Markdown file under `root` for `query` (F-14).
///
/// The walk uses the listing's depth, skip and extension rules but its own
/// [`MAX_SEARCH_HITS`] cap, and reads file contents on the calling (worker)
/// thread. A file-name match is `line == 0`; every matching line is reported
/// 1-based with a trimmed preview centred on the first match. An unreadable
/// file is skipped. An empty query returns no hits.
pub fn search_tree(root: &Path, query: &str) -> Result<Vec<SearchHit>, TreeError> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let canonical = fs::canonicalize(root).map_err(|source| TreeError::Io { path: root.to_path_buf(), source })?;
    let meta = fs::metadata(&canonical).map_err(|source| TreeError::Io { path: canonical.clone(), source })?;
    if !meta.is_dir() {
        return Err(TreeError::NotADirectory(canonical));
    }
    let mut hits = Vec::new();
    search_dir(&canonical, 0, &needle, &mut hits);
    Ok(hits)
}

fn search_dir(dir: &Path, depth: usize, needle: &str, hits: &mut Vec<SearchHit>) {
    if hits.len() >= MAX_SEARCH_HITS {
        return;
    }
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut entries: Vec<(PathBuf, String, bool)> = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let is_dir = entry_is_dir(&entry, &path);
        entries.push((path, name, is_dir));
    }
    // Search results group by file, so the walk follows name order rather
    // than the listing's folders-first order.
    entries.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()).then_with(|| a.1.cmp(&b.1)));

    for (path, name, is_dir) in entries {
        if hits.len() >= MAX_SEARCH_HITS {
            return;
        }
        if is_dir {
            if depth + 1 >= MAX_TREE_DEPTH || is_skipped_folder(&name) {
                continue;
            }
            search_dir(&path, depth + 1, needle, hits);
        } else {
            if !is_markdown_name(&name) {
                continue;
            }
            if name.to_lowercase().contains(needle) {
                hits.push(SearchHit { path: path.clone(), name: name.clone(), line: 0, preview: String::new() });
                if hits.len() >= MAX_SEARCH_HITS {
                    return;
                }
            }
            let Ok(content) = fs::read_to_string(&path) else { continue };
            for (index, line) in content.lines().enumerate() {
                if hits.len() >= MAX_SEARCH_HITS {
                    return;
                }
                if line.to_lowercase().contains(needle) {
                    hits.push(SearchHit {
                        path: path.clone(),
                        name: name.clone(),
                        line: (index + 1) as u32,
                        preview: preview(line, needle),
                    });
                }
            }
        }
    }
}

/// Whether a directory entry is a folder, following a symlink as the original
/// does (the recursive walk's depth cap bounds a link cycle).
fn entry_is_dir(entry: &fs::DirEntry, path: &Path) -> bool {
    match entry.file_type() {
        Ok(kind) if kind.is_symlink() => path.is_dir(),
        Ok(kind) => kind.is_dir(),
        Err(_) => path.is_dir(),
    }
}

/// A line trimmed and windowed on its first case-insensitive match, with
/// ellipses where text was cut. Never longer than [`SEARCH_PREVIEW_CHARS`].
pub fn preview(line: &str, needle_lower: &str) -> String {
    let trimmed = line.trim();
    let lower = trimmed.to_lowercase();
    let match_byte = lower.find(needle_lower).unwrap_or(0);
    let match_char = lower[..match_byte].chars().count();
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= SEARCH_PREVIEW_CHARS {
        return trimmed.to_owned();
    }

    // Reserve room for the ellipses first, then window the text around the
    // match (about 60 characters before it) without passing the end.
    let lead_at = match_char.saturating_sub(SEARCH_PREVIEW_LEAD);
    let mut budget = SEARCH_PREVIEW_CHARS.saturating_sub(2);
    let mut start = lead_at.min(chars.len().saturating_sub(budget));
    let mut end = (start + budget).min(chars.len());
    let lead = start > 0;
    let trail = end < chars.len();
    budget = SEARCH_PREVIEW_CHARS.saturating_sub(usize::from(lead) + usize::from(trail));
    start = lead_at.min(chars.len().saturating_sub(budget));
    end = (start + budget).min(chars.len());

    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;

    fn write(root: &Path, relative: &str, body: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    fn names(listing: &TreeListing) -> Vec<(&str, u32, bool)> {
        listing.rows.iter().map(|row| (row.name.as_str(), row.depth, row.dir)).collect()
    }

    #[test]
    fn a_listing_keeps_markdown_only_and_hides_empty_or_skipped_folders() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(root, "afile.md", "a");
        write(root, "README.mkd", "b");
        write(root, "image.png", "png");
        write(root, "notes.txt", "text");
        write(root, ".hidden/secret.md", "secret");
        write(root, ".hidden.md", "a dot file with a Markdown extension is a file");
        write(root, "node_modules/package.md", "no");
        write(root, "Target/build.md", "no");
        write(root, "dist/out.md", "no");
        write(root, "only-text/readme.txt", "no");
        write(root, "nested/deeper.md", "deep");
        write(root, "nested/also.markdown", "also");
        write(root, "l1/l2/l3/deep.md", "four components");
        write(root, "l1/l2/l3/l4/too-deep.md", "five components");
        write(root, "Zdir/z.md", "z");

        let listing = list_tree(root).unwrap();
        assert_eq!(listing.name, root.file_name().unwrap().to_string_lossy());
        let names = names(&listing);
        assert!(names.contains(&("Zdir", 0, true)), "folders first: {names:?}");
        assert!(names.contains(&("nested", 0, true)));
        assert_eq!(names[0], ("l1", 0, true), "directories before files");
        assert_eq!(names[1], ("l2", 1, true), "a folder's children follow its row");
        assert!(names.contains(&("deeper.md", 1, false)), "a folder's file keeps the folder's level");
        assert!(names.contains(&("deep.md", 3, false)), "four components are listed");
        assert!(!names.iter().any(|(name, _, _)| *name == "too-deep.md"), "five components are not");
        assert!(!names.iter().any(|(name, _, _)| *name == "l4"), "a folder with no listed files is hidden");
        assert!(!names.iter().any(|(name, _, _)| *name == ".hidden"));
        assert!(!names.iter().any(|(name, _, _)| *name == "node_modules"));
        assert!(!names.iter().any(|(name, _, _)| *name == "Target"));
        assert!(!names.iter().any(|(name, _, _)| *name == "dist"));
        assert!(!names.iter().any(|(name, _, _)| *name == "only-text"), "a folder without Markdown is hidden");
        assert!(!names.iter().any(|(name, _, _)| *name == "image.png"));
        assert!(!names.iter().any(|(name, _, _)| *name == "notes.txt"));
        assert!(names.iter().any(|(name, _, _)| *name == "README.mkd"), "extensions are case-insensitive");
        assert!(names.iter().any(|(name, _, _)| *name == ".hidden.md"), "dot files are still Markdown files");
        assert_eq!(listing.file_count(), 7);

        // Directories sort before files, each group by lowercase name.
        let top: Vec<&str> = listing.rows.iter().filter(|row| row.depth == 0).map(|row| row.name.as_str()).collect();
        assert_eq!(top, ["l1", "nested", "Zdir", ".hidden.md", "afile.md", "README.mkd"]);
    }

    #[test]
    fn the_listing_caps_markdown_files_at_two_thousand() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        for index in 0..(MAX_TREE_FILES + 1) {
            write(root, &format!("file-{index:04}.md"), "x");
        }
        let listing = list_tree(root).unwrap();
        assert_eq!(listing.file_count(), MAX_TREE_FILES);
    }

    #[test]
    fn an_empty_or_missing_folder_is_reported_honestly() {
        let temp = TempDir::new().unwrap();
        let listing = list_tree(temp.path()).unwrap();
        assert!(listing.rows.is_empty(), "an empty folder lists successfully and empty");

        let missing = temp.path().join("nope");
        assert!(matches!(list_tree(&missing), Err(TreeError::Io { .. })));

        let file = temp.path().join("doc.md");
        fs::write(&file, "x").unwrap();
        assert!(matches!(list_tree(&file), Err(TreeError::NotADirectory(_))));
    }

    #[test]
    fn search_reports_names_lines_and_snippets() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(root, "PIM.md", "no matching body\n");
        write(root, "notes.md", "first line\nA pim reference here\nLast line\n");
        write(root, "image.png", "pim");
        write(root, ".hidden/pim.md", "pim");
        write(root, "node_modules/pim.md", "pim");
        write(root, "l1/l2/l3/l4/too-deep.md", "pim");

        let hits = search_tree(root, "pim").unwrap();
        assert_eq!(hits.len(), 2, "the line hit and the file-name hit: {hits:?}");
        assert_eq!(hits[0].name, "notes.md");
        assert_eq!(hits[0].line, 2);
        assert_eq!(hits[0].preview, "A pim reference here");
        assert_eq!(hits[1].line, 0, "the file-name hit");
        assert_eq!(hits[1].name, "PIM.md");
        assert!(hits[1].preview.is_empty());
        assert!(!hits.iter().any(|hit| hit.path.ends_with("image.png")));
        assert!(!hits.iter().any(|hit| hit.path.to_string_lossy().contains("too-deep")));
        assert!(search_tree(root, "   ").unwrap().is_empty());
    }

    #[test]
    fn search_stops_at_two_hundred_hits() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let body: String = (1..=300).map(|line| format!("match {line}\n")).collect();
        write(root, "many.md", &body);
        let hits = search_tree(root, "match").unwrap();
        assert_eq!(hits.len(), MAX_SEARCH_HITS);
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[MAX_SEARCH_HITS - 1].line, MAX_SEARCH_HITS as u32);
    }

    #[test]
    fn a_long_preview_is_centred_on_the_match_and_bounded() {
        let line = format!("{}NEEDLE{}", "a".repeat(300), "b".repeat(300));
        let text = preview(&line, "needle");
        assert!(text.starts_with('…'));
        assert!(text.ends_with('…'));
        assert!(text.contains("NEEDLE"));
        assert!(text.chars().count() <= SEARCH_PREVIEW_CHARS, "{} chars", text.chars().count());

        let short = preview("  PIM here  ", "pim");
        assert_eq!(short, "PIM here");
    }

    #[test]
    fn extension_and_skip_rules_are_explicit() {
        for name in ["a.md", "A.MARKDOWN", "x.mdown", "x.mkd", "x.mkdn", "x.mdwn", "x.markdn", "x.mdtxt"] {
            assert!(is_markdown_name(name), "{name}");
        }
        for name in ["a.mdx", "a.txt", "md", "a.md.bak", ".md"] {
            assert!(!is_markdown_name(name), "{name}");
        }
        for name in [".git", "node_modules", "NODE_MODULES", "target", "Program Files", "$RECYCLE.BIN"] {
            assert!(is_skipped_folder(name), "{name}");
        }
        assert!(!is_skipped_folder("notes"));
    }
    #[cfg(unix)]
    #[test]
    fn a_symlinked_folder_is_listed_and_a_cycle_stops_at_the_depth_cap() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(root, "real/note.md", "note");
        symlink(root.join("real"), root.join("linked")).unwrap();
        let listing = list_tree(root).unwrap();
        let names = names(&listing);
        assert!(names.contains(&("linked", 0, true)), "a symlinked folder is a folder: {names:?}");
        assert!(names.contains(&("note.md", 1, false)));

        // A folder linking to its parent would recurse forever; the four-level cap
        // ends it and the cycle's own rows stay within it.
        write(root, "loop/inside.md", "x");
        symlink(root, root.join("loop/up")).unwrap();
        let listing = list_tree(root).unwrap();
        assert!(listing.rows.iter().all(|row| row.depth <= 3));
    }
}
