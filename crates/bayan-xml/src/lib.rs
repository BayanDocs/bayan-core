//! # bayan-xml
//!
//! The XML layer under every part of a document: a strict streaming reader, namespaces with the Strict and Transitional names of Office Open XML, Markup Compatibility processing (ECMA-376 Part 3), lossless capture of what an application does not understand, and a deterministic writer (work package [CORE-006]).
//!
//! Every part of a `.docx` file is XML that someone else wrote, so this crate treats it as hostile ([threat model] T1): it refuses document type declarations, and with them every entity but the five predefined ones, so that entity expansion is impossible by construction; it enforces [`Limits`] before the work they protect; it takes time in proportion to its input, also for input crafted to make it slower; it never fetches anything; and it contains no `unsafe` code and no C code ([ADR-0006]). It keeps what an application does not understand, so that a document can be written back without loss ([ADR-0007] §2–3, [ADR-0018], [document model §13]).
//!
//! ## What is here
//!
//! - [`Source`] decodes a part (UTF-8 with or without a byte order mark, UTF-16 with one), checks that every character is one XML allows, and reads the XML declaration. Every offset in this crate is a byte offset into its decoded text, [`Source::text`].
//! - [`Reader`] reads it event by event ([`Event`]): start tags with their attributes, end tags, text, CDATA sections, comments and processing instructions, each with its source span, names resolved to their namespaces and the `xml:space` mode in force. It refuses everything that XML 1.0 (fifth edition) and Namespaces in XML 1.0 (third edition) forbid, with an [`Error`] that gives a kind and an offset and never quotes the document. Underneath, quick-xml only splits the text into markup and character data; bayan-xml checks everything inside each piece itself, because quick-xml accepts much that XML forbids.
//! - [`KnownNamespace`] names the namespaces of Office Open XML, Microsoft's extensions to it, and the packaging and metadata vocabularies, with both names of each ECMA-376 vocabulary: the Transitional one (`http://schemas.openxmlformats.org/…`) and the Strict one (`http://purl.oclc.org/ooxml/…`) map to the same variant, so code matches a vocabulary once for both conformance classes.
//! - [`McReader`] applies Markup Compatibility as [`McConfig`] says which namespaces the application understands ([`McEvent`]): it leaves out elements and attributes of ignorable namespaces it does not understand, replaces elements that `mc:ProcessContent` names by their content, selects among the alternatives of `mc:AlternateContent` at every level of nesting, passes application-defined extension elements (Office Open XML's `extLst`) through unchanged, and stops at a namespace that `mc:MustUnderstand` names and the application does not understand. Everything it leaves out it reports whole, so that nothing is lost.
//! - [`Fragment`] keeps an element exactly as it was written, with its [`Context`] (the namespace bindings and white space mode it was in) and the bindings it uses: written back where it came from, it is the same bytes; written anywhere else, it gets the declarations it needs there ([`Reader::capture`], [`McReader::capture`], [`Writer::fragment`]). Source spans ([`StartTag::span`], [`EndTag::element_span`]) locate every element in the part.
//! - [`Writer`] writes XML deterministically: the same calls give the same bytes on every platform. It declares namespaces where they are needed, escapes text and attribute values so that reading them back gives the same characters, writes attributes in the order it is given them, adds `xml:space="preserve"` where text needs it ([`Writer::text_element`]), and refuses to write anything that would not read back as written.
//! - [`canonicalize`] gives a canonical form of a part for comparisons: the same for parts that mean the same however they are written (prefixes, attribute order, references, CDATA sections, empty-element tags), different for parts that do not.
//!
//! ```
//! use bayan_xml::{
//!     AttributeKind, KnownNamespace, Limits, McConfig, McEvent, McReader, NamespaceName, Source,
//!     Writer,
//! };
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let part = br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" mc:Ignorable="w14"><w:body><w:p w14:paraId="1A2B3C4D"><w:r><w:t>Hello</w:t></w:r></w:p></w:body></w:document>"#;
//! let limits = Limits::default();
//! let source = Source::new(part, &limits)?;
//! // A word processor that understands WordprocessingML, but not Word 2010's extensions.
//! let config = McConfig::new()
//!     .understand(NamespaceName::None)
//!     .understand(KnownNamespace::WordprocessingMl);
//! let mut reader = McReader::new(&source, &limits, &config);
//! let mut writer = Writer::new();
//! while let Some(event) = reader.next_event()? {
//!     match event {
//!         McEvent::Start(tag) => {
//!             // w14:paraId is not part of what the application interprets...
//!             if let Some((_, kind)) = tag.attributes().find(|(a, _)| a.name().local() == "paraId") {
//!                 assert_eq!(kind, AttributeKind::Ignored { preserve: false });
//!             }
//!             // ...but it is kept: the start tag is written back whole.
//!             writer.start_tag(tag.tag())?;
//!         }
//!         McEvent::End(_) => writer.end()?,
//!         McEvent::Text(text) => writer.text(text.value())?,
//!         _ => {}
//!     }
//! }
//! // Written back, the part is the same bytes.
//! assert_eq!(writer.finish()?.as_bytes(), part);
//! # Ok(())
//! # }
//! ```
//!
//! ## Limits
//!
//! Every limit is a field of [`Limits`], with a default chosen to read ordinary documents comfortably and stop malicious ones. Each is checked before the work it protects:
//!
//! | Limit | Default | Checked |
//! |---|---|---|
//! | [`Limits::max_size`] | 256 MiB | by [`Source::new`], before anything is decoded |
//! | [`Limits::max_depth`] | 256 | at each start tag, before its element is opened |
//! | [`Limits::max_attributes`] | 256, namespace declarations included | at each start tag, at the attribute beyond the limit, before it is read |
//! | [`Limits::max_name_length`] | 256 bytes | for element and attribute names with their prefix, processing instruction targets, entity names in references and the prefixes of a [`Context`] |
//! | [`Limits::max_namespaces`] | 4,096 distinct namespace names per document | at each declaration of a new name, before it is stored |
//! | [`Limits::max_compatibility_tokens`] | 256 names in one list | at each Markup Compatibility attribute, before its names are resolved |
//!
//! ## Time and memory
//!
//! Every byte is read once: reading, capturing a fragment and Markup Compatibility processing take time in proportion to the size of the part, and writing a fragment in proportion to its size and the bindings it uses, not to the bindings in scope. Resolving a prefix, checking the attributes of a start tag for duplicates and every lookup of Markup Compatibility take logarithmic time, so that crafted input (thousands of bindings in scope, the longest start tags, nesting as deep as allowed, the longest lists, deeply nested alternate content, many ignored elements) costs no more per byte than ordinary input; the tests `stays_linear_*` compare the two.
//!
//! What the reader keeps besides the document is bounded by the limits and by the part itself: the decoded text (none for UTF-8, which is borrowed; for UTF-16, a copy at most one and a half times its size), the open elements (at most [`Limits::max_depth`]), the namespace bindings in scope (at most [`Limits::max_depth`] times [`Limits::max_attributes`]) and the distinct namespace names (at most [`Limits::max_namespaces`]), the attributes of the current start tag (at most [`Limits::max_attributes`]), and one copy of the current run of text where references or line ends make it differ from the document, never longer than the run itself. [`McReader`] adds what the Markup Compatibility attributes in scope declare, at most [`Limits::max_compatibility_tokens`] names for each of them. A [`Fragment`] borrows its text from the document and shares its context with every other fragment of the same place; its own memory is the few bindings it uses. [`canonicalize`] builds its result in memory, a few times as large as the part at most.
//!
//! ## What is refused
//!
//! Besides everything XML 1.0 and Namespaces in XML forbid (malformed markup, names and references; characters XML does not allow, also as character references; `<` in attribute values and `]]>` in text; duplicate attributes, also when two prefixes name one namespace; undeclared prefixes, undeclaring a prefix with `xmlns:p=""`, and every misuse of the `xml` and `xmlns` prefixes and namespaces; content outside the root element and a second root element), the reader refuses, each with its own [`ErrorKind`]: document type declarations anywhere, and with them every entity but `&lt;`, `&gt;`, `&amp;`, `&apos;` and `&quot;` (an undefined entity is an error, never something to fetch); encodings other than UTF-8 and UTF-16, a UTF-16 document without a byte order mark, and an XML declaration whose encoding is not the bytes'; XML 1.1; an XML declaration anywhere but at the start, and a second byte order mark; and input beyond the [`Limits`]. Every reference stands for one character, never longer than the reference itself, so decoding never makes text longer. The crate has no input or output of its own: it never reads a file or opens a connection, whatever a document refers to.
//!
//! ## Markup Compatibility
//!
//! [`McReader`] follows the processing model of the fifth edition of ECMA-376 Part 3 (2015), §9. It also accepts the `mc:PreserveElements` and `mc:PreserveAttributes` attributes of the second edition (2008, §10.1.3), which later editions removed but documents written to the second edition may carry, checks them like `mc:ProcessContent`, and reports them as suggestions ([`IgnoredElement::preserve`], [`AttributeKind::Ignored`]). The second edition's rule that an `mc:Ignorable` attribute resets the process-content and preservation pairs of the namespaces it names is not applied; the fifth edition dropped it. Part 3 calls an element or attribute in a namespace that is neither understood nor ignorable a mismatch, which a processor reports and may continue after: [`McReader`] reports it ([`McStartTag::is_understood`], [`AttributeKind::NotUnderstood`]) and leaves the decision to the application, which can keep it; `mc:MustUnderstand` of a namespace that is not understood, and every breach of the syntax of Part 3 §7 that processing reaches, stop processing.
//!
//! ## Layer
//!
//! bayan-xml belongs to the **Foundation** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-xml may therefore depend only on other Foundation crates and on external libraries.
//!
//! [CORE-006]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-006-bayan-xml.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core
//! [threat model]: https://github.com/BayanDocs/docs/blob/HEAD/specs/threat-model.md
//! [document model §13]: https://github.com/BayanDocs/docs/blob/HEAD/specs/document-model.md#13-preservation-and-source-spans
//! [ADR-0006]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0006-rust-core-and-memory-safety.md
//! [ADR-0007]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0007-document-model.md
//! [ADR-0018]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0018-file-formats-and-priorities.md

#![forbid(unsafe_code)]

mod canonical;
mod chars;
mod compat;
mod error;
mod escape;
mod event;
mod fragment;
mod limits;
mod markup;
mod namespace;
mod reader;
mod source;
mod writer;

pub use canonical::canonicalize;
pub use compat::{
    AlternateContent, AttributeKind, IgnoredElement, McConfig, McEvent, McReader, McStartTag,
    Selection,
};
pub use error::{CompatibilityError, Error, ErrorKind, WriteError};
pub use event::{
    Attribute, Attributes, CData, Comment, EndTag, Event, Name, ProcessingInstruction, Space,
    StartTag, Text,
};
pub use fragment::{Context, Fragment};
pub use limits::Limits;
pub use namespace::{KnownNamespace, KnownUri, Namespace, NamespaceName};
pub use reader::Reader;
pub use source::{Declaration, Encoding, Source};
pub use writer::{QName, Writer};
