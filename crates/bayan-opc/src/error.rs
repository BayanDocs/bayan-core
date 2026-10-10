//! The errors of bayan-opc.
//!
//! Every error says what is wrong in terms of the package's structure: which entry (by its position in the ZIP archive), which limit, which rule. None carries a part name, a relationship target or any other text from the package, because such text is document content, which must never reach logs, crash reports or error messages that leave the machine (`docs/AGENTS.md` §6). A caller that needs the name of an entry for a local diagnostic can look it up by its position.

use core::fmt;

/// Everything that can go wrong while reading or writing a package or a compound file.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The ZIP container is malformed, or uses a feature that packages must not use.
    Zip(ZipError),
    /// The input exceeds one of the configured [`Limits`](crate::Limits).
    Limit(LimitError),
    /// A metadata part (the content types stream, a relationships part or the core properties part) is not acceptable XML.
    Xml {
        /// The position of the part's entry in the ZIP archive, or `None` when the XML was parsed on its own.
        entry: Option<usize>,
        /// What is wrong with it.
        error: XmlError,
    },
    /// The package breaks a rule of the Open Packaging Conventions (ECMA-376 Part 2).
    Package(PackageError),
    /// An OLE compound file is malformed or exceeds a limit.
    CompoundFile(CompoundFileError),
    /// Memory for data whose size passed every limit could not be allocated.
    OutOfMemory,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Zip(error) => write!(f, "invalid ZIP container: {error}"),
            Error::Limit(error) => write!(f, "limit exceeded: {error}"),
            Error::Xml {
                entry: Some(entry),
                error,
            } => write!(
                f,
                "invalid XML in the metadata part at entry {entry}: {error}"
            ),
            Error::Xml { entry: None, error } => write!(f, "invalid XML: {error}"),
            Error::Package(error) => write!(f, "invalid package: {error}"),
            Error::CompoundFile(error) => write!(f, "invalid compound file: {error}"),
            Error::OutOfMemory => f.write_str("out of memory"),
        }
    }
}

impl core::error::Error for Error {}

impl From<ZipError> for Error {
    fn from(error: ZipError) -> Self {
        Error::Zip(error)
    }
}

impl From<LimitError> for Error {
    fn from(error: LimitError) -> Self {
        Error::Limit(error)
    }
}

impl From<PackageError> for Error {
    fn from(error: PackageError) -> Self {
        Error::Package(error)
    }
}

impl From<PartNameError> for Error {
    fn from(error: PartNameError) -> Self {
        Error::Package(PackageError::InvalidPartName(error))
    }
}

impl From<CompoundFileError> for Error {
    fn from(error: CompoundFileError) -> Self {
        Error::CompoundFile(error)
    }
}

