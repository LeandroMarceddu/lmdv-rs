//! SP-03 typing benchmark (OI-064): single-field vs block mode.
//!
//! Run on demand; it is not part of a CI gate and makes no timing assertions:
//!
//! ```text
//! cargo run --release -p lmdv-edit --example typing_bench
//! cargo run --release -p lmdv-edit --example typing_bench -- --keystrokes 500 --warmup 50
//! cargo run --release -p lmdv-edit --example typing_bench -- --no-huge-block
//! ```
//!
//! The harness builds deterministic documents from a fixed seed (realistic prose with
//! headings, lists, tables and fenced code) and measures, at 64 KiB and 1 MiB:
//!
//! - one ASCII character typed into a single `EditorSession` (`EditorSession::insert` →
//!   `TextEdit::insert`),
//! - one ASCII character typed into one block field followed by
//!   `commit_block_as(BlockEditKind::Typing)`, plus `text()` and the journal depth, as the
//!   app does per keystroke,
//! - the same insert plus the per-frame `syntax_spans` re-highlight below 128 KiB,
//! - undo/redo of one step at each size.
//!
//! It also runs the OI-058 worst case: a 1 MiB document with no blank line, so block mode
//! holds a single ~1 MiB field, and a single-field ladder (32 KiB…1 MiB) to show where the
//! 16 ms p95 budget (docs/TESTING.md §9) would fall if the ADR-016 threshold moved.
//!
//! Each timing run discards a warm-up and reports nearest-rank p50/p95/p99/max in ms. The
//! `bytes/op` column is a model, not an instrumented counter: it is the minimum number of
//! bytes the measured path copies, scans or shifts per operation, with the formula noted on
//! the case. On the typing rows this is bytes per keystroke. `MB/s` divides that model by the
//! p50, and `doc×` shows how much of the document one operation touches — which is the property
//! ADR-016 is about.

#![forbid(unsafe_code)]

use std::hint::black_box;
use std::process;
use std::time::{Duration, Instant};

use lmdv_core::Eol;
use lmdv_edit::{BlockEditKind, EditorSession, block_ranges};
use rune_kit::syntax::SyntaxTheme;

/// p95 budget from `docs/TESTING.md` §9 (typing commit, 64 KiB single field / 1 MiB block).
const BUDGET: Duration = Duration::from_millis(16);

/// The characters typed, cycled. Spaces are included so lines and words look like typing.
const TYPED: &[&str] =
    &["t", "h", "e", " ", "q", "u", "i", "c", "k", " ", "b", "r", "o", "w", "n", " ", "f", "o", "x", " "];

/// Single-field sizes measured to locate the 16 ms p95 crossing.
const SINGLE_FIELD_SIZES: &[(&str, usize)] = &[
    ("32 KiB", 32 * 1024),
    ("64 KiB", 64 * 1024),
    ("128 KiB", 128 * 1024),
    ("256 KiB", 256 * 1024),
    ("1 MiB", 1024 * 1024),
];

const BLOCK_SIZE: usize = 1024 * 1024;

