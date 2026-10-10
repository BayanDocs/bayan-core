//! Crafted malicious and malformed archives: each one is refused with its own error, within the limits, before any large amount of memory is set aside (work package CORE-005, acceptance criterion 2).
//!
//! The archives are built byte by byte with the small builder in [`craft`], independent of the crate's own writer, so that a mistake in the writer cannot hide a mistake in the reader. Many of them declare sizes of terabytes: if the reader tried to allocate memory for what an archive claims before checking it, the test process would abort instead of passing.

#![cfg(test)]

use bayan_opc::zip::ZipArchive;
use bayan_opc::{
    Error, LimitError, Limits, Package, PackageError, RelationshipsError, XmlErrorKind, ZipError,
};

use craft::{Archive, Descriptor, Item, Symbol};

/// A valid minimal package: a content types stream and one part.
fn package_items() -> Vec<Item> {
    vec![
        Item::deflated(
            "[Content_Types].xml",
            b"<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"xml\" ContentType=\"application/xml\"/></Types>",
        ),
        Item::deflated("word/document.xml", b"<document/>"),
    ]
}

fn open_zip(bytes: &[u8]) -> Result<ZipArchive<'_>, Error> {
    ZipArchive::new(bytes, &Limits::default())
}

fn zip_error(bytes: &[u8]) -> ZipError {
    match open_zip(bytes) {
        Err(Error::Zip(error)) => error,
        other => panic!("expected a ZIP error, got {other:?}"),
    }
}

fn limit_error(bytes: &[u8], limits: &Limits) -> LimitError {
    match ZipArchive::new(bytes, limits) {
        Err(Error::Limit(error)) => error,
        other => panic!("expected a limit error, got {other:?}"),
    }
}

fn package_error(bytes: &[u8]) -> Error {
    Package::open(bytes, &Limits::default()).unwrap_err()
}

#[test]
fn the_builder_makes_valid_archives() {
    let bytes = Archive::new(package_items()).build();
    let package = Package::open(&bytes, &Limits::default()).unwrap();
    assert_eq!(package.parts().count(), 1);
    // Data descriptors with and without a signature, with 4- and 8-byte sizes, and ZIP64 records are all fine.
    for descriptor in [
        Descriptor {
            signature: true,
            wide: false,
        },
        Descriptor {
            signature: false,
            wide: false,
        },
        Descriptor {
            signature: true,
            wide: true,
        },
        Descriptor {
            signature: false,
            wide: true,
        },
    ] {
        let mut item = Item::deflated("a.xml", b"<a/>");
        item.descriptor = Some(descriptor);
        item.zip64 = descriptor.wide;
        let bytes = Archive::new(vec![item]).build();
        assert_eq!(open_zip(&bytes).unwrap().read(0).unwrap(), b"<a/>");
    }
    let mut zip64 = Archive::new(package_items());
    zip64.zip64_end = true;
    for item in &mut zip64.items {
        item.zip64 = true;
    }
    let bytes = zip64.build();
    assert_eq!(open_zip(&bytes).unwrap().read(1).unwrap(), b"<document/>");
}

// Zip bombs and lying sizes.

#[test]
fn refuses_an_entry_that_claims_more_than_the_entry_limit() {
    let mut item = Item::stored("bomb.bin", b"tiny");
    item.size = 1 << 40;
    item.method = 8;
    item.zip64 = true;
    assert_eq!(
        limit_error(&Archive::new(vec![item]).build(), &Limits::default()),
        LimitError::EntryTooLarge {
            entry: 0,
            limit: 512 << 20
        }
    );
}

#[test]
fn refuses_a_highly_compressed_entry() {
    // 10 MiB of zeros compress to about 10 KiB: a ratio near 1,000 to 1.
    let zeros = vec![0_u8; 10 << 20];
    let item = Item::deflated("zeros.bin", &zeros);
    assert!(item.stored.len() < 20_000);
    assert_eq!(
        limit_error(&Archive::new(vec![item]).build(), &Limits::default()),
        LimitError::CompressionRatio {
            entry: Some(0),
            limit: 100
        }
    );
}

#[test]
fn refuses_many_small_highly_compressed_entries() {
    // Each entry stays below the grace size of 1 MiB, but together they make a bomb.
    let zeros = vec![0_u8; 1 << 20];
    let compressed = craft::deflate(&zeros);
    let crc32 = craft::crc32(&zeros);
    let items = (0..64)
        .map(|index| {
            let mut item = Item::stored(&format!("part{index}.bin"), &[]);
            item.stored.clone_from(&compressed);
            item.method = 8;
            item.size = 1 << 20;
            item.crc32 = crc32;
            item
        })
        .collect();
    assert_eq!(
        limit_error(&Archive::new(items).build(), &Limits::default()),
        LimitError::CompressionRatio {
            entry: None,
            limit: 100
        }
    );
}

#[test]
fn refuses_entries_that_add_up_to_more_than_the_total_limit() {
    let items: Vec<Item> = (0..5)
        .map(|index| Item::stored(&format!("part{index}.bin"), &vec![7_u8; 1_000]))
        .collect();
    let limits = Limits {
        max_total_size: 4_000,
        ..Limits::default()
    };
    assert_eq!(
        limit_error(&Archive::new(items).build(), &limits),
        LimitError::TotalTooLarge { limit: 4_000 }
    );
}

