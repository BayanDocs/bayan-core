//! Reading well-formed XML, and refusing each kind of malformed XML with its own error at the right place.

use bayan_xml::{ErrorKind, Event, Limits, Reader, Source, Space};

use crate::common::{accepts, describe, describe_with, refusal, refusal_with};

#[test]
fn reads_elements_attributes_and_text() {
    assert_eq!(
        describe("<a b=\"1\" c='2'><d/>text<e>more</e></a>").unwrap(),
        [
            "<a b=\"1\" c=\"2\">",
            "<d/",
            "</d>",
            "text \"text\"",
            "<e>",
            "text \"more\"",
            "</e>",
            "</a>",
        ]
    );
}

#[test]
fn reports_the_span_of_every_event() {
    let text =
        "<?xml version=\"1.0\"?>\r\n<!--c--><a x=\"1\">t&amp;u<b/><![CDATA[d]]><?p q?></a>\n";
    let source = Source::new(text.as_bytes(), &Limits::default()).unwrap();
    let mut reader = Reader::new(&source, &Limits::default());
    let mut spans = Vec::new();
    while let Some(event) = reader.next_event().unwrap() {
        spans.push(match event {
            Event::Start(tag) => tag.span(),
            Event::End(tag) => tag.span(),
            Event::Text(text) => text.span(),
            Event::CData(cdata) => cdata.span(),
            Event::Comment(comment) => comment.span(),
            Event::ProcessingInstruction(pi) => pi.span(),
        });
    }
    let pieces: Vec<&str> = spans.iter().map(|span| &text[span.clone()]).collect();
    assert_eq!(
        pieces,
        [
            "<!--c-->",
            "<a x=\"1\">",
            "t&amp;u",
            "<b/>",
            // The end of an empty-element tag is the empty span at its end.
            "",
            "<![CDATA[d]]>",
            "<?p q?>",
            "</a>",
        ]
    );
    assert_eq!(spans[4], 51..51);
}

#[test]
fn reports_whole_elements_and_their_depth() {
    let text = "<a><b>x</b><c/></a>";
    let source = Source::new(text.as_bytes(), &Limits::default()).unwrap();
    let mut reader = Reader::new(&source, &Limits::default());
    let mut ends = Vec::new();
    while let Some(event) = reader.next_event().unwrap() {
        match event {
            Event::Start(tag) => assert_eq!(tag.depth(), reader_depth_of(tag.name().local())),
            Event::End(tag) => ends.push((
                tag.name().local().to_owned(),
                tag.element_span(),
                tag.depth(),
            )),
            _ => {}
        }
    }
    assert_eq!(
        ends,
        [
            ("b".to_owned(), 3..11, 2),
            ("c".to_owned(), 11..15, 2),
            ("a".to_owned(), 0..19, 1),
        ]
    );
}

fn reader_depth_of(name: &str) -> usize {
    if name == "a" { 1 } else { 2 }
}

#[test]
fn resolves_references_in_text_and_attributes() {
    assert_eq!(
        describe("<a v=\"&lt;&amp;&gt;&quot;&apos;&#65;&#x42;&#x1F600;\">&lt;&amp;&gt;&quot;&apos;&#65;&#x42;&#x1f600;&#0000000000067;</a>").unwrap(),
        [
            "<a v=\"<&>\\\"'AB\u{1F600}\">",
            "text \"<&>\\\"'AB\u{1F600}C\"",
            "</a>",
        ]
    );
}

#[test]
fn merges_text_that_references_split() {
    let text = "<a>one &amp; two &#x26; three</a>";
    let source = Source::new(text.as_bytes(), &Limits::default()).unwrap();
    let mut reader = Reader::new(&source, &Limits::default());
    reader.next_event().unwrap();
    let Some(Event::Text(value)) = reader.next_event().unwrap() else {
        panic!("expected one text event");
    };
    assert_eq!(value.value(), "one & two & three");
    assert_eq!(value.raw(), "one &amp; two &#x26; three");
    assert_eq!(value.span(), 3..29);
    assert!(matches!(reader.next_event().unwrap(), Some(Event::End(_))));
}