fn main() {
    let options = Options::parse();
    println!("SP-03 typing benchmark (OI-064) — lmdv-edit");
    println!(
        "build: {}",
        if cfg!(debug_assertions) { "debug — numbers are not meaningful; rerun with --release" } else { "release" }
    );
    println!("run: {} keystrokes per case, {} warm-up, nearest-rank percentiles", options.keystrokes, options.warmup);
    println!("documents: deterministic xorshift64* seed, prose + headings + lists + tables + fenced code");
    if let Some(cpu) = cpu_model() {
        println!("host: {} ({})", cpu, std::env::consts::ARCH);
    }

    let mut rows = Vec::new();
    let mut notes = Vec::new();

    let mut ladder = Vec::new();
    for &(label, size) in SINGLE_FIELD_SIZES {
        let text = generate_prose_document(size);
        let blocks = block_ranges(&text).len();
        notes.push(format!("{label:>7} prose: {} bytes, {blocks} blocks", text.len()));

        let (mut samples, bytes, note) = bench_single_field_insert(&text, &options);
        rows.push(Row::new(format!("single-field insert {label}"), Stats::of(&mut samples), bytes, text.len()));
        notes.push(note);

        let p95 = rows.last().and_then(|row| row.stats).map_or(0, |stats| stats.p95);
        ladder.push((label, p95));

        if size == 64 * 1024 {
            let (mut samples, bytes, note) = bench_single_field_insert_with_syntax(&text, &options);
            rows.push(Row::new(
                format!("single-field insert + syntax {label}"),
                Stats::of(&mut samples),
                bytes,
                text.len(),
            ));
            notes.push(note);
        }
    }

    let prose = generate_prose_document(BLOCK_SIZE);
    {
        let (mut samples, bytes, note) = bench_block_commit(&prose, &options, "prose");
        rows.push(Row::new(
            "block field + commit 1 MiB prose".to_string(),
            Stats::of(&mut samples),
            bytes,
            prose.len(),
        ));
        notes.push(note);
    }

    if options.huge_block {
        let huge = generate_single_block_document(BLOCK_SIZE);
        let blocks = block_ranges(&huge).len();
        assert_eq!(blocks, 1, "the OI-058 fixture must be one blank-line block");
        let (mut samples, bytes, note) = bench_block_commit(&huge, &options, "one block");
        rows.push(Row::new(
            "block field + commit 1 MiB one block".to_string(),
            Stats::of(&mut samples),
            bytes,
            huge.len(),
        ));
        notes.push(note);
    }

    let small = generate_prose_document(64 * 1024);
    let (mut samples, bytes, note) = bench_undo_redo(&small, &options, Mode::SingleField, false);
    rows.push(Row::new("undo single-field 64 KiB".to_string(), Stats::of(&mut samples), bytes, small.len()));
    notes.push(note);
    let (mut samples, bytes, note) = bench_undo_redo(&small, &options, Mode::SingleField, true);
    rows.push(Row::new("redo single-field 64 KiB".to_string(), Stats::of(&mut samples), bytes, small.len()));
    notes.push(note);

    let (mut samples, bytes, note) = bench_undo_redo(&prose, &options, Mode::SingleField, false);
    rows.push(Row::new("undo single-field 1 MiB".to_string(), Stats::of(&mut samples), bytes, prose.len()));
    notes.push(note);

    let (mut samples, bytes, note) = bench_undo_redo(&prose, &options, Mode::Block, false);
    rows.push(Row::new("undo block 1 MiB prose".to_string(), Stats::of(&mut samples), bytes, prose.len()));
    notes.push(note);
    let (mut samples, bytes, note) = bench_undo_redo(&prose, &options, Mode::Block, true);
    rows.push(Row::new("redo block 1 MiB prose".to_string(), Stats::of(&mut samples), bytes, prose.len()));
    notes.push(note);

    print_table(&rows);
    print_notes(&notes);
    print_verdict(&rows, &ladder, options.undo_steps);
}

/// Parsed command line.
struct Options {
    keystrokes: usize,
    warmup: usize,
    undo_steps: usize,
    huge_block: bool,
}

impl Options {
    fn parse() -> Self {
        let mut options = Self { keystrokes: 300, warmup: 20, undo_steps: 100, huge_block: true };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--keystrokes" => options.keystrokes = count(&mut args, "--keystrokes"),
                "--warmup" => options.warmup = count(&mut args, "--warmup"),
                "--undo-steps" => options.undo_steps = count(&mut args, "--undo-steps"),
                "--no-huge-block" => options.huge_block = false,
                "--help" | "-h" => {
                    print_help();
                    process::exit(0);
                }
                other => {
                    eprintln!("unknown argument: {other}");
                    print_help();
                    process::exit(2);
                }
            }
        }
        if options.keystrokes == 0 {
            eprintln!("--keystrokes must be greater than zero");
            process::exit(2);
        }
        options
    }
}

fn print_help() {
    println!("usage: typing_bench [--keystrokes N] [--warmup N] [--undo-steps N] [--no-huge-block]");
    println!();
    println!("  --keystrokes N     measured keystrokes per case (default 300)");
    println!("  --warmup N         discarded keystrokes per case (default 20)");
    println!("  --undo-steps N     undo/redo steps prepared per case (default 100; the history");
    println!("                     caps at 128 steps / 1 MiB, so fewer may be available)");
    println!("  --no-huge-block    skip the OI-058 1 MiB single-block worst case");
}

fn count(args: &mut impl Iterator<Item = String>, flag: &str) -> usize {
    match args.next().and_then(|value| value.parse::<usize>().ok()) {
        Some(value) => value,
        None => {
            eprintln!("{flag} needs a number");
            process::exit(2);
        }
    }
}