#[test]
fn stops_decompressing_at_the_declared_size() {
    // The headers say 100 bytes; the data would expand to 8 MiB. Decompression stops after 100 bytes.
    let mut item = Item::deflated("liar.bin", &vec![0_u8; 8 << 20]);
    item.size = 100;
    let bytes = Archive::new(vec![item]).build();
    let archive = open_zip(&bytes).unwrap();
    assert_eq!(
        archive.read(0),
        Err(Error::Zip(ZipError::SizeMismatch { entry: 0 }))
    );
    assert_eq!(
        archive.verify(0),
        Err(Error::Zip(ZipError::SizeMismatch { entry: 0 }))
    );
    // Decompression stops as soon as the output passes the declared size, before it reads further: here 200 good bytes are followed by a damaged block, and the answer is the size, not the damage.
    let mut stream = vec![0x00, 200, 0, !200_u8, 0xFF];
    stream.extend_from_slice(&[b'a'; 200]);
    stream.push(0x07);
    let mut item = Item::stored("early.bin", &[]);
    item.stored = stream;
    item.method = 8;
    item.size = 100;
    let bytes = Archive::new(vec![item]).build();
    let archive = open_zip(&bytes).unwrap();
    assert_eq!(
        archive.read(0),
        Err(Error::Zip(ZipError::SizeMismatch { entry: 0 }))
    );
    assert_eq!(
        archive.verify(0),
        Err(Error::Zip(ZipError::SizeMismatch { entry: 0 }))
    );
    // And data that is shorter than declared is refused too.
    let mut item = Item::deflated("short.bin", b"short");
    item.size = 1_000;
    let bytes = Archive::new(vec![item]).build();
    let archive = open_zip(&bytes).unwrap();
    assert_eq!(
        archive.read(0),
        Err(Error::Zip(ZipError::SizeMismatch { entry: 0 }))
    );
    assert_eq!(
        archive.verify(0),
        Err(Error::Zip(ZipError::SizeMismatch { entry: 0 }))
    );
}

#[test]
fn refuses_entries_that_share_a_local_header() {
    // Two central directory entries point to the same local header. Under different names, the second name cannot match the one written there.
    let mut archive = Archive::new(vec![
        Item::stored("a.xml", b"same"),
        Item::stored("b.xml", b"same"),
    ]);
    archive.items[1].central_offset = Some(0);
    archive.items[1].skip_local = true;
    assert_eq!(
        zip_error(&archive.build()),
        ZipError::HeaderMismatch { entry: 1 }
    );
    // Under the same name, they are duplicates.
    let mut archive = Archive::new(vec![
        Item::stored("a.xml", b"same"),
        Item::stored("a.xml", b"same"),
    ]);
    archive.items[1].central_offset = Some(0);
    archive.items[1].skip_local = true;
    assert_eq!(
        zip_error(&archive.build()),
        ZipError::DuplicateName { entry: 1, first: 0 }
    );
}

#[test]
fn refuses_overlapping_data() {
    // The second entry's local header lies inside the first entry's data.
    let inner = Archive::new(vec![Item::stored("inner.xml", b"<i/>")]).build();
    let local_header_size = 30 + "inner.xml".len();
    // David Fifield's "better zip bomb" nests entries like this, so that a small archive yields its data many times over.
    let outer = Item::stored("outer.bin", &inner[..local_header_size + 4]);
    let mut archive = Archive::new(vec![outer, Item::stored("inner.xml", b"<i/>")]);
    let outer_data_start = 30 + "outer.bin".len();
    archive.items[1].central_offset = Some(u64::try_from(outer_data_start).unwrap());
    archive.items[1].skip_local = true;
    assert_eq!(zip_error(&archive.build()), ZipError::Overlap { entry: 1 });
}

#[test]
fn refuses_too_many_entries() {
    let items: Vec<Item> = (0..4)
        .map(|index| Item::stored(&format!("{index}"), b""))
        .collect();
    let limits = Limits {
        max_entries: 3,
        ..Limits::default()
    };
    assert_eq!(
        limit_error(&Archive::new(items).build(), &limits),
        LimitError::TooManyEntries { limit: 3 }
    );
    // A count that the central directory is far too small to hold is a lie, refused before anything is allocated for it.
    let mut archive = Archive::new(vec![Item::stored("a", b"")]);
    archive.count = Some(9_000);
    assert_eq!(zip_error(&archive.build()), ZipError::MalformedDirectory);
}

#[test]
fn refuses_names_longer_than_the_limit() {
    let name = "a".repeat(2_000);
    assert_eq!(
        limit_error(
            &Archive::new(vec![Item::stored(&name, b"x")]).build(),
            &Limits::default()
        ),
        LimitError::NameTooLong {
            entry: 0,
            limit: 1_024
        }
    );
}

// Names.

#[test]
fn refuses_names_that_escape_or_confuse() {
    for name in [
        "../evil.xml",
        "word/../../evil.xml",
        "/etc/passwd",
        "C:/Windows/evil.dll",
        "c:evil",
        "..\\..\\evil.xml",
        "word\\document.xml",
        "word/./document.xml",
        "word//document.xml",
        "word/doc\u{0}.xml",
        "word/doc\n.xml",
    ] {
        let bytes = Archive::new(vec![Item::stored(name, b"x")]).build();
        assert_eq!(
            zip_error(&bytes),
            ZipError::UnsafeName { entry: 0 },
            "{name:?}"
        );
    }
    // Names that are not UTF-8 cannot be compared reliably.
    let mut item = Item::stored("x", b"x");
    item.name_bytes = Some(vec![b'w', 0xFF, b'x']);
    assert_eq!(
        zip_error(&Archive::new(vec![item]).build()),
        ZipError::UnsafeName { entry: 0 }
    );
}

#[test]
fn refuses_duplicate_and_equivalent_names() {
    for (first, second) in [
        ("word/document.xml", "word/document.xml"),
        ("word/document.xml", "Word/Document.XML"),
        ("[Content_Types].xml", "[content_types].XML"),
        ("m%C3%A9dia/a.png", "média/a.png"),
        ("M%C3%A9dia/a.png", "m%c3%a9dia/A.png"),
        ("word/", "WORD/"),
    ] {
        let items = vec![Item::stored(first, b""), Item::stored(second, b"")];
        assert_eq!(
            zip_error(&Archive::new(items).build()),
            ZipError::DuplicateName { entry: 1, first: 0 },
            "{first} {second}"
        );
    }
}

// Encryption, compression methods and special files.

#[test]
fn refuses_encrypted_entries() {
    for (flags, method) in [(1, 8), (1 | 1 << 6, 8), (1 << 13, 8), (0, 99)] {
        let mut item = Item::stored("secret.xml", b"x");
        item.flags = flags;
        item.method = method;
        assert_eq!(
            zip_error(&Archive::new(vec![item]).build()),
            ZipError::Encrypted { entry: 0 },
            "{flags} {method}"
        );
    }
}

