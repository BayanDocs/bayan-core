//! A minimal, strict XML parser for the package's three metadata parts: the content types stream, relationships parts and the core properties part.
//!
//! **Marked for replacement.** Work package CORE-006 builds `bayan-xml`, the engine's real XML layer (streaming, Markup Compatibility, lossless fragments). Until it lands, CORE-005 needs only this much XML, so this module implements exactly that and nothing more; once `bayan-xml` exists, the metadata parsers move onto it and this module is deleted.
//!
//! What it does, for small documents that fit in memory:
//!
//! - Decodes UTF-8 (with or without a byte order mark) and UTF-16 with a byte order mark, the only encodings packages may use (ECMA-376 Part 2 §6.2.5), and checks that the XML declaration agrees.
//! - Refuses document type declarations outright, so entity-expansion attacks cannot happen by construction; only the five predefined entities and character references are understood (§6.2.5 b).
//! - Checks well-formedness and Namespaces in XML: matching tags, unique attributes, declared prefixes, characters XML allows.
//! - Enforces the limits on depth, attributes per element and name length, and never recurses, so deep nesting cannot overflow the stack.
//! - Records the byte range of every element, tag, attribute and text in the decoded text, so callers can write unchanged markup back exactly as it was.

use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::Arc;

use crate::{Error, LimitError, Limits, XmlError, XmlErrorKind};

/// The namespace of the `xml:` prefix, which is always declared.
pub(crate) const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// The namespace of namespace declarations, which may not be bound to a prefix.
const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

/// The character encoding of a parsed document, kept so that a changed document can be written back in the same encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Encoding {
    /// UTF-8, with or without a byte order mark.
    Utf8 {
        /// Whether the document started with a byte order mark.
        bom: bool,
    },
    /// UTF-16, little-endian, with a byte order mark.
    Utf16Le,
    /// UTF-16, big-endian, with a byte order mark.
    Utf16Be,
}

/// A parsed document.
#[derive(Clone, Debug)]
pub(crate) struct Document {
    /// The document as UTF-8 text, without a byte order mark. All ranges refer to it.
    pub(crate) text: String,
    /// The encoding of the original bytes.
    pub(crate) encoding: Encoding,
    /// The root element.
    pub(crate) root: Element,
}

/// An element.
#[derive(Clone, Debug)]
pub(crate) struct Element {
    /// From the `<` of the start tag to the `>` of the end tag (or of the empty-element tag).
    pub(crate) span: Range<usize>,
    /// The start tag, or the empty-element tag.
    pub(crate) start_tag: Range<usize>,
    /// The end tag; `None` for an empty-element tag such as `<a/>`.
    pub(crate) end_tag: Option<Range<usize>>,
    /// The name as written, with its prefix.
    pub(crate) qualified_name: String,
    /// The name without its prefix.
    pub(crate) local_name: String,
    /// The namespace the name belongs to, shared with every other name in it rather than copied.
    pub(crate) namespace: Option<Arc<str>>,
    /// The attributes, without the namespace declarations.
    pub(crate) attributes: Vec<Attribute>,
    /// The content.
    pub(crate) children: Vec<Node>,
}

impl Element {
    /// Whether the element has the local name `local` in the namespace `namespace`.
    pub(crate) fn is(&self, namespace: &str, local: &str) -> bool {
        self.local_name == local && self.namespace.as_deref() == Some(namespace)
    }

    /// The value of the attribute without a namespace named `local`.
    pub(crate) fn attribute(&self, local: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|attribute| attribute.namespace.is_none() && attribute.local_name == local)
            .map(|attribute| attribute.value.as_str())
    }
}

/// An attribute.
#[derive(Clone, Debug)]
pub(crate) struct Attribute {
    /// The name without its prefix.
    pub(crate) local_name: String,
    /// The namespace the name belongs to, shared like an element's; attributes without a prefix have none.
    pub(crate) namespace: Option<Arc<str>>,
    /// The value, with references replaced and white space normalized as XML 1.0 §3.3.3 requires.
    pub(crate) value: String,
}

/// A piece of an element's content.
#[derive(Clone, Debug)]
pub(crate) enum Node {
    /// A child element.
    Element(Element),
    /// Character data, with references replaced and line ends normalized.
    Text {
        /// Where it is.
        span: Range<usize>,
        /// The text.
        value: String,
    },
    /// A CDATA section.
    CData {
        /// The text inside, with line ends normalized.
        value: String,
    },
    /// A comment or a processing instruction.
    Markup {
        /// Where it is.
        span: Range<usize>,
    },
}

/// Parses a metadata part, the content of the ZIP entry at `entry` (if known), within `limits`: first its size against [`Limits::max_metadata_size`], then its XML.
pub(crate) fn parse_part(
    bytes: &[u8],
    limits: &Limits,
    entry: Option<usize>,
) -> Result<Document, Error> {
    if bytes.len() > limits.max_metadata_size {
        return Err(LimitError::MetadataTooLarge {
            entry,
            limit: limits.max_metadata_size,
        }
        .into());
    }
    parse(bytes, limits).map_err(|error| Error::Xml { entry, error })
}

