//! A canonical form of documents, for comparing what they mean rather than how they are written.

use std::collections::BTreeMap;

use crate::chars::{is_space, split_qname};
use crate::escape;
use crate::event::{Event, Space};
use crate::namespace::KnownNamespace;
use crate::reader::XML_NAMESPACE;
use crate::{Error, Limits, Reader, Source};

/// The canonical form of `source`, for comparisons: two documents have the same canonical form exactly when they have the same elements, attributes, text, comments and processing instructions, with the same expanded names (namespace name and local name).
///
/// It ignores how a document is written: its encoding and XML declaration, prefixes and namespace declarations, the order of attributes and their quotes, references, CDATA sections, empty-element tags, line ends (which reading normalizes), white space outside the root element, and `xml:space` attributes that do not change the white space mode in force. Prefixes in attribute values are resolved where Markup Compatibility (ECMA-376 Part 3) and XML Schema give them a meaning: the lists of `mc:Ignorable`, `mc:MustUnderstand`, `mc:ProcessContent`, `mc:PreserveElements` and `mc:PreserveAttributes`, the list of `Requires` on `mc:Choice`, and the qualified name of `xsi:type`. Other qualified names in attribute values or text are compared as written, as in Exclusive XML Canonicalization.
///
/// The form starts with the namespace names, numbered in order of first use, one per line (`{0} http://…`), followed by the content: every name in a namespace written `{number}local`, every element with a start tag and an end tag, the attributes of each start tag sorted by namespace name and then local name, and text and attribute values escaped as in Canonical XML. It is meant for comparisons, not for reading, and it is at most a few times larger than the document.
///
/// # Errors
///
/// The first problem [`Reader`] finds in the document.
pub fn canonicalize(source: &Source<'_>, limits: &Limits) -> Result<String, Error> {
    let mut reader = Reader::new(source, limits);
    let mut form = Form::default();
    while reader.advance()? {
        match reader.event() {
            Some(Event::Start(tag)) => {
                form.flush_text();
                let parent = form.spaces.last().copied().unwrap_or_default();
                form.spaces.push(tag.space());
                let name = tag.name();
                let choice = name.is(KnownNamespace::MarkupCompatibility, "Choice");
                let mut attributes: Vec<(&str, &str, &str)> = tag
                    .attributes()
                    .filter(|attribute| !attribute.is_namespace_declaration())
                    .map(|attribute| {
                        let name = attribute.name();
                        let uri = name.namespace().map_or("", |namespace| namespace.uri());
                        (uri, name.local(), attribute.value())
                    })
                    .filter(|&(uri, local, value)| {
                        // xml:space that leaves the white space mode as it was changes nothing.
                        let redundant = uri == XML_NAMESPACE
                            && local == "space"
                            && matches!(value, "default" | "preserve")
                            && tag.space() == parent;
                        !redundant
                    })
                    .collect();
                attributes.sort_unstable_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
                form.body.push('<');
                form.name(
                    name.namespace().map(|namespace| namespace.uri()),
                    name.local(),
                );
                for (uri, local, value) in attributes {
                    form.body.push(' ');
                    form.name((!uri.is_empty()).then_some(uri), local);
                    form.body.push_str("=\"");
                    match ValueKind::of(choice, uri, local) {
                        ValueKind::Plain => escape::attribute(value, &mut form.body),
                        kind => form.qualified_value(&reader, kind, value),
                    }
                    form.body.push('"');
                }
                form.body.push('>');
            }
            Some(Event::End(tag)) => {
                form.flush_text();
                form.spaces.pop();
                let name = tag.name();
                form.body.push_str("</");
                form.name(
                    name.namespace().map(|namespace| namespace.uri()),
                    name.local(),
                );
                form.body.push('>');
            }
            Some(Event::Text(text)) => form.text.push_str(text.value()),
            Some(Event::CData(cdata)) => form.text.push_str(cdata.value()),
            Some(Event::Comment(comment)) => {
                form.flush_text();
                form.body.push_str("<!--");
                form.body.push_str(comment.text());
                form.body.push_str("-->");
            }
            Some(Event::ProcessingInstruction(pi)) => {
                form.flush_text();
                form.body.push_str("<?");
                form.body.push_str(pi.target());
                if !pi.data().is_empty() {
                    form.body.push(' ');
                    form.body.push_str(pi.data());
                }
                form.body.push_str("?>");
            }
            None => {}
        }
    }
    form.flush_text();
    Ok(form.finish())
}

