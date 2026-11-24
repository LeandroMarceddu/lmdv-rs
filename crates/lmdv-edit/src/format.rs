//! Formatting-bar and line commands over Markdown source.
//!
//! These are pure functions on a [`TextEdit`], shared by the format bar, the
//! `/` block menu, the Tools menu and the editor keymap (parity rows
//! E-22–E-31). Every content command applies its changes inside one Rune undo
//! group and leaves the selection on the edited text; caret-only commands
//! (`select_line`, `goto_line`) do not create history.
//!
//! The algorithms mirror the original's `src/editCommands.ts`: surrounding
//! whitespace stays outside inline markers, `*` and `**` use run parity so
//! italic never eats bold, list prefixes preserve indentation, ordered lists
//! renumber `1..n`, and block inserts replace an empty line or add blank lines
//! around the template.

use std::ops::Range;

use rune::TextEdit;

use crate::block::{fence_open, is_fence_close};

/// A list or quote prefix toggled by [`toggle_line_prefix`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinePrefix {
    /// `- ` on each selected line.
    Bullet,
    /// `1. `, `2. ` … on the selected lines.
    Ordered,
    /// `- [ ] ` on each selected line.
    Task,
    /// `> ` on each selected line.
    Quote,
}

/// A block template from the Insert / `/` menu (E-27).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// A GFM table with a selected first column label.
    Table,
    /// A fenced `mermaid` flowchart.
    Mermaid,
    /// A `$$` display-math block.
    Math,
    /// A plain fenced code block.
    Code,
    /// A `[!NOTE]` callout.
    Note,
    /// A `[!TIP]` callout.
    Tip,
    /// A `[!WARNING]` callout.
    Warning,
    /// A horizontal rule.
    Divider,
}

impl BlockKind {
    /// All menu entries in display order.
    pub const ALL: [BlockKind; 8] = [
        BlockKind::Table,
        BlockKind::Mermaid,
        BlockKind::Math,
        BlockKind::Code,
        BlockKind::Note,
        BlockKind::Tip,
        BlockKind::Warning,
        BlockKind::Divider,
    ];

    /// The template inserted for this kind. `{{…}}` marks the placeholder
    /// selection; the text between the braces is what the user edits.
    pub fn template(self) -> &'static str {
        match self {
            BlockKind::Table => {
                "| {{Column}} | Column | Column |\n| --- | --- | --- |\n| Cell | Cell | Cell |\n| Cell | Cell | Cell |"
            }
            BlockKind::Mermaid => {
                "```mermaid\nflowchart LR\n  A[{{Start}}] --> B{Decision}\n  B -->|Yes| C[Do it]\n  B -->|No| D[Skip]\n```"
            }
            BlockKind::Math => "$$\n{{E = mc^2}}\n$$",
            BlockKind::Code => "```\n{{code}}\n```",
            BlockKind::Note => "> [!NOTE]\n> {{Useful information.}}",
            BlockKind::Tip => "> [!TIP]\n> {{A helpful suggestion.}}",
            BlockKind::Warning => "> [!WARNING]\n> {{Something to watch out for.}}",
            BlockKind::Divider => "---\n",
        }
    }

    /// Inserts this block at the caret.
    pub fn insert(self, edit: &mut TextEdit) -> bool {
        insert_block(edit, self.template())
    }
}

#[derive(Clone, Debug)]
struct Change {
    range: Range<usize>,
    insert: String,
}

impl Change {
    fn new(range: Range<usize>, insert: impl Into<String>) -> Self {
        Self { range, insert: insert.into() }
    }
}

struct LineMap<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineMap<'a> {
    fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(offset, _)| offset + 1));
        Self { text, starts }
    }

    fn count(&self) -> usize {
        self.starts.len()
    }

    fn start(&self, line: usize) -> usize {
        self.starts.get(line).copied().unwrap_or(self.text.len())
    }

    fn end(&self, line: usize) -> usize {
        match self.starts.get(line + 1) {
            Some(next) => next.saturating_sub(1),
            None => self.text.len(),
        }
    }

    /// End of the line including its newline.
    fn full_end(&self, line: usize) -> usize {
        self.starts.get(line + 1).copied().unwrap_or(self.text.len())
    }

    fn index_of(&self, byte: usize) -> usize {
        let byte = byte.min(self.text.len());
        self.starts.partition_point(|&start| start <= byte).saturating_sub(1)
    }

    fn selected(&self, edit: &TextEdit) -> (usize, usize) {
        let (from, to) = edit.range();
        (self.index_of(from), self.index_of(to))
    }

    fn line_text(&self, line: usize) -> &'a str {
        &self.text[self.start(line)..self.end(line)]
    }

    fn indent_end(&self, line: usize) -> usize {
        let start = self.start(line);
        let end = self.end(line);
        let mut offset = start;
        while offset < end && matches!(self.text.as_bytes()[offset], b' ' | b'\t') {
            offset += 1;
        }
        offset
    }
}

/// Toggles `marker` around the selection, or around the word under the caret
/// (E-23). Surrounding whitespace stays outside; an existing marker run outside
/// or inside the selection is removed.
pub fn toggle_wrap(edit: &mut TextEdit, marker: &str) -> bool {
    if marker.is_empty() {
        return false;
    }
    let text = edit.text.clone();
    let (mut from, mut to) = edit.range();
    if from == to {
        if let Some((start, end)) = word_at(&text, from) {
            from = start;
            to = end;
        }
    } else {
        let selected = &text[from..to];
        let leading = selected.len() - selected.trim_start().len();
        let trailing = selected.len() - selected.trim_end().len();
        from += leading;
        to = to.saturating_sub(trailing);
        if from > to {
            to = from;
        }
    }

    let width = marker.len();
    let mut changes = Vec::new();
    let selection;
    if wrapped_by(&text, from, to, marker) {
        changes.push(Change::new(from - width..from, ""));
        changes.push(Change::new(to..to + width, ""));
        selection = (from - width, to - width);
    } else {
        let inner = &text[from..to];
        let strip = inner.len() >= 2 * width
            && inner.starts_with(marker)
            && inner.ends_with(marker)
            && (marker != "*" || !inner.starts_with("**") || inner.starts_with("***"));
        if strip {
            changes.push(Change::new(from..to, inner[width..inner.len() - width].to_owned()));
            selection = (from, to - 2 * width);
        } else {
            let mut inserted = String::with_capacity(inner.len() + 2 * width);
            inserted.push_str(marker);
            inserted.push_str(inner);
            inserted.push_str(marker);
            changes.push(Change::new(from..to, inserted));
            selection = (from + width, to + width);
        }
    }
    changes.sort_by_key(|change| change.range.start);
    apply_changes(edit, &changes, Some(selection))
}

/// Sets the ATX heading level on every selected line; the same level again (or
/// level 0) removes it (E-25). Indentation is preserved.
pub fn set_heading(edit: &mut TextEdit, level: u8) -> bool {
    let level = level.min(6);
    let text = edit.text.clone();
    let map = LineMap::new(&text);
    let (first, last) = map.selected(edit);
    let lines: Vec<usize> = (first..=last).collect();
    let all_at_level = level > 0
        && lines.iter().all(|&line| heading_prefix(&map, line).is_some_and(|(hashes, _)| hashes == level as usize));

    let mut changes = Vec::new();
    for &line in &lines {
        let start = map.indent_end(line);
        let take = heading_prefix(&map, line).map(|(_, take)| take).unwrap_or(0);
        let insert =
            if level > 0 && !all_at_level { format!("{} ", "#".repeat(level as usize)) } else { String::new() };
        if take > 0 || !insert.is_empty() {
            changes.push(Change::new(start..start + take, insert));
        }
    }
    apply_with_selection(edit, changes)
}

