//! The XML of the package's three metadata parts, the content types stream, relationships parts and the core properties part: reading them with bayan-xml's streaming reader within the package's [`Limits`], and the little XML that writing them back needs.
//!
//! bayan-xml does all the reading ([`bayan_xml::Source`], [`bayan_xml::Reader`]): it decodes UTF-8 and UTF-16, refuses document type declarations and with them every entity but the five predefined ones, checks well-formedness and namespaces, and enforces its own limits, which [`read`] sets from bayan-opc's. This module adds what the metadata parts need on top:
//!
//! - It counts the nodes of each part against [`Limits::max_xml_nodes`], a budget that all the metadata of a package shares, exactly as bayan-opc counted them before it read its metadata with bayan-xml: every element, attribute (namespace declarations included), run of text and CDATA section, and every comment and processing instruction inside the root element, is one node; end tags, and what comes before or after the root element, are none.
//! - It hands every event to the model being read, the content types stream, a relationships part or the core properties part, which keeps only what it needs. No part is held as a tree: an event is gone once the model has seen it, so a part's nodes cost the reader nothing once they are read, and only what the models keep stays in memory.
//! - A rule of the model that an event breaks is remembered, the model sees no more events, and reading goes on to the end of the part, so that a part that is not acceptable XML, or has too many nodes, is refused as such, whatever its model would say: the errors are the same as when the whole part was parsed before its model looked at it.
//! - It keeps the decoded text, which every span of bayan-xml's events refers to, and the part's encoding, so that a model can write an unchanged part back exactly as it was ([`encode`] gives the original bytes back, UTF-16 included, since decoding UTF-16 without errors loses nothing) and a changed part in its own encoding (bayan-xml's writer writes only UTF-8, so a part kept in UTF-16 is encoded here).
//!
//! Writing is plain text, assembled by the models from the original text and new entries, with [`escape_attribute`] and [`escape_text`] and the character checks below; the rules for names exist once, in bayan-xml ([`is_ncname`]).

use bayan_xml::{Event, Reader, Source};

use crate::{Error, LimitError, Limits, XmlError, XmlErrorKind};

pub(crate) use bayan_xml::Encoding;

/// The namespace of the `xml:` prefix, which is always declared.
pub(crate) const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// A metadata part that was read: its decoded text, which every span of bayan-xml's events refers to, and the encoding of its bytes.
#[derive(Debug)]
pub(crate) struct Document {
    /// The part as UTF-8 text, without a byte order mark.
    pub(crate) text: String,
    /// The encoding of the original bytes.
    pub(crate) encoding: Encoding,
}

