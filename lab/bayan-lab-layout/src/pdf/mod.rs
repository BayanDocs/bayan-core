//! The PDF extractor: a strict reader of PDF files and an interpreter of their content streams that turns a PDF into a layout JSON ([`extract`]).
//!
//! The Fidelity Lab measures Word's layout through the PDFs that Word exports on the reference machine ([Fidelity Lab specification][spec] §4 and §5): a PDF's text-showing operators give every glyph's exact origin. The extractor:
//!
//! 1. reads the file's structure (`file.rs`): the header, the cross-reference tables and streams with their incremental updates, object streams, and the stream filters that extraction needs (`filter.rs`), with every limit of [`Limits`];
//! 2. walks the page tree (`pages.rs`) and, for each page, interprets its content streams (`content.rs`): the graphics state with the current transformation matrix, text objects with their text and line matrices, `TJ` adjustments, character and word spacing, horizontal scaling and rise, form XObjects, and the boxes of images and rules;
//! 3. measures and decodes each glyph through its font (`font.rs`): widths from `Widths`, `W` and `DW`, characters from the `ToUnicode` map (`cmap.rs`) or the font's encoding (`encoding.rs`, `glyph_names.rs`);
//! 4. groups the glyphs into lines by baseline and drawing order, and puts each line in reading order, right-to-left lines included (`lines.rs`).
//!
//! Everything it does not understand is refused with an [`Error`] that names what and where (object numbers, page numbers), never the document's text: encrypted files, filters other than FlateDecode, ASCIIHexDecode, ASCII85Decode and RunLengthDecode, composite fonts other than `Identity-H`, unknown operators outside `BX`/`EX`, optional content. Glyphs whose characters cannot be known are kept with U+FFFD and counted ([`Extraction::unmapped_glyphs`]); glyphs that are not upright or not visible are counted and left off the lines ([`Extraction::skipped_glyphs`]).
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md

use std::fmt;

use crate::layout::{Layout, LineCoverage, SchemaVersion, Source, TextOffsets};

mod cmap;
mod content;
mod encoding;
mod file;
mod filter;
mod font;
mod glyph_names;
mod lexer;
mod lines;
mod object;
mod pages;

/// The version of the extraction rules: it changes whenever the same PDF would give another layout JSON, so that ground truth extracted with older rules can be found and extracted again.
pub const EXTRACTOR_VERSION: u32 = 1;

/// The `producer` of every layout JSON the extractor writes.
pub const PRODUCER: &str = "bayan-lab pdf-extract 1";

/// The longest number, keyword or name, in bytes. PDF's own limit for names is 127 bytes (ISO 32000-1, Annex C); no producer writes tokens anywhere near this long, so a longer one is refused as malformed.
pub const MAX_TOKEN_BYTES: usize = 255;

/// The most characters one glyph may stand for in a `ToUnicode` map: a ligature stands for two to four, and PDF allows 512 bytes (ISO 32000-2 §9.10.3). A longer mapping is refused as malformed.
pub const MAX_GLYPH_CHARACTERS: usize = 32;

/// The most filters one stream may name. Producers name one, or two such as `[/ASCII85Decode /FlateDecode]`; a longer chain is refused as malformed, so that preparing a stream's filters is a small, fixed amount of work at every use.
pub const MAX_FILTERS: usize = 8;

