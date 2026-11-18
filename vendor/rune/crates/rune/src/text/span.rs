//! What an app styles a text element's byte ranges with: spans, links, highlights and pills.

use super::*;

/// Brush carried through Parley: 0 is the element's own colour, n is `spans[n - 1]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ink(pub u16);

/// A styled byte range of a text element (markdown inline code, bold, links).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub weight: Option<f32>,
    pub mono: bool,
    pub size: Option<f32>,
    pub color: Option<Color>,
    /// Rounded background behind the span's runs (inline code), padded by `bg_pad` points.
    pub bg: Option<Color>,
    pub bg_pad: f32,
    pub underline: Option<Color>,
    /// The family's italic face, or a synthetic oblique when it has none.
    pub italic: bool,
    /// An image drawn in place of this span (see [`Span::image`]).
    pub image: Option<Box<InlineImage>>,
    /// Baseline shift in points; 0 sits on the baseline (see [`Span::rise`]).
    pub rise: f32,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span { start: start as u32, end: end as u32, ..Default::default() }
    }

    pub fn weight(mut self, weight: f32) -> Span {
        self.weight = Some(weight);
        self
    }

    /// Weight 600.
    pub fn bold(mut self) -> Span {
        self.weight = Some(600.0);
        self
    }

    pub fn italic(mut self) -> Span {
        self.italic = true;
        self
    }

    pub fn mono(mut self) -> Span {
        self.mono = true;
        self
    }

    pub fn size(mut self, size: f32) -> Span {
        self.size = Some(size);
        self
    }

    pub fn color(mut self, color: Color) -> Span {
        self.color = Some(color);
        self
    }

    /// A rounded `color` background padded by `pad` points, as inline code draws.
    pub fn bg(mut self, color: Color, pad: f32) -> Span {
        self.bg = Some(color);
        self.bg_pad = pad;
        self
    }

    pub fn underline(mut self, color: Color) -> Span {
        self.underline = Some(color);
        self
    }

    /// Draws `image` where this span's first [`U+FFFC`](OBJECT_REPLACEMENT) placeholder is, in
    /// room of its own: the placeholders shape as zero-width spaces, so the image takes their
    /// place and the text around it moves aside. The span must hold a placeholder; without one
    /// it draws nothing, and its bytes stay ordinary text. The image never splits across lines;
    /// a line break falls before or after it, as one around an atomic inline box does.
    ///
    /// Selection and copy still see the span's placeholder bytes: mark the same range
    /// [`El::uncopied`](crate::El::uncopied) to copy around the image, or leave it to copy the
    /// `U+FFFC` object marker.
    ///
    /// ```ignore
    /// rich("the area is A = πr² in all cases", vec![Span::new(a, a + 3).image(InlineImage::new(pixels, 22.0, 12.0))])
    /// ```
    pub fn image(mut self, image: InlineImage) -> Span {
        self.image = Some(Box::new(image));
        self
    }

    /// Raises this span `points` above the baseline, negative lowers it, as CSS's
    /// `vertical-align: <length>` does; an image in the span rises with it. The line still
    /// makes room for where the span is drawn.
    pub fn rise(mut self, points: f32) -> Span {
        self.rise = if points.is_finite() { points } else { 0.0 };
        self
    }
}

/// The character a text puts where an inline image goes. Inside a [`Span::image`] it draws
/// nothing and takes no width; outside one it is an ordinary character.
pub const OBJECT_REPLACEMENT: char = '\u{fffc}';

/// A bitmap drawn inline with text at its own size, such as a formula: a picture in place of
/// text, not a glyph.
///
/// `width` and `height` are what it draws, in points. `ascent` and `descent` are the room it
/// reserves above and below the baseline, so a formula's depth dips below the line as its
/// descenders do; its top sits `ascent` points above the baseline. [`new`](Self::new) puts the
/// whole height above the baseline, as an inline image at the baseline does.
#[derive(Clone, Debug)]
pub struct InlineImage {
    /// The pixels drawn, as [`El::pixels`](crate::El::pixels) draws them.
    pub pixels: crate::Pixels,
    /// The drawn width in points.
    pub width: f32,
    /// The drawn height in points.
    pub height: f32,
    /// The room above the baseline in points.
    pub ascent: f32,
    /// The room below the baseline in points.
    pub descent: f32,
}

