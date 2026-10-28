//! Decoding a part into text: byte order marks, UTF-8 and UTF-16, characters XML does not allow, the size limit and the XML declaration.

use bayan_xml::{Encoding, ErrorKind, Limits, Source};

fn utf16(text: &str, little_endian: bool) -> Vec<u8> {
    let mut bytes = if little_endian {
        vec![0xFF, 0xFE]
    } else {
        vec![0xFE, 0xFF]
    };
    for unit in text.encode_utf16() {
        let pair = if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        };
        bytes.extend_from_slice(&pair);
    }
    bytes
}

fn refusal(bytes: &[u8]) -> (ErrorKind, usize) {
    let error = Source::new(bytes, &Limits::default()).unwrap_err();
    (error.kind(), error.offset())
}

#[test]
fn reads_utf8_with_and_without_a_byte_order_mark() {
    let source = Source::new(b"<a>x</a>", &Limits::default()).unwrap();
    assert_eq!(source.text(), "<a>x</a>");
    assert_eq!(source.encoding(), Encoding::Utf8 { bom: false });
    let source = Source::new(b"\xEF\xBB\xBF<a>x</a>", &Limits::default()).unwrap();
    // The text, and every offset into it, starts after the byte order mark.
    assert_eq!(source.text(), "<a>x</a>");
    assert_eq!(source.encoding(), Encoding::Utf8 { bom: true });
}

#[test]
fn reads_utf16_in_both_byte_orders() {
    let text = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><a>é中\u{1F600}</a>";
    for little_endian in [true, false] {
        let bytes = utf16(text, little_endian);
        let source = Source::new(&bytes, &Limits::default()).unwrap();
        assert_eq!(source.text(), text);
        let expected = if little_endian {
            Encoding::Utf16Le
        } else {
            Encoding::Utf16Be
        };
        assert_eq!(source.encoding(), expected);
    }
}

#[test]
fn refuses_input_larger_than_the_limit() {
    let limits = Limits {
        max_size: 8,
        ..Limits::default()
    };
    assert!(Source::new(b"<a>xx</a", &limits).is_ok());
    let error = Source::new(b"<a>xxx</a", &limits).unwrap_err();
    assert_eq!((error.kind(), error.offset()), (ErrorKind::TooLarge, 0));
    // The limit counts the bytes as they come, before UTF-16 is decoded.
    let bytes = utf16("<a/>", true);
    let limits = Limits {
        max_size: bytes.len() - 1,
        ..Limits::default()
    };
    assert_eq!(
        Source::new(&bytes, &limits).unwrap_err().kind(),
        ErrorKind::TooLarge
    );
}

#[test]
fn refuses_bytes_that_are_not_their_encoding() {
    assert_eq!(refusal(b"<a>\xC3</a>"), (ErrorKind::InvalidEncoding, 3));
    assert_eq!(
        refusal(b"<a>\xED\xA0\x80</a>"),
        (ErrorKind::InvalidEncoding, 3)
    );
    // An unpaired surrogate, and an odd number of bytes, in UTF-16.
    let mut bytes = utf16("<a>", true);
    bytes.extend_from_slice(&[0x00, 0xD8, b'<', 0x00]);
    assert_eq!(refusal(&bytes).0, ErrorKind::InvalidEncoding);
    let mut bytes = utf16("<a/>", false);
    bytes.push(0);
    assert_eq!(refusal(&bytes).0, ErrorKind::InvalidEncoding);
}

#[test]
fn refuses_characters_xml_does_not_allow() {
    assert_eq!(refusal(b"<a>\x00</a>"), (ErrorKind::InvalidCharacter, 3));
    assert_eq!(refusal(b"<a>\x01</a>"), (ErrorKind::InvalidCharacter, 3));
    assert_eq!(refusal(b"<a>\x1F</a>"), (ErrorKind::InvalidCharacter, 3));
    assert_eq!(
        refusal("<a>\u{FFFE}</a>".as_bytes()),
        (ErrorKind::InvalidCharacter, 3)
    );
    assert_eq!(
        refusal("<a x=\"\u{FFFF}\"/>".as_bytes()),
        (ErrorKind::InvalidCharacter, 6)
    );
    // The offset is in the decoded text, after the byte order mark.
    assert_eq!(
        refusal(b"\xEF\xBB\xBF<a>\x0B</a>"),
        (ErrorKind::InvalidCharacter, 3)
    );
    assert_eq!(
        refusal(&utf16("<a>é\u{8}</a>", false)),
        (ErrorKind::InvalidCharacter, 5)
    );
    // UTF-16 without a byte order mark reads as UTF-8 full of NUL characters.
    assert_eq!(
        refusal(b"<\x00a\x00/\x00>\x00").0,
        ErrorKind::InvalidCharacter
    );
}

