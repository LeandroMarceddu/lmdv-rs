//! Blocks to elements: the article, paragraphs, headings, code, lists, quotes,
//! diagrams, math, images and the generated footnote section.
//!
//! Every top-level block is wrapped in a column keyed by its
//! [`lmdv_md::BlockSpan::key`], so `Tx::scroll_key_to("b7", 0)` and `--line`
//! land on a real block. A heading's text element additionally carries
//! `h{index}` inside that wrapper (ARCHITECTURE_DETAIL §5.3), which is what the
//! outline jumps to.

use std::cell::{Cell, RefCell};

use lmdv_md::{Block, ImageBlock};
use rune::{Align, Color, El, Span, col, el, icon, rich, row, text};
use rune_kit::glyphs::CHECK;

use crate::inline::InlineOptions;
use crate::theme::DocTheme;
use crate::{DOC_KEY, HighlightKind, MediaKind, MediaRef, PreviewBlock, PreviewText, RenderOptions, TaskRef, parts};

/// One drawn picture's box, for an image map's coordinates (L-12).
pub(crate) struct DrawnImage {
    /// The drawn width in points.
    pub(crate) width: f32,
    /// The drawn height in points.
    pub(crate) height: f32,
    /// The natural size in pixels, the space an `<area>`'s coords are in.
    pub(crate) natural: [f32; 2],
}

/// The renderer, a pure view over one document.
pub(crate) struct Renderer<'a> {
    pub(crate) doc: &'a lmdv_md::MdDoc,
    pub(crate) theme: &'a DocTheme,
    pub(crate) opts: &'a RenderOptions<'a>,
    /// A hash of the document source, for memo identities.
    pub(crate) source_hash: u64,
    /// A hash of the theme, computed once per render.
    pub(crate) theme_hash: u64,
    /// The top-level block being drawn, for task references inside nested
    /// blocks; `usize::MAX` outside one.
    pub(crate) current_block: Cell<usize>,
    /// How many lists the block being drawn sits in, for copied indentation.
    pub(crate) list_depth: Cell<u32>,
    /// The copy destination's index for the next code block in the top-level
    /// block being drawn (M-13). It starts at the number of code blocks before
    /// that block, so it does not depend on which blocks a memo reused, and it
    /// matches [`lmdv_md::MdDoc::code_texts`].
    pub(crate) code_base: Cell<usize>,
    /// The block being drawn's inline text and key scope, for [`crate::BlockWire`].
    /// Only filled while a block wire is set.
    pub(crate) preview: RefCell<PreviewBlock>,
    /// The HTML image maps of the block being drawn (L-12): the block index
    /// they were collected for, and every `<map>` node in that block.
    pub(crate) html_maps: RefCell<(usize, Vec<lmdv_md::HtmlNode>)>,
}

impl<'a> Renderer<'a> {
    /// The reading column: front matter, every block, the generated footnotes.
    pub(crate) fn article(&self) -> El {
        let mut c = col().key(DOC_KEY).w_full().selection_group(self.opts.selectable);
        let mut first = true;
        if let Some(front) = &self.doc.front_matter {
            c = c.child(parts::front_matter(self, front));
            first = false;
        }
        // M-13: each block's code destinations start after all the code
        // blocks before it; the running count keeps the article one pass.
        let mut code_base = 0;
        for (index, block) in self.doc.blocks.iter().enumerate() {
            c = c.child(self.block_at(index, block, first, code_base));
            first = false;
            code_base += self.doc.code_count_in(index);
        }
        c
    }

