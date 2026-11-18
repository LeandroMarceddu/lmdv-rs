//! Command handlers that need more than one state edit: pasting the clipboard
//! as a new document (C-11), the copy-path/reveal pair (T-18), the always-on-top
//! toggle, tab stepping, the closed-tab stack (T-07) and the sequential
//! close-others/right queue (T-18/T-23).
//!
//! Nothing here touches the filesystem: the desktop reveal is a worker job, and
//! a reopened file goes through the normal load job.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use lmdv_core::{DocStatus, Document, DocumentKey, PasteId, ScrollAnchor, SourceBuffer, TabId};
use rune::Tx;

use crate::window::{Tab, WindowState, title_for_path};

/// How many closed tabs a window remembers, newest last (the original's
/// `MAX_CLOSED`).
pub const MAX_CLOSED_TABS: usize = 25;

/// A tab closed in this window: what it showed, where it sat and the place it
/// was being read (T-07). Start pages are never remembered.
#[derive(Clone, Debug, PartialEq)]
pub enum ClosedTab {
    /// A file's tab, restored from its path.
    File {
        /// The document's path.
        path: PathBuf,
        /// The strip index it was at when it closed.
        index: usize,
        /// The reading position it had.
        anchor: ScrollAnchor,
    },
    /// Pasted text with no file: its paste id keeps drafts and history keys,
    /// and the text itself is kept because the buffer may be gone.
    Pasted { id: u64, title: String, text: String, index: usize, anchor: ScrollAnchor },
}

impl ClosedTab {
    /// The strip index the tab closed at.
    pub fn index(&self) -> usize {
        match self {
            Self::File { index, .. } | Self::Pasted { index, .. } => *index,
        }
    }

    /// The file path, for a file's tab.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::File { path, .. } => Some(path),
            Self::Pasted { .. } => None,
        }
    }
}

/// One window's closed-tab stack and the close-others/right queue.
#[derive(Debug, Default)]
pub struct TabActionsState {
    closed: Vec<ClosedTab>,
    batch: VecDeque<TabId>,
    batch_current: Option<TabId>,
}

impl TabActionsState {
    /// The closed tabs, oldest first.
    pub fn closed(&self) -> &[ClosedTab] {
        &self.closed
    }

    /// Whether a close-others/right sequence is running.
    pub fn closing_batch(&self) -> bool {
        !self.batch.is_empty() || self.batch_current.is_some()
    }

    /// Forgets every closed tab and stops a close sequence (a scene reset).
    pub fn clear(&mut self) {
        self.closed.clear();
        self.batch.clear();
        self.batch_current = None;
    }

    fn remember(&mut self, tab: ClosedTab) {
        if self.closed.len() >= MAX_CLOSED_TABS {
            self.closed.remove(0);
        }
        self.closed.push(tab);
    }
}

impl WindowState {
    // ---- clipboard paste --------------------------------------------------

    /// `Ctrl+V` in the viewer and `file.paste`: reads the clipboard and opens
    /// its text as a new untitled document (C-11).
    pub(crate) fn paste_as_document(&mut self, tx: &mut Tx) {
        tx.read_clipboard(|state: &mut WindowState, content, _| {
            state.open_clipboard_text(content.text(), content.image().is_some());
        });
    }

    /// Opens clipboard text as a pasted document, or says why nothing opened.
    pub(crate) fn open_clipboard_text(&mut self, raw: &str, has_image: bool) {
        let text = pasted_text(raw);
        if text.trim().is_empty() {
            let message = if has_image { lmdv_i18n::t!("msg.pasteImage") } else { lmdv_i18n::t!("msg.pasteEmpty") };
            self.notice(message);
            return;
        }
        let title = {
            let title = pasted_title(&text);
            if title.is_empty() { lmdv_i18n::t!("paste.untitled").to_owned() } else { title }
        };
        self.open_synthetic(&title, &text);
        self.notice(lmdv_i18n::t!("msg.pastedOpened", "name" => title));
    }

