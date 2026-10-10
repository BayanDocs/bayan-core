//! Crafted malformed and malicious packages (LAB-001 AC-3): each must be refused with the error that names its flaw, quickly and within the scanner's limits, and nothing may make the scanner panic.
//!
//! The packages are built in memory by `support`, field by field, so every flaw is deliberate and visible in the test. The limits are the ones the tools use ([`Limits::DEFAULT`]) unless a test says otherwise.

use bayan_lab::scan::opc::OpcError;
use bayan_lab::scan::xml::XmlError;
use bayan_lab::scan::zip::ZipError;
use bayan_lab::scan::{Limits, ScanError, scan};
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

fn zip_refusal(bytes: &[u8]) -> ZipError {
    match refusal(bytes) {
        ScanError::Zip(error) => error,
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

fn xml_refusal(xml: &[u8]) -> XmlError {
    match refusal(&with_document(xml)) {
        ScanError::Part {
            part,
            error: OpcError::Xml(error),
        } => {
            assert_eq!(part, "/word/document.xml");
            error
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
            ZipError::MultiDisk
        );
    }
    let bytes = with_entry("word/document.xml", |entry| entry.disk = 2);
    assert_eq!(zip_refusal(&bytes), ZipError::MultiDisk);
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
            ZipError::Corrupt("the central directory is too small for its entry count"),
        ),
        (
            ZipLayout {
                directory_offset: Some(u32::try_from(real.len()).unwrap()),
                ..ZipLayout::default()
            },
            ZipError::Corrupt("the central directory extends past its end record"),
        ),
        (
            ZipLayout {
                directory_offset: Some(u32::MAX - 4),
                ..ZipLayout::default()
            },
            ZipError::Corrupt("the central directory extends past its end record"),
        ),
        (
            ZipLayout {
                directory_size: Some(1000),
                ..ZipLayout::default()
            },
            ZipError::Corrupt("the central directory extends past its end record"),
        ),
        (
            ZipLayout {
                entries: Some(10_001),
                ..ZipLayout::default()
            },
            ZipError::TooManyEntries,
        ),
    ] {
        assert_eq!(zip_refusal(&zip_with(&entries, &layout)), expected);
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
        assert_eq!(zip_refusal(&bytes), ZipError::BadName, "{name:?}");
    }
    let mut invalid_utf8 = ZipEntry::stored("word/x.xml", b"x");
    invalid_utf8.name = b"word/\xFF.xml".to_vec();
    invalid_utf8.local_name = Some(invalid_utf8.name.clone());
    assert_eq!(
        zip_refusal(&with_extra_entry(invalid_utf8)),
        ZipError::BadName
    );
    let long = with_extra_entry(ZipEntry::stored(&"a".repeat(1025), b"x"));
    assert_eq!(zip_refusal(&long), ZipError::NameTooLong);
}

#[test]
fn two_names_for_one_part() {
    // Names that differ only in case, or only in percent-encoding, name the same part: which one would Word read?
    for twin in ["WORD/DOCUMENT.XML", "word/%64ocument.xml"] {
        let mut entries = valid().entries();
        let mut copy = entries
            .iter()
            .find(|entry| entry.name == b"word/document.xml")
            .unwrap()
            .clone();
        copy.name = twin.as_bytes().to_vec();
        entries.push(copy);
        assert_eq!(
            zip_refusal(&zip(&entries)),
            ZipError::DuplicateName,
            "{twin}"
        );
    }
}

