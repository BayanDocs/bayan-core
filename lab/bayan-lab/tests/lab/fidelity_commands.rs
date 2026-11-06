//! The fidelity commands of LAB-004, `bayan-lab compare` and `bayan-lab report`, run in-process on layouts and corpora in temporary folders.

use std::fs;
use std::path::{Path, PathBuf};

use bayan_lab::cli::{Environment, Outcome, run_in};
use bayan_lab::hash::Sha256;
use bayan_lab_compare::summary::Summary;
use bayan_lab_layout::layout::{
    Direction, Font, Glyph, Layout, Line, LineCoverage, Page, Points, SchemaVersion, Source,
    TextOffsets,
};

use crate::support::{Docx, temporary_folder};

// REUSE-IgnoreStart: REUSE would read the test source's copyright statement as this file's own.
const COPYRIGHT: &str = "Copyright (c) Example";
// REUSE-IgnoreEnd

/// What one command printed, and how it ended.
#[derive(Debug)]
struct Ran {
    outcome: Outcome,
    out: String,
    err: String,
}

fn lab(args: &[&str]) -> Ran {
    let args: Vec<String> = args.iter().map(|&arg| arg.to_owned()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let outcome = run_in(&Environment::default(), &args, &mut out, &mut err);
    Ran {
        outcome,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
    }
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// A sample layout of bayan-lab-layout's schema folder.
fn sample(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../schema/samples")
        .join(name)
}

fn points(millis: i64) -> Points {
    Points::from_millis(millis).unwrap()
}

/// A layout of `pages`, each a list of lines (text, baseline in points), one glyph per character 6 pt apart: Word's side (`pdf`, with offsets through its text) or BayanDocs' (`engine`).
fn layout(source: Source, pages: &[&[(&str, i64)]]) -> Layout {
    let mut offset = 0_u64;
    let mut built = Vec::new();
    for lines in pages {
        let start = offset;
        let mut page_lines = Vec::new();
        for &(line_text, baseline) in *lines {
            let count = u64::try_from(line_text.chars().count()).unwrap();
            let glyphs = (0..line_text.chars().count())
                .map(|index| Glyph {
                    offset: u32::try_from(index).unwrap(),
                    length: 1,
                    x: points(72_000 + 6_000 * i64::try_from(index).unwrap()),
                    y: points(baseline * 1000),
                    advance: points(6_000),
                    font: 0,
                    size: points(12_000),
                    gid: None,
                    x_blu: None,
                    y_blu: None,
                    advance_blu: None,
                    size_blu: None,
                })
                .collect();
            let pdf = source == Source::Pdf;
            page_lines.push(Line {
                start: pdf.then_some(offset),
                end: pdf.then_some(offset + count),
                text: Some(line_text.to_owned()),
                direction: Some(Direction::Ltr),
                baseline: Some(points(baseline * 1000)),
                baseline_blu: None,
                glyphs: Some(glyphs),
            });
            offset += count + 1;
        }
        built.push(Page {
            width: points(612_000),
            height: points(792_000),
            width_blu: None,
            height_blu: None,
            start: (source == Source::Pdf).then_some(start),
            lines: page_lines,
            elements: (source == Source::Pdf).then(Vec::new),
            skipped_glyphs: (source == Source::Pdf).then_some(0),
        });
    }
    let layout = Layout {
        schema: SchemaVersion::V1,
        source,
        producer: "test".to_owned(),
        text_offsets: if source == Source::Pdf {
            TextOffsets::Lines
        } else {
            TextOffsets::Document
        },
        line_coverage: LineCoverage::All,
        page_count: u32::try_from(pages.len()).unwrap(),
        fonts: vec![Font {
            name: "Test".to_owned(),
            family: None,
        }],
        pages: built,
    };
    layout.check().unwrap();
    layout
}

fn json(value: &str) -> serde_json::Value {
    serde_json::from_str(value).unwrap()
}

#[test]
fn compare_writes_the_metrics_of_two_layouts() {
    // A real extraction compared with itself agrees everywhere, right-to-left lines included.
    let file = sample("pdf-libreoffice.json");
    let ran = lab(&["compare", text(&file), text(&file)]);
    assert_eq!(ran.outcome, Outcome::Success, "{}", ran.err);
    assert_eq!(ran.err, "");
    let result = json(&ran.out);
    assert_eq!(result["format"], "bayan-lab-fidelity-comparison");
    assert_eq!(result["metrics"], 1);
    let comparison = &result["comparison"];
    assert_eq!(
        comparison["m1"],
        json(r#"{"reference": 2, "candidate": 2}"#)
    );
    for metric in ["m2", "m3"] {
        assert_eq!(
            comparison[metric]["agreed"], comparison[metric]["total"],
            "{metric}"
        );
        assert!(comparison[metric].get("first_difference").is_none());
    }
    assert_eq!(comparison["m4"]["x"]["max"], 0.0);
    assert_eq!(comparison["m4"]["y"]["max"], 0.0);
    assert_eq!(
        comparison["text"]["matched"],
        comparison["text"]["reference"]
    );
    assert_eq!(result["notes"].as_array().unwrap().len(), 2);
    // With --output, to a file.
    let folder = temporary_folder("compare-output");
    let output = folder.join("result.json");
    let ran = lab(&[
        "compare",
        "--output",
        text(&output),
        text(&file),
        text(&file),
    ]);
    assert_eq!((ran.outcome, ran.out.as_str()), (Outcome::Success, ""));
    assert_eq!(json(&fs::read_to_string(&output).unwrap()), result);
}

#[test]
fn compare_with_hints_measures_m7() {
    let folder = temporary_folder("compare-hints");
    let docx = folder.join("document.docx");
    fs::write(
        &docx,
        Docx::new("<w:p><w:r><w:t>First page</w:t></w:r></w:p><w:p><w:r><w:lastRenderedPageBreak/><w:t>Second page</w:t></w:r></w:p>").bytes(),
    )
    .unwrap();
    let reference = folder.join("word.json");
    let candidate = folder.join("bayandocs.json");
    fs::write(
        &reference,
        layout(
            Source::Pdf,
            &[&[("First page", 100)], &[("Second page", 100)]],
        )
        .to_json()
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &candidate,
        layout(
            Source::Engine,
            &[&[("First page", 100), ("Second page", 114)]],
        )
        .to_json()
        .unwrap(),
    )
    .unwrap();
    let ran = lab(&[
        "compare",
        "--hints",
        text(&docx),
        text(&reference),
        text(&candidate),
    ]);
    assert_eq!(ran.outcome, Outcome::Success, "{}", ran.err);
    let result = json(&ran.out);
    assert_eq!(
        result["hints"],
        json(
            r#"{"hints": {"total": 1, "agreed": 0, "agreed_breaks": [], "first_difference": {"page": 2}}, "pages": {"reference": 2, "candidate": 1}}"#
        )
    );
    assert_eq!(result["comparison"]["m2"]["agreed"], 1);
    // A document without hints: M7 does not apply.
    fs::write(
        &docx,
        Docx::new("<w:p><w:r><w:t>First page</w:t></w:r></w:p>").bytes(),
    )
    .unwrap();
    let ran = lab(&[
        "compare",
        "--hints",
        text(&docx),
        text(&reference),
        text(&candidate),
    ]);
    assert_eq!(ran.outcome, Outcome::Success);
    assert!(json(&ran.out).get("hints").is_none());
    assert_eq!(
        ran.err,
        "bayan-lab: the document has no w:lastRenderedPageBreak hints, so M7 does not apply\n"
    );
}

#[test]
fn compare_refuses_what_is_not_a_layout_without_naming_it() {
    let folder = temporary_folder("compare-refused");
    let secret = folder.join("Confidential merger.json");
    fs::write(&secret, "{\"schema\": \"Confidential merger\"}").unwrap();
    let good = sample("engine.json");
    let ran = lab(&["compare", text(&good), text(&secret)]);
    assert_eq!(ran.outcome, Outcome::Failed);
    assert_eq!(ran.out, "");
    assert!(
        ran.err
            .starts_with("bayan-lab: the candidate is not a valid layout JSON: line 1, column"),
        "{}",
        ran.err
    );
    assert!(!ran.err.contains("Confidential"), "{}", ran.err);
    // A file that cannot be read is not a refusal: the command could not run.
    let ran = lab(&["compare", text(&good), text(&folder.join("missing.json"))]);
    assert_eq!(ran.outcome, Outcome::Error);
    assert!(
        ran.err
            .starts_with("bayan-lab: cannot read the candidate: "),
        "{}",
        ran.err
    );
    for args in [
        &["compare", text(&good)][..],
        &["compare", "--output", "", text(&good), text(&good)][..],
        &["compare", "--unknown", "x", text(&good), text(&good)][..],
    ] {
        assert_eq!(lab(args).outcome, Outcome::Error, "{args:?}");
    }
}

/// A corpus of two documents in a temporary folder, with a store, and folders for Word's and BayanDocs' layouts.
struct Run {
    manifest: PathBuf,
    store: PathBuf,
    references: PathBuf,
    candidates: PathBuf,
    folder: PathBuf,
    /// The document with hints, and the one without.
    hinted: Sha256,
    plain: Sha256,
}

impl Run {
    fn new(name: &str) -> Self {
        let folder = temporary_folder(name);
        let source = folder.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("LICENSE"),
            format!("MIT License\n\n{COPYRIGHT}\n"),
        )
        .unwrap();
        let hinted = Docx::new("<w:p><w:r><w:t>First page</w:t></w:r></w:p><w:p><w:r><w:lastRenderedPageBreak/><w:t>Second page</w:t></w:r></w:p>").bytes();
        let plain = Docx::new("<w:p><w:r><w:t>Only page</w:t></w:r></w:p>").bytes();
        fs::write(source.join("hinted.docx"), &hinted).unwrap();
        fs::write(source.join("plain.docx"), &plain).unwrap();
        let run = Self {
            manifest: folder.join("manifest.json"),
            store: folder.join("store"),
            references: folder.join("word"),
            candidates: folder.join("bayandocs"),
            hinted: Sha256::of(&hinted),
            plain: Sha256::of(&plain),
            folder,
        };
        fs::create_dir_all(&run.references).unwrap();
        fs::create_dir_all(&run.candidates).unwrap();
        let license = format!("LICENSE={}", text(&source.join("LICENSE")));
        let corpus = |command: &[&str]| {
            let mut args = vec!["corpus"];
            args.extend_from_slice(command);
            args.extend_from_slice(&[
                "--manifest",
                text(&run.manifest),
                "--store",
                text(&run.store),
            ]);
            let ran = lab(&args);
            assert_eq!(ran.outcome, Outcome::Success, "{}", ran.err);
        };
        corpus(&[
            "source",
            "add",
            "example",
            "--url",
            "https://example.invalid/repository",
            "--revision",
            "0123456789abcdef0123456789abcdef01234567",
            "--license",
            "MIT",
            "--copyright",
            COPYRIGHT,
            "--license-file",
            &license,
        ]);
        corpus(&[
            "add",
            "--source",
            "example",
            "--root",
            text(&source),
            "hinted.docx",
            "plain.docx",
        ]);
        run
    }

    /// Writes a layout for the document `sha256` into `folder`.
    fn put(folder: &Path, sha256: Sha256, layout: &Layout) {
        fs::write(
            folder.join(format!("{sha256}.json")),
            layout.to_json().unwrap(),
        )
        .unwrap();
    }

    fn report(&self, extra: &[&str]) -> Ran {
        let (json, html) = (
            self.folder.join("summary.json"),
            self.folder.join("report.html"),
        );
        let mut args = vec![
            "report",
            "--manifest",
            text(&self.manifest),
            "--store",
            text(&self.store),
            "--references",
            text(&self.references),
            "--candidates",
            text(&self.candidates),
            "--json",
            text(&json),
            "--html",
            text(&html),
            "--title",
            "Test corpus",
        ];
        args.extend_from_slice(extra);
        lab(&args)
    }

    fn summary(&self) -> Summary {
        Summary::from_json(&fs::read_to_string(self.folder.join("summary.json")).unwrap()).unwrap()
    }
}

#[test]
fn report_compares_a_corpus_and_detects_regressions() {
    let run = Run::new("report");
    let word = layout(
        Source::Pdf,
        &[&[("First page", 100)], &[("Second page", 100)]],
    );
    Run::put(&run.references, run.hinted, &word);
    Run::put(
        &run.candidates,
        run.hinted,
        &layout(
            Source::Engine,
            &[&[("First page", 100)], &[("Second page", 100)]],
        ),
    );
    let ran = run.report(&[]);
    assert_eq!(ran.outcome, Outcome::Success, "{}", ran.err);
    assert_eq!(
        ran.out,
        "report: 2 documents; 1 compared with Word's layout (1 without it, 0 without BayanDocs' layout), 1 with hints; 0 failed\n"
    );
    assert_eq!(ran.err, "");
    let summary = run.summary();
    assert_eq!(summary.title, "Test corpus");
    assert_eq!(
        (summary.overall.m2.total, summary.overall.m2.agreed),
        (2, 2)
    );
    assert_eq!(
        (summary.overall.m7.total, summary.overall.m7.agreed),
        (1, 1)
    );
    let entry = summary
        .documents
        .iter()
        .find(|entry| entry.document.sha256 == run.hinted.to_string())
        .unwrap();
    assert_eq!(entry.document.name.as_deref(), Some("example:hinted.docx"));
    assert_eq!(
        entry.reference_sha256.as_deref(),
        Some(
            Sha256::of(word.to_json().unwrap().as_bytes())
                .to_string()
                .as_str()
        )
    );
    let page = fs::read_to_string(run.folder.join("report.html")).unwrap();
    assert!(page.starts_with("<!DOCTYPE html>\n") && page.contains("<title>Test corpus</title>"));

    // The same run against its own summary: nothing lost.
    let baseline = run.folder.join("baseline.json");
    fs::copy(run.folder.join("summary.json"), &baseline).unwrap();
    let ran = run.report(&["--baseline", text(&baseline)]);
    assert_eq!(ran.outcome, Outcome::Success, "{}", ran.err);
    assert!(ran.out.ends_with("baseline: 2 documents compared, 0 lost agreement, 0 gained, 0 re-baselined, 0 gone, 0 new\n"), "{}", ran.out);

    // BayanDocs now keeps everything on one page: the page break and the hint are lost.
    Run::put(
        &run.candidates,
        run.hinted,
        &layout(
            Source::Engine,
            &[&[("First page", 100), ("Second page", 114)]],
        ),
    );
    let ran = run.report(&["--baseline", text(&baseline)]);
    assert_eq!(ran.outcome, Outcome::Failed);
    assert_eq!(
        ran.err,
        format!(
            "bayan-lab: document {} lost agreement: 1 page breaks, 0 line breaks, 1 hints, and its page count no longer matches\n",
            run.hinted
        )
    );
    assert!(run.summary().regressed());

    // A candidate that is not a layout: the document fails, without its file's name.
    fs::write(
        run.candidates.join(format!("{}.json", run.plain)),
        "not json",
    )
    .unwrap();
    let ran = run.report(&[]);
    assert_eq!(ran.outcome, Outcome::Failed);
    assert!(
        ran.err.contains(&format!(
            "bayan-lab: document {}: the candidate is not a valid layout JSON: line 1, column",
            run.plain
        )),
        "{}",
        ran.err
    );
    assert!(!ran.err.contains(text(&run.candidates)), "{}", ran.err);
}

#[test]
fn report_needs_an_output_and_takes_no_operands() {
    let run = Run::new("report-arguments");
    let manifest = text(&run.manifest);
    let html = run.folder.join("report.html");
    for args in [
        &["report", "--manifest", manifest][..],
        &[
            "report",
            "--manifest",
            manifest,
            "--html",
            text(&html),
            "extra",
        ][..],
        &["report", "--manifest", manifest, "--html", ""][..],
        &[
            "report",
            "--manifest",
            manifest,
            "--html",
            text(&html),
            "--references",
            manifest,
        ][..],
        &[
            "report",
            "--manifest",
            manifest,
            "--html",
            text(&html),
            "--title",
            " ",
        ][..],
        &[
            "report",
            "--manifest",
            manifest,
            "--html",
            text(&html),
            "--baseline",
            manifest,
        ][..],
    ] {
        let ran = lab(args);
        assert_eq!(ran.outcome, Outcome::Error, "{args:?}: {}", ran.err);
    }
    // Without a store, M7 is not read; without layouts, nothing is compared, and nothing fails.
    let ran = lab(&["report", "--manifest", manifest, "--html", text(&html)]);
    assert_eq!(ran.outcome, Outcome::Success, "{}", ran.err);
    assert_eq!(
        ran.out,
        "report: 2 documents; 0 compared with Word's layout (2 without it, 0 without BayanDocs' layout), 0 with hints; 0 failed\n"
    );
}
