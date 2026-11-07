//! Feature tagging: reads a `.docx` package, safely and within limits, and reports the coverage-matrix features, fonts, languages, scripts and compatibility settings it uses.
//!
//! [`scan`] opens the package with bayan-opc, the engine's package layer, which checks the ZIP container, the part names, the content types stream and every relationships part, and then reads the XML of the main document part and of the parts it relies on with bayan-xml, the engine's XML reader ([`events`]): styles, numbering, settings, the font table, the theme, headers, footers, footnotes, endnotes, comments and charts, plus the application properties. The glossary document (building blocks) is noted but not read, because its content is not part of the document. Nothing a package refers to outside itself is ever fetched, and nothing in it is run.
//!
//! The scanner reads documents exactly as strictly as BayanDocs does: a package that bayan-opc refuses, or a part that bayan-xml refuses, is refused, never read with a weaker reader, because a document that BayanDocs would refuse is a finding in itself. [`Limits::DEFAULT`] bounds every resource the scan uses: the package size, bayan-opc's limits for opening it (the number and sizes of its entries and the budgets of its metadata), the number of parts read, and bayan-xml's limits for each part (nesting depth, attributes, names and namespaces). A package that exceeds a limit is refused with an error that says which; the tests in `tests/lab/hostile_packages.rs` check each one with crafted files.

pub mod events;
pub mod features;
pub mod scripts;
pub mod tagger;
pub mod vocabulary;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use bayan_opc::{PackageError, PartName, Relationship, RelationshipSource, Target};
use features::Feature;
use scripts::Script;
use tagger::{Collected, PartKind, PartTagger};
use vocabulary::Rel;

/// The version of the tagging rules. Raise it whenever a rule changes what a document is tagged with; `corpus verify` then reports the documents tagged by an older version, and `corpus tag` tags them again.
pub const TAGGER_VERSION: u32 = 3;

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
    /// The limits bayan-opc opens the package with: its ZIP container, part names, content types stream and relationships parts, and the data of every part the tagger reads.
    pub package: bayan_opc::Limits,
    /// The limits bayan-xml reads each part the tagger reads with.
    pub xml: bayan_xml::Limits,
    /// The most parts the tagger reads from one package, besides the content types stream and the relationships parts that bayan-opc reads when it opens the package.
    pub max_parts: usize,
}

impl Limits {
    /// The limits the corpus tools use: the engine's own defaults, bayan-opc's and bayan-xml's, except that entries may uncompress to at most 256 MiB each and 1 GiB together (bayan-opc allows 512 MiB and 2 GiB), as the scanner always allowed. None is looser than the engine's: a document that BayanDocs would refuse is recorded as refused. Real documents stay far below them: in the 17,458 XML parts of the public corpus v1, no element has more than 76 attributes, no more than 74 namespace declarations are in scope at once, nothing is nested deeper than 29 elements, and no name is longer than 37 bytes.
    pub const DEFAULT: Self = Self {
        max_package_size: 512 * 1024 * 1024,
        package: bayan_opc::Limits {
            max_entry_size: 256 * 1024 * 1024,
            max_total_size: 1024 * 1024 * 1024,
            ..bayan_opc::Limits::DEFAULT
        },
        xml: bayan_xml::Limits::DEFAULT,
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
    /// bayan-opc refuses the package: its ZIP container, a part name, its content types stream or a relationships part, or the data of a part the tagger reads. BayanDocs would refuse it too.
    Package(bayan_opc::Error),
    /// A part every package needs is missing; the text names it.
    MissingPart(&'static str),
    /// The main part is not a WordprocessingML document; the text says what it is.
    NotADocument(&'static str),
    /// bayan-xml refuses a part the tagger reads.
    Part {
        /// The part's name (part names of packages are not document content).
        part: String,
        /// What is wrong with it.
        error: bayan_xml::Error,
    },
    /// More parts to read than [`Limits::max_parts`].
    TooManyParts,
}

impl fmt::Display for ScanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => formatter.write_str("the file is larger than the limit"),
            Self::OleCompoundFile => formatter.write_str("an OLE compound file (an encrypted document or a binary .doc), not a .docx package"),
            Self::Package(error) => error.fmt(formatter),
            Self::MissingPart(part) => write!(formatter, "the package has no {part}"),
            Self::NotADocument(what) => write!(formatter, "the main part is {what}, not a .docx document"),
            Self::Part { part, error } => write!(formatter, "part {}: {error}", printable(part, 100)),
            Self::TooManyParts => formatter.write_str("more parts to read than the limit"),
        }
    }
}