#[test]
fn encrypted_entries_links_and_unknown_compression() {
    type Change = fn(&mut ZipEntry);
    let cases: [(Change, ZipError); 5] = [
        (|entry| entry.flags = 1, ZipError::Encrypted),
        (|entry| entry.flags = 0x40, ZipError::Encrypted),
        (
            |entry| {
                entry.method = 99;
                entry.local_method = Some(99);
            },
            ZipError::Encrypted,
        ),
        (
            |entry| {
                entry.version_made_by = 3 << 8;
                entry.external_attributes = 0o120_777 << 16;
            },
            ZipError::SymbolicLink,
        ),
        (
            |entry| {
                entry.method = 12;
                entry.local_method = Some(12);
            },
            ZipError::UnsupportedCompression(12),
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
    assert_eq!(zip_refusal(&huge), ZipError::EntryTooLarge);
    // Five entries of 250 MiB each declare more than the 1 GiB total.
    let mut entries = valid().entries();
    for number in 0..5 {
        let mut entry = ZipEntry::deflated(&format!("word/media/big{number}.bin"), b"x");
        entry.uncompressed_size = 250 * 1024 * 1024;
        entries.push(entry);
    }
    assert_eq!(zip_refusal(&zip(&entries)), ZipError::TotalTooLarge);
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
    assert_eq!(zip_refusal(&bytes), ZipError::SizeMismatch);
}

#[test]
fn entries_that_overlap() {
    // The document's data seems to run on into the next entry: the "overlapping files" bomb reuses data this way.
    let bytes = with_entry("word/document.xml", |entry| {
        entry.compressed_size += 40;
    });
    assert_eq!(zip_refusal(&bytes), ZipError::Overlap);
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
    assert_eq!(zip_refusal(&zip(&entries)), ZipError::Overlap);
}

#[test]
fn local_headers_that_contradict_the_central_directory() {
    let renamed = with_entry("word/document.xml", |entry| {
        entry.local_name = Some(b"word/other.xml".to_vec());
    });
    assert_eq!(
        zip_refusal(&renamed),
        ZipError::Corrupt("local header's name differs from the central directory")
    );
    let method = with_entry("word/document.xml", |entry| entry.local_method = Some(0));
    assert_eq!(
        zip_refusal(&method),
        ZipError::Corrupt("local header's compression method differs from the central directory")
    );
    let misplaced = with_entry("word/document.xml", |entry| entry.offset = Some(3));
    assert_eq!(
        zip_refusal(&misplaced),
        ZipError::Corrupt("local header signature")
    );
}

#[test]
fn content_that_does_not_match_its_header() {
    let crc = with_entry("word/document.xml", |entry| entry.crc32 ^= 1);
    assert_eq!(zip_refusal(&crc), ZipError::CrcMismatch);
    let shorter = with_entry("word/document.xml", |entry| entry.uncompressed_size += 1);
    assert_eq!(zip_refusal(&shorter), ZipError::SizeMismatch);
    let inflate = with_entry("word/document.xml", |entry| {
        entry.data = vec![0xFF; entry.data.len()];
    });
    assert_eq!(zip_refusal(&inflate), ZipError::Inflate);
    let stored = with_entry("word/document.xml", |entry| {
        let content = document_xml("<w:p/>").into_bytes();
        *entry = ZipEntry::stored("word/document.xml", &content);
        entry.uncompressed_size += 1;
    });
    assert_eq!(
        zip_refusal(&stored),
        ZipError::Corrupt("a stored entry's sizes differ")
    );
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
    assert_eq!(
        zip_refusal(&broken),
        ZipError::Corrupt("ZIP64 end locator missing")
    );
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
    for number in 0..10_001 {
        relationships.push_str(&format!(
            r#"<Relationship Id="r{number}" Type="t" Target="x{number}"/>"#
        ));
    }
    relationships.push_str("</Relationships>");
    let bytes = with_entry("_rels/.rels", |entry| {
        *entry = ZipEntry::deflated("_rels/.rels", relationships.as_bytes());
    });
    assert_eq!(
        refusal(&bytes),
        ScanError::Part {
            part: "/_rels/.rels".to_owned(),
            error: OpcError::TooManyItems
        }
    );
}

// The XML layer.

#[test]
fn entity_expansion_and_external_entities_are_impossible() {
    let billion_laughs = br#"<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">]><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>&lol2;</w:t></w:r></w:p></w:body></w:document>"#;
    assert_eq!(xml_refusal(billion_laughs), XmlError::Doctype);
    let external = br#"<?xml version="1.0"?><!DOCTYPE d [<!ENTITY xxe SYSTEM "file:///etc/passwd">]><d>&xxe;</d>"#;
    assert_eq!(xml_refusal(external), XmlError::Doctype);
    let undeclared = document_xml("<w:p><w:r><w:t>&xxe;</w:t></w:r></w:p>");
    assert_eq!(xml_refusal(undeclared.as_bytes()), XmlError::Entity);
    let bad_reference = document_xml("<w:p><w:r><w:t>&#0;</w:t></w:r></w:p>");
    assert_eq!(xml_refusal(bad_reference.as_bytes()), XmlError::Entity);
}

#[test]
fn nesting_attributes_names_and_namespaces_beyond_the_limits() {
    let deep = document_xml(&format!(
        "{}{}",
        "<w:p>".repeat(1_100),
        "</w:p>".repeat(1_100)
    ));
    assert_eq!(xml_refusal(deep.as_bytes()), XmlError::TooDeep);
    let attributes: String = (0..300)
        .map(|number| format!(r#" a{number}="1""#))
        .collect();
    let crowded = document_xml(&format!("<w:p{attributes}/>"));
    assert_eq!(xml_refusal(crowded.as_bytes()), XmlError::TooManyAttributes);
    let long = document_xml(&format!("<w:{}/>", "p".repeat(300)));
    assert_eq!(xml_refusal(long.as_bytes()), XmlError::NameTooLong);
    let namespaces: String = (0..300)
        .map(|number| format!(r#" xmlns:n{number}="urn:example:{number}""#))
        .collect();
    let declared = document_xml(&format!("<w:p{namespaces}/>"));
    // 300 declarations are also 300 attributes; whichever limit is checked first refuses them.
    assert!(matches!(
        xml_refusal(declared.as_bytes()),
        XmlError::TooManyNamespaces | XmlError::TooManyAttributes
    ));
    // Namespace declarations pile up across nested elements too.
    let mut nested = String::new();
    for number in 0..300 {
        nested.push_str(&format!(r#"<w:p xmlns:n{number}="urn:example:{number}">"#));
    }
    nested.push_str(&"</w:p>".repeat(300));
    assert_eq!(
        xml_refusal(document_xml(&nested).as_bytes()),
        XmlError::TooManyNamespaces
    );
}

#[test]
fn xml_that_is_not_well_formed() {
    for (body, expected) in [
        ("<x:p/>", XmlError::UnboundPrefix),
        ("<w:p></w:r>", XmlError::MismatchedEnd),
        ("<w:p>\u{1}</w:p>", XmlError::InvalidCharacter),
    ] {
        assert_eq!(
            xml_refusal(document_xml(body).as_bytes()),
            expected,
            "{body}"
        );
    }
    let unclosed = document_xml("<w:p>").replace("</w:body></w:document>", "");
    assert_eq!(xml_refusal(unclosed.as_bytes()), XmlError::Unclosed);
    assert_eq!(xml_refusal(b"\xFF\xFE\x00"), XmlError::Encoding);
    assert_eq!(xml_refusal(b"<w:document \xC3\x28/>"), XmlError::Encoding);
    assert!(matches!(xml_refusal(b"<a b=1/>"), XmlError::Malformed(_)));
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
    assert_eq!(
        zip_refusal(&disguised),
        ZipError::Corrupt("two end-of-central-directory records end the file")
    );
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
