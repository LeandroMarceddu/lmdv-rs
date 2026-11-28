//! Parser and model tests: fixtures, extensions, edge cases and source maps.

use std::ops::Range;
use std::sync::Arc;

use crate::model::*;
use crate::{heading_slug, legacy_slug, parse, parse_with_revision, scan_math};

const FEATURES: &str = include_str!("../../../tests/fixtures/test-features.md");
const MATH: &str = include_str!("../../../tests/fixtures/commonmark-math.md");

fn doc(source: &str) -> MdDoc {
    parse(source)
}

fn kinds(doc: &MdDoc) -> Vec<&'static str> {
    doc.blocks.iter().map(kind).collect()
}

fn kind(block: &Block) -> &'static str {
    match block {
        Block::Para(_) => "para",
        Block::Heading { .. } => "heading",
        Block::Code { .. } => "code",
        Block::List { .. } => "list",
        Block::Quote { .. } => "quote",
        Block::Table(_) => "table",
        Block::Rule => "rule",
        Block::Mermaid { .. } => "mermaid",
        Block::MathBlock { .. } => "math",
        Block::HtmlBlock(_) => "html",
        Block::Image(_) => "image",
        Block::FootnoteDefs => "footnotes",
        Block::Toc => "toc",
    }
}

fn para_texts(doc: &MdDoc) -> Vec<String> {
    doc.blocks
        .iter()
        .filter_map(|block| match block {
            Block::Para(inline) => Some(inline.text.clone()),
            _ => None,
        })
        .collect()
}

fn block_texts(block: &Block) -> Vec<String> {
    match block {
        Block::Para(inline) => vec![inline.text.clone()],
        Block::Quote { blocks, .. } => blocks.iter().flat_map(block_texts).collect(),
        Block::List { items, .. } => items.iter().flatten().flat_map(block_texts).collect(),
        _ => Vec::new(),
    }
}

fn collect_math(doc: &MdDoc) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for block in &doc.blocks {
        collect_block_math(block, &mut out);
    }
    out
}

fn collect_block_math(block: &Block, out: &mut Vec<(String, bool)>) {
    match block {
        Block::Para(inline) | Block::Heading { inline, .. } => {
            out.extend(inline.math.iter().map(|math| (math.tex.clone(), math.display)));
        }
        Block::MathBlock { tex, .. } => out.push((tex.clone(), true)),
        Block::Quote { blocks, .. } => {
            for block in blocks {
                collect_block_math(block, out);
            }
        }
        Block::List { items, .. } => {
            for block in items.iter().flatten() {
                collect_block_math(block, out);
            }
        }
        Block::Table(table) => {
            for inline in table.head.iter().chain(table.rows.iter().flatten()) {
                out.extend(inline.math.iter().map(|math| (math.tex.clone(), math.display)));
            }
        }
        _ => {}
    }
}

fn collect_callouts(doc: &MdDoc) -> Vec<Callout> {
    doc.blocks.iter().flat_map(block_callouts).collect()
}

fn block_callouts(block: &Block) -> Vec<Callout> {
    match block {
        Block::Quote { blocks, callout } => {
            callout.clone().into_iter().chain(blocks.iter().flat_map(block_callouts)).collect()
        }
        Block::List { items, .. } => items.iter().flatten().flat_map(block_callouts).collect(),
        _ => Vec::new(),
    }
}

fn source_range_of(inline: &Inline, needle: &str) -> Range<usize> {
    let start = inline.text.find(needle).expect("needle in inline text");
    inline.source_map.source_range(start..start + needle.len()).expect("mapped")
}

// ---------------------------------------------------------------- the model

#[test]
fn parsed_documents_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<MdDoc>();
    assert_send_sync::<Arc<str>>();
    assert_send_sync::<Block>();
    assert_send_sync::<Inline>();
    assert_send_sync::<HtmlNode>();
}

#[test]
fn revision_is_kept() {
    assert_eq!(doc("# x").revision, 0);
    assert_eq!(parse_with_revision("# x", 42).revision, 42);
}

#[test]
fn empty_and_whitespace_sources_parse() {
    for source in ["", "\n", "   \n\t\n"] {
        let doc = doc(source);
        assert!(doc.blocks.is_empty(), "{source:?}");
        assert!(!doc.line_starts.is_empty());
    }
}

#[test]
fn line_starts_and_line_numbers() {
    let doc = doc("a\n\n## b\n\ntext\n");
    assert_eq!(&*doc.line_starts, &[0, 2, 3, 8, 9, 14]);
    assert_eq!(doc.line_count(), 6);
    assert_eq!(doc.line_of(0), 0);
    assert_eq!(doc.line_of(2), 1);
    assert_eq!(doc.line_of(3), 2);
    assert_eq!(doc.line_of(12), 4);
    assert_eq!(doc.line_of(13), 4);
    assert_eq!(doc.line_of(14), 5);
    assert_eq!(doc.headings[0].line, 2);
    assert_eq!(doc.block_spans[2].line, 4);
    assert_eq!(doc.block_spans[2].end_line, 5);
}

#[test]
fn block_keys_are_stable_b_indices() {
    let doc = doc("# a\n\nb\n\nc\n");
    assert_eq!(doc.block_spans.len(), 3);
    for (index, span) in doc.block_spans.iter().enumerate() {
        assert_eq!(span.key, format!("b{index}"));
        assert_eq!(doc.block_key(index), Some(span.key.as_str()));
    }
    assert_eq!(MdDoc::heading_key(0), "h0");
}

// ------------------------------------------------------------- front matter

#[test]
fn front_matter_flat_subset() {
    let source = "---\ntitle: Test document\ntags: [a, b, \"c\"]\nlist:\n  - one\n  - 'two'\ndate: 2026-10-15 # comment\n---\n# Body\n";
    let doc = doc(source);
    let front = doc.front_matter.as_ref().expect("front matter");
    assert!(front.valid);
    assert_eq!(front.raw.lines().count(), 6);
    assert_eq!(front.entries.len(), 4);
    assert_eq!(front.get("title"), Some(&FmValue::Scalar("Test document".into())));
    assert_eq!(front.get("tags"), Some(&FmValue::List(vec!["a".into(), "b".into(), "c".into()])));
    assert_eq!(front.get("list"), Some(&FmValue::List(vec!["one".into(), "two".into()])));
    assert_eq!(front.get("date"), Some(&FmValue::Scalar("2026-10-15".into())));
    // The front matter is not a block, and the body starts at the same line.
    assert_eq!(kinds(&doc), ["heading"]);
    assert_eq!(doc.headings[0].line, 8);
    assert_eq!(doc.block_spans[0].source.start, source.find("# Body").unwrap());
}

#[test]
fn front_matter_offsets_and_lines_are_preserved() {
    let source = "---\ntitle: x\n---\n# H\n\nparagraph\n";
    let doc = doc(source);
    assert_eq!(doc.block_spans[0].source.start, source.find("# H").unwrap());
    assert_eq!(doc.headings[0].line, 3);
    assert_eq!(doc.block_spans[1].line, 5);
}

#[test]
fn front_matter_accepts_longer_fences_and_dots() {
    for close in ["---", "-----", "..."] {
        let source = format!("---\ntitle: x\n{close}\n# H\n");
        let doc = doc(&source);
        assert!(doc.front_matter.as_ref().is_some_and(|front| front.valid), "{close}");
    }
}

#[test]
fn front_matter_nested_or_odd_falls_back_to_raw() {
    let source = "---\ntitle: x\nnested:\n  child: y\n---\nbody\n";
    let doc = doc(source);
    let front = doc.front_matter.as_ref().expect("front matter");
    assert!(!front.valid);
    assert!(front.entries.is_empty());
    assert!(front.raw.contains("child: y"));
    assert_eq!(para_texts(&doc), ["body"]);
}

#[test]
fn front_matter_unclosed_runs_to_eof() {
    let source = "---\ntitle: x\nstill: y\n";
    let doc = doc(source);
    let front = doc.front_matter.as_ref().expect("front matter");
    assert!(front.valid);
    assert!(doc.blocks.is_empty());
}

#[test]
fn no_front_matter_when_the_document_does_not_start_with_it() {
    let doc = doc("\n---\ntitle: x\n---\n");
    assert!(doc.front_matter.is_none());
}

// ---------------------------------------------------------------- headings

#[test]
fn github_heading_slug_matches_the_original() {
    assert_eq!(heading_slug("1. Introduzione"), "1-introduzione");
    assert_eq!(heading_slug("Q&A"), "qa");
    assert_eq!(heading_slug("Attività"), "attività");
    assert_eq!(heading_slug("Test document — lmdv"), "test-document--lmdv");
    assert_eq!(heading_slug("A_B"), "a_b");
    assert_eq!(heading_slug("!!!"), "!!!");
    assert_eq!(heading_slug("$$"), "%24%24");
}

#[test]
fn legacy_slug_matches_encode_uri_component() {
    assert_eq!(legacy_slug("1. Introduzione"), "1.-introduzione");
    assert_eq!(legacy_slug("Q&A"), "q%26a");
    assert_eq!(legacy_slug("Attività"), "attivit%C3%A0");
    assert_eq!(legacy_slug("A  B"), "a-b");
    assert_eq!(legacy_slug("(x)"), "(x)");
}

