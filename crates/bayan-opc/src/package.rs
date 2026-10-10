//! Packages: reading a whole package, and writing one that copies untouched parts unchanged.

use std::collections::BTreeMap;

use crate::content_types::{self, ContentTypes};
use crate::core_properties::{self, CoreProperties};
use crate::part_name::{PartName, RelationshipSource};
use crate::relationships::{self, Relationships, Target};
use crate::zip::{ZipArchive, ZipWriter};
use crate::{CorePropertiesError, Error, LimitError, Limits, PackageError, RelationshipsError};

/// A package that has been opened and checked: its ZIP container, its parts, their media types and their relationships.
///
/// Opening reads the ZIP structure ([`ZipArchive::new`]), maps entry names to part names (an entry whose name is not a valid part name is not a part, §7.2.5.5, and is ignored), refuses part names that are derivable from one another, refuses interleaved parts, and reads the content types stream and every relationships part. Other parts are decompressed only when asked for ([`Package::read_part`]). Relationship targets are resolved when asked for, so a package with a broken hyperlink still opens.
#[derive(Debug)]
pub struct Package<'a> {
    zip: ZipArchive<'a>,
    limits: Limits,
    content_types: ContentTypes,
    content_types_entry: usize,
    /// The parts, in the order of the ZIP archive.
    parts: Vec<Part>,
    /// The position in `parts` of each part, by its key.
    index: BTreeMap<String, usize>,
    /// The relationships of each source that has a relationships part.
    relationships: BTreeMap<RelationshipSource, Relationships>,
    /// What opening used of the two budgets that the metadata parts share, [`Limits::max_metadata_total_size`] and [`Limits::max_xml_nodes`], so that [`Package::core_properties`] reads within the rest.
    metadata_size: u64,
    nodes_left: usize,
}

/// A part of a package: its name and the ZIP entry that holds it.
#[derive(Clone, Debug)]
struct Part {
    name: PartName,
    entry: usize,
}

impl<'a> Package<'a> {
    /// Opens the package in `data` within `limits`.
    ///
    /// # Errors
    ///
    /// [`Error::Zip`] and [`Error::Limit`] for the reasons [`ZipArchive::new`] gives; [`PackageError::MissingContentTypes`], [`PackageError::InterleavedPart`] and [`PackageError::DerivablePartName`] for packages that break those rules; and the errors of [`ContentTypes::parse`] and [`Relationships::parse`] for the metadata parts, which also fail with [`RelationshipsError::InvalidSource`] or [`RelationshipsError::RelationshipsOfRelationshipsPart`] when a relationships part's name does not name a part that can have relationships. The metadata parts share two budgets: [`LimitError::MetadataTotalTooLarge`] if together they uncompress to more than [`Limits::max_metadata_total_size`] bytes, and [`Error::Xml`] with [`XmlErrorKind::TooManyNodes`](crate::XmlErrorKind::TooManyNodes) if together they have more than [`Limits::max_xml_nodes`] nodes.
    pub fn open(data: &'a [u8], limits: &Limits) -> Result<Self, Error> {
        let zip = ZipArchive::new(data, limits)?;
        let mut content_types_entry = None;
        let mut parts = Vec::new();
        let mut index = BTreeMap::new();
        for (entry, zip_entry) in zip.entries().iter().enumerate() {
            if zip_entry.is_folder() {
                continue;
            }
            let name = zip_entry.name();
            if is_piece(name) {
                return Err(PackageError::InterleavedPart { entry }.into());
            }
            if name.eq_ignore_ascii_case(content_types::ZIP_NAME) {
                content_types_entry = Some(entry);
                continue;
            }
            let Ok(part_name) = PartName::from_zip_name(name) else {
                continue;
            };
            // The ZIP layer already refuses names that are equivalent, so every key is new.
            index.insert(part_name.key().to_owned(), parts.len());
            parts.push(Part {
                name: part_name,
                entry,
            });
        }
        check_derivable(&parts, &index)?;

        let content_types_entry = content_types_entry.ok_or(PackageError::MissingContentTypes)?;
        // The budgets that the content types stream and every relationships part share.
        let mut metadata_size = 0;
        let mut nodes = limits.max_xml_nodes;
        let content_types = ContentTypes::parse_entry(
            &read_metadata(&zip, content_types_entry, limits, &mut metadata_size)?,
            limits,
            Some(content_types_entry),
            &mut nodes,
        )?;

        let mut relationships = BTreeMap::new();
        for part in &parts {
            let Some(source) = part.name.relationships_source() else {
                continue;
            };
            let invalid = |error| PackageError::Relationships {
                entry: Some(part.entry),
                error,
            };
            let source = source.map_err(|_| invalid(RelationshipsError::InvalidSource))?;
            if let RelationshipSource::Part(source_part) = &source
                && source_part.is_relationships_part()
            {
                return Err(invalid(RelationshipsError::RelationshipsOfRelationshipsPart).into());
            }
            let parsed = Relationships::parse_entry(
                &read_metadata(&zip, part.entry, limits, &mut metadata_size)?,
                limits,
                Some(part.entry),
                &mut nodes,
            )?;
            relationships.insert(source, parsed);
        }

        Ok(Package {
            zip,
            limits: *limits,
            content_types,
            content_types_entry,
            parts,
            index,
            relationships,
            metadata_size,
            nodes_left: nodes,
        })
    }