#[test]
fn normalizes_line_ends_in_text() {
    assert_eq!(
        describe("<a>1\r\n2\r3\n4&#xD;5&#13;&#xA;6</a>").unwrap(),
        ["<a>", "text \"1\\n2\\n3\\n4\\r5\\r\\n6\"", "</a>"]
    );
    assert_eq!(
        describe("<a><![CDATA[1\r\n2\r3]]></a>").unwrap(),
        ["<a>", "cdata \"1\\n2\\n3\"", "</a>"]
    );
}

#[test]
fn normalizes_white_space_in_attribute_values() {
    // Each literal tab, line feed and carriage return becomes a space, a CR LF pair one space; references keep their characters (XML 1.0 §3.3.3).
    assert_eq!(
        describe("<a v=\"1\t2\n3\r\n4\r5  6&#x9;&#xA;&#xD;7\"/>").unwrap(),
        ["<a v=\"1 2 3 4 5  6\\t\\n\\r7\"/", "</a>"]
    );
}

#[test]
fn keeps_the_raw_attribute_value() {
    let text = "<a v=\"x &amp;\ty\"/>";
    let source = Source::new(text.as_bytes(), &Limits::default()).unwrap();
    let mut reader = Reader::new(&source, &Limits::default());
    let Some(Event::Start(tag)) = reader.next_event().unwrap() else {
        panic!("expected a start tag");
    };
    let attribute = tag.attributes().next().unwrap();
    assert_eq!(attribute.value(), "x & y");
    assert_eq!(attribute.raw_value(), "x &amp;\ty");
    assert_eq!(attribute.span(), 3..16);
    assert_eq!(attribute.value_span(), 6..15);
    assert!(!attribute.is_namespace_declaration());
}

#[test]
fn reads_comments_cdata_and_processing_instructions() {
    assert_eq!(
        describe("<!-- before --><?before x?><a><!--in--><![CDATA[<x>&amp;]]><?t  data ?><?u?><!----><!---> - -->x</a><!--after--><?after?>").unwrap(),
        [
            "comment \" before \"",
            "pi before \"x\"",
            "<a>",
            "comment \"in\"",
            "cdata \"<x>&amp;\"",
            "pi t \"data \"",
            "pi u \"\"",
            "comment \"\"",
            "comment \"-> - \"",
            "text \"x\"",
            "</a>",
            "comment \"after\"",
            "pi after \"\"",
        ]
    );
}

#[test]
fn skips_white_space_outside_the_root() {
    assert_eq!(
        describe(" \r\n\t<a> </a>\n \n").unwrap(),
        ["<a>", "text \" \"", "</a>"]
    );
}

#[test]
fn reports_the_white_space_mode() {
    let text = "<a><b xml:space=\"preserve\"><c>x</c><d xml:space=\"default\">y</d><e xml:space=\"other\">z</e></b></a>";
    let source = Source::new(text.as_bytes(), &Limits::default()).unwrap();
    let mut reader = Reader::new(&source, &Limits::default());
    let mut modes = Vec::new();
    while let Some(event) = reader.next_event().unwrap() {
        match event {
            Event::Start(tag) => modes.push((tag.name().local().to_owned(), tag.space())),
            Event::Text(text) => modes.push((format!("text {}", text.value()), text.space())),
            _ => {}
        }
    }
    assert_eq!(
        modes,
        [
            ("a".to_owned(), Space::Default),
            ("b".to_owned(), Space::Preserve),
            ("c".to_owned(), Space::Preserve),
            ("text x".to_owned(), Space::Preserve),
            ("d".to_owned(), Space::Default),
            ("text y".to_owned(), Space::Default),
            // A value other than "default" and "preserve" leaves the mode as it was.
            ("e".to_owned(), Space::Preserve),
            ("text z".to_owned(), Space::Preserve),
        ]
    );
}

