//! bayan-xml's Markup Compatibility processing (`McReader`) on arbitrary bytes, with a configuration that the first byte chooses: which of six namespaces and names without one are understood, and whether Office Open XML's extension lists and one more extension element count. For every document it processes: the lossless events, written back, give a document with the input's canonical form; every fragment it reports (ignored elements, whole alternate content) stands on its own with the declarations it records, unless its root element may not keep its white space mode, and processed again standalone it is refused for no attribute in the XML namespace on its root element; an element captured from the processor reads again in its context; the output document, processed again, is unchanged; and the document written back with every `xml:space` attribute left out of its start tags, so that fragments are written where another white space mode holds, is refused only for a fragment whose root element may not set it, and is processed again without error otherwise.

#![no_main]

use bayan_xml::{
    AttributeKind, CompatibilityError, ErrorKind, Event, Fragment, Limits, McConfig, McEvent,
    McReader, NamespaceName, QName, Reader, Source, Space, StartTag, WriteError, Writer,
    canonicalize,
};
use libfuzzer_sys::fuzz_target;

/// The limits documents meet.
const LIMITS: Limits = Limits::DEFAULT;

/// The limits for reading back what the writer wrote, which may hold more attributes than any start tag of its input: the declarations and `xml:space` it adds where a fragment needs them.
const READ_BACK: Limits = Limits {
    max_attributes: usize::MAX,
    ..Limits::DEFAULT
};

/// The namespace of the `xml` prefix.
const XML: &str = "http://www.w3.org/XML/1998/namespace";

/// The namespaces the configuration may understand, one for each of the first six bits of the first byte.
const NAMESPACES: [&str; 6] = [
    "urn:a",
    "urn:b",
    "urn:c",
    "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    "http://schemas.microsoft.com/office/word/2010/wordml",
    "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
];

fn config(selector: u8) -> McConfig {
    let mut config = McConfig::new();
    for (bit, uri) in NAMESPACES.iter().enumerate() {
        if selector & (1 << bit) != 0 {
            config = config.understand(NamespaceName::from_uri(uri));
        }
    }
    if selector & 0x40 != 0 {
        config = config.understand(NamespaceName::None);
    }
    if selector & 0x80 != 0 {
        config = config
            .with_ooxml_extension_elements()
            .extension_element(NamespaceName::from_uri("urn:a"), "ext");
    }
    config
}

/// The canonical form of `text`, which the writer wrote.
fn canonical(text: &str) -> String {
    let source = Source::new(text.as_bytes(), &READ_BACK).unwrap();
    canonicalize(&source, &READ_BACK).unwrap()
}

/// How a pass writes the document back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Start {
    /// Every start tag as it was.
    AsItWas,
    /// Every start tag without its `xml:space` attribute, so that fragments are written where another white space mode may hold.
    WithoutSpace,
}

/// What one pass over the document gives: the lossless writer's and the output writer's results, and the fragments reported.
struct Pass {
    lossless: Result<String, WriteError>,
    output: Result<String, WriteError>,
    fragments: Vec<Fragment<'static>>,
}

/// Opens `tag` as [`Writer::start_tag`] does, but without its `xml:space` attribute.
fn start_without_space(writer: &mut Writer, tag: &StartTag<'_>) -> Result<(), WriteError> {
    writer.start(QName::from_name(&tag.name()))?;
    for attribute in tag.attributes() {
        let name = attribute.name();
        if attribute.is_namespace_declaration() {
            let prefix = if name.prefix().is_empty() {
                ""
            } else {
                name.local()
            };
            writer.declare(prefix, attribute.value())?;
        } else if !(name.local() == "space" && name.namespace().is_some_and(|ns| ns.uri() == XML)) {
            writer.attribute(QName::from_name(&name), attribute.value())?;
        }
    }
    Ok(())
}

/// Writes `fragment`. The writer may refuse it for its white space mode only if its root element may not set one (`fixed`: alternate content, or an element whose content is processed).
fn write_fragment(
    writer: &mut Writer,
    fragment: &Fragment<'_>,
    fixed: bool,
) -> Result<(), WriteError> {
    let result = writer.fragment(fragment);
    if result == Err(WriteError::SpaceConflict) {
        assert!(fixed);
    }
    result
}

