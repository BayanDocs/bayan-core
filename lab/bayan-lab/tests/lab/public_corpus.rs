//! The committed files of the corpus: the manifest's JSON Schema, and the public corpus v1 (`lab/corpus/public/`) with its exclusion list and frequency report (LAB-001 AC-1 and AC-2).
//!
//! The documents themselves are not in Git, so these tests check everything that can be checked without them: that the manifest is consistent, current and in its deterministic layout, that it holds at least 300 public documents, that no document the license review excluded is in it, and that the generated files are what the code generates now. `lab/corpus/public/import.sh` rebuilds the manifest from the sources; after a change to the schema or the report, regenerate those files with `BAYAN_UPDATE_GENERATED=1 cargo test -p bayan-lab --test lab` and review the difference.

use std::path::PathBuf;

use bayan_lab::exclusions::Exclusions;
use bayan_lab::manifest::{Manifest, PUBLIC_LICENSES, Tier};
use bayan_lab::schema;
use bayan_lab::stats::Stats;

/// The report's title; `lab/corpus/public/import.sh` uses the same one.
const REPORT_TITLE: &str = "Public corpus v1: frequency report";

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("corpus")
}

fn read(path: &PathBuf) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        .replace("\r\n", "\n")
}

fn public_manifest() -> Manifest {
    Manifest::from_json(&read(&corpus_dir().join("public").join("manifest.json"))).unwrap()
}

/// Compares a committed generated file with what the code produces now, or rewrites it when `BAYAN_UPDATE_GENERATED` is set.
fn check_generated(path: &PathBuf, generated: &str, what: &str) {
    if std::env::var_os("BAYAN_UPDATE_GENERATED").is_some() {
        std::fs::write(path, generated).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        committed == generated,
        "{} is out of date with {what}. Regenerate it with `BAYAN_UPDATE_GENERATED=1 cargo test -p bayan-lab --test lab` and review the difference.",
        path.display()
    );
}

#[test]
fn the_committed_schema_is_current() {
    check_generated(
        &corpus_dir().join("manifest.schema.json"),
        &schema::json_schema_file().unwrap(),
        "the manifest's Rust types",
    );
}

#[test]
fn the_public_manifest_is_consistent_current_and_in_its_layout() {
    let path = corpus_dir().join("public").join("manifest.json");
    let manifest = public_manifest();
    if let Err(problems) = manifest.check() {
        panic!("the public manifest is not consistent:\n{problems}");
    }
    // Written by the tools, never by hand: the file is exactly the manifest's deterministic layout.
    assert!(
        read(&path) == manifest.to_json().unwrap(),
        "{} is not in the layout the tools write; rebuild it with lab/corpus/public/import.sh",
        path.display()
    );
    assert!(
        manifest.documents.len() >= 300,
        "the public corpus v1 needs at least 300 documents (LAB-001 AC-1), it has {}",
        manifest.documents.len()
    );
    for document in &manifest.documents {
        let id = document.sha256.short();
        assert_eq!(document.tier, Tier::T1, "{id}");
        assert!(PUBLIC_LICENSES.contains(&document.license.as_str()), "{id}");
        assert!(
            document
                .provenance
                .iter()
                .all(|provenance| provenance.path.is_some()),
            "{id}: a public document records where it was found"
        );
        assert!(document.scan.is_some(), "{id}: not tagged");
    }
    for (name, source) in &manifest.sources {
        assert!(
            source.revision.len() == 40
                && source.revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "source {name}: the revision is a full Git commit hash"
        );
        assert!(!source.license_texts.is_empty(), "source {name}");
    }
}

#[test]
fn no_document_the_license_review_excluded_is_in_the_public_corpus() {
    let exclusions =
        Exclusions::parse(&read(&corpus_dir().join("public").join("excluded.tsv"))).unwrap();
    assert!(!exclusions.is_empty());
    let manifest = public_manifest();
    for (sha256, exclusion) in exclusions.iter() {
        assert!(
            manifest.document(sha256).is_none(),
            "{} ({}: {}) is excluded but in the manifest",
            sha256.short(),
            exclusion.source,
            exclusion.path
        );
        assert!(
            manifest.sources.contains_key(&exclusion.source),
            "the exclusion list names the unknown source {}",
            exclusion.source
        );
    }
}

#[test]
fn the_frequency_report_is_current() {
    let manifest = public_manifest();
    check_generated(
        &corpus_dir().join("public").join("frequency-report.md"),
        &Stats::of(&manifest).to_markdown(REPORT_TITLE, &manifest),
        "the public manifest",
    );
}