/// Parses `bytes` as an XML document, within `limits`.
pub(crate) fn parse(bytes: &[u8], limits: &Limits) -> Result<Document, XmlError> {
    let (text, encoding) = decode(bytes)?;
    check_characters(&text)?;
    let root = {
        let mut parser = Parser {
            text: &text,
            position: 0,
            limits,
            xml_namespace: Arc::from(XML_NAMESPACE),
        };
        parser.declaration(encoding)?;
        parser.miscellaneous()?;
        if !parser.at_start_tag() {
            return Err(parser.error(XmlErrorKind::ContentOutsideRoot));
        }
        let root = parser.element_tree()?;
        parser.miscellaneous()?;
        if parser.position != text.len() {
            return Err(parser.error(XmlErrorKind::ContentOutsideRoot));
        }
        root
    };
    Ok(Document {
        text,
        encoding,
        root,
    })
}

/// Decodes the bytes to text, after a UTF-8 or UTF-16 byte order mark if there is one.
fn decode(bytes: &[u8]) -> Result<(String, Encoding), XmlError> {
    let invalid = |offset| XmlError {
        offset,
        kind: XmlErrorKind::InvalidEncoding,
    };
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        let text = std::str::from_utf8(rest).map_err(|error| invalid(error.valid_up_to()))?;
        return Ok((text.to_owned(), Encoding::Utf8 { bom: true }));
    }
    let utf16 = if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        Some((rest, Encoding::Utf16Le))
    } else {
        bytes
            .strip_prefix(&[0xFE, 0xFF])
            .map(|rest| (rest, Encoding::Utf16Be))
    };
    if let Some((rest, encoding)) = utf16 {
        if rest.len() % 2 != 0 {
            return Err(invalid(rest.len()));
        }
        let (pairs, _) = rest.as_chunks::<2>();
        let units = pairs.iter().map(|&pair| {
            if encoding == Encoding::Utf16Le {
                u16::from_le_bytes(pair)
            } else {
                u16::from_be_bytes(pair)
            }
        });
        let mut text = String::with_capacity(rest.len() / 2);
        for character in char::decode_utf16(units) {
            text.push(character.map_err(|_| invalid(text.len()))?);
        }
        return Ok((text, encoding));
    }
    let text = std::str::from_utf8(bytes).map_err(|error| invalid(error.valid_up_to()))?;
    Ok((text.to_owned(), Encoding::Utf8 { bom: false }))
}

/// Checks that every character is one XML 1.0 allows (§2.2): no control characters other than tab, line feed and carriage return, and neither U+FFFE nor U+FFFF.
fn check_characters(text: &str) -> Result<(), XmlError> {
    match text
        .char_indices()
        .find(|&(_, character)| !is_xml_character(character))
    {
        Some((offset, _)) => Err(XmlError {
            offset,
            kind: XmlErrorKind::InvalidCharacter,
        }),
        None => Ok(()),
    }
}

/// Whether XML 1.0 allows `character` (§2.2, `Char`).
fn is_xml_character(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\r' | ' '..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

/// An element whose end tag has not been reached yet.
struct Open {
    element: Element,
    /// How many namespace bindings were in scope before its start tag.
    bindings_before: usize,
}

/// A namespace binding: a prefix (or `None` for the default namespace) and the namespace name (empty to undeclare the default namespace). The name is stored once, here, and every element and attribute in the namespace shares it.
struct Binding {
    prefix: Option<String>,
    namespace: Arc<str>,
}

/// What the parser found in an element's content.
enum Content {
    Node(Node),
    StartTag,
    EndTag {
        span: Range<usize>,
        qualified_name: String,
    },
}

struct Parser<'t> {
    text: &'t str,
    position: usize,
    limits: &'t Limits,
    /// The namespace of the `xml:` prefix, made once for the names that use it.
    xml_namespace: Arc<str>,
}

impl<'t> Parser<'t> {
    fn error(&self, kind: XmlErrorKind) -> XmlError {
        XmlError {
            offset: self.position,
            kind,
        }
    }

    fn rest(&self) -> &'t str {
        self.text.get(self.position..).unwrap_or_default()
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.rest().starts_with(prefix)
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    /// Skips white space and returns whether there was any.
    fn skip_space(&mut self) -> bool {
        let rest = self.rest();
        let trimmed = rest.trim_start_matches(is_space);
        self.position += rest.len() - trimmed.len();
        rest.len() != trimmed.len()
    }

    /// Consumes `literal`, or fails with `kind`.
    fn expect(&mut self, literal: &str, kind: XmlErrorKind) -> Result<(), XmlError> {
        if self.starts_with(literal) {
            self.position += literal.len();
            Ok(())
        } else if self.position >= self.text.len() {
            Err(self.error(XmlErrorKind::UnexpectedEnd))
        } else {
            Err(self.error(kind))
        }
    }

