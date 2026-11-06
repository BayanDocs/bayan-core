//! The `w:lastRenderedPageBreak` hints of a `.docx` file, for M7: where Word's pages began when Word last laid the document out and saved it.
//!
//! Word writes an empty `w:lastRenderedPageBreak` element where a page began in its last layout (ECMA-376 Part 1 §17.3.3.13). The hints are approximate (Word may have laid the document out with other fonts or another printer, and it writes them only in the main text), but they cost nothing to read, so they give a signal on every document Word saved, the private corpus included, without running Word (Fidelity Lab specification §6, M7).
//!
//! [`read`] opens the package with bayan-opc, reads the main document part with bayan-xml's Markup Compatibility reader, and collects the displayed text of the body in the comparable form ([`bayan_lab_layout::text::comparable_chars`]) with the position of every hint in it. It follows the text rules of the layout JSON as far as the file shows them:
//!
//! - the text of `w:t`, `w:sym` (its character code), and `w:noBreakHyphen` (a hyphen) in runs of the body, its tables and its content controls, in document order;
//! - not deleted text (`w:delText`, and runs inside `w:del` and `w:moveFrom`), not field codes (`w:instrText`, and runs between a complex field's `begin` and `separate`), not runs hidden with `w:vanish` in their own properties, and not text boxes (`w:txbxContent`), which a page shows where they float, not where they are anchored;
//! - nothing for what the file does not hold as text: numbering labels, footnote reference marks, headers and footers.
//!
//! The alignment of M7 tolerates what this leaves out, as it tolerates labels and field results ([`crate::align`]). Hidden text set by a style, rather than in a run's own properties, is read as shown.
//!
//! The package and its XML are untrusted input: bayan-opc and bayan-xml check them within their limits ([`Limits`]), and this module keeps only the text it collects, its hints and one small entry per open element, also within limits. Errors say what is wrong, never what the document contains.

use core::fmt;

use bayan_lab_layout::text::comparable_chars;
use bayan_opc::{Package, RelationshipSource, Target};
use bayan_xml::{KnownNamespace, McConfig, McEvent, McReader, Source};

/// The relationship type of the main document part, in the Transitional and the Strict conformance class.
const MAIN_DOCUMENT: [&str; 2] = [
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument",
    "http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument",
];

/// The limits of [`read`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// bayan-opc's limits for the package.
    pub package: bayan_opc::Limits,
    /// bayan-xml's limits for the main document part.
    pub xml: bayan_xml::Limits,
    /// The most characters of text, in the comparable form, that the body may hold.
    pub max_chars: usize,
    /// The most hints the body may hold.
    pub max_hints: usize,
}

impl Limits {
    /// The defaults: those of bayan-opc and bayan-xml, 16 million characters (as [`crate::metrics::Limits::DEFAULT`]) and a million hints.
    pub const DEFAULT: Self = Self {
        package: bayan_opc::Limits::DEFAULT,
        xml: bayan_xml::Limits::DEFAULT,
        max_chars: 16 << 20,
        max_hints: 1 << 20,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The text of a document's body and its hints.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hints {
    /// The displayed text of the body, in the comparable form.
    pub chars: Vec<char>,
    /// The position in `chars` of every `w:lastRenderedPageBreak`, in document order (ascending, with repeats where Word began pages without text between them).
    pub breaks: Vec<usize>,
}

/// Why the hints of a file could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// bayan-opc refused the package.
    Package(bayan_opc::Error),
    /// The package has no main document part, or its relationship does not lead to a part of the package.
    NoMainDocument,
    /// bayan-xml refused the main document part.
    Xml(bayan_xml::Error),
    /// The body holds more characters than [`Limits::max_chars`].
    TooMuchText(usize),
    /// The body holds more hints than [`Limits::max_hints`].
    TooManyHints(usize),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Package(error) => write!(formatter, "the package was refused: {error}"),
            Self::NoMainDocument => formatter.write_str("the package has no main document part"),
            Self::Xml(error) => write!(formatter, "the main document part was refused: {error}"),
            Self::TooMuchText(limit) => write!(
                formatter,
                "the body holds more than {limit} characters in the comparable form"
            ),
            Self::TooManyHints(limit) => {
                write!(
                    formatter,
                    "the body holds more than {limit} page break hints"
                )
            }
        }
    }
}