#[test]
fn refuses_other_compression_methods() {
    // Shrink, implode, Deflate64, bzip2, LZMA, Zstandard, XZ and PPMd.
    for method in [1, 6, 9, 12, 14, 93, 95, 98] {
        let mut item = Item::stored("a.bin", b"x");
        item.method = method;
        assert_eq!(
            zip_error(&Archive::new(vec![item]).build()),
            ZipError::UnsupportedCompression { entry: 0, method }
        );
    }
}

#[test]
fn refuses_patch_data() {
    let mut item = Item::stored("a.bin", b"x");
    item.flags = 1 << 5;
    assert_eq!(
        zip_error(&Archive::new(vec![item]).build()),
        ZipError::UnsupportedFeature { entry: 0 }
    );
}

#[test]
fn refuses_symbolic_links_and_special_files() {
    for (host, mode) in [
        (3, 0o120_777),
        (19, 0o120_755),
        (3, 0o060_644),
        (3, 0o010_644),
        (3, 0o140_644),
        (3, 0o040_755),
    ] {
        let mut item = Item::stored("link", b"/etc/passwd");
        item.made_by = host << 8 | 63;
        item.external = mode << 16;
        assert_eq!(
            zip_error(&Archive::new(vec![item]).build()),
            ZipError::SpecialFile { entry: 0 },
            "{mode:o}"
        );
    }
    // A volume label (MS-DOS attribute 0x08).
    let mut item = Item::stored("label", b"");
    item.external = 0x08;
    assert_eq!(
        zip_error(&Archive::new(vec![item]).build()),
        ZipError::SpecialFile { entry: 0 }
    );
    // Regular files and folders with Unix modes are fine.
    let mut file = Item::stored("file.xml", b"x");
    file.made_by = 3 << 8 | 30;
    file.external = 0o100_644 << 16;
    let mut folder = Item::stored("folder/", b"");
    folder.made_by = 3 << 8 | 30;
    folder.external = 0o040_755 << 16 | 0x10;
    assert!(open_zip(&Archive::new(vec![file, folder]).build()).is_ok());
}

#[test]
fn refuses_folders_with_data() {
    assert_eq!(
        zip_error(&Archive::new(vec![Item::stored("folder/", b"data")]).build()),
        ZipError::FolderWithData { entry: 0 }
    );
}

// Damaged data.

#[test]
fn refuses_damaged_data() {
    let mut item = Item::deflated("a.xml", b"<a>some content</a>");
    item.crc32 ^= 1;
    let bytes = Archive::new(vec![item]).build();
    let archive = open_zip(&bytes).unwrap();
    assert_eq!(
        archive.read(0),
        Err(Error::Zip(ZipError::ChecksumMismatch { entry: 0 }))
    );
    assert_eq!(
        archive.verify(0),
        Err(Error::Zip(ZipError::ChecksumMismatch { entry: 0 }))
    );
    assert_eq!(
        archive.raw_entry(0),
        Err(Error::Zip(ZipError::ChecksumMismatch { entry: 0 }))
    );

    // A DEFLATE block of the reserved type 3.
    let mut item = Item::deflated("a.xml", b"<a/>");
    item.stored = vec![0x07, 0x00, 0x00];
    let bytes = Archive::new(vec![item]).build();
    let archive = open_zip(&bytes).unwrap();
    assert_eq!(
        archive.read(0),
        Err(Error::Zip(ZipError::CorruptData { entry: 0 }))
    );
    assert_eq!(
        archive.verify(0),
        Err(Error::Zip(ZipError::CorruptData { entry: 0 }))
    );

    // A stream with bytes after its end, where other data could hide.
    let mut item = Item::deflated("a.xml", b"<a/>");
    item.stored.extend_from_slice(b"hidden");
    let bytes = Archive::new(vec![item]).build();
    let archive = open_zip(&bytes).unwrap();
    assert_eq!(
        archive.read(0),
        Err(Error::Zip(ZipError::CorruptData { entry: 0 }))
    );
    assert_eq!(
        archive.verify(0),
        Err(Error::Zip(ZipError::CorruptData { entry: 0 }))
    );

    // A stream cut short.
    let mut item = Item::deflated("a.xml", &b"<a>content</a>".repeat(100));
    item.stored.truncate(item.stored.len() / 2);
    let bytes = Archive::new(vec![item]).build();
    let archive = open_zip(&bytes).unwrap();
    assert_eq!(
        archive.read(0),
        Err(Error::Zip(ZipError::CorruptData { entry: 0 }))
    );
    assert_eq!(
        archive.verify(0),
        Err(Error::Zip(ZipError::CorruptData { entry: 0 }))
    );

    // Stored data whose size disagrees with the uncompressed size.
    let mut item = Item::stored("a.bin", b"abc");
    item.size = 4;
    assert_eq!(
        zip_error(&Archive::new(vec![item]).build()),
        ZipError::SizeMismatch { entry: 0 }
    );
}

#[test]
fn refuses_references_to_data_before_the_start() {
    // DEFLATE repeats earlier output with (length, distance) pairs, and a distance that reaches back before the first byte is an error (zlib: "invalid distance too far back"). A decoder that does not check it repeats whatever its buffer holds there, so these headers declare the size and checksum of exactly that, and only the check itself can refuse the data: a copy from 1 byte back as the very first symbol, and the literal `a` followed by a copy from 2 bytes back. The fuzz target zip_archive found the second kind in a damaged document: `read` refused it and `verify`, which then decompressed through a 32 KiB ring buffer, did not.
    let streams = [
        (
            craft::fixed_huffman(&[Symbol::Match {
                length: 3,
                distance: 1,
            }]),
            vec![0, 0, 0],
        ),
        (
            craft::fixed_huffman(&[
                Symbol::Literal(b'a'),
                Symbol::Match {
                    length: 3,
                    distance: 2,
                },
            ]),
            b"a\0a\0".to_vec(),
        ),
    ];
    // The builder's streams, which zlib refuses with that message.
    assert_eq!(streams[0].0, [0x03, 0x02, 0x00]);
    assert_eq!(streams[1].0, [0x4B, 0x04, 0x42, 0x00]);
    for (stream, unchecked) in streams {
        let mut item = Item::stored("a.bin", &unchecked);
        item.method = 8;
        item.stored = stream;
        let bytes = Archive::new(vec![item]).build();
        let archive = open_zip(&bytes).unwrap();
        assert_eq!(
            archive.read(0),
            Err(Error::Zip(ZipError::CorruptData { entry: 0 }))
        );
        assert_eq!(
            archive.verify(0),
            Err(Error::Zip(ZipError::CorruptData { entry: 0 }))
        );
        assert_eq!(
            archive.raw_entry(0),
            Err(Error::Zip(ZipError::CorruptData { entry: 0 }))
        );
    }
    // The same `a` followed by a copy from 1 byte back, which repeats it, is correct (and zlib agrees).
    let stream = craft::fixed_huffman(&[
        Symbol::Literal(b'a'),
        Symbol::Match {
            length: 3,
            distance: 1,
        },
    ]);
    assert_eq!(stream, [0x4B, 0x04, 0x02, 0x00]);
    let mut item = Item::stored("a.bin", b"aaaa");
    item.method = 8;
    item.stored = stream;
    let bytes = Archive::new(vec![item]).build();
    let archive = open_zip(&bytes).unwrap();
    assert_eq!(archive.read(0).unwrap(), b"aaaa");
    assert_eq!(archive.verify(0), Ok(()));
}

