//! The XML of one part, as the tagger and the S3 store's listings read it: element starts and ends with their namespace-resolved names and attributes, and text, handed to a [`Handler`] in document order.
//!
//! bayan-xml's streaming reader does all the reading ([`bayan_xml::Reader`]), the same reader that the engine uses, so a part is read here exactly as strictly as BayanDocs reads it: it decodes UTF-8 and UTF-16 (with a byte order mark), refuses document type declarations and with them every entity but the five predefined ones (no "billion laughs", no external entities), refuses everything else that XML 1.0 and Namespaces in XML forbid, and enforces its [`Limits`] on size, nesting, attributes, names and namespaces before the work they protect. Time and memory grow linearly with the size of the part. This module only hands its events on, and keeps the namespace bindings in scope for [`Start::namespace_of_prefix`].

use std::borrow::Cow;
use std::collections::BTreeMap;

use bayan_xml::{Event, Limits, Reader, Source, StartTag};

/// The namespace that the prefix `xml` is always bound to.
pub const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// A namespace-resolved name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Name<'a> {
    /// The namespace URI, or `""` for no namespace.
    pub namespace: &'a str,
    /// The local name.
    pub local: &'a str,
}

impl<'a> Name<'a> {
    fn of(name: &bayan_xml::Name<'a>) -> Self {
        Self {
            namespace: name.namespace().map_or("", |namespace| namespace.uri()),
            local: name.local(),
        }
    }
}

/// An attribute value, with its references resolved and its white space normalized as XML 1.0 §3.3.3 requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Value<'a>(&'a str);

impl<'a> Value<'a> {
    /// The value.
    #[must_use]
    pub fn decoded(&self) -> Cow<'a, str> {
        Cow::Borrowed(self.0)
    }
}

/// Text inside an element: a run of character data, with its references resolved and its line ends normalized, or the content of a CDATA section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Text<'a>(&'a str);

impl<'a> Text<'a> {
    /// The text.
    #[must_use]
    pub fn decoded(&self) -> Cow<'a, str> {
        Cow::Borrowed(self.0)
    }
}

/// An element's start tag, as the [`Handler`] sees it.
pub struct Start<'b, 'r> {
    tag: &'b StartTag<'r>,
    scope: &'b Scope,
}

impl<'b, 'r> Start<'b, 'r> {
    /// The element's name.
    #[must_use]
    pub fn name(&self) -> Name<'r> {
        Name::of(&self.tag.name())
    }

    /// The element's attributes, without namespace declarations, with their resolved names.
    pub fn attributes(&self) -> impl Iterator<Item = (Name<'r>, Value<'r>)> + 'b {
        self.tag
            .attributes()
            .filter(|attribute| !attribute.is_namespace_declaration())
            .map(|attribute| (Name::of(&attribute.name()), Value(attribute.value())))
    }

    /// The value of the attribute with this namespace (`""` for none) and local name.
    #[must_use]
    pub fn attribute(&self, namespace: &str, local: &str) -> Option<Value<'r>> {
        self.attributes()
            .find(|(name, _)| name.namespace == namespace && name.local == local)
            .map(|(_, value)| value)
    }

    /// The namespace URI that `prefix` is bound to where this element starts, its own declarations included, as Markup Compatibility's `Requires` and `Ignorable` attributes need; `None` if it is not bound to one.
    #[must_use]
    pub fn namespace_of_prefix(&self, prefix: &str) -> Option<&'b str> {
        if prefix == "xml" {
            return Some(XML_NAMESPACE);
        }
        self.scope.namespace_of(prefix)
    }
}

