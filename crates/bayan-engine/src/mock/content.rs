//! The fixed content of the mock document: its blocks, in reading order.

/// A block of the mock document.
pub(crate) enum Block {
    /// A heading of the given level.
    Heading(u8, &'static str),
    /// A paragraph.
    Paragraph(&'static str),
    /// The editable line, which shows the text the shell sends (`input.text`, `input.composition`).
    Editable,
    /// A bulleted list.
    List(&'static [&'static str]),
    /// A table: a header row, then the body rows, all with the same number of columns.
    Table(&'static [&'static [&'static str]]),
    /// The next block starts on a new page.
    PageBreak,
}

/// The accessible name of the editable line.
pub(crate) const EDITABLE_NAME: &str = "Editable line";

/// The accessible name of the document.
pub(crate) const DOCUMENT_NAME: &str = "BayanDocs mock document";

/// The blocks, in reading order. The text is read by screen readers through the accessibility tree; on the page it is drawn as grey bars until the text pipeline exists (CORE-003).
pub(crate) const BLOCKS: &[Block] = &[
    Block::Heading(1, "BayanDocs mock document"),
    Block::Paragraph(
        "This document stands in for real documents until the engine can open them. Its text is drawn as grey bars, but screen readers and other assistive technology receive the real words through the accessibility tree.",
    ),
    Block::Paragraph(
        "The box below is the editable line. Text you type or compose with an input method appears in it, one box per character.",
    ),
    Block::Editable,
    Block::Heading(2, "A short list"),
    Block::List(&[
        "Scrolling and zooming across pages",
        "Typing and input method composition",
        "Reading the document with a screen reader",
    ]),
    Block::Heading(2, "A small table"),
    Block::Table(&[
        &["Shell", "Toolkit", "Language"],
        &["Desktop", "Qt Quick", "C++"],
        &["Web", "React", "TypeScript"],
    ]),
    Block::PageBreak,
    Block::Heading(2, "Why a mock document"),
    Block::Paragraph(
        "The engine skeleton exists so that the desktop and web shells can be built against a real engine long before the engine can read Word documents. It speaks the same protocol, on the same thread model, as the finished engine will.",
    ),
    Block::Paragraph(
        "Every page you see is drawn by the engine itself, in tiles that the shell requests as you scroll and zoom. The shell never decides where text goes; it only places the tiles and the caret that the engine describes.",
    ),
    Block::Paragraph(
        "Pressing a key, clicking or composing text with an input method sends a message to the engine. The engine changes the editable line on the first page, then tells the shell which part of the page to draw again.",
    ),
    Block::Paragraph(
        "A recording captures every message the engine receives. Replaying it with the same engine version reproduces the session exactly, which turns a problem seen on one computer into a test that fails on every computer.",
    ),
    Block::Paragraph(
        "If the engine ever fails while handling a message, it reports an error instead of taking the application down with it, and the shell starts a new session.",
    ),
    Block::PageBreak,
    Block::Heading(2, "The last page"),
    Block::Paragraph(
        "Tables, lists and headings in this document have their real structure in the accessibility tree, so a screen reader can move between them as it would in any other document.",
    ),
    Block::Paragraph(
        "Characters from any script can be typed into the editable line. Wide characters, such as those of Chinese, Japanese and Korean, take two columns.",
    ),
    Block::Paragraph(
        "The page size is US Letter, eight and a half by eleven inches, with margins of one inch.",
    ),
    Block::Paragraph("This is the end of the mock document."),
];
