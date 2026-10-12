//! Font files, checked against the manifest and parsed once: harfrust shapes with them and skrifa draws their outlines, both through read-fonts, one font parser (ADR-0009 §1).

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::manifest::PinnedFont;

/// A font file the manifest pins, ready for shaping and drawing.
pub struct Font {
    file: String,
    sha256: [u8; 32],
    data: Arc<Vec<u8>>,
    shaping: harfrust::Font,
    units_per_em: u16,
    ascent: i32,
    descent: i32,
}

impl Font {
    /// Checks `data` against the pin of `file` in `pins`, then parses it.
    ///
    /// # Errors
    ///
    /// A message if no pin names `file`, if `data` is not the pinned file, or if it is not a font.
    pub fn load(file: &str, data: Vec<u8>, pins: &[PinnedFont]) -> Result<Font, String> {
        let pin = pins
            .iter()
            .find(|pin| pin.file == file)
            .ok_or_else(|| format!("{file} is not pinned in fonts/manifest.txt"))?;
        // The checksum is compared before anything parses the bytes.
        let sha256 = crate::hash::sha256(&data);
        if sha256 != pin.sha256 {
            return Err(format!(
                "{file} has SHA-256 {}, but fonts/manifest.txt pins {}",
                crate::hash::hex(&sha256),
                crate::hash::hex(&pin.sha256)
            ));
        }
        let data = Arc::new(data);
        let (units_per_em, ascent, descent) = {
            use skrifa::raw::TableProvider;
            let font = skrifa::FontRef::new(&data).map_err(|error| format!("{file}: {error}"))?;
            let head = font
                .head()
                .map_err(|error| format!("{file}: head: {error}"))?;
            let (ascent, descent) = match font.os2() {
                Ok(os2) => (
                    i32::from(os2.us_win_ascent()),
                    i32::from(os2.us_win_descent()),
                ),
                Err(_) => {
                    let hhea = font
                        .hhea()
                        .map_err(|error| format!("{file}: hhea: {error}"))?;
                    (
                        i32::from(hhea.ascender().to_i16()),
                        -i32::from(hhea.descender().to_i16()),
                    )
                }
            };
            (head.units_per_em(), ascent, descent)
        };
        if units_per_em == 0 {
            return Err(format!("{file}: the em square has no size"));
        }
        let shared: Arc<dyn AsRef<[u8]> + Send + Sync> = data.clone();
        let shaping = harfrust::Font::new(shared, 0)
            .ok_or_else(|| format!("{file}: harfrust cannot read this font"))?;
        Ok(Font {
            file: file.to_owned(),
            sha256,
            data,
            shaping,
            units_per_em,
            ascent,
            descent,
        })
    }

    /// The file name, as the manifest pins it.
    #[must_use]
    pub fn file(&self) -> &str {
        &self.file
    }

    /// The SHA-256 of the file, its identity (ADR-0010 §3).
    #[must_use]
    pub fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    /// The size of the em square, in font units.
    #[must_use]
    pub fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    /// The height above the baseline that a line of this font needs, in font units: the OS/2 table's `usWinAscent`, or the `hhea` table's ascender for a font without OS/2.
    #[must_use]
    pub fn ascent(&self) -> i32 {
        self.ascent
    }

    /// The depth below the baseline that a line of this font needs, in font units, as a positive number: `usWinDescent`, or the `hhea` descender.
    #[must_use]
    pub fn descent(&self) -> i32 {
        self.descent
    }

    /// Whether the font's character map has a glyph for `c`.
    #[must_use]
    pub fn covers(&self, c: char) -> bool {
        self.shaping.charmap().map_unicode(c).is_some()
    }

    /// The font as harfrust shapes with it.
    #[must_use]
    pub fn shaping(&self) -> &harfrust::Font {
        &self.shaping
    }

    /// The font as skrifa reads it, for outlines.
    ///
    /// # Errors
    ///
    /// A message if skrifa cannot read the file, which [`Font::load`] has already ruled out.
    pub fn outlines(&self) -> Result<skrifa::FontRef<'_>, String> {
        skrifa::FontRef::new(&self.data).map_err(|error| format!("{}: {error}", self.file))
    }
}

