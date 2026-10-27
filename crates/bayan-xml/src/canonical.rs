//! A canonical form of documents, for comparing what they mean rather than how they are written.

use std::collections::BTreeMap;

use crate::chars::{is_space, split_qname};
use crate::escape;
use crate::event::{Event, NamespaceId, Space};
use crate::namespace::{KnownNamespace, Namespace};
use crate::reader::XML_ID;
use crate::{Error, Limits, Reader, Source};

/// The canonical form of `source`, for comparisons: two documents have the same canonical form exactly when they have the same elements, attributes, text, comments and processing instructions, with the same expanded names (namespace name and local name).
///
/// It ignores how a document is written: its encoding and XML declaration, prefixes and namespace declarations, the order of attributes and their quotes, references, CDATA sections, empty-element tags, line ends (which reading normalizes), white space outside the root element, and `xml:space` attributes that do not change the white space mode in force. Prefixes in attribute values are resolved where Markup Compatibility (ECMA-376 Part 3) and XML Schema give them a meaning: the lists of `mc:Ignorable`, `mc:MustUnderstand`, `mc:ProcessContent`, `mc:PreserveElements` and `mc:PreserveAttributes`, the list of `Requires` on `mc:Choice`, and the qualified name of `xsi:type`. Other qualified names in attribute values or text are compared as written, as in Exclusive XML Canonicalization.
///
/// The form is the content, with every name in a namespace written `{number}local`, every element with a start tag and an end tag, and text and attribute values escaped as in Canonical XML, followed by the namespace names, numbered in order of first use, each on a line of its own (`{0} http://…`). The attributes of each start tag are sorted: first those in no namespace, then by the number of their namespace, each by local name; namespaces that the attributes of one start tag use for the first time are numbered in the order of their names, so that the numbers do not depend on the order the attributes are written in. It is meant for comparisons, not for reading, and it is at most a few times larger than the document; building it takes at most about twice its own size in memory, and time in proportion to the document, however long its namespace names are.
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
                let choice = tag.name().is(KnownNamespace::MarkupCompatibility, "Choice");
                let mut attributes: Vec<(Option<NamespaceId>, &str, &str)> = tag
                    .attributes
                    .iter()
                    .filter(|attribute| !attribute.declaration)
                    .map(|attribute| {
                        (
                            attribute.namespace,
                            attribute.local,
                            attribute.value.as_ref(),
                        )
                    })
                    .filter(|&(namespace, local, value)| {
                        // xml:space that leaves the white space mode as it was changes nothing.
                        let redundant = namespace == Some(XML_ID)
                            && local == "space"
                            && matches!(value, "default" | "preserve")
                            && tag.space() == parent;
                        !redundant
                    })
                    .collect();
                form.body.push('<');
                form.name(tag.element.namespace, tag.element.local);
                // The namespaces the attributes use for the first time, numbered in the order of their names.
                let mut new: Vec<NamespaceId> = attributes
                    .iter()
                    .filter_map(|&(namespace, _, _)| namespace)
                    .filter(|&id| !form.numbers.contains_key(&id))
                    .collect();
                new.sort_unstable();
                new.dedup();
                new.sort_unstable_by_key(|&id| {
                    reader.namespace(id).map(|namespace| namespace.uri())
                });
                for id in new {
                    form.number(id);
                }
                attributes.sort_unstable_by_key(|&(namespace, local, _)| {
                    (
                        namespace.and_then(|id| form.numbers.get(&id).copied()),
                        local,
                    )
                });
                for (namespace, local, value) in attributes {
                    form.body.push(' ');
                    form.name(namespace, local);
                    form.body.push_str("=\"");
                    let view = namespace.and_then(|id| reader.namespace(id));
                    match ValueKind::of(choice, namespace.is_none(), view, local) {
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
                form.body.push_str("</");
                form.name(tag.element.namespace, tag.element.local);
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
    Ok(form.finish(&reader))
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
    /// The kind of the value of the attribute `local` in `namespace` (`unqualified` for an attribute in no namespace) on an element that is `mc:Choice` if `choice`.
    fn of(
        choice: bool,
        unqualified: bool,
        namespace: Option<Namespace<'_>>,
        local: &str,
    ) -> ValueKind {
        match (namespace.and_then(|namespace| namespace.known()), local) {
            (Some(KnownNamespace::MarkupCompatibility), "Ignorable" | "MustUnderstand") => {
                ValueKind::Prefixes
            }
            (None, "Requires") if choice && unqualified => ValueKind::Prefixes,
            (
                Some(KnownNamespace::MarkupCompatibility),
                "ProcessContent" | "PreserveElements" | "PreserveAttributes",
            ) => ValueKind::QualifiedNames,
            (Some(KnownNamespace::XmlSchemaInstance), "type") => ValueKind::QualifiedName,
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
    /// The number of each namespace, by the reader's identifier: comparing identifiers takes no time, however long the names.
    numbers: BTreeMap<NamespaceId, usize>,
    /// The namespaces in order of their numbers.
    names: Vec<NamespaceId>,
    /// The white space mode inside each open element.
    spaces: Vec<Space>,
}

impl Form {
    fn number(&mut self, id: NamespaceId) -> usize {
        if let Some(&number) = self.numbers.get(&id) {
            return number;
        }
        let number = self.names.len();
        self.numbers.insert(id, number);
        self.names.push(id);
        number
    }

    /// Writes the namespace `id` as its number in braces.
    fn namespace(&mut self, id: NamespaceId) {
        let number = self.number(id);
        self.body.push('{');
        self.body.push_str(&number.to_string());
        self.body.push('}');
    }

    /// Writes an element or attribute name.
    fn name(&mut self, namespace: Option<NamespaceId>, local: &str) {
        if let Some(id) = namespace {
            self.namespace(id);
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
                Some((prefix, local)) => match reader.namespace_id_of(prefix) {
                    Some(id) => {
                        self.namespace(id);
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
                ValueKind::Prefixes => reader.namespace_id_of(token).map(|id| (id, "")),
                _ => token.split_once(':').and_then(|(prefix, local)| {
                    Some((
                        reader
                            .namespace_id_of(prefix)
                            .filter(|_| !prefix.is_empty())?,
                        local,
                    ))
                }),
            };
            match resolved {
                Some((id, local)) => {
                    self.namespace(id);
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

    /// The content followed by the numbered namespace names, which `reader` holds, appended to it rather than put before it, so that the content is never copied.
    fn finish(self, reader: &Reader<'_>) -> String {
        let mut form = self.body;
        for (number, id) in self.names.iter().enumerate() {
            form.push('\n');
            form.push('{');
            form.push_str(&number.to_string());
            form.push_str("} ");
            let uri = reader
                .namespace(*id)
                .map_or("", |namespace| namespace.uri());
            escape::attribute(uri, &mut form);
        }
        form
    }
}
