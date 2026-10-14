//! The canonical form: the same for documents that mean the same, different for documents that do not.

use bayan_xml::{ErrorKind, Limits, Source, canonicalize};

use crate::common::first_difference;

const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";

/// The canonical form of `text`, with `'MC'` and `'XSI'` standing for the namespace names of Markup Compatibility and XML Schema instances.
fn canonical(text: &str) -> String {
    let text = text
        .replace("'MC'", &format!("'{MC}'"))
        .replace("'XSI'", &format!("'{XSI}'"));
    canonical_bytes(text.as_bytes())
}

fn canonical_bytes(bytes: &[u8]) -> String {
    let limits = Limits::default();
    let source = Source::new(bytes, &limits).unwrap();
    canonicalize(&source, &limits).unwrap()
}

#[test]
fn writes_the_documented_form() {
    let text = concat!(
        "<?xml version=\"1.0\"?>\n<!-- c --><?pi data?>\n",
        "<w:doc xmlns:w=\"urn:w\" xmlns:mc='MC' xmlns:x=\"urn:x\" mc:Ignorable=\"x\" b=\"2\" a=\"1&#9;\">",
        "a &amp; <![CDATA[<b>]]>\r\n<x:e/><?p?></w:doc>\n<!--after-->"
    );
    assert_eq!(
        canonical(text),
        concat!(
            "{0} urn:w\n{1} http://schemas.openxmlformats.org/markup-compatibility/2006\n{2} urn:x\n",
            "<!-- c --><?pi data?>",
            "<{0}doc a=\"1&#x9;\" b=\"2\" {1}Ignorable=\"{2}\">a &amp; &lt;b&gt;\n<{2}e></{2}e><?p?></{0}doc>",
            "<!--after-->"
        )
    );
}

/// Pairs of documents written differently that mean the same.
const SAME: &[(&str, &str)] = &[
    // Attribute order and quotes.
    ("<a x='1' y=\"2\"/>", "<a y='2' x=\"1\"/>"),
    // Empty-element tags.
    ("<a><b/></a>", "<a><b></b></a>"),
    // Prefixes, default namespaces and declarations nothing uses.
    (
        "<p:a xmlns:p='urn:a'><p:b/></p:a>",
        "<a xmlns='urn:a'><b/></a>",
    ),
    (
        "<p:a xmlns:p='urn:a'/>",
        "<q:a xmlns:q='urn:a' xmlns:unused='urn:u'/>",
    ),
    (
        "<a xmlns:p='urn:p' p:x='1'/>",
        "<a xmlns:q='urn:p' q:x='1'/>",
    ),
    // References, CDATA sections and line ends.
    ("<a>x &lt; y &amp; z</a>", "<a><![CDATA[x < y & z]]></a>"),
    ("<a>&#65;&#x42;</a>", "<a>AB</a>"),
    ("<a>1\r\n2\r3</a>", "<a>1\n2\n3</a>"),
    ("<a>x<![CDATA[]]>y</a>", "<a>xy</a>"),
    // Attribute values as normalized.
    ("<a v='&#x20;'/>", "<a v=' '/>"),
    ("<a v='a\nb\tc'/>", "<a v='a b c'/>"),
    // The XML declaration and white space outside the root element.
    ("<?xml version='1.0' encoding='UTF-8'?>\n<a/>\n", "<a/>"),
    // xml:space where it changes nothing.
    ("<a xml:space='default'><b/></a>", "<a><b/></a>"),
    (
        "<a xml:space='preserve'><b xml:space='preserve'/></a>",
        "<a xml:space='preserve'><b/></a>",
    ),
    // Markup Compatibility lists and xsi:type name namespaces, whatever their prefixes and spacing.
    (
        "<a xmlns:mc='MC' xmlns:p='urn:p' mc:Ignorable='p'/>",
        "<a xmlns:m='MC' xmlns:q='urn:p' m:Ignorable=' q '/>",
    ),
    (
        "<a xmlns:mc='MC' xmlns:p='urn:p' xmlns:q='urn:q' mc:MustUnderstand='p q'/>",
        "<a xmlns:mc='MC' xmlns:x='urn:p' xmlns:y='urn:q' mc:MustUnderstand='x\ty'/>",
    ),
    (
        "<a xmlns:mc='MC' xmlns:p='urn:p' mc:ProcessContent='p:x p:*'/>",
        "<a xmlns:mc='MC' xmlns:q='urn:p' mc:ProcessContent='q:x q:*'/>",
    ),
    (
        "<a xmlns:mc='MC' xmlns:p='urn:p' mc:PreserveElements='p:x' mc:PreserveAttributes='p:y'/>",
        "<a xmlns:mc='MC' xmlns:q='urn:p' mc:PreserveElements='q:x' mc:PreserveAttributes='q:y'/>",
    ),
    (
        "<mc:Choice xmlns:mc='MC' xmlns:p='urn:p' Requires='p'/>",
        "<mc:Choice xmlns:mc='MC' xmlns:q='urn:p' Requires='q'/>",
    ),
    (
        "<a xmlns:xsi='XSI' xmlns:p='urn:p' xsi:type='p:t'/>",
        "<a xmlns:xsi='XSI' xmlns:q='urn:p' xsi:type=' q:t '/>",
    ),
    // An unprefixed name in xsi:type is in the default namespace, or in none.
    (
        "<a xmlns:xsi='XSI' xsi:type=' t '/>",
        "<a xmlns:xsi='XSI' xsi:type='t'/>",
    ),
    (
        "<p:a xmlns:p='urn:a' xmlns:xsi='XSI' xmlns:t='urn:t' xsi:type='t:x'/>",
        "<p:a xmlns:p='urn:a' xmlns:xsi='XSI' xmlns='urn:t' xsi:type='x'/>",
    ),
];