#[test]
fn duplicate_heading_ids_are_numbered() {
    let doc = doc("# A\n# A\n# A\n");
    assert_eq!(doc.headings.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["a", "a-1", "a-2"]);
}

#[test]
fn duplicate_heading_ids_advance_without_restarting() {
    // Each duplicate starts where the previous one ended, so a long run of
    // identical headings stays linear (OI-144).
    let mut source = String::new();
    for _ in 0..4_000 {
        source.push_str("## Same\n\n");
    }
    let doc = doc(&source);
    assert_eq!(doc.headings.len(), 4_000);
    assert_eq!(doc.headings[0].id, "same");
    assert_eq!(doc.headings[1].id, "same-1");
    assert_eq!(doc.headings[2].id, "same-2");
    assert_eq!(doc.headings[3_999].id, "same-3999");
}

#[test]
fn a_suffix_occupied_by_another_heading_is_still_skipped() {
    let doc = doc("# Same\n\n# Same-1\n\n# Same\n\n# Same\n");
    assert_eq!(doc.headings.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["same", "same-1", "same-2", "same-3"]);
}

#[test]
fn github_and_legacy_suffixes_are_numbered_independently() {
    let doc = doc("# A & B\n\n# A & B\n\n# A & B\n");
    assert_eq!(doc.headings.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["a--b", "a--b-1", "a--b-2"]);
    assert_eq!(
        doc.headings.iter().map(|h| h.legacy_id.as_deref()).collect::<Vec<_>>(),
        [Some("a-%26-b"), Some("a-%26-b-1"), Some("a-%26-b-2")]
    );
}

#[test]
fn legacy_ids_are_recorded_when_they_differ() {
    let doc = doc("# Q&A\n# Plain\n");
    assert_eq!(doc.headings[0].id, "qa");
    assert_eq!(doc.headings[0].legacy_id.as_deref(), Some("q%26a"));
    assert_eq!(doc.headings[1].legacy_id, None);
    assert_eq!(doc.resolve_heading("q%26a"), Some(0));
    assert_eq!(doc.resolve_heading("qa"), Some(0));
}

#[test]
fn reserved_heading_ids_get_user_content_prefix() {
    let doc = doc("# Toolbar\n# mmd-12\n# tree-group-3\n");
    assert_eq!(doc.headings[0].id, "user-content-toolbar");
    assert_eq!(doc.headings[0].anchor.as_deref(), Some("toolbar"));
    assert_eq!(doc.headings[1].id, "user-content-mmd-12");
    assert_eq!(doc.headings[2].id, "user-content-tree-group-3");
    assert_eq!(doc.resolve_heading("toolbar"), Some(0));
    assert_eq!(doc.resolve_heading("user-content-toolbar"), Some(0));
}

#[test]
fn heading_text_drops_markdown_but_keeps_code() {
    let doc = doc("# **Bold** *em* `code` [link](x)\n");
    assert_eq!(doc.headings[0].text, "Bold em code link");
    assert_eq!(doc.headings[0].id, "bold-em-code-link");
}

#[test]
fn math_and_images_are_not_part_of_the_heading_title() {
    let doc = doc("# Before $x$ after\n\n# ![alt](a.png)\n");
    assert_eq!(doc.headings[0].text, "Before  after");
    assert_eq!(doc.headings[1].text, "");
}

#[test]
fn toc_covers_h1_to_h4_only() {
    let doc = doc("# A\n## B\n### C\n#### D\n##### E\n###### F\n");
    assert_eq!(
        doc.headings.iter().map(|heading| heading.in_toc).collect::<Vec<_>>(),
        [true, true, true, true, false, false]
    );
}

// ----------------------------------------------------------------- callouts

#[test]
fn all_five_callouts_parse_with_bodies() {
    let doc = doc(
        "> [!NOTE]\n> note body\n\n> [!TIP]\n> tip body\n\n> [!IMPORTANT]\n> important body\n\n> [!WARNING]\n> warning body\n\n> [!CAUTION]\n> caution body\n",
    );
    let callouts = collect_callouts(&doc);
    assert_eq!(
        callouts.iter().map(|callout| callout.kind).collect::<Vec<_>>(),
        [CalloutKind::Note, CalloutKind::Tip, CalloutKind::Important, CalloutKind::Warning, CalloutKind::Caution]
    );
    let bodies: Vec<String> = doc.blocks.iter().flat_map(block_texts).collect();
    assert_eq!(bodies.len(), 5);
    assert!(bodies[0].contains("note body"));
}

#[test]
fn callouts_take_a_custom_title_and_are_case_insensitive() {
    let doc = doc("> [!note] A custom title\n> Body.\n");
    let callout = &collect_callouts(&doc)[0];
    assert_eq!(callout.kind, CalloutKind::Note);
    assert_eq!(callout.title.as_deref(), Some("A custom title"));
    assert_eq!(block_texts(&doc.blocks[0]), ["Body."]);
}

#[test]
fn a_marker_that_does_not_open_the_first_paragraph_is_text() {
    let doc = doc("> intro\n> [!NOTE]\n");
    assert!(collect_callouts(&doc).is_empty());
    let texts = block_texts(&doc.blocks[0]);
    assert!(texts.join(" ").contains("[!NOTE]"));
}

#[test]
fn nested_quote_callouts_work() {
    let doc = doc("> > [!TIP] Inner\n> > body\n");
    let callouts = collect_callouts(&doc);
    assert_eq!(callouts.len(), 1);
    assert_eq!(callouts[0].kind, CalloutKind::Tip);
}

#[test]
fn callout_body_can_contain_lists_and_code() {
    let doc = doc("> [!WARNING]\n> Read this:\n>\n> - one\n> - two\n");
    let quote = match &doc.blocks[0] {
        Block::Quote { blocks, .. } => blocks,
        _ => panic!("quote expected"),
    };
    assert!(quote.iter().any(|block| matches!(block, Block::List { .. })));
}

#[test]
fn a_marker_inside_code_is_not_a_callout() {
    let doc = doc("> ```\n> [!NOTE]\n> ```\n");
    assert!(collect_callouts(&doc).is_empty());
}

// ---------------------------------------------------------------- footnotes

#[test]
fn footnotes_are_collected_and_numbered_by_reference() {
    let source = "First[^b] then[^a].\n\n[^a]: Note A.\n[^b]: Note *B*.\n";
    let doc = doc(source);
    assert_eq!(
        doc.footnotes.iter().map(|footnote| (footnote.label.as_str(), footnote.number)).collect::<Vec<_>>(),
        [("b", 1), ("a", 2)]
    );
    assert!(matches!(doc.blocks.last(), Some(Block::FootnoteDefs)));
    // Definitions do not render in place.
    assert!(!para_texts(&doc).iter().any(|text| text.contains("Note A")));
    let first = &doc.footnotes[0];
    assert_eq!(first.blocks.len(), 1);
    assert_eq!(doc.footnotes[0].blocks.iter().flat_map(block_texts).collect::<Vec<_>>(), ["Note B."]);
}

#[test]
fn footnote_references_are_links_with_numbers() {
    let doc = doc("Text[^1].\n\n[^1]: Note.\n");
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("paragraph"),
    };
    assert_eq!(inline.text, "Text1.");
    let link = inline.links.iter().find(|link| link.dest == "fn:1").expect("footnote link");
    assert_eq!(&inline.text[link.range.clone()], "1");
    assert!(inline.spans.iter().any(|span| span.style == InlineStyle::Sup));
}

#[test]
fn an_undefined_footnote_stays_text_with_a_diagnostic() {
    let doc = doc("Text[^missing].\n");
    assert!(doc.footnotes.is_empty());
    assert!(!doc.blocks.iter().any(|block| matches!(block, Block::FootnoteDefs)));
    assert_eq!(doc.diagnostics.len(), 1);
    assert_eq!(doc.diagnostics[0].code, DiagnosticCode::UndefinedFootnote);
    assert!(inline_text(&doc.blocks[0]).contains("[^missing]"));
}

#[test]
fn duplicate_definitions_keep_the_last_and_report_it() {
    let doc = doc("Text[^1].\n\n[^1]: First.\n\n[^1]: Second.\n");
    assert_eq!(doc.footnotes.len(), 1);
    assert_eq!(doc.footnotes[0].blocks.iter().flat_map(block_texts).collect::<Vec<_>>(), ["Second."]);
    assert!(doc.diagnostics.iter().any(|diagnostic| diagnostic.code == DiagnosticCode::DuplicateFootnote));
}

fn inline_text(block: &Block) -> String {
    match block {
        Block::Para(inline) => inline.text.clone(),
        _ => String::new(),
    }
}

// -------------------------------------------------------------------- math