/// A problem with the ZIP container. `entry` is the position of the entry in the archive's central directory, counting from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ZipError {
    /// The input has no end-of-central-directory record: it is not a ZIP archive, or it is truncated.
    NotZip,
    /// Several end-of-central-directory records fit the end of the input, so different readers could see different contents.
    AmbiguousEnd,
    /// The archive claims to span several disks (a split archive); packages are always a single file.
    MultipleDisks,
    /// A record of the archive's directory structure (end of central directory, ZIP64 records or the central directory itself) is malformed, inconsistent or points outside the input.
    MalformedDirectory,
    /// The archive uses central directory encryption or the second version of the ZIP64 end-of-central-directory record, which packages must not use.
    UnsupportedDirectory,
    /// A header of the entry is malformed or points outside the input.
    MalformedEntry {
        /// The entry.
        entry: usize,
    },
    /// The entry's local header, or its data descriptor, disagrees with its central directory header (name, flags, compression method, sizes or checksum).
    HeaderMismatch {
        /// The entry.
        entry: usize,
    },
    /// The entry's data overlaps another entry or the central directory, a trick that makes several names share one stream of data.
    Overlap {
        /// The entry.
        entry: usize,
    },
    /// The entry is encrypted.
    Encrypted {
        /// The entry.
        entry: usize,
    },
    /// The entry uses a compression method other than stored (0) or DEFLATE (8), the only two packages may use.
    UnsupportedCompression {
        /// The entry.
        entry: usize,
        /// The compression method number written in the entry.
        method: u16,
    },
    /// The entry uses another ZIP feature that packages must not use, such as patch data.
    UnsupportedFeature {
        /// The entry.
        entry: usize,
    },
    /// The entry is a symbolic link, a device, a volume label or another kind of entry that is neither a file nor a folder.
    SpecialFile {
        /// The entry.
        entry: usize,
    },
    /// The entry's name could escape the folder it is extracted to or be read differently by different tools: it is empty, absolute, has a drive letter, a backslash, a `.` or `..` segment, an empty segment or a control character, it is not valid UTF-8, or it has characters beyond ASCII without the flag that marks it as UTF-8 (other tools would read it in code page 437).
    UnsafeName {
        /// The entry.
        entry: usize,
    },
    /// Two entries have the same name, or names that are equivalent under the package's rules (ASCII letters compared without case, and every character beyond ASCII equal to its percent-encoding in UTF-8).
    DuplicateName {
        /// The later of the two entries.
        entry: usize,
        /// The earlier one.
        first: usize,
    },
    /// A folder entry holds data.
    FolderWithData {
        /// The entry.
        entry: usize,
    },
    /// The entry's DEFLATE data is corrupt, or is followed by extra bytes.
    CorruptData {
        /// The entry.
        entry: usize,
    },
    /// The entry's data does not have the size its headers declare.
    SizeMismatch {
        /// The entry.
        entry: usize,
    },
    /// The CRC-32 checksum of the entry's data differs from the one its headers declare.
    ChecksumMismatch {
        /// The entry.
        entry: usize,
    },
    /// The entry is too large to be held in memory on this platform (32-bit WebAssembly can address at most 4 GiB).
    TooLargeForPlatform {
        /// The entry.
        entry: usize,
    },
    /// There is no entry at this position.
    NoSuchEntry,
    /// A name given to the writer is not a safe ZIP entry name (see [`ZipError::UnsafeName`]).
    InvalidName,
    /// The writer was given two entries with equivalent names.
    DuplicateNameWritten,
    /// The archive being written would exceed what the ZIP64 format can describe.
    ArchiveTooLarge,
}

impl fmt::Display for ZipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ZipError::NotZip => f.write_str("no end of central directory record"),
            ZipError::AmbiguousEnd => {
                f.write_str("more than one end of central directory record fits")
            }
            ZipError::MultipleDisks => {
                f.write_str("archives that span several disks are not supported")
            }
            ZipError::MalformedDirectory => f.write_str("malformed central directory"),
            ZipError::UnsupportedDirectory => {
                f.write_str("central directory encryption is not supported")
            }
            ZipError::MalformedEntry { entry } => write!(f, "entry {entry} is malformed"),
            ZipError::HeaderMismatch { entry } => {
                write!(f, "the headers of entry {entry} disagree")
            }
            ZipError::Overlap { entry } => {
                write!(f, "the data of entry {entry} overlaps other data")
            }
            ZipError::Encrypted { entry } => write!(f, "entry {entry} is encrypted"),
            ZipError::UnsupportedCompression { entry, method } => {
                write!(
                    f,
                    "entry {entry} uses unsupported compression method {method}"
                )
            }
            ZipError::UnsupportedFeature { entry } => {
                write!(f, "entry {entry} uses an unsupported ZIP feature")
            }
            ZipError::SpecialFile { entry } => {
                write!(f, "entry {entry} is neither a file nor a folder")
            }
            ZipError::UnsafeName { entry } => write!(f, "entry {entry} has an unsafe name"),
            ZipError::DuplicateName { entry, first } => {
                write!(f, "entry {entry} has the same name as entry {first}")
            }
            ZipError::FolderWithData { entry } => write!(f, "folder entry {entry} holds data"),
            ZipError::CorruptData { entry } => write!(f, "the data of entry {entry} is corrupt"),
            ZipError::SizeMismatch { entry } => {
                write!(
                    f,
                    "the data of entry {entry} does not have its declared size"
                )
            }
            ZipError::ChecksumMismatch { entry } => {
                write!(f, "the checksum of entry {entry} does not match")
            }
            ZipError::TooLargeForPlatform { entry } => {
                write!(f, "entry {entry} is too large for this platform")
            }
            ZipError::NoSuchEntry => f.write_str("no such entry"),
            ZipError::InvalidName => f.write_str("invalid entry name"),
            ZipError::DuplicateNameWritten => {
                f.write_str("an entry with an equivalent name was already written")
            }
            ZipError::ArchiveTooLarge => f.write_str("the archive would be too large"),
        }
    }
}

