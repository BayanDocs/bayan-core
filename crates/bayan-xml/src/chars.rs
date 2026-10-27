//! XML's character classes (XML 1.0 Fifth Edition §2.2, §2.3) and qualified names (Namespaces in XML 1.0 Third Edition §3, §4).

/// Whether `character` is one that XML allows (production [2] `Char`): tab, line feed, carriage return and everything from U+0020 except the surrogates, U+FFFE and U+FFFF.
pub(crate) fn is_char(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}'
    )
}

/// The byte offset of the first character that XML does not allow, if `text` has one.
pub(crate) fn find_invalid_char(text: &str) -> Option<usize> {
    // `text` is valid UTF-8, so it holds no surrogates; what remains to find are the control characters (single bytes below 0x20 other than tab, line feed and carriage return) and U+FFFE and U+FFFF (EF BF BE and EF BF BF).
    let bytes = text.as_bytes();
    bytes.iter().enumerate().find_map(|(offset, &byte)| {
        let invalid = match byte {
            b'\t' | b'\n' | b'\r' => false,
            0x00..=0x1F => true,
            0xEF => matches!(bytes.get(offset + 1..offset + 3), Some([0xBF, 0xBE | 0xBF])),
            _ => false,
        };
        invalid.then_some(offset)
    })
}

/// Whether `byte` is XML white space (production [3] `S`): space, tab, carriage return or line feed.
pub(crate) const fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

/// Whether `character` may start a name (production [4] `NameStartChar`).
pub(crate) fn is_name_start_char(character: char) -> bool {
    matches!(
        character,
        ':' | 'A'..='Z'
            | '_'
            | 'a'..='z'
            | '\u{C0}'..='\u{D6}'
            | '\u{D8}'..='\u{F6}'
            | '\u{F8}'..='\u{2FF}'
            | '\u{370}'..='\u{37D}'
            | '\u{37F}'..='\u{1FFF}'
            | '\u{200C}'..='\u{200D}'
            | '\u{2070}'..='\u{218F}'
            | '\u{2C00}'..='\u{2FEF}'
            | '\u{3001}'..='\u{D7FF}'
            | '\u{F900}'..='\u{FDCF}'
            | '\u{FDF0}'..='\u{FFFD}'
            | '\u{10000}'..='\u{EFFFF}'
    )
}

/// Whether `character` may continue a name (production [4a] `NameChar`).
pub(crate) fn is_name_char(character: char) -> bool {
    is_name_start_char(character)
        || matches!(
            character,
            '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}'
        )
}

/// Whether `text` is an XML name (production [5] `Name`).
pub(crate) fn is_name(text: &str) -> bool {
    let mut characters = text.chars();
    characters.next().is_some_and(is_name_start_char) && characters.all(is_name_char)
}

/// Whether `text` is an `NCName`: a name without a colon (Namespaces in XML, production [4]).
pub(crate) fn is_ncname(text: &str) -> bool {
    !text.contains(':') && is_name(text)
}

/// Splits a qualified name into its prefix and its local part: `("", "p")` for `p` and `("w", "p")` for `w:p`. `None` if `text` is not a qualified name (Namespaces in XML, production [7] `QName`), for example `:p`, `w:`, `a:b:c` or anything that is not a name.
pub(crate) fn split_qname(text: &str) -> Option<(&str, &str)> {
    match text.split_once(':') {
        None => is_ncname(text).then_some(("", text)),
        Some((prefix, local)) => (is_ncname(prefix) && is_ncname(local)).then_some((prefix, local)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_the_characters_of_the_char_production() {
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
            '\u{FDD0}',
            '\u{1FFFE}',
            '\u{85}',
        ] {
            assert!(is_char(character), "U+{:04X}", u32::from(character));
        }
        for character in [
            '\0', '\u{1}', '\u{8}', '\u{B}', '\u{C}', '\u{E}', '\u{1F}', '\u{FFFE}', '\u{FFFF}',
        ] {
            assert!(!is_char(character), "U+{:04X}", u32::from(character));
        }
    }

    #[test]
    fn finds_the_first_character_xml_does_not_allow() {
        assert_eq!(find_invalid_char("<a>tab\tlf\ncr\r</a>"), None);
        assert_eq!(find_invalid_char("ab\u{0}"), Some(2));
        assert_eq!(find_invalid_char("é\u{1F}"), Some(2));
        assert_eq!(find_invalid_char("x\u{FFFE}y"), Some(1));
        assert_eq!(find_invalid_char("x\u{FFFF}"), Some(1));
        // U+FFFD and U+EFBF... start with the same byte as U+FFFE and U+FFFF but are allowed.
        assert_eq!(find_invalid_char("\u{FFFD}\u{EFBF}\u{FEFF}\u{FFBE}"), None);
        assert_eq!(find_invalid_char(""), None);
    }

    #[test]
    fn knows_xml_white_space() {
        for byte in *b" \t\r\n" {
            assert!(is_space(byte));
        }
        for byte in [b'\x0B', b'\x0C', b'a', 0xA0] {
            assert!(!is_space(byte));
        }
    }

    #[test]
    fn knows_the_name_characters() {
        for character in [
            ':',
            'A',
            'z',
            '_',
            '\u{C0}',
            '\u{37F}',
            '\u{200C}',
            '\u{3001}',
            '\u{10000}',
            '\u{EFFFF}',
        ] {
            assert!(
                is_name_start_char(character),
                "U+{:04X}",
                u32::from(character)
            );
            assert!(is_name_char(character), "U+{:04X}", u32::from(character));
        }
        for character in [
            '-', '.', '0', '9', '\u{B7}', '\u{300}', '\u{36F}', '\u{203F}', '\u{2040}',
        ] {
            assert!(
                !is_name_start_char(character),
                "U+{:04X}",
                u32::from(character)
            );
            assert!(is_name_char(character), "U+{:04X}", u32::from(character));
        }
        for character in [
            ' ',
            '<',
            '>',
            '&',
            '"',
            '\'',
            '=',
            '/',
            '\u{D7}',
            '\u{F7}',
            '\u{37E}',
            '\u{2000}',
            '\u{F0000}',
        ] {
            assert!(
                !is_name_start_char(character),
                "U+{:04X}",
                u32::from(character)
            );
            assert!(!is_name_char(character), "U+{:04X}", u32::from(character));
        }
    }

    #[test]
    fn checks_names_and_ncnames() {
        for name in ["a", "w:p", "_x", "a-b.c", "é", "x\u{B7}y", "a:b:c", ":a"] {
            assert!(is_name(name), "name {name:?}");
        }
        for name in ["", "1a", "-a", ".a", "a b", "a>"] {
            assert!(!is_name(name), "name {name:?}");
        }
        for name in ["a", "_x", "a-b.c", "é"] {
            assert!(is_ncname(name), "ncname {name:?}");
        }
        for name in ["", "w:p", ":a", "a:", "1a"] {
            assert!(!is_ncname(name), "ncname {name:?}");
        }
    }

    #[test]
    fn splits_qualified_names() {
        assert_eq!(split_qname("p"), Some(("", "p")));
        assert_eq!(split_qname("w:p"), Some(("w", "p")));
        assert_eq!(split_qname("w14:paraId"), Some(("w14", "paraId")));
        for name in ["", ":p", "w:", "a:b:c", "1:p", "w:1", "a b", "w::p"] {
            assert_eq!(split_qname(name), None, "{name:?}");
        }
    }
}
