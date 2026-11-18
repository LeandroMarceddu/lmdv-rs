//! The full Help dialog: `locales/help/<id>.html` parsed from its small HTML
//! subset and drawn as one scrollable, theme-aware column.
//!
//! The pages use a deliberately small vocabulary — `<h1>`–`<h6>`, `<p>`,
//! `<ul>`/`<li>`, a two-column `<table class="help-keys">`, `<kbd>`, `<code>`,
//! `<strong>`/`<b>`, `<em>`/`<i>`, `<br>`, links and entities. [`parse`] is a
//! tokenizer for exactly those shapes: no HTML engine and no new dependency.
//! Unknown tags are ignored, so a page can grow a wrapper without breaking the
//! dialog. The parsed page is cached per locale id; only the first open pays
//! for the parse.
//!
//! Views stay pure: this module only reads the page text (embedded at build
//! time) and the live palette, and returns elements.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use rune::{Color, Cx, El, ScrollSpec, Span, TextStyle, col, el, icon, row, text};
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit};

use crate::window::{HelpView, WindowState};

/// The modal root's key.
pub const HELP_KEY: &str = "help-full-root";
/// The scrim's key.
pub const HELP_SCRIM: &str = "help-full-scrim";
/// The panel's key (`role=dialog`).
pub const HELP_PANEL: &str = "help-full-dialog";
/// The dialog title's key.
pub const HELP_TITLE: &str = "help-full-title";
/// The close button's key.
pub const HELP_CLOSE: &str = "help-full-close";
/// The scrolling page body's key.
pub const HELP_BODY: &str = "help-full-body";

/// The panel's widest, in points.
pub const HELP_WIDTH: f32 = 720.0;
/// The narrowest the panel gets when the window is small.
pub const HELP_MIN_WIDTH: f32 = 360.0;
/// The body's tallest before it scrolls.
pub const HELP_BODY_MAX: f32 = 620.0;

/// A parsed help page: its blocks in source order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HelpPage {
    /// Top-level blocks.
    pub blocks: Vec<Block>,
}

/// One top-level block of a help page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// A section or sub-section heading.
    Heading {
        /// 1 to 6, from `<h1>`…`<h6>`.
        level: u8,
        /// The heading's text.
        text: Inline,
    },
    /// A paragraph.
    Paragraph(Inline),
    /// One `<li>` item.
    Bullet(Inline),
    /// The rows of a `<table class="help-keys">`.
    Keys(Vec<KeyRow>),
}

/// One row of the key table: the chord(s), then what they do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyRow {
    /// The key column.
    pub keys: Inline,
    /// The description column.
    pub action: Inline,
}

/// Inline text split into styled runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inline {
    /// The runs in reading order; whitespace is already collapsed.
    pub runs: Vec<Run>,
}

impl Inline {
    /// The text as drawn, without styles.
    pub fn text(&self) -> String {
        self.runs.iter().map(|run| run.text.as_str()).collect()
    }

    /// Whether there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.runs.iter().all(|run| run.text.is_empty())
    }
}

/// One run of inline text with the marks that apply to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// The run's text.
    pub text: String,
    /// `<strong>`/`<b>`.
    pub bold: bool,
    /// `<em>`/`<i>`.
    pub italic: bool,
    /// `<code>`: monospace with an inline-code background.
    pub mono: bool,
    /// `<kbd>`: a monospace keycap with its own background.
    pub keycap: bool,
}

/// The parsed page for `locale_id`, parsed once and kept for later frames.
pub fn page(locale_id: &str) -> Arc<HelpPage> {
    static PAGES: OnceLock<Mutex<HashMap<String, Arc<HelpPage>>>> = OnceLock::new();
    let pages = PAGES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut pages = pages.lock().unwrap_or_else(|poison| poison.into_inner());
    pages.entry(locale_id.to_owned()).or_insert_with(|| Arc::new(parse(lmdv_i18n::help_page(locale_id)))).clone()
}

