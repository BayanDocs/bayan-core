//! Read-only access to OLE compound files (Microsoft's Compound File Binary format, [MS-CFB]): the container of legacy `.doc` files, of the VBA projects inside `.docm` files, of embedded OLE objects, and of password-encrypted Office documents.
//!
//! Only this small API is public; underneath it is the [`cfb`] crate, which may be replaced without changing callers. The API adds what the crate leaves to its users: limits, names that cannot be misread, and no platform-dependent paths. Entries are addressed by the list of names from the root, never by a file system path, because the crate turns names into a `std::path::Path`, whose separators differ between platforms, and because a hostile file can name an entry `..` or put a backslash in a name. Opening therefore refuses entries with such names and checks that looking up every entry by its names finds exactly that entry, so that the listing and the lookup can never disagree.

use std::collections::BTreeSet;
use std::io::{Cursor, Read};
use std::path::{Component, PathBuf};

use crate::{CompoundFileError, Error, LimitError, Limits};

/// The first eight bytes of every compound file.
pub const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// The characters [MS-CFB] §2.6.1 forbids in entry names.
const FORBIDDEN: [char; 5] = ['/', '\\', ':', '!', '\0'];

/// An open compound file, read-only.
pub struct CompoundFile<'a> {
    inner: cfb::CompoundFile<Cursor<&'a [u8]>>,
    entries: Vec<CompoundEntry>,
    root_class_id: [u8; 16],
    max_stream_size: u64,
    input_size: u64,
}

impl core::fmt::Debug for CompoundFile<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CompoundFile")
            .field("entries", &self.entries)
            .finish_non_exhaustive()
    }
}

/// A storage (like a folder) or a stream (like a file) of a compound file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompoundEntry {
    path: Vec<String>,
    kind: CompoundEntryKind,
    size: u64,
    class_id: [u8; 16],
}

/// Whether an entry is a storage or a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompoundEntryKind {
    /// A storage, which holds other entries.
    Storage,
    /// A stream, which holds data.
    Stream,
}

impl CompoundEntry {
    /// The names from the root to this entry, the entry's own name last.
    pub fn path(&self) -> &[String] {
        &self.path
    }

    /// The entry's own name.
    pub fn name(&self) -> &str {
        self.path.last().map_or("", String::as_str)
    }

    /// Whether this is a storage or a stream.
    pub fn kind(&self) -> CompoundEntryKind {
        self.kind
    }

    /// The size of a stream's data, in bytes (0 for a storage).
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The class identifier (CLSID) of a storage, as the 16 bytes stored in the file; all zero when it has none.
    pub fn class_id(&self) -> [u8; 16] {
        self.class_id
    }
}

impl<'a> CompoundFile<'a> {
    /// Whether `data` starts with the compound file signature, as an encrypted Office document or a `.doc` file does, where a package would start with a ZIP header.
    pub fn is_compound_file(data: &[u8]) -> bool {
        data.starts_with(&SIGNATURE)
    }

