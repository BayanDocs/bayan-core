//! The hardened ZIP reader.

use std::collections::BTreeMap;

use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress_with_limit};

use super::crc32::Crc32;
use super::{
    CENTRAL_HEADER, CENTRAL_HEADER_SIZE, CompressionMethod, DATA_DESCRIPTOR, DIGITAL_SIGNATURE,
    END_OF_CENTRAL_DIRECTORY, END_OF_CENTRAL_DIRECTORY_SIZE, LOCAL_HEADER, LOCAL_HEADER_SIZE,
    ZIP64_END_OF_CENTRAL_DIRECTORY, ZIP64_END_OF_CENTRAL_DIRECTORY_SIZE, ZIP64_EXTRA,
    ZIP64_LOCATOR, ZIP64_LOCATOR_SIZE, check_name, equivalence_key, flags, u16_at, u32_at, u64_at,
};
use crate::{Error, LimitError, Limits, ZipError};

/// How far back DEFLATE may refer to earlier output: 32 KiB (RFC 1951, section 3.2.5).
const HISTORY: usize = 32 * 1024;

/// The buffer that DEFLATE data is decompressed through: the last [`HISTORY`] bytes of output, then room for new ones.
const WINDOW: usize = 4 * HISTORY;

/// The ZIP comment can be at most this long, so the end-of-central-directory record starts at most this far before the end of the archive (plus its own size).
const MAX_COMMENT: usize = 0xFFFF;

/// The Unix and macOS "version made by" hosts, whose external attributes hold a Unix file mode in their upper 16 bits.
const UNIX_HOSTS: [u8; 2] = [3, 19];

/// The MS-DOS attribute bit of a volume label.
const VOLUME_LABEL: u32 = 0x08;

/// The largest 64-bit size or offset a package may hold: Annex B keeps the high bit clear.
const MAX_64: u64 = i64::MAX.unsigned_abs();

/// An archive that has passed every structural check and every limit. Its entries can then be read, verified or copied.
#[derive(Clone, Debug)]
pub struct ZipArchive<'a> {
    data: &'a [u8],
    entries: Vec<ZipEntry>,
}

/// One entry of an archive, as its central directory header describes it, after the reader checked it against the entry's local header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipEntry {
    name: String,
    method: CompressionMethod,
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
    data_offset: u64,
    folder: bool,
}

impl ZipEntry {
    /// The entry's name, exactly as the archive spells it (packages use ASCII names; non-ASCII characters of part names are percent-encoded).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// How the entry's data is stored.
    pub fn method(&self) -> CompressionMethod {
        self.method
    }

    /// The CRC-32 checksum of the uncompressed data.
    pub fn crc32(&self) -> u32 {
        self.crc32
    }

    /// The size of the stored data, in bytes.
    pub fn compressed_size(&self) -> u64 {
        self.compressed_size
    }

    /// The size of the uncompressed data, in bytes.
    pub fn uncompressed_size(&self) -> u64 {
        self.uncompressed_size
    }

    /// Whether the entry is a folder (its name ends with `/`). Folders hold no data and are not parts.
    pub fn is_folder(&self) -> bool {
        self.folder
    }
}

/// An entry's stored data with what is needed to copy it into another archive unchanged: the compression method, the checksum and the uncompressed size. Only [`ZipArchive::raw_entry`] makes one, after checking that the data decompresses to the declared size and checksum, so a copy never carries damaged data into a new file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawEntry<'a> {
    pub(super) method: CompressionMethod,
    pub(super) crc32: u32,
    pub(super) uncompressed_size: u64,
    pub(super) data: &'a [u8],
}

impl<'a> RawEntry<'a> {
    /// How the data is stored.
    pub fn method(&self) -> CompressionMethod {
        self.method
    }

    /// The CRC-32 checksum of the uncompressed data.
    pub fn crc32(&self) -> u32 {
        self.crc32
    }

    /// The size of the uncompressed data, in bytes.
    pub fn uncompressed_size(&self) -> u64 {
        self.uncompressed_size
    }

    /// The stored data, exactly as it is in the archive.
    pub fn data(&self) -> &'a [u8] {
        self.data
    }
}

/// Where the central directory is, as the end records describe it.
struct Directory {
    /// The number of entries.
    entries: u64,
    /// The offset of the first central directory header.
    offset: u64,
    /// The offset where the central directory must end: the ZIP64 end-of-central-directory record if there is one, otherwise the end-of-central-directory record.
    end: u64,
}

/// The fields of a central directory header that the reader keeps while it checks the local header.
struct Central {
    name: String,
    name_bytes_offset: usize,
    name_length: usize,
    flags: u16,
    method: CompressionMethod,
    method_code: u16,
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
    header_offset: u64,
    folder: bool,
}

