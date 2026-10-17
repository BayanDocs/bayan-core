//! The writer: exact, deterministic output; namespace declarations where they are needed; escaping; xml:space; and fragments written verbatim.

use bayan_xml::{
    Event, Fragment, KnownNamespace, Limits, QName, Reader, Source, WriteError, Writer,
};

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const W14: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const XML: &str = "http://www.w3.org/XML/1998/namespace";

fn w(local: &str) -> QName<'_> {
    QName::new(Some(W), "w", local)
}

/// Every event of `text` written again, start tags with their declarations and attributes as parsed.
fn rewrite(text: &str) -> String {
    let limits = Limits::default();
    let source = Source::new(text.as_bytes(), &limits).unwrap();
    let mut reader = Reader::new(&source, &limits);
    let mut writer = Writer::new();
    while let Some(event) = reader.next_event().unwrap() {
        match event {
            Event::Start(tag) => writer.start_tag(&tag).unwrap(),
            Event::End(_) => writer.end().unwrap(),
            Event::Text(text) => writer.text(text.value()).unwrap(),
            Event::CData(cdata) => writer.cdata(cdata.value()).unwrap(),
            Event::Comment(comment) => writer.comment(comment.text()).unwrap(),
            Event::ProcessingInstruction(pi) => {
                writer
                    .processing_instruction(pi.target(), pi.data())
                    .unwrap();
            }
        }
    }
    writer.finish().unwrap()
}

#[test]
fn writes_the_same_bytes_every_time() {
    let write = || {
        let mut writer = Writer::new();
        writer.declaration(Some(true)).unwrap();
        writer.whitespace("\r\n").unwrap();
        writer.start(w("document")).unwrap();
        writer.declare("w14", W14).unwrap();
        writer.declare("mc", MC).unwrap();
        writer
            .attribute(QName::new(Some(MC), "mc", "Ignorable"), "w14")
            .unwrap();
        writer.start(w("body")).unwrap();
        writer.start(w("p")).unwrap();
        writer
            .attribute(QName::new(Some(W14), "w14", "paraId"), "1A2B3C4D")
            .unwrap();
        writer.start(w("r")).unwrap();
        writer.start(w("rPr")).unwrap();
        writer.start(w("b")).unwrap();
        writer.end().unwrap();
        writer.end().unwrap();
        writer.text_element(w("t"), " Hello, world ").unwrap();
        writer.end().unwrap();
        writer.end().unwrap();
        writer.end().unwrap();
        writer.end().unwrap();
        writer.finish().unwrap()
    };
    let expected = concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n",
        "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" ",
        "xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\" ",
        "xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" mc:Ignorable=\"w14\">",
        "<w:body><w:p w14:paraId=\"1A2B3C4D\"><w:r><w:rPr><w:b/></w:rPr>",
        "<w:t xml:space=\"preserve\"> Hello, world </w:t></w:r></w:p></w:body></w:document>"
    );
    assert_eq!(write(), expected);
    assert_eq!(write(), write());
}

#[test]
fn escapes_text_and_attribute_values() {
    let mut writer = Writer::new();
    writer.start(QName::unqualified("a")).unwrap();
    writer
        .attribute(QName::unqualified("v"), "& < > \" ' \t \n \r ]]>")
        .unwrap();
    writer.text("& < > \" ' \t \n \r ]]>").unwrap();
    writer.end().unwrap();
    let written = writer.finish().unwrap();
    assert_eq!(
        written,
        "<a v=\"&amp; &lt; &gt; &quot; ' &#x9; &#xA; &#xD; ]]&gt;\">&amp; &lt; &gt; \" ' \t \n &#xD; ]]&gt;</a>"
    );
    // Read back, both are exactly what was written.
    let limits = Limits::default();
    let source = Source::new(written.as_bytes(), &limits).unwrap();
    let mut reader = Reader::new(&source, &limits);
    let Some(Event::Start(tag)) = reader.next_event().unwrap() else {
        panic!("expected the start tag");
    };
    assert_eq!(
        tag.attributes().next().unwrap().value(),
        "& < > \" ' \t \n \r ]]>"
    );
    let Some(Event::Text(text)) = reader.next_event().unwrap() else {
        panic!("expected the text");
    };
    assert_eq!(text.value(), "& < > \" ' \t \n \r ]]>");
}