/// Nearest-rank statistics of one timing run, in nanoseconds.
#[derive(Clone, Copy)]
struct Stats {
    n: usize,
    p50: u64,
    p95: u64,
    p99: u64,
    max: u64,
}

impl Stats {
    fn of(samples: &mut [u64]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        samples.sort_unstable();
        let n = samples.len();
        let rank = |percent: usize| samples[(n * percent / 100).min(n - 1)];
        Some(Self { n, p50: rank(50), p95: rank(95), p99: rank(99), max: samples[n - 1] })
    }
}

struct Row {
    case: String,
    stats: Option<Stats>,
    bytes_per_op: usize,
    doc_len: usize,
}

impl Row {
    fn new(case: String, stats: Option<Stats>, bytes_per_op: usize, doc_len: usize) -> Self {
        Self { case, stats, bytes_per_op, doc_len }
    }
}

fn print_table(rows: &[Row]) {
    println!();
    println!(
        "{:<46} {:>5} {:>9} {:>9} {:>9} {:>9} {:>12} {:>9} {:>7}",
        "case", "n", "p50 ms", "p95 ms", "p99 ms", "max ms", "bytes/op", "MB/s", "doc×"
    );
    for row in rows {
        let Some(stats) = row.stats else {
            println!("{:<46} {:>5}", row.case, "n/a");
            continue;
        };
        let seconds = stats.p50.max(1) as f64 / 1e9;
        let throughput = row.bytes_per_op as f64 / seconds / 1e6;
        let doc_ratio = if row.doc_len == 0 { 0.0 } else { row.bytes_per_op as f64 / row.doc_len as f64 };
        println!(
            "{:<46} {:>5} {:>9.4} {:>9.4} {:>9.4} {:>9.4} {:>12} {:>9.0} {:>7.2}",
            row.case,
            stats.n,
            stats.p50 as f64 / 1e6,
            stats.p95 as f64 / 1e6,
            stats.p99 as f64 / 1e6,
            stats.max as f64 / 1e6,
            row.bytes_per_op,
            throughput,
            doc_ratio
        );
    }
}

fn print_notes(notes: &[String]) {
    println!();
    println!("case notes");
    for note in notes {
        println!("  {note}");
    }
}

fn print_verdict(rows: &[Row], ladder: &[(&str, u64)], undo_steps: usize) {
    println!();
    println!("16 ms p95 budget (docs/TESTING.md §9)");
    for row in rows {
        let Some(stats) = row.stats else { continue };
        let verdict = if stats.p95 <= BUDGET.as_nanos() as u64 { "within budget" } else { "OVER budget" };
        println!("  {:<46} p95 {:>8.4} ms  {verdict}", row.case, stats.p95 as f64 / 1e6);
    }

    println!();
    println!("single-field threshold check (p95 per size)");
    let mut inside = Vec::new();
    let mut outside = Vec::new();
    for (label, p95) in ladder {
        let over = *p95 > BUDGET.as_nanos() as u64;
        let line = format!("{label} p95 {:.4} ms", *p95 as f64 / 1e6);
        if over {
            outside.push(line);
        } else {
            inside.push(line);
        }
    }
    if inside.is_empty() {
        println!("  no measured single-field size is inside the 16 ms budget");
    } else {
        println!("  inside the budget: {}", inside.join(", "));
    }
    if outside.is_empty() {
        println!("  outside the budget: none of the measured sizes");
    } else {
        println!("  outside the budget: {}", outside.join(", "));
    }
    let single_1m = rows.iter().find(|row| row.case == "undo single-field 1 MiB").and_then(|row| row.stats);
    let block_1m = rows.iter().find(|row| row.case == "undo block 1 MiB prose").and_then(|row| row.stats);
    if let (None, Some(block)) = (single_1m, block_1m) {
        println!();
        println!("finding: a 1 MiB single field keeps no undo step — Rune caps history at 1 MiB and one");
        println!(
            "         before/after pair is ~2 MiB. Block mode kept {}/{} steps at the same size.",
            block.n, undo_steps
        );
    }

    println!();
    println!("notes: bytes/op is a lower-bound model of the measured code path (see the module docs);");
    println!("       timings are per operation on this machine, informational only.");
}