#[test]
fn reads_and_verifies_copies_from_the_longest_distance() {
    // 32 KiB of noise as literals, then copies of 258 bytes from 32 KiB back, the longest distance DEFLATE allows, until the data passes 300 KiB. `verify` moves its window several times on the way, and every copy must still find the byte from exactly 32 KiB earlier.
    let mut data = craft::noise(32 * 1024);
    let mut symbols: Vec<Symbol> = data.iter().map(|&byte| Symbol::Literal(byte)).collect();
    while data.len() < 300 * 1024 {
        symbols.push(Symbol::Match {
            length: 258,
            distance: 32_768,
        });
        for _ in 0..258 {
            data.push(data[data.len() - 32 * 1024]);
        }
    }
    let mut item = Item::stored("far.bin", &data);
    item.method = 8;
    item.stored = craft::fixed_huffman(&symbols);
    let bytes = Archive::new(vec![item.clone()]).build();
    let archive = open_zip(&bytes).unwrap();
    assert_eq!(archive.read(0).unwrap(), data);
    assert_eq!(archive.verify(0), Ok(()));

    // A declared size one byte off either way is refused by both.
    for size in [data.len() - 1, data.len() + 1] {
        let mut wrong = item.clone();
        wrong.size = u64::try_from(size).unwrap();
        let bytes = Archive::new(vec![wrong]).build();
        let archive = open_zip(&bytes).unwrap();
        assert_eq!(
            archive.read(0),
            Err(Error::Zip(ZipError::SizeMismatch { entry: 0 }))
        );
        assert_eq!(
            archive.verify(0),
            Err(Error::Zip(ZipError::SizeMismatch { entry: 0 }))
        );
    }
}

#[test]
fn reads_verifies_and_copies_damaged_entries_alike() {
    // A large entry compressed as usual, damaged in 24 places one at a time: `read`, `verify` and `raw_entry` give the same answer for each.
    let item = Item::deflated("text.txt", &craft::text(300 * 1024));
    let length = item.stored.len();
    for step in 0..24 {
        let mut damaged = item.clone();
        let position = step * length / 24 + step;
        damaged.stored[position] ^= 1 << (step % 8);
        let bytes = Archive::new(vec![damaged]).build();
        let archive = open_zip(&bytes).unwrap();
        let verified = archive.verify(0);
        assert_eq!(
            archive.read(0).map(|_| ()),
            verified,
            "damage at {position}"
        );
        assert_eq!(
            archive.raw_entry(0).map(|_| ()),
            verified,
            "damage at {position}"
        );
    }
}

// Headers that disagree.

#[test]
fn refuses_local_headers_that_disagree_with_the_central_directory() {
    let mut item = Item::stored("word/document.xml", b"x");
    item.local_name = Some("word/evil.xml".to_owned());
    assert_eq!(
        zip_error(&Archive::new(vec![item]).build()),
        ZipError::HeaderMismatch { entry: 0 }
    );
    let mut item = Item::stored("a.xml", b"x");
    item.local_method = Some(8);
    assert_eq!(
        zip_error(&Archive::new(vec![item]).build()),
        ZipError::HeaderMismatch { entry: 0 }
    );
    let mut item = Item::stored("a.xml", b"x");
    item.local_flags = Some(1 << 3);
    assert_eq!(
        zip_error(&Archive::new(vec![item]).build()),
        ZipError::HeaderMismatch { entry: 0 }
    );
    let mut item = Item::stored("a.xml", b"x");
    item.local_crc32 = Some(0x1234_5678);
    assert_eq!(
        zip_error(&Archive::new(vec![item]).build()),
        ZipError::HeaderMismatch { entry: 0 }
    );
    let mut item = Item::deflated("a.xml", b"<a/>");
    item.descriptor = Some(Descriptor {
        signature: true,
        wide: false,
    });
    item.descriptor_crc32 = Some(0);
    assert_eq!(
        zip_error(&Archive::new(vec![item]).build()),
        ZipError::HeaderMismatch { entry: 0 }
    );
}

// The end of the archive.

#[test]
fn refuses_inputs_without_an_end_record() {
    assert_eq!(zip_error(b""), ZipError::NotZip);
    assert_eq!(zip_error(b"PK\x03\x04 not really a zip"), ZipError::NotZip);
    let bytes = Archive::new(package_items()).build();
    assert_eq!(zip_error(&bytes[..bytes.len() - 1]), ZipError::NotZip);
    // A comment that does not reach the end of the input.
    let mut archive = Archive::new(package_items());
    archive.comment = b"comment".to_vec();
    let mut bytes = archive.build();
    bytes.push(b'!');
    assert_eq!(zip_error(&bytes), ZipError::NotZip);
}

#[test]
fn refuses_an_ambiguous_end() {
    // The comment holds a second end record that also ends exactly at the end of the input: readers that search differently would see different archives.
    let mut fake = vec![0x50, 0x4B, 0x05, 0x06];
    fake.extend_from_slice(&[0; 16]);
    fake.extend_from_slice(&0_u16.to_le_bytes());
    let mut archive = Archive::new(package_items());
    archive.comment = fake;
    assert_eq!(zip_error(&archive.build()), ZipError::AmbiguousEnd);
}

