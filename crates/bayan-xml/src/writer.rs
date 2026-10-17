//! The writer: deterministic, well-formed XML with namespace management, and verbatim fragments.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::WriteError;
use crate::chars::{find_invalid_char, is_name, is_ncname, is_space};
use crate::escape;
use crate::event::{Name, NamespaceId, Space, StartTag};
use crate::fragment::Fragment;
use crate::namespace::KnownNamespace;
use crate::reader::{XML_NAMESPACE, XMLNS_NAMESPACE};

/// A name to write: its namespace (`None` for none), the prefix to write it with, and its local part.
///
/// The writer uses the prefix as given: it declares it where it is not yet bound to the namespace, and never invents another. An element name without a prefix is in the default namespace (or in none, if `namespace` is `None`); an attribute name without a prefix is in no namespace, so an attribute in a namespace needs a prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QName<'s> {
    /// The namespace name (URI), `None` for a name in no namespace.
    pub namespace: Option<&'s str>,
    /// The prefix to write, `""` for none.
    pub prefix: &'s str,
    /// The local part.
    pub local: &'s str,
}

impl<'s> QName<'s> {
    /// A name in `namespace`, written with `prefix`.
    pub fn new(namespace: Option<&'s str>, prefix: &'s str, local: &'s str) -> QName<'s> {
        QName {
            namespace,
            prefix,
            local,
        }
    }

    /// A name in no namespace, without a prefix.
    pub fn unqualified(local: &'s str) -> QName<'s> {
        QName::new(None, "", local)
    }

    /// A name in a known namespace, with its Transitional name (ECMA-376 rule for writing, ADR-0018) and its conventional prefix.
    pub fn known(namespace: KnownNamespace, local: &'s str) -> QName<'s> {
        QName::new(Some(namespace.uri()), namespace.prefix(), local)
    }

    /// A name that the reader found, with the prefix it was written with.
    pub fn from_name(name: &Name<'s>) -> QName<'s> {
        QName::new(
            name.namespace().map(|namespace| namespace.uri()),
            name.prefix(),
            name.local(),
        )
    }
}

/// A namespace name that the writer has seen: its position in the writer's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct UriId(usize);

/// The empty namespace name, which undeclares the default namespace: the first name of every table.
const NO_NAMESPACE: UriId = UriId(0);
/// The namespace of the `xml` prefix: the second name of every table.
const XML_ID: UriId = UriId(1);
/// The namespace of namespace declarations: the third name of every table.
const XMLNS_ID: UriId = UriId(2);
/// The Markup Compatibility namespace: the fourth name of every table.
const COMPATIBILITY_ID: UriId = UriId(3);

/// The namespace names the writer has seen, each stored once and known by its position, so that bindings and names are compared by position, without reading the names.
///
/// A name given as text is found by comparing it with the names in the table, which takes time in proportion to its length, as reading the text does. A name that comes from a reader, in a start tag or a fragment it gave out, is the copy that the reader shares and is found by the address of that copy, without being read: the table keeps every copy it has looked up, so that no other name can take its address while the writer lives. Names from a reader therefore cost nothing per element or fragment, however long they are.
#[derive(Debug)]
struct Uris {
    names: Vec<Arc<str>>,
    by_text: BTreeMap<Arc<str>, UriId>,
    by_address: BTreeMap<usize, (Arc<str>, UriId)>,
}

impl Uris {
    fn new() -> Uris {
        let mut uris = Uris {
            names: Vec::new(),
            by_text: BTreeMap::new(),
            by_address: BTreeMap::new(),
        };
        for uri in [
            "",
            XML_NAMESPACE,
            XMLNS_NAMESPACE,
            KnownNamespace::MarkupCompatibility.uri(),
        ] {
            uris.insert(Arc::from(uri));
        }
        uris
    }

    fn insert(&mut self, uri: Arc<str>) -> UriId {
        let id = UriId(self.names.len());
        self.names.push(Arc::clone(&uri));
        self.by_text.insert(uri, id);
        id
    }

    /// The position of `uri`, given as text, or [`WriteError::InvalidCharacter`] for a name XML cannot represent.
    fn of_text(&mut self, uri: &str) -> Result<UriId, WriteError> {
        if let Some(&id) = self.by_text.get(uri) {
            return Ok(id);
        }
        check_characters(uri)?;
        Ok(self.insert(Arc::from(uri)))
    }

    /// The position of `uri`, the copy of a name that a reader shares, and checked.
    fn of_shared(&mut self, uri: &Arc<str>) -> UriId {
        // Readers make a new empty name wherever no default namespace is in force; it needs no lookup, and keeping each would only grow the table.
        if uri.is_empty() {
            return NO_NAMESPACE;
        }
        let address = Arc::as_ptr(uri).cast::<u8>().addr();
        if let Some(&(_, id)) = self.by_address.get(&address) {
            return id;
        }
        let id = match self.by_text.get(&**uri) {
            Some(&id) => id,
            None => self.insert(Arc::clone(uri)),
        };
        self.by_address.insert(address, (Arc::clone(uri), id));
        id
    }

    /// The name at `id`.
    fn text(&self, id: UriId) -> &str {
        self.names.get(id.0).map_or("", |name| name)
    }
}

/// A name with its own strings, kept in an open start tag until it is written.
#[derive(Clone, Debug)]
struct OwnedName {
    namespace: Option<UriId>,
    prefix: String,
    local: String,
}

impl OwnedName {
    fn new(prefix: &str, local: &str, namespace: Option<UriId>) -> OwnedName {
        OwnedName {
            namespace,
            prefix: prefix.to_owned(),
            local: local.to_owned(),
        }
    }

    fn write(&self, out: &mut String) {
        if !self.prefix.is_empty() {
            out.push_str(&self.prefix);
            out.push(':');
        }
        out.push_str(&self.local);
    }
}

/// A start tag that is open: it takes declarations and attributes until anything else is written.
#[derive(Debug)]
struct PendingTag {
    name: OwnedName,
    declarations: Vec<(String, UriId)>,
    attributes: Vec<(OwnedName, String)>,
}

/// An element whose start tag is written.
#[derive(Debug)]
struct OpenElement {
    qname: String,
    /// The number of bindings before the element's own.
    bindings: usize,
    space: Space,
}

/// Where the writer is in the document: the root element is not yet started, open, or finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Prolog,
    Root,
    Epilog,
}

/// Writes a document, deterministically: the same calls always give the same bytes, on every platform.
///
/// - **Namespaces:** names are written with the prefix they are given. Where that prefix is not already bound to the name's namespace, the writer declares it on the element being written, in this order: the element's own namespace, then the declarations asked for with [`Writer::declare`] in the order of the calls, then the namespaces of attributes in the order of the attributes. It refuses one prefix for two namespaces on one tag, and every use of the `xml` and `xmlns` prefixes and namespaces that Namespaces in XML forbids. Markup Compatibility attributes (`mc:Ignorable` and the others, and `Requires` on `mc:Choice`) may list only prefixes that are declared where they are written.
/// - **Attributes** are written in the order they are given; the writer never sorts them, so the order of generated content is the order of the code that writes it.
/// - **Escaping:** `&`, `<` and `>` in text; `&`, `<`, `>` and `"` in attribute values, which are always in double quotes; and the characters that reading would change otherwise as character references (carriage return in text, tab, line feed and carriage return in attribute values). Characters that XML cannot represent at all are refused.
/// - **Empty elements** are written as `<a/>`.
/// - **`xml:space`:** [`Writer::text_element`] adds `xml:space="preserve"` when the text needs it.
/// - **Fragments** ([`Writer::fragment`]) are written byte for byte where the bindings they use and their white space mode hold, and anywhere else with only the declarations and the `xml:space` they need added; a Markup Compatibility element, which may not have `xml:space`, is refused where its white space mode does not hold.
///
/// Each call checks its own arguments and changes nothing when it refuses them. The checks that need a whole start tag (conflicting prefixes, duplicate attributes, undeclared prefixes in Markup Compatibility attributes) run when the tag is written, which is when anything else is written after it; after such an error the writer should be dropped.
///
/// Writing takes time in proportion to what is written and to the arguments of each call: a namespace name given as text is read each time it is given. Names that come from a reader ([`Writer::start_tag`], [`Writer::fragment`]) are the copies it shares, which the writer recognizes without reading them, so that they cost nothing per element or fragment however long they are.
///
/// ```
/// use bayan_xml::{KnownNamespace, QName, Writer};
///
/// let mut writer = Writer::new();
/// writer.declaration(Some(true))?;
/// writer.whitespace("\r\n")?;
/// writer.start(QName::known(KnownNamespace::WordprocessingMl, "document"))?;
/// writer.start(QName::known(KnownNamespace::WordprocessingMl, "body"))?;
/// writer.start(QName::known(KnownNamespace::WordprocessingMl, "p"))?;
/// writer.start(QName::known(KnownNamespace::WordprocessingMl, "r"))?;
/// writer.text_element(QName::known(KnownNamespace::WordprocessingMl, "t"), "Hello ")?;
/// for _ in 0..4 {
///     writer.end()?;
/// }
/// let part = writer.finish()?;
/// assert!(part.ends_with("<w:r><w:t xml:space=\"preserve\">Hello </w:t></w:r></w:p></w:body></w:document>"));
/// # Ok::<(), bayan_xml::WriteError>(())
/// ```
#[derive(Debug)]
pub struct Writer {
    out: String,
    phase: Phase,
    pending: Option<PendingTag>,
    elements: Vec<OpenElement>,
    uris: Uris,
    bindings: Vec<(String, UriId)>,
    by_prefix: BTreeMap<String, Vec<usize>>,
}

impl Default for Writer {
    fn default() -> Self {
        Writer::new()
    }
}

/// Checks the syntax of the prefix and the local part of a name given to the writer.
fn check_parts(prefix: &str, local: &str) -> Result<(), WriteError> {
    if !is_ncname(local) || !(prefix.is_empty() || is_ncname(prefix)) {
        return Err(WriteError::InvalidName);
    }
    Ok(())
}

/// Checks a name for an element (`attribute` false) or an attribute, with its namespace looked up in the writer's table (`None` for no namespace), against the rules of Namespaces in XML.
fn check_namespace(
    prefix: &str,
    local: &str,
    namespace: Option<UriId>,
    attribute: bool,
) -> Result<(), WriteError> {
    match namespace {
        None if attribute && prefix.is_empty() && local == "xmlns" => {
            Err(WriteError::ReservedPrefix)
        }
        None if !prefix.is_empty() => Err(if attribute {
            WriteError::InvalidAttributeName
        } else {
            WriteError::InvalidName
        }),
        None => Ok(()),
        Some(NO_NAMESPACE) => Err(WriteError::InvalidName),
        Some(id) => {
            if prefix == "xmlns" || id == XMLNS_ID || (prefix == "xml") != (id == XML_ID) {
                return Err(WriteError::ReservedPrefix);
            }
            if attribute && prefix.is_empty() {
                return Err(WriteError::InvalidAttributeName);
            }
            Ok(())
        }
    }
}

/// Checks that `text` holds only characters XML can represent.
fn check_characters(text: &str) -> Result<(), WriteError> {
    match find_invalid_char(text) {
        Some(_) => Err(WriteError::InvalidCharacter),
        None => Ok(()),
    }
}

/// The white space mode inside the element of `tag`, written where `parent` is in force.
fn tag_space(tag: &PendingTag, parent: Space) -> Space {
    tag.attributes
        .iter()
        .find(|(name, _)| name.namespace == Some(XML_ID) && name.local == "space")
        .and_then(|(_, value)| match value.as_str() {
            "preserve" => Some(Space::Preserve),
            "default" => Some(Space::Default),
            _ => None,
        })
        .unwrap_or(parent)
}

/// Whether text needs `xml:space="preserve"` to keep its white space with every reader: white space at its start or end, a tab, line feed or carriage return, or two spaces in a row.
fn needs_preserve(text: &str) -> bool {
    let bytes = text.as_bytes();
    let edge = |byte: Option<&u8>| byte.copied().is_some_and(is_space);
    edge(bytes.first())
        || edge(bytes.last())
        || bytes
            .iter()
            .any(|&byte| matches!(byte, b'\t' | b'\n' | b'\r'))
        || text.contains("  ")
}

impl Writer {
    /// A writer with nothing written.
    pub fn new() -> Writer {
        Writer {
            out: String::new(),
            phase: Phase::Prolog,
            pending: None,
            elements: Vec::new(),
            uris: Uris::new(),
            bindings: Vec::new(),
            by_prefix: BTreeMap::new(),
        }
    }

    /// The namespace name bound to `prefix` where the writer is ([`NO_NAMESPACE`] for a default namespace undeclared with `xmlns=""`).
    fn bound(&self, prefix: &str) -> Option<UriId> {
        if prefix == "xml" {
            return Some(XML_ID);
        }
        let index = *self.by_prefix.get(prefix)?.last()?;
        self.bindings.get(index).map(|&(_, uri)| uri)
    }

    fn bind(&mut self, prefix: &str, uri: UriId) {
        self.by_prefix
            .entry(prefix.to_owned())
            .or_default()
            .push(self.bindings.len());
        self.bindings.push((prefix.to_owned(), uri));
    }

    fn unbind_to(&mut self, length: usize) {
        while self.bindings.len() > length {
            let Some((prefix, _)) = self.bindings.pop() else {
                break;
            };
            if let Some(stack) = self.by_prefix.get_mut(&prefix) {
                stack.pop();
                if stack.is_empty() {
                    self.by_prefix.remove(&prefix);
                }
            }
        }
    }

    /// The white space mode inside the innermost open element.
    fn space(&self) -> Space {
        self.elements
            .last()
            .map_or(Space::Default, |element| element.space)
    }

    /// Writes the XML declaration, `<?xml version="1.0" encoding="UTF-8"?>`, with `standalone="yes"` or `"no"` if asked. It must come first.
    ///
    /// # Errors
    ///
    /// [`WriteError::MisplacedDeclaration`] if anything was written before.
    pub fn declaration(&mut self, standalone: Option<bool>) -> Result<(), WriteError> {
        if !self.out.is_empty() || self.pending.is_some() {
            return Err(WriteError::MisplacedDeclaration);
        }
        self.out
            .push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"");
        match standalone {
            Some(true) => self.out.push_str(" standalone=\"yes\""),
            Some(false) => self.out.push_str(" standalone=\"no\""),
            None => {}
        }
        self.out.push_str("?>");
        Ok(())
    }

    /// Writes white space before or after the root element, such as the line break that Word writes after the XML declaration.
    ///
    /// # Errors
    ///
    /// [`WriteError::ContentOutsideRoot`] inside the root element (write text there), or if `text` is not white space only.
    pub fn whitespace(&mut self, text: &str) -> Result<(), WriteError> {
        if self.phase == Phase::Root || self.pending.is_some() || !text.bytes().all(is_space) {
            return Err(WriteError::ContentOutsideRoot);
        }
        self.out.push_str(text);
        Ok(())
    }

    /// Opens the start tag of an element, which takes [`Writer::declare`] and [`Writer::attribute`] until anything else is written.
    ///
    /// # Errors
    ///
    /// [`WriteError::InvalidName`] for a local part or prefix that is not an NCName, a prefix without a namespace, or an empty namespace name; [`WriteError::ReservedPrefix`] for the `xmlns` prefix or namespace, or the `xml` prefix with another namespace or the reverse; [`WriteError::InvalidCharacter`] for a namespace name XML cannot represent; [`WriteError::ContentOutsideRoot`] after the root element. Writing the open start tag of the parent first can fail as [`Writer::end`] does.
    pub fn start(&mut self, name: QName<'_>) -> Result<(), WriteError> {
        check_parts(name.prefix, name.local)?;
        let namespace = match name.namespace {
            Some(uri) => Some(self.uris.of_text(uri)?),
            None => None,
        };
        self.open(name.prefix, name.local, namespace)
    }

    /// Opens the start tag of the element `local`, written with `prefix`, in the namespace at `namespace`.
    fn open(
        &mut self,
        prefix: &str,
        local: &str,
        namespace: Option<UriId>,
    ) -> Result<(), WriteError> {
        check_namespace(prefix, local, namespace, false)?;
        if self.phase == Phase::Epilog {
            return Err(WriteError::ContentOutsideRoot);
        }
        self.flush()?;
        self.pending = Some(PendingTag {
            name: OwnedName::new(prefix, local, namespace),
            declarations: Vec::new(),
            attributes: Vec::new(),
        });
        Ok(())
    }

    /// Declares `prefix` (`""` for the default namespace) for `uri` (`""` to undeclare the default namespace) on the open start tag, whether it is needed or not.
    ///
    /// # Errors
    ///
    /// [`WriteError::NoOpenElement`] without an open start tag; [`WriteError::InvalidName`] for a prefix that is not an NCName; [`WriteError::ReservedPrefix`] for a declaration that Namespaces in XML forbids; [`WriteError::InvalidCharacter`] for a name XML cannot represent.
    pub fn declare(&mut self, prefix: &str, uri: &str) -> Result<(), WriteError> {
        if !(prefix.is_empty() || is_ncname(prefix)) {
            return Err(WriteError::InvalidName);
        }
        let uri = self.uris.of_text(uri)?;
        let reserved = prefix == "xmlns"
            || uri == XMLNS_ID
            || ((prefix == "xml") != (uri == XML_ID))
            || (!prefix.is_empty() && uri == NO_NAMESPACE);
        if reserved {
            return Err(WriteError::ReservedPrefix);
        }
        let Some(pending) = self.pending.as_mut() else {
            return Err(WriteError::NoOpenElement);
        };
        pending.declarations.push((prefix.to_owned(), uri));
        Ok(())
    }

    /// Adds an attribute to the open start tag.
    ///
    /// # Errors
    ///
    /// [`WriteError::NoOpenElement`] without an open start tag; [`WriteError::InvalidName`], [`WriteError::ReservedPrefix`] or [`WriteError::InvalidAttributeName`] for a name that cannot be written (see [`QName`]); [`WriteError::InvalidCharacter`] for a value XML cannot represent.
    pub fn attribute(&mut self, name: QName<'_>, value: &str) -> Result<(), WriteError> {
        check_parts(name.prefix, name.local)?;
        let namespace = match name.namespace {
            Some(uri) => Some(self.uris.of_text(uri)?),
            None => None,
        };
        self.add_attribute(name.prefix, name.local, namespace, value)
    }

    /// Adds the attribute `local`, written with `prefix`, in the namespace at `namespace`, to the open start tag.
    fn add_attribute(
        &mut self,
        prefix: &str,
        local: &str,
        namespace: Option<UriId>,
        value: &str,
    ) -> Result<(), WriteError> {
        check_namespace(prefix, local, namespace, true)?;
        check_characters(value)?;
        let Some(pending) = self.pending.as_mut() else {
            return Err(WriteError::NoOpenElement);
        };
        pending
            .attributes
            .push((OwnedName::new(prefix, local, namespace), value.to_owned()));
        Ok(())
    }

    /// Writes the open start tag, if there is one.
    fn flush(&mut self) -> Result<(), WriteError> {
        match self.pending.take() {
            Some(tag) => self.write_start_tag(&tag, false),
            None => Ok(()),
        }
    }

    /// Writes `tag`, as an empty-element tag if `empty`, after the checks that need the whole tag.
    fn write_start_tag(&mut self, tag: &PendingTag, empty: bool) -> Result<(), WriteError> {
        let mark = self.bindings.len();
        // Whether the tag already declares `prefix`: its innermost binding is one of the tag's own.
        let declared_here = |writer: &Writer, prefix: &str| {
            writer
                .by_prefix
                .get(prefix)
                .and_then(|stack| stack.last())
                .is_some_and(|&index| index >= mark)
        };
        for (prefix, uri) in &tag.declarations {
            if declared_here(self, prefix) {
                return Err(WriteError::DuplicateAttribute);
            }
            self.bind(prefix, *uri);
        }
        let mut own_namespace = None;
        match tag.name.namespace {
            None => {
                if self.bound("").unwrap_or(NO_NAMESPACE) != NO_NAMESPACE {
                    if declared_here(self, "") {
                        return Err(WriteError::PrefixConflict);
                    }
                    self.bind("", NO_NAMESPACE);
                    own_namespace = Some((String::new(), NO_NAMESPACE));
                }
            }
            Some(uri) => {
                if self.bound(&tag.name.prefix) != Some(uri) {
                    if declared_here(self, &tag.name.prefix) {
                        return Err(WriteError::PrefixConflict);
                    }
                    self.bind(&tag.name.prefix, uri);
                    own_namespace = Some((tag.name.prefix.clone(), uri));
                }
            }
        }
        let mut attribute_namespaces = Vec::new();
        let mut names = BTreeSet::new();
        for (name, _) in &tag.attributes {
            if !names.insert((name.namespace, name.local.as_str())) {
                return Err(WriteError::DuplicateAttribute);
            }
            if let Some(uri) = name.namespace
                && self.bound(&name.prefix) != Some(uri)
            {
                if declared_here(self, &name.prefix) {
                    return Err(WriteError::PrefixConflict);
                }
                self.bind(&name.prefix, uri);
                attribute_namespaces.push((name.prefix.clone(), uri));
            }
        }
        self.check_compatibility_attributes(tag)?;
        let space = tag_space(tag, self.space());
        let uris = &self.uris;
        let out = &mut self.out;
        out.push('<');
        tag.name.write(out);
        let declarations = own_namespace
            .iter()
            .chain(&tag.declarations)
            .chain(&attribute_namespaces);
        for (prefix, uri) in declarations {
            if prefix.is_empty() {
                out.push_str(" xmlns=\"");
            } else {
                out.push_str(" xmlns:");
                out.push_str(prefix);
                out.push_str("=\"");
            }
            escape::attribute(uris.text(*uri), out);
            out.push('"');
        }
        for (name, value) in &tag.attributes {
            out.push(' ');
            name.write(out);
            out.push_str("=\"");
            escape::attribute(value, out);
            out.push('"');
        }
        if empty {
            out.push_str("/>");
            self.unbind_to(mark);
            if self.elements.is_empty() {
                self.phase = Phase::Epilog;
            }
        } else {
            out.push('>');
            let mut qname = String::new();
            tag.name.write(&mut qname);
            self.elements.push(OpenElement {
                qname,
                bindings: mark,
                space,
            });
            self.phase = Phase::Root;
        }
        Ok(())
    }

    /// Refuses Markup Compatibility attributes of `tag` that list prefixes not declared where the tag is. The lists are checked name by name, without keeping them.
    fn check_compatibility_attributes(&self, tag: &PendingTag) -> Result<(), WriteError> {
        let choice = tag.name.namespace == Some(COMPATIBILITY_ID) && tag.name.local == "Choice";
        for (name, value) in &tag.attributes {
            // Whether the list holds qualified names (`prefix:local`) rather than prefixes.
            let qualified = match (name.namespace, name.local.as_str()) {
                (Some(COMPATIBILITY_ID), "Ignorable" | "MustUnderstand") => false,
                (None, "Requires") if choice => false,
                (
                    Some(COMPATIBILITY_ID),
                    "ProcessContent" | "PreserveElements" | "PreserveAttributes",
                ) => true,
                _ => continue,
            };
            for token in value.split_ascii_whitespace() {
                let prefix = if qualified {
                    token.split_once(':').map_or("", |(prefix, _)| prefix)
                } else {
                    token
                };
                if prefix.is_empty() || self.bound(prefix).is_none_or(|uri| uri == NO_NAMESPACE) {
                    return Err(WriteError::UndeclaredPrefix);
                }
            }
        }
        Ok(())
    }

    /// Closes the innermost element: writes its end tag, or the open start tag as an empty-element tag.
    ///
    /// # Errors
    ///
    /// [`WriteError::NoOpenElement`] when no element is open; for an open start tag, [`WriteError::PrefixConflict`], [`WriteError::DuplicateAttribute`] or [`WriteError::UndeclaredPrefix`] from the checks that need the whole tag.
    pub fn end(&mut self) -> Result<(), WriteError> {
        if let Some(tag) = self.pending.take() {
            return self.write_start_tag(&tag, true);
        }
        let Some(element) = self.elements.pop() else {
            return Err(WriteError::NoOpenElement);
        };
        self.out.push_str("</");
        self.out.push_str(&element.qname);
        self.out.push('>');
        self.unbind_to(element.bindings);
        if self.elements.is_empty() {
            self.phase = Phase::Epilog;
        }
        Ok(())
    }

    /// Checks that content can be written here: inside the root element, after writing an open start tag.
    fn content(&mut self) -> Result<(), WriteError> {
        self.flush()?;
        if self.elements.is_empty() {
            return Err(WriteError::ContentOutsideRoot);
        }
        Ok(())
    }

    /// Writes character data, escaped.
    ///
    /// # Errors
    ///
    /// [`WriteError::InvalidCharacter`] for a character XML cannot represent; [`WriteError::ContentOutsideRoot`] outside the root element.
    pub fn text(&mut self, text: &str) -> Result<(), WriteError> {
        check_characters(text)?;
        self.content()?;
        escape::text(text, &mut self.out);
        Ok(())
    }

    /// Writes an element that holds `text`, such as WordprocessingML's `w:t`, with `xml:space="preserve"` when the text has white space that a reader in the default mode might change (at its start or end, a tab, line feed or carriage return, or two spaces in a row) and preserve is not already in force.
    ///
    /// # Errors
    ///
    /// As [`Writer::start`] and [`Writer::text`].
    pub fn text_element(&mut self, name: QName<'_>, text: &str) -> Result<(), WriteError> {
        check_characters(text)?;
        self.start(name)?;
        if needs_preserve(text) && self.space() == Space::Default {
            self.attribute(QName::new(Some(XML_NAMESPACE), "xml", "space"), "preserve")?;
        }
        if !text.is_empty() {
            self.text(text)?;
        }
        self.end()
    }

    /// Writes `text` as a CDATA section, split where it holds `]]>` or a carriage return, which a CDATA section cannot.
    ///
    /// # Errors
    ///
    /// As [`Writer::text`].
    pub fn cdata(&mut self, text: &str) -> Result<(), WriteError> {
        check_characters(text)?;
        self.content()?;
        self.out.push_str("<![CDATA[");
        for (index, segment) in text.split('\r').enumerate() {
            if index > 0 {
                self.out.push_str("]]>&#xD;<![CDATA[");
            }
            self.out
                .push_str(&segment.replace("]]>", "]]]]><![CDATA[>"));
        }
        self.out.push_str("]]>");
        Ok(())
    }

    /// Writes a comment.
    ///
    /// # Errors
    ///
    /// [`WriteError::InvalidCharacter`] for a character XML cannot represent; [`WriteError::InvalidComment`] if `text` contains `--` or ends with `-`.
    pub fn comment(&mut self, text: &str) -> Result<(), WriteError> {
        check_characters(text)?;
        if text.contains("--") || text.ends_with('-') {
            return Err(WriteError::InvalidComment);
        }
        self.flush()?;
        self.out.push_str("<!--");
        self.out.push_str(text);
        self.out.push_str("-->");
        Ok(())
    }

    /// Writes a processing instruction.
    ///
    /// # Errors
    ///
    /// [`WriteError::InvalidName`] for a target that is not a name, contains a colon or is `xml` in any mix of cases; [`WriteError::InvalidCharacter`] for a character XML cannot represent; [`WriteError::InvalidProcessingInstruction`] for data that contains `?>` or starts with white space.
    pub fn processing_instruction(&mut self, target: &str, data: &str) -> Result<(), WriteError> {
        if !is_name(target) || target.contains(':') || target.eq_ignore_ascii_case("xml") {
            return Err(WriteError::InvalidName);
        }
        check_characters(data)?;
        if data.contains("?>") || data.bytes().next().is_some_and(is_space) {
            return Err(WriteError::InvalidProcessingInstruction);
        }
        self.flush()?;
        self.out.push_str("<?");
        self.out.push_str(target);
        if !data.is_empty() {
            self.out.push(' ');
            self.out.push_str(data);
        }
        self.out.push_str("?>");
        Ok(())
    }

    /// Opens a start tag like one the reader found: the same name and prefixes, the same declarations and attributes in the same order, with the values as read (escaped again where needed).
    ///
    /// # Errors
    ///
    /// As [`Writer::start`], [`Writer::declare`] and [`Writer::attribute`].
    pub fn start_tag(&mut self, tag: &StartTag<'_>) -> Result<(), WriteError> {
        let element = tag.element;
        check_parts(element.prefix, element.local)?;
        let namespace = self.shared(tag, element.namespace);
        self.open(element.prefix, element.local, namespace)?;
        for attribute in tag.attributes {
            if attribute.declaration {
                let prefix = if attribute.prefix.is_empty() {
                    ""
                } else {
                    attribute.local
                };
                self.declare(prefix, &attribute.value)?;
            } else {
                check_parts(attribute.prefix, attribute.local)?;
                let namespace = self.shared(tag, attribute.namespace);
                self.add_attribute(
                    attribute.prefix,
                    attribute.local,
                    namespace,
                    &attribute.value,
                )?;
            }
        }
        Ok(())
    }

    /// The position of the namespace with the reader's identifier `id` in the namespaces of `tag`, through the copy of its name that the reader shares.
    fn shared(&mut self, tag: &StartTag<'_>, id: Option<NamespaceId>) -> Option<UriId> {
        let uri = tag.namespaces.shared(id?)?;
        Some(self.uris.of_shared(&uri))
    }

    /// Writes `fragment`, an element with its context, as content or as the root element.
    ///
    /// The fragment's text is written verbatim, byte for byte, where every binding of its context that it uses ([`Fragment::needed_namespaces`]) and its white space mode are the same as where the writer is. Elsewhere, declarations are added to its start tag for the namespaces it uses whose binding differs here, and `xml:space` if the white space mode differs and the element does not set it. This takes time in proportion to the fragment's text and the bindings it uses, not to its context.
    ///
    /// Two kinds of element may not have `xml:space`, so they can be written only where their white space mode holds: elements of the Markup Compatibility namespace, such as the `mc:AlternateContent` that [`McEvent::AlternateContentEnd`](crate::McEvent::AlternateContentEnd) reports, which may have no attribute in the XML namespace (ECMA-376 Part 3, 5th edition, §7.1), and elements whose content Markup Compatibility processes in their place (§9.2), which [`McReader::capture`](crate::McReader::capture) marks when it captures one. Elements that [`Reader::capture`](crate::Reader::capture) or [`Fragment::new`] read without Markup Compatibility are marked only if they are in its namespace.
    ///
    /// # Errors
    ///
    /// [`WriteError::SpaceConflict`] if the fragment's root element may not have `xml:space` and its white space mode is not the one in force here; nothing is written then, and the open start tag stays open, so that `xml:space` can be added to it with [`Writer::attribute`] before the fragment is written again. [`WriteError::ContentOutsideRoot`] after the root element. Writing an open start tag first can fail as [`Writer::end`] does.
    pub fn fragment(&mut self, fragment: &Fragment<'_>) -> Result<(), WriteError> {
        if self.phase == Phase::Epilog {
            return Err(WriteError::ContentOutsideRoot);
        }
        // The white space mode where the fragment goes: inside the open start tag, if there is one.
        let here = self
            .pending
            .as_ref()
            .map_or(self.space(), |tag| tag_space(tag, self.space()));
        let space = fragment.context.space();
        let add_space = !fragment.sets_space && here != space;
        if add_space && fragment.fixed_space {
            return Err(WriteError::SpaceConflict);
        }
        self.flush()?;
        let mut added = String::new();
        for binding in fragment.uses.iter() {
            let uri = self.uris.of_shared(&binding.uri);
            if self.bound(&binding.prefix).unwrap_or(NO_NAMESPACE) == uri {
                continue;
            }
            if binding.prefix.is_empty() {
                added.push_str(" xmlns=\"");
            } else {
                added.push_str(" xmlns:");
                added.push_str(&binding.prefix);
                added.push_str("=\"");
            }
            escape::attribute(&binding.uri, &mut added);
            added.push('"');
        }
        if add_space {
            added.push_str(match space {
                Space::Preserve => " xml:space=\"preserve\"",
                Space::Default => " xml:space=\"default\"",
            });
        }
        if added.is_empty() {
            self.out.push_str(fragment.text());
        } else {
            self.out.push_str(&fragment.with_inserted(&added));
        }
        if self.elements.is_empty() {
            self.phase = Phase::Epilog;
        }
        Ok(())
    }

    /// The document written.
    ///
    /// # Errors
    ///
    /// [`WriteError::Unfinished`] if an element is still open or no root element was written.
    pub fn finish(self) -> Result<String, WriteError> {
        if self.pending.is_some() || !self.elements.is_empty() || self.phase != Phase::Epilog {
            return Err(WriteError::Unfinished);
        }
        Ok(self.out)
    }
}
