//! The documents the tests read, compiled in: five real .docx files, and synthetic parts written for these tests.

/// Five real .docx files from python-docx 1.2.0 (MIT), which bayan-opc's tests keep; their source and checksums are in the README.md next to them. Most of their parts were written by Word.
pub const DOCX: &[(&str, &[u8])] = &[
    (
        "default.docx",
        include_bytes!("../../../bayan-opc/tests/fixtures/python-docx/default.docx"),
    ),
    (
        "doc-coreprops.docx",
        include_bytes!("../../../bayan-opc/tests/fixtures/python-docx/doc-coreprops.docx"),
    ),
    (
        "doc-word-default-blank.docx",
        include_bytes!("../../../bayan-opc/tests/fixtures/python-docx/doc-word-default-blank.docx"),
    ),
    (
        "hdr-header-footer.docx",
        include_bytes!("../../../bayan-opc/tests/fixtures/python-docx/hdr-header-footer.docx"),
    ),
    (
        "par-hyperlinks.docx",
        include_bytes!("../../../bayan-opc/tests/fixtures/python-docx/par-hyperlinks.docx"),
    ),
];

/// Synthetic document parts written for these tests: shapes in nested Markup Compatibility alternate content, and a document in the Strict conformance class.
pub const SAMPLES: &[(&str, &[u8])] = &[
    (
        "word-shapes.xml",
        include_bytes!("../fixtures/word-shapes.xml"),
    ),
    (
        "strict-document.xml",
        include_bytes!("../fixtures/strict-document.xml"),
    ),
];

/// Every XML part of the real documents and every sample, as (name, bytes): the parts' names are `<document>/<part name>`.
pub fn all_parts() -> Vec<(String, Vec<u8>)> {
    let mut parts = Vec::new();
    for (document, archive) in DOCX {
        for (part, bytes) in xml_parts(archive) {
            parts.push((format!("{document}/{part}"), bytes));
        }
    }
    for (name, bytes) in SAMPLES {
        parts.push(((*name).to_owned(), bytes.to_vec()));
    }
    parts
}

/// The XML parts of a ZIP archive (entries whose names end in `.xml` or `.rels`), as (name, bytes), in the order of its central directory. A minimal reader for trusted test files only: it panics on anything it does not expect.
pub fn xml_parts(archive: &[u8]) -> Vec<(String, Vec<u8>)> {
    let u16_at = |offset: usize| u16::from_le_bytes([archive[offset], archive[offset + 1]]);
    let u32_at = |offset: usize| {
        let value = u32::from_le_bytes(archive[offset..offset + 4].try_into().unwrap());
        usize::try_from(value).unwrap()
    };
    let end = (0..=archive.len() - 22)
        .rev()
        .find(|&offset| u32_at(offset) == 0x0605_4b50)
        .unwrap();
    let count = usize::from(u16_at(end + 10));
    let mut entry = u32_at(end + 16);
    let mut parts = Vec::new();
    for _ in 0..count {
        assert_eq!(u32_at(entry), 0x0201_4b50, "central directory entry");
        let method = u16_at(entry + 10);
        let compressed = u32_at(entry + 20);
        let size = u32_at(entry + 24);
        let name_length = usize::from(u16_at(entry + 28));
        let extra_length = usize::from(u16_at(entry + 30));
        let comment_length = usize::from(u16_at(entry + 32));
        let local = u32_at(entry + 42);
        let name = std::str::from_utf8(&archive[entry + 46..entry + 46 + name_length])
            .unwrap()
            .to_owned();
        entry += 46 + name_length + extra_length + comment_length;
        if !(name.ends_with(".xml") || name.ends_with(".rels")) {
            continue;
        }
        assert_eq!(u32_at(local), 0x0403_4b50, "local header");
        let start = local + 30 + usize::from(u16_at(local + 26)) + usize::from(u16_at(local + 28));
        let data = &archive[start..start + compressed];
        let bytes = match method {
            0 => data.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec(data).unwrap(),
            _ => panic!("unexpected compression method"),
        };
        assert_eq!(bytes.len(), size, "uncompressed size");
        parts.push((name, bytes));
    }
    parts
}