/// The limits the extractor enforces while it reads a PDF (ADR-0006 §5: every parser limits sizes, depths, counts and work).
///
/// A PDF is hostile input: a few kilobytes of FlateDecode can claim gigabytes, a cross-reference chain can loop, objects can nest without end or overlap so that the same bytes are parsed again and again, and a content stream can draw the same form XObject millions of times. Every limit is checked before the work it protects, except that the bytes a parse reads count as soon as the object, or the cross-reference table and its trailer, has been read; each limit has a test that reaches it. The defaults ([`Limits::DEFAULT`]) read the PDFs of very long documents and stop malicious ones.
///
/// # Memory
///
/// Everything the extractor holds grows only with what a limit counts. With the defaults it holds at most about 4.5 GiB, in the worst case of every limit reached at once; each figure is rounded up and includes the allocator's overhead and the spare capacity of growing vectors:
///
/// - the input, which the caller holds: [`Limits::max_input_bytes`], 256 MiB;
/// - the cross-reference entries, 4,194,304 ([`Limits::max_objects`]) × 48 bytes = 192 MiB, and the members that object streams declare, at most as many in all, × 16 bytes = 64 MiB;
/// - the parsed objects, which stay in memory until extraction ends: 2,097,152 values ([`Limits::max_object_values`]) × 128 bytes = 256 MiB, and the bytes of their strings, names and keys, which are copies of bytes read;
/// - those copies and all decoded stream data held at a time (object streams, form XObjects, the content being read), all of which count toward [`Limits::max_read_bytes`] when they are read: 512 MiB;
/// - while a stream is decoded and a page's content streams are joined, for a moment: 4 × [`Limits::max_stream_bytes`] = 256 MiB;
/// - the values waiting for operators: 65,536 ([`Limits::max_operand_values`]) × 128 bytes = 8 MiB, the space of the 17 lists that hold them while form XObjects nest ([`Limits::max_form_depth`] + 1, × 65,536 × 40 bytes = 43 MiB), and the bytes of their strings, copies of the content being read (64 MiB): 120 MiB;
/// - fonts: 2,097,152 entries ([`Limits::max_font_entries`]) × 256 bytes = 512 MiB (a `ToUnicode` entry of 32 characters takes about that), and about a kilobyte for each of 4,096 fonts ([`Limits::max_fonts`]), 4 MiB;
/// - glyphs: 4,194,304 ([`Limits::max_glyphs`], which counts a glyph once for each of its characters) × 512 bytes = 2 GiB while their page is built (each glyph as drawn, as placed on its line and as written into the layout, with its characters and the work of the Unicode Bidirectional Algorithm), of which at most 400 bytes a glyph stay in the layout;
/// - images and rules: 1,048,576 ([`Limits::max_elements`]) × 208 bytes = 208 MiB;
/// - paths: 1,048,576 segments and subpaths ([`Limits::max_path_segments`]) × 80 bytes = 80 MiB;
/// - pages: 65,536 ([`Limits::max_pages`]) × 512 bytes = 32 MiB, with the nodes of the page tree.
///
/// # Work
///
/// Every byte the reader reads counts toward [`Limits::max_read_bytes`]: every object, cross-reference table and trailer it parses, again for objects that overlap, and every stream's data at every use, filtered or not, together with every filter's output. With the defaults the reader therefore reads at most 512 MiB, and one object or table more before the count stops it (at most [`Limits::max_input_bytes`]); looking ahead, it reads some bytes again, a few times at most. Each of at most 100,000,000 operators ([`Limits::max_operators`]) costs a few lookups in sorted lists, each glyph a few more, fonts are read at most [`Limits::max_fonts`] times with at most [`Limits::max_font_entries`] entries in all, and putting glyphs into lines takes time in proportion to their number, or to it times its logarithm where they are sorted. With the defaults, extraction takes a few minutes at most.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The largest input, in bytes. Default: 256 MiB (Word's PDF of a 1,000-page report with images takes tens of megabytes).
    pub max_input_bytes: usize,
    /// The most objects the cross-reference sections may list, the highest object number plus one, and the most members all object streams together may declare. Default: 4,194,304.
    pub max_objects: usize,
    /// The deepest nesting of arrays and dictionaries, of objects read while another is read, of references to references, and of the page tree. Default: 64.
    pub max_nesting: usize,
    /// The most values parsed from the file's objects in total: every object (the trailers, the indirect objects and the members of object streams) and, at any depth, every item of its arrays and every value of its dictionaries. Parsed objects stay in memory until extraction ends: about 128 bytes for each value, and the bytes of its strings, names and keys, which count toward [`Limits::max_read_bytes`]. Default: 2,097,152.
    pub max_object_values: usize,
    /// The most values waiting for one content-stream operator: its operands and, at any depth, the items of the arrays and the values of the dictionaries among them; and the values of one inline image's dictionary. No operator of PDF 2.0 takes more than 33 operands, and the `TJ` array of a long line holds a few thousand items. Default: 65,536.
    pub max_operand_values: usize,
    /// The most cross-reference sections (each incremental update adds one; a hybrid file's stream counts too). Default: 64.
    pub max_xref_sections: usize,
    /// The most bytes one filter may decode a stream to; also the most bytes of an unfiltered stream, and of a page's content streams joined. Default: 64 MiB.
    pub max_stream_bytes: usize,
    /// The most bytes the reader reads in all: every object, cross-reference table and trailer it parses (objects can overlap, so that the same bytes are parsed for another object: they count again), and every stream's data at every use (a form XObject drawn a thousand times counts a thousand times), as it lies in the file, filtered or not, and as each filter puts it out. Default: 512 MiB.
    pub max_read_bytes: u64,
    /// The most pages. Default: 65,536.
    pub max_pages: usize,
    /// The most content-stream operators executed in the whole document, form XObjects counted each time they are drawn. Default: 100,000,000.
    pub max_operators: u64,
    /// The most glyphs shown in the whole document, those left off the lines included. A glyph counts once for each of its characters, and at least once (a ligature of three letters counts three), because the layout keeps every character. Default: 4,194,304 (about 1,400 dense pages).
    pub max_glyphs: u64,
    /// The deepest nesting of form XObjects. Default: 16.
    pub max_form_depth: usize,
    /// The deepest nesting of saved graphics states (`q`) and of marked content and compatibility sections. Default: 256.
    pub max_graphics_depth: usize,
    /// The most font dictionaries loaded. Default: 4,096.
    pub max_fonts: usize,
    /// The most entries of `ToUnicode` maps, `Widths`, `W` arrays, `Differences` and `CIDToGIDMap` streams of all fonts together. Default: 2,097,152.
    pub max_font_entries: usize,
    /// The most segments of the paths being built, each subpath counting as one more: the path of the content being read (a path ends when it is painted), and the paths that wait while the form XObjects their content draws are read. Default: 1,048,576.
    pub max_path_segments: usize,
    /// The most images and rules on all pages together. Default: 1,048,576.
    pub max_elements: usize,
}

