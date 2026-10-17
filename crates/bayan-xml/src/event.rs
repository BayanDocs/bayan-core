//! The events of the reader: what it found in the document, borrowed from it.

use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use crate::namespace::{KnownNamespace, KnownUri, Namespace};

/// The white space handling in force for an element and its text (`xml:space`, XML 1.0 §2.10). It is inherited by descendants until one of them sets it again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Space {
    /// `xml:space="default"`, or no `xml:space` in force: applications may handle white space as they usually do (WordprocessingML, for example, trims the text of `w:t`).
    #[default]
    Default,
    /// `xml:space="preserve"`: every white space character is significant.
    Preserve,
}

/// An interned namespace name: an index into the reader's table of the namespace names of a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct NamespaceId(pub(crate) u32);

/// A namespace name in the table.
#[derive(Debug)]
struct Entry<'a> {
    uri: Cow<'a, str>,
    known: Option<KnownUri>,
    /// The copy of the name that contexts, fragments and writers share, made the first time one needs it.
    shared: OnceCell<Arc<str>>,
}

/// The namespace names of a document, each stored once. Names without references in their declaration are borrowed from the text. What outlives the reader (contexts, the bindings fragments use) or compares names (writers) shares one copy of each name, made the first time it is needed, so that keeping or comparing a name never copies or reads it again, however long it is.
#[derive(Debug, Default)]
pub(crate) struct NamespaceTable<'a> {
    entries: Vec<Entry<'a>>,
    index: BTreeMap<Cow<'a, str>, NamespaceId>,
}

impl<'a> NamespaceTable<'a> {
    /// The identifier of `uri`, if it is in the table.
    pub(crate) fn find(&self, uri: &str) -> Option<NamespaceId> {
        self.index.get(uri).copied()
    }

    /// Adds `uri` if it is not in the table yet, and returns its identifier. `shared`, if given, is a copy of the name to share, such as the one a context holds.
    pub(crate) fn intern(&mut self, uri: Cow<'a, str>, shared: Option<&Arc<str>>) -> NamespaceId {
        if let Some(id) = self.find(&uri) {
            return id;
        }
        let id = NamespaceId(u32::try_from(self.entries.len()).unwrap_or(u32::MAX));
        let known = KnownNamespace::lookup(&uri);
        self.entries.push(Entry {
            uri: uri.clone(),
            known,
            shared: shared.cloned().map(OnceCell::from).unwrap_or_default(),
        });
        self.index.insert(uri, id);
        id
    }

    /// How many names the table holds.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// The namespace with identifier `id`.
    pub(crate) fn get(&self, id: NamespaceId) -> Option<Namespace<'_>> {
        let entry = self.entries.get(usize::try_from(id.0).ok()?)?;
        Some(Namespace::with_known(&entry.uri, entry.known))
    }

    /// The shared copy of the namespace name with identifier `id`.
    pub(crate) fn shared(&self, id: NamespaceId) -> Option<Arc<str>> {
        let entry = self.entries.get(usize::try_from(id.0).ok()?)?;
        Some(
            entry
                .shared
                .get_or_init(|| Arc::from(entry.uri.as_ref()))
                .clone(),
        )
    }
}

/// An element or attribute name, with its namespace resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Name<'r> {
    prefix: &'r str,
    local: &'r str,
    namespace: Option<Namespace<'r>>,
}

impl<'r> Name<'r> {
    pub(crate) fn new(prefix: &'r str, local: &'r str, namespace: Option<Namespace<'r>>) -> Self {
        Name {
            prefix,
            local,
            namespace,
        }
    }

    /// The prefix as written, `""` for none.
    pub fn prefix(&self) -> &'r str {
        self.prefix
    }

    /// The local part of the name.
    pub fn local(&self) -> &'r str {
        self.local
    }

    /// The namespace, `None` for a name in no namespace: an element without a prefix outside any default namespace declaration, or an attribute without a prefix. Namespace declarations (`xmlns`, `xmlns:p`) are in the namespace `http://www.w3.org/2000/xmlns/`.
    pub fn namespace(&self) -> Option<Namespace<'r>> {
        self.namespace
    }

    /// Whether this is `local` in `namespace` (by either of its names).
    pub fn is(&self, namespace: KnownNamespace, local: &str) -> bool {
        self.local == local && self.namespace.is_some_and(|ours| ours.is(namespace))
    }
}