/// Parses the help pages' HTML subset into blocks.
///
/// The parser is deliberately forgiving: tags it does not know are skipped,
/// text outside a block becomes a paragraph, and an unclosed block still
/// reaches the page. It exists for the six reviewed pages, not for arbitrary
/// web HTML.
pub fn parse(html: &str) -> HelpPage {
    let mut blocks = Vec::new();
    let mut open: Option<Open> = None;
    let mut style = Style::default();
    let mut table: Option<Vec<KeyRow>> = None;
    let mut cells: Option<Vec<Inline>> = None;
    let mut rest = html;

    while let Some(token) = next_token(&mut rest) {
        match token {
            Token::Other => {}
            Token::Text(raw) => push_text(&mut open, raw, style),
            Token::Start { name } => {
                if let Some(level) = heading_level(name) {
                    finish_open(&mut open, &mut blocks);
                    style = Style::default();
                    open = Some(Open::Heading { level, builder: InlineBuilder::default() });
                } else if tag_is(name, "p") {
                    finish_open(&mut open, &mut blocks);
                    style = Style::default();
                    open = Some(Open::Paragraph(InlineBuilder::default()));
                } else if tag_is(name, "li") {
                    finish_open(&mut open, &mut blocks);
                    style = Style::default();
                    open = Some(Open::Bullet(InlineBuilder::default()));
                } else if tag_is(name, "table") {
                    finish_open(&mut open, &mut blocks);
                    table = Some(Vec::new());
                } else if tag_is(name, "tr") {
                    finish_cell(&mut open, &mut cells);
                    cells = Some(Vec::new());
                } else if tag_is(name, "td") || tag_is(name, "th") {
                    finish_cell(&mut open, &mut cells);
                    style = Style::default();
                    open = Some(Open::Cell(InlineBuilder::default()));
                } else if tag_is(name, "br") {
                    if open.is_none() {
                        open = Some(Open::Paragraph(InlineBuilder::default()));
                    }
                    if let Some(builder) = open.as_mut().map(Open::builder_mut) {
                        builder.push_break();
                    }
                } else if tag_is(name, "strong") || tag_is(name, "b") {
                    style.bold = style.bold.saturating_add(1);
                } else if tag_is(name, "em") || tag_is(name, "i") {
                    style.italic = style.italic.saturating_add(1);
                } else if tag_is(name, "code") {
                    style.code = style.code.saturating_add(1);
                } else if tag_is(name, "kbd") {
                    style.keycap = style.keycap.saturating_add(1);
                }
            }
            Token::End(name) => {
                if heading_level(name).is_some() || tag_is(name, "p") || tag_is(name, "li") {
                    finish_open(&mut open, &mut blocks);
                } else if tag_is(name, "td") || tag_is(name, "th") {
                    finish_cell(&mut open, &mut cells);
                } else if tag_is(name, "tr") {
                    finish_cell(&mut open, &mut cells);
                    if let Some(row_cells) = cells.take()
                        && let Some(rows) = table.as_mut()
                    {
                        push_key_row(rows, row_cells);
                    }
                } else if tag_is(name, "table") {
                    finish_cell(&mut open, &mut cells);
                    if let Some(rows) = table.take()
                        && !rows.is_empty()
                    {
                        blocks.push(Block::Keys(rows));
                    }
                } else if tag_is(name, "strong") || tag_is(name, "b") {
                    style.bold = style.bold.saturating_sub(1);
                } else if tag_is(name, "em") || tag_is(name, "i") {
                    style.italic = style.italic.saturating_sub(1);
                } else if tag_is(name, "code") {
                    style.code = style.code.saturating_sub(1);
                } else if tag_is(name, "kbd") {
                    style.keycap = style.keycap.saturating_sub(1);
                }
            }
        }
    }

    finish_open(&mut open, &mut blocks);
    if let Some(rows) = table.take()
        && !rows.is_empty()
    {
        blocks.push(Block::Keys(rows));
    }
    HelpPage { blocks }
}

/// What an open block is collecting text for.
enum Open {
    Heading { level: u8, builder: InlineBuilder },
    Paragraph(InlineBuilder),
    Bullet(InlineBuilder),
    Cell(InlineBuilder),
}

impl Open {
    fn builder_mut(&mut self) -> &mut InlineBuilder {
        match self {
            Self::Heading { builder, .. } | Self::Paragraph(builder) | Self::Bullet(builder) | Self::Cell(builder) => {
                builder
            }
        }
    }
}

fn finish_open(open: &mut Option<Open>, blocks: &mut Vec<Block>) {
    match open.take() {
        Some(Open::Heading { level, builder }) => {
            let inline = builder.finish();
            if !inline.is_empty() {
                blocks.push(Block::Heading { level, text: inline });
            }
        }
        Some(Open::Paragraph(builder)) => {
            let inline = builder.finish();
            if !inline.is_empty() {
                blocks.push(Block::Paragraph(inline));
            }
        }
        Some(Open::Bullet(builder)) => {
            let inline = builder.finish();
            if !inline.is_empty() {
                blocks.push(Block::Bullet(inline));
            }
        }
        // A cell closes in its own row; a stray one carries no block.
        Some(Open::Cell(_)) | None => {}
    }
}