/// Best-effort CPU model for a self-describing report.
fn cpu_model() -> Option<String> {
    let info = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    info.lines()
        .find_map(|line| line.strip_prefix("model name"))
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, model)| model.trim().to_string())
}

/// One ASCII character typed at the document midpoint in one `TextEdit`.
fn bench_single_field_insert(text: &str, options: &Options) -> (Vec<u64>, usize, String) {
    let mut session = EditorSession::new(text, Eol::Lf);
    let caret = text.floor_char_boundary(text.len() / 2);
    session.set_selection(caret..caret);
    let mut samples = Vec::with_capacity(options.keystrokes);
    for step in 0..options.warmup + options.keystrokes {
        let started = Instant::now();
        let changed = session.insert(TYPED[step % TYPED.len()]);
        let len = session.text().len();
        let elapsed = started.elapsed().as_nanos() as u64;
        assert!(changed, "every typed character must change the buffer");
        black_box(len);
        if step >= options.warmup {
            samples.push(elapsed);
        }
    }
    // Model: one whole-buffer undo snapshot (Rune stores before/after text per step) plus the
    // spliced tail. The full-text history comparisons are extra reads and are not counted.
    let bytes = text.len() + (text.len() - caret);
    let note = format!(
        "{:>7} single-field: typed {} chars at byte {caret} (doc× model = 1 whole snapshot + tail)",
        human_size(text.len()),
        options.keystrokes
    );
    (samples, bytes, note)
}

/// The same insert plus the next frame's syntax spans; below 128 KiB that re-highlights the
/// whole buffer because the session revision changed.
fn bench_single_field_insert_with_syntax(text: &str, options: &Options) -> (Vec<u64>, usize, String) {
    let mut session = EditorSession::new(text, Eol::Lf);
    let theme = SyntaxTheme::neutral(true);
    let caret = text.floor_char_boundary(text.len() / 2);
    session.set_selection(caret..caret);
    let mut samples = Vec::with_capacity(options.keystrokes);
    let mut spans = 0;
    for step in 0..options.warmup + options.keystrokes {
        let started = Instant::now();
        let changed = session.insert(TYPED[step % TYPED.len()]);
        spans = session.syntax_spans(theme, None).len();
        let elapsed = started.elapsed().as_nanos() as u64;
        assert!(changed, "every typed character must change the buffer");
        if step >= options.warmup {
            samples.push(elapsed);
        }
    }
    black_box(spans);
    // Model: the insert path (snapshot + tail) plus one highlight pass over the whole text.
    let bytes = text.len() + (text.len() - caret) + text.len();
    let note = format!(
        "{:>7} insert + syntax: {spans} spans from a full re-highlight (SYNTAX_MAX_BYTES = 128 KiB)",
        human_size(text.len())
    );
    (samples, bytes, note)
}

/// One ASCII character typed into one block field and committed with `Typing`, plus the
/// per-keystroke `text()` and journal reads the app performs.
fn bench_block_commit(text: &str, options: &Options, label: &str) -> (Vec<u64>, usize, String) {
    let mut session = EditorSession::new(text, Eol::Lf);
    assert!(
        session.block_mode().is_some(),
        "{} bytes must be above the block-mode threshold (case: {label})",
        text.len()
    );
    let spot = pick_block(&session, text.len() / 2);
    let field = session.block_editor_mut().and_then(|blocks| blocks.block_mut(spot.index));
    field.expect("the chosen block").set_caret(spot.local);

    let mut samples = Vec::with_capacity(options.keystrokes);
    for step in 0..options.warmup + options.keystrokes {
        let started = Instant::now();
        if let Some(field) = session.block_editor_mut().and_then(|blocks| blocks.block_mut(spot.index)) {
            field.insert(TYPED[step % TYPED.len()]);
        }
        let changed = session.commit_block_as(spot.index, BlockEditKind::Typing);
        let len = session.text().len();
        let depth = journal_depth(&session);
        let elapsed = started.elapsed().as_nanos() as u64;
        assert!(changed, "every committed character must change the source");
        black_box((len, depth));
        if step >= options.warmup {
            samples.push(elapsed);
        }
    }
    let depth = journal_depth(&session);
    let blocks = session.block_editor().map_or(0, |editor| editor.blocks().len());
    // Model: the journal's before/after block copies and the delta scan, the re-partition scan
    // over the edited block ± one neighbour, the shifted source tail, and the block-range shift.
    let bytes = 3 * spot.len + spot.tail + 16 * blocks;
    let note = format!(
        "{:>7} {label}: {}, block #{} is {} bytes, journal has {} after the run",
        human_size(text.len()),
        plural(blocks, "block", "blocks"),
        spot.index,
        spot.len,
        plural(depth, "entry", "entries")
    );
    (samples, bytes, note)
}