impl Limits {
    /// The default limits, described with each field.
    pub const DEFAULT: Self = Self {
        max_input_bytes: 256 << 20,
        max_objects: 4_194_304,
        max_nesting: 64,
        max_object_values: 2_097_152,
        max_operand_values: 65_536,
        max_xref_sections: 64,
        max_stream_bytes: 64 << 20,
        max_read_bytes: 512 << 20,
        max_pages: 65_536,
        max_operators: 100_000_000,
        max_glyphs: 4_194_304,
        max_form_depth: 16,
        max_graphics_depth: 256,
        max_fonts: 4_096,
        max_font_entries: 2_097_152,
        max_path_segments: 1_048_576,
        max_elements: 1_048_576,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Which limit a PDF passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// [`Limits::max_input_bytes`].
    InputSize,
    /// [`Limits::max_objects`].
    Objects,
    /// [`Limits::max_nesting`].
    Nesting,
    /// [`Limits::max_object_values`].
    ObjectValues,
    /// [`Limits::max_operand_values`].
    OperandValues,
    /// [`Limits::max_xref_sections`].
    XrefSections,
    /// [`Limits::max_stream_bytes`].
    StreamSize,
    /// [`Limits::max_read_bytes`].
    ReadTotal,
    /// [`Limits::max_pages`].
    Pages,
    /// [`Limits::max_operators`].
    Operators,
    /// [`Limits::max_glyphs`].
    Glyphs,
    /// [`Limits::max_form_depth`].
    FormDepth,
    /// [`Limits::max_graphics_depth`].
    GraphicsDepth,
    /// [`Limits::max_fonts`].
    Fonts,
    /// [`Limits::max_font_entries`].
    FontEntries,
    /// [`Limits::max_path_segments`].
    PathSegments,
    /// [`Limits::max_elements`].
    Elements,
}

impl Limit {
    /// What the limit limits, and its field in [`Limits`].
    const fn describe(self) -> (&'static str, &'static str) {
        match self {
            Self::InputSize => ("the size of the input", "max_input_bytes"),
            Self::Objects => ("the number of objects", "max_objects"),
            Self::Nesting => ("nesting", "max_nesting"),
            Self::ObjectValues => (
                "the number of values in the file's objects",
                "max_object_values",
            ),
            Self::OperandValues => (
                "the number of values waiting for an operator",
                "max_operand_values",
            ),
            Self::XrefSections => (
                "the number of cross-reference sections",
                "max_xref_sections",
            ),
            Self::StreamSize => ("the decoded size of a stream", "max_stream_bytes"),
            Self::ReadTotal => ("the number of bytes read", "max_read_bytes"),
            Self::Pages => ("the number of pages", "max_pages"),
            Self::Operators => ("the number of content-stream operators", "max_operators"),
            Self::Glyphs => ("the number of glyphs", "max_glyphs"),
            Self::FormDepth => ("the nesting of form XObjects", "max_form_depth"),
            Self::GraphicsDepth => ("the nesting of graphics states", "max_graphics_depth"),
            Self::Fonts => ("the number of fonts", "max_fonts"),
            Self::FontEntries => ("the size of the fonts' tables", "max_font_entries"),
            Self::PathSegments => ("the number of segments of a path", "max_path_segments"),
            Self::Elements => ("the number of images and rules", "max_elements"),
        }
    }
}

/// Why a PDF could not be extracted. Messages name object numbers, page numbers and counts, never a document's text or names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The file breaks the PDF standard where extraction needs it.
    Malformed(String),
    /// The file uses something the extractor does not support.
    Unsupported(String),
    /// The file passes one of the [`Limits`].
    Limit(Limit),
}

