//! Preview → source navigation: E-35's double-click caret and E-04's
//! “Edit from here”.
//!
//! The renderer wraps every top-level block with a [`BlockRef`] carrying its
//! stable key, its source lines and its inline text exactly as drawn. This
//! module turns a double-click on that text into a caret in the source:
//!
//! * Read the word the click selected from Rune's own text selection
//!   (`Tx::text_selection`), and find its offset in the block's drawn text
//!   through the [`PreviewText`] the renderer noted for that text element.
//! * Count how many times that word occurs before that offset (any case,
//!   inside longer words too), then pick the same occurrence in the block's
//!   source, where comments, images, link targets and HTML tags are blanked so
//!   they cannot throw the count off (the original's `shownText`/`findWord`).
//! * Place the caret there: the single source field in single-field mode, the
//!   block's own virtual row in block mode, and focus it.
//!
//! A right-click on a block records it for the small context menu the view
//! draws; “Edit from here” enters edit mode (or moves the caret when already
//! editing) at the block's first line.
//!
//! Only links, buttons, task boxes, images, `<summary>` cards and diagrams keep
//! their own clicks: Rune's word selection is the hit signal, and a press on one
//! of those leaves the block's text unselected, so the block handler does
//! nothing.

use std::ops::Range;
use std::rc::Rc;

use lmdv_md::{Block, MdDoc};
use lmdv_render::{BlockRef, BlockWire, PreviewText};
use rune::{El, PointerEvent, PointerPress, SelectedText, TextSelection, Tx};

use crate::edit_ui::SOURCE_FIELD_KEY;
use crate::editor::scroll_sync::line_from_preview_y;
use crate::window::{Mode, WindowState};

/// The context menu's anchor element key.
pub const BLOCK_MENU_ANCHOR: &str = "block-menu-anchor";

/// The block a right-click landed on, waiting for the menu's choice.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockMenu {
    /// The block under the pointer, as the renderer drew it.
    pub block: BlockRef,
    /// Where the menu opens, in window points.
    pub point: PointerPress,
}

/// The cheap identity of a drawn block, kept by the wire instead of the whole
/// [`BlockRef`] (OI-097). The renderer still builds the drawn text for its wire
/// call, but no handler needs a per-frame copy of it: pointer moves need only
/// the key, and a double press or right click rebuilds the drawn reference from
/// the parsed model at event time (both are rare, unlike a frame).
#[derive(Clone, Debug)]
pub(crate) struct BlockHead {
    key: String,
    line: usize,
    end_line: usize,
    code: bool,
}

impl BlockHead {
    pub(crate) fn of(block: &BlockRef) -> Self {
        Self { key: block.key.clone(), line: block.line, end_line: block.end_line, code: block.code }
    }
}

/// The renderer's one block wire: E-35's double press and E-04's right-click.
pub fn block_wire() -> BlockWire {
    Rc::new(|element: El, block: &BlockRef| {
        // OI-097: three handlers share this head, not the drawn text. The full
        // `BlockRef` is rebuilt from the model only when an event fires.
        let head = BlockHead::of(block);
        let pressed = head.clone();
        let hovered = head.clone();
        let menu = head;
        element
            // Focusable but out of the Tab order: a click puts the keyboard on
            // the block so Shift+F10 can open its menu, without adding a Tab
            // stop per paragraph.
            .focusable(true)
            .tab_index(-1)
            .no_focus_ring()
            // E-35: the padding fallback needs the press point, and Rune's
            // double-press handler does not carry one; the pointer's last move
            // over the block is the same point for a click.
            .on_pointer_move_local(move |state: &mut WindowState, point: PointerEvent, _: &mut Tx| {
                state.editor.preview_point.set(Some([point.window_x, point.window_y]));
                state.editor.preview_block.set(block_index(&hovered.key));
            })
            .on_double_press_tx(move |state: &mut WindowState, tx: &mut Tx| {
                state.preview_double_press_head(&pressed, tx)
            })
            .on_secondary_press(move |state: &mut WindowState, point, _| state.show_block_menu_head(&menu, point))
    })
}