/// Reads a metadata part, the content of the ZIP entry at `entry` (if known), within `limits`: first its size against [`Limits::max_metadata_size`], then its XML with bayan-xml, whose nodes it counts against `nodes`, a budget that the metadata parts of one package share.
///
/// `model` sees every event with its depth: for a start or end tag, the element's own depth (the root element is at depth 1); for anything else, the depth of the element it is in (0 outside the root element). The first error `model` returns is the part's error, unless the part turns out not to be acceptable XML; `model` sees no event after it.
///
/// `nodes` is reduced by the part's nodes only when the part is read without an error.
pub(crate) fn read(
    bytes: &[u8],
    limits: &Limits,
    entry: Option<usize>,
    nodes: &mut usize,
    mut model: impl FnMut(&Event<'_>, usize) -> Result<(), Error>,
) -> Result<Document, Error> {
    if bytes.len() > limits.max_metadata_size {
        return Err(LimitError::MetadataTooLarge {
            entry,
            limit: limits.max_metadata_size,
        }
        .into());
    }
    let xml_limits = xml_limits(limits);
    let refused = |error: bayan_xml::Error| refusal(error, entry, limits);
    let source = Source::new(bytes, &xml_limits).map_err(refused)?;
    let mut reader = Reader::new(&source, &xml_limits);
    let mut budget = *nodes;
    // How many elements are open around the current event.
    let mut open: usize = 0;
    let mut broken: Option<Error> = None;
    while let Some(event) = reader.next_event().map_err(refused)? {
        count(&event, open, &mut budget).map_err(|error| Error::Xml { entry, error })?;
        let depth = match &event {
            Event::Start(tag) => tag.depth(),
            Event::End(end) => end.depth(),
            _ => open,
        };
        if broken.is_none()
            && let Err(error) = model(&event, depth)
        {
            broken = Some(error);
        }
        match event {
            Event::Start(_) => open = open.saturating_add(1),
            Event::End(_) => open = open.saturating_sub(1),
            _ => {}
        }
    }
    if let Some(error) = broken {
        return Err(error);
    }
    *nodes = budget;
    Ok(Document {
        text: source.text().to_owned(),
        encoding: source.encoding(),
    })
}

/// bayan-xml's limits for a metadata part, from bayan-opc's.
fn xml_limits(limits: &Limits) -> bayan_xml::Limits {
    bayan_xml::Limits {
        // Checked before, with bayan-opc's own error; this only keeps bayan-xml's lower default from refusing a part that a host's limits allow.
        max_size: limits.max_metadata_size,
        max_depth: limits.max_xml_depth,
        max_attributes: limits.max_xml_attributes,
        max_name_length: limits.max_xml_name_length,
        // Every namespace that a part declares takes an attribute, a node, so the node budget bounds them already; and bayan-xml keeps each namespace name once, however many elements use it, and however long it is within the part's size.
        max_namespaces: limits.max_xml_nodes,
        max_namespace_name_length: limits.max_metadata_size,
        // The plain reader does not read Markup Compatibility attributes.
        max_compatibility_tokens: bayan_xml::Limits::DEFAULT.max_compatibility_tokens,
    }
}

/// Counts the nodes of `event`, which is inside `open` elements, against `budget`, each at the offset where it starts, as bayan-opc has always counted them: an element and each of its attributes, namespace declarations included; a run of text and a CDATA section; and a comment or processing instruction inside the root element. End tags count nothing.
fn count(event: &Event<'_>, open: usize, budget: &mut usize) -> Result<(), XmlError> {
    let mut take = |offset: usize| {
        *budget = budget.checked_sub(1).ok_or(XmlError {
            offset,
            kind: XmlErrorKind::TooManyNodes,
        })?;
        Ok(())
    };
    match event {
        Event::Start(tag) => {
            take(tag.span().start)?;
            for attribute in tag.attributes() {
                take(attribute.span().start)?;
            }
            Ok(())
        }
        Event::Text(text) => take(text.span().start),
        Event::CData(cdata) => take(cdata.span().start),
        Event::Comment(comment) if open > 0 => take(comment.span().start),
        Event::ProcessingInstruction(instruction) if open > 0 => take(instruction.span().start),
        Event::Comment(_) | Event::ProcessingInstruction(_) | Event::End(_) => Ok(()),
    }
}

/// The error for a part that bayan-xml refuses, in bayan-opc's terms.
fn refusal(error: bayan_xml::Error, entry: Option<usize>, limits: &Limits) -> Error {
    use bayan_xml::ErrorKind;
    let kind = match error.kind() {
        // [`read`] checks the size first, with the same limit; kept for completeness.
        ErrorKind::TooLarge => {
            return LimitError::MetadataTooLarge {
                entry,
                limit: limits.max_metadata_size,
            }
            .into();
        }
        ErrorKind::InvalidEncoding => XmlErrorKind::InvalidEncoding,
        ErrorKind::UnsupportedEncoding => XmlErrorKind::UnsupportedEncoding,
        ErrorKind::MalformedDeclaration => XmlErrorKind::MalformedDeclaration,
        ErrorKind::DoctypeNotAllowed => XmlErrorKind::DoctypeNotAllowed,
        ErrorKind::UndefinedEntity => XmlErrorKind::UndefinedEntity,
        ErrorKind::InvalidCharacterReference => XmlErrorKind::InvalidCharacterReference,
        ErrorKind::InvalidCharacter => XmlErrorKind::InvalidCharacter,
        ErrorKind::UnexpectedEnd => XmlErrorKind::UnexpectedEnd,
        ErrorKind::MismatchedEndTag => XmlErrorKind::MismatchedEndTag,
        ErrorKind::DuplicateAttribute => XmlErrorKind::DuplicateAttribute,
        ErrorKind::UndeclaredPrefix
        | ErrorKind::InvalidNamespaceDeclaration
        | ErrorKind::ReservedPrefix => XmlErrorKind::NamespaceError,
        ErrorKind::ContentOutsideRoot | ErrorKind::NoRootElement => {
            XmlErrorKind::ContentOutsideRoot
        }
        ErrorKind::TooDeep => XmlErrorKind::TooDeep,
        ErrorKind::TooManyAttributes => XmlErrorKind::TooManyAttributes,
        ErrorKind::NameTooLong | ErrorKind::NamespaceNameTooLong => XmlErrorKind::NameTooLong,
        // Every declaration of a new namespace is a node, so a part reaches this limit only past the node budget ([`xml_limits`]).
        ErrorKind::TooManyNamespaces => XmlErrorKind::TooManyNodes,
        ErrorKind::MisplacedDeclaration
        | ErrorKind::MalformedReference
        | ErrorKind::MalformedTag
        | ErrorKind::MalformedComment
        | ErrorKind::MalformedProcessingInstruction
        | ErrorKind::MalformedMarkup
        | ErrorKind::InvalidName
        | ErrorKind::LessThanInAttribute
        | ErrorKind::CDataEndInText => XmlErrorKind::Malformed,
        // What only Markup Compatibility processing and fragments report, which the plain reader never does, and kinds that later versions of bayan-xml may add.
        _ => XmlErrorKind::Malformed,
    };
    Error::Xml {
        entry,
        error: XmlError {
            offset: error.offset(),
            kind,
        },
    }
}

/// The qualified name of an element or attribute, as written: `prefix:local`, or `local` without a prefix.
pub(crate) fn qualified_name(name: &bayan_xml::Name<'_>) -> String {
    if name.prefix().is_empty() {
        name.local().to_owned()
    } else {
        format!("{}:{}", name.prefix(), name.local())
    }
}

/// Whether `name` is `local` in the namespace `namespace`, compared exactly.
pub(crate) fn is(name: &bayan_xml::Name<'_>, namespace: &str, local: &str) -> bool {
    name.local() == local
        && name
            .namespace()
            .is_some_and(|found| found.uri() == namespace)
}

/// The namespace of `name`, compared exactly, if it has one.
pub(crate) fn namespace_of<'r>(name: &bayan_xml::Name<'r>) -> Option<&'r str> {
    name.namespace().map(|namespace| namespace.uri())
}