    /// One top-level block, keyed and spaced like a CSS block.
    fn block_at(&self, index: usize, block: &Block, first: bool, code_base: usize) -> El {
        let key = self.doc.block_key(index).map(str::to_owned).unwrap_or_else(|| format!("b{index}"));
        let (top, bottom) = self.margins(block);
        let top = if first { 0.0 } else { top };
        let build = || {
            let previous = self.current_block.replace(index);
            // M-13: this block's code blocks are numbered from all the code
            // blocks before it, never from a per-frame counter: a memoised
            // block's build does not run, and its neighbours must not shift.
            let previous_code = self.code_base.replace(code_base);
            let drawn = self.draw_block(block, index);
            self.code_base.set(previous_code);
            self.current_block.set(previous);
            col().w_full().mt(top).mb(bottom).child(drawn)
        };
        // E-04/E-35: the app wraps every block to turn a preview click into a
        // source line. It needs the block exactly as drawn, so the build
        // collects its inline text in reading order (never memoised with a wire,
        // as every other app handler).
        if let Some(wire) = &self.opts.wires.block {
            {
                let mut preview = self.preview.borrow_mut();
                preview.text.clear();
                preview.texts.clear();
                preview.scope.clear();
            }
            let drawn = build().key(key.clone());
            let (text, texts) = {
                let mut preview = self.preview.borrow_mut();
                (std::mem::take(&mut preview.text), std::mem::take(&mut preview.texts))
            };
            let span = self.doc.block_spans.get(index);
            let reference = crate::BlockRef {
                key,
                line: span.map_or(0, |span| span.line),
                end_line: span.map_or(1, |span| span.end_line),
                code: matches!(block, Block::Code { .. }),
                text,
                texts,
            };
            return wire(drawn, &reference);
        }
        // Memoise only read-only trees: a handler built here would go stale
        // across commits, as Rune's own Markdown avoids memoising with
        // callbacks set (markdown.rs:427-445). The document hash keeps two
        // articles on screen from sharing a block cache.
        if self.is_read_only() {
            let version = (
                self.doc.revision,
                self.source_hash,
                self.theme_hash,
                (self.opts.width * 4.0) as i32,
                self.opts.selectable,
                self.opts.sticky_tables,
                self.opts.sticky_inset.to_bits(),
                self.opts.scale.to_bits(),
                self.highlight_version(index),
                index,
            );
            let id = format!("lmdv/{:x}-{:x}/b{index}", self.doc.revision, self.source_hash);
            rune::memo(id, version, build).key(key)
        } else {
            build().key(key)
        }
    }