/// Processes `source` to its end within `limits`, or returns `None` at an error. `capture_at` names the start tag (counted from 0) whose element is captured whole instead of processed.
fn process(
    source: &Source<'_>,
    limits: &Limits,
    config: &McConfig,
    capture_at: Option<usize>,
    start: Start,
) -> Option<Pass> {
    let mut reader = McReader::new(source, limits, config);
    let mut lossless = Writer::new();
    let mut lossless_result = Ok(());
    let mut output = Writer::new();
    let mut output_result = Ok(());
    if let Some(declaration) = source.declaration() {
        lossless_result = lossless.declaration(declaration.standalone());
    }
    let mut fragments = Vec::new();
    // How deep inside alternate content the events are: its content is written from its fragment.
    let mut alternate = 0usize;
    let mut starts = 0usize;
    loop {
        let event = match reader.next_event() {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(_) => {
                assert!(matches!(reader.next_event(), Ok(None)));
                return None;
            }
        };
        let mut capture = false;
        // Whether the element captured may not set its white space mode.
        let mut fixed = false;
        match &event {
            McEvent::Start(tag)
            | McEvent::UnwrapStart(tag)
            | McEvent::AlternateContentStart(tag) => {
                capture = capture_at == Some(starts);
                fixed = !matches!(event, McEvent::Start(_));
                starts += 1;
                if !capture && alternate == 0 && !matches!(event, McEvent::AlternateContentStart(_))
                {
                    lossless_result = lossless_result.and_then(|()| match start {
                        Start::AsItWas => lossless.start_tag(tag.tag()),
                        Start::WithoutSpace => start_without_space(&mut lossless, tag.tag()),
                    });
                }
                if !capture && matches!(event, McEvent::Start(_)) {
                    output_result = output_result.and_then(|()| {
                        output.start(QName::from_name(&tag.name()))?;
                        for (attribute, kind) in tag.attributes() {
                            let name = attribute.name();
                            match kind {
                                AttributeKind::NamespaceDeclaration => {
                                    let prefix = if name.prefix().is_empty() {
                                        ""
                                    } else {
                                        name.local()
                                    };
                                    output.declare(prefix, attribute.value())?;
                                }
                                AttributeKind::Output
                                | AttributeKind::NotUnderstood
                                | AttributeKind::Unprocessed => {
                                    output.attribute(QName::from_name(&name), attribute.value())?;
                                }
                                AttributeKind::Ignored { .. } | AttributeKind::Compatibility => {}
                            }
                        }
                        Ok(())
                    });
                }
            }
            McEvent::End(_) => {
                if alternate == 0 {
                    lossless_result = lossless_result.and_then(|()| lossless.end());
                }
                output_result = output_result.and_then(|()| output.end());
            }
            McEvent::UnwrapEnd(_) => {
                if alternate == 0 {
                    lossless_result = lossless_result.and_then(|()| lossless.end());
                }
            }
            McEvent::Text(text) => {
                if alternate == 0 {
                    lossless_result = lossless_result.and_then(|()| lossless.text(text.value()));
                }
                output_result = output_result.and_then(|()| output.text(text.value()));
            }
            McEvent::CData(cdata) => {
                if alternate == 0 {
                    lossless_result = lossless_result.and_then(|()| lossless.cdata(cdata.value()));
                }
                output_result = output_result.and_then(|()| output.cdata(cdata.value()));
            }
            McEvent::Comment(comment) => {
                if alternate == 0 {
                    lossless_result =
                        lossless_result.and_then(|()| lossless.comment(comment.text()));
                }
                output_result = output_result.and_then(|()| output.comment(comment.text()));
            }
            McEvent::ProcessingInstruction(pi) => {
                if alternate == 0 {
                    lossless_result = lossless_result
                        .and_then(|()| lossless.processing_instruction(pi.target(), pi.data()));
                }
                output_result = output_result
                    .and_then(|()| output.processing_instruction(pi.target(), pi.data()));
            }
            McEvent::Ignored(ignored) => {
                if alternate == 0 {
                    lossless_result = lossless_result
                        .and_then(|()| write_fragment(&mut lossless, ignored.fragment(), false));
                }
                fragments.push(ignored.fragment().clone().into_owned());
            }
            McEvent::AlternateContentEnd(content) => {
                alternate -= 1;
                if alternate == 0 {
                    lossless_result = lossless_result
                        .and_then(|()| write_fragment(&mut lossless, content.fragment(), true));
                }
                fragments.push(content.fragment().clone().into_owned());
            }
        }
        if let McEvent::AlternateContentStart(_) = event
            && !capture
        {
            alternate += 1;
        }
        if capture {
            let fragment = reader.capture().unwrap();
            // An element captured whole reads again in its context.
            let again = Fragment::new(fragment.text(), fragment.context().clone(), limits).unwrap();
            assert_eq!(again.text(), fragment.text());
            assert_eq!(again.needed_namespaces(), fragment.needed_namespaces());
            if alternate == 0 {
                lossless_result =
                    lossless_result.and_then(|()| write_fragment(&mut lossless, &fragment, fixed));
            }
            fragments.push(fragment.into_owned());
        }
    }
    Some(Pass {
        lossless: lossless_result.and_then(|()| lossless.finish()),
        output: output_result.and_then(|()| output.finish()),
        fragments,
    })
}