impl<'a> ZipArchive<'a> {
    /// Reads the archive in `data`, checking its whole structure and the `limits` before anything is decompressed.
    ///
    /// # Errors
    ///
    /// [`Error::Zip`] if the structure is malformed, ambiguous or uses a feature packages must not use, and [`Error::Limit`] if the archive exceeds a limit; see [`ZipError`] and [`LimitError`] for the individual reasons.
    pub fn new(data: &'a [u8], limits: &Limits) -> Result<Self, Error> {
        let directory = find_directory(data)?;
        if directory.entries > as_u64(limits.max_entries) {
            return Err(LimitError::TooManyEntries {
                limit: limits.max_entries,
            }
            .into());
        }
        let start = to_usize(directory.offset).ok_or(ZipError::MalformedDirectory)?;
        let end = to_usize(directory.end).ok_or(ZipError::MalformedDirectory)?;
        // Every central directory header takes at least 46 bytes, so a count that does not fit is a lie, and is refused before anything is allocated for it.
        let size = end.checked_sub(start).ok_or(ZipError::MalformedDirectory)?;
        if directory.entries > as_u64(size / CENTRAL_HEADER_SIZE) {
            return Err(ZipError::MalformedDirectory.into());
        }
        let count = to_usize(directory.entries).ok_or(ZipError::MalformedDirectory)?;

        let mut entries = Vec::with_capacity(count);
        // The extent of every entry in the archive (local header, data and data descriptor), to find overlaps.
        let mut extents: Vec<(u64, u64, usize)> = Vec::with_capacity(count);
        let mut names: BTreeMap<String, usize> = BTreeMap::new();
        let mut total: u64 = 0;
        let mut at = start;
        for index in 0..count {
            let central = read_central(data, at, end, index, limits)?;
            at = central.next;
            let central = central.entry;

            let key = equivalence_key(&central.name);
            if let Some(&first) = names.get(&key) {
                return Err(ZipError::DuplicateName {
                    entry: index,
                    first,
                }
                .into());
            }
            names.insert(key, index);

            check_sizes(&central, index, limits, &mut total)?;
            let (data_offset, extent_end) = read_local(data, &central, directory.offset, index)?;
            extents.push((central.header_offset, extent_end, index));
            entries.push(ZipEntry {
                name: central.name,
                method: central.method,
                crc32: central.crc32,
                compressed_size: central.compressed_size,
                uncompressed_size: central.uncompressed_size,
                data_offset,
                folder: central.folder,
            });
        }
        check_trailer(data, at, end)?;
        check_overlaps(&mut extents)?;
        // The archive as a whole may not be compressed more than the ratio either; otherwise many small entries, each below the grace size, could add up to a bomb.
        if total > limits.compression_ratio_grace
            && u128::from(total)
                > u128::from(limits.max_compression_ratio) * u128::from(as_u64(data.len()))
        {
            return Err(LimitError::CompressionRatio {
                entry: None,
                limit: limits.max_compression_ratio,
            }
            .into());
        }
        Ok(ZipArchive { data, entries })
    }

