//! The deterministic ZIP writer.

use std::collections::BTreeSet;

use super::{
    CENTRAL_HEADER, CompressionMethod, END_OF_CENTRAL_DIRECTORY, LOCAL_HEADER, RawEntry,
    ZIP64_END_OF_CENTRAL_DIRECTORY, ZIP64_EXTRA, ZIP64_LOCATOR, check_name, crc32, equivalence_key,
    flags,
};
use crate::{Error, ZipError};

/// The DEFLATE level the writer always uses: zlib's default, a good balance of size and speed. Changing it changes the bytes of every part BayanDocs writes (but never of copied parts).
const DEFLATE_LEVEL: u8 = 6;

/// The modification date every entry gets: 1980-01-01, the earliest an MS-DOS date can hold, with the time 00:00:00.
const DOS_DATE: u16 = (1 << 5) | 1;

/// The value of a 32-bit size or offset field that sends the reader to the ZIP64 extra field, and the size at which the writer starts using it.
const ZIP64_MARKER: u32 = 0xFFFF_FFFF;

/// The largest entry count the end-of-central-directory record can hold without ZIP64; `0xFFFF` itself is the marker.
const MAX_ENTRIES_16: usize = 0xFFFE;

/// Writes a ZIP archive deterministically: the same calls with the same data always give the same bytes, on every platform.
///
/// Entries are written in the order they are added. Every entry gets the same timestamp, no extra fields (except ZIP64 information where sizes or offsets need it), no comment and no file attributes, and new data is compressed with DEFLATE at a fixed level. The archive is built in memory and returned by [`ZipWriter::finish`].
#[derive(Debug)]
pub struct ZipWriter {
    output: Vec<u8>,
    records: Vec<Record>,
    names: BTreeSet<String>,
    /// The size or offset from which ZIP64 fields are written. Only the tests lower it, to exercise ZIP64 without gigabytes of data.
    zip64_from: u64,
}

/// What the central directory needs to know about an entry that has been written.
#[derive(Debug)]
struct Record {
    name: String,
    method: CompressionMethod,
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
    offset: u64,
}

impl Default for ZipWriter {
    fn default() -> Self {
        ZipWriter::new()
    }
}

impl ZipWriter {
    /// An empty archive.
    pub fn new() -> Self {
        ZipWriter {
            output: Vec::new(),
            records: Vec::new(),
            names: BTreeSet::new(),
            zip64_from: u64::from(ZIP64_MARKER),
        }
    }

    /// Adds an entry named `name` with `data`, compressed with DEFLATE, or stored as it is if compression would not make it smaller (as for images, which are compressed already).
    ///
    /// # Errors
    ///
    /// [`ZipError::InvalidName`] if `name` is not a safe file name (see [`ZipError::UnsafeName`]), [`ZipError::DuplicateNameWritten`] if an entry with an equivalent name was added before, and [`ZipError::ArchiveTooLarge`] if the archive would become too large.
    pub fn add(&mut self, name: &str, data: &[u8]) -> Result<(), Error> {
        let compressed = miniz_oxide::deflate::compress_to_vec(data, DEFLATE_LEVEL);
        if compressed.len() < data.len() {
            self.add_with(name, data, CompressionMethod::Deflated, &compressed)
        } else {
            self.add_with(name, data, CompressionMethod::Stored, data)
        }
    }

    /// Adds an entry named `name` with `data`, stored as it is.
    ///
    /// # Errors
    ///
    /// As for [`ZipWriter::add`].
    pub fn add_stored(&mut self, name: &str, data: &[u8]) -> Result<(), Error> {
        self.add_with(name, data, CompressionMethod::Stored, data)
    }

    /// Copies an entry from another archive under `name`, without decompressing it: its stored bytes, compression method and checksum are taken over unchanged.
    ///
    /// # Errors
    ///
    /// As for [`ZipWriter::add`].
    pub fn add_raw(&mut self, name: &str, raw: &RawEntry<'_>) -> Result<(), Error> {
        self.write_entry(name, raw.method, raw.crc32, raw.uncompressed_size, raw.data)
    }

    fn add_with(
        &mut self,
        name: &str,
        data: &[u8],
        method: CompressionMethod,
        stored: &[u8],
    ) -> Result<(), Error> {
        self.write_entry(name, method, crc32(data), as_u64(data.len()), stored)
    }

