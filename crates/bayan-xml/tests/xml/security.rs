//! Security (AC-3): document type declarations and external entities are refused wherever they appear, so entity expansion is impossible by construction (decoding never makes text longer), and the time every part of the crate takes grows linearly with its input, also for input crafted to make it grow faster.

use std::time::{Duration, Instant};

use bayan_xml::{
    Context, ErrorKind, Event, Fragment, Limits, McConfig, McReader, NamespaceName, QName, Reader,
    Source, Writer, canonicalize,
};
use proptest::prelude::*;

use crate::common::refusal;

const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";

/// Documents that declare a document type, with where the declaration starts.
const DOCUMENT_TYPES: &[(&str, usize)] = &[
    ("<!DOCTYPE a><a/>", 0),
    ("<!DOCTYPE a [<!ELEMENT a ANY>]><a/>", 0),
    // Billion laughs: entities that expand to ever more of each other.
    (
        "<?xml version=\"1.0\"?>\n<!DOCTYPE l [\n<!ENTITY l0 \"ha\">\n<!ENTITY l1 \"&l0;&l0;&l0;&l0;&l0;&l0;&l0;&l0;&l0;&l0;\">\n<!ENTITY l2 \"&l1;&l1;&l1;&l1;&l1;&l1;&l1;&l1;&l1;&l1;\">\n<!ENTITY l3 \"&l2;&l2;&l2;&l2;&l2;&l2;&l2;&l2;&l2;&l2;\">\n]>\n<l>&l3;</l>",
        22,
    ),
    // External entities, by system and by public identifier.
    (
        "<!DOCTYPE a [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><a>&x;</a>",
        0,
    ),
    (
        "<!DOCTYPE a PUBLIC \"-//Example//DTD a//EN\" \"http://example.org/a.dtd\"><a/>",
        0,
    ),
    ("<!DOCTYPE a SYSTEM \"http://example.org/a.dtd\"><a/>", 0),
    // Parameter entities.
    (
        "<!DOCTYPE a [<!ENTITY % p SYSTEM \"http://example.org/p.dtd\"> %p;]><a/>",
        0,
    ),
    // Wherever it stands.
    ("<a><!DOCTYPE b></a>", 3),
    ("<a/><!DOCTYPE b>", 4),
    ("<!-- c --><!DOCTYPE a><a/>", 10),
];

#[test]
fn refuses_document_type_declarations() {
    for (text, offset) in DOCUMENT_TYPES {
        assert_eq!(
            refusal(text),
            (ErrorKind::DoctypeNotAllowed, *offset),
            "{text}"
        );
    }
}

#[test]
fn refuses_document_type_declarations_in_every_encoding_and_every_reader() {
    let limits = Limits::default();
    for (text, offset) in DOCUMENT_TYPES {
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let source = Source::new(&utf16, &limits).unwrap();
        let mut reader = Reader::new(&source, &limits);
        let error = loop {
            match reader.next_event() {
                Ok(Some(_)) => {}
                Ok(None) => panic!("accepted a document type declaration"),
                Err(error) => break error,
            }
        };
        // Offsets count bytes of the decoded (UTF-8) text; these documents are ASCII.
        assert_eq!(
            (error.kind(), error.offset()),
            (ErrorKind::DoctypeNotAllowed, *offset),
            "{text}"
        );
        let source = Source::new(text.as_bytes(), &limits).unwrap();
        let mut reader = McReader::new(&source, &limits, &McConfig::new());
        let error = loop {
            match reader.next_event() {
                Ok(Some(_)) => {}
                Ok(None) => panic!("accepted a document type declaration"),
                Err(error) => break error,
            }
        };
        assert_eq!(error.kind(), ErrorKind::DoctypeNotAllowed, "{text}");
        let error = canonicalize(&source, &limits).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::DoctypeNotAllowed, "{text}");
    }
    let error = Fragment::new("<a><!DOCTYPE b></a>", Context::empty(), &limits).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::DoctypeNotAllowed);
}

