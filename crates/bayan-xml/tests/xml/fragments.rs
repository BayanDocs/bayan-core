//! Lossless capture: an element's exact source text, with the namespace bindings and white space mode it had where it was found.

use bayan_xml::{Context, ErrorKind, Event, Fragment, Limits, Reader, Source, Space};

const DOCUMENT: &str = "<w:document xmlns:w=\"urn:w\" xmlns:w14=\"urn:w14\" xmlns:mc=\"urn:mc\" xmlns=\"urn:default\"><w:body><w:p w14:paraId=\"1\"><w:r><w:t xml:space=\"preserve\"> a </w:t></w:r><w14:x><w14:y/></w14:x><w:r/></w:p></w:body></w:document>";

/// Reads `text` until the start tag of the `occurrence`-th element named `local` (counting from 0) and captures it.
fn capture(text: &str, local: &str, occurrence: usize) -> Fragment<'static> {
    let limits = Limits::default();
    let source = Source::new(text.as_bytes(), &limits).unwrap();
    let mut reader = Reader::new(&source, &limits);
    let mut seen = 0;
    loop {
        let found = match reader.next_event().unwrap() {
            Some(Event::Start(tag)) => tag.name().local() == local,
            Some(_) => false,
            None => panic!("no such element"),
        };
        if found {
            if seen == occurrence {
                return reader.capture().unwrap().into_owned();
            }
            seen += 1;
        }
    }
}

fn bindings(context: &Context) -> Vec<(String, String)> {
    owned(&context.bindings())
}

fn owned(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(prefix, uri)| ((*prefix).to_owned(), (*uri).to_owned()))
        .collect()
}

fn pairs<'s>(list: &[(&'s str, &'s str)]) -> Vec<(&'s str, &'s str)> {
    list.to_vec()
}

#[test]
fn captures_the_exact_text_of_an_element() {
    let fragment = capture(DOCUMENT, "p", 0);
    assert_eq!(
        fragment.text(),
        "<w:p w14:paraId=\"1\"><w:r><w:t xml:space=\"preserve\"> a </w:t></w:r><w14:x><w14:y/></w14:x><w:r/></w:p>"
    );
    assert_eq!(capture(DOCUMENT, "y", 0).text(), "<w14:y/>");
    assert_eq!(capture(DOCUMENT, "r", 1).text(), "<w:r/>");
}

#[test]
fn reading_continues_after_the_captured_element() {
    let limits = Limits::default();
    let source = Source::new(DOCUMENT.as_bytes(), &limits).unwrap();
    let mut reader = Reader::new(&source, &limits);
    while !matches!(reader.next_event().unwrap(), Some(Event::Start(tag)) if tag.name().local() == "x")
    {
    }
    reader.capture().unwrap();
    // The current event is now the captured element's end tag; the next one follows it.
    let Some(Event::Start(next)) = reader.next_event().unwrap() else {
        panic!("expected the next start tag");
    };
    assert_eq!(next.name().local(), "r");
    assert_eq!(reader.depth(), 4);
}

#[test]
fn records_the_context_of_the_element() {
    let fragment = capture(DOCUMENT, "p", 0);
    assert_eq!(
        bindings(fragment.context()),
        owned(&[
            ("", "urn:default"),
            ("mc", "urn:mc"),
            ("w", "urn:w"),
            ("w14", "urn:w14")
        ])
    );
    assert_eq!(fragment.context().space(), Space::Default);
    assert_eq!(fragment.context().uri_of("w14"), Some("urn:w14"));
    assert_eq!(fragment.context().uri_of("other"), None);
    // Inside w:t the white space mode is "preserve"; an element there inherits it.
    let fragment = capture(
        "<a xmlns:p=\"urn:p\"><b xml:space=\"preserve\" xmlns:p=\"urn:inner\" xmlns=\"\"><c/></b></a>",
        "c",
        0,
    );
    assert_eq!(fragment.context().space(), Space::Preserve);
    assert_eq!(
        bindings(fragment.context()),
        owned(&[("", ""), ("p", "urn:inner")])
    );
}