#[test]
fn refuses_split_archives() {
    let mut archive = Archive::new(package_items());
    archive.disk = 1;
    assert_eq!(zip_error(&archive.build()), ZipError::MultipleDisks);
}

#[test]
fn refuses_a_truncated_or_shifted_central_directory() {
    let bytes = Archive::new(package_items()).build();
    // Junk before the archive shifts every offset; offsets are never guessed.
    let mut shifted = b"MZ junk".to_vec();
    shifted.extend_from_slice(&bytes);
    assert!(matches!(
        open_zip(&shifted),
        Err(Error::Zip(
            ZipError::MalformedDirectory | ZipError::MalformedEntry { .. }
        ))
    ));
    // An archive cut in the middle of its central directory.
    let central = bytes
        .windows(4)
        .position(|window| window == [0x50, 0x4B, 0x01, 0x02])
        .unwrap();
    let mut cut = bytes[..central + 20].to_vec();
    cut.extend_from_slice(&bytes[bytes.len() - 22..]);
    assert!(matches!(open_zip(&cut), Err(Error::Zip(_))));
}

// Package rules.

#[test]
fn refuses_packages_that_break_the_packaging_rules() {
    let mut items = package_items();
    items.remove(0);
    assert_eq!(
        package_error(&Archive::new(items).build()),
        Error::Package(PackageError::MissingContentTypes)
    );

    let mut items = package_items();
    items.push(Item::stored("word/document.xml/[0].piece", b"<a"));
    assert_eq!(
        package_error(&Archive::new(items).build()),
        Error::Package(PackageError::InterleavedPart { entry: 2 })
    );

    let mut items = package_items();
    items.push(Item::stored("Word/Document.XML/extra.xml", b"x"));
    assert_eq!(
        package_error(&Archive::new(items).build()),
        Error::Package(PackageError::DerivablePartName { entry: 2, other: 1 })
    );
}

#[test]
fn refuses_entity_expansion_in_metadata() {
    let billion_laughs = b"<?xml version=\"1.0\"?><!DOCTYPE lolz [<!ENTITY lol \"lol\"><!ENTITY lol9 \"&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;\">]><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">&lol9;</Types>";
    let items = vec![Item::deflated("[Content_Types].xml", billion_laughs)];
    match package_error(&Archive::new(items).build()) {
        Error::Xml {
            entry: Some(0),
            error,
        } => assert_eq!(error.kind, XmlErrorKind::DoctypeNotAllowed),
        other => panic!("{other:?}"),
    }
}

#[test]
fn opens_a_long_namespace_on_many_elements_cheaply() {
    // An extension element in the package's relationships part binds a 32 KiB namespace name, then holds 32,768 empty elements in that namespace: under 1 KB compressed. Every element refers to the one stored copy of the name; when each element had its own copy, opening this package needed 1 GiB (the review of pull request 17), and a part just under 1 MiB could ask for tens of GiB.
    let mut rels = format!(
        "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><x:e xmlns:x=\"{}\">",
        "u".repeat(32 * 1024)
    );
    rels.push_str(&"<x:b/>".repeat(32 * 1024));
    rels.push_str("</x:e></Relationships>");
    let mut items = package_items();
    items.push(Item::deflated("_rels/.rels", rels.as_bytes()));
    let bytes = Archive::new(items).build();
    assert!(bytes.len() < 2_000, "{}", bytes.len());
    let package = Package::open(&bytes, &Limits::default()).unwrap();
    let relationships = package
        .relationships(&bayan_opc::RelationshipSource::Package)
        .unwrap();
    assert!(relationships.is_empty());
}

// Many entries: every lookup and duplicate check in the metadata takes logarithmic time, so a stream with a hundred thousand entries opens in a moment instead of minutes (the review of pull request 17 measured 34 to 78 s, in release builds, for streams of 160 to 250 KB). The streams are stored uncompressed, to keep the tests quick.

const CONTENT_TYPES_NAMESPACE: &str =
    "http://schemas.openxmlformats.org/package/2006/content-types";

#[test]
fn opens_content_types_with_many_overrides_quickly() {
    let mut types = format!(
        "<Types xmlns=\"{CONTENT_TYPES_NAMESPACE}\"><Default Extension=\"xml\" ContentType=\"application/xml\"/>"
    );
    for index in 0..100_000 {
        types.push_str(&format!(
            "<Override PartName=\"/p{index}\" ContentType=\"a/b\"/>"
        ));
    }
    types.push_str("</Types>");
    let mut items = vec![Item::stored("[Content_Types].xml", types.as_bytes())];
    // Many parts too, each looked up in the overrides.
    for index in (0..100_000).step_by(20) {
        items.push(Item::stored(&format!("p{index}"), b"x"));
    }
    let bytes = Archive::new(items).build();
    let package = Package::open(&bytes, &Limits::default()).unwrap();
    assert_eq!(package.parts().count(), 5_000);
    for part in package.parts() {
        assert_eq!(package.content_type(part), Some("a/b"));
    }
    // A duplicate at the very end is still found.
    let duplicate = types.replace(
        "</Types>",
        "<Override PartName=\"/P99999\" ContentType=\"a/c\"/></Types>",
    );
    let bytes = Archive::new(vec![Item::stored(
        "[Content_Types].xml",
        duplicate.as_bytes(),
    )])
    .build();
    assert_eq!(
        package_error(&bytes),
        Error::Package(PackageError::ContentTypes(
            bayan_opc::ContentTypesError::DuplicateOverride
        ))
    );
}