#[test]
fn refuses_characters_xml_cannot_represent() {
    for bad in ["\u{0}", "a\u{1}b", "\u{B}", "\u{FFFE}", "\u{FFFF}"] {
        let mut writer = Writer::new();
        writer.start(QName::unqualified("a")).unwrap();
        assert_eq!(
            writer.attribute(QName::unqualified("v"), bad),
            Err(WriteError::InvalidCharacter)
        );
        let mut writer = Writer::new();
        writer.start(QName::unqualified("a")).unwrap();
        assert_eq!(writer.text(bad), Err(WriteError::InvalidCharacter));
        assert_eq!(writer.cdata(bad), Err(WriteError::InvalidCharacter));
        assert_eq!(writer.comment(bad), Err(WriteError::InvalidCharacter));
        assert_eq!(
            writer.processing_instruction("p", bad),
            Err(WriteError::InvalidCharacter)
        );
        assert_eq!(
            writer.declare("p", &format!("urn:{bad}")),
            Err(WriteError::InvalidCharacter)
        );
    }
}

#[test]
fn declares_namespaces_where_they_are_needed() {
    let mut writer = Writer::new();
    writer.start(w("document")).unwrap();
    writer.start(w("body")).unwrap();
    // A new namespace is declared on the element that first needs it, and only there.
    writer.start(QName::new(Some(W14), "w14", "x")).unwrap();
    writer
        .attribute(QName::new(Some("urn:a"), "a", "y"), "1")
        .unwrap();
    writer.start(QName::new(Some(W14), "w14", "z")).unwrap();
    writer.end().unwrap();
    writer.end().unwrap();
    writer.start(QName::new(Some(W14), "w14", "x")).unwrap();
    writer.end().unwrap();
    // A declaration on an empty element is gone after it, too.
    writer.start(QName::new(Some(W14), "w14", "x")).unwrap();
    writer.end().unwrap();
    // The default namespace, and an element in no namespace inside it.
    writer.start(QName::new(Some("urn:d"), "", "d")).unwrap();
    writer.start(QName::unqualified("plain")).unwrap();
    writer.end().unwrap();
    writer.end().unwrap();
    writer.end().unwrap();
    writer.end().unwrap();
    assert_eq!(
        writer.finish().unwrap(),
        concat!(
            "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>",
            "<w14:x xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\" xmlns:a=\"urn:a\" a:y=\"1\"><w14:z/></w14:x>",
            "<w14:x xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\"/>",
            "<w14:x xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\"/>",
            "<d xmlns=\"urn:d\"><plain xmlns=\"\"/></d></w:body></w:document>"
        )
    );
}

#[test]
fn uses_the_conventional_prefixes_of_known_namespaces() {
    let mut writer = Writer::new();
    writer
        .start(QName::known(KnownNamespace::WordprocessingMl, "document"))
        .unwrap();
    writer
        .attribute(QName::known(KnownNamespace::Relationships, "id"), "rId1")
        .unwrap();
    writer.end().unwrap();
    assert_eq!(
        writer.finish().unwrap(),
        "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" r:id=\"rId1\"/>"
    );
}

#[test]
fn refuses_one_prefix_for_two_namespaces_on_one_tag() {
    let mut writer = Writer::new();
    writer.start(QName::new(Some("urn:a"), "p", "x")).unwrap();
    writer.declare("p", "urn:b").unwrap();
    assert_eq!(writer.end(), Err(WriteError::PrefixConflict));
    let mut writer = Writer::new();
    writer.start(QName::new(Some("urn:a"), "p", "x")).unwrap();
    writer
        .attribute(QName::new(Some("urn:b"), "p", "y"), "1")
        .unwrap();
    assert_eq!(writer.end(), Err(WriteError::PrefixConflict));
    let mut writer = Writer::new();
    writer.start(QName::unqualified("x")).unwrap();
    writer.declare("", "urn:d").unwrap();
    assert_eq!(writer.end(), Err(WriteError::PrefixConflict));
    // An inner element may bind a prefix again.
    let mut writer = Writer::new();
    writer.start(QName::new(Some("urn:a"), "p", "x")).unwrap();
    writer.start(QName::new(Some("urn:b"), "p", "y")).unwrap();
    writer.end().unwrap();
    writer.end().unwrap();
    assert_eq!(
        writer.finish().unwrap(),
        "<p:x xmlns:p=\"urn:a\"><p:y xmlns:p=\"urn:b\"/></p:x>"
    );
}