/// Whether XML 1.0 allows `character` (§2.2, `Char`), as itself or as a character reference: no control characters other than tab, line feed and carriage return, and neither U+FFFE nor U+FFFF. What a writer checks before it writes text from elsewhere.
pub(crate) fn is_xml_character(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\r' | ' '..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

/// XML white space (§2.3, `S`).
pub(crate) fn is_space(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\r' | '\n')
}

/// Whether `text` is an `NCName` of Namespaces in XML (a name without a colon), the form of `xsd:ID` values such as relationship identifiers: as bayan-xml's writer judges the name of an element without a prefix, so that the rules for names exist once, in bayan-xml.
pub(crate) fn is_ncname(text: &str) -> bool {
    bayan_xml::Writer::new()
        .start(bayan_xml::QName::unqualified(text))
        .is_ok()
}

/// Writes `value` as the content of an attribute in double quotes: `&`, `<`, `>` and `"` as entity references, and tab, line feed and carriage return as character references, so that attribute value normalization gives them back.
pub(crate) fn escape_attribute(value: &str, out: &mut String) {
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            other => out.push(other),
        }
    }
}

/// Writes `value` as character data: `&`, `<` and `>` as entity references, and carriage return as a character reference, so that line end normalization gives it back.
pub(crate) fn escape_text(value: &str, out: &mut String) {
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            other => out.push(other),
        }
    }
}

