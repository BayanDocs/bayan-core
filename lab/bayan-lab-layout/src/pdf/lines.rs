//! Lines: the glyphs of a page grouped by baseline, each line put in reading order (`lab/schema/layout-json.md`, "Lines from a PDF").
//!
//! 1. **Grouping.** Glyphs are taken in drawing order. A glyph continues the current line when its baseline lies within half an em of the line's: the em of the larger of the glyph and the line's largest glyph so far, whose baseline is the line's reference. Otherwise it starts a new line. Word, LibreOffice, Typst and browsers draw text line by line, so drawing order keeps columns, table cells and floating text apart, and superscripts and subscripts join their lines.
//! 2. **Display order.** A line's glyphs are sorted by the x of their origins, left to right; glyphs at the same x keep their drawing order.
//! 3. **Reading order.** A line with no right-to-left character reads in display order. Otherwise its base direction is right to left when it holds more strong right-to-left characters (Bidi_Class R or AL) than left-to-right ones (L), or as many and its rightmost strong character is right to left. The Unicode Bidirectional Algorithm (UAX #9) then resolves the levels of its characters in display order and reverses their runs (rule L2): because the reordering of each run is its own inverse, this turns display order back into reading order. A glyph takes the level of its first character; a glyph without characters takes the level of the glyph before it.
//! 4. **Baseline.** The line's baseline is the y most of its largest glyphs share, the smaller y on a tie.
//!
//! The line's text is its glyphs' characters in reading order; each glyph, kept in display order, records where its characters start in that text.

use std::collections::BTreeMap;

use icu_properties::CodePointMapData;
use icu_properties::props::BidiClass;
use unicode_bidi::{BidiInfo, Level};

use super::Error;
use super::content::{Drawn, RawGlyph};
use super::pages::PageInfo;
use crate::layout::{Direction, Glyph, Line, Page, Points};

/// The running state of the text stream of `text_offsets: lines`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Stream {
    /// The offset where the next line starts.
    pub(crate) offset: u64,
    /// Glyphs on lines whose characters are unknown.
    pub(crate) unmapped: u64,
}

/// A glyph on its way into a line: its position rounded, and its characters.
struct Placed {
    x: Points,
    y: Points,
    advance: Points,
    size: Points,
    raw: RawGlyph,
}

/// Builds the layout page from what the page drew.
///
/// # Errors
///
/// [`Error::Malformed`] if a position is out of range.
pub(crate) fn build_page(
    page: &PageInfo,
    drawn: Drawn,
    stream: &mut Stream,
) -> Result<Page, Error> {
    let out_of_range =
        || Error::malformed("a glyph or the page lies out of range (beyond a billion points)");
    let width = Points::from_points(drawn.width).ok_or_else(out_of_range)?;
    let height = Points::from_points(drawn.height).ok_or_else(out_of_range)?;
    let _ = page;
    let page_start = stream.offset;
    let mut lines = Vec::new();
    for group in group(drawn.glyphs)? {
        lines.push(build_line(group, stream)?);
    }
    Ok(Page {
        width,
        height,
        width_blu: None,
        height_blu: None,
        start: Some(page_start),
        lines,
        elements: Some(drawn.elements),
        skipped_glyphs: Some(drawn.skipped),
    })
}

/// Rule 1: groups glyphs, in drawing order, into lines.
fn group(glyphs: Vec<RawGlyph>) -> Result<Vec<Vec<Placed>>, Error> {
    let mut lines: Vec<Vec<Placed>> = Vec::new();
    // The reference of the current line: the y and size of its largest glyph (the first of equal ones).
    let mut reference: Option<(Points, Points)> = None;
    for raw in glyphs {
        let point = |value: f64| {
            Points::from_points(value).ok_or_else(|| {
                Error::malformed("a glyph lies out of range (beyond a billion points)")
            })
        };
        let placed = Placed {
            x: point(raw.x)?,
            y: point(raw.y)?,
            advance: point(raw.advance)?,
            size: point(raw.size)?,
            raw,
        };
        let joins = reference.is_some_and(|(y, size)| {
            let em = if placed.size > size {
                placed.size
            } else {
                size
            };
            // Within half an em, in thousandths of a point.
            2 * (placed.y.millis() - y.millis()).abs() <= em.millis()
        });
        match (joins, lines.last_mut()) {
            (true, Some(line)) => {
                if reference.is_some_and(|(_, size)| placed.size > size) {
                    reference = Some((placed.y, placed.size));
                }
                line.push(placed);
            }
            _ => {
                reference = Some((placed.y, placed.size));
                lines.push(vec![placed]);
            }
        }
    }
    Ok(lines)
}