    /// Remembers one drawn inline text for the block wire and returns the key
    /// its element must carry (no-op without one).
    ///
    /// A heading keeps its `h{n}` key; a paragraph's generic `text` and every
    /// keyless text take a block-scoped identity, so the app can look the
    /// element's box up and two identical texts never share one (E-35).
    pub(crate) fn note_text(&self, key: Option<&str>, drawn: &str) -> Option<String> {
        self.opts.wires.block.as_ref()?;
        let mut preview = self.preview.borrow_mut();
        let index = preview.texts.len();
        let block = self
            .doc
            .block_key(self.current_block.get())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("b{}", self.current_block.get()));
        let identity = match key {
            Some(key) if key != "text" => key.to_owned(),
            _ => format!("{block}-text-{index}"),
        };
        let start = preview.text.len();
        let path = preview.scope.clone();
        preview.text.push_str(drawn);
        preview.text.push('\n');
        preview.texts.push(PreviewText { key: Some(identity.clone()), index, range: start..start + drawn.len(), path });
        Some(identity)
    }

    /// Adds a renderer-created key to the scope a noted text carries.
    pub(crate) fn push_scope(&self, key: String) {
        if self.opts.wires.block.is_some() {
            self.preview.borrow_mut().scope.push(key);
        }
    }

    /// Leaves the innermost [`Self::push_scope`].
    pub(crate) fn pop_scope(&self) {
        if self.opts.wires.block.is_some() {
            self.preview.borrow_mut().scope.pop();
        }
    }

    /// The top-level block index of the block being drawn; 0 outside one (a
    /// standalone article fragment).
    pub(crate) fn nested_block(&self) -> usize {
        let index = self.current_block.get();
        if index == usize::MAX { 0 } else { index }
    }

    /// The [`MediaRef`] for a picture or diagram drawn in the current block.
    ///
    /// The block key and source lines come from the top-level block, so a
    /// nested image still names the block the app can scroll to. `source` is
    /// the resolved picture URL or the Mermaid source; `width`/`height` are
    /// the content's natural size in points.
    fn media_ref(&self, kind: MediaKind, source: &str, alt: &str, width: f32, height: f32) -> MediaRef {
        let index = self.current_block.get();
        let span = self.doc.block_spans.get(index);
        MediaRef {
            block: self.doc.block_key(index).map(str::to_owned).unwrap_or_else(|| format!("b{index}")),
            index: if index == usize::MAX { 0 } else { index },
            kind,
            source: source.to_owned(),
            alt: alt.to_owned(),
            width: width.max(0.0),
            height: height.max(0.0),
            line: span.map_or(0, |span| span.line),
            end_line: span.map_or(1, |span| span.end_line),
        }
    }

    fn is_read_only(&self) -> bool {
        self.opts.wires.link.is_none()
            && self.opts.wires.task.is_none()
            && self.opts.wires.task_control.is_none()
            && self.opts.wires.block.is_none()
            && self.opts.wires.table.is_none()
            && self.opts.wires.media.is_none()
            && self.opts.wires.area.is_none()
            && self.opts.details.is_none()
    }

    /// The paint of one highlight kind, from the reading theme.
    pub(crate) fn highlight_color(&self, kind: HighlightKind) -> Color {
        match kind {
            HighlightKind::Line => self.theme.highlight_line,
            HighlightKind::Landing => self.theme.highlight_landing,
            HighlightKind::Find => self.theme.highlight_find,
            HighlightKind::Current => self.theme.highlight_current,
        }
    }

    /// The part of the block memo version that changes with the highlights that
    /// reach this block.
    ///
    /// Only the ranges that overlap the block's source join the hash, so a match
    /// moving inside one paragraph repaints that paragraph and leaves every
    /// other memoised block alone. A generated block (the footnote section, the
    /// TOC) has no source span of its own, so all ranges join; an empty list is
    /// the constant 0 and costs nothing.
    fn highlight_version(&self, index: usize) -> u64 {
        use std::hash::{Hash, Hasher};
        if self.opts.highlights.is_empty() {
            return 0;
        }
        let span = self.doc.block_spans.get(index).map(|span| span.source.clone());
        let all = span.as_ref().is_none_or(|source| source.is_empty());
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let mut count = 0usize;
        for (position, highlight) in self.opts.highlights.iter().enumerate() {
            if let Some(source) = span.as_ref().filter(|_| !all)
                && (highlight.range.start >= source.end || highlight.range.end <= source.start)
            {
                continue;
            }
            position.hash(&mut hasher);
            highlight.range.hash(&mut hasher);
            highlight.kind.hash(&mut hasher);
            count += 1;
        }
        count.hash(&mut hasher);
        hasher.finish()
    }

    /// Vertical space before and after a block, as the page's margins collapse.
    pub(crate) fn margins(&self, block: &Block) -> (f32, f32) {
        match block {
            Block::Para(_) => (0.0, self.theme.block_gap),
            Block::Heading { heading, .. } => {
                let level = self.doc.headings[*heading].level.clamp(1, 6) as usize;
                self.theme.heading_margins[level - 1]
            }
            Block::Code { .. } | Block::Mermaid { .. } => (18.0, 18.0),
            Block::MathBlock { .. } => (18.0, 18.0),
            Block::List { .. } => (8.0, 12.0),
            Block::Quote { callout: Some(_), .. } => (16.0, 16.0),
            Block::Quote { .. } => (16.0, 16.0),
            Block::Table(_) => (16.0, 16.0),
            Block::Rule => (24.0, 24.0),
            Block::Image(image) => {
                if self.image_visible(self.image_source(image).0) {
                    (24.0, 24.0)
                } else {
                    (0.0, 0.0)
                }
            }
            Block::Toc => (12.0, 16.0),
            Block::FootnoteDefs => (0.0, 0.0),
            Block::HtmlBlock(_) => (12.0, 12.0),
        }
    }

    /// A block's content; `index` is its place in [`lmdv_md::MdDoc::blocks`].
    pub(crate) fn draw_block(&self, block: &Block, index: usize) -> El {
        match block {
            Block::Para(inline) => {
                self.inline(inline, InlineOptions::new(self.theme.body, self.theme.text).key("text"))
            }
            Block::Heading { heading, inline } => {
                let model = &self.doc.headings[*heading];
                let level = model.level;
                let style = self.theme.heading(level);
                let mut e =
                    self.inline(inline, InlineOptions::new(style, self.theme.text).key(&crate::heading_key(*heading)));
                e = e.role("heading").level(u32::from(level));
                if level == 2 {
                    e = e.pb(6.0).border_sides([0.0, 0.0, 0.5, 0.0], self.theme.rule).border_layout();
                }
                e
            }
            Block::Rule => crate::horizontal_rule(self.theme.rule),
            Block::Code { lang, text, source } => self.code_block(lang.as_deref(), text, "code", Some(source)),
            Block::List { ordered, start, tight, items } => self.list(*ordered, *start, *tight, items),
            Block::Quote { blocks, callout } => match callout {
                Some(callout) => parts::callout(self, callout, blocks),
                None => self.quote(blocks),
            },
            Block::Table(table) => self.table(table),
            Block::Mermaid { source, .. } => self.mermaid(source),
            Block::MathBlock { tex, .. } => self.math_block(tex),
            Block::HtmlBlock(node) => self.html_block(node, index),
            Block::Image(image) => self.image_block(image),
            Block::FootnoteDefs => parts::footnotes(self),
            Block::Toc => parts::toc(self, index),
        }
    }

    /// A block quote: a left rule with the body's margins passing through it.
    pub(crate) fn quote(&self, blocks: &[Block]) -> El {
        let mut inner = col().key("body").w_full().pl(14.0).pt(2.0).pb(2.0);
        for (i, block) in blocks.iter().enumerate() {
            let (top, bottom) = self.margins(block);
            let top = if i == 0 { 0.0 } else { top };
            inner = inner.child(col().w_full().mt(top).mb(bottom).child(self.draw_block(block, self.nested_block())));
        }
        row()
            .key("quote")
            .role("blockquote")
            .w_full()
            .child(col().w(3.0).h_full().fill(self.theme.quote_rule))
            .child(inner.grow(1.0).shrink())
    }

    /// A fenced or indented code block with its language label and copy action.
    ///
    /// `source` is the block's source range for a real code block, so find and
    /// `--line` highlights can paint inside it; a Mermaid/math fallback passes
    /// `None` because its text is not drawn from the block's source map.
    ///
    /// M-13: a real code block's copy action names the block by its position in
    /// [`lmdv_md::MdDoc::code_texts`] — the same drawing order — so any number
    /// of code blocks, nested or duplicated, copy their own text. The Mermaid
    /// and math fallbacks draw their source too but are not Markdown code
    /// blocks, so they carry no copy action.
    pub(crate) fn code_block(
        &self,
        lang: Option<&str>,
        code: &str,
        key: &str,
        source: Option<&std::ops::Range<usize>>,
    ) -> El {
        // A code block's text is the whole block's body; noted for the block
        // wire so a double-click inside one maps back to its source word.
        let identity = self.note_text(None, code);
        let highlighted = rune_kit::syntax::highlight(lang, code);
        let spans: Vec<Span> = self.theme.syntax.spans(&highlighted, 0);
        let mut head = row().w_full().items_center().pb(6.0);
        if let Some(lang) = lang.filter(|l| !l.is_empty()) {
            head = head.child(
                text(lang.to_owned()).key("language").font(self.theme.small).color(self.theme.code_label).grow(1.0),
            );
        } else {
            head = head.child(col().grow(1.0));
        }
        if key == "code" {
            let index = self.code_base.get();
            self.code_base.set(index.saturating_add(1));
            head = head.child(self.action_chip(&self.theme.labels.copy, crate::copy_code_link(&index.to_string())));
        }
        let mut body = rich(code.to_owned(), spans).font(self.theme.code).color(self.theme.text).w_full();
        if let Some(identity) = identity {
            body = body.key(identity);
        }
        if let Some(source) = source {
            body = self.highlight_code(body, code, source);
        }
        if self.opts.selectable {
            body = body.selectable(true).selection_color(self.theme.selection);
        }
        let viewport = col()
            .key("scroll")
            .w_full()
            .scroll_x()
            .scroll_margin(crate::HORIZONTAL_REVEAL_MARGIN)
            .scroll_edge_fade(10.0);
        col()
            .key(key)
            .role("code")
            .w_full()
            .radius(self.theme.radius)
            .fill(self.theme.code_fill)
            .border(0.5, self.theme.code_border)
            .border_layout()
            .px(12.0)
            .py(10.0)
            .child(head)
            .child(viewport.child(body))
    }

    /// Paints the app's highlights inside a code block's text.
    ///
    /// A fence's body is verbatim, so its drawn bytes are the source bytes after
    /// the opening line; an indented block drops each line's indentation. The
    /// drawn range is found line by line, so `--line` over a code block and a
    /// find hit in it paint the code itself.
    fn highlight_code(&self, body: El, code: &str, source: &std::ops::Range<usize>) -> El {
        if self.opts.highlights.is_empty() || code.is_empty() {
            return body;
        }
        let segments = code_segments(self.doc.source.as_ref(), code, source);
        let mut parts = Vec::new();
        for (index, highlight) in self.opts.highlights.iter().enumerate() {
            if highlight.range.is_empty() {
                continue;
            }
            for (from, drawn) in &segments {
                let start = highlight.range.start.max(from.start);
                let end = highlight.range.end.min(from.end);
                if start >= end {
                    continue;
                }
                let offset = start - from.start;
                let range = drawn.start + offset..drawn.start + offset + (end - start);
                parts.push((range, highlight.kind, index));
            }
        }
        let parts = crate::inline::overlay(parts, code.len());
        if parts.is_empty() { body } else { self.painted(body, parts) }
    }

    /// A small linked text chip (copy, open externally, the front-matter toggle).
    pub(crate) fn action_chip(&self, label: &str, dest: String) -> El {
        let mut chip = text(label.to_owned())
            .font(self.theme.small)
            .color(self.theme.muted)
            .px(6.0)
            .py(2.0)
            .radius(4.0)
            .fill(self.theme.code_bg)
            .link(rune::Link::new(0..label.len(), dest));
        if let Some(wire) = &self.opts.wires.link {
            chip = wire(chip);
        }
        chip
    }

    /// A diagram through `rune-mermaid`; an unsupported one falls back to its
    /// source with a notice (M-14, ADR-014).
    ///
    /// E-06: a card the app's cache already holds for `source` is reused as-is;
    /// only a miss renders the diagram and stores the card.
    fn mermaid(&self, source: &str) -> El {
        let holder = col().key("mermaid").w_full();
        let render = || rune_kit::mermaid::render(source, self.opts.style);
        let diagram = match self.opts.mermaid {
            Some(cache) => cache.get(source).or_else(|| {
                let diagram = render();
                if let Some(card) = diagram.as_ref() {
                    cache.put(source, card.clone());
                }
                diagram
            }),
            None => render(),
        };
        match diagram {
            Some(diagram) => {
                let holder =
                    holder.role("image").label(self.theme.labels.mermaid.clone()).child(diagram.key("diagram"));
                // V-01: a supported diagram is zoomable. The drawing is laid
                // out a second time only to measure it for the app; the card
                // the article draws stays exactly `rune_kit`'s, so a wire that
                // returns its element leaves the tree unchanged.
                let Some(wire) = &self.opts.wires.media else { return holder };
                // The app re-measures at the window's width when the preview
                // opens; measuring here too laid the diagram out twice.
                let reference = self.media_ref(MediaKind::Diagram, source, "", 0.0, 0.0);
                wire(holder, &reference)
            }
            None => holder.child(
                col()
                    .key("fallback")
                    .w_full()
                    .child(
                        text(self.theme.labels.invalid_mermaid.clone())
                            .font(self.theme.small)
                            .color(self.theme.callouts[3].accent)
                            .pb(6.0),
                    )
                    .child(self.code_block(Some("mermaid"), source, "source", None)),
            ),
        }
    }

    /// Display math: its own horizontally scrollable line (M-16–M-18, M-33).
    fn math_block(&self, tex: &str) -> El {
        let label = lmdv_i18n::render(&self.theme.labels.math, &[("tex", &tex)]);
        let max_width = (self.opts.width - 8.0).max(16.0);
        match self.opts.math.display(tex, self.theme.body.size) {
            Some(image) => {
                let image = crate::math::fit(image, max_width, max_width.max(48.0));
                let (width, height) = (image.width, image.height);
                let drawn = el().size(width, height).pixels(&image.pixels).a11y_hidden();
                col().key("math").role("math").label(label).w_full().child(
                    col()
                        .key("scroll")
                        .w_full()
                        .scroll_x()
                        .scroll_margin(crate::HORIZONTAL_REVEAL_MARGIN)
                        .scroll_edge_fade(10.0)
                        .child(row().w_full().justify_center().child(drawn)),
                )
            }
            None => col().key("math").role("math").label(label).w_full().child(self.code_block(
                Some("math"),
                tex,
                "source",
                None,
            )),
        }
    }

    /// A block image through the app's resolver; a missing picture keeps the
    /// alt text and source visible (M-24, M-25).
    pub(crate) fn image_block(&self, image: &ImageBlock) -> El {
        let (src, alt, title) = self.image_source(image);
        if !self.image_visible(src) {
            // M-29: `#gh-dark-mode-only`/`#gh-light-mode-only` hides the picture
            // in the other scheme, as the original's CSS does.
            return col().key("image").w_full().h(0.0);
        }
        let mut frame = col().key("image").w_full().items_start();
        match self.drawn_image(src, alt, title) {
            Some((drawn, _)) => frame = frame.child(drawn),
            None => {
                frame = frame.child(
                    row()
                        .key("missing")
                        .role("image")
                        .label(if alt.is_empty() { src.to_owned() } else { alt.to_owned() })
                        .w_full()
                        .gap(8.0)
                        .items_center()
                        .child(col().size(16.0, 16.0).radius(2.0).border(0.5, self.theme.muted).a11y_hidden())
                        .child(
                            text(if alt.is_empty() { src.to_owned() } else { alt.to_owned() })
                                .font(self.theme.body)
                                .color(self.theme.faint),
                        ),
                );
            }
        }
        frame
    }

    /// The drawn picture and its box: the element the article shows, its drawn
    /// size in points and its natural (intrinsic) pixel size.
    ///
    /// L-12 needs the natural size to scale an image map's `coords`; the block
    /// image and the HTML image map share this one resolution so both draw the
    /// same picture at the same box.
    pub(crate) fn drawn_image(&self, src: &str, alt: &str, title: &str) -> Option<(El, DrawnImage)> {
        let max_width = self.opts.width.min(self.theme.image_max_width).max(16.0);
        let picture = self.opts.images.resolve(src, alt, title)?;
        let [pixels_w, pixels_h] = picture.size().unwrap_or([640, 360]);
        let ratio = pixels_w as f32 / pixels_h.max(1) as f32;
        let width = (pixels_w as f32).min(max_width);
        let height = (width / ratio.max(0.001)).max(1.0);
        let mut drawn = el()
            .role("image")
            .label(if alt.is_empty() { self.theme.labels.image.clone() } else { alt.to_owned() })
            .w(width)
            .h(height)
            .pixels(picture.pixels())
            .radius(4.0);
        if !title.is_empty() {
            drawn = drawn.help(title.to_owned());
        }
        // V-01: a decoded picture is zoomable. A missing or broken one never
        // reaches the wire (its placeholder stays inert).
        if let Some(wire) = &self.opts.wires.media {
            let reference = self.media_ref(MediaKind::Image, src, alt, pixels_w as f32, pixels_h as f32);
            drawn = wire(drawn, &reference);
        }
        Some((drawn, DrawnImage { width, height, natural: [pixels_w as f32, pixels_h.max(1) as f32] }))
    }

    /// A block image's chosen source, alt and title: a `<picture>` source that
    /// matches the scheme, else a `srcset` candidate, else the markdown source.
    pub(crate) fn image_source<'b>(&self, image: &'b ImageBlock) -> (&'b str, &'b str, &'b str) {
        let title = image.title.as_deref().unwrap_or_default();
        if !self.image_visible(image.src.as_str()) {
            return (image.src.as_str(), image.alt.as_str(), title);
        }
        if let Some(source) = image.picture.iter().find(|source| {
            source.media.as_deref().is_none_or(|media| {
                let media = media.to_ascii_lowercase();
                if media.contains("dark") {
                    self.theme.dark
                } else if media.contains("light") {
                    !self.theme.dark
                } else {
                    true
                }
            })
        }) && let Some(candidate) = best_candidate(&source.srcset, self.opts.scale)
        {
            return (candidate.url.as_str(), image.alt.as_str(), title);
        }
        if let Some(candidate) = best_candidate(&image.srcset, self.opts.scale) {
            return (candidate.url.as_str(), image.alt.as_str(), title);
        }
        (image.src.as_str(), image.alt.as_str(), title)
    }

    /// Whether the scheme fragment on an image source lets it show (M-29).
    pub(crate) fn image_visible(&self, src: &str) -> bool {
        let src = src.to_ascii_lowercase();
        if src.contains("#gh-dark-mode-only") {
            self.theme.dark
        } else if src.contains("#gh-light-mode-only") {
            !self.theme.dark
        } else {
            true
        }
    }

    /// A bullet, ordered or task list.
    fn list(&self, ordered: bool, start: u64, tight: bool, items: &[Vec<Block>]) -> El {
        let depth = self.list_depth.get();
        let mut c = col().key("list").role("list").w_full().pl(if depth > 0 { 12.0 } else { 0.0 });
        let n = items.len();
        let gap = if tight { 2.0 } else { self.theme.list_gap.max(6.0) };
        self.list_depth.set(depth + 1);
        for (k, item) in items.iter().enumerate() {
            self.push_scope(format!("i{k}"));
            let checkbox = item.first().and_then(first_checkbox);
            let checked = checkbox.as_ref().map(|(checked, _)| *checked);
            let label = checked.map(|_| self.item_label(item));
            let task = checkbox.as_ref().map(|(checked, source)| TaskRef {
                block: self.doc.block_key(self.current_block.get()).map(str::to_owned).unwrap_or_default(),
                item: k,
                checked: *checked,
                label: label.clone().unwrap_or_default(),
                source: source.clone(),
            });
            let lead = match (checked, ordered) {
                (Some(true), _) => "- [x] ",
                (Some(false), _) => "- [ ] ",
                (None, true) => "",
                (None, false) => "- ",
            };
            let marker_text = if ordered { format!("{}.", start.saturating_add(k as u64)) } else { "•".to_owned() };
            let prefix = if ordered {
                format!("{}{}. ", "  ".repeat(depth as usize), start.saturating_add(k as u64))
            } else {
                format!("{}{}", "  ".repeat(depth as usize), lead)
            };
            let mut body = col().key("body").grow(1.0).shrink().pt(if tight { 0.0 } else { 2.0 });
            for (i, block) in item.iter().enumerate() {
                let (top, bottom) = self.margins(block);
                let top = if i == 0 { 0.0 } else { top.max(6.0) };
                let first_para = i == 0 && matches!(block, Block::Para(_));
                let drawn = if first_para {
                    if let Block::Para(inline) = block {
                        self.inline(
                            inline,
                            InlineOptions::new(self.theme.body, self.theme.text)
                                .breaks(if depth == 0 && k == 0 { 2 } else { 1 }, 1)
                                .strip_tasks(checked.is_some())
                                .copy_prefix(prefix.clone()),
                        )
                    } else {
                        self.draw_block(block, self.nested_block())
                    }
                } else {
                    self.draw_block(block, self.nested_block())
                };
                body =
                    body.child(col().w_full().mt(top).mb(if i + 1 == item.len() { 0.0 } else { bottom }).child(drawn));
            }
            let mut row =
                row().key(format!("i{k}")).role("listitem").position_in_set(k + 1, n).w_full().gap(4.0).mt(if k == 0 {
                    0.0
                } else {
                    gap
                });
            match task.as_ref() {
                Some(task) => {
                    row = row.child(self.task_control(task));
                }
                None => {
                    let marker = marker_text;
                    row = row.child(
                        text(marker)
                            .font(self.theme.body)
                            .color(self.theme.muted)
                            .exact_lines()
                            .w(self.theme.list_indent - 4.0)
                            .align(Align::End)
                            .a11y_hidden(),
                    );
                }
            }
            row = row.child(body);
            if let (Some(wire), Some(task)) = (&self.opts.wires.task, &task) {
                row = wire(row, task);
            }
            self.pop_scope();
            c = c.child(row);
        }
        self.list_depth.set(depth);
        c
    }

    /// The task checkbox control: a real checkbox. The app's control wire makes
    /// the box itself live, so it keeps its role, checked state, label and
    /// keyboard focus instead of adding an overlay (OI-091).
    fn task_control(&self, task: &TaskRef) -> El {
        let checked = task.checked;
        let wired = self.opts.wires.task_control.is_some() || self.opts.wires.task.is_some();
        let size = 15.0;
        let (fill, border) =
            if checked { (self.theme.accent, self.theme.accent) } else { (Color::TRANSPARENT, self.theme.muted) };
        let mut box_ = el()
            .key(format!("task-{}", task.item))
            .role("checkbox")
            .label(task.label.clone())
            .checked(checked)
            .read_only(!wired)
            .focusable(wired)
            .size(size, size)
            .radius(3.0)
            .fill(fill)
            .border(1.0, border)
            .center()
            .child(icon(&CHECK, size * 0.625).color(Color::WHITE).opacity(if checked { 1.0 } else { 0.0 }));
        if let Some(wire) = &self.opts.wires.task_control {
            box_ = wire(box_, task);
        }
        row().w(self.theme.list_indent - 2.0).pt(4.0).child(box_)
    }
}

