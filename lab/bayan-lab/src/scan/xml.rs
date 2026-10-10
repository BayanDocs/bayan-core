//! A minimal, strict, namespace-aware XML scanner for the parts of OPC packages.
//!
//! **Marked for replacement** by `bayan-xml` (CORE-006), the engine's XML layer, once it exists. Until then the Fidelity Lab needs its own reader, and this one does only what feature tagging needs: it reports element starts and ends with their namespace-resolved names and attributes, and text, to a [`Handler`].
//!
//! Every part is hostile until proven otherwise:
//!
//! - document type declarations are refused, so no entity can be declared and none can expand: only the five predefined entities and character references are read (no "billion laughs", no external entities);
//! - the input must be well-formed: one root element, matching end tags, quoted attribute values, no duplicate attributes, bound prefixes, and only characters XML allows;
//! - [`XmlLimits`] caps nesting depth, attributes per element, name length and the namespace declarations in scope, and the scanner keeps its own stack, so no input can exhaust the call stack;
//! - it reads UTF-8 (with or without a byte order mark) and UTF-16 with a byte order mark, the encodings OOXML allows.
//!
//! Time and memory grow linearly with the size of the part, which the package reader limits.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

/// The namespace that the prefix `xml` is always bound to.
pub const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// Limits for scanning one part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XmlLimits {
    /// The deepest nesting of elements.
    pub max_depth: usize,
    /// The most attributes (namespace declarations included) on one element.
    pub max_attributes: usize,
    /// The longest element or attribute name, in bytes.
    pub max_name_len: usize,
    /// The most namespace declarations in scope at once.
    pub max_namespaces: usize,
}

/// Why a part is not XML this scanner accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmlError {
    /// Not UTF-8, or UTF-16 without a byte order mark or with an unpaired surrogate.
    Encoding,
    /// A document type declaration, which OOXML never needs and which could declare entities.
    Doctype,
    /// Not well-formed; the text says how.
    Malformed(&'static str),
    /// An entity other than the five predefined ones, or a character reference to a character XML does not allow.
    Entity,
    /// A character XML does not allow, such as most control characters.
    InvalidCharacter,
    /// A prefix without a namespace declaration in scope.
    UnboundPrefix,
    /// An end tag that does not match the element it closes.
    MismatchedEnd,
    /// Elements still open at the end of the part.
    Unclosed,
    /// Nesting deeper than [`XmlLimits::max_depth`].
    TooDeep,
    /// More attributes on one element than [`XmlLimits::max_attributes`].
    TooManyAttributes,
    /// A name longer than [`XmlLimits::max_name_len`].
    NameTooLong,
    /// More namespace declarations in scope than [`XmlLimits::max_namespaces`].
    TooManyNamespaces,
}

impl fmt::Display for XmlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encoding => formatter.write_str("not UTF-8, or UTF-16 without a byte order mark"),
            Self::Doctype => {
                formatter.write_str("a document type declaration (refused: OOXML needs none)")
            }
            Self::Malformed(what) => write!(formatter, "not well-formed XML: {what}"),
            Self::Entity => {
                formatter.write_str("an undeclared entity or an invalid character reference")
            }
            Self::InvalidCharacter => formatter.write_str("a character XML does not allow"),
            Self::UnboundPrefix => formatter.write_str("a namespace prefix without a declaration"),
            Self::MismatchedEnd => {
                formatter.write_str("an end tag that does not match its start tag")
            }
            Self::Unclosed => formatter.write_str("elements still open at the end of the part"),
            Self::TooDeep => formatter.write_str("elements nested deeper than the limit"),
            Self::TooManyAttributes => {
                formatter.write_str("more attributes on one element than the limit")
            }
            Self::NameTooLong => formatter.write_str("a name longer than the limit"),
            Self::TooManyNamespaces => {
                formatter.write_str("more namespace declarations in scope than the limit")
            }
        }
    }
}

