//! Shaping a run with harfrust, in font units (harfrust has no font size; ADR-0005 §2 turns font units into BLU later, with an explicit rounding).

use core::ops::Range;

use icu_properties::props::Script;

use crate::fonts::Font;

/// The OpenType features a probe switches, as Word does per run (ADR-0009 §6): kerning (`kern`) and standard ligatures (`liga` and `clig`). Every other feature keeps harfrust's default for the script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    /// Whether pair kerning is applied.
    pub kerning: bool,
    /// Whether standard and contextual ligatures are formed.
    pub ligatures: bool,
}

/// A glyph as harfrust shaped it, in font units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShapedGlyph {
    /// The glyph identifier in the font.
    pub id: u32,
    /// The byte offset in the paragraph of the first character of the glyph's cluster.
    pub cluster: usize,
    /// How far the pen moves after the glyph.
    pub x_advance: i32,
    /// How far the pen moves up after the glyph (zero in horizontal text).
    pub y_advance: i32,
    /// How far the glyph is drawn to the right of the pen.
    pub x_offset: i32,
    /// How far the glyph is drawn above the pen.
    pub y_offset: i32,
    /// Whether cutting the run before this glyph and shaping the two parts separately could give other glyphs (HarfBuzz's `UNSAFE_TO_BREAK`).
    pub unsafe_to_break: bool,
}

/// Shapes `paragraph[range]` with `font`, the rest of the paragraph serving as context, in visual order (right to left text comes out reversed, as the glyphs are drawn).
///
/// # Errors
///
/// A message if the script has no ISO 15924 code that harfrust knows, or if harfrust refuses the font.
pub fn shape(
    font: &Font,
    paragraph: &str,
    range: Range<usize>,
    rtl: bool,
    script: Script,
    language: &str,
    features: Features,
) -> Result<Vec<ShapedGlyph>, String> {
    let mut buffer = harfrust::Buffer::new();
    buffer.push_str(&paragraph[range.clone()]);
    // The text around the run decides, for example, whether an Arabic letter at its edge joins.
    buffer.set_pre_context(&paragraph[..range.start]);
    buffer.set_post_context(&paragraph[range.end..]);
    let mut flags = harfrust::BufferFlags::empty();
    if range.start == 0 {
        flags |= harfrust::BufferFlags::BEGINNING_OF_TEXT;
    }
    if range.end == paragraph.len() {
        flags |= harfrust::BufferFlags::END_OF_TEXT;
    }
    buffer.set_flags(flags);
    buffer.set_direction(if rtl {
        harfrust::Direction::RightToLeft
    } else {
        harfrust::Direction::LeftToRight
    });
    buffer.set_script(Some(harfrust_script(script)?));
    buffer.set_language(language.parse().ok());
    let all = ..;
    let features = [
        harfrust::Feature::new(
            harfrust::Tag::new(b"kern"),
            u32::from(features.kerning),
            all,
        ),
        harfrust::Feature::new(
            harfrust::Tag::new(b"liga"),
            u32::from(features.ligatures),
            all,
        ),
        harfrust::Feature::new(
            harfrust::Tag::new(b"clig"),
            u32::from(features.ligatures),
            all,
        ),
    ];
    harfrust::shape(
        &harfrust::ShaperFont::new(font.shaping()),
        &mut buffer,
        harfrust::ShapeOptions::new().features(&features),
    )
    .map_err(|error| format!("{}: {error}", font.file()))?;
    Ok(buffer
        .glyph_infos()
        .iter()
        .zip(buffer.glyph_positions())
        .map(|(info, position)| ShapedGlyph {
            id: info.glyph_id,
            // Clusters count bytes from the start of what was pushed, which fits in a usize because the text does.
            cluster: range.start + usize::try_from(info.cluster).unwrap_or(0),
            x_advance: position.x_advance,
            y_advance: position.y_advance,
            x_offset: position.x_offset,
            y_offset: position.y_offset,
            unsafe_to_break: info.unsafe_to_break(),
        })
        .collect())
}

/// The four-letter ISO 15924 code of a script, such as `Hebr`.
///
/// # Errors
///
/// A message if the script has none.
pub fn script_code(script: Script) -> Result<String, String> {
    icu_properties::PropertyNamesShortBorrowed::<Script>::new()
        .get_locale_script(script)
        .map(|code| code.as_str().to_owned())
        .ok_or_else(|| format!("the script {script:?} has no ISO 15924 code"))
}

/// The harfrust script for an ICU4X script, through its ISO 15924 code.
fn harfrust_script(script: Script) -> Result<harfrust::Script, String> {
    let code = script_code(script)?;
    let tag: [u8; 4] = code
        .as_bytes()
        .try_into()
        .map_err(|_| format!("the ISO 15924 code {code} is not four letters"))?;
    harfrust::Script::from_iso15924_tag(harfrust::Tag::new(&tag))
        .ok_or_else(|| format!("harfrust does not know the script {code}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::tests::committed_book;

    const PLAIN: Features = Features {
        kerning: true,
        ligatures: true,
    };

    #[test]
    fn shapes_hebrew_right_to_left_in_visual_order() {
        let book = committed_book();
        let text = "שלום";
        let glyphs = shape(
            &book.fonts()[0],
            text,
            0..text.len(),
            true,
            Script::Hebrew,
            "he",
            PLAIN,
        )
        .unwrap();
        // Visual order: the last letter first.
        let clusters: Vec<usize> = glyphs.iter().map(|glyph| glyph.cluster).collect();
        assert_eq!(clusters, [6, 4, 2, 0]);
        assert!(
            glyphs
                .iter()
                .all(|glyph| glyph.id != 0 && glyph.x_advance > 0)
        );
    }

    #[test]
    fn reports_clusters_as_offsets_in_the_paragraph() {
        let book = committed_book();
        let text = "שלום עולם";
        // Only the second word, with the first as context.
        let glyphs = shape(
            &book.fonts()[0],
            text,
            9..text.len(),
            true,
            Script::Hebrew,
            "he",
            PLAIN,
        )
        .unwrap();
        let clusters: Vec<usize> = glyphs.iter().map(|glyph| glyph.cluster).collect();
        assert_eq!(clusters, [15, 13, 11, 9]);
    }

    #[test]
    fn keeps_thai_marks_in_their_cluster() {
        let book = committed_book();
        // น้ำ (NO NU, MAI THO, SARA AM) is one cluster; the Thai shaper splits SARA AM into NIKHAHIT and SARA AA, and the marks take no advance.
        let text = "น้ำ";
        let glyphs = shape(
            &book.fonts()[1],
            text,
            0..text.len(),
            false,
            Script::Thai,
            "th",
            PLAIN,
        )
        .unwrap();
        assert!(glyphs.len() >= 3, "{glyphs:?}");
        assert!(glyphs.iter().all(|glyph| glyph.cluster == 0), "{glyphs:?}");
        assert!(
            glyphs.iter().any(|glyph| glyph.x_advance == 0),
            "{glyphs:?}"
        );
    }

    #[test]
    fn a_missing_glyph_is_glyph_zero() {
        let book = committed_book();
        let glyphs = shape(
            &book.fonts()[0],
            "ท",
            0..3,
            false,
            Script::Thai,
            "th",
            PLAIN,
        )
        .unwrap();
        assert_eq!(glyphs.len(), 1);
        assert_eq!(glyphs[0].id, 0);
    }
}