impl Error {
    /// A [`Error::Malformed`] with `message`.
    pub(crate) fn malformed(message: impl Into<String>) -> Self {
        Self::Malformed(message.into())
    }

    /// A [`Error::Unsupported`] with `message`.
    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self::Unsupported(message.into())
    }

    /// The same error, saying on which page (counted from 1) it happened.
    pub(crate) fn on_page(self, page: usize) -> Self {
        match self {
            Self::Malformed(message) => Self::Malformed(format!("page {page}: {message}")),
            Self::Unsupported(message) => Self::Unsupported(format!("page {page}: {message}")),
            limit @ Self::Limit(_) => limit,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(message) => write!(formatter, "the PDF is malformed: {message}"),
            Self::Unsupported(message) => {
                write!(
                    formatter,
                    "the PDF uses something the extractor does not support: {message}"
                )
            }
            Self::Limit(limit) => {
                let (what, field) = limit.describe();
                write!(
                    formatter,
                    "the PDF exceeds the limit on {what} (Limits::{field})"
                )
            }
        }
    }
}

impl std::error::Error for Error {}

/// A PDF's layout and what the extractor could not place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extraction {
    /// The layout JSON.
    pub layout: Layout,
    /// Glyphs on the lines whose characters neither a `ToUnicode` map nor the font's encoding gives; their text is U+FFFD.
    pub unmapped_glyphs: u64,
    /// Glyphs left off the lines, on all pages together: text that is not upright, and invisible text (`Page::skipped_glyphs` counts them per page).
    pub skipped_glyphs: u64,
}

/// What the interpreter counts across the whole document.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Counters {
    /// Operators executed.
    pub(crate) operators: u64,
    /// Glyphs shown, each once for each of its characters and at least once.
    pub(crate) glyphs: u64,
    /// Images and rules found.
    pub(crate) elements: usize,
}

/// Extracts the layout JSON of the PDF `data`.
///
/// # Errors
///
/// An [`Error`] if the PDF is malformed, uses something the extractor does not support, or passes a limit; nothing is extracted then.
pub fn extract(data: &[u8], limits: &Limits) -> Result<Extraction, Error> {
    let mut file = file::File::open(data, *limits)?;
    let pages = pages::collect(&mut file)?;
    let mut fonts = font::FontCache::default();
    let mut counters = Counters::default();
    let mut stream = lines::Stream::default();
    let mut layout_pages = Vec::with_capacity(pages.len());
    let mut skipped = 0_u64;
    for page in &pages {
        let drawn = content::interpret_page(&mut file, page, &mut fonts, &mut counters)
            .map_err(|error| error.on_page(page.number))?;
        skipped += drawn.skipped;
        let layout_page = lines::build_page(page, drawn, &mut stream)
            .map_err(|error| error.on_page(page.number))?;
        layout_pages.push(layout_page);
    }
    let layout = Layout {
        schema: SchemaVersion::V1,
        source: Source::Pdf,
        producer: PRODUCER.to_owned(),
        text_offsets: TextOffsets::Lines,
        line_coverage: LineCoverage::All,
        page_count: u32::try_from(layout_pages.len()).map_err(|_| Error::Limit(Limit::Pages))?,
        fonts: fonts.into_layout_fonts(),
        pages: layout_pages,
    };
    // The extractor's own output must keep the format's rules; if it does not, that is a bug here, reported rather than written.
    layout.check().map_err(|message| {
        Error::malformed(format!(
            "the extracted layout breaks the format's rules ({message})"
        ))
    })?;
    Ok(Extraction {
        layout,
        unmapped_glyphs: stream.unmapped,
        skipped_glyphs: skipped,
    })
}

/// A PDF of `objects`, numbered from 1, with a cross-reference table and a trailer that names object 1 as the catalog: the files of the unit tests.
#[cfg(test)]
pub(crate) fn test_pdf(objects: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes());
    }
    let xref = out.len();
    let size = objects.len() + 1;
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}
