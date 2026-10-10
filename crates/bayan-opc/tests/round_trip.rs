//! Reading and writing real `.docx` files: an unmodified package written back keeps every part byte-identical, regenerated metadata keeps its exact content, and an edit changes only what it touches (work package CORE-005, acceptance criterion 1).
//!
//! The documents are python-docx's test files (see `fixtures/python-docx/README.md`); they are compiled into the tests, so the tests read no files and also run in WebAssembly.
//!
//! A failure message names the fixture and the position of a part or relationship, never a part name or anything else taken from a document, which nothing may print (CodeQL's code scanning, which treats part names as sensitive, holds the tests to this too).

#![cfg(test)]

use bayan_opc::content_types::ZIP_NAME;
use bayan_opc::{
    ContentTypes, CoreProperties, Limits, Package, PackageWriter, PartName, Relationship,
    RelationshipSource, Relationships, Target, TargetMode,
};

const FIXTURES: [(&str, &[u8]); 5] = [
    (
        "doc-word-default-blank.docx",
        include_bytes!("fixtures/python-docx/doc-word-default-blank.docx"),
    ),
    (
        "par-hyperlinks.docx",
        include_bytes!("fixtures/python-docx/par-hyperlinks.docx"),
    ),
    (
        "doc-coreprops.docx",
        include_bytes!("fixtures/python-docx/doc-coreprops.docx"),
    ),
    (
        "hdr-header-footer.docx",
        include_bytes!("fixtures/python-docx/hdr-header-footer.docx"),
    ),
    (
        "default.docx",
        include_bytes!("fixtures/python-docx/default.docx"),
    ),
];

fn open(bytes: &[u8]) -> Package<'_> {
    Package::open(bytes, &Limits::default()).unwrap()
}

fn names(package: &Package<'_>) -> Vec<String> {
    package
        .parts()
        .map(|part| part.as_str().to_owned())
        .collect()
}

/// Every relationship source of the package: the package and every part that has a relationships part.
fn sources(package: &Package<'_>) -> Vec<RelationshipSource> {
    let mut sources = vec![RelationshipSource::Package];
    sources.extend(
        package
            .parts()
            .map(|part| RelationshipSource::Part(part.clone()))
            .filter(|source| package.relationships(source).is_some()),
    );
    sources
}

#[test]
fn opens_and_reads_every_part_of_real_documents() {
    for (fixture, bytes) in FIXTURES {
        let package = open(bytes);
        assert!(
            names(&package).contains(&"/word/document.xml".to_owned()),
            "{fixture}"
        );
        for (position, part) in package.parts().enumerate() {
            package.read_part(part).unwrap();
            if !part.is_relationships_part() {
                assert!(
                    package.content_type(part).is_some(),
                    "{fixture}: part {position}"
                );
            }
        }
        // Every internal relationship of these documents resolves to a part they contain.
        for (position, source) in sources(&package).into_iter().enumerate() {
            for (index, relationship) in package.relationships(&source).unwrap().iter().enumerate()
            {
                match relationship.resolve(&source).unwrap() {
                    Target::Part(target) => assert!(
                        package.contains(&target),
                        "{fixture}: relationship {index} of source {position}"
                    ),
                    Target::External(_) => {
                        assert_eq!(relationship.target_mode(), TargetMode::External)
                    }
                }
            }
        }
    }
}

#[test]
fn records_external_hyperlinks_as_written() {
    let package = open(FIXTURES[1].1);
    let document = RelationshipSource::Part(PartName::new("/word/document.xml").unwrap());
    let external: Vec<&Relationship> = package
        .relationships(&document)
        .unwrap()
        .iter()
        .filter(|relationship| relationship.target_mode() == TargetMode::External)
        .collect();
    assert!(!external.is_empty());
    for relationship in external {
        assert_eq!(
            relationship.resolve(&document).unwrap(),
            Target::External(relationship.target().to_owned())
        );
    }
}