    /// Reads an XML name (§2.3, `Name`) within the length limit.
    fn name(&mut self) -> Result<&'t str, XmlError> {
        let rest = self.rest();
        let mut characters = rest.char_indices();
        match characters.next() {
            Some((_, first)) if is_name_start(first) => {}
            Some(_) => return Err(self.error(XmlErrorKind::Malformed)),
            None => return Err(self.error(XmlErrorKind::UnexpectedEnd)),
        }
        let length = characters
            .find(|&(_, character)| !is_name_character(character))
            .map_or(rest.len(), |(index, _)| index);
        if length > self.limits.max_xml_name_length {
            return Err(self.error(XmlErrorKind::NameTooLong));
        }
        self.position += length;
        Ok(rest.get(..length).unwrap_or_default())
    }

    /// Reads the XML declaration, if the document starts with one, and checks that it agrees with the encoding of the bytes (§4.3.3).
    fn declaration(&mut self, encoding: Encoding) -> Result<(), XmlError> {
        let is_declaration = self.starts_with("<?xml")
            && self
                .rest()
                .chars()
                .nth(5)
                .is_some_and(|character| is_space(character) || character == '?');
        if !is_declaration {
            return Ok(());
        }
        let malformed = XmlErrorKind::MalformedDeclaration;
        self.position += "<?xml".len();
        let mut declared = Vec::new();
        loop {
            let spaced = self.skip_space();
            if self.starts_with("?>") {
                self.position += 2;
                break;
            }
            if !spaced {
                return Err(self.error(malformed));
            }
            let name = self.name()?;
            self.skip_space();
            self.expect("=", malformed)?;
            self.skip_space();
            let quote = self.peek().filter(|&quote| quote == '"' || quote == '\'');
            let Some(quote) = quote else {
                return Err(self.error(malformed));
            };
            self.position += 1;
            let rest = self.rest();
            let end = rest
                .find(quote)
                .ok_or_else(|| self.error(XmlErrorKind::UnexpectedEnd))?;
            declared.push((name, rest.get(..end).unwrap_or_default()));
            self.position += end + 1;
        }
        // version, then optionally encoding, then optionally standalone, in this order.
        let mut fields = declared.into_iter().peekable();
        match fields.next() {
            Some(("version", "1.0")) => {}
            _ => return Err(self.error(malformed)),
        }
        if let Some(&("encoding", name)) = fields.peek() {
            fields.next();
            let utf16 = matches!(encoding, Encoding::Utf16Le | Encoding::Utf16Be);
            let agrees = if utf16 {
                name.eq_ignore_ascii_case("UTF-16")
            } else {
                name.eq_ignore_ascii_case("UTF-8")
            };
            if !agrees {
                return Err(self.error(XmlErrorKind::UnsupportedEncoding));
            }
        }
        if let Some(&("standalone", value)) = fields.peek() {
            fields.next();
            if value != "yes" && value != "no" {
                return Err(self.error(malformed));
            }
        }
        if fields.next().is_some() {
            return Err(self.error(malformed));
        }
        Ok(())
    }

    /// Skips white space, comments and processing instructions before or after the root element, and refuses a document type declaration.
    fn miscellaneous(&mut self) -> Result<(), XmlError> {
        loop {
            self.skip_space();
            if self.starts_with("<!--") {
                self.comment()?;
            } else if self.starts_with("<?") {
                self.processing_instruction()?;
            } else if self.starts_with("<!") {
                return Err(self.markup_declaration());
            } else {
                return Ok(());
            }
        }
    }

    /// The error for `<!` that starts neither a comment nor a CDATA section: a document type declaration is refused as such.
    fn markup_declaration(&self) -> XmlError {
        if self.starts_with("<!DOCTYPE") {
            self.error(XmlErrorKind::DoctypeNotAllowed)
        } else {
            self.error(XmlErrorKind::Malformed)
        }
    }

    /// Whether a start tag begins here.
    fn at_start_tag(&self) -> bool {
        let mut characters = self.rest().chars();
        characters.next() == Some('<') && characters.next().is_some_and(is_name_start)
    }

    /// Reads a comment; its text may not contain `--` (§2.5).
    fn comment(&mut self) -> Result<Range<usize>, XmlError> {
        let start = self.position;
        self.position += "<!--".len();
        let rest = self.rest();
        let end = rest
            .find("--")
            .ok_or_else(|| self.error(XmlErrorKind::UnexpectedEnd))?;
        if rest.get(end + 2..end + 3) != Some(">") {
            self.position += end;
            return Err(self.error(XmlErrorKind::Malformed));
        }
        self.position += end + 3;
        Ok(start..self.position)
    }

    /// Reads a processing instruction; its target may not be `xml` in any case (§2.6).
    fn processing_instruction(&mut self) -> Result<Range<usize>, XmlError> {
        let start = self.position;
        self.position += "<?".len();
        let target = self.name()?;
        if target.eq_ignore_ascii_case("xml") {
            return Err(self.error(XmlErrorKind::Malformed));
        }
        if !self.starts_with("?>") && !self.skip_space() {
            return Err(self.error(XmlErrorKind::Malformed));
        }
        let end = self
            .rest()
            .find("?>")
            .ok_or_else(|| self.error(XmlErrorKind::UnexpectedEnd))?;
        self.position += end + 2;
        Ok(start..self.position)
    }

    /// Reads the root element and everything inside it, keeping the open elements on an explicit stack instead of recursing.
    fn element_tree(&mut self) -> Result<Element, XmlError> {
        let mut stack: Vec<Open> = Vec::new();
        let mut bindings: Vec<Binding> = Vec::new();
        loop {
            // Here a start tag begins.
            if stack.len() >= self.limits.max_xml_depth {
                return Err(self.error(XmlErrorKind::TooDeep));
            }
            let bindings_before = bindings.len();
            let (element, empty) = self.start_tag(&mut bindings)?;
            let mut completed = if empty {
                bindings.truncate(bindings_before);
                Some(element)
            } else {
                stack.push(Open {
                    element,
                    bindings_before,
                });
                None
            };
            loop {
                if let Some(element) = completed.take() {
                    match stack.last_mut() {
                        Some(parent) => parent.element.children.push(Node::Element(element)),
                        None => return Ok(element),
                    }
                }
                match self.content()? {
                    Content::Node(node) => {
                        if let Some(parent) = stack.last_mut() {
                            parent.element.children.push(node);
                        }
                    }
                    Content::StartTag => break,
                    Content::EndTag {
                        span,
                        qualified_name,
                    } => {
                        let Some(open) = stack.pop() else {
                            return Err(self.error(XmlErrorKind::Malformed));
                        };
                        let mut element = open.element;
                        if qualified_name != element.qualified_name {
                            return Err(XmlError {
                                offset: span.start,
                                kind: XmlErrorKind::MismatchedEndTag,
                            });
                        }
                        element.span = element.span.start..span.end;
                        element.end_tag = Some(span);
                        bindings.truncate(open.bindings_before);
                        completed = Some(element);
                    }
                }
            }
        }
    }

    /// Reads the next piece of an element's content.
    fn content(&mut self) -> Result<Content, XmlError> {
        let start = self.position;
        if start >= self.text.len() {
            return Err(self.error(XmlErrorKind::UnexpectedEnd));
        }
        if self.starts_with("</") {
            self.position += 2;
            let qualified_name = self.name()?.to_owned();
            self.skip_space();
            self.expect(">", XmlErrorKind::Malformed)?;
            return Ok(Content::EndTag {
                span: start..self.position,
                qualified_name,
            });
        }
        if self.starts_with("<!--") {
            let span = self.comment()?;
            return Ok(Content::Node(Node::Markup { span }));
        }
        if self.starts_with("<![CDATA[") {
            self.position += "<![CDATA[".len();
            let content_start = self.position;
            let end = self
                .rest()
                .find("]]>")
                .ok_or_else(|| self.error(XmlErrorKind::UnexpectedEnd))?;
            let raw = self
                .text
                .get(content_start..content_start + end)
                .unwrap_or_default();
            self.position += end + 3;
            return Ok(Content::Node(Node::CData {
                value: normalize_line_ends(raw),
            }));
        }
        if self.starts_with("<?") {
            let span = self.processing_instruction()?;
            return Ok(Content::Node(Node::Markup { span }));
        }
        if self.starts_with("<!") {
            return Err(self.markup_declaration());
        }
        if self.starts_with("<") {
            if self.at_start_tag() {
                return Ok(Content::StartTag);
            }
            return Err(self.error(XmlErrorKind::Malformed));
        }
        let rest = self.rest();
        let end = rest.find('<').unwrap_or(rest.len());
        let raw = rest.get(..end).unwrap_or_default();
        if let Some(offset) = raw.find("]]>") {
            self.position += offset;
            return Err(self.error(XmlErrorKind::Malformed));
        }
        let value = decode_references(raw, start, false)?;
        self.position += end;
        Ok(Content::Node(Node::Text {
            span: start..self.position,
            value,
        }))
    }

    /// Reads a start tag or empty-element tag, declares its namespaces in `bindings` and resolves its names. Returns the element (without content yet) and whether the tag was an empty-element tag.
    fn start_tag(&mut self, bindings: &mut Vec<Binding>) -> Result<(Element, bool), XmlError> {
        let start = self.position;
        self.position += 1;
        let qualified_name = self.name()?.to_owned();
        let mut raw_attributes: Vec<(String, String, Range<usize>)> = Vec::new();
        let empty = loop {
            let spaced = self.skip_space();
            if self.starts_with("/>") {
                self.position += 2;
                break true;
            }
            if self.starts_with(">") {
                self.position += 1;
                break false;
            }
            if self.position >= self.text.len() {
                return Err(self.error(XmlErrorKind::UnexpectedEnd));
            }
            if !spaced {
                return Err(self.error(XmlErrorKind::Malformed));
            }
            let attribute_start = self.position;
            let name = self.name()?.to_owned();
            self.skip_space();
            self.expect("=", XmlErrorKind::Malformed)?;
            self.skip_space();
            let quote = self.peek().filter(|&quote| quote == '"' || quote == '\'');
            let Some(quote) = quote else {
                return Err(self.error(XmlErrorKind::Malformed));
            };
            self.position += 1;
            let value_start = self.position;
            let rest = self.rest();
            let end = rest
                .find(quote)
                .ok_or_else(|| self.error(XmlErrorKind::UnexpectedEnd))?;
            let raw = rest.get(..end).unwrap_or_default();
            if let Some(offset) = raw.find('<') {
                self.position += offset;
                return Err(self.error(XmlErrorKind::Malformed));
            }
            let value = decode_references(raw, value_start, true)?;
            self.position += end + 1;
            raw_attributes.push((name, value, attribute_start..self.position));
            if raw_attributes.len() > self.limits.max_xml_attributes {
                return Err(XmlError {
                    offset: attribute_start,
                    kind: XmlErrorKind::TooManyAttributes,
                });
            }
        };
        let tag = start..self.position;
        let namespace_error = XmlError {
            offset: start,
            kind: XmlErrorKind::NamespaceError,
        };

        let mut seen = BTreeSet::new();
        for (name, _, span) in &raw_attributes {
            if !seen.insert(name.as_str()) {
                return Err(XmlError {
                    offset: span.start,
                    kind: XmlErrorKind::DuplicateAttribute,
                });
            }
        }
        // Declarations first: they apply to the element's own name and attributes.
        for (name, value, _) in &raw_attributes {
            let prefix = if name == "xmlns" {
                None
            } else if let Some(prefix) = name.strip_prefix("xmlns:") {
                check_declaration(prefix, value).map_err(|()| namespace_error)?;
                Some(prefix.to_owned())
            } else {
                continue;
            };
            if prefix.is_none() && (value == XML_NAMESPACE || value == XMLNS_NAMESPACE) {
                return Err(namespace_error);
            }
            bindings.push(Binding {
                prefix,
                namespace: Arc::from(value.as_str()),
            });
        }
        let (element_prefix, local_name) = split_name(&qualified_name).ok_or(namespace_error)?;
        let namespace = self
            .resolve(bindings, element_prefix, true)
            .ok_or(namespace_error)?;
        let mut attributes = Vec::new();
        let mut expanded = BTreeSet::new();
        for (name, value, span) in raw_attributes {
            if name == "xmlns" || name.starts_with("xmlns:") {
                continue;
            }
            let (prefix, local) = split_name(&name).ok_or(namespace_error)?;
            let attribute_namespace = self
                .resolve(bindings, prefix, false)
                .ok_or(namespace_error)?;
            if !expanded.insert((attribute_namespace.clone(), local.to_owned())) {
                return Err(XmlError {
                    offset: span.start,
                    kind: XmlErrorKind::DuplicateAttribute,
                });
            }
            attributes.push(Attribute {
                local_name: local.to_owned(),
                namespace: attribute_namespace,
                value,
            });
        }
        let local_name = local_name.to_owned();
        Ok((
            Element {
                span: tag.clone(),
                start_tag: tag,
                end_tag: None,
                qualified_name,
                local_name,
                namespace,
                attributes,
                children: Vec::new(),
            },
            empty,
        ))
    }

    /// The namespace of `prefix` in `bindings` (the innermost declaration wins), shared rather than copied. Element names without a prefix take the default namespace; attribute names without a prefix have none. `None` (the outer one) if the prefix is not declared.
    fn resolve(
        &self,
        bindings: &[Binding],
        prefix: Option<&str>,
        element: bool,
    ) -> Option<Option<Arc<str>>> {
        match prefix {
            Some("xml") => Some(Some(Arc::clone(&self.xml_namespace))),
            Some(prefix) => bindings
                .iter()
                .rev()
                .find(|binding| binding.prefix.as_deref() == Some(prefix))
                .map(|binding| Some(Arc::clone(&binding.namespace))),
            None if !element => Some(None),
            None => Some(
                bindings
                    .iter()
                    .rev()
                    .find(|binding| binding.prefix.is_none())
                    .map(|binding| Arc::clone(&binding.namespace))
                    .filter(|namespace| !namespace.is_empty()),
            ),
        }
    }
}