    /// Opens the compound file in `data` within `limits`, and lists its entries.
    ///
    /// # Errors
    ///
    /// [`CompoundFileError::NotCompoundFile`] if `data` does not start with the signature, [`CompoundFileError::Malformed`] if its structure is damaged, [`CompoundFileError::InvalidName`] or [`CompoundFileError::DuplicateName`] if an entry's name could be misread, and [`LimitError::TooManyCompoundFileEntries`] if it has more entries than [`Limits::max_compound_file_entries`].
    pub fn open(data: &'a [u8], limits: &Limits) -> Result<Self, Error> {
        if !CompoundFile::is_compound_file(data) {
            return Err(CompoundFileError::NotCompoundFile.into());
        }
        // The crate's messages can quote entry names, which are document content, so they are not passed on.
        let inner =
            cfb::CompoundFile::open(Cursor::new(data)).map_err(|_| CompoundFileError::Malformed)?;
        let mut entries = Vec::new();
        let mut seen = BTreeSet::new();
        for entry in inner.walk() {
            if entry.is_root() {
                continue;
            }
            if entries.len() >= limits.max_compound_file_entries {
                return Err(LimitError::TooManyCompoundFileEntries {
                    limit: limits.max_compound_file_entries,
                }
                .into());
            }
            let name = entry.name();
            if !is_valid_name(name) {
                return Err(CompoundFileError::InvalidName.into());
            }
            let mut path = Vec::new();
            for component in entry.path().components() {
                match component {
                    Component::RootDir => {}
                    Component::Normal(part) => {
                        let part = part.to_str().filter(|part| is_valid_name(part));
                        path.push(part.ok_or(CompoundFileError::InvalidName)?.to_owned());
                    }
                    _ => return Err(CompoundFileError::InvalidName.into()),
                }
            }
            if path.last().map(String::as_str) != Some(name) {
                return Err(CompoundFileError::InvalidName.into());
            }
            if !seen.insert(path.clone()) {
                return Err(CompoundFileError::DuplicateName.into());
            }
            entries.push(CompoundEntry {
                path,
                kind: if entry.is_stream() {
                    CompoundEntryKind::Stream
                } else {
                    CompoundEntryKind::Storage
                },
                size: if entry.is_stream() { entry.len() } else { 0 },
                class_id: *entry.clsid().as_bytes(),
            });
        }
        // The crate finds entries by name through an index that compares names without case; every entry must be found as itself, or two entries share a name and the listing and the lookup disagree.
        for entry in &entries {
            let found = inner
                .entry(to_path(&entry.path))
                .map_err(|_| CompoundFileError::DuplicateName)?;
            let same = found.name() == entry.name()
                && found.is_stream() == (entry.kind == CompoundEntryKind::Stream)
                && (!found.is_stream() || found.len() == entry.size)
                && *found.clsid().as_bytes() == entry.class_id;
            if !same {
                return Err(CompoundFileError::DuplicateName.into());
            }
        }
        let root_class_id = *inner.root_entry().clsid().as_bytes();
        Ok(CompoundFile {
            inner,
            entries,
            root_class_id,
            max_stream_size: limits.max_entry_size,
            input_size: u64::try_from(data.len()).unwrap_or(u64::MAX),
        })
    }

    /// The storages and streams, the root excluded, in the order the crate walks them: depth first, each storage's entries in the order of their names as [MS-CFB] sorts them.
    pub fn entries(&self) -> &[CompoundEntry] {
        &self.entries
    }

    /// The class identifier of the root storage.
    pub fn root_class_id(&self) -> [u8; 16] {
        self.root_class_id
    }

    /// Reads the stream at `path` (the names from the root, compared exactly).
    ///
    /// # Errors
    ///
    /// [`CompoundFileError::NotFound`] if there is no entry at `path`, [`CompoundFileError::NotAStream`] if it is a storage, [`LimitError::StreamTooLarge`] if it is larger than [`Limits::max_entry_size`], [`CompoundFileError::Malformed`] if its data cannot be read in full, and [`Error::OutOfMemory`] if its memory cannot be allocated.
    pub fn read_stream(&mut self, path: &[&str]) -> Result<Vec<u8>, Error> {
        let entry = self
            .entries
            .iter()
            .find(|entry| {
                entry
                    .path
                    .iter()
                    .map(String::as_str)
                    .eq(path.iter().copied())
            })
            .ok_or(CompoundFileError::NotFound)?;
        if entry.kind != CompoundEntryKind::Stream {
            return Err(CompoundFileError::NotAStream.into());
        }
        if entry.size > self.max_stream_size {
            return Err(LimitError::StreamTooLarge {
                limit: self.max_stream_size,
            }
            .into());
        }
        // A stream lives in the file's sectors, so it can never be larger than the file: a larger size is a lie, refused before any memory is set aside for it.
        if entry.size > self.input_size {
            return Err(CompoundFileError::Malformed.into());
        }
        let size = usize::try_from(entry.size).map_err(|_| CompoundFileError::Malformed)?;
        let mut data = Vec::new();
        data.try_reserve_exact(size)
            .map_err(|_| Error::OutOfMemory)?;
        let stream = self
            .inner
            .open_stream(to_path(&entry.path))
            .map_err(|_| CompoundFileError::Malformed)?;
        stream
            .take(entry.size.saturating_add(1))
            .read_to_end(&mut data)
            .map_err(|_| CompoundFileError::Malformed)?;
        if data.len() != size {
            return Err(CompoundFileError::Malformed.into());
        }
        Ok(data)
    }
}

/// Whether `name` is an entry name that cannot be misread: not empty, not `.` or `..`, and without the characters [MS-CFB] forbids (or a null character).
fn is_valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(FORBIDDEN)
}

/// The crate's path for the entry at `names`: a root and one component per name. The names have been checked, so none of them contains a separator or means anything special.
fn to_path<S: AsRef<str>>(names: &[S]) -> PathBuf {
    let mut path = PathBuf::from("/");
    for name in names {
        path.push(name.as_ref());
    }
    path
}
