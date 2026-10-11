//! Which writing systems a document's text uses, by Unicode block.
//!
//! The tagger counts the characters of each script in the text of the document's stories. Scripts are recognized by Unicode block, an approximation of the Unicode Script property that is exact for letters and needs no data files: a block's punctuation and digits count for its script (Arabic-Indic digits count as Arabic), while characters shared by all scripts (spaces, ASCII digits and punctuation, symbols, combining marks) and scripts not listed here are not counted. The counts are aggregate numbers; no text is kept.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A writing system the tagger recognizes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum Script {
    /// Latin, including its fullwidth forms.
    Latin,
    /// Greek.
    Greek,
    /// Cyrillic.
    Cyrillic,
    /// Armenian.
    Armenian,
    /// Hebrew.
    Hebrew,
    /// Arabic, including the letters of Persian, Urdu and other languages written in it.
    Arabic,
    /// Syriac.
    Syriac,
    /// Thaana (Dhivehi).
    Thaana,
    /// N'Ko.
    #[serde(rename = "NKo")]
    #[schemars(rename = "NKo")]
    NKo,
    /// Devanagari.
    Devanagari,
    /// Bengali.
    Bengali,
    /// Gurmukhi.
    Gurmukhi,
    /// Gujarati.
    Gujarati,
    /// Oriya (Odia).
    Oriya,
    /// Tamil.
    Tamil,
    /// Telugu.
    Telugu,
    /// Kannada.
    Kannada,
    /// Malayalam.
    Malayalam,
    /// Sinhala.
    Sinhala,
    /// Thai.
    Thai,
    /// Lao.
    Lao,
    /// Tibetan.
    Tibetan,
    /// Myanmar.
    Myanmar,
    /// Georgian.
    Georgian,
    /// Hangul (Korean).
    Hangul,
    /// Ethiopic.
    Ethiopic,
    /// Cherokee.
    Cherokee,
    /// Unified Canadian Aboriginal Syllabics.
    #[serde(rename = "Canadian Aboriginal")]
    #[schemars(rename = "Canadian Aboriginal")]
    CanadianAboriginal,
    /// Khmer.
    Khmer,
    /// Mongolian.
    Mongolian,
    /// Han (Chinese characters, also used in Japanese and Korean).
    Han,
    /// Hiragana (Japanese).
    Hiragana,
    /// Katakana (Japanese), including the halfwidth forms.
    Katakana,
    /// Bopomofo (Zhuyin).
    Bopomofo,
    /// Yi.
    Yi,
}

impl Script {
    /// The script's name as the manifest records it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Latin => "Latin",
            Self::Greek => "Greek",
            Self::Cyrillic => "Cyrillic",
            Self::Armenian => "Armenian",
            Self::Hebrew => "Hebrew",
            Self::Arabic => "Arabic",
            Self::Syriac => "Syriac",
            Self::Thaana => "Thaana",
            Self::NKo => "NKo",
            Self::Devanagari => "Devanagari",
            Self::Bengali => "Bengali",
            Self::Gurmukhi => "Gurmukhi",
            Self::Gujarati => "Gujarati",
            Self::Oriya => "Oriya",
            Self::Tamil => "Tamil",
            Self::Telugu => "Telugu",
            Self::Kannada => "Kannada",
            Self::Malayalam => "Malayalam",
            Self::Sinhala => "Sinhala",
            Self::Thai => "Thai",
            Self::Lao => "Lao",
            Self::Tibetan => "Tibetan",
            Self::Myanmar => "Myanmar",
            Self::Georgian => "Georgian",
            Self::Hangul => "Hangul",
            Self::Ethiopic => "Ethiopic",
            Self::Cherokee => "Cherokee",
            Self::CanadianAboriginal => "Canadian Aboriginal",
            Self::Khmer => "Khmer",
            Self::Mongolian => "Mongolian",
            Self::Han => "Han",
            Self::Hiragana => "Hiragana",
            Self::Katakana => "Katakana",
            Self::Bopomofo => "Bopomofo",
            Self::Yi => "Yi",
        }
    }

    /// Whether the script needs Arabic or Hebrew shaping and bidirectional layout (the matrix's "Arabic and Hebrew shaping and bidi").
    #[must_use]
    pub const fn is_arabic_or_hebrew(self) -> bool {
        matches!(self, Self::Arabic | Self::Hebrew)
    }

    /// Whether Word treats the script as East Asian, whose language `w:lang/@w:eastAsia` declares.
    #[must_use]
    pub const fn is_east_asian(self) -> bool {
        matches!(
            self,
            Self::Han | Self::Hiragana | Self::Katakana | Self::Hangul | Self::Bopomofo | Self::Yi
        )
    }

    /// Whether Word treats the script as complex, whose language `w:lang/@w:bidi` declares: the right-to-left scripts and those that need complex shaping.
    #[must_use]
    pub const fn is_complex(self) -> bool {
        self.is_arabic_or_hebrew()
            || self.is_indic_or_southeast_asian()
            || matches!(
                self,
                Self::Syriac | Self::Thaana | Self::NKo | Self::Mongolian
            )
    }

    /// Whether the script needs Indic or Southeast Asian shaping and line breaking (the matrix's "Indic and Southeast Asian shaping and breaking").
    #[must_use]
    pub const fn is_indic_or_southeast_asian(self) -> bool {
        matches!(
            self,
            Self::Devanagari
                | Self::Bengali
                | Self::Gurmukhi
                | Self::Gujarati
                | Self::Oriya
                | Self::Tamil
                | Self::Telugu
                | Self::Kannada
                | Self::Malayalam
                | Self::Sinhala
                | Self::Thai
                | Self::Lao
                | Self::Tibetan
                | Self::Myanmar
                | Self::Khmer
        )
    }
}