    // ---- path actions -----------------------------------------------------

    /// The path `file.copy-path` and `file.reveal` act on: the document in
    /// front, else the focused file-browser row.
    pub fn copy_path_target(&self) -> Option<PathBuf> {
        self.active_path().or_else(|| self.browser.focus.as_deref().map(PathBuf::from))
    }

    /// Copies the path the active document (or focused browser row) shows.
    pub(crate) fn copy_path_to_clipboard(&mut self, tx: &mut Tx) -> bool {
        let Some(path) = self.copy_path_target() else { return false };
        tx.copy(path.to_string_lossy().into_owned());
        // A path action's confirmation (OI-160).
        self.notice(lmdv_i18n::t!("msg.pathCopied"));
        true
    }

    /// Opens the folder containing the active document (or focused row) with
    /// the desktop's file manager, through the same worker job F-12 uses.
    pub(crate) fn reveal_path(&mut self) -> bool {
        let Some(path) = self.copy_path_target() else { return false };
        self.reveal_in_folder(&path)
    }

    /// Opens `path`'s containing folder on the desktop, off the UI thread.
    pub(crate) fn reveal_in_folder(&mut self, path: &Path) -> bool {
        let Some(parent) = path.parent().map(Path::to_path_buf) else { return false };
        self.shared.open_external(self.worker().clone(), parent);
        true
    }

    /// Copies one tab's file path (T-18's Copy path).
    pub(crate) fn copy_tab_path(&mut self, tab: TabId, tx: &mut Tx) -> bool {
        let Some(path) = self.tab_path(tab) else { return false };
        tx.copy(path.to_string_lossy().into_owned());
        self.notice(lmdv_i18n::t!("msg.pathCopied"));
        true
    }

    /// Reveals one tab's file in its folder (T-18's Show in folder).
    pub(crate) fn reveal_tab(&mut self, tab: TabId) -> bool {
        let Some(path) = self.tab_path(tab) else { return false };
        self.reveal_in_folder(&path)
    }

    fn tab_path(&self, tab: TabId) -> Option<PathBuf> {
        self.tab(tab).and_then(Tab::document).and_then(|doc| doc.key.borrow().path().map(Path::to_path_buf))
    }

    // ---- always on top ----------------------------------------------------

    /// `view.pin`: toggles always-on-top for this window.
    ///
    /// Wayland has no protocol for it, so the toggle refuses with a notice
    /// instead of showing a check that does nothing. The request goes through
    /// [`AppShared::set_always_on_top`], whose test seam records it.
    pub(crate) fn toggle_pin(&mut self) {
        if !self.shared.pin_available() {
            self.notice(lmdv_i18n::t!("msg.pinWayland"));
            return;
        }
        let on = !self.pinned;
        if self.shared.set_always_on_top(self.id(), on) {
            self.pinned = on;
        } else {
            self.notice(lmdv_i18n::t!("msg.windowNotOnScreen"));
        }
    }

    // ---- tab stepping -----------------------------------------------------

    /// `tab.next`/`tab.previous`: moves the front one tab, wrapping (C-11).
    pub fn step_tab(&mut self, direction: i32) -> bool {
        let len = self.tabs.len();
        if len < 2 {
            return false;
        }
        let step = if direction < 0 { len - 1 } else { 1 };
        let next = (self.active + step) % len;
        self.switch_to_index(next)
    }

    // ---- closed tabs ------------------------------------------------------

    /// Whether Ctrl+Shift+T has anything to reopen (T-07).
    pub fn can_reopen_tab(&self) -> bool {
        !self.tab_actions.closed.is_empty()
    }

    /// The closed tabs this window remembers, oldest first.
    pub fn closed_tabs(&self) -> &[ClosedTab] {
        self.tab_actions.closed()
    }

    /// Records a closing tab before it leaves the strip (T-07).
    pub(crate) fn remember_closed_tab(&mut self, record: ClosedTab) {
        self.tab_actions.remember(record);
    }