/// Undo or redo of one discrete step, at one mode and size.
fn bench_undo_redo(text: &str, options: &Options, mode: Mode, redo: bool) -> (Vec<u64>, usize, String) {
    let steps = options.undo_steps;
    let (mut session, spot) = match mode {
        Mode::SingleField => {
            let mut session = EditorSession::new(text, Eol::Lf);
            let caret = text.floor_char_boundary(text.len() / 2);
            for step in 0..steps {
                session.set_selection(caret + step..caret + step);
                let changed = session.insert(TYPED[step % TYPED.len()]);
                assert!(changed, "each prepared edit must change the buffer");
            }
            (session, None)
        }
        Mode::Block => {
            let mut session = EditorSession::new(text, Eol::Lf);
            assert!(session.block_mode().is_some(), "block undo needs block mode");
            let spot = pick_block(&session, text.len() / 2);
            for step in 0..steps {
                if let Some(field) = session.block_editor_mut().and_then(|blocks| blocks.block_mut(spot.index)) {
                    field.set_caret(spot.local);
                    field.insert(TYPED[step % TYPED.len()]);
                }
                let changed = session.commit_block_as(spot.index, BlockEditKind::Discrete);
                assert!(changed, "each prepared block edit must change the source");
            }
            (session, Some(spot))
        }
    };

    // The history caps at 128 steps / 1 MiB, so the number of available steps can be smaller.
    if redo {
        // Walk back to the start first; a redo needs something to redo.
        for _ in 0..steps {
            if !session.undo() {
                break;
            }
        }
    }
    let mut samples = Vec::with_capacity(steps);
    for _ in 0..steps {
        let started = Instant::now();
        let changed = if redo { session.redo() } else { session.undo() };
        let elapsed = started.elapsed().as_nanos() as u64;
        if !changed {
            break;
        }
        samples.push(elapsed);
    }
    let available = samples.len();

    let (bytes, note) = match spot {
        None => {
            // Model: EditorSession clones the buffer, then Rune compares and restores full text.
            let bytes = 3 * text.len();
            let note = if available == 0 {
                format!(
                    "{:>7} {} single-field: no step is retained at this size — Rune's 1 MiB history cap drops every before/after pair (~2 MiB here), so the field has no undo",
                    human_size(text.len()),
                    if redo { "redo" } else { "undo" }
                )
            } else {
                format!(
                    "{:>7} {} single-field: {available} of {steps} steps available (Rune history caps at 128 steps / 1 MiB)",
                    human_size(text.len()),
                    if redo { "redo" } else { "undo" }
                )
            };
            (bytes, note)
        }
        Some(spot) => {
            // Model: BlockEditor::undo scans every block's pending state, then restores one block.
            let bytes = text.len() + 2 * spot.len;
            let blocks = session.block_editor().map_or(0, |editor| editor.blocks().len());
            let note = format!(
                "{:>7} {} block: {available} of {steps} steps available; {} blocks, edited block #{} ({} bytes)",
                human_size(text.len()),
                if redo { "redo" } else { "undo" },
                blocks,
                spot.index,
                spot.len
            );
            (bytes, note)
        }
    };
    (samples, bytes, note)
}

#[derive(Clone, Copy)]
enum Mode {
    SingleField,
    Block,
}

/// Where to type in block mode: the edited block and the caret's offset inside it.
struct BlockSpot {
    index: usize,
    local: usize,
    len: usize,
    tail: usize,
}