/// Rebuilds the drawn reference a wire head names from the active document.
///
/// The rendered text of a block is its inline text in reading order, each text
/// followed by a newline, with one [`PreviewText`] per inline (OI-097). The
/// renderer's own `note_text` uses the same shape, so a selected word maps to
/// the same offset and the same occurrence of a repeated word as it did when
/// the wire carried a copy of the frame's `BlockRef`.
pub(crate) fn drawn_block(doc: &MdDoc, head: &BlockHead) -> Option<BlockRef> {
    let index = doc.block_spans.iter().position(|span| span.key == head.key)?;
    let mut drawn = DrawnBlock::new(&head.key);
    drawn.block(&doc.blocks[index], doc);
    if matches!(doc.blocks.get(index), Some(Block::FootnoteDefs)) {
        for footnote in &doc.footnotes {
            for body in &footnote.blocks {
                drawn.block(body, doc);
            }
        }
    }
    Some(BlockRef {
        key: head.key.clone(),
        line: head.line,
        end_line: head.end_line,
        code: head.code,
        text: drawn.text,
        texts: drawn.texts,
    })
}

/// Collects a block's inline texts the way the renderer's `note_text` does.
struct DrawnBlock {
    block: String,
    text: String,
    texts: Vec<PreviewText>,
    path: Vec<String>,
}

impl DrawnBlock {
    fn new(block: &str) -> Self {
        Self { block: block.to_owned(), text: String::new(), texts: Vec::new(), path: Vec::new() }
    }

    fn note(&mut self, key: Option<&str>, drawn: &str) {
        let index = self.texts.len();
        let start = self.text.len();
        let identity = key.map(str::to_owned).unwrap_or_else(|| format!("{}-text-{index}", self.block));
        self.text.push_str(drawn);
        self.text.push('\n');
        self.texts.push(PreviewText {
            key: Some(identity),
            index,
            range: start..start + drawn.len(),
            path: self.path.clone(),
        });
    }

    fn block(&mut self, block: &Block, doc: &MdDoc) {
        match block {
            Block::Para(inline) => self.note(None, &inline.text),
            Block::Heading { heading, inline } => self.note(Some(&lmdv_render::heading_key(*heading)), &inline.text),
            Block::Code { text, .. } => self.note(None, text),
            Block::List { items, .. } => {
                for (k, item) in items.iter().enumerate() {
                    self.path.push(format!("i{k}"));
                    for child in item {
                        self.block(child, doc);
                    }
                    self.path.pop();
                }
            }
            Block::Quote { blocks, .. } => {
                for child in blocks {
                    self.block(child, doc);
                }
            }
            Block::Table(table) => {
                // The renderer builds the body rows before the sticky head and
                // scopes each cell with `c{column}`; the drawn-text order is
                // body first, head last (table.rs:79-96).
                for (row, cells) in table.rows.iter().enumerate() {
                    self.path.push(format!("row-{row}"));
                    for (column, inline) in cells.iter().enumerate() {
                        self.path.push(format!("c{column}"));
                        self.note(None, &inline.text);
                        self.path.pop();
                    }
                    self.path.pop();
                }
                self.path.push("head-row".to_owned());
                for (column, inline) in table.head.iter().enumerate() {
                    self.path.push(format!("c{column}"));
                    self.note(None, &inline.text);
                    self.path.pop();
                }
                self.path.pop();
            }
            // Pictures, diagrams, formulas, rules and the TOC hold no selectable
            // text in the rendered article; a task box strips into its item's
            // inline text, already noted above.
            Block::Rule | Block::Image(_) | Block::Mermaid { .. } | Block::MathBlock { .. } | Block::Toc => {}
            // The renderer notes the allowed HTML subset as styled inlines;
            // its concatenated text is enough to count a clicked word and to
            // offer the block's plain text for copy.
            Block::HtmlBlock(node) => {
                let text = node.text();
                if !text.is_empty() {
                    self.note(None, &text);
                }
            }
            Block::FootnoteDefs => {
                for footnote in &doc.footnotes {
                    for body in &footnote.blocks {
                        self.block(body, doc);
                    }
                }
            }
        }
    }
}

/// The index in a block's `b{index}` stable key.
fn block_index(key: &str) -> Option<usize> {
    key.strip_prefix('b').and_then(|number| number.parse().ok())
}

