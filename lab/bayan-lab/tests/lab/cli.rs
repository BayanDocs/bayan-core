//! The `bayan-lab corpus` commands, run in-process on corpora in temporary folders, with a local-directory store.

use std::fs;
use std::path::{Path, PathBuf};

use bayan_lab::cli::{Outcome, run};
use bayan_lab::hash::Sha256;
use bayan_lab::manifest::{Document, GroundTruth, Manifest, Provenance, Tier};
use bayan_lab::store::Key;

use crate::support::{Docx, temporary_folder};

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
    let outcome = run(&args, &mut out, &mut err);
    Ran {
        outcome,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
    }
}

fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// A corpus in a temporary folder: a manifest path, a store folder and a source checkout.
struct Corpus {
    folder: PathBuf,
    manifest: PathBuf,
    store: PathBuf,
    source: PathBuf,
}

impl Corpus {
    fn new(name: &str) -> Self {
        let folder = temporary_folder(name);
        let source = folder.join("source");
        fs::create_dir_all(source.join("sub")).unwrap();
        fs::write(
            source.join("LICENSE"),
            "MIT License\n\nCopyright (c) Example\n",
        )
        .unwrap();
        Self {
            manifest: folder.join("manifest.json"),
            store: folder.join("store"),
            folder,
            source,
        }
    }

    /// Runs a corpus command with this corpus's manifest and store, given before any `--`.
    fn run(&self, command: &[&str]) -> Ran {
        let mut args = vec!["corpus"];
        args.extend_from_slice(command);
        let options = [
            "--manifest",
            text(&self.manifest),
            "--store",
            text(&self.store),
        ];
        let at = args
            .iter()
            .position(|&arg| arg == "--")
            .unwrap_or(args.len());
        args.splice(at..at, options);
        lab(&args)
    }

