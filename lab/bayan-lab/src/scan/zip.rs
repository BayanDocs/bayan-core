//! A minimal, strict reader for the ZIP archives that hold OPC packages (ECMA-376 Part 2 §8; PKWARE's APPNOTE 6.3).
//!
//! **Marked for replacement** by `bayan-opc` (CORE-005), the engine's hardened package layer, once it exists. Until then the Fidelity Lab needs its own reader, and this one reads only what scanning needs: the central directory, stored and DEFLATE entries, and ZIP64 sizes.
//!
//! Every archive is hostile until proven otherwise, so the reader is strict where a lenient reader could be tricked:
//!
//! - the end-of-central-directory record must end the file exactly, and only one may: a second record hidden in the comment of the first, or appended data, cannot change what the archive contains;
//! - every entry's local header must repeat its central-directory name and compression method, and no two entries may overlap each other or the central directory (the "overlapping files" zip bomb);
//! - names must be relative, without `..`, `.`, empty segments, backslashes or control characters, and unique when compared as OPC compares part names (ASCII case-insensitively);
//! - encrypted entries, symbolic links, archives spanning several disks and compression methods other than stored and DEFLATE are refused;
//! - [`ZipLimits`] caps the number of entries, the length of names, each entry's declared size and the total of all declared sizes before anything is decompressed, and decompression stops as soon as an entry produces more than it declared; every entry's CRC-32 is checked.

use std::collections::BTreeMap;
use std::fmt;

use super::crc32::crc32;

const END_OF_CENTRAL_DIRECTORY: u32 = 0x0605_4b50;
const ZIP64_END_LOCATOR: u32 = 0x0706_4b50;
const ZIP64_END_OF_CENTRAL_DIRECTORY: u32 = 0x0606_4b50;
const CENTRAL_FILE_HEADER: u32 = 0x0201_4b50;
const LOCAL_FILE_HEADER: u32 = 0x0403_4b50;

/// The size of the end-of-central-directory record without its comment.
const END_RECORD_SIZE: usize = 22;
/// The largest comment the end-of-central-directory record can carry.
const MAX_COMMENT: usize = 0xFFFF;
/// The size of a central-directory file header without its variable fields.
const CENTRAL_HEADER_SIZE: usize = 46;
/// The size of a local file header without its variable fields.
const LOCAL_HEADER_SIZE: usize = 30;

/// Limits for reading an archive, checked before anything is decompressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZipLimits {
    /// The most entries an archive may have.
    pub max_entries: usize,
    /// The longest entry name, in bytes.
    pub max_name_len: usize,
    /// The largest declared uncompressed size of one entry, in bytes.
    pub max_entry_size: u64,
    /// The largest total of all entries' declared uncompressed sizes, in bytes.
    pub max_total_size: u64,
}

/// Why an archive cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZipError {
    /// No end-of-central-directory record ends the file: not a ZIP archive, or data was appended to one.
    NotZip,
    /// The archive spans several disks.
    MultiDisk,
    /// A structure is malformed or points outside the file; the text says which.
    Corrupt(&'static str),
    /// More entries than [`ZipLimits::max_entries`].
    TooManyEntries,
    /// An entry name longer than [`ZipLimits::max_name_len`].
    NameTooLong,
    /// An entry name that is absolute, climbs out with `..`, or contains characters a part name may not.
    BadName,
    /// Two entries whose names are equal when compared as OPC compares part names.
    DuplicateName,
    /// An encrypted entry.
    Encrypted,
    /// A symbolic link.
    SymbolicLink,
    /// A compression method other than stored (0) and DEFLATE (8).
    UnsupportedCompression(u16),
    /// An entry that declares more than [`ZipLimits::max_entry_size`].
    EntryTooLarge,
    /// Entries that together declare more than [`ZipLimits::max_total_size`].
    TotalTooLarge,
    /// Entries that overlap each other or the central directory.
    Overlap,
    /// An entry that decompresses to a size other than the one it declares.
    SizeMismatch,
    /// An entry whose CRC-32 does not match its content.
    CrcMismatch,
    /// Invalid DEFLATE data.
    Inflate,
}