#[test]
fn reads_core_properties() {
    let properties = open(FIXTURES[2].1).core_properties().unwrap().unwrap();
    assert_eq!(properties.title.as_deref(), Some("Title"));
    assert_eq!(properties.keywords.as_deref(), Some("key; word; keyword"));
    let template = open(FIXTURES[4].1).core_properties().unwrap().unwrap();
    assert_eq!(template.creator.as_deref(), Some("python-docx"));
}

/// Acceptance criterion 1: an unmodified package written back keeps the stored bytes of every part, and of the content types stream, exactly.
#[test]
fn writes_an_unmodified_package_back_with_every_part_byte_identical() {
    for (fixture, bytes) in FIXTURES {
        let package = open(bytes);
        let written = PackageWriter::from_package(&package).finish().unwrap();
        let reopened = open(&written);
        assert!(
            names(&package) == names(&reopened),
            "{fixture}: the copy has other parts"
        );
        for (position, part) in package.parts().enumerate() {
            let before = package.entry_of(part).unwrap();
            let after = reopened.entry_of(part).unwrap();
            let (before_entry, after_entry) = (
                package.zip().entry(before).unwrap(),
                reopened.zip().entry(after).unwrap(),
            );
            assert_eq!(
                package.zip().raw_data(before).unwrap(),
                reopened.zip().raw_data(after).unwrap(),
                "{fixture}: part {position}"
            );
            assert_eq!(before_entry.method(), after_entry.method());
            assert_eq!(before_entry.crc32(), after_entry.crc32());
        }
        let content_types = |package: &Package<'_>| {
            let index = package
                .zip()
                .entries()
                .iter()
                .position(|entry| entry.name() == ZIP_NAME)
                .unwrap();
            package.zip().raw_data(index).unwrap().to_vec()
        };
        assert_eq!(
            content_types(&package),
            content_types(&reopened),
            "{fixture}"
        );
        // Writing the copy again gives the same bytes: the writer's own layout is stable.
        assert_eq!(
            PackageWriter::from_package(&reopened).finish().unwrap(),
            written,
            "{fixture}"
        );
    }
}

/// Acceptance criterion 1, second half: metadata parts that are regenerated keep exactly the content they had.
#[test]
fn regenerated_metadata_parts_keep_their_exact_content() {
    for (fixture, bytes) in FIXTURES {
        let package = open(bytes);
        let original_types = read_entry(&package, ZIP_NAME);
        assert_eq!(
            ContentTypes::parse(&original_types, &Limits::default())
                .unwrap()
                .to_xml(),
            original_types,
            "{fixture}"
        );
        for (position, source) in sources(&package).into_iter().enumerate() {
            let original = package.read_part(&source.relationships_part()).unwrap();
            assert_eq!(
                Relationships::parse(&original, &Limits::default())
                    .unwrap()
                    .to_xml(),
                original,
                "{fixture}: relationships part {position}"
            );
        }

        // Touch every metadata part and undo the change, so that the writer regenerates them all.
        let mut writer = PackageWriter::from_package(&package);
        let types = writer.content_types_mut();
        types
            .set_default("bayan", "application/x-bayan-test")
            .unwrap();
        assert!(types.remove_default("bayan"));
        assert!(types.is_modified());
        for source in sources(&package) {
            let relationships = writer.relationships_mut(&source).unwrap();
            let id = relationships.next_id();
            relationships
                .add(Relationship::new(
                    id.clone(),
                    "urn:test",
                    "x.xml",
                    TargetMode::Internal,
                ))
                .unwrap();
            relationships.remove(&id).unwrap();
            assert!(relationships.is_modified());
        }
        let written = writer.finish().unwrap();
        let reopened = open(&written);
        assert_eq!(read_entry(&reopened, ZIP_NAME), original_types, "{fixture}");
        for (position, source) in sources(&package).into_iter().enumerate() {
            let name = source.relationships_part();
            assert_eq!(
                reopened.read_part(&name).unwrap(),
                package.read_part(&name).unwrap(),
                "{fixture}: relationships part {position}"
            );
        }
        // Every other part is still copied byte for byte.
        for (position, part) in package.parts().enumerate() {
            if part.is_relationships_part() {
                continue;
            }
            assert_eq!(
                package
                    .zip()
                    .raw_data(package.entry_of(part).unwrap())
                    .unwrap(),
                reopened
                    .zip()
                    .raw_data(reopened.entry_of(part).unwrap())
                    .unwrap(),
                "{fixture}: part {position}"
            );
        }
    }
}

