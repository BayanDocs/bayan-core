//! Glyph names to characters (ISO 32000-2 §9.10.2): the names a simple font's `Differences` array gives its codes, read as the Adobe Glyph List reads them.
//!
//! A name is read in three ways, in this order: as a name of [`NAMES`] (PDF's Latin character set and the Symbol font's Greek letters and mathematical signs); as `uniXXXX` (one or more groups of four hexadecimal digits, each a character outside the surrogates) or `uXXXX` to `uXXXXXX` (one character); or, for a name with underscores, as the ligature of the characters of its parts (`f_f_i`). A suffix after a full stop is ignored (`a.sc` is `a`). Any other name gives no character, and the glyph is counted as unmapped.

/// The characters a glyph name stands for, or `None` if the name is not one of the forms above.
pub(crate) fn characters(name: &[u8]) -> Option<String> {
    let base = name.split(|&byte| byte == b'.').next().unwrap_or_default();
    if base.is_empty() {
        return None;
    }
    let mut out = String::new();
    for part in base.split(|&byte| byte == b'_') {
        component(part, &mut out)?;
    }
    Some(out)
}

/// Adds the characters of one part of a glyph name.
fn component(part: &[u8], out: &mut String) -> Option<()> {
    if let Ok(index) = NAMES.binary_search_by(|(name, _)| name.cmp(&part)) {
        out.push(char::from_u32(u32::from(NAMES[index].1))?);
        return Some(());
    }
    if let Some(hex) = part.strip_prefix(b"uni")
        && !hex.is_empty()
        && hex.len() % 4 == 0
    {
        for group in hex.as_chunks::<4>().0 {
            let value = hex_number(group)?;
            if (0xD800..=0xDFFF).contains(&value) {
                return None;
            }
            out.push(char::from_u32(value)?);
        }
        return Some(());
    }
    if let Some(hex) = part.strip_prefix(b"u")
        && (4..=6).contains(&hex.len())
    {
        out.push(char::from_u32(hex_number(hex)?)?);
        return Some(());
    }
    None
}

/// Upper-case hexadecimal digits as a number (the Adobe Glyph List's forms use upper case only).
fn hex_number(digits: &[u8]) -> Option<u32> {
    digits.iter().try_fold(0_u32, |value, &digit| {
        let digit = match digit {
            b'0'..=b'9' => digit - b'0',
            b'A'..=b'F' => digit - b'A' + 10,
            _ => return None,
        };
        Some(value * 16 + u32::from(digit))
    })
}