/// Toggles a list or quote marker on every selected line (E-26). Empty lines
/// are skipped when the selection has content; ordered lists renumber from 1.
pub fn toggle_line_prefix(edit: &mut TextEdit, kind: LinePrefix) -> bool {
    let text = edit.text.clone();
    let map = LineMap::new(&text);
    let (first, last) = map.selected(edit);
    let all_lines: Vec<usize> = (first..=last).collect();
    let content: Vec<usize> =
        all_lines.iter().copied().filter(|&line| !map.line_text(line).trim().is_empty()).collect();
    let targets = if content.is_empty() { all_lines } else { content };
    if targets.is_empty() {
        return false;
    }

    if kind == LinePrefix::Quote {
        let all_quoted = targets.iter().all(|&line| quote_prefix(&text, map.indent_end(line), map.end(line)).is_some());
        let mut changes = Vec::new();
        if all_quoted {
            for &line in &targets {
                let start = map.indent_end(line);
                if let Some(end) = quote_prefix(&text, start, map.end(line)) {
                    changes.push(Change::new(start..end, ""));
                }
            }
        } else {
            for &line in &targets {
                if quote_prefix(&text, map.indent_end(line), map.end(line)).is_none() {
                    changes.push(Change::new(map.start(line)..map.start(line), "> "));
                }
            }
        }
        return apply_with_selection(edit, changes);
    }

    let requested = match kind {
        LinePrefix::Bullet => ListKind::Bullet,
        LinePrefix::Ordered => ListKind::Ordered,
        LinePrefix::Task => ListKind::Task,
        LinePrefix::Quote => return false,
    };
    let all_of_kind = targets.iter().all(|&line| {
        list_prefix(&text, map.indent_end(line), map.end(line)).is_some_and(|(_, found)| found == requested)
    });
    let mut changes = Vec::new();
    for (position, &line) in targets.iter().enumerate() {
        let start = map.indent_end(line);
        let end = match list_prefix(&text, start, map.end(line)) {
            Some((end, _)) => end,
            None => start,
        };
        let insert = if all_of_kind {
            String::new()
        } else {
            match requested {
                ListKind::Bullet => "- ".to_owned(),
                ListKind::Task => "- [ ] ".to_owned(),
                ListKind::Ordered => format!("{}. ", position + 1),
            }
        };
        if end != start || !insert.is_empty() {
            changes.push(Change::new(start..end, insert));
        }
    }
    apply_with_selection(edit, changes)
}

/// Wraps the selection in a Markdown link, or turns a bare URL into an empty
/// link label (E-24).
pub fn insert_link(edit: &mut TextEdit) -> bool {
    let text = edit.text.clone();
    let (from, to) = edit.range();
    let selected = text[from..to].to_owned();
    let (change, selection) = if is_bare_url(&selected) {
        (Change::new(from..to, format!("[]({selected})")), (from + 1, from + 1))
    } else {
        let label = if selected.is_empty() { "link text".to_owned() } else { selected.clone() };
        let selection = if selected.is_empty() {
            (from + 1, from + 1 + label.len())
        } else {
            (from + label.len() + 3, from + label.len() + 6)
        };
        (Change::new(from..to, format!("[{label}](url)")), selection)
    };
    apply_changes(edit, &[change], Some(selection))
}

/// Inserts a block template (E-27). An empty line is replaced; otherwise the
/// block goes after the current line with blank lines around it. The `{{…}}`
/// placeholder is selected.
pub fn insert_block(edit: &mut TextEdit, template: &str) -> bool {
    let text = edit.text.clone();
    let map = LineMap::new(&text);
    let line = map.index_of(edit.focus);
    let start = map.start(line);
    let end = map.end(line);
    let empty = text[start..end].trim().is_empty();

    let (from, to, before) = if empty {
        let previous_has_content = line > 0 && !map.line_text(line - 1).trim().is_empty();
        (start, end, if previous_has_content { "\n" } else { "" })
    } else {
        (end, end, "\n\n")
    };
    let next_has_content = line + 1 < map.count() && !map.line_text(line + 1).trim().is_empty();
    let after = if next_has_content { "\n" } else { "" };

    let (body, placeholder) = render_template(template);
    let inserted = format!("{before}{body}{after}");
    let body_start = from + before.len();
    let selection = match placeholder {
        Some((open, close)) => (body_start + open, body_start + close),
        None => (body_start + body.len(), body_start + body.len()),
    };
    apply_changes(edit, &[Change::new(from..to, inserted)], Some(selection))
}

/// Moves the selected lines up one line (E-29). False on the first line.
pub fn move_line_up(edit: &mut TextEdit) -> bool {
    let text = edit.text.clone();
    let map = LineMap::new(&text);
    let (first, last) = map.selected(edit);
    if first == 0 {
        return false;
    }
    let block_start = map.start(first);
    let block_end = map.full_end(last);
    let previous_start = map.start(first - 1);
    if block_start == 0 || text.as_bytes().get(block_start - 1) != Some(&b'\n') {
        return false;
    }
    let previous = &text[previous_start..block_start - 1];
    let block = &text[block_start..block_end];
    let ends_with_newline = block.ends_with('\n');
    let replacement = if ends_with_newline { format!("{block}{previous}\n") } else { format!("{block}\n{previous}") };
    let inserted_len = replacement.len();
    let new_len = text.len() - (block_end - previous_start) + inserted_len;
    let (from, to) = edit.range();
    let map_offset = |offset: usize| -> usize {
        if offset == text.len() && block_end == text.len() {
            return new_len;
        }
        if (block_start..block_end).contains(&offset) {
            return previous_start + (offset - block_start);
        }
        if (previous_start..block_start).contains(&offset) {
            let extra = if ends_with_newline { 0 } else { 1 };
            return previous_start + block.len() + extra + (offset - previous_start);
        }
        offset
    };
    let selection = (map_offset(from), map_offset(to));
    apply_changes(edit, &[Change::new(previous_start..block_end, replacement)], Some(selection))
}

/// Moves the selected lines down one line (E-29). False on the last line.
pub fn move_line_down(edit: &mut TextEdit) -> bool {
    let text = edit.text.clone();
    let map = LineMap::new(&text);
    let (first, last) = map.selected(edit);
    if last + 1 >= map.count() {
        return false;
    }
    let block_start = map.start(first);
    let block_end = map.full_end(last);
    let next_end = map.full_end(last + 1);
    if block_end >= next_end {
        return false;
    }
    let block = &text[block_start..block_end];
    let next = &text[block_end..next_end];
    if next.is_empty() {
        return false;
    }
    let block_core = block.strip_suffix('\n').unwrap_or(block);
    let next_core = next.strip_suffix('\n').unwrap_or(next);
    let trailing = if next.ends_with('\n') { "\n" } else { "" };
    let replacement = format!("{next_core}\n{block_core}{trailing}");
    let (from, to) = edit.range();
    let map_offset = |offset: usize| -> usize {
        if (block_start..block_end).contains(&offset) {
            return offset + next.len();
        }
        if (block_end..next_end).contains(&offset) {
            return block_start + (offset - block_end);
        }
        offset
    };
    let selection = (map_offset(from), map_offset(to));
    apply_changes(edit, &[Change::new(block_start..next_end, replacement)], Some(selection))
}

