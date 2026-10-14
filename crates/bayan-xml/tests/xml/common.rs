//! Helpers shared by the integration tests.

use bayan_xml::{Error, ErrorKind, Event, Limits, Reader, Source};

/// A compact description of every event of `text`, for comparisons: `<qname a="value">` (attributes as written, values normalized, namespace declarations included), `</qname>`, `text "…"`, `cdata "…"`, `comment "…"`, `pi target "data"`.
pub fn describe(text: &str) -> Result<Vec<String>, (ErrorKind, usize)> {
    describe_with(text, &Limits::default())
}

/// [`describe`] with other limits.
pub fn describe_with(text: &str, limits: &Limits) -> Result<Vec<String>, (ErrorKind, usize)> {
    let fail = |error: Error| (error.kind(), error.offset());
    let source = Source::new(text.as_bytes(), limits).map_err(fail)?;
    let mut reader = Reader::new(&source, limits);
    let mut events = Vec::new();
    while let Some(event) = reader.next_event().map_err(fail)? {
        events.push(match event {
            Event::Start(tag) => {
                let mut description =
                    format!("<{}", qualified(tag.name().prefix(), tag.name().local()));
                for attribute in tag.attributes() {
                    description.push_str(&format!(
                        " {}={:?}",
                        qualified(attribute.name().prefix(), attribute.name().local()),
                        attribute.value()
                    ));
                }
                description.push(if tag.is_empty_element() { '/' } else { '>' });
                description
            }
            Event::End(tag) => format!("</{}>", qualified(tag.name().prefix(), tag.name().local())),
            Event::Text(text) => format!("text {:?}", text.value()),
            Event::CData(cdata) => format!("cdata {:?}", cdata.value()),
            Event::Comment(comment) => format!("comment {:?}", comment.text()),
            Event::ProcessingInstruction(pi) => format!("pi {} {:?}", pi.target(), pi.data()),
        });
    }
    Ok(events)
}

fn qualified(prefix: &str, local: &str) -> String {
    if prefix.is_empty() {
        local.to_owned()
    } else {
        format!("{prefix}:{local}")
    }
}

/// The error `text` is refused with, as kind and offset; panics if it is accepted.
pub fn refusal(text: &str) -> (ErrorKind, usize) {
    refusal_with(text, &Limits::default())
}

/// [`refusal`] with other limits.
pub fn refusal_with(text: &str, limits: &Limits) -> (ErrorKind, usize) {
    match describe_with(text, limits) {
        Ok(_) => panic!("accepted a document that must be refused"),
        Err(error) => error,
    }
}

/// Whether `text` is read without error.
pub fn accepts(text: &str) -> bool {
    describe(text).is_ok()
}

/// The offset of the first byte where `a` and `b` differ, or `None` if they are equal. Tests report this rather than the texts, so that a failure prints no document content.
pub fn first_difference(a: &str, b: &str) -> Option<usize> {
    let common = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    (common < a.len().max(b.len())).then_some(common)
}