    /// The entries, in the order of the central directory.
    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }

    /// The entry at `index`.
    ///
    /// # Errors
    ///
    /// [`ZipError::NoSuchEntry`] if there is no entry at `index`.
    pub fn entry(&self, index: usize) -> Result<&ZipEntry, Error> {
        self.entries
            .get(index)
            .ok_or(Error::Zip(ZipError::NoSuchEntry))
    }

    /// The stored data of the entry at `index`, exactly as it is in the archive (compressed, for a DEFLATE entry).
    ///
    /// # Errors
    ///
    /// [`ZipError::NoSuchEntry`] if there is no entry at `index`, and [`ZipError::TooLargeForPlatform`] if the data cannot be addressed on this platform.
    pub fn raw_data(&self, index: usize) -> Result<&'a [u8], Error> {
        let entry = self.entry(index)?;
        let too_large = ZipError::TooLargeForPlatform { entry: index };
        let start = to_usize(entry.data_offset).ok_or(too_large)?;
        let length = to_usize(entry.compressed_size).ok_or(too_large)?;
        // The reader checked that the data lies inside the archive.
        start
            .checked_add(length)
            .and_then(|end| self.data.get(start..end))
            .ok_or(Error::Zip(ZipError::MalformedEntry { entry: index }))
    }

    /// Decompresses the entry at `index` and checks its size and CRC-32 checksum.
    ///
    /// The result is allocated once, at exactly the declared size, which the limits have already bounded; decompression stops with an error as soon as the data would grow beyond it.
    ///
    /// # Errors
    ///
    /// [`ZipError::CorruptData`], [`ZipError::SizeMismatch`] or [`ZipError::ChecksumMismatch`] if the data is damaged, [`ZipError::TooLargeForPlatform`] if it cannot be held in memory on this platform, and [`Error::OutOfMemory`] if the memory cannot be allocated.
    pub fn read(&self, index: usize) -> Result<Vec<u8>, Error> {
        let entry = self.entry(index)?;
        let raw = self.raw_data(index)?;
        let size = to_usize(entry.uncompressed_size)
            .ok_or(ZipError::TooLargeForPlatform { entry: index })?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(size)
            .map_err(|_| Error::OutOfMemory)?;
        match entry.method {
            CompressionMethod::Stored => {
                if raw.len() != size {
                    return Err(ZipError::SizeMismatch { entry: index }.into());
                }
                output.extend_from_slice(raw);
            }
            CompressionMethod::Deflated => {
                inflate(raw, entry.uncompressed_size, index, |piece| {
                    output.extend_from_slice(piece);
                })?;
            }
        }
        let mut crc = Crc32::new();
        crc.update(&output);
        if crc.finish() != entry.crc32 {
            return Err(ZipError::ChecksumMismatch { entry: index }.into());
        }
        Ok(output)
    }

    /// Checks that the entry at `index` decompresses to its declared size and CRC-32 checksum, without keeping the data: it is decompressed through a window of 128 KiB. It accepts and refuses exactly the entries that [`ZipArchive::read`] does, because both decompress with the same code.
    ///
    /// # Errors
    ///
    /// As for [`ZipArchive::read`], except that it never runs out of memory.
    pub fn verify(&self, index: usize) -> Result<(), Error> {
        let entry = self.entry(index)?;
        let raw = self.raw_data(index)?;
        let mut crc = Crc32::new();
        match entry.method {
            CompressionMethod::Stored => {
                if as_u64(raw.len()) != entry.uncompressed_size {
                    return Err(ZipError::SizeMismatch { entry: index }.into());
                }
                crc.update(raw);
            }
            CompressionMethod::Deflated => {
                inflate(raw, entry.uncompressed_size, index, |piece| {
                    crc.update(piece)
                })?;
            }
        }
        if crc.finish() != entry.crc32 {
            return Err(ZipError::ChecksumMismatch { entry: index }.into());
        }
        Ok(())
    }

    /// The entry at `index`, ready to be copied unchanged into another archive with [`ZipWriter::add_raw`](super::ZipWriter::add_raw). The data is verified first ([`ZipArchive::verify`]).
    ///
    /// # Errors
    ///
    /// As for [`ZipArchive::verify`].
    pub fn raw_entry(&self, index: usize) -> Result<RawEntry<'a>, Error> {
        self.verify(index)?;
        let entry = self.entry(index)?;
        Ok(RawEntry {
            method: entry.method,
            crc32: entry.crc32,
            uncompressed_size: entry.uncompressed_size,
            data: self.raw_data(index)?,
        })
    }
}