/// A double-click while reading only selects a word (R-27); while editing it
/// places the source caret (E-35).
impl WindowState {
    /// `on_double_press` of a preview block from the wire's lightweight head
    /// (OI-097). Viewer presses only select a word, so only an editor press
    /// rebuilds the drawn reference it maps through.
    fn preview_double_press_head(&mut self, head: &BlockHead, tx: &mut Tx) {
        if self.active_tab().is_none_or(|tab| tab.view.mode != Mode::Editor) {
            return;
        }
        let Some(doc) = self.active_reader() else { return };
        let Some(block) = drawn_block(&doc, head) else { return };
        self.preview_double_press(&block, tx);
    }

    /// `on_secondary_press` of a preview block from the wire's head (OI-097).
    /// The menu is opened by an actual right click, so rebuilding the drawn
    /// reference here keeps its source range without a per-frame copy.
    fn show_block_menu_head(&mut self, head: &BlockHead, point: PointerPress) {
        let Some(doc) = self.active_reader() else { return };
        let Some(block) = drawn_block(&doc, head) else { return };
        self.show_block_menu(&block, point);
    }

    /// `on_double_press` of a preview block (E-35).
    pub(crate) fn preview_double_press(&mut self, block: &BlockRef, tx: &mut Tx) {
        if !self.active_tab().is_some_and(|tab| tab.view.mode == Mode::Editor) {
            return;
        }
        // Rune's own word selection is the hit signal. A selected word inside
        // the block names the caret exactly; anything else — no selection, a
        // task box, a link, a picture, a diagram — goes through the guarded
        // padding fallback, which only moves the caret when the press missed
        // every drawn text box.
        let selection = tx.text_selection();
        let found = selection
            .filter(|selection| selection_in_block(block, selection))
            .and_then(|selection| locate_word(block, selection));
        match found {
            Some((offset, word)) => {
                let offset = offset.min(block.text.len());
                let before = block.text[..offset].to_owned();
                self.place_caret_in_block(block, Some(&word), Some(&before), tx);
            }
            // E-35's preview-height caret for the block's padding.
            None => {
                self.place_caret_on_preview_padding(block, tx);
            }
        }
    }

    /// E-35: a double-click on a block's padding lands on the source line at
    /// the click's fraction of the block's drawn height.
    ///
    /// The measured text boxes keep a press on a link, task box, picture or
    /// diagram from moving the caret; without a measurement (the first frame)
    /// nothing happens rather than guessing.
    pub(crate) fn place_caret_on_preview_padding(&mut self, block: &BlockRef, tx: &mut Tx) -> bool {
        let Some(point) = self.editor.preview_point.get() else { return false };
        let index = block_index(&block.key).or_else(|| self.editor.preview_block.get());
        let Some(index) = index else { return false };
        let line = {
            let geometry = self.editor.preview.borrow();
            let Some(measured) = geometry.blocks.iter().find(|candidate| candidate.index == index) else {
                return false;
            };
            if measured.text_boxes.is_empty() {
                return false;
            }
            let content_y = point[1] + self.scroll_snapshot().offset;
            let inside_text =
                measured.text_boxes.iter().any(|box_| content_y >= box_[1] && content_y <= box_[1] + box_[3]);
            if inside_text {
                return false;
            }
            line_from_preview_y(measured.top, measured.height, block.line, block.end_line, content_y)
        };
        let Some(range) = self.source_range_for_line(block, line) else { return false };
        self.apply_source_selection(range, tx)
    }

    /// `on_secondary_press` of a preview block: remember where the menu opens.
    pub(crate) fn show_block_menu(&mut self, block: &BlockRef, point: PointerPress) {
        self.ui.block_menu = Some(BlockMenu { block: block.clone(), point });
    }

    /// The open block menu, for the view.
    pub fn block_menu(&self) -> Option<&BlockMenu> {
        self.ui.block_menu.as_ref()
    }

    /// Closes the block menu (Cancel, Escape, a press outside, a tab switch).
    pub fn close_block_menu(&mut self) {
        self.ui.block_menu = None;
    }

    /// “Edit from here”: enter edit mode at `block`'s first source line, or move
    /// the caret when the tab is already editing (E-04).
    pub(crate) fn edit_from_here(&mut self, block: &BlockRef, tx: &mut Tx) {
        if self.active_tab().is_some_and(|tab| tab.view.mode == Mode::Editor) {
            self.place_caret_in_block(block, None, None, tx);
            return;
        }
        if self.enter_editor(None, tx) {
            self.place_caret_in_block(block, None, None, tx);
        }
    }