#[test]
fn inline_math_follows_the_github_delimiters() {
    let cases: &[(&str, usize)] = &[
        ("a $x$ b", 1),
        ("$ x $", 0),
        ("$5 and $10", 0),
        ("\\$x\\$", 0),
        ("$a$ and $b$", 2),
        ("US$5 to $x$", 1),
        ("<span>$</span>5 for $n$", 1),
        ("$`\\sqrt{2}`$", 1),
        ("$`\\text{for all $\\alpha$}`$", 1),
        ("`$x$`", 0),
        ("[a](http://example.com/$x$)", 0),
        ("[a $x$ b](http://example.com)", 1),
    ];
    for (source, expected) in cases {
        assert_eq!(collect_math(&doc(source)).len(), *expected, "{source:?}");
    }
}

#[test]
fn backtick_math_keeps_the_tex_without_backticks() {
    let doc = doc("$`\\sqrt{2}`$ and $`\\text{for all $\\alpha$}`$\n");
    let math = collect_math(&doc);
    assert_eq!(math[0].0, "\\sqrt{2}");
    assert_eq!(math[1].0, "\\text{for all $\\alpha$}");
}

#[test]
fn display_blocks_need_a_clean_close() {
    let cases: &[(&str, bool)] = &[
        ("$$\nx\n$$", true),
        ("$$E = mc^2$$", true),
        ("$$ E = mc^2\n", false),
        ("$$ 100 l'uno", false),
        ("$$ 100 l'uno, oppure $$ 50 per due.", false),
        ("$$ a\n\nb\n$$", false),
        ("$$x$$ and text", true),
        ("$$$$", false),
        ("$$\n$$\n$$", false),
        ("```\n$$\nx\n```", false),
    ];
    for (source, expected) in cases {
        let math = collect_math(&doc(source));
        assert_eq!(!math.is_empty(), *expected, "{source:?} => {math:?}");
    }
}

#[test]
fn standalone_display_math_becomes_a_math_block() {
    let doc = doc("$$\nx^2\n$$\n");
    assert_eq!(kinds(&doc), ["math"]);
    assert_eq!(doc.blocks[0], Block::MathBlock { tex: "x^2".into(), line: 0 });
    assert_eq!(doc.block_spans[0].line, 0);
    assert_eq!(doc.block_spans[0].end_line, 3);
}

#[test]
fn display_math_in_a_sentence_stays_in_the_paragraph() {
    let doc = doc("The famous $$E = mc^2$$ formula\n");
    assert_eq!(kinds(&doc), ["para"]);
    let math = collect_math(&doc);
    assert_eq!(math, [("E = mc^2".to_string(), true)]);
    assert!(para_texts(&doc)[0].contains(OBJECT_CHAR));
}

#[test]
fn math_inside_lists_and_quotes_becomes_a_block_in_place() {
    let doc = doc("- first\n\n  $$\n  x\n  $$\n\n- second\n\n> quote\n> $$\n> y\n> $$\n> after\n");
    let list = match &doc.blocks[0] {
        Block::List { items, .. } => items,
        _ => panic!("list"),
    };
    assert!(list[0].iter().any(|block| matches!(block, Block::MathBlock { tex, .. } if tex == "x")), "{:?}", list[0]);
    let quote = match &doc.blocks[1] {
        Block::Quote { blocks, .. } => blocks,
        _ => panic!("quote"),
    };
    assert!(quote.iter().any(|block| match block {
        Block::MathBlock { tex, .. } => tex == "y",
        Block::Para(inline) => inline.math.iter().any(|math| math.tex == "y" && math.display),
        _ => false,
    }));
    assert!(quote.iter().any(|block| matches!(block, Block::Para(inline) if inline.text == "after")));
}

#[test]
fn fenced_math_ignores_words_after_the_language() {
    let parsed = doc("```math Fundamental theorem\n\\frac{a}{b}\n```\n");
    assert_eq!(parsed.blocks[0], Block::MathBlock { tex: "\\frac{a}{b}".into(), line: 0 });
    let tilde = doc("~~~math\nx\n~~~\n");
    assert_eq!(tilde.blocks[0], Block::MathBlock { tex: "x".into(), line: 0 });
}

#[test]
fn math_is_not_scanned_inside_code_or_raw_html_blocks() {
    let doc = doc("```\n$x$\n```\n\n<div>\n$y$\n</div>\n");
    assert!(collect_math(&doc).is_empty());
}

#[test]
fn math_span_rewrite_keeps_length_and_lines() {
    let source = "a $x$ b\n\n$$\ny^2\n$$\n\n```\n$z$\n```\n";
    let scan = scan_math(source);
    assert_eq!(scan.text.len(), source.len());
    assert_eq!(scan.text.matches('\n').count(), source.matches('\n').count());
    assert_eq!(scan.spans.len(), 2);
    assert_eq!(scan.placeholders.len(), 2);
    for (span, &placeholder) in scan.spans.iter().zip(&scan.placeholders) {
        assert!(span.source.contains(&placeholder));
        assert_eq!(&scan.text[placeholder..placeholder + 3], "\u{FFFC}");
    }
}

#[test]
fn math_placeholders_never_overwrite_a_newline() {
    // A formula line with three or more bytes uses U+FFFC on that line.
    let source = "$$\nx^2\n$$\n";
    let scan = scan_math(source);
    let placeholder = scan.placeholders[0];
    assert!(placeholder > 0);
    assert_eq!(scan.text.matches('\n').count(), source.matches('\n').count());
    assert!(scan.text[placeholder..].starts_with(OBJECT_CHAR));

    // A one-byte formula line uses the one-byte sentinel at the span start;
    // every newline is still preserved.
    let short = scan_math("$$\nx\n$$\n");
    assert_eq!(short.text.matches('\n').count(), "$$\nx\n$$\n".matches('\n').count());
    assert_eq!(short.placeholders[0], short.spans[0].source.start);
    assert!(short.text[short.placeholders[0]..].starts_with('\u{1}'));
}

// --------------------------------------------------------------------- TOC

#[test]
fn toc_placeholder_becomes_a_marker() {
    for marker in ["[TOC]", "[[TOC]]", "[toc]", "  [[Toc]]  "] {
        let doc = doc(&format!("{marker}\n\n# A\n## B\n"));
        assert_eq!(kinds(&doc), ["toc", "heading", "heading"], "{marker:?}");
    }
}

#[test]
fn toc_requires_the_whole_paragraph() {
    let doc = doc("[TOC] and more\n");
    assert_eq!(kinds(&doc), ["para"]);
}

#[test]
fn toc_placeholder_is_real_content_and_headings_after_it_are_marked() {
    let doc = doc("[TOC]\n\n# A\n\n### C\n\n##### E\n");
    assert_eq!(doc.blocks[0], Block::Toc);
    assert_eq!(doc.headings.iter().map(|heading| heading.in_toc).collect::<Vec<_>>(), [true, true, false]);
}

// -------------------------------------------------------------------- HTML

#[test]
fn tagfilter_shows_unsafe_tags_as_text() {
    let parsed = doc("<title>x</title>\n");
    assert_eq!(parsed.blocks.len(), 1);
    let text = match &parsed.blocks[0] {
        Block::HtmlBlock(node) => node.text(),
        Block::Para(inline) => inline.text.clone(),
        other => panic!("unexpected block: {other:?}"),
    };
    assert!(text.contains("<title>x</title>"), "{text:?}");
}

#[test]
fn unknown_tags_keep_their_children_as_text() {
    let parsed = doc("<name>visible</name>\n");
    let text = match &parsed.blocks[0] {
        Block::HtmlBlock(node) => node.text(),
        Block::Para(inline) => inline.text.clone(),
        other => panic!("unexpected block: {other:?}"),
    };
    assert_eq!(text, "visible");
    let empty = doc("<name></name>\n");
    assert!(empty.blocks.is_empty());
}

#[test]
fn dropped_elements_drop_their_content() {
    for source in ["<object data=x>fallback</object>\n", "<embed src=x>\n"] {
        let doc = doc(source);
        assert!(doc.blocks.is_empty(), "{source:?}");
    }
}

#[test]
fn details_picture_and_media_are_preserved() {
    let doc = doc("<details><summary>More</summary>\n\nBody\n\n</details>\n");
    let node = match &doc.blocks[0] {
        Block::HtmlBlock(node) => node,
        other => panic!("html block: {other:?}"),
    };
    assert!(node_to_tags(node).contains(&"details".to_owned()));
    assert!(node_to_tags(node).contains(&"summary".to_owned()));
}

fn node_to_tags(node: &HtmlNode) -> Vec<String> {
    let mut out = Vec::new();
    if node.tag != HtmlTag::Fragment {
        out.push(node.tag.name().to_owned());
    }
    for child in &node.children {
        if let HtmlChild::Node(node) = child {
            out.extend(node_to_tags(node));
        }
    }
    out
}

#[test]
fn inline_html_maps_to_styles_and_objects() {
    let doc = doc("Press <kbd>Ctrl</kbd> then <br> go and x<sup>2</sup> and <mark>hi</mark>.\n");
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    assert!(inline.spans.iter().any(|span| span.style == InlineStyle::Kbd));
    assert!(inline.spans.iter().any(|span| span.style == InlineStyle::Sup));
    assert!(inline.spans.iter().any(|span| span.style == InlineStyle::Mark));
    assert!(inline.objects.iter().any(|object| object.kind == InlineObjectKind::LineBreak));
    assert!(inline.text.contains('\n'));
}