#[test]
fn opens_content_types_with_many_defaults_quickly() {
    let mut types = format!("<Types xmlns=\"{CONTENT_TYPES_NAMESPACE}\">");
    for index in 0..60_000 {
        types.push_str(&format!(
            "<Default Extension=\"e{index}\" ContentType=\"a/b\"/>"
        ));
    }
    types.push_str("</Types>");
    let items = vec![
        Item::stored("[Content_Types].xml", types.as_bytes()),
        Item::stored("a.e59999", b"x"),
    ];
    let bytes = Archive::new(items).build();
    let package = Package::open(&bytes, &Limits::default()).unwrap();
    let part = bayan_opc::PartName::new("/a.e59999").unwrap();
    assert_eq!(package.content_type(&part), Some("a/b"));
    let duplicate = types.replace(
        "</Types>",
        "<Default Extension=\"E59999\" ContentType=\"a/c\"/></Types>",
    );
    let bytes = Archive::new(vec![Item::stored(
        "[Content_Types].xml",
        duplicate.as_bytes(),
    )])
    .build();
    assert_eq!(
        package_error(&bytes),
        Error::Package(PackageError::ContentTypes(
            bayan_opc::ContentTypesError::DuplicateDefault
        ))
    );
}

#[test]
fn finds_relationships_quickly_among_many() {
    // With the largest possible number taken, the next identifier is the first free one counting from 1, and every relationship is found by its identifier.
    let mut text = String::from(
        "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId18446744073709551615\" Type=\"t\" Target=\"a\"/>",
    );
    for index in 1..=60_000 {
        text.push_str(&format!(
            "<Relationship Id=\"rId{index}\" Type=\"t\" Target=\"a\"/>"
        ));
    }
    text.push_str("</Relationships>");
    let relationships =
        bayan_opc::Relationships::parse(text.as_bytes(), &Limits::default()).unwrap();
    assert_eq!(relationships.len(), 60_001);
    assert_eq!(relationships.next_id(), "rId60001");
    for index in 1..=60_000 {
        assert!(relationships.get(&format!("rId{index}")).is_some());
    }
    assert!(relationships.get("rId60001").is_none());
}

#[test]
fn refuses_oversized_metadata_before_decompressing_it() {
    let mut items = package_items();
    items[0].size = 1 << 40;
    items[0].zip64 = true;
    let limits = Limits {
        max_entry_size: 1 << 41,
        max_total_size: 1 << 42,
        max_compression_ratio: u64::MAX,
        ..Limits::default()
    };
    assert_eq!(
        Package::open(&Archive::new(items).build(), &limits).unwrap_err(),
        Error::Limit(LimitError::MetadataTooLarge {
            entry: Some(0),
            limit: 32 << 20
        })
    );
}

#[test]
fn refuses_deeply_nested_metadata() {
    let deep = format!(
        "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">{}",
        "<a>".repeat(10_000)
    );
    let items = vec![Item::deflated("[Content_Types].xml", deep.as_bytes())];
    match package_error(&Archive::new(items).build()) {
        Error::Xml {
            entry: Some(0),
            error,
        } => assert_eq!(error.kind, XmlErrorKind::TooDeep),
        other => panic!("{other:?}"),
    }
}

#[test]
fn refuses_xml_base_in_relationships() {
    let mut items = package_items();
    items.push(Item::deflated(
        "_rels/.rels",
        b"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\" xml:base=\"http://attacker.example/\"><Relationship Id=\"rId1\" Type=\"t\" Target=\"word/document.xml\"/></Relationships>",
    ));
    assert_eq!(
        package_error(&Archive::new(items).build()),
        Error::Package(PackageError::Relationships {
            entry: Some(2),
            error: RelationshipsError::XmlBase
        })
    );
}

#[test]
fn refuses_relationships_of_relationships_parts() {
    let mut items = package_items();
    items.push(Item::deflated(
        "_rels/.rels",
        b"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"/>",
    ));
    items.push(Item::deflated(
        "_rels/_rels/.rels.rels",
        b"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"/>",
    ));
    assert_eq!(
        package_error(&Archive::new(items).build()),
        Error::Package(PackageError::Relationships {
            entry: Some(3),
            error: RelationshipsError::RelationshipsOfRelationshipsPart
        })
    );
}

#[test]
fn ignores_entries_that_are_not_parts() {
    // Names that the part name grammar does not allow are not parts (ECMA-376 Part 2 §7.2.5.5): they are left out, never guessed at.
    let mut items = package_items();
    items.push(Item::stored("[trash]/0000.dat", b"junk"));
    items.push(Item::stored("word/ends-with-dot.", b"junk"));
    items.push(Item::stored("word/a%2Fb.xml", b"junk"));
    let bytes = Archive::new(items).build();
    let package = Package::open(&bytes, &Limits::default()).unwrap();
    let names: Vec<&str> = package.parts().map(bayan_opc::PartName::as_str).collect();
    assert_eq!(names, ["/word/document.xml"]);
}