#[test]
fn an_edit_changes_only_what_it_touches() {
    let package = open(FIXTURES[0].1);
    let document = PartName::new("/word/document.xml").unwrap();
    let mut writer = PackageWriter::from_package(&package);
    writer
        .replace_part(&document, b"<w:document/>".to_vec())
        .unwrap();
    let image = PartName::new("/word/media/image1.png").unwrap();
    writer
        .add_part(&image, "image/png", vec![0x89, b'P', b'N', b'G'])
        .unwrap();
    let source = RelationshipSource::Part(document.clone());
    let relationships = writer.relationships_mut(&source).unwrap();
    let id = relationships.next_id();
    relationships
        .add(Relationship::new(
            id.clone(),
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image",
            "media/image1.png",
            TargetMode::Internal,
        ))
        .unwrap();
    let written = writer.finish().unwrap();
    // The same edits always give the same bytes.
    let mut again = PackageWriter::from_package(&package);
    again
        .replace_part(&document, b"<w:document/>".to_vec())
        .unwrap();
    again
        .add_part(&image, "image/png", vec![0x89, b'P', b'N', b'G'])
        .unwrap();
    again
        .relationships_mut(&source)
        .unwrap()
        .add(Relationship::new(
            id.clone(),
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image",
            "media/image1.png",
            TargetMode::Internal,
        ))
        .unwrap();
    assert_eq!(again.finish().unwrap(), written);

    let reopened = open(&written);
    assert_eq!(reopened.read_part(&document).unwrap(), b"<w:document/>");
    assert_eq!(
        reopened.read_part(&image).unwrap(),
        [0x89, b'P', b'N', b'G']
    );
    assert_eq!(reopened.content_type(&image), Some("image/png"));
    let added = reopened.relationships(&source).unwrap().get(&id).unwrap();
    assert_eq!(added.resolve(&source).unwrap(), Target::Part(image.clone()));
    for (position, part) in package.parts().enumerate() {
        if *part == document || *part == source.relationships_part() {
            continue;
        }
        assert_eq!(
            package
                .zip()
                .raw_data(package.entry_of(part).unwrap())
                .unwrap(),
            reopened
                .zip()
                .raw_data(reopened.entry_of(part).unwrap())
                .unwrap(),
            "part {position}"
        );
    }
    // The new part comes last, after every original part.
    assert_eq!(reopened.parts().last(), Some(&image));
}

#[test]
fn removes_a_part_with_its_relationships_and_override() {
    let package = open(FIXTURES[4].1);
    let item = PartName::new("/customXml/item1.xml").unwrap();
    let source = RelationshipSource::Part(item.clone());
    assert!(package.relationships(&source).is_some());
    let mut writer = PackageWriter::from_package(&package);
    writer.remove_part(&item).unwrap();
    let written = writer.finish().unwrap();
    let reopened = open(&written);
    assert!(!reopened.contains(&item));
    assert!(!reopened.contains(&source.relationships_part()));
    assert!(reopened.relationships(&source).is_none());
}

#[test]
fn removes_the_override_of_a_removed_relationships_part() {
    // A relationships part can have an override of its own instead of the default for `rels`; removing its part must remove that override too (the review of pull request 17).
    let part = PartName::new("/a.xml").unwrap();
    let source = RelationshipSource::Part(part.clone());
    let relationships_part = source.relationships_part();
    let mut writer = PackageWriter::new();
    writer
        .add_part(&part, "application/xml", b"<a/>".to_vec())
        .unwrap();
    writer
        .relationships_mut(&source)
        .unwrap()
        .add(Relationship::new(
            "rId1",
            "urn:test",
            "https://example.com/",
            TargetMode::External,
        ))
        .unwrap();
    writer
        .content_types_mut()
        .set_override(&relationships_part, bayan_opc::relationships::CONTENT_TYPE)
        .unwrap();
    let bytes = writer.finish().unwrap();
    let package = open(&bytes);
    let overrides = |package: &Package<'_>| -> Vec<String> {
        package
            .content_types()
            .overrides()
            .map(|(name, _)| name.as_str().to_owned())
            .collect()
    };
    assert_eq!(overrides(&package), ["/_rels/a.xml.rels"]);
    let mut writer = PackageWriter::from_package(&package);
    writer.remove_part(&part).unwrap();
    let written = writer.finish().unwrap();
    let reopened = open(&written);
    assert!(!reopened.contains(&relationships_part));
    assert!(overrides(&reopened).is_empty());
}