/// A configured limit that the input exceeds (see [`Limits`](crate::Limits)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LimitError {
    /// The archive has more entries than [`Limits::max_entries`](crate::Limits::max_entries).
    TooManyEntries {
        /// The limit.
        limit: usize,
    },
    /// An entry's name is longer than [`Limits::max_name_length`](crate::Limits::max_name_length) bytes.
    NameTooLong {
        /// The entry.
        entry: usize,
        /// The limit.
        limit: usize,
    },
    /// An entry uncompresses to more than [`Limits::max_entry_size`](crate::Limits::max_entry_size) bytes.
    EntryTooLarge {
        /// The entry.
        entry: usize,
        /// The limit.
        limit: u64,
    },
    /// All entries together uncompress to more than [`Limits::max_total_size`](crate::Limits::max_total_size) bytes.
    TotalTooLarge {
        /// The limit.
        limit: u64,
    },
    /// An entry, or the archive as a whole, uncompresses to more than [`Limits::max_compression_ratio`](crate::Limits::max_compression_ratio) times its compressed size.
    CompressionRatio {
        /// The entry, or `None` for the archive as a whole.
        entry: Option<usize>,
        /// The limit.
        limit: u64,
    },
    /// A metadata part is larger than [`Limits::max_metadata_size`](crate::Limits::max_metadata_size) bytes.
    MetadataTooLarge {
        /// The part's entry, or `None` when the XML was parsed on its own.
        entry: Option<usize>,
        /// The limit.
        limit: usize,
    },
    /// The metadata parts that opening a package parses uncompress to more than [`Limits::max_metadata_total_size`](crate::Limits::max_metadata_total_size) bytes together.
    MetadataTotalTooLarge {
        /// The entry of the part that would have passed the limit; it is not decompressed.
        entry: usize,
        /// The limit.
        limit: usize,
    },
    /// A compound file has more entries than [`Limits::max_compound_file_entries`](crate::Limits::max_compound_file_entries).
    TooManyCompoundFileEntries {
        /// The limit.
        limit: usize,
    },
    /// A stream of a compound file is larger than [`Limits::max_entry_size`](crate::Limits::max_entry_size) bytes.
    StreamTooLarge {
        /// The limit.
        limit: u64,
    },
}

impl fmt::Display for LimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LimitError::TooManyEntries { limit } => write!(f, "more than {limit} entries"),
            LimitError::NameTooLong { entry, limit } => {
                write!(f, "the name of entry {entry} is longer than {limit} bytes")
            }
            LimitError::EntryTooLarge { entry, limit } => {
                write!(f, "entry {entry} uncompresses to more than {limit} bytes")
            }
            LimitError::TotalTooLarge { limit } => {
                write!(
                    f,
                    "the entries uncompress to more than {limit} bytes in total"
                )
            }
            LimitError::CompressionRatio {
                entry: Some(entry),
                limit,
            } => write!(f, "entry {entry} is compressed more than {limit} to 1"),
            LimitError::CompressionRatio { entry: None, limit } => {
                write!(f, "the archive is compressed more than {limit} to 1")
            }
            LimitError::MetadataTooLarge { limit, .. } => {
                write!(f, "a metadata part is larger than {limit} bytes")
            }
            LimitError::MetadataTotalTooLarge { entry, limit } => write!(
                f,
                "with entry {entry}, the metadata parts uncompress to more than {limit} bytes in total"
            ),
            LimitError::TooManyCompoundFileEntries { limit } => {
                write!(f, "a compound file has more than {limit} entries")
            }
            LimitError::StreamTooLarge { limit } => {
                write!(f, "a compound file stream is larger than {limit} bytes")
            }
        }
    }
}