/// Pairs of documents that look alike but mean something different.
const DIFFERENT: &[(&str, &str)] = &[
    ("<p:a xmlns:p='urn:a'/>", "<p:a xmlns:p='urn:b'/>"),
    ("<a/>", "<b/>"),
    ("<a x='1'/>", "<a x='2'/>"),
    ("<a x='1'/>", "<a xmlns:p='urn:p' p:x='1'/>"),
    ("<a>x</a>", "<a>y</a>"),
    // White space inside the root element is content.
    ("<a><b/> </a>", "<a><b/></a>"),
    ("<a><!--c--></a>", "<a/>"),
    ("<a><?p?></a>", "<a/>"),
    ("<a/><!--c-->", "<a/>"),
    // xml:space that changes the white space mode.
    ("<a xml:space='preserve'/>", "<a/>"),
    (
        "<a xml:space='preserve'><b xml:space='default'/></a>",
        "<a xml:space='preserve'><b/></a>",
    ),
    // The namespaces that Markup Compatibility lists and xsi:type name.
    (
        "<a xmlns:mc='MC' xmlns:p='urn:p' mc:Ignorable='p'/>",
        "<a xmlns:mc='MC' xmlns:p='urn:q' mc:Ignorable='p'/>",
    ),
    (
        "<a xmlns:mc='MC' xmlns:p='urn:p' mc:ProcessContent='p:x'/>",
        "<a xmlns:mc='MC' xmlns:p='urn:p' mc:ProcessContent='p:y'/>",
    ),
    (
        "<a xmlns:mc='MC' mc:Ignorable='p'/>",
        "<a xmlns:mc='MC' xmlns:p='urn:p' mc:Ignorable='p'/>",
    ),
    (
        "<mc:Choice xmlns:mc='MC' xmlns:p='urn:p' Requires='p'/>",
        "<mc:Choice xmlns:mc='MC' xmlns:p='urn:q' Requires='p'/>",
    ),
    (
        "<a xmlns:xsi='XSI' xmlns:p='urn:p' xsi:type='p:t'/>",
        "<a xmlns:xsi='XSI' xmlns:p='urn:q' xsi:type='p:t'/>",
    ),
    // Requires means nothing on other elements.
    (
        "<a xmlns:p='urn:p' Requires='p'/>",
        "<a xmlns:q='urn:p' Requires='q'/>",
    ),
    // A token written like a numbered namespace is not taken for one.
    (
        "<a xmlns:mc='MC' xmlns:p='urn:p' p:x='' mc:Ignorable='{1}'/>",
        "<a xmlns:mc='MC' xmlns:p='urn:p' p:x='' mc:Ignorable='p'/>",
    ),
    (
        "<a xmlns:mc='MC' xmlns:p='urn:p' p:x='' mc:ProcessContent='p:{1}'/>",
        "<a xmlns:mc='MC' xmlns:p='urn:p' p:x='' mc:ProcessContent='p:x'/>",
    ),
];

#[test]
fn ignores_how_a_document_is_written() {
    for (index, (a, b)) in SAME.iter().enumerate() {
        let difference = first_difference(&canonical(a), &canonical(b));
        assert_eq!(difference, None, "pair {index} differs");
    }
}

#[test]
fn notices_what_a_document_means() {
    for (index, (a, b)) in DIFFERENT.iter().enumerate() {
        assert_ne!(canonical(a), canonical(b), "pair {index} is the same");
    }
}

#[test]
fn ignores_the_encoding() {
    let text = "<a>\u{e9}\u{10000}</a>";
    let utf16le: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    let utf16be: Vec<u8> = [0xFE, 0xFF]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_be_bytes))
        .collect();
    let with_bom = format!("\u{feff}{text}");
    assert_eq!(canonical_bytes(&utf16le), canonical(text));
    assert_eq!(canonical_bytes(&utf16be), canonical(text));
    assert_eq!(canonical_bytes(with_bom.as_bytes()), canonical(text));
}

#[test]
fn refuses_what_the_reader_refuses() {
    let limits = Limits::default();
    let source = Source::new(b"<a><b></a>", &limits).unwrap();
    let error = canonicalize(&source, &limits).unwrap_err();
    assert_eq!(
        (error.kind(), error.offset()),
        (ErrorKind::MismatchedEndTag, 6)
    );
}