/// Glyph names and their characters, sorted by name (byte order) for binary search: the 229 names of PDF's Latin character set (ISO 32000-2 Annex D.2), the Greek letters and mathematical signs of the Symbol font's set, and a few other names that the Adobe Glyph List gives the same way.
const NAMES: [(&[u8], u16); 300] = [
    (b"A", 0x0041),
    (b"AE", 0x00C6),
    (b"Aacute", 0x00C1),
    (b"Acircumflex", 0x00C2),
    (b"Adieresis", 0x00C4),
    (b"Agrave", 0x00C0),
    (b"Alpha", 0x0391),
    (b"Aring", 0x00C5),
    (b"Atilde", 0x00C3),
    (b"B", 0x0042),
    (b"Beta", 0x0392),
    (b"C", 0x0043),
    (b"Ccedilla", 0x00C7),
    (b"Chi", 0x03A7),
    (b"D", 0x0044),
    (b"Delta", 0x0394),
    (b"E", 0x0045),
    (b"Eacute", 0x00C9),
    (b"Ecircumflex", 0x00CA),
    (b"Edieresis", 0x00CB),
    (b"Egrave", 0x00C8),
    (b"Epsilon", 0x0395),
    (b"Eta", 0x0397),
    (b"Eth", 0x00D0),
    (b"Euro", 0x20AC),
    (b"F", 0x0046),
    (b"G", 0x0047),
    (b"Gamma", 0x0393),
    (b"H", 0x0048),
    (b"I", 0x0049),
    (b"Iacute", 0x00CD),
    (b"Icircumflex", 0x00CE),
    (b"Idieresis", 0x00CF),
    (b"Igrave", 0x00CC),
    (b"Iota", 0x0399),
    (b"J", 0x004A),
    (b"K", 0x004B),
    (b"Kappa", 0x039A),
    (b"L", 0x004C),
    (b"Lambda", 0x039B),
    (b"Lslash", 0x0141),
    (b"M", 0x004D),
    (b"Mu", 0x039C),
    (b"N", 0x004E),
    (b"Ntilde", 0x00D1),
    (b"Nu", 0x039D),
    (b"O", 0x004F),
    (b"OE", 0x0152),
    (b"Oacute", 0x00D3),
    (b"Ocircumflex", 0x00D4),
    (b"Odieresis", 0x00D6),
    (b"Ograve", 0x00D2),
    (b"Omega", 0x03A9),
    (b"Omicron", 0x039F),
    (b"Oslash", 0x00D8),
    (b"Otilde", 0x00D5),
    (b"P", 0x0050),
    (b"Phi", 0x03A6),
    (b"Pi", 0x03A0),
    (b"Psi", 0x03A8),
    (b"Q", 0x0051),
    (b"R", 0x0052),
    (b"Rho", 0x03A1),
    (b"S", 0x0053),
    (b"Scaron", 0x0160),
    (b"Sigma", 0x03A3),
    (b"T", 0x0054),
    (b"Tau", 0x03A4),
    (b"Theta", 0x0398),
    (b"Thorn", 0x00DE),
    (b"U", 0x0055),
    (b"Uacute", 0x00DA),
    (b"Ucircumflex", 0x00DB),
    (b"Udieresis", 0x00DC),
    (b"Ugrave", 0x00D9),
    (b"Upsilon", 0x03A5),
    (b"V", 0x0056),
    (b"W", 0x0057),
    (b"X", 0x0058),
    (b"Xi", 0x039E),
    (b"Y", 0x0059),
    (b"Yacute", 0x00DD),
    (b"Ydieresis", 0x0178),
    (b"Z", 0x005A),
    (b"Zcaron", 0x017D),
    (b"Zeta", 0x0396),
    (b"a", 0x0061),
    (b"aacute", 0x00E1),
    (b"acircumflex", 0x00E2),
    (b"acute", 0x00B4),
    (b"adieresis", 0x00E4),
    (b"ae", 0x00E6),
    (b"agrave", 0x00E0),
    (b"alpha", 0x03B1),
    (b"ampersand", 0x0026),
    (b"approxequal", 0x2248),
    (b"aring", 0x00E5),
    (b"arrowboth", 0x2194),
    (b"arrowdown", 0x2193),
    (b"arrowleft", 0x2190),
    (b"arrowright", 0x2192),
    (b"arrowup", 0x2191),
    (b"asciicircum", 0x005E),
    (b"asciitilde", 0x007E),
    (b"asterisk", 0x002A),
    (b"at", 0x0040),
    (b"atilde", 0x00E3),
    (b"b", 0x0062),
    (b"backslash", 0x005C),
    (b"bar", 0x007C),
    (b"beta", 0x03B2),
    (b"braceleft", 0x007B),
    (b"braceright", 0x007D),
    (b"bracketleft", 0x005B),
    (b"bracketright", 0x005D),
    (b"breve", 0x02D8),
    (b"brokenbar", 0x00A6),
    (b"bullet", 0x2022),
    (b"c", 0x0063),
    (b"caron", 0x02C7),
    (b"ccedilla", 0x00E7),
    (b"cedilla", 0x00B8),
    (b"cent", 0x00A2),
    (b"chi", 0x03C7),
    (b"circumflex", 0x02C6),
    (b"colon", 0x003A),
    (b"comma", 0x002C),
    (b"copyright", 0x00A9),
    (b"currency", 0x00A4),
    (b"d", 0x0064),
    (b"dagger", 0x2020),
    (b"daggerdbl", 0x2021),
    (b"degree", 0x00B0),
    (b"delta", 0x03B4),
    (b"dieresis", 0x00A8),
    (b"divide", 0x00F7),
    (b"dollar", 0x0024),
    (b"dotaccent", 0x02D9),
    (b"dotlessi", 0x0131),
    (b"dotlessj", 0x0237),
    (b"e", 0x0065),
    (b"eacute", 0x00E9),
    (b"ecircumflex", 0x00EA),
    (b"edieresis", 0x00EB),
    (b"egrave", 0x00E8),
    (b"eight", 0x0038),
    (b"ellipsis", 0x2026),
    (b"emdash", 0x2014),
    (b"endash", 0x2013),
    (b"epsilon", 0x03B5),
    (b"equal", 0x003D),
    (b"eta", 0x03B7),
    (b"eth", 0x00F0),
    (b"exclam", 0x0021),
    (b"exclamdown", 0x00A1),
    (b"f", 0x0066),
    (b"ff", 0xFB00),
    (b"ffi", 0xFB03),
    (b"ffl", 0xFB04),
    (b"fi", 0xFB01),
    (b"five", 0x0035),
    (b"fl", 0xFB02),
    (b"florin", 0x0192),
    (b"four", 0x0034),
    (b"fraction", 0x2044),
    (b"g", 0x0067),
    (b"gamma", 0x03B3),
    (b"germandbls", 0x00DF),
    (b"grave", 0x0060),
    (b"greater", 0x003E),
    (b"greaterequal", 0x2265),
    (b"guillemotleft", 0x00AB),
    (b"guillemotright", 0x00BB),
    (b"guilsinglleft", 0x2039),
    (b"guilsinglright", 0x203A),
    (b"h", 0x0068),
    (b"hungarumlaut", 0x02DD),
    (b"hyphen", 0x002D),
    (b"i", 0x0069),
    (b"iacute", 0x00ED),
    (b"icircumflex", 0x00EE),
    (b"idieresis", 0x00EF),
    (b"igrave", 0x00EC),
    (b"infinity", 0x221E),
    (b"integral", 0x222B),
    (b"iota", 0x03B9),
    (b"j", 0x006A),
    (b"k", 0x006B),
    (b"kappa", 0x03BA),
    (b"l", 0x006C),
    (b"lambda", 0x03BB),
    (b"less", 0x003C),
    (b"lessequal", 0x2264),
    (b"logicalnot", 0x00AC),
    (b"lozenge", 0x25CA),
    (b"lslash", 0x0142),
    (b"m", 0x006D),
    (b"macron", 0x00AF),
    (b"middot", 0x00B7),
    (b"minus", 0x2212),
    (b"mu", 0x00B5),
    (b"multiply", 0x00D7),
    (b"n", 0x006E),
    (b"nbspace", 0x00A0),
    (b"nine", 0x0039),
    (b"notequal", 0x2260),
    (b"ntilde", 0x00F1),
    (b"nu", 0x03BD),
    (b"numbersign", 0x0023),
    (b"o", 0x006F),
    (b"oacute", 0x00F3),
    (b"ocircumflex", 0x00F4),
    (b"odieresis", 0x00F6),
    (b"oe", 0x0153),
    (b"ogonek", 0x02DB),
    (b"ograve", 0x00F2),
    (b"omega", 0x03C9),
    (b"omicron", 0x03BF),
    (b"one", 0x0031),
    (b"onehalf", 0x00BD),
    (b"onequarter", 0x00BC),
    (b"onesuperior", 0x00B9),
    (b"ordfeminine", 0x00AA),
    (b"ordmasculine", 0x00BA),
    (b"oslash", 0x00F8),
    (b"otilde", 0x00F5),
    (b"p", 0x0070),
    (b"paragraph", 0x00B6),
    (b"parenleft", 0x0028),
    (b"parenright", 0x0029),
    (b"partialdiff", 0x2202),
    (b"percent", 0x0025),
    (b"period", 0x002E),
    (b"periodcentered", 0x00B7),
    (b"perthousand", 0x2030),
    (b"phi", 0x03C6),
    (b"pi", 0x03C0),
    (b"plus", 0x002B),
    (b"plusminus", 0x00B1),
    (b"product", 0x220F),
    (b"psi", 0x03C8),
    (b"q", 0x0071),
    (b"question", 0x003F),
    (b"questiondown", 0x00BF),
    (b"quotedbl", 0x0022),
    (b"quotedblbase", 0x201E),
    (b"quotedblleft", 0x201C),
    (b"quotedblright", 0x201D),
    (b"quoteleft", 0x2018),
    (b"quoteright", 0x2019),
    (b"quotesinglbase", 0x201A),
    (b"quotesingle", 0x0027),
    (b"r", 0x0072),
    (b"radical", 0x221A),
    (b"registered", 0x00AE),
    (b"rho", 0x03C1),
    (b"ring", 0x02DA),
    (b"s", 0x0073),
    (b"scaron", 0x0161),
    (b"section", 0x00A7),
    (b"semicolon", 0x003B),
    (b"seven", 0x0037),
    (b"sfthyphen", 0x00AD),
    (b"sigma", 0x03C3),
    (b"sigma1", 0x03C2),
    (b"six", 0x0036),
    (b"slash", 0x002F),
    (b"space", 0x0020),
    (b"sterling", 0x00A3),
    (b"summation", 0x2211),
    (b"t", 0x0074),
    (b"tau", 0x03C4),
    (b"theta", 0x03B8),
    (b"thorn", 0x00FE),
    (b"three", 0x0033),
    (b"threequarters", 0x00BE),
    (b"threesuperior", 0x00B3),
    (b"tilde", 0x02DC),
    (b"trademark", 0x2122),
    (b"two", 0x0032),
    (b"twosuperior", 0x00B2),
    (b"u", 0x0075),
    (b"uacute", 0x00FA),
    (b"ucircumflex", 0x00FB),
    (b"udieresis", 0x00FC),
    (b"ugrave", 0x00F9),
    (b"underscore", 0x005F),
    (b"upsilon", 0x03C5),
    (b"v", 0x0076),
    (b"w", 0x0077),
    (b"x", 0x0078),
    (b"xi", 0x03BE),
    (b"y", 0x0079),
    (b"yacute", 0x00FD),
    (b"ydieresis", 0x00FF),
    (b"yen", 0x00A5),
    (b"z", 0x007A),
    (b"zcaron", 0x017E),
    (b"zero", 0x0030),
    (b"zeta", 0x03B6),
];