#[test]
fn refuses_invalid_names_at_once() {
    for name in [
        QName::unqualified(""),
        QName::unqualified("1a"),
        QName::unqualified("a:b"),
        QName::new(Some("urn:a"), "1p", "a"),
        QName::new(Some("urn:a"), "p:q", "a"),
        QName::new(None, "p", "a"),
        QName::new(Some(""), "p", "a"),
    ] {
        assert_eq!(
            Writer::new().start(name),
            Err(WriteError::InvalidName),
            "{name:?}"
        );
    }
    let mut writer = Writer::new();
    writer.start(QName::unqualified("a")).unwrap();
    for target in ["", "1", "a:b", "xml", "XmL", "a b"] {
        assert_eq!(
            writer.processing_instruction(target, ""),
            Err(WriteError::InvalidName),
            "{target}"
        );
    }
    assert_eq!(writer.declare("1p", "urn:p"), Err(WriteError::InvalidName));
    assert_eq!(
        writer.attribute(QName::unqualified("1a"), "x"),
        Err(WriteError::InvalidName)
    );
    // A refused call changes nothing: the writer goes on.
    writer.end().unwrap();
    assert_eq!(writer.finish().unwrap(), "<a/>");
}

#[test]
fn refuses_reserved_prefixes_and_namespaces_at_once() {
    for (prefix, uri) in [
        ("xmlns", "urn:x"),
        ("xml", "urn:other"),
        ("p", "http://www.w3.org/2000/xmlns/"),
        ("p", XML),
        ("", XML),
        ("", "http://www.w3.org/2000/xmlns/"),
        ("p", ""),
    ] {
        let mut writer = Writer::new();
        writer.start(QName::unqualified("a")).unwrap();
        assert_eq!(
            writer.declare(prefix, uri),
            Err(WriteError::ReservedPrefix),
            "{prefix} {uri}"
        );
    }
    for name in [
        QName::new(Some("urn:x"), "xmlns", "a"),
        QName::new(Some("urn:x"), "xml", "a"),
        QName::new(Some(XML), "p", "a"),
    ] {
        assert_eq!(
            Writer::new().start(name),
            Err(WriteError::ReservedPrefix),
            "{name:?}"
        );
    }
    let mut writer = Writer::new();
    writer.start(QName::unqualified("a")).unwrap();
    assert_eq!(
        writer.attribute(QName::unqualified("xmlns"), "urn:x"),
        Err(WriteError::ReservedPrefix)
    );
    assert_eq!(
        writer.attribute(
            QName::new(Some("http://www.w3.org/2000/xmlns/"), "xmlns", "p"),
            "urn:x"
        ),
        Err(WriteError::ReservedPrefix)
    );
    // The xml prefix is always there for its own namespace.
    writer
        .attribute(QName::new(Some(XML), "xml", "lang"), "en")
        .unwrap();
    writer.end().unwrap();
    assert_eq!(writer.finish().unwrap(), "<a xml:lang=\"en\"/>");
}

#[test]
fn refuses_attribute_prefixes_that_do_not_match_their_namespace() {
    for name in [
        QName::new(None, "p", "a"),
        QName::new(Some("urn:x"), "", "a"),
    ] {
        let mut writer = Writer::new();
        writer.start(QName::unqualified("e")).unwrap();
        assert_eq!(
            writer.attribute(name, "1"),
            Err(WriteError::InvalidAttributeName),
            "{name:?}"
        );
    }
}