#[test]
fn skips_the_xml_declaration() {
    assert_eq!(
        describe("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<a/>").unwrap(),
        ["<a/", "</a>"]
    );
}

#[test]
fn refuses_content_outside_the_root() {
    assert_eq!(refusal("<a/>x"), (ErrorKind::ContentOutsideRoot, 4));
    assert_eq!(refusal("x<a/>"), (ErrorKind::ContentOutsideRoot, 0));
    assert_eq!(refusal("<a/><b/>"), (ErrorKind::ContentOutsideRoot, 4));
    assert_eq!(
        refusal("<a/><![CDATA[x]]>"),
        (ErrorKind::ContentOutsideRoot, 4)
    );
    assert_eq!(
        refusal("<![CDATA[x]]><a/>"),
        (ErrorKind::ContentOutsideRoot, 0)
    );
    // A reference is content, even one to white space.
    assert_eq!(refusal("<a/>&#32;"), (ErrorKind::ContentOutsideRoot, 4));
    assert_eq!(refusal("&amp;<a/>"), (ErrorKind::ContentOutsideRoot, 0));
}

#[test]
fn refuses_a_document_without_a_root() {
    for text in ["", " \n ", "<!--c-->", "<?p?>"] {
        assert_eq!(
            refusal(text),
            (ErrorKind::NoRootElement, text.len()),
            "{text:?}"
        );
    }
}

#[test]
fn refuses_elements_left_open() {
    assert_eq!(refusal("<a><b>"), (ErrorKind::UnexpectedEnd, 6));
    assert_eq!(refusal("<a>text"), (ErrorKind::UnexpectedEnd, 7));
}

#[test]
fn refuses_markup_cut_short() {
    for text in [
        "<a><",
        "<a></a",
        "<a><b",
        "<a b=\"1",
        "<a><!--x",
        "<a><![CDATA[x",
        "<a><?p",
    ] {
        assert_eq!(refusal(text).0, ErrorKind::UnexpectedEnd, "{text:?}");
    }
    assert_eq!(refusal("<a><!---></a>").0, ErrorKind::UnexpectedEnd);
    // A document type declaration is refused as one even when it is cut short.
    assert_eq!(refusal("<!DOCTYPE"), (ErrorKind::DoctypeNotAllowed, 0));
}

#[test]
fn refuses_end_tags_that_do_not_match() {
    assert_eq!(refusal("<a></b>"), (ErrorKind::MismatchedEndTag, 3));
    assert_eq!(refusal("<a><b></a></b>"), (ErrorKind::MismatchedEndTag, 6));
    assert_eq!(
        refusal("<a:b xmlns:a=\"u\" xmlns:c=\"u\"></c:b>").0,
        ErrorKind::MismatchedEndTag
    );
    assert_eq!(refusal("<a/></a>"), (ErrorKind::MismatchedEndTag, 4));
    // White space after the name is allowed in an end tag; nothing else is.
    assert!(accepts("<a></a \t\r\n>"));
    assert_eq!(refusal("<a></a b=\"c\">"), (ErrorKind::MalformedTag, 3));
    assert_eq!(refusal("<a></ a>").0, ErrorKind::InvalidName);
}

#[test]
fn refuses_malformed_tags() {
    for (text, offset) in [
        ("<a b=\"1\"c=\"2\"/>", 8),
        ("<a b/>", 4),
        ("<a b=1/>", 5),
        ("<a b = c/>", 7),
        ("<a b=\"1\" / >", 9),
        ("<a/ >", 2),
        ("<a b=\"1\"/x>", 8),
    ] {
        assert_eq!(refusal(text), (ErrorKind::MalformedTag, offset), "{text}");
    }
    assert!(accepts("<a\n\tb\r\n=\n'1'\t/>"));
}