    /// Ctrl+Shift+T: reopens the last closed tab at its index and place.
    ///
    /// A file whose document is still alive reuses it (no load); pasted text
    /// reuses the paste id, so its drafts and history keys stay the same.
    pub fn reopen_tab(&mut self) -> bool {
        let Some(record) = self.tab_actions.closed.pop() else { return false };
        match record {
            ClosedTab::File { path, index, anchor } => self.reopen_file(path, index, anchor),
            ClosedTab::Pasted { id, title, text, index, anchor } => self.reopen_pasted(id, title, text, index, anchor),
        }
    }

    fn reopen_file(&mut self, path: PathBuf, index: usize, anchor: ScrollAnchor) -> bool {
        let path = lmdv_io::absolute_lexical(&path);
        let doc = {
            let mut docs = self.shared.docs_mut();
            docs.insert_loading(DocumentKey::File(path.clone()), title_for_path(&path))
        };
        if let Some(existing) = self.index_of_document(doc.id) {
            self.switch_to_index(existing);
            return true;
        }
        let needs_load = !matches!(*doc.status.borrow(), DocStatus::Ready);
        self.attach_document_at(doc.clone(), Some(index), Some(anchor));
        if needs_load {
            self.start_load(doc.id, path);
        }
        true
    }

    fn reopen_pasted(&mut self, id: u64, title: String, text: String, index: usize, anchor: ScrollAnchor) -> bool {
        if let Some(existing) = self.tabs.iter().position(|tab| tab_paste_id(tab) == Some(id)) {
            self.switch_to_index(existing);
            return true;
        }
        let paste = PasteId(id);
        self.shared.reserve_paste(paste);
        let doc_id = self.shared.docs_mut().allocate_id();
        let doc = Rc::new(Document::ready(doc_id, DocumentKey::Untitled(paste), title, SourceBuffer::new(&text)));
        self.shared.docs_mut().insert_ready(doc.clone());
        let revision = doc.revision();
        let model = std::sync::Arc::new(lmdv_md::parse_with_revision(&text, revision));
        self.shared.store_parsed(doc_id, revision, Ok(model));
        let tab = self.attach_document_at(doc, Some(index), Some(anchor));
        if let Some(tab) = self.tabs.iter_mut().find(|candidate| candidate.id == tab) {
            tab.view.pasted_base = Some(text.clone());
        }
        true
    }

    // ---- close others / right (T-23) --------------------------------------

    /// T-18/T-23: activates `keep`, then closes every other tab in strip order.
    /// A dirty tab asks the ordinary close question; Cancel stops the run.
    pub fn close_other_tabs(&mut self, keep: TabId) -> bool {
        if self.tabs.len() < 2 || self.tab(keep).is_none() {
            return false;
        }
        if let Some(index) = self.tabs.iter().position(|tab| tab.id == keep) {
            self.switch_to_index(index);
        }
        let queue: VecDeque<TabId> = self.tabs.iter().map(|tab| tab.id).filter(|id| *id != keep).collect();
        self.start_close_batch(queue);
        true
    }