impl std::error::Error for Error {}

/// Reads the body text and the hints of the `.docx` file `docx` (see the module's documentation).
///
/// # Errors
///
/// An [`Error`] if the package or its main document part is refused, or the body exceeds the limits.
pub fn read(docx: &[u8], limits: &Limits) -> Result<Hints, Error> {
    let package = Package::open(docx, &limits.package).map_err(Error::Package)?;
    let relationships = package
        .relationships(&RelationshipSource::Package)
        .ok_or(Error::NoMainDocument)?;
    let main = MAIN_DOCUMENT
        .iter()
        .find_map(|kind| relationships.of_type(kind).next())
        .ok_or(Error::NoMainDocument)?;
    let Ok(Target::Part(part)) = main.resolve(&RelationshipSource::Package) else {
        return Err(Error::NoMainDocument);
    };
    if !package.contains(&part) {
        return Err(Error::NoMainDocument);
    }
    let bytes = package.read_part(&part).map_err(Error::Package)?;
    read_document_part(&bytes, limits)
}

/// What an open element is to [`read_document_part`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Open {
    /// `w:body`.
    Body,
    /// `w:r`, a run.
    Run,
    /// `w:rPr` of a run.
    RunProperties,
    /// `w:t`, text.
    Text,
    /// An element whose content is not displayed, or is inside one.
    Skipped,
    /// A property element (`w:pPr`, `w:rPr` of a paragraph mark, `w:trPr` …), in which `w:del` and `w:moveFrom` are marks rather than deleted content.
    Properties,
    /// Anything else.
    Other,
}

/// Where a complex field is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    /// Between `begin` and `separate`: the field code, not displayed.
    Code,
    /// Between `separate` and `end`: the result, displayed.
    Result,
}

/// The state of [`read_document_part`]. Counters stand for what the open elements and fields hold, so that every event takes constant time however deep the nesting.
struct Walk {
    open: Vec<Open>,
    /// How many open elements are [`Open::Skipped`].
    skipped: usize,
    /// How many open elements are [`Open::Body`].
    bodies: usize,
    hints: Hints,
    hidden_run: bool,
    fields: Vec<Field>,
    /// How many of `fields` are in their code.
    codes: usize,
    limits: Limits,
}

impl Walk {
    const fn skipping(&self) -> bool {
        self.skipped > 0
    }

    const fn in_body(&self) -> bool {
        self.bodies > 0
    }

    /// Whether text here is displayed.
    const fn displayed(&self) -> bool {
        self.in_body() && !self.skipping() && !self.hidden_run && self.codes == 0
    }

    fn push(&mut self, kind: Open) {
        match kind {
            Open::Skipped => self.skipped += 1,
            Open::Body => self.bodies += 1,
            _ => {}
        }
        self.open.push(kind);
    }

    fn pop(&mut self) {
        match self.open.pop() {
            Some(Open::Skipped) => self.skipped -= 1,
            Some(Open::Body) => self.bodies -= 1,
            Some(Open::Run) => self.hidden_run = false,
            _ => {}
        }
    }

    fn push_text(&mut self, text: &str) -> Result<(), Error> {
        for (_, character) in comparable_chars(text) {
            if self.hints.chars.len() >= self.limits.max_chars {
                return Err(Error::TooMuchText(self.limits.max_chars));
            }
            self.hints.chars.push(character);
        }
        Ok(())
    }