/// Duplicates the selected lines below and keeps the selection on the copy
/// (E-29, `Shift+Alt+↓`).
pub fn duplicate_line(edit: &mut TextEdit) -> bool {
    let text = edit.text.clone();
    let map = LineMap::new(&text);
    let (first, last) = map.selected(edit);
    let block_start = map.start(first);
    let block_end = map.full_end(last);
    let block = &text[block_start..block_end];
    let insertion =
        if block_end == text.len() && !block.ends_with('\n') { format!("\n{block}") } else { block.to_owned() };
    if insertion.is_empty() {
        return false;
    }
    let shift = insertion.len();
    let (from, to) = edit.range();
    let map_offset =
        |offset: usize| -> usize { if (block_start..=block_end).contains(&offset) { offset + shift } else { offset } };
    let selection = (map_offset(from), map_offset(to));
    apply_changes(edit, &[Change::new(block_end..block_end, insertion)], Some(selection))
}

/// Deletes the selected lines (E-29, `Ctrl+Shift+K`).
pub fn delete_line(edit: &mut TextEdit) -> bool {
    let text = edit.text.clone();
    let map = LineMap::new(&text);
    let (first, last) = map.selected(edit);
    let block_start = map.start(first);
    let block_end = map.full_end(last);
    let block = &text[block_start..block_end];
    let (range, caret) = if block_end == text.len() && !block.ends_with('\n') && block_start > 0 {
        let previous = block_start - 1;
        if text.as_bytes().get(previous) == Some(&b'\n') {
            (previous..block_end, previous)
        } else {
            (block_start..block_end, block_start)
        }
    } else {
        (block_start..block_end, block_start)
    };
    let new_len = text.len() - (range.end - range.start);
    let caret = caret.min(new_len);
    apply_changes(edit, &[Change::new(range, "")], Some((caret, caret)))
}

/// Selects the lines the selection touches, excluding the final newline
/// (E-29, `Alt+L`). Caret-only: no undo step.
pub fn select_line(edit: &mut TextEdit) -> bool {
    let text = edit.text.clone();
    let map = LineMap::new(&text);
    let (first, last) = map.selected(edit);
    let range = map.start(first)..map.end(last);
    if edit.range() == (range.start, range.end) {
        return false;
    }
    edit.set_selection(range.start, range.end);
    edit.break_history_group();
    true
}

/// Toggles an HTML comment around the selected lines, or the fenced language's
/// own comment inside a code fence (E-30).
///
/// Two places are a deliberate no-op, with no guessing:
///
/// * a fence whose language has no comment syntax here (an unknown or
///   data-only label): inserting `<!-- -->` into code would not comment it, it
///   would become code, and a guessed marker could corrupt the block;
/// * indented code: its lines carry no language, and the only marker Markdown
///   knows (`<!-- -->`) would sit inside the indented block as literal code.
///
/// `false` means "nothing changed"; the caller keeps the caret and text.
pub fn toggle_comment(edit: &mut TextEdit) -> bool {
    let text = edit.text.clone();
    let map = LineMap::new(&text);
    let (first, last) = map.selected(edit);
    let style = match fence_language_at(&map, first) {
        Some(language) => match comment_style(&language) {
            // OI-060: an unknown fence language is a documented no-op.
            Some(style) => style,
            None => return false,
        },
        None if indented_code_at(&map, first) => {
            // OI-060: indented code has no language to comment with.
            return false;
        }
        None => CommentStyle::Block { open: "<!--", close: "-->" },
    };
    let changes = match style {
        CommentStyle::Line(marker) => line_comment_changes(&text, &map, first, last, marker),
        CommentStyle::Block { open, close } => block_comment_changes(&text, &map, first, last, open, close),
    };
    apply_with_selection(edit, changes)
}

/// Whether the selected line is indented code, so a Markdown comment would not
/// comment it (E-30/OI-060).
///
/// A line indented by four or more columns is code when it starts a block: at
/// the document's start, after a blank line, or directly under another
/// indented line. A lazy paragraph continuation (`prose` then `    more`) and a
/// nested list item's paragraph are not code. Inside a list or quote the
/// enclosing container's content indent is the base, so its own four columns
/// are required. This is a conservative reading of CommonMark's indented-code
/// rule: when in doubt the comment command does nothing rather than corrupt the
/// block.
fn indented_code_at(map: &LineMap<'_>, line: usize) -> bool {
    let indent = indent_columns(map.line_text(line));
    if indent < 4 {
        return false;
    }
    // The nearest non-blank line indented less than this one is the enclosing
    // container: a list item or quote sets the content indent, prose sets
    // itself (so a lazy continuation is never code).
    let mut base = 0usize;
    for previous in (0..line).rev() {
        let text = map.line_text(previous);
        if text.trim().is_empty() {
            continue;
        }
        let previous_indent = indent_columns(text);
        if previous_indent < indent {
            base = match container_content_indent(text) {
                Some(content) => content,
                None => previous_indent,
            };
            break;
        }
    }
    if indent < base.saturating_add(4) {
        return false;
    }
    // Code starts a block: at the document's start, after a blank line, under
    // another line of the same indented run, or after a line that ends a block
    // (a heading, a rule, a quote or a fence opener).
    match line.checked_sub(1) {
        None => true,
        Some(previous) => {
            let above = map.line_text(previous);
            above.trim().is_empty() || indent_columns(above) >= 4 || block_boundary(above)
        }
    }
}

/// Whether `line` closes the block before it: an ATX heading, a thematic break,
/// a quote or a fence opener.
fn block_boundary(line: &str) -> bool {
    let trimmed = line.trim_start();
    let hashes = trimmed.bytes().take_while(|&byte| byte == b'#').count();
    let heading = hashes > 0
        && hashes <= 6
        && (hashes == trimmed.len() || matches!(trimmed.as_bytes().get(hashes), Some(b' ' | b'\t')));
    if heading || trimmed.starts_with('>') || fence_open(trimmed).is_some() {
        return true;
    }
    let run = trimmed.bytes().take_while(|byte| matches!(byte, b'-' | b'*' | b'_')).count();
    run >= 3 && trimmed.bytes().all(|byte| matches!(byte, b'-' | b'*' | b'_') || byte == b' ' || byte == b'\t')
}

/// The content indent a list item line establishes for its children.
///
/// A quote is not a container here: its continuation lines carry `>` again, so
/// an indented line after one is a new block (indented code).
fn container_content_indent(line: &str) -> Option<usize> {
    let indent = indent_columns(line);
    let byte_indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let trimmed = line.trim_start();
    let end = byte_indent + trimmed.len();
    list_prefix(line, byte_indent, end).map(|(content, _)| indent + (content - byte_indent))
}

/// Visual columns of leading whitespace, with a tab taking four.
fn indent_columns(line: &str) -> usize {
    let mut columns = 0usize;
    for ch in line.chars() {
        match ch {
            ' ' => columns += 1,
            '\t' => columns += 4,
            _ => break,
        }
    }
    columns
}