/// Checks a declaration of `prefix` as `namespace` against Namespaces in XML 1.0 §3: `xmlns` cannot be declared, `xml` only as its own namespace, no other prefix as either reserved namespace, and a prefix cannot be undeclared.
fn check_declaration(prefix: &str, namespace: &str) -> Result<(), ()> {
    let reserved = namespace == XML_NAMESPACE || namespace == XMLNS_NAMESPACE;
    let acceptable = match prefix {
        "xmlns" => false,
        "xml" => namespace == XML_NAMESPACE,
        _ => !prefix.is_empty() && !prefix.contains(':') && !namespace.is_empty() && !reserved,
    };
    if acceptable { Ok(()) } else { Err(()) }
}

/// Splits a qualified name into its prefix and local name; `None` if it is not a valid qualified name (at most one colon, with names on both sides).
fn split_name(name: &str) -> Option<(Option<&str>, &str)> {
    match name.split_once(':') {
        None => Some((None, name)),
        Some((prefix, local))
            if !prefix.is_empty()
                && !local.is_empty()
                && !local.contains(':')
                && local.chars().next().is_some_and(is_name_start) =>
        {
            Some((Some(prefix), local))
        }
        Some(_) => None,
    }
}

/// Replaces the predefined entity references and character references in `raw`, which starts at `offset` in the text, and normalizes line ends (§2.11); in an attribute value, white space characters also become spaces (§3.3.3).
fn decode_references(raw: &str, offset: usize, attribute: bool) -> Result<String, XmlError> {
    let mut value = String::with_capacity(raw.len());
    let mut characters = raw.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        match character {
            '&' => {
                let rest = raw.get(index + 1..).unwrap_or_default();
                let end = rest.find(';').ok_or(XmlError {
                    offset: offset + index,
                    kind: XmlErrorKind::Malformed,
                })?;
                let reference = rest.get(..end).unwrap_or_default();
                let replacement = resolve_reference(reference).map_err(|kind| XmlError {
                    offset: offset + index,
                    kind,
                })?;
                value.push(replacement);
                // Skip the reference and its semicolon.
                while characters
                    .next_if(|&(next, _)| next <= index + 1 + end)
                    .is_some()
                {}
            }
            '\r' => {
                characters.next_if(|&(_, next)| next == '\n');
                value.push(if attribute { ' ' } else { '\n' });
            }
            '\n' | '\t' if attribute => value.push(' '),
            other => value.push(other),
        }
    }
    Ok(value)
}

