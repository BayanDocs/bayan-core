//! Round trips (AC-1): every XML part of real and synthetic documents, read and written again, means the same; every element captured and written back where it was is the same bytes; and an element moved to a hostile context keeps its meaning.

use bayan_xml::{Event, Fragment, Limits, QName, Reader, Source, Writer, canonicalize};

use crate::common::first_difference;
use crate::documents::all_parts;

const XML: &str = "http://www.w3.org/XML/1998/namespace";

fn canonical(bytes: &[u8]) -> String {
    let limits = Limits::default();
    canonicalize(&Source::new(bytes, &limits).unwrap(), &limits).unwrap()
}

/// Writes an event as it was read.
fn write_event(writer: &mut Writer, event: &Event<'_>) {
    match event {
        Event::Start(tag) => writer.start_tag(tag).unwrap(),
        Event::End(_) => writer.end().unwrap(),
        Event::Text(text) => writer.text(text.value()).unwrap(),
        Event::CData(cdata) => writer.cdata(cdata.value()).unwrap(),
        Event::Comment(comment) => writer.comment(comment.text()).unwrap(),
        Event::ProcessingInstruction(pi) => writer
            .processing_instruction(pi.target(), pi.data())
            .unwrap(),
    }
}

/// `bytes` read and written again: every element at `depth` captured and written as a fragment (none for depth 0), everything else written from its events.
fn rewrite(bytes: &[u8], depth: usize) -> String {
    let limits = Limits::default();
    let source = Source::new(bytes, &limits).unwrap();
    let mut reader = Reader::new(&source, &limits);
    let mut writer = Writer::new();
    if let Some(declaration) = source.declaration() {
        writer.declaration(declaration.standalone()).unwrap();
    }
    loop {
        let capture = match reader.next_event().unwrap() {
            None => break,
            Some(Event::Start(tag)) if tag.depth() == depth => true,
            Some(event) => {
                write_event(&mut writer, &event);
                false
            }
        };
        if capture {
            let fragment = reader.capture().unwrap();
            writer.fragment(&fragment, &limits).unwrap();
        }
    }
    writer.finish().unwrap()
}

/// The text of every element as written, by depth (the root element's at index 0), in document order.
fn elements_by_depth(bytes: &[u8]) -> Vec<Vec<String>> {
    let limits = Limits::default();
    let source = Source::new(bytes, &limits).unwrap();
    let mut reader = Reader::new(&source, &limits);
    let mut found: Vec<Vec<String>> = Vec::new();
    while let Some(event) = reader.next_event().unwrap() {
        if let Event::End(tag) = event {
            if found.len() < tag.depth() {
                found.resize(tag.depth(), Vec::new());
            }
            found[tag.depth() - 1].push(source.text()[tag.element_span()].to_owned());
        }
    }
    found
}

/// Every element at `depth`, captured with its context.
fn fragments_at(bytes: &[u8], depth: usize) -> Vec<Fragment<'static>> {
    let limits = Limits::default();
    let source = Source::new(bytes, &limits).unwrap();
    let mut reader = Reader::new(&source, &limits);
    let mut found = Vec::new();
    loop {
        let capture = match reader.next_event().unwrap() {
            None => break,
            Some(Event::Start(tag)) => tag.depth() == depth,
            Some(_) => false,
        };
        if capture {
            found.push(reader.capture().unwrap().into_owned());
        }
    }
    found
}

#[test]
fn reads_every_part_of_the_documents() {
    let parts = all_parts();
    // 5 documents with 11 to 15 XML parts each, and the samples.
    assert!(parts.len() >= 55, "only {} parts", parts.len());
    for (name, _) in &parts {
        assert!(name.ends_with(".xml") || name.ends_with(".rels"), "{name}");
    }
}

#[test]
fn every_part_of_real_documents_round_trips_through_the_writer() {
    for (name, bytes) in &all_parts() {
        let written = rewrite(bytes, 0);
        assert_eq!(
            first_difference(&canonical(bytes), &canonical(written.as_bytes())),
            None,
            "{name}"
        );
        // What the writer writes, read and written again, is the same bytes.
        assert_eq!(
            first_difference(&rewrite(written.as_bytes(), 0), &written),
            None,
            "{name}, written twice"
        );
    }
}

#[test]
fn every_element_is_written_back_byte_identical() {
    for (name, bytes) in &all_parts() {
        for (level, before) in elements_by_depth(bytes).iter().enumerate() {
            // Every element at this depth captured and written as a fragment, where it was.
            let depth = level + 1;
            let written = elements_by_depth(rewrite(bytes, depth).as_bytes());
            let after = written.get(level).map_or(&[][..], Vec::as_slice);
            assert_eq!(after.len(), before.len(), "{name}, depth {depth}");
            for (index, (before, after)) in before.iter().zip(after).enumerate() {
                assert_eq!(
                    first_difference(before, after),
                    None,
                    "{name}, depth {depth}, element {index}"
                );
            }
        }
    }
}

/// Prefixes that documents use, which the hostile context binds to other namespaces; `""` is the default namespace.
const HOSTILE_PREFIXES: &[&str] = &[
    "", "w", "r", "a", "wp", "mc", "w14", "w15", "wp14", "wps", "wpg", "v", "o", "w10", "m", "pic",
    "xsi", "dc", "dcterms", "cp", "vt", "ep", "ds",
];

#[test]
fn fragments_keep_their_meaning_in_a_new_context() {
    let limits = Limits::default();
    for (name, bytes) in &all_parts() {
        for depth in 2..=3 {
            // At most about 40 elements of each depth, spread over the part.
            let fragments = fragments_at(bytes, depth);
            let step = (fragments.len() / 40).max(1);
            for (index, fragment) in fragments.iter().enumerate().step_by(step) {
                let before = canonical(fragment.to_standalone(&limits).unwrap().as_bytes());
                for preserve in [false, true] {
                    let mut writer = Writer::new();
                    writer
                        .start(QName::new(Some("urn:hostile"), "hostile", "root"))
                        .unwrap();
                    for prefix in HOSTILE_PREFIXES {
                        writer
                            .declare(prefix, &format!("urn:hostile:{prefix}"))
                            .unwrap();
                    }
                    if preserve {
                        writer
                            .attribute(QName::new(Some(XML), "xml", "space"), "preserve")
                            .unwrap();
                    }
                    writer.fragment(fragment, &limits).unwrap();
                    writer.end().unwrap();
                    let written = writer.finish().unwrap();
                    let moved = fragments_at(written.as_bytes(), 2);
                    assert_eq!(moved.len(), 1, "{name}, depth {depth}, element {index}");
                    let after = canonical(moved[0].to_standalone(&limits).unwrap().as_bytes());
                    assert_eq!(
                        first_difference(&before, &after),
                        None,
                        "{name}, depth {depth}, element {index}, preserve {preserve}"
                    );
                }
            }
        }
    }
}