/// Moves the caret to a line from CodeMirror's go-to-line grammar (E-31):
/// `40`, `+5`, `-2`, `50%`, `12:4`. Returns the zero-based line reached.
pub fn goto_line(edit: &mut TextEdit, spec: &str) -> Option<usize> {
    let text = edit.text.clone();
    let map = LineMap::new(&text);
    let current = map.index_of(edit.focus);
    let (line, caret) = goto_target_offset(&text, spec, current)?;
    edit.set_caret(caret);
    edit.break_history_group();
    Some(line)
}

/// [`goto_line`]'s grammar on plain text, for the block editor (E-31).
///
/// `current` is the zero-based line of the caret in `text`. Returns the reached
/// zero-based line and the byte offset of its `line:col` caret (the line start
/// without a column). The same `+N`, `-N`, `N%` and `line:col` forms as the
/// single field; a caret-only move, with no history.
pub fn goto_target_offset(text: &str, spec: &str, current: usize) -> Option<(usize, usize)> {
    let map = LineMap::new(text);
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    let (line, column) = parse_goto(spec, map.count(), current)?;
    let line = line.min(map.count().saturating_sub(1));
    let start = map.start(line);
    let end = map.end(line);
    let mut caret = start;
    if let Some(column) = column {
        for (characters, (offset, ch)) in text[start..end].char_indices().enumerate() {
            if characters >= column {
                break;
            }
            caret = start + offset + ch.len_utf8();
        }
    }
    Some((line, caret))
}

/// `Ctrl+D` (E-29): select the word at the caret, or the next occurrence of the
/// selected text, wrapping at the end of the document.
///
/// The pinned Rune field has one selection, so this is the next occurrence as a
/// single selection: the first press selects the word under the caret, each
/// press after it moves the selection to the following occurrence. Adding a
/// second cursor (`Ctrl+Alt+↑/↓`) needs Rune's multi-cursor model and is
/// recorded as a gap rather than faked here.
pub fn select_next_occurrence(edit: &mut TextEdit) -> bool {
    let text = edit.text.clone();
    let (from, to) = edit.range();
    let (from, to) = (from.min(to), from.max(to));
    let (needle, origin) = if from == to {
        // The first press selects the word under the caret.
        match word_at(&text, from) {
            Some((start, end)) => {
                edit.set_selection(start, end);
                edit.break_history_group();
                return true;
            }
            None => return false,
        }
    } else {
        let selected = text.get(from..to).unwrap_or("");
        if selected.trim().is_empty() || selected.contains('\n') {
            return false;
        }
        (selected.to_owned(), from)
    };
    let after = origin.saturating_add(needle.len());
    let Some(found) =
        next_occurrence(&text, &needle, after, text.len()).or_else(|| next_occurrence(&text, &needle, 0, origin))
    else {
        return false;
    };
    if found.start == from && found.end == to {
        return false;
    }
    edit.set_selection(found.start, found.end);
    edit.break_history_group();
    true
}