#[test]
fn html_attributes_are_filtered_and_ids_namespaced() {
    let doc = doc(
        "<div id=\"x\" style=\"color:red\" onmouseover=\"x\" class=\"other gh-dark-mode-only\" data-x=\"1\">t</div>\n",
    );
    let node = match &doc.blocks[0] {
        Block::HtmlBlock(node) => node,
        _ => panic!("html block"),
    };
    let div = child_node(node, "div").expect("div");
    assert_eq!(div.attr("id"), Some("user-content-x"));
    assert_eq!(div.attr("class"), Some("gh-dark-mode-only"));
    assert!(div.attr("style").is_none());
    assert!(div.attr("onmouseover").is_none());
    assert!(div.attr("data-x").is_none());
}

fn child_node<'a>(node: &'a HtmlNode, tag: &str) -> Option<&'a HtmlNode> {
    for child in &node.children {
        if let HtmlChild::Node(node) = child {
            if node.tag.name() == tag {
                return Some(node);
            }
            if let Some(found) = child_node(node, tag) {
                return Some(found);
            }
        }
    }
    None
}

#[test]
fn html_images_keep_srcset_and_lose_scripts() {
    let doc = doc("<img src=\"a.png\" srcset=\"a.png 1x, b,c.png 2x\" onerror=\"x\">\n");
    let image = match &doc.blocks[0] {
        Block::Image(image) => image,
        other => panic!("image: {other:?}"),
    };
    assert_eq!(image.src, "a.png");
    assert_eq!(image.srcset.len(), 2);
    assert_eq!(image.srcset[1].url, "b,c.png");
    assert_eq!(image.srcset[1].descriptor.as_deref(), Some("2x"));
}

#[test]
fn unsafe_urls_are_dropped() {
    let doc = doc("<a href=\"javascript:alert(1)\">x</a>\n");
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => return,
    };
    assert!(!inline.links.iter().any(|link| link.dest.contains("javascript")));
}

#[test]
fn url_paths_decode_one_layer_and_never_fail() {
    // L-15: `decodeURIComponent` with a raw fallback.
    assert_eq!(crate::decode_url_path("my%20notes.md"), "my notes.md");
    assert_eq!(crate::decode_url_path("100% done.md"), "100% done.md", "a stray % stays as written");
    assert_eq!(crate::decode_url_path("100%25 done.md"), "100% done.md");
    assert_eq!(crate::decode_url_path("a%2520b.png"), "a%20b.png", "exactly one layer");
    assert_eq!(crate::decode_url_path("%FF.png"), "\u{FFFD}.png", "invalid UTF-8 is lossy, never a panic");
}

#[test]
fn srcset_drops_network_device_and_unc_candidates() {
    // M-26: an off-limits candidate never outlives the attribute filter. A
    // local spelling, including an ordinary `///abs` path, stays as written
    // with its descriptor.
    let doc = doc(
        r#"<img src="local.png" srcset="//cdn.test/a.png 2x, \\server\share\b.png 3x, C:/pics/c.png 4x, local.png 1x, ///abs/d.png 5x" alt="x">"#,
    );
    let image = match &doc.blocks[0] {
        Block::Image(image) => image,
        other => panic!("image: {other:?}"),
    };
    let urls: Vec<(&str, Option<&str>)> =
        image.srcset.iter().map(|candidate| (candidate.url.as_str(), candidate.descriptor.as_deref())).collect();
    assert_eq!(urls, [("local.png", Some("1x")), ("///abs/d.png", Some("5x"))]);
    assert_eq!(image.src, "local.png", "the src spelling is not a candidate and is untouched");
}

#[test]
fn picture_sources_also_drop_network_srcset_candidates() {
    let doc = doc(r#"<picture>
<source media="(min-width: 800px)" srcset="//cdn.test/wide.png 1x, wide.png 2x">
<img src="small.png" alt="Logo">
</picture>
"#);
    let image = match &doc.blocks[0] {
        Block::Image(image) => image,
        other => panic!("image: {other:?}"),
    };
    assert_eq!(image.picture[0].srcset.len(), 1);
    assert_eq!(image.picture[0].srcset[0].url, "wide.png");
}

// ------------------------------------------------------------------ L-12

fn collect_tag<'a>(node: &'a HtmlNode, tag: &str, out: &mut Vec<&'a HtmlNode>) {
    if node.tag.name() == tag {
        out.push(node);
    }
    for child in &node.children {
        if let HtmlChild::Node(child) = child {
            collect_tag(child, tag, out);
        }
    }
}

#[test]
fn image_map_areas_and_svg_links_keep_only_safe_destinations() {
    let doc = doc(
        r##"<div><map name="map1"><area href="#one" alt="One" shape="rect" coords="0,0,10,10"><area href="javascript:alert(1)" alt="Bad"></map><svg onload="alert(2)"><script>alert(3)</script><a href="#a">A</a><a xlink:href="#b">B</a><a xlink:href="javascript:alert(4)">C</a></svg></div>
"##,
    );
    let fragment = match &doc.blocks[0] {
        Block::HtmlBlock(node) => node,
        other => panic!("html block: {other:?}"),
    };
    let map = child_node(fragment, "map").expect("the map is part of the safe subset");
    assert_eq!(map.attr("name"), Some("map1"));
    let mut areas = Vec::new();
    collect_tag(map, "area", &mut areas);
    assert_eq!(areas.len(), 2);
    assert_eq!(areas[0].attr("href"), Some("#one"));
    assert_eq!(areas[0].attr("alt"), Some("One"));
    assert_eq!(areas[0].attr("shape"), Some("rect"));
    assert_eq!(areas[0].attr("coords"), Some("0,0,10,10"));
    assert_eq!(areas[1].attr("href"), None, "a script destination is dropped");

    let svg = child_node(fragment, "svg").expect("the svg container is part of the safe subset");
    assert!(svg.attr("onload").is_none(), "an event handler never survives");
    let mut anchors = Vec::new();
    collect_tag(svg, "a", &mut anchors);
    assert_eq!(anchors.len(), 3);
    assert_eq!(anchors[0].attr("href"), Some("#a"));
    // L-12: `xlink:href` is stored under `href`, so the inline parser and a
    // future link menu read one name; html5ever already did this for SVG's
    // XLink namespace, and `filter_attrs` does it for the literal spelling.
    assert_eq!(anchors[1].attr("href"), Some("#b"));
    assert!(anchors[1].attr("xlink:href").is_none());
    let third = anchors[2].attr("href");
    assert_eq!(third, None, "a script destination is dropped");
    // GitHub's tagfilter escaped `<script>` before the fragment was parsed, so
    // it is inert text here, never an element.
    let mut scripts = Vec::new();
    collect_tag(fragment, "script", &mut scripts);
    assert!(scripts.is_empty(), "a script never becomes an element: {fragment:?}");
}

#[test]
fn an_inline_xlink_href_names_a_link_and_keeps_the_url_policy() {
    let parsed = doc("Before <a xlink:href=\"#part\">jump</a> after\n");
    let inline = match &parsed.blocks[0] {
        Block::Para(inline) => inline,
        other => panic!("para: {other:?}"),
    };
    assert!(inline.links.iter().any(|link| link.dest == "#part"), "{:?}", inline.links);
    let bad = doc("Before <a xlink:href=\"javascript:alert(1)\">bad</a> after\n");
    let inline = match &bad.blocks[0] {
        Block::Para(inline) => inline,
        other => panic!("para: {other:?}"),
    };
    assert!(inline.links.iter().all(|link| !link.dest.contains("javascript")), "{:?}", inline.links);
}

#[test]
fn srcset_parser_handles_commas_and_trailing_commas() {
    let candidates = crate::html::parse_srcset("a,b.png 1x, c.png 2x,d.png,");
    assert_eq!(candidates.len(), 3);
    assert_eq!(candidates[0].url, "a,b.png");
    assert_eq!(candidates[1].url, "c.png");
    assert_eq!(candidates[2].url, "d.png");
}

#[test]
fn srcset_candidates_are_sanitized_like_src() {
    // Every candidate goes through the URL policy, and the descriptor and
    // candidate shape survive (OI-146).
    let doc = doc(
        "<img src=\"a.png\" srcset=\"javascript:alert(1) 1x, vbscript:msgbox(1) 2x, data:text/html;base64,PHNjcmlwdD4= 3x, b.png 4x\" alt=\"x\">\n",
    );
    let image = match &doc.blocks[0] {
        Block::Image(image) => image,
        other => panic!("image: {other:?}"),
    };
    assert_eq!(image.srcset.len(), 1);
    assert_eq!(image.srcset[0].url, "b.png");
    assert_eq!(image.srcset[0].descriptor.as_deref(), Some("4x"));
}

#[test]
fn picture_sources_keep_only_safe_srcset_candidates() {
    let doc = doc(
        "<picture>\n<source media=\"(min-width: 800px)\" srcset=\"javascript:alert(1) 1x, data:image/png;base64,iVBOR 2x\">\n<img src=\"small.png\" alt=\"Logo\">\n</picture>\n",
    );
    let image = match &doc.blocks[0] {
        Block::Image(image) => image,
        other => panic!("image: {other:?}"),
    };
    assert_eq!(image.src, "small.png");
    assert_eq!(image.picture.len(), 1);
    assert_eq!(image.picture[0].media.as_deref(), Some("(min-width: 800px)"));
    assert_eq!(image.picture[0].srcset.len(), 1);
    assert_eq!(image.picture[0].srcset[0].url, "data:image/png;base64,iVBOR");
    assert_eq!(image.picture[0].srcset[0].descriptor.as_deref(), Some("2x"));
}

