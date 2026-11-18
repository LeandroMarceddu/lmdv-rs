//! P7-SEC: deterministic security and pathology corpus for the Markdown parser.
//!
//! Untrusted documents reach `lmdv_md::parse` on a worker thread. This suite is
//! the gate that says a hostile or merely broken document cannot panic, hang,
//! or produce a model that breaks the renderer's range assumptions:
//!
//! * empty, whitespace-only and BOM/CRLF/tab documents;
//! * 200+ levels of block quotes and lists, deep emphasis/link/HTML nesting;
//! * 100k-character lines, words and CJK runs, and very many headings, links,
//!   formulas, list items, footnotes and table cells;
//! * unclosed or mismatched fences, code spans, math delimiters, HTML tags and
//!   comments, YAML front matter and footnote references;
//! * NUL/control characters, bidi controls, combining marks and a UTF-8 BOM;
//! * `[TOC]`, duplicate/reserved anchors, `javascript:`/`data:` links and
//!   images, `<script>`/`<style>`/event-handler HTML;
//! * a fixed-seed random corpus (2,000 cases) built only from valid `str`.
//!
//! Every case is parsed inside `catch_unwind` and checked against the same
//! invariants (`check_doc`): byte ranges in bounds and on char boundaries,
//! block keys and indices valid, block spans ordered, every inline range
//! inside its rendered text, complete source-map coverage, rectangular tables,
//! footnote numbering, diagnostic ranges, and the HTML attribute allowlist.
//! The per-case wall-clock budget is deliberately generous (5 s in debug): it
//! is there to catch a hang, not to benchmark the parser.
//!
//! Parsing is pure: this suite performs no file or network I/O and has no
//! external fuzzer dependency.

use std::any::Any;
use std::ops::Range;
use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use lmdv_md::{
    Block, Diagnostic, Footnote, Heading, HtmlChild, HtmlNode, HtmlTag, Inline, InlineObjectKind, MdDoc, OBJECT_CHAR,
    scan_math,
};

/// A case is allowed 5 s in a debug build; anything near this is a hang.
const CASE_BUDGET: Duration = Duration::from_secs(5);

/// Attributes the parser is allowed to keep (`html::filter_attrs` plus the
/// attributes of a Markdown image). Anything else is a security regression.
const ALLOWED_ATTRS: &[&str] = &[
    "href", "src", "alt", "title", "width", "height", "media", "type", "start", "colspan", "rowspan", "align",
    "poster", "controls", "loop", "muted", "preload", "kind", "srclang", "label", "id", "class",
];

struct Case {
    name: String,
    source: String,
}

fn case(name: &str, source: impl Into<String>) -> Case {
    Case { name: name.to_owned(), source: source.into() }
}

fn add_case(cases: &mut Vec<Case>, name: &str, source: impl Into<String>) {
    cases.push(case(name, source));
}

fn repeated(pattern: &str, times: usize) -> String {
    pattern.repeat(times)
}

/// A bullet list nested `depth` levels with two-space indentation.
fn nested_list(depth: usize) -> String {
    let mut out = String::new();
    for level in 0..depth {
        out.push_str(&"  ".repeat(level));
        out.push_str("- item ");
        out.push_str(&level.to_string());
        out.push('\n');
    }
    out
}

/// A GFM table with `rows` body rows and `columns` columns.
fn wide_table(rows: usize, columns: usize) -> String {
    let mut out = String::new();
    out.push('|');
    for column in 0..columns {
        out.push_str(&format!(" h{column} |"));
    }
    out.push('\n');
    out.push('|');
    for column in 0..columns {
        out.push_str(if column % 2 == 0 { " --- |" } else { " :---: |" });
    }
    out.push('\n');
    for row in 0..rows {
        out.push('|');
        for column in 0..columns {
            out.push_str(&format!(" r{row}c{column} |"));
        }
        out.push('\n');
    }
    out
}