/// A small, independent ZIP builder for the tests: it writes exactly the fields it is told to, including wrong ones.
mod craft {
    /// CRC-32, computed bit by bit, independently of the crate's table.
    pub fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0_u32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    pub fn deflate(data: &[u8]) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec(data, 6)
    }

    /// A symbol of a DEFLATE stream: a byte, or a copy of `length` bytes from `distance` bytes back.
    #[derive(Clone, Copy, Debug)]
    pub enum Symbol {
        Literal(u8),
        Match { length: u16, distance: u16 },
    }

    /// One final DEFLATE block with the fixed Huffman codes (RFC 1951, section 3.2.6), written bit by bit, independently of the crate's DEFLATE library. It refers back wherever `symbols` say, also to before the start.
    pub fn fixed_huffman(symbols: &[Symbol]) -> Vec<u8> {
        const LENGTH_BASE: [u16; 29] = [
            3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99,
            115, 131, 163, 195, 227, 258,
        ];
        const LENGTH_EXTRA: [u32; 29] = [
            0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
        ];
        const DISTANCE_BASE: [u16; 30] = [
            1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025,
            1537, 2049, 3073, 4097, 6145, 8193, 12_289, 16_385, 24_577,
        ];
        let mut bits = Bits::default();
        // The final block, with fixed codes.
        bits.value(1, 1);
        bits.value(1, 2);
        for symbol in symbols {
            match *symbol {
                Symbol::Literal(byte) => bits.literal_or_length(u32::from(byte)),
                Symbol::Match { length, distance } => {
                    let code = LENGTH_BASE
                        .iter()
                        .rposition(|&base| base <= length)
                        .unwrap();
                    bits.literal_or_length(257 + u32::try_from(code).unwrap());
                    bits.value(u32::from(length - LENGTH_BASE[code]), LENGTH_EXTRA[code]);
                    let code = DISTANCE_BASE
                        .iter()
                        .rposition(|&base| base <= distance)
                        .unwrap();
                    bits.code(u32::try_from(code).unwrap(), 5);
                    let extra = u32::try_from(code / 2).unwrap().saturating_sub(1);
                    bits.value(u32::from(distance - DISTANCE_BASE[code]), extra);
                }
            }
        }
        // The end of the block.
        bits.literal_or_length(256);
        bits.bytes
    }

    /// Bits written the way DEFLATE packs them: from the lowest bit of each byte up.
    #[derive(Default)]
    struct Bits {
        bytes: Vec<u8>,
        used: u32,
    }

    impl Bits {
        /// `count` bits of `value`, lowest first, as DEFLATE writes header fields and extra bits.
        fn value(&mut self, value: u32, count: u32) {
            for bit in 0..count {
                self.push((value >> bit) & 1);
            }
        }

        /// A Huffman code of `count` bits, highest first.
        fn code(&mut self, code: u32, count: u32) {
            for bit in (0..count).rev() {
                self.push((code >> bit) & 1);
            }
        }

        /// A literal byte, a length, or the end of the block (256), in the fixed codes.
        fn literal_or_length(&mut self, symbol: u32) {
            match symbol {
                0..=143 => self.code(0x30 + symbol, 8),
                144..=255 => self.code(0x190 + symbol - 144, 9),
                256..=279 => self.code(symbol - 256, 7),
                _ => self.code(0xC0 + symbol - 280, 8),
            }
        }

        fn push(&mut self, bit: u32) {
            if self.used == 0 {
                self.bytes.push(0);
            }
            if bit == 1 {
                *self.bytes.last_mut().unwrap() |= 1 << self.used;
            }
            self.used = (self.used + 1) % 8;
        }
    }

    /// `length` bytes of noise from a fixed seed, which DEFLATE cannot shorten.
    pub fn noise(length: usize) -> Vec<u8> {
        let mut state = 0x2545_F491_u32;
        (0..length)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state.to_le_bytes()[0]
            })
            .collect()
    }

    /// `length` bytes of text: words from a small vocabulary in an order from a fixed seed, so that DEFLATE finds copies at many distances.
    pub fn text(length: usize) -> Vec<u8> {
        const WORDS: [&str; 16] = [
            "package ",
            "part ",
            "relationship ",
            "content ",
            "type ",
            "core ",
            "property ",
            "stream ",
            "archive ",
            "entry ",
            "window ",
            "distance ",
            "length ",
            "literal ",
            "block ",
            "checksum ",
        ];
        let mut state = 0x9E37_79B9_u32;
        let mut text = Vec::new();
        while text.len() < length {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let word = WORDS[usize::from(state.to_le_bytes()[0] % 16)];
            text.extend_from_slice(word.as_bytes());
        }
        text.truncate(length);
        text
    }

    fn size_of(data: &[u8]) -> u64 {
        u64::try_from(data.len()).unwrap()
    }

    /// One entry, with every field that a test may want to get wrong.
    #[derive(Clone, Debug)]
    pub struct Item {
        pub name: String,
        /// The name's bytes, if they are not the UTF-8 of `name`.
        pub name_bytes: Option<Vec<u8>>,
        /// The data as stored.
        pub stored: Vec<u8>,
        pub method: u16,
        pub crc32: u32,
        /// The declared uncompressed size.
        pub size: u64,
        /// The declared compressed size, if not the length of `stored`.
        pub compressed_size: Option<u64>,
        pub flags: u16,
        pub made_by: u16,
        pub external: u32,
        /// Sizes and offset in ZIP64 extra fields.
        pub zip64: bool,
        pub descriptor: Option<Descriptor>,
        /// The checksum in the data descriptor, if not `crc32`.
        pub descriptor_crc32: Option<u32>,
        /// What the local header says, where it differs from the central directory.
        pub local_name: Option<String>,
        pub local_method: Option<u16>,
        pub local_flags: Option<u16>,
        pub local_crc32: Option<u32>,
        /// The name in the central directory, where it differs from `name`.
        pub central_name: Option<String>,
        /// Where the central directory says the local header is, if not where it is written.
        pub central_offset: Option<u64>,
        /// Write no local header or data at all (for entries that point into another entry).
        pub skip_local: bool,
    }

    /// How a data descriptor is written.
    #[derive(Clone, Copy, Debug)]
    pub struct Descriptor {
        pub signature: bool,
        /// 8-byte sizes.
        pub wide: bool,
    }

    impl Item {
        pub fn stored(name: &str, data: &[u8]) -> Item {
            Item {
                name: name.to_owned(),
                name_bytes: None,
                stored: data.to_vec(),
                method: 0,
                crc32: crc32(data),
                size: size_of(data),
                compressed_size: None,
                flags: 0,
                made_by: 20,
                external: 0,
                zip64: false,
                descriptor: None,
                descriptor_crc32: None,
                local_name: None,
                local_method: None,
                local_flags: None,
                local_crc32: None,
                central_name: None,
                central_offset: None,
                skip_local: false,
            }
        }

        pub fn deflated(name: &str, data: &[u8]) -> Item {
            let mut item = Item::stored(name, data);
            item.stored = deflate(data);
            item.method = 8;
            item
        }

        fn compressed(&self) -> u64 {
            self.compressed_size
                .unwrap_or_else(|| size_of(&self.stored))
        }
    }

    /// A whole archive.
    #[derive(Clone, Debug)]
    pub struct Archive {
        pub items: Vec<Item>,
        pub comment: Vec<u8>,
        /// The entry count written in the end record, if not the real one.
        pub count: Option<u16>,
        pub disk: u16,
        /// Write ZIP64 end records.
        pub zip64_end: bool,
    }

    impl Archive {
        pub fn new(items: Vec<Item>) -> Archive {
            Archive {
                items,
                comment: Vec::new(),
                count: None,
                disk: 0,
                zip64_end: false,
            }
        }

        pub fn build(&self) -> Vec<u8> {
            let mut out = Vec::new();
            let mut offsets = Vec::new();
            for item in &self.items {
                offsets.push(size_of(&out));
                if item.skip_local {
                    continue;
                }
                let name = item.local_name.as_deref().unwrap_or(&item.name);
                let name = item
                    .name_bytes
                    .clone()
                    .unwrap_or_else(|| name.as_bytes().to_vec());
                let flags = item.local_flags.unwrap_or(item.flags)
                    | if item.descriptor.is_some() { 1 << 3 } else { 0 };
                put32(&mut out, 0x0403_4B50);
                put16(&mut out, if item.zip64 { 45 } else { 20 });
                put16(&mut out, flags);
                put16(&mut out, item.local_method.unwrap_or(item.method));
                put16(&mut out, 0);
                put16(&mut out, 0x21);
                let mut extra = Vec::new();
                if item.descriptor.is_some() {
                    put32(&mut out, 0);
                    if item.zip64 {
                        put32(&mut out, 0xFFFF_FFFF);
                        put32(&mut out, 0xFFFF_FFFF);
                        put16(&mut extra, 1);
                        put16(&mut extra, 16);
                        put64(&mut extra, 0);
                        put64(&mut extra, 0);
                    } else {
                        put32(&mut out, 0);
                        put32(&mut out, 0);
                    }
                } else {
                    put32(&mut out, item.local_crc32.unwrap_or(item.crc32));
                    if item.zip64 {
                        put32(&mut out, 0xFFFF_FFFF);
                        put32(&mut out, 0xFFFF_FFFF);
                        put16(&mut extra, 1);
                        put16(&mut extra, 16);
                        put64(&mut extra, item.size);
                        put64(&mut extra, item.compressed());
                    } else {
                        put32(&mut out, low32(item.compressed()));
                        put32(&mut out, low32(item.size));
                    }
                }
                put16(&mut out, u16::try_from(name.len()).unwrap());
                put16(&mut out, u16::try_from(extra.len()).unwrap());
                out.extend_from_slice(&name);
                out.extend_from_slice(&extra);
                out.extend_from_slice(&item.stored);
                if let Some(descriptor) = item.descriptor {
                    if descriptor.signature {
                        put32(&mut out, 0x0807_4B50);
                    }
                    put32(&mut out, item.descriptor_crc32.unwrap_or(item.crc32));
                    if descriptor.wide {
                        put64(&mut out, item.compressed());
                        put64(&mut out, item.size);
                    } else {
                        put32(&mut out, low32(item.compressed()));
                        put32(&mut out, low32(item.size));
                    }
                }
            }
            let directory_offset = size_of(&out);
            for (item, &offset) in self.items.iter().zip(&offsets) {
                let offset = item.central_offset.unwrap_or(offset);
                let name = item.central_name.as_deref().unwrap_or(&item.name);
                let name = item
                    .name_bytes
                    .clone()
                    .unwrap_or_else(|| name.as_bytes().to_vec());
                let flags = item.flags | if item.descriptor.is_some() { 1 << 3 } else { 0 };
                let mut extra = Vec::new();
                if item.zip64 {
                    put16(&mut extra, 1);
                    put16(&mut extra, 24);
                    put64(&mut extra, item.size);
                    put64(&mut extra, item.compressed());
                    put64(&mut extra, offset);
                }
                put32(&mut out, 0x0201_4B50);
                put16(&mut out, item.made_by);
                put16(&mut out, if item.zip64 { 45 } else { 20 });
                put16(&mut out, flags);
                put16(&mut out, item.method);
                put16(&mut out, 0);
                put16(&mut out, 0x21);
                put32(&mut out, item.crc32);
                if item.zip64 {
                    put32(&mut out, 0xFFFF_FFFF);
                    put32(&mut out, 0xFFFF_FFFF);
                } else {
                    put32(&mut out, low32(item.compressed()));
                    put32(&mut out, low32(item.size));
                }
                put16(&mut out, u16::try_from(name.len()).unwrap());
                put16(&mut out, u16::try_from(extra.len()).unwrap());
                put16(&mut out, 0);
                put16(&mut out, 0);
                put16(&mut out, 0);
                put32(&mut out, item.external);
                put32(
                    &mut out,
                    if item.zip64 {
                        0xFFFF_FFFF
                    } else {
                        low32(offset)
                    },
                );
                out.extend_from_slice(&name);
                out.extend_from_slice(&extra);
            }
            let directory_end = size_of(&out);
            let directory_size = directory_end - directory_offset;
            let count = self
                .count
                .unwrap_or_else(|| u16::try_from(self.items.len()).unwrap());
            if self.zip64_end {
                put32(&mut out, 0x0606_4B50);
                put64(&mut out, 44);
                put16(&mut out, 45);
                put16(&mut out, 45);
                put32(&mut out, 0);
                put32(&mut out, 0);
                put64(&mut out, u64::from(count));
                put64(&mut out, u64::from(count));
                put64(&mut out, directory_size);
                put64(&mut out, directory_offset);
                put32(&mut out, 0x0706_4B50);
                put32(&mut out, 0);
                put64(&mut out, directory_end);
                put32(&mut out, 1);
            }
            put32(&mut out, 0x0605_4B50);
            put16(&mut out, self.disk);
            put16(&mut out, self.disk);
            put16(&mut out, if self.zip64_end { 0xFFFF } else { count });
            put16(&mut out, if self.zip64_end { 0xFFFF } else { count });
            put32(
                &mut out,
                if self.zip64_end {
                    0xFFFF_FFFF
                } else {
                    low32(directory_size)
                },
            );
            put32(
                &mut out,
                if self.zip64_end {
                    0xFFFF_FFFF
                } else {
                    low32(directory_offset)
                },
            );
            put16(&mut out, u16::try_from(self.comment.len()).unwrap());
            out.extend_from_slice(&self.comment);
            out
        }
    }

    /// The low 32 bits of a size the test means to write in a 32-bit field (a lying size of terabytes keeps its low bits).
    fn low32(value: u64) -> u32 {
        u32::try_from(value & 0xFFFF_FFFF).unwrap()
    }

    fn put16(out: &mut Vec<u8>, value: u16) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn put32(out: &mut Vec<u8>, value: u32) {
        out.extend_from_slice(&value.to_le_bytes());
    }

    fn put64(out: &mut Vec<u8>, value: u64) {
        out.extend_from_slice(&value.to_le_bytes());
    }
}