    /// T-23: closes the tabs after `of`, right to left.
    pub fn close_tabs_to_the_right(&mut self, of: TabId) -> bool {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == of) else { return false };
        let queue: VecDeque<TabId> = self.tabs[index + 1..].iter().rev().map(|tab| tab.id).collect();
        if queue.is_empty() {
            return false;
        }
        self.start_close_batch(queue);
        true
    }

    fn start_close_batch(&mut self, queue: VecDeque<TabId>) {
        self.tab_actions.batch = queue;
        self.tab_actions.batch_current = None;
        self.continue_close_batch();
    }

    /// Advances the close-others/right queue: one tab at a time, waiting for a
    /// dirty tab's question and its save. Cancel or a failed save stops it, so
    /// nothing is ever discarded without the ordinary prompt.
    pub(crate) fn continue_close_batch(&mut self) -> bool {
        if !self.tab_actions.closing_batch() {
            return false;
        }
        let mut changed = false;
        loop {
            if self.editor.prompt.is_some() || self.editor.quit.saving || self.close_batch_saving() {
                return changed;
            }
            if let Some(current) = self.tab_actions.batch_current.take()
                && self.tab(current).is_some()
            {
                // The tab stayed: the question was cancelled or its save failed.
                self.tab_actions.batch.clear();
                return changed;
            }
            let Some(next) = self.tab_actions.batch.pop_front() else { return changed };
            if self.tab(next).is_none() {
                continue;
            }
            // T-23 with the original: a dirty tab is brought forward before its
            // question, so what is being asked about is on screen. Clean tabs
            // close without a visible switch.
            let dirty = self.tab(next).and_then(Tab::document).is_some_and(|doc| doc.is_dirty());
            if dirty && let Some(index) = self.tabs.iter().position(|tab| tab.id == next) {
                self.switch_to_index(index);
            }
            self.tab_actions.batch_current = Some(next);
            self.request_close_tab(next);
            changed = true;
            if self.editor.prompt.is_some() {
                return changed;
            }
            if self.tab(next).is_some() {
                // The close refused (a running save or quit): stop the run.
                self.tab_actions.batch.clear();
                self.tab_actions.batch_current = None;
                return changed;
            }
            self.tab_actions.batch_current = None;
        }
    }

    /// Drops a pending close-others/right run (Cancel ends the sequence).
    pub(crate) fn cancel_close_batch(&mut self) {
        self.tab_actions.batch.clear();
        self.tab_actions.batch_current = None;
    }

    fn close_batch_saving(&self) -> bool {
        self.editor.after_save.values().any(|after| matches!(after, crate::editor::AfterSave::CloseTab(_)))
    }
}

fn tab_paste_id(tab: &Tab) -> Option<u64> {
    let document = tab.document()?;
    let key = document.key.borrow();
    match &*key {
        DocumentKey::Untitled(paste) => Some(paste.0),
        DocumentKey::File(_) | DocumentKey::Start => None,
    }
}

/// What closing `tab` remembers for T-07, or `None` for a start page.
pub(crate) fn closed_tab_record(tab: &Tab, index: usize) -> Option<ClosedTab> {
    let document = tab.document()?;
    let anchor = tab.view.anchor.clone();
    let key = document.key.borrow().clone();
    match key {
        DocumentKey::File(path) => Some(ClosedTab::File { path, index, anchor }),
        DocumentKey::Untitled(paste) => Some(ClosedTab::Pasted {
            id: paste.0,
            title: tab.full_title(),
            // As pasted, not as edited: “Don't save” then Ctrl+Shift+T must
            // not resurrect the discarded edits (the original's paste.original).
            text: tab.view.pasted_base.clone().unwrap_or_else(|| document.text().text().to_owned()),
            index,
            anchor,
        }),
        DocumentKey::Start => None,
    }
}

/// Clipboard text as pasted documents read it: no BOM, no trailing NULs and
/// LF line ends (the original's `pastedText`).
pub fn pasted_text(raw: &str) -> String {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let raw = raw.trim_end_matches('\0');
    raw.replace("\r\n", "\n").replace('\r', "\n")
}

