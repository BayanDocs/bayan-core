//! The errors of bayan-xml.
//!
//! Every error says what is wrong and where, as a byte offset into the decoded text ([`Source::text`](crate::Source::text)). None carries a name, a value or any other text from the document, because such text is document content, which must never reach logs, crash reports or error messages that leave the machine (`docs/AGENTS.md` §6). A caller that needs the offending text for a local diagnostic can look it up at the offset.

use core::fmt;

/// Why a document, or a fragment of one, was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    offset: usize,
    kind: ErrorKind,
}

impl Error {
    pub(crate) const fn new(offset: usize, kind: ErrorKind) -> Error {
        Error { offset, kind }
    }

    /// The byte offset in the decoded text ([`Source::text`](crate::Source::text)) where the problem was found: the start of the offending markup, reference or character. For problems that only show at the end of the input, such as an element that is never closed, it is the length of the text.
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// What is wrong.
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.kind, self.offset)
    }
}

impl core::error::Error for Error {}

/// What is wrong with a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The input is larger than [`Limits::max_size`](crate::Limits::max_size).
    TooLarge,
    /// The bytes are not valid UTF-8, or not valid UTF-16 after a UTF-16 byte order mark.
    InvalidEncoding,
    /// The XML declaration names an encoding other than UTF-8 or UTF-16, the only two that Office Open XML allows, or one that does not match the bytes.
    UnsupportedEncoding,
    /// The XML declaration is malformed, or names an XML version other than 1.0.
    MalformedDeclaration,
    /// An XML declaration (`<?xml …?>`) that is not at the very start of the document, or a processing instruction whose target is `xml` in any mix of cases, which XML reserves.
    MisplacedDeclaration,
    /// The document contains a character that XML does not allow (most control characters, U+FFFE and U+FFFF).
    InvalidCharacter,
    /// The document contains a document type declaration (`<!DOCTYPE`), which Office Open XML does not allow and which would allow entity-expansion attacks and external entities.
    DoctypeNotAllowed,
    /// The document refers to an entity other than the five that XML predefines (`&amp;`, `&lt;`, `&gt;`, `&apos;`, `&quot;`); without a document type declaration, no other entity can exist.
    UndefinedEntity,
    /// An `&` that does not start a well-formed reference (`&name;`, `&#digits;` or `&#xhex;`).
    MalformedReference,
    /// A character reference names a character that XML does not allow, or a number beyond Unicode.
    InvalidCharacterReference,
    /// The input ends in the middle of markup or before every element is closed.
    UnexpectedEnd,
    /// A start or end tag is malformed: a missing `=` or quote, no white space between attributes, or anything else the tag grammar does not allow.
    MalformedTag,
    /// A comment contains `--` or ends with `-`.
    MalformedComment,
    /// A processing instruction has no valid target, a target with a colon, or no white space between its target and its data.
    MalformedProcessingInstruction,
    /// Markup that is none of the kinds XML allows here, such as `<!` that starts neither a comment nor a CDATA section.
    MalformedMarkup,
    /// A name is not a valid XML name, or an element or attribute name is not a valid qualified name (Namespaces in XML).
    InvalidName,
    /// An attribute value contains `<`.
    LessThanInAttribute,
    /// Character data contains `]]>`, which XML reserves for the end of CDATA sections.
    CDataEndInText,
    /// An end tag does not match the start tag it closes, or closes an element that is not open.
    MismatchedEndTag,
    /// An element has two attributes with the same name, or with the same namespace and local name.
    DuplicateAttribute,
    /// There is content other than comments, processing instructions and white space outside the root element: text, a CDATA section or a second root element.
    ContentOutsideRoot,
    /// The document has no root element.
    NoRootElement,
    /// An element or attribute name uses a namespace prefix that is not declared.
    UndeclaredPrefix,
    /// A namespace declaration breaks the rules of Namespaces in XML: it binds the `xml` prefix to another namespace or another prefix to the XML namespace, declares the `xmlns` prefix or binds anything to its namespace, or binds a prefix to an empty name.
    InvalidNamespaceDeclaration,
    /// An element name uses the prefix `xmlns`, which is reserved for namespace declarations.
    ReservedPrefix,
    /// The elements are nested more deeply than [`Limits::max_depth`](crate::Limits::max_depth).
    TooDeep,
    /// An element has more attributes than [`Limits::max_attributes`](crate::Limits::max_attributes).
    TooManyAttributes,
    /// A name is longer than [`Limits::max_name_length`](crate::Limits::max_name_length) bytes.
    NameTooLong,
    /// A namespace name is longer than [`Limits::max_namespace_name_length`](crate::Limits::max_namespace_name_length) bytes.
    NamespaceNameTooLong,
    /// The document declares more distinct namespace names than [`Limits::max_namespaces`](crate::Limits::max_namespaces).
    TooManyNamespaces,
    /// A Markup Compatibility attribute lists more names than [`Limits::max_compatibility_tokens`](crate::Limits::max_compatibility_tokens).
    TooManyCompatibilityTokens,
    /// The document breaks a rule of Markup Compatibility and Extensibility (ECMA-376 Part 3).
    Compatibility(CompatibilityError),
    /// [`Reader::capture`](crate::Reader::capture) was called when the current event was not a start tag.
    NotAtStartTag,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            ErrorKind::TooLarge => "input too large",
            ErrorKind::InvalidEncoding => "invalid character encoding",
            ErrorKind::UnsupportedEncoding => "unsupported character encoding",
            ErrorKind::MalformedDeclaration => "malformed XML declaration",
            ErrorKind::MisplacedDeclaration => "misplaced XML declaration",
            ErrorKind::InvalidCharacter => "character not allowed in XML",
            ErrorKind::DoctypeNotAllowed => "document type declarations are not allowed",
            ErrorKind::UndefinedEntity => "undefined entity",
            ErrorKind::MalformedReference => "malformed reference",
            ErrorKind::InvalidCharacterReference => "invalid character reference",
            ErrorKind::UnexpectedEnd => "unexpected end of input",
            ErrorKind::MalformedTag => "malformed tag",
            ErrorKind::MalformedComment => "malformed comment",
            ErrorKind::MalformedProcessingInstruction => "malformed processing instruction",
            ErrorKind::MalformedMarkup => "malformed markup",
            ErrorKind::InvalidName => "invalid name",
            ErrorKind::LessThanInAttribute => "'<' in an attribute value",
            ErrorKind::CDataEndInText => "']]>' in character data",
            ErrorKind::MismatchedEndTag => "mismatched end tag",
            ErrorKind::DuplicateAttribute => "duplicate attribute",
            ErrorKind::ContentOutsideRoot => "content outside the root element",
            ErrorKind::NoRootElement => "no root element",
            ErrorKind::UndeclaredPrefix => "undeclared namespace prefix",
            ErrorKind::InvalidNamespaceDeclaration => "invalid namespace declaration",
            ErrorKind::ReservedPrefix => "reserved namespace prefix",
            ErrorKind::TooDeep => "elements nested too deeply",
            ErrorKind::TooManyAttributes => "too many attributes",
            ErrorKind::NameTooLong => "name too long",
            ErrorKind::NamespaceNameTooLong => "namespace name too long",
            ErrorKind::TooManyNamespaces => "too many namespaces",
            ErrorKind::TooManyCompatibilityTokens => {
                "too many names in a Markup Compatibility attribute"
            }
            ErrorKind::Compatibility(error) => return write!(f, "{error}"),
            ErrorKind::NotAtStartTag => "not at a start tag",
        };
        f.write_str(text)
    }
}