/// What is wrong with a metadata part's XML, and where: `offset` counts bytes from the start of the part's text after any conversion from UTF-16 to UTF-8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XmlError {
    /// The position of the problem, in bytes.
    pub offset: usize,
    /// The problem.
    pub kind: XmlErrorKind,
}

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.kind, self.offset)
    }
}

/// The kinds of XML problems.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum XmlErrorKind {
    /// The bytes are not valid UTF-8, or not valid UTF-16 after a UTF-16 byte order mark.
    InvalidEncoding,
    /// The XML declaration names an encoding other than UTF-8 or UTF-16, or one that does not match the bytes.
    UnsupportedEncoding,
    /// The XML declaration is malformed, or names an XML version other than 1.0.
    MalformedDeclaration,
    /// The document contains a document type declaration (`<!DOCTYPE`), which packages must not use and which would allow entity-expansion attacks.
    DoctypeNotAllowed,
    /// The document refers to an entity other than the five predefined ones and character references.
    UndefinedEntity,
    /// A character reference is malformed or names a character that XML does not allow.
    InvalidCharacterReference,
    /// The document contains a character that XML does not allow.
    InvalidCharacter,
    /// The input ends in the middle of the document.
    UnexpectedEnd,
    /// The markup is malformed.
    Malformed,
    /// An end tag does not match the start tag it closes.
    MismatchedEndTag,
    /// An element has the same attribute twice.
    DuplicateAttribute,
    /// An element or attribute name uses a namespace prefix that is not declared, or a namespace declaration breaks the rules of Namespaces in XML.
    NamespaceError,
    /// There is content other than comments, processing instructions and white space outside the root element, or no root element at all.
    ContentOutsideRoot,
    /// The elements are nested more deeply than [`Limits::max_xml_depth`](crate::Limits::max_xml_depth).
    TooDeep,
    /// An element has more attributes than [`Limits::max_xml_attributes`](crate::Limits::max_xml_attributes).
    TooManyAttributes,
    /// A name is longer than [`Limits::max_xml_name_length`](crate::Limits::max_xml_name_length) bytes.
    NameTooLong,
    /// The document has more nodes than [`Limits::max_xml_nodes`](crate::Limits::max_xml_nodes) allows (or, while a package is opened, all its metadata parts together do).
    TooManyNodes,
}

impl fmt::Display for XmlErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            XmlErrorKind::InvalidEncoding => "invalid character encoding",
            XmlErrorKind::UnsupportedEncoding => "unsupported character encoding",
            XmlErrorKind::MalformedDeclaration => "malformed XML declaration",
            XmlErrorKind::DoctypeNotAllowed => "document type declarations are not allowed",
            XmlErrorKind::UndefinedEntity => "undefined entity",
            XmlErrorKind::InvalidCharacterReference => "invalid character reference",
            XmlErrorKind::InvalidCharacter => "character not allowed in XML",
            XmlErrorKind::UnexpectedEnd => "unexpected end of input",
            XmlErrorKind::Malformed => "malformed markup",
            XmlErrorKind::MismatchedEndTag => "mismatched end tag",
            XmlErrorKind::DuplicateAttribute => "duplicate attribute",
            XmlErrorKind::NamespaceError => "namespace error",
            XmlErrorKind::ContentOutsideRoot => "content outside the root element",
            XmlErrorKind::TooDeep => "elements nested too deeply",
            XmlErrorKind::TooManyAttributes => "too many attributes",
            XmlErrorKind::NameTooLong => "name too long",
            XmlErrorKind::TooManyNodes => "too many nodes",
        })
    }
}

