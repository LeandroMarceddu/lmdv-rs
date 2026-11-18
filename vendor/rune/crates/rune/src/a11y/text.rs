//! Text inside the accessibility tree: a text field's lines as text runs with each
//! character's place, so a reader moves by character, word and line and a magnifier follows
//! the caret; a selectable text's lines likewise; and a text's inline links as link nodes.

use accesskit::{Action as Ak, Node as AkNode, NodeId as AkId, Rect as AkRect, Role, TextPosition, TextSelection};
use std::borrow::Cow;

use super::a11y_tree::{element_id, line_id, link_id, round};
use super::*;
use crate::text::{OBJECT_REPLACEMENT, TextFlow};

/// Text longer than this gets no per-character runs (a reader still gets its whole value). A field
/// past it marks its selection on itself, and the vendored adapters answer its lines from its value.
const RUN_MAX_BYTES: usize = 64 * 1024;

/// One laid-out line: its bytes in the drawn text and each character's place in points.
pub(crate) struct Line {
    pub range: std::ops::Range<usize>,
    pub rect: [f32; 4],
    pub lengths: Vec<u8>,
    pub positions: Vec<f32>,
    pub widths: Vec<f32>,
    pub word_starts: Vec<u8>,
    /// The line's words as a screen reader reads them, when an image placeholder in it speaks
    /// ([`InlineImage::label`](crate::text::InlineImage::label)); `None` is `range` of the
    /// drawn text as it is.
    pub spoken: Option<String>,
}

/// `content`'s lines from `layout` (device px at `s`), placed from `origin` in points. A
/// by-paragraph field's newline is the last character of the line it ends, as in one layout.
/// An image placeholder speaks its image's label (`labels`, sorted by byte), so a reader
/// hears the formula or picture instead of an object marker.
pub(crate) fn lines(
    layout: TextFlow<'_>,
    content: &str,
    labels: &[(usize, &str)],
    s: f32,
    origin: [f32; 2],
) -> Vec<Line> {
    let mut out = Vec::new();
    for p in layout.pieces() {
        let first = out.len();
        piece_lines(p, content, s, origin, &mut out);
        if p.is_whole() {
            continue;
        }
        if out.len() == first {
            out.push(Line {
                range: p.start..p.start,
                rect: [origin[0], origin[1] + p.y / s, 0.0, p.height / s],
                lengths: Vec::new(),
                positions: Vec::new(),
                widths: Vec::new(),
                word_starts: Vec::new(),
                spoken: None,
            });
        }
        if content.as_bytes().get(p.end) == Some(&b'\n')
            && let Some(l) = out.last_mut()
        {
            l.range.end += 1;
            l.lengths.push(1);
            l.positions.push(l.rect[2]);
            l.widths.push(0.0);
        }
    }
    speak_images(&mut out, content, labels);
    out
}

fn piece_lines(p: crate::text::Piece<'_>, content: &str, s: f32, origin: [f32; 2], out: &mut Vec<Line>) {
    for line in p.layout.lines() {
        let m = line.metrics();
        let range = p.content(line.text_range().start)..p.content(line.text_range().end);
        if range.end > content.len() {
            break;
        }
        let (mut lengths, mut positions, mut widths, mut word_starts) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut x = m.offset;
        let mut at = range.start;
        for run in line.runs() {
            for cluster in run.clusters() {
                let r = cluster.text_range();
                // A pill's isolate marks hold no content bytes.
                let r = p.content(r.start)..p.content(r.end);
                if r.start < at || r.end > range.end || r.is_empty() {
                    continue;
                }
                // Bytes a cluster skipped (an ignorable) join the character before.
                if r.start > at
                    && let Some(last) = lengths.last_mut()
                {
                    *last = (*last as usize + r.start - at).min(255) as u8;
                }
                if cluster.is_word_boundary() && !lengths.is_empty() && lengths.len() < 256 {
                    word_starts.push(lengths.len() as u8);
                }
                let adv = cluster.advance();
                lengths.push((r.end - r.start).min(255) as u8);
                positions.push((x - m.offset) / s);
                widths.push(adv / s);
                x += adv;
                at = r.end;
            }
        }
        if at < range.end {
            match lengths.last_mut() {
                Some(last) => *last = (*last as usize + range.end - at).min(255) as u8,
                None => {
                    lengths.push((range.end - at).min(255) as u8);
                    positions.push(0.0);
                    widths.push(0.0);
                }
            }
        }
        // AccessKit indexes word starts by u8: a line past 255 characters keeps the first ones.
        word_starts.retain(|&w| (w as usize) < lengths.len());
        let top = (p.y + m.block_min_coord) / s;
        let height = (m.block_max_coord - m.block_min_coord) / s;
        let rect = [origin[0] + m.offset / s, origin[1] + top, (x - m.offset) / s, height];
        out.push(Line { range, rect, lengths, positions, widths, word_starts, spoken: None });
    }
}