impl InlineImage {
    /// `pixels` drawn `width` by `height` points, sitting on the baseline: all of its height is
    /// above the baseline.
    pub fn new(pixels: crate::Pixels, width: f32, height: f32) -> InlineImage {
        let (width, height) = (length(width), length(height));
        InlineImage { pixels, width, height, ascent: height, descent: 0.0 }
    }

    /// The room it reserves above the baseline in points, where its top sits.
    pub fn ascent(mut self, points: f32) -> InlineImage {
        self.ascent = length(points);
        self
    }

    /// The room it reserves below the baseline in points.
    pub fn descent(mut self, points: f32) -> InlineImage {
        self.descent = length(points);
        self
    }
}

impl PartialEq for InlineImage {
    /// The same pixels ([`Pixels::set`] starts a new image) at the same size and baseline.
    fn eq(&self, other: &InlineImage) -> bool {
        self.pixels.signature() == other.pixels.signature()
            && self.width == other.width
            && self.height == other.height
            && self.ascent == other.ascent
            && self.descent == other.descent
    }
}

/// `points` clamped to a finite, non-negative length.
fn length(points: f32) -> f32 {
    if points.is_finite() { points.max(0.0) } else { 0.0 }
}

/// An atomic pill drawn as a chip: byte range `start..end` of the content, and its leading icon.
/// A `bare` one is an [`InlineIcon`](crate::InlineIcon): no chip, only the icon before the
/// word it keeps to its line.
#[derive(Clone, Debug)]
pub(crate) struct PillChip {
    pub start: u32,
    pub end: u32,
    pub icon: Option<&'static Icon>,
    pub bg: Option<Color>,
    pub fg: Option<Color>,
    pub bare: Option<Box<BareIcon>>,
}

/// An inline icon's room and look in points (see [`InlineIcon`](crate::InlineIcon)).
#[derive(Clone, Debug)]
pub(crate) struct BareIcon {
    pub size: f32,
    pub gap: f32,
    pub radius: f32,
    pub image: Option<crate::Pixels>,
}

/// A pill chip's horizontal geometry in points for text of `size` points: the room before its
/// label (outer margin, padding, then any icon and its gap) and after it (padding, margin).
pub(crate) struct PillMetrics {
    pub margin: f32,
    pub pad: f32,
    pub icon: f32,
    pub icon_gap: f32,
}

impl PillMetrics {
    pub fn new(size: f32) -> Self {
        Self {
            margin: (size * 0.12).max(1.5),
            pad: (size * 0.36).max(3.0),
            icon: (size * 0.95).round(),
            icon_gap: (size * 0.22).max(2.0),
        }
    }

    /// Room before the label, inside the chip and out.
    pub fn lead(&self, icon: bool) -> f32 {
        self.margin + self.pad + if icon { self.icon + self.icon_gap } else { 0.0 }
    }

    pub fn trail(&self) -> f32 {
        self.pad + self.margin
    }
}

/// Tags the inline boxes around pill labels: the rest of the id is the pill's index shifted
/// left once, with the low bit set on the box after the label.
pub(super) const PILL_BOX: u64 = 1 << 62;

/// Tags an inline image's box: the rest of the id is the image span's index in the layout's
/// spans (see [`Span::image`]).
pub(super) const IMAGE_BOX: u64 = 1 << 61;