    /// `Edit from here` from the keyboard (OI-098): the block at the reading
    /// anchor, without a pointer or a right-click.
    ///
    /// The reader view publishes the top-level block under the viewport's top
    /// each frame (`reading_block`); a document that has not measured one yet
    /// falls back to the nearest heading above the viewport, then to the first
    /// block. Nothing happens outside the reading view with a parsed model.
    pub(crate) fn edit_from_reading(&mut self, tx: &mut Tx) -> bool {
        if self.active_tab().is_none_or(|tab| tab.view.mode != Mode::Viewer) {
            return false;
        }
        let Some(doc) = self.active_reader() else { return false };
        let index = self
            .reading
            .reader_block
            .get()
            .filter(|index| doc.block_key(*index).is_some())
            .or_else(|| {
                self.reading
                    .viewport_anchor
                    .get()
                    .and_then(|anchor| anchor.heading)
                    .and_then(|heading| doc.headings.get(heading))
                    .map(|heading| heading.block)
                    .filter(|block| *block < doc.blocks.len())
            })
            .or_else(|| (!doc.blocks.is_empty()).then_some(0));
        let Some(index) = index else { return false };
        let span = doc.block_spans.get(index);
        let block = BlockRef {
            key: doc.block_key(index).unwrap_or_default().to_owned(),
            line: span.map_or(0, |span| span.line),
            end_line: span.map_or(1, |span| span.end_line),
            code: matches!(doc.blocks.get(index), Some(Block::Code { .. })),
            text: String::new(),
            texts: Vec::new(),
        };
        self.edit_from_here(&block, tx);
        true
    }

    /// Puts the caret (or a word selection) in the source at `block`'s line.
    ///
    /// `word`/`before` are the clicked word and the block's drawn text in front
    /// of it: the same occurrence of a repeated word. Without a match the caret
    /// goes to the block's first source line. Focuses the source field and
    /// scrolls it into view.
    pub(crate) fn place_caret_in_block(
        &mut self,
        block: &BlockRef,
        word: Option<&str>,
        before: Option<&str>,
        tx: &mut Tx,
    ) -> bool {
        let Some(range) = self.source_range_for_word(block, word, before) else { return false };
        self.apply_source_selection(range, tx)
    }

    /// The source byte range a click maps to: the word's occurrence, else the
    /// block's first line start.
    fn source_range_for_word(
        &self,
        block: &BlockRef,
        word: Option<&str>,
        before: Option<&str>,
    ) -> Option<Range<usize>> {
        let editor = self.active_tab()?.view.editor.as_ref()?;
        let text = editor.text();
        let mut start = line_start(text, block.line);
        let end = line_start(text, block.end_line.max(block.line + 1)).max(start).min(text.len());
        start = start.min(end);
        if block.code && is_fence_opener(text.get(start..).unwrap_or("").lines().next().unwrap_or("")) {
            // A fence's language label is not part of the code: never match it.
            start = line_start(text, block.line + 1).min(end);
        }
        let source = text.get(start..end).unwrap_or("");
        let shown = if block.code { source.to_owned() } else { shown_source(source) };
        match word.and_then(|word| find_word(&shown, word, before)) {
            Some(hit) => Some(start + hit.start..start + hit.end),
            None => Some(start..start),
        }
    }

    /// The byte at the start of `line`, clamped into `block`'s own source span
    /// (E-35's padding→line fallback).
    fn source_range_for_line(&self, block: &BlockRef, line: usize) -> Option<Range<usize>> {
        let editor = self.active_tab()?.view.editor.as_ref()?;
        let text = editor.text();
        let mut start = line_start(text, block.line);
        let end = line_start(text, block.end_line.max(block.line + 1)).max(start).min(text.len());
        start = start.min(end);
        if block.code && is_fence_opener(text.get(start..).unwrap_or("").lines().next().unwrap_or("")) {
            start = line_start(text, block.line + 1).min(end);
        }
        let byte = line_start(text, line).clamp(start, end);
        Some(byte..byte)
    }