/// Checks `fragment` standalone: refused only where its root element may not keep its white space mode; read as a document of its own; and processed again with Markup Compatibility, refused for no attribute in the XML namespace on its root element, the attributes that the writer adds. (Processed on its own, without the Markup Compatibility attributes of its ancestors, it may be refused for other reasons.)
fn check_standalone(fragment: &Fragment<'_>, config: &McConfig) {
    let text = match fragment.to_standalone() {
        Ok(text) => text,
        Err(error) => {
            assert_eq!(error, WriteError::SpaceConflict);
            assert_eq!(fragment.context().space(), Space::Preserve);
            return;
        }
    };
    canonical(&text);
    let source = Source::new(text.as_bytes(), &READ_BACK).unwrap();
    let mut reader = McReader::new(&source, &READ_BACK, config);
    let error = loop {
        match reader.next_event() {
            Ok(Some(_)) => {}
            Ok(None) => return,
            Err(error) => break error,
        }
    };
    if let ErrorKind::Compatibility(
        CompatibilityError::UnexpectedAttribute
        | CompatibilityError::XmlAttributeOnUnwrappedElement,
    ) = error.kind()
    {
        let mut plain = Reader::new(&source, &READ_BACK);
        let Some(Event::Start(root)) = plain.next_event().unwrap() else {
            panic!("a standalone fragment starts with its root element");
        };
        let refused = root
            .attributes()
            .find(|attribute| attribute.span().start == error.offset());
        assert!(!refused.is_some_and(|attribute| {
            attribute
                .name()
                .namespace()
                .is_some_and(|namespace| namespace.uri() == XML)
        }));
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let config = config(selector);
    let Ok(source) = Source::new(rest, &LIMITS) else {
        return;
    };
    // Capture one element whole, chosen by the length of the input.
    let capture_at = Some(rest.len() % 7);
    for (capture, start) in [
        (None, Start::AsItWas),
        (capture_at, Start::AsItWas),
        (None, Start::WithoutSpace),
    ] {
        let Some(pass) = process(&source, &LIMITS, &config, capture, start) else {
            return;
        };
        match start {
            Start::AsItWas => {
                let expected = canonicalize(&source, &LIMITS).unwrap();
                // The writer refuses one thing processing passes through unchecked: a Markup Compatibility list naming a prefix that is not declared, inside an extension element.
                match pass.lossless {
                    Ok(written) => assert_eq!(canonical(&written), expected),
                    Err(error) => assert_eq!(error, WriteError::UndeclaredPrefix),
                }
            }
            Start::WithoutSpace => match pass.lossless {
                // Without its xml:space attributes, the document differs only in white space modes, which decide nothing in Markup Compatibility.
                Ok(written) => {
                    let written = Source::new(written.as_bytes(), &READ_BACK).unwrap();
                    assert!(process(&written, &READ_BACK, &config, None, Start::AsItWas).is_some());
                }
                Err(error) => assert!(matches!(
                    error,
                    WriteError::UndeclaredPrefix | WriteError::SpaceConflict
                )),
            },
        }
        // The other passes report the same fragments.
        if start == Start::AsItWas {
            for fragment in &pass.fragments {
                check_standalone(fragment, &config);
            }
        }
        // The output document may be no document at all (an ignored root element, or the content of an unwrapped one); where it is one, processing it again changes nothing.
        if capture.is_none()
            && start == Start::AsItWas
            && let Ok(output) = pass.output
        {
            let output_source = Source::new(output.as_bytes(), &READ_BACK).unwrap();
            let again = process(&output_source, &READ_BACK, &config, None, Start::AsItWas).unwrap();
            assert_eq!(canonical(&again.output.unwrap()), canonical(&output));
        }
    }
});
