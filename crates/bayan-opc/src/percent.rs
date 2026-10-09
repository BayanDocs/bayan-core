//! Percent-encoding of non-ASCII characters, as ECMA-376 Part 2 maps part names to ZIP entry names and back (§7.3.4, §7.3.5).
//!
//! ZIP entry names in a package are ASCII: a part name's non-ASCII characters are written as the percent-encoded bytes of their UTF-8 form (`é` becomes `%C3%A9`). Reading reverses only that: percent-encoded sequences that spell a non-ASCII character are decoded, while percent-encoded ASCII (such as `%20`) stays as it is, because it is part of the part name itself.

use std::borrow::Cow;

/// Replaces every percent-encoded sequence that spells a non-ASCII character in UTF-8 by that character. Everything else, including percent-encoded ASCII, malformed sequences and sequences that are not valid UTF-8, is left as it is.
pub(crate) fn decode_non_ascii(text: &str) -> Cow<'_, str> {
    if !text.contains('%') {
        return Cow::Borrowed(text);
    }
    let bytes = text.as_bytes();
    let mut decoded = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        if byte == b'%'
            && let Some((character, length)) = decode_character(bytes, at)
        {
            decoded.push(character);
            at += length;
            continue;
        }
        // `at` is always at the start of a character: it starts at 0 and moves by whole characters or by whole percent-encoded sequences, which are ASCII.
        let Some(character) = text.get(at..).and_then(|rest| rest.chars().next()) else {
            break;
        };
        decoded.push(character);
        at += character.len_utf8();
    }
    Cow::Owned(decoded)
}

/// The non-ASCII character spelled by the percent-encoded sequence at `at`, and the number of bytes the sequence takes, if there is one.
pub(crate) fn decode_character(bytes: &[u8], at: usize) -> Option<(char, usize)> {
    let lead = encoded_byte(bytes, at)?;
    let length = match lead {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return None,
    };
    let mut buffer = [lead, 0, 0, 0];
    for (index, slot) in buffer.iter_mut().enumerate().take(length).skip(1) {
        *slot = encoded_byte(bytes, at + 3 * index)?;
    }
    // `from_utf8` rejects overlong forms, surrogates and continuation bytes in the wrong place.
    let character = std::str::from_utf8(buffer.get(..length)?)
        .ok()?
        .chars()
        .next()?;
    Some((character, 3 * length))
}

/// The byte encoded by `%XY` at `at`.
fn encoded_byte(bytes: &[u8], at: usize) -> Option<u8> {
    if *bytes.get(at)? != b'%' {
        return None;
    }
    let high = hex_digit(*bytes.get(at + 1)?)?;
    let low = hex_digit(*bytes.get(at + 2)?)?;
    Some(high << 4 | low)
}

/// The value of one hexadecimal digit.
pub(crate) fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Percent-encodes every non-ASCII character as the bytes of its UTF-8 form, with upper-case hexadecimal digits.
pub(crate) fn encode_non_ascii(text: &str) -> Cow<'_, str> {
    if text.is_ascii() {
        return Cow::Borrowed(text);
    }
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_ascii() {
            encoded.push(character);
        } else {
            let mut utf8 = [0_u8; 4];
            for &byte in character.encode_utf8(&mut utf8).as_bytes() {
                encoded.push('%');
                encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
                encoded.push(char::from(DIGITS[usize::from(byte & 0x0F)]));
            }
        }
    }
    Cow::Owned(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_only_non_ascii_characters() {
        assert_eq!(decode_non_ascii("word/document.xml"), "word/document.xml");
        assert_eq!(decode_non_ascii("m%C3%A9dia/%E2%82%AC.png"), "média/€.png");
        assert_eq!(decode_non_ascii("m%c3%a9dia"), "média");
        assert_eq!(decode_non_ascii("%F0%9F%98%80"), "😀");
        // Percent-encoded ASCII is part of the name and stays encoded.
        assert_eq!(decode_non_ascii("a%20b%41"), "a%20b%41");
        // Malformed or invalid sequences stay as they are.
        assert_eq!(decode_non_ascii("%"), "%");
        assert_eq!(decode_non_ascii("%C3"), "%C3");
        assert_eq!(decode_non_ascii("%C3%"), "%C3%");
        assert_eq!(decode_non_ascii("%C3%28"), "%C3%28");
        assert_eq!(decode_non_ascii("%C0%AF"), "%C0%AF");
        assert_eq!(decode_non_ascii("%ED%A0%80"), "%ED%A0%80");
        assert_eq!(decode_non_ascii("%FF%zz"), "%FF%zz");
        // Characters that are already non-ASCII stay as they are.
        assert_eq!(decode_non_ascii("é%C3%A9"), "éé");
    }

    #[test]
    fn encodes_only_non_ascii_characters() {
        assert_eq!(encode_non_ascii("word/document.xml"), "word/document.xml");
        assert_eq!(encode_non_ascii("média/€.png"), "m%C3%A9dia/%E2%82%AC.png");
        assert_eq!(encode_non_ascii("a%20b"), "a%20b");
        for text in ["média/€.png", "😀", "plain", "a%41é"] {
            assert_eq!(decode_non_ascii(&encode_non_ascii(text)), text);
        }
    }
}
