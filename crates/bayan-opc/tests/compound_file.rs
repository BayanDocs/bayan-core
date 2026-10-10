//! Reading OLE compound files through bayan-opc's small API (work package CORE-005: "only an API and tests now").
//!
//! The files are made in memory with the cfb crate's writer, so the tests read no files and also run in WebAssembly. Damaged files are made by changing bytes of a valid one.

#![cfg(test)]

use std::io::{Cursor, Write};

use bayan_opc::{CompoundEntryKind, CompoundFile, CompoundFileError, Error, LimitError, Limits};

/// A compound file shaped like a `.docm` file's VBA project: a storage with streams, one of them large enough to live in regular sectors rather than the mini stream.
fn vba_project() -> Vec<u8> {
    let mut file = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    file.create_storage("/VBA").unwrap();
    file.create_stream("/VBA/dir")
        .unwrap()
        .write_all(b"compressed dir stream")
        .unwrap();
    file.create_stream("/VBA/Module1")
        .unwrap()
        .write_all(&b"Attribute VB_Name = \"Module1\"\r\n".repeat(200))
        .unwrap();
    file.create_stream("/PROJECT")
        .unwrap()
        .write_all(b"ID=\"{00000000-0000-0000-0000-000000000000}\"")
        .unwrap();
    file.create_stream("/\u{1}CompObj")
        .unwrap()
        .write_all(&[1, 0, 254, 255])
        .unwrap();
    file.flush().unwrap();
    file.into_inner().into_inner()
}

#[test]
fn lists_entries_and_reads_streams() {
    let bytes = vba_project();
    assert!(CompoundFile::is_compound_file(&bytes));
    let mut file = CompoundFile::open(&bytes, &Limits::default()).unwrap();
    let paths: Vec<Vec<String>> = file
        .entries()
        .iter()
        .map(|entry| entry.path().to_vec())
        .collect();
    // Each storage's entries in the order compound files sort names: shorter names first, then without regard to case.
    assert_eq!(
        paths,
        [
            vec!["VBA".to_owned()],
            vec!["VBA".to_owned(), "dir".to_owned()],
            vec!["VBA".to_owned(), "Module1".to_owned()],
            vec!["PROJECT".to_owned()],
            vec!["\u{1}CompObj".to_owned()],
        ]
    );
    let module = file
        .entries()
        .iter()
        .find(|entry| entry.name() == "Module1")
        .unwrap();
    assert_eq!(module.kind(), CompoundEntryKind::Stream);
    assert_eq!(module.size(), 6_200);
    assert_eq!(file.entries()[0].kind(), CompoundEntryKind::Storage);
    assert_eq!(
        file.read_stream(&["VBA", "dir"]).unwrap(),
        b"compressed dir stream"
    );
    assert_eq!(
        file.read_stream(&["VBA", "Module1"]).unwrap(),
        b"Attribute VB_Name = \"Module1\"\r\n".repeat(200)
    );
    assert_eq!(
        file.read_stream(&["\u{1}CompObj"]).unwrap(),
        [1, 0, 254, 255]
    );
}

#[test]
fn reports_what_cannot_be_read() {
    let bytes = vba_project();
    let mut file = CompoundFile::open(&bytes, &Limits::default()).unwrap();
    assert_eq!(
        file.read_stream(&["VBA"]),
        Err(Error::CompoundFile(CompoundFileError::NotAStream))
    );
    assert_eq!(
        file.read_stream(&["missing"]),
        Err(Error::CompoundFile(CompoundFileError::NotFound))
    );
    // Names are compared exactly; the crate's own lookup would ignore case.
    assert_eq!(
        file.read_stream(&["vba", "DIR"]),
        Err(Error::CompoundFile(CompoundFileError::NotFound))
    );
    assert_eq!(
        file.read_stream(&[]),
        Err(Error::CompoundFile(CompoundFileError::NotFound))
    );
    let small = Limits {
        max_entry_size: 100,
        ..Limits::default()
    };
    let mut file = CompoundFile::open(&bytes, &small).unwrap();
    assert_eq!(
        file.read_stream(&["VBA", "Module1"]),
        Err(Error::Limit(LimitError::StreamTooLarge { limit: 100 }))
    );
    let few = Limits {
        max_compound_file_entries: 4,
        ..Limits::default()
    };
    assert_eq!(
        CompoundFile::open(&bytes, &few).unwrap_err(),
        Error::Limit(LimitError::TooManyCompoundFileEntries { limit: 4 })
    );
}