impl std::error::Error for XmlError {}

/// A namespace-resolved name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Name<'a> {
    /// The namespace URI, or `""` for no namespace.
    pub namespace: &'a str,
    /// The local name.
    pub local: &'a str,
}

/// An attribute value as written, whose entities have been checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Value<'s>(&'s str);

impl<'s> Value<'s> {
    /// The value with its entities and character references replaced.
    #[must_use]
    pub fn decoded(&self) -> Cow<'s, str> {
        decode(self.0)
    }
}

/// Text between tags: raw, with checked entities, or the content of a CDATA section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Text<'s> {
    /// Character data as written.
    Raw(&'s str),
    /// The content of a CDATA section, taken literally.
    CData(&'s str),
}

impl<'s> Text<'s> {
    /// The text with entities and character references replaced.
    #[must_use]
    pub fn decoded(&self) -> Cow<'s, str> {
        match *self {
            Self::Raw(raw) => decode(raw),
            Self::CData(literal) => Cow::Borrowed(literal),
        }
    }
}

/// A namespace declaration in scope.
#[derive(Debug, Clone)]
struct Binding<'s> {
    /// The prefix, or `""` for the default namespace.
    prefix: &'s str,
    /// The namespace URI, or `""` where `xmlns=""` undeclares the default namespace.
    uri: Cow<'s, str>,
}

/// An attribute whose prefix is resolved.
#[derive(Debug, Clone, Copy)]
struct Attribute<'s> {
    /// Index into the bindings, or `None` for no namespace (unprefixed attributes).
    binding: Option<usize>,
    /// Whether the prefix is `xml`.
    xml: bool,
    local: &'s str,
    value: &'s str,
}

/// The namespace declarations in scope, with an index by prefix, so that resolving a prefix takes the same short time however many declarations are in scope.
#[derive(Debug, Default)]
struct Bindings<'s> {
    /// The declarations, outermost first.
    all: Vec<Binding<'s>>,
    /// For each declared prefix, the indices of its declarations in `all`, innermost last.
    by_prefix: BTreeMap<&'s str, Vec<usize>>,
}

impl<'s> Bindings<'s> {
    fn len(&self) -> usize {
        self.all.len()
    }

    fn get(&self, index: usize) -> Option<&Binding<'s>> {
        self.all.get(index)
    }

    fn push(&mut self, binding: Binding<'s>) {
        self.by_prefix
            .entry(binding.prefix)
            .or_default()
            .push(self.all.len());
        self.all.push(binding);
    }

    /// Removes the declarations after the first `len`, as their element ends.
    fn truncate(&mut self, len: usize) {
        while self.all.len() > len {
            let Some(binding) = self.all.pop() else {
                break;
            };
            if let Some(indices) = self.by_prefix.get_mut(binding.prefix) {
                indices.pop();
                if indices.is_empty() {
                    self.by_prefix.remove(binding.prefix);
                }
            }
        }
    }

    /// The index of the innermost declaration of `prefix`, if it binds a namespace (`xmlns=""` binds none).
    fn innermost(&self, prefix: &str) -> Option<usize> {
        let index = *self.by_prefix.get(prefix)?.last()?;
        self.all
            .get(index)
            .is_some_and(|binding| !binding.uri.is_empty())
            .then_some(index)
    }
}

/// An element's start tag, as the [`Handler`] sees it.
pub struct Start<'b, 's> {
    namespace: &'b str,
    local: &'s str,
    attributes: &'b [Attribute<'s>],
    bindings: &'b Bindings<'s>,
}

impl<'b, 's> Start<'b, 's> {
    /// The element's name.
    #[must_use]
    pub fn name(&self) -> Name<'b>
    where
        's: 'b,
    {
        Name {
            namespace: self.namespace,
            local: self.local,
        }
    }

