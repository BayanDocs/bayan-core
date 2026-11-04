//! `ToUnicode` CMaps (ISO 32000-2 §9.10.3): the characters each character code of a font stands for.
//!
//! A `ToUnicode` CMap is a small PostScript program; the reader takes from it only the `bfchar` and `bfrange` sections, which hold the mappings, and refuses `usecmap`, which would bring in another CMap. Each destination is UTF-16BE (or, rarely, a glyph name). Destinations that are not text (unpaired surrogates, control characters) become U+FFFD, which the extractor counts as unmapped.

use std::collections::BTreeMap;

use super::glyph_names;
use super::lexer::{Lexer, Token};
use super::{Error, Limit, MAX_GLYPH_CHARACTERS};

/// The most entries one `bfrange` may cover: a whole range of two-byte codes.
const MAX_RANGE: u32 = 0x1_0000;

/// A parsed `ToUnicode` CMap: codes, by their length in bytes and value, to text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ToUnicode {
    map: BTreeMap<(u8, u32), Box<str>>,
}

impl ToUnicode {
    /// The text of the code `value` of `length` bytes, if the CMap maps it.
    pub(crate) fn get(&self, length: u8, value: u32) -> Option<&str> {
        self.map.get(&(length, value)).map(|text| &**text)
    }

    /// Parses the decoded stream `data`. Every mapping takes one of `entries`, the budget of [`super::Limits::max_font_entries`].
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for broken sections, [`Error::Unsupported`] for `usecmap`, and [`Error::Limit`] when the budget runs out.
    pub(crate) fn parse(
        data: &[u8],
        entries: &mut usize,
        max_nesting: usize,
    ) -> Result<Self, Error> {
        let mut cmap = Self::default();
        let mut lexer = Lexer::new(data, 0);
        while let Some(token) = lexer.next_token()? {
            match token {
                Token::Keyword(b"beginbfchar") => cmap.bfchar(&mut lexer, entries)?,
                Token::Keyword(b"beginbfrange") => cmap.bfrange(&mut lexer, entries)?,
                Token::Keyword(b"usecmap") => {
                    return Err(Error::unsupported(
                        "a ToUnicode CMap that uses another CMap",
                    ));
                }
                // Everything else is the PostScript around the mappings: names, dictionaries, `begincmap`, `defineresource` and the like.
                Token::ArrayStart | Token::DictStart => {
                    skip_composite(&mut lexer, token, max_nesting)?;
                }
                _ => {}
            }
        }
        Ok(cmap)
    }

    fn insert(
        &mut self,
        length: u8,
        value: u32,
        text: Box<str>,
        entries: &mut usize,
    ) -> Result<(), Error> {
        *entries = entries
            .checked_sub(1)
            .ok_or(Error::Limit(Limit::FontEntries))?;
        self.map.insert((length, value), text);
        Ok(())
    }

    /// `n beginbfchar <code> <destination> … endbfchar`.
    fn bfchar(&mut self, lexer: &mut Lexer<'_>, entries: &mut usize) -> Result<(), Error> {
        loop {
            let code = match lexer.next_token()? {
                Some(Token::Keyword(b"endbfchar")) => return Ok(()),
                Some(Token::String(code)) => code,
                _ => {
                    return Err(Error::malformed(
                        "a ToUnicode `bfchar` section is malformed",
                    ));
                }
            };
            let text = match lexer.next_token()? {
                Some(Token::String(destination)) => utf16_text(&destination)?,
                Some(Token::Name(name)) => name_text(&name),
                _ => {
                    return Err(Error::malformed(
                        "a ToUnicode `bfchar` entry has no destination",
                    ));
                }
            };
            let (length, value) = code_value(&code)?;
            self.insert(length, value, text, entries)?;
        }
    }