impl fmt::Display for Script {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Ranges of code points, inclusive, sorted and not overlapping, with their script (from the Unicode 16.0 block list).
const RANGES: [(u32, u32, Script); 87] = [
    (0x0041, 0x005A, Script::Latin),
    (0x0061, 0x007A, Script::Latin),
    (0x00AA, 0x00AA, Script::Latin),
    (0x00BA, 0x00BA, Script::Latin),
    (0x00C0, 0x00D6, Script::Latin),
    (0x00D8, 0x00F6, Script::Latin),
    (0x00F8, 0x02AF, Script::Latin),
    (0x0370, 0x03E1, Script::Greek),
    (0x03F0, 0x03FF, Script::Greek),
    (0x0400, 0x052F, Script::Cyrillic),
    (0x0531, 0x058F, Script::Armenian),
    (0x0591, 0x05FF, Script::Hebrew),
    (0x0600, 0x06FF, Script::Arabic),
    (0x0700, 0x074F, Script::Syriac),
    (0x0750, 0x077F, Script::Arabic),
    (0x0780, 0x07BF, Script::Thaana),
    (0x07C0, 0x07FF, Script::NKo),
    (0x0860, 0x086F, Script::Syriac),
    (0x0870, 0x08FF, Script::Arabic),
    (0x0900, 0x097F, Script::Devanagari),
    (0x0980, 0x09FF, Script::Bengali),
    (0x0A00, 0x0A7F, Script::Gurmukhi),
    (0x0A80, 0x0AFF, Script::Gujarati),
    (0x0B00, 0x0B7F, Script::Oriya),
    (0x0B80, 0x0BFF, Script::Tamil),
    (0x0C00, 0x0C7F, Script::Telugu),
    (0x0C80, 0x0CFF, Script::Kannada),
    (0x0D00, 0x0D7F, Script::Malayalam),
    (0x0D80, 0x0DFF, Script::Sinhala),
    (0x0E00, 0x0E7F, Script::Thai),
    (0x0E80, 0x0EFF, Script::Lao),
    (0x0F00, 0x0FFF, Script::Tibetan),
    (0x1000, 0x109F, Script::Myanmar),
    (0x10A0, 0x10FF, Script::Georgian),
    (0x1100, 0x11FF, Script::Hangul),
    (0x1200, 0x139F, Script::Ethiopic),
    (0x13A0, 0x13FF, Script::Cherokee),
    (0x1400, 0x167F, Script::CanadianAboriginal),
    (0x1780, 0x17FF, Script::Khmer),
    (0x1800, 0x18AF, Script::Mongolian),
    (0x18B0, 0x18FF, Script::CanadianAboriginal),
    (0x19E0, 0x19FF, Script::Khmer),
    (0x1C80, 0x1C8F, Script::Cyrillic),
    (0x1C90, 0x1CBF, Script::Georgian),
    (0x1E00, 0x1EFF, Script::Latin),
    (0x1F00, 0x1FFF, Script::Greek),
    (0x2C60, 0x2C7F, Script::Latin),
    (0x2D00, 0x2D2F, Script::Georgian),
    (0x2D80, 0x2DDF, Script::Ethiopic),
    (0x2DE0, 0x2DFF, Script::Cyrillic),
    (0x2E80, 0x2FDF, Script::Han),
    (0x3005, 0x3005, Script::Han),
    (0x3007, 0x3007, Script::Han),
    (0x3021, 0x3029, Script::Han),
    (0x3038, 0x303B, Script::Han),
    (0x3041, 0x309F, Script::Hiragana),
    (0x30A0, 0x30FF, Script::Katakana),
    (0x3100, 0x312F, Script::Bopomofo),
    (0x3130, 0x318F, Script::Hangul),
    (0x31A0, 0x31BF, Script::Bopomofo),
    (0x31F0, 0x31FF, Script::Katakana),
    (0x3400, 0x4DBF, Script::Han),
    (0x4E00, 0x9FFF, Script::Han),
    (0xA000, 0xA4CF, Script::Yi),
    (0xA640, 0xA69F, Script::Cyrillic),
    (0xA720, 0xA7FF, Script::Latin),
    (0xA8E0, 0xA8FF, Script::Devanagari),
    (0xA960, 0xA97F, Script::Hangul),
    (0xA9E0, 0xA9FF, Script::Myanmar),
    (0xAA60, 0xAA7F, Script::Myanmar),
    (0xAB00, 0xAB2F, Script::Ethiopic),
    (0xAB30, 0xAB6F, Script::Latin),
    (0xAB70, 0xABBF, Script::Cherokee),
    (0xAC00, 0xD7AF, Script::Hangul),
    (0xD7B0, 0xD7FF, Script::Hangul),
    (0xF900, 0xFAFF, Script::Han),
    (0xFB00, 0xFB06, Script::Latin),
    (0xFB13, 0xFB17, Script::Armenian),
    (0xFB1D, 0xFB4F, Script::Hebrew),
    (0xFB50, 0xFDFF, Script::Arabic),
    (0xFE70, 0xFEFC, Script::Arabic),
    (0xFF21, 0xFF3A, Script::Latin),
    (0xFF41, 0xFF5A, Script::Latin),
    (0xFF66, 0xFF9F, Script::Katakana),
    (0xFFA0, 0xFFDC, Script::Hangul),
    (0x2_0000, 0x2_FA1F, Script::Han),
    (0x3_0000, 0x3_23AF, Script::Han),
];

/// The script of `character`, or `None` for characters shared by all scripts and for scripts not listed.
#[must_use]
pub fn script_of(character: char) -> Option<Script> {
    let code = u32::from(character);
    let index = RANGES.partition_point(|&(_, end, _)| end < code);
    RANGES
        .get(index)
        .filter(|&&(start, _, _)| start <= code)
        .map(|&(_, _, script)| script)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ranges_are_sorted_and_do_not_overlap() {
        for (start, end, _) in RANGES {
            assert!(start <= end);
        }
        for pair in RANGES.windows(2) {
            assert!(
                pair[0].1 < pair[1].0,
                "{:X} overlaps {:X}",
                pair[0].1,
                pair[1].0
            );
        }
    }

    #[test]
    fn recognizes_letters_of_each_script() {
        for (character, script) in [
            ('a', Some(Script::Latin)),
            ('Z', Some(Script::Latin)),
            ('é', Some(Script::Latin)),
            ('ẞ', Some(Script::Latin)),
            ('Ａ', Some(Script::Latin)),
            ('α', Some(Script::Greek)),
            ('ж', Some(Script::Cyrillic)),
            ('א', Some(Script::Hebrew)),
            ('ب', Some(Script::Arabic)),
            ('ﻻ', Some(Script::Arabic)),
            ('ک', Some(Script::Arabic)),
            ('क', Some(Script::Devanagari)),
            ('த', Some(Script::Tamil)),
            ('ก', Some(Script::Thai)),
            ('ក', Some(Script::Khmer)),
            ('中', Some(Script::Han)),
            ('𠀀', Some(Script::Han)),
            ('あ', Some(Script::Hiragana)),
            ('ア', Some(Script::Katakana)),
            ('ｱ', Some(Script::Katakana)),
            ('한', Some(Script::Hangul)),
            ('ㄅ', Some(Script::Bopomofo)),
            ('ሀ', Some(Script::Ethiopic)),
            ('ა', Some(Script::Georgian)),
            ('1', None),
            (' ', None),
            ('.', None),
            ('\u{301}', None),
            ('€', None),
            ('、', None),
            ('\u{FEFF}', None),
        ] {
            assert_eq!(script_of(character), script, "{character:?}");
        }
    }

    #[test]
    fn names_round_trip_through_serde() {
        for script in [
            Script::Latin,
            Script::NKo,
            Script::CanadianAboriginal,
            Script::Han,
        ] {
            let json = serde_json::to_string(&script).unwrap();
            assert_eq!(json, format!("\"{}\"", script.name()));
            assert_eq!(serde_json::from_str::<Script>(&json).unwrap(), script);
        }
    }
}