/// A rule of Markup Compatibility and Extensibility (ECMA-376 Part 3) that the document breaks.
///
/// Part 3 calls some of these situations "mismatches" between the document and what the reading application understands, and lets a processor continue after signalling them; bayan-xml continues only after the mismatches that leave the output well defined (an element or attribute in a namespace that is neither understood nor ignorable, which [`McReader`](crate::McReader) passes on and marks). Everything here stops reading, because the document cannot be interpreted as its producer intended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompatibilityError {
    /// `mc:MustUnderstand` names a namespace that the configuration does not understand (Part 3 §7.4, §9.4).
    MustUnderstand,
    /// A Markup Compatibility attribute's value is not a list of the names it must hold: prefixes for `mc:Ignorable`, `mc:MustUnderstand` and `Requires`, qualified names or `prefix:*` for `mc:ProcessContent`, `mc:PreserveElements` and `mc:PreserveAttributes`; or `Requires` is empty.
    InvalidList,
    /// A prefix listed in a Markup Compatibility attribute is not declared.
    UndeclaredPrefix,
    /// A Markup Compatibility attribute lists the Markup Compatibility namespace itself.
    CompatibilityNamespaceListed,
    /// `mc:ProcessContent`, `mc:PreserveElements` or `mc:PreserveAttributes` names a namespace that is not declared ignorable on the same element or an ancestor (Part 3 §7.3).
    NotIgnorable,
    /// An element in the Markup Compatibility namespace other than `AlternateContent`, `Choice` and `Fallback`.
    UnknownElement,
    /// An attribute in the Markup Compatibility namespace other than `Ignorable`, `ProcessContent`, `MustUnderstand`, `PreserveElements` and `PreserveAttributes`.
    UnknownAttribute,
    /// `mc:Choice` that is not a child of `mc:AlternateContent`, or that follows `mc:Fallback`.
    MisplacedChoice,
    /// `mc:Fallback` that is not a child of `mc:AlternateContent`, or a second `mc:Fallback`.
    MisplacedFallback,
    /// `mc:AlternateContent` without `mc:Choice` (Part 3 §7.5).
    MissingChoice,
    /// `mc:Choice` without a `Requires` attribute (Part 3 §7.6).
    MissingRequires,
    /// An attribute that a Markup Compatibility element may not have: an unqualified attribute other than `Requires` on `mc:Choice`, a qualified attribute in a namespace that is neither the Markup Compatibility namespace nor ignorable, or an attribute in the XML namespace (Part 3 §7.1, §7.5–7.7).
    UnexpectedAttribute,
    /// Content that `mc:AlternateContent` may not have: text other than white space, or a child element that is neither `mc:Choice` nor `mc:Fallback` nor ignored (Part 3 §7.5, §9.4).
    UnexpectedContent,
    /// An element whose content is processed in its place (`mc:ProcessContent`) has an `xml:base`, `xml:lang` or `xml:space` attribute (Part 3 §9.2).
    XmlAttributeOnUnwrappedElement,
}