// ------------------------------------------------------------------ tables

#[test]
fn tables_keep_alignment_and_inline_cells() {
    let doc = doc("| A | B | C |\n|:--|:-:|--:|\n| 1 | **2** | `3` |\n| 4 | 5 | 6 |\n");
    let table = match &doc.blocks[0] {
        Block::Table(table) => table,
        _ => panic!("table"),
    };
    assert_eq!(table.align, [Align::Left, Align::Center, Align::Right]);
    assert_eq!(table.head.len(), 3);
    assert_eq!(table.rows.len(), 2);
    assert_eq!(table.head[0].text, "A");
    assert_eq!(table.rows[0][1].text, "2");
    assert!(table.rows[0][1].spans.iter().any(|span| span.style == InlineStyle::Strong));
    assert!(table.rows[0][2].spans.iter().any(|span| span.style == InlineStyle::Code));
    assert_eq!(doc.block_spans[0].line, 0);
    assert_eq!(doc.block_spans[0].end_line, 4);
}

#[test]
fn a_short_or_long_row_is_padded_to_the_columns() {
    let doc = doc("| A | B | C |\n|---|---|---|\n| 1 |\n");
    let table = match &doc.blocks[0] {
        Block::Table(table) => table,
        _ => panic!("table"),
    };
    assert_eq!(table.head.len(), 3);
    assert_eq!(table.rows[0].len(), 3);
    assert!(table.rows[0][2].text.is_empty());
}

// -------------------------------------------------------------- autolinks

#[test]
fn explicit_urls_and_www_are_linkified() {
    let doc = doc("See https://example.com/a, www.example.com and mail@example.com.\n");
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    let destinations: Vec<&str> = inline.links.iter().map(|link| link.dest.as_str()).collect();
    assert!(destinations.contains(&"https://example.com/a"));
    assert!(destinations.contains(&"http://www.example.com"));
    assert!(destinations.contains(&"mailto:mail@example.com"));
}

#[test]
fn bare_filenames_and_partial_www_stay_text() {
    let doc = doc("README.md main.rs file.csv dev-www.example.com and foo.www.example.com.\n");
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    assert!(inline.links.is_empty(), "{:?}", inline.links);
}

#[test]
fn angle_autolinks_are_kept_by_pulldown() {
    let doc = doc("<mail@example.com> and <https://example.com>\n");
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    assert!(inline.links.iter().any(|link| link.dest.starts_with("mailto:")), "{:?}", inline.links);
    assert!(inline.links.iter().any(|link| link.dest == "https://example.com"));
}

#[test]
fn links_do_not_linkify_inside_themselves() {
    let doc = doc("[https://inner.example](https://outer.example)\n");
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    assert_eq!(inline.links.len(), 1);
    assert_eq!(inline.links[0].dest, "https://outer.example");
}

// ------------------------------------------------------------- typography

#[test]
fn quotes_curl_but_replacements_do_not() {
    let doc = doc("Quotes like \"these\" and it's -- (c) ... stay.\n");
    let text = &para_texts(&doc)[0];
    assert!(text.contains('“') && text.contains('”'));
    assert!(text.contains("it’s"));
    assert!(text.contains("--"));
    assert!(text.contains("(c)"));
    assert!(text.contains("..."));
}

// ------------------------------------------------------- tasks and images