impl fmt::Display for ZipError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotZip => formatter.write_str("not a ZIP archive (no end-of-central-directory record ends the file)"),
            Self::MultiDisk => formatter.write_str("the ZIP archive spans several disks"),
            Self::Corrupt(what) => write!(formatter, "corrupt ZIP archive: {what}"),
            Self::TooManyEntries => formatter.write_str("the ZIP archive has more entries than the limit"),
            Self::NameTooLong => formatter.write_str("a ZIP entry name is longer than the limit"),
            Self::BadName => formatter.write_str("a ZIP entry name is absolute, climbs out of the archive, or contains characters a part name may not"),
            Self::DuplicateName => formatter.write_str("two ZIP entries have the same part name"),
            Self::Encrypted => formatter.write_str("a ZIP entry is encrypted"),
            Self::SymbolicLink => formatter.write_str("a ZIP entry is a symbolic link"),
            Self::UnsupportedCompression(method) => write!(formatter, "a ZIP entry uses compression method {method}; only stored (0) and DEFLATE (8) are supported"),
            Self::EntryTooLarge => formatter.write_str("a ZIP entry declares a size above the limit"),
            Self::TotalTooLarge => formatter.write_str("the ZIP entries together declare a size above the limit"),
            Self::Overlap => formatter.write_str("ZIP entries overlap each other or the central directory"),
            Self::SizeMismatch => formatter.write_str("a ZIP entry decompresses to a size other than the one it declares"),
            Self::CrcMismatch => formatter.write_str("a ZIP entry's CRC-32 does not match its content"),
            Self::Inflate => formatter.write_str("a ZIP entry holds invalid DEFLATE data"),
        }
    }
}

impl std::error::Error for ZipError {}

/// How an entry is compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Stored,
    Deflate,
}

/// An entry of the central directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    name: String,
    method: Method,
    crc32: u32,
    compressed_size: usize,
    uncompressed_size: usize,
    /// The offset of the entry's compressed data in the archive, set once its local header is checked.
    data_start: usize,
}

impl Entry {
    /// The entry's name as stored, such as `word/document.xml`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The declared uncompressed size, in bytes.
    #[must_use]
    pub const fn size(&self) -> usize {
        self.uncompressed_size
    }

    /// Whether the entry is a folder (its name ends with `/`); OPC packages need none, but some writers add them.
    #[must_use]
    pub fn is_folder(&self) -> bool {
        self.name.ends_with('/')
    }
}

/// A ZIP archive whose central directory and local headers have been checked.
#[derive(Debug)]
pub struct Archive<'a> {
    data: &'a [u8],
    entries: Vec<Entry>,
    /// Entry index by name in ASCII lower case, the way OPC compares part names.
    by_name: BTreeMap<String, usize>,
}

impl<'a> Archive<'a> {
    /// Reads and checks the central directory and every local header of `data`.
    ///
    /// # Errors
    ///
    /// Any [`ZipError`] except those that only decompression finds ([`ZipError::SizeMismatch`], [`ZipError::CrcMismatch`] and [`ZipError::Inflate`]).
    pub fn open(data: &'a [u8], limits: &ZipLimits) -> Result<Self, ZipError> {
        let end = find_end_record(data)?;
        let directory = read_directory_location(data, end, limits)?;
        let mut entries = read_central_directory(data, &directory, limits)?;
        check_local_headers_and_overlaps(data, &mut entries, directory.offset)?;
        let mut by_name = BTreeMap::new();
        for (index, entry) in entries.iter().enumerate() {
            if by_name
                .insert(entry.entry.name.to_ascii_lowercase(), index)
                .is_some()
            {
                return Err(ZipError::DuplicateName);
            }
        }
        Ok(Self {
            data,
            entries: entries.into_iter().map(|parsed| parsed.entry).collect(),
            by_name,
        })
    }

