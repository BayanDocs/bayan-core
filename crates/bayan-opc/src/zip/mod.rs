//! The ZIP container under every package: a hardened reader and a deterministic writer, for the subset of ZIP that packages use (ECMA-376 Part 2 §7.3 and Annex B; the format itself is PKWARE's APPNOTE).
//!
//! **Reading** ([`ZipArchive`]) works on the whole archive in memory, as the engine receives it from the host. Before anything is decompressed, the reader checks the archive's structure and the [`Limits`](crate::Limits): it finds exactly one end-of-central-directory record, follows the ZIP64 records when they are present, reads every central directory header, and compares it with the entry's local header and data descriptor, so that every tool sees the same names, sizes and checksums (Annex B.2). It refuses what packages must not contain (encryption, compression methods other than stored and DEFLATE, split archives, symbolic links and other special files), names that are unsafe or ambiguous (absolute paths, drive letters, backslashes, `.` and `..` segments, duplicates, including names that differ only in the case of ASCII letters), and data that overlaps other data, a trick that lets one compressed stream appear under thousands of names. Decompression is pure Rust ([`miniz_oxide`]) into a buffer of exactly the declared size, followed by a CRC-32 check.
//!
//! **Writing** ([`ZipWriter`]) is deterministic: the same entries in the same order always give the same bytes, on every platform. Every entry gets the same timestamp (1980-01-01 00:00, the earliest a ZIP file can record, as Word writes it), no extra fields, comments or file attributes, and DEFLATE at the fixed level 6; ZIP64 records are written only when sizes or offsets need them. An entry can also be copied as it is ([`ZipWriter::add_raw`]): its compressed bytes are taken over unchanged from the original archive, which keeps untouched parts byte-identical (ADR-0018, rule 3).

mod crc32;
mod read;
mod write;

pub use read::{RawEntry, ZipArchive, ZipEntry};
pub use write::ZipWriter;

pub(crate) use crc32::crc32;

use crate::percent;

/// How an entry's data is stored. These are the only two methods packages may use (ECMA-376 Part 2 §7.3.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompressionMethod {
    /// Stored as it is (method 0).
    Stored,
    /// Compressed with DEFLATE (method 8).
    Deflated,
}

impl CompressionMethod {
    /// The method's number in ZIP headers.
    fn code(self) -> u16 {
        match self {
            CompressionMethod::Stored => 0,
            CompressionMethod::Deflated => 8,
        }
    }
}

/// The signature of a local file header.
const LOCAL_HEADER: u32 = 0x0403_4B50;
/// The signature of a central directory file header.
const CENTRAL_HEADER: u32 = 0x0201_4B50;
/// The signature of the end-of-central-directory record.
const END_OF_CENTRAL_DIRECTORY: u32 = 0x0605_4B50;
/// The signature of the ZIP64 end-of-central-directory record.
const ZIP64_END_OF_CENTRAL_DIRECTORY: u32 = 0x0606_4B50;
/// The signature of the ZIP64 end-of-central-directory locator.
const ZIP64_LOCATOR: u32 = 0x0706_4B50;
/// The optional signature of a data descriptor.
const DATA_DESCRIPTOR: u32 = 0x0807_4B50;
/// The signature of the central directory's digital signature record, which readers ignore (Annex B, Table B.1).
const DIGITAL_SIGNATURE: u32 = 0x0505_4B50;
/// The header ID of the ZIP64 extended information extra field.
const ZIP64_EXTRA: u16 = 0x0001;

/// The fixed size of a local file header.
const LOCAL_HEADER_SIZE: usize = 30;
/// The fixed size of a central directory file header.
const CENTRAL_HEADER_SIZE: usize = 46;
/// The fixed size of the end-of-central-directory record.
const END_OF_CENTRAL_DIRECTORY_SIZE: usize = 22;
/// The size of the ZIP64 end-of-central-directory locator.
const ZIP64_LOCATOR_SIZE: usize = 20;
/// The fixed size of the ZIP64 end-of-central-directory record.
const ZIP64_END_OF_CENTRAL_DIRECTORY_SIZE: usize = 56;