impl std::error::Error for ScanError {}

impl From<bayan_opc::Error> for ScanError {
    fn from(error: bayan_opc::Error) -> Self {
        match error {
            // Kept apart, as a part every package needs.
            bayan_opc::Error::Package(PackageError::MissingContentTypes) => {
                Self::MissingPart("[Content_Types].xml")
            }
            other => Self::Package(other),
        }
    }
}

impl ScanError {
    /// The error as a message, with the name of the part at fault only if `name_parts`: a package's part names are chosen by whoever made it, so messages about private documents leave them out.
    #[must_use]
    pub fn message(&self, name_parts: bool) -> String {
        match self {
            Self::Part { error, .. } if !name_parts => format!("a part: {error}"),
            _ => self.to_string(),
        }
    }
}

/// Whether a character must not reach a message, a report or the manifest from a document: a control character, or a character that is invisible or changes how the text around it reads, as in "Trojan Source" attacks on reviewers. The manifest is read by people and by AI agents, and both can be misled by text they cannot see.
#[must_use]
pub fn is_hidden_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            // Soft hyphen and combining grapheme joiner: invisible.
            '\u{00AD}' | '\u{034F}'
            // Arabic letter mark; Hangul and Khmer fillers and inherent vowels, which display as nothing.
            | '\u{061C}' | '\u{115F}' | '\u{1160}' | '\u{17B4}' | '\u{17B5}' | '\u{3164}' | '\u{FFA0}'
            // Mongolian variation selectors and vowel separator.
            | '\u{180B}'..='\u{180F}'
            // Zero-width characters and directional marks.
            | '\u{200B}'..='\u{200F}'
            // Line and paragraph separators, and the bidirectional embeddings and overrides.
            | '\u{2028}'..='\u{202E}'
            // Word joiner, invisible operators, bidirectional isolates and the deprecated format characters.
            | '\u{2060}'..='\u{206F}'
            // Variation selectors, the byte order mark and the interlinear annotation characters.
            | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}'
            // Tag characters, which can spell out hidden text, and the variation selectors supplement.
            | '\u{E0000}'..='\u{E007F}' | '\u{E0100}'..='\u{E01EF}'
        )
}