    fn add_source(&self) -> Ran {
        let license = format!("LICENSE={}", text(&self.source.join("LICENSE")));
        self.run(&[
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
            "Copyright (c) Example",
            "--license-file",
            &license,
        ])
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Sha256 {
        fs::write(self.source.join(path), bytes).unwrap();
        Sha256::of(bytes)
    }

    fn manifest(&self) -> Manifest {
        Manifest::from_json(&fs::read_to_string(&self.manifest).unwrap()).unwrap()
    }

    fn object(&self, sha256: Sha256) -> PathBuf {
        Key::document(sha256)
            .path()
            .split('/')
            .fold(self.store.clone(), |path, segment| path.join(segment))
    }
}

fn table_document() -> Vec<u8> {
    Docx::new("<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>")
        .bytes()
}

fn text_document(words: &str) -> Vec<u8> {
    Docx::new(&format!("<w:p><w:r><w:t>{words}</w:t></w:r></w:p>")).bytes()
}

#[test]
fn a_corpus_from_its_source_to_its_verification() {
    let corpus = Corpus::new("lifecycle");
    let added = corpus.add_source();
    assert_eq!(added.outcome, Outcome::Success, "{}", added.err);
    assert_eq!(added.out, "source example: added\n");
    // Registering the same source again changes nothing; changing it is refused: sources never change.
    assert_eq!(corpus.add_source().out, "source example: unchanged\n");
    let license = format!("LICENSE={}", text(&corpus.source.join("LICENSE")));
    let changed = corpus.run(&[
        "source",
        "add",
        "example",
        "--url",
        "https://example.invalid/other",
        "--revision",
        "1",
        "--license",
        "MIT",
        "--copyright",
        "Copyright (c) Example",
        "--license-file",
        &license,
    ]);
    assert_eq!(changed.outcome, Outcome::Error);
    assert!(
        changed.err.contains("sources never change"),
        "{}",
        changed.err
    );

    let table = corpus.write("table.docx", &table_document());
    let words = corpus.write("sub/words.docx", &text_document("Words"));
    let excluded = corpus.write("excluded.docx", &text_document("Excluded"));
    corpus.write("broken.docx", b"not a package");
    let list = corpus.folder.join("excluded.tsv");
    fs::write(
        &list,
        format!("sha256\tsource\tpath\treason\n{excluded}\texample\texcluded.docx\tthird-party content\n"),
    )
    .unwrap();
    let root = text(&corpus.source).to_owned();
    let added = corpus.run(&[
        "add",
        "--source",
        "example",
        "--root",
        &root,
        "--exclude",
        text(&list),
        "--",
        "table.docx",
        "sub/words.docx",
        "excluded.docx",
        "broken.docx",
        "table.docx",
    ]);
    assert_eq!(
        added.outcome,
        Outcome::Failed,
        "one document is refused: {}",
        added.err
    );
    assert!(
        added
            .out
            .ends_with("corpus add: 2 added, 1 already in the corpus, 1 excluded, 1 refused\n"),
        "{}",
        added.out
    );
    assert!(added.out.contains(&format!(
        "excluded {} excluded.docx: third-party content",
        excluded.short()
    )));
    assert!(
        added.err.contains("refused broken.docx (") && added.err.contains("not a ZIP archive"),
        "{}",
        added.err
    );

    let manifest = corpus.manifest();
    assert_eq!(manifest.documents.len(), 2);
    let document = manifest.document(&words).unwrap();
    assert_eq!(document.tier, Tier::T1);
    assert_eq!(document.license, "MIT");
    assert_eq!(
        document.provenance.first().unwrap().path.as_deref(),
        Some("sub/words.docx")
    );
    assert!(document.scan.is_some());
    assert!(corpus.object(table).is_file());

    let verified = corpus.run(&["verify", "--exclude", text(&list)]);
    assert_eq!(
        verified.outcome,
        Outcome::Success,
        "{}{}",
        verified.out,
        verified.err
    );
    assert!(
        verified
            .out
            .contains("manifest: 2 documents from 1 sources, consistent")
    );
    assert!(
        verified
            .out
            .contains("exclusions: 0 of the 1 excluded documents in the manifest")
    );
    assert!(
        verified
            .out
            .contains("2 of 2 documents and 1 license texts present")
    );
    assert!(verified.out.ends_with("corpus verify: OK\n"));

    // Listing, filtered by feature, source and format.
    let listed = corpus.run(&["list", "--feature", "Grid, widths, fixed layout"]);
    assert_eq!(listed.outcome, Outcome::Success);
    assert_eq!(listed.out.lines().count(), 1);
    assert!(listed.out.contains(&table.to_string()));
    assert!(listed.out.trim_end().ends_with("example:table.docx"));
    assert_eq!(
        corpus
            .run(&["list", "--source", "example"])
            .out
            .lines()
            .count(),
        2
    );
    assert_eq!(corpus.run(&["list", "--source", "other"]).out, "");
    let json = corpus.run(&["list", "--format", "json"]).out;
    for line in json.lines() {
        serde_json::from_str::<serde_json::Value>(line).unwrap();
    }
    let unknown = corpus.run(&["list", "--feature", "Teleportation"]);
    assert_eq!(unknown.outcome, Outcome::Error);

    let stats = corpus.run(&["stats", "--title", "Test corpus"]);
    assert!(stats.out.starts_with("# Test corpus\n"));
    assert!(stats.out.contains("| Documents | 2 |"));
    let json = corpus.run(&["stats", "--format", "json"]).out;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json).unwrap()["documents"],
        2
    );

    // Tagging again: nothing is out of date, unless everything is asked for.
    assert!(corpus.run(&["tag"]).out.contains("0 tagged"));
    let retagged = corpus.run(&["tag", "--all"]);
    assert_eq!(retagged.outcome, Outcome::Success);
    assert!(retagged.out.contains("2 tagged"));
    assert_eq!(
        corpus.manifest(),
        manifest,
        "tagging again changed the manifest"
    );
}

