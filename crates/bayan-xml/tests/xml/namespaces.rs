//! Namespaces in XML: resolving prefixes, the rules for declarations, known namespaces and their Strict names.

use bayan_xml::{ErrorKind, Event, KnownNamespace, Limits, Reader, Source};

use crate::common::{accepts, refusal, refusal_with};

/// Every element and attribute name of `text`, in document order, as `(prefix, local name, namespace URI)`; declarations included.
fn names(text: &str) -> Vec<(String, String, Option<String>)> {
    let source = Source::new(text.as_bytes(), &Limits::default()).unwrap();
    let mut reader = Reader::new(&source, &Limits::default());
    let mut names = Vec::new();
    let mut add = |name: bayan_xml::Name<'_>| {
        names.push((
            name.prefix().to_owned(),
            name.local().to_owned(),
            name.namespace().map(|namespace| namespace.uri().to_owned()),
        ));
    };
    while let Some(event) = reader.next_event().unwrap() {
        if let Event::Start(tag) = event {
            add(tag.name());
            for attribute in tag.attributes() {
                add(attribute.name());
            }
        }
    }
    names
}

fn name(prefix: &str, local: &str, uri: Option<&str>) -> (String, String, Option<String>) {
    (prefix.to_owned(), local.to_owned(), uri.map(str::to_owned))
}

const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
const XML: &str = "http://www.w3.org/XML/1998/namespace";

#[test]
fn resolves_prefixes_and_the_default_namespace() {
    assert_eq!(
        names(
            "<a xmlns=\"urn:d\" xmlns:p=\"urn:p\" b=\"1\" p:c=\"2\"><p:e/><f xml:space=\"preserve\"/></a>"
        ),
        [
            name("", "a", Some("urn:d")),
            name("", "xmlns", Some(XMLNS)),
            name("xmlns", "p", Some(XMLNS)),
            // An attribute without a prefix is in no namespace, whatever the default namespace.
            name("", "b", None),
            name("p", "c", Some("urn:p")),
            name("p", "e", Some("urn:p")),
            name("", "f", Some("urn:d")),
            // The prefix xml is bound without a declaration.
            name("xml", "space", Some(XML)),
        ]
    );
}

#[test]
fn declarations_shadow_outer_ones_until_their_element_ends() {
    assert_eq!(
        names(
            "<p:a xmlns:p=\"urn:1\"><p:b xmlns:p=\"urn:2\"><p:c/></p:b><p:d/><e xmlns=\"urn:3\"><f xmlns=\"\"/><g/></e><h/></p:a>"
        ),
        [
            name("p", "a", Some("urn:1")),
            name("xmlns", "p", Some(XMLNS)),
            name("p", "b", Some("urn:2")),
            name("xmlns", "p", Some(XMLNS)),
            name("p", "c", Some("urn:2")),
            name("p", "d", Some("urn:1")),
            name("", "e", Some("urn:3")),
            name("", "xmlns", Some(XMLNS)),
            // xmlns="" undeclares the default namespace.
            name("", "f", None),
            name("", "xmlns", Some(XMLNS)),
            name("", "g", Some("urn:3")),
            name("", "h", None),
        ]
    );
}

#[test]
fn declarations_apply_to_their_own_element_wherever_they_are_written() {
    assert_eq!(
        names("<p:a p:b=\"1\" xmlns:p=\"urn:p\"/>"),
        [
            name("p", "a", Some("urn:p")),
            name("p", "b", Some("urn:p")),
            name("xmlns", "p", Some(XMLNS)),
        ]
    );
}

#[test]
fn resolves_references_in_namespace_names() {
    assert_eq!(
        names("<p:a xmlns:p=\"urn:x?a=1&amp;b=&#x32;\"/>")[0],
        name("p", "a", Some("urn:x?a=1&b=2"))
    );
}

#[test]
fn refuses_undeclared_prefixes() {
    assert_eq!(refusal("<p:a/>"), (ErrorKind::UndeclaredPrefix, 1));
    assert_eq!(refusal("<a p:b=\"1\"/>"), (ErrorKind::UndeclaredPrefix, 3));
    assert_eq!(
        refusal("<a><p:b xmlns:p=\"urn:p\"/><p:c/></a>"),
        (ErrorKind::UndeclaredPrefix, 26)
    );
    assert_eq!(refusal("<a></p:a>").0, ErrorKind::MismatchedEndTag);
}

#[test]
fn refuses_the_xmlns_prefix_on_elements() {
    assert_eq!(refusal("<xmlns:a/>"), (ErrorKind::ReservedPrefix, 1));
}

