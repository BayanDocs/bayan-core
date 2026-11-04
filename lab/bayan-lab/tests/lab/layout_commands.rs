//! The layout JSON commands, `bayan-lab pdf-extract` and `bayan-lab layout schema|check` (LAB-003), run in-process on the fixture PDFs of bayan-lab-layout and on files in temporary folders.

use std::fs;
use std::path::{Path, PathBuf};

use bayan_lab::cli::{Environment, Outcome, run_in};
use bayan_lab_layout::layout::Layout;

use crate::support::temporary_folder;

/// What one command printed, and how it ended.
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

/// A fixture of bayan-lab-layout's tests.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bayan-lab-layout/tests/fixtures")
        .join(name)
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn pdf_extract_writes_the_layout_json() {
    let pdf = fixture("libreoffice.pdf");
    let ran = lab(&["pdf-extract", text(&pdf)]);
    assert_eq!(ran.outcome, Outcome::Success, "{}", ran.err);
    assert_eq!(ran.err, "");
    let layout = Layout::from_json(&ran.out).unwrap();
    assert_eq!(layout.page_count, 2);
    assert!(
        ran.out.ends_with("}\n") && !ran.out.contains("\n  "),
        "compact JSON with a final line feed"
    );
    // The same with --output.
    let folder = temporary_folder("pdf-extract");
    let output = folder.join("layout.json");
    let ran = lab(&["pdf-extract", "--output", text(&output), text(&pdf)]);
    assert_eq!(
        (ran.outcome, ran.out.as_str(), ran.err.as_str()),
        (Outcome::Success, "", "")
    );
    assert_eq!(
        Layout::from_json(&fs::read_to_string(&output).unwrap()).unwrap(),
        layout
    );
}

#[test]
fn pdf_extract_fails_on_a_refused_pdf_without_repeating_its_name() {
    let folder = temporary_folder("pdf-extract-refused");
    let file = folder.join("Confidential report.pdf");
    fs::write(&file, b"%PDF-1.7\nnot really a PDF").unwrap();
    let ran = lab(&["pdf-extract", text(&file)]);
    assert_eq!(ran.outcome, Outcome::Failed);
    assert_eq!(ran.out, "");
    assert_eq!(
        ran.err,
        "bayan-lab: the PDF is malformed: the file does not end with `startxref`\n"
    );
    // A file that does not exist is not a refused PDF: the command could not run.
    let ran = lab(&["pdf-extract", text(&folder.join("missing.pdf"))]);
    assert_eq!(ran.outcome, Outcome::Error);
    assert!(
        ran.err.starts_with("bayan-lab: cannot read the PDF: "),
        "{}",
        ran.err
    );
    assert!(!ran.err.contains("missing.pdf"), "{}", ran.err);
}

#[test]
fn pdf_extract_warns_about_glyphs_without_characters_or_lines() {
    // A one-page PDF whose font has no ToUnicode map and no encoding, and whose second string is drawn vertically.
    let content = b"BT /F1 10 Tf 100 700 Td (AB) Tj 0 1 -1 0 300 300 Tm (C) Tj ET";
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_owned(),
        "<< /Type /Font /Subtype /TrueType /BaseFont /Symbolic /FirstChar 65 /LastChar 67 /Widths [500 500 500] >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{}\nendstream", content.len(), String::from_utf8_lossy(content)),
    ];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    let folder = temporary_folder("pdf-extract-warnings");
    let file = folder.join("symbolic.pdf");
    fs::write(&file, &pdf).unwrap();
    let ran = lab(&["pdf-extract", text(&file)]);
    assert_eq!(ran.outcome, Outcome::Success);
    assert_eq!(
        ran.err,
        "bayan-lab: warning: 2 glyphs have characters that neither a ToUnicode map nor their font's encoding gives; their text is U+FFFD\n\
         bayan-lab: warning: 1 glyphs are not upright or are invisible, and are on no line (each page's skipped_glyphs counts them)\n"
    );
    let layout = Layout::from_json(&ran.out).unwrap();
    assert_eq!(
        layout.pages[0].lines[0].text.as_deref(),
        Some("\u{FFFD}\u{FFFD}")
    );
}

#[test]
fn pdf_extract_refuses_wrong_arguments() {
    for (args, message) in [
        (&["pdf-extract"][..], "takes exactly one PDF"),
        (&["pdf-extract", "a.pdf", "b.pdf"], "takes exactly one PDF"),
        (
            &["pdf-extract", "--pages", "1", "a.pdf"],
            "unknown option --pages",
        ),
        (
            &["pdf-extract", "--output", "", "a.pdf"],
            "--output cannot be empty",
        ),
    ] {
        let ran = lab(args);
        assert_eq!(ran.outcome, Outcome::Error, "{args:?}");
        assert!(ran.err.contains(message), "{args:?}: {}", ran.err);
    }
}

#[test]
fn layout_schema_prints_the_committed_schema() {
    let ran = lab(&["layout", "schema"]);
    assert_eq!(ran.outcome, Outcome::Success);
    let committed = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../schema/layout.schema.json"),
    )
    .unwrap();
    assert_eq!(ran.out, committed.replace("\r\n", "\n"));
    assert_eq!(lab(&["layout", "schema", "extra"]).outcome, Outcome::Error);
    assert_eq!(lab(&["layout"]).outcome, Outcome::Error);
}

#[test]
fn layout_check_reports_each_file() {
    let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../schema/samples");
    let folder = temporary_folder("layout-check");
    let broken = folder.join("broken.json");
    let engine = fs::read_to_string(samples.join("engine.json")).unwrap();
    fs::write(
        &broken,
        engine.replace("\"page_count\": 1", "\"page_count\": 3"),
    )
    .unwrap();
    let good = samples.join("word.json");
    let ran = lab(&["layout", "check", text(&good), text(&broken)]);
    assert_eq!(ran.outcome, Outcome::Failed);
    assert_eq!(ran.out, format!("{}: ok\n", text(&good)));
    assert_eq!(
        ran.err,
        format!(
            "bayan-lab: {}: page_count is 3 but there are 1 pages\n",
            text(&broken)
        )
    );
    let ran = lab(&["layout", "check", text(&good)]);
    assert_eq!(ran.outcome, Outcome::Success);
    assert_eq!(lab(&["layout", "check"]).outcome, Outcome::Error);
}
