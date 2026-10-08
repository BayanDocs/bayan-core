//! # bayan-crdt
//!
//! The adapter between BayanDocs and its CRDT library.
//!
//! Collaborative editing stores documents in a CRDT (a data structure that lets several people edit at once and always merges to the same result). This crate is the only one that talks to the CRDT library (Loro, [ADR-0008]); it exposes BayanDocs-shaped primitives, so the library can be replaced without touching the rest of the engine. No Loro type appears in its public interface.
//!
//! **Status:** a prototype written by the spike [CORE-004], which validates ADR-0007 and ADR-0008 (see `spikes/crdt-model/REPORT.md`). Its shape is what the spike evaluated; the production interface arrives with CORE-101.
//!
//! ## What it offers
//!
//! - [`Doc`]: one replica of a document, with snapshots, incremental updates, version checkout, and an [`Doc::import`] that treats every blob as untrusted input and enforces [`ImportLimits`].
//! - [`Story`]: a story of the document model (spec §3–§5), stored as rich text. Text characters are themselves; special atoms (paragraph ends, field delimiters, table blocks…) are placeholder characters bound to their entity by the mark [`ATOM_KEY`] ([`Story::insert_atom`]). Formatting, hyperlinks, comments and revisions are marks, each family with its own expansion behavior ([`MarkFamily`], [`Expand`]).
//! - [`Registry`] and [`PropertyMap`]: entities (paragraphs, tables, rows, cells, objects…) as maps of properties, keyed by identifier.
//! - [`IdList`]: movable lists of identifiers, for table rows and the cells of a row.
//! - [`UndoManager`]: undo and redo of this replica's own changes only.
//!
//! ## Untrusted input
//!
//! Every blob from another replica, from storage or from the network goes through [`Doc::import`], within [`ImportLimits`]. The limits and checks there narrow what a hostile blob can do; they cannot make Loro 1.16.2 safe (CORE-004 report, §7 and §8):
//!
//! - **Panics.** Loro panics on many crafted blobs. [`Doc::import`] contains such a panic where panics unwind (native builds) and poisons the document: from then on it and every handle to it refuse to work ([`CrdtError::Poisoned`], or an empty value where a method returns no error), because the library's internal locks are poisoned and any further call would panic outside the containment. Where panics abort (WebAssembly), the engine stops and its host must restart it and reload the last good state.
//! - **Panic messages.** Before the panic is contained, Rust's panic hook runs, and Loro's panic messages can quote document text. The adapter does not install a hook (it is process-wide): **a host must install a panic hook that never prints or records panic messages before it imports untrusted blobs** (docs AGENTS.md §6). bayan-ffi does so for panics inside the engine (CORE-007).
//! - **Aborts.** Some blobs make Loro abort the process (allocation failures, stack exhaustion while decoding deeply nested values), which nothing inside the process can catch. The engine's hosts must isolate the import of untrusted blobs and restart an engine that aborts.
//!
//! ## Layer
//!
//! bayan-crdt belongs to the **Model and formats** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-crdt may therefore depend only on Model and formats crates, on Foundation crates and on external libraries.
//!
//! [CORE-004]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-004-spike-crdt-document-model.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core
//! [ADR-0008]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0008-crdt-engine-and-local-first.md

#![forbid(unsafe_code)]

mod doc;
mod import;
mod map;
mod poison;
mod story;
mod undo;
mod value;

use std::fmt;

pub use doc::{Doc, DocStats, PeerId, Version, VersionVector};
pub use import::{ImportError, ImportLimits, ImportReport};
pub use map::{IdList, PropertyMap, Registry, Stories};
pub use story::{ATOM_KEY, Expand, MarkFamily, Run, Story};
pub use undo::{MAX_UNDO_STEPS, UndoManager};
pub use value::{MAX_VALUE_DEPTH, Value};

/// Why an operation on the document failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrdtError {
    /// A position or range lies outside the story or list.
    OutOfRange {
        /// The offending position.
        pos: usize,
        /// The length of the story or list.
        len: usize,
    },
    /// A value cannot be stored (see [`Value::Unsupported`] and [`MAX_VALUE_DEPTH`]).
    InvalidValue(&'static str),
    /// A mark family name is empty, contains a colon, or is repeated.
    InvalidFamily(String),
    /// A root container name that the CRDT library refuses when another replica decodes it (see [`Doc::registry`]).
    InvalidName(String),
    /// An import was refused.
    Import(ImportError),
    /// The document was poisoned by a hostile import (see [`ImportError::ValueTooDeep`] and [`ImportError::Panicked`]); it and every handle to it refuse to work.
    Poisoned,
    /// The CRDT library refused the operation, with its explanation.
    Library(String),
}

impl CrdtError {
    pub(crate) fn library(error: impl fmt::Display) -> Self {
        Self::Library(error.to_string())
    }
}

impl fmt::Display for CrdtError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange { pos, len } => {
                write!(formatter, "position {pos} is outside a sequence of {len}")
            }
            Self::InvalidValue(reason) => write!(formatter, "invalid value: {reason}"),
            Self::InvalidFamily(name) => write!(formatter, "invalid mark family name `{name}`"),
            Self::InvalidName(name) => write!(formatter, "invalid root container name `{name}`"),
            Self::Import(error) => write!(formatter, "import refused: {error}"),
            Self::Poisoned => formatter.write_str("the document was poisoned by a hostile import"),
            Self::Library(reason) => write!(formatter, "the CRDT library refused: {reason}"),
        }
    }
}

impl std::error::Error for CrdtError {}