#[test]
fn task_lists_are_live_checkbox_objects() {
    let doc = doc("- [x] done\n- [ ] todo\n");
    let list = match &doc.blocks[0] {
        Block::List { items, tight, .. } => (items, tight),
        _ => panic!("list"),
    };
    assert!(*list.1);
    assert_eq!(list.0.len(), 2);
    let checked: Vec<bool> = list
        .0
        .iter()
        .filter_map(|item| item.first())
        .filter_map(|block| match block {
            Block::Para(inline) => inline.objects.iter().find_map(|object| match object.kind {
                InlineObjectKind::TaskCheckbox { checked } => Some(checked),
                _ => None,
            }),
            _ => None,
        })
        .collect();
    assert_eq!(checked, [true, false]);
    // The marker is mapped back to the source so a click can write the file.
    let inline = match &list.0[0][0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    let source = inline.source_map.source_of(0).expect("marker source");
    let text = "- [x] done\n";
    assert_eq!(&text[source..source + 3], "[x]");
}

#[test]
fn markdown_images_are_block_level() {
    let doc = doc("![Alt text](./img/a.png \"Title\")\n");
    assert_eq!(
        doc.blocks[0],
        Block::Image(ImageBlock {
            src: "./img/a.png".into(),
            alt: "Alt text".into(),
            title: Some("Title".into()),
            srcset: Vec::new(),
            picture: Vec::new(),
            source: 0..33,
            line: 0,
        })
    );
}

#[test]
fn an_image_inside_a_sentence_stays_inline() {
    let doc = doc("Before ![alt](a.png) after\n");
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    assert!(inline.objects.iter().any(|object| object.kind == InlineObjectKind::Image));
    assert!(inline.text.contains(OBJECT_CHAR));
}

// ------------------------------------------------------------ source maps

#[test]
fn inline_source_map_maps_rendered_text_to_source() {
    let source = "Hello **dark** world\n";
    let doc = doc(source);
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    let range = source_range_of(inline, "dark");
    assert_eq!(&source[range], "dark");
    let hello = source_range_of(inline, "Hello");
    assert_eq!(&source[hello], "Hello");
}

#[test]
fn source_maps_survive_emphasis_code_and_links() {
    let source = "A **b** `c` [d](e) f\n";
    let doc = doc(source);
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    for needle in ["A", "b", "c", "d", "f"] {
        let range = source_range_of(inline, needle);
        assert_eq!(&source[range], needle, "{needle}");
    }
}

#[test]
fn source_ranges_stop_at_the_end_of_a_quote_expansion() {
    // `a'` renders as `a’`: one source byte becomes three rendered bytes, so
    // the linear endpoint must be clamped to the inline's source end (OI-145).
    let source = "a'";
    let doc = doc(source);
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    assert_eq!(inline.text, "a\u{2019}");
    let quote = inline.text.find('\u{2019}').expect("curled quote");
    assert_eq!(inline.source_map.source_range(quote..inline.text.len()), Some(1..2));
    let whole = inline.source_map.source_range(0..inline.text.len()).expect("mapped");
    assert_eq!(whole, 0..2);
    assert!(whole.end <= source.len(), "{whole:?} past the source");
}

#[test]
fn source_ranges_clamp_an_interior_quote_expansion() {
    let source = "a' b\n";
    let doc = doc(source);
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    let quote = inline.text.find('\u{2019}').expect("curled quote");
    // The next mapped run starts at source byte 2, so the quote ends there
    // instead of two bytes into the following text.
    assert_eq!(inline.source_map.source_range(quote..quote + 3), Some(1..2));
    assert_eq!(source_range_of(inline, "b"), 3..4);
}

#[test]
fn block_spans_cover_the_whole_block() {
    let source = "# H\n\npara one\n\n---\n\n```rs\ncode\n```\n";
    let doc = doc(source);
    assert_eq!(doc.block_spans.len(), 4);
    let paragraph = &doc.block_spans[1];
    assert_eq!(source[paragraph.source.clone()].trim_end(), "para one");
    assert_eq!(source[doc.block_spans[2].source.clone()].trim_end(), "---");
}

// --------------------------------------------------------------- fixtures

#[test]
fn test_features_fixture_projection() {
    let doc = doc(FEATURES);
    assert_eq!(doc.headings.len(), 13);
    assert_eq!(doc.footnotes.len(), 2);
    assert!(doc.diagnostics.is_empty());
    assert_eq!(
        kinds(&doc),
        [
            "heading",
            "para",
            "heading",
            "para",
            "quote",
            "heading",
            "table",
            "heading",
            "list",
            "heading",
            "code",
            "code",
            "heading",
            "mermaid",
            "heading",
            "mermaid",
            "heading",
            "image",
            "heading",
            "para",
            "heading",
            "para",
            "para",
            "math",
            "para",
            "math",
            "para",
            "math",
            "heading",
            "quote",
            "quote",
            "quote",
            "quote",
            "quote",
            "heading",
            "para",
            "heading",
            "para",
            "footnotes",
        ]
    );
    assert_eq!(doc.headings[0].id, "test-document--lmdv");
    assert_eq!(doc.headings[9].id, "math-katex");
    assert_eq!(doc.headings[9].legacy_id.as_deref(), Some("math-(katex)"));
    assert_eq!(doc.footnotes.iter().map(|footnote| footnote.label.as_str()).collect::<Vec<_>>(), ["1", "note"]);
    let callouts = collect_callouts(&doc);
    assert_eq!(callouts.len(), 5);
    assert_eq!(callouts[4].kind, CalloutKind::Caution);
    let math = collect_math(&doc);
    assert!(math.iter().any(|(tex, display)| tex == "E = mc^2" && !display));
    assert!(math.iter().any(|(tex, display)| *display && tex.contains("\\int")));
    assert_eq!(doc.front_matter.as_ref().map(|front| front.entries.len()), Some(5));
}

#[test]
fn commonmark_math_fixture_projection() {
    let doc = doc(MATH);
    assert_eq!(doc.headings.len(), 30);
    assert!(doc.footnotes.is_empty());
    assert!(doc.diagnostics.is_empty());
    assert!(doc.front_matter.is_none());
    let math = collect_math(&doc);
    assert!(math.len() >= 20, "{math:?}");
    assert!(math.iter().any(|(tex, display)| tex == "\\sqrt{2}" && !display));
    assert!(math.iter().any(|(tex, display)| tex == "\\text{for all $\\alpha$}" && !display));
    assert!(math.iter().any(|(tex, display)| tex == "a + b\n= c" && *display));
    assert!(math.iter().any(|(tex, display)| tex == "\\sum_{k=1}^{n} k = \\frac{n(n+1)}{2}" && *display));
    assert!(!math.iter().any(|(tex, _)| tex == "100 l'uno"));
    // Code fences and the blank-line formula stay text.
    let joined = para_texts(&doc).join("\n");
    assert!(joined.contains("$$ 100 l’uno"));
    assert!(joined.contains("Empty $$$$"));
    let code = doc.blocks.iter().find_map(|block| match block {
        Block::Code { text, .. } => Some(text),
        _ => None,
    });
    assert_eq!(code.map(String::as_str), Some("$$\nx\n$$"));
}

// ------------------------------------------------------- malformed input

#[test]
fn malformed_input_never_panics() {
    let cases: &[&str] = &[
        "$",
        "$$",
        "$$$",
        "$$$$",
        "$`",
        "`$",
        "---",
        "---\n",
        "---\ntitle",
        "[^",
        "[^]:",
        "[^x]:",
        "<",
        "<div",
        "<div>",
        "</",
        "<script>",
        "> [!NOTE",
        "> [!",
        "| a |",
        "| a | b |\n|---|---|\n|",
        "- ",
        "- [",
        "1. ",
        "\u{FFFC}",
        "\u{FFFC}$x$\u{FFFC}",
        "[[TOC]",
        "\\",
        "****",
        "~~~~",
        "` `",
        "# ",
        "# \u{FFFC}",
    ];
    for source in cases {
        let _ = doc(source);
    }
    let deep = format!("{}{}", "> ".repeat(200), "text");
    let _ = doc(&deep);
    let list = format!("{}- x\n", "  ".repeat(100));
    let _ = doc(&list);
    let long = format!("${}$", "x".repeat(10_000));
    let parsed = doc(&long);
    assert_eq!(collect_math(&parsed).len(), 1);
}

// --------------------------------------------------------------- helpers

// ------------------------------------------------------- additions round 2

#[test]
fn footnotes_expose_the_first_reference_for_backlinks() {
    let doc = doc("A[^x] then again[^x].\n\n[^x]: Note.\n");
    let footnote = &doc.footnotes[0];
    let reference = footnote.first_ref.clone().expect("first reference");
    assert_eq!(&doc.source[reference.clone()], "[^x]");
    assert_eq!(doc.block_index_at(reference.start), Some(0));
    // The reference is a link with the expected destination.
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    assert_eq!(inline.links.iter().filter(|link| link.dest == "fn:x").count(), 2);
}

#[test]
fn footnote_definitions_may_have_several_blocks() {
    let doc = doc("Text[^n].\n\n[^n]: First paragraph.\n\n    Second paragraph with a list:\n\n    - one\n    - two\n");
    let footnote = &doc.footnotes[0];
    assert!(footnote.blocks.len() >= 2, "{:?}", footnote.blocks);
    assert!(footnote.blocks.iter().any(|block| matches!(block, Block::List { .. })));
}

#[test]
fn block_index_at_maps_source_offsets() {
    let source = "# H\n\npara one\n\n---\n\npara two\n";
    let parsed = doc(source);
    assert_eq!(parsed.block_index_at(0), Some(0));
    assert_eq!(parsed.block_index_at(source.find("para one").unwrap()), Some(1));
    assert_eq!(parsed.block_index_at(source.find("---").unwrap()), Some(2));
    assert_eq!(parsed.block_index_at(source.find("para two").unwrap()), Some(3));
    assert_eq!(parsed.block_index_at(source.len()), Some(3));
    assert_eq!(doc("").block_index_at(0), None);
}

#[test]
fn github_line_breaks_become_spaces_or_hard_breaks() {
    let doc = doc("A\nB\n\nC  \nD\n\nE\\\nF\n");
    let paragraphs = para_texts(&doc);
    assert_eq!(paragraphs[0], "A B");
    assert_eq!(paragraphs[1], "C\nD");
    assert_eq!(paragraphs[2], "E\nF");
    let hard = match &doc.blocks[1] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    assert!(hard.objects.iter().any(|object| object.kind == InlineObjectKind::LineBreak));
}

#[test]
fn display_math_may_break_a_line_just_inside() {
    let doc = doc("text $$ \n x \n $$ more\n");
    let math = collect_math(&doc);
    assert_eq!(math, [("x".to_string(), true)]);
}

#[test]
fn math_works_in_table_cells() {
    let doc = doc("| a | b |\n|---|---|\n| $x$ | $y^2$ |\n");
    let table = match &doc.blocks[0] {
        Block::Table(table) => table,
        _ => panic!("table"),
    };
    assert_eq!(table.rows[0][0].math[0].tex, "x");
    assert_eq!(table.rows[0][1].math[0].tex, "y^2");
}

#[test]
fn a_picture_block_is_an_image_with_sources() {
    let doc = doc(
        "<picture>\n<source media=\"(min-width: 800px)\" srcset=\"wide.png 1x, wide2.png 2x\">\n<img src=\"small.png\" alt=\"Logo\">\n</picture>\n",
    );
    let image = match &doc.blocks[0] {
        Block::Image(image) => image,
        other => panic!("image: {other:?}"),
    };
    assert_eq!(image.src, "small.png");
    assert_eq!(image.alt, "Logo");
    assert_eq!(image.picture.len(), 1);
    assert_eq!(image.picture[0].media.as_deref(), Some("(min-width: 800px)"));
    assert_eq!(image.picture[0].srcset.len(), 2);
}

#[test]
fn a_loose_list_with_two_paragraphs_in_one_item_is_not_tight() {
    let parsed = doc("- one\n\n  two\n");
    match &parsed.blocks[0] {
        Block::List { tight, items, .. } => {
            assert!(!tight);
            assert_eq!(items[0].len(), 2);
        }
        other => panic!("list: {other:?}"),
    }
    let tight = doc("- one\n- two\n");
    assert!(matches!(&tight.blocks[0], Block::List { tight: true, .. }));
}

#[test]
fn callouts_work_inside_a_list_item_and_not_for_a_list_inside_a_quote() {
    let inside_list = doc("- > [!NOTE]\n  > body\n");
    assert_eq!(collect_callouts(&inside_list).len(), 1);

    let inside_quote = doc("> - [!NOTE]\n>   body\n");
    assert!(collect_callouts(&inside_quote).is_empty());
}

#[test]
fn an_anchor_without_href_is_not_a_link() {
    let doc = doc("<a name=\"x\">text</a>\n");
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        Block::HtmlBlock(node) => {
            assert_eq!(node.text(), "text");
            return;
        }
        other => panic!("unexpected block: {other:?}"),
    };
    assert_eq!(inline.text, "text");
    assert!(inline.links.is_empty());
}

#[test]
fn media_objects_are_kept_inline() {
    let doc = doc("Before <video controls src=\"a.mp4\"></video> after\n");
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    assert!(inline.objects.iter().any(|object| object.kind == InlineObjectKind::Video));
}

