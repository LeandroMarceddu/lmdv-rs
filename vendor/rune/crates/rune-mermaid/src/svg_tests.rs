//! The static SVG serializer: size, title, text, escaping and self-containment.

use rune::Color;

use crate::{DiagramStyle, Hover, prepare_drawing, render_drawing, render_svg};

const FLOW: &str = "flowchart LR\n    A[Source] --> B{Parse}\n    B -->|ok| C[Render]\n    B -->|error| D[Notice]";

fn attribute(svg: &str, name: &str) -> f32 {
    let needle = format!("{name}=\"");
    let start =
        svg.find(&needle).unwrap_or_else(|| panic!("no {name} in {}", &svg[..svg.len().min(200)])) + needle.len();
    let end = start + svg[start..].find('"').expect("a closing quote");
    svg[start..end].parse().unwrap_or_else(|_| panic!("{name} is not a number: {}", &svg[start..end]))
}

#[test]
fn a_flowchart_becomes_one_self_contained_svg() {
    let paint = DiagramStyle::neutral(false);
    let svg = render_svg(FLOW, &paint).expect("the flowchart draws");
    assert!(svg.starts_with("<svg "), "{svg:.120}");
    assert!(svg.ends_with("</svg>"));
    let drawing = render_drawing(FLOW, &paint, Hover::Off, Color::TRANSPARENT).expect("draws");
    assert!((attribute(&svg, "width") - drawing.size[0]).abs() < 0.01, "the document is the drawing's width");
    assert!(svg.contains("viewBox=\"0 0 "), "a viewBox sizes the drawing");
    assert!(svg.contains("<path "), "fills and strokes are paths");
    assert!(svg.contains("<text "), "labels are text");
    for label in ["Source", "Parse", "Render", "Notice"] {
        assert!(svg.contains(label), "missing label {label}");
    }
    // The namespace is the document's only external-looking URL.
    assert_eq!(svg.matches("http").count(), 1, "one SVG namespace, no external reference");
    for forbidden in ["<script", "<image", "href=", "url(", "@import", "<foreignObject"] {
        assert!(!svg.contains(forbidden), "the document carries {forbidden}");
    }
    assert!(!svg.contains("flowchart"), "the source syntax never reaches the drawing: {svg:.200}");
}

#[test]
fn the_title_is_measured_above_the_drawing() {
    let paint = DiagramStyle::neutral(true);
    let src = "---\ntitle: Deploy\n---\nflowchart LR\n    A --> B";
    let drawing = render_drawing(src, &paint, Hover::Off, Color::TRANSPARENT).expect("draws");
    let svg = drawing.to_svg();
    assert!(svg.contains(">Deploy</text>"), "the title is drawn: {svg:.300}");
    assert!((attribute(&svg, "width") - drawing.size[0]).abs() < 0.01);
    assert!((attribute(&svg, "height") - drawing.size[1]).abs() < 0.01);
}

#[test]
fn a_title_makes_the_document_taller_than_its_drawing() {
    let paint = DiagramStyle::neutral(false);
    let untitled = render_svg(FLOW, &paint).expect("draws");
    let titled = format!("---\ntitle: A title\n---\n{FLOW}");
    let titled = render_svg(&titled, &paint).expect("draws");
    assert!(attribute(&titled, "height") > attribute(&untitled, "height"), "the title adds a row");
    assert!(titled.contains(">A title</text>"));
}

#[test]
fn label_text_is_xml_escaped_and_kept_verbatim() {
    let svg = render_svg("flowchart LR\n    A[\"x < y & z\"] --> B", &DiagramStyle::neutral(false)).expect("draws");
    assert!(svg.contains("x &lt; y &amp; z</text>"), "escaped label: {svg:.400}");
    assert!(!svg.contains("< y & z"), "raw markup never reaches the document");
}