/// The title of pasted text: its first level-1 heading (ATX or setext), else
/// its first line with text, as the reader sees it, at most 80 characters
/// (T-18's tab name).
///
/// Fenced code, front matter, HTML comments, link definitions and table rules
/// are skipped. The inline pass covers the common forms a pasted title carries:
/// code spans, links and images (their text), emphasis markers, HTML tags,
/// entities and backslash escapes.
pub fn pasted_title(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut fence: Option<(char, usize)> = None;
    let mut comment = false;
    // The paragraph going on, which a line of `===` under it makes a heading.
    let mut para: Option<Vec<&str>> = None;
    // Inside a list item, a quote or an HTML block: a line there is no
    // paragraph of its own.
    let mut nested = false;
    let mut first: Option<String> = None;
    let mut index = front_matter_end(&lines);
    while index < lines.len() {
        let line = head_line(lines[index]);
        index += 1;
        if let Some((ch, len)) = fence {
            if closes_fence(line, ch, len) {
                fence = None;
            } else if first.is_none() {
                first = Some(line.trim().to_owned());
            }
            continue;
        }
        if comment {
            if line.contains("-->") {
                comment = false;
            }
            continue;
        }
        if line.trim().is_empty() {
            para = None;
            nested = false;
            continue;
        }
        if let Some((ch, len)) = open_fence(line) {
            fence = Some((ch, len));
            para = None;
            continue;
        }
        if let Some((level, content)) = atx_heading(line) {
            let title = inline_text(content);
            if level == 1 && !title.is_empty() {
                return finish_title(&title);
            }
            set_first(&mut first, title);
            para = None;
            nested = false;
            continue;
        }
        if !nested && para.is_some() && is_setext_underline(line, '=') {
            let title = inline_text(&para.take().unwrap_or_default().join(" "));
            if !title.is_empty() {
                return finish_title(&title);
            }
            continue;
        }
        // A level-2 underline under a paragraph, or a rule: no title in either.
        if (!nested && para.is_some() && is_setext_underline(line, '-')) || is_thematic_break(line) {
            para = None;
            nested = false;
            continue;
        }
        let start = line.trim_start();
        if para.is_none() && start.starts_with("<!--") {
            comment = !line.contains("-->");
            continue;
        }
        if is_quote_or_item(start) || (para.is_none() && is_html_block(start)) {
            set_first(&mut first, clean_line(line));
            para = None;
            nested = true;
            continue;
        }
        if let Some(parts) = para.as_mut() {
            parts.push(line);
            continue;
        }
        // An indented code block: its text as written.
        if !nested && (line.starts_with("    ") || line.starts_with('\t')) {
            set_first(&mut first, line.trim().to_owned());
            continue;
        }
        if is_link_definition(start) || is_table_rule(start) {
            continue;
        }
        if !nested {
            para = Some(vec![line]);
        }
        set_first(&mut first, clean_line(line));
    }
    finish_title(&first.unwrap_or_default())
}

fn set_first(first: &mut Option<String>, title: String) {
    if first.is_none() && !title.is_empty() {
        *first = Some(title);
    }
}

/// `===` (level 1) or `---` (level 2) under a paragraph.
fn is_setext_underline(line: &str, marker: char) -> bool {
    let start = line.trim_start_matches(' ');
    if line.len() - start.len() > 3 {
        return false;
    }
    let trimmed = start.trim_end_matches([' ', '\t']);
    !trimmed.is_empty() && trimmed.chars().all(|ch| ch == marker)
}

/// `***`, `---`, `___` or a spaced variant: a thematic break, never a title.
fn is_thematic_break(line: &str) -> bool {
    let start = line.trim_start_matches(' ');
    if line.len() - start.len() > 3 {
        return false;
    }
    let Some(marker) = start.chars().next() else { return false };
    if !matches!(marker, '-' | '*' | '_') {
        return false;
    }
    let mut count = 0;
    for ch in start.chars() {
        if ch == marker {
            count += 1;
        } else if !matches!(ch, ' ' | '\t') {
            return false;
        }
    }
    count >= 3
}

/// A block quote or a list item: a line there is not a paragraph of its own.
fn is_quote_or_item(line: &str) -> bool {
    line.starts_with('>') || strip_list_marker(line).is_some()
}

/// An HTML block start (`<div>`, `</p>`, `<!--`, `<!DOCTYPE`, `<?xml`).
fn is_html_block(line: &str) -> bool {
    let Some(rest) = line.strip_prefix('<') else { return false };
    rest.starts_with('/')
        || rest.starts_with('!')
        || rest.starts_with('?')
        || rest.chars().next().is_some_and(|ch| ch.is_ascii_alphabetic())
}