/// Picks a block near `at` whose first line is content (not an empty leading line or a bare
/// fence opener), so typing cannot create or remove a blank-line boundary mid-run.
fn pick_block(session: &EditorSession, at: usize) -> BlockSpot {
    let Some(editor) = session.block_editor() else {
        panic!("block mode is not active");
    };
    let count = editor.blocks().len();
    let start = editor.block_index_at(at).unwrap_or(0);
    let usable = |index: usize| -> bool {
        let Some(block) = editor.block(index) else {
            return false;
        };
        let text = &block.edit.text;
        let line_end = text.find('\n').unwrap_or(text.len());
        let first = text[..line_end].trim_start();
        first.len() >= 16
            && first.chars().next().is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '#' || ch == '-' || ch == '>')
    };
    let index = (start..count)
        .find(|&candidate| usable(candidate))
        .or_else(|| (0..start).rev().find(|&candidate| usable(candidate)))
        .unwrap_or(start);
    let block = editor.block(index).expect("the chosen block exists");
    let text = &block.edit.text;
    let line_end = text.find('\n').unwrap_or(text.len());
    let local = text.floor_char_boundary(line_end / 2);
    BlockSpot { index, local, len: text.len(), tail: session.text().len() - block.range.end }
}

fn journal_depth(session: &EditorSession) -> usize {
    session.block_editor().map_or(0, |editor| editor.undo_depth())
}

/// Deterministic generator: prose paragraphs, headings, bullet lists and fenced code,
/// trimmed to `target` bytes (plus a final newline when the cut landed mid-line).
fn generate_prose_document(target: usize) -> String {
    let mut rng = Rng::new(0x5eed_1ead_cafe_f00d);
    let mut out = String::with_capacity(target + 8192);
    let mut section = 0usize;
    while out.len() < target {
        match section % 7 {
            0 => {
                out.push_str("## Section ");
                out.push_str(&(section + 1).to_string());
                out.push_str("\n\n");
                for _ in 0..3 {
                    let words = 40 + (rng.next() % 60) as usize;
                    push_paragraph(&mut out, &mut rng, words);
                }
                out.push('\n');
            }
            4 => {
                out.push_str("```rust\n");
                for line in 0..8 {
                    out.push_str("let value_");
                    out.push_str(&line.to_string());
                    out.push_str(" = compute(");
                    out.push_str(&(line * 17).to_string());
                    out.push_str(");\n");
                }
                out.push_str("```\n\n");
            }
            5 => {
                out.push_str("- first item\n- second item with `code`\n- third item\n\n");
            }
            _ => {
                let words = 70 + (rng.next() % 80) as usize;
                push_paragraph(&mut out, &mut rng, words);
                out.push('\n');
            }
        }
        section += 1;
    }
    trim_to_target(out, target)
}

/// The OI-058 worst case: no blank line at all, so block mode builds one field the size of
/// the whole document. Lines wrap every 12 words.
fn generate_single_block_document(target: usize) -> String {
    let mut rng = Rng::new(0x0dd_b17e_5eed_1eaf);
    let mut out = String::with_capacity(target + 8192);
    let mut column = 0;
    while out.len() < target {
        if column > 0 {
            out.push(' ');
        }
        out.push_str(rng.pick(WORDS));
        column += 1;
        if column == 12 {
            out.push('\n');
            column = 0;
        }
    }
    trim_to_target(out, target)
}

fn push_paragraph(out: &mut String, rng: &mut Rng, words: usize) {
    for word in 0..words {
        if word > 0 {
            out.push(' ');
        }
        out.push_str(rng.pick(WORDS));
    }
    out.push_str(".\n");
}

fn trim_to_target(mut text: String, target: usize) -> String {
    if text.len() > target {
        let mut end = target;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn human_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.0} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.0} KiB", bytes as f64 / 1024.0)
    }
}

/// xorshift64*: deterministic and dependency-free.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut state = self.0;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.0 = state;
        state.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next() % items.len() as u64) as usize]
    }
}

const WORDS: &[&str] = &[
    "the",
    "editor",
    "keeps",
    "canonical",
    "text",
    "and",
    "line",
    "ending",
    "so",
    "save",
    "writes",
    "exactly",
    "what",
    "the",
    "user",
    "saw",
    "while",
    "typing",
    "a",
    "large",
    "document",
    "block",
    "mode",
    "splits",
    "paragraphs",
    "into",
    "fields",
    "so",
    "each",
    "keystroke",
    "touches",
    "one",
    "block",
    "instead",
    "of",
    "the",
    "whole",
    "buffer",
    "journal",
    "undo",
    "redo",
    "across",
    "boundaries",
    "without",
    "losing",
    "the",
    "caret",
    "preview",
    "reparse",
    "after",
    "debounce",
    "drafts",
    "watcher",
    "conflicts",
    "and",
    "search",
    "still",
    "returns",
    "matches",
    "from",
    "the",
    "committed",
    "source",
];