/// Finds the end-of-central-directory record, follows the ZIP64 records if there are any, and returns where the central directory is.
fn find_directory(data: &[u8]) -> Result<Directory, ZipError> {
    let end = find_end(data)?;
    let disk = u16_at(data, end + 4).ok_or(ZipError::NotZip)?;
    let directory_disk = u16_at(data, end + 6).ok_or(ZipError::NotZip)?;
    let entries_here = u16_at(data, end + 8).ok_or(ZipError::NotZip)?;
    let entries = u16_at(data, end + 10).ok_or(ZipError::NotZip)?;
    let size = u32_at(data, end + 12).ok_or(ZipError::NotZip)?;
    let offset = u32_at(data, end + 16).ok_or(ZipError::NotZip)?;

    let locator = end
        .checked_sub(ZIP64_LOCATOR_SIZE)
        .filter(|&at| u32_at(data, at) == Some(ZIP64_LOCATOR));
    let Some(locator) = locator else {
        if disk != 0 || directory_disk != 0 || entries_here != entries {
            return Err(ZipError::MultipleDisks);
        }
        let offset = u64::from(offset);
        let directory_end = offset
            .checked_add(u64::from(size))
            .ok_or(ZipError::MalformedDirectory)?;
        if directory_end != as_u64(end) {
            return Err(ZipError::MalformedDirectory);
        }
        return Ok(Directory {
            entries: u64::from(entries),
            offset,
            end: directory_end,
        });
    };

    // ZIP64: the locator sits right before the end record and points to the ZIP64 end record, which must end right where the locator starts.
    let zip64_disk = u32_at(data, locator + 4).ok_or(ZipError::MalformedDirectory)?;
    let zip64_offset = u64_at(data, locator + 8).ok_or(ZipError::MalformedDirectory)?;
    let disks = u32_at(data, locator + 16).ok_or(ZipError::MalformedDirectory)?;
    if zip64_disk != 0 || disks > 1 {
        return Err(ZipError::MultipleDisks);
    }
    let record = to_usize(zip64_offset).ok_or(ZipError::MalformedDirectory)?;
    if u32_at(data, record) != Some(ZIP64_END_OF_CENTRAL_DIRECTORY) {
        return Err(ZipError::MalformedDirectory);
    }
    let record_size = u64_at(data, record + 4).ok_or(ZipError::MalformedDirectory)?;
    let record_end = zip64_offset
        .checked_add(12)
        .and_then(|start| start.checked_add(record_size))
        .ok_or(ZipError::MalformedDirectory)?;
    if record_size < as_u64(ZIP64_END_OF_CENTRAL_DIRECTORY_SIZE - 12)
        || record_end != as_u64(locator)
    {
        return Err(ZipError::MalformedDirectory);
    }
    let version_needed = u16_at(data, record + 14).ok_or(ZipError::MalformedDirectory)?;
    // Version 6.2 of the record belongs to central directory encryption (Annex B, Table B.1: "No").
    if version_needed & 0xFF >= 62 {
        return Err(ZipError::UnsupportedDirectory);
    }
    let disk64 = u32_at(data, record + 16).ok_or(ZipError::MalformedDirectory)?;
    let directory_disk64 = u32_at(data, record + 20).ok_or(ZipError::MalformedDirectory)?;
    let entries_here64 = u64_at(data, record + 24).ok_or(ZipError::MalformedDirectory)?;
    let entries64 = u64_at(data, record + 32).ok_or(ZipError::MalformedDirectory)?;
    let size64 = u64_at(data, record + 40).ok_or(ZipError::MalformedDirectory)?;
    let offset64 = u64_at(data, record + 48).ok_or(ZipError::MalformedDirectory)?;
    if disk64 != 0 || directory_disk64 != 0 || entries_here64 != entries64 {
        return Err(ZipError::MultipleDisks);
    }
    // A field of the end record that is not the "see ZIP64" marker must agree with the ZIP64 record, so that readers that ignore ZIP64 see the same archive.
    let agrees = |value: u64, marker: u64, value64: u64| value == marker || value == value64;
    if !agrees(u64::from(disk), 0xFFFF, 0)
        || !agrees(u64::from(directory_disk), 0xFFFF, 0)
        || !agrees(u64::from(entries_here), 0xFFFF, entries64)
        || !agrees(u64::from(entries), 0xFFFF, entries64)
        || !agrees(u64::from(size), 0xFFFF_FFFF, size64)
        || !agrees(u64::from(offset), 0xFFFF_FFFF, offset64)
    {
        return Err(ZipError::MalformedDirectory);
    }
    // Annex B: 64-bit sizes and offsets keep the high bit clear.
    if size64 > MAX_64 || offset64 > MAX_64 {
        return Err(ZipError::MalformedDirectory);
    }
    let directory_end = offset64
        .checked_add(size64)
        .ok_or(ZipError::MalformedDirectory)?;
    if directory_end != zip64_offset {
        return Err(ZipError::MalformedDirectory);
    }
    Ok(Directory {
        entries: entries64,
        offset: offset64,
        end: directory_end,
    })
}

/// Finds the one end-of-central-directory record whose comment ends exactly at the end of the input. More than one such record means readers could disagree about the archive, so it is refused.
fn find_end(data: &[u8]) -> Result<usize, ZipError> {
    let last = data
        .len()
        .checked_sub(END_OF_CENTRAL_DIRECTORY_SIZE)
        .ok_or(ZipError::NotZip)?;
    let first = last.saturating_sub(MAX_COMMENT);
    let mut found = None;
    for at in (first..=last).rev() {
        if u32_at(data, at) != Some(END_OF_CENTRAL_DIRECTORY) {
            continue;
        }
        let Some(comment) = u16_at(data, at + 20) else {
            continue;
        };
        if at + END_OF_CENTRAL_DIRECTORY_SIZE + usize::from(comment) == data.len() {
            if found.is_some() {
                return Err(ZipError::AmbiguousEnd);
            }
            found = Some(at);
        }
    }
    found.ok_or(ZipError::NotZip)
}

/// A central directory header that has been read, and where the next one starts.
struct ReadCentral {
    entry: Central,
    next: usize,
}

