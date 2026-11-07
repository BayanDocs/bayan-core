//! Crafted malformed and malicious packages (LAB-001 AC-3): each must be refused with the error that names its flaw, quickly and within the scanner's limits, and nothing may make the scanner panic.
//!
//! The packages are built in memory by `support`, field by field, so every flaw is deliberate and visible in the test. The limits are the ones the tools use ([`Limits::DEFAULT`]) unless a test says otherwise. Since CORE-011 the scanner opens packages with bayan-opc and reads their XML with bayan-xml, the engine's own readers, so each flaw is refused with the error that BayanDocs gives it.

use bayan_lab::scan::features::Feature;
use bayan_lab::scan::{Limits, ScanError, scan};
use bayan_opc::{Error, LimitError, XmlErrorKind, ZipError};
use bayan_xml::ErrorKind;
use proptest::prelude::*;

use crate::support::{
    DOCUMENT_TYPE, Docx, ZipEntry, ZipLayout, document_xml, local_header, rel, zip, zip_with,
};

fn valid() -> Docx {
    Docx::new("<w:p><w:r><w:t>Text</w:t></w:r></w:p>")
}

fn refusal(bytes: &[u8]) -> ScanError {
    match scan(bytes, &Limits::DEFAULT) {
        Ok(findings) => panic!("accepted: {findings:?}"),
        Err(error) => error,
    }
}

/// How bayan-opc refuses the package: its ZIP container, its part names, its metadata, or the data of a part.
fn package_refusal(bytes: &[u8]) -> Error {
    match refusal(bytes) {
        ScanError::Package(error) => error,
        other => panic!("refused, but not by bayan-opc: {other}"),
    }
}

/// How bayan-opc's ZIP reader refuses the package.
fn zip_refusal(bytes: &[u8]) -> ZipError {
    match package_refusal(bytes) {
        Error::Zip(error) => error,
        other => panic!("refused, but not by the ZIP reader: {other}"),
    }
}

/// The valid package's entries with the entry named `name` changed by `change`.
fn with_entry(name: &str, change: impl FnOnce(&mut ZipEntry)) -> Vec<u8> {
    let mut entries = valid().entries();
    let entry = entries
        .iter_mut()
        .find(|entry| entry.name == name.as_bytes())
        .unwrap();
    change(entry);
    zip(&entries)
}

/// The valid package with one more entry.
fn with_extra_entry(entry: ZipEntry) -> Vec<u8> {
    let mut entries = valid().entries();
    entries.push(entry);
    zip(&entries)
}

/// The valid package with its main document part replaced by `xml`.
fn with_document(xml: &[u8]) -> Vec<u8> {
    Docx::with_main(DOCUMENT_TYPE, xml).bytes()
}

/// How bayan-xml refuses the main document part.
fn xml_refusal(xml: &[u8]) -> ErrorKind {
    match refusal(&with_document(xml)) {
        ScanError::Part { part, error } => {
            assert_eq!(part, "/word/document.xml");
            error.kind()
        }
        other => panic!("refused, but not as XML: {other}"),
    }
}

#[test]
fn the_valid_package_is_accepted() {
    scan(&valid().bytes(), &Limits::DEFAULT).unwrap();
}

// The ZIP layer.

#[test]
fn files_that_are_not_zip_archives() {
    for bytes in [&b""[..], b"PK", b"not a zip archive at all", &[0; 64]] {
        assert_eq!(zip_refusal(bytes), ZipError::NotZip);
    }
    let mut ole = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    ole.resize(4096, 0);
    assert_eq!(refusal(&ole), ScanError::OleCompoundFile);
}

#[test]
fn data_after_the_end_record_or_a_comment_of_the_wrong_length() {
    // A second, fake end record hidden after the real one cannot redirect the reader: the record must end the file.
    let trailing = zip_with(
        &valid().entries(),
        &ZipLayout {
            trailing: b"appended".to_vec(),
            ..ZipLayout::default()
        },
    );
    assert_eq!(zip_refusal(&trailing), ZipError::NotZip);
    let mut short_comment = zip_with(
        &valid().entries(),
        &ZipLayout {
            comment: b"comment".to_vec(),
            ..ZipLayout::default()
        },
    );
    short_comment.pop();
    assert_eq!(zip_refusal(&short_comment), ZipError::NotZip);
    let commented = zip_with(
        &valid().entries(),
        &ZipLayout {
            comment: b"a comment is fine".to_vec(),
            ..ZipLayout::default()
        },
    );
    scan(&commented, &Limits::DEFAULT).unwrap();
}