#[test]
fn reports_the_context_of_the_current_event() {
    let limits = Limits::default();
    let source = Source::new(
        b"<a xmlns:p=\"urn:p\"><b xmlns:q=\"urn:q\">x</b></a>",
        &limits,
    )
    .unwrap();
    let mut reader = Reader::new(&source, &limits);
    let mut contexts = Vec::new();
    while let Some(event) = reader.next_event().unwrap() {
        let kind = match event {
            Event::Start(tag) => format!("<{}", tag.name().local()),
            Event::End(tag) => format!("</{}", tag.name().local()),
            _ => "text".to_owned(),
        };
        contexts.push((kind, bindings(&reader.context())));
    }
    let p = ("p".to_owned(), "urn:p".to_owned());
    let q = ("q".to_owned(), "urn:q".to_owned());
    assert_eq!(
        contexts,
        [
            // A start tag's context is where the element is, without its own declarations.
            ("<a".to_owned(), vec![]),
            ("<b".to_owned(), vec![p.clone()]),
            ("text".to_owned(), vec![p.clone(), q.clone()]),
            ("</b".to_owned(), vec![p.clone()]),
            ("</a".to_owned(), vec![]),
        ]
    );
}

#[test]
fn capture_needs_a_start_tag() {
    let limits = Limits::default();
    let source = Source::new(b"<a>x</a>", &limits).unwrap();
    let mut reader = Reader::new(&source, &limits);
    assert_eq!(
        reader.capture().unwrap_err().kind(),
        ErrorKind::NotAtStartTag
    );
    reader.next_event().unwrap();
    reader.next_event().unwrap();
    assert_eq!(
        reader.capture().unwrap_err().kind(),
        ErrorKind::NotAtStartTag
    );
}

#[test]
fn finds_the_namespaces_a_fragment_needs() {
    let fragment = capture(DOCUMENT, "p", 0);
    // w and w14 for names; not mc or the default namespace, which nothing in it uses.
    assert_eq!(
        fragment.needed_namespaces(),
        pairs(&[("w", "urn:w"), ("w14", "urn:w14")])
    );
    // Unprefixed element names need the default namespace, and an element without one needs none to be in force.
    let fragment = capture(
        "<r xmlns=\"urn:d\" xmlns:p=\"urn:p\"><a><p:b/></a></r>",
        "a",
        0,
    );
    assert_eq!(
        fragment.needed_namespaces(),
        pairs(&[("", "urn:d"), ("p", "urn:p")])
    );
    let fragment = capture("<r><a/></r>", "a", 0);
    assert_eq!(fragment.needed_namespaces(), pairs(&[("", "")]));
    // Prefixes in Markup Compatibility attributes and in xsi:type are uses too; declarations inside the fragment and the xml prefix are not needed.
    let fragment = capture(
        "<r xmlns:w=\"urn:w\" xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" xmlns:v1=\"urn:v1\" xmlns:v2=\"urn:v2\" xmlns:v3=\"urn:v3\" xmlns:v4=\"urn:v4\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns:t=\"urn:t\" xmlns:n=\"urn:n\"><w:a mc:Ignorable=\"v1 v2\" mc:ProcessContent=\"v2:x v1:*\"><mc:AlternateContent><mc:Choice Requires=\"v3\"><w:b xsi:type=\"t:typed\" xml:lang=\"en\" xmlns:n=\"urn:inner\" n:c=\"1\"/></mc:Choice></mc:AlternateContent></w:a></r>",
        "a",
        0,
    );
    assert_eq!(
        fragment.needed_namespaces(),
        pairs(&[
            (
                "mc",
                "http://schemas.openxmlformats.org/markup-compatibility/2006"
            ),
            ("t", "urn:t"),
            ("v1", "urn:v1"),
            ("v2", "urn:v2"),
            ("v3", "urn:v3"),
            ("w", "urn:w"),
            ("xsi", "http://www.w3.org/2001/XMLSchema-instance"),
        ])
    );
}