#[test]
fn refuses_entities_without_a_document_type() {
    // Without a document type nothing can define an entity, so a reference to one is an error, and nothing is fetched.
    assert_eq!(refusal("<a>&xxe;</a>"), (ErrorKind::UndefinedEntity, 3));
    assert_eq!(refusal("<a b='&xxe;'/>"), (ErrorKind::UndefinedEntity, 6));
    // Entity declarations outside a document type are no markup at all.
    let (kind, offset) = refusal("<!ENTITY x \"y\"><a/>");
    assert_eq!(offset, 0);
    assert_ne!(kind, ErrorKind::DoctypeNotAllowed);
}

/// The length in bytes of every value the reader decodes: text, CDATA sections and attribute values.
fn decoded_length(text: &str) -> Option<usize> {
    let limits = Limits::default();
    let source = Source::new(text.as_bytes(), &limits).ok()?;
    let mut reader = Reader::new(&source, &limits);
    let mut length = 0;
    while let Some(event) = reader.next_event().ok()? {
        length += match event {
            Event::Start(tag) => tag
                .attributes()
                .map(|attribute| attribute.value().len())
                .sum(),
            Event::Text(text) => text.value().len(),
            Event::CData(cdata) => cdata.value().len(),
            Event::End(_) | Event::Comment(_) | Event::ProcessingInstruction(_) => 0,
        };
    }
    Some(length)
}

/// Pieces of content, each well-formed on its own, that decoding changes the most.
const PIECES: &[&str] = &[
    "x",
    "\u{e9}",
    "\u{10000}",
    "&lt;",
    "&gt;",
    "&amp;",
    "&apos;",
    "&quot;",
    "&#9;",
    "&#65;",
    "&#x41;",
    "&#xE9;",
    "&#x10FFFF;",
    "&#1114111;",
    "\r\n",
    "\r",
    " ",
    "<![CDATA[&lt;\r\n]]>",
    "<b/>",
    "<c d='&amp;&#9;&#xA;&lt;&#x10FFFF;' e=\"\r\n\t\"/>",
    "<!--c-->",
    "<?p d?>",
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    // Every reference stands for one character, never longer than the reference itself, and line ends and attribute values only shrink when normalized: what the reader decodes is never longer than the document.
    #[test]
    fn decoding_never_grows_text(pieces in prop::collection::vec(prop::sample::select(PIECES), 0..48)) {
        let value: String = pieces.iter().filter(|piece| !piece.contains('<')).copied().collect();
        let text = format!("<r a='{value}'>{}</r>", pieces.concat());
        let decoded = decoded_length(&text);
        prop_assert!(decoded.is_some());
        prop_assert!(decoded.unwrap_or(usize::MAX) <= text.len());
    }
}

#[test]
fn references_cannot_amplify_input() {
    // The references that decode to the most bytes for their length.
    for reference in ["&#x10FFFF;", "&#65536;", "&lt;", "&#9;"] {
        let text = format!(
            "<a b='{}'>{}</a>",
            reference.repeat(1_000),
            reference.repeat(10_000)
        );
        assert!(decoded_length(&text).unwrap() <= text.len(), "{reference}");
    }
}

/// Checks that `crafted` takes at most four times as long as `ordinary`, which does the same amount of work on ordinary input, where work that grows faster than its input would make it take eight times as long or more. Each runs seven times, alternately, so that the load of the machine affects both alike, and the shortest runs are compared, with a millisecond of slack for the timer.
fn assert_linear(what: &str, mut crafted: impl FnMut(), mut ordinary: impl FnMut()) {
    let mut slow = Duration::MAX;
    let mut fast = Duration::MAX;
    for _ in 0..7 {
        let start = Instant::now();
        crafted();
        slow = slow.min(start.elapsed());
        let start = Instant::now();
        ordinary();
        fast = fast.min(start.elapsed());
    }
    assert!(
        slow <= fast * 4 + Duration::from_millis(1),
        "{what}: {slow:?} against {fast:?}"
    );
}