    /// The names of the parts, relationships parts included, in the order of the ZIP archive.
    pub fn parts(&self) -> impl Iterator<Item = &PartName> {
        self.parts.iter().map(|part| &part.name)
    }

    /// Whether the package has a part named `name` (or an equivalent name).
    pub fn contains(&self, name: &PartName) -> bool {
        self.index.contains_key(name.key())
    }

    /// The position of the ZIP entry that holds the part named `name`.
    pub fn entry_of(&self, name: &PartName) -> Option<usize> {
        self.find(name).map(|part| part.entry)
    }

    fn find(&self, name: &PartName) -> Option<&Part> {
        self.index
            .get(name.key())
            .and_then(|&position| self.parts.get(position))
    }

    /// The media type of the part named `name`, from the content types stream.
    pub fn content_type(&self, name: &PartName) -> Option<&str> {
        self.content_types.content_type(name)
    }

    /// The content types stream.
    pub fn content_types(&self) -> &ContentTypes {
        &self.content_types
    }

    /// Decompresses and returns the content of the part named `name`, checked against its size and CRC-32 checksum.
    ///
    /// # Errors
    ///
    /// [`PackageError::PartNotFound`] if there is no such part, and the errors of [`ZipArchive::read`].
    pub fn read_part(&self, name: &PartName) -> Result<Vec<u8>, Error> {
        let part = self.find(name).ok_or(PackageError::PartNotFound)?;
        self.zip.read(part.entry)
    }

    /// The relationships whose source is `source`, if it has a relationships part.
    pub fn relationships(&self, source: &RelationshipSource) -> Option<&Relationships> {
        self.relationships.get(source)
    }

    /// The core properties, from the part that the package's core properties relationship points to; `None` if there is no such relationship.
    ///
    /// # Errors
    ///
    /// [`PackageError::CoreProperties`] with [`CorePropertiesError::MultipleCoreProperties`] if there is more than one such relationship (§8.2), or with [`CorePropertiesError::MissingPart`] if it does not point to a part of the package; and the errors of [`CoreProperties::parse`]. The part shares the budgets of the metadata that opening the package parsed: [`LimitError::MetadataTotalTooLarge`] if it uncompresses to more than opening left of [`Limits::max_metadata_total_size`], and [`Error::Xml`] with [`XmlErrorKind::TooManyNodes`](crate::XmlErrorKind::TooManyNodes) if it has more XML nodes than opening left of [`Limits::max_xml_nodes`].
    pub fn core_properties(&self) -> Result<Option<CoreProperties>, Error> {
        let invalid = |error| Error::Package(PackageError::CoreProperties(error));
        let Some(relationships) = self.relationships.get(&RelationshipSource::Package) else {
            return Ok(None);
        };
        let mut found = relationships.of_type(core_properties::RELATIONSHIP_TYPE);
        let Some(relationship) = found.next() else {
            return Ok(None);
        };
        if found.next().is_some() {
            return Err(invalid(CorePropertiesError::MultipleCoreProperties));
        }
        let target = relationship
            .resolve(&RelationshipSource::Package)
            .map_err(|_| invalid(CorePropertiesError::MissingPart))?;
        let Target::Part(name) = target else {
            return Err(invalid(CorePropertiesError::MissingPart));
        };
        let part = self
            .find(&name)
            .ok_or(invalid(CorePropertiesError::MissingPart))?;
        // Read on demand, after opening, within what opening left of the two budgets that all the metadata parts of a package share, core properties included, so that opening a package and reading its core properties take no more memory together than the budgets allow.
        let mut metadata_size = self.metadata_size;
        let bytes = read_metadata(&self.zip, part.entry, &self.limits, &mut metadata_size)?;
        let mut nodes = self.nodes_left;
        CoreProperties::parse_entry(&bytes, &self.limits, Some(part.entry), &mut nodes).map(Some)
    }