/// General purpose flag bits.
mod flags {
    /// The entry is encrypted.
    pub(super) const ENCRYPTED: u16 = 1 << 0;
    /// Sizes and checksum follow the data in a data descriptor.
    pub(super) const DATA_DESCRIPTOR: u16 = 1 << 3;
    /// The entry holds patch data.
    pub(super) const PATCHED: u16 = 1 << 5;
    /// Strong encryption.
    pub(super) const STRONG_ENCRYPTION: u16 = 1 << 6;
    /// The name is UTF-8 (the language encoding flag).
    pub(super) const UTF8: u16 = 1 << 11;
    /// Local header values are masked (central directory encryption).
    pub(super) const MASKED: u16 = 1 << 13;
    /// The bits whose values the local and central headers must agree on.
    pub(super) const COMPARED: u16 =
        ENCRYPTED | DATA_DESCRIPTOR | PATCHED | STRONG_ENCRYPTION | MASKED;
}

/// Checks that `name` is a safe entry name: not empty, relative, with forward slashes only, without a drive letter, control characters, or empty, `.` or `..` segments. A name that ends with `/` names a folder. Returns whether it does, or `None` for an unsafe name.
pub(crate) fn check_name(name: &str) -> Option<bool> {
    if name.starts_with('/')
        || name
            .chars()
            .any(|character| character == '\\' || character.is_control())
    {
        return None;
    }
    let (path, folder) = match name.strip_suffix('/') {
        Some(path) => (path, true),
        None => (name, false),
    };
    if path.is_empty() {
        return None;
    }
    for (index, segment) in path.split('/').enumerate() {
        if segment.is_empty() || segment == "." || segment == ".." {
            return None;
        }
        // `C:` or `C:name` would make a Windows path that ignores the folder it is extracted to.
        let mut characters = segment.chars();
        if index == 0
            && characters
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic())
            && characters.next() == Some(':')
        {
            return None;
        }
    }
    Some(folder)
}

/// The key under which two entry names are the same: the characters that part names hold as themselves percent-decoded, as [`PartName::from_zip_name`](crate::PartName::from_zip_name) decodes them (§7.3.5), then ASCII letters in lower case (the equivalence of part names, §6.2.2.3).
pub(crate) fn equivalence_key(name: &str) -> String {
    let mut key = percent::decode_ucschar(name).into_owned();
    key.make_ascii_lowercase();
    key
}

/// The little-endian `u16` at `at`, if the input is long enough.
fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    let bytes = data.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

/// The little-endian `u32` at `at`, if the input is long enough.
fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// The little-endian `u64` at `at`, if the input is long enough.
fn u64_at(data: &[u8], at: usize) -> Option<u64> {
    let bytes = data.get(at..at.checked_add(8)?)?;
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_names_and_folders() {
        assert_eq!(check_name("[Content_Types].xml"), Some(false));
        assert_eq!(check_name("word/document.xml"), Some(false));
        assert_eq!(check_name("word/_rels/document.xml.rels"), Some(false));
        assert_eq!(check_name("word/"), Some(true));
        assert_eq!(check_name("média/é.png"), Some(false));
        assert_eq!(check_name("ab:/c"), Some(false));
        assert_eq!(check_name("x/a:b"), Some(false));
        assert_eq!(check_name(".rels/x"), Some(false));
        assert_eq!(check_name("x.y/..z"), Some(false));
    }

    #[test]
    fn refuses_unsafe_names() {
        for name in [
            "",
            "/",
            "/word/document.xml",
            "//server/share",
            "word\\document.xml",
            "..\\evil",
            "../evil",
            "word/../../evil",
            "word/./document.xml",
            "./word",
            "word/..",
            "word//document.xml",
            "word//",
            "C:/Windows/evil",
            "c:evil",
            "a:b/c",
            "word/doc\0.xml",
            "word/doc\n.xml",
            "word/doc\u{7f}.xml",
            "word/doc\u{85}.xml",
        ] {
            assert_eq!(check_name(name), None, "{name:?}");
        }
    }

    #[test]
    fn compares_names_like_part_names() {
        assert_eq!(equivalence_key("Word/Document.XML"), "word/document.xml");
        assert_eq!(equivalence_key("M%C3%A9dia/x"), equivalence_key("Média/X"));
        assert_ne!(equivalence_key("%C3%A9"), equivalence_key("%C3%89"));
        assert_ne!(equivalence_key("a%41"), equivalence_key("aa"));
    }
}