/// The longest title, in characters.
const TITLE_MAX: usize = 80;
/// The most of a line read for a title (a minified file pasted as one line).
const TITLE_LINE_MAX: usize = 2000;

fn head_line(line: &str) -> &str {
    if line.len() <= TITLE_LINE_MAX {
        return line;
    }
    let mut end = TITLE_LINE_MAX;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    &line[..end]
}

/// The index of the first line after a leading YAML front matter (M-11's
/// shape: `---` at the very start up to `---`/`...`; unclosed runs to the end).
fn front_matter_end(lines: &[&str]) -> usize {
    let Some(first) = lines.first() else { return 0 };
    let open = *first;
    let dashes = open.chars().take_while(|ch| *ch == '-').count();
    if dashes < 3 {
        return 0;
    }
    for (index, line) in lines.iter().enumerate().skip(1) {
        let trimmed = line.trim();
        if trimmed == "..." {
            return index + 1;
        }
        let close_dashes = trimmed.chars().take_while(|ch| *ch == '-').count();
        if close_dashes >= dashes && trimmed[close_dashes..].trim().is_empty() {
            return index + 1;
        }
    }
    lines.len()
}

fn open_fence(line: &str) -> Option<(char, usize)> {
    let start = line.trim_start_matches(' ');
    if line.len() - start.len() > 3 {
        return None;
    }
    let ch = start.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let len = start.chars().take_while(|c| *c == ch).count();
    if len < 3 {
        return None;
    }
    // A backtick fence's info string has no backticks (```js`x` is inline code).
    if ch == '`' && start[len..].contains('`') {
        return None;
    }
    Some((ch, len))
}

fn closes_fence(line: &str, ch: char, len: usize) -> bool {
    let trimmed = line.trim();
    let count = trimmed.chars().take_while(|c| *c == ch).count();
    count >= len && trimmed[count * ch.len_utf8()..].trim().is_empty()
}

/// `# Title`: the ATX heading's level and text.
fn atx_heading(line: &str) -> Option<(u8, &str)> {
    let start = line.trim_start_matches(' ');
    if line.len() - start.len() > 3 {
        return None;
    }
    let hashes = start.chars().take_while(|ch| *ch == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &start[hashes..];
    if !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')) {
        return None;
    }
    let mut text = rest.trim();
    // A closing run of #s is not part of the title.
    let closing = text.chars().rev().take_while(|ch| *ch == '#').count();
    if closing > 0 {
        let without = text.len() - closing;
        if without == 0 || text[..without].ends_with(' ') {
            text = text[..without].trim_end();
        }
    }
    Some((hashes as u8, text))
}

fn is_link_definition(line: &str) -> bool {
    let Some(rest) = line.strip_prefix('[') else { return false };
    rest.contains("]:")
}

fn is_table_rule(line: &str) -> bool {
    let trimmed = line.trim().trim_matches('|').trim();
    !trimmed.is_empty() && trimmed.chars().all(|ch| matches!(ch, '-' | ':' | ' ' | '|')) && trimmed.contains('-')
}

/// A line as its text reads: the markers that open it go (quotes, list items,
/// task boxes, a heading's #s, a table's outer pipes), then the inline Markdown.
fn clean_line(line: &str) -> String {
    let mut text = line.trim();
    // Block quote markers.
    while let Some(rest) = text.strip_prefix('>') {
        text = rest.trim_start();
    }
    // One list item or task box.
    if let Some(rest) = strip_list_marker(text) {
        text = rest.trim_start();
    }
    if let Some(rest) =
        text.strip_prefix("[ ]").or_else(|| text.strip_prefix("[x]")).or_else(|| text.strip_prefix("[X]"))
    {
        text = rest.trim_start();
    }
    if let Some((_, rest)) = atx_heading(text) {
        text = rest;
    }
    let text = text.trim_matches('|').trim();
    let text = text.trim_end_matches('#').trim_end();
    if text.is_empty()
        || text.chars().all(|ch| matches!(ch, '|' | ':' | '=' | '-' | '*' | '_' | '#' | '>' | '+' | '~' | '`' | '.'))
    {
        return String::new();
    }
    inline_text(text)
}

