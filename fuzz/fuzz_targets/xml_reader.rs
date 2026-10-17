//! bayan-xml's reader, writer, fragments and canonical form, on arbitrary bytes. For every document the reader accepts: the spans of its events follow each other without gap inside the root element; what it decodes is never longer than the document; written again from its events, it has the same canonical form, and that output written again is the same bytes; the root element captured is the same bytes; every child of the root element is, standalone, what the writer writes as the root element of a document, and refused only where its root element may not keep its white space mode; and every child moved into a context that binds the prefixes it uses to other namespaces keeps its meaning.

#![no_main]

use bayan_xml::{
    Event, Fragment, Limits, QName, Reader, Source, Space, WriteError, Writer, canonicalize,
};
use libfuzzer_sys::fuzz_target;

/// The limits documents meet.
const LIMITS: Limits = Limits::DEFAULT;

/// The limits for reading back what the writer wrote, which may go beyond those of its input: a start tag with the declarations a fragment needs added, and the new root element's namespaces and prefix.
const READ_BACK: Limits = Limits {
    max_attributes: usize::MAX,
    max_name_length: usize::MAX,
    max_namespaces: usize::MAX,
    ..Limits::DEFAULT
};

/// The namespace of the `xml` prefix.
const XML: &str = "http://www.w3.org/XML/1998/namespace";

/// The document written again from its events.
fn rewrite(source: &Source<'_>) -> Result<String, WriteError> {
    let mut reader = Reader::new(source, &LIMITS);
    let mut writer = Writer::new();
    if let Some(declaration) = source.declaration() {
        writer.declaration(declaration.standalone())?;
    }
    while let Some(event) = reader.next_event().unwrap() {
        match event {
            Event::Start(tag) => writer.start_tag(&tag)?,
            Event::End(_) => writer.end()?,
            Event::Text(text) => writer.text(text.value())?,
            Event::CData(cdata) => writer.cdata(cdata.value())?,
            Event::Comment(comment) => writer.comment(comment.text())?,
            Event::ProcessingInstruction(pi) => {
                writer.processing_instruction(pi.target(), pi.data())?;
            }
        }
    }
    writer.finish()
}

/// The canonical form of `text`, which the writer wrote.
fn canonical(text: &str) -> String {
    let source = Source::new(text.as_bytes(), &READ_BACK).unwrap();
    canonicalize(&source, &READ_BACK).unwrap()
}

/// Opens an element that sets `space`, the white space mode of a fragment written inside it, so that any fragment can be written there.
fn set_space(writer: &mut Writer, space: Space) {
    if space == Space::Preserve {
        writer
            .attribute(QName::new(Some(XML), "xml", "space"), "preserve")
            .unwrap();
    }
}

/// The canonical form of `fragment` written inside a root element in no namespace that keeps its white space mode.
fn in_its_mode(fragment: &Fragment<'_>) -> String {
    let mut writer = Writer::new();
    writer.start(QName::unqualified("n")).unwrap();
    set_space(&mut writer, fragment.context().space());
    writer.fragment(fragment).unwrap();
    writer.end().unwrap();
    canonical(&writer.finish().unwrap())
}