/// Reads and checks the central directory header at `at`, which must end before `end`.
fn read_central(
    data: &[u8],
    at: usize,
    end: usize,
    index: usize,
    limits: &Limits,
) -> Result<ReadCentral, Error> {
    let malformed = ZipError::MalformedDirectory;
    let header = data
        .get(at..end)
        .filter(|header| header.len() >= CENTRAL_HEADER_SIZE)
        .ok_or(malformed)?;
    if u32_at(header, 0) != Some(CENTRAL_HEADER) {
        return Err(malformed.into());
    }
    let field16 = |offset| u16_at(header, offset).ok_or(malformed);
    let field32 = |offset| u32_at(header, offset).ok_or(malformed);
    let made_by = field16(4)?;
    let flag_bits = field16(8)?;
    let method_code = field16(10)?;
    let crc32 = field32(16)?;
    let compressed32 = field32(20)?;
    let uncompressed32 = field32(24)?;
    let name_length = usize::from(field16(28)?);
    let extra_length = usize::from(field16(30)?);
    let comment_length = usize::from(field16(32)?);
    let disk16 = field16(34)?;
    let external = field32(38)?;
    let offset32 = field32(42)?;

    let name_start = CENTRAL_HEADER_SIZE;
    let extra_start = name_start + name_length;
    let comment_start = extra_start + extra_length;
    let header_end = comment_start + comment_length;
    if header.len() < header_end {
        return Err(malformed.into());
    }
    if name_length > limits.max_name_length {
        return Err(LimitError::NameTooLong {
            entry: index,
            limit: limits.max_name_length,
        }
        .into());
    }

    if flag_bits & (flags::ENCRYPTED | flags::STRONG_ENCRYPTION | flags::MASKED) != 0 {
        return Err(ZipError::Encrypted { entry: index }.into());
    }
    if flag_bits & flags::PATCHED != 0 {
        return Err(ZipError::UnsupportedFeature { entry: index }.into());
    }
    let method = match method_code {
        0 => CompressionMethod::Stored,
        8 => CompressionMethod::Deflated,
        // Method 99 is AES encryption (WinZip's extension).
        99 => return Err(ZipError::Encrypted { entry: index }.into()),
        other => {
            return Err(ZipError::UnsupportedCompression {
                entry: index,
                method: other,
            }
            .into());
        }
    };

    let name_bytes = header.get(name_start..extra_start).ok_or(malformed)?;
    // Without the UTF-8 flag, a ZIP name is in code page 437, where non-ASCII bytes are other characters than in UTF-8: other programs would read another name, so such a name is refused. (Packages write non-ASCII characters percent-encoded, or as UTF-8 with the flag.)
    if !name_bytes.is_ascii() && flag_bits & flags::UTF8 == 0 {
        return Err(ZipError::UnsafeName { entry: index }.into());
    }
    let name = std::str::from_utf8(name_bytes)
        .map_err(|_| ZipError::UnsafeName { entry: index })?
        .to_owned();
    let folder = check_name(&name).ok_or(ZipError::UnsafeName { entry: index })?;

    // The ZIP64 extra field holds, in this order, exactly the values whose 32-bit fields are set to the marker.
    let extra = header.get(extra_start..comment_start).ok_or(malformed)?;
    let mut uncompressed_size = u64::from(uncompressed32);
    let mut compressed_size = u64::from(compressed32);
    let mut header_offset = u64::from(offset32);
    let mut disk = u32::from(disk16);
    let zip64 = find_zip64_extra(extra, index)?;
    let needs_zip64 = uncompressed32 == 0xFFFF_FFFF
        || compressed32 == 0xFFFF_FFFF
        || offset32 == 0xFFFF_FFFF
        || disk16 == 0xFFFF;
    if needs_zip64 {
        let zip64 = zip64.ok_or(ZipError::MalformedEntry { entry: index })?;
        let mut position = 0;
        let mut next64 = || -> Result<u64, ZipError> {
            let value = u64_at(zip64, position).ok_or(ZipError::MalformedEntry { entry: index })?;
            position += 8;
            Ok(value)
        };
        if uncompressed32 == 0xFFFF_FFFF {
            uncompressed_size = next64()?;
        }
        if compressed32 == 0xFFFF_FFFF {
            compressed_size = next64()?;
        }
        if offset32 == 0xFFFF_FFFF {
            header_offset = next64()?;
        }
        if disk16 == 0xFFFF {
            disk = u32_at(zip64, position).ok_or(ZipError::MalformedEntry { entry: index })?;
        }
    }
    if disk != 0 {
        return Err(ZipError::MultipleDisks.into());
    }
    // Annex B: 64-bit sizes and offsets keep the high bit clear.
    if uncompressed_size > MAX_64 || compressed_size > MAX_64 || header_offset > MAX_64 {
        return Err(ZipError::MalformedEntry { entry: index }.into());
    }

    check_file_type(made_by, external, folder, index)?;
    if folder && uncompressed_size != 0 {
        return Err(ZipError::FolderWithData { entry: index }.into());
    }

    Ok(ReadCentral {
        entry: Central {
            name,
            name_bytes_offset: at + name_start,
            name_length,
            flags: flag_bits,
            method,
            method_code,
            crc32,
            compressed_size,
            uncompressed_size,
            header_offset,
            folder,
        },
        next: at + header_end,
    })
}