/// The line and character index of drawn byte `byte`.
fn position(lines: &[Line], byte: usize) -> Option<(usize, usize)> {
    let last = lines.len().checked_sub(1)?;
    let k = lines.iter().position(|l| byte < l.range.end).unwrap_or(last);
    let l = &lines[k];
    let mut at = l.range.start;
    for (c, len) in l.lengths.iter().enumerate() {
        if byte < at + *len as usize {
            return Some((k, c));
        }
        at += *len as usize;
    }
    Some((k, l.lengths.len()))
}

/// Drawn byte at line `k`'s character `c`.
pub(crate) fn byte_at(lines: &[Line], k: usize, c: usize) -> Option<usize> {
    let l = lines.get(k)?;
    Some(l.range.start + l.lengths.iter().take(c).map(|&n| n as usize).sum::<usize>())
}

/// The image spans' placeholders in `content`, each with the words its image speaks: its
/// label, or empty when it has none, so the object marker never reaches a reader.
pub(super) fn image_labels<'a>(spec: &'a crate::el::TextSpec, content: &str) -> Vec<(usize, &'a str)> {
    let mut out: Vec<(usize, &str)> = spec
        .spans
        .iter()
        .filter_map(|span| {
            let image = span.image.as_deref()?;
            let (a, b) = ((span.start as usize).min(content.len()), (span.end as usize).min(content.len()));
            let at = a + content.get(a..b)?.find(OBJECT_REPLACEMENT)?;
            Some((at, image.label.as_deref().unwrap_or("")))
        })
        .collect();
    out.sort_unstable_by_key(|(at, _)| *at);
    out.dedup_by_key(|(at, _)| *at);
    out
}

