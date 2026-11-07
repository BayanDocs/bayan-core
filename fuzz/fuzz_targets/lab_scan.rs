//! The Fidelity Lab's scanner (bayan-lab's `scan`, CORE-011) with its default limits, on arbitrary bytes: bayan-opc opens the package, bayan-xml reads the parts the tagger needs, and the tagger's rules tag the document. An input that starts with `PK` is a whole package, scanned as it is. Any other input is one part that the tagger reads, which this target puts into a small package before scanning it: the input's first byte chooses the part ([`PARTS`], by its value modulo their number) and the rest is the part's XML. bayan-opc checks the CRC-32 of every part, so a mutated package rarely gets past the ZIP layer; a part written by bayan-opc's own writer reaches bayan-xml and the tagger whatever mutation made of it, as `docx_hints` reaches its reader. `fuzz/ci.py` writes seeds of both kinds, with the same numbering of the parts.
//!
//! For every input the scanner accepts: scanning the same bytes again gives the same findings; every finding can enter a corpus manifest (each feature is one that scanning can detect, and no script is counted without characters, as `corpus verify` checks); and what the tagger copies from a document stays within the bounds bayan-lab documents, because it reaches manifests and reports: at most 200 font names of at most 64 characters each, at most 100 language tags, and an application name of at most 128 bytes, none of them with a control or invisible character.

#![no_main]

use std::borrow::Cow;

use bayan_lab::scan::tagger::{MAX_FONTS, MAX_LANGUAGES};
use bayan_lab::scan::{Limits, is_hidden_character, scan};
use bayan_opc::{PackageWriter, PartName, Relationship, RelationshipSource, TargetMode};
use libfuzzer_sys::fuzz_target;

/// The main document part's name and media type.
const DOCUMENT: &str = "/word/document.xml";
const DOCUMENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";

/// The relationship types of Transitional WordprocessingML.
const RELATIONSHIPS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/";

/// A main document part with nothing in it, for inputs that are another part.
const EMPTY_DOCUMENT: &[u8] = br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body/></w:document>"#;

/// A part that the tagger reads: its name, its media type, and the type of the relationship that leads to it, from the main document part or, for the application properties, from the package. The first is the main document part itself. `fuzz/ci.py` numbers them the same way (`SCAN_PARTS`).
struct Part {
    name: &'static str,
    content_type: &'static str,
    relationship: &'static str,
}

const PARTS: [Part; 14] = [
    Part {
        name: DOCUMENT,
        content_type: DOCUMENT_TYPE,
        relationship: "officeDocument",
    },
    Part {
        name: "/word/styles.xml",
        content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml",
        relationship: "styles",
    },
    Part {
        name: "/word/numbering.xml",
        content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml",
        relationship: "numbering",
    },
    Part {
        name: "/word/settings.xml",
        content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml",
        relationship: "settings",
    },
    Part {
        name: "/word/fontTable.xml",
        content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.fontTable+xml",
        relationship: "fontTable",
    },
    Part {
        name: "/word/theme/theme1.xml",
        content_type: "application/vnd.openxmlformats-officedocument.theme+xml",
        relationship: "theme",
    },
    Part {
        name: "/word/header1.xml",
        content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml",
        relationship: "header",
    },
    Part {
        name: "/word/footer1.xml",
        content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml",
        relationship: "footer",
    },
    Part {
        name: "/word/footnotes.xml",
        content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml",
        relationship: "footnotes",
    },
    Part {
        name: "/word/endnotes.xml",
        content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml",
        relationship: "endnotes",
    },
    Part {
        name: "/word/comments.xml",
        content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml",
        relationship: "comments",
    },
    Part {
        name: "/word/commentsExtended.xml",
        content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtended+xml",
        relationship: "http://schemas.microsoft.com/office/2011/relationships/commentsExtended",
    },
    Part {
        name: "/word/charts/chart1.xml",
        content_type: "application/vnd.openxmlformats-officedocument.drawingml.chart+xml",
        relationship: "chart",
    },
    Part {
        name: "/docProps/app.xml",
        content_type: "application/vnd.openxmlformats-officedocument.extended-properties+xml",
        relationship: "extended-properties",
    },
];

/// The type URI of a relationship of [`PARTS`]: written in full, or a Transitional WordprocessingML type by its last segment.
fn relationship_type(relationship: &str) -> String {
    if relationship.starts_with("http") {
        relationship.to_owned()
    } else {
        format!("{RELATIONSHIPS}{relationship}")
    }
}

/// The relative reference from `source` (a part name, or `/` for the package) to the part named `target`.
fn reference(source: &str, target: &str) -> String {
    let folder = source.rsplit_once('/').map_or("", |(folder, _)| folder);
    target
        .strip_prefix(folder)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(target)
        .to_owned()
}

/// A package that holds `xml` as the part numbered `choice` (modulo the number of [`PARTS`]), with the main document part and the relationships that lead to it.
fn package_of(choice: u8, xml: &[u8]) -> Vec<u8> {
    let part = &PARTS[usize::from(choice) % PARTS.len()];
    let document = PartName::new(DOCUMENT).expect("a part name");
    let mut writer = PackageWriter::new();
    let main = if part.name == DOCUMENT {
        xml.to_vec()
    } else {
        EMPTY_DOCUMENT.to_vec()
    };
    writer
        .add_part(&document, DOCUMENT_TYPE, main)
        .expect("the main document part");
    writer
        .relationships_mut(&RelationshipSource::Package)
        .expect("the package's relationships")
        .add(Relationship::new(
            "rId1",
            relationship_type("officeDocument"),
            "word/document.xml",
            TargetMode::Internal,
        ))
        .expect("the main document's relationship");
    if part.name != DOCUMENT {
        let name = PartName::new(part.name).expect("a part name");
        writer
            .add_part(&name, part.content_type, xml.to_vec())
            .expect("the part");
        let (source, from) = if part.relationship == "extended-properties" {
            (RelationshipSource::Package, "/")
        } else {
            (RelationshipSource::Part(document), DOCUMENT)
        };
        writer
            .relationships_mut(&source)
            .expect("the source's relationships")
            .add(Relationship::new(
                "rId2",
                relationship_type(part.relationship),
                reference(from, part.name),
                TargetMode::Internal,
            ))
            .expect("the part's relationship");
    }
    writer.finish().expect("the package")
}

fuzz_target!(|data: &[u8]| {
    let bytes: Cow<'_, [u8]> = match data.split_first() {
        Some((&choice, xml)) if !data.starts_with(b"PK") => Cow::Owned(package_of(choice, xml)),
        _ => Cow::Borrowed(data),
    };
    let Ok(findings) = scan(&bytes, &Limits::DEFAULT) else {
        return;
    };
    assert_eq!(
        scan(&bytes, &Limits::DEFAULT).as_ref(),
        Ok(&findings),
        "the same bytes give the same findings"
    );
    for feature in &findings.features {
        assert_eq!(feature.not_detectable(), None, "{}", feature.name());
    }
    assert!(findings.scripts.values().all(|&count| count > 0));
    let clean = |text: &str| !text.chars().any(is_hidden_character);
    assert!(findings.fonts.len() <= MAX_FONTS);
    assert!(
        findings
            .fonts
            .iter()
            .all(|font| font.chars().count() <= 64 && clean(font))
    );
    assert!(findings.languages.len() <= MAX_LANGUAGES);
    assert!(findings.languages.iter().all(|language| clean(language)));
    assert!(
        findings
            .application
            .as_deref()
            .is_none_or(|application| application.len() <= 128 && clean(application))
    );
});