/// A rule of the Open Packaging Conventions (ECMA-376 Part 2) that the package breaks, or a request to the writer that would break one.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PackageError {
    /// The package has no content types stream (`[Content_Types].xml`).
    MissingContentTypes,
    /// The package stores a part, or the content types stream, in pieces ("interleaving", ECMA-376 Part 2 §7.2.4), which this reader does not support.
    InterleavedPart {
        /// The entry of the piece.
        entry: usize,
    },
    /// The name of one part is derivable from another's (for example `/a` and `/a/b`), which ECMA-376 Part 2 §6.2.2.3 forbids.
    DerivablePartName {
        /// The entry of the longer name.
        entry: usize,
        /// The entry of the shorter name.
        other: usize,
    },
    /// A part name, or a name given to the writer, breaks the part name grammar.
    InvalidPartName(PartNameError),
    /// The content types stream breaks a rule.
    ContentTypes(ContentTypesError),
    /// A relationships part breaks a rule.
    Relationships {
        /// The entry of the relationships part, or `None` when the XML was parsed on its own.
        entry: Option<usize>,
        /// The rule.
        error: RelationshipsError,
    },
    /// The core properties part breaks a rule.
    CoreProperties(CorePropertiesError),
    /// A relationship's target cannot be resolved to a part name.
    InvalidTarget(TargetError),
    /// The package has no part with this name.
    PartNotFound,
    /// The writer already has a part with this name, or with an equivalent one.
    PartExists,
    /// A part name given to the writer is derivable from an existing part's name, or the other way round.
    DerivableName,
    /// A relationships part cannot be added or replaced as an ordinary part; the writer creates it from the relationships.
    RelationshipsPartName,
    /// A media type given to the writer is not a valid media type.
    InvalidContentType,
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageError::MissingContentTypes => f.write_str("no content types stream"),
            PackageError::InterleavedPart { entry } => {
                write!(
                    f,
                    "entry {entry} is a piece of an interleaved part, which is not supported"
                )
            }
            PackageError::DerivablePartName { entry, other } => write!(
                f,
                "the part name of entry {entry} is derivable from that of entry {other}"
            ),
            PackageError::InvalidPartName(error) => write!(f, "invalid part name: {error}"),
            PackageError::ContentTypes(error) => write!(f, "content types stream: {error}"),
            PackageError::Relationships {
                entry: Some(entry),
                error,
            } => write!(f, "relationships part at entry {entry}: {error}"),
            PackageError::Relationships { entry: None, error } => {
                write!(f, "relationships part: {error}")
            }
            PackageError::CoreProperties(error) => write!(f, "core properties: {error}"),
            PackageError::InvalidTarget(error) => write!(f, "relationship target: {error}"),
            PackageError::PartNotFound => f.write_str("no such part"),
            PackageError::PartExists => f.write_str("a part with an equivalent name exists"),
            PackageError::DerivableName => f.write_str(
                "the part name is derivable from another part's name, or the other way round",
            ),
            PackageError::RelationshipsPartName => {
                f.write_str("relationships parts are written from their relationships")
            }
            PackageError::InvalidContentType => f.write_str("invalid media type"),
        }
    }
}

/// Why a string is not a valid part name (ECMA-376 Part 2 §6.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PartNameError {
    /// The name is empty, or does not start with `/`.
    MissingLeadingSlash,
    /// A segment is empty (two slashes in a row, or a slash at the end).
    EmptySegment,
    /// A segment ends with a dot (this includes the segments `.` and `..`).
    TrailingDot,
    /// The name contains a character that part names do not allow.
    InvalidCharacter,
    /// A `%` is not followed by two hexadecimal digits.
    InvalidPercentEncoding,
    /// The name percent-encodes a character that must be written as itself (an unreserved character such as a letter, a digit or a non-ASCII letter) or a slash or backslash.
    ForbiddenPercentEncoding,
}