fn finish_cell(open: &mut Option<Open>, cells: &mut Option<Vec<Inline>>) {
    if let Some(Open::Cell(builder)) = open.take()
        && let Some(cells) = cells.as_mut()
    {
        cells.push(builder.finish());
    }
}

fn push_key_row(rows: &mut Vec<KeyRow>, cells: Vec<Inline>) {
    let mut cells = cells.into_iter();
    let keys = cells.next().unwrap_or_default();
    let action = cells.next().unwrap_or_default();
    if !keys.is_empty() || !action.is_empty() {
        rows.push(KeyRow { keys, action });
    }
}

fn push_text(open: &mut Option<Open>, raw: &str, style: Style) {
    if raw.trim().is_empty() && open.is_none() {
        return;
    }
    if open.is_none() {
        *open = Some(Open::Paragraph(InlineBuilder::default()));
    }
    if let Some(builder) = open.as_mut().map(Open::builder_mut) {
        builder.push_text(raw, style);
    }
}

/// The inline marks that are open at the current point.
#[derive(Clone, Copy, Debug, Default)]
struct Style {
    bold: u32,
    italic: u32,
    code: u32,
    keycap: u32,
}

/// Accumulates runs, collapsing whitespace as an HTML renderer would.
#[derive(Default)]
struct InlineBuilder {
    runs: Vec<Run>,
}

impl InlineBuilder {
    fn push_text(&mut self, raw: &str, style: Style) {
        let mut buf = String::new();
        let mut space = false;
        let mut chars = raw.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '&' {
                if let Some(decoded) = decode_entity(&mut chars) {
                    if space {
                        buf.push(' ');
                        space = false;
                    }
                    buf.push(decoded);
                } else {
                    if space {
                        buf.push(' ');
                        space = false;
                    }
                    buf.push('&');
                }
                continue;
            }
            if ch.is_whitespace() {
                space = true;
                continue;
            }
            if space {
                buf.push(' ');
                space = false;
            }
            buf.push(ch);
        }
        if space {
            buf.push(' ');
        }
        self.push_run(buf, style);
    }

    fn push_break(&mut self) {
        self.push_run("\n".to_owned(), Style::default());
    }

    fn push_run(&mut self, text: String, style: Style) {
        if text.is_empty() {
            return;
        }
        let mut text = text;
        if let Some(last) = self.runs.last()
            && last.text.ends_with([' ', '\n'])
            && text.starts_with(' ')
        {
            let drop = text.len() - text.trim_start_matches(' ').len();
            text.drain(..drop);
        }
        if text.is_empty() {
            return;
        }
        let run = Run {
            text,
            bold: style.bold > 0,
            italic: style.italic > 0,
            mono: style.code > 0 || style.keycap > 0,
            keycap: style.keycap > 0,
        };
        match self.runs.last_mut() {
            Some(last) if same_style(last, &run) => last.text.push_str(&run.text),
            _ => self.runs.push(run),
        }
    }

    fn finish(mut self) -> Inline {
        for run in &mut self.runs {
            if run.text.chars().all(char::is_whitespace) {
                run.text.clear();
            }
        }
        if let Some(first) = self.runs.iter_mut().find(|run| !run.text.is_empty()) {
            first.text = first.text.trim_start_matches([' ', '\n', '\t']).to_owned();
        }
        if let Some(last) = self.runs.iter_mut().rev().find(|run| !run.text.is_empty()) {
            last.text = last.text.trim_end_matches([' ', '\n', '\t']).to_owned();
        }
        self.runs.retain(|run| !run.text.is_empty());
        Inline { runs: self.runs }
    }
}

fn same_style(a: &Run, b: &Run) -> bool {
    a.bold == b.bold && a.italic == b.italic && a.mono == b.mono && a.keycap == b.keycap
}

