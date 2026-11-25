//! Static SVG for a laid-out scene.
//!
//! The scene's finished fills and strokes become paths, its labels become
//! `<text>` elements in the diagram's own size, weight and family, and a title
//! (when the source has one) sits above the drawing. The result is one
//! self-contained document: no scripts, no images, and no font or stylesheet
//! references, so a rasterizer or an exported page needs nothing else.
//!
//! Text is SVG text rather than outlines: the rasterizer or viewer shapes it
//! with the family the diagram names, exactly as the app names a font slot. A
//! wrapped label is split into one `<text>` per line at the same width the
//! layout measured, so it keeps the diagram's line breaks.

use std::fmt::Write as _;

use rune::{Align, Color, Family, LineCap, TextStyle};

use crate::DiagramStyle;
use crate::canvas::{Label, Op, Scene, TITLE_GAP, measure, title_font};

impl Scene {
    /// This scene as a static SVG document at [`Scene::size`], in points.
    ///
    /// The card's surface and border frame the drawing, so a light diagram on
    /// white paper and a dark one on a dark page rasterize as they are read.
    pub(crate) fn svg(&self, paint: &DiagramStyle) -> String {
        let title = self.title.as_deref().map(|title| {
            let font = title_font(paint);
            let measured = measure(title, font);
            (title, font, measured)
        });
        let [diagram_width, diagram_height] = self.size;
        let (title_width, title_height) = title.as_ref().map_or((0.0, 0.0), |(_, _, m)| (m.size[0], m.size[1]));
        let width = if title.is_some() { diagram_width.max(title_width + 2.0) } else { diagram_width };
        let height = diagram_height + title.as_ref().map_or(0.0, |_| title_height + TITLE_GAP);
        let dx = (width - diagram_width) / 2.0;
        let dy = title.as_ref().map_or(0.0, |_| title_height + TITLE_GAP);

        let mut out = String::with_capacity(512 + self.svg_ops.len() * 56);
        write!(
            out,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}">"#,
            num(width),
            num(height),
            num(width),
            num(height),
        )
        .ok();
        write!(out, r#"<rect x="0" y="0" width="{}" height="{}" rx="6""#, num(width), num(height)).ok();
        color_attr(&mut out, "fill", paint.surface);
        color_attr(&mut out, "stroke", paint.surface_border);
        out.push_str(r#" stroke-width="0.5"/>"#);
        if let Some((title, font, measured)) = &title {
            let at = [(width - measured.size[0]) / 2.0, 0.0];
            push_lines(&mut out, title, at, measured.size[0], *font, paint.text, Align::Center, measured.wrap);
        }
        write!(out, r#"<g transform="translate({} {}) scale({})">"#, num(dx), num(dy), num(self.scale)).ok();
        for op in &self.svg_ops {
            push_op(&mut out, op);
        }
        for label in &self.labels {
            push_label(&mut out, label);
        }
        out.push_str("</g></svg>");
        out
    }
}

/// The diagram's own font family as an SVG family name. Slot families become
/// their generic names; a named family keeps its CSS list.
fn family(font: TextStyle) -> &'static str {
    match font.family {
        Family::Sans => "sans-serif",
        Family::Mono | Family::SystemMono => "monospace",
        Family::Named(name) => name,
    }
}

fn line_height(font: TextStyle) -> f32 {
    if font.line_height > 0.0 { font.line_height } else { font.size * 1.25 }
}

/// The first line's baseline below a box's top, from the line height and the
/// font's em box. Rune's text shaping has no public ascent here, so the em box
/// is split the way most UI faces do (0.8 above, 0.2 below the baseline).
fn baseline(font: TextStyle) -> f32 {
    let line = line_height(font);
    ((line - font.size) / 2.0).max(0.0) + font.size * 0.8
}

fn push_label(out: &mut String, label: &Label) {
    push_lines(out, &label.text, label.at, label.size[0], label.font, label.color, label.align, label.wrap);
}

/// One label or title: its text split into measured lines and anchored in its
/// box.
#[allow(clippy::too_many_arguments)]
fn push_lines(
    out: &mut String,
    text: &str,
    at: [f32; 2],
    box_width: f32,
    font: TextStyle,
    color: Color,
    align: Align,
    wrap: bool,
) {
    let lines = lines(text, font, box_width, wrap);
    let line = line_height(font);
    let base = baseline(font);
    for (index, text) in lines.iter().enumerate() {
        if text.is_empty() {
            continue;
        }
        let y = at[1] + index as f32 * line + base;
        let (x, anchor) = match align {
            Align::Start => (at[0], "start"),
            Align::Center => (at[0] + box_width / 2.0, "middle"),
            Align::End => (at[0] + box_width, "end"),
        };
        push_text(out, text, x, y, font, color, anchor);
    }
}

/// Text split at its measured width when it wraps, and at its hard newlines
/// always.
fn lines(text: &str, font: TextStyle, width: f32, wrap: bool) -> Vec<String> {
    let mut out = Vec::new();
    for hard in text.split('\n') {
        if !wrap || width <= 0.0 {
            out.push(hard.to_owned());
            continue;
        }
        let mut line = String::new();
        for word in hard.split_whitespace() {
            let candidate = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
            let fits = rune::text::measure(&candidate, &[], font, None).0 <= width;
            if fits || line.is_empty() {
                line = candidate;
            } else {
                out.push(std::mem::take(&mut line));
                line = word.to_owned();
            }
        }
        out.push(line);
    }
    out
}

fn push_text(out: &mut String, text: &str, x: f32, y: f32, font: TextStyle, color: Color, anchor: &str) {
    write!(out, r#"<text x="{}" y="{}""#, num(x), num(y)).ok();
    write!(out, r#" font-family="{}""#, escape_attr(family(font))).ok();
    write!(out, r#" font-size="{}""#, num(font.size)).ok();
    if (font.weight - 400.0).abs() > 0.5 {
        write!(out, r#" font-weight="{}""#, (font.weight / 100.0).round() * 100.0).ok();
    }
    if font.italic {
        out.push_str(r#" font-style="italic""#);
    }
    if font.tracking.abs() > 0.001 {
        write!(out, r#" letter-spacing="{}""#, num(font.tracking)).ok();
    }
    if anchor != "start" {
        write!(out, r#" text-anchor="{anchor}""#).ok();
    }
    color_attr(out, "fill", color);
    out.push('>');
    escape_text(out, text);
    out.push_str("</text>");
}

fn push_op(out: &mut String, op: &Op) {
    match op {
        Op::Fill(points, color) => {
            if points.len() < 3 {
                return;
            }
            out.push_str(r#"<path d=""#);
            path(out, points, true);
            out.push('"');
            color_attr(out, "fill", *color);
            out.push_str(r#" fill-rule="evenodd"/>"#);
        }
        Op::Stroke { points, closed, width, cap, color } => {
            if points.len() < 2 || *width <= 0.0 {
                return;
            }
            out.push_str(r#"<path d=""#);
            path(out, points, *closed);
            out.push_str(r#"" fill="none""#);
            color_attr(out, "stroke", *color);
            write!(
                out,
                r#" stroke-width="{}" stroke-linecap="{}" stroke-linejoin="round"/>"#,
                num(*width),
                if *cap == LineCap::Round { "round" } else { "butt" },
            )
            .ok();
        }
    }
}

fn path(out: &mut String, points: &[[f32; 2]], closed: bool) {
    for (index, point) in points.iter().enumerate() {
        let command = if index == 0 { 'M' } else { 'L' };
        write!(out, "{command}{} {}", num(point[0]), num(point[1])).ok();
    }
    if closed {
        out.push('Z');
    }
}

/// `name="#rrggbb"` and, for translucent ink, `name-opacity`.
fn color_attr(out: &mut String, name: &str, color: Color) {
    let hex = color.hex();
    write!(out, r#" {name}="{}""#, &hex[..7]).ok();
    if color.a < 0.999 {
        write!(out, r#" {name}-opacity="{}""#, num(color.a)).ok();
    }
}

fn escape_text(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

fn escape_attr(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// A number with at most two decimals and no trailing zeros, so a document
/// with a thousand points stays small.
fn num(value: f32) -> String {
    if !value.is_finite() || value.abs() < 0.005 {
        return "0".to_owned();
    }
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
#[path = "svg_tests.rs"]
mod tests;