fn corpus() -> Vec<Case> {
    let mut cases = Vec::new();

    // -------------------------------------------------------- real fixtures
    add_case(&mut cases, "fixture-test-features", include_str!("../../../tests/fixtures/test-features.md"));
    add_case(&mut cases, "fixture-commonmark-math", include_str!("../../../tests/fixtures/commonmark-math.md"));

    // ---------------------------------------------------------- empty / EOL
    for (name, source) in [
        ("empty", ""),
        ("space", " "),
        ("spaces", "     "),
        ("newline", "\n"),
        ("blank-lines", "\n\n\n"),
        ("whitespace-lines", " \n\t\n   \n"),
        ("tabs-only", "\t\t\t"),
        ("crlf-empty", "\r\n\r\n"),
        ("cr-only", "\r\r\r"),
        ("bom-only", "\u{feff}"),
        ("bom-and-newline", "\u{feff}\n"),
        ("nul-only", "\0"),
        ("nul-lines", "\0\n\0\0\n"),
        ("replacement-char", "\u{FFFD}\u{FFFD}\n"),
        ("noncharacters", "\u{FFFE}\u{FFFF}\n"),
    ] {
        add_case(&mut cases, name, source.to_owned());
    }

    // ------------------------------------------------------ deep block nesting
    add_case(&mut cases, "blockquote-220", format!("{}deep\n", repeated("> ", 220)));
    add_case(&mut cases, "blockquote-500-no-spaces", format!("{}text\n", repeated(">", 500)));
    add_case(
        &mut cases,
        "blockquote-300-lazy",
        format!("{}# Heading\n\n{}para\n", repeated("> ", 300), repeated("> ", 300)),
    );
    add_case(&mut cases, "blockquote-list-alternation", format!("{}x\n", repeated("> - ", 160)));
    add_case(&mut cases, "nested-lists-220", nested_list(220));
    add_case(&mut cases, "nested-lists-400", nested_list(400));
    let deep_quote = repeated("> ", 200);
    add_case(
        &mut cases,
        "nested-quote-math",
        format!("{deep_quote}$$\n{deep_quote}x\n{deep_quote}$$\n{deep_quote}after\n"),
    );
    add_case(&mut cases, "deep-footnote-quote", format!("ref[^a]\n\n[^a]: {}note\n", repeated("> ", 220)));
    add_case(&mut cases, "deep-html-blocks", format!("{}text{}", repeated("<div>\n", 400), repeated("</div>\n", 400)));
    add_case(&mut cases, "deep-inline-html", format!("{}x{}\n", repeated("<b>", 2_000), repeated("</b>", 2_000)));

    // ------------------------------------------- deep inline emphasis / links
    add_case(&mut cases, "emphasis-600", format!("{}x{}\n", repeated("*", 600), repeated("*", 600)));
    add_case(&mut cases, "underscore-600", format!("{}x{}\n", repeated("_", 600), repeated("_", 600)));
    add_case(&mut cases, "strong-300", format!("{}x{}\n", repeated("**", 300), repeated("**", 300)));
    add_case(&mut cases, "strike-400", format!("{}x{}\n", repeated("~~", 400), repeated("~~", 400)));
    add_case(&mut cases, "links-400", format!("{}x{}\n", repeated("[", 400), repeated("](u)", 400)));
    add_case(&mut cases, "images-300", format!("{}x{}\n", repeated("![", 300), repeated("](i)", 300)));
    add_case(&mut cases, "emphasis-inside-link", format!("{}x{}](u)\n", repeated("[*", 300), repeated("*", 300)));

    // ------------------------------------------------------ long lines / words
    add_case(&mut cases, "word-100k", "a".repeat(100_000));
    add_case(&mut cases, "word-100k-newline", format!("{}\n", "a".repeat(100_000)));
    add_case(&mut cases, "spaces-100k", format!("{}x\n", " ".repeat(100_000)));
    add_case(&mut cases, "hashes-100k", format!("{}\n", "#".repeat(100_000)));
    add_case(&mut cases, "dashes-100k", format!("{}\n", "-".repeat(100_000)));
    add_case(&mut cases, "pipes-20k", format!("{}\n", "|".repeat(20_000)));
    add_case(&mut cases, "cjk-100k", format!("{}\n", "字".repeat(100_000)));
    add_case(&mut cases, "combining-50k", format!("e{}\n", "\u{0301}".repeat(50_000)));
    add_case(&mut cases, "emoji-zwj-5k", "👩‍👩‍👧‍👦".repeat(5_000));
    add_case(&mut cases, "alternating-100k", "a ".repeat(50_000));
    add_case(&mut cases, "mixed-markers-100k", "*a$b`c<d>_~".repeat(8_000));

    // ----------------------------------------------------- many of one thing
    let mut headings = String::new();
    for index in 0..2_000 {
        headings.push_str(&format!("## Heading {index}\n\nBody {index}.\n\n"));
    }
    add_case(&mut cases, "headings-2k", headings);
    let mut duplicate_headings = String::new();
    for _ in 0..500 {
        duplicate_headings.push_str("## Same\n\n");
    }
    add_case(&mut cases, "headings-duplicate-500", duplicate_headings);
    let links: String = (0..3_000).map(|index| format!("[t{index}](u{index}) ")).collect();
    add_case(&mut cases, "links-3k", links);
    let formulas: String = (0..2_000).map(|index| format!("$x_{index}$ ")).collect();
    add_case(&mut cases, "inline-math-2k", formulas);
    let formula_lines: String = (0..300).map(|index| format!("line {index} $x$\n")).collect();
    add_case(&mut cases, "inline-math-300-lines", formula_lines);
    let mut items = String::new();
    for index in 0..2_000 {
        items.push_str(&format!("- item {index}\n"));
    }
    add_case(&mut cases, "list-items-2k", items);
    let mut paragraphs = String::new();
    for index in 0..10_000 {
        paragraphs.push_str(&format!("Paragraph {index}.\n\n"));
    }
    add_case(&mut cases, "paragraphs-10k", paragraphs);
    let mut tables = String::new();
    for _ in 0..200 {
        tables.push_str("| A | B | C |\n| --- | --- | --- |\n| 1 | 2 | 3 |\n| 4 | 5 | 6 |\n\n");
    }
    add_case(&mut cases, "tables-200", tables);
    let mut footnotes = String::new();
    for index in 0..500 {
        footnotes.push_str(&format!("Text[^n{index}].\n\n[^n{index}]: Note {index}.\n\n"));
    }
    add_case(&mut cases, "footnotes-500", footnotes);

    // ------------------------------------------- unclosed / mismatched syntax
    for (name, source) in [
        ("fence-unclosed", "```\nunclosed\n"),
        ("fence-unclosed-tilde", "~~~rust\nfn main() {}\n"),
        ("fence-mismatched", "```a\n~~~b\n"),
        ("fence-nested-four", "````\n```\ninner\n```\n````\n"),
        ("fence-only-backticks", "```"),
        ("code-span-unclosed", "`unclosed\n"),
        ("code-span-mismatched", "``a`b``\n"),
        ("code-span-odd", "`` ` ``\n"),
        ("math-dollar", "$\n"),
        ("math-two", "$$\n"),
        ("math-three", "$$$\n"),
        ("math-four", "$$$$\n"),
        ("math-open-block", "$$ x\n"),
        ("math-open-block-lines", "$$\nx\n"),
        ("math-blank-inside", "$$\n\n$$\n"),
        ("math-adjacent", "$$a$$$$b$$\n"),
        ("math-code-content", "$`\\sqrt{2}`$\n"),
        ("math-quote-mismatch", "$$\n> $$\n"),
        ("html-open-tag", "<div\n"),
        ("html-open-div", "<div>\n"),
        ("html-close-div", "</div>\n"),
        ("html-unclosed-nested", "<div><span>text\n"),
        ("html-comment-unclosed", "<!-- unclosed\n"),
        ("html-comment-empty", "<!-->\n"),
        ("html-comment-closed", "<!---->\n"),
        ("html-cdata", "<![CDATA[ x ]]>\n"),
        ("html-doctype", "<!DOCTYPE html>\n"),
        ("html-processing", "<?php echo 1; ?>\n"),
        ("html-script", "<script>alert(1)</script>\n"),
        ("html-script-unclosed", "<script>\nalert(1)\n"),
        ("html-style", "<style>body{color:red}</style>\n"),
        ("html-iframe", "<iframe src=x>\n"),
        ("html-svg", "<svg onload=alert(1)><script>alert(2)</script></svg>\n"),
        ("html-img-handler", "<img src=x onerror=alert(1)>\n"),
        ("html-anchor-javascript", "<a href=\"javascript:alert(1)\">x</a>\n"),
        ("html-template", "<template><b>x</b></template>\n"),
        ("html-object", "<object data=x>y</object>\n"),
        ("html-inline-unclosed", "<b><i>unclosed\n"),
        ("html-stray-closing", "</b></i>stray\n"),
        ("html-comment-dashes", "<!-- a -- b --- c -->\n"),
        ("math-in-link-dest", "[x](http://a/$b$) $c$\n"),
        ("math-nul", "$a\0b$\n"),
        ("table-nul", "| A | B |\n| --- | --- |\n| a\0b | c |\n"),
        ("frontmatter-open", "---\n"),
        ("frontmatter-unclosed", "---\ntitle: x\n"),
        ("frontmatter-empty", "---\n---\n"),
        ("frontmatter-dots", "---\n...\n"),
        ("frontmatter-array-open", "---\nkey: [a, b\n---\n"),
        ("frontmatter-map", "---\nkey: {a: b}\n---\n"),
        ("frontmatter-list-only", "---\n- item\n---\n"),
        ("frontmatter-empty-key", "---\n: x\n---\n"),
        ("frontmatter-four-dashes", "----\nkey: x\n----\n"),
        ("footnote-open", "[^\n"),
        ("footnote-empty", "[^]\n"),
        ("footnote-unclosed", "[^x\n"),
        ("footnote-empty-def", "[^x]:\n"),
        ("footnote-nested-cycle", "[^a]: [^b]\n\n[^b]: [^a]\n"),
    ] {
        add_case(&mut cases, name, source.to_owned());
    }
    add_case(&mut cases, "footnote-long-label", format!("[^{}] trailing\n", "x".repeat(10_000)));
    add_case(&mut cases, "code-span-huge", format!("{}x{}\n", repeated("`", 5_000), repeated("`", 5_000)));
    let mut duplicate_defs = String::new();
    for index in 0..300 {
        duplicate_defs.push_str(&format!("[^same]: definition {index}\n"));
    }
    duplicate_defs.push_str("\nReference[^same].\n");
    add_case(&mut cases, "footnote-duplicate-300", duplicate_defs);
    let mut huge_front = String::from("---\n");
    for index in 0..5_000 {
        huge_front.push_str(&format!("key{index}: value {index}\n"));
    }
    huge_front.push_str("---\n\nBody\n");
    add_case(&mut cases, "frontmatter-5k-lines", huge_front);

    // --------------------------------------------- bytes, controls, unicode
    let mut controls = String::new();
    for byte in 0u8..=31 {
        if byte != b'\n' {
            controls.push(byte as char);
        }
    }
    controls.push('\u{7f}');
    controls.push_str("text\n");
    add_case(&mut cases, "control-chars", controls);
    add_case(&mut cases, "nul-in-code", "```\na\0b\n```\n");
    add_case(&mut cases, "nul-in-link", "[x\0y](u\0v)\n");
    add_case(&mut cases, "nul-in-heading", "# a\0b\n");
    add_case(&mut cases, "nul-in-frontmatter", "---\nk\0ey: v\n---\nbody\n");
    add_case(&mut cases, "control-escape", "\u{1b}[31mred\u{1b}[0m\n");
    add_case(
        &mut cases,
        "bidi-controls",
        "\u{202a}LTR\u{202b}RTL\u{202c}\u{202d}override\u{202e}reversed\u{2066}isolate\u{2069}\n",
    );
    add_case(&mut cases, "rtl-text", "# مرحبا\n\nשלום עולם and English.\n");
    add_case(&mut cases, "combining-marks", "a\u{0301}e\u{0308}\u{0301}o\u{0327}\u{0301}\u{0302}\n");
    add_case(&mut cases, "lone-combining", "\u{0301}\u{0301}\u{0301}\n");
    add_case(&mut cases, "quotes-and-apostrophes", "'single' \"double\" l'uno d'oro\n\nends with a quote'\n");
    add_case(&mut cases, "case-expanding-heading", "# İstanbul ẞ ǅ\n\n# Ǆ Ǳ\n");
    add_case(&mut cases, "line-separators", "a\u{2028}b\u{2029}c\u{0085}d\n");
    add_case(&mut cases, "html-numeric-entities", "<div title=\"&#xFFFFFFFF; &#x110000; &#0; &#x1F600;\">x</div>\n");
    add_case(&mut cases, "frontmatter-unicode", "---\nkey: 日\nother: \"quoted é\"\n---\nBody\n");
    add_case(&mut cases, "math-crlf", "$$\r\nx = 1\r\n$$\r\n\r\nInline $y$\r\n");
    add_case(&mut cases, "html-nul", "<div>\0</div>\n");
    add_case(&mut cases, "bom-middle", "abc\u{feff}def\n".to_owned());
    add_case(&mut cases, "bom-before-heading", "\u{feff}# Heading\n\nBody\n");
    add_case(&mut cases, "crlf-document", "# H\r\n\r\n- a\r\n- b\r\n\r\n| A | B |\r\n| --- | --- |\r\n| 1 | 2 |\r\n");
    add_case(&mut cases, "crlf-frontmatter", "---\r\ntitle: x\r\n---\r\nBody\r\n");
    add_case(&mut cases, "tabs-document", "\t# H\n\n\t- item\n\n\t\tcode\n");
    add_case(&mut cases, "tab-in-table", "| A\t| B |\n| --- | --- |\n| 1\t| 2 |\n");
    add_case(&mut cases, "cjk-heading-slug", format!("# {}\n", "日本語の見出し".repeat(1_000)));
    add_case(&mut cases, "emoji-heading", "# 👩‍👩‍👧‍👦 emoji 🏳️‍🌈\n\nBody\n");

    // ------------------------------------------------------------- big tables
    add_case(&mut cases, "table-1500x10", wide_table(1_500, 10));
    add_case(&mut cases, "table-40x40", wide_table(40, 40));
    add_case(&mut cases, "table-20x200", wide_table(20, 200));
    add_case(&mut cases, "table-header-only", "| A | B |\n| --- | --- |\n");
    add_case(&mut cases, "table-no-delimiter", "| A | B |\n| 1 | 2 |\n");
    add_case(&mut cases, "table-ragged", "| A | B | C |\n| --- | --- |\n| 1 |\n| 1 | 2 | 3 | 4 |\n");
    add_case(&mut cases, "table-escaped-pipes", "| A | B |\n| --- | --- |\n| a \\| b | `c \\| d` |\n");
    add_case(&mut cases, "table-empty-cells", "| | |\n| --- | --- |\n| | |\n");
    add_case(&mut cases, "table-long-cell", format!("| A | B |\n| --- | --- |\n| {} | x |\n", "y".repeat(10_000)));
    add_case(&mut cases, "table-in-quote-list", "> - | A | B |\n>   | --- | --- |\n>   | 1 | 2 |\n");

    // ------------------------------------------------ TOC, anchors, duplicate ids
    let mut toc_doc = String::from("[TOC]\n\n");
    for index in 0..300 {
        toc_doc.push_str(&format!("## Section {index}\n\nBody.\n\n"));
    }
    add_case(&mut cases, "toc-300", toc_doc);
    add_case(&mut cases, "toc-variants", "[TOC]\n\n[[TOC]]\n\n[toc]\n\n[TOC] extra\n\n# A\n");
    add_case(
        &mut cases,
        "reserved-anchors",
        "# title\n\n# mmd-1\n\n# palette\n\n# user-content-title\n\n# title\n\n# mmd-2\n",
    );
    add_case(
        &mut cases,
        "duplicate-anchors",
        format!("{}{}", repeated("# Same\n\n", 100), repeated("# same\n\n", 100)),
    );
    add_case(&mut cases, "anchor-punctuation", "# ..\n\n# ---\n\n# \u{FFFC}\n\n# 123\n\n# a_b-c.d\n\n# \u{0301}\n");

    // ------------------------------------------------ unsafe links and images
    for (name, source) in [
        ("link-javascript", "[x](javascript:alert(1))\n"),
        ("link-javascript-upper", "[x](JaVaScRiPt:alert(1))\n"),
        ("link-data", "[x](data:text/html;base64,PHNjcmlwdD4=)\n"),
        ("link-vbscript", "[x](vbscript:msgbox(1))\n"),
        ("link-file", "[x](file:///etc/passwd)\n"),
        ("image-javascript", "![x](javascript:alert(1))\n"),
        ("image-data-text", "![x](data:text/html;base64,PHNjcmlwdD4=)\n"),
        (
            "image-data-png",
            "![x](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lK3Q6wAAAABJRU5ErkJggg==)\n",
        ),
        ("reference-javascript", "[x][y]\n\n[y]: javascript:alert(1)\n"),
        ("autolink-javascript", "<javascript:alert(1)>\n"),
        ("html-link-javascript", "<div><a href=\"javascript:alert(1)\">x</a></div>\n"),
        ("html-link-data", "<div><a href=\"data:text/html;base64,PHNjcmlwdD4=\">x</a></div>\n"),
        ("html-img-data-image", "<img src=\"data:image/png;base64,iVBOR\" alt=\"x\">\n"),
        ("html-img-data-text", "<img src=\"data:text/html;base64,PHNjcmlwdD4=\" alt=\"x\">\n"),
        ("html-img-javascript", "<img src=\"javascript:alert(1)\" alt=\"x\">\n"),
        ("html-video-poster", "<video poster=\"javascript:alert(1)\"></video>\n"),
        ("html-handler-attrs", "<div onclick=\"x\" onmouseover=\"y\" style=\"z\">text</div>\n"),
        ("html-srcset", "<img src=\"a.png\" srcset=\"javascript:alert(1) 1x, b.png 2x\" alt=\"x\">\n"),
        ("html-id-class", "<div id=\"title\" class=\"gh-dark-mode-only other\">x</div>\n"),
    ] {
        add_case(&mut cases, name, source.to_owned());
    }

    // ------------------------------------------------------------- mixed soup
    add_case(
        &mut cases,
        "mixed-hostile",
        format!(
            "---\ntitle: \"[^x]\"\n---\n\n# {}\n\n> [!WARNING]\n> {}\n\n{}\n\n| A | B |\n| --- | --- |\n| <script>alert(1)</script> | $x$ |\n\n[^x]: {}\n",
            "heading ".repeat(1_000),
            "$".repeat(10_000),
            "<div><img src=x onerror=alert(1)>\n\nalert",
            "- item\n".repeat(1_000),
        ),
    );

    cases
}