#[test]
fn refuses_what_is_not_a_compound_file() {
    assert_eq!(
        CompoundFile::open(b"PK\x03\x04", &Limits::default()).unwrap_err(),
        Error::CompoundFile(CompoundFileError::NotCompoundFile)
    );
    assert_eq!(
        CompoundFile::open(&[], &Limits::default()).unwrap_err(),
        Error::CompoundFile(CompoundFileError::NotCompoundFile)
    );
    // The signature alone, or a damaged header, is malformed.
    let mut truncated = vba_project();
    truncated.truncate(600);
    assert_eq!(
        CompoundFile::open(&truncated, &Limits::default()).unwrap_err(),
        Error::CompoundFile(CompoundFileError::Malformed)
    );
    assert_eq!(
        CompoundFile::open(&bayan_opc::compound_file::SIGNATURE, &Limits::default()).unwrap_err(),
        Error::CompoundFile(CompoundFileError::Malformed)
    );
}

/// The offset of the directory entry named `name` (UTF-16, in the directory sectors).
fn directory_entry(bytes: &[u8], name: &str) -> usize {
    let encoded: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    bytes
        .windows(encoded.len())
        .position(|window| window == encoded.as_slice())
        .unwrap()
}

/// A compound file with one stream, `ab`, holding `data`.
fn single_stream() -> Vec<u8> {
    let mut file = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    file.create_stream("/ab")
        .unwrap()
        .write_all(b"data")
        .unwrap();
    file.flush().unwrap();
    file.into_inner().into_inner()
}

/// Renames the directory entry at `at` to `name`: its UTF-16 characters, a terminating null, and the length in bytes at offset 64.
fn rename(bytes: &mut [u8], at: usize, name: &str) {
    let mut encoded: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let length = if encoded.is_empty() {
        0
    } else {
        encoded.len() + 2
    };
    encoded.resize(64, 0);
    bytes[at..at + 64].copy_from_slice(&encoded);
    bytes[at + 64..at + 66].copy_from_slice(&u16::try_from(length).unwrap().to_le_bytes());
}

#[test]
fn refuses_names_that_could_be_misread() {
    // `.` and `..` are legal names in a compound file, but a path would read them as "this storage" and "the storage above"; an empty name would vanish from a path.
    for name in [".", "..", ""] {
        let mut bytes = single_stream();
        let at = directory_entry(&bytes, "ab");
        rename(&mut bytes, at, name);
        assert_eq!(
            CompoundFile::open(&bytes, &Limits::default()).unwrap_err(),
            Error::CompoundFile(CompoundFileError::InvalidName),
            "{name:?}"
        );
    }
    // A null character inside a name.
    let mut bytes = vba_project();
    let at = directory_entry(&bytes, "PROJECT");
    bytes[at + 8] = 0;
    assert_eq!(
        CompoundFile::open(&bytes, &Limits::default()).unwrap_err(),
        Error::CompoundFile(CompoundFileError::InvalidName)
    );
    // The characters MS-CFB forbids (which a path would split or mangle) are refused too; the cfb crate already refuses them while it reads the directory.
    for forbidden in ['/', '\\', ':', '!'] {
        let mut bytes = vba_project();
        let at = directory_entry(&bytes, "PROJECT");
        bytes[at + 8] = u8::try_from(u32::from(forbidden)).unwrap();
        assert!(
            matches!(
                CompoundFile::open(&bytes, &Limits::default()),
                Err(Error::CompoundFile(
                    CompoundFileError::InvalidName | CompoundFileError::Malformed
                ))
            ),
            "{forbidden}"
        );
    }
    // The renamed file is otherwise valid: with an ordinary name it opens.
    let mut bytes = single_stream();
    let at = directory_entry(&bytes, "ab");
    rename(&mut bytes, at, "cd");
    let mut file = CompoundFile::open(&bytes, &Limits::default()).unwrap();
    assert_eq!(file.read_stream(&["cd"]).unwrap(), b"data");
}

#[test]
fn refuses_damaged_structure_without_panicking() {
    // Changing any single byte of the header or the directory gives either a valid file or an error, never a panic.
    let bytes = vba_project();
    for at in 0..bytes.len().min(4_096) {
        for value in [0x00, 0xFF, 0x7F] {
            let mut damaged = bytes.clone();
            damaged[at] = value;
            if let Ok(mut file) = CompoundFile::open(&damaged, &Limits::default()) {
                let paths: Vec<Vec<String>> = file
                    .entries()
                    .iter()
                    .map(|entry| entry.path().to_vec())
                    .collect();
                for path in paths {
                    let names: Vec<&str> = path.iter().map(String::as_str).collect();
                    let _ = file.read_stream(&names);
                }
            }
        }
    }
}