    /// `n beginbfrange <low> <high> <destination> … endbfrange`, where the destination is a string whose last UTF-16 unit counts up through the range, or an array of one string per code.
    fn bfrange(&mut self, lexer: &mut Lexer<'_>, entries: &mut usize) -> Result<(), Error> {
        loop {
            let low = match lexer.next_token()? {
                Some(Token::Keyword(b"endbfrange")) => return Ok(()),
                Some(Token::String(low)) => low,
                _ => {
                    return Err(Error::malformed(
                        "a ToUnicode `bfrange` section is malformed",
                    ));
                }
            };
            let Some(Token::String(high)) = lexer.next_token()? else {
                return Err(Error::malformed(
                    "a ToUnicode `bfrange` entry has no upper end",
                ));
            };
            let (length, first) = code_value(&low)?;
            let (high_length, last) = code_value(&high)?;
            if high_length != length || last < first || last - first >= MAX_RANGE {
                return Err(Error::malformed(
                    "a ToUnicode `bfrange` entry's codes are out of order or out of range",
                ));
            }
            let count = last - first + 1;
            match lexer.next_token()? {
                Some(Token::String(destination)) => {
                    let units = utf16_units(&destination)?;
                    let Some((&final_unit, prefix)) = units.split_last() else {
                        return Err(Error::malformed(
                            "a ToUnicode `bfrange` entry has an empty destination",
                        ));
                    };
                    for offset in 0..count {
                        let unit = u32::from(final_unit) + offset;
                        let unit = u16::try_from(unit).map_err(|_| {
                            Error::malformed(
                                "a ToUnicode `bfrange` entry counts past the last UTF-16 unit",
                            )
                        })?;
                        let mut units = prefix.to_vec();
                        units.push(unit);
                        self.insert(length, first + offset, units_text(&units)?, entries)?;
                    }
                }
                Some(Token::ArrayStart) => {
                    let mut destinations = Vec::new();
                    loop {
                        match lexer.next_token()? {
                            Some(Token::ArrayEnd) => break,
                            Some(Token::String(destination)) => {
                                destinations.push(utf16_text(&destination)?)
                            }
                            Some(Token::Name(name)) => destinations.push(name_text(&name)),
                            _ => {
                                return Err(Error::malformed(
                                    "a ToUnicode `bfrange` array holds something other than strings",
                                ));
                            }
                        }
                        if destinations.len() > usize::try_from(MAX_RANGE).unwrap_or(usize::MAX) {
                            return Err(Error::malformed(
                                "a ToUnicode `bfrange` array is longer than its range",
                            ));
                        }
                    }
                    if u32::try_from(destinations.len()).ok() != Some(count) {
                        return Err(Error::malformed(
                            "a ToUnicode `bfrange` array does not have one destination per code",
                        ));
                    }
                    for (offset, text) in (0..count).zip(destinations) {
                        self.insert(length, first + offset, text, entries)?;
                    }
                }
                _ => {
                    return Err(Error::malformed(
                        "a ToUnicode `bfrange` entry has no destination",
                    ));
                }
            }
        }
    }
}

/// A code's length in bytes (1 to 4) and its value.
fn code_value(code: &[u8]) -> Result<(u8, u32), Error> {
    if code.is_empty() || code.len() > 4 {
        return Err(Error::malformed(
            "a ToUnicode code is not one to four bytes long",
        ));
    }
    let value = code
        .iter()
        .fold(0_u32, |value, &byte| (value << 8) | u32::from(byte));
    Ok((u8::try_from(code.len()).unwrap_or(4), value))
}

/// A destination's UTF-16 units: at most two per character of [`MAX_GLYPH_CHARACTERS`].
fn utf16_units(bytes: &[u8]) -> Result<Vec<u16>, Error> {
    if !bytes.len().is_multiple_of(2) {
        return Err(Error::malformed(
            "a ToUnicode destination has an odd number of bytes",
        ));
    }
    if bytes.len() > MAX_GLYPH_CHARACTERS * 4 {
        return Err(too_long());
    }
    Ok(bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_be_bytes(pair))
        .collect())
}

fn too_long() -> Error {
    Error::malformed(format!(
        "a ToUnicode destination is longer than {MAX_GLYPH_CHARACTERS} characters"
    ))
}

/// A UTF-16BE destination as text.
fn utf16_text(bytes: &[u8]) -> Result<Box<str>, Error> {
    units_text(&utf16_units(bytes)?)
}

/// UTF-16 units as text: unpaired surrogates, control characters and noncharacters become U+FFFD, since they show nothing a comparison could use.
fn units_text(units: &[u16]) -> Result<Box<str>, Error> {
    let text: String = char::decode_utf16(units.iter().copied())
        .map(|unit| match unit {
            Ok(character) if displayable(character) => character,
            _ => char::REPLACEMENT_CHARACTER,
        })
        .collect();
    if text.chars().count() > MAX_GLYPH_CHARACTERS {
        return Err(too_long());
    }
    Ok(text.into_boxed_str())
}