// --------------------------------------------------------------- invariants

fn preview(source: &str) -> String {
    let mut out: String = source.chars().take(200).collect();
    if out.len() < source.len() {
        out.push('…');
    }
    out.escape_debug().to_string()
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|message| (*message).to_owned()))
        .unwrap_or_else(|| "non-string panic payload".to_owned())
}

struct Checker<'a> {
    doc: &'a MdDoc,
    problems: Vec<String>,
}

impl<'a> Checker<'a> {
    fn new(doc: &'a MdDoc) -> Self {
        Self { doc, problems: Vec::new() }
    }

    fn fail(&mut self, path: &str, message: impl Into<String>) {
        if self.problems.len() < 40 {
            self.problems.push(format!("{path}: {}", message.into()));
        }
    }

    fn source_range(&mut self, path: &str, label: &str, range: &Range<usize>) {
        let len = self.doc.source.len();
        if range.start > range.end {
            self.fail(path, format!("{label} has start {} > end {}", range.start, range.end));
            return;
        }
        if range.end > len {
            self.fail(path, format!("{label} end {} exceeds source length {len}", range.end));
            return;
        }
        if !self.doc.source.is_char_boundary(range.start) {
            self.fail(path, format!("{label} start {} is not a char boundary", range.start));
        }
        if !self.doc.source.is_char_boundary(range.end) {
            self.fail(path, format!("{label} end {} is not a char boundary", range.end));
        }
    }