#[test]
fn refuses_duplicate_attributes_and_declarations() {
    let mut writer = Writer::new();
    writer.start(QName::unqualified("e")).unwrap();
    writer.attribute(QName::unqualified("a"), "1").unwrap();
    writer.attribute(QName::unqualified("a"), "2").unwrap();
    assert_eq!(writer.end(), Err(WriteError::DuplicateAttribute));
    let mut writer = Writer::new();
    writer.start(QName::unqualified("e")).unwrap();
    writer
        .attribute(QName::new(Some("urn:x"), "p", "a"), "1")
        .unwrap();
    writer
        .attribute(QName::new(Some("urn:x"), "q", "a"), "2")
        .unwrap();
    assert_eq!(writer.end(), Err(WriteError::DuplicateAttribute));
    let mut writer = Writer::new();
    writer.start(QName::unqualified("e")).unwrap();
    writer.declare("p", "urn:a").unwrap();
    writer.declare("p", "urn:a").unwrap();
    assert_eq!(writer.end(), Err(WriteError::DuplicateAttribute));
}

#[test]
fn refuses_compatibility_attributes_that_name_undeclared_prefixes() {
    let mut writer = Writer::new();
    writer.start(w("document")).unwrap();
    writer
        .attribute(QName::new(Some(MC), "mc", "Ignorable"), "w14 w15")
        .unwrap();
    writer.declare("w14", W14).unwrap();
    assert_eq!(writer.end(), Err(WriteError::UndeclaredPrefix));
    let mut writer = Writer::new();
    writer.start(w("document")).unwrap();
    writer
        .attribute(QName::new(Some(MC), "mc", "ProcessContent"), "w14:*")
        .unwrap();
    assert_eq!(writer.end(), Err(WriteError::UndeclaredPrefix));
    // A ProcessContent token needs a prefix (ECMA-376 Part 3 §7.3), even where a default namespace is declared.
    let mut writer = Writer::new();
    writer.start(w("document")).unwrap();
    writer.declare("", W14).unwrap();
    writer
        .attribute(QName::new(Some(MC), "mc", "ProcessContent"), "x")
        .unwrap();
    assert_eq!(writer.end(), Err(WriteError::UndeclaredPrefix));
    let mut writer = Writer::new();
    writer.start(QName::new(Some(MC), "mc", "Choice")).unwrap();
    writer
        .attribute(QName::unqualified("Requires"), "wps")
        .unwrap();
    assert_eq!(writer.end(), Err(WriteError::UndeclaredPrefix));
    // Declared on the same tag or an ancestor, they are fine.
    let mut writer = Writer::new();
    writer.start(w("document")).unwrap();
    writer.declare("wps", "urn:wps").unwrap();
    writer
        .start(QName::new(Some(MC), "mc", "AlternateContent"))
        .unwrap();
    writer.start(QName::new(Some(MC), "mc", "Choice")).unwrap();
    writer
        .attribute(QName::unqualified("Requires"), " wps ")
        .unwrap();
    writer
        .attribute(QName::new(Some(MC), "mc", "Ignorable"), "w14")
        .unwrap();
    writer.declare("w14", W14).unwrap();
    writer.end().unwrap();
    writer.end().unwrap();
    writer.end().unwrap();
    assert!(writer.finish().is_ok());
}

#[test]
fn writes_xml_space_preserve_when_text_needs_it() {
    for (text, preserve) in [
        ("a", false),
        ("a b", false),
        ("", false),
        (" a", true),
        ("a ", true),
        ("a  b", true),
        ("a\tb", true),
        ("a\nb", true),
        ("a\rb", true),
        ("\u{A0}a\u{A0}", false),
    ] {
        let mut writer = Writer::new();
        writer.start(QName::unqualified("r")).unwrap();
        writer.text_element(QName::unqualified("t"), text).unwrap();
        writer.end().unwrap();
        let written = writer.finish().unwrap();
        assert_eq!(
            written.contains("xml:space=\"preserve\""),
            preserve,
            "{text:?}"
        );
    }
    // Where preserve is already in force, it is not repeated.
    let mut writer = Writer::new();
    writer.start(QName::unqualified("r")).unwrap();
    writer
        .attribute(QName::new(Some(XML), "xml", "space"), "preserve")
        .unwrap();
    writer.text_element(QName::unqualified("t"), " a ").unwrap();
    writer.end().unwrap();
    assert_eq!(
        writer.finish().unwrap(),
        "<r xml:space=\"preserve\"><t> a </t></r>"
    );
}