#[test]
fn archives_spanning_several_disks() {
    for layout in [
        ZipLayout {
            disk: 1,
            ..ZipLayout::default()
        },
        ZipLayout {
            directory_disk: 1,
            ..ZipLayout::default()
        },
    ] {
        assert_eq!(
            zip_refusal(&zip_with(&valid().entries(), &layout)),
            ZipError::MultipleDisks
        );
    }
    let bytes = with_entry("word/document.xml", |entry| entry.disk = 2);
    assert_eq!(zip_refusal(&bytes), ZipError::MultipleDisks);
}

#[test]
fn a_central_directory_that_lies_about_itself() {
    let entries = valid().entries();
    let real = zip(&entries);
    for (layout, expected) in [
        (
            ZipLayout {
                entries: Some(200),
                ..ZipLayout::default()
            },
            Error::Zip(ZipError::MalformedDirectory),
        ),
        (
            ZipLayout {
                directory_offset: Some(u32::try_from(real.len()).unwrap()),
                ..ZipLayout::default()
            },
            Error::Zip(ZipError::MalformedDirectory),
        ),
        (
            ZipLayout {
                directory_offset: Some(u32::MAX - 4),
                ..ZipLayout::default()
            },
            Error::Zip(ZipError::MalformedDirectory),
        ),
        (
            ZipLayout {
                directory_size: Some(1000),
                ..ZipLayout::default()
            },
            Error::Zip(ZipError::MalformedDirectory),
        ),
        (
            ZipLayout {
                entries: Some(10_001),
                ..ZipLayout::default()
            },
            Error::Limit(LimitError::TooManyEntries { limit: 10_000 }),
        ),
    ] {
        assert_eq!(package_refusal(&zip_with(&entries, &layout)), expected);
    }
}

#[test]
fn entry_names_that_climb_out_or_are_not_part_names() {
    for name in [
        "/word/document.xml",
        "../document.xml",
        "word/../../document.xml",
        "./word/x.xml",
        "word//x.xml",
        "C:x.xml",
        "word\\x.xml",
        "word/x\u{1}.xml",
    ] {
        let bytes = with_extra_entry(ZipEntry::stored(name, b"x"));
        assert!(
            matches!(zip_refusal(&bytes), ZipError::UnsafeName { .. }),
            "{name:?}"
        );
    }
    let mut invalid_utf8 = ZipEntry::stored("word/x.xml", b"x");
    invalid_utf8.name = b"word/\xFF.xml".to_vec();
    invalid_utf8.local_name = Some(invalid_utf8.name.clone());
    assert!(matches!(
        zip_refusal(&with_extra_entry(invalid_utf8)),
        ZipError::UnsafeName { .. }
    ));
    let long = with_extra_entry(ZipEntry::stored(&"a".repeat(1025), b"x"));
    assert!(matches!(
        package_refusal(&long),
        Error::Limit(LimitError::NameTooLong { limit: 1_024, .. })
    ));
}

#[test]
fn two_names_for_one_part() {
    // Names that differ only in case, or only in the percent-encoding of a character beyond ASCII, name the same part: which one would Word read? bayan-opc refuses both.
    let twins = |first: &str, second: &str| {
        let mut entries = valid().entries();
        let mut copy = entries
            .iter()
            .find(|entry| entry.name == b"word/document.xml")
            .unwrap()
            .clone();
        // Names beyond ASCII are UTF-8, and say so with the flag (bit 11) that ZIP has for it.
        copy.flags |= 0x0800;
        for name in [first, second] {
            copy.name = name.as_bytes().to_vec();
            copy.local_name = None;
            entries.push(copy.clone());
        }
        zip(&entries)
    };
    assert!(matches!(
        zip_refusal(&twins("WORD/DOCUMENT.XML", "word/other.xml")),
        ZipError::DuplicateName { .. }
    ));
    assert!(matches!(
        zip_refusal(&twins("word/média.xml", "word/m%C3%A9dia.xml")),
        ZipError::DuplicateName { .. }
    ));
    // A letter written percent-encoded, `%64` for `d`, makes a name that is no part name (ECMA-376 Part 2 §6.2.2.2 forbids encoding a character that can be written as itself): bayan-opc ignores such an entry, so `word/%64ocument.xml` is never read as the main document part, and the package is tagged from `word/document.xml` alone.
    let mut entries = valid().entries();
    let mut copy = entries
        .iter()
        .find(|entry| entry.name == b"word/document.xml")
        .unwrap()
        .clone();
    copy.name = b"word/%64ocument.xml".to_vec();
    copy.local_name = None;
    entries.push(copy);
    scan(&zip(&entries), &Limits::DEFAULT).unwrap();
}