    /// Applies `range` to the active editor, focusing and revealing it.
    fn apply_source_selection(&mut self, range: Range<usize>, tx: &mut Tx) -> bool {
        let block_mode =
            self.active_tab().and_then(|tab| tab.view.editor.as_ref()).is_some_and(|editor| editor.block_mode_active());
        if block_mode {
            let index = {
                let Some(editor) = self.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()) else {
                    return false;
                };
                let Some(blocks) = editor.block_editor() else { return false };
                let Some(index) = blocks.block_index_at(range.start) else { return false };
                let offset = blocks.block(index).map_or(0, |block| block.range.start);
                let Some(field) = editor.block_editor_mut().and_then(|blocks| blocks.block_mut(index)) else {
                    return false;
                };
                let start = range.start.saturating_sub(offset).min(field.text.len());
                let end = range.end.saturating_sub(offset).min(field.text.len()).max(start);
                field.set_selection(start, end);
                index
            };
            self.focus_block(index, tx);
            tx.instant();
            return true;
        }
        let Some(editor) = self.active_tab_mut().and_then(|tab| tab.view.editor.as_mut()) else { return false };
        let end = range.end.min(editor.text().len());
        let start = range.start.min(end);
        editor.set_selection(start..end);
        tx.focus(SOURCE_FIELD_KEY, false);
        self.scroll_to_editor_selection(tx);
        tx.instant();
        true
    }
}

/// Whether `selection` covers part of `block`'s drawn text (E-35's hit signal).
///
/// A press on a link, a task box, a picture or a diagram leaves no selection
/// on the block's text, so those keep their own clicks.
pub fn selection_in_block(block: &BlockRef, selection: &TextSelection) -> bool {
    let marker = format!("/{}/", block.key);
    selection.texts().iter().any(|text| text.path().contains(&marker))
}

/// The drawn offset of the word `selection` picked, and the word itself.
///
/// `selection` must be inside `block`: its text element is matched to one of
/// the renderer's [`PreviewText`]s by key and by the path keys both sides know
/// (a list item, a table row/cell), then by length and content as a fallback.
/// An empty selection (a press on punctuation) yields `None`.
pub fn locate_word(block: &BlockRef, selection: &TextSelection) -> Option<(usize, String)> {
    for selected in selection.texts() {
        let word = selected.text().trim();
        if word.is_empty() {
            continue;
        }
        let Some(segment) = segment_for(block, selected) else { continue };
        let range = selected.range();
        if range.end > segment.range.len() {
            continue;
        }
        let offset = segment.range.start + range.start;
        if offset <= block.text.len() {
            return Some((offset, word.to_owned()));
        }
    }
    None
}

/// The rendered text element `selected` belongs to.
fn segment_for<'a>(block: &'a BlockRef, selected: &SelectedText) -> Option<&'a PreviewText> {
    let range = selected.range();
    segment_for_parts(block, selected.key(), selected.path(), selected.len(), &range, selected.text())
}

/// [`segment_for`] on primitives, so the matching rules are testable.
fn segment_for_parts<'a>(
    block: &'a BlockRef,
    key: Option<&str>,
    path: &str,
    len: usize,
    range: &Range<usize>,
    text: &str,
) -> Option<&'a PreviewText> {
    let path_keys: Vec<&str> = path.split('/').filter(|part| !part.starts_with('#')).collect();
    let mut best: Option<&PreviewText> = None;
    for segment in &block.texts {
        if segment.key.as_deref() != key {
            continue;
        }
        if !segment.path.iter().all(|key| path_keys.contains(&key.as_str())) {
            continue;
        }
        // Prefer the most specific scope: a nested list item over its parent.
        if best.is_none_or(|previous| previous.path.len() < segment.path.len()) {
            best = Some(segment);
        }
    }
    if best.is_some() {
        return best;
    }
    // No key or scope matched (a table cell without keys, an HTML card): fall
    // back to the text's length and the selected bytes.
    block.texts.iter().find(|segment| {
        segment.range.len() == len
            && block.text.get(segment.range.start + range.start..segment.range.start + range.end) == Some(text)
    })
}

/// The byte offset of a 0-based source line's start, clamped to `text`.
pub fn line_start(text: &str, line: usize) -> usize {
    if line == 0 {
        return 0;
    }
    text.split_inclusive('\n').take(line).map(str::len).sum::<usize>().min(text.len())
}

fn is_fence_opener(line: &str) -> bool {
    let trimmed = line.trim_start();
    let marker = trimmed.as_bytes().first().copied();
    let run = trimmed.bytes().take_while(|byte| Some(byte) == marker.as_ref()).count();
    matches!(marker, Some(b'`' | b'~')) && run >= 3
}