#[test]
fn writes_comments_cdata_and_processing_instructions() {
    let mut writer = Writer::new();
    writer.comment(" before ").unwrap();
    writer.processing_instruction("p", "").unwrap();
    writer.start(QName::unqualified("a")).unwrap();
    writer.cdata("x]]>y\rz").unwrap();
    writer.processing_instruction("q", "data ?").unwrap();
    writer.end().unwrap();
    writer.comment("after").unwrap();
    let written = writer.finish().unwrap();
    assert_eq!(
        written,
        "<!-- before --><?p?><a><![CDATA[x]]]]><![CDATA[>y]]>&#xD;<![CDATA[z]]><?q data ??></a><!--after-->"
    );
    // The CDATA sections read back as the text that was written.
    let limits = Limits::default();
    let source = Source::new(written.as_bytes(), &limits).unwrap();
    let mut reader = Reader::new(&source, &limits);
    let mut text = String::new();
    while let Some(event) = reader.next_event().unwrap() {
        match event {
            Event::CData(cdata) => text.push_str(cdata.value()),
            Event::Text(more) => text.push_str(more.value()),
            _ => {}
        }
    }
    assert_eq!(text, "x]]>y\rz");
    let mut writer = Writer::new();
    writer.start(QName::unqualified("a")).unwrap();
    for comment in ["a--b", "a-", "-"] {
        assert_eq!(
            writer.comment(comment),
            Err(WriteError::InvalidComment),
            "{comment}"
        );
    }
    for data in ["a?>b", " a", "\ta"] {
        assert_eq!(
            writer.processing_instruction("p", data),
            Err(WriteError::InvalidProcessingInstruction),
            "{data:?}"
        );
    }
}

#[test]
fn refuses_requests_out_of_place() {
    let mut writer = Writer::new();
    assert_eq!(writer.end(), Err(WriteError::NoOpenElement));
    assert_eq!(
        writer.attribute(QName::unqualified("a"), "1"),
        Err(WriteError::NoOpenElement)
    );
    assert_eq!(writer.declare("p", "urn:p"), Err(WriteError::NoOpenElement));
    assert_eq!(writer.text("x"), Err(WriteError::ContentOutsideRoot));
    assert_eq!(writer.cdata("x"), Err(WriteError::ContentOutsideRoot));
    assert_eq!(writer.whitespace(" x"), Err(WriteError::ContentOutsideRoot));
    writer.whitespace(" \n").unwrap();
    assert_eq!(
        writer.declaration(None),
        Err(WriteError::MisplacedDeclaration)
    );
    writer.start(QName::unqualified("a")).unwrap();
    // Once the start tag is closed by content, it takes no more attributes.
    writer.text("x").unwrap();
    assert_eq!(
        writer.attribute(QName::unqualified("b"), "1"),
        Err(WriteError::NoOpenElement)
    );
    assert_eq!(writer.whitespace(" "), Err(WriteError::ContentOutsideRoot));
    writer.end().unwrap();
    assert_eq!(
        writer.start(QName::unqualified("b")),
        Err(WriteError::ContentOutsideRoot)
    );
    assert_eq!(writer.text("x"), Err(WriteError::ContentOutsideRoot));
    assert_eq!(Writer::new().finish(), Err(WriteError::Unfinished));
    let mut writer = Writer::new();
    writer.start(QName::unqualified("a")).unwrap();
    assert_eq!(writer.finish(), Err(WriteError::Unfinished));
}

#[test]
fn copies_parsed_start_tags() {
    let text = "<w:document xmlns:w=\"urn:w\" xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" xmlns=\"urn:d\" mc:Ignorable=\"w\"><w:p w:a=\"x&amp;y&#9;z\" b='q\"'><d/><plain xmlns=\"\"/></w:p></w:document>";
    assert_eq!(
        rewrite(text),
        "<w:document xmlns:w=\"urn:w\" xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" xmlns=\"urn:d\" mc:Ignorable=\"w\"><w:p w:a=\"x&amp;y&#x9;z\" b=\"q&quot;\"><d/><plain xmlns=\"\"/></w:p></w:document>"
    );
}