    /// The ZIP container, for access to individual entries.
    pub fn zip(&self) -> &ZipArchive<'a> {
        &self.zip
    }

    /// The limits the package was opened with.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }
}

/// Reads the metadata part at `entry`, refusing it before decompression if it is larger than the limit for one metadata part, or if it would take `total`, the size of the metadata read so far, past the limit for all of them together.
fn read_metadata(
    zip: &ZipArchive<'_>,
    entry: usize,
    limits: &Limits,
    total: &mut u64,
) -> Result<Vec<u8>, Error> {
    let size = zip.entry(entry)?.uncompressed_size();
    if u64::try_from(limits.max_metadata_size).is_ok_and(|limit| size > limit) {
        return Err(LimitError::MetadataTooLarge {
            entry: Some(entry),
            limit: limits.max_metadata_size,
        }
        .into());
    }
    let sum = total.saturating_add(size);
    if u64::try_from(limits.max_metadata_total_size).is_ok_and(|limit| sum > limit) {
        return Err(LimitError::MetadataTotalTooLarge {
            entry,
            limit: limits.max_metadata_total_size,
        }
        .into());
    }
    *total = sum;
    zip.read(entry)
}

/// Whether `name` is the name of a piece of an interleaved part: its last segment is `[n].piece` or `[n].last.piece`, in any case (§7.2.5.2).
fn is_piece(name: &str) -> bool {
    let last = name
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let Some(rest) = last.strip_prefix('[') else {
        return false;
    };
    let Some((number, suffix)) = rest.split_once(']') else {
        return false;
    };
    !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && (suffix == ".piece" || suffix == ".last.piece")
}

/// Refuses a part name that is derivable from another's (§6.2.2.3): for every part, none of the names made of its leading segments may be a part.
fn check_derivable(parts: &[Part], index: &BTreeMap<String, usize>) -> Result<(), PackageError> {
    for part in parts {
        let key = part.name.key();
        for (slash, _) in key.match_indices('/').skip(1) {
            if let Some(&shorter) = key.get(..slash).and_then(|prefix| index.get(prefix))
                && let Some(other) = parts.get(shorter)
            {
                return Err(PackageError::DerivablePartName {
                    entry: part.entry,
                    other: other.entry,
                });
            }
        }
    }
    Ok(())
}

/// Writes a package deterministically, keeping everything that was not changed as it was.
///
/// Made from an opened package ([`PackageWriter::from_package`]), it starts as an exact copy of the package's parts: written without changes, every part's stored bytes, and the content types stream and relationships parts, are copied unchanged from the original, so every part is byte-identical ([`ZipWriter::add_raw`]); ZIP entries that are not parts are left out. A part that is replaced, added or removed changes only itself; the content types stream and a relationships part are written anew only when they change, and even then keep every unchanged entry as it was written ([`ContentTypes`], [`Relationships`]).
///
/// The writer does not apply the reader's [`Limits`]: what it writes can exceed them, for example a part name longer than [`Limits::max_name_length`], or a relationships part larger than [`Limits::max_metadata_size`] (about 40,000 hyperlinks at the default), and [`Package::open`] with the same limits then refuses the package. A host that writes such a package opens it again with the limits raised.
///
/// The output is deterministic: the content types stream comes first, then the parts in the order of the original package, then new parts in the order they were added, all with the ZIP writer's fixed timestamps and compression level. The same changes to the same package always give the same bytes.
#[derive(Debug)]
pub struct PackageWriter<'p, 'a> {
    source: Option<&'p Package<'a>>,
    /// The parts in the order they will be written; `None` where a part was removed.
    slots: Vec<Option<Slot>>,
    /// The slot of each part, by its key.
    index: BTreeMap<String, usize>,
    content_types: ContentTypes,
    relationships: BTreeMap<RelationshipSource, Relationships>,
}

#[derive(Debug)]
struct Slot {
    name: PartName,
    data: Data,
}

#[derive(Debug)]
enum Data {
    /// The part as it is in the source package's ZIP entry.
    Original { entry: usize },
    /// New content.
    New(Vec<u8>),
    /// A relationships part, written from the relationships of `source`; `entry` is the original's ZIP entry, if there was one.
    Relationships {
        source: RelationshipSource,
        entry: Option<usize>,
    },
}

impl Default for PackageWriter<'_, '_> {
    fn default() -> Self {
        PackageWriter::new()
    }
}