/// Whether a character may stand in a line's text: not a control character other than the tab, and not a noncharacter.
pub(crate) fn displayable(character: char) -> bool {
    let value = u32::from(character);
    let noncharacter = (0xFDD0..=0xFDEF).contains(&value) || (value & 0xFFFE) == 0xFFFE;
    (character == '\t' || !character.is_control()) && !noncharacter
}

/// A glyph name destination as text, or U+FFFD.
fn name_text(name: &[u8]) -> Box<str> {
    glyph_names::characters(name)
        .filter(|text| {
            text.chars().all(displayable) && text.chars().count() <= MAX_GLYPH_CHARACTERS
        })
        .unwrap_or_else(|| char::REPLACEMENT_CHARACTER.to_string())
        .into_boxed_str()
}

/// Skips an array or a dictionary whose opening token was `token`.
fn skip_composite(
    lexer: &mut Lexer<'_>,
    token: Token<'_>,
    max_nesting: usize,
) -> Result<(), Error> {
    super::object::parse_from(token, lexer, false, max_nesting, 0).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMAP: &[u8] = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def
/CMapName /Adobe-Identity-UCS def /CMapType 2 def
1 begincodespacerange <0000> <FFFF> endcodespacerange
3 beginbfchar
<0003> <0020>
<0011> <00660069>
<0012> <D835DC00>
endbfchar
2 beginbfrange
<0020> <0022> <0041>
<0030> <0031> [<05D0> <05D1>]
endbfrange
endcmap CMapName currentdict /CMap defineresource pop end end";

    #[test]
    fn reads_bfchar_and_bfrange() {
        let mut entries = 100;
        let cmap = ToUnicode::parse(CMAP, &mut entries, 8).unwrap();
        assert_eq!(cmap.get(2, 3), Some(" "));
        assert_eq!(cmap.get(2, 0x11), Some("fi"));
        assert_eq!(cmap.get(2, 0x12), Some("\u{1D400}"));
        assert_eq!(cmap.get(2, 0x20), Some("A"));
        assert_eq!(cmap.get(2, 0x22), Some("C"));
        assert_eq!(cmap.get(2, 0x31), Some("\u{5D1}"));
        assert_eq!(cmap.get(2, 0x23), None);
        assert_eq!(cmap.get(1, 0x20), None);
        assert_eq!(entries, 100 - 8);
    }

    #[test]
    fn replaces_what_is_not_text() {
        let mut entries = 100;
        let cmap = ToUnicode::parse(
            b"beginbfchar <01> <D800> <02> <000A> <03> <FFFF> <04> /eacute <05> /nonsense endbfchar",
            &mut entries,
            8,
        )
        .unwrap();
        for code in 1..=3 {
            assert_eq!(cmap.get(1, code), Some("\u{FFFD}"));
        }
        assert_eq!(cmap.get(1, 4), Some("\u{E9}"));
        assert_eq!(cmap.get(1, 5), Some("\u{FFFD}"));
    }

    #[test]
    fn refuses_broken_sections() {
        for cmap in [
            &b"beginbfchar <01> endbfchar"[..],
            b"beginbfchar <01> <0041>",
            b"beginbfchar <0102030405> <0041> endbfchar",
            b"beginbfchar <01> <00 4> extra endbfchar",
            b"beginbfrange <02> <01> <0041> endbfrange",
            b"beginbfrange <01> <0002> <0041> endbfrange",
            b"beginbfrange <01> <02> [<0041>] endbfrange",
            b"beginbfrange <01> <02> <FFFF> endbfrange",
            b"beginbfrange <00000000> <00010000> <0041> endbfrange",
            b"/Other usecmap",
        ] {
            let mut entries = 1000;
            assert!(
                ToUnicode::parse(cmap, &mut entries, 8).is_err(),
                "{}",
                String::from_utf8_lossy(cmap)
            );
        }
        let long = format!(
            "beginbfchar <01> <{}> endbfchar",
            "0041".repeat(MAX_GLYPH_CHARACTERS + 1)
        );
        assert!(ToUnicode::parse(long.as_bytes(), &mut 10, 8).is_err());
    }

    #[test]
    fn counts_entries_against_the_budget() {
        let mut entries = 7;
        assert_eq!(
            ToUnicode::parse(CMAP, &mut entries, 8).unwrap_err(),
            Error::Limit(Limit::FontEntries)
        );
        let mut entries = 8;
        assert!(ToUnicode::parse(CMAP, &mut entries, 8).is_ok());
    }
}
