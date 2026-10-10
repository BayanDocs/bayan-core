//! Feature tagging: reads a `.docx` package, safely and within limits, and reports the coverage-matrix features, fonts, languages, scripts and compatibility settings it uses.
//!
//! [`scan`] reads the ZIP archive ([`zip`]), the content types and relationships ([`opc`]), and then the XML of the main document part and of the parts it relies on ([`xml`]): styles, numbering, settings, the font table, the theme, headers, footers, footnotes, endnotes, comments and charts, plus the application properties. The glossary document (building blocks) is noted but not read, because its content is not part of the document. Nothing a package refers to outside itself is ever fetched, and nothing in it is run.
//!
//! [`Limits::DEFAULT`] bounds every resource the scan uses: the package size, the number and sizes of its entries, the number of parts read, nesting depth, attributes, names and namespace declarations. A package that exceeds a limit is refused with an error that says which; the tests in `tests/lab/hostile_packages.rs` check each one with crafted files.

pub mod crc32;
pub mod features;
pub mod opc;
pub mod scripts;
pub mod tagger;
pub mod vocabulary;
pub mod xml;
pub mod zip;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use features::Feature;
use opc::{ContentTypes, OpcError, Relationship, Target};
use scripts::Script;
use tagger::{Collected, PartKind, PartTagger};
use vocabulary::Rel;
use xml::{XmlError, XmlLimits};
use zip::{Archive, ZipError, ZipLimits};

/// The version of the tagging rules. Raise it whenever a rule changes what a document is tagged with; `corpus verify` then reports the documents tagged by an older version, and `corpus tag` tags them again.
pub const TAGGER_VERSION: u32 = 1;

/// The content type of the main part of a `.docx` document (Transitional and Strict alike).
const DOCUMENT_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";

/// The first bytes of an OLE compound file: an encrypted `.docx` or a binary `.doc`.
const OLE_SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// Resource limits of a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest package accepted, in bytes.
    pub max_package_size: usize,
    /// Limits of the ZIP reader.
    pub zip: ZipLimits,
    /// Limits of the XML scanner, for each part.
    pub xml: XmlLimits,
    /// The most parts read from one package.
    pub max_parts: usize,
}

impl Limits {
    /// The limits the corpus tools use. Real documents stay far below them: in the 19,209 XML parts of the public corpus v1, no element has more than 76 attributes, no more than 74 namespace declarations are in scope at once, nothing is nested deeper than 43 elements, and no name is longer than 37 bytes.
    pub const DEFAULT: Self = Self {
        max_package_size: 512 * 1024 * 1024,
        zip: ZipLimits {
            max_entries: 10_000,
            max_name_len: 1_024,
            max_entry_size: 256 * 1024 * 1024,
            max_total_size: 1024 * 1024 * 1024,
        },
        xml: XmlLimits {
            max_depth: 1_024,
            max_attributes: 256,
            max_name_len: 256,
            max_namespaces: 256,
        },
        max_parts: 2_000,
    };
}

/// Why a package cannot be tagged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanError {
    /// The package is larger than [`Limits::max_package_size`].
    TooLarge,
    /// An OLE compound file: an encrypted package or a binary Word document, not a `.docx`.
    OleCompoundFile,
    /// The ZIP archive cannot be read.
    Zip(ZipError),
    /// A part every package needs is missing; the text names it.
    MissingPart(&'static str),
    /// The main part is not a WordprocessingML document; the text says what it is.
    NotADocument(&'static str),
    /// A part cannot be read as XML.
    Part {
        /// The part's name (part names of packages are not document content).
        part: String,
        /// What is wrong with it.
        error: OpcError,
    },
    /// More parts to read than [`Limits::max_parts`].
    TooManyParts,
}

impl fmt::Display for ScanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => formatter.write_str("the file is larger than the limit"),
            Self::OleCompoundFile => formatter.write_str("an OLE compound file (an encrypted document or a binary .doc), not a .docx package"),
            Self::Zip(error) => error.fmt(formatter),
            Self::MissingPart(part) => write!(formatter, "the package has no {part}"),
            Self::NotADocument(what) => write!(formatter, "the main part is {what}, not a .docx document"),
            Self::Part { part, error } => write!(formatter, "part {}: {error}", printable(part)),
            Self::TooManyParts => formatter.write_str("more parts to read than the limit"),
        }
    }
}

impl std::error::Error for ScanError {}

impl From<ZipError> for ScanError {
    fn from(error: ZipError) -> Self {
        Self::Zip(error)
    }
}

/// A part name for a message: at most 100 characters, with control characters replaced.
fn printable(name: &str) -> String {
    name.chars()
        .take(100)
        .map(|character| {
            if character.is_control() {
                '?'
            } else {
                character
            }
        })
        .collect()
}

