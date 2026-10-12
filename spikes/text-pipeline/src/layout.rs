//! Laying out one paragraph: itemize, shape, measure in BLU, break into lines, reorder each line for display, and place every glyph. The result is the layout JSON, whose SHA-256 is the layout hash of a probe.
//!
//! **Measurement** (ADR-0005 §2): harfrust's advances and offsets are font units; each is turned into BLU on its own, `units × size ÷ units_per_em` rounded half to even, and a line's glyphs are placed by adding those BLU values in integers. The line height of single spacing is the largest `usWinAscent` plus the largest `usWinDescent` of the line's fonts, each scaled the same way. These rounding points are the spike's choice; where Word rounds is for the Fidelity Lab to find out (LAB-005).
//!
//! **Lines**: first fit at UAX #14 opportunities ([`crate::linebreak`]). A run that a line boundary cuts keeps its glyphs where HarfBuzz says cutting is safe, and is shaped again on each side where it is not (`UNSAFE_TO_BREAK`). Each line is reordered with the bidi algorithm's rules L1 and L2 (unicode-bidi's `visual_runs`); a left-to-right paragraph starts at the left edge, a right-to-left one ends at the right edge.

use core::ops::Range;

use bayan_units::{Blu, Rounding};
use icu_properties::CodePointMapData;
use icu_properties::props::BidiClass;
use serde::Serialize;
use unicode_bidi::{BidiInfo, Level};

use crate::fonts::{Font, FontBook};
use crate::itemize::{Run, itemize};
use crate::linebreak::{Segment, break_lines};
use crate::segment::line_breaks;
use crate::shape::{Features, ShapedGlyph, script_code, shape};

/// A paragraph to lay out.
#[derive(Clone, Copy, Debug)]
pub struct Paragraph<'a> {
    /// The text, with no paragraph separator inside.
    pub text: &'a str,
    /// Whether the paragraph's direction is right to left (`w:bidi`).
    pub rtl: bool,
    /// The fonts it may use, as indices into the font book, first choice first.
    pub fonts: &'a [usize],
    /// The font size.
    pub size: Blu,
    /// The width of its lines.
    pub measure: Blu,
    /// The BCP 47 language of the text, which selects language-specific forms (`locl`).
    pub language: &'a str,
    /// The OpenType features it switches.
    pub features: Features,
}

/// The layout of a paragraph, as the layout JSON writes it. Every length is in BLU, and every position is measured from the paragraph's top left corner, x to the right and y down.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Layout {
    /// The width of the lines.
    pub measure: i64,
    /// The height of all lines together.
    pub height: i64,
    /// The font size.
    pub size: i64,
    /// The fonts the paragraph may use, in the order of the paragraph's list; runs refer to them by index.
    pub fonts: Vec<LayoutFont>,
    /// The lines, from top to bottom.
    pub lines: Vec<Line>,
}

/// A font of a layout, identified by the SHA-256 of its file (ADR-0010 §3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LayoutFont {
    /// The file name.
    pub file: String,
    /// The SHA-256 of the file, in lowercase hexadecimal.
    pub sha256: String,
}

/// A line of a layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Line {
    /// The byte offset of the line's first character in the paragraph.
    pub start: usize,
    /// The byte offset after its last character.
    pub end: usize,
    /// The top of the line box.
    pub top: i64,
    /// The height of the line box above the baseline.
    pub ascent: i64,
    /// The depth of the line box below the baseline.
    pub descent: i64,
    /// Where the pen starts: the left edge of the first glyph's advance.
    pub left: i64,
    /// The advance of all its glyphs, whitespace at its end included.
    pub advance: i64,
    /// The runs of the line in display order, from left to right.
    pub runs: Vec<LineRun>,
}

/// A run of a line: glyphs of one font, script and bidi level.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LineRun {
    /// The index of the run's font in [`Layout::fonts`].
    pub font: usize,
    /// The ISO 15924 code of its script.
    pub script: String,
    /// Its bidi level on this line (after rule L1).
    pub level: u8,
    /// The byte offset of its first character.
    pub start: usize,
    /// The byte offset after its last character.
    pub end: usize,
    /// Its glyphs in display order, from left to right.
    pub glyphs: Vec<Glyph>,
}