#[test]
fn documents_serialize_for_goldens() {
    let doc = doc("# H\n\nA **b** [c](d) $x$\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
    let value = serde_json::to_value(&doc).expect("serialize");
    assert_eq!(value["blocks"].as_array().map(Vec::len), Some(3));
    assert_eq!(value["headings"][0]["id"], "h");
    assert!(value["line_starts"].is_array());
    let back: MdDoc = serde_json::from_value(value).expect("deserialize");
    assert_eq!(back.source, doc.source);
    assert_eq!(back.blocks.len(), doc.blocks.len());
}

#[test]
fn a_void_embed_does_not_swallow_following_text() {
    let doc = doc("Before <embed src=\"x\"> after\n");
    let text = match &doc.blocks[0] {
        Block::Para(inline) => inline.text.clone(),
        other => panic!("unexpected block: {other:?}"),
    };
    assert!(text.contains("after"), "{text:?}");
}

#[test]
fn a_literal_object_character_does_not_steal_a_formula() {
    let doc = doc("\u{FFFC} and $x$\n");
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    assert_eq!(inline.text.matches(OBJECT_CHAR).count(), 2);
    assert_eq!(inline.math.len(), 1);
    assert_eq!(inline.math[0].tex, "x");
}

#[test]
fn footnote_like_text_in_front_matter_is_not_diagnosed() {
    let doc = doc("---\nreference: \"[^x]\"\n---\nBody without notes.\n");
    assert!(!doc.diagnostics.iter().any(|diagnostic| diagnostic.code == DiagnosticCode::UndefinedFootnote));
}

#[test]
fn an_empty_nested_block_does_not_skip_the_next_one() {
    let doc = doc("> before[^x]\n>\n> [^x]: note\n>\n> after\n");
    let quote = match &doc.blocks[0] {
        Block::Quote { blocks, .. } => blocks,
        _ => panic!("quote"),
    };
    assert!(quote.iter().any(|block| matches!(block, Block::Para(inline) if inline.text.contains("after"))));
}

#[test]
fn inline_display_math_keeps_the_paragraph_across_a_line_break() {
    let doc = doc("text $$\nx\n$$ more\n");
    assert_eq!(kinds(&doc), ["para"]);
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    assert_eq!(inline.math.len(), 1);
    assert!(inline.math[0].display);
    assert_eq!(inline.math[0].tex, "x");
    assert!(inline.text.starts_with("text "));
    assert!(inline.text.ends_with(" more"));
}

#[test]
fn setext_headings_and_ordered_list_starts_work() {
    let doc = doc("Guide\n=====\n\n3. three\n4. four\n");
    assert_eq!(doc.headings[0].level, 1);
    assert_eq!(doc.headings[0].text, "Guide");
    assert_eq!(doc.headings[0].id, "guide");
    match &doc.blocks[1] {
        Block::List { ordered, start, items, .. } => {
            assert!(ordered);
            assert_eq!(*start, 3);
            assert_eq!(items.len(), 2);
        }
        other => panic!("list: {other:?}"),
    }
}

#[test]
fn multi_line_paragraph_spans_end_after_the_last_line() {
    let doc = doc("first\nsecond\n\nthird\n");
    assert_eq!(doc.block_spans[0].line, 0);
    assert_eq!(doc.block_spans[0].end_line, 2);
    assert_eq!(doc.block_spans[1].line, 3);
}

#[test]
fn code_blocks_keep_language_and_text() {
    let fenced = doc("```typescript\nlet x = 1;\n```\n");
    match &fenced.blocks[0] {
        Block::Code { lang, text, .. } => {
            assert_eq!(lang.as_deref(), Some("typescript"));
            assert_eq!(text, "let x = 1;");
        }
        other => panic!("code: {other:?}"),
    }
    let indented = doc("    plain\n    code\n");
    assert!(matches!(&indented.blocks[0], Block::Code { lang: None, .. }));
}

#[test]
fn every_code_block_has_its_text_in_drawing_order() {
    let model = doc(concat!(
        "```rust\nlet top = 1;\n```\n\n",
        "> ```rust\n> let quoted = 2;\n> ```\n\n",
        "- item\n\n  ```rust\n  let listed = 3;\n  ```\n\n",
        "<div><pre><code>let raw = 4;</code></pre></div>\n\n",
        "Footnote.[^1]\n\n[^1]: ```rust\n    let foot = 5;\n    ```\n",
    ));
    let texts: Vec<String> = model.code_texts().into_iter().map(|text| text.into_owned()).collect();
    assert_eq!(texts, vec!["let top = 1;", "let quoted = 2;", "let listed = 3;", "let raw = 4;", "let foot = 5;"]);
    // Two identical blocks stay two, in order (M-13's duplicated case).
    let twice = doc("```\nsame\n```\n\n```\nsame\n```\n");
    assert_eq!(twice.code_texts(), vec!["same", "same"]);
    assert!(doc("no code here\n").code_texts().is_empty());
    // A `<pre>` inside an HTML table cell is drawn inline, not as a code
    // block, so it is not a copy destination.
    assert!(doc("<table><tr><td><pre>not code</pre></td></tr></table>\n").code_texts().is_empty());
    // A block's destinations begin where the code blocks before it end.
    assert_eq!(model.code_count_before(0), 0);
    assert_eq!(model.code_texts().len(), 5);
    assert_eq!(
        model.code_count_before(model.blocks.len()),
        4,
        "the footnote's block is numbered inside its own section"
    );
}

#[test]
fn strikethrough_is_a_style_span() {
    let doc = doc("before ~~gone~~ after\n");
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        _ => panic!("para"),
    };
    assert!(inline.spans.iter().any(|span| span.style == InlineStyle::Strike));
    assert_eq!(inline.text, "before gone after");
}

#[test]
fn nested_lists_stay_inside_their_item() {
    let doc = doc("- outer\n  - inner\n  - second\n- last\n");
    let outer = match &doc.blocks[0] {
        Block::List { items, .. } => items,
        _ => panic!("list"),
    };
    assert_eq!(outer.len(), 2);
    assert!(outer[0].iter().any(|block| matches!(block, Block::List { .. })));
}

#[test]
fn multi_line_inline_display_math_stays_inside_a_quote() {
    let doc = doc("> text $$\n> x\n> $$ more\n");
    let quote = match &doc.blocks[0] {
        Block::Quote { blocks, .. } => blocks,
        other => panic!("quote: {other:?}"),
    };
    assert_eq!(quote.len(), 1, "{quote:?}");
    let inline = match &quote[0] {
        Block::Para(inline) => inline,
        other => panic!("para: {other:?}"),
    };
    assert_eq!(inline.math.len(), 1);
    assert!(inline.math[0].display);
    assert_eq!(inline.math[0].tex, "x");
    assert!(inline.text.starts_with("text "));
    assert!(inline.text.ends_with(" more"));
}

// ------------------------------------------------ HTML model and SVG geometry

fn marked_svg_nodes(node: &HtmlNode, out: &mut Vec<HtmlNode>) {
    for child in &node.children {
        if let HtmlChild::Node(child) = child {
            if child.attr(crate::html::SVG_SHAPE_ATTR).is_some() {
                out.push(child.clone());
            }
            marked_svg_nodes(child, out);
        }
    }
}

#[test]
fn svg_shapes_keep_geometry_and_drop_event_handlers() {
    let doc = doc(concat!(
        "<div><svg viewBox=\"0 0 100 50\" onload=\"evil()\">",
        "<rect x=\"1\" y=\"2\" width=\"10\" height=\"20\" fill=\"#f00\" onclick=\"evil()\"/>",
        "<g transform=\"translate(3,4)\"><circle cx=\"5\" cy=\"6\" r=\"7\"/></g>",
        "<path d=\"M0 0 L9 9\" fill-rule=\"evenodd\"/>",
        "<a href=\"javascript:evil()\"><line x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"/></a>",
        "</svg></div>\n",
    ));
    let fragment = match &doc.blocks[0] {
        Block::HtmlBlock(node) => node,
        other => panic!("html block: {other:?}"),
    };
    let svg = child_node(fragment, "svg").expect("the svg container is part of the safe subset");
    assert_eq!(svg.tag, HtmlTag::Svg);
    assert_eq!(svg.attr("viewbox"), Some("0 0 100 50"));
    assert!(svg.attr("onload").is_none());
    let mut shapes = Vec::new();
    marked_svg_nodes(svg, &mut shapes);
    let names: Vec<&str> = shapes.iter().filter_map(|node| node.attr(crate::html::SVG_SHAPE_ATTR)).collect();
    assert_eq!(names, ["rect", "g", "circle", "path", "line"]);
    assert_eq!(shapes[0].attr("fill"), Some("#f00"));
    assert_eq!(shapes[0].attr("x"), Some("1"));
    assert!(shapes[0].attr("onclick").is_none());
    assert_eq!(shapes[2].attr("r"), Some("7"));
    assert_eq!(shapes[3].attr("fill-rule"), Some("evenodd"));
    assert_eq!(shapes[3].attr("d"), Some("M0 0 L9 9"));
    let anchors = {
        let mut out = Vec::new();
        collect_tag(svg, "a", &mut out);
        out
    };
    assert!(anchors[0].attr("href").is_none(), "unsafe SVG hrefs stay refused");
}

#[test]
fn inline_svg_in_a_paragraph_records_shapes_and_keeps_link_runs() {
    let doc = doc(concat!(
        "Before <svg viewBox=\"0 0 10 10\" width=\"10\" height=\"10\" onload=\"evil()\">",
        "<rect x=\"0\" y=\"0\" width=\"10\" height=\"10\" fill=\"#f00\"/>",
        "<a href=\"#dest\">Go<circle cx=\"5\" cy=\"5\" r=\"2\" onclick=\"evil()\"/></a>",
        "<a href=\"javascript:evil()\">Bad</a>",
        "</svg> after\n",
    ));
    let Block::Para(inline) = &doc.blocks[0] else { panic!("a paragraph: {:?}", doc.blocks[0]) };
    assert_eq!(inline.text, format!("Before {}GoBad after", crate::model::OBJECT_CHAR));
    let objects: Vec<_> = inline
        .objects
        .iter()
        .filter(|object| object.html.as_ref().is_some_and(|node| node.tag == HtmlTag::Svg))
        .collect();
    assert_eq!(objects.len(), 1, "one inline svg object");
    let node = objects[0].html.as_ref().unwrap();
    assert_eq!(node.attr("viewbox"), Some("0 0 10 10"));
    assert!(node.attr("onload").is_none());
    let mut shapes = Vec::new();
    marked_svg_nodes(node, &mut shapes);
    let names: Vec<&str> = shapes.iter().filter_map(|node| node.attr(crate::html::SVG_SHAPE_ATTR)).collect();
    assert_eq!(names, ["rect", "circle"]);
    assert_eq!(shapes[0].attr("fill"), Some("#f00"));
    assert!(shapes[1].attr("onclick").is_none(), "shape event handlers stay refused");
    let dests: Vec<&str> = inline.links.iter().map(|link| link.dest.as_str()).collect();
    assert_eq!(dests, ["#dest"], "safe svg links stay in the paragraph, unsafe ones do not");
}

