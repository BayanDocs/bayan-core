//! # bayan-opc
//!
//! Open Packaging Conventions: the hardened package layer under every `.docx` (ECMA-376 Part 2, work package [CORE-005]).
//!
//! A `.docx` file is a **package**: a ZIP archive whose entries are **parts** (the document text, styles, images and so on), with a **content types stream** that gives every part its media type and **relationships parts** that connect the package and its parts to other parts and to external resources. This crate is the first code to touch the bytes of a document someone else made, so it treats them as hostile ([threat model] T1, T3): every structure is checked before it is used, every size is limited before anything is decompressed, nothing in it is ever fetched or run, and it contains no `unsafe` code and no C code ([ADR-0006]).
//!
//! ## What is here
//!
//! - [`Package`] opens a package and checks it: the ZIP container, the part names, the content types stream and every relationships part. [`Package::read_part`] decompresses a part on demand; [`Package::relationships`] and [`Relationship::resolve`] follow relationships; [`Package::core_properties`] reads the title, author and dates.
//! - [`PackageWriter`] writes a package deterministically. Made from an opened package, it copies every untouched part's compressed bytes unchanged, so untouched parts stay byte-identical, the guarantee of lossless round-tripping ([ADR-0018] rule 3, Tier B of [ADR-0004]); changed metadata is rewritten with only its changed entries replaced.
//! - [`zip`]: the hardened ZIP reader and deterministic ZIP writer underneath, for the subset of ZIP that packages may use (stored and DEFLATE entries, data descriptors, ZIP64).
//! - [`PartName`], [`ContentTypes`], [`Relationships`] and [`CoreProperties`]: the pieces of the package model, each usable on its own.
//! - [`CompoundFile`]: read-only access to OLE compound files, for legacy `.doc` files, VBA projects and encrypted documents in later work packages.
//!
//! ```
//! use bayan_opc::{Limits, Package, PackageWriter, PartName, RelationshipSource, Target};
//!
//! # fn main() -> Result<(), bayan_opc::Error> {
//! // A minimal package, written with the writer.
//! let mut writer = PackageWriter::new();
//! let document = PartName::new("/word/document.xml")?;
//! writer.add_part(&document, "application/xml", b"<doc/>".to_vec())?;
//! let relationships = writer.relationships_mut(&RelationshipSource::Package)?;
//! let id = relationships.next_id();
//! relationships.add(bayan_opc::Relationship::new(id, "urn:example:main", "word/document.xml", bayan_opc::TargetMode::Internal))?;
//! let bytes = writer.finish()?;
//!
//! // Read back: open, follow the relationship, read the part.
//! let package = Package::open(&bytes, &Limits::default())?;
//! let main = package.relationships(&RelationshipSource::Package).and_then(|r| r.of_type("urn:example:main").next());
//! let Some(Ok(Target::Part(part))) = main.map(|r| r.resolve(&RelationshipSource::Package)) else {
//!     panic!("the relationship resolves to a part");
//! };
//! assert_eq!(package.read_part(&part)?, b"<doc/>");
//! assert_eq!(package.content_type(&part), Some("application/xml"));
//!
//! // Written again without changes, every part is byte-identical.
//! let again = PackageWriter::from_package(&package).finish()?;
//! assert_eq!(again, bytes);
//! # Ok(())
//! # }
//! ```
//!
//! ## Limits
//!
//! Every limit is a field of [`Limits`], with a default chosen to open ordinary documents comfortably and stop malicious ones; a host can raise one for a document the user trusts. The reader checks them before the work they protect:
//!
//! | Limit | Default | Checked |
//! |---|---|---|
//! | [`Limits::max_entries`] | 10,000 | when the end of the central directory is read, before any entry |
//! | [`Limits::max_name_length`] | 1,024 bytes | for each entry, before its name is read |
//! | [`Limits::max_entry_size`] | 512 MiB | for each entry, from its declared size, before anything is decompressed; also the largest compound file stream |
//! | [`Limits::max_total_size`] | 2 GiB | for all entries together, before anything is decompressed |
//! | [`Limits::max_compression_ratio`] | 100 to 1 | for each entry, and for the archive as a whole, that uncompresses to more than [`Limits::compression_ratio_grace`] (1 MiB) |
//! | [`Limits::max_metadata_size`] | 8 MiB | for the content types stream, relationships parts and the core properties part, before they are decompressed |
//! | [`Limits::max_metadata_total_size`] | 16 MiB | for the content types stream and every relationships part together, while a package is opened, before each is decompressed |
//! | [`Limits::max_xml_nodes`] | 250,000 | while metadata XML is parsed, for all the metadata that opening a package parses together |
//! | [`Limits::max_xml_depth`], [`Limits::max_xml_attributes`], [`Limits::max_xml_name_length`] | 64 (at most 256), 64, 256 bytes | while metadata XML is parsed |
//! | [`Limits::max_compound_file_entries`] | 65,536 | while a compound file's entries are listed |
//!
//! Decompression never goes beyond a declared size: a part is read into memory set aside once, at exactly its declared size, which the limits have bounded, and decompression stops with an error as soon as the data would go beyond it. A part that is only checked, not kept (when the writer copies it unchanged), passes through a window of 128 KiB instead. Reading a part therefore takes at most [`Limits::max_entry_size`] bytes, plus that window, whatever its headers claim.
//!
//! The metadata parts are different: their XML is held as a tree while it is parsed, and every node of the tree (an element, an attribute, a run of text) takes a few hundred bytes however short it is in the input, so a small part can need a hundred times its size. What bounds the memory that opening a package takes is therefore [`Limits::max_xml_nodes`], together with [`Limits::max_metadata_total_size`] for the text that is kept. With the default limits, opening a package takes at most about 90 MiB on top of the package's own bytes, whatever it contains: packages built to use as much memory as the limits allow needed from 40 to 89 MiB in measurements (the most for a content types stream of 83,000 overrides with long part names, 66 MiB for relationships parts of a quarter of a million tiny elements, in UTF-8 or UTF-16). Opening a package reads no other part.
//!
//! ## What is refused
//!
//! Besides malformed structure, the reader refuses, each with its own error ([`ZipError`], [`PackageError`]): encrypted entries; compression methods other than stored and DEFLATE; symbolic links and other special files; entry names that are absolute, carry a drive letter, contain a backslash, a control character or a `.`, `..` or empty segment; duplicate names, including names that are equivalent under the package's rules (ASCII letters compared without case, `%C3%A9` equal to `é`); entries whose data overlaps another's (several names for one compressed stream, a classic zip bomb); local headers that disagree with the central directory; ambiguous ends of the archive; split archives; part names derivable from one another; interleaved parts; document type declarations and entities other than the predefined ones in metadata XML (so entity expansion is impossible); and `xml:base` in relationships parts. External relationship targets are recorded as written and never fetched ([ADR-0023]).
//!
//! ## XML
//!
//! The three metadata parts are read by a small internal XML parser, because the engine's XML layer, bayan-xml, is still to be built ([CORE-006]). The parser is deliberately minimal and strict, and is marked for replacement by bayan-xml.
//!
//! ## Layer
//!
//! bayan-opc belongs to the **Foundation** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-opc may therefore depend only on other Foundation crates and on external libraries.
//!
//! [CORE-005]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-005-bayan-opc.md
//! [CORE-006]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-006-bayan-xml.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core
//! [threat model]: https://github.com/BayanDocs/docs/blob/HEAD/specs/threat-model.md
//! [ADR-0004]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0004-fidelity-contract-and-determinism.md
//! [ADR-0006]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0006-rust-core-and-memory-safety.md
//! [ADR-0018]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0018-file-formats-and-priorities.md
//! [ADR-0023]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0023-automation-macros-active-content.md

#![forbid(unsafe_code)]

pub mod compound_file;
pub mod content_types;
pub mod core_properties;
mod error;
mod limits;
mod package;
pub mod part_name;
mod percent;
pub mod relationships;
mod xml;
pub mod zip;

pub use compound_file::{CompoundEntry, CompoundEntryKind, CompoundFile};
pub use content_types::ContentTypes;
pub use core_properties::CoreProperties;
pub use error::{
    CompoundFileError, ContentTypesError, CorePropertiesError, Error, LimitError, PackageError,
    PartNameError, RelationshipsError, TargetError, XmlError, XmlErrorKind, ZipError,
};
pub use limits::Limits;
pub use package::{Package, PackageWriter};
pub use part_name::{PartName, RelationshipSource};
pub use relationships::{Relationship, Relationships, Target, TargetMode};