/// Rules 2 to 4, and the line's offsets in the text stream.
fn build_line(mut glyphs: Vec<Placed>, stream: &mut Stream) -> Result<Line, Error> {
    // Rule 2: display order (the sort is stable, so equal x keep their drawing order).
    glyphs.sort_by_key(|glyph| glyph.x);
    // Rule 4: the baseline.
    let largest = glyphs
        .iter()
        .map(|glyph| glyph.size)
        .max()
        .unwrap_or(Points::ZERO);
    let mut votes: BTreeMap<Points, usize> = BTreeMap::new();
    for glyph in glyphs.iter().filter(|glyph| glyph.size == largest) {
        *votes.entry(glyph.y).or_default() += 1;
    }
    let most = votes.values().copied().max().unwrap_or(0);
    let baseline = votes
        .iter()
        .find(|&(_, &count)| count == most)
        .map_or(Points::ZERO, |(&y, _)| y);
    // Rule 3: the reading order.
    let (direction, order) = reading_order(&glyphs);
    let mut text = String::new();
    let mut offsets = vec![(0_u32, 0_u32); glyphs.len()];
    let mut characters = 0_u32;
    for &index in &order {
        let glyph = &glyphs[index];
        let length = u32::try_from(glyph.raw.text.chars().count())
            .map_err(|_| Error::malformed("a line is too long"))?;
        offsets[index] = (characters, length);
        characters = characters
            .checked_add(length)
            .ok_or_else(|| Error::malformed("a line is too long"))?;
        text.push_str(&glyph.raw.text);
        if glyph.raw.unmapped {
            stream.unmapped += 1;
        }
    }
    let start = stream.offset;
    let end = start + u64::from(characters);
    stream.offset = end + 1;
    let glyphs = glyphs
        .into_iter()
        .zip(offsets)
        .map(|(glyph, (offset, length))| Glyph {
            offset,
            length,
            x: glyph.x,
            y: glyph.y,
            advance: glyph.advance,
            font: glyph.raw.font,
            size: glyph.size,
            gid: glyph.raw.gid,
            x_blu: None,
            y_blu: None,
            advance_blu: None,
            size_blu: None,
        })
        .collect();
    Ok(Line {
        start: Some(start),
        end: Some(end),
        text: Some(text),
        direction: Some(direction),
        baseline: Some(baseline),
        baseline_blu: None,
        glyphs: Some(glyphs),
    })
}