#[test]
fn refuses_relationships_parts_with_derivable_names() {
    use bayan_opc::PackageError;
    // A relationships part is a part too, so its name may not be derivable from another part's name, or the other way round, or the package would not open again (the review of pull request 17).
    let refused = |result: Result<&mut bayan_opc::Relationships, bayan_opc::Error>| {
        assert!(
            matches!(
                result,
                Err(bayan_opc::Error::Package(PackageError::DerivableName))
            ),
            "{result:?}"
        );
    };
    let mut writer = PackageWriter::new();
    writer
        .add_part(
            &PartName::new("/_rels").unwrap(),
            "application/xml",
            Vec::new(),
        )
        .unwrap();
    // `/_rels/.rels` would be derivable from `/_rels`.
    refused(writer.relationships_mut(&RelationshipSource::Package));
    let part = PartName::new("/w/a.xml").unwrap();
    writer
        .add_part(&part, "application/xml", Vec::new())
        .unwrap();
    writer
        .add_part(
            &PartName::new("/w/_rels/a.xml.rels/b.xml").unwrap(),
            "application/xml",
            Vec::new(),
        )
        .unwrap();
    // `/w/_rels/a.xml.rels/b.xml` would be derivable from `/w/_rels/a.xml.rels`.
    refused(writer.relationships_mut(&RelationshipSource::Part(part)));
    // The refusals left nothing behind: the package opens.
    let written = writer.finish().unwrap();
    let package = open(&written);
    assert_eq!(
        names(&package),
        ["/_rels", "/w/a.xml", "/w/_rels/a.xml.rels/b.xml"]
    );
}

#[test]
fn writes_new_core_properties() {
    let mut writer = PackageWriter::new();
    let core = PartName::new("/docProps/core.xml").unwrap();
    let properties = CoreProperties {
        title: Some("Bayan".to_owned()),
        created: Some("2026-10-09T00:00:00Z".to_owned()),
        ..CoreProperties::default()
    };
    writer
        .add_part(
            &core,
            bayan_opc::core_properties::CONTENT_TYPE,
            properties.to_xml().unwrap(),
        )
        .unwrap();
    writer
        .relationships_mut(&RelationshipSource::Package)
        .unwrap()
        .add(Relationship::new(
            "rId1",
            bayan_opc::core_properties::RELATIONSHIP_TYPE,
            "docProps/core.xml",
            TargetMode::Internal,
        ))
        .unwrap();
    let bytes = writer.finish().unwrap();
    let package = open(&bytes);
    assert_eq!(package.core_properties().unwrap(), Some(properties));
    // The writer gave the relationships parts their media type.
    assert_eq!(
        package.content_type(&RelationshipSource::Package.relationships_part()),
        Some(bayan_opc::relationships::CONTENT_TYPE)
    );
}

