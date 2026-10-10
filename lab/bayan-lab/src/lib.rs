//! # bayan-lab
//!
//! The Fidelity Lab's corpus tooling (work package [LAB-001]): the corpus manifest, content-addressed storage, verification, and feature tagging of `.docx` documents.
//!
//! Every fidelity number the lab produces depends on the corpus it was measured on ([Fidelity Lab specification][spec] §3). This crate keeps that corpus honest:
//!
//! - [`manifest`]: the corpus manifest, a JSON file committed to Git that identifies every document by its SHA-256 and records its provenance, license, tier and detected features. The documents themselves live outside Git. [`schema`] generates the manifest's JSON Schema from the same Rust types.
//! - [`store`]: content-addressed storage for the documents (`objects/ab/cdef….docx`) and for the license texts of their sources, with a local-directory backend and an S3-compatible backend behind one interface.
//! - [`scan`]: a hardened scanner that reads a package's parts and element names, within limits, and tags the document with the features it uses, by the names of the [coverage matrix][matrix].
//! - [`exclusions`]: lists of documents a curator reviewed and kept out of a corpus, by SHA-256, each with its reason.
//! - [`stats`]: frequencies by feature, font, script, language and compatibility mode, as text, Markdown or JSON.
//! - [`cli`]: the `bayan-lab corpus …` commands that tie them together.
//!
//! **Status:** LAB-001. The scanner's ZIP and XML readers are minimal and marked for replacement by `bayan-opc` (CORE-005) and `bayan-xml` (CORE-006) once those exist.
//!
//! ## Rules this crate keeps
//!
//! - **Hostile input:** every document is treated as hostile. The scanner enforces limits on sizes, counts, depth and names, refuses document type declarations, never follows external references, and never runs anything a document contains ([`scan::Limits`]).
//! - **Privacy:** the tool never prints or records a document's text. It records only what it derives: feature names, font and program names, compatibility options from the standard's list, language tags and character counts per script, with control and invisible characters refused. Messages name documents by their SHA-256 and by the provenance path the operator chose to record (never for private documents, whose part names are left out too); reports contain only aggregate numbers (LAB-001, T2 corpus rules).
//! - **Determinism:** the same documents always give the same manifest, byte for byte, on every platform: ordered maps only, integer arithmetic only, no clock, no locale.
//!
//! ## Layer
//!
//! bayan-lab belongs to the **Bindings and tools** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-lab may therefore depend on crates of every layer and on external libraries, and no crate under `crates/` may depend on it.
//!
//! [LAB-001]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/LAB-001-corpus-infrastructure.md
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md
//! [matrix]: https://github.com/BayanDocs/docs/blob/HEAD/specs/coverage-matrix.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core

#![forbid(unsafe_code)]

pub mod cli;
pub mod exclusions;
pub mod hash;
pub mod manifest;
pub mod scan;
pub mod schema;
pub mod stats;
pub mod store;