#[test]
fn encrypted_entries_links_and_unknown_compression() {
    type Change = fn(&mut ZipEntry);
    // The main document part is the valid package's second entry.
    let entry = 1;
    let cases: [(Change, ZipError); 5] = [
        (|entry| entry.flags = 1, ZipError::Encrypted { entry }),
        (|entry| entry.flags = 0x40, ZipError::Encrypted { entry }),
        (
            |entry| {
                entry.method = 99;
                entry.local_method = Some(99);
            },
            ZipError::Encrypted { entry },
        ),
        (
            |entry| {
                entry.version_made_by = 3 << 8;
                entry.external_attributes = 0o120_777 << 16;
            },
            ZipError::SpecialFile { entry },
        ),
        (
            |entry| {
                entry.method = 12;
                entry.local_method = Some(12);
            },
            ZipError::UnsupportedCompression { entry, method: 12 },
        ),
    ];
    for (change, expected) in cases {
        let bytes = with_entry("word/document.xml", change);
        assert_eq!(zip_refusal(&bytes), expected);
    }
    // A regular file made on Unix is fine.
    let regular = with_entry("word/document.xml", |entry| {
        entry.version_made_by = 3 << 8;
        entry.external_attributes = 0o100_644 << 16;
    });
    scan(&regular, &Limits::DEFAULT).unwrap();
}

#[test]
fn sizes_above_the_limits_are_refused_before_anything_is_decompressed() {
    let huge = with_entry("word/document.xml", |entry| {
        entry.uncompressed_size = 256 * 1024 * 1024 + 1;
    });
    assert_eq!(
        package_refusal(&huge),
        Error::Limit(LimitError::EntryTooLarge {
            entry: 1,
            limit: 256 * 1024 * 1024
        })
    );
    // Five entries of 250 MiB each declare more than the 1 GiB total; each claims to come from one byte, so the first is refused as a compression bomb even before the total is added up.
    let mut entries = valid().entries();
    for number in 0..5 {
        let mut entry = ZipEntry::deflated(&format!("word/media/big{number}.bin"), b"x");
        entry.uncompressed_size = 250 * 1024 * 1024;
        entries.push(entry);
    }
    assert!(matches!(
        package_refusal(&zip(&entries)),
        Error::Limit(LimitError::CompressionRatio {
            entry: Some(_),
            limit: 100
        })
    ));
    // Entries whose sizes are honest still may not add up to more than the total, here a total of 100 KiB.
    let mut entries = valid().entries();
    for number in 0..3 {
        entries.push(ZipEntry::stored(
            &format!("word/media/big{number}.bin"),
            &vec![b'x'; 40 * 1024],
        ));
    }
    let total = Limits {
        package: bayan_opc::Limits {
            max_total_size: 100 * 1024,
            ..Limits::DEFAULT.package
        },
        ..Limits::DEFAULT
    };
    assert_eq!(
        scan(&zip(&entries), &total).unwrap_err(),
        ScanError::Package(Error::Limit(LimitError::TotalTooLarge {
            limit: 100 * 1024
        }))
    );
    let small = Limits {
        max_package_size: 100,
        ..Limits::DEFAULT
    };
    assert_eq!(
        scan(&valid().bytes(), &small).unwrap_err(),
        ScanError::TooLarge
    );
}

#[test]
fn a_compression_bomb_stops_at_its_declared_size() {
    // 4 MiB of zeros compress to a few kilobytes; the entry claims 1 KiB.
    let zeros = vec![0_u8; 4 * 1024 * 1024];
    let bytes = with_entry("word/document.xml", |entry| {
        *entry = ZipEntry::deflated("word/document.xml", &zeros);
        entry.uncompressed_size = 1024;
    });
    assert_eq!(zip_refusal(&bytes), ZipError::SizeMismatch { entry: 1 });
}

#[test]
fn a_truncated_stream_stops_at_its_declared_size() {
    // Inflating stops as soon as an entry produces more than it declares. A truncated stream tells this apart from the size check after inflating: inflated to its end, it would fail as a broken stream instead.
    let mut state = 1_u32;
    let noise: Vec<u8> = (0..20_000)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            u8::try_from(state >> 24).unwrap()
        })
        .collect();
    let bytes = with_entry("word/document.xml", |entry| {
        *entry = ZipEntry::deflated("word/document.xml", &noise);
        entry.data.truncate(entry.data.len() / 2);
        entry.compressed_size = u32::try_from(entry.data.len()).unwrap();
        entry.uncompressed_size = 100;
    });
    assert_eq!(zip_refusal(&bytes), ZipError::SizeMismatch { entry: 1 });
}