/// The character that the reference `&reference;` stands for: one of the five predefined entities, or a decimal or hexadecimal character reference to a character XML allows.
fn resolve_reference(reference: &str) -> Result<char, XmlErrorKind> {
    match reference {
        "lt" => return Ok('<'),
        "gt" => return Ok('>'),
        "amp" => return Ok('&'),
        "apos" => return Ok('\''),
        "quot" => return Ok('"'),
        _ => {}
    }
    let Some(number) = reference.strip_prefix('#') else {
        return Err(XmlErrorKind::UndefinedEntity);
    };
    let (digits, radix) = match number.strip_prefix('x') {
        Some(hexadecimal) => (hexadecimal, 16),
        None => (number, 10),
    };
    // At most 8 digits: enough for every character with any leading zeros a writer may use, and no huge numbers to overflow.
    if digits.is_empty() || digits.len() > 8 || !digits.chars().all(|digit| digit.is_digit(radix)) {
        return Err(XmlErrorKind::InvalidCharacterReference);
    }
    u32::from_str_radix(digits, radix)
        .ok()
        .and_then(char::from_u32)
        .filter(|&character| is_xml_character(character))
        .ok_or(XmlErrorKind::InvalidCharacterReference)
}

/// Normalizes line ends: CR LF and a lone CR become LF (§2.11).
fn normalize_line_ends(raw: &str) -> String {
    raw.replace("\r\n", "\n").replace('\r', "\n")
}