    fn line(&mut self, path: &str, label: &str, line: usize) {
        let count = self.doc.line_count();
        if line >= count {
            self.fail(path, format!("{label} line {line} is outside {count} lines"));
        }
    }

    fn end_line(&mut self, path: &str, label: &str, line: usize) {
        let count = self.doc.line_count();
        if line > count {
            self.fail(path, format!("{label} end line {line} exceeds {count} lines"));
        }
    }

    fn doc(&mut self, source: &str) {
        let doc = self.doc;
        if &*doc.source != source {
            self.fail("doc", "source was not preserved verbatim");
        }
        let mut starts = vec![0u32];
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                starts.push((index + 1) as u32);
            }
        }
        if doc.line_starts.as_ref() != starts.as_slice() {
            self.fail("doc", format!("line_starts mismatch: {:?} != {:?}", doc.line_starts, starts));
        }
        if doc.line_count() != starts.len() {
            self.fail("doc", "line_count disagrees with line_starts");
        }
        for (line, start) in starts.iter().enumerate() {
            if doc.line_of(*start as usize) != line {
                self.fail("doc", format!("line_of({start}) is not {line}"));
            }
        }
        if doc.line_of(source.len()) + 1 != starts.len() {
            self.fail("doc", "line_of(len) is not the last line");
        }
        if doc.blocks.len() != doc.block_spans.len() {
            self.fail("doc", "blocks and block_spans have different lengths");
        }
        let mut previous_end = 0;
        for (index, span) in doc.block_spans.iter().enumerate() {
            let path = format!("block[{index}]");
            if span.key != format!("b{index}") {
                self.fail(&path, format!("key {:?} is not b{index}", span.key));
            }
            self.source_range(&path, "span", &span.source);
            self.line(&path, "span", span.line);
            self.end_line(&path, "span", span.end_line);
            if span.source.is_empty() {
                if span.line != span.end_line {
                    self.fail(&path, "an empty span must have line == end_line");
                }
            } else {
                if span.line != doc.line_of(span.source.start) {
                    self.fail(&path, "span.line is not the line of its start");
                }
                if span.end_line != doc.line_of(span.source.end - 1) + 1 {
                    self.fail(&path, "span.end_line is not the line after its end");
                }
            }
            if span.source.start < previous_end {
                self.fail(&path, "block spans overlap or are out of order");
            }
            previous_end = span.source.end;
            if doc.block_key(index) != Some(span.key.as_str()) {
                self.fail(&path, "block_key disagrees with block_spans");
            }
        }

        for (index, heading) in doc.headings.iter().enumerate() {
            let path = format!("heading[{index}]");
            self.heading(&path, index, heading);
        }
        for (index, footnote) in doc.footnotes.iter().enumerate() {
            let path = format!("footnote[{index}]");
            self.footnote(&path, footnote);
        }
        for (index, diagnostic) in doc.diagnostics.iter().enumerate() {
            let path = format!("diagnostic[{index}]");
            self.diagnostic(&path, diagnostic);
        }
        if let Some(front) = &doc.front_matter {
            if front.valid && front.entries.is_empty() {
                self.fail("front-matter", "valid front matter has no entries");
            }
            if !front.valid && !front.entries.is_empty() {
                self.fail("front-matter", "invalid front matter has entries");
            }
            if front.raw.len() > source.len() {
                self.fail("front-matter", "raw is longer than the source");
            }
            for (key, _) in &front.entries {
                if key.is_empty() {
                    self.fail("front-matter", "empty key");
                }
            }
        }

        let stride = (source.len() / 2_048).max(1);
        let mut offsets: Vec<usize> = (0..=source.len()).step_by(stride).collect();
        offsets.extend([0, source.len()]);
        for span in &doc.block_spans {
            offsets.extend([span.source.start, span.source.end.saturating_sub(1)]);
        }
        offsets.sort_unstable();
        offsets.dedup();
        for offset in offsets {
            let offset = offset.min(source.len());
            let Some(index) = doc.block_index_at(offset) else { continue };
            if index >= doc.block_spans.len() {
                self.fail("block_index_at", format!("offset {offset} returned block {index}"));
            } else if offset < source.len() && !doc.block_spans[index].source.contains(&offset) {
                self.fail("block_index_at", format!("offset {offset} returned block {index} that does not contain it"));
            }
        }
    }

    fn heading(&mut self, path: &str, index: usize, heading: &Heading) {
        if !(1..=6).contains(&heading.level) {
            self.fail(path, format!("level {} is outside 1..=6", heading.level));
        }
        self.line(path, "heading", heading.line);
        if heading.block != usize::MAX {
            if heading.block >= self.doc.blocks.len() {
                self.fail(path, format!("block {} is out of range", heading.block));
            } else if let Block::Heading { heading: block_heading, .. } = &self.doc.blocks[heading.block]
                && *block_heading != index
            {
                self.fail(path, "top-level heading block points at a different heading");
            }
        }
        if heading.in_toc && heading.level > 4 {
            self.fail(path, "in_toc on a level 5/6 heading");
        }
        if self.doc.resolve_heading(&heading.id).is_none() {
            self.fail(path, format!("id {:?} does not resolve", heading.id));
        }
        if let Some(legacy) = &heading.legacy_id
            && self.doc.resolve_heading(legacy).is_none()
        {
            self.fail(path, format!("legacy id {legacy:?} does not resolve"));
        }
        if let Some(anchor) = &heading.anchor
            && self.doc.resolve_heading(anchor).is_none()
        {
            self.fail(path, format!("anchor {anchor:?} does not resolve"));
        }
    }

    fn footnote(&mut self, path: &str, footnote: &Footnote) {
        if footnote.number == 0 {
            self.fail(path, "footnote number 0");
        }
        self.source_range(path, "footnote source", &footnote.source);
        if let Some(text) = self.doc.source.get(footnote.source.clone())
            && (!text.starts_with("[^") || !text.contains("]:"))
        {
            self.fail(path, format!("footnote source does not start a definition: {text:?}"));
        }
        if let Some(reference) = &footnote.first_ref {
            self.source_range(path, "first reference", reference);
            if let Some(text) = self.doc.source.get(reference.clone())
                && (!text.starts_with("[^") || !text.ends_with(']'))
            {
                self.fail(path, format!("first reference is not a footnote marker: {text:?}"));
            }
        }
        for (index, block) in footnote.blocks.iter().enumerate() {
            self.block(&format!("{path}.block[{index}]"), block);
        }
    }

    fn diagnostic(&mut self, path: &str, diagnostic: &Diagnostic) {
        if let Some(line) = diagnostic.line {
            self.line(path, "diagnostic", line);
        }
        if let Some(range) = &diagnostic.source {
            self.source_range(path, "diagnostic", range);
        }
    }

    fn block(&mut self, path: &str, block: &Block) {
        match block {
            Block::Para(inline) => self.inline(path, inline),
            Block::Heading { heading, inline } => {
                if *heading >= self.doc.headings.len() {
                    self.fail(path, format!("heading index {heading} is out of range"));
                }
                self.inline(path, inline);
            }
            Block::Code { source, .. } => self.source_range(path, "code", source),
            Block::List { items, .. } => {
                for (index, item) in items.iter().enumerate() {
                    for (child, block) in item.iter().enumerate() {
                        self.block(&format!("{path}.item[{index}].block[{child}]"), block);
                    }
                }
            }
            Block::Quote { blocks, .. } => {
                for (index, block) in blocks.iter().enumerate() {
                    self.block(&format!("{path}.quote[{index}]"), block);
                }
            }
            Block::Table(table) => {
                self.source_range(path, "table", &table.source);
                self.line(path, "table", table.line);
                if table.align.len() != table.head.len() {
                    self.fail(path, "table align and head widths differ");
                }
                for (index, cell) in table.head.iter().enumerate() {
                    self.inline(&format!("{path}.head[{index}]"), cell);
                }
                for (row, cells) in table.rows.iter().enumerate() {
                    if cells.len() != table.align.len() {
                        self.fail(
                            path,
                            format!("row {row} has {} cells for {} columns", cells.len(), table.align.len()),
                        );
                    }
                    for (column, cell) in cells.iter().enumerate() {
                        self.inline(&format!("{path}.row[{row}][{column}]"), cell);
                    }
                }
            }
            Block::Rule | Block::FootnoteDefs | Block::Toc => {}
            Block::Mermaid { line, .. } => self.line(path, "mermaid", *line),
            Block::MathBlock { line, .. } => self.line(path, "math", *line),
            Block::HtmlBlock(node) => self.html(path, node),
            Block::Image(image) => {
                self.source_range(path, "image", &image.source);
                self.line(path, "image", image.line);
            }
        }
    }

    fn inline(&mut self, path: &str, inline: &Inline) {
        let text = &inline.text;
        let mut expected = 0;
        for (index, segment) in inline.source_map.segments.iter().enumerate() {
            if segment.rendered.is_empty() {
                self.fail(path, format!("source_map segment {index} is empty"));
            }
            if segment.rendered.start != expected {
                self.fail(
                    path,
                    format!("source_map gap/overlap at segment {index}: {} != {expected}", segment.rendered.start),
                );
            }
            expected = segment.rendered.end;
            if segment.rendered.end > text.len() {
                self.fail(path, format!("source_map segment {index} ends past the rendered text"));
            }
            if !text.is_char_boundary(segment.rendered.start) || !text.is_char_boundary(segment.rendered.end) {
                self.fail(path, format!("source_map segment {index} is not on char boundaries"));
            }
            if segment.source > self.doc.source.len() {
                self.fail(path, format!("source_map segment {index} points past the source"));
            }
            match inline.source_map.source_of(segment.rendered.start) {
                Some(source) if source == segment.source => {}
                other => self.fail(path, format!("source_of({}) returned {other:?}", segment.rendered.start)),
            }
            if !segment.rendered.is_empty()
                && inline.source_map.source_of(segment.rendered.end - 1)
                    != Some(segment.source + segment.rendered.len() - 1)
            {
                self.fail(path, format!("source_of({}) is not linear", segment.rendered.end - 1));
            }
        }
        if expected != text.len() {
            self.fail(path, format!("source_map covers {expected} of {} rendered bytes", text.len()));
        }
        if !text.is_empty() && inline.source_map.source_range(0..text.len()).is_none() {
            self.fail(path, "source_range does not cover the rendered text");
        }
        for (index, span) in inline.spans.iter().enumerate() {
            if span.range.start > span.range.end {
                self.fail(path, format!("span {index} is reversed"));
            }
            if span.range.end > text.len() {
                self.fail(path, format!("span {index} is past the rendered text"));
            }
            if !text.is_char_boundary(span.range.start) || !text.is_char_boundary(span.range.end) {
                self.fail(path, format!("span {index} is not on char boundaries"));
            }
        }
        for (index, link) in inline.links.iter().enumerate() {
            if link.range.is_empty() {
                self.fail(path, format!("link {index} is empty"));
            }
            if link.range.end > text.len() {
                self.fail(path, format!("link {index} is past the rendered text"));
            }
            if !text.is_char_boundary(link.range.start) || !text.is_char_boundary(link.range.end) {
                self.fail(path, format!("link {index} is not on char boundaries"));
            }
        }
        for (index, math) in inline.math.iter().enumerate() {
            if text.get(math.range.clone()) != Some("\u{FFFC}") {
                self.fail(path, format!("math {index} does not cover exactly one object character"));
            }
        }
        for (index, object) in inline.objects.iter().enumerate() {
            if object.range.start > object.range.end {
                self.fail(path, format!("object {index} is reversed"));
            }
            if object.range.end > text.len() {
                self.fail(path, format!("object {index} is past the rendered text"));
            }
            if !text.is_char_boundary(object.range.start) || !text.is_char_boundary(object.range.end) {
                self.fail(path, format!("object {index} is not on char boundaries"));
            }
            let expected = match &object.kind {
                InlineObjectKind::Image | InlineObjectKind::TaskCheckbox { .. } => Some("\u{FFFC}"),
                InlineObjectKind::LineBreak => Some("\n"),
                _ => None,
            };
            if let Some(expected) = expected
                && text.get(object.range.clone()) != Some(expected)
            {
                self.fail(path, format!("object {index} does not cover {expected:?}"));
            }
            if let Some(node) = &object.html {
                self.html(&format!("{path}.object[{index}]"), node);
            }
        }
    }

    fn html(&mut self, path: &str, node: &HtmlNode) {
        let mut stack = vec![node];
        while let Some(node) = stack.pop() {
            if matches!(node.tag, HtmlTag::Fragment) && !node.attrs.is_empty() {
                self.fail(path, "fragment root has attributes");
            }
            for (name, value) in &node.attrs {
                if name != &name.to_ascii_lowercase() {
                    self.fail(path, format!("attribute {name:?} is not lowercase"));
                }
                if !ALLOWED_ATTRS.contains(&name.as_str()) {
                    self.fail(path, format!("attribute {name:?} is not in the allowlist"));
                }
                if name.starts_with("on") {
                    self.fail(path, format!("event-handler attribute {name:?} survived"));
                }
                if name == "id" && !value.starts_with("user-content-") {
                    self.fail(path, format!("id {value:?} was not namespaced"));
                }
                if name == "class" {
                    for class in value.split_whitespace() {
                        if !matches!(class, "gh-dark-mode-only" | "gh-light-mode-only") {
                            self.fail(path, format!("class {class:?} survived"));
                        }
                    }
                }
            }
            for candidate in &node.srcset {
                if candidate.url.is_empty() {
                    self.fail(path, "srcset candidate with an empty URL");
                }
                let lower = candidate.url.to_ascii_lowercase();
                if lower.starts_with("javascript:")
                    || lower.starts_with("vbscript:")
                    || (lower.starts_with("data:") && !lower.starts_with("data:image/"))
                {
                    self.fail(path, format!("unsafe srcset candidate {:?} survived", candidate.url));
                }
            }
            for child in &node.children {
                if let HtmlChild::Node(child) = child {
                    stack.push(child);
                }
            }
        }
    }
}