#[test]
fn refuses_invalid_names() {
    for (text, offset) in [
        ("< a/>", 1),
        ("<1a/>", 1),
        ("<-a/>", 1),
        ("<a:b:c/>", 1),
        ("<:a/>", 1),
        ("<a:/>", 1),
        ("<a 1b=\"x\"/>", 3),
        ("<a b:c:d=\"x\"/>", 3),
        ("<a :b=\"x\"/>", 3),
        // An attribute name that is missing is not a name either.
        ("<a ='1'/>", 3),
        ("<a b=\"1\" @=\"2\"/>", 9),
    ] {
        assert_eq!(refusal(text), (ErrorKind::InvalidName, offset), "{text}");
    }
}

#[test]
fn refuses_less_than_in_attribute_values() {
    assert_eq!(
        refusal("<a b=\"x<y\"/>"),
        (ErrorKind::LessThanInAttribute, 7)
    );
    assert!(accepts("<a b=\"x&lt;y>z\"/>"));
}

#[test]
fn refuses_duplicate_attributes() {
    assert_eq!(
        refusal("<a b=\"1\" b=\"2\"/>"),
        (ErrorKind::DuplicateAttribute, 9)
    );
    // Two prefixes for one namespace make the same expanded name (Namespaces in XML §6.3).
    assert_eq!(
        refusal("<a x:b=\"1\" y:b=\"2\" xmlns:x=\"urn:u\" xmlns:y=\"urn:u\"/>"),
        (ErrorKind::DuplicateAttribute, 11)
    );
    assert_eq!(
        refusal("<a xmlns:x=\"urn:u\" xmlns:x=\"urn:v\"/>"),
        (ErrorKind::DuplicateAttribute, 19)
    );
    assert_eq!(
        refusal("<a xmlns=\"urn:u\" xmlns=\"urn:v\"/>").0,
        ErrorKind::DuplicateAttribute
    );
    // The same local name in different namespaces, or with and without one, is fine.
    assert!(accepts(
        "<a b=\"1\" x:b=\"2\" y:b=\"3\" xmlns:x=\"urn:u\" xmlns:y=\"urn:v\"/>"
    ));
}

#[test]
fn refuses_cdata_end_in_text() {
    assert_eq!(refusal("<a>]]></a>"), (ErrorKind::CDataEndInText, 3));
    assert_eq!(
        refusal("<a>x &amp; y]]>z</a>"),
        (ErrorKind::CDataEndInText, 12)
    );
    assert!(accepts("<a>]]&gt; ] ]> ]]</a>"));
}

#[test]
fn refuses_malformed_comments() {
    assert_eq!(
        refusal("<a><!--x--y--></a>"),
        (ErrorKind::MalformedComment, 3)
    );
    assert_eq!(
        refusal("<a><!--x---></a>"),
        (ErrorKind::MalformedComment, 3)
    );
    assert_eq!(refusal("<!-----><a/>"), (ErrorKind::MalformedComment, 0));
}

#[test]
fn refuses_misplaced_xml_declarations() {
    assert_eq!(
        refusal(" <?xml version=\"1.0\"?><a/>"),
        (ErrorKind::MisplacedDeclaration, 1)
    );
    assert_eq!(
        refusal("<a><?xml version=\"1.0\"?></a>"),
        (ErrorKind::MisplacedDeclaration, 3)
    );
    assert_eq!(
        refusal("<a/><?xml version=\"1.0\"?>"),
        (ErrorKind::MisplacedDeclaration, 4)
    );
    for target in ["xml", "XML", "xMl"] {
        let text = format!("<a><?{target} x?></a>");
        assert_eq!(
            refusal(&text),
            (ErrorKind::MisplacedDeclaration, 3),
            "{target}"
        );
    }
    // Targets that merely start with "xml" are allowed.
    assert!(accepts("<?xml-stylesheet href=\"s\"?><a><?xmlfoo?></a>"));
}