/// XML white space (§2.3, `S`).
pub(crate) fn is_space(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\r' | '\n')
}

/// `NameStartChar` of XML 1.0 (fifth edition) §2.3.
fn is_name_start(character: char) -> bool {
    matches!(character,
        ':' | 'A'..='Z' | '_' | 'a'..='z'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

/// `NameChar` of XML 1.0 (fifth edition) §2.3.
fn is_name_character(character: char) -> bool {
    is_name_start(character)
        || matches!(character,
            '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// Writes `value` as the content of an attribute in double quotes: `&`, `<`, `>` and `"` as entity references, and tab, line feed and carriage return as character references, so that attribute value normalization gives them back.
pub(crate) fn escape_attribute(value: &str, out: &mut String) {
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            other => out.push(other),
        }
    }
}

/// Writes `value` as character data: `&`, `<` and `>` as entity references, and carriage return as a character reference, so that line end normalization gives it back.
pub(crate) fn escape_text(value: &str, out: &mut String) {
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            other => out.push(other),
        }
    }
}

/// Encodes `text` in `encoding`, with the byte order mark the original had.
pub(crate) fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Utf8 { bom: false } => text.as_bytes().to_vec(),
        Encoding::Utf8 { bom: true } => {
            let mut bytes = Vec::with_capacity(text.len() + 3);
            bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            bytes.extend_from_slice(text.as_bytes());
            bytes
        }
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let little = encoding == Encoding::Utf16Le;
            let mut bytes = Vec::with_capacity(2 + 2 * text.len());
            bytes.extend_from_slice(if little { &[0xFF, 0xFE] } else { &[0xFE, 0xFF] });
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            bytes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(text: &str) -> Document {
        parse(text.as_bytes(), &Limits::default()).unwrap()
    }

    fn error_of(bytes: &[u8]) -> XmlErrorKind {
        parse(bytes, &Limits::default()).unwrap_err().kind
    }

    #[test]
    fn parses_a_relationships_part() {
        let text = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"t\" Target=\"word/document.xml\"/></Relationships>";
        let document = parse_ok(text);
        let root = &document.root;
        assert!(root.is(
            "http://schemas.openxmlformats.org/package/2006/relationships",
            "Relationships"
        ));
        assert_eq!(root.attributes.len(), 0);
        let [Node::Element(child)] = root.children.as_slice() else {
            panic!("{:?}", root.children);
        };
        assert_eq!(child.local_name, "Relationship");
        assert_eq!(child.attribute("Target"), Some("word/document.xml"));
        assert_eq!(
            &document.text[child.span.clone()],
            "<Relationship Id=\"rId1\" Type=\"t\" Target=\"word/document.xml\"/>"
        );
        assert_eq!(
            &document.text[root.end_tag.clone().unwrap()],
            "</Relationships>"
        );
        assert_eq!(document.encoding, Encoding::Utf8 { bom: false });
    }

    #[test]
    fn resolves_namespaces() {
        let document = parse_ok(
            "<p:a xmlns:p='urn:p' xmlns='urn:d'><b p:x='1' y='2'/><p:c xmlns:p='urn:q'/><d xmlns=''/></p:a>",
        );
        let root = &document.root;
        assert!(root.is("urn:p", "a"));
        let children: Vec<&Element> = root
            .children
            .iter()
            .filter_map(|node| match node {
                Node::Element(element) => Some(element),
                _ => None,
            })
            .collect();
        assert!(children[0].is("urn:d", "b"));
        assert_eq!(
            children[0].attributes[0].namespace.as_deref(),
            Some("urn:p")
        );
        assert_eq!(children[0].attributes[1].namespace, None);
        assert!(children[1].is("urn:q", "c"));
        assert_eq!(children[2].namespace, None);
    }

    #[test]
    fn replaces_references_and_normalizes() {
        let document = parse_ok(
            "<a v='&lt;&amp;&#65;&#x42;\t\r\nz'>x&gt;&quot;&apos;\r\ny\rz<![CDATA[<&]]></a>",
        );
        assert_eq!(document.root.attribute("v"), Some("<&AB  z"));
        let texts: Vec<&str> = document
            .root
            .children
            .iter()
            .map(|node| match node {
                Node::Text { value, .. } | Node::CData { value, .. } => value.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(texts, ["x>\"'\ny\nz", "<&"]);
    }

    #[test]
    fn decodes_utf16_and_byte_order_marks() {
        let text = "<?xml version='1.0' encoding='UTF-16'?><a>é</a>";
        for encoding in [Encoding::Utf16Le, Encoding::Utf16Be] {
            let document = parse(&encode(text, encoding), &Limits::default()).unwrap();
            assert_eq!(document.encoding, encoding);
            assert_eq!(document.text, text);
        }
        let with_bom = encode("<a/>", Encoding::Utf8 { bom: true });
        assert_eq!(
            parse(&with_bom, &Limits::default()).unwrap().encoding,
            Encoding::Utf8 { bom: true }
        );
        assert_eq!(
            error_of(&encode(
                "<?xml version='1.0' encoding='UTF-8'?><a/>",
                Encoding::Utf16Le
            )),
            XmlErrorKind::UnsupportedEncoding
        );
        assert_eq!(
            error_of(b"<?xml version='1.0' encoding='UTF-16'?><a/>"),
            XmlErrorKind::UnsupportedEncoding
        );
        assert_eq!(
            error_of(b"<?xml version='1.0' encoding='ISO-8859-1'?><a/>"),
            XmlErrorKind::UnsupportedEncoding
        );
        assert_eq!(error_of(&[0xFF, 0xFE, 0x3C]), XmlErrorKind::InvalidEncoding);
        assert_eq!(
            error_of(&[0xFF, 0xFE, 0x00, 0xD8, 0x3C, 0x00]),
            XmlErrorKind::InvalidEncoding
        );
        assert_eq!(error_of(b"<a>\xC3</a>"), XmlErrorKind::InvalidEncoding);
    }

    #[test]
    fn refuses_document_type_declarations_and_entities() {
        let billion_laughs = b"<?xml version=\"1.0\"?><!DOCTYPE lolz [<!ENTITY lol \"lol\"><!ENTITY lol2 \"&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;\">]><lolz>&lol2;</lolz>";
        assert_eq!(error_of(billion_laughs), XmlErrorKind::DoctypeNotAllowed);
        let external = b"<!DOCTYPE r [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><r>&x;</r>";
        assert_eq!(error_of(external), XmlErrorKind::DoctypeNotAllowed);
        assert_eq!(error_of(b"<r>&x;</r>"), XmlErrorKind::UndefinedEntity);
        assert_eq!(error_of(b"<r a='&x;'/>"), XmlErrorKind::UndefinedEntity);
        assert_eq!(error_of(b"<r><!ENTITY x 'y'></r>"), XmlErrorKind::Malformed);
        assert_eq!(
            error_of(b"<r>&#0;</r>"),
            XmlErrorKind::InvalidCharacterReference
        );
        assert_eq!(
            error_of(b"<r>&#xD800;</r>"),
            XmlErrorKind::InvalidCharacterReference
        );
        assert_eq!(
            error_of(b"<r>&#x110000;</r>"),
            XmlErrorKind::InvalidCharacterReference
        );
        assert_eq!(
            error_of(b"<r>&#0000000000065;</r>"),
            XmlErrorKind::InvalidCharacterReference
        );
        assert_eq!(error_of(b"<r>& b</r>"), XmlErrorKind::Malformed);
        assert_eq!(error_of(b"<r>&x y;</r>"), XmlErrorKind::UndefinedEntity);
        assert_eq!(error_of(b"<a xmlns:='u'/>"), XmlErrorKind::NamespaceError);
        assert_eq!(error_of(b"<r>&amp</r>"), XmlErrorKind::Malformed);
    }

    #[test]
    fn refuses_malformed_documents() {
        let cases: [(&[u8], XmlErrorKind); 24] = [
            (b"", XmlErrorKind::ContentOutsideRoot),
            (b"text", XmlErrorKind::ContentOutsideRoot),
            (b"<a>", XmlErrorKind::UnexpectedEnd),
            (b"<a></b>", XmlErrorKind::MismatchedEndTag),
            (b"<a/><b/>", XmlErrorKind::ContentOutsideRoot),
            (b"<a/>text", XmlErrorKind::ContentOutsideRoot),
            (b"<a x='1' x='2'/>", XmlErrorKind::DuplicateAttribute),
            (
                b"<a xmlns:p='u' xmlns:q='u' p:x='1' q:x='2'/>",
                XmlErrorKind::DuplicateAttribute,
            ),
            (b"<p:a/>", XmlErrorKind::NamespaceError),
            (b"<a p:x='1'/>", XmlErrorKind::NamespaceError),
            (b"<a xmlns:p=''/>", XmlErrorKind::NamespaceError),
            (b"<a xmlns:xmlns='u'/>", XmlErrorKind::NamespaceError),
            (b"<a xmlns:xml='u'/>", XmlErrorKind::NamespaceError),
            (b"<a:b:c xmlns:a='u'/>", XmlErrorKind::NamespaceError),
            (b"<a x=1/>", XmlErrorKind::Malformed),
            (b"<a x='<'/>", XmlErrorKind::Malformed),
            (b"<a x='1'y='2'/>", XmlErrorKind::Malformed),
            (b"<a>]]></a>", XmlErrorKind::Malformed),
            (b"<a><!-- a -- b --></a>", XmlErrorKind::Malformed),
            (b"<a><?xml x?></a>", XmlErrorKind::Malformed),
            (
                b"<?xml version='2.0'?><a/>",
                XmlErrorKind::MalformedDeclaration,
            ),
            (
                b"<?xml encoding='UTF-8'?><a/>",
                XmlErrorKind::MalformedDeclaration,
            ),
            (
                b"<?xml version='1.0' standalone='maybe'?><a/>",
                XmlErrorKind::MalformedDeclaration,
            ),
            (b"<a>\x01</a>", XmlErrorKind::InvalidCharacter),
        ];
        for (bytes, kind) in cases {
            assert_eq!(error_of(bytes), kind, "{}", String::from_utf8_lossy(bytes));
        }
        // The declaration is only allowed at the very start.
        assert_eq!(
            error_of(b" <?xml version='1.0'?><a/>"),
            XmlErrorKind::Malformed
        );
    }

    #[test]
    fn keeps_comments_and_processing_instructions_around_the_root() {
        let document = parse_ok(
            "<?xml version='1.0'?>\n<!-- c --><?pi data?>\n<a><!--x--><?p?></a>\n<!-- end -->\n",
        );
        assert_eq!(document.root.children.len(), 2);
        assert_eq!(
            &document.text[document.root.span.clone()],
            "<a><!--x--><?p?></a>"
        );
    }

    #[test]
    fn enforces_the_limits() {
        let limits = Limits {
            max_xml_depth: 3,
            max_xml_attributes: 2,
            max_xml_name_length: 4,
            ..Limits::default()
        };
        assert!(parse(b"<a><b><c/></b></a>", &limits).is_ok());
        assert_eq!(
            parse(b"<a><b><c><d/></c></b></a>", &limits)
                .unwrap_err()
                .kind,
            XmlErrorKind::TooDeep
        );
        assert!(parse(b"<a x='1' y='2'/>", &limits).is_ok());
        assert_eq!(
            parse(b"<a x='1' y='2' z='3'/>", &limits).unwrap_err().kind,
            XmlErrorKind::TooManyAttributes
        );
        assert_eq!(
            parse(b"<abcde/>", &limits).unwrap_err().kind,
            XmlErrorKind::NameTooLong
        );
        // Deep nesting is refused without recursion, so even a deep limit cannot overflow the stack.
        let deep = "<a>".repeat(100_000);
        assert_eq!(
            parse(deep.as_bytes(), &Limits::default()).unwrap_err().kind,
            XmlErrorKind::TooDeep
        );
    }

    #[test]
    fn shares_namespace_names_instead_of_copying_them() {
        // Every element and attribute in a namespace refers to the one copy of its name that the declaration made, so a long name used by many elements costs its length once (the review of pull request 17 found an 863-byte package that needed 1 GiB when each element had a copy).
        let document = parse_ok(
            "<p:a xmlns:p='urn:a-long-namespace-name' xmlns='urn:d'><p:b p:x='1'/><p:c/><d xml:lang='en'/><e xml:space='preserve'/></p:a>",
        );
        let root = &document.root;
        let children: Vec<&Element> = root
            .children
            .iter()
            .filter_map(|node| match node {
                Node::Element(element) => Some(element),
                _ => None,
            })
            .collect();
        let namespace = |element: &Element| element.namespace.clone().unwrap();
        let shared = namespace(root);
        assert!(Arc::ptr_eq(&shared, &namespace(children[0])));
        assert!(Arc::ptr_eq(&shared, &namespace(children[1])));
        assert!(Arc::ptr_eq(
            &shared,
            children[0].attributes[0].namespace.as_ref().unwrap()
        ));
        // The default namespace, and the namespace of the `xml:` prefix, are shared the same way.
        assert!(Arc::ptr_eq(
            &namespace(children[2]),
            &namespace(children[3])
        ));
        assert!(Arc::ptr_eq(
            children[2].attributes[0].namespace.as_ref().unwrap(),
            children[3].attributes[0].namespace.as_ref().unwrap()
        ));
    }

    #[test]
    fn escapes_values_so_that_they_read_back_the_same() {
        let value = "a<b>&\"'\t\n\r c";
        let mut attribute = String::new();
        escape_attribute(value, &mut attribute);
        let mut text = String::new();
        escape_text(value, &mut text);
        let xml = format!("<r a=\"{attribute}\">{text}</r>");
        let document = parse_ok(&xml);
        assert_eq!(document.root.attribute("a"), Some(value));
        let [Node::Text { value: read, .. }] = document.root.children.as_slice() else {
            panic!();
        };
        assert_eq!(read, value);
    }
}