/// Refuses symbolic links and other special files, as their Unix file mode or MS-DOS attributes mark them.
fn check_file_type(
    made_by: u16,
    external: u32,
    folder: bool,
    index: usize,
) -> Result<(), ZipError> {
    let [_, host] = made_by.to_le_bytes();
    if UNIX_HOSTS.contains(&host) {
        let kind = (external >> 16) & 0o170_000;
        let acceptable = match kind {
            // No type at all (some tools write none), or a regular file.
            0 | 0o100_000 => true,
            0o040_000 => folder,
            // Symbolic links, sockets, devices and pipes.
            _ => false,
        };
        if !acceptable {
            return Err(ZipError::SpecialFile { entry: index });
        }
    }
    if external & VOLUME_LABEL != 0 && !UNIX_HOSTS.contains(&host) {
        return Err(ZipError::SpecialFile { entry: index });
    }
    Ok(())
}

/// The data of the ZIP64 extra field among `extra`, if there is one. Fails if the extra fields are malformed or hold two ZIP64 fields.
fn find_zip64_extra(extra: &[u8], index: usize) -> Result<Option<&[u8]>, ZipError> {
    let mut found = None;
    let mut at = 0;
    // Some tools pad the extra fields with a few zero bytes, too few to form a field; those are ignored.
    while extra.len().saturating_sub(at) >= 4 {
        let id = u16_at(extra, at).ok_or(ZipError::MalformedEntry { entry: index })?;
        let size =
            usize::from(u16_at(extra, at + 2).ok_or(ZipError::MalformedEntry { entry: index })?);
        let body = extra
            .get(at + 4..at + 4 + size)
            .ok_or(ZipError::MalformedEntry { entry: index })?;
        if id == ZIP64_EXTRA {
            if found.is_some() {
                return Err(ZipError::MalformedEntry { entry: index });
            }
            found = Some(body);
        }
        at += 4 + size;
    }
    if extra
        .get(at..)
        .is_some_and(|rest| rest.iter().any(|&byte| byte != 0))
    {
        return Err(ZipError::MalformedEntry { entry: index });
    }
    Ok(found)
}

/// Checks the entry's sizes against the limits and adds its uncompressed size to the running total.
fn check_sizes(
    central: &Central,
    index: usize,
    limits: &Limits,
    total: &mut u64,
) -> Result<(), Error> {
    if central.uncompressed_size > limits.max_entry_size {
        return Err(LimitError::EntryTooLarge {
            entry: index,
            limit: limits.max_entry_size,
        }
        .into());
    }
    *total = total
        .checked_add(central.uncompressed_size)
        .filter(|&total| total <= limits.max_total_size)
        .ok_or(LimitError::TotalTooLarge {
            limit: limits.max_total_size,
        })?;
    if central.uncompressed_size > limits.compression_ratio_grace
        && u128::from(central.uncompressed_size)
            > u128::from(limits.max_compression_ratio) * u128::from(central.compressed_size)
    {
        return Err(LimitError::CompressionRatio {
            entry: Some(index),
            limit: limits.max_compression_ratio,
        }
        .into());
    }
    if central.method == CompressionMethod::Stored
        && central.compressed_size != central.uncompressed_size
    {
        return Err(ZipError::SizeMismatch { entry: index }.into());
    }
    Ok(())
}