fn check_doc(source: &str, doc: &MdDoc) -> Vec<String> {
    let mut checker = Checker::new(doc);
    checker.doc(source);
    for (index, block) in doc.blocks.iter().enumerate() {
        checker.block(&format!("block[{index}]"), block);
    }
    checker.problems
}

fn check_math_scan(source: &str) {
    let scan = scan_math(source);
    let mut problems = Vec::new();
    if scan.text.len() != source.len() {
        problems.push(format!("math text length {} != {}", scan.text.len(), source.len()));
    }
    if scan.text.matches('\n').count() != source.matches('\n').count() {
        problems.push("math text changed the line count".to_owned());
    }
    if scan.spans.len() != scan.placeholders.len() {
        problems.push("math spans and placeholders have different lengths".to_owned());
    }
    let mut previous_end = 0;
    for (index, span) in scan.spans.iter().enumerate() {
        if span.source.start > span.source.end || span.source.end > source.len() {
            problems.push(format!("math span {index} is out of bounds"));
        }
        if !source.is_char_boundary(span.source.start) || !source.is_char_boundary(span.source.end) {
            problems.push(format!("math span {index} is not on char boundaries"));
        }
        if span.source.start < previous_end {
            problems.push(format!("math span {index} overlaps the previous one"));
        }
        previous_end = span.source.end;
        if span.tex.trim().is_empty() {
            problems.push(format!("math span {index} has empty TeX"));
        }
        let Some(placeholder) = scan.placeholders.get(index).copied() else { continue };
        if placeholder >= scan.text.len() || placeholder < span.source.start {
            problems.push(format!("math placeholder {index} is out of bounds"));
            continue;
        }
        if !scan.text.is_char_boundary(placeholder) {
            problems.push(format!("math placeholder {index} is not on a char boundary"));
            continue;
        }
        let first = scan.text[placeholder..].chars().next();
        if !matches!(first, Some(OBJECT_CHAR) | Some('\u{0001}')) {
            problems.push(format!("math placeholder {index} is {first:?}"));
        }
    }
    if !problems.is_empty() {
        panic!("math scan invariants failed: {problems:?}");
    }
}

