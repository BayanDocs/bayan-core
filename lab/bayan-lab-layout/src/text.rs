//! The comparable form of a line's text: what the comparisons of LAB-004 align, so that the two sides of a comparison agree wherever they show the same thing.
//!
//! Producers write a line's `text` as `lab/schema/layout-json.md` says: the characters the line displays, in reading order. Some characters still differ between producers without the page looking any different: a PDF may draw a space as a glyph or leave a gap, a tab is a gap in a PDF and a character in the engine's text, a ligature may be the letters it stands for or one compatibility character, and a displayed hyphen has several spellings. [`comparable`] removes those differences and nothing else.

/// The comparable form of `text`:
///
/// 1. whitespace (Unicode's `White_Space` property: spaces, tabs, no-break spaces, line and paragraph separators) is removed, because a PDF shows it as a gap or a glyph at the producer's choice;
/// 2. characters that display nothing are removed: the zero-width space, joiner and non-joiner (U+200B–U+200D), the word joiner and invisible operators (U+2060–U+2064), the byte order mark (U+FEFF) and the bidirectional formatting characters (U+061C, U+200E, U+200F, U+202A–U+202E, U+2066–U+2069);
/// 3. the soft hyphen, hyphen and non-breaking hyphen (U+00AD, U+2010, U+2011), which display as a hyphen, become the hyphen-minus (U+002D);
/// 4. the Latin compatibility ligatures (U+FB00–U+FB06) become the letters they stand for.
///
/// Everything else stays as it is: no case folding and no Unicode normalization.
#[must_use]
pub fn comparable(text: &str) -> String {
    comparable_chars(text)
        .map(|(_, character)| character)
        .collect()
}

/// The characters of [`comparable`]`(text)`, each with the index of the character of `text` it comes from, counted in characters (Unicode scalar values) as the offsets of a layout JSON are. A ligature such as U+FB01 gives two characters with the same index; a removed character gives none.
///
/// The comparisons of LAB-004 use the indices to find the glyph that shows each character they align.
pub fn comparable_chars(text: &str) -> impl Iterator<Item = (usize, char)> + '_ {
    text.chars()
        .enumerate()
        .flat_map(|(index, character)| replacement(character).map(move |c| (index, c)))
}

/// What one character of a line's text becomes in the comparable form: nothing, itself, or the letters of a ligature.
fn replacement(character: char) -> impl Iterator<Item = char> {
    let letters: &[char] = match character {
        _ if character.is_whitespace() || displays_nothing(character) => &[],
        '\u{AD}' | '\u{2010}' | '\u{2011}' => &['-'],
        '\u{FB00}' => &['f', 'f'],
        '\u{FB01}' => &['f', 'i'],
        '\u{FB02}' => &['f', 'l'],
        '\u{FB03}' => &['f', 'f', 'i'],
        '\u{FB04}' => &['f', 'f', 'l'],
        '\u{FB05}' => &['\u{17F}', 't'],
        '\u{FB06}' => &['s', 't'],
        _ => return Replacement::One(Some(character)),
    };
    Replacement::Many(letters.iter())
}

/// The characters that one character becomes.
enum Replacement {
    /// The character itself, until it has been taken.
    One(Option<char>),
    /// Nothing, or the letters of a ligature or the hyphen.
    Many(core::slice::Iter<'static, char>),
}

impl Iterator for Replacement {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        match self {
            Self::One(character) => character.take(),
            Self::Many(letters) => letters.next().copied(),
        }
    }
}

/// The format characters that rule 2 of [`comparable`] removes.
const fn displays_nothing(character: char) -> bool {
    matches!(
        character,
        '\u{061C}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::{comparable, comparable_chars};

    #[test]
    fn every_comparable_character_knows_where_it_comes_from() {
        let pairs: Vec<(usize, char)> = comparable_chars("a \u{FB01}\u{AD}\u{200B}b").collect();
        assert_eq!(
            pairs,
            [(0, 'a'), (2, 'f'), (2, 'i'), (3, '-'), (5, 'b')],
            "the space and the zero-width space give nothing, the ligature two letters of one source character"
        );
        assert_eq!(comparable_chars("").count(), 0);
    }

    #[test]
    fn whitespace_and_invisible_characters_are_removed() {
        assert_eq!(comparable("Hello world"), "Helloworld");
        assert_eq!(comparable("Name\tValue\u{A0}1"), "NameValue1");
        assert_eq!(comparable("a\u{200B}b\u{200D}c\u{FEFF}"), "abc");
        assert_eq!(
            comparable("\u{202B}\u{5E9}\u{5DC}\u{5D5}\u{5DD}\u{202C} \u{200F}"),
            "\u{5E9}\u{5DC}\u{5D5}\u{5DD}"
        );
    }

    #[test]
    fn hyphens_and_ligatures_have_one_spelling() {
        assert_eq!(
            comparable("co\u{AD}op re\u{2010}enter non\u{2011}stop"),
            "co-opre-enternon-stop"
        );
        assert_eq!(
            comparable("\u{FB01}nd \u{FB02}uent \u{FB06}"),
            "findfluentst"
        );
        assert_eq!(
            comparable("\u{FB00} \u{FB03} \u{FB04} \u{FB05}"),
            "ffffiffl\u{17F}t"
        );
    }

    #[test]
    fn everything_else_is_kept() {
        let text = "Ünïcode \u{3042}\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064A}\u{0629} 1\u{2013}2 \u{F0B7}";
        assert_eq!(
            comparable(text),
            "Ünïcode\u{3042}\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064A}\u{0629}1\u{2013}2\u{F0B7}"
        );
    }
}