fuzz_target!(|data: &[u8]| {
    let Ok(source) = Source::new(data, &LIMITS) else {
        return;
    };
    let text = source.text();
    let mut reader = Reader::new(&source, &LIMITS);
    let mut end = 0;
    let mut inside = false;
    let mut decoded = 0usize;
    let mut root = None;
    let mut children = Vec::new();
    loop {
        let event = match reader.next_event() {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(_) => {
                // After an error, the reader stays at the end.
                assert!(matches!(reader.next_event(), Ok(None)));
                return;
            }
        };
        let span = match &event {
            Event::Start(tag) => {
                decoded += tag
                    .attributes()
                    .map(|attribute| attribute.value().len())
                    .sum::<usize>();
                tag.span()
            }
            Event::End(tag) => {
                if tag.depth() == 1 {
                    root = Some(tag.element_span());
                }
                if tag.depth() == 2 {
                    children.push(tag.element_span());
                }
                tag.span()
            }
            Event::Text(text) => {
                decoded += text.value().len();
                text.span()
            }
            Event::CData(cdata) => {
                decoded += cdata.value().len();
                cdata.span()
            }
            Event::Comment(comment) => comment.span(),
            Event::ProcessingInstruction(pi) => pi.span(),
        };
        assert!(span.start <= span.end && span.end <= text.len());
        // Inside the root element, every event starts where the one before ended.
        if inside {
            assert_eq!(span.start, end);
        } else {
            assert!(span.start >= end);
        }
        end = span.end;
        match &event {
            Event::Start(_) => inside = true,
            Event::End(tag) if tag.depth() == 1 => inside = false,
            _ => {}
        }
    }
    // Entity expansion is impossible: decoding never makes text longer.
    assert!(decoded <= text.len());
    let root = root.expect("an accepted document has a root element");
    let expected = canonicalize(&source, &LIMITS).unwrap();
    // Written again from its events, the document means the same, and the writer's output is stable. The writer refuses one thing the reader accepts: a Markup Compatibility list naming a prefix that is not declared, which Markup Compatibility processing refuses too.
    match rewrite(&source) {
        Ok(written) => {
            assert_eq!(canonical(&written), expected);
            let again = Source::new(written.as_bytes(), &LIMITS).unwrap();
            assert_eq!(rewrite(&again).unwrap(), written);
        }
        Err(error) => assert_eq!(error, WriteError::UndeclaredPrefix),
    }
    // The root element captured is its text, and standalone it needs nothing.
    let mut reader = Reader::new(&source, &LIMITS);
    while !matches!(reader.next_event().unwrap(), Some(Event::Start(_))) {}
    let fragment = reader.capture().unwrap();
    assert_eq!(fragment.text(), &text[root]);
    assert_eq!(fragment.to_standalone(), Ok(fragment.text().to_owned()));
    // Every child of the root element, in a context that binds the prefixes it uses to another namespace, keeps its meaning.
    let mut reader = Reader::new(&source, &LIMITS);
    let mut captured = Vec::new();
    loop {
        let capture = match reader.next_event().unwrap() {
            None => break,
            Some(Event::Start(tag)) => tag.depth() == 2,
            Some(_) => false,
        };
        if capture {
            captured.push(reader.capture().unwrap());
        }
    }
    assert_eq!(captured.len(), children.len());
    for (fragment, span) in captured.iter().zip(&children) {
        assert_eq!(fragment.text(), &text[span.clone()]);
        // Standalone, a child is what the writer writes as the root element of a document; both refuse it only where its root element may not have the xml:space that would keep its white space mode.
        let standalone = fragment.to_standalone();
        let mut alone = Writer::new();
        assert_eq!(
            alone.fragment(fragment).and_then(|()| alone.finish()),
            standalone
        );
        match &standalone {
            Ok(standalone) => {
                canonical(standalone);
            }
            Err(error) => {
                assert_eq!(*error, WriteError::SpaceConflict);
                assert_eq!(fragment.context().space(), Space::Preserve);
            }
        }
        let before = in_its_mode(fragment);
        let needed = fragment.needed_namespaces();
        // A prefix for the new root element that the fragment does not use.
        let mut prefix = String::from("h");
        while needed.iter().any(|(used, _)| *used == prefix) {
            prefix.push('h');
        }
        let mut writer = Writer::new();
        writer
            .start(QName::new(Some("urn:bayan:hostile"), &prefix, "root"))
            .unwrap();
        for (used, _) in &needed {
            writer.declare(used, "urn:bayan:other").unwrap();
        }
        set_space(&mut writer, fragment.context().space());
        writer.fragment(fragment).unwrap();
        writer.end().unwrap();
        let moved = writer.finish().unwrap();
        let moved_source = Source::new(moved.as_bytes(), &READ_BACK).unwrap();
        let mut reader = Reader::new(&moved_source, &READ_BACK);
        reader.next_event().unwrap();
        reader.next_event().unwrap();
        let moved = reader.capture().unwrap();
        assert_eq!(in_its_mode(&moved), before);
    }
});