#[test]
fn reads_the_xml_declaration() {
    let text = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<a/>";
    let source = Source::new(text.as_bytes(), &Limits::default()).unwrap();
    let declaration = source.declaration().unwrap();
    assert_eq!(declaration.span(), 0..55);
    assert_eq!(declaration.standalone(), Some(true));
    assert!(declaration.declares_encoding());
    for (text, standalone, encoding) in [
        ("<?xml version='1.0'?><a/>", None, false),
        ("<?xml version = \"1.0\" ?><a/>", None, false),
        (
            "<?xml version=\"1.0\" standalone='no'?><a/>",
            Some(false),
            false,
        ),
        (
            "<?xml\tversion=\"1.0\"\nencoding=\"utf-8\"\r\n?><a/>",
            None,
            true,
        ),
    ] {
        let source = Source::new(text.as_bytes(), &Limits::default()).unwrap();
        let declaration = source.declaration().unwrap();
        assert_eq!(declaration.standalone(), standalone, "{text}");
        assert_eq!(declaration.declares_encoding(), encoding, "{text}");
    }
    assert!(
        Source::new(b"<a/>", &Limits::default())
            .unwrap()
            .declaration()
            .is_none()
    );
    // A processing instruction whose target starts with "xml" is not a declaration.
    let source = Source::new(b"<?xml-stylesheet href=\"s\"?><a/>", &Limits::default()).unwrap();
    assert!(source.declaration().is_none());
    // Only a declaration at the very start is one; anything later is for the reader to refuse.
    assert!(
        Source::new(b" <?xml version=\"1.0\"?><a/>", &Limits::default())
            .unwrap()
            .declaration()
            .is_none()
    );
}

#[test]
fn refuses_malformed_declarations() {
    for text in [
        "<?xml?><a/>",
        "<?xml ?><a/>",
        "<?xml version=\"1.1\"?><a/>",
        "<?xml version=\"1.0 \"?><a/>",
        "<?xml version='1.0\"?><a/>",
        "<?xml version=1.0?><a/>",
        "<?xml version\"1.0\"?><a/>",
        "<?xml encoding=\"UTF-8\" version=\"1.0\"?><a/>",
        "<?xml version=\"1.0\" standalone=\"yes\" encoding=\"UTF-8\"?><a/>",
        "<?xml version=\"1.0\"standalone=\"yes\"?><a/>",
        "<?xml version=\"1.0\" standalone=\"maybe\"?><a/>",
        "<?xml version=\"1.0\" standalone=\"YES\"?><a/>",
        "<?xml version=\"1.0\" foo=\"bar\"?><a/>",
        "<?xml version=\"1.0\" version=\"1.0\"?><a/>",
        "<?xml VERSION=\"1.0\"?><a/>",
        "<?xml version=\"1.0\"? ><a/>",
    ] {
        assert_eq!(
            refusal(text.as_bytes()).0,
            ErrorKind::MalformedDeclaration,
            "{text}"
        );
    }
    for text in ["<?xml version=\"1.0", "<?xml version=\"1.0\"", "<?xml"] {
        assert_eq!(
            refusal(text.as_bytes()).0,
            ErrorKind::UnexpectedEnd,
            "{text}"
        );
    }
}

#[test]
fn refuses_each_declaration_field_as_soon_as_it_is_read() {
    // Each declaration has a field it may not have, followed by the end of the text or, in the last case, by a syntax error, so it is refused at that field's value; a reader that collected the fields before checking them would reach the later problem first and report that instead (finding M3 of the architecture review of 2026-10-27).
    for (text, kind, offset) in [
        ("<?xml version=\"1.1\"", ErrorKind::MalformedDeclaration, 15),
        (
            "<?xml encoding=\"UTF-8\"",
            ErrorKind::MalformedDeclaration,
            16,
        ),
        (
            "<?xml version=\"1.0\" =\"\"",
            ErrorKind::MalformedDeclaration,
            22,
        ),
        (
            "<?xml version=\"1.0\" version=\"1.0\"",
            ErrorKind::MalformedDeclaration,
            29,
        ),
        (
            "<?xml version=\"1.0\" encoding=\"UTF-16\"",
            ErrorKind::UnsupportedEncoding,
            30,
        ),
        (
            "<?xml version=\"1.0\" standalone=\"maybe\"",
            ErrorKind::MalformedDeclaration,
            32,
        ),
        (
            "<?xml version=\"1.0\" standalone=\"yes\" encoding=\"UTF-8\"",
            ErrorKind::MalformedDeclaration,
            47,
        ),
        (
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\" standalone=\"no\"",
            ErrorKind::MalformedDeclaration,
            65,
        ),
        (
            "<?xml version=\"1.1\" x?><a/>",
            ErrorKind::MalformedDeclaration,
            15,
        ),
    ] {
        assert_eq!(refusal(text.as_bytes()), (kind, offset), "{text}");
    }
}

#[test]
fn refuses_encodings_other_than_the_bytes() {
    for text in [
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?><a/>",
        "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a/>",
        "<?xml version=\"1.0\" encoding=\"UTF8\"?><a/>",
        "<?xml version=\"1.0\" encoding=\"\"?><a/>",
    ] {
        assert_eq!(
            refusal(text.as_bytes()).0,
            ErrorKind::UnsupportedEncoding,
            "{text}"
        );
    }
    let bytes = utf16("<?xml version=\"1.0\" encoding=\"UTF-8\"?><a/>", true);
    assert_eq!(refusal(&bytes).0, ErrorKind::UnsupportedEncoding);
    // UTF-16 needs no encoding declaration, and may name itself in any case.
    assert!(
        Source::new(
            &utf16("<?xml version=\"1.0\"?><a/>", true),
            &Limits::default()
        )
        .is_ok()
    );
    assert!(
        Source::new(
            &utf16("<?xml version=\"1.0\" encoding=\"utf-16\"?><a/>", false),
            &Limits::default()
        )
        .is_ok()
    );
}