#[test]
fn a_long_label_wraps_into_several_text_lines() {
    // `measure` wraps labels over 200 points; the serializer splits them at the
    // measured width, so one label becomes more than one `<text>`.
    let label = "a label that is deliberately far too long for one diagram line and keeps going";
    let source = format!("flowchart LR\n    A[{label}]");
    let svg = render_svg(&source, &DiagramStyle::neutral(false)).expect("draws");
    assert!(svg.matches("<text ").count() > 1, "the long label wrapped: {svg:.400}");
    assert!(svg.contains("a label that is deliberately"), "the first line is present");
    assert!(svg.contains("keeps going"), "the last line is present");
}

#[test]
fn a_width_limited_diagram_shrinks_the_group_not_the_document() {
    let mut paint = DiagramStyle::neutral(false);
    paint.max_width = 120.0;
    let drawing = render_drawing(
        "flowchart LR\n    A[A very wide source node label] --> B[And another wide target]",
        &paint,
        Hover::Off,
        Color::TRANSPARENT,
    )
    .expect("draws");
    let svg = drawing.to_svg();
    assert!(svg.contains(" scale("), "the drawing is fitted: {svg:.300}");
    let scale = svg
        .split(" scale(")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .and_then(|value| value.parse::<f32>().ok())
        .expect("a scale in the group");
    assert!((0.0..1.0).contains(&scale), "the fitted scale is under one: {scale}");
    assert!((attribute(&svg, "width") - drawing.size[0]).abs() < 0.01);
}

#[test]
fn the_model_and_the_drawing_produce_the_same_document() {
    let paint = DiagramStyle::neutral(false);
    let drawing = render_drawing(FLOW, &paint, Hover::Off, Color::TRANSPARENT).expect("draws");
    let model = prepare_drawing(FLOW, &paint, Hover::Off, Color::TRANSPARENT).expect("prepares");
    assert_eq!(drawing.to_svg(), model.to_svg(), "one layout, one document");
}

#[test]
fn every_supported_family_serializes_its_drawing() {
    let sources = [
        "flowchart TD\n  subgraph S [Box]\n    A --> B\n  end\n  B -.-> C",
        "sequenceDiagram\n  participant A\n  participant B\n  A->>B: hi\n  B-->>A: back",
        "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Busy : go",
        "classDiagram\n  Animal <|-- Duck\n  class Animal {\n    +int age\n  }",
        "erDiagram\n  CUSTOMER ||--o{ ORDER : places",
        "gantt\n  title Plan\n  section Work\n  Code :a1, 2024-01-01, 3d",
        "pie title Pets\n  \"Dogs\" : 3\n  \"Cats\" : 1",
        "mindmap\n  root\n    child",
        "timeline\n  2024 : one\n  2025 : two",
        "journey\n  title Day\n  section Work\n    Code: 5: Me",
        "gitGraph\n  commit\n  branch dev\n  commit\n  checkout main\n  merge dev",
        "quadrantChart\n  x-axis Low --> High\n  y-axis Low --> High\n  A: [0.3, 0.6]",
        "xychart-beta\n  x-axis [a, b]\n  bar [3, 5]",
        "sankey-beta\n  A,B,3\n  B,C,2",
        "block-beta\n  columns 2\n  A B",
        "requirementDiagram\n  requirement req {\n    id: 1\n    text: must work\n  }",
        "C4Context\n  Person(a, \"A user\")",
    ];
    for source in sources {
        let svg = render_svg(source, &DiagramStyle::neutral(false)).unwrap_or_else(|| panic!("serializes {source:?}"));
        assert!(svg.contains("<path "), "drawing paths for {source:?}");
        assert!(svg.contains("<text "), "labels for {source:?}");
        assert!(svg.ends_with("</svg>"));
    }
}

#[test]
fn unsupported_sources_have_no_document() {
    assert!(render_svg("", &DiagramStyle::neutral(false)).is_none());
    assert!(render_svg("not a diagram ->", &DiagramStyle::neutral(false)).is_none());
}