/// Decodes `&name;`, `&#NNN;` and `&#xHH;`; `None` leaves the `&` literal.
fn decode_entity(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<char> {
    let mut name = String::new();
    let look = chars.clone();
    let mut closed = false;
    for ch in look {
        if ch == ';' {
            closed = true;
            break;
        }
        if ch.is_whitespace() || ch == '&' || ch == '<' || name.len() >= 32 {
            return None;
        }
        name.push(ch);
    }
    if !closed || name.is_empty() {
        return None;
    }
    let value = entity_char(&name)?;
    for _ in 0..=name.chars().count() {
        chars.next();
    }
    Some(value)
}

fn entity_char(name: &str) -> Option<char> {
    Some(match name {
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
        _ => {
            let number = name.strip_prefix('#')?;
            let n = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            if n == 0 || n > 0x10ffff || (0xd800..=0xdfff).contains(&n) {
                return None;
            }
            char::from_u32(n)?
        }
    })
}

/// One token of the help pages' HTML.
enum Token<'a> {
    Start { name: &'a str },
    End(&'a str),
    Text(&'a str),
    Other,
}

fn next_token<'a>(rest: &mut &'a str) -> Option<Token<'a>> {
    if rest.is_empty() {
        return None;
    }
    if rest.starts_with("<!--") {
        let end = rest.find("-->").map(|index| index + 3).unwrap_or(rest.len());
        *rest = &rest[end..];
        return Some(Token::Other);
    }
    if rest.starts_with('<') {
        if let Some(close) = rest.find('>') {
            let raw = rest[1..close].trim();
            *rest = &rest[close + 1..];
            if let Some(name) = raw.strip_prefix('/') {
                return Some(Token::End(name.trim()));
            }
            if raw.starts_with('!') || raw.starts_with('?') {
                return Some(Token::Other);
            }
            let body = raw.trim_end_matches('/').trim_end();
            let end = body.find(char::is_whitespace).unwrap_or(body.len());
            return Some(Token::Start { name: &body[..end] });
        }
        // A `<` with no `>` is ordinary text (a pasted page could contain it).
        let all = *rest;
        *rest = "";
        return Some(Token::Text(all));
    }
    match rest.find('<') {
        Some(0) => Some(Token::Other),
        Some(index) => {
            let text = &rest[..index];
            *rest = &rest[index..];
            Some(Token::Text(text))
        }
        None => {
            let text = *rest;
            *rest = "";
            Some(Token::Text(text))
        }
    }
}

fn tag_is(name: &str, wanted: &str) -> bool {
    name.eq_ignore_ascii_case(wanted)
}

fn heading_level(name: &str) -> Option<u8> {
    let mut chars = name.chars();
    if !chars.next()?.eq_ignore_ascii_case(&'h') {
        return None;
    }
    let rest: String = chars.collect();
    let level: u8 = rest.parse().ok()?;
    (1..=6).contains(&level).then_some(level)
}

/// The full-help modal: `None` unless [`HelpView::Full`] is open.
pub fn full_help_dialog(state: &WindowState, cx: &mut Cx<WindowState>, kit: &Kit, layer: &mut Vec<El>) {
    if state.help_view() != Some(HelpView::Full) {
        return;
    }
    let c = &kit.c;
    let [width, height] = cx.layout_size("window-body").unwrap_or([1100.0, 760.0]);
    let panel_width = (width - 48.0).clamp(HELP_MIN_WIDTH, HELP_WIDTH);
    let body_max = (height - 170.0).clamp(140.0, HELP_BODY_MAX);
    let page = page(lmdv_i18n::locale_id());

    let scrim = el()
        .key(HELP_SCRIM)
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(c.scrim)
        .a11y_hidden()
        .on_press(|state: &mut WindowState| state.close_help());

    let header = row()
        .key("help-full-header")
        .w_full()
        .h(52.0)
        .px(20.0)
        .gap(10.0)
        .center_y()
        .child(icon(&crate::icons::HELP, 20.0).color(c.icon_muted))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(1.0)
                .child(
                    text(lmdv_i18n::t!("help.title"))
                        .key(HELP_TITLE)
                        .role("heading")
                        .level(1)
                        .font(TextStyle { line_height: 22.0, ..kit.ty.ui_medium })
                        .color(c.text_base)
                        .w_full(),
                )
                .child(
                    text(lmdv_i18n::t!("start.version", "version" => env!("CARGO_PKG_VERSION")))
                        .key("help-full-version")
                        .font(kit.ty.small)
                        .color(c.text_muted)
                        .w_full(),
                ),
        )
        .child(
            IconButton::new(kit.glyph.close)
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Regular)
                .label(lmdv_i18n::t!("help.close"))
                .help(lmdv_i18n::t!("help.close"))
                .on_press(|state: &mut WindowState| state.close_help())
                .build(kit)
                .key(HELP_CLOSE),
        );

    let body = col()
        .key(HELP_BODY)
        .w_full()
        .max_h(body_max)
        .min_h(0.0)
        .px(20.0)
        .pb(14.0)
        .focusable(true)
        .no_focus_ring()
        .autofocus()
        .scroll(ScrollSpec { stick_bottom: false })
        .child(blocks_column(&page, panel_width - 40.0, kit));

    let panel = col()
        .key(HELP_PANEL)
        .w(panel_width)
        .max_h(height - 32.0)
        .radius(10.0)
        .fill(c.layer_01)
        .border(1.0, c.border_strong)
        .shadow(12.0, 40.0, Color { r: 0.0, g: 0.0, b: 0.0, a: 0.30 })
        .preserve_focus_on_press(true)
        .child(header)
        .child(el().key("help-full-rule").w_full().h(1.0).fill(c.border_muted))
        .child(body);

    let centred =
        col().key("help-full-layer").absolute(0.0, 0.0).w_full().h_full().py(24.0).px(24.0).items_center().child(panel);
    let root = el()
        .key(HELP_KEY)
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .role("dialog")
        .label(lmdv_i18n::t!("help.title"))
        .focusable(true)
        .no_focus_ring()
        .modal_tx(false, |state: &mut WindowState, _| state.close_help())
        .child(scrim)
        .child(centred);
    layer.push(root);
}