fn check_case(case: &Case) -> (Vec<String>, Duration) {
    let started = Instant::now();
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let doc = lmdv_md::parse(&case.source);
        let mut problems = check_doc(&case.source, &doc);
        if let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(|| check_math_scan(&case.source))) {
            problems.push(format!("math scan panicked: {}", panic_message(&*payload)));
        }
        let second = lmdv_md::parse(&case.source);
        if doc.blocks != second.blocks
            || doc.block_spans != second.block_spans
            || doc.headings != second.headings
            || doc.footnotes != second.footnotes
            || doc.diagnostics != second.diagnostics
            || doc.front_matter != second.front_matter
        {
            problems.push("parsing the same source twice gave different models".to_owned());
        }
        problems
    }));
    let elapsed = started.elapsed();
    match result {
        Ok(problems) => (problems, elapsed),
        Err(payload) => (vec![format!("panicked: {}", panic_message(&*payload))], elapsed),
    }
}

#[test]
fn security_corpus_never_panics_and_keeps_invariants() {
    let cases = corpus();
    assert!(cases.len() >= 100, "corpus lost cases: {}", cases.len());
    let total_bytes: usize = cases.iter().map(|item| item.source.len()).sum();
    let largest = cases.iter().map(|item| item.source.len()).max().unwrap_or(0);
    let started = Instant::now();
    let mut failures = Vec::new();
    let mut slowest = (String::new(), Duration::ZERO);
    for case in &cases {
        let (problems, elapsed) = check_case(case);
        if elapsed > slowest.1 {
            slowest = (case.name.clone(), elapsed);
        }
        if elapsed > CASE_BUDGET {
            failures.push(format!("{}: took {elapsed:?}, over the {CASE_BUDGET:?} budget", case.name));
        }
        if !problems.is_empty() {
            failures.push(format!(
                "{} ({})\n  source: {}\n  {}",
                case.name,
                case.source.len(),
                preview(&case.source),
                problems.join("\n  ")
            ));
        }
    }
    eprintln!(
        "P7-SEC corpus: {} cases ({total_bytes} bytes, largest {largest}) in {:?}; slowest {} at {:?}",
        cases.len(),
        started.elapsed(),
        slowest.0,
        slowest.1
    );
    assert!(failures.is_empty(), "{} case(s) failed:\n\n{}", failures.len(), failures.join("\n\n"));
}

