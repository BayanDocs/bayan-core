//! # bayan-model
//!
//! The Bayan Document Model.
//!
//! Documents are modelled the way Word models them internally: a set of **stories** (main text, headers and footers, notes, comments, text boxes, table cells), each a sequence of **atoms** (characters, paragraph ends, field delimiters, object anchors), with formatting as marks over ranges, tables as objects whose cells own stories, and everything BayanDocs does not understand preserved for round-trip ([document model specification][model-spec], [ADR-0007]).
//!
//! **Status:** a minimal but representative prototype written by the spike [CORE-004], which validates ADR-0007 and ADR-0008 (see `spikes/crdt-model/REPORT.md`). The production model arrives with CORE-101.
//!
//! ## What it offers
//!
//! - [`Document`]: a document stored in the CRDT (through bayan-crdt), with the operations of the brief (insert text, delete, format, split and merge paragraphs, tables with movable rows and columns, comments, fields, objects, bookmarks, moving a range), each one transaction and one undo step.
//! - [`RawDocument`]: the stored state as it is, which concurrent edits can leave inconsistent.
//! - [`normalize`]: the deterministic projection of the stored state onto a [`View`] (rules N1–N7, document model §14), which every replica computes identically and which never writes back by itself.
//! - [`check_invariants`]: the invariants I1–I7, checked on a view.
//! - [`simulation`]: seeded random editing by several replicas with partitions and reordered delivery, used by the convergence tests and the spike's long runs.
//!
//! ## Layer
//!
//! bayan-model belongs to the **Model and formats** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-model may therefore depend only on Model and formats crates, on Foundation crates and on external libraries.
//!
//! [CORE-004]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-004-spike-crdt-document-model.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core
//! [model-spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/document-model.md
//! [ADR-0007]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0007-document-model.md

#![forbid(unsafe_code)]

mod atoms;
mod ids;
mod invariants;
pub mod marks;
mod normalize;
mod raw;
pub mod simulation;
mod view;

pub use atoms::{AtomKind, decode_binding, encode_binding, is_text_character};
pub use ids::{EntityId, IdGenerator};
pub use invariants::{Violation, check_invariants};
pub use normalize::{
    MAX_TABLE_DEPTH, Report, Rule, SECTION_DEFAULTS, VirtualParagraphEnd, normalize,
};
pub use raw::{Props, RawDocument, RawRow, RawTable, registry};
pub use view::{Cell, Comment, Item, Row, Table, View};