#[cfg(test)]
mod tests {
    use super::{NAMES, characters};

    #[test]
    fn the_table_is_sorted_for_binary_search() {
        assert!(NAMES.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    #[test]
    fn reads_names_in_every_form() {
        assert_eq!(characters(b"A").as_deref(), Some("A"));
        assert_eq!(characters(b"eacute").as_deref(), Some("\u{E9}"));
        assert_eq!(characters(b"quoteright").as_deref(), Some("\u{2019}"));
        assert_eq!(characters(b"fi").as_deref(), Some("\u{FB01}"));
        assert_eq!(characters(b"Omega").as_deref(), Some("\u{3A9}"));
        assert_eq!(characters(b"uni05D0").as_deref(), Some("\u{5D0}"));
        assert_eq!(characters(b"uni00660066").as_deref(), Some("ff"));
        assert_eq!(characters(b"u1F600").as_deref(), Some("\u{1F600}"));
        assert_eq!(characters(b"f_f_i").as_deref(), Some("ffi"));
        assert_eq!(characters(b"a.sc").as_deref(), Some("a"));
        assert_eq!(characters(b"uni05D0.fina").as_deref(), Some("\u{5D0}"));
        for unknown in [
            &b""[..],
            b".notdef",
            b"g123",
            b"cid42",
            b"uniD800",
            b"uni05d0",
            b"uni05D",
            b"u12",
            b"u1234567",
            b"f_unknown",
        ] {
            assert_eq!(
                characters(unknown),
                None,
                "{}",
                String::from_utf8_lossy(unknown)
            );
        }
    }
}