    fn write_entry(
        &mut self,
        name: &str,
        method: CompressionMethod,
        crc32: u32,
        uncompressed_size: u64,
        stored: &[u8],
    ) -> Result<(), Error> {
        if check_name(name) != Some(false) {
            return Err(ZipError::InvalidName.into());
        }
        let name_length = u16::try_from(name.len()).map_err(|_| ZipError::InvalidName)?;
        if !self.names.insert(equivalence_key(name)) {
            return Err(ZipError::DuplicateNameWritten.into());
        }
        let compressed_size = as_u64(stored.len());
        let offset = as_u64(self.output.len());
        let zip64 = uncompressed_size >= self.zip64_from || compressed_size >= self.zip64_from;

        let out = &mut self.output;
        put32(out, LOCAL_HEADER);
        put16(out, version_needed(method, zip64));
        put16(out, name_flags(name));
        put16(out, method.code());
        put16(out, 0);
        put16(out, DOS_DATE);
        put32(out, crc32);
        if zip64 {
            put32(out, ZIP64_MARKER);
            put32(out, ZIP64_MARKER);
        } else {
            put32(out, low32(compressed_size));
            put32(out, low32(uncompressed_size));
        }
        put16(out, name_length);
        put16(out, if zip64 { 20 } else { 0 });
        out.extend_from_slice(name.as_bytes());
        if zip64 {
            put16(out, ZIP64_EXTRA);
            put16(out, 16);
            put64(out, uncompressed_size);
            put64(out, compressed_size);
        }
        out.extend_from_slice(stored);

        self.records.push(Record {
            name: name.to_owned(),
            method,
            crc32,
            compressed_size,
            uncompressed_size,
            offset,
        });
        Ok(())
    }

    /// Writes the central directory and the end records, and returns the archive.
    ///
    /// # Errors
    ///
    /// [`ZipError::ArchiveTooLarge`] if a name is too long for its header or the archive exceeds what ZIP64 can describe.
    pub fn finish(mut self) -> Result<Vec<u8>, Error> {
        let directory_offset = as_u64(self.output.len());
        let out = &mut self.output;
        for record in &self.records {
            let zip64_sizes = record.uncompressed_size >= self.zip64_from
                || record.compressed_size >= self.zip64_from;
            let zip64_offset = record.offset >= self.zip64_from;
            let mut extra = Vec::new();
            if zip64_sizes {
                put64(&mut extra, record.uncompressed_size);
                put64(&mut extra, record.compressed_size);
            }
            if zip64_offset {
                put64(&mut extra, record.offset);
            }
            let zip64 = zip64_sizes || zip64_offset;
            let version = version_needed(record.method, zip64);
            let name_length =
                u16::try_from(record.name.len()).map_err(|_| ZipError::ArchiveTooLarge)?;
            let extra_length = if extra.is_empty() {
                0
            } else {
                u16::try_from(extra.len() + 4).map_err(|_| ZipError::ArchiveTooLarge)?
            };

            put32(out, CENTRAL_HEADER);
            // Made by: MS-DOS (0), the default for packages (Annex B, Table B.2), with the same version as needed to extract.
            put16(out, version);
            put16(out, version);
            put16(out, name_flags(&record.name));
            put16(out, record.method.code());
            put16(out, 0);
            put16(out, DOS_DATE);
            put32(out, record.crc32);
            if zip64_sizes {
                put32(out, ZIP64_MARKER);
                put32(out, ZIP64_MARKER);
            } else {
                put32(out, low32(record.compressed_size));
                put32(out, low32(record.uncompressed_size));
            }
            put16(out, name_length);
            put16(out, extra_length);
            // No comment, disk 0, no internal or external attributes.
            put16(out, 0);
            put16(out, 0);
            put16(out, 0);
            put32(out, 0);
            put32(
                out,
                if zip64_offset {
                    ZIP64_MARKER
                } else {
                    low32(record.offset)
                },
            );
            out.extend_from_slice(record.name.as_bytes());
            if !extra.is_empty() {
                put16(out, ZIP64_EXTRA);
                put16(out, extra_length - 4);
                out.extend_from_slice(&extra);
            }
        }
        let directory_end = as_u64(out.len());
        let directory_size = directory_end - directory_offset;
        let count = self.records.len();
        let zip64 = count > MAX_ENTRIES_16
            || directory_offset >= self.zip64_from
            || directory_size >= self.zip64_from;
        if directory_end > i64::MAX.unsigned_abs() {
            return Err(ZipError::ArchiveTooLarge.into());
        }
        if zip64 {
            put32(out, ZIP64_END_OF_CENTRAL_DIRECTORY);
            // The size of the rest of the record.
            put64(out, 44);
            put16(out, 45);
            put16(out, 45);
            put32(out, 0);
            put32(out, 0);
            put64(out, as_u64(count));
            put64(out, as_u64(count));
            put64(out, directory_size);
            put64(out, directory_offset);
            put32(out, ZIP64_LOCATOR);
            put32(out, 0);
            put64(out, directory_end);
            put32(out, 1);
        }
        put32(out, END_OF_CENTRAL_DIRECTORY);
        put16(out, 0);
        put16(out, 0);
        let count16 = if zip64 {
            0xFFFF
        } else {
            u16::try_from(count).unwrap_or(0xFFFF)
        };
        put16(out, count16);
        put16(out, count16);
        put32(
            out,
            if zip64 {
                ZIP64_MARKER
            } else {
                low32(directory_size)
            },
        );
        put32(
            out,
            if zip64 {
                ZIP64_MARKER
            } else {
                low32(directory_offset)
            },
        );
        // No comment (Annex B, Table B.2: "No").
        put16(out, 0);
        Ok(self.output)
    }