#[test]
fn refuses_malformed_processing_instructions() {
    for text in [
        "<a><??></a>",
        "<a><?t?x?></a>",
        "<a><?a:b?></a>",
        "<a><?1?></a>",
        "<a><? t?></a>",
    ] {
        assert_eq!(
            refusal(text),
            (ErrorKind::MalformedProcessingInstruction, 3),
            "{text}"
        );
    }
}

#[test]
fn refuses_markup_that_xml_does_not_have() {
    assert_eq!(
        refusal("<a><!ELEMENT x ANY></a>"),
        (ErrorKind::MalformedMarkup, 3)
    );
    assert_eq!(refusal("<a><!x></a>"), (ErrorKind::MalformedMarkup, 3));
    assert_eq!(
        refusal("<a><![CDATA x]]></a>").0,
        ErrorKind::MalformedMarkup
    );
}

#[test]
fn refuses_malformed_references() {
    for (text, offset) in [
        ("<a>&</a>", 3),
        ("<a>x & y</a>", 5),
        ("<a>&;</a>", 3),
        ("<a>&#;</a>", 3),
        ("<a>&#x;</a>", 3),
        ("<a>&#xG;</a>", 3),
        ("<a>&#12a;</a>", 3),
        ("<a>&# 65;</a>", 3),
        ("<a>&1;</a>", 3),
        ("<a b=\"&\"/>", 6),
        ("<a b=\"&amp\"/>", 6),
        ("<a b=\"&#x;\"/>", 6),
    ] {
        assert_eq!(
            refusal(text),
            (ErrorKind::MalformedReference, offset),
            "{text}"
        );
    }
}

#[test]
fn refuses_references_to_characters_xml_does_not_allow() {
    for text in [
        "<a>&#0;</a>",
        "<a>&#x1;</a>",
        "<a>&#x1F;</a>",
        "<a>&#xD800;</a>",
        "<a>&#xDFFF;</a>",
        "<a>&#xFFFE;</a>",
        "<a>&#xFFFF;</a>",
        "<a>&#x110000;</a>",
        "<a>&#99999999999999999999999;</a>",
        "<a>&#x10000000000000000000041;</a>",
        "<a b=\"&#8;\"/>",
    ] {
        assert_eq!(
            refusal(text).0,
            ErrorKind::InvalidCharacterReference,
            "{text}"
        );
    }
}

#[test]
fn refuses_entities_that_are_not_predefined() {
    assert_eq!(refusal("<a>&nbsp;</a>"), (ErrorKind::UndefinedEntity, 3));
    assert_eq!(
        refusal("<a b=\"&copy;\"/>"),
        (ErrorKind::UndefinedEntity, 6)
    );
    // Entity names are compared exactly.
    assert_eq!(refusal("<a>&AMP;</a>"), (ErrorKind::UndefinedEntity, 3));
}

#[test]
fn refuses_a_run_of_text_at_its_first_problem() {
    // Each run has two problems, and is refused at the first, because the reader checks each piece of a run as soon as it reads it; a reader that collected the pieces before checking them would report the later one (finding M3 of the architecture review of 2026-10-27).
    for (text, kind, offset) in [
        // `]]>` after an undefined entity,
        ("<a>&nbsp;]]></a>", ErrorKind::UndefinedEntity, 3),
        // a reference cut short by the end of the text after one,
        ("<a>&nbsp;x &", ErrorKind::UndefinedEntity, 3),
        // and one cut short by markup after a reference to a character XML does not allow.
        (
            "<a>&lt;&#0;x &amp</a>",
            ErrorKind::InvalidCharacterReference,
            7,
        ),
    ] {
        assert_eq!(refusal(text), (kind, offset), "{text}");
    }
}

#[test]
fn limits_the_depth() {
    let limits = Limits {
        max_depth: 3,
        ..Limits::default()
    };
    assert!(describe_with("<a><b><c/></b></a>", &limits).is_ok());
    assert_eq!(
        refusal_with("<a><b><c><d/></c></b></a>", &limits),
        (ErrorKind::TooDeep, 9)
    );
}