#[test]
fn entry_data_that_runs_into_the_central_directory() {
    // The last entry's data seems to go on into the central directory, which no other entry's data would overlap.
    let mut entries = valid().entries();
    let last = entries.last_mut().unwrap();
    last.compressed_size += 40;
    assert!(matches!(
        zip_refusal(&zip(&entries)),
        ZipError::Overlap { .. }
    ));
}

#[test]
fn a_field_instruction_is_read_only_up_to_its_limit() {
    // Only the first 512 bytes of an instruction are kept: its first word says what the field is, and a crafted instruction of any length costs no more.
    let field = |instruction: &str| {
        scan(
            &Docx::new(&format!(
                r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">{instruction}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
            ))
            .bytes(),
            &Limits::DEFAULT,
        )
        .unwrap()
        .features
    };
    let simple = |instruction: &str| {
        scan(
            &Docx::new(&format!(
                r#"<w:p><w:fldSimple w:instr="{instruction}"><w:r><w:t>1</w:t></w:r></w:fldSimple></w:p>"#
            ))
            .bytes(),
            &Limits::DEFAULT,
        )
        .unwrap()
        .features
    };
    for field in [field, simple] {
        let short = field(r" PAGE \* MERGEFORMAT ");
        assert!(short.contains(&Feature::FieldFormattingSwitches));
        let long = field(&format!(r" PAGE {} \* MERGEFORMAT ", " ".repeat(600)));
        assert!(long.contains(&Feature::PageFields));
        assert!(!long.contains(&Feature::FieldFormattingSwitches));
    }
}

#[test]
fn entries_that_overlap() {
    // The document's data seems to run on into the next entry: the "overlapping files" bomb reuses data this way.
    let bytes = with_entry("word/document.xml", |entry| {
        entry.compressed_size += 40;
    });
    assert_eq!(zip_refusal(&bytes), ZipError::Overlap { entry: 2 });
    // An entry whose local header and data lie inside another entry's data: both are consistent on their own, but they share bytes.
    let mut entries = valid().entries();
    let inner = ZipEntry::stored("word/inner.xml", b"<inner/>");
    let mut quoted = local_header(&inner);
    quoted.extend_from_slice(&inner.data);
    let before: usize = entries
        .iter()
        .map(|entry| local_header(entry).len() + entry.data.len())
        .sum();
    let outer = ZipEntry::stored("word/outer.bin", &quoted);
    let inner_offset = before + local_header(&outer).len();
    entries.push(outer);
    entries.push(ZipEntry {
        offset: Some(u32::try_from(inner_offset).unwrap()),
        has_local_header: false,
        ..inner
    });
    assert!(matches!(
        zip_refusal(&zip(&entries)),
        ZipError::Overlap { .. }
    ));
}

#[test]
fn local_headers_that_contradict_the_central_directory() {
    let renamed = with_entry("word/document.xml", |entry| {
        entry.local_name = Some(b"word/other.xml".to_vec());
    });
    assert_eq!(zip_refusal(&renamed), ZipError::HeaderMismatch { entry: 1 });
    let method = with_entry("word/document.xml", |entry| entry.local_method = Some(0));
    assert_eq!(zip_refusal(&method), ZipError::HeaderMismatch { entry: 1 });
    let misplaced = with_entry("word/document.xml", |entry| entry.offset = Some(3));
    assert_eq!(
        zip_refusal(&misplaced),
        ZipError::MalformedEntry { entry: 1 }
    );
}

#[test]
fn content_that_does_not_match_its_header() {
    let crc = with_entry("word/document.xml", |entry| entry.crc32 ^= 1);
    assert_eq!(zip_refusal(&crc), ZipError::ChecksumMismatch { entry: 1 });
    let shorter = with_entry("word/document.xml", |entry| entry.uncompressed_size += 1);
    assert_eq!(zip_refusal(&shorter), ZipError::SizeMismatch { entry: 1 });
    let inflate = with_entry("word/document.xml", |entry| {
        entry.data = vec![0xFF; entry.data.len()];
    });
    assert_eq!(zip_refusal(&inflate), ZipError::CorruptData { entry: 1 });
    let stored = with_entry("word/document.xml", |entry| {
        let content = document_xml("<w:p/>").into_bytes();
        *entry = ZipEntry::stored("word/document.xml", &content);
        entry.uncompressed_size += 1;
    });
    assert_eq!(zip_refusal(&stored), ZipError::SizeMismatch { entry: 1 });
}

#[test]
fn zip64_archives_are_read_and_their_records_checked() {
    let layout = ZipLayout {
        zip64: true,
        ..ZipLayout::default()
    };
    scan(&zip_with(&valid().entries(), &layout), &Limits::DEFAULT).unwrap();
    let mut broken = zip_with(&valid().entries(), &layout);
    // The locator sits just before the 22-byte end record; break its signature.
    let locator = broken.len() - 22 - 20;
    if let Some(byte) = broken.get_mut(locator) {
        *byte = 0;
    }
    assert_eq!(zip_refusal(&broken), ZipError::MalformedDirectory);
}