/// What tagging found in one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Findings {
    /// The coverage-matrix features the document uses.
    pub features: BTreeSet<Feature>,
    /// The fonts it names: the font table, `w:rFonts`, DrawingML text, and the theme fonts it uses.
    pub fonts: BTreeSet<String>,
    /// The language tags it declares (`w:lang`, `w:themeFontLang`), in canonical case.
    pub languages: BTreeSet<String>,
    /// Characters per script in its text.
    pub scripts: BTreeMap<Script, u64>,
    /// Its `compatibilityMode` setting, if any.
    pub compatibility_mode: Option<u32>,
    /// The legacy compatibility options it switches on.
    pub compatibility_options: BTreeSet<String>,
    /// The application that last saved it, from its application properties.
    pub application: Option<String>,
    /// The page count that application recorded.
    pub pages_hint: Option<u32>,
    /// Non-fatal observations, from a fixed list of messages.
    pub notes: BTreeSet<&'static str>,
}

/// Tags the package in `bytes`.
///
/// # Errors
///
/// A [`ScanError`] when the package is not a `.docx` document this scanner can read safely within `limits`.
pub fn scan(bytes: &[u8], limits: &Limits) -> Result<Findings, ScanError> {
    if bytes.len() > limits.max_package_size {
        return Err(ScanError::TooLarge);
    }
    if bytes.starts_with(&OLE_SIGNATURE) {
        return Err(ScanError::OleCompoundFile);
    }
    let archive = Archive::open(bytes, &limits.zip)?;
    let mut package = Package::new(&archive, limits)?;
    let types_bytes = package
        .read("/[Content_Types].xml")?
        .ok_or(ScanError::MissingPart("[Content_Types].xml"))?;
    let types =
        ContentTypes::parse(&types_bytes, &limits.xml).map_err(|error| ScanError::Part {
            part: "/[Content_Types].xml".to_owned(),
            error,
        })?;
    let package_relationships = package.relationships("/")?.ok_or(ScanError::MissingPart(
        "package relationships (/_rels/.rels)",
    ))?;
    let mut collected = Collected::default();
    collected.features.insert(Feature::OpcPackage);
    let mut main = None;
    for relationship in &package_relationships {
        match (kind_of(relationship), &relationship.target) {
            (Rel::OfficeDocument, Target::Internal(part)) if main.is_none() => {
                main = Some(part.clone())
            }
            (Rel::ExtendedProperties, Target::Internal(part)) => {
                collected.features.insert(Feature::DocumentProperties);
                package.scan_part(part, PartKind::ApplicationProperties, &mut collected)?;
            }
            (kind, _) => tag_relationship(kind, &mut collected),
        }
    }
    let main = main.ok_or(ScanError::MissingPart("main document part"))?;
    match types.of(&main) {
        Some(DOCUMENT_CONTENT_TYPE) => {}
        Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml",
        ) => {
            return Err(ScanError::NotADocument("a template (.dotx)"));
        }
        Some("application/vnd.ms-word.document.macroenabled.main+xml") => {
            return Err(ScanError::NotADocument("a macro-enabled document (.docm)"));
        }
        Some("application/vnd.ms-word.template.macroenabledtemplate.main+xml") => {
            return Err(ScanError::NotADocument("a macro-enabled template (.dotm)"));
        }
        _ => return Err(ScanError::NotADocument("not WordprocessingML")),
    }
    if !package.exists(&main) {
        return Err(ScanError::MissingPart("main document part"));
    }
    package.scan_part(&main, PartKind::Document, &mut collected)?;
    // The parts the main document relies on, and the parts those rely on (images and charts of headers, for example).
    let mut pending = vec![main];
    while let Some(source) = pending.pop() {
        let Some(relationships) = package.relationships(&source)? else {
            continue;
        };
        for relationship in &relationships {
            let kind = kind_of(relationship);
            tag_relationship(kind, &mut collected);
            let Target::Internal(part) = &relationship.target else {
                continue;
            };
            let part_kind = match kind {
                Rel::Styles => PartKind::Styles,
                Rel::Numbering => PartKind::Numbering,
                Rel::Settings => PartKind::Settings,
                Rel::FontTable => PartKind::FontTable,
                Rel::Theme => PartKind::Theme,
                Rel::Header => PartKind::Header,
                Rel::Footer => PartKind::Footer,
                Rel::Footnotes => PartKind::Footnotes,
                Rel::Endnotes => PartKind::Endnotes,
                Rel::Comments => PartKind::Comments,
                Rel::CommentExtensions => PartKind::CommentExtensions,
                Rel::Chart => PartKind::Chart,
                _ => continue,
            };
            if !package.exists(part) {
                collected
                    .notes
                    .insert("a relationship points to a part that is missing");
                continue;
            }
            if package.scan_part(part, part_kind, &mut collected)?
                && matches!(
                    part_kind,
                    PartKind::Header
                        | PartKind::Footer
                        | PartKind::Footnotes
                        | PartKind::Endnotes
                        | PartKind::Comments
                )
            {
                pending.push(part.clone());
            }
        }
    }
    collected.finish();
    Ok(Findings {
        features: collected.features,
        fonts: collected.fonts,
        languages: collected.languages,
        scripts: collected.scripts,
        compatibility_mode: collected.compatibility_mode,
        compatibility_options: collected.compatibility_options,
        application: collected.application,
        pages_hint: collected.pages_hint,
        notes: collected.notes,
    })
}

