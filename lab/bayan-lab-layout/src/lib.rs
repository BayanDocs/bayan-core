//! # bayan-lab-layout
//!
//! The Fidelity Lab's layout JSON (work package [LAB-003]): one format in which Word's layout and BayanDocs' layout of the same document can be compared, and the extractor that reads it out of PDFs.
//!
//! Comparisons are fair only if both sides speak the same format ([Fidelity Lab specification][spec] §5). This crate defines it and produces Word's side:
//!
//! - [`layout`]: the layout JSON, version 1: pages, lines with their baselines and text offsets, glyphs with their origins, advances, fonts and sizes, and other positioned elements (images, rules, table cells), with lengths in points at a fixed precision, and in BLU too where the engine produced them. [`layout::Layout::check`] enforces its rules; [`schema`] generates its JSON Schema from the same types.
//! - [`text`]: the comparable form of a line's text, which removes the differences between producers that do not change what a page shows (spaces drawn or left as gaps, tabs, invisible characters, hyphen and ligature spellings).
//! - [`pdf`]: a strict reader of PDF files and an interpreter of their content streams that turns a PDF into a layout JSON: glyph origins from the text matrices, the current transformation matrix, `TJ` adjustments, character and word spacing and horizontal scaling; their characters from `ToUnicode` maps and encodings; glyphs grouped into lines by baseline and put in reading order, right-to-left lines included.
//!
//! `bayan-lab pdf-extract` runs the extractor on a file; `lab/schema/layout-json.md` documents the format and its rules for producers.
//!
//! ## Rules this crate keeps
//!
//! - **Hostile input:** every PDF is treated as hostile (ADR-0006). The reader reads only what extraction needs and refuses everything else with a clear error, never guessing or repairing: encrypted files, compression it does not implement (LZW and the image codecs), broken cross-reference tables, fonts whose widths it cannot know, vertical writing, unknown operators. Every size, count, depth and amount of work is limited ([`pdf::Limits`]), and the fuzz target `pdf_extract` in `fuzz/` exercises the reader and the interpreter together.
//! - **No outside world:** the crate reads the bytes it is given and returns a value; it opens no files, reads no environment and has no clock. `cargo xtask verify` checks this for the code that ships. Errors never quote a document's text or names, only object numbers, page numbers and counts.
//! - **Determinism:** the same PDF gives the same layout JSON, byte for byte, on every platform: ordered maps only, IEEE-754 basic operations only (no transcendental functions, ADR-0005 §4), and every result rounded to a thousandth of a point.
//!
//! ## Layer
//!
//! bayan-lab-layout belongs to the **Bindings and tools** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-lab-layout may therefore depend on crates of every layer and on external libraries, and no crate under `crates/` may depend on it.
//!
//! [LAB-003]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/LAB-003-layout-json-and-pdf-extraction.md
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core

#![forbid(unsafe_code)]

pub mod layout;
pub mod pdf;
pub mod schema;
pub mod text;