#[test]
fn verify_finds_damaged_missing_and_unexpected_objects() {
    let corpus = Corpus::new("damage");
    corpus.add_source();
    let first = corpus.write("first.docx", &text_document("First"));
    let second = corpus.write("second.docx", &text_document("Second"));
    let root = text(&corpus.source).to_owned();
    let added = corpus.run(&[
        "add",
        "--source",
        "example",
        "--root",
        &root,
        "first.docx",
        "second.docx",
    ]);
    assert_eq!(added.outcome, Outcome::Success, "{}", added.err);

    fs::write(corpus.object(first), b"tampered").unwrap();
    fs::remove_file(corpus.object(second)).unwrap();
    fs::write(corpus.store.join("objects").join("stray.docx"), b"stray").unwrap();
    let verified = corpus.run(&["verify"]);
    assert_eq!(verified.outcome, Outcome::Failed);
    assert!(verified.err.contains(&format!(
        "{}: the stored object does not match its SHA-256",
        first.short()
    )));
    assert!(
        verified
            .err
            .contains(&format!("{}: not in the store", second.short()))
    );
    assert!(
        verified
            .err
            .contains("unexpected object in the store: objects/stray.docx")
    );
    assert!(
        verified.out.ends_with("corpus verify: 3 problems\n"),
        "{}",
        verified.out
    );

    // Adding the files again repairs the store: documents already in the manifest are stored again if missing.
    fs::remove_file(corpus.object(first)).unwrap();
    fs::remove_file(corpus.store.join("objects").join("stray.docx")).unwrap();
    let readded = corpus.run(&[
        "add",
        "--source",
        "example",
        "--root",
        &root,
        "first.docx",
        "second.docx",
    ]);
    assert!(readded.out.contains("0 added, 2 already in the corpus"));
    assert_eq!(corpus.run(&["verify"]).outcome, Outcome::Success);

    // A document the exclusion list names fails verification.
    let list = corpus.folder.join("excluded.tsv");
    fs::write(
        &list,
        format!("sha256\tsource\tpath\treason\n{first}\texample\tfirst.docx\tlicense unclear\n"),
    )
    .unwrap();
    let verified = corpus.run(&["verify", "--exclude", text(&list)]);
    assert_eq!(verified.outcome, Outcome::Failed);
    assert!(
        verified
            .err
            .contains("the exclusion list excludes this document")
    );
    // A malformed exclusion list is refused before anything is done.
    fs::write(&list, "sha256,source,path,reason\n").unwrap();
    assert_eq!(
        corpus.run(&["verify", "--exclude", text(&list)]).outcome,
        Outcome::Error
    );
}

#[test]
fn add_never_leaves_the_root() {
    let corpus = Corpus::new("root");
    corpus.add_source();
    let outside = corpus.folder.join("private.docx");
    fs::write(&outside, text_document("Private")).unwrap();
    let root = text(&corpus.source).to_owned();
    for operand in [
        "../private.docx",
        "/etc/passwd",
        "sub/../../private.docx",
        "sub\\x.docx",
    ] {
        let added = corpus.run(&["add", "--source", "example", "--root", &root, operand]);
        assert_eq!(added.outcome, Outcome::Failed, "{operand}");
        assert!(
            added.err.contains("relative paths inside it"),
            "{operand}: {}",
            added.err
        );
    }
    let folder = corpus.run(&["add", "--source", "example", "--root", &root, "sub"]);
    assert!(folder.err.contains("not a regular file"), "{}", folder.err);
    // A symbolic link inside the source that points outside it is not followed. Creating one can fail on Windows without the privilege; then there is nothing to check.
    #[expect(
        deprecated,
        reason = "the one portable way to make a symbolic link; the platform functions need conditions this workspace does not approve"
    )]
    let linked = std::fs::soft_link(&outside, corpus.source.join("link.docx"));
    if linked.is_ok() {
        let added = corpus.run(&["add", "--source", "example", "--root", &root, "link.docx"]);
        assert_eq!(added.outcome, Outcome::Failed);
        assert!(
            added.err.contains("a symbolic link is on its path"),
            "{}",
            added.err
        );
        assert!(!corpus.manifest.exists() || corpus.manifest().documents.is_empty());
    }
}