    /// The entries, in central-directory order.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter()
    }

    /// The entry with this name, compared as OPC compares part names (ASCII case-insensitively).
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .and_then(|&index| self.entries.get(index))
    }

    /// The decompressed content of `entry`, which must belong to this archive.
    ///
    /// # Errors
    ///
    /// [`ZipError::SizeMismatch`], [`ZipError::CrcMismatch`] or [`ZipError::Inflate`] when the entry's data does not match its header, and [`ZipError::Corrupt`] if `entry` belongs to another archive.
    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>, ZipError> {
        let compressed = entry
            .data_start
            .checked_add(entry.compressed_size)
            .and_then(|end| self.data.get(entry.data_start..end))
            .ok_or(ZipError::Corrupt("entry data outside the archive"))?;
        let content = match entry.method {
            Method::Stored => compressed.to_vec(),
            Method::Deflate => miniz_oxide::inflate::decompress_to_vec_with_limit(
                compressed,
                entry.uncompressed_size,
            )
            .map_err(|error| match error.status {
                miniz_oxide::inflate::TINFLStatus::HasMoreOutput => ZipError::SizeMismatch,
                _ => ZipError::Inflate,
            })?,
        };
        if content.len() != entry.uncompressed_size {
            return Err(ZipError::SizeMismatch);
        }
        if crc32(&content) != entry.crc32 {
            return Err(ZipError::CrcMismatch);
        }
        Ok(content)
    }
}

/// Where the central directory is, and how many entries it says it holds.
struct DirectoryLocation {
    offset: usize,
    size: usize,
    entries: usize,
    /// Where the records after the central directory start; the directory must end before it.
    end: usize,
}

/// An entry with the parts of its central-directory header that are checked against its local header.
struct ParsedEntry {
    entry: Entry,
    raw_name: Vec<u8>,
    raw_method: u16,
    local_header: usize,
}

