//! # bayan-lab-compare
//!
//! The Fidelity Lab's comparisons (work package [LAB-004]): how closely two layouts of the same document agree, as numbers, and the reports that collect those numbers over a corpus.
//!
//! The lab measures BayanDocs against Microsoft Word with metrics M1 to M7 ([Fidelity Lab specification][spec] §6). This crate computes them from two layout JSON files of one document ([`bayan_lab_layout::layout`]), Word's (the reference, extracted from the PDF Word exported) and BayanDocs' (the candidate, from the engine), and from the `.docx` file itself for M7:
//!
//! - [`stream`]: each layout's text as one stream in the comparable form, its lines joined where they share a baseline, with the position of every page and line start in it.
//! - [`align`]: the alignment of the two streams, a `diff` on characters that tolerates small differences such as numbering labels and field results, and the matching of breaks through it.
//! - [`metrics`]: the metrics of one document: M1 page count, M2 page-break agreement, M3 line-break agreement, M4 glyph position error, M7 agreement with Word's `w:lastRenderedPageBreak` hints, and how much of the text the alignment matched.
//! - [`hints`]: the `w:lastRenderedPageBreak` hints of a `.docx` file, read with bayan-opc and bayan-xml.
//! - [`visual`]: M5, the visual difference of two page rasters, behind the interface the lab will feed once it can rasterize Word's PDFs.
//! - [`histogram`]: exact integer histograms of glyph errors, from which medians and percentiles of any group of documents are computed.
//! - [`summary`]: a corpus run: every document's metrics, their aggregation by tier, feature, script and compatibility mode, the worst documents, the features most correlated with disagreement, and the documents that lost agreement relative to a baseline, as JSON.
//! - [`report`]: the same summary as a static HTML page with no scripts, no external assets and inline SVG charts.
//!
//! `bayan-lab compare` and `bayan-lab report` run them on files; `lab/compare.md` explains the metrics and the reports for people who read them. M6, round-trip integrity, is a process of the reference machine that this crate does not compute; `lab/compare.md` describes it.
//!
//! ## Rules this crate keeps
//!
//! - **Determinism:** the same inputs give the same numbers and the same bytes of every report on every platform: integer arithmetic only (percentages, percentiles and correlations included), ordered maps, and every list in a fixed order. Its tests also run in WebAssembly.
//! - **Hostile input:** a layout JSON is checked against its rules before it is compared ([`bayan_lab_layout::layout::Layout::from_json`]), and a `.docx` file is opened only through bayan-opc and bayan-xml, within their limits and this crate's ([`hints::Limits`]). Every amount of work is limited ([`align::Limits`]), so a crafted pair of layouts cannot keep a comparison busy for long.
//! - **Privacy:** no result or report contains a document's text: documents are named by their SHA-256 and, for public documents only, by the path their source gives them. Private (T2) documents appear in reports only as numbers inside the aggregates (Fidelity Lab specification §12).
//! - **No outside world:** the crate reads the bytes it is given and returns values; it opens no files, reads no environment and has no clock. `cargo xtask verify` checks this for the code that ships.
//!
//! ## Layer
//!
//! bayan-lab-compare belongs to the **Bindings and tools** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-lab-compare may therefore depend on crates of every layer and on external libraries, and no crate under `crates/` may depend on it.
//!
//! [LAB-004]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/LAB-004-metrics-and-report.md
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core

#![forbid(unsafe_code)]

pub mod align;
pub mod hints;
pub mod histogram;
pub mod metrics;
pub mod report;
pub mod stream;
pub mod summary;
pub mod visual;