/// The kind of a relationship, from its type.
fn kind_of(relationship: &Relationship) -> Rel {
    vocabulary::relationship(&relationship.rel_type)
}

/// Tags what a relationship alone reveals.
fn tag_relationship(kind: Rel, collected: &mut Collected) {
    let feature = match kind {
        Rel::CoreProperties | Rel::ExtendedProperties | Rel::CustomProperties => {
            Feature::DocumentProperties
        }
        Rel::CustomXml => Feature::CustomXmlParts,
        Rel::GlossaryDocument => Feature::GlossaryDocument,
        Rel::Font => Feature::EmbeddedFonts,
        Rel::DigitalSignature => Feature::DigitalSignatures,
        Rel::Vba => Feature::VbaProject,
        Rel::Image => Feature::ImageFormats,
        Rel::Hyperlink => Feature::Hyperlinks,
        Rel::Chart => Feature::Charts,
        Rel::ChartEx => Feature::NewerChartTypes,
        Rel::OleObject => Feature::OleObjects,
        Rel::Diagram => Feature::SmartArt,
        Rel::Comments | Rel::CommentExtensions => Feature::Comments,
        Rel::Model3d => Feature::Models3d,
        Rel::Unknown => Feature::UnknownPartsAndExtensions,
        Rel::OfficeDocument
        | Rel::Styles
        | Rel::StylesWithEffects
        | Rel::Numbering
        | Rel::Settings
        | Rel::WebSettings
        | Rel::FontTable
        | Rel::Theme
        | Rel::Header
        | Rel::Footer
        | Rel::Footnotes
        | Rel::Endnotes
        | Rel::ChartExtras
        | Rel::AttachedTemplate
        | Rel::Thumbnail
        | Rel::PrinterSettings
        | Rel::ClassificationLabels => return,
    };
    collected.features.insert(feature);
}

/// The parts of an archive by normalized part name, and the parts already read.
struct Package<'a, 'b> {
    archive: &'b Archive<'a>,
    limits: &'b Limits,
    parts: BTreeMap<String, &'b zip::Entry>,
    scanned: BTreeSet<String>,
    reads: usize,
}

impl<'a, 'b> Package<'a, 'b> {
    /// The parts of `archive`. Two entries that name the same part once percent-encoding is decoded (`word/document.xml` and `word/%64ocument.xml`) make the package ambiguous, so it is refused, as the ZIP reader refuses names that differ only in case.
    fn new(archive: &'b Archive<'a>, limits: &'b Limits) -> Result<Self, ScanError> {
        let mut parts = BTreeMap::new();
        for entry in archive.entries().filter(|entry| !entry.is_folder()) {
            if parts.insert(opc::normalize(entry.name()), entry).is_some() {
                return Err(ScanError::Zip(ZipError::DuplicateName));
            }
        }
        Ok(Self {
            archive,
            limits,
            parts,
            scanned: BTreeSet::new(),
            reads: 0,
        })
    }

    fn exists(&self, part: &str) -> bool {
        self.parts.contains_key(&opc::normalize(part))
    }

    /// The content of a part, or `None` if the package has no such part.
    fn read(&mut self, part: &str) -> Result<Option<Vec<u8>>, ScanError> {
        let Some(entry) = self.parts.get(&opc::normalize(part)).copied() else {
            return Ok(None);
        };
        self.reads += 1;
        if self.reads > self.limits.max_parts {
            return Err(ScanError::TooManyParts);
        }
        Ok(Some(self.archive.read(entry)?))
    }

    /// The relationships of a part (`/` for the package), or `None` if it has no relationships part.
    fn relationships(&mut self, source: &str) -> Result<Option<Vec<Relationship>>, ScanError> {
        let name = opc::relationships_part(source);
        let Some(bytes) = self.read(&name)? else {
            return Ok(None);
        };
        opc::relationships(&bytes, source, &self.limits.xml)
            .map(Some)
            .map_err(|error| ScanError::Part { part: name, error })
    }

    /// Tags one part, unless it was already read; returns whether it was read now.
    fn scan_part(
        &mut self,
        part: &str,
        kind: PartKind,
        collected: &mut Collected,
    ) -> Result<bool, ScanError> {
        let key = opc::normalize(part);
        if !self.scanned.insert(key) {
            return Ok(false);
        }
        let Some(bytes) = self.read(part)? else {
            collected
                .notes
                .insert("a relationship points to a part that is missing");
            return Ok(false);
        };
        let mut tagger = PartTagger::new(collected, kind);
        xml::scan(&bytes, &self.limits.xml, &mut tagger).map_err(|error: XmlError| {
            ScanError::Part {
                part: part.to_owned(),
                error: OpcError::Xml(error),
            }
        })?;
        tagger.finish();
        Ok(true)
    }
}