impl fmt::Display for CompatibilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CompatibilityError::MustUnderstand => "a namespace that must be understood is not",
            CompatibilityError::InvalidList => "malformed list in a Markup Compatibility attribute",
            CompatibilityError::UndeclaredPrefix => {
                "undeclared prefix in a Markup Compatibility attribute"
            }
            CompatibilityError::CompatibilityNamespaceListed => {
                "a Markup Compatibility attribute lists its own namespace"
            }
            CompatibilityError::NotIgnorable => {
                "a Markup Compatibility attribute names a namespace that is not ignorable"
            }
            CompatibilityError::UnknownElement => "unknown Markup Compatibility element",
            CompatibilityError::UnknownAttribute => "unknown Markup Compatibility attribute",
            CompatibilityError::MisplacedChoice => "misplaced Choice element",
            CompatibilityError::MisplacedFallback => "misplaced Fallback element",
            CompatibilityError::MissingChoice => "AlternateContent without Choice",
            CompatibilityError::MissingRequires => "Choice without Requires",
            CompatibilityError::UnexpectedAttribute => {
                "attribute not allowed on a Markup Compatibility element"
            }
            CompatibilityError::UnexpectedContent => "content not allowed in AlternateContent",
            CompatibilityError::XmlAttributeOnUnwrappedElement => {
                "XML attribute on an element whose content is processed"
            }
        })
    }
}

