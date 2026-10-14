//! # bayan-xml
//!
//! Streaming XML with Markup Compatibility processing and lossless preservation (work package [CORE-006]).
//!
//! **Status:** in progress.
//!
//! ## Layer
//!
//! bayan-xml belongs to the **Foundation** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-xml may therefore depend only on other Foundation crates and on external libraries.
//!
//! [CORE-006]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-006-bayan-xml.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core

#![forbid(unsafe_code)]

mod chars;
mod error;
mod event;
mod limits;
mod markup;
mod namespace;
mod reader;
mod source;

pub use error::{CompatibilityError, Error, ErrorKind, WriteError};
pub use event::{
    Attribute, Attributes, CData, Comment, EndTag, Event, Name, ProcessingInstruction, Space,
    StartTag, Text,
};
pub use limits::Limits;
pub use namespace::{KnownNamespace, KnownUri, Namespace, NamespaceName};
pub use reader::Reader;
pub use source::{Declaration, Encoding, Source};