#[test]
fn private_documents_record_no_names_and_their_messages_name_none() {
    let corpus = Corpus::new("private");
    let registered = corpus.run(&[
        "source",
        "add",
        "private-2026",
        "--url",
        "https://example.invalid/private",
        "--revision",
        "2026-10",
        "--license",
        "NOASSERTION",
        "--copyright",
        "",
    ]);
    assert_eq!(registered.outcome, Outcome::Success, "{}", registered.err);
    let document = corpus.folder.join("Jane Doe - medical report.docx");
    fs::write(&document, text_document("Private")).unwrap();
    let broken = corpus.folder.join("Jane Doe - tax return.docx");
    fs::write(&broken, b"broken").unwrap();
    let added = corpus.run(&[
        "add",
        "--source",
        "private-2026",
        "--tier",
        "T2",
        text(&document),
        text(&broken),
    ]);
    assert_eq!(added.outcome, Outcome::Failed);
    for output in [&added.out, &added.err] {
        assert!(!output.contains("Jane"), "a file name leaked: {output}");
    }
    assert!(added.out.contains("input 1"));
    assert!(added.err.contains("refused input 2"));
    let manifest = corpus.manifest();
    let entry = manifest.documents.first().unwrap();
    assert_eq!(entry.tier, Tier::T2);
    assert_eq!(entry.license, "NOASSERTION");
    assert_eq!(entry.provenance.first().unwrap().path, None);
    assert!(
        !fs::read_to_string(&corpus.manifest)
            .unwrap()
            .contains("Jane")
    );
    // Paths cannot be recorded for private documents, and public tiers need a license.
    let root = text(&corpus.folder).to_owned();
    assert_eq!(
        corpus
            .run(&[
                "add",
                "--source",
                "private-2026",
                "--tier",
                "T2",
                "--root",
                &root,
                "x.docx"
            ])
            .outcome,
        Outcome::Error
    );
    assert_eq!(
        corpus
            .run(&["add", "--source", "private-2026", text(&document)])
            .outcome,
        Outcome::Error
    );
}

#[test]
fn arguments_that_cannot_work() {
    for args in [
        &[][..],
        &["corpus"],
        &["corpus", "teleport"],
        &["corpus", "list"],
        &["corpus", "list", "--manifest"],
        &[
            "corpus",
            "verify",
            "--manifest",
            "a.json",
            "--colour",
            "red",
        ],
        &[
            "corpus",
            "stats",
            "--manifest",
            "/nonexistent/manifest.json",
        ],
    ] {
        let ran = lab(args);
        assert_eq!(ran.outcome, Outcome::Error, "{args:?}");
        assert!(ran.err.starts_with("bayan-lab: "), "{args:?}: {}", ran.err);
    }
    let help = lab(&["--help"]);
    assert_eq!(help.outcome, Outcome::Success);
    assert!(help.out.starts_with("Usage: bayan-lab corpus"));
    let corpus = Corpus::new("arguments");
    assert_eq!(
        corpus
            .run(&["add", "--source", "missing", "x.docx"])
            .outcome,
        Outcome::Error
    );
    corpus.add_source();
    assert_eq!(
        corpus
            .run(&["add", "--source", "example", "--tier", "T9", "x.docx"])
            .outcome,
        Outcome::Error
    );
    assert_eq!(
        corpus.run(&["stats", "--format", "xml"]).outcome,
        Outcome::Error
    );
    assert_eq!(Outcome::Error.code(), 2);
}