/// Reads and checks the entry's local header and data descriptor against its central directory header. Returns the offset of the entry's data and the end of everything the entry occupies, which must lie before the central directory at `directory`.
fn read_local(
    data: &[u8],
    central: &Central,
    directory: u64,
    index: usize,
) -> Result<(u64, u64), Error> {
    let malformed = ZipError::MalformedEntry { entry: index };
    let mismatch = ZipError::HeaderMismatch { entry: index };
    let start = to_usize(central.header_offset).ok_or(malformed)?;
    let header = data
        .get(start..)
        .filter(|header| header.len() >= LOCAL_HEADER_SIZE)
        .ok_or(malformed)?;
    if u32_at(header, 0) != Some(LOCAL_HEADER) {
        return Err(malformed.into());
    }
    let field16 = |offset| u16_at(header, offset).ok_or(malformed);
    let field32 = |offset| u32_at(header, offset).ok_or(malformed);
    let flag_bits = field16(6)?;
    let method_code = field16(8)?;
    let crc32 = field32(14)?;
    let compressed32 = field32(18)?;
    let uncompressed32 = field32(22)?;
    let name_length = usize::from(field16(26)?);
    let extra_length = usize::from(field16(28)?);

    // The name, the compression method and the flags that change how the entry is read must be the same in both headers (Annex B.2).
    let local_name = header
        .get(LOCAL_HEADER_SIZE..LOCAL_HEADER_SIZE + name_length)
        .ok_or(malformed)?;
    let central_name = data
        .get(central.name_bytes_offset..central.name_bytes_offset + central.name_length)
        .ok_or(malformed)?;
    // For a name beyond ASCII, the flag that marks it as UTF-8 must be the same too, or a tool that reads the local header would read it in code page 437.
    let utf8_differs = !central_name.is_ascii() && (flag_bits ^ central.flags) & flags::UTF8 != 0;
    if local_name != central_name
        || method_code != central.method_code
        || flag_bits & flags::COMPARED != central.flags & flags::COMPARED
        || utf8_differs
    {
        return Err(mismatch.into());
    }

    let extra_start = LOCAL_HEADER_SIZE + name_length;
    let extra = header
        .get(extra_start..extra_start + extra_length)
        .ok_or(malformed)?;
    let zip64 = find_zip64_extra(extra, index)?;
    // In a local header, the ZIP64 extra field holds both sizes, uncompressed first.
    let mut uncompressed_size = u64::from(uncompressed32);
    let mut compressed_size = u64::from(compressed32);
    if uncompressed32 == 0xFFFF_FFFF || compressed32 == 0xFFFF_FFFF {
        let zip64 = zip64.ok_or(malformed)?;
        uncompressed_size = u64_at(zip64, 0).ok_or(malformed)?;
        compressed_size = u64_at(zip64, 8).ok_or(malformed)?;
    }
    let descriptor = flag_bits & flags::DATA_DESCRIPTOR != 0;
    if descriptor {
        // With a data descriptor, the local header's values are zero, though some tools write the real ones.
        let fits = |value: u64, expected: u64| value == 0 || value == expected;
        if !fits(u64::from(crc32), u64::from(central.crc32))
            || !fits(compressed_size, central.compressed_size)
            || !fits(uncompressed_size, central.uncompressed_size)
        {
            return Err(mismatch.into());
        }
    } else if crc32 != central.crc32
        || compressed_size != central.compressed_size
        || uncompressed_size != central.uncompressed_size
    {
        return Err(mismatch.into());
    }

    let data_offset = central
        .header_offset
        .checked_add(as_u64(extra_start + extra_length))
        .ok_or(malformed)?;
    let data_end = data_offset
        .checked_add(central.compressed_size)
        .ok_or(malformed)?;
    if data_end > directory {
        return Err(ZipError::Overlap { entry: index }.into());
    }
    let mut end = data_end;
    if descriptor {
        let at = to_usize(data_end).ok_or(malformed)?;
        let length = read_descriptor(data, at, central, zip64.is_some(), index)?;
        end = data_end.checked_add(as_u64(length)).ok_or(malformed)?;
        if end > directory {
            return Err(ZipError::Overlap { entry: index }.into());
        }
    }
    Ok((data_offset, end))
}

/// Reads the data descriptor at `at` and checks it against the central directory header. The descriptor may start with a signature or not, and holds 8-byte sizes when the local header has a ZIP64 extra field (APPNOTE 4.3.9), though some tools write 8-byte sizes without one or the other way round, so both widths are tried, the expected one first. Whichever reading is taken, its values equal the central directory's. Returns the descriptor's length.
fn read_descriptor(
    data: &[u8],
    at: usize,
    central: &Central,
    zip64: bool,
    index: usize,
) -> Result<usize, ZipError> {
    let widths: [usize; 2] = if zip64 { [8, 4] } else { [4, 8] };
    let read_size = |offset: usize, width: usize| -> Option<u64> {
        if width == 8 {
            u64_at(data, offset)
        } else {
            u32_at(data, offset).map(u64::from)
        }
    };
    let matches = |start: usize, width: usize| -> bool {
        u32_at(data, start) == Some(central.crc32)
            && read_size(start + 4, width) == Some(central.compressed_size)
            && read_size(start + 4 + width, width) == Some(central.uncompressed_size)
    };
    for width in widths {
        // A checksum can equal the signature's value, so the readings with and without a signature are both tried.
        if u32_at(data, at) == Some(DATA_DESCRIPTOR) && matches(at + 4, width) {
            return Ok(4 + 4 + 2 * width);
        }
        if matches(at, width) {
            return Ok(4 + 2 * width);
        }
    }
    Err(ZipError::HeaderMismatch { entry: index })
}