impl<'p, 'a> PackageWriter<'p, 'a> {
    /// A writer for a new, empty package.
    pub fn new() -> Self {
        PackageWriter {
            source: None,
            slots: Vec::new(),
            index: BTreeMap::new(),
            content_types: ContentTypes::new(),
            relationships: BTreeMap::new(),
        }
    }

    /// A writer that starts as an exact copy of `package`'s parts and content types stream.
    ///
    /// Only parts are copied: ZIP entries that are not parts, which [`Package::open`] ignores (folder entries, and entries whose names are not valid part names, such as `image 1.png` with an unencoded space), are left out of the package it writes.
    pub fn from_package(package: &'p Package<'a>) -> Self {
        let mut slots = Vec::with_capacity(package.parts.len());
        let mut index = BTreeMap::new();
        for part in &package.parts {
            let data = match part.name.relationships_source() {
                Some(Ok(source)) => Data::Relationships {
                    source,
                    entry: Some(part.entry),
                },
                _ => Data::Original { entry: part.entry },
            };
            index.insert(part.name.key().to_owned(), slots.len());
            slots.push(Some(Slot {
                name: part.name.clone(),
                data,
            }));
        }
        PackageWriter {
            source: Some(package),
            slots,
            index,
            content_types: package.content_types.clone(),
            relationships: package.relationships.clone(),
        }
    }

    /// Whether the package being written has a part named `name`.
    pub fn contains(&self, name: &PartName) -> bool {
        self.index.contains_key(name.key())
    }

    /// Adds a new part with the media type `content_type`, which is entered in the content types stream as §7.2.3.4 describes ([`ContentTypes::register`]).
    ///
    /// # Errors
    ///
    /// [`PackageError::RelationshipsPartName`] for the name of a relationships part (use [`PackageWriter::relationships_mut`]), [`PackageError::PartExists`] if a part with an equivalent name exists, [`PackageError::DerivableName`] if the name is derivable from an existing part's name or the other way round, and [`PackageError::InvalidContentType`] if `content_type` is not a valid media type.
    pub fn add_part(
        &mut self,
        name: &PartName,
        content_type: &str,
        data: Vec<u8>,
    ) -> Result<(), Error> {
        if name.is_relationships_part() {
            return Err(PackageError::RelationshipsPartName.into());
        }
        if self.contains(name) {
            return Err(PackageError::PartExists.into());
        }
        if self.is_derivable(name) {
            return Err(PackageError::DerivableName.into());
        }
        if !content_types::is_media_type(content_type) {
            return Err(PackageError::InvalidContentType.into());
        }
        self.content_types.register(name, content_type)?;
        self.index.insert(name.key().to_owned(), self.slots.len());
        self.slots.push(Some(Slot {
            name: name.clone(),
            data: Data::New(data),
        }));
        Ok(())
    }

    /// Replaces the content of the part named `name`.
    ///
    /// # Errors
    ///
    /// [`PackageError::PartNotFound`] if there is no such part, and [`PackageError::RelationshipsPartName`] for a relationships part.
    pub fn replace_part(&mut self, name: &PartName, data: Vec<u8>) -> Result<(), Error> {
        let slot = self.slot_mut(name)?;
        if matches!(slot.data, Data::Relationships { .. }) {
            return Err(PackageError::RelationshipsPartName.into());
        }
        slot.data = Data::New(data);
        Ok(())
    }

    /// Removes the part named `name`, its override in the content types stream, and its relationships part with that part's override. Relationships that point to it from elsewhere are left as they are; removing them is up to the caller, who knows what they mean.
    ///
    /// # Errors
    ///
    /// [`PackageError::PartNotFound`] if there is no such part, and [`PackageError::RelationshipsPartName`] for a relationships part.
    pub fn remove_part(&mut self, name: &PartName) -> Result<(), Error> {
        if matches!(self.slot_mut(name)?.data, Data::Relationships { .. }) {
            return Err(PackageError::RelationshipsPartName.into());
        }
        self.free(name);
        self.content_types.remove_override(name);
        let source = RelationshipSource::Part(name.clone());
        if self.relationships.remove(&source).is_some() {
            let relationships_part = source.relationships_part();
            self.free(&relationships_part);
            self.content_types.remove_override(&relationships_part);
        }
        Ok(())
    }

    fn slot_mut(&mut self, name: &PartName) -> Result<&mut Slot, Error> {
        self.index
            .get(name.key())
            .and_then(|&position| self.slots.get_mut(position))
            .and_then(Option::as_mut)
            .ok_or(Error::Package(PackageError::PartNotFound))
    }