// The package layer.

#[test]
fn packages_without_the_parts_every_document_needs() {
    let without = |name: &str| {
        let entries: Vec<ZipEntry> = valid()
            .entries()
            .into_iter()
            .filter(|entry| entry.name != name.as_bytes())
            .collect();
        refusal(&zip(&entries))
    };
    assert_eq!(
        without("[Content_Types].xml"),
        ScanError::MissingPart("[Content_Types].xml")
    );
    assert_eq!(
        without("_rels/.rels"),
        ScanError::MissingPart("package relationships (/_rels/.rels)")
    );
    assert_eq!(
        without("word/document.xml"),
        ScanError::MissingPart("main document part")
    );
}

#[test]
fn packages_that_are_not_docx_documents() {
    for (content_type, expected) in [
        (
            "application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml",
            "a template (.dotx)",
        ),
        (
            "application/vnd.ms-word.document.macroEnabled.main+xml",
            "a macro-enabled document (.docm)",
        ),
        (
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
            "not WordprocessingML",
        ),
    ] {
        let docx = Docx::with_main(content_type, document_xml("<w:p/>").as_bytes());
        assert_eq!(refusal(&docx.bytes()), ScanError::NotADocument(expected));
    }
}

#[test]
fn relationships_that_climb_out_point_outside_or_go_round_in_circles() {
    let header = |next: &str| {
        format!(
            r#"<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:r><w:t>{next}</w:t></w:r></w:p></w:hdr>"#
        )
    };
    let docx = valid()
        .part("word/header1.xml", "header", &header("one"))
        .part_from(
            "word/header1.xml",
            "word/header2.xml",
            &rel::of("header"),
            &header("two"),
        )
        // header2 refers back to header1: each part is still read once.
        .relationship("word/header2.xml", &rel::of("header"), "header1.xml", false)
        // Targets above the package root, and outside it, are never followed.
        .relationship(
            "word/document.xml",
            &rel::of("styles"),
            "../../../etc/passwd",
            false,
        )
        .relationship(
            "word/document.xml",
            &rel::of("header"),
            "file:///etc/passwd",
            true,
        )
        .relationship(
            "word/document.xml",
            &rel::of("footer"),
            "https://example.invalid/footer.xml",
            true,
        );
    let found = scan(&docx.bytes(), &Limits::DEFAULT).unwrap();
    let latin = found
        .scripts
        .get(&bayan_lab::scan::scripts::Script::Latin)
        .copied();
    // "Text", "one" and "two", each counted once.
    assert_eq!(latin, Some(10));
}