fn strip_list_marker(text: &str) -> Option<&str> {
    let first = text.chars().next()?;
    if matches!(first, '-' | '*' | '+') {
        let rest = &text[first.len_utf8()..];
        return (rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')).then_some(rest);
    }
    let digits = text.chars().take_while(|ch| ch.is_ascii_digit()).count().min(9);
    if digits == 0 {
        return None;
    }
    let rest = &text[digits..];
    let rest = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')'))?;
    (rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')).then_some(rest)
}

/// Inline Markdown as the reader sees it: code, link text, images' alt text,
/// entities and emphasis markers (a subset of the original's inline pass).
fn inline_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '`' => {
                // A code span keeps its text without the ticks.
                let mut ticks = 1;
                while chars.peek() == Some(&'`') {
                    chars.next();
                    ticks += 1;
                }
                let mut code = String::new();
                let mut closed = 0;
                for next in chars.by_ref() {
                    if next == '`' {
                        closed += 1;
                        if closed == ticks {
                            break;
                        }
                        code.push('`');
                    } else {
                        closed = 0;
                        code.push(next);
                    }
                }
                let code = code.strip_prefix(' ').and_then(|c| c.strip_suffix(' ')).unwrap_or(&code);
                out.push_str(code);
            }
            '!' if chars.peek() == Some(&'[') => {
                chars.next();
                match take_bracket(&mut chars) {
                    Some(alt) => {
                        if matches!(chars.peek(), Some('(') | Some('[')) {
                            skip_link_target(&mut chars);
                            out.push_str(&inline_text(&alt));
                        } else {
                            out.push('!');
                            out.push('[');
                            out.push_str(&inline_text(&alt));
                            out.push(']');
                        }
                    }
                    None => {
                        out.push('!');
                        out.push('[');
                    }
                }
            }
            '[' => match take_bracket(&mut chars) {
                Some(label) => {
                    let target = matches!(chars.peek(), Some('(') | Some('['));
                    if target {
                        skip_link_target(&mut chars);
                    }
                    // Footnote references show nothing; a plain `[text]`
                    // without a target keeps its brackets.
                    if !label.starts_with('^') {
                        if !target {
                            out.push('[');
                        }
                        out.push_str(&inline_text(&label));
                        if !target {
                            out.push(']');
                        }
                    }
                }
                None => out.push('['),
            },
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            '<' => {
                let Some(tag) = take_tag(&mut chars) else {
                    out.push('<');
                    continue;
                };
                if is_autolink(&tag) {
                    out.push_str(&tag);
                } else if tag.eq_ignore_ascii_case("br") || tag.eq_ignore_ascii_case("br/") {
                    out.push(' ');
                }
            }
            '&' => {
                if let Some(entity) = take_entity(&mut chars) {
                    out.push_str(&entity);
                } else {
                    out.push('&');
                }
            }
            '*' | '~' => {
                // An emphasis marker goes; the text it wrapped stays.
                let marker = ch;
                let run = 1 + chars.clone().take_while(|c| *c == marker).count();
                for _ in 1..run {
                    chars.next();
                }
            }
            '_' => {
                // A snake_case name keeps its underscores; a marker run goes.
                let prev_word = out.chars().last().is_some_and(char::is_alphanumeric);
                let mut run = 1;
                let mut clone = chars.clone();
                while clone.peek() == Some(&'_') {
                    clone.next();
                    run += 1;
                }
                let next_word = clone.peek().is_some_and(|c| c.is_alphanumeric());
                if prev_word && next_word {
                    for _ in 0..run {
                        out.push('_');
                    }
                }
                for _ in 1..run {
                    chars.next();
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Reads a label after `[` through its matching `]`; `None` when unclosed.
fn take_bracket(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    let mut look = chars.clone();
    let mut depth = 1;
    let mut closed = false;
    for ch in look.by_ref() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    closed = true;
                    break;
                }
            }
            _ => {}
        }
    }
    if !closed {
        return None;
    }
    let mut label = String::new();
    let mut depth = 1;
    for ch in chars.by_ref() {
        match ch {
            '[' => {
                depth += 1;
                label.push(ch);
            }
            ']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                label.push(ch);
            }
            other => label.push(other),
        }
    }
    Some(label)
}