/// Flattens overlapping [`Span`]s into non-overlapping elementary intervals so that Parley's
/// single `Brush(Ink)` index per glyph run carries the combined colour, background, underline,
/// weight, italic, and monospace attributes of all overlapping spans.
pub(crate) fn flatten_spans(spans: &[Span]) -> Vec<Span> {
    if spans.len() <= 1 || spans.windows(2).all(|w| w[0].start < w[0].end && w[0].end <= w[1].start) {
        return spans.iter().filter(|s| s.start < s.end).cloned().collect();
    }
    let mut bounds = Vec::with_capacity(spans.len() * 2);
    let mut order: Vec<usize> = Vec::with_capacity(spans.len());
    for (i, s) in spans.iter().enumerate() {
        if s.start < s.end {
            bounds.push(s.start);
            bounds.push(s.end);
            order.push(i);
        }
    }
    bounds.sort_unstable();
    bounds.dedup();
    order.sort_by_key(|&i| spans[i].start);
    // Sweep the elementary intervals, keeping the spans that cover the current one in input
    // order so later spans still win, without rescanning every span per interval.
    let mut active: Vec<usize> = Vec::new();
    let mut next = 0;
    let mut out: Vec<Span> = Vec::with_capacity(bounds.len());
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        active.retain(|&i| spans[i].end > a);
        while next < order.len() && spans[order[next]].start <= a {
            let i = order[next];
            let at = active.partition_point(|&j| j < i);
            active.insert(at, i);
            next += 1;
        }
        let mut merged: Option<Span> = None;
        for &i in &active {
            let s = &spans[i];
            let m = merged.get_or_insert_with(|| Span::new(a as usize, b as usize));
            if s.weight.is_some() {
                m.weight = s.weight;
            }
            m.mono |= s.mono;
            if s.size.is_some() {
                m.size = s.size;
            }
            if s.color.is_some() {
                m.color = s.color;
            }
            if s.bg.is_some() {
                m.bg = s.bg;
                m.bg_pad = s.bg_pad;
            }
            if s.underline.is_some() {
                m.underline = s.underline;
            }
            // An image draws at its span's first byte: the interval starting there keeps it,
            // and the rest of a split span keeps its look but no second copy of the image.
            if s.image.is_some() && a == s.start {
                m.image = s.image.clone();
            }
            if s.rise != 0.0 {
                m.rise = s.rise;
            }
            m.italic |= s.italic;
        }
        if let Some(m) = merged {
            if let Some(last) = out.last_mut()
                && last.end == m.start
                && last.weight == m.weight
                && last.mono == m.mono
                && last.size == m.size
                && last.color == m.color
                && last.bg == m.bg
                && last.bg_pad == m.bg_pad
                && last.underline == m.underline
                && last.italic == m.italic
                && last.image == m.image
                && last.rise == m.rise
            {
                last.end = m.end;
            } else {
                out.push(m);
            }
        }
    }
    out
}

/// A pressable byte range of a text element: a link, an @mention, a file chip. Attach with
/// [`El::link`](crate::El::link) and handle with [`El::on_link`](crate::El::on_link). Its resting
/// look is whatever [`Span`]s cover it; [`LinkPaint`]s add the hover, pressed and focus looks.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) id: String,
    pub(crate) role: &'static str,
    pub(crate) label: Option<String>,
    pub(crate) hover: LinkPaint,
    pub(crate) pressed: Option<LinkPaint>,
    pub(crate) focused: Option<LinkPaint>,
    pub(crate) help: Option<String>,
}

impl Link {
    /// Bytes `range` of the text, reported to `on_link` as `id`. Its role is "link".
    pub fn new(range: std::ops::Range<usize>, id: impl Into<String>) -> Link {
        Link {
            start: range.start as u32,
            end: range.end as u32,
            id: id.into(),
            role: "link",
            label: None,
            hover: LinkPaint::default(),
            pressed: None,
            focused: None,
            help: None,
        }
    }
    /// Accessibility role, such as "button" for a file chip.
    pub fn role(mut self, role: &'static str) -> Self {
        self.role = role;
        self
    }
    /// Accessible name; the range's text by default.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    /// The look while the pointer is over it.
    pub fn hover(mut self, paint: LinkPaint) -> Self {
        self.hover = paint;
        self
    }
    /// The look while it is held down; the hover look by default.
    pub fn pressed(mut self, paint: LinkPaint) -> Self {
        self.pressed = Some(paint);
        self
    }
    /// The look while it has keyboard focus; the hover look by default, or an underline in
    /// its text colour when that is empty.
    pub fn focused(mut self, paint: LinkPaint) -> Self {
        self.focused = Some(paint);
        self
    }
    /// A tooltip for this link alone, as [`El::help`](crate::El::help) is for an element:
    /// shown after the hover delay while the pointer rests on the link's glyphs, and hidden
    /// by a press, scroll, key or the pointer leaving them. An empty string sets none.
    pub fn help(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.help = (!text.is_empty()).then_some(text);
        self
    }
}