    /// The element's attributes, without namespace declarations, with their resolved names.
    pub fn attributes(&self) -> impl Iterator<Item = (Name<'b>, Value<'s>)> + '_
    where
        's: 'b,
    {
        self.attributes.iter().map(|attribute| {
            (
                Name {
                    namespace: self.attribute_namespace(attribute),
                    local: attribute.local,
                },
                Value(attribute.value),
            )
        })
    }

    /// The value of the attribute with this namespace (`""` for none) and local name.
    #[must_use]
    pub fn attribute(&self, namespace: &str, local: &str) -> Option<Value<'s>> {
        self.attributes
            .iter()
            .find(|attribute| {
                attribute.local == local && self.attribute_namespace(attribute) == namespace
            })
            .map(|attribute| Value(attribute.value))
    }

    /// The namespace URI that `prefix` is bound to where this element starts, as Markup Compatibility's `Requires` and `Ignorable` attributes need.
    #[must_use]
    pub fn namespace_of_prefix(&self, prefix: &str) -> Option<&'b str> {
        if prefix == "xml" {
            return Some(XML_NAMESPACE);
        }
        self.bindings
            .innermost(prefix)
            .and_then(|index| self.bindings.get(index))
            .map(|binding| binding.uri.as_ref())
    }

    fn attribute_namespace(&self, attribute: &Attribute<'s>) -> &'b str {
        if attribute.xml {
            return XML_NAMESPACE;
        }
        attribute
            .binding
            .and_then(|index| self.bindings.get(index))
            .map_or("", |binding| binding.uri.as_ref())
    }
}

/// Receives what the scanner reads, in document order. For an empty element (`<a/>`), [`Handler::end`] follows [`Handler::start`] immediately.
pub trait Handler {
    /// An element starts.
    fn start(&mut self, start: &Start<'_, '_>);
    /// An element ends.
    fn end(&mut self, name: Name<'_>);
    /// Text inside an element. Long text may arrive in several pieces.
    fn text(&mut self, text: Text<'_>);
}

/// Scans `bytes` as one XML part, reporting to `handler`.
///
/// # Errors
///
/// An [`XmlError`] when the part is not well-formed XML, uses a feature this scanner refuses, or exceeds `limits`. The handler may already have received the events before the error.
pub fn scan(bytes: &[u8], limits: &XmlLimits, handler: &mut impl Handler) -> Result<(), XmlError> {
    let text = to_text(bytes)?;
    Scanner {
        text: &text,
        position: 0,
        limits,
        bindings: Bindings::default(),
        open: Vec::new(),
        raw_attributes: Vec::new(),
        attributes: Vec::new(),
        seen_root: false,
    }
    .run(handler)
}

/// The part as text: UTF-8 (a byte order mark is skipped), or UTF-16 with a byte order mark.
fn to_text(bytes: &[u8]) -> Result<Cow<'_, str>, XmlError> {
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => std::str::from_utf8(rest)
            .map(Cow::Borrowed)
            .map_err(|_| XmlError::Encoding),
        [0xFF, 0xFE, rest @ ..] => from_utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => from_utf16(rest, u16::from_be_bytes),
        _ => std::str::from_utf8(bytes)
            .map(Cow::Borrowed)
            .map_err(|_| XmlError::Encoding),
    }
}

fn from_utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> Result<Cow<'_, str>, XmlError> {
    let (pairs, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        return Err(XmlError::Encoding);
    }
    char::decode_utf16(pairs.iter().map(|&pair| unit(pair)))
        .collect::<Result<String, _>>()
        .map(Cow::Owned)
        .map_err(|_| XmlError::Encoding)
}