/// A document with `bindings` prefixes in scope, declared 250 to an element, around `count` elements that each use one of them.
fn many_bindings(bindings: usize, count: usize) -> String {
    let mut text = String::new();
    let wrappers = bindings.div_ceil(250);
    for wrapper in 0..wrappers {
        text.push_str(&format!("<w{wrapper}"));
        for index in wrapper * 250..bindings.min((wrapper + 1) * 250) {
            text.push_str(&format!(" xmlns:p{index}='urn:p{index}'"));
        }
        text.push('>');
    }
    for index in 0..count {
        text.push_str(&format!("<p{}:x a='{index}'/>", index % bindings));
    }
    for wrapper in (0..wrappers).rev() {
        text.push_str(&format!("</w{wrapper}>"));
    }
    text
}

/// Every element named `x`, captured.
fn captured(text: &str) -> Vec<Fragment<'static>> {
    let limits = Limits::default();
    let source = Source::new(text.as_bytes(), &limits).unwrap();
    let mut reader = Reader::new(&source, &limits);
    let mut found = Vec::new();
    loop {
        let capture = match reader.next_event().unwrap() {
            None => break,
            Some(Event::Start(tag)) => tag.name().local() == "x",
            Some(_) => false,
        };
        if capture {
            found.push(reader.capture().unwrap().into_owned());
        }
    }
    found
}