/// Something the reader found in the document. Events borrow the reader and are valid until the next call to [`Reader::next_event`](crate::Reader::next_event).
#[derive(Clone, Debug)]
pub enum Event<'r> {
    /// A start tag, or an empty-element tag (`<a/>`), which is followed by an [`Event::End`] of its own.
    Start(StartTag<'r>),
    /// An end tag, or the end of an empty-element tag.
    End(EndTag<'r>),
    /// Character data between markup, with references resolved and line ends normalized. Text outside the root element (white space only) is not reported.
    Text(Text<'r>),
    /// A CDATA section.
    CData(CData<'r>),
    /// A comment.
    Comment(Comment<'r>),
    /// A processing instruction.
    ProcessingInstruction(ProcessingInstruction<'r>),
}

/// The data of an open element, kept while it is open.
#[derive(Clone, Debug)]
pub(crate) struct ElementData<'a> {
    pub(crate) qname: &'a str,
    pub(crate) prefix: &'a str,
    pub(crate) local: &'a str,
    pub(crate) namespace: Option<NamespaceId>,
    pub(crate) span: Range<usize>,
    pub(crate) empty: bool,
    pub(crate) depth: usize,
    pub(crate) space: Space,
}

/// The data of an attribute of the current start tag.
#[derive(Clone, Debug)]
pub(crate) struct AttributeData<'a> {
    pub(crate) prefix: &'a str,
    pub(crate) local: &'a str,
    pub(crate) namespace: Option<NamespaceId>,
    pub(crate) raw: &'a str,
    pub(crate) value: Cow<'a, str>,
    pub(crate) span: Range<usize>,
    pub(crate) value_span: Range<usize>,
    pub(crate) declaration: bool,
}

/// A start tag (`<w:p w14:paraId="1">`) or an empty-element tag (`<w:b/>`).
#[derive(Clone, Copy, Debug)]
pub struct StartTag<'r> {
    pub(crate) element: &'r ElementData<'r>,
    pub(crate) attributes: &'r [AttributeData<'r>],
    pub(crate) namespaces: &'r NamespaceTable<'r>,
}

impl<'r> StartTag<'r> {
    /// The element's name.
    pub fn name(&self) -> Name<'r> {
        element_name(self.element, self.namespaces)
    }

    /// The attributes, in the order they are written, namespace declarations included.
    pub fn attributes(&self) -> Attributes<'r> {
        Attributes {
            attributes: self.attributes.iter(),
            namespaces: self.namespaces,
        }
    }

    /// The attribute `local` in `namespace` (`None` for an attribute without a prefix), if the element has it.
    pub fn attribute(
        &self,
        namespace: Option<KnownNamespace>,
        local: &str,
    ) -> Option<Attribute<'r>> {
        self.attributes().find(|attribute| {
            let name = attribute.name();
            name.local() == local
                && !attribute.is_namespace_declaration()
                && match namespace {
                    None => name.namespace().is_none(),
                    Some(namespace) => name.namespace().is_some_and(|ours| ours.is(namespace)),
                }
        })
    }

    /// Where the tag is in the text, from `<` to `>`.
    pub fn span(&self) -> Range<usize> {
        self.element.span.clone()
    }

    /// Whether this is an empty-element tag (`<a/>`).
    pub fn is_empty_element(&self) -> bool {
        self.element.empty
    }

    /// The nesting depth of the element; the root element is at depth 1.
    pub fn depth(&self) -> usize {
        self.element.depth
    }

    /// The white space handling in force for the element's content.
    pub fn space(&self) -> Space {
        self.element.space
    }
}

fn element_name<'r>(element: &'r ElementData<'r>, namespaces: &'r NamespaceTable<'r>) -> Name<'r> {
    Name::new(
        element.prefix,
        element.local,
        element.namespace.and_then(|id| namespaces.get(id)),
    )
}

/// The attributes of a start tag, in the order they are written.
#[derive(Clone, Debug)]
pub struct Attributes<'r> {
    attributes: core::slice::Iter<'r, AttributeData<'r>>,
    namespaces: &'r NamespaceTable<'r>,
}

impl<'r> Iterator for Attributes<'r> {
    type Item = Attribute<'r>;

    fn next(&mut self) -> Option<Attribute<'r>> {
        let data = self.attributes.next()?;
        Some(Attribute {
            data,
            namespaces: self.namespaces,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.attributes.size_hint()
    }
}

impl ExactSizeIterator for Attributes<'_> {}

/// An attribute of a start tag.
#[derive(Clone, Copy, Debug)]
pub struct Attribute<'r> {
    data: &'r AttributeData<'r>,
    namespaces: &'r NamespaceTable<'r>,
}