#[test]
fn makes_a_standalone_element() {
    let fragment = capture(DOCUMENT, "p", 0);
    let standalone = fragment.to_standalone().unwrap();
    assert_eq!(
        standalone,
        "<w:p xmlns:w=\"urn:w\" xmlns:w14=\"urn:w14\" w14:paraId=\"1\"><w:r><w:t xml:space=\"preserve\"> a </w:t></w:r><w14:x><w14:y/></w14:x><w:r/></w:p>"
    );
    // The white space mode travels too, unless the element sets its own.
    let fragment = capture(
        "<a xml:space=\"preserve\"><b>  </b><c xml:space=\"default\"/></a>",
        "b",
        0,
    );
    assert_eq!(
        fragment.to_standalone().unwrap(),
        "<b xml:space=\"preserve\">  </b>"
    );
    let fragment = capture(
        "<a xml:space=\"preserve\"><b>  </b><c xml:space=\"default\"/></a>",
        "c",
        0,
    );
    assert_eq!(
        fragment.to_standalone().unwrap(),
        "<c xml:space=\"default\"/>"
    );
    // Namespace names are escaped as attribute values.
    let fragment = capture("<r xmlns:p=\"urn:a&amp;b&quot;c\"><p:x/></r>", "x", 0);
    assert_eq!(
        fragment.to_standalone().unwrap(),
        "<p:x xmlns:p=\"urn:a&amp;b&quot;c\"/>"
    );
}

#[test]
fn reads_a_fragment_again_in_its_context() {
    let limits = Limits::default();
    let fragment = capture(DOCUMENT, "p", 0);
    let source = Source::new(fragment.text().as_bytes(), &limits).unwrap();
    let mut reader = Reader::with_context(&source, &limits, fragment.context()).unwrap();
    let Some(Event::Start(tag)) = reader.next_event().unwrap() else {
        panic!("expected the fragment's start tag");
    };
    assert_eq!(
        tag.name().namespace().map(|namespace| namespace.uri()),
        Some("urn:w")
    );
    while reader.next_event().unwrap().is_some() {}
}

#[test]
fn checks_fragments_made_by_hand() {
    let limits = Limits::default();
    let context = Context::new(&[("w", "urn:w")], Space::Default);
    let fragment = Fragment::new("<w:p><w:r/></w:p>", context.clone(), &limits).unwrap();
    assert_eq!(fragment.text(), "<w:p><w:r/></w:p>");
    for (text, kind) in [
        ("<w:p/><w:p/>", ErrorKind::ContentOutsideRoot),
        (" <w:p/>", ErrorKind::ContentOutsideRoot),
        ("<w:p/> ", ErrorKind::ContentOutsideRoot),
        ("<w:p/><!--c-->", ErrorKind::ContentOutsideRoot),
        (
            "<?xml version=\"1.0\"?><w:p/>",
            ErrorKind::MisplacedDeclaration,
        ),
        ("\u{FEFF}<w:p/>", ErrorKind::ContentOutsideRoot),
        ("<x:p/>", ErrorKind::UndeclaredPrefix),
        ("<w:p>", ErrorKind::UnexpectedEnd),
        ("text", ErrorKind::ContentOutsideRoot),
    ] {
        let error = Fragment::new(text, context.clone(), &limits).unwrap_err();
        assert_eq!(error.kind(), kind, "{text:?}");
    }
}

#[test]
fn refuses_contexts_that_namespaces_in_xml_forbids() {
    let limits = Limits::default();
    for bindings in [
        &[("xmlns", "urn:x")][..],
        &[("p", "")],
        &[("p", "http://www.w3.org/2000/xmlns/")],
        &[("xml", "urn:other")],
        &[("1p", "urn:x")],
        &[("p:q", "urn:x")],
        &[("p", "urn:\u{0}")],
    ] {
        let context = Context::new(bindings, Space::Default);
        let error = Fragment::new("<a/>", context, &limits).unwrap_err();
        assert_eq!(
            error.kind(),
            ErrorKind::InvalidNamespaceDeclaration,
            "{bindings:?}"
        );
    }
    let limits = Limits {
        max_namespaces: 1,
        ..Limits::default()
    };
    let context = Context::new(&[("p", "urn:p"), ("q", "urn:q")], Space::Default);
    assert_eq!(
        Fragment::new("<a/>", context, &limits).unwrap_err().kind(),
        ErrorKind::TooManyNamespaces
    );
}