/// Prose source as the preview shows it: comments, images, link targets and
/// HTML tags become spaces of the same byte length, so every offset stays
/// valid. Code spans stay as written (the original's `shownText`).
pub fn shown_source(source: &str) -> String {
    let mut out = source.as_bytes().to_vec();
    let mut i = 0;
    while i < source.len() {
        let rest = &source[i..];
        if rest.starts_with('`') {
            let run = rest.bytes().take_while(|byte| *byte == b'`').count();
            match closing_run(source, i + run, run) {
                Some(at) => i = at + run,
                None => i += run,
            }
            continue;
        }
        if let Some(len) = comment_len(rest) {
            blank(&mut out, i, i + len);
            i += len;
            continue;
        }
        if let Some(len) = image_len(rest) {
            blank(&mut out, i, i + len);
            i += len;
            continue;
        }
        if let Some(after) = rest.strip_prefix(']')
            && let Some(len) = target_len(after)
        {
            blank(&mut out, i, i + 1 + len);
            i += 1 + len;
            continue;
        }
        if let Some(len) = tag_len(rest) {
            blank(&mut out, i, i + len);
            i += len;
            continue;
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    String::from_utf8(out).unwrap_or_else(|_| source.to_owned())
}

/// Blanks `from..to`, keeping newlines so line structure survives.
fn blank(out: &mut [u8], from: usize, to: usize) {
    let to = to.min(out.len());
    for byte in &mut out[from..to] {
        if *byte != b'\n' && *byte != b'\r' {
            *byte = b' ';
        }
    }
}

fn closing_run(source: &str, from: usize, run: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let length = bytes[i..].iter().take_while(|byte| **byte == b'`').count();
            if length == run {
                return Some(i);
            }
            i += length;
        } else {
            i += 1;
        }
    }
    None
}

fn comment_len(rest: &str) -> Option<usize> {
    rest.starts_with("<!--").then(|| rest.find("-->").map_or(rest.len(), |at| at + 3))
}

/// An image as the original blanks it: `![alt](target)` with its target.
fn image_len(rest: &str) -> Option<usize> {
    let body = rest.strip_prefix("![")?;
    let close = body.find(']')?;
    if body[..close].contains('\n') {
        return None;
    }
    let target = target_len(&body[close + 1..])?;
    Some(2 + close + 1 + target)
}