/// Writes `fragments` as the content of a root element.
fn write_all(fragments: &[Fragment<'_>]) {
    let mut writer = Writer::new();
    writer.start(QName::unqualified("root")).unwrap();
    for fragment in fragments {
        writer.fragment(fragment).unwrap();
    }
    writer.end().unwrap();
    writer.finish().unwrap();
}

// Writing a fragment takes time in proportion to what it uses, not to the bindings of its context: thousands of bindings in scope make writing many small fragments no slower.
#[test]
fn stays_linear_with_many_bindings_in_scope() {
    let crowded = captured(&many_bindings(4_000, 2_000));
    let sparse = captured(&many_bindings(4, 2_000));
    assert_linear(
        "writing fragments",
        || write_all(&crowded),
        || write_all(&sparse),
    );
}

// Long start tags cost no more per attribute than short ones, to read and to write.
#[test]
fn stays_linear_with_many_attributes() {
    let tags = |count: usize, attributes: usize| {
        let mut text = String::from("<root>");
        for _ in 0..count {
            text.push_str("<a");
            for index in 0..attributes {
                text.push_str(&format!(
                    " xmlns:p{index}='urn:p{index}' p{index}:v='{index}'"
                ));
            }
            text.push_str("/>");
        }
        text.push_str("</root>");
        text
    };
    // The same number of attributes in 64 times fewer tags.
    let long = tags(32, 128);
    let short = tags(2_048, 2);
    let read = |text: &str| {
        let text = text.to_owned();
        move || {
            let limits = Limits::default();
            let source = Source::new(text.as_bytes(), &limits).unwrap();
            let mut reader = Reader::new(&source, &limits);
            while reader.next_event().unwrap().is_some() {}
        }
    };
    assert_linear("reading", read(&long), read(&short));
    // The writer takes start tags of any length: the same number of declarations and attributes in 512 times fewer tags.
    let names = |attributes: usize| -> Vec<(String, String)> {
        (0..attributes)
            .map(|index| (format!("p{index}"), format!("urn:p{index}")))
            .collect()
    };
    let write = |count: usize, names: &[(String, String)]| {
        let mut writer = Writer::new();
        writer.start(QName::unqualified("root")).unwrap();
        for _ in 0..count {
            writer.start(QName::unqualified("a")).unwrap();
            for (prefix, uri) in names {
                writer.declare(prefix, uri).unwrap();
                writer
                    .attribute(QName::new(Some(uri), prefix, "v"), "1")
                    .unwrap();
            }
            writer.end().unwrap();
        }
        writer.end().unwrap();
        writer.finish().unwrap();
    };
    let long = names(2_048);
    let short = names(4);
    assert_linear("writing", || write(4, &long), || write(2_048, &short));
}

// Capturing and writing elements deep in a document costs no more than near its root: here each uses two bindings from outside, one declared by the root and one by its parent.
#[test]
fn stays_linear_with_deep_nesting() {
    let nested = |depth: usize, count: usize| {
        let mut text = String::new();
        for level in 0..depth {
            text.push_str(&format!("<e{level} xmlns:q{level}='urn:q{level}'>"));
        }
        for index in 0..count {
            text.push_str(&format!("<q0:x n='{index}'><q{}:y/></q0:x>", depth - 1));
        }
        for level in (0..depth).rev() {
            text.push_str(&format!("</e{level}>"));
        }
        text
    };
    let deep = nested(250, 1_000);
    let shallow = nested(2, 1_000);
    let capture_all = |text: &str| {
        let text = text.to_owned();
        move || {
            let limits = Limits::default();
            let source = Source::new(text.as_bytes(), &limits).unwrap();
            let mut reader = Reader::new(&source, &limits);
            let mut writer = Writer::new();
            writer.start(QName::unqualified("root")).unwrap();
            loop {
                let capture = match reader.next_event().unwrap() {
                    None => break,
                    Some(Event::Start(tag)) => tag.name().local() == "x",
                    Some(_) => false,
                };
                if capture {
                    writer.fragment(&reader.capture().unwrap()).unwrap();
                }
            }
            writer.end().unwrap();
            writer.finish().unwrap();
        }
    };
    assert_linear(
        "capturing deep elements",
        capture_all(&deep),
        capture_all(&shallow),
    );
}

/// Processes `text` with Markup Compatibility, understanding names in no namespace.
fn process(text: &str) -> impl FnMut() {
    let text = text.to_owned();
    move || {
        let limits = Limits::default();
        let source = Source::new(text.as_bytes(), &limits).unwrap();
        let config = McConfig::new().understand(NamespaceName::None);
        let mut reader = McReader::new(&source, &limits, &config);
        while reader.next_event().unwrap().is_some() {}
    }
}

// Markup Compatibility lists of the longest length allowed cost no more per entry than short ones.
#[test]
fn stays_linear_with_long_compatibility_lists() {
    let lists = |count: usize, tokens: usize| {
        let mut text = format!("<root xmlns:mc='{MC}'");
        for index in 0..255 {
            text.push_str(&format!(" xmlns:i{index}='urn:i{index}'"));
        }
        text.push('>');
        let list: Vec<String> = (0..tokens).map(|index| format!("i{index}")).collect();
        let pairs: Vec<String> = (0..tokens).map(|index| format!("i{index}:*")).collect();
        for _ in 0..count {
            text.push_str(&format!(
                "<a mc:Ignorable='{}' mc:ProcessContent='{}'><i0:b/></a>",
                list.join(" "),
                pairs.join(" ")
            ));
        }
        text.push_str("</root>");
        text
    };
    // The same number of entries in 16 times fewer lists.
    assert_linear(
        "long lists",
        process(&lists(64, 240)),
        process(&lists(1_024, 15)),
    );
}

// Alternate content nested as deep as the limits allow takes time in proportion to its size.
#[test]
fn stays_linear_with_nested_alternate_content() {
    let nested = |depth: usize, repeat: usize| {
        let mut inner = String::from("<x/>");
        for level in 0..depth {
            inner = format!(
                "<mc:AlternateContent xmlns:n{level}='urn:n{level}'><mc:Choice Requires='n{level}'><y/></mc:Choice><mc:Fallback>{inner}</mc:Fallback></mc:AlternateContent>"
            );
        }
        format!("<root xmlns:mc='{MC}'>{}</root>", inner.repeat(repeat))
    };
    // About the same size: one alternate content nested 120 deep, or 15 nested 8 deep.
    assert_linear(
        "nested alternate content",
        process(&nested(120, 1)),
        process(&nested(8, 15)),
    );
}

/// A document whose root element binds the ignorable prefix `i` to the namespace named `used`, which `count` empty elements use, and `u` to the one named `unused`, which nothing uses.
fn named(used: &str, unused: &str, count: usize) -> String {
    format!(
        "<root xmlns:mc='{MC}' xmlns:i='{used}' xmlns:u='{unused}' mc:Ignorable='i'>{}</root>",
        "<i:x/>".repeat(count)
    )
}

/// Some work on a document, within limits.
type Work = fn(&str, &Limits);

/// Processes `text` with Markup Compatibility, which keeps every ignored element whole.
fn ignore_all(text: &str, limits: &Limits) {
    let source = Source::new(text.as_bytes(), limits).unwrap();
    let config = McConfig::new().understand(NamespaceName::None);
    let mut reader = McReader::new(&source, limits, &config);
    while reader.next_event().unwrap().is_some() {}
}

/// Reads `text` and writes every start tag again as it was.
fn rewrite_all(text: &str, limits: &Limits) {
    let source = Source::new(text.as_bytes(), limits).unwrap();
    let mut reader = Reader::new(&source, limits);
    let mut writer = Writer::new();
    while let Some(event) = reader.next_event().unwrap() {
        match event {
            Event::Start(tag) => writer.start_tag(&tag).unwrap(),
            Event::End(_) => writer.end().unwrap(),
            _ => {}
        }
    }
    writer.finish().unwrap();
}

/// Reads `text`, writes its root element's start tag again, and captures every child and writes it back as a fragment.
fn capture_all(text: &str, limits: &Limits) {
    let source = Source::new(text.as_bytes(), limits).unwrap();
    let mut reader = Reader::new(&source, limits);
    let mut writer = Writer::new();
    loop {
        let capture = match reader.next_event().unwrap() {
            None => break,
            Some(Event::Start(tag)) if tag.depth() == 1 => {
                writer.start_tag(&tag).unwrap();
                false
            }
            Some(Event::Start(_)) => true,
            Some(Event::End(_)) => {
                writer.end().unwrap();
                false
            }
            Some(_) => false,
        };
        if capture {
            writer.fragment(&reader.capture().unwrap()).unwrap();
        }
    }
    writer.finish().unwrap();
}

/// The canonical form of `text`.
fn canonical_of(text: &str, limits: &Limits) {
    let source = Source::new(text.as_bytes(), limits).unwrap();
    canonicalize(&source, limits).unwrap();
}

// The length of a namespace name costs nothing per element or fragment that uses it: with 5,000 elements in a namespace whose name is as long as the limit allows, at the default limit and at one raised to 256 KiB, Markup Compatibility (which keeps every ignored element), writing start tags again, capturing and writing fragments, and the canonical form take as long as when the elements use a short name and the long one is only declared.
#[test]
fn stays_linear_with_long_namespace_names() {
    for length in [Limits::default().max_namespace_name_length, 256 << 10] {
        let limits = Limits {
            max_namespace_name_length: length,
            ..Limits::default()
        };
        let long = format!("urn:{}", "n".repeat(length - 4));
        let crafted = named(&long, "urn:u", 5_000);
        let ordinary = named("urn:i", &long, 5_000);
        let runs: [(&str, Work); 4] = [
            ("Markup Compatibility", ignore_all),
            ("writing start tags again", rewrite_all),
            ("capturing and writing fragments", capture_all),
            ("the canonical form", canonical_of),
        ];
        for (what, run) in runs {
            assert_linear(
                &format!("{what}, namespace names of {length} bytes"),
                || run(&crafted, &limits),
                || run(&ordinary, &limits),
            );
        }
    }
}

// Ignoring many elements, each kept whole, takes time in proportion to their number.
#[test]
fn stays_linear_with_many_ignored_elements() {
    let ignored = |count: usize| {
        format!(
            "<root xmlns:mc='{MC}' xmlns:i='urn:i' mc:Ignorable='i'>{}</root>",
            "<i:x><y/></i:x>".repeat(count)
        )
    };
    // One document with eight times as many, against eight documents.
    let few_text = ignored(2_000);
    let mut few = process(&few_text);
    assert_linear("ignored elements", process(&ignored(16_000)), || {
        for _ in 0..8 {
            few();
        }
    });
}