impl<'r> Attribute<'r> {
    /// The attribute's name.
    pub fn name(&self) -> Name<'r> {
        Name::new(
            self.data.prefix,
            self.data.local,
            self.data.namespace.and_then(|id| self.namespaces.get(id)),
        )
    }

    /// The value, with references resolved and white space normalized as XML 1.0 §3.3.3 requires: every literal tab, line feed and carriage return becomes a space (a carriage return and line feed together, one space), while characters written as references are kept.
    pub fn value(&self) -> &'r str {
        &self.data.value
    }

    /// The value exactly as written between the quotes.
    pub fn raw_value(&self) -> &'r str {
        self.data.raw
    }

    /// Where the attribute is in the text, from its name to the closing quote.
    pub fn span(&self) -> Range<usize> {
        self.data.span.clone()
    }

    /// Where the value is in the text, between the quotes.
    pub fn value_span(&self) -> Range<usize> {
        self.data.value_span.clone()
    }

    /// Whether this is a namespace declaration (`xmlns` or `xmlns:prefix`).
    pub fn is_namespace_declaration(&self) -> bool {
        self.data.declaration
    }
}

/// An end tag (`</w:p>`), or the end of an empty-element tag.
#[derive(Clone, Debug)]
pub struct EndTag<'r> {
    pub(crate) element: &'r ElementData<'r>,
    pub(crate) span: Range<usize>,
    pub(crate) namespaces: &'r NamespaceTable<'r>,
}

impl<'r> EndTag<'r> {
    /// The element's name.
    pub fn name(&self) -> Name<'r> {
        element_name(self.element, self.namespaces)
    }

    /// Where the end tag is in the text, from `<` to `>`; for an empty-element tag, the empty span at its end.
    pub fn span(&self) -> Range<usize> {
        self.span.clone()
    }

    /// Where the whole element is in the text, from the `<` of its start tag to the `>` of its end tag: the element's source span.
    pub fn element_span(&self) -> Range<usize> {
        self.element.span.start..self.span.end
    }

    /// The nesting depth of the element; the root element is at depth 1.
    pub fn depth(&self) -> usize {
        self.element.depth
    }
}

/// Character data between two pieces of markup.
#[derive(Clone, Debug)]
pub struct Text<'r> {
    pub(crate) value: &'r str,
    pub(crate) raw: &'r str,
    pub(crate) span: Range<usize>,
    pub(crate) space: Space,
}

impl<'r> Text<'r> {
    /// The text, with references resolved and line ends normalized (XML 1.0 §2.11: a carriage return and line feed together, and a carriage return alone, become a line feed; carriage returns written as references are kept).
    pub fn value(&self) -> &'r str {
        self.value
    }

    /// The text exactly as written.
    pub fn raw(&self) -> &'r str {
        self.raw
    }

    /// Where the text is.
    pub fn span(&self) -> Range<usize> {
        self.span.clone()
    }

    /// The white space handling in force for the text.
    pub fn space(&self) -> Space {
        self.space
    }
}

/// A CDATA section (`<![CDATA[…]]>`).
#[derive(Clone, Debug)]
pub struct CData<'r> {
    pub(crate) value: &'r str,
    pub(crate) span: Range<usize>,
}

impl<'r> CData<'r> {
    /// The content, with line ends normalized.
    pub fn value(&self) -> &'r str {
        self.value
    }

    /// Where the section is, from `<![CDATA[` to `]]>`.
    pub fn span(&self) -> Range<usize> {
        self.span.clone()
    }
}

/// A comment (`<!--…-->`).
#[derive(Clone, Debug)]
pub struct Comment<'r> {
    pub(crate) text: &'r str,
    pub(crate) span: Range<usize>,
}

impl<'r> Comment<'r> {
    /// The text between `<!--` and `-->`, exactly as written.
    pub fn text(&self) -> &'r str {
        self.text
    }

    /// Where the comment is.
    pub fn span(&self) -> Range<usize> {
        self.span.clone()
    }
}

/// A processing instruction (`<?target data?>`).
#[derive(Clone, Debug)]
pub struct ProcessingInstruction<'r> {
    pub(crate) target: &'r str,
    pub(crate) data: &'r str,
    pub(crate) span: Range<usize>,
}

impl<'r> ProcessingInstruction<'r> {
    /// The target.
    pub fn target(&self) -> &'r str {
        self.target
    }

    /// The data after the target and the white space that follows it, exactly as written.
    pub fn data(&self) -> &'r str {
        self.data
    }

    /// Where the processing instruction is.
    pub fn span(&self) -> Range<usize> {
        self.span.clone()
    }
}
