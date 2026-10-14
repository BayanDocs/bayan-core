//! The writer: deterministic, well-formed XML with namespace management, and verbatim fragments.

use std::collections::{BTreeMap, BTreeSet};

use crate::WriteError;
use crate::chars::{find_invalid_char, is_name, is_ncname, is_space};
use crate::escape;
use crate::event::{Name, Space, StartTag};
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

/// A name with its own strings, kept in an open start tag until it is written.
#[derive(Clone, Debug)]
struct OwnedName {
    namespace: Option<String>,
    prefix: String,
    local: String,
}

impl OwnedName {
    fn new(name: &QName<'_>) -> OwnedName {
        OwnedName {
            namespace: name.namespace.map(str::to_owned),
            prefix: name.prefix.to_owned(),
            local: name.local.to_owned(),
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
    declarations: Vec<(String, String)>,
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
/// - **Fragments** are written verbatim ([`Writer::fragment`]).
///
/// Each call checks its own arguments and changes nothing when it refuses them. The checks that need a whole start tag (conflicting prefixes, duplicate attributes, undeclared prefixes in Markup Compatibility attributes) run when the tag is written, which is when anything else is written after it; after such an error the writer should be dropped.
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
    bindings: Vec<(String, String)>,
    by_prefix: BTreeMap<String, Vec<usize>>,
}

impl Default for Writer {
    fn default() -> Self {
        Writer::new()
    }
}

/// Checks the syntax of a name given to the writer, for an element (`attribute` false) or an attribute.
fn check_name(name: &QName<'_>, attribute: bool) -> Result<(), WriteError> {
    if !is_ncname(name.local) || !(name.prefix.is_empty() || is_ncname(name.prefix)) {
        return Err(WriteError::InvalidName);
    }
    if attribute && name.namespace.is_none() && name.prefix.is_empty() && name.local == "xmlns" {
        return Err(WriteError::ReservedPrefix);
    }
    match name.namespace {
        None if !name.prefix.is_empty() => Err(if attribute {
            WriteError::InvalidAttributeName
        } else {
            WriteError::InvalidName
        }),
        None => Ok(()),
        Some("") => Err(WriteError::InvalidName),
        Some(uri) => {
            if find_invalid_char(uri).is_some() {
                return Err(WriteError::InvalidCharacter);
            }
            if name.prefix == "xmlns" || uri == XMLNS_NAMESPACE {
                return Err(WriteError::ReservedPrefix);
            }
            if (name.prefix == "xml") != (uri == XML_NAMESPACE) {
                return Err(WriteError::ReservedPrefix);
            }
            if attribute && name.prefix.is_empty() {
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
            bindings: Vec::new(),
            by_prefix: BTreeMap::new(),
        }
    }

    /// The namespace name bound to `prefix` where the writer is (`Some("")` for a default namespace undeclared with `xmlns=""`).
    fn bound(&self, prefix: &str) -> Option<&str> {
        if prefix == "xml" {
            return Some(XML_NAMESPACE);
        }
        let index = *self.by_prefix.get(prefix)?.last()?;
        self.bindings.get(index).map(|(_, uri)| uri.as_str())
    }

    fn bind(&mut self, prefix: &str, uri: &str) {
        self.by_prefix
            .entry(prefix.to_owned())
            .or_default()
            .push(self.bindings.len());
        self.bindings.push((prefix.to_owned(), uri.to_owned()));
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
        check_name(&name, false)?;
        if self.phase == Phase::Epilog {
            return Err(WriteError::ContentOutsideRoot);
        }
        self.flush()?;
        self.pending = Some(PendingTag {
            name: OwnedName::new(&name),
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
        check_characters(uri)?;
        let reserved = prefix == "xmlns"
            || uri == XMLNS_NAMESPACE
            || ((prefix == "xml") != (uri == XML_NAMESPACE))
            || (!prefix.is_empty() && uri.is_empty());
        if reserved {
            return Err(WriteError::ReservedPrefix);
        }
        let Some(pending) = self.pending.as_mut() else {
            return Err(WriteError::NoOpenElement);
        };
        pending
            .declarations
            .push((prefix.to_owned(), uri.to_owned()));
        Ok(())
    }

    /// Adds an attribute to the open start tag.
    ///
    /// # Errors
    ///
    /// [`WriteError::NoOpenElement`] without an open start tag; [`WriteError::InvalidName`], [`WriteError::ReservedPrefix`] or [`WriteError::InvalidAttributeName`] for a name that cannot be written (see [`QName`]); [`WriteError::InvalidCharacter`] for a value XML cannot represent.
    pub fn attribute(&mut self, name: QName<'_>, value: &str) -> Result<(), WriteError> {
        check_name(&name, true)?;
        check_characters(value)?;
        let Some(pending) = self.pending.as_mut() else {
            return Err(WriteError::NoOpenElement);
        };
        pending
            .attributes
            .push((OwnedName::new(&name), value.to_owned()));
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
            self.bind(prefix, uri);
        }
        let mut own_namespace = None;
        match tag.name.namespace.as_deref() {
            None => {
                if !self.bound("").unwrap_or_default().is_empty() {
                    if declared_here(self, "") {
                        return Err(WriteError::PrefixConflict);
                    }
                    self.bind("", "");
                    own_namespace = Some((String::new(), String::new()));
                }
            }
            Some(uri) => {
                if self.bound(&tag.name.prefix) != Some(uri) {
                    if declared_here(self, &tag.name.prefix) {
                        return Err(WriteError::PrefixConflict);
                    }
                    self.bind(&tag.name.prefix, uri);
                    own_namespace = Some((tag.name.prefix.clone(), uri.to_owned()));
                }
            }
        }
        let mut attribute_namespaces = Vec::new();
        let mut names = BTreeSet::new();
        for (name, _) in &tag.attributes {
            if !names.insert((name.namespace.as_deref(), name.local.as_str())) {
                return Err(WriteError::DuplicateAttribute);
            }
            if let Some(uri) = name.namespace.as_deref()
                && self.bound(&name.prefix) != Some(uri)
            {
                if declared_here(self, &name.prefix) {
                    return Err(WriteError::PrefixConflict);
                }
                self.bind(&name.prefix, uri);
                attribute_namespaces.push((name.prefix.clone(), uri.to_owned()));
            }
        }
        self.check_compatibility_attributes(tag)?;
        let space = tag
            .attributes
            .iter()
            .find(|(name, _)| {
                name.namespace.as_deref() == Some(XML_NAMESPACE) && name.local == "space"
            })
            .and_then(|(_, value)| match value.as_str() {
                "preserve" => Some(Space::Preserve),
                "default" => Some(Space::Default),
                _ => None,
            })
            .unwrap_or(self.space());
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
            escape::attribute(uri, out);
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

    /// Refuses Markup Compatibility attributes of `tag` that list prefixes not declared where the tag is.
    fn check_compatibility_attributes(&self, tag: &PendingTag) -> Result<(), WriteError> {
        let compatibility = KnownNamespace::MarkupCompatibility.uri();
        let choice =
            tag.name.namespace.as_deref() == Some(compatibility) && tag.name.local == "Choice";
        for (name, value) in &tag.attributes {
            let tokens = value.split_ascii_whitespace();
            let prefixes: Vec<&str> = match (name.namespace.as_deref(), name.local.as_str()) {
                (Some(uri), "Ignorable" | "MustUnderstand") if uri == compatibility => {
                    tokens.collect()
                }
                (None, "Requires") if choice => tokens.collect(),
                (Some(uri), "ProcessContent" | "PreserveElements" | "PreserveAttributes")
                    if uri == compatibility =>
                {
                    tokens
                        .map(|token| token.split_once(':').map_or("", |(prefix, _)| prefix))
                        .collect()
                }
                _ => continue,
            };
            if prefixes
                .iter()
                .any(|prefix| prefix.is_empty() || self.bound(prefix).is_none_or(str::is_empty))
            {
                return Err(WriteError::UndeclaredPrefix);
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
        self.start(QName::from_name(&tag.name()))?;
        for attribute in tag.attributes() {
            let name = attribute.name();
            if attribute.is_namespace_declaration() {
                let prefix = if name.prefix().is_empty() {
                    ""
                } else {
                    name.local()
                };
                self.declare(prefix, attribute.value())?;
            } else {
                self.attribute(QName::from_name(&name), attribute.value())?;
            }
        }
        Ok(())
    }

    /// Writes `fragment`, an element with its context, as content or as the root element.
    ///
    /// The fragment's text is written verbatim, byte for byte, where every binding of its context that it uses ([`Fragment::needed_namespaces`]) and its white space mode are the same as where the writer is. Elsewhere, declarations are added to its start tag for the namespaces it uses whose binding differs here, and `xml:space` if the white space mode differs and the element does not set it. This takes time in proportion to the fragment's text and the bindings it uses, not to its context.
    ///
    /// # Errors
    ///
    /// [`WriteError::ContentOutsideRoot`] after the root element. Writing an open start tag first can fail as [`Writer::end`] does.
    pub fn fragment(&mut self, fragment: &Fragment<'_>) -> Result<(), WriteError> {
        if self.phase == Phase::Epilog {
            return Err(WriteError::ContentOutsideRoot);
        }
        self.flush()?;
        let mut added = String::new();
        for binding in fragment.uses.iter() {
            if self.bound(&binding.prefix).unwrap_or_default() == &*binding.uri {
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
        let space = fragment.context.space();
        if !fragment.sets_space && self.space() != space {
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