/// `text` with each placeholder in `labels` (sorted by byte) replaced by its words.
fn speak(text: &str, labels: &[(usize, &str)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for &(byte, words) in labels {
        let end = byte + OBJECT_REPLACEMENT.len_utf8();
        if byte < at || text.get(byte..end).is_none_or(|s| !s.starts_with(OBJECT_REPLACEMENT)) {
            continue;
        }
        out.push_str(&text[at..byte]);
        out.push_str(words);
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

/// Rewrites the lines that hold an image placeholder: the value a reader hears has the
/// image's words where the marker was, and its character lengths, positions and widths match
/// it. A line without one keeps its drawn text as it is.
fn speak_images(lines: &mut [Line], content: &str, labels: &[(usize, &str)]) {
    if labels.is_empty() {
        return;
    }
    for line in lines {
        let inside: Vec<(usize, &str)> = labels
            .iter()
            .copied()
            .filter(|&(at, _)| line.range.start <= at && at + OBJECT_REPLACEMENT.len_utf8() <= line.range.end)
            .collect();
        if inside.is_empty() {
            continue;
        }
        let Some(raw) = content.get(line.range.clone()) else { continue };
        let local: Vec<(usize, &str)> = inside.iter().map(|&(at, words)| (at - line.range.start, words)).collect();
        let spoken = speak(raw, &local);
        // Where each replacement's words land in `spoken`, and whether they start a word.
        let mut plans: Vec<(usize, &str, bool)> = Vec::with_capacity(local.len());
        let mut shift = 0isize;
        for &(byte, words) in &local {
            let at = (byte as isize + shift).max(0) as usize;
            let before = spoken.get(..at).and_then(|s| s.chars().next_back());
            let first = words.chars().next();
            let starts_word = first.is_some_and(|c| !c.is_whitespace()) && before.is_none_or(char::is_whitespace);
            plans.push((byte, words, starts_word));
            shift += words.len() as isize - OBJECT_REPLACEMENT.len_utf8() as isize;
        }
        // The placeholder's cluster becomes one entry per character of its words, zero-width
        // at the place the image draws; every other cluster keeps its own room.
        let mut lengths: Vec<u8> = Vec::with_capacity(line.lengths.len() + spoken.len());
        let mut positions: Vec<f32> = Vec::with_capacity(lengths.capacity());
        let mut widths: Vec<f32> = Vec::with_capacity(lengths.capacity());
        let mut places: Vec<usize> = Vec::with_capacity(line.lengths.len());
        let mut words_at: Vec<usize> = Vec::new();
        let mut at = 0usize;
        let mut next = 0usize;
        for (k, &len) in line.lengths.iter().enumerate() {
            let start = at;
            at += usize::from(len);
            let pos = line.positions.get(k).copied().unwrap_or(0.0);
            let width = line.widths.get(k).copied().unwrap_or(0.0);
            places.push(lengths.len());
            while next < plans.len() && plans[next].0 < start {
                next += 1;
            }
            if next < plans.len() && plans[next].0 == start {
                let (_, words, starts_word) = plans[next];
                next += 1;
                let mut prev: Option<char> = None;
                for (i, ch) in words.chars().enumerate() {
                    let starts = if i == 0 { starts_word } else { prev.is_some_and(char::is_whitespace) };
                    if starts && !ch.is_whitespace() && lengths.len() < 256 {
                        words_at.push(lengths.len());
                    }
                    lengths.push(ch.len_utf8() as u8);
                    positions.push(pos);
                    widths.push(0.0);
                    prev = Some(ch);
                }
            } else {
                lengths.push(len);
                positions.push(pos);
                widths.push(width);
            }
        }
        let mut starts: Vec<usize> =
            line.word_starts.iter().filter_map(|&w| places.get(usize::from(w)).copied()).collect();
        starts.extend(words_at);
        starts.sort_unstable();
        starts.dedup();
        starts.retain(|&i| i < lengths.len() && i < 256);
        if lengths.iter().map(|&n| usize::from(n)).sum::<usize>() == spoken.len() {
            line.word_starts = starts.into_iter().map(|i| i as u8).collect();
        } else {
            // A cluster merged the marker with its neighbours: keep the words a reader hears and
            // one entry per character, rather than places that disagree with the value.
            lengths = spoken.chars().map(|c| c.len_utf8() as u8).collect();
            positions = vec![line.positions.first().copied().unwrap_or(0.0); lengths.len()];
            widths = vec![0.0; lengths.len()];
            let mut starts = Vec::new();
            let mut prev: Option<char> = None;
            for (i, ch) in spoken.chars().enumerate() {
                if !ch.is_whitespace() && prev.is_none_or(char::is_whitespace) && i < 256 {
                    starts.push(i as u8);
                }
                prev = Some(ch);
            }
            line.word_starts = starts;
        }
        line.spoken = Some(spoken);
        line.lengths = lengths;
        line.positions = positions;
        line.widths = widths;
    }
}

impl<S: 'static> Runtime<S> {
    /// Node `i`'s text as AccessKit sees it: the lines of a field or selectable text, and the
    /// links of any text, pushed after it as its children.
    pub(super) fn a11y_text_nodes(&self, i: usize, w: &a11y_tree::Walk, out: &mut Vec<(AkId, AkNode)>) {
        let n = &self.nodes[i];
        if let Some(g) = n.vis.grid.as_deref() {
            return self.a11y_grid_rows(i, g, w, out);
        }
        let Some(spec) = n.text.as_deref() else { return };
        let me = out.len() - 1;
        let runs = n.input.is_some() || (spec.selectable && !n.is_secure());
        let found = self.a11y_lines(i).filter(|_| runs);
        // A field's rows that wait to be shaped have no lines yet: runs for only the shaped ones
        // would be a shorter document, so the field answers from its value instead, below.
        let found = found.filter(|(content, lines)| {
            n.input.is_none() || lines.iter().map(|l| l.range.len()).sum::<usize>() == content.len()
        });
        if let Some((content, lines)) = found {
            for (k, l) in lines.iter().enumerate() {
                let id = line_id(n.id, k);
                let mut run = AkNode::new(Role::TextRun);
                run.set_value(l.spoken.as_deref().unwrap_or_else(|| content.get(l.range.clone()).unwrap_or_default()));
                run.set_bounds(AkRect {
                    x0: round(l.rect[0] + w.shift[0]),
                    y0: round(l.rect[1] + w.shift[1]),
                    x1: round(l.rect[0] + l.rect[2] + w.shift[0]),
                    y1: round(l.rect[1] + l.rect[3] + w.shift[1]),
                });
                run.set_character_lengths(l.lengths.clone());
                run.set_character_positions(l.positions.clone());
                run.set_character_widths(l.widths.clone());
                run.set_word_starts(l.word_starts.clone());
                out[me].1.push_child(id);
                out.push((id, run));
            }
            if let Some(sel) = self.a11y_selection(i, &content, &lines) {
                out[me].1.set_text_selection(sel);
            }
        } else if let Some(input) = n.input.as_deref()
            && !input.edit.is_secure()
        {
            // A field with no runs (past `RUN_MAX_BYTES`, or rows still unshaped): its selection in its value's bytes,
            // on the field itself. The vendored adapters answer its lines from the value when a
            // reader asks (`vendor/accesskit_windows.patch`, `vendor/accesskit_macos.patch`).
            let at = |character_index| TextPosition { node: element_id(n.id), character_index };
            out[me].1.set_text_selection(TextSelection { anchor: at(input.edit.anchor), focus: at(input.edit.focus) });
        }
        if spec.cold.links.is_empty() || spec.redacted {
            return;
        }
        let boxes = self.link_boxes(i, self.drawn_box(i));
        for (k, l) in spec.cold.links.iter().enumerate() {
            let Some(r) = boxes.iter().filter(|(j, _)| *j == k).fold(None, |u: Option<[f32; 4]>, (_, r)| {
                Some(u.map_or(*r, |u| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])]))
            }) else {
                continue;
            };
            let role = crate::a11y::roles::map(
                l.role,
                &crate::a11y::roles::RoleCtx {
                    pressable: true,
                    focusable: true,
                    input: false,
                    multiline: false,
                    secure: false,
                    scrolls: false,
                    modal: false,
                    default_button: false,
                    labelled: true,
                    keyed: true,
                    has_text: true,
                    image: false,
                },
            )
            .unwrap_or(Role::Link);
            let mut link = AkNode::new(role);
            let words = spec.source_slice(l.start, l.end).unwrap_or("");
            link.set_label(l.label.as_deref().unwrap_or(words));
            link.set_author_id(l.id.as_str());
            if role == Role::Link && l.id.contains("://") {
                link.set_url(l.id.as_str());
            }
            link.set_bounds(AkRect {
                x0: round(r[0] + w.shift[0]),
                y0: round(r[1] + w.shift[1]),
                x1: round(r[2] + w.shift[0]),
                y1: round(r[3] + w.shift[1]),
            });
            if self.allowed(i) {
                if spec.cold.on_link.is_some() {
                    link.add_action(Ak::Click);
                }
                if self.can_focus(i) {
                    link.add_action(Ak::Focus);
                }
                if spec.cold.on_link_secondary.is_some() {
                    link.add_action(Ak::ShowContextMenu);
                }
            }
            let id = link_id(n.id, k);
            out[me].1.push_child(id);
            out.push((id, link));
        }
    }

    /// A terminal grid's rows as its text lines, top to bottom: each row's text (its pinned
    /// cells in their columns) with every character's place on the cell grid, so a reader moves
    /// through the screen by character, word and line.
    fn a11y_grid_rows(&self, i: usize, g: &crate::el::grid::Grid, w: &a11y_tree::Walk, out: &mut Vec<(AkId, AkNode)>) {
        let n = &self.nodes[i];
        let me = out.len() - 1;
        let r = self.drawn[i];
        let [cw, ch] = g.cell;
        let shown = g.rows.len().min((r[3] / ch.max(1.0)).ceil() as usize);
        for (k, row) in g.rows.iter().take(shown).enumerate() {
            let (mut text, mut places) = row.reading();
            let read = places.len();
            let y = r[1] + k as f32 * ch;
            // A row's line ends in a line break, as a text field's does, so a reader's text
            // calls (and braille, and say-all) keep rows apart; a wrapped row runs on into the
            // next. The break takes no room, at the row's end.
            let end = places.last().map_or(0, |&(c, w)| c + w.max(1));
            if !row.wraps && k + 1 < shown {
                text.push('\n');
                places.push((end, 0));
            }
            // A wrapped row keeps the spaces at its end: they are the line's, between its words
            // ("echo" at the edge, "ab" on the next row).
            if row.wraps {
                text.extend((end..g.cols).map(|_| ' '));
                places.extend((end..g.cols).map(|col| (col, 1)));
            }
            let mut run = AkNode::new(Role::TextRun);
            run.set_bounds(AkRect {
                x0: round(r[0] + w.shift[0]),
                y0: round(y + w.shift[1]),
                x1: round(r[0] + f32::from(g.cols) * cw + w.shift[0]),
                y1: round(y + ch + w.shift[1]),
            });
            run.set_character_lengths(text.chars().map(|c| c.len_utf8() as u8).collect::<Vec<_>>());
            run.set_character_positions(places.iter().map(|&(col, _)| f32::from(col) * cw).collect::<Vec<_>>());
            run.set_character_widths(places.iter().map(|&(_, span)| f32::from(span) * cw).collect::<Vec<_>>());
            let chars: Vec<char> = text.chars().collect();
            let starts: Vec<u8> = (0..chars.len())
                .filter(|&c| !chars[c].is_whitespace() && (c == 0 || chars[c - 1].is_whitespace()))
                .map(|c| c.min(255) as u8)
                .collect();
            run.set_word_starts(starts);
            let id = line_id(n.id, k);
            // The cursor's cell is the caret, so a reader follows typing and output.
            if let Some((col, _)) = g.caret.filter(|c| usize::from(c.1) == k) {
                // Past the text, the caret sits before the line break, at the row's end.
                let at = TextPosition { node: id, character_index: crate::el::GridRow::char_at(&places[..read], col) };
                out[me].1.set_text_selection(TextSelection { anchor: at, focus: at });
            }
            run.set_value(text);
            out[me].1.push_child(id);
            out.push((id, run));
        }
    }

    /// What node `i` draws as text: a field's display text (its mask when secure), else its
    /// content.
    fn a11y_drawn_text(&self, i: usize) -> Option<Cow<'_, str>> {
        let n = &self.nodes[i];
        match n.input.as_deref() {
            Some(input) if !input.edit.is_secure() && input.edit.preedit().is_none() => {
                Some(Cow::Borrowed(input.edit.text.as_str()))
            }
            Some(input) => Some(Cow::Owned(input.edit.display().0)),
            None => n.text.as_deref().map(|t| Cow::Borrowed(t.content.as_str())),
        }
    }

    /// Node `i`'s text as a screen reader reads it: an image placeholder is the words its
    /// [`InlineImage`](crate::text::InlineImage) speaks, and leaves no character without them.
    /// Fields keep their own text.
    pub(super) fn a11y_spoken_text(&self, i: usize) -> Cow<'_, str> {
        let n = &self.nodes[i];
        let Some(spec) = n.text.as_deref().filter(|_| n.input.is_none()) else {
            return Cow::Borrowed("");
        };
        let text = spec.source_text();
        let labels = image_labels(spec, text);
        if labels.is_empty() {
            return Cow::Borrowed(text);
        }
        Cow::Owned(speak(text, &labels))
    }

    /// Node `i`'s laid-out lines, in points in its scroll viewport's content space (before the
    /// walk's shift).
    pub(super) fn a11y_lines(&self, i: usize) -> Option<(Cow<'_, str>, Vec<Line>)> {
        let n = &self.nodes[i];
        let (spec, layout) = (n.text.as_deref()?, n.text_layout.as_ref()?.view());
        if n.elided.is_some() {
            return None;
        }
        let content = self.a11y_drawn_text(i)?;
        if content.len() > RUN_MAX_BYTES {
            return None;
        }
        let s = self.env.scale;
        let r = self.drawn[i];
        let (x0, x1) = (r[0] + n.inset[0], r[0] + r[2] - n.inset[2]);
        let mut x = text_x(spec, x0, x1, layout.full_width() / s, layout.flow());
        if let Some(input) = n.input.as_deref()
            && input.single_line
        {
            x -= self.input_scroll.get(&n.path).copied().unwrap_or(0.0);
        }
        // A field's text is its own edit buffer: prose spans, the only ones with images, would
        // map the wrong bytes.
        let labels = if n.input.is_some() { Vec::new() } else { image_labels(spec, &content) };
        let lines = lines(layout, &content, &labels, s, [x, r[1] + n.inset[1]]);
        Some((content, lines))
    }

    /// A field's caret or selection, in its lines.
    fn a11y_selection(&self, i: usize, content: &str, lines: &[Line]) -> Option<TextSelection> {
        let n = &self.nodes[i];
        let input = n.input.as_deref()?;
        let edit = &input.edit;
        let (anchor, focus) = if edit.preedit().is_some() {
            let caret = caret_index(edit, content);
            (caret, caret)
        } else {
            (edit.display_index(edit.anchor), edit.display_index(edit.focus))
        };
        let at = |byte: usize| {
            position(lines, byte).map(|(k, c)| TextPosition { node: line_id(n.id, k), character_index: c })
        };
        Some(TextSelection { anchor: at(anchor)?, focus: at(focus)? })
    }

    /// The value and state a text field adds to its node.
    pub(super) fn a11y_input(&self, _i: usize, input: &crate::el::InputSpec, node: &mut AkNode) {
        if !input.placeholder.is_empty() {
            node.set_placeholder(input.placeholder.as_str());
        }
        if !input.edit.is_secure() {
            node.set_value(input.edit.text.as_str());
        }
        if input.access.is_none() {
            node.set_read_only();
        }
    }
}