    fn free(&mut self, name: &PartName) {
        if let Some(position) = self.index.remove(name.key())
            && let Some(slot) = self.slots.get_mut(position)
        {
            *slot = None;
        }
    }

    /// Whether `name` is derivable from the name of a part of the package being written, or the other way round (§6.2.2.3).
    fn is_derivable(&self, name: &PartName) -> bool {
        self.slots
            .iter()
            .flatten()
            .any(|slot| slot.name.is_derivable_from(name) || name.is_derivable_from(&slot.name))
    }

    /// The content types stream as it will be written.
    pub fn content_types(&self) -> &ContentTypes {
        &self.content_types
    }

    /// The content types stream, to change it.
    pub fn content_types_mut(&mut self) -> &mut ContentTypes {
        &mut self.content_types
    }

    /// The relationships of `source`, if it has any.
    pub fn relationships(&self, source: &RelationshipSource) -> Option<&Relationships> {
        self.relationships.get(source)
    }

    /// The relationships of `source`, to change them. A source without a relationships part gets one, written after the other parts if it ends up with at least one relationship.
    ///
    /// # Errors
    ///
    /// [`PackageError::PartNotFound`] if `source` is a part that the package does not have, [`PackageError::RelationshipsPartName`] if it is a relationships part, which cannot have relationships, and [`PackageError::DerivableName`] if the relationships part it would get has a name that is derivable from an existing part's name or the other way round.
    pub fn relationships_mut(
        &mut self,
        source: &RelationshipSource,
    ) -> Result<&mut Relationships, Error> {
        if let RelationshipSource::Part(part) = source {
            if part.is_relationships_part() {
                return Err(PackageError::RelationshipsPartName.into());
            }
            if !self.contains(part) {
                return Err(PackageError::PartNotFound.into());
            }
        }
        if !self.relationships.contains_key(source) {
            let name = source.relationships_part();
            if self.is_derivable(&name) {
                return Err(PackageError::DerivableName.into());
            }
            self.index.insert(name.key().to_owned(), self.slots.len());
            self.slots.push(Some(Slot {
                name,
                data: Data::Relationships {
                    source: source.clone(),
                    entry: None,
                },
            }));
        }
        Ok(self.relationships.entry(source.clone()).or_default())
    }

    /// Writes the package and returns its bytes.
    ///
    /// # Errors
    ///
    /// The errors of [`ZipArchive::raw_entry`] if an original part turns out to be damaged when it is verified before copying, and those of [`ZipWriter`].
    pub fn finish(mut self) -> Result<Vec<u8>, Error> {
        // Relationships parts need a media type too; Word expects the default for the `rels` extension.
        let rels_written = self.slots.iter().flatten().any(|slot| match &slot.data {
            Data::Relationships { source, entry } => {
                entry.is_some()
                    || self
                        .relationships
                        .get(source)
                        .is_some_and(|relationships| !relationships.is_empty())
            }
            _ => false,
        });
        let rels_name = RelationshipSource::Package.relationships_part();
        if rels_written && self.content_types.content_type(&rels_name).is_none() {
            self.content_types
                .set_default("rels", relationships::CONTENT_TYPE)?;
        }

        let mut zip = ZipWriter::new();
        match self.source {
            Some(source) if !self.content_types.is_modified() => {
                zip.add_raw(
                    content_types::ZIP_NAME,
                    &source.zip.raw_entry(source.content_types_entry)?,
                )?;
            }
            _ => zip.add(content_types::ZIP_NAME, &self.content_types.to_xml())?,
        }
        for slot in self.slots.iter().flatten() {
            let name = slot.name.zip_name();
            match &slot.data {
                Data::Original { entry } => {
                    let source = self.source.ok_or(PackageError::PartNotFound)?;
                    zip.add_raw(&name, &source.zip.raw_entry(*entry)?)?;
                }
                Data::New(data) => zip.add(&name, data)?,
                Data::Relationships { source, entry } => {
                    let relationships = self.relationships.get(source);
                    let modified = relationships.is_none_or(Relationships::is_modified);
                    match (self.source, entry) {
                        (Some(package), Some(entry)) if !modified => {
                            zip.add_raw(&name, &package.zip.raw_entry(*entry)?)?;
                        }
                        // A relationships part that the writer created and that has no relationships is left out.
                        (_, None) if relationships.is_none_or(Relationships::is_empty) => {}
                        _ => {
                            let xml = relationships.map(Relationships::to_xml).unwrap_or_default();
                            zip.add(&name, &xml)?;
                        }
                    }
                }
            }
        }
        zip.finish()
    }
}