/// The `srcset` candidate for `scale`: a `2x` (or higher) one from 1.5x, the
/// `1x` one below, and the first candidate when the descriptors do not say.
pub(crate) fn best_candidate(candidates: &[lmdv_md::SrcSetCandidate], scale: f32) -> Option<&lmdv_md::SrcSetCandidate> {
    let ratio = |candidate: &lmdv_md::SrcSetCandidate| {
        candidate.descriptor.as_deref().and_then(|d| d.strip_suffix('x')).and_then(|x| x.parse::<f32>().ok())
    };
    if scale >= 1.5 {
        candidates.iter().find(|candidate| ratio(candidate).is_some_and(|r| r >= 1.5)).or_else(|| candidates.first())
    } else {
        candidates
            .iter()
            .find(|candidate| ratio(candidate).is_some_and(|r| (r - 1.0).abs() < 0.01))
            .or_else(|| candidates.first())
    }
}

/// The first task checkbox of an item's first block, with its marker's source
/// bytes when the inline source map has them.
fn first_checkbox(block: &Block) -> Option<(bool, std::ops::Range<usize>)> {
    let Block::Para(inline) = block else { return None };
    let (checked, rendered) = inline.objects.iter().find_map(|object| match &object.kind {
        lmdv_md::InlineObjectKind::TaskCheckbox { checked } => Some((*checked, object.range.start)),
        _ => None,
    })?;
    let source = inline.source_map.source_of(rendered).map(|start| start..start + 3).unwrap_or(0..0);
    Some((checked, source))
}