/// Text from outside (a part name, a path, a file name) made safe for a message: at most `max_chars` characters, with every [hidden character](is_hidden_character) replaced by `?`.
#[must_use]
pub fn printable(text: &str, max_chars: usize) -> String {
    text.chars()
        .take(max_chars)
        .map(|character| {
            if is_hidden_character(character) {
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
    let package = bayan_opc::Package::open(bytes, &limits.package)?;
    let mut reader = Parts {
        package: &package,
        limits,
        scanned: BTreeSet::new(),
        reads: 0,
    };
    let package_relationships =
        package
            .relationships(&RelationshipSource::Package)
            .ok_or(ScanError::MissingPart(
                "package relationships (/_rels/.rels)",
            ))?;
    let mut collected = Collected::default();
    collected.features.insert(Feature::OpcPackage);
    let mut main = None;
    for relationship in package_relationships.iter() {
        match (
            kind_of(relationship),
            target_of(relationship, &RelationshipSource::Package),
        ) {
            (Rel::OfficeDocument, Some(part)) if main.is_none() => main = Some(part),
            (Rel::ExtendedProperties, Some(part)) => {
                collected.features.insert(Feature::DocumentProperties);
                reader.scan_part(&part, PartKind::ApplicationProperties, &mut collected)?;
            }
            (kind, _) => tag_relationship(kind, &mut collected),
        }
    }
    let main = main.ok_or(ScanError::MissingPart("main document part"))?;
    // Media types are compared without regard to case (RFC 2045), as Word compares them.
    let content_type = package.content_type(&main).map(str::to_ascii_lowercase);
    match content_type.as_deref() {
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
    if !package.contains(&main) {
        return Err(ScanError::MissingPart("main document part"));
    }
    reader.scan_part(&main, PartKind::Document, &mut collected)?;
    // The parts the main document relies on, and the parts those rely on (images and charts of headers, for example).
    let mut pending = vec![main];
    while let Some(source) = pending.pop() {
        let source = RelationshipSource::Part(source);
        let Some(relationships) = package.relationships(&source) else {
            continue;
        };
        for relationship in relationships.iter() {
            let kind = kind_of(relationship);
            tag_relationship(kind, &mut collected);
            let Some(part) = target_of(relationship, &source) else {
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
            if !package.contains(&part) {
                collected
                    .notes
                    .insert("a relationship points to a part that is missing");
                continue;
            }
            if reader.scan_part(&part, part_kind, &mut collected)?
                && matches!(
                    part_kind,
                    PartKind::Header
                        | PartKind::Footer
                        | PartKind::Footnotes
                        | PartKind::Endnotes
                        | PartKind::Comments
                )
            {
                pending.push(part);
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

/// The part that an internal relationship from `source` points to, as bayan-opc resolves it; `None` for an external target, and for one that names no part.
fn target_of(relationship: &Relationship, source: &RelationshipSource) -> Option<PartName> {
    match relationship.resolve(source) {
        Ok(Target::Part(part)) => Some(part),
        Ok(Target::External(_)) | Err(_) => None,
    }
}

/// The kind of a relationship, from its type.
fn kind_of(relationship: &Relationship) -> Rel {
    vocabulary::relationship(relationship.relationship_type().trim())
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
        Rel::Model3d => Feature::Models3d,
        Rel::Unknown => Feature::UnknownPartsAndExtensions,
        // A comments part, its extensions and the people part (the authors of comments or of tracked changes) can exist without a single comment; comments count where `w:comment`, `w:commentReference` or `w15:commentEx` appear.
        Rel::Comments
        | Rel::CommentExtensions
        | Rel::OfficeDocument
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

/// Reads the parts of a package that the tagger needs, each at most once, and at most [`Limits::max_parts`] of them.
struct Parts<'p, 'a> {
    package: &'p bayan_opc::Package<'a>,
    limits: &'p Limits,
    /// The parts already read (two part names are equal when they are equivalent, as bayan-opc compares them).
    scanned: BTreeSet<PartName>,
    reads: usize,
}

impl Parts<'_, '_> {
    /// Tags one part, unless it was already read; returns whether it was read now.
    fn scan_part(
        &mut self,
        part: &PartName,
        kind: PartKind,
        collected: &mut Collected,
    ) -> Result<bool, ScanError> {
        if !self.scanned.insert(part.clone()) {
            return Ok(false);
        }
        if !self.package.contains(part) {
            collected
                .notes
                .insert("a relationship points to a part that is missing");
            return Ok(false);
        }
        self.reads += 1;
        if self.reads > self.limits.max_parts {
            return Err(ScanError::TooManyParts);
        }
        let bytes = self.package.read_part(part)?;
        let mut tagger = PartTagger::new(collected, kind);
        events::read(&bytes, &self.limits.xml, &mut tagger).map_err(|error| ScanError::Part {
            part: part.as_str().to_owned(),
            error,
        })?;
        tagger.finish();
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_characters_are_refused_and_visible_ones_kept() {
        for hidden in [
            '\u{0}',
            '\u{1B}',
            '\u{7F}',
            '\u{85}',
            '\u{00AD}',
            '\u{034F}',
            '\u{061C}',
            '\u{115F}',
            '\u{17B4}',
            '\u{180E}',
            '\u{200B}',
            '\u{200D}',
            '\u{200F}',
            '\u{2028}',
            '\u{2029}',
            '\u{202E}',
            '\u{2066}',
            '\u{206A}',
            '\u{206F}',
            '\u{3164}',
            '\u{FE0F}',
            '\u{FEFF}',
            '\u{FFA0}',
            '\u{FFF9}',
            '\u{E0001}',
            '\u{E0041}',
            '\u{E007F}',
            '\u{E0100}',
            '\u{E01EF}',
        ] {
            assert!(is_hidden_character(hidden), "U+{:04X}", u32::from(hidden));
        }
        for visible in [
            'a', 'Z', ' ', '-', 'é', 'ß', 'ع', 'א', '字', 'ㄱ', '한', '🙂', '\u{2010}',
        ] {
            assert!(
                !is_hidden_character(visible),
                "U+{:04X}",
                u32::from(visible)
            );
        }
        assert_eq!(printable("Ari\u{202E}la\u{E0041}", 100), "Ari?la?");
    }
}