/// `(address "title")`, one level of parentheses inside, as written.
fn target_len(rest: &str) -> Option<usize> {
    let body = rest.strip_prefix('(')?;
    let mut depth = 0usize;
    let mut i = 0;
    while i < body.len() {
        match body.as_bytes()[i] {
            b'\n' => return None,
            b'(' => depth += 1,
            b')' => {
                if depth == 0 {
                    return Some(1 + i + 1);
                }
                depth -= 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn tag_len(rest: &str) -> Option<usize> {
    let body = rest.strip_prefix('<')?;
    let name = body.strip_prefix('/').unwrap_or(body);
    if !name.chars().next()?.is_ascii_alphabetic() {
        return None;
    }
    let end = rest.find('>')?;
    if rest[1..end].contains('<') {
        return None;
    }
    Some(end + 1)
}

/// Every occurrence of `word` in `text`, with the original's matching rules:
/// whitespace inside `word` matches any run of whitespace, `ignore_case`
/// compares characters case-insensitively, and `whole` requires a non-word
/// character (or the end) on both sides.
pub fn matches_of(text: &str, word: &str, ignore_case: bool, whole: bool) -> Vec<Range<usize>> {
    let parts: Vec<&str> = word.split_whitespace().collect();
    if parts.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if let Some((end, next)) = match_at(&chars, i, &parts, ignore_case, whole) {
            out.push(chars[i].0..end);
            i = next;
        } else {
            i += 1;
        }
    }
    out
}

/// One match of the word's parts at char index `start`: (end byte, next index).
fn match_at(
    chars: &[(usize, char)],
    start: usize,
    parts: &[&str],
    ignore_case: bool,
    whole: bool,
) -> Option<(usize, usize)> {
    if whole && start > 0 && is_word_char(chars[start - 1].1) {
        return None;
    }
    let mut at = start;
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            let mut skipped = false;
            while at < chars.len() && chars[at].1.is_whitespace() {
                at += 1;
                skipped = true;
            }
            if !skipped {
                return None;
            }
        }
        for expected in part.chars() {
            let (_, actual) = *chars.get(at)?;
            if !char_eq(actual, expected, ignore_case) {
                return None;
            }
            at += 1;
        }
    }
    if whole && at < chars.len() && is_word_char(chars[at].1) {
        return None;
    }
    let end = if at == 0 { 0 } else { chars[at - 1].0 + chars[at - 1].1.len_utf8() };
    Some((end, at))
}

fn char_eq(a: char, b: char, ignore_case: bool) -> bool {
    a == b || (ignore_case && a.to_lowercase().eq(b.to_lowercase()))
}

/// The original's word character class: `\p{L}\p{M}\p{N}` (OI-098).
fn is_word_char(c: char) -> bool {
    crate::word_boundary::is_word_char(c)
}

/// The word's occurrence in the block's source (the original's `findWord`).
///
/// With `before` — the block's drawn text in front of the click — the same
/// occurrence of a repeated word is picked, counting loosely (any case,
/// inside longer words). Without it: the first whole-word exact match, then
/// any case, then inside a longer word.
pub fn find_word(text: &str, word: &str, before: Option<&str>) -> Option<Range<usize>> {
    let word = word.trim();
    if word.is_empty() || word.chars().count() > 200 {
        return None;
    }
    if let Some(before) = before {
        let all = matches_of(text, word, true, false);
        if all.is_empty() {
            return None;
        }
        let count = matches_of(before, word, true, false).len();
        return all.get(count.min(all.len() - 1)).cloned();
    }
    for (ignore_case, whole) in [(false, true), (true, true), (false, false), (true, false)] {
        if let Some(first) = matches_of(text, word, ignore_case, whole).into_iter().next() {
            return Some(first);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use lmdv_render::PreviewText;

    fn block(text: &str, texts: Vec<PreviewText>) -> BlockRef {
        BlockRef { key: "b1".to_owned(), line: 2, end_line: 3, code: false, text: text.to_owned(), texts }
    }

    fn preview(key: Option<&str>, range: Range<usize>, path: &[&str]) -> PreviewText {
        PreviewText {
            key: key.map(str::to_owned),
            index: 0,
            range,
            path: path.iter().map(|key| (*key).to_owned()).collect(),
        }
    }

    #[test]
    fn a_line_starts_after_its_preceding_newlines() {
        let text = "one\ntwo\nthree";
        assert_eq!(line_start(text, 0), 0);
        assert_eq!(line_start(text, 1), 4);
        assert_eq!(line_start(text, 2), 8);
        assert_eq!(line_start(text, 9), text.len(), "past the end clamps");
    }

    #[test]
    fn a_word_matches_whole_any_case_and_inside_longer_words() {
        let text = "the Budget budgets THE budget";
        assert_eq!(matches_of(text, "budget", false, true), vec![23..29], "exact case, whole word");
        assert_eq!(matches_of(text, "budget", true, true), vec![4..10, 23..29], "any case, whole word");
        assert_eq!(matches_of(text, "budget", false, false).len(), 2, "exact case inside `budgets` too");
        assert_eq!(matches_of(text, "budget", true, false).len(), 3, "any case, inside longer words");
        assert_eq!(matches_of("the telescope", "scope", false, true), Vec::<Range<usize>>::new());
        assert_eq!(matches_of("the telescope", "scope", false, false), vec![8..13]);
    }

    #[test]
    fn a_combining_mark_script_word_is_whole() {
        // OI-098: Devanagari matras are combining marks, so `किताब` is one word.
        let text = "किताब और किताब";
        assert_eq!(matches_of(text, "किताब", false, true).len(), 2);
        assert_eq!(find_word(text, "किताब", None), Some(0..text.find(' ').unwrap_or(text.len())));
    }

    #[test]
    fn a_word_with_spaces_matches_any_whitespace_run() {
        let text = "one   two\nthree";
        assert_eq!(matches_of(text, "one two", false, false), vec![0..9]);
        assert_eq!(matches_of(text, "two three", false, false), vec![6..15]);
        assert_eq!(matches_of(text, "one\nthree", false, false), Vec::<Range<usize>>::new());
    }

    #[test]
    fn the_before_text_picks_the_same_occurrence() {
        let source = "budget one and budget two";
        let word = "budget";
        assert_eq!(find_word(source, word, None), Some(0..6));
        assert_eq!(find_word(source, word, Some("budget one and ")), Some(15..21));
        // Nothing before it: the first match; more occurrences before it than
        // the block holds: the last (the original's clamp).
        assert_eq!(find_word(source, word, Some("nothing here")), Some(0..6));
        assert_eq!(find_word(source, word, Some("budget budget budget")), Some(15..21));
    }

    #[test]
    fn matching_falls_back_from_whole_word_to_any_case_then_inside() {
        assert_eq!(find_word("The BUDGET", "budget", None), Some(4..10), "any case, whole word");
        assert_eq!(find_word("budgets", "budget", None), Some(0..6), "inside a longer word");
        assert_eq!(find_word("no match", "budget", None), None);
    }

    #[test]
    fn shown_source_blanks_comments_images_targets_and_tags() {
        let source = "A <!-- budget --> B ![budget](b.png) [x](budget.md) <b budget> `budget`";
        let shown = shown_source(source);
        assert_eq!(tag_len("<b budget> x"), Some(10));
        assert_eq!(target_len("(b.png) x"), Some(7));
        assert_eq!(shown.len(), source.len(), "offsets stay valid");
        assert!(!shown.contains("<!--") && !shown.contains("<b "), "comments and tags are blanked: {shown:?}");
        assert!(shown.contains("`budget`"), "a code span stays as written");
        assert_eq!(matches_of(&shown, "budget", true, false).len(), 1, "{shown:?}");
    }

    #[test]
    fn an_unclosed_comment_blanks_the_rest_of_the_block() {
        let shown = shown_source("a <!-- never closed\nbudget");
        assert!(!shown.contains("budget"));
        assert!(shown.contains('\n'), "the newline survives");
    }

    #[test]
    fn prose_parentheses_are_not_mistaken_for_link_targets() {
        let shown = shown_source("the budget (and the budget) stays");
        assert_eq!(shown, "the budget (and the budget) stays");
        assert_eq!(matches_of(&shown, "budget", true, false).len(), 2);
    }

    #[test]
    fn a_block_word_maps_through_its_text_segment() {
        let first = preview(Some("text"), 0..8, &[]);
        let second = preview(None, 9..23, &["i1"]);
        let source = block("budget a\none budget two\n", vec![first, second]);
        assert_eq!(source.texts[1].range, 9..23);
        assert_eq!(&source.text[9..23], "one budget two");
    }

    #[test]
    fn a_selection_is_matched_by_key_then_path_then_content() {
        let text = "one apple\ntwo budget";
        let first = preview(None, 0..9, &["i0"]);
        let second = preview(None, 10..20, &["i1"]);
        let source = block(text, vec![first, second]);

        let found = segment_for_parts(&source, None, "#0/b1/#3/i1/#0/body/#1/#0", 10, &(4..10), "budget")
            .expect("the second item");
        assert_eq!(found.range, 10..20, "the path's i1 picks the second text");

        // Without a usable path, the length and the selected bytes still name it.
        let found = segment_for_parts(&source, None, "#0", 10, &(4..10), "budget").expect("content fallback");
        assert_eq!(found.range, 10..20);

        // A keyed text element matches by key alone.
        let heading = preview(Some("h2"), 0..9, &[]);
        let heading_block = block("A heading\n", vec![heading]);
        let found = segment_for_parts(&heading_block, Some("h2"), "#0", 9, &(2..9), "heading");
        assert_eq!(found.map(|segment| segment.range.clone()), Some(0..9));
    }

    #[test]
    fn a_fence_opener_is_skipped_when_searching_code() {
        assert!(is_fence_opener("```rust"));
        assert!(is_fence_opener("  ~~~~ js"));
        assert!(!is_fence_opener("`` inline"));
        assert!(!is_fence_opener("plain text"));
        assert!(!is_fence_opener(""));
    }

    #[test]
    fn a_word_outside_every_segment_maps_to_nothing() {
        let source = block("one\n", vec![preview(Some("text"), 0..3, &[])]);
        assert!(segment_for_parts(&source, None, "#0", 4, &(0..4), "one!").is_none());
    }
}