#[test]
fn reads_back_every_part_name_it_writes() {
    // A part name may hold a non-ASCII character that IRIs do not allow as itself (U+0080 here, which is not a `ucschar`) only percent-encoded; reading must leave that encoding alone, or the part would vanish (the review of pull request 17). `%C3%A9`, by contrast, is how a ZIP entry name writes `é`.
    let odd = PartName::new("/a%C2%80.bin").unwrap();
    let accented = PartName::new("/média/é.bin").unwrap();
    let mut writer = PackageWriter::new();
    for name in [&odd, &accented] {
        writer
            .add_part(name, "application/octet-stream", b"x".to_vec())
            .unwrap();
        // An override, as a part without a default for its extension gets one.
        writer
            .content_types_mut()
            .set_override(name, "application/x-test")
            .unwrap();
    }
    let relationships = writer
        .relationships_mut(&RelationshipSource::Package)
        .unwrap();
    relationships
        .add(Relationship::new(
            "rId1",
            "urn:test",
            "a%C2%80.bin",
            TargetMode::Internal,
        ))
        .unwrap();
    relationships
        .add(Relationship::new(
            "rId2",
            "urn:test",
            "m%C3%A9dia/%C3%A9.bin",
            TargetMode::Internal,
        ))
        .unwrap();
    let bytes = writer.finish().unwrap();
    let package = open(&bytes);
    assert_eq!(names(&package)[..2], ["/a%C2%80.bin", "/média/é.bin"]);
    for name in [&odd, &accented] {
        assert_eq!(package.read_part(name).unwrap(), b"x");
        assert_eq!(package.content_type(name), Some("application/x-test"));
    }
    let relationships = package.relationships(&RelationshipSource::Package).unwrap();
    let target = |id| {
        relationships
            .get(id)
            .unwrap()
            .resolve(&RelationshipSource::Package)
            .unwrap()
    };
    assert_eq!(target("rId1"), Target::Part(odd.clone()));
    assert_eq!(target("rId2"), Target::Part(accented.clone()));
}

#[test]
fn refuses_edits_that_would_break_the_rules() {
    let package = open(FIXTURES[0].1);
    let mut writer = PackageWriter::from_package(&package);
    let document = PartName::new("/word/document.xml").unwrap();
    let error = |result: Result<(), bayan_opc::Error>| match result {
        Err(bayan_opc::Error::Package(error)) => error,
        other => panic!("{other:?}"),
    };
    use bayan_opc::PackageError;
    assert_eq!(
        error(writer.add_part(&document, "application/xml", Vec::new())),
        PackageError::PartExists
    );
    assert_eq!(
        error(writer.add_part(
            &PartName::new("/WORD/DOCUMENT.XML").unwrap(),
            "application/xml",
            Vec::new()
        )),
        PackageError::PartExists
    );
    assert_eq!(
        error(writer.add_part(
            &PartName::new("/word/document.xml/x").unwrap(),
            "application/xml",
            Vec::new()
        )),
        PackageError::DerivableName
    );
    assert_eq!(
        error(writer.add_part(
            &PartName::new("/word").unwrap(),
            "application/xml",
            Vec::new()
        )),
        PackageError::DerivableName
    );
    assert_eq!(
        error(writer.add_part(
            &PartName::new("/x/_rels/y.rels").unwrap(),
            "application/xml",
            Vec::new()
        )),
        PackageError::RelationshipsPartName
    );
    assert_eq!(
        error(writer.add_part(
            &PartName::new("/new.xml").unwrap(),
            "not a type",
            Vec::new()
        )),
        PackageError::InvalidContentType
    );
    assert_eq!(
        error(writer.replace_part(&PartName::new("/missing.xml").unwrap(), Vec::new())),
        PackageError::PartNotFound
    );
    assert_eq!(
        error(writer.replace_part(&PartName::new("/_rels/.rels").unwrap(), Vec::new())),
        PackageError::RelationshipsPartName
    );
    assert_eq!(
        error(writer.remove_part(&PartName::new("/_rels/.rels").unwrap())),
        PackageError::RelationshipsPartName
    );
    assert!(matches!(
        writer.relationships_mut(&RelationshipSource::Part(
            PartName::new("/missing.xml").unwrap()
        )),
        Err(bayan_opc::Error::Package(PackageError::PartNotFound))
    ));
}

/// The uncompressed content of the ZIP entry named `name`.
fn read_entry(package: &Package<'_>, name: &str) -> Vec<u8> {
    let index = package
        .zip()
        .entries()
        .iter()
        .position(|entry| entry.name() == name)
        .unwrap();
    package.zip().read(index).unwrap()
}