/// How an attribute value is compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ValueKind {
    /// As it is.
    Plain,
    /// A list of prefixes: `mc:Ignorable`, `mc:MustUnderstand` and `Requires` on `mc:Choice`.
    Prefixes,
    /// A list of `prefix:local` or `prefix:*`: `mc:ProcessContent`, `mc:PreserveElements` and `mc:PreserveAttributes`.
    QualifiedNames,
    /// One qualified name, whose prefix may be the default namespace's: `xsi:type`.
    QualifiedName,
}

impl ValueKind {
    fn of(choice: bool, uri: &str, local: &str) -> ValueKind {
        let compatibility = KnownNamespace::MarkupCompatibility.uri();
        match (uri, local) {
            (_, "Ignorable" | "MustUnderstand") if uri == compatibility => ValueKind::Prefixes,
            ("", "Requires") if choice => ValueKind::Prefixes,
            (_, "ProcessContent" | "PreserveElements" | "PreserveAttributes")
                if uri == compatibility =>
            {
                ValueKind::QualifiedNames
            }
            (_, "type") if uri == KnownNamespace::XmlSchemaInstance.uri() => {
                ValueKind::QualifiedName
            }
            _ => ValueKind::Plain,
        }
    }
}

/// The canonical form being written.
#[derive(Debug, Default)]
struct Form {
    /// The content.
    body: String,
    /// Text not yet written: adjacent text and CDATA sections are one text.
    text: String,
    /// The number of each namespace name, in order of first use.
    numbers: BTreeMap<String, usize>,
    names: Vec<String>,
    /// The white space mode inside each open element.
    spaces: Vec<Space>,
}

impl Form {
    fn number(&mut self, uri: &str) -> usize {
        if let Some(&number) = self.numbers.get(uri) {
            return number;
        }
        let number = self.names.len();
        self.numbers.insert(uri.to_owned(), number);
        self.names.push(uri.to_owned());
        number
    }

    /// Writes the namespace name `uri` as its number in braces.
    fn namespace(&mut self, uri: &str) {
        let number = self.number(uri);
        self.body.push('{');
        self.body.push_str(&number.to_string());
        self.body.push('}');
    }

    /// Writes an element or attribute name.
    fn name(&mut self, uri: Option<&str>, local: &str) {
        if let Some(uri) = uri {
            self.namespace(uri);
        }
        self.body.push_str(local);
    }

    /// Writes part of an attribute value as written, escaped, with braces escaped too, so that it can never look like a numbered namespace.
    fn literal(&mut self, text: &str) {
        let mut escaped = String::new();
        escape::attribute(text, &mut escaped);
        self.body
            .push_str(&escaped.replace('{', "&#x7B;").replace('}', "&#x7D;"));
    }

    /// Writes an attribute value that holds prefixes or qualified names, with every prefix that is bound replaced by its namespace and every list in single spaces.
    fn qualified_value(&mut self, reader: &Reader<'_>, kind: ValueKind, value: &str) {
        if kind == ValueKind::QualifiedName {
            let trimmed = value.trim_matches(|c: char| u8::try_from(c).is_ok_and(is_space));
            match split_qname(trimmed) {
                Some((prefix, local)) => match reader.namespace_of(prefix) {
                    Some(uri) => {
                        self.namespace(uri);
                        self.literal(local);
                    }
                    // A name in no namespace.
                    None if prefix.is_empty() => self.literal(local),
                    None => self.literal(value),
                },
                None => self.literal(value),
            }
            return;
        }
        for (index, token) in value.split_ascii_whitespace().enumerate() {
            if index > 0 {
                self.body.push(' ');
            }
            let resolved = match kind {
                ValueKind::Prefixes => reader.namespace_of(token).map(|uri| (uri, "")),
                _ => token.split_once(':').and_then(|(prefix, local)| {
                    Some((
                        reader.namespace_of(prefix).filter(|_| !prefix.is_empty())?,
                        local,
                    ))
                }),
            };
            match resolved {
                Some((uri, local)) => {
                    self.namespace(uri);
                    self.literal(local);
                }
                None => self.literal(token),
            }
        }
    }

    /// Writes the text not yet written.
    fn flush_text(&mut self) {
        if !self.text.is_empty() {
            escape::text(&self.text, &mut self.body);
            self.text.clear();
        }
    }

    /// The header of numbered namespaces followed by the content.
    fn finish(self) -> String {
        let mut form = String::new();
        for (number, uri) in self.names.iter().enumerate() {
            form.push('{');
            form.push_str(&number.to_string());
            form.push_str("} ");
            escape::attribute(uri, &mut form);
            form.push('\n');
        }
        form.push_str(&self.body);
        form
    }
}