impl core::fmt::Debug for Font {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Font")
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

/// The fonts of a run of the spike, by file name.
#[derive(Default)]
pub struct FontBook {
    fonts: Vec<Font>,
    by_file: BTreeMap<String, usize>,
}

impl FontBook {
    /// An empty book.
    #[must_use]
    pub fn new() -> FontBook {
        FontBook::default()
    }

    /// Adds the font file `file`, checked against the compiled-in manifest, and returns its index.
    ///
    /// # Errors
    ///
    /// A message if the file is not one the manifest pins, if it was added already, or if [`Font::load`] refuses it.
    pub fn add(&mut self, file: &str, data: Vec<u8>) -> Result<usize, String> {
        if self.by_file.contains_key(file) {
            return Err(format!("{file} was added twice"));
        }
        let font = Font::load(file, data, &crate::manifest::pinned_fonts()?)?;
        let index = self.fonts.len();
        self.fonts.push(font);
        self.by_file.insert(file.to_owned(), index);
        Ok(index)
    }

    /// The index of the font file `file`, if it was added.
    #[must_use]
    pub fn index(&self, file: &str) -> Option<usize> {
        self.by_file.get(file).copied()
    }

    /// The fonts, in the order they were added.
    #[must_use]
    pub fn fonts(&self) -> &[Font] {
        &self.fonts
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::manifest::pinned_fonts;

    pub(crate) const HEBREW: &[u8] = include_bytes!("../fonts/NotoSansHebrew-Regular.ttf");
    pub(crate) const THAI: &[u8] = include_bytes!("../fonts/NotoSansThai-Regular.ttf");

    /// A book with the two committed fonts: 0 is Noto Sans Hebrew, 1 Noto Sans Thai.
    pub(crate) fn committed_book() -> FontBook {
        let mut book = FontBook::new();
        assert_eq!(
            book.add("NotoSansHebrew-Regular.ttf", HEBREW.to_vec()),
            Ok(0)
        );
        assert_eq!(book.add("NotoSansThai-Regular.ttf", THAI.to_vec()), Ok(1));
        book
    }

    #[test]
    fn loads_a_pinned_font_and_reads_its_metrics() {
        let book = committed_book();
        let hebrew = &book.fonts()[0];
        assert_eq!(hebrew.file(), "NotoSansHebrew-Regular.ttf");
        assert_eq!(hebrew.units_per_em(), 1000);
        assert!(
            hebrew.ascent() > 500 && hebrew.ascent() < 1500,
            "{}",
            hebrew.ascent()
        );
        assert!(
            hebrew.descent() > 100 && hebrew.descent() < 800,
            "{}",
            hebrew.descent()
        );
        assert!(hebrew.covers('ש'));
        assert!(!hebrew.covers('ท'));
        assert!(book.fonts()[1].covers('ท'));
        assert_eq!(book.index("NotoSansThai-Regular.ttf"), Some(1));
        assert_eq!(book.index("Unknown.ttf"), None);
    }

    #[test]
    fn refuses_a_file_that_is_not_the_pinned_one() {
        let pins = pinned_fonts().unwrap();
        let mut changed = HEBREW.to_vec();
        changed[100] ^= 1;
        let error = Font::load("NotoSansHebrew-Regular.ttf", changed, &pins).unwrap_err();
        assert!(error.contains("SHA-256"), "{error}");
        // The right bytes under another name are refused too.
        let error = Font::load("NotoSansThai-Regular.ttf", HEBREW.to_vec(), &pins).unwrap_err();
        assert!(error.contains("SHA-256"), "{error}");
    }

    #[test]
    fn refuses_a_file_the_manifest_does_not_pin() {
        let error = Font::load("Other.ttf", HEBREW.to_vec(), &pinned_fonts().unwrap()).unwrap_err();
        assert!(error.contains("not pinned"), "{error}");
    }

    #[test]
    fn refuses_a_font_added_twice() {
        let mut book = committed_book();
        assert!(book.add("NotoSansThai-Regular.ttf", THAI.to_vec()).is_err());
    }
}