#[test]
fn a_document_has_one_tier() {
    let corpus = Corpus::new("tiers");
    corpus.add_source();
    let private = corpus.run(&[
        "source",
        "add",
        "private",
        "--url",
        "https://example.invalid/private",
        "--revision",
        "1",
        "--license",
        "NOASSERTION",
        "--copyright",
        "",
    ]);
    assert_eq!(private.outcome, Outcome::Success, "{}", private.err);
    let first = corpus.write("first.docx", &text_document("First"));
    let second = corpus.write("second.docx", &text_document("Second"));
    let root = text(&corpus.source).to_owned();
    // Private first, then the same bytes from a public source: refused, so the private document records no path.
    let file = text(&corpus.source.join("first.docx")).to_owned();
    assert_eq!(
        corpus
            .run(&["add", "--source", "private", "--tier", "T2", &file])
            .outcome,
        Outcome::Success
    );
    let public = corpus.run(&["add", "--source", "example", "--root", &root, "first.docx"]);
    assert_eq!(public.outcome, Outcome::Failed);
    assert!(
        public
            .err
            .contains("already in the corpus as a T2 document"),
        "{}",
        public.err
    );
    // Public first, then private: refused, so the public document does not gain a private source.
    assert_eq!(
        corpus
            .run(&["add", "--source", "example", "--root", &root, "second.docx"])
            .outcome,
        Outcome::Success
    );
    let file = text(&corpus.source.join("second.docx")).to_owned();
    let private = corpus.run(&["add", "--source", "private", "--tier", "T2", &file]);
    assert_eq!(private.outcome, Outcome::Failed);
    assert!(
        private
            .err
            .contains("already in the corpus as a T1 document"),
        "{}",
        private.err
    );
    let manifest = corpus.manifest();
    manifest.check().unwrap();
    assert_eq!(manifest.document(&first).unwrap().provenance.len(), 1);
    assert_eq!(manifest.document(&second).unwrap().provenance.len(), 1);
}

#[test]
fn a_mistyped_store_address_is_not_a_folder() {
    let corpus = Corpus::new("scheme");
    corpus.add_source();
    corpus.write("one.docx", &text_document("One"));
    let root = text(&corpus.source).to_owned();
    let manifest = text(&corpus.manifest).to_owned();
    for store in [
        "S3://bucket/private?endpoint=https://example.invalid",
        "s3:/bucket",
        "file:///tmp/store",
    ] {
        let ran = lab(&[
            "corpus",
            "add",
            "--manifest",
            &manifest,
            "--store",
            store,
            "--source",
            "example",
            "--root",
            &root,
            "one.docx",
        ]);
        assert_eq!(ran.outcome, Outcome::Error, "{store}");
        assert!(
            ran.err.contains("does not start a store address"),
            "{store}: {}",
            ran.err
        );
    }
    assert!(!std::path::Path::new("S3:").exists() && !std::path::Path::new("s3:").exists());
}

#[test]
fn a_refused_source_leaves_nothing_in_the_store() {
    let corpus = Corpus::new("orphan");
    let license = format!("LICENSE={}", text(&corpus.source.join("LICENSE")));
    let refused = corpus.run(&[
        "source",
        "add",
        "Bad_Name",
        "--url",
        "http://example.invalid/",
        "--revision",
        "1",
        "--license",
        "MIT",
        "--copyright",
        "Copyright (c) Example",
        "--license-file",
        &license,
    ]);
    assert_eq!(refused.outcome, Outcome::Error);
    assert!(
        refused.err.contains("names use lower-case letters"),
        "{}",
        refused.err
    );
    assert!(!corpus.store.join("licenses").exists());
    assert!(!corpus.manifest.exists());
}

#[test]
fn empty_notes_are_refused_before_anything_is_stored() {
    let corpus = Corpus::new("empty-notes");
    let license = format!("LICENSE={}", text(&corpus.source.join("LICENSE")));
    let source = corpus.run(&[
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
        "Copyright (c) Example",
        "--license-file",
        &license,
        "--notes",
        "",
    ]);
    assert_eq!(source.outcome, Outcome::Error);
    assert!(
        source.err.contains("--notes cannot be empty"),
        "{}",
        source.err
    );
    assert!(!corpus.store.exists());
    assert!(!corpus.manifest.exists());

    corpus.add_source();
    let sha256 = corpus.write("one.docx", &text_document("One"));
    let before = fs::read(&corpus.manifest).unwrap();
    let root = text(&corpus.source).to_owned();
    let added = corpus.run(&[
        "add", "--source", "example", "--root", &root, "--notes", " ", "one.docx",
    ]);
    assert_eq!(added.outcome, Outcome::Error);
    assert!(
        added.err.contains("--notes cannot be empty"),
        "{}",
        added.err
    );
    assert!(!corpus.object(sha256).exists());
    assert_eq!(fs::read(&corpus.manifest).unwrap(), before);
}