/// Why the [`Writer`](crate::Writer) refused a request. The writer refuses anything that would make its output malformed, or mean something other than what was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteError {
    /// A name is not a valid XML name: an element or attribute local name or prefix that is not an NCName, or a processing instruction target that is not a name, contains a colon or is `xml` in any mix of cases.
    InvalidName,
    /// Text, an attribute value, a comment, a CDATA section or a processing instruction contains a character that XML cannot represent, not even as a character reference.
    InvalidCharacter,
    /// A declaration or name uses the `xml` or `xmlns` prefix or their namespaces in a way Namespaces in XML forbids, or binds a prefix to an empty namespace name.
    ReservedPrefix,
    /// One start tag would need the same prefix for two different namespaces.
    PrefixConflict,
    /// An attribute in a namespace was given without a prefix (attributes without a prefix are in no namespace), or an attribute in no namespace was given a prefix.
    InvalidAttributeName,
    /// A Markup Compatibility attribute, or the `Requires` attribute of `mc:Choice`, lists a prefix that is not declared where it is written.
    UndeclaredPrefix,
    /// One element would have two attributes with the same namespace and local name, or two declarations of the same prefix.
    DuplicateAttribute,
    /// A comment contains `--` or ends with `-`.
    InvalidComment,
    /// The data of a processing instruction contains `?>` or starts with white space.
    InvalidProcessingInstruction,
    /// `declare` or `attribute` was called when no start tag was open for them, or `end` with no element open.
    NoOpenElement,
    /// Text or a CDATA section outside the root element, white space inside it, or a second root element.
    ContentOutsideRoot,
    /// The XML declaration was not the first thing written.
    MisplacedDeclaration,
    /// `finish` was called before every element was closed, or before a root element was written.
    Unfinished,
    /// A fragment would need `xml:space` to keep its white space mode where it is written, and its root element may not have one: an element of the Markup Compatibility namespace, which may have no attribute in the XML namespace, or an element whose content Markup Compatibility processes in its place, which may not have `xml:space` (ECMA-376 Part 3, 5th edition, §7.1 and §9.2). See [`Writer::fragment`](crate::Writer::fragment).
    SpaceConflict,
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::InvalidName => f.write_str("invalid name"),
            WriteError::InvalidCharacter => f.write_str("character that XML cannot represent"),
            WriteError::ReservedPrefix => f.write_str("reserved prefix or namespace"),
            WriteError::PrefixConflict => f.write_str("prefix bound to two namespaces"),
            WriteError::InvalidAttributeName => {
                f.write_str("attribute prefix does not match its namespace")
            }
            WriteError::UndeclaredPrefix => {
                f.write_str("undeclared prefix in a Markup Compatibility attribute")
            }
            WriteError::DuplicateAttribute => f.write_str("duplicate attribute or declaration"),
            WriteError::InvalidComment => f.write_str("invalid comment"),
            WriteError::InvalidProcessingInstruction => {
                f.write_str("invalid processing instruction")
            }
            WriteError::NoOpenElement => f.write_str("no open element"),
            WriteError::ContentOutsideRoot => f.write_str("content outside the root element"),
            WriteError::MisplacedDeclaration => f.write_str("the XML declaration must come first"),
            WriteError::Unfinished => f.write_str("document not finished"),
            WriteError::SpaceConflict => {
                f.write_str("fragment needs a white space mode it may not set")
            }
        }
    }
}

impl core::error::Error for WriteError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_name_the_problem_and_the_offset() {
        let error = Error::new(42, ErrorKind::UndefinedEntity);
        assert_eq!(error.to_string(), "undefined entity at byte 42");
        let error = Error::new(
            7,
            ErrorKind::Compatibility(CompatibilityError::MustUnderstand),
        );
        assert_eq!(
            error.to_string(),
            "a namespace that must be understood is not at byte 7"
        );
        assert_eq!(
            WriteError::PrefixConflict.to_string(),
            "prefix bound to two namespaces"
        );
    }
}