/// A placed glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Glyph {
    /// The glyph identifier in the run's font.
    pub id: u32,
    /// The byte offset of the first character of its cluster.
    pub cluster: usize,
    /// The x of its origin.
    pub x: i64,
    /// The y of its origin (on the baseline, unless the font moves it up or down).
    pub y: i64,
    /// Its advance.
    pub advance: i64,
}

impl Layout {
    /// The layout JSON: compact, with the fields in the order of the types above, and ending with a line feed.
    ///
    /// # Errors
    ///
    /// A message if serialization fails, which it cannot for these types.
    pub fn to_json(&self) -> Result<Vec<u8>, String> {
        let mut json = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        json.push(b'\n');
        Ok(json)
    }
}

/// Lays out `paragraph` with the fonts of `book`.
///
/// # Errors
///
/// A message if the paragraph is empty or holds a paragraph separator, if it names a font the book lacks, or if shaping or measuring fails.
pub fn lay_out(book: &FontBook, paragraph: &Paragraph<'_>) -> Result<Layout, String> {
    let text = paragraph.text;
    if text.is_empty() {
        return Err("an empty paragraph has no lines".to_owned());
    }
    let fonts = paragraph
        .fonts
        .iter()
        .map(|&index| {
            book.fonts()
                .get(index)
                .ok_or_else(|| format!("the font book has no font {index}"))
        })
        .collect::<Result<Vec<&Font>, String>>()?;
    if fonts.is_empty() {
        return Err("a paragraph needs at least one font".to_owned());
    }
    let size = paragraph.size.0;
    let base = if paragraph.rtl {
        Level::rtl()
    } else {
        Level::ltr()
    };
    let bidi =
        BidiInfo::new_with_data_source(&CodePointMapData::<BidiClass>::new(), text, Some(base));
    let [bidi_paragraph] = &bidi.paragraphs[..] else {
        return Err("the text holds a paragraph separator".to_owned());
    };
    let runs = itemize(text, &bidi.levels, fonts.len(), &|font, c| {
        fonts[font].covers(c)
    });
    let shaper = Shaper {
        text,
        fonts: &fonts,
        size,
        language: paragraph.language,
        features: paragraph.features,
    };
    let shaped = runs
        .iter()
        .map(|run| {
            let level = Level::new(run.level)
                .map_err(|_| format!("bidi level {} is too deep", run.level))?;
            shaper.shape(run, run.start..run.end, level)
        })
        .collect::<Result<Vec<Vec<Measured>>, String>>()?;

    let ends = break_lines(&segments(text, &shaped), paragraph.measure.0);
    let mut lines = Vec::with_capacity(ends.len());
    let mut start = 0;
    let mut top = 0_i64;
    for end in ends {
        let line = shaper.line(
            &bidi,
            bidi_paragraph,
            &runs,
            &shaped,
            start..end,
            top,
            paragraph,
        )?;
        top = top
            .checked_add(line.ascent)
            .and_then(|top| top.checked_add(line.descent))
            .ok_or("the paragraph is too high")?;
        start = end;
        lines.push(line);
    }
    Ok(Layout {
        measure: paragraph.measure.0,
        height: top,
        size,
        fonts: fonts
            .iter()
            .map(|font| LayoutFont {
                file: font.file().to_owned(),
                sha256: crate::hash::hex(font.sha256()),
            })
            .collect(),
        lines,
    })
}

/// A shaped glyph with its advance and offsets in BLU.
#[derive(Clone, Copy, Debug)]
struct Measured {
    glyph: ShapedGlyph,
    advance: i64,
    x_offset: i64,
    y_offset: i64,
}

/// The segments between break opportunities, measured from the glyphs each run was shaped into.
fn segments(text: &str, shaped: &[Vec<Measured>]) -> Vec<Segment> {
    // The advance of the glyphs of each cluster, at the byte offset where the cluster starts.
    let mut advance_at = vec![0_i64; text.len()];
    for glyph in shaped.iter().flatten() {
        let slot = &mut advance_at[glyph.glyph.cluster];
        *slot = slot.saturating_add(glyph.advance);
    }
    let sum = |range: Range<usize>| {
        advance_at[range]
            .iter()
            .fold(0_i64, |total, &advance| total.saturating_add(advance))
    };
    let mut segments = Vec::new();
    let mut start = 0;
    for opportunity in line_breaks(text) {
        let end = opportunity.offset;
        let content_end = start + text[start..end].trim_end_matches(char::is_whitespace).len();
        segments.push(Segment {
            end,
            width: sum(start..end),
            hang: sum(content_end..end),
            mandatory: opportunity.mandatory,
        });
        start = end;
    }
    segments
}