    /// The kind of a start tag of WordprocessingML named `local`, inside `parent`, and what it does on the way.
    fn start(&mut self, local: &str, tag: &bayan_xml::StartTag<'_>) -> Result<Open, Error> {
        let parent = self.open.last().copied();
        let kind = match local {
            "body" => Open::Body,
            _ if !self.in_body() || self.skipping() => Open::Other,
            "txbxContent" | "delText" | "instrText" | "delInstrText" => Open::Skipped,
            "del" | "moveFrom"
                if parent != Some(Open::Properties) && parent != Some(Open::RunProperties) =>
            {
                Open::Skipped
            }
            "r" => {
                self.hidden_run = false;
                Open::Run
            }
            "rPr" if parent == Some(Open::Run) => Open::RunProperties,
            "vanish" if parent == Some(Open::RunProperties) => {
                let value = tag.attribute(Some(KnownNamespace::WordprocessingMl), "val");
                self.hidden_run =
                    !value.is_some_and(|value| matches!(value.value(), "0" | "false" | "off"));
                Open::Other
            }
            "t" => Open::Text,
            "noBreakHyphen" => {
                if self.displayed() {
                    self.push_text("-")?;
                }
                Open::Other
            }
            "sym" => {
                if self.displayed()
                    && let Some(code) =
                        tag.attribute(Some(KnownNamespace::WordprocessingMl), "char")
                    && let Some(character) = u32::from_str_radix(code.value(), 16)
                        .ok()
                        .and_then(char::from_u32)
                {
                    let mut buffer = [0; 4];
                    self.push_text(character.encode_utf8(&mut buffer))?;
                }
                Open::Other
            }
            "fldChar" => {
                let kind = tag.attribute(Some(KnownNamespace::WordprocessingMl), "fldCharType");
                match kind.as_ref().map(bayan_xml::Attribute::value) {
                    Some("begin") => {
                        self.fields.push(Field::Code);
                        self.codes += 1;
                    }
                    Some("separate") => {
                        if let Some(field) = self.fields.last_mut()
                            && *field == Field::Code
                        {
                            *field = Field::Result;
                            self.codes -= 1;
                        }
                    }
                    Some("end") if self.fields.pop() == Some(Field::Code) => self.codes -= 1,
                    _ => {}
                }
                Open::Other
            }
            "lastRenderedPageBreak" => {
                if self.hints.breaks.len() >= self.limits.max_hints {
                    return Err(Error::TooManyHints(self.limits.max_hints));
                }
                self.hints.breaks.push(self.hints.chars.len());
                Open::Other
            }
            _ if local.ends_with("Pr") || local.ends_with("PrChange") => Open::Properties,
            _ => Open::Other,
        };
        Ok(kind)
    }
}

/// Reads the body text and hints of a main document part.
///
/// # Errors
///
/// An [`Error`] if bayan-xml refuses the part or the body exceeds the limits.
pub fn read_document_part(bytes: &[u8], limits: &Limits) -> Result<Hints, Error> {
    let source = Source::new(bytes, &limits.xml).map_err(Error::Xml)?;
    let config = McConfig::new().understand(KnownNamespace::WordprocessingMl);
    let mut reader = McReader::new(&source, &limits.xml, &config);
    let mut walk = Walk {
        open: Vec::new(),
        skipped: 0,
        bodies: 0,
        hints: Hints::default(),
        hidden_run: false,
        fields: Vec::new(),
        codes: 0,
        limits: *limits,
    };
    while let Some(event) = reader.next_event().map_err(Error::Xml)? {
        match event {
            McEvent::Start(tag) => {
                let name = tag.name();
                let kind = if name
                    .namespace()
                    .is_some_and(|namespace| namespace.is(KnownNamespace::WordprocessingMl))
                {
                    walk.start(name.local(), tag.tag())?
                } else {
                    Open::Other
                };
                walk.push(kind);
            }
            McEvent::End(_) => walk.pop(),
            McEvent::Text(text) => {
                if walk.open.last() == Some(&Open::Text) && walk.displayed() {
                    walk.push_text(text.value())?;
                }
            }
            McEvent::CData(data) => {
                if walk.open.last() == Some(&Open::Text) && walk.displayed() {
                    walk.push_text(data.value())?;
                }
            }
            McEvent::Comment(_)
            | McEvent::ProcessingInstruction(_)
            | McEvent::Ignored(_)
            | McEvent::UnwrapStart(_)
            | McEvent::UnwrapEnd(_)
            | McEvent::AlternateContentStart(_)
            | McEvent::AlternateContentEnd(_) => {}
        }
    }
    Ok(walk.hints)
}