/// A link's look in one state, painted over its resting span style without reshaping. Unset
/// parts keep the span's own.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LinkPaint {
    pub(crate) color: Option<Color>,
    pub(crate) bg: Option<Color>,
    pub(crate) ring: Option<(Color, f32)>,
    pub(crate) underline: Option<Color>,
}

impl LinkPaint {
    pub fn new() -> LinkPaint {
        LinkPaint::default()
    }
    /// Glyph colour.
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    /// A rounded background behind each line of the range.
    pub fn bg(mut self, c: Color) -> Self {
        self.bg = Some(c);
        self
    }
    /// An inset `c` border ring `width` points wide inside the rounded background on each line.
    pub fn ring(mut self, c: Color, width: f32) -> Self {
        self.ring = (width > 0.0 && width.is_finite()).then_some((c, width));
        self
    }
    pub fn underline(mut self, c: Color) -> Self {
        self.underline = Some(c);
        self
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.color.is_none() && self.bg.is_none() && self.ring.is_none() && self.underline.is_none()
    }
}

/// A paint-only highlight over `start..end` UTF-8 bytes of a [`text`](crate::text) element.
///
/// Drawn from the shaped layout's glyph boxes without splitting runs, so adding, moving or
/// clearing highlights never reshapes text or shifts line breaks.
#[derive(Clone, Debug, PartialEq)]
pub struct TextHighlight {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) bg: Color,
    pub(crate) fg: Option<Color>,
    pub(crate) ring: Option<(Color, f32)>,
    pub(crate) pad: f32,
    pub(crate) key: Option<String>,
}

impl TextHighlight {
    /// Bytes `range` of the text, painted with a rounded `bg` fill behind each line's glyphs.
    pub fn new(range: std::ops::Range<usize>, bg: Color) -> TextHighlight {
        TextHighlight {
            start: range.start.min(u32::MAX as usize) as u32,
            end: range.end.min(u32::MAX as usize) as u32,
            bg,
            fg: None,
            ring: None,
            pad: 1.5,
            key: None,
        }
    }
    /// Glyph colour over the highlighted range.
    pub fn color(mut self, c: Color) -> Self {
        self.fg = Some(c);
        self
    }
    /// Inset `c` border ring `width` points wide inside the rounded background on each line.
    pub fn ring(mut self, c: Color, width: f32) -> Self {
        self.ring = (width > 0.0 && width.is_finite()).then_some((c, width));
        self
    }
    /// Horizontal padding around each line's background, in points (`1.5` by default).
    pub fn pad(mut self, pad: f32) -> Self {
        self.pad = pad;
        self
    }
    /// Optional key for [`Tx::scroll_into_view`](crate::Tx::scroll_into_view),
    /// [`Cx::layout`](crate::Cx::layout) and [`Runtime::find`](crate::Runtime::find).
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }
    /// The UTF-8 byte range of this highlight.
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start as usize..self.end as usize
    }
    /// Returns this highlight with its UTF-8 byte range replaced by `range`.
    pub fn with_range(mut self, range: std::ops::Range<usize>) -> Self {
        self.start = range.start.min(u32::MAX as usize) as u32;
        self.end = range.end.min(u32::MAX as usize) as u32;
        self
    }
}
