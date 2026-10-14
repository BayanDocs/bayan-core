//! bayan-xml's Markup Compatibility processing (`McReader`) on arbitrary bytes, with a configuration that the first byte chooses: which of six namespaces and names without one are understood, and whether Office Open XML's extension lists and one more extension element count. For every document it processes: the lossless events, written back, give a document with the input's canonical form; every fragment it reports (ignored elements, whole alternate content) stands on its own with the declarations it records; an element captured from the processor reads again in its context; and the output document, processed again, is unchanged.

#![no_main]

use bayan_xml::{
    AttributeKind, Fragment, Limits, McConfig, McEvent, McReader, NamespaceName, QName, Source,
    WriteError, Writer, canonicalize,
};
use libfuzzer_sys::fuzz_target;

/// The limits documents meet.
const LIMITS: Limits = Limits::DEFAULT;

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

fn canonical(text: &str) -> String {
    let source = Source::new(text.as_bytes(), &LIMITS).unwrap();
    canonicalize(&source, &LIMITS).unwrap()
}

/// What one pass over the document gives: the lossless writer's and the output writer's results, and the fragments reported.
struct Pass {
    lossless: Result<String, WriteError>,
    output: Result<String, WriteError>,
    fragments: Vec<Fragment<'static>>,
}

/// Processes `source` to its end, or returns `None` at an error. `capture_at` names the start tag (counted from 0) whose element is captured whole instead of processed.
fn process(source: &Source<'_>, config: &McConfig, capture_at: Option<usize>) -> Option<Pass> {
    let mut reader = McReader::new(source, &LIMITS, config);
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
        match &event {
            McEvent::Start(tag) | McEvent::UnwrapStart(tag) | McEvent::AlternateContentStart(tag) => {
                capture = capture_at == Some(starts);
                starts += 1;
                if !capture && alternate == 0 && !matches!(event, McEvent::AlternateContentStart(_)) {
                    lossless_result = lossless_result.and_then(|()| lossless.start_tag(tag.tag()));
                }
                if !capture && matches!(event, McEvent::Start(_)) {
                    output_result = output_result.and_then(|()| {
                        output.start(QName::from_name(&tag.name()))?;
                        for (attribute, kind) in tag.attributes() {
                            let name = attribute.name();
                            match kind {
                                AttributeKind::NamespaceDeclaration => {
                                    let prefix = if name.prefix().is_empty() { "" } else { name.local() };
                                    output.declare(prefix, attribute.value())?;
                                }
                                AttributeKind::Output | AttributeKind::NotUnderstood | AttributeKind::Unprocessed => {
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
                    lossless_result = lossless_result.and_then(|()| lossless.comment(comment.text()));
                }
                output_result = output_result.and_then(|()| output.comment(comment.text()));
            }
            McEvent::ProcessingInstruction(pi) => {
                if alternate == 0 {
                    lossless_result = lossless_result
                        .and_then(|()| lossless.processing_instruction(pi.target(), pi.data()));
                }
                output_result =
                    output_result.and_then(|()| output.processing_instruction(pi.target(), pi.data()));
            }
            McEvent::Ignored(ignored) => {
                if alternate == 0 {
                    lossless_result = lossless_result.and_then(|()| lossless.fragment(ignored.fragment()));
                }
                fragments.push(ignored.fragment().clone().into_owned());
            }
            McEvent::AlternateContentEnd(content) => {
                alternate -= 1;
                if alternate == 0 {
                    lossless_result = lossless_result.and_then(|()| lossless.fragment(content.fragment()));
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
            let again = Fragment::new(fragment.text(), fragment.context().clone(), &LIMITS).unwrap();
            assert_eq!(again.text(), fragment.text());
            assert_eq!(again.needed_namespaces(), fragment.needed_namespaces());
            if alternate == 0 {
                lossless_result = lossless_result.and_then(|()| lossless.fragment(&fragment));
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
    for capture in [None, capture_at] {
        let Some(pass) = process(&source, &config, capture) else {
            return;
        };
        let expected = canonicalize(&source, &LIMITS).unwrap();
        // The writer refuses one thing processing passes through unchecked: a Markup Compatibility list naming a prefix that is not declared, inside an extension element.
        match pass.lossless {
            Ok(written) => assert_eq!(canonical(&written), expected),
            Err(error) => assert_eq!(error, WriteError::UndeclaredPrefix),
        }
        for fragment in &pass.fragments {
            canonical(&fragment.to_standalone());
        }
        // The output document may be no document at all (an ignored root element, or the content of an unwrapped one); where it is one, processing it again changes nothing.
        if capture.is_none()
            && let Ok(output) = pass.output
        {
            let output_source = Source::new(output.as_bytes(), &LIMITS).unwrap();
            let again = process(&output_source, &config, None).unwrap();
            assert_eq!(canonical(&again.output.unwrap()), canonical(&output));
        }
    }
});