/// Encodes `text` in `encoding`, with the byte order mark the original had.
pub(crate) fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Utf8 { bom: false } => text.as_bytes().to_vec(),
        Encoding::Utf8 { bom: true } => {
            let mut bytes = Vec::with_capacity(text.len() + 3);
            bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            bytes.extend_from_slice(text.as_bytes());
            bytes
        }
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let little = encoding == Encoding::Utf16Le;
            let mut bytes = Vec::with_capacity(2 + 2 * text.len());
            bytes.extend_from_slice(if little { &[0xFF, 0xFE] } else { &[0xFE, 0xFF] });
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            bytes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads `bytes` with a model that accepts every event.
    fn parse(bytes: &[u8], limits: &Limits) -> Result<Document, Error> {
        let mut nodes = limits.max_xml_nodes;
        read(bytes, limits, None, &mut nodes, |_, _| Ok(()))
    }

    fn error_of(bytes: &[u8]) -> XmlErrorKind {
        match parse(bytes, &Limits::default()) {
            Err(Error::Xml { entry: None, error }) => error.kind,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn counts_every_node_against_the_limit() {
        // An element, its attributes (namespace declarations included), runs of text, CDATA sections, comments and processing instructions each count as one node; end tags, and what comes before or after the root element, do not. The same count as bayan-opc's own parser before it read its metadata with bayan-xml.
        let text = "<!-- before --><a xmlns:p='urn:p' x='1'>text<!--c--><?pi?><b/><![CDATA[d]]></a>\n<?after?>";
        let limits = |nodes| Limits {
            max_xml_nodes: nodes,
            ..Limits::default()
        };
        assert!(parse(text.as_bytes(), &limits(8)).is_ok());
        assert_eq!(
            parse(text.as_bytes(), &limits(7)).map(|_| ()),
            Err(Error::Xml {
                entry: None,
                error: XmlError {
                    offset: text.find("<![CDATA[").unwrap(),
                    kind: XmlErrorKind::TooManyNodes
                }
            })
        );
        // Each node at the offset where it starts: here the second attribute.
        assert_eq!(
            parse(text.as_bytes(), &limits(2)).map(|_| ()),
            Err(Error::Xml {
                entry: None,
                error: XmlError {
                    offset: text.find("x='1'").unwrap(),
                    kind: XmlErrorKind::TooManyNodes
                }
            })
        );
        // A budget shared between parts is reduced by the nodes of each, and only by a part read without an error.
        let mut nodes = 20;
        let read_counted = |nodes: &mut usize| {
            read(
                text.as_bytes(),
                &Limits::default(),
                Some(3),
                nodes,
                |_, _| Ok(()),
            )
        };
        read_counted(&mut nodes).unwrap();
        assert_eq!(nodes, 12);
        read_counted(&mut nodes).unwrap();
        assert_eq!(nodes, 4);
        assert!(matches!(
            read_counted(&mut nodes),
            Err(Error::Xml {
                entry: Some(3),
                error: XmlError {
                    kind: XmlErrorKind::TooManyNodes,
                    ..
                }
            })
        ));
        assert_eq!(nodes, 4);
        // Text is one node however many references and line ends it holds.
        assert!(parse(b"<a>x&amp;y\r\nz&#65;</a>", &limits(2)).is_ok());
        assert!(parse(b"<a>x&amp;y\r\nz&#65;</a>", &limits(1)).is_err());
    }

    #[test]
    fn hands_every_event_to_the_model_with_its_depth() {
        let mut seen = Vec::new();
        let mut nodes = usize::MAX;
        read(
            b"<!--c--><a><b>t</b><!--d--></a>",
            &Limits::default(),
            None,
            &mut nodes,
            |event, depth| {
                let kind = match event {
                    Event::Start(_) => "start",
                    Event::End(_) => "end",
                    Event::Text(_) => "text",
                    Event::Comment(_) => "comment",
                    Event::CData(_) | Event::ProcessingInstruction(_) => "other",
                };
                seen.push((kind, depth));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            seen,
            [
                ("comment", 0),
                ("start", 1),
                ("start", 2),
                ("text", 2),
                ("end", 2),
                ("comment", 1),
                ("end", 1)
            ]
        );
    }

    #[test]
    fn xml_errors_come_before_the_models() {
        // The model breaks a rule at the first event; the part is still read to its end, where it turns out not to be XML, which is the error. The model sees nothing after its own error.
        let mut calls = 0;
        let mut nodes = usize::MAX;
        let model_error = Error::Package(crate::PackageError::ContentTypes(
            crate::ContentTypesError::UnexpectedRoot,
        ));
        let result = read(
            b"<a><b/><c/>",
            &Limits::default(),
            None,
            &mut nodes,
            |_, _| {
                calls += 1;
                Err(model_error.clone())
            },
        );
        assert!(matches!(
            result,
            Err(Error::Xml {
                error: XmlError {
                    kind: XmlErrorKind::UnexpectedEnd,
                    ..
                },
                ..
            })
        ));
        assert_eq!(calls, 1);
        // Too many nodes come first too.
        let limits = Limits {
            max_xml_nodes: 2,
            ..Limits::default()
        };
        let mut nodes = limits.max_xml_nodes;
        let result = read(b"<a><b/><c/></a>", &limits, None, &mut nodes, |_, _| {
            Err(model_error.clone())
        });
        assert!(matches!(
            result,
            Err(Error::Xml {
                error: XmlError {
                    kind: XmlErrorKind::TooManyNodes,
                    ..
                },
                ..
            })
        ));
        // Without one, the model's first error is the part's error.
        let mut nodes = usize::MAX;
        assert_eq!(
            read(b"<a/>", &Limits::default(), None, &mut nodes, |_, _| Err(
                model_error.clone()
            ))
            .map(|_| ()),
            Err(model_error)
        );
    }

    #[test]
    fn refuses_what_bayan_xml_refuses_with_bayan_opcs_errors() {
        let cases: [(&[u8], XmlErrorKind); 37] = [
            (b"", XmlErrorKind::ContentOutsideRoot),
            (b"text", XmlErrorKind::ContentOutsideRoot),
            (b"<a/><b/>", XmlErrorKind::ContentOutsideRoot),
            (b"<a/>text", XmlErrorKind::ContentOutsideRoot),
            (b"<a>", XmlErrorKind::UnexpectedEnd),
            (b"<a></b>", XmlErrorKind::MismatchedEndTag),
            (b"<a x='1' x='2'/>", XmlErrorKind::DuplicateAttribute),
            (
                b"<a xmlns:p='u' xmlns:q='u' p:x='1' q:x='2'/>",
                XmlErrorKind::DuplicateAttribute,
            ),
            (b"<p:a/>", XmlErrorKind::NamespaceError),
            (b"<a p:x='1'/>", XmlErrorKind::NamespaceError),
            (b"<a xmlns:p=''/>", XmlErrorKind::NamespaceError),
            (b"<a xmlns:xmlns='u'/>", XmlErrorKind::NamespaceError),
            (b"<a xmlns:xml='u'/>", XmlErrorKind::NamespaceError),
            (b"<a x=1/>", XmlErrorKind::Malformed),
            (b"<a x='<'/>", XmlErrorKind::Malformed),
            (b"<a x='1'y='2'/>", XmlErrorKind::Malformed),
            (b"<a>]]></a>", XmlErrorKind::Malformed),
            (b"<a><!-- a -- b --></a>", XmlErrorKind::Malformed),
            (b"<a><?xml x?></a>", XmlErrorKind::Malformed),
            (b" <?xml version='1.0'?><a/>", XmlErrorKind::Malformed),
            (b"<a:b:c xmlns:a='u'/>", XmlErrorKind::Malformed),
            (b"<r>& b</r>", XmlErrorKind::Malformed),
            (b"<r>&amp</r>", XmlErrorKind::Malformed),
            (b"<r><!ENTITY x 'y'></r>", XmlErrorKind::Malformed),
            (
                b"<?xml version='2.0'?><a/>",
                XmlErrorKind::MalformedDeclaration,
            ),
            (
                b"<?xml encoding='UTF-8'?><a/>",
                XmlErrorKind::MalformedDeclaration,
            ),
            (
                b"<?xml version='1.0' standalone='maybe'?><a/>",
                XmlErrorKind::MalformedDeclaration,
            ),
            (b"<a>\x01</a>", XmlErrorKind::InvalidCharacter),
            (
                b"<?xml version=\"1.0\"?><!DOCTYPE lolz [<!ENTITY lol \"lol\"><!ENTITY lol2 \"&lol;&lol;&lol;&lol;&lol;\">]><lolz>&lol2;</lolz>",
                XmlErrorKind::DoctypeNotAllowed,
            ),
            (
                b"<!DOCTYPE r [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><r>&x;</r>",
                XmlErrorKind::DoctypeNotAllowed,
            ),
            (b"<r>&x;</r>", XmlErrorKind::UndefinedEntity),
            (b"<r a='&x;'/>", XmlErrorKind::UndefinedEntity),
            (b"<r>&#0;</r>", XmlErrorKind::InvalidCharacterReference),
            (b"<r>&#xD800;</r>", XmlErrorKind::InvalidCharacterReference),
            (b"<r>&#x110000;</r>", XmlErrorKind::InvalidCharacterReference),
            (
                b"<?xml version='1.0' encoding='ISO-8859-1'?><a/>",
                XmlErrorKind::UnsupportedEncoding,
            ),
            (b"<a>\xC3</a>", XmlErrorKind::InvalidEncoding),
        ];
        for (bytes, kind) in cases {
            assert_eq!(error_of(bytes), kind, "{}", String::from_utf8_lossy(bytes));
        }
    }

    #[test]
    fn decodes_utf16_and_byte_order_marks_and_encodes_them_back() {
        let text = "<?xml version='1.0' encoding='UTF-16'?><a>é 中</a>";
        for encoding in [Encoding::Utf16Le, Encoding::Utf16Be] {
            let bytes = encode(text, encoding);
            let document = parse(&bytes, &Limits::default()).unwrap();
            assert_eq!(document.encoding, encoding);
            assert_eq!(document.text, text);
            assert_eq!(encode(&document.text, document.encoding), bytes);
        }
        for bom in [false, true] {
            let bytes = encode("<a>é</a>", Encoding::Utf8 { bom });
            let document = parse(&bytes, &Limits::default()).unwrap();
            assert_eq!(document.encoding, Encoding::Utf8 { bom });
            assert_eq!(document.text, "<a>é</a>");
            assert_eq!(encode(&document.text, document.encoding), bytes);
        }
        assert_eq!(
            error_of(&encode(
                "<?xml version='1.0' encoding='UTF-8'?><a/>",
                Encoding::Utf16Le
            )),
            XmlErrorKind::UnsupportedEncoding
        );
        assert_eq!(
            error_of(b"<?xml version='1.0' encoding='UTF-16'?><a/>"),
            XmlErrorKind::UnsupportedEncoding
        );
        assert_eq!(error_of(&[0xFF, 0xFE, 0x3C]), XmlErrorKind::InvalidEncoding);
        assert_eq!(
            error_of(&[0xFF, 0xFE, 0x00, 0xD8, 0x3C, 0x00]),
            XmlErrorKind::InvalidEncoding
        );
    }

    #[test]
    fn applies_bayan_opcs_limits() {
        let limits = Limits {
            max_xml_depth: 3,
            max_xml_attributes: 2,
            max_xml_name_length: 4,
            max_metadata_size: 64,
            ..Limits::default()
        };
        let error = |bytes: &[u8]| match parse(bytes, &limits) {
            Err(Error::Xml { error, .. }) => error.kind,
            other => panic!("{other:?}"),
        };
        assert!(parse(b"<a><b><c/></b></a>", &limits).is_ok());
        assert_eq!(error(b"<a><b><c><d/></c></b></a>"), XmlErrorKind::TooDeep);
        assert!(parse(b"<a x='1' y='2'/>", &limits).is_ok());
        assert_eq!(
            error(b"<a x='1' y='2' z='3'/>"),
            XmlErrorKind::TooManyAttributes
        );
        assert!(parse(b"<abcd/>", &limits).is_ok());
        assert_eq!(error(b"<abcde/>"), XmlErrorKind::NameTooLong);
        // The size is checked before anything is decoded, with bayan-opc's own error.
        let large = format!("<a>{}</a>", "x".repeat(58));
        assert!(parse(&large.as_bytes()[..64], &limits).is_err());
        assert_eq!(
            parse(large.as_bytes(), &limits).map(|_| ()),
            Err(Error::Limit(LimitError::MetadataTooLarge {
                entry: None,
                limit: 64
            }))
        );
        // Deep nesting is refused without recursion, so even a deep limit cannot overflow the stack.
        let deep = "<a>".repeat(100_000);
        assert_eq!(error_of(deep.as_bytes()), XmlErrorKind::TooDeep);
        // No part is held as a tree, so a depth limit above bayan-xml's default of 256 applies as it is.
        let nested = |depth: usize| format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
        let deeper = Limits {
            max_xml_depth: 1_000,
            ..Limits::default()
        };
        assert!(parse(nested(1_000).as_bytes(), &deeper).is_ok());
        assert!(parse(nested(1_001).as_bytes(), &deeper).is_err());
        // bayan-xml's own limits on namespaces are not a second budget: the node budget and the part's size bound them already.
        let long = format!("<a xmlns='urn:{}'/>", "u".repeat(4_096));
        assert!(parse(long.as_bytes(), &Limits::default()).is_ok());
        let many: String = (0..5_000)
            .map(|index| format!("<b xmlns='urn:{index}'/>"))
            .collect();
        assert!(parse(format!("<a>{many}</a>").as_bytes(), &Limits::default()).is_ok());
    }

    #[test]
    fn escapes_values_so_that_they_read_back_the_same() {
        let value = "a<b>&\"'\t\n\r c";
        let mut attribute = String::new();
        escape_attribute(value, &mut attribute);
        let mut text = String::new();
        escape_text(value, &mut text);
        let xml = format!("<r a=\"{attribute}\">{text}</r>");
        let (mut read_attribute, mut read_text) = (None, String::new());
        let mut nodes = usize::MAX;
        read(
            xml.as_bytes(),
            &Limits::default(),
            None,
            &mut nodes,
            |event, _| {
                match event {
                    Event::Start(tag) => {
                        read_attribute = tag.attribute(None, "a").map(|a| a.value().to_owned());
                    }
                    Event::Text(run) => read_text.push_str(run.value()),
                    _ => {}
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(read_attribute.as_deref(), Some(value));
        assert_eq!(read_text, value);
    }

    #[test]
    fn judges_names_and_characters_as_xml_does() {
        for name in ["rId1", "_a", "a-b.c7", "é", "a·b"] {
            assert!(is_ncname(name), "{name}");
        }
        for name in ["", "1a", "a b", "a:b", "-a", "a\u{1}", ".a", ":"] {
            assert!(!is_ncname(name), "{name:?}");
        }
        for character in [
            '\t',
            '\n',
            '\r',
            ' ',
            'a',
            '\u{D7FF}',
            '\u{E000}',
            '\u{FFFD}',
            '\u{10000}',
            '\u{10FFFF}',
        ] {
            assert!(is_xml_character(character), "{:X}", u32::from(character));
        }
        for character in ['\u{0}', '\u{8}', '\u{B}', '\u{1F}', '\u{FFFE}', '\u{FFFF}'] {
            assert!(!is_xml_character(character), "{:X}", u32::from(character));
        }
    }
}