fn u16_at(data: &[u8], at: usize) -> Result<u16, ZipError> {
    at.checked_add(2)
        .and_then(|end| data.get(at..end))
        .and_then(|bytes| bytes.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or(ZipError::Corrupt(
            "a header extends past the end of the file",
        ))
}

fn u32_at(data: &[u8], at: usize) -> Result<u32, ZipError> {
    at.checked_add(4)
        .and_then(|end| data.get(at..end))
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(ZipError::Corrupt(
            "a header extends past the end of the file",
        ))
}

fn u64_at(data: &[u8], at: usize) -> Result<u64, ZipError> {
    at.checked_add(8)
        .and_then(|end| data.get(at..end))
        .and_then(|bytes| bytes.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(ZipError::Corrupt(
            "a header extends past the end of the file",
        ))
}

/// `start..start + len` of `data`, if it lies inside.
fn slice_at(data: &[u8], start: usize, len: usize) -> Result<&[u8], ZipError> {
    start
        .checked_add(len)
        .and_then(|end| data.get(start..end))
        .ok_or(ZipError::Corrupt(
            "a field extends past the end of the file",
        ))
}

fn to_usize(value: u64) -> Result<usize, ZipError> {
    usize::try_from(value).map_err(|_| ZipError::Corrupt("a size or offset does not fit in memory"))
}

/// The offset of the end-of-central-directory record, which must end the file exactly (its comment included). A file in which two records both end the file is ambiguous (one hides in the other's comment, and readers disagree on which counts), so it is refused.
fn find_end_record(data: &[u8]) -> Result<usize, ZipError> {
    let last = data
        .len()
        .checked_sub(END_RECORD_SIZE)
        .ok_or(ZipError::NotZip)?;
    let first = last.saturating_sub(MAX_COMMENT);
    let mut found = None;
    for at in (first..=last).rev() {
        if u32_at(data, at)? == END_OF_CENTRAL_DIRECTORY {
            let comment = usize::from(u16_at(data, at + 20)?);
            if at + END_RECORD_SIZE + comment == data.len() {
                if found.is_some() {
                    return Err(ZipError::Corrupt(
                        "two end-of-central-directory records end the file",
                    ));
                }
                found = Some(at);
            }
        }
    }
    found.ok_or(ZipError::NotZip)
}

fn read_directory_location(
    data: &[u8],
    end: usize,
    limits: &ZipLimits,
) -> Result<DirectoryLocation, ZipError> {
    let disk = u16_at(data, end + 4)?;
    let directory_disk = u16_at(data, end + 6)?;
    let entries_on_disk = u16_at(data, end + 8)?;
    let entries = u16_at(data, end + 10)?;
    let size = u32_at(data, end + 12)?;
    let offset = u32_at(data, end + 16)?;
    let zip64 = disk == u16::MAX
        || directory_disk == u16::MAX
        || entries_on_disk == u16::MAX
        || entries == u16::MAX
        || size == u32::MAX
        || offset == u32::MAX;
    let location = if zip64 {
        read_zip64_location(data, end)?
    } else {
        if disk != 0 || directory_disk != 0 || entries_on_disk != entries {
            return Err(ZipError::MultiDisk);
        }
        DirectoryLocation {
            offset: to_usize(u64::from(offset))?,
            size: to_usize(u64::from(size))?,
            entries: usize::from(entries),
            end,
        }
    };
    if location.entries > limits.max_entries {
        return Err(ZipError::TooManyEntries);
    }
    let directory_end = location
        .offset
        .checked_add(location.size)
        .ok_or(ZipError::Corrupt("central directory size overflows"))?;
    if directory_end > location.end {
        return Err(ZipError::Corrupt(
            "the central directory extends past its end record",
        ));
    }
    // Every header takes at least 46 bytes, so a count the size cannot hold is a lie.
    if location
        .entries
        .checked_mul(CENTRAL_HEADER_SIZE)
        .is_none_or(|needed| needed > location.size)
    {
        return Err(ZipError::Corrupt(
            "the central directory is too small for its entry count",
        ));
    }
    Ok(location)
}

/// The central directory's location from the ZIP64 records, which the locator just before the end record points to.
fn read_zip64_location(data: &[u8], end: usize) -> Result<DirectoryLocation, ZipError> {
    let locator = end
        .checked_sub(20)
        .ok_or(ZipError::Corrupt("ZIP64 end locator missing"))?;
    if u32_at(data, locator)? != ZIP64_END_LOCATOR {
        return Err(ZipError::Corrupt("ZIP64 end locator missing"));
    }
    if u32_at(data, locator + 4)? != 0 || u32_at(data, locator + 16)? != 1 {
        return Err(ZipError::MultiDisk);
    }
    let record = to_usize(u64_at(data, locator + 8)?)?;
    if record
        .checked_add(56)
        .is_none_or(|record_end| record_end > locator)
    {
        return Err(ZipError::Corrupt("ZIP64 end record outside the archive"));
    }
    if u32_at(data, record)? != ZIP64_END_OF_CENTRAL_DIRECTORY {
        return Err(ZipError::Corrupt("ZIP64 end record missing"));
    }
    if u32_at(data, record + 16)? != 0 || u32_at(data, record + 20)? != 0 {
        return Err(ZipError::MultiDisk);
    }
    let entries_on_disk = u64_at(data, record + 24)?;
    let entries = u64_at(data, record + 32)?;
    if entries_on_disk != entries {
        return Err(ZipError::MultiDisk);
    }
    Ok(DirectoryLocation {
        entries: usize::try_from(entries).map_err(|_| ZipError::TooManyEntries)?,
        size: to_usize(u64_at(data, record + 40)?)?,
        offset: to_usize(u64_at(data, record + 48)?)?,
        end: record,
    })
}

fn read_central_directory(
    data: &[u8],
    directory: &DirectoryLocation,
    limits: &ZipLimits,
) -> Result<Vec<ParsedEntry>, ZipError> {
    let mut entries = Vec::with_capacity(directory.entries);
    let mut at = directory.offset;
    let mut total: u64 = 0;
    for _ in 0..directory.entries {
        if u32_at(data, at)? != CENTRAL_FILE_HEADER {
            return Err(ZipError::Corrupt("central directory header signature"));
        }
        let made_by = u16_at(data, at + 4)?;
        let flags = u16_at(data, at + 8)?;
        let raw_method = u16_at(data, at + 10)?;
        let crc = u32_at(data, at + 16)?;
        let mut compressed = u64::from(u32_at(data, at + 20)?);
        let mut uncompressed = u64::from(u32_at(data, at + 24)?);
        let name_len = usize::from(u16_at(data, at + 28)?);
        let extra_len = usize::from(u16_at(data, at + 30)?);
        let comment_len = usize::from(u16_at(data, at + 32)?);
        let mut disk = u32::from(u16_at(data, at + 34)?);
        let external = u32_at(data, at + 38)?;
        let mut local_header = u64::from(u32_at(data, at + 42)?);
        if name_len > limits.max_name_len {
            return Err(ZipError::NameTooLong);
        }
        let raw_name = slice_at(data, at + CENTRAL_HEADER_SIZE, name_len)?;
        let extra = slice_at(data, at + CENTRAL_HEADER_SIZE + name_len, extra_len)?;
        apply_zip64_extra(
            extra,
            &mut uncompressed,
            &mut compressed,
            &mut local_header,
            &mut disk,
        )?;
        if disk != 0 {
            return Err(ZipError::MultiDisk);
        }
        // Bit 0: encrypted; bit 6: strong encryption. Method 99 is WinZip's AES.
        if flags & 0x0041 != 0 || raw_method == 99 {
            return Err(ZipError::Encrypted);
        }
        // A link made on Unix: the high half of the external attributes holds the file mode.
        if made_by >> 8 == 3 && (external >> 16) & 0o170_000 == 0o120_000 {
            return Err(ZipError::SymbolicLink);
        }
        let method = match raw_method {
            0 => Method::Stored,
            8 => Method::Deflate,
            other => return Err(ZipError::UnsupportedCompression(other)),
        };
        if method == Method::Stored && compressed != uncompressed {
            return Err(ZipError::Corrupt("a stored entry's sizes differ"));
        }
        if uncompressed > limits.max_entry_size {
            return Err(ZipError::EntryTooLarge);
        }
        total = total
            .checked_add(uncompressed)
            .ok_or(ZipError::TotalTooLarge)?;
        if total > limits.max_total_size {
            return Err(ZipError::TotalTooLarge);
        }
        let name = std::str::from_utf8(raw_name)
            .map_err(|_| ZipError::BadName)?
            .to_owned();
        check_name(&name)?;
        if name.ends_with('/') && uncompressed != 0 {
            return Err(ZipError::BadName);
        }
        entries.push(ParsedEntry {
            entry: Entry {
                name,
                method,
                crc32: crc,
                compressed_size: to_usize(compressed)?,
                uncompressed_size: to_usize(uncompressed)?,
                data_start: 0,
            },
            raw_name: raw_name.to_vec(),
            raw_method,
            local_header: to_usize(local_header)?,
        });
        at = at
            .checked_add(CENTRAL_HEADER_SIZE + name_len + extra_len + comment_len)
            .ok_or(ZipError::Corrupt("central directory overflows"))?;
    }
    if at != directory.offset + directory.size {
        return Err(ZipError::Corrupt(
            "the central directory's size does not match its entries",
        ));
    }
    Ok(entries)
}

/// Replaces the saturated 32-bit fields of a central-directory header by their values from the ZIP64 extra field (header ID 1), which lists them in a fixed order.
fn apply_zip64_extra(
    extra: &[u8],
    uncompressed: &mut u64,
    compressed: &mut u64,
    local_header: &mut u64,
    disk: &mut u32,
) -> Result<(), ZipError> {
    let mut at = 0;
    while at < extra.len() {
        let id = u16_at(extra, at)?;
        let len = usize::from(u16_at(extra, at + 2)?);
        let field = slice_at(extra, at + 4, len)?;
        if id == 1 {
            let mut cursor = 0;
            let mut next = || -> Result<u64, ZipError> {
                let value = u64_at(field, cursor)
                    .map_err(|_| ZipError::Corrupt("ZIP64 extra field too short"))?;
                cursor += 8;
                Ok(value)
            };
            if *uncompressed == u64::from(u32::MAX) {
                *uncompressed = next()?;
            }
            if *compressed == u64::from(u32::MAX) {
                *compressed = next()?;
            }
            if *local_header == u64::from(u32::MAX) {
                *local_header = next()?;
            }
            if *disk == u32::from(u16::MAX) {
                *disk = u32_at(field, cursor)
                    .map_err(|_| ZipError::Corrupt("ZIP64 extra field too short"))?;
            }
        }
        at += 4 + len;
    }
    Ok(())
}

/// Checks that a name is a relative path that stays inside the archive: no leading `/`, no drive letter, no `.` or `..` segment, no empty segment (except the trailing one of a folder), no backslash and no control character.
fn check_name(name: &str) -> Result<(), ZipError> {
    if name.is_empty()
        || name.starts_with('/')
        || name
            .chars()
            .any(|character| character == '\\' || character.is_control())
    {
        return Err(ZipError::BadName);
    }
    let mut bytes = name.bytes();
    if let (Some(letter), Some(b':')) = (bytes.next(), bytes.next())
        && letter.is_ascii_alphabetic()
    {
        return Err(ZipError::BadName);
    }
    let path = name.strip_suffix('/').unwrap_or(name);
    if path
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(ZipError::BadName);
    }
    Ok(())
}

/// Checks every local header against its central-directory header, records where each entry's data starts, and checks that no two entries overlap each other or the central directory.
fn check_local_headers_and_overlaps(
    data: &[u8],
    entries: &mut [ParsedEntry],
    directory_offset: usize,
) -> Result<(), ZipError> {
    let mut spans = Vec::with_capacity(entries.len());
    for parsed in entries.iter_mut() {
        let at = parsed.local_header;
        if u32_at(data, at)? != LOCAL_FILE_HEADER {
            return Err(ZipError::Corrupt("local header signature"));
        }
        if u16_at(data, at + 8)? != parsed.raw_method {
            return Err(ZipError::Corrupt(
                "local header's compression method differs from the central directory",
            ));
        }
        let name_len = usize::from(u16_at(data, at + 26)?);
        let extra_len = usize::from(u16_at(data, at + 28)?);
        if slice_at(data, at + LOCAL_HEADER_SIZE, name_len)? != parsed.raw_name.as_slice() {
            return Err(ZipError::Corrupt(
                "local header's name differs from the central directory",
            ));
        }
        let data_start = at
            .checked_add(LOCAL_HEADER_SIZE + name_len + extra_len)
            .ok_or(ZipError::Corrupt("local header overflows"))?;
        let data_end = data_start
            .checked_add(parsed.entry.compressed_size)
            .ok_or(ZipError::Corrupt("entry size overflows"))?;
        if data_end > directory_offset {
            return Err(ZipError::Overlap);
        }
        parsed.entry.data_start = data_start;
        spans.push((at, data_end));
    }
    spans.sort_unstable();
    if spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(ZipError::Overlap);
    }
    Ok(())
}