/// The line's base direction, and the indices of its glyphs (in display order) in reading order.
fn reading_order(glyphs: &[Placed]) -> (Direction, Vec<usize>) {
    let identity: Vec<usize> = (0..glyphs.len()).collect();
    let classes = CodePointMapData::<BidiClass>::new();
    let strong = |character: char| match classes.get(character) {
        BidiClass::RightToLeft | BidiClass::ArabicLetter => Some(true),
        BidiClass::LeftToRight => Some(false),
        _ => None,
    };
    let (mut right_to_left, mut left_to_right) = (0_usize, 0_usize);
    let mut rightmost_strong = None;
    for glyph in glyphs {
        for character in glyph.raw.text.chars() {
            match strong(character) {
                Some(true) => right_to_left += 1,
                Some(false) => left_to_right += 1,
                None => continue,
            }
            rightmost_strong = strong(character);
        }
    }
    if right_to_left == 0 {
        return (Direction::Ltr, identity);
    }
    let rtl = right_to_left > left_to_right
        || (right_to_left == left_to_right && rightmost_strong == Some(true));
    let base = if rtl { Level::rtl() } else { Level::ltr() };
    // The characters in display order, and where each glyph's characters start.
    let mut text = String::new();
    let mut starts = Vec::with_capacity(glyphs.len());
    for glyph in glyphs {
        starts.push((!glyph.raw.text.is_empty()).then_some(text.len()));
        text.push_str(&glyph.raw.text);
    }
    let info = BidiInfo::new_with_data_source(&classes, &text, Some(base));
    let mut levels = Vec::with_capacity(glyphs.len());
    let mut previous = base;
    for start in starts {
        let level = start
            .and_then(|start| info.levels.get(start).copied())
            .unwrap_or(previous);
        levels.push(level);
        previous = level;
    }
    let direction = if rtl { Direction::Rtl } else { Direction::Ltr };
    (direction, BidiInfo::reorder_visual(&levels))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(x: f64, y: f64, size: f64, text: &str) -> RawGlyph {
        RawGlyph {
            x,
            y,
            advance: 5.0,
            size,
            text: text.into(),
            font: 0,
            gid: None,
            unmapped: false,
        }
    }

    fn texts(glyphs: Vec<RawGlyph>) -> Vec<(String, Direction)> {
        let mut stream = Stream::default();
        group(glyphs)
            .unwrap()
            .into_iter()
            .map(|line| {
                let line = build_line(line, &mut stream).unwrap();
                (line.text.unwrap(), line.direction.unwrap())
            })
            .collect()
    }

    #[test]
    fn groups_by_baseline_in_drawing_order() {
        let lines = texts(vec![
            glyph(10.0, 100.0, 10.0, "a"),
            glyph(15.0, 100.0, 10.0, "b"),
            // A superscript, raised by a third of an em and smaller, stays on the line.
            glyph(20.0, 96.7, 6.5, "1"),
            glyph(24.0, 100.0, 10.0, "c"),
            // The next line, 12 points lower.
            glyph(10.0, 112.0, 10.0, "d"),
            // A second column on the first line's baseline, drawn after the first column: a line of its own.
            glyph(300.0, 100.0, 10.0, "e"),
        ]);
        assert_eq!(
            lines,
            [
                ("ab1c".to_owned(), Direction::Ltr),
                ("d".to_owned(), Direction::Ltr),
                ("e".to_owned(), Direction::Ltr)
            ]
        );
    }

    #[test]
    fn a_line_that_starts_with_a_superscript_takes_its_baseline_from_the_text() {
        let mut stream = Stream::default();
        let lines = group(vec![
            glyph(10.0, 96.7, 6.5, "1"),
            glyph(14.0, 100.0, 10.0, "N"),
            glyph(20.0, 100.0, 10.0, "o"),
        ])
        .unwrap();
        assert_eq!(lines.len(), 1);
        let line = build_line(lines.into_iter().next().unwrap(), &mut stream).unwrap();
        assert_eq!(line.baseline, Points::from_millis(100_000));
        assert_eq!(line.text.as_deref(), Some("1No"));
    }

    #[test]
    fn sorts_glyphs_drawn_out_of_order() {
        let lines = texts(vec![
            glyph(20.0, 50.0, 10.0, "c"),
            glyph(10.0, 50.0, 10.0, "a"),
            glyph(15.0, 50.0, 10.0, "b"),
        ]);
        assert_eq!(lines, [("abc".to_owned(), Direction::Ltr)]);
    }

    #[test]
    fn reads_right_to_left_lines_in_logical_order() {
        // שלום עולם ("hello world") drawn from left to right as it appears: the last letter first.
        let visual = [
            "\u{5DD}", "\u{5DC}", "\u{5D5}", "\u{5E2}", " ", "\u{5DD}", "\u{5D5}", "\u{5DC}",
            "\u{5E9}",
        ];
        let glyphs = visual
            .iter()
            .enumerate()
            .map(|(index, text)| {
                glyph(
                    10.0 + 5.0 * f64::from(u8::try_from(index).unwrap()),
                    50.0,
                    10.0,
                    text,
                )
            })
            .collect();
        assert_eq!(
            texts(glyphs),
            [(
                "\u{5E9}\u{5DC}\u{5D5}\u{5DD} \u{5E2}\u{5D5}\u{5DC}\u{5DD}".to_owned(),
                Direction::Rtl
            )]
        );
    }

    #[test]
    fn reads_mixed_lines_in_logical_order() {
        let place = |visual: &[&str]| -> Vec<RawGlyph> {
            visual
                .iter()
                .enumerate()
                .map(|(index, text)| {
                    glyph(
                        10.0 + 5.0 * f64::from(u8::try_from(index).unwrap()),
                        50.0,
                        10.0,
                        text,
                    )
                })
                .collect()
        };
        // Hebrew with a number: "שנת 2026" (the year 2026) displays as "2026 תנש".
        assert_eq!(
            texts(place(&[
                "2", "0", "2", "6", " ", "\u{5EA}", "\u{5E0}", "\u{5E9}"
            ])),
            [("\u{5E9}\u{5E0}\u{5EA} 2026".to_owned(), Direction::Rtl)]
        );
        // Hebrew with an English word: "אני אוהב PDF מאוד" displays as "דואמ PDF בהוא ינא".
        assert_eq!(
            texts(place(&["\u{5D3}", "\u{5D5}", "\u{5D0}", "\u{5DE}", " ", "P", "D", "F", " ", "\u{5D1}", "\u{5D4}", "\u{5D5}", "\u{5D0}", " ", "\u{5D9}", "\u{5E0}", "\u{5D0}"])),
            [("\u{5D0}\u{5E0}\u{5D9} \u{5D0}\u{5D5}\u{5D4}\u{5D1} PDF \u{5DE}\u{5D0}\u{5D5}\u{5D3}".to_owned(), Direction::Rtl)]
        );
        // English with a Hebrew word: "the word שלום here" displays as "the word םולש here".
        assert_eq!(
            texts(place(&[
                "t", "h", "e", " ", "w", "o", "r", "d", " ", "\u{5DD}", "\u{5D5}", "\u{5DC}",
                "\u{5E9}", " ", "h", "e", "r", "e"
            ])),
            [(
                "the word \u{5E9}\u{5DC}\u{5D5}\u{5DD} here".to_owned(),
                Direction::Ltr
            )]
        );
        // Arabic with a ligature glyph that stands for two letters, lam-alef, as one glyph: "سلام" displays as "م" "لا" "س".
        assert_eq!(
            texts(place(&["\u{645}", "\u{644}\u{627}", "\u{633}"])),
            [("\u{633}\u{644}\u{627}\u{645}".to_owned(), Direction::Rtl)]
        );
    }

    #[test]
    fn glyphs_keep_display_order_and_point_into_the_logical_text() {
        let mut stream = Stream {
            offset: 10,
            unmapped: 0,
        };
        let glyphs = vec![
            glyph(10.0, 50.0, 10.0, "\u{5D1}"),
            glyph(15.0, 50.0, 10.0, "\u{5D0}"),
        ];
        let line = build_line(
            group(glyphs).unwrap().into_iter().next().unwrap(),
            &mut stream,
        )
        .unwrap();
        assert_eq!(line.text.as_deref(), Some("\u{5D0}\u{5D1}"));
        assert_eq!((line.start, line.end), (Some(10), Some(12)));
        assert_eq!(stream.offset, 13);
        let glyphs = line.glyphs.unwrap();
        assert_eq!((glyphs[0].x.millis(), glyphs[0].offset), (10_000, 1));
        assert_eq!((glyphs[1].x.millis(), glyphs[1].offset), (15_000, 0));
    }
}