/// Skips `(target)` or `[ref]` after a link or image label.
fn skip_link_target(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    let Some(open) = chars.peek().copied() else { return };
    if open != '(' && open != '[' {
        return;
    }
    chars.next();
    let close = if open == '(' { ')' } else { ']' };
    let mut depth = 1;
    for ch in chars.by_ref() {
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                break;
            }
        }
    }
}

/// Reads `<...>` when it is an HTML tag or an autolink; a plain `<` (a
/// comparison such as `1 < 2`) stays text.
fn take_tag(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    let mut tag = String::new();
    let look = chars.clone();
    let mut closed = false;
    for next in look {
        if next == '>' {
            closed = true;
            break;
        }
        tag.push(next);
        if tag.len() > 200 {
            return None;
        }
    }
    if !closed {
        return None;
    }
    let raw = tag.trim();
    let first = tag.chars().next();
    let html = matches!(first, Some('/') | Some('!') | Some('?')) || first.is_some_and(|ch| ch.is_ascii_alphabetic());
    if !html && !is_autolink(raw) {
        return None;
    }
    for _ in 0..tag.chars().count() + 1 {
        chars.next();
    }
    Some(tag)
}

fn is_autolink(tag: &str) -> bool {
    let raw = tag.trim();
    if raw.starts_with("http://")
        || raw.starts_with("https://")
        || raw.starts_with("mailto:")
        || raw.starts_with("ftp://")
    {
        return true;
    }
    let Some((local, domain)) = raw.split_once('@') else { return false };
    !local.is_empty()
        && !local.contains(char::is_whitespace)
        && domain.split_once('.').is_some_and(|(_, suffix)| !suffix.is_empty() && !suffix.contains(char::is_whitespace))
}

/// Reads a `&name;`/`&#NNN;`/`&#xHH;` entity; `None` leaves the `&` literal.
fn take_entity(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    let mut name = String::new();
    let look = chars.clone();
    let mut closed = false;
    for next in look {
        if next == ';' {
            closed = true;
            break;
        }
        if next.is_whitespace() || next == '&' || next == '<' || name.len() >= 32 {
            return None;
        }
        name.push(next);
    }
    if !closed || name.is_empty() {
        return None;
    }
    let text = entity_text(&name)?;
    for _ in 0..name.chars().count() + 1 {
        chars.next();
    }
    Some(text)
}

fn entity_text(entity: &str) -> Option<String> {
    let ch = match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "middot" => '·',
        "bull" => '•',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "euro" => '€',
        other => {
            let number = other.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            if code == 0 || code > 0x10ffff || (0xd800..=0xdfff).contains(&code) {
                return None;
            }
            char::from_u32(code)?
        }
    };
    Some(ch.to_string())
}

/// Collapses whitespace and caps the title at [`TITLE_MAX`] characters.
fn finish_title(text: &str) -> String {
    let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = one.chars().collect();
    if chars.len() <= TITLE_MAX {
        return one;
    }
    let mut cut = &chars[..TITLE_MAX - 1];
    if let Some(space) = cut.iter().rposition(|ch| *ch == ' ')
        && space >= TITLE_MAX - 21
    {
        cut = &cut[..space];
    }
    let trimmed: String = cut.iter().copied().collect();
    let trimmed = trimmed
        .trim_end_matches(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ';' | ':' | '.' | '-' | '–' | '—'));
    format!("{trimmed}…")
}