impl fmt::Display for PartNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PartNameError::MissingLeadingSlash => "it does not start with a slash",
            PartNameError::EmptySegment => "it has an empty segment",
            PartNameError::TrailingDot => "a segment ends with a dot",
            PartNameError::InvalidCharacter => "it contains a character part names do not allow",
            PartNameError::InvalidPercentEncoding => "malformed percent-encoding",
            PartNameError::ForbiddenPercentEncoding => {
                "it percent-encodes a character that must be written as itself"
            }
        })
    }
}

/// A rule of the content types stream (ECMA-376 Part 2 §7.2.3) that it breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContentTypesError {
    /// The root element is not `Types` in the content types namespace.
    UnexpectedRoot,
    /// An element other than `Default` or `Override` in the content types namespace, or an element inside one of them.
    UnexpectedElement,
    /// An attribute that the element does not have; the stream may not use extensions.
    UnexpectedAttribute,
    /// Text other than white space.
    UnexpectedText,
    /// A required attribute is missing.
    MissingAttribute,
    /// Two `Default` elements map the same extension.
    DuplicateDefault,
    /// Two `Override` elements name the same part.
    DuplicateOverride,
    /// An `Extension` attribute breaks the `ST_Extension` pattern.
    InvalidExtension,
    /// A `ContentType` attribute is not a valid media type.
    InvalidContentType,
    /// A `PartName` attribute is not a valid part name.
    InvalidPartName(PartNameError),
    /// A `PartName` or `Extension` attribute is longer than a ZIP entry name may be ([`Limits::max_name_length`](crate::Limits::max_name_length) bytes, not counting a part name's leading slash), so it could never apply to a part.
    NameTooLong,
}

impl fmt::Display for ContentTypesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContentTypesError::UnexpectedRoot => f.write_str("the root element is not Types"),
            ContentTypesError::UnexpectedElement => f.write_str("unexpected element"),
            ContentTypesError::UnexpectedAttribute => f.write_str("unexpected attribute"),
            ContentTypesError::UnexpectedText => f.write_str("unexpected text"),
            ContentTypesError::MissingAttribute => f.write_str("missing attribute"),
            ContentTypesError::DuplicateDefault => f.write_str("two defaults for one extension"),
            ContentTypesError::DuplicateOverride => f.write_str("two overrides for one part"),
            ContentTypesError::InvalidExtension => f.write_str("invalid extension"),
            ContentTypesError::InvalidContentType => f.write_str("invalid media type"),
            ContentTypesError::InvalidPartName(error) => write!(f, "invalid part name: {error}"),
            ContentTypesError::NameTooLong => {
                f.write_str("a part name or extension is longer than an entry name may be")
            }
        }
    }
}

/// A rule of relationships parts (ECMA-376 Part 2 §6.5) that a part breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RelationshipsError {
    /// The root element is not `Relationships` in the relationships namespace.
    UnexpectedRoot,
    /// An element in the relationships namespace other than `Relationship`, or one nested where it does not belong.
    UnexpectedElement,
    /// Text other than white space between relationships.
    UnexpectedText,
    /// A required attribute (`Id`, `Type` or `Target`) is missing or empty.
    MissingAttribute,
    /// The `TargetMode` attribute is neither `Internal` nor `External`.
    InvalidTargetMode,
    /// Two relationships have the same `Id`.
    DuplicateId,
    /// The part uses an `xml:base` attribute, which would change how targets resolve.
    XmlBase,
    /// The part is a relationships part of a relationships part; relationships parts have no relationships.
    RelationshipsOfRelationshipsPart,
    /// The relationships part's name does not name a valid source part.
    InvalidSource,
    /// A relationship being added has a type or target with a character that XML cannot hold (most control characters, U+FFFE and U+FFFF), so it could not be written.
    InvalidCharacter,
    /// A relationship being added has an identifier that is not an XML identifier: a name without a colon (`xsd:ID`).
    InvalidId,
    /// A relationship's identifier is longer than [`Limits::max_xml_name_length`](crate::Limits::max_xml_name_length) bytes, the limit on XML names, which identifiers are (`xsd:ID`).
    IdTooLong,
}