/// The source and drawn byte ranges of each line of a code block's text.
///
/// The drawn text is the block's `text`: for a fence, the source after the
/// opening line up to the closing fence; for indented code, the same lines with
/// their indentation dropped. Each non-empty line is found in order inside the
/// block's own source span, so a line that appears in the fence info can't be
/// mistaken for the body.
fn code_segments(
    source: &str,
    code: &str,
    span: &std::ops::Range<usize>,
) -> Vec<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let Some(block) = source.get(span.clone()) else { return Vec::new() };
    // A fence may sit at up to three spaces; four (or a tab) is indented code
    // even when its first line happens to start with backticks.
    let indent = block.len() - block.trim_start_matches(' ').len();
    let fenced = indent <= 3 && {
        let rest = &block[indent..];
        rest.starts_with("```") || rest.starts_with("~~~")
    };
    let mut cursor = if fenced { block.find('\n').map_or(block.len(), |at| at + 1) } else { 0 };
    let mut drawn = 0usize;
    let mut segments = Vec::new();
    for line in code.split('\n') {
        if !line.is_empty()
            && let Some(at) = block[cursor..].find(line)
        {
            let start = span.start + cursor + at;
            segments.push((start..start + line.len(), drawn..drawn + line.len()));
            cursor += at + line.len();
        }
        drawn += line.len() + 1;
    }
    segments
}