/// Checks what follows the last central directory header: nothing, or one digital signature record that ends exactly where the central directory must end.
fn check_trailer(data: &[u8], at: usize, end: usize) -> Result<(), ZipError> {
    if at == end {
        return Ok(());
    }
    if u32_at(data, at) == Some(DIGITAL_SIGNATURE)
        && let Some(size) = u16_at(data, at + 4)
        && at + 6 + usize::from(size) == end
    {
        return Ok(());
    }
    Err(ZipError::MalformedDirectory)
}

/// Checks that no two entries occupy the same bytes. `extents` holds each entry's start, end and position in the central directory.
fn check_overlaps(extents: &mut [(u64, u64, usize)]) -> Result<(), ZipError> {
    extents.sort_unstable();
    for pair in extents.windows(2) {
        if let [(_, previous_end, _), (start, _, index)] = pair
            && start < previous_end
        {
            return Err(ZipError::Overlap { entry: *index });
        }
    }
    Ok(())
}

/// Decompresses `input`, handing the output to `sink` piece by piece, and checks that it comes to exactly `expected` bytes and that the stream ends exactly at the end of `input`.
///
/// [`ZipArchive::read`] and [`ZipArchive::verify`] both decompress with this function, so they accept and refuse exactly the same data; they differ only in what `sink` does with the output. The decoder writes into a buffer of [`WINDOW`] bytes in miniz_oxide's non-wrapping mode, which refuses a back-reference to before the start of the buffer. Until the buffer is first full, that is the start of the output, so a stream that refers back before its first byte is refused; when the buffer is full, this function keeps its last [`HISTORY`] bytes, as far back as DEFLATE may refer, and continues after them, so from then on no valid reference reaches before the start of the buffer. (miniz_oxide's ring-buffer mode does not check such references at all and repeats whatever its buffer holds there; `tests/malicious.rs` has streams of the kind the fuzz target found.) No call may write more than what is still missing from `expected`, so the decoder stops at exactly the point where it would stop when writing into a buffer of the declared size.
fn inflate(
    input: &[u8],
    expected: u64,
    index: usize,
    mut sink: impl FnMut(&[u8]),
) -> Result<(), ZipError> {
    // Some tools store an empty entry as DEFLATE with no data at all instead of an empty DEFLATE stream.
    if input.is_empty() && expected == 0 {
        return Ok(());
    }
    let mut state = Box::<DecompressorOxide>::default();
    let mut window = vec![0_u8; WINDOW];
    // Where the next output goes; the bytes before it are the output so far, or its last `HISTORY` bytes once the window has moved.
    let mut position = 0;
    let mut input_position = 0;
    let mut remaining = expected;
    loop {
        let space = WINDOW - position;
        let room = to_usize(remaining).map_or(space, |remaining| remaining.min(space));
        let rest = input.get(input_position..).unwrap_or_default();
        let (status, consumed, written) = decompress_with_limit(
            &mut state,
            rest,
            &mut window,
            position,
            room,
            TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
        );
        remaining = remaining
            .checked_sub(as_u64(written))
            .ok_or(ZipError::SizeMismatch { entry: index })?;
        let end = position + written;
        sink(
            window
                .get(position..end)
                .ok_or(ZipError::CorruptData { entry: index })?,
        );
        input_position += consumed;
        position = end;
        match status {
            TINFLStatus::Done if input_position == input.len() => {
                return if remaining == 0 {
                    Ok(())
                } else {
                    Err(ZipError::SizeMismatch { entry: index })
                };
            }
            // The stream goes on after the declared size.
            TINFLStatus::HasMoreOutput if remaining == 0 => {
                return Err(ZipError::SizeMismatch { entry: index });
            }
            // The window is full: keep the last `HISTORY` bytes and continue after them.
            TINFLStatus::HasMoreOutput if position == WINDOW => {
                window.copy_within(WINDOW - HISTORY.., 0);
                position = HISTORY;
            }
            // Extra bytes after the end of the stream, a stream that ends too early, or damaged data.
            _ => return Err(ZipError::CorruptData { entry: index }),
        }
    }
}

/// `value` as a `u64`. `usize` is at most 64 bits wide on every platform Rust supports, so this never fails.
fn as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// `value` as a `usize`, if it fits on this platform.
fn to_usize(value: u64) -> Option<usize> {
    usize::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hands_on_no_more_than_the_declared_size() {
        // 8 MiB of zeros under a declared size of 100 bytes: the decoder stops after exactly 100 bytes, so `read` never grows its output beyond the memory it set aside.
        let stream = miniz_oxide::deflate::compress_to_vec(&vec![0_u8; 8 << 20], 6);
        let mut received = 0;
        assert_eq!(
            inflate(&stream, 100, 0, |piece| received += piece.len()),
            Err(ZipError::SizeMismatch { entry: 0 })
        );
        assert_eq!(received, 100);
    }
}