impl fmt::Display for RelationshipsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RelationshipsError::UnexpectedRoot => "the root element is not Relationships",
            RelationshipsError::UnexpectedElement => "unexpected element",
            RelationshipsError::UnexpectedText => "unexpected text",
            RelationshipsError::MissingAttribute => "missing attribute",
            RelationshipsError::InvalidTargetMode => "invalid target mode",
            RelationshipsError::DuplicateId => "two relationships with one identifier",
            RelationshipsError::XmlBase => "xml:base is not allowed",
            RelationshipsError::RelationshipsOfRelationshipsPart => {
                "relationships parts cannot have relationships"
            }
            RelationshipsError::InvalidSource => "the name does not name a source part",
            RelationshipsError::InvalidCharacter => "a character that XML cannot hold",
            RelationshipsError::InvalidId => "the identifier is not an XML identifier",
            RelationshipsError::IdTooLong => "the identifier is longer than an XML name may be",
        })
    }
}

/// A rule of the core properties part (ECMA-376 Part 2 §8) that it breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CorePropertiesError {
    /// The root element is not `coreProperties` in the core properties namespace.
    UnexpectedRoot,
    /// A core property appears twice.
    DuplicateProperty,
    /// The package has more than one core properties relationship.
    MultipleCoreProperties,
    /// The core properties relationship does not point to a part of the package.
    MissingPart,
    /// A property to be written has a character that XML cannot hold (most control characters, U+FFFE and U+FFFF).
    InvalidCharacter,
}

impl fmt::Display for CorePropertiesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CorePropertiesError::UnexpectedRoot => "the root element is not coreProperties",
            CorePropertiesError::DuplicateProperty => "a property appears twice",
            CorePropertiesError::MultipleCoreProperties => {
                "more than one core properties relationship"
            }
            CorePropertiesError::MissingPart => "the core properties part is missing",
            CorePropertiesError::InvalidCharacter => {
                "a property has a character that XML cannot hold"
            }
        })
    }
}

/// Why a relationship's target does not resolve to a part name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TargetError {
    /// The target is an absolute reference with a scheme (such as `http:`) or an authority (`//host`), but the relationship's target mode is internal.
    NotRelative,
    /// The target refers to the source itself, not to a part.
    Empty,
    /// The target resolves to a string that is not a valid part name.
    InvalidPartName(PartNameError),
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TargetError::NotRelative => {
                f.write_str("an internal target is not a relative reference")
            }
            TargetError::Empty => f.write_str("the target is empty"),
            TargetError::InvalidPartName(error) => {
                write!(f, "the target is not a valid part name: {error}")
            }
        }
    }
}

/// A problem with an OLE compound file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompoundFileError {
    /// The input does not start with the compound file signature.
    NotCompoundFile,
    /// The compound file's structure (header, allocation tables or directory) is malformed.
    Malformed,
    /// An entry's name is empty, is `.` or `..`, or contains a character that compound files forbid (`/`, `\`, `:`, `!`).
    InvalidName,
    /// Two entries of one storage have names that the compound file's own lookup cannot tell apart.
    DuplicateName,
    /// There is no entry at this path.
    NotFound,
    /// The entry at this path is a storage, not a stream.
    NotAStream,
}

impl fmt::Display for CompoundFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CompoundFileError::NotCompoundFile => "not a compound file",
            CompoundFileError::Malformed => "malformed structure",
            CompoundFileError::InvalidName => "invalid entry name",
            CompoundFileError::DuplicateName => "two entries with names that cannot be told apart",
            CompoundFileError::NotFound => "no such entry",
            CompoundFileError::NotAStream => "the entry is not a stream",
        })
    }
}