/// A piece of a line: the part of one run that lies on the line and has one bidi level there.
struct Piece {
    run: usize,
    start: usize,
    end: usize,
    level: Level,
}

/// What shaping and measuring a paragraph's runs needs.
struct Shaper<'a> {
    text: &'a str,
    fonts: &'a [&'a Font],
    size: i64,
    language: &'a str,
    features: Features,
}

impl Shaper<'_> {
    /// Shapes `range` of `run` at the bidi `level` and measures its glyphs in BLU.
    fn shape(&self, run: &Run, range: Range<usize>, level: Level) -> Result<Vec<Measured>, String> {
        let font = self.fonts[run.font];
        let glyphs = shape(
            font,
            self.text,
            range,
            level.is_rtl(),
            run.script,
            self.language,
            self.features,
        )?;
        let units_per_em = i64::from(font.units_per_em());
        let blu = |units: i32| {
            bayan_units::scale(
                i64::from(units),
                self.size,
                units_per_em,
                Rounding::HalfEven,
            )
            .ok_or_else(|| format!("{}: a glyph metric overflows", font.file()))
        };
        glyphs
            .into_iter()
            .map(|glyph| {
                Ok(Measured {
                    glyph,
                    advance: blu(glyph.x_advance)?,
                    x_offset: blu(glyph.x_offset)?,
                    y_offset: blu(glyph.y_offset)?,
                })
            })
            .collect()
    }

    /// The glyphs of `piece`: the run's own where the line cuts it safely, otherwise the piece shaped again.
    fn piece_glyphs(
        &self,
        runs: &[Run],
        shaped: &[Vec<Measured>],
        piece: &Piece,
    ) -> Result<Vec<Measured>, String> {
        let run = &runs[piece.run];
        let glyphs = &shaped[piece.run];
        if piece.level.number() == run.level {
            // Cutting at `at` is safe if a glyph's cluster starts there and none of those glyphs is unsafe to break before.
            let safe = |at: usize| {
                at == run.start
                    || at == run.end
                    || (glyphs.iter().any(|glyph| glyph.glyph.cluster == at)
                        && !glyphs
                            .iter()
                            .any(|glyph| glyph.glyph.cluster == at && glyph.glyph.unsafe_to_break))
            };
            if safe(piece.start) && safe(piece.end) {
                return Ok(glyphs
                    .iter()
                    .filter(|glyph| (piece.start..piece.end).contains(&glyph.glyph.cluster))
                    .copied()
                    .collect());
            }
        }
        self.shape(run, piece.start..piece.end, piece.level)
    }

    /// Lays out the line `range`, whose box starts at `top`.
    #[expect(
        clippy::too_many_arguments,
        reason = "a private step of `lay_out`, which owns all of these"
    )]
    fn line(
        &self,
        bidi: &BidiInfo<'_>,
        bidi_paragraph: &unicode_bidi::ParagraphInfo,
        runs: &[Run],
        shaped: &[Vec<Measured>],
        range: Range<usize>,
        top: i64,
        paragraph: &Paragraph<'_>,
    ) -> Result<Line, String> {
        // The levels after rule L1 (whitespace at the end of the line takes the paragraph's level), and the level runs in display order (rule L2).
        let (levels, level_runs) = bidi.visual_runs(bidi_paragraph, range.clone());
        let mut pieces = Vec::new();
        for (index, run) in runs.iter().enumerate() {
            let start = run.start.max(range.start);
            let end = run.end.min(range.end);
            if start >= end {
                continue;
            }
            let mut piece_start = start;
            for (offset, _) in self.text[start..end].char_indices().skip(1) {
                let at = start + offset;
                if levels[at] != levels[piece_start] {
                    pieces.push(Piece {
                        run: index,
                        start: piece_start,
                        end: at,
                        level: levels[piece_start],
                    });
                    piece_start = at;
                }
            }
            pieces.push(Piece {
                run: index,
                start: piece_start,
                end,
                level: levels[piece_start],
            });
        }
        // Display order: the level runs as rule L2 orders them, and inside each the pieces in logical order, reversed for a right-to-left level.
        let mut ordered: Vec<&Piece> = Vec::with_capacity(pieces.len());
        for level_run in &level_runs {
            let mut inside: Vec<&Piece> = pieces
                .iter()
                .filter(|piece| piece.start >= level_run.start && piece.end <= level_run.end)
                .collect();
            if levels[level_run.start].is_rtl() {
                inside.reverse();
            }
            ordered.extend(inside);
        }
        let blu = |units: i32, font: &Font| {
            bayan_units::scale(
                i64::from(units),
                self.size,
                i64::from(font.units_per_em()),
                Rounding::HalfEven,
            )
            .ok_or_else(|| format!("{}: a font metric overflows", font.file()))
        };
        let mut ascent = 0_i64;
        let mut descent = 0_i64;
        let mut glyph_runs = Vec::with_capacity(ordered.len());
        for piece in ordered {
            let run = &runs[piece.run];
            let font = self.fonts[run.font];
            ascent = ascent.max(blu(font.ascent(), font)?);
            descent = descent.max(blu(font.descent(), font)?);
            glyph_runs.push((piece, self.piece_glyphs(runs, shaped, piece)?));
        }
        let advance = glyph_runs
            .iter()
            .flat_map(|(_, glyphs)| glyphs)
            .try_fold(0_i64, |total, glyph| total.checked_add(glyph.advance))
            .ok_or("a line is too wide")?;
        let left = if paragraph.rtl {
            paragraph
                .measure
                .0
                .checked_sub(advance)
                .ok_or("a line is too wide")?
        } else {
            0
        };
        let baseline = top.checked_add(ascent).ok_or("the paragraph is too high")?;
        let mut pen = left;
        let mut line_runs = Vec::with_capacity(glyph_runs.len());
        for (piece, glyphs) in glyph_runs {
            let run = &runs[piece.run];
            let mut placed = Vec::with_capacity(glyphs.len());
            for glyph in glyphs {
                placed.push(Glyph {
                    id: glyph.glyph.id,
                    cluster: glyph.glyph.cluster,
                    x: pen
                        .checked_add(glyph.x_offset)
                        .ok_or("a line is too wide")?,
                    y: baseline
                        .checked_sub(glyph.y_offset)
                        .ok_or("a glyph is too far off its line")?,
                    advance: glyph.advance,
                });
                pen = pen.checked_add(glyph.advance).ok_or("a line is too wide")?;
            }
            line_runs.push(LineRun {
                font: run.font,
                script: script_code(run.script)?,
                level: piece.level.number(),
                start: piece.start,
                end: piece.end,
                glyphs: placed,
            });
        }
        Ok(Line {
            start: range.start,
            end: range.end,
            top,
            ascent,
            descent,
            left,
            advance,
            runs: line_runs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::tests::committed_book;

    const PLAIN: Features = Features {
        kerning: true,
        ligatures: true,
    };

    fn paragraph<'a>(text: &'a str, rtl: bool, fonts: &'a [usize], measure: Blu) -> Paragraph<'a> {
        Paragraph {
            text,
            rtl,
            fonts,
            size: Blu::from_points(12),
            measure,
            language: if rtl { "he" } else { "th" },
            features: PLAIN,
        }
    }

    /// Every glyph's cluster in display order, line after line.
    fn visual_clusters(line: &Line) -> Vec<usize> {
        line.runs
            .iter()
            .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.cluster))
            .collect()
    }

    #[test]
    fn reorders_numbers_inside_right_to_left_text() {
        let book = committed_book();
        let layout = lay_out(&book, &paragraph("שלום 123", true, &[0], Blu::INCH * 4)).unwrap();
        assert_eq!(layout.lines.len(), 1);
        // Displayed from left to right: the number, then the word read from the right ("123 םולש").
        assert_eq!(
            visual_clusters(&layout.lines[0]),
            [9, 10, 11, 8, 6, 4, 2, 0]
        );
        let levels: Vec<u8> = layout.lines[0].runs.iter().map(|run| run.level).collect();
        assert_eq!(levels, [2, 1]);
    }

    #[test]
    fn places_glyphs_one_after_another_and_right_aligns_right_to_left_lines() {
        let book = committed_book();
        let measure = Blu::INCH * 4;
        // Without kerning: the font kerns ו against ם in right-to-left text by shortening the advance and moving the glyph by an offset, as HarfBuzz does.
        let mut unkerned = paragraph("שלום עולם", true, &[0], measure);
        unkerned.features.kerning = false;
        let layout = lay_out(&book, &unkerned).unwrap();
        let line = &layout.lines[0];
        let mut pen = line.left;
        for glyph in line.runs.iter().flat_map(|run| &run.glyphs) {
            // No glyph of this text is moved by an offset then, so each starts where the last one ended.
            assert_eq!(glyph.x, pen);
            assert_eq!(glyph.y, line.top + line.ascent);
            pen += glyph.advance;
        }
        assert_eq!(pen, line.left + line.advance);
        assert_eq!(line.left + line.advance, measure.0);
    }

    #[test]
    fn breaks_thai_lines_between_dictionary_words() {
        let book = committed_book();
        let text = "ภาษาไทยไม่มีการเว้นวรรคระหว่างคำจึงต้องใช้พจนานุกรมในการตัดคำ";
        let measure = Blu::INCH;
        let layout = lay_out(&book, &paragraph(text, false, &[1], measure)).unwrap();
        assert!(layout.lines.len() > 2, "{}", layout.lines.len());
        let opportunities: Vec<usize> = crate::segment::line_breaks(text)
            .iter()
            .map(|b| b.offset)
            .collect();
        let mut start = 0;
        for line in &layout.lines {
            assert_eq!(line.start, start);
            assert!(opportunities.contains(&line.end), "{}", line.end);
            assert!(
                line.advance <= measure.0,
                "{} > {}",
                line.advance,
                measure.0
            );
            start = line.end;
        }
        assert_eq!(start, text.len());
        // Line boxes are stacked without gaps, and the paragraph is as high as its lines.
        for pair in layout.lines.windows(2) {
            assert_eq!(pair[1].top, pair[0].top + pair[0].ascent + pair[0].descent);
        }
        let last = layout.lines.last().unwrap();
        assert_eq!(layout.height, last.top + last.ascent + last.descent);
    }

    #[test]
    fn measures_in_blu_from_font_units() {
        let book = committed_book();
        let font = &book.fonts()[0];
        let layout = lay_out(&book, &paragraph("ש", true, &[0], Blu::INCH)).unwrap();
        let line = &layout.lines[0];
        // 12 points is 304,800 BLU; the ascent of a 1,000-unit em scales by 304.8, rounded half to even.
        let scaled = |units: i32| {
            bayan_units::scale(i64::from(units), 304_800, 1_000, Rounding::HalfEven).unwrap()
        };
        assert_eq!(line.ascent, scaled(font.ascent()));
        assert_eq!(line.descent, scaled(font.descent()));
        assert_eq!(layout.size, 304_800);
        assert_eq!(layout.fonts[0].sha256, crate::hash::hex(font.sha256()));
    }

    #[test]
    fn writes_the_same_json_every_time() {
        let book = committed_book();
        let paragraph = paragraph("ภาษาไทย 2026", false, &[1, 0], Blu::INCH * 2);
        let first = lay_out(&book, &paragraph).unwrap().to_json().unwrap();
        let second = lay_out(&book, &paragraph).unwrap().to_json().unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with(b"{\"measure\":"));
        assert!(first.ends_with(b"}\n"));
    }

    #[test]
    fn refuses_what_it_cannot_lay_out() {
        let book = committed_book();
        assert!(lay_out(&book, &paragraph("", false, &[1], Blu::INCH)).is_err());
        assert!(lay_out(&book, &paragraph("א\nב", true, &[0], Blu::INCH)).is_err());
        assert!(lay_out(&book, &paragraph("א", true, &[7], Blu::INCH)).is_err());
        assert!(lay_out(&book, &paragraph("א", true, &[], Blu::INCH)).is_err());
    }
}