/// Whether XML allows `character` in text and attribute values (XML 1.0 §2.2). Surrogates cannot occur in Rust text.
const fn is_xml_char(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

/// Checks that `raw` (text or an attribute value as written) contains only allowed characters, and only predefined entities and valid character references.
fn check_characters(raw: &str) -> Result<(), XmlError> {
    let mut rest = raw;
    while let Some(at) = rest.find(|character: char| character == '&' || !is_xml_char(character)) {
        let (_, tail) = rest.split_at(at);
        if !tail.starts_with('&') {
            return Err(XmlError::InvalidCharacter);
        }
        let end = tail.find(';').ok_or(XmlError::Entity)?;
        let reference = &tail[1..end];
        reference_value(reference).ok_or(XmlError::Entity)?;
        rest = &tail[end + 1..];
    }
    Ok(())
}

/// The character an entity or character reference (between `&` and `;`) stands for, if it is predefined or valid.
fn reference_value(reference: &str) -> Option<char> {
    let character = match reference {
        "lt" => '<',
        "gt" => '>',
        "amp" => '&',
        "quot" => '"',
        "apos" => '\'',
        _ => {
            let code = if let Some(hex) = reference.strip_prefix("#x") {
                if hex.is_empty()
                    || hex.len() > 6
                    || !hex.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return None;
                }
                u32::from_str_radix(hex, 16).ok()?
            } else {
                let decimal = reference.strip_prefix('#')?;
                if decimal.is_empty()
                    || decimal.len() > 7
                    || !decimal.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return None;
                }
                decimal.parse().ok()?
            };
            char::from_u32(code)?
        }
    };
    is_xml_char(character).then_some(character)
}

/// `raw` with its (already checked) entities and character references replaced.
fn decode(raw: &str) -> Cow<'_, str> {
    if !raw.contains('&') {
        return Cow::Borrowed(raw);
    }
    let mut decoded = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        decoded.push_str(&rest[..at]);
        let tail = &rest[at..];
        match tail
            .find(';')
            .and_then(|end| Some((end, reference_value(&tail[1..end])?)))
        {
            Some((end, character)) => {
                decoded.push(character);
                rest = &tail[end + 1..];
            }
            // Unreachable for checked input; kept literally rather than panicking.
            None => {
                decoded.push('&');
                rest = &tail[1..];
            }
        }
    }
    decoded.push_str(rest);
    Cow::Owned(decoded)
}

/// What a name may not contain: white space and the characters that end a name in markup.
const fn ends_name(byte: u8) -> bool {
    matches!(
        byte,
        b' ' | b'\t' | b'\n' | b'\r' | b'/' | b'>' | b'=' | b'<' | b'"' | b'\'' | b'&'
    )
}

const fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

/// An attribute as written: its qualified name and its raw value.
#[derive(Debug, Clone, Copy)]
struct RawAttribute<'s> {
    qname: &'s str,
    value: &'s str,
}

/// An open element: its qualified name and how many namespace declarations were in scope before it.
struct Open<'s> {
    qname: &'s str,
    bindings_before: usize,
}

struct Scanner<'s, 'l> {
    text: &'s str,
    position: usize,
    limits: &'l XmlLimits,
    bindings: Bindings<'s>,
    open: Vec<Open<'s>>,
    raw_attributes: Vec<RawAttribute<'s>>,
    attributes: Vec<Attribute<'s>>,
    seen_root: bool,
}