/// Deterministic random corpus: a fixed seed, valid `str` only, 2,000 cases.
///
/// The generator draws Markdown tokens (including multibyte text, controls,
/// math delimiters, tables and HTML) so the cases exercise combinations the
/// hand-written corpus does not.
#[test]
fn deterministic_fuzz_corpus_never_panics() {
    const TOKENS: &[&str] = &[
        "$",
        "$$",
        "`",
        "```",
        "~~~",
        "<",
        ">",
        "</",
        "<!--",
        "-->",
        "[",
        "]",
        "(",
        ")",
        "![",
        "](",
        "[^",
        "]:",
        "*",
        "_",
        "~~",
        "#",
        "## ",
        "|",
        "---",
        "---\n",
        ":",
        "-",
        "+",
        "1. ",
        "> [!NOTE]\n",
        "&amp;",
        "&",
        "\\",
        "\n",
        "\n\n",
        " ",
        "\t",
        "text",
        "é",
        "日",
        "\u{FFFC}",
        "\u{0001}",
        "\u{0002}",
        "x",
        "=",
        "!",
        "\r\n",
        "\u{feff}",
        "\"",
        "'",
        "src=",
        "<div>",
        "</div>",
        "<img src=\"x\">",
        "| a | b |\n| --- | --- |\n",
        "\u{0301}",
        "\0",
        "\u{202e}",
        "ä",
        "👩‍👩‍👧‍👦",
    ];

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 11
        }
    }

    let mut rng = Lcg(0x5eed_1a2b_3c4d_5e6f);
    let started = Instant::now();
    let mut failures = Vec::new();
    let mut slowest = Duration::ZERO;
    let cases = 2_000;
    for index in 0..cases {
        let tokens = 1 + (rng.next() % 32) as usize;
        let mut source = String::new();
        for _ in 0..tokens {
            source.push_str(TOKENS[(rng.next() % TOKENS.len() as u64) as usize]);
        }
        let (problems, elapsed) = check_case(&Case { name: format!("fuzz-{index}"), source: source.clone() });
        slowest = slowest.max(elapsed);
        if elapsed > CASE_BUDGET {
            failures.push(format!("fuzz-{index}: took {elapsed:?}"));
        }
        if !problems.is_empty() {
            failures.push(format!("fuzz-{index}\n  source: {}\n  {}", preview(&source), problems.join("\n  ")));
            if failures.len() >= 20 {
                break;
            }
        }
    }
    eprintln!("P7-SEC fuzz: {cases} cases in {:?}; slowest {slowest:?}", started.elapsed());
    assert!(failures.is_empty(), "{} random case(s) failed:\n\n{}", failures.len(), failures.join("\n\n"));
}

/// Tracking test for the one panic this suite found (`math.rs:351`).
///
/// `closing_dollar` slices `source[start + 3..]` where `start + 3` can be the
/// middle of a multibyte character after the opening backtick of a GitHub
/// code-span formula. Remove `#[ignore]` together with the parser fix; the
/// random generator's `defuse_known_panic` workaround can go then too.
#[test]
fn backtick_math_with_multibyte_content_does_not_panic() {
    for source in ["$`é", "x $`é y`$ z", "x $`日`$ z", "$`\u{feff}`$", "text $`\u{FFFC}$ more"] {
        let _ = lmdv_md::parse(source);
    }
}

#[test]
fn raw_html_never_keeps_scripts_handlers_or_unsafe_urls() {
    let sources = [
        "<script>alert(1)</script>",
        "<style>body{color:red}</style>",
        "<iframe src=\"javascript:alert(1)\"></iframe>",
        "<div onclick=\"alert(1)\" onmouseover=\"alert(2)\" style=\"x\">text</div>",
        "<a href=\"javascript:alert(1)\">link</a>",
        "<a href=\"data:text/html;base64,PHNjcmlwdD4=\">link</a>",
        "<a href=\"vbscript:msgbox(1)\">link</a>",
        "<img src=\"javascript:alert(1)\" alt=\"x\">",
        "<img src=\"data:text/html;base64,PHNjcmlwdD4=\" alt=\"x\">",
        "<video poster=\"javascript:alert(1)\"></video>",
        "<svg onload=\"alert(1)\"><script>alert(2)</script></svg>",
    ];
    for source in sources {
        let doc = lmdv_md::parse(source);
        let mut nodes: Vec<&HtmlNode> = Vec::new();
        collect_html(&doc.blocks, &mut nodes);
        for node in nodes {
            for (name, value) in &node.attrs {
                assert!(!name.starts_with("on"), "{source}: {name} survived");
                if matches!(name.as_str(), "href" | "src" | "poster") {
                    let lower = value.to_ascii_lowercase();
                    assert!(!lower.starts_with("javascript:"), "{source}: {name} kept javascript:");
                    assert!(!lower.starts_with("vbscript:"), "{source}: {name} kept vbscript:");
                    assert!(
                        !lower.starts_with("data:") || lower.starts_with("data:image/"),
                        "{source}: {name} kept a non-image data URL"
                    );
                }
                if name == "id" {
                    assert!(value.starts_with("user-content-"), "{source}: id {value:?} not namespaced");
                }
            }
        }
        // Tagfiltered blocks (`script`, `style`, `iframe`, ...) become plain
        // text, so the fragment must not contain an element child at all.
        if source.starts_with("<script>") || source.starts_with("<style>") || source.starts_with("<iframe") {
            let fragment = match &doc.blocks[0] {
                Block::HtmlBlock(node) => node,
                Block::Para(inline) => {
                    assert!(inline.objects.is_empty(), "{source}: unexpected inline object");
                    continue;
                }
                other => panic!("{source}: unexpected block {other:?}"),
            };
            assert!(
                fragment.children.iter().all(|child| matches!(child, HtmlChild::Text(_))),
                "{source}: filtered tag kept an element"
            );
        }
    }

    for source in [
        "<img src=\"javascript:alert(1)\" alt=\"x\">",
        "<img src=\"data:text/html;base64,PHNjcmlwdD4=\" alt=\"x\">",
        "<img src=\"vbscript:msgbox(1)\" alt=\"x\">",
    ] {
        let doc = lmdv_md::parse(source);
        let Block::Image(image) = &doc.blocks[0] else { panic!("{source}: expected a block image") };
        assert!(image.src.is_empty(), "{source}: unsafe image source survived: {:?}", image.src);
    }

    let doc = lmdv_md::parse("<img src=\"data:image/png;base64,iVBOR\" alt=\"x\">");
    let html = match &doc.blocks[0] {
        Block::Image(image) => image.src.clone(),
        Block::HtmlBlock(node) => node
            .children
            .iter()
            .find_map(|child| match child {
                HtmlChild::Node(node) if node.tag == HtmlTag::Img => node.attr("src").map(str::to_owned),
                _ => None,
            })
            .unwrap_or_default(),
        other => panic!("unexpected block for a data image: {other:?}"),
    };
    assert!(html.starts_with("data:image/png;base64,"), "image data URL should survive: {html:?}");
}