    /// Makes the writer use ZIP64 fields from `size` on, so that tests can check ZIP64 without gigabytes of data.
    #[cfg(test)]
    pub(crate) fn with_zip64_from(mut self, size: u64) -> Self {
        self.zip64_from = size;
        self
    }
}

/// The version needed to extract an entry (Annex B, Table B.3): 1.0 for stored data, 2.0 for DEFLATE, 4.5 with ZIP64.
fn version_needed(method: CompressionMethod, zip64: bool) -> u16 {
    match (zip64, method) {
        (true, _) => 45,
        (false, CompressionMethod::Stored) => 10,
        (false, CompressionMethod::Deflated) => 20,
    }
}

/// The general purpose flags of an entry: only the language encoding flag, for a name that is not ASCII (package part names always are).
fn name_flags(name: &str) -> u16 {
    if name.is_ascii() { 0 } else { flags::UTF8 }
}

/// The low 32 bits of `value`, which the callers have checked to be below the ZIP64 threshold.
fn low32(value: u64) -> u32 {
    let [a, b, c, d, ..] = value.to_le_bytes();
    u32::from_le_bytes([a, b, c, d])
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

/// `value` as a `u64`. `usize` is at most 64 bits wide on every platform Rust supports, so this never fails.
fn as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip::ZipArchive;
    use crate::{Limits, ZipError};

    /// Some text that DEFLATE compresses well.
    fn text() -> Vec<u8> {
        b"<w:p><w:r><w:t>BayanDocs</w:t></w:r></w:p>".repeat(50)
    }

    fn sample(writer: ZipWriter) -> Vec<u8> {
        let mut writer = writer;
        writer.add("[Content_Types].xml", &text()).unwrap();
        writer.add("word/document.xml", b"x").unwrap();
        writer
            .add_stored("docProps/thumbnail.jpeg", &[0xFF, 0xD8, 0xFF, 0xE0])
            .unwrap();
        writer.add("word/empty.xml", b"").unwrap();
        writer.finish().unwrap()
    }

    #[test]
    fn writes_what_the_reader_reads_back() {
        let bytes = sample(ZipWriter::new());
        let archive = ZipArchive::new(&bytes, &Limits::default()).unwrap();
        let names: Vec<&str> = archive.entries().iter().map(|entry| entry.name()).collect();
        assert_eq!(
            names,
            [
                "[Content_Types].xml",
                "word/document.xml",
                "docProps/thumbnail.jpeg",
                "word/empty.xml"
            ]
        );
        let methods: Vec<CompressionMethod> = archive
            .entries()
            .iter()
            .map(|entry| entry.method())
            .collect();
        // Text is compressed; a single byte, an image and nothing are not, because compressing would not make them smaller.
        assert_eq!(
            methods,
            [
                CompressionMethod::Deflated,
                CompressionMethod::Stored,
                CompressionMethod::Stored,
                CompressionMethod::Stored
            ]
        );
        assert_eq!(archive.read(0).unwrap(), text());
        assert_eq!(archive.read(1).unwrap(), b"x");
        assert_eq!(archive.read(2).unwrap(), [0xFF, 0xD8, 0xFF, 0xE0]);
        assert_eq!(archive.read(3).unwrap(), b"");
        for index in 0..4 {
            archive.verify(index).unwrap();
        }
    }

    #[test]
    fn writes_the_same_bytes_every_time_on_every_platform() {
        let bytes = sample(ZipWriter::new());
        assert_eq!(bytes, sample(ZipWriter::new()));
        // The fingerprint of the exact output, the same on every platform CI runs the tests on (Linux, Windows, macOS and WebAssembly). If it changes, the writer's output changed: that may be intended (a new compression level, a new miniz_oxide), but it must be noticed, because it changes the bytes of every part BayanDocs writes.
        assert_eq!((bytes.len(), crc32(&bytes)), (533, 0x062E_25EF));
        // The first local header: signature, version 2.0, no flags, DEFLATE, the fixed time and date (00:00, 1980-01-01), the checksum, 56 compressed and 2,100 uncompressed bytes, a name of 19 bytes and no extra field.
        assert_eq!(
            &bytes[..30],
            &[
                0x50, 0x4B, 0x03, 0x04, 20, 0, 0, 0, 8, 0, 0, 0, 0x21, 0, 0xBD, 0x7B, 0xEE, 0x25,
                56, 0, 0, 0, 0x34, 0x08, 0, 0, 19, 0, 0, 0,
            ]
        );
    }

    #[test]
    fn writes_zip64_records_when_sizes_or_offsets_need_them() {
        let bytes = sample(ZipWriter::new().with_zip64_from(0));
        let archive = ZipArchive::new(&bytes, &Limits::default()).unwrap();
        assert_eq!(archive.entries().len(), 4);
        assert_eq!(archive.read(0).unwrap(), text());
        assert_eq!(archive.read(2).unwrap(), [0xFF, 0xD8, 0xFF, 0xE0]);
        // ZIP64 end of central directory record and locator, then the end record with markers.
        let end = bytes.len() - 22;
        assert_eq!(&bytes[end..end + 4], &[0x50, 0x4B, 0x05, 0x06]);
        assert_eq!(&bytes[end - 20..end - 16], &[0x50, 0x4B, 0x06, 0x07]);
        assert_eq!(&bytes[end + 8..end + 20], &[0xFF; 12]);
        // Only sizes from a threshold use ZIP64: with it just above the text, only the text entry does.
        let bytes = sample(ZipWriter::new().with_zip64_from(1_000));
        let archive = ZipArchive::new(&bytes, &Limits::default()).unwrap();
        assert_eq!(archive.read(0).unwrap(), text());
    }

    #[test]
    fn copies_raw_entries_unchanged() {
        let original = sample(ZipWriter::new());
        let archive = ZipArchive::new(&original, &Limits::default()).unwrap();
        let mut writer = ZipWriter::new();
        for (index, entry) in archive.entries().iter().enumerate() {
            writer
                .add_raw(entry.name(), &archive.raw_entry(index).unwrap())
                .unwrap();
        }
        let copy = writer.finish().unwrap();
        assert_eq!(copy, original);
        let copied = ZipArchive::new(&copy, &Limits::default()).unwrap();
        for index in 0..archive.entries().len() {
            assert_eq!(
                copied.raw_data(index).unwrap(),
                archive.raw_data(index).unwrap()
            );
        }
    }

    #[test]
    fn refuses_unsafe_and_duplicate_names() {
        let mut writer = ZipWriter::new();
        for name in ["", "/abs", "a/../b", "a\\b", "folder/", "C:/x"] {
            assert_eq!(
                writer.add(name, b"x"),
                Err(ZipError::InvalidName.into()),
                "{name:?}"
            );
        }
        writer.add("Word/Document.xml", b"x").unwrap();
        assert_eq!(
            writer.add("word/document.XML", b"y"),
            Err(ZipError::DuplicateNameWritten.into())
        );
        assert_eq!(
            writer.add("Word/Document.xml", b"y"),
            Err(ZipError::DuplicateNameWritten.into())
        );
        writer.add("a\u{E000}.bin", b"x").unwrap();
        assert_eq!(
            writer.add("a%EE%80%80.bin", b"y"),
            Err(ZipError::DuplicateNameWritten.into())
        );
    }

    #[test]
    fn marks_non_ascii_names_as_utf8() {
        let mut writer = ZipWriter::new();
        writer.add("média.txt", b"x").unwrap();
        let bytes = writer.finish().unwrap();
        assert_eq!(u16::from_le_bytes([bytes[6], bytes[7]]), 1 << 11);
        let archive = ZipArchive::new(&bytes, &Limits::default()).unwrap();
        assert_eq!(archive.entries()[0].name(), "média.txt");
    }

    #[test]
    fn writes_an_empty_archive() {
        let bytes = ZipWriter::new().finish().unwrap();
        assert_eq!(
            bytes,
            [
                0x50, 0x4B, 0x05, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ]
        );
        assert!(
            ZipArchive::new(&bytes, &Limits::default())
                .unwrap()
                .entries()
                .is_empty()
        );
    }
}