#[test]
fn inline_svg_without_drawable_shapes_keeps_its_text_and_links() {
    let doc = doc("A <svg viewBox=\"0 0 10 10\"><a href=\"#x\">Hi</a></svg> B\n");
    let Block::Para(inline) = &doc.blocks[0] else { panic!("a paragraph: {:?}", doc.blocks[0]) };
    assert_eq!(inline.text, format!("A {}Hi B", crate::model::OBJECT_CHAR));
    let object = inline
        .objects
        .iter()
        .find(|object| object.html.as_ref().is_some_and(|node| node.tag == HtmlTag::Svg))
        .expect("the svg object exists even without shapes");
    let mut shapes = Vec::new();
    marked_svg_nodes(object.html.as_ref().unwrap(), &mut shapes);
    assert!(shapes.is_empty(), "unsupported svg has no shapes to draw");
    assert_eq!(inline.links.len(), 1, "its link text still works");
    assert_eq!(inline.links[0].dest, "#x");
}

#[test]
fn entity_and_escape_boundaries_map_back_to_their_source_spelling() {
    let source = "A &amp; B \\*C\\* &copy; D\n";
    let doc = doc(source);
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        other => panic!("para: {other:?}"),
    };
    assert_eq!(inline.text, "A & B *C* © D");
    assert_eq!(source_range_of(inline, "&"), 2..3);
    assert_eq!(source_range_of(inline, "*"), 11..12);
    assert_eq!(source_range_of(inline, "C"), 12..13);
    assert_eq!(source_range_of(inline, "©"), 16..17);
    assert_eq!(source_range_of(inline, "D"), 23..24);
}

#[test]
fn inline_picture_keeps_its_sources_and_emits_one_object() {
    let source = concat!(
        "Before <picture><source media=\"(prefers-color-scheme: dark)\" ",
        "srcset=\"dark.png 1x, javascript:evil() 2x\">",
        "<img src=\"light.png\" alt=\"Logo\"></picture> After\n",
    );
    let doc = doc(source);
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        other => panic!("para: {other:?}"),
    };
    let pictures: Vec<&InlineObject> =
        inline.objects.iter().filter(|object| object.kind == InlineObjectKind::Picture).collect();
    assert_eq!(pictures.len(), 1, "one picture object: {:?}", inline.objects);
    assert!(!inline.objects.iter().any(|object| object.kind == InlineObjectKind::Image));
    let picture = pictures[0].html.as_ref().expect("picture node");
    assert_eq!(picture.tag, HtmlTag::Picture);
    let nodes: Vec<&HtmlNode> = picture
        .children
        .iter()
        .filter_map(|child| match child {
            HtmlChild::Node(node) => Some(node),
            _ => None,
        })
        .collect();
    assert_eq!(nodes.len(), 2, "one source and one img: {nodes:?}");
    assert_eq!(nodes[0].tag, HtmlTag::Source);
    assert_eq!(nodes[0].attr("media"), Some("(prefers-color-scheme: dark)"));
    assert_eq!(nodes[0].srcset.len(), 1, "the unsafe candidate was dropped");
    assert_eq!(nodes[0].srcset[0].url, "dark.png");
    assert_eq!(nodes[1].tag, HtmlTag::Img);
    assert_eq!(nodes[1].attr("src"), Some("light.png"));
}

#[test]
fn two_inline_pictures_each_keep_their_own_source() {
    let source = concat!(
        "<picture><source srcset=\"first.png 1x\"><img src=\"first-fallback.png\"></picture> and ",
        "<picture><source srcset=\"second.png 1x\"><img src=\"second-fallback.png\"></picture>\n",
    );
    let doc = doc(source);
    let inline = match &doc.blocks[0] {
        Block::Para(inline) => inline,
        other => panic!("para: {other:?}"),
    };
    let pictures: Vec<&InlineObject> =
        inline.objects.iter().filter(|object| object.kind == InlineObjectKind::Picture).collect();
    assert_eq!(pictures.len(), 2, "one object per picture: {:?}", inline.objects);
    assert!(!inline.objects.iter().any(|object| object.kind == InlineObjectKind::Image));
    for (picture, name) in pictures.iter().zip(["first.png", "second.png"]) {
        let node = picture.html.as_ref().expect("picture node");
        let sources: Vec<&HtmlNode> = node
            .children
            .iter()
            .filter_map(|child| match child {
                HtmlChild::Node(child) if child.tag == HtmlTag::Source => Some(child),
                _ => None,
            })
            .collect();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].srcset[0].url, name);
    }
}

#[test]
fn details_body_after_a_blank_line_stays_in_the_card() {
    let doc = doc("<details open><summary>More</summary>\n\nBody text\n\n</details>\n");
    assert_eq!(doc.blocks.len(), 1, "the blank lines no longer split the card");
    let fragment = match &doc.blocks[0] {
        Block::HtmlBlock(node) => node,
        other => panic!("html block: {other:?}"),
    };
    let node = child_node(fragment, "details").expect("the details card");
    assert_eq!(node.tag, HtmlTag::Details);
    assert!(node.attr("open").is_some(), "the open state is kept");
    assert!(node.text().contains("Body text"), "the card owns its body");
}

// ---- OI-024: source-range find belongs to the parser ----------------------

#[test]
fn find_matches_drawn_text_as_source_ranges_in_document_order() {
    let doc = doc(
        "# Alpha Title\n\nAlpha and **alpha**.\n\n| Field | Value |\n| --- | --- |\n| Alpha | x |\n\n```rust\nlet alpha = 1;\n```\n",
    );
    let hits = doc.find("alpha");
    assert_eq!(hits.len(), 5, "heading, two paragraph hits, cell and code: {hits:?}");
    for hit in &hits {
        assert_eq!(doc.source[hit.clone()].to_lowercase(), "alpha", "a hit names drawn text, not a marker");
    }
    assert!(hits.windows(2).all(|pair| pair[0].start < pair[1].start), "document order");
    assert!(doc.find("").is_empty(), "an empty query matches nothing");
    assert!(doc.find("rust").is_empty(), "a code fence's language is not drawn text");
}

#[test]
fn find_folds_case_without_regex_or_accent_folding() {
    let doc = doc("Beyoncé BEYONCÉ\n\nAkeneo and akeneo.\n");
    assert_eq!(doc.find("beyoncé").len(), 2, "case folding covers non-ASCII");
    assert!(doc.find("beyonce").is_empty(), "no accent folding (R-18)");
    assert_eq!(doc.find("AKENEO").len(), 2);
    assert!(doc.find("Akeneo|akeneo").is_empty(), "no regex");
    assert!(doc.find("a.*o").is_empty(), "no regex");
}

#[test]
fn find_keeps_byte_offsets_through_multibyte_lowercasing() {
    // `İ` lowercases to two characters; the hit still covers the original char.
    assert_eq!(crate::match_ranges("İstanbul", "i\u{307}stanbul"), vec![0..9]);
    assert_eq!(crate::match_ranges("AKENEO akeneo", "akeneo"), vec![0..6, 7..13]);
    let doc = doc("İstanbul\n");
    assert_eq!(doc.find("i\u{307}stanbul"), vec![0..9]);
}

#[test]
fn find_marks_and_link_destinations_are_not_searchable() {
    let doc = doc("[**Alpha**](https://example.com/secret)\n\n`alpha`\n\n> alpha\n\n- alpha\n");
    let hits = doc.find("alpha");
    assert_eq!(hits.len(), 4, "link text, code span, quote and list item: {hits:?}");
    for hit in &hits {
        assert_eq!(doc.source[hit.clone()].to_lowercase(), "alpha");
    }
    assert!(doc.find("secret").is_empty(), "a destination is not drawn text");
    assert!(doc.find("https").is_empty());
}

#[test]
fn find_walks_footnote_definitions_and_nested_blocks() {
    let doc = doc("Alpha[^1]\n\n[^1]: alpha in the footnote\n    - nested alpha\n");
    let hits = doc.find("alpha");
    assert_eq!(hits.len(), 3, "reference, definition text and nested list: {hits:?}");
    assert!(hits.iter().any(|hit| hit.start > 10), "the definition body is searched too: {hits:?}");
    for hit in &hits {
        assert_eq!(doc.source[hit.clone()].to_lowercase(), "alpha");
    }
}