/// OI-146: every `srcset` candidate goes through the same URL policy as `src`.
#[test]
fn srcset_candidates_never_keep_unsafe_urls() {
    fn safe(source: &str, url: &str) {
        let lower = url.to_ascii_lowercase();
        assert!(!lower.starts_with("javascript:"), "{source}: srcset kept {url}");
        assert!(!lower.starts_with("vbscript:"), "{source}: srcset kept {url}");
        assert!(
            !lower.starts_with("data:") || lower.starts_with("data:image/"),
            "{source}: srcset kept a non-image data URL {url}"
        );
    }

    fn nodes<'a>(node: &'a HtmlNode, out: &mut Vec<&'a HtmlNode>) {
        out.push(node);
        for child in &node.children {
            if let HtmlChild::Node(child) = child {
                nodes(child, out);
            }
        }
    }

    for source in [
        "<img src=\"a.png\" srcset=\"javascript:alert(1) 1x, b.png 2x\" alt=\"x\">",
        "<img srcset=\"vbscript:msgbox(1) 1x, data:text/html;base64,PHNjcmlwdD4= 2x\" alt=\"x\">",
        "<div><img srcset=\"javascript:alert(1) 1x, b.png 2x\" alt=\"x\"></div>",
        "<picture>\n<source srcset=\"javascript:alert(1) 1x, wide.png 2x\">\n<img src=\"small.png\" alt=\"Logo\">\n</picture>\n",
    ] {
        let doc = lmdv_md::parse(source);
        let mut found = Vec::new();
        collect_html(&doc.blocks, &mut found);
        let mut all = Vec::new();
        for node in &found {
            nodes(node, &mut all);
        }
        for node in &all {
            for candidate in &node.srcset {
                safe(source, &candidate.url);
            }
        }
        for block in &doc.blocks {
            if let Block::Image(image) = block {
                for candidate in image.srcset.iter().chain(image.picture.iter().flat_map(|picture| &picture.srcset)) {
                    safe(source, &candidate.url);
                }
            }
        }
    }

    // The safe candidates and their descriptors survive.
    let doc = lmdv_md::parse(
        "<img src=\"a.png\" srcset=\"javascript:alert(1) 1x, data:image/png;base64,iVBOR 2x, b.png 3x\" alt=\"x\">",
    );
    let Block::Image(image) = &doc.blocks[0] else { panic!("expected a block image") };
    assert_eq!(image.srcset.len(), 2);
    assert_eq!(image.srcset[0].url, "data:image/png;base64,iVBOR");
    assert_eq!(image.srcset[0].descriptor.as_deref(), Some("2x"));
    assert_eq!(image.srcset[1].url, "b.png");
    assert_eq!(image.srcset[1].descriptor.as_deref(), Some("3x"));
}

fn collect_html<'a>(blocks: &'a [Block], out: &mut Vec<&'a HtmlNode>) {
    for block in blocks {
        match block {
            Block::Para(inline) | Block::Heading { inline, .. } => {
                for object in &inline.objects {
                    if let Some(node) = &object.html {
                        out.push(node);
                    }
                }
            }
            Block::HtmlBlock(node) => out.push(node),
            Block::Quote { blocks, .. } => collect_html(blocks, out),
            Block::List { items, .. } => {
                for block in items.iter().flatten() {
                    collect_html(std::slice::from_ref(block), out);
                }
            }
            Block::Table(table) => {
                for cell in table.head.iter().chain(table.rows.iter().flatten()) {
                    for object in &cell.objects {
                        if let Some(node) = &object.html {
                            out.push(node);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[test]
fn parse_with_revision_agrees_with_parse() {
    let sources = [
        "",
        "# Heading\n\nBody with $x$ and [a link](u).\n",
        "| A | B |\n| --- | --- |\n| 1 | 2 |\n",
        "> - nested\n>   - list\n",
    ];
    for source in sources {
        let plain = lmdv_md::parse(source);
        let revision = lmdv_md::parse_with_revision(source, 7);
        assert_eq!(revision.revision, 7);
        assert_eq!(plain.blocks, revision.blocks);
        assert_eq!(plain.block_spans, revision.block_spans);
        assert_eq!(plain.headings, revision.headings);
    }
}

#[test]
fn markdown_link_destinations_are_kept_for_the_policy_layer() {
    for (source, dest) in [
        ("[x](javascript:alert(1))\n", "javascript:alert(1)"),
        ("[x](data:text/html;base64,PHNjcmlwdD4=)\n", "data:text/html;base64,PHNjcmlwdD4="),
        ("[x](file:///etc/passwd)\n", "file:///etc/passwd"),
    ] {
        let doc = lmdv_md::parse(source);
        let Block::Para(inline) = &doc.blocks[0] else { panic!("paragraph") };
        assert_eq!(inline.links.len(), 1, "{source}");
        assert_eq!(inline.links[0].dest, dest);
        assert_eq!(inline.text, "x");
    }
}

#[test]
fn bidi_controls_and_combining_marks_are_preserved_verbatim() {
    let source = "# \u{202e}abc\u{202c}\n\nשלום e\u{0301}\u{0301}\n\n\u{2066}isolate\u{2069}\n";
    let doc = lmdv_md::parse(source);
    assert_eq!(&*doc.source, source);
    let heading = &doc.headings[0];
    assert!(heading.text.contains('\u{202e}'));
    assert!(heading.text.contains('\u{202c}'));
    let joined = doc
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Para(inline) => Some(inline.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("שלום"));
    assert!(joined.contains("e\u{0301}\u{0301}"));
    assert!(joined.contains("\u{2066}isolate\u{2069}"));
}

#[test]
fn toc_duplicate_and_reserved_anchors_stay_resolvable() {
    let source = format!(
        "[TOC]\n\n# Same\n\n# Same\n\n# title\n\n# user-content-title\n\n# mmd-1\n\n{}",
        repeated("# Same\n\n", 50)
    );
    let doc = lmdv_md::parse(&source);
    assert!(doc.blocks.iter().any(|block| matches!(block, Block::Toc)));
    assert!(doc.headings.len() > 50);
    for heading in &doc.headings {
        assert!(doc.resolve_heading(&heading.id).is_some(), "{heading:?}");
        assert!(doc.resolve_heading(&format!("#{}", heading.id)).is_some(), "{heading:?}");
        if let Some(legacy) = &heading.legacy_id {
            assert!(doc.resolve_heading(legacy).is_some(), "{heading:?}");
        }
        if let Some(anchor) = &heading.anchor {
            assert!(doc.resolve_heading(anchor).is_some(), "{heading:?}");
        }
    }
    assert!(doc.headings.iter().any(|heading| heading.id == "user-content-title"));
}

#[test]
fn huge_documents_keep_block_and_line_arithmetic_in_bounds() {
    for source in ["a".repeat(100_000), format!("{}\n", "字".repeat(100_000)), format!("{}x\n", " ".repeat(100_000))] {
        let doc = lmdv_md::parse(&source);
        assert_eq!(&*doc.source, source);
        assert_eq!(doc.line_of(source.len()), doc.line_count() - 1);
        if !doc.blocks.is_empty() {
            assert_eq!(doc.block_index_at(source.len()), Some(doc.blocks.len() - 1));
        }
        for offset in [0, source.len() / 2, source.len()] {
            if let Some(index) = doc.block_index_at(offset) {
                assert!(index < doc.block_spans.len());
            }
        }
    }
}