/// The first whole-word occurrence of `needle` at or after `from`, before `to`.
fn next_occurrence(text: &str, needle: &str, from: usize, to: usize) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let to = to.min(text.len());
    let mut at = from.min(to);
    while at <= to {
        let offset = at + text.get(at..to)?.find(needle)?;
        let end = offset + needle.len();
        let first_word = needle.chars().next().is_some_and(is_word);
        let last_word = needle.chars().next_back().is_some_and(is_word);
        let starts_whole = !text[..offset].chars().next_back().is_some_and(is_word);
        let ends_whole = !text[end..].chars().next().is_some_and(is_word);
        if (!first_word || starts_whole) && (!last_word || ends_whole) {
            return Some(offset..end);
        }
        at = end.max(offset + 1);
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListKind {
    Bullet,
    Ordered,
    Task,
}

fn heading_prefix(map: &LineMap<'_>, line: usize) -> Option<(usize, usize)> {
    let start = map.indent_end(line);
    let rest = &map.text[start..map.end(line)];
    let hashes = rest.bytes().take_while(|&byte| byte == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    if hashes == rest.len() {
        return Some((hashes, hashes));
    }
    if matches!(rest.as_bytes()[hashes], b' ' | b'\t') {
        let mut take = hashes;
        while take < rest.len() && matches!(rest.as_bytes()[take], b' ' | b'\t') {
            take += 1;
        }
        return Some((hashes, take));
    }
    None
}

fn list_prefix(text: &str, start: usize, end: usize) -> Option<(usize, ListKind)> {
    let rest = &text[start..end];
    let bytes = rest.as_bytes();
    if bytes.len() >= 6
        && matches!(bytes[0], b'-' | b'*' | b'+')
        && bytes[1] == b' '
        && bytes[2] == b'['
        && matches!(bytes[3], b' ' | b'x' | b'X')
        && bytes[4] == b']'
        && bytes[5] == b' '
    {
        return Some((start + 6, ListKind::Task));
    }
    let mut digits = 0;
    while digits < bytes.len() && bytes[digits].is_ascii_digit() {
        digits += 1;
    }
    if digits > 0 && matches!(bytes.get(digits), Some(b'.') | Some(b')')) && bytes.get(digits + 1) == Some(&b' ') {
        return Some((start + digits + 2, ListKind::Ordered));
    }
    if bytes.len() >= 2 && matches!(bytes[0], b'-' | b'*' | b'+') && bytes[1] == b' ' {
        return Some((start + 2, ListKind::Bullet));
    }
    None
}

fn quote_prefix(text: &str, start: usize, end: usize) -> Option<usize> {
    if start >= end || text.as_bytes()[start] != b'>' {
        return None;
    }
    let mut offset = start + 1;
    if offset < end && text.as_bytes()[offset] == b' ' {
        offset += 1;
    }
    Some(offset)
}

fn word_at(text: &str, at: usize) -> Option<(usize, usize)> {
    let at = at.min(text.len());
    let mut start = at;
    while let Some((offset, ch)) = text[..start].char_indices().next_back() {
        if is_word(ch) {
            start = offset;
        } else {
            break;
        }
    }
    let mut end = at;
    for (offset, ch) in text[at..].char_indices() {
        if is_word(ch) {
            end = at + offset + ch.len_utf8();
        } else {
            break;
        }
    }
    (start < end).then_some((start, end))
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn run_len_before(text: &str, at: usize, ch: char) -> usize {
    let mut len = 0;
    for found in text[..at].chars().rev() {
        if found != ch {
            break;
        }
        len += 1;
    }
    len
}

fn run_len_after(text: &str, at: usize, ch: char) -> usize {
    let mut len = 0;
    for found in text[at..].chars() {
        if found != ch {
            break;
        }
        len += 1;
    }
    len
}

fn wrapped_by(text: &str, from: usize, to: usize, marker: &str) -> bool {
    let width = marker.len();
    if from < width || to + width > text.len() {
        return false;
    }
    if marker == "*" || marker == "**" {
        let before = run_len_before(text, from, '*');
        let after = run_len_after(text, to, '*');
        if marker == "*" { before % 2 == 1 && after % 2 == 1 } else { before >= 2 && after >= 2 }
    } else {
        text[..from].ends_with(marker) && text[to..].starts_with(marker)
    }
}

fn is_bare_url(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let prefix = ["http://", "https://", "mailto:", "www."].into_iter().find(|candidate| lower.starts_with(candidate));
    prefix.is_some_and(|prefix| text.len() > prefix.len() && !text.chars().any(char::is_whitespace))
}

fn render_template(template: &str) -> (String, Option<(usize, usize)>) {
    if let Some(open) = template.find("{{")
        && let Some(close) = template[open + 2..].find("}}").map(|offset| open + 2 + offset)
    {
        let body = format!("{}{}{}", &template[..open], &template[open + 2..close], &template[close + 2..]);
        return (body, Some((open, close - 2)));
    }
    (template.to_owned(), None)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommentStyle {
    Line(&'static str),
    Block { open: &'static str, close: &'static str },
}

fn comment_style(language: &str) -> Option<CommentStyle> {
    let language = language.trim().to_ascii_lowercase();
    match language.as_str() {
        "rust" | "rs" | "c" | "h" | "cpp" | "c++" | "cc" | "cxx" | "hpp" | "cs" | "c#" | "java" | "js"
        | "javascript" | "jsx" | "ts" | "typescript" | "tsx" | "go" | "golang" | "kotlin" | "kt" | "swift" | "dart"
        | "scala" | "groovy" | "php" | "objc" | "objective-c" | "objectivec" | "jsonc" => {
            Some(CommentStyle::Line("//"))
        }
        "py" | "python" | "rb" | "ruby" | "sh" | "bash" | "zsh" | "fish" | "shell" | "yaml" | "yml" | "toml"
        | "perl" | "pl" | "r" | "makefile" | "make" | "dockerfile" | "ini" | "conf" | "nix" | "elixir" | "ex"
        | "julia" | "jl" | "powershell" | "ps1" | "awk" | "cmake" => Some(CommentStyle::Line("#")),
        "sql" | "lua" | "haskell" | "hs" | "ada" | "vhdl" => Some(CommentStyle::Line("--")),
        "mermaid" => Some(CommentStyle::Line("%%")),
        "lisp" | "clojure" | "clj" | "scheme" | "asm" | "nasm" => Some(CommentStyle::Line(";")),
        "latex" | "tex" | "erlang" | "erl" | "matlab" | "octave" => Some(CommentStyle::Line("%")),
        "vb" | "visualbasic" => Some(CommentStyle::Line("'")),
        "html" | "htm" | "xml" | "svg" | "markdown" | "md" | "vue" | "svelte" => {
            Some(CommentStyle::Block { open: "<!--", close: "-->" })
        }
        "css" | "scss" | "less" | "stylus" => Some(CommentStyle::Block { open: "/*", close: "*/" }),
        _ => None,
    }
}

fn fence_language_at(map: &LineMap<'_>, target: usize) -> Option<String> {
    let mut open: Option<(char, usize, String)> = None;
    for line in 0..=target {
        let trimmed = map.line_text(line).trim_start();
        if let Some((marker, run, language)) = &open {
            if line == target {
                return Some(language.clone());
            }
            if is_fence_close(trimmed, *marker, *run) {
                open = None;
            }
        } else if let Some((marker, run, info)) = fence_open(trimmed) {
            let language = info.split_whitespace().next().unwrap_or("").to_owned();
            if line == target {
                return Some(language);
            }
            open = Some((marker, run, language));
        } else if line == target {
            return None;
        }
    }
    None
}

fn line_comment_changes(text: &str, map: &LineMap<'_>, first: usize, last: usize, marker: &str) -> Vec<Change> {
    let targets: Vec<usize> = (first..=last).filter(|&line| !map.line_text(line).trim().is_empty()).collect();
    if targets.is_empty() {
        return Vec::new();
    }
    let all_commented = targets.iter().all(|&line| {
        let start = map.indent_end(line);
        text[start..map.end(line)].starts_with(marker)
    });
    let mut changes = Vec::new();
    for &line in &targets {
        let start = map.indent_end(line);
        if all_commented {
            let mut take = marker.len();
            if text.as_bytes().get(start + take) == Some(&b' ') {
                take += 1;
            }
            changes.push(Change::new(start..start + take, ""));
        } else if !text[start..map.end(line)].starts_with(marker) {
            changes.push(Change::new(start..start, format!("{marker} ")));
        }
    }
    changes
}

fn block_comment_changes(
    text: &str,
    map: &LineMap<'_>,
    first: usize,
    last: usize,
    open: &str,
    close: &str,
) -> Vec<Change> {
    let open_pos = map.indent_end(first);
    let close_pos = map.end(last);
    if open_pos > close_pos || close_pos > text.len() {
        return Vec::new();
    }

    let has_open = text[open_pos..map.end(first)].starts_with(open);
    let has_close = text[..close_pos].ends_with(close);
    if has_open && has_close {
        let mut open_end = open_pos + open.len();
        if text.as_bytes().get(open_end) == Some(&b' ') {
            open_end += 1;
        }
        let mut close_start = close_pos - close.len();
        if close_start > 0 && text.as_bytes()[close_start - 1] == b' ' {
            close_start -= 1;
        }
        close_start = close_start.max(open_end);
        if open_end <= close_start {
            return vec![Change::new(open_pos..open_end, ""), Change::new(close_start..close_pos, "")];
        }
    }

    // When the line is empty both insertions land on the same byte, so the
    // closing marker goes first: applying it before the opening one leaves the
    // opening marker to its left.
    vec![Change::new(close_pos..close_pos, close.to_owned()), Change::new(open_pos..open_pos, open.to_owned())]
}

fn parse_goto(spec: &str, line_count: usize, current: usize) -> Option<(usize, Option<usize>)> {
    if let Some(percent) = spec.strip_suffix('%') {
        let value: f64 = percent.trim().parse().ok()?;
        if !value.is_finite() || !(0.0..=100.0).contains(&value) {
            return None;
        }
        let line = ((value / 100.0) * line_count.saturating_sub(1) as f64).round() as usize;
        return Some((line, None));
    }

    let (line_part, column_part) = match spec.split_once(':') {
        Some((line, column)) => (line, Some(column)),
        None => (spec, None),
    };
    let column = match column_part {
        Some(column) => Some(column.trim().parse::<usize>().ok()?.saturating_sub(1)),
        None => None,
    };
    let (relative, digits) = if let Some(rest) = line_part.strip_prefix('+') {
        (Some(1isize), rest)
    } else if let Some(rest) = line_part.strip_prefix('-') {
        (Some(-1isize), rest)
    } else {
        (None, line_part)
    };
    let number: usize = digits.trim().parse().ok()?;
    let line = match relative {
        Some(sign) => (current as isize + sign * number as isize).max(0) as usize,
        None => number.saturating_sub(1),
    };
    Some((line, column))
}

fn map_offset(offset: usize, changes: &[Change]) -> usize {
    let mut delta = 0isize;
    for change in changes {
        let start = change.range.start;
        let end = change.range.end;
        let inserted = change.insert.len() as isize;
        if offset < start {
            break;
        }
        if offset < end {
            return (start as isize + delta + inserted).max(0) as usize;
        }
        delta += inserted - (end as isize - start as isize);
    }
    (offset as isize + delta).max(0) as usize
}

fn map_selection(selection: (usize, usize), changes: &[Change]) -> (usize, usize) {
    (map_offset(selection.0, changes), map_offset(selection.1, changes))
}

/// Sorts, applies and maps the selection through the changes.
fn apply_with_selection(edit: &mut TextEdit, mut changes: Vec<Change>) -> bool {
    if changes.is_empty() {
        return false;
    }
    changes.sort_by_key(|change| change.range.start);
    let selection = map_selection(edit.range(), &changes);
    apply_changes(edit, &changes, Some(selection))
}

fn apply_changes(edit: &mut TextEdit, changes: &[Change], selection: Option<(usize, usize)>) -> bool {
    let len = edit.text.len();
    let mut ordered: Vec<&Change> =
        changes.iter().filter(|change| change.range.start <= change.range.end && change.range.end <= len).collect();
    if ordered.is_empty() {
        return false;
    }
    let effective = ordered.iter().any(|change| edit.text.get(change.range.clone()) != Some(change.insert.as_str()));
    if !effective {
        return false;
    }
    ordered.sort_by(|a, b| b.range.start.cmp(&a.range.start).then(b.range.end.cmp(&a.range.end)));
    edit.undo_group(|edit| {
        for change in &ordered {
            edit.set_selection(change.range.start, change.range.end);
            edit.insert(&change.insert);
        }
        if let Some((from, to)) = selection {
            edit.set_selection(from, to);
        }
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(text: &str, caret: usize) -> TextEdit {
        let mut edit = TextEdit::new(text);
        edit.set_caret(caret);
        edit
    }

    fn selected(edit: &TextEdit) -> (usize, usize) {
        edit.range()
    }

    #[test]
    fn bold_toggle_wraps_and_unwraps_in_one_undo_step() {
        let mut text = edit("word", 0);
        text.set_selection(0, 4);
        assert!(toggle_wrap(&mut text, "**"));
        assert_eq!(text.text, "**word**");
        assert_eq!(selected(&text), (2, 6));
        text.undo();
        assert_eq!(text.text, "word");
        text.redo();
        assert_eq!(text.text, "**word**");

        text.set_selection(2, 6);
        assert!(toggle_wrap(&mut text, "**"));
        assert_eq!(text.text, "word");
        assert_eq!(selected(&text), (0, 4));
    }

    #[test]
    fn selecting_the_markers_strips_them() {
        let mut text = TextEdit::new("**bold**");
        text.set_selection(0, 8);
        assert!(toggle_wrap(&mut text, "**"));
        assert_eq!(text.text, "bold");
        assert_eq!(selected(&text), (0, 4));
    }

    #[test]
    fn whitespace_stays_outside_the_markers() {
        let mut text = TextEdit::new(" a ");
        text.set_selection(0, 3);
        assert!(toggle_wrap(&mut text, "**"));
        assert_eq!(text.text, " **a** ");
        assert_eq!(selected(&text), (3, 4));
        assert!(toggle_wrap(&mut text, "**"));
        assert_eq!(text.text, " a ");
    }

    #[test]
    fn word_under_caret_is_wrapped() {
        let mut text = edit("hello world", 2);
        assert!(toggle_wrap(&mut text, "*"));
        assert_eq!(text.text, "*hello* world");
        assert_eq!(selected(&text), (1, 6));
        assert!(toggle_wrap(&mut text, "*"));
        assert_eq!(text.text, "hello world");
    }

    #[test]
    fn italic_and_bold_runs_do_not_eat_each_other() {
        let mut text = edit("***x***", 3);
        text.set_selection(3, 4);
        assert!(toggle_wrap(&mut text, "*"));
        assert_eq!(text.text, "**x**");
        assert_eq!(selected(&text), (2, 3));

        let mut bold = TextEdit::new("**x**");
        bold.set_selection(2, 3);
        assert!(toggle_wrap(&mut bold, "**"));
        assert_eq!(bold.text, "x");

        let mut italic = TextEdit::new("*x*");
        italic.set_selection(1, 2);
        assert!(toggle_wrap(&mut italic, "*"));
        assert_eq!(italic.text, "x");
    }

    #[test]
    fn strike_and_code_markers_round_trip() {
        let mut strike = TextEdit::new("gone");
        strike.set_selection(0, 4);
        assert!(toggle_wrap(&mut strike, "~~"));
        assert_eq!(strike.text, "~~gone~~");
        assert!(toggle_wrap(&mut strike, "~~"));
        assert_eq!(strike.text, "gone");

        let mut code = TextEdit::new("call()");
        code.set_selection(0, 6);
        assert!(toggle_wrap(&mut code, "`"));
        assert_eq!(code.text, "`call()`");
        assert!(toggle_wrap(&mut code, "`"));
        assert_eq!(code.text, "call()");
    }

    #[test]
    fn empty_selection_without_a_word_inserts_an_empty_pair() {
        let mut text = edit("", 0);
        assert!(toggle_wrap(&mut text, "~~"));
        assert_eq!(text.text, "~~~~");
        assert_eq!(selected(&text), (2, 2));
    }

    #[test]
    fn headings_set_replace_and_remove() {
        let mut text = edit("text\n", 0);
        assert!(set_heading(&mut text, 1));
        assert_eq!(text.text, "# text\n");
        assert_eq!(selected(&text), (2, 2));
        assert!(set_heading(&mut text, 1));
        assert_eq!(text.text, "text\n");

        assert!(set_heading(&mut text, 2));
        assert_eq!(text.text, "## text\n");
        assert!(set_heading(&mut text, 3));
        assert_eq!(text.text, "### text\n");
        assert!(set_heading(&mut text, 0));
        assert_eq!(text.text, "text\n");
    }

    #[test]
    fn headings_cover_selected_lines_and_keep_indentation() {
        let mut text = TextEdit::new("  one\n    two\n");
        text.set_selection(0, text.text.len() - 1);
        assert!(set_heading(&mut text, 2));
        assert_eq!(text.text, "  ## one\n    ## two\n");
        assert!(set_heading(&mut text, 2));
        assert_eq!(text.text, "  one\n    two\n");
    }

    #[test]
    fn headings_ignore_non_atx_hash_runs() {
        let mut text = edit("####### seven\n#tag\n", 0);
        assert!(set_heading(&mut text, 1));
        assert_eq!(text.text, "# ####### seven\n#tag\n");
    }

    #[test]
    fn heading_levels_above_six_clamp() {
        let mut text = edit("text\n", 0);
        assert!(set_heading(&mut text, 9));
        assert_eq!(text.text, "###### text\n");
    }

    #[test]
    fn bullet_quote_and_task_round_trip() {
        let mut text = TextEdit::new("  one\n    two\n");
        text.set_selection(0, text.text.len());
        assert!(toggle_line_prefix(&mut text, LinePrefix::Bullet));
        assert_eq!(text.text, "  - one\n    - two\n");
        assert!(toggle_line_prefix(&mut text, LinePrefix::Bullet));
        assert_eq!(text.text, "  one\n    two\n");

        assert!(toggle_line_prefix(&mut text, LinePrefix::Task));
        assert_eq!(text.text, "  - [ ] one\n    - [ ] two\n");
        assert!(toggle_line_prefix(&mut text, LinePrefix::Task));
        assert_eq!(text.text, "  one\n    two\n");

        assert!(toggle_line_prefix(&mut text, LinePrefix::Quote));
        assert_eq!(text.text, ">   one\n>     two\n");
        assert!(toggle_line_prefix(&mut text, LinePrefix::Quote));
        assert_eq!(text.text, "  one\n    two\n");
    }

    #[test]
    fn ordered_lists_renumber_and_convert() {
        let mut text = TextEdit::new("a\nb\nc\n");
        text.set_selection(0, text.text.len());
        assert!(toggle_line_prefix(&mut text, LinePrefix::Ordered));
        assert_eq!(text.text, "1. a\n2. b\n3. c\n");
        assert!(toggle_line_prefix(&mut text, LinePrefix::Ordered));
        assert_eq!(text.text, "a\nb\nc\n");

        assert!(toggle_line_prefix(&mut text, LinePrefix::Ordered));
        assert!(toggle_line_prefix(&mut text, LinePrefix::Task));
        assert_eq!(text.text, "- [ ] a\n- [ ] b\n- [ ] c\n");
    }

    #[test]
    fn empty_lines_are_untouched_when_content_lines_exist() {
        let mut text = TextEdit::new("a\n\nb\n");
        text.set_selection(0, text.text.len());
        assert!(toggle_line_prefix(&mut text, LinePrefix::Bullet));
        assert_eq!(text.text, "- a\n\n- b\n");
        assert!(toggle_line_prefix(&mut text, LinePrefix::Bullet));
        assert_eq!(text.text, "a\n\nb\n");
    }

    #[test]
    fn links_cover_url_text_and_empty_selections() {
        let mut text = edit("", 0);
        assert!(insert_link(&mut text));
        assert_eq!(text.text, "[link text](url)");
        assert_eq!(selected(&text), (1, 10));

        let mut bare = TextEdit::new("https://example.test/x");
        bare.set_selection(0, bare.text.len());
        assert!(insert_link(&mut bare));
        assert_eq!(bare.text, "[](https://example.test/x)");
        assert_eq!(selected(&bare), (1, 1));

        let mut label = TextEdit::new("abc");
        label.set_selection(0, 3);
        assert!(insert_link(&mut label));
        assert_eq!(label.text, "[abc](url)");
        assert_eq!(selected(&label), (6, 9));
    }

    #[test]
    fn table_block_replaces_an_empty_line_and_selects_the_placeholder() {
        let mut text = edit("", 0);
        assert!(BlockKind::Table.insert(&mut text));
        assert!(text.text.starts_with("| Column | Column | Column |\n| --- |"));
        assert_eq!(&text.text[selected(&text).0..selected(&text).1], "Column");
        text.undo();
        assert_eq!(text.text, "");
    }

    #[test]
    fn blocks_after_content_get_blank_lines_around_them() {
        let mut text = edit("x\n", 1);
        assert!(BlockKind::Divider.insert(&mut text));
        assert_eq!(text.text, "x\n\n---\n\n");

        let mut between = TextEdit::new("x\ny\n");
        between.set_caret(1);
        assert!(BlockKind::Note.insert(&mut between));
        assert_eq!(between.text, "x\n\n> [!NOTE]\n> Useful information.\n\ny\n");
        let (from, to) = selected(&between);
        assert_eq!(&between.text[from..to], "Useful information.");
    }

    #[test]
    fn block_templates_select_their_placeholders() {
        for kind in BlockKind::ALL {
            let mut text = edit("", 0);
            assert!(kind.insert(&mut text), "{kind:?}");
            let (from, to) = selected(&text);
            assert!(from <= to && to <= text.text.len());
            if kind != BlockKind::Divider {
                assert!(from < to, "{kind:?} has a placeholder");
            }
        }
    }

    #[test]
    fn moving_lines_up_and_down_keeps_the_selection() {
        let mut text = edit("a\nb\n", 2);
        assert!(move_line_up(&mut text));
        assert_eq!(text.text, "b\na\n");
        assert_eq!(selected(&text), (0, 0));
        assert!(!move_line_up(&mut text));

        assert!(move_line_down(&mut text));
        assert_eq!(text.text, "a\nb\n");
        assert_eq!(selected(&text), (2, 2));

        let mut last = edit("a\nb", 2);
        assert!(!move_line_down(&mut last));
    }

    #[test]
    fn moving_a_multiline_block_up_keeps_its_lines() {
        let mut text = TextEdit::new("a\nb\nc\n");
        text.set_selection(2, 3);
        assert!(move_line_up(&mut text));
        assert_eq!(text.text, "b\na\nc\n");
        assert_eq!(selected(&text), (0, 1));
    }

    #[test]
    fn moving_the_last_line_down_over_a_blank_line_works() {
        let mut text = edit("a\n\nb\n", 0);
        assert!(move_line_down(&mut text));
        assert_eq!(text.text, "\na\nb\n");
        assert_eq!(selected(&text), (1, 1));
    }

    #[test]
    fn duplicate_and_delete_lines_cover_the_final_newline() {
        let mut text = edit("a\nb\n", 0);
        assert!(duplicate_line(&mut text));
        assert_eq!(text.text, "a\na\nb\n");
        assert_eq!(selected(&text), (2, 2));
        text.undo();
        assert_eq!(text.text, "a\nb\n");

        let mut no_newline = edit("a\nb", 2);
        assert!(duplicate_line(&mut no_newline));
        assert_eq!(no_newline.text, "a\nb\nb");
        assert_eq!(selected(&no_newline), (4, 4));

        let mut delete = TextEdit::new("a\nb\nc\n");
        delete.set_caret(2);
        assert!(delete_line(&mut delete));
        assert_eq!(delete.text, "a\nc\n");
        assert_eq!(selected(&delete), (2, 2));

        let mut last = edit("a\nb", 2);
        assert!(delete_line(&mut last));
        assert_eq!(last.text, "a");
        assert_eq!(selected(&last), (1, 1));

        let mut last_newline = TextEdit::new("a\nb\n");
        last_newline.set_caret(2);
        assert!(delete_line(&mut last_newline));
        assert_eq!(last_newline.text, "a\n");

        let mut blank = TextEdit::new("a\n\nb\n");
        blank.set_caret(2);
        assert!(delete_line(&mut blank));
        assert_eq!(blank.text, "a\nb\n");
    }

    #[test]
    fn deleting_a_multi_line_selection_removes_every_touched_line() {
        let mut text = TextEdit::new("a\nb\nc\n");
        text.set_selection(0, 3);
        assert!(delete_line(&mut text));
        assert_eq!(text.text, "c\n");
        assert_eq!(selected(&text), (0, 0));
    }

    #[test]
    fn select_line_expands_to_the_touched_lines() {
        let mut text = edit("abc\ndef\n", 1);
        assert!(select_line(&mut text));
        assert_eq!(selected(&text), (0, 3));
        assert!(!select_line(&mut text));

        text.set_selection(1, 6);
        assert!(select_line(&mut text));
        assert_eq!(selected(&text), (0, 7));
    }

    #[test]
    fn html_comments_wrap_and_unwrap() {
        let mut text = TextEdit::new("a\nb\n");
        text.set_selection(0, text.text.len());
        assert!(toggle_comment(&mut text));
        assert_eq!(text.text, "<!--a\nb\n-->");
        assert!(toggle_comment(&mut text));
        assert_eq!(text.text, "a\nb\n");

        let mut indented = edit("  abc\n", 2);
        assert!(toggle_comment(&mut indented));
        assert_eq!(indented.text, "  <!--abc-->\n");
        assert!(toggle_comment(&mut indented));
        assert_eq!(indented.text, "  abc\n");
    }

    #[test]
    fn empty_and_spaced_html_comments_round_trip() {
        let mut empty = edit("", 0);
        assert!(toggle_comment(&mut empty));
        assert_eq!(empty.text, "<!---->");
        assert!(toggle_comment(&mut empty));
        assert_eq!(empty.text, "");

        let mut spaced = TextEdit::new("<!-- -->");
        spaced.set_selection(0, 8);
        assert!(toggle_comment(&mut spaced));
        assert_eq!(spaced.text, "");
    }

    #[test]
    fn css_fences_use_a_block_comment() {
        let mut css = TextEdit::new("```css\na {}\n```\n");
        css.set_caret(9);
        assert!(toggle_comment(&mut css));
        assert_eq!(css.text, "```css\n/*a {}*/\n```\n");
        assert!(toggle_comment(&mut css));
        assert_eq!(css.text, "```css\na {}\n```\n");
    }

    #[test]
    fn fenced_code_uses_its_language_comment() {
        let mut rust = TextEdit::new("```rust\nlet x = 1;\n```\n");
        rust.set_caret(10);
        assert!(toggle_comment(&mut rust));
        assert_eq!(rust.text, "```rust\n// let x = 1;\n```\n");
        assert!(toggle_comment(&mut rust));
        assert_eq!(rust.text, "```rust\nlet x = 1;\n```\n");

        let mut python = TextEdit::new("```python\nx = 1\n```\n");
        python.set_caret(12);
        assert!(toggle_comment(&mut python));
        assert_eq!(python.text, "```python\n# x = 1\n```\n");

        let mut mermaid = TextEdit::new("```mermaid\nflowchart LR\n```\n");
        mermaid.set_caret(14);
        assert!(toggle_comment(&mut mermaid));
        assert_eq!(mermaid.text, "```mermaid\n%% flowchart LR\n```\n");

        let mut unknown = TextEdit::new("```wat\nx\n```\n");
        unknown.set_caret(8);
        assert!(!toggle_comment(&mut unknown));
    }

    #[test]
    fn gotos_accept_absolute_relative_percentage_and_column_forms() {
        let mut text = edit("one\ntwo\nthree", 0);
        assert_eq!(goto_line(&mut text, "2"), Some(1));
        assert_eq!(text.range(), (4, 4));

        assert_eq!(goto_line(&mut text, "+1"), Some(2));
        assert_eq!(goto_line(&mut text, "-1"), Some(1));
        assert_eq!(goto_line(&mut text, "50%"), Some(1));
        assert_eq!(goto_line(&mut text, "3:2"), Some(2));
        assert_eq!(text.range(), (9, 9));
        assert_eq!(goto_line(&mut text, "1:99"), Some(0));
        assert_eq!(text.range(), (3, 3));
        assert_eq!(goto_line(&mut text, "nope"), None);
        assert_eq!(goto_line(&mut text, "101%"), None);
        assert_eq!(goto_line(&mut text, "999"), Some(2));
    }

    #[test]
    fn unknown_fences_and_indented_code_are_documented_no_ops() {
        // OI-060: an unknown fence language has no comment syntax here; the
        // command leaves the code untouched rather than inserting `<!-- -->`
        // into it as literal code.
        let mut unknown = TextEdit::new("```wat\nx\n```\n");
        unknown.set_caret(8);
        assert!(!toggle_comment(&mut unknown));
        assert_eq!(unknown.text, "```wat\nx\n```\n");

        // Indented code has no language either: a Markdown comment would sit
        // inside the indented block as code.
        let mut code = TextEdit::new("prose\n\n    let x = 1;\n    let y = 2;\n");
        code.set_caret(13);
        assert!(!toggle_comment(&mut code));
        assert_eq!(code.text, "prose\n\n    let x = 1;\n    let y = 2;\n");

        // A lazy paragraph continuation is not code, so its line still comments.
        let mut lazy = TextEdit::new("prose\n    more prose\n");
        lazy.set_caret(8);
        assert!(toggle_comment(&mut lazy));
        assert_eq!(lazy.text, "prose\n    <!--more prose-->\n");

        // A nested list item's paragraph is not indented code either.
        let mut nested = TextEdit::new("- item\n\n    paragraph in the item\n");
        nested.set_caret(10);
        assert!(toggle_comment(&mut nested));
        assert_eq!(nested.text, "- item\n\n    <!--paragraph in the item-->\n");

        // Four columns deeper under a list item *is* code.
        let mut in_list = TextEdit::new("- item\n\n        code in the item\n");
        in_list.set_caret(10);
        assert!(!toggle_comment(&mut in_list));
        assert_eq!(in_list.text, "- item\n\n        code in the item\n");

        // A heading, a rule or a quote ends a block, so an indented line after
        // one is code even without a blank line.
        for text in ["# Heading\n    code\n", "---\n    code\n", "> quote\n    code\n"] {
            let mut edit = TextEdit::new(text);
            edit.set_caret(text.find("code").unwrap());
            assert!(!toggle_comment(&mut edit), "{text:?}");
            assert_eq!(edit.text, text);
        }
    }

    #[test]
    fn goto_target_offset_speaks_the_same_grammar_as_the_single_field() {
        let text = "one\ntwo\nthree";
        assert_eq!(goto_target_offset(text, "2", 0), Some((1, 4)));
        assert_eq!(goto_target_offset(text, "+1", 0), Some((1, 4)));
        assert_eq!(goto_target_offset(text, "-1", 2), Some((1, 4)));
        assert_eq!(goto_target_offset(text, "50%", 0), Some((1, 4)));
        assert_eq!(goto_target_offset(text, "3:2", 0), Some((2, 9)));
        assert_eq!(goto_target_offset(text, "1:99", 0), Some((0, 3)), "a column past the line clamps to its end");
        assert_eq!(goto_target_offset(text, "12:1", 0), Some((2, 8)), "a line past the end clamps");
        assert_eq!(goto_target_offset(text, "nope", 0), None);
        assert_eq!(goto_target_offset(text, "", 0), None);
    }

    #[test]
    fn ctrl_d_selects_the_word_then_its_next_occurrence() {
        let mut text = edit("one budget two budget three", 6);
        assert!(select_next_occurrence(&mut text));
        assert_eq!(text.range(), (4, 10), "the caret's word is selected first");
        assert!(select_next_occurrence(&mut text));
        assert_eq!(text.range(), (15, 21), "the next whole occurrence");
        assert!(select_next_occurrence(&mut text));
        assert_eq!(text.range(), (4, 10), "it wraps at the end");
        let mut blank = edit("   ", 2);
        assert!(!select_next_occurrence(&mut blank), "no word under the caret");

        // A selection goes to the next occurrence of exactly that text.
        let mut selected = TextEdit::new("a one b one c");
        selected.set_selection(2, 5);
        assert!(select_next_occurrence(&mut selected));
        assert_eq!(selected.range(), (8, 11));

        // Inside a longer word the whole-word pass skips it.
        let mut longer = edit("budget budgets budget", 2);
        assert!(select_next_occurrence(&mut longer));
        assert_eq!(longer.range(), (0, 6));
        assert!(select_next_occurrence(&mut longer));
        assert_eq!(longer.range(), (15, 21), "`budgets` is not the next whole word");
    }

    #[test]
    fn every_format_command_is_one_undo_step() {
        let mut text = TextEdit::new("a\n\nb\n");
        text.set_selection(0, text.text.len());
        assert!(toggle_line_prefix(&mut text, LinePrefix::Ordered));
        assert_eq!(text.text, "1. a\n\n2. b\n");
        text.undo();
        assert_eq!(text.text, "a\n\nb\n");

        assert!(toggle_comment(&mut text));
        text.undo();
        assert_eq!(text.text, "a\n\nb\n");

        assert!(duplicate_line(&mut text));
        text.undo();
        assert_eq!(text.text, "a\n\nb\n");
    }
}