fn blocks_column(page: &HelpPage, content_width: f32, kit: &Kit) -> El {
    let mut column = col().key("help-full-content").w_full().gap(8.0);
    let mut section = 0usize;
    for (index, block) in page.blocks.iter().enumerate() {
        match block {
            Block::Heading { level, text: inline } => {
                section += 1;
                column = column.child(
                    inline_el(inline, heading_font(*level, kit), kit.c.text_base, kit)
                        .key(format!("help-section-{section}"))
                        .role("heading")
                        .level(u32::from(*level))
                        .w_full()
                        .pt(if section > 1 { 10.0 } else { 0.0 }),
                );
            }
            Block::Paragraph(inline) => {
                column = column.child(
                    inline_el(inline, kit.ty.ui, kit.c.text_base, kit).key(format!("help-block-{index}")).w_full(),
                );
            }
            Block::Bullet(inline) => {
                column = column.child(
                    row()
                        .key(format!("help-block-{index}"))
                        .w_full()
                        .gap(8.0)
                        .items_start()
                        .child(text("•").font(kit.ty.small).color(kit.c.text_faint).a11y_hidden())
                        .child(inline_el(inline, kit.ty.ui, kit.c.text_base, kit).grow(1.0).min_w(0.0)),
                );
            }
            Block::Keys(rows) => {
                column = column.child(key_table(index, rows, content_width, kit));
            }
        }
    }
    column
}

fn key_table(index: usize, rows: &[KeyRow], content_width: f32, kit: &Kit) -> El {
    let key_width = (content_width * 0.38).clamp(104.0, 240.0);
    let mut table = col().key(format!("help-block-{index}")).w_full().gap(6.0);
    for (row_index, KeyRow { keys, action }) in rows.iter().enumerate() {
        table = table.child(
            row()
                .key(format!("help-key-{index}-{row_index}"))
                .w_full()
                .gap(12.0)
                .items_start()
                .child(inline_el(keys, kit.ty.ui, kit.c.text_muted, kit).w(key_width).shrink())
                .child(inline_el(action, kit.ty.ui, kit.c.text_base, kit).grow(1.0).min_w(0.0)),
        );
    }
    table
}

fn heading_font(level: u8, kit: &Kit) -> TextStyle {
    let (size, weight) = match level {
        1 => (20.0, 650.0),
        2 => (17.0, 620.0),
        3 => (15.0, 600.0),
        _ => (13.0, 600.0),
    };
    TextStyle { size, weight, line_height: size * 1.35, ..kit.ty.ui_medium }
}

/// Draws one inline as a wrapping text with styled byte ranges.
fn inline_el(inline: &Inline, font: TextStyle, color: Color, kit: &Kit) -> El {
    let content = inline.text();
    let mut spans = Vec::new();
    let mut offset = 0;
    for run in &inline.runs {
        let start = offset;
        offset += run.text.len();
        let end = offset;
        if end == start {
            continue;
        }
        let mut span = Span::new(start, end);
        if run.bold {
            span = span.weight(600.0);
        }
        if run.italic {
            span = span.italic();
        }
        if run.mono || run.keycap {
            span = span.mono();
        }
        if run.keycap {
            span = span.bg(kit.c.layer_02, 2.5).color(kit.c.text_base);
        } else if run.mono {
            span = span.bg(kit.c.layer_02, 2.0);
        }
        spans.push(span);
    }
    text(content).spans(spans).font(font).color(color).wrap_anywhere()
}