/// The `occurrence`-th element named `local` in `text`, captured.
fn capture(text: &str, local: &str) -> Fragment<'static> {
    let limits = Limits::default();
    let source = Source::new(text.as_bytes(), &limits).unwrap();
    let mut reader = Reader::new(&source, &limits);
    loop {
        if let Some(Event::Start(tag)) = reader.next_event().unwrap()
            && tag.name().local() == local
        {
            return reader.capture().unwrap().into_owned();
        }
    }
}

const DOCUMENT: &str = "<w:document xmlns:w=\"urn:w\" xmlns:w14=\"urn:w14\"><w:body><w:p w14:paraId=\"1\"><w:r><w:t>x</w:t></w:r></w:p></w:body></w:document>";

#[test]
fn writes_fragments_verbatim_where_their_context_holds() {
    let paragraph = capture(DOCUMENT, "p");
    let mut writer = Writer::new();
    writer
        .start(QName::new(Some("urn:w"), "w", "document"))
        .unwrap();
    writer.declare("w14", "urn:w14").unwrap();
    writer
        .start(QName::new(Some("urn:w"), "w", "body"))
        .unwrap();
    writer.fragment(&paragraph).unwrap();
    writer.end().unwrap();
    writer.end().unwrap();
    assert_eq!(writer.finish().unwrap(), DOCUMENT);
}

#[test]
fn adds_only_the_declarations_a_fragment_lacks() {
    let paragraph = capture(DOCUMENT, "p");
    // Into an empty document: everything it uses.
    let mut writer = Writer::new();
    writer.fragment(&paragraph).unwrap();
    assert_eq!(
        writer.finish().unwrap(),
        "<w:p xmlns:w=\"urn:w\" xmlns:w14=\"urn:w14\" w14:paraId=\"1\"><w:r><w:t>x</w:t></w:r></w:p>"
    );
    // Where w is bound already but w14 elsewhere: only w14.
    let mut writer = Writer::new();
    writer
        .start(QName::new(Some("urn:w"), "w", "body"))
        .unwrap();
    writer.declare("w14", "urn:other").unwrap();
    writer.declare("x", "urn:unused").unwrap();
    writer.fragment(&paragraph).unwrap();
    writer.end().unwrap();
    assert_eq!(
        writer.finish().unwrap(),
        "<w:body xmlns:w=\"urn:w\" xmlns:w14=\"urn:other\" xmlns:x=\"urn:unused\"><w:p xmlns:w14=\"urn:w14\" w14:paraId=\"1\"><w:r><w:t>x</w:t></w:r></w:p></w:body>"
    );
}

#[test]
fn keeps_unqualified_fragments_out_of_a_default_namespace() {
    let fragment = capture("<r><a><b/></a></r>", "a");
    let mut writer = Writer::new();
    writer.start(QName::new(Some("urn:d"), "", "root")).unwrap();
    writer.fragment(&fragment).unwrap();
    writer.end().unwrap();
    assert_eq!(
        writer.finish().unwrap(),
        "<root xmlns=\"urn:d\"><a xmlns=\"\"><b/></a></root>"
    );
}

#[test]
fn carries_the_white_space_mode_of_a_fragment() {
    let fragment = capture("<r xml:space=\"preserve\"><a> x </a></r>", "a");
    let mut writer = Writer::new();
    writer.start(QName::unqualified("r")).unwrap();
    writer.fragment(&fragment).unwrap();
    writer.end().unwrap();
    assert_eq!(
        writer.finish().unwrap(),
        "<r><a xml:space=\"preserve\"> x </a></r>"
    );
    let fragment = capture("<r><a> x </a></r>", "a");
    let mut writer = Writer::new();
    writer.start(QName::unqualified("r")).unwrap();
    writer
        .attribute(QName::new(Some(XML), "xml", "space"), "preserve")
        .unwrap();
    writer.fragment(&fragment).unwrap();
    writer.end().unwrap();
    assert_eq!(
        writer.finish().unwrap(),
        "<r xml:space=\"preserve\"><a xml:space=\"default\"> x </a></r>"
    );
}