#[test]
fn refuses_declarations_that_namespaces_in_xml_forbids() {
    for (text, offset) in [
        ("<a xmlns:xml=\"urn:other\"/>", 3),
        ("<a xmlns:p=\"http://www.w3.org/XML/1998/namespace\"/>", 3),
        ("<a xmlns=\"http://www.w3.org/XML/1998/namespace\"/>", 3),
        ("<a xmlns:xmlns=\"http://www.w3.org/2000/xmlns/\"/>", 3),
        ("<a xmlns:p=\"http://www.w3.org/2000/xmlns/\"/>", 3),
        ("<a xmlns=\"http://www.w3.org/2000/xmlns/\"/>", 3),
        // Namespaces in XML 1.0 has no way to undeclare a prefix.
        ("<a xmlns:p=\"\"/>", 3),
    ] {
        assert_eq!(
            refusal(text),
            (ErrorKind::InvalidNamespaceDeclaration, offset),
            "{text}"
        );
    }
    // The xml prefix may be declared, to its own namespace.
    assert!(accepts(
        "<a xmlns:xml=\"http://www.w3.org/XML/1998/namespace\" xml:lang=\"en\"/>"
    ));
}

#[test]
fn knows_the_namespaces_of_office_documents_by_both_names() {
    let text = "<w:document xmlns:w=\"http://purl.oclc.org/ooxml/wordprocessingml/main\" xmlns:t=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\" xmlns:u=\"urn:unknown\"><w:body/><t:body/><w14:x/><u:y/><plain/></w:document>";
    let source = Source::new(text.as_bytes(), &Limits::default()).unwrap();
    let mut reader = Reader::new(&source, &Limits::default());
    let mut found = Vec::new();
    while let Some(event) = reader.next_event().unwrap() {
        if let Event::Start(tag) = event {
            let namespace = tag.name().namespace();
            found.push((
                tag.name().local().to_owned(),
                namespace.and_then(|namespace| namespace.known()),
                namespace.is_some_and(|namespace| namespace.is_strict()),
            ));
            if tag.name().local() == "body" {
                assert!(tag.name().is(KnownNamespace::WordprocessingMl, "body"));
                assert!(!tag.name().is(KnownNamespace::WordprocessingMl, "p"));
            }
        }
    }
    assert_eq!(
        found,
        [
            (
                "document".to_owned(),
                Some(KnownNamespace::WordprocessingMl),
                true
            ),
            (
                "body".to_owned(),
                Some(KnownNamespace::WordprocessingMl),
                true
            ),
            (
                "body".to_owned(),
                Some(KnownNamespace::WordprocessingMl),
                false
            ),
            ("x".to_owned(), Some(KnownNamespace::W14), false),
            ("y".to_owned(), None, false),
            ("plain".to_owned(), None, false),
        ]
    );
}

#[test]
fn limits_the_number_of_distinct_namespaces() {
    let limits = Limits {
        max_namespaces: 2,
        ..Limits::default()
    };
    // The namespaces of the xml and xmlns prefixes are not declared, so they do not count; declaring the same name again, anywhere, does not count again.
    let text =
        "<a xmlns:p=\"urn:1\" xmlns:q=\"urn:2\"><b xmlns:r=\"urn:1\"/><c xmlns=\"urn:2\"/></a>";
    let source = Source::new(text.as_bytes(), &limits).unwrap();
    let mut reader = Reader::new(&source, &limits);
    while reader.next_event().unwrap().is_some() {}
    assert_eq!(
        refusal_with(
            "<a xmlns:p=\"urn:1\"><b xmlns:q=\"urn:2\"/><c xmlns:q=\"urn:3\"/></a>",
            &limits
        ),
        (ErrorKind::TooManyNamespaces, 42)
    );
}

#[test]
fn limits_the_length_of_namespace_names() {
    let limits = Limits {
        max_namespace_name_length: 8,
        ..Limits::default()
    };
    // As long as the limit allows, counted after references are resolved: read.
    let text = "<a xmlns:p=\"urn:1234\" xmlns=\"urn:&#x31;&#x32;34\"><p:b/></a>";
    let source = Source::new(text.as_bytes(), &limits).unwrap();
    let mut reader = Reader::new(&source, &limits);
    while reader.next_event().unwrap().is_some() {}
    // One byte longer: refused where the declaration starts, for a prefix or the default namespace, anywhere.
    assert_eq!(
        refusal_with("<a xmlns:p=\"urn:12345\"/>", &limits),
        (ErrorKind::NamespaceNameTooLong, 3)
    );
    assert_eq!(
        refusal_with("<a><b xmlns=\"urn:1234&#x35;\"/></a>", &limits),
        (ErrorKind::NamespaceNameTooLong, 6)
    );
}