#[test]
fn names_from_outside_are_printed_harmlessly() {
    let corpus = Corpus::new("names");
    corpus.add_source();
    let root = text(&corpus.source).to_owned();
    corpus.write("one.docx", &text_document("One"));
    assert_eq!(
        corpus
            .run(&["add", "--source", "example", "--root", &root, "one.docx"])
            .outcome,
        Outcome::Success
    );
    // A store file whose name holds terminal escape sequences (some systems refuse such names; then there is nothing to check).
    let hostile = corpus
        .store
        .join("objects")
        .join("x\u{1b}]0;pwned\u{7}\u{202E}y");
    if fs::write(&hostile, b"x").is_ok() {
        let verified = corpus.run(&["verify"]);
        assert!(
            verified
                .err
                .contains("unexpected object in the store: objects/x?]0;pwned??y"),
            "{:?}",
            verified.err
        );
        assert!(!verified.err.contains('\u{1b}') && !verified.err.contains('\u{202E}'));
    }
}

#[test]
fn messages_about_private_documents_name_no_part() {
    let corpus = Corpus::new("parts");
    corpus.run(&[
        "source",
        "add",
        "private",
        "--url",
        "https://example.invalid/private",
        "--revision",
        "1",
        "--license",
        "NOASSERTION",
        "--copyright",
        "",
    ]);
    // A part whose name says something about its author, and which is not well-formed.
    let package = Docx::new("<w:p/>")
        .part("word/Jane-Doe-diagnosis.xml", "styles", "<w:styles")
        .bytes();
    let file = corpus.folder.join("private.docx");
    fs::write(&file, &package).unwrap();
    let added = corpus.run(&["add", "--source", "private", "--tier", "T2", text(&file)]);
    assert_eq!(added.outcome, Outcome::Failed);
    assert!(
        added.err.contains("a part: not well-formed XML"),
        "{}",
        added.err
    );
    assert!(!added.err.to_lowercase().contains("jane"), "{}", added.err);
    // For a public source, where paths are recorded anyway, the part is named.
    corpus.add_source();
    fs::write(corpus.source.join("public.docx"), &package).unwrap();
    let root = text(&corpus.source).to_owned();
    let public = corpus.run(&["add", "--source", "example", "--root", &root, "public.docx"]);
    assert!(
        public.err.contains("part /word/jane-doe-diagnosis.xml"),
        "{}",
        public.err
    );
    // `corpus tag` keeps the rule too, for a private document the tagger cannot read: here one in the manifest untagged, as documents are after the tagging rules change.
    let mut manifest = corpus.manifest();
    let sha256 = Sha256::of(&package);
    manifest
        .insert(Document {
            sha256,
            size: u64::try_from(package.len()).unwrap(),
            tier: Tier::T2,
            license: "NOASSERTION".to_owned(),
            provenance: vec![Provenance {
                source: "private".to_owned(),
                path: None,
            }],
            scan: None,
            page_count: None,
            ground_truth: GroundTruth::default(),
            notes: None,
        })
        .unwrap();
    fs::write(&corpus.manifest, manifest.to_json().unwrap()).unwrap();
    let object = corpus.object(sha256);
    fs::create_dir_all(object.parent().unwrap()).unwrap();
    fs::write(&object, &package).unwrap();
    let tagged = corpus.run(&["tag"]);
    assert_eq!(tagged.outcome, Outcome::Failed);
    assert!(
        tagged
            .err
            .contains("cannot be tagged: a part: not well-formed XML"),
        "{}",
        tagged.err
    );
    assert!(
        !tagged.err.to_lowercase().contains("jane"),
        "{}",
        tagged.err
    );
}