#[test]
fn limits_the_attributes_of_an_element() {
    let limits = Limits {
        max_attributes: 3,
        ..Limits::default()
    };
    assert!(describe_with("<a b=\"1\" c=\"2\" xmlns:d=\"urn:d\"/>", &limits).is_ok());
    assert_eq!(
        refusal_with("<a b=\"1\" c=\"2\" xmlns:d=\"urn:d\" e=\"3\"/>", &limits),
        (ErrorKind::TooManyAttributes, 31)
    );
}

#[test]
fn limits_the_length_of_names() {
    // The limit counts names as written, with their prefix.
    let limits = Limits {
        max_name_length: 7,
        ..Limits::default()
    };
    assert!(
        describe_with(
            "<w:pp xmlns:w=\"urn:w\" w:ab=\"1\"><?abcd?>&#65;&amp;</w:pp>",
            &limits
        )
        .is_ok()
    );
    for (text, offset) in [
        ("<abcdefgh/>", 1),
        ("<w:pppppp xmlns:w=\"urn:w\"/>", 1),
        ("<a bcdefghi=\"1\"/>", 3),
        // A declaration's name, xmlns:prefix, is a name too.
        ("<a xmlns:abcdef=\"urn:x\"/>", 3),
        ("<a><?abcdefgh?></a>", 3),
        ("<a>&abcdefgh;</a>", 3),
        ("<a b=\"&abcdefgh;\"/>", 6),
    ] {
        assert_eq!(
            refusal_with(text, &limits),
            (ErrorKind::NameTooLong, offset),
            "{text}"
        );
    }
}

#[test]
fn errors_never_contain_document_text() {
    for text in [
        "<SECRET></b>",
        "<a SECRET=\"1\" SECRET=\"2\"/>",
        "<a>&SECRET;</a>",
        "<SECRET:a/>",
        "<a SECRET=\"x<\"/>",
        "<a/>SECRET",
        "<a><?SECRET:x?></a>",
    ] {
        let source = Source::new(text.as_bytes(), &Limits::default()).unwrap();
        let mut reader = Reader::new(&source, &Limits::default());
        let error = loop {
            match reader.next_event() {
                Ok(Some(_)) => {}
                Ok(None) => panic!("accepted a document that must be refused"),
                Err(error) => break error,
            }
        };
        assert!(!error.to_string().contains("SECRET"));
        assert!(!format!("{error:?}").contains("SECRET"));
    }
}

#[test]
fn reading_after_an_error_or_the_end_stays_at_the_end() {
    let source = Source::new(b"<a></b>", &Limits::default()).unwrap();
    let mut reader = Reader::new(&source, &Limits::default());
    assert!(reader.next_event().is_ok());
    assert!(reader.next_event().is_err());
    assert!(matches!(reader.next_event(), Ok(None)));
    let source = Source::new(b"<a/>", &Limits::default()).unwrap();
    let mut reader = Reader::new(&source, &Limits::default());
    while reader.next_event().unwrap().is_some() {}
    assert!(matches!(reader.next_event(), Ok(None)));
}

#[test]
fn refuses_a_second_byte_order_mark() {
    // quick-xml drops a byte order mark at the start of its input without counting it; after the one that Source removes, a second one is a character before the root element.
    let error = Source::new(b"\xEF\xBB\xBF\xEF\xBB\xBF<a/>", &Limits::default())
        .and_then(|source| {
            let mut reader = Reader::new(&source, &Limits::default());
            reader.next_event().map(|_| ())
        })
        .unwrap_err();
    assert_eq!(
        (error.kind(), error.offset()),
        (ErrorKind::ContentOutsideRoot, 0)
    );
    // Inside the root it is an ordinary character.
    assert_eq!(
        describe("<a>\u{FEFF}</a>").unwrap(),
        ["<a>", "text \"\\u{feff}\"", "</a>"]
    );
}