/// Receives what the reader reads, in document order. For an empty element (`<a/>`), [`Handler::end`] follows [`Handler::start`] immediately.
pub trait Handler {
    /// An element starts.
    fn start(&mut self, start: &Start<'_, '_>);
    /// An element ends.
    fn end(&mut self, name: Name<'_>);
    /// Text inside an element: a run of character data or a CDATA section.
    fn text(&mut self, text: Text<'_>);
}

/// The namespace bindings in scope, by prefix (`""` for the default namespace), innermost last, with the depth of the element that declared each. bayan-xml checks the declarations; this keeps them only for [`Start::namespace_of_prefix`], which its events do not answer.
#[derive(Debug, Default)]
struct Scope {
    by_prefix: BTreeMap<String, Vec<(usize, String)>>,
    /// The prefixes in the order they were declared, with the depth of the element that declared them.
    declared: Vec<(usize, String)>,
}

impl Scope {
    /// Adds the namespace declarations of the start tag `tag`.
    fn enter(&mut self, tag: &StartTag<'_>) {
        let depth = tag.depth();
        for attribute in tag.attributes() {
            if !attribute.is_namespace_declaration() {
                continue;
            }
            let name = attribute.name();
            // `xmlns` declares the default namespace, `xmlns:p` the prefix p.
            let prefix = if name.prefix().is_empty() {
                ""
            } else {
                name.local()
            };
            self.by_prefix
                .entry(prefix.to_owned())
                .or_default()
                .push((depth, attribute.value().to_owned()));
            self.declared.push((depth, prefix.to_owned()));
        }
    }

    /// Removes the declarations of the element at `depth`, which ends.
    fn leave(&mut self, depth: usize) {
        while self
            .declared
            .last()
            .is_some_and(|(declared, _)| *declared >= depth)
        {
            let Some((_, prefix)) = self.declared.pop() else {
                break;
            };
            if let Some(bindings) = self.by_prefix.get_mut(&prefix) {
                bindings.pop();
                if bindings.is_empty() {
                    self.by_prefix.remove(&prefix);
                }
            }
        }
    }

    /// The namespace that `prefix` is bound to, if it is bound to one (`xmlns=""` binds none).
    fn namespace_of(&self, prefix: &str) -> Option<&str> {
        self.by_prefix
            .get(prefix)
            .and_then(|bindings| bindings.last())
            .map(|(_, uri)| uri.as_str())
            .filter(|uri| !uri.is_empty())
    }
}

/// Reads `bytes` as one XML part within `limits`, handing its events to `handler`.
///
/// # Errors
///
/// bayan-xml's [`bayan_xml::Error`] when the part is not well-formed XML, uses a feature that bayan-xml refuses, or goes beyond `limits`. The handler may already have received the events before the error.
pub fn read(
    bytes: &[u8],
    limits: &Limits,
    handler: &mut impl Handler,
) -> Result<(), bayan_xml::Error> {
    let source = Source::new(bytes, limits)?;
    let mut reader = Reader::new(&source, limits);
    let mut scope = Scope::default();
    while let Some(event) = reader.next_event()? {
        match event {
            Event::Start(tag) => {
                scope.enter(&tag);
                handler.start(&Start {
                    tag: &tag,
                    scope: &scope,
                });
            }
            Event::End(end) => {
                handler.end(Name::of(&end.name()));
                scope.leave(end.depth());
            }
            Event::Text(text) => handler.text(Text(text.value())),
            Event::CData(cdata) => handler.text(Text(cdata.value())),
            Event::Comment(_) | Event::ProcessingInstruction(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records what a handler receives.
    #[derive(Default)]
    struct Recorder {
        seen: Vec<String>,
    }

    impl Handler for Recorder {
        fn start(&mut self, start: &Start<'_, '_>) {
            let name = start.name();
            let attributes: Vec<String> = start
                .attributes()
                .map(|(name, value)| {
                    format!("{{{}}}{}={}", name.namespace, name.local, value.decoded())
                })
                .collect();
            let requires = start.attribute("", "Requires").map(|prefix| {
                start
                    .namespace_of_prefix(&prefix.decoded())
                    .unwrap_or("-")
                    .to_owned()
            });
            self.seen.push(format!(
                "start {{{}}}{} {attributes:?} {requires:?}",
                name.namespace, name.local
            ));
        }
        fn end(&mut self, name: Name<'_>) {
            self.seen
                .push(format!("end {{{}}}{}", name.namespace, name.local));
        }
        fn text(&mut self, text: Text<'_>) {
            self.seen.push(format!("text {:?}", text.decoded()));
        }
    }

    #[test]
    fn hands_on_names_attributes_and_text_in_document_order() {
        let mut recorder = Recorder::default();
        read(
            b"<?xml version='1.0'?><!--c--><r xmlns='urn:d' xmlns:p='urn:p' p:a='1&amp;2' b='x\ty'><p:e/>t&lt;\r\n<![CDATA[<c>]]><?pi?></r>",
            &Limits::DEFAULT,
            &mut recorder,
        )
        .unwrap();
        assert_eq!(
            recorder.seen,
            [
                r#"start {urn:d}r ["{urn:p}a=1&2", "{}b=x y"] None"#,
                "start {urn:p}e [] None",
                "end {urn:p}e",
                r#"text "t<\n""#,
                r#"text "<c>""#,
                "end {urn:d}r",
            ]
        );
    }

    #[test]
    fn resolves_prefixes_where_the_element_starts() {
        // An element's own declarations count; those of elements that ended do not; `xmlns=""` and `xml` are handled as XML says.
        let mut recorder = Recorder::default();
        read(
            br#"<r xmlns:a="urn:1"><x Requires="a"/><y xmlns:a="urn:2" Requires="a"/><z Requires="a"/><w xmlns:b="urn:3"/><v Requires="b"/><u Requires="xml"/><t xmlns="" Requires=""/></r>"#,
            &Limits::DEFAULT,
            &mut recorder,
        )
        .unwrap();
        let requires: Vec<&str> = recorder
            .seen
            .iter()
            .filter(|seen| seen.starts_with("start"))
            .map(|seen| seen.rsplit(' ').next().unwrap())
            .collect();
        assert_eq!(
            requires,
            [
                "None",
                r#"Some("urn:1")"#,
                r#"Some("urn:2")"#,
                r#"Some("urn:1")"#,
                "None",
                r#"Some("-")"#,
                r#"Some("http://www.w3.org/XML/1998/namespace")"#,
                r#"Some("-")"#,
            ]
        );
    }

    #[test]
    fn refuses_what_bayan_xml_refuses() {
        for (bytes, kind) in [
            (
                &b"<!DOCTYPE d [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><d>&x;</d>"[..],
                bayan_xml::ErrorKind::DoctypeNotAllowed,
            ),
            (b"<d>&x;</d>", bayan_xml::ErrorKind::UndefinedEntity),
            (b"<p:d/>", bayan_xml::ErrorKind::UndeclaredPrefix),
            (b"<d>", bayan_xml::ErrorKind::UnexpectedEnd),
        ] {
            let error = read(bytes, &Limits::DEFAULT, &mut Recorder::default()).unwrap_err();
            assert_eq!(error.kind(), kind);
        }
        let shallow = Limits {
            max_depth: 2,
            ..Limits::DEFAULT
        };
        let mut recorder = Recorder::default();
        let error = read(b"<a><b><c/></b></a>", &shallow, &mut recorder).unwrap_err();
        assert_eq!(error.kind(), bayan_xml::ErrorKind::TooDeep);
        // The handler has the events before the error.
        assert_eq!(recorder.seen.len(), 2);
    }
}