#[test]
fn too_many_parts_relationships_or_content_types() {
    // The tagger reads at most 2,000 parts; the relationships parts and the content types stream, which bayan-opc reads when it opens the package, share bayan-opc's metadata budgets instead: 250,000 XML nodes and 16 MiB for all of them together.
    let mut docx = valid();
    for number in 0..2_001 {
        docx = docx.part(
            &format!("word/header{number}.xml"),
            "header",
            r#"<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>"#,
        );
    }
    assert_eq!(refusal(&docx.bytes()), ScanError::TooManyParts);
    let mut relationships = String::from(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    // Four nodes each: the element and its three attributes.
    for number in 0..62_500 {
        relationships.push_str(&format!(
            r#"<Relationship Id="r{number}" Type="t" Target="x{number}"/>"#
        ));
    }
    relationships.push_str("</Relationships>");
    let bytes = with_entry("_rels/.rels", |entry| {
        *entry = ZipEntry::deflated("_rels/.rels", relationships.as_bytes());
    });
    assert!(matches!(
        package_refusal(&bytes),
        Error::Xml {
            entry: Some(_),
            error: bayan_opc::XmlError {
                kind: XmlErrorKind::TooManyNodes,
                ..
            }
        }
    ));
    let mut types = String::from(
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#,
    );
    // Three nodes each, the element and its two attributes, and two for the root: the last `ContentType` attribute is node 250,001.
    for number in 0..83_333 {
        types.push_str(&format!(
            r#"<Default Extension="x{number}" ContentType="application/x"/>"#
        ));
    }
    types.push_str("</Types>");
    let bytes = with_entry("[Content_Types].xml", |entry| {
        *entry = ZipEntry::deflated("[Content_Types].xml", types.as_bytes());
    });
    assert_eq!(
        package_refusal(&bytes),
        Error::Xml {
            entry: Some(0),
            error: bayan_opc::XmlError {
                offset: types.rfind("ContentType").unwrap(),
                kind: XmlErrorKind::TooManyNodes,
            }
        }
    );
}

// The XML layer.

#[test]
fn entity_expansion_and_external_entities_are_impossible() {
    let billion_laughs = br#"<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">]><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>&lol2;</w:t></w:r></w:p></w:body></w:document>"#;
    assert_eq!(xml_refusal(billion_laughs), ErrorKind::DoctypeNotAllowed);
    let external = br#"<?xml version="1.0"?><!DOCTYPE d [<!ENTITY xxe SYSTEM "file:///etc/passwd">]><d>&xxe;</d>"#;
    assert_eq!(xml_refusal(external), ErrorKind::DoctypeNotAllowed);
    let undeclared = document_xml("<w:p><w:r><w:t>&xxe;</w:t></w:r></w:p>");
    assert_eq!(
        xml_refusal(undeclared.as_bytes()),
        ErrorKind::UndefinedEntity
    );
    let bad_reference = document_xml("<w:p><w:r><w:t>&#0;</w:t></w:r></w:p>");
    assert_eq!(
        xml_refusal(bad_reference.as_bytes()),
        ErrorKind::InvalidCharacterReference
    );
}

#[test]
fn nesting_attributes_names_and_namespaces_beyond_the_limits() {
    let deep = document_xml(&format!(
        "{}{}",
        "<w:p>".repeat(1_100),
        "</w:p>".repeat(1_100)
    ));
    assert_eq!(xml_refusal(deep.as_bytes()), ErrorKind::TooDeep);
    let attributes: String = (0..300)
        .map(|number| format!(r#" a{number}="1""#))
        .collect();
    let crowded = document_xml(&format!("<w:p{attributes}/>"));
    assert_eq!(
        xml_refusal(crowded.as_bytes()),
        ErrorKind::TooManyAttributes
    );
    let long = document_xml(&format!("<w:{}/>", "p".repeat(300)));
    assert_eq!(xml_refusal(long.as_bytes()), ErrorKind::NameTooLong);
    let namespaces: String = (0..300)
        .map(|number| format!(r#" xmlns:n{number}="urn:example:{number}""#))
        .collect();
    let declared = document_xml(&format!("<w:p{namespaces}/>"));
    // 300 declarations are also 300 attributes; whichever limit is checked first refuses them.
    assert!(matches!(
        xml_refusal(declared.as_bytes()),
        ErrorKind::TooManyNamespaces | ErrorKind::TooManyAttributes
    ));
    // Namespace names pile up across the part too: bayan-xml keeps each distinct name once, and refuses a part that declares more than 4,096 of them.
    let many: String = (0..4_097)
        .map(|number| format!(r#"<w:p xmlns:n="urn:example:{number}"/>"#))
        .collect();
    assert_eq!(
        xml_refusal(document_xml(&many).as_bytes()),
        ErrorKind::TooManyNamespaces
    );
}

#[test]
fn xml_that_is_not_well_formed() {
    for (body, expected) in [
        ("<x:p/>", ErrorKind::UndeclaredPrefix),
        ("<w:p></w:r>", ErrorKind::MismatchedEndTag),
        ("<w:p>\u{1}</w:p>", ErrorKind::InvalidCharacter),
    ] {
        assert_eq!(
            xml_refusal(document_xml(body).as_bytes()),
            expected,
            "{body}"
        );
    }
    let unclosed = document_xml("<w:p>").replace("</w:body></w:document>", "");
    assert_eq!(xml_refusal(unclosed.as_bytes()), ErrorKind::UnexpectedEnd);
    assert_eq!(xml_refusal(b"\xFF\xFE\x00"), ErrorKind::InvalidEncoding);
    assert_eq!(
        xml_refusal(b"<w:document \xC3\x28/>"),
        ErrorKind::InvalidEncoding
    );
    assert_eq!(xml_refusal(b"<a b=1/>"), ErrorKind::MalformedTag);
    // UTF-16 with a byte order mark is allowed.
    let utf16: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain(
            document_xml("<w:p><w:r><w:t>UTF</w:t></w:r></w:p>")
                .replace(r#"encoding="UTF-8""#, r#"encoding="UTF-16""#)
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        )
        .collect();
    scan(&with_document(&utf16), &Limits::DEFAULT).unwrap();
}

#[test]
fn field_nesting_and_instruction_text_stay_bounded() {
    let begin = r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#;
    let deep = format!("<w:p>{}</w:p>", begin.repeat(100));
    let found = scan(
        &with_document(document_xml(&deep).as_bytes()),
        &Limits::DEFAULT,
    )
    .unwrap();
    assert!(
        found
            .notes
            .contains("fields nested deeper than the limit of 64 were not classified")
    );
    let long = format!(
        r#"<w:p>{begin}<w:r><w:instrText>{}</w:instrText></w:r></w:p>"#,
        "PAGE ".repeat(100_000)
    );
    scan(
        &with_document(document_xml(&long).as_bytes()),
        &Limits::DEFAULT,
    )
    .unwrap();
}

// No input makes the scanner panic.

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2_048)) {
        let _ = scan(&bytes, &Limits::DEFAULT);
    }

    #[test]
    fn damaged_packages_never_panic(
        changes in proptest::collection::vec((any::<proptest::sample::Index>(), any::<u8>()), 1..16),
        cut in any::<proptest::sample::Index>(),
        truncate in any::<bool>(),
    ) {
        let mut bytes = valid().part(
            "word/styles.xml",
            "styles",
            r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="paragraph"><w:name w:val="Normal"/></w:style></w:styles>"#,
        ).bytes();
        for (at, value) in changes {
            let index = at.index(bytes.len());
            if let Some(byte) = bytes.get_mut(index) {
                *byte = value;
            }
        }
        if truncate {
            bytes.truncate(cut.index(bytes.len()));
        }
        let _ = scan(&bytes, &Limits::DEFAULT);
    }

    #[test]
    fn damaged_xml_never_panics(
        changes in proptest::collection::vec((any::<proptest::sample::Index>(), any::<u8>()), 1..16),
    ) {
        let mut xml = document_xml(concat!(
            r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
            r#"<w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:t>&amp;&#x41;</w:t></w:r>"#,
            r#"<mc:AlternateContent><mc:Choice Requires="wps"><w:r/></mc:Choice><mc:Fallback/></mc:AlternateContent></w:p>"#,
        )).into_bytes();
        for (at, value) in changes {
            let index = at.index(xml.len());
            if let Some(byte) = xml.get_mut(index) {
                *byte = value;
            }
        }
        let _ = scan(&with_document(&xml), &Limits::DEFAULT);
    }
}

#[test]
fn an_end_record_hidden_in_the_comment_of_another() {
    // Record B, a copy of the real end record, hides in the comment of record A; both end the file exactly. Readers disagree on which one counts, so the archive is refused.
    let entries = valid().entries();
    let real = zip(&entries);
    let record = real.get(real.len() - 22..).unwrap().to_vec();
    let disguised = zip_with(
        &entries,
        &ZipLayout {
            comment: record,
            ..ZipLayout::default()
        },
    );
    assert_eq!(zip_refusal(&disguised), ZipError::AmbiguousEnd);
}

#[test]
fn an_end_record_signature_in_the_comment_that_does_not_end_the_file() {
    // The real record A ends the file; its comment holds the signature of a record B that does not. Python's zipfile and Info-ZIP take the last signature they find, B, which can point to another central directory, so the archive is refused.
    let entries = valid().entries();
    let real = zip(&entries);
    let mut record = real.get(real.len() - 22..).unwrap().to_vec();
    record.extend_from_slice(b"tail");
    let disguised = zip_with(
        &entries,
        &ZipLayout {
            comment: record,
            ..ZipLayout::default()
        },
    );
    assert_eq!(zip_refusal(&disguised), ZipError::AmbiguousEnd);
}

#[test]
fn a_gap_between_the_central_directory_and_its_end_record() {
    // Python's zipfile looks for the central directory just before its end record (to allow data in front of an archive), so a gap would let it read other bytes than the declared offset points to.
    let gapped = zip_with(
        &valid().entries(),
        &ZipLayout {
            gap: b"other bytes".to_vec(),
            ..ZipLayout::default()
        },
    );
    assert_eq!(zip_refusal(&gapped), ZipError::MalformedDirectory);
}

#[test]
fn a_zip64_locator_before_an_end_record_that_needs_none() {
    // A ZIP64 archive whose classic end record states the real values: readers that always follow the locator read the ZIP64 record instead. bayan-opc lets the two records exist side by side only where they agree, so that every reader finds the same central directory.
    let entries = valid().entries();
    let mut bytes = zip_with(
        &entries,
        &ZipLayout {
            zip64: true,
            ..ZipLayout::default()
        },
    );
    let end = bytes.len() - 22;
    let classic = zip(&entries);
    let classic_end = classic.len() - 22;
    // The real entry count, directory size and offset, in place of the saturated ones.
    let real_fields = classic
        .get(classic_end + 8..classic_end + 20)
        .unwrap()
        .to_vec();
    bytes.splice(end + 8..end + 20, real_fields);
    scan(&bytes, &Limits::DEFAULT).unwrap();
    // Where the classic record puts the central directory one byte further on than the ZIP64 record, the readers would disagree, so the archive is refused.
    let offset = u32::from_le_bytes(bytes[end + 16..end + 20].try_into().unwrap());
    bytes.splice(end + 16..end + 20, (offset + 1).to_le_bytes());
    assert_eq!(zip_refusal(&bytes), ZipError::MalformedDirectory);
}

#[test]
fn a_zip64_record_with_extensible_data() {
    // Python's zipfile reads the ZIP64 record just before its locator and assumes it has no extensible data.
    let mut bytes = zip_with(
        &valid().entries(),
        &ZipLayout {
            zip64: true,
            ..ZipLayout::default()
        },
    );
    let record = bytes.len() - 22 - 20 - 56;
    assert_eq!(
        bytes.get(record..record + 4).unwrap(),
        [0x50, 0x4b, 0x06, 0x06]
    );
    bytes.splice(record + 4..record + 12, 52_u64.to_le_bytes());
    assert_eq!(zip_refusal(&bytes), ZipError::MalformedDirectory);
}

#[test]
fn a_zip64_record_away_from_its_locator() {
    // The locator points to the record, but Python's zipfile looks just before the locator: eight bytes between them give it other bytes to read.
    let mut bytes = zip_with(
        &valid().entries(),
        &ZipLayout {
            zip64: true,
            ..ZipLayout::default()
        },
    );
    let record_end = bytes.len() - 22 - 20;
    bytes.splice(record_end..record_end, [0_u8; 8]);
    assert_eq!(zip_refusal(&bytes), ZipError::MalformedDirectory);
}

#[test]
fn entry_counts_that_disagree_claim_several_disks() {
    // The end record counts the entries on this disk and in all; a single-disk archive has the same number in both.
    let mut classic = zip(&valid().entries());
    let end = classic.len() - 22;
    let entries = u16::from_le_bytes([classic[end + 10], classic[end + 11]]);
    classic.splice(end + 8..end + 10, (entries - 1).to_le_bytes());
    assert_eq!(zip_refusal(&classic), ZipError::MultipleDisks);
    let mut zip64 = zip_with(
        &valid().entries(),
        &ZipLayout {
            zip64: true,
            ..ZipLayout::default()
        },
    );
    let record = zip64.len() - 22 - 20 - 56;
    let entries = u64::from_le_bytes(zip64[record + 32..record + 40].try_into().unwrap());
    zip64.splice(record + 24..record + 32, (entries - 1).to_le_bytes());
    assert_eq!(zip_refusal(&zip64), ZipError::MultipleDisks);
}

#[test]
fn a_folder_entry_with_content() {
    // A name ending in `/` is a folder; content under it would be a file that most readers never show.
    let mut entries = valid().entries();
    entries.push(ZipEntry::stored("word/hidden/", b"content"));
    assert_eq!(
        zip_refusal(&zip(&entries)),
        ZipError::FolderWithData { entry: 3 }
    );
    let mut empty = valid().entries();
    empty.push(ZipEntry::stored("word/folder/", b""));
    assert!(scan(&zip(&empty), &Limits::DEFAULT).is_ok());
}

#[test]
fn compatibility_options_of_a_documents_own_choosing_are_not_recorded() {
    // Every child of w:compat used to become a recorded option: a million invented names made a 24 MB manifest entry.
    let mut options = String::from("<w:useFELayout/><w:doNotExpandShiftReturn w:val=\"0\"/>");
    for number in 0..10_000 {
        options.push_str(&format!("<w:invented{number}/>"));
    }
    let settings = format!(
        r#"<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:compat>{options}</w:compat></w:settings>"#
    );
    let found = scan(
        &valid()
            .part("word/settings.xml", "settings", &settings)
            .bytes(),
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(
        found.compatibility_options.into_iter().collect::<Vec<_>>(),
        ["useFELayout"]
    );
    assert!(
        found
            .notes
            .contains("a compatibility option that ECMA-376 does not define was not recorded")
    );
}

#[test]
fn hidden_characters_in_names_are_not_recorded() {
    // A font name with a right-to-left override could make a reviewer read a report wrongly ("Trojan Source").
    let table = "<w:fonts xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:font w:name=\"Ari\u{202E}la\"/><w:font w:name=\"Calibri\"/></w:fonts>";
    let found = scan(
        &valid()
            .part("word/fontTable.xml", "fontTable", table)
            .bytes(),
        &Limits::DEFAULT,
    )
    .unwrap();
    assert_eq!(found.fonts.into_iter().collect::<Vec<_>>(), ["Calibri"]);
    assert!(
        found
            .notes
            .contains("a font name with control or invisible characters was not recorded")
    );
}