impl<'s> Scanner<'s, '_> {
    fn bytes(&self) -> &'s [u8] {
        self.text.as_bytes()
    }

    fn rest(&self) -> &'s str {
        self.text.get(self.position..).unwrap_or("")
    }

    fn run(mut self, handler: &mut impl Handler) -> Result<(), XmlError> {
        while self.position < self.text.len() {
            let rest = self.rest();
            let text_len = rest.find('<').unwrap_or(rest.len());
            if text_len > 0 {
                let raw = &rest[..text_len];
                self.position += text_len;
                if self.open.is_empty() {
                    if !raw.bytes().all(is_space) {
                        return Err(XmlError::Malformed("text outside the root element"));
                    }
                } else {
                    check_characters(raw)?;
                    handler.text(Text::Raw(raw));
                }
                continue;
            }
            self.markup(handler)?;
        }
        if !self.open.is_empty() {
            return Err(XmlError::Unclosed);
        }
        if !self.seen_root {
            return Err(XmlError::Malformed("no root element"));
        }
        Ok(())
    }

    /// Reads one piece of markup starting at `<`.
    fn markup(&mut self, handler: &mut impl Handler) -> Result<(), XmlError> {
        let rest = self.rest();
        if let Some(body) = rest.strip_prefix("<!--") {
            let end = body
                .find("-->")
                .ok_or(XmlError::Malformed("unterminated comment"))?;
            self.position += 4 + end + 3;
        } else if let Some(body) = rest.strip_prefix("<![CDATA[") {
            if self.open.is_empty() {
                return Err(XmlError::Malformed(
                    "CDATA section outside the root element",
                ));
            }
            let end = body
                .find("]]>")
                .ok_or(XmlError::Malformed("unterminated CDATA section"))?;
            let literal = &body[..end];
            if !literal.chars().all(is_xml_char) {
                return Err(XmlError::InvalidCharacter);
            }
            handler.text(Text::CData(literal));
            self.position += 9 + end + 3;
        } else if rest.starts_with("<!DOCTYPE") {
            return Err(XmlError::Doctype);
        } else if rest.starts_with("<!") {
            return Err(XmlError::Malformed("unknown markup declaration"));
        } else if let Some(body) = rest.strip_prefix("<?") {
            // A processing instruction or the XML declaration; neither matters here.
            let end = body
                .find("?>")
                .ok_or(XmlError::Malformed("unterminated processing instruction"))?;
            self.position += 2 + end + 2;
        } else if rest.starts_with("</") {
            self.end_tag(handler)?;
        } else {
            self.start_tag(handler)?;
        }
        Ok(())
    }

    /// Reads a name at the current position.
    fn name(&mut self) -> Result<&'s str, XmlError> {
        let rest = self.rest();
        let len = rest.bytes().position(ends_name).unwrap_or(rest.len());
        if len == 0 {
            return Err(XmlError::Malformed("a name is missing"));
        }
        if len > self.limits.max_name_len {
            return Err(XmlError::NameTooLong);
        }
        let name = &rest[..len];
        let colons = name.bytes().filter(|&byte| byte == b':').count();
        if colons > 1 || name.starts_with(':') || name.ends_with(':') {
            return Err(XmlError::Malformed("a name has a misplaced colon"));
        }
        self.position += len;
        Ok(name)
    }

    fn skip_space(&mut self) -> bool {
        let rest = self.bytes().get(self.position..).unwrap_or(&[]);
        let len = rest
            .iter()
            .position(|&byte| !is_space(byte))
            .unwrap_or(rest.len());
        self.position += len;
        len > 0
    }

    fn peek(&self) -> Option<u8> {
        self.bytes().get(self.position).copied()
    }

    fn start_tag(&mut self, handler: &mut impl Handler) -> Result<(), XmlError> {
        if self.open.is_empty() && self.seen_root {
            return Err(XmlError::Malformed("content after the root element"));
        }
        if self.open.len() >= self.limits.max_depth {
            return Err(XmlError::TooDeep);
        }
        self.position += 1;
        let qname = self.name()?;
        self.raw_attributes.clear();
        let empty = loop {
            let spaced = self.skip_space();
            match self.peek() {
                Some(b'>') => {
                    self.position += 1;
                    break false;
                }
                Some(b'/') => {
                    if self.bytes().get(self.position + 1) != Some(&b'>') {
                        return Err(XmlError::Malformed("`/` not followed by `>`"));
                    }
                    self.position += 2;
                    break true;
                }
                Some(_) if spaced => self.attribute()?,
                Some(_) => {
                    return Err(XmlError::Malformed(
                        "attributes must be separated by white space",
                    ));
                }
                None => return Err(XmlError::Malformed("unterminated start tag")),
            }
        };
        let bindings_before = self.bindings.len();
        self.declare_namespaces()?;
        let (namespace_index, local) = self.resolve_element(qname)?;
        self.resolve_attributes()?;
        self.seen_root = true;
        let namespace = namespace_index
            .and_then(|index| self.bindings.get(index))
            .map_or("", |binding| binding.uri.as_ref());
        let start = Start {
            namespace,
            local,
            attributes: &self.attributes,
            bindings: &self.bindings,
        };
        handler.start(&start);
        if empty {
            let name = Name { namespace, local };
            handler.end(name);
            self.bindings.truncate(bindings_before);
        } else {
            self.open.push(Open {
                qname,
                bindings_before,
            });
        }
        Ok(())
    }

    fn attribute(&mut self) -> Result<(), XmlError> {
        if self.raw_attributes.len() >= self.limits.max_attributes {
            return Err(XmlError::TooManyAttributes);
        }
        let qname = self.name()?;
        self.skip_space();
        if self.peek() != Some(b'=') {
            return Err(XmlError::Malformed("an attribute has no `=`"));
        }
        self.position += 1;
        self.skip_space();
        let quote = match self.peek() {
            Some(quote @ (b'"' | b'\'')) => char::from(quote),
            _ => return Err(XmlError::Malformed("an attribute value is not quoted")),
        };
        self.position += 1;
        let rest = self.rest();
        let len = rest
            .find(quote)
            .ok_or(XmlError::Malformed("unterminated attribute value"))?;
        let value = &rest[..len];
        if value.contains('<') {
            return Err(XmlError::Malformed("`<` in an attribute value"));
        }
        check_characters(value)?;
        self.position += len + 1;
        self.raw_attributes.push(RawAttribute { qname, value });
        Ok(())
    }

    /// Adds the namespace declarations among the attributes to the bindings in scope.
    fn declare_namespaces(&mut self) -> Result<(), XmlError> {
        let mut qnames: Vec<&str> = self
            .raw_attributes
            .iter()
            .map(|attribute| attribute.qname)
            .collect();
        qnames.sort_unstable();
        if qnames.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(XmlError::Malformed("duplicate attribute"));
        }
        for attribute in &self.raw_attributes {
            let prefix = if attribute.qname == "xmlns" {
                ""
            } else if let Some(prefix) = attribute.qname.strip_prefix("xmlns:") {
                if prefix == "xml" || prefix == "xmlns" {
                    return Err(XmlError::Malformed(
                        "the prefixes xml and xmlns cannot be redeclared",
                    ));
                }
                if attribute.value.is_empty() {
                    return Err(XmlError::Malformed(
                        "a prefix cannot be undeclared in XML 1.0",
                    ));
                }
                prefix
            } else {
                continue;
            };
            if self.bindings.len() >= self.limits.max_namespaces {
                return Err(XmlError::TooManyNamespaces);
            }
            self.bindings.push(Binding {
                prefix,
                uri: decode(attribute.value),
            });
        }
        Ok(())
    }

    /// The index of the binding of the element's prefix (or of the default namespace), and its local name.
    fn resolve_element(&self, qname: &'s str) -> Result<(Option<usize>, &'s str), XmlError> {
        let (prefix, local) = qname.split_once(':').unwrap_or(("", qname));
        if prefix == "xmlns" || (prefix == "xml") {
            return Err(XmlError::Malformed(
                "an element cannot use the prefixes xml or xmlns",
            ));
        }
        let binding = self.binding_of(prefix);
        if !prefix.is_empty() && binding.is_none() {
            return Err(XmlError::UnboundPrefix);
        }
        Ok((binding, local))
    }

    /// The index of the innermost binding of `prefix`, if its namespace is not empty.
    fn binding_of(&self, prefix: &str) -> Option<usize> {
        self.bindings.innermost(prefix)
    }

    /// Resolves the attributes that are not namespace declarations.
    fn resolve_attributes(&mut self) -> Result<(), XmlError> {
        self.attributes.clear();
        for index in 0..self.raw_attributes.len() {
            let Some(raw) = self.raw_attributes.get(index).copied() else {
                break;
            };
            if raw.qname == "xmlns" || raw.qname.starts_with("xmlns:") {
                continue;
            }
            let resolved = match raw.qname.split_once(':') {
                None => Attribute {
                    binding: None,
                    xml: false,
                    local: raw.qname,
                    value: raw.value,
                },
                Some(("xml", local)) => Attribute {
                    binding: None,
                    xml: true,
                    local,
                    value: raw.value,
                },
                Some((prefix, local)) => Attribute {
                    binding: Some(self.binding_of(prefix).ok_or(XmlError::UnboundPrefix)?),
                    xml: false,
                    local,
                    value: raw.value,
                },
            };
            self.attributes.push(resolved);
        }
        Ok(())
    }

    fn end_tag(&mut self, handler: &mut impl Handler) -> Result<(), XmlError> {
        self.position += 2;
        let qname = self.name()?;
        self.skip_space();
        if self.peek() != Some(b'>') {
            return Err(XmlError::Malformed("unterminated end tag"));
        }
        self.position += 1;
        let open = self.open.pop().ok_or(XmlError::MismatchedEnd)?;
        if open.qname != qname {
            return Err(XmlError::MismatchedEnd);
        }
        let (namespace_index, local) = self.resolve_element(qname)?;
        let namespace = namespace_index
            .and_then(|index| self.bindings.get(index))
            .map_or("", |binding| binding.uri.as_ref());
        handler.end(Name { namespace, local });
        self.bindings.truncate(open.bindings_before);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: XmlLimits = XmlLimits {
        max_depth: 16,
        max_attributes: 8,
        max_name_len: 32,
        max_namespaces: 8,
    };

    /// Records events as text, such as `+{w}p a=1`, `-{w}p` and `"hello"`.
    #[derive(Default)]
    struct Recorder(Vec<String>);

    impl Handler for Recorder {
        fn start(&mut self, start: &Start<'_, '_>) {
            let name = start.name();
            let mut line = format!("+{{{}}}{}", name.namespace, name.local);
            for (attribute, value) in start.attributes() {
                line.push_str(&format!(
                    " {{{}}}{}={}",
                    attribute.namespace,
                    attribute.local,
                    value.decoded()
                ));
            }
            self.0.push(line);
        }
        fn end(&mut self, name: Name<'_>) {
            self.0
                .push(format!("-{{{}}}{}", name.namespace, name.local));
        }
        fn text(&mut self, text: Text<'_>) {
            self.0.push(format!("{:?}", text.decoded()));
        }
    }

    fn events(xml: &str) -> Result<Vec<String>, XmlError> {
        let mut recorder = Recorder::default();
        scan(xml.as_bytes(), &LIMITS, &mut recorder).map(|()| recorder.0)
    }

    #[test]
    fn reads_namespaces_attributes_and_text() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="urn:w" xmlns="urn:d"><w:p w:a="1&amp;2" b='&#x41;&#66;'>x &lt; y<![CDATA[<z>]]></w:p><e xml:space="preserve"/><!-- c --></w:document>"#;
        assert_eq!(
            events(xml).unwrap(),
            [
                "+{urn:w}document",
                "+{urn:w}p {urn:w}a=1&2 {}b=AB",
                "\"x < y\"",
                "\"<z>\"",
                "-{urn:w}p",
                "+{urn:d}e {http://www.w3.org/XML/1998/namespace}space=preserve",
                "-{urn:d}e",
                "-{urn:w}document",
            ]
        );
    }

    #[test]
    fn scopes_namespace_declarations_to_their_element() {
        let xml = r#"<a xmlns:p="urn:1"><p:b xmlns:p="urn:2"/><p:c/><d xmlns=""/></a>"#;
        assert_eq!(
            events(xml).unwrap(),
            [
                "+{}a",
                "+{urn:2}b",
                "-{urn:2}b",
                "+{urn:1}c",
                "-{urn:1}c",
                "+{}d",
                "-{}d",
                "-{}a"
            ]
        );
    }

    #[test]
    fn reads_utf16_with_a_byte_order_mark() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "<a>\u{0627}</a>".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let mut recorder = Recorder::default();
        scan(&bytes, &LIMITS, &mut recorder).unwrap();
        assert_eq!(recorder.0, ["+{}a", "\"\u{0627}\"", "-{}a"]);
        assert_eq!(
            scan(&bytes[..bytes.len() - 1], &LIMITS, &mut Recorder::default()),
            Err(XmlError::Encoding)
        );
    }

    #[test]
    fn refuses_document_type_declarations_and_unknown_entities() {
        assert_eq!(
            events(r#"<!DOCTYPE lolz [<!ENTITY lol "lol">]><a>&lol;</a>"#),
            Err(XmlError::Doctype)
        );
        assert_eq!(events("<a>&lol;</a>"), Err(XmlError::Entity));
        assert_eq!(events("<a b='&lol;'/>"), Err(XmlError::Entity));
        assert_eq!(events("<a>&#0;</a>"), Err(XmlError::Entity));
        assert_eq!(events("<a>&#x110000;</a>"), Err(XmlError::Entity));
        assert_eq!(events("<a>& b</a>"), Err(XmlError::Entity));
        assert_eq!(events("<a>\u{1}</a>"), Err(XmlError::InvalidCharacter));
    }

    #[test]
    fn refuses_malformed_markup() {
        for xml in [
            "",
            "text",
            "<a>",
            "<a></b>",
            "<a/><b/>",
            "<a b=1/>",
            "<a b='1'c='2'/>",
            "<a b='1' b='2'/>",
            "<a b='<'/>",
            "<a><!-- x</a>",
            "<a><![CDATA[x</a>",
            "<a:b/>",
            "<a xmlns:p=''/>",
            "<a p:b='1'/>",
            "<:a/>",
            "<a:b:c xmlns:a='u'/>",
            "<a/>x",
            "</a>",
        ] {
            assert!(events(xml).is_err(), "{xml:?} was accepted");
        }
    }

    #[test]
    fn enforces_its_limits() {
        let deep = "<a>".repeat(17) + &"</a>".repeat(17);
        assert_eq!(events(&deep), Err(XmlError::TooDeep));
        let ok = "<a>".repeat(16) + &"</a>".repeat(16);
        assert!(events(&ok).is_ok());
        assert_eq!(
            events("<a b='1' c='1' d='1' e='1' f='1' g='1' h='1' i='1' j='1'/>"),
            Err(XmlError::TooManyAttributes)
        );
        assert_eq!(
            events(&format!("<{}/>", "a".repeat(33))),
            Err(XmlError::NameTooLong)
        );
        let namespaces: String = (0..9)
            .map(|index| format!("<a xmlns:p{index}='u'>"))
            .collect();
        assert_eq!(events(&namespaces), Err(XmlError::TooManyNamespaces));
    }

    #[test]
    fn reports_the_namespace_of_a_prefix_in_scope() {
        struct Prefixes(Vec<Option<String>>);
        impl Handler for Prefixes {
            fn start(&mut self, start: &Start<'_, '_>) {
                self.0
                    .push(start.namespace_of_prefix("w14").map(str::to_owned));
            }
            fn end(&mut self, _: Name<'_>) {}
            fn text(&mut self, _: Text<'_>) {}
        }
        let mut prefixes = Prefixes(Vec::new());
        scan(b"<a xmlns:w14='urn:14'><b/></a>", &LIMITS, &mut prefixes).unwrap();
        assert_eq!(
            prefixes.0,
            [Some("urn:14".to_owned()), Some("urn:14".to_owned())]
        );
    }
}
