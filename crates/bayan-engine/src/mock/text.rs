//! A stand-in for text layout until the text pipeline exists (CORE-003): every character has a fixed advance, wide characters twice that, and lines break greedily at spaces.
//!
//! The words are drawn as grey bars ("greeked" text), and every character gets a box for the accessibility tree, so screen readers can tell where each character is.
//!
//! Right-to-left text is laid out from the column's right edge, with the parts that the caller marks left to right in reading order inside it: what the Unicode Bidirectional Algorithm (UAX #9) gives for a right-to-left paragraph whose left-to-right parts hold no right-to-left characters, its rule L1 included, which moves white space at a line's end to the paragraph's direction. The mock knows the direction of its parts from its content; the text pipeline will find it with the algorithm itself (ADR-0009).

use crate::protocol::{CharBox, Rect, TextDirection};
use crate::raster::{Rgb, Shape};

use super::geometry::inches;

/// How a kind of text is laid out and drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextStyle {
    /// The advance of a narrow character, in BLU.
    pub(crate) advance: i64,
    /// The distance from one line to the next.
    pub(crate) line_pitch: i64,
    /// The height of the bars that stand for words.
    pub(crate) bar_height: i64,
    /// The bars' colour.
    pub(crate) color: Rgb,
}

/// Body text: about 86 characters on a line of 6.5 inches.
pub(crate) const BODY: TextStyle = TextStyle {
    advance: inches(75, 1000),
    line_pitch: inches(2, 10),
    bar_height: inches(7, 100),
    color: Rgb {
        r: 95,
        g: 99,
        b: 104,
    },
};

/// Headings of level 1.
pub(crate) const HEADING_1: TextStyle = TextStyle {
    advance: inches(15, 100),
    line_pitch: inches(35, 100),
    bar_height: inches(14, 100),
    color: Rgb {
        r: 31,
        g: 95,
        b: 191,
    },
};

/// Headings of other levels.
pub(crate) const HEADING_2: TextStyle = TextStyle {
    advance: inches(1, 10),
    line_pitch: inches(25, 100),
    bar_height: inches(9, 100),
    color: Rgb {
        r: 31,
        g: 95,
        b: 191,
    },
};

/// Whether a character is wide, as East Asian characters are: a simplified version of the wide and fullwidth ranges of Unicode's East Asian Width property (UAX #11), good enough for a mock.
pub(crate) fn is_wide(character: char) -> bool {
    matches!(
        u32::from(character),
        0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1F64F
            | 0x1F900..=0x1F9FF
            | 0x20000..=0x3FFFD
    )
}

/// The advance of a character in a style.
pub(crate) fn advance(style: &TextStyle, character: char) -> i64 {
    if is_wide(character) {
        2 * style.advance
    } else {
        style.advance
    }
}

/// A character placed on a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Placed {
    /// Its first UTF-16 code unit in the text.
    pub(crate) start: u32,
    /// The code unit after it.
    pub(crate) end: u32,
    /// Its left edge, relative to the text's left edge.
    pub(crate) x: i64,
    /// Its advance.
    pub(crate) advance: i64,
    /// Whether it is white space, which is not drawn.
    pub(crate) space: bool,
    /// Whether it runs right to left.
    pub(crate) rtl: bool,
}

/// One line of laid-out text as hit-testing sees it (spec §14.4): its vertical extent and its characters in visual order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HitLine {
    /// The line's top edge, in page coordinates.
    pub(crate) top: i64,
    /// The edge below the line.
    pub(crate) bottom: i64,
    /// The offset where the line starts in its text: the caret position of a line without characters.
    pub(crate) start: u32,
    /// Where the caret of a line without characters stands, in page coordinates.
    pub(crate) empty_x: i64,
    /// The characters, from left to right, with their boxes.
    pub(crate) items: Vec<HitItem>,
}

/// A character of a [`HitLine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HitItem {
    /// The box's left edge, in page coordinates.
    pub(crate) left: i64,
    /// The edge right of the box.
    pub(crate) right: i64,
    /// The character's first UTF-16 code unit.
    pub(crate) start: u32,
    /// The code unit after it.
    pub(crate) end: u32,
    /// Whether it runs right to left, so that its right edge is the offset before it.
    pub(crate) rtl: bool,
}

/// Text laid out into lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextBlock {
    style: TextStyle,
    left: i64,
    top: i64,
    width: i64,
    /// The characters of each line, in reading order.
    lines: Vec<Vec<Placed>>,
}

/// The direction of a text, and the parts of it that run left to right inside right-to-left text, as UTF-16 ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Bidi<'a> {
    /// The text's direction.
    pub(crate) dir: TextDirection,
    /// In right-to-left text, the parts that run left to right.
    pub(crate) ltr: &'a [(u32, u32)],
}

/// Left-to-right text.
pub(crate) const LTR: Bidi<'static> = Bidi {
    dir: TextDirection::Ltr,
    ltr: &[],
};

/// Whether the UTF-16 range of a character lies inside one of `spans`.
fn inside(spans: &[(u32, u32)], start: u32, end: u32) -> bool {
    spans.iter().any(|&(from, to)| from <= start && end <= to)
}

impl TextBlock {
    /// Lays out left-to-right text in a column `width` wide whose top-left corner is at `left`, `top`. A word wider than the column is broken between characters. The layout lays every text out with [`TextBlock::directed`]; the tests use this shorthand.
    #[cfg(test)]
    pub(crate) fn new(text: &str, style: TextStyle, left: i64, top: i64, width: i64) -> Self {
        Self::directed(text, style, left, top, width, LTR)
    }

    /// Lays out text in a column as [`TextBlock::new`] does, in the direction of `bidi`. In right-to-left text, the characters inside `bidi.ltr` run left to right; each line starts at the column's right edge, and white space at a line's end goes to its left end.
    pub(crate) fn directed(
        text: &str,
        style: TextStyle,
        left: i64,
        top: i64,
        width: i64,
        bidi: Bidi<'_>,
    ) -> Self {
        let mut block = Self::broken(text, style, left, top, width);
        if bidi.dir == TextDirection::Rtl {
            for line in &mut block.lines {
                mirror(line, width, bidi.ltr);
            }
        }
        block
    }

    /// Breaks text into left-to-right lines.
    fn broken(text: &str, style: TextStyle, left: i64, top: i64, width: i64) -> Self {
        let mut characters = Vec::new();
        let mut offset = 0_u32;
        for character in text.chars() {
            let units = u32::try_from(character.len_utf16()).unwrap_or(1);
            characters.push((character, offset, offset + units));
            offset += units;
        }
        let mut lines: Vec<Vec<Placed>> = Vec::new();
        let mut line: Vec<Placed> = Vec::new();
        let mut x = 0;
        let mut index = 0;
        while let Some(&(character, _, _)) = characters.get(index) {
            if character.is_whitespace() {
                // Spaces stay on the line they follow, even beyond the column: they are not drawn.
                let (_, start, end) = characters[index];
                let step = advance(&style, character);
                line.push(Placed {
                    start,
                    end,
                    x,
                    advance: step,
                    space: true,
                    rtl: false,
                });
                x += step;
                index += 1;
                continue;
            }
            let word_end = characters[index..]
                .iter()
                .position(|&(character, _, _)| character.is_whitespace())
                .map_or(characters.len(), |position| index + position);
            let word_width: i64 = characters[index..word_end]
                .iter()
                .map(|&(character, _, _)| advance(&style, character))
                .sum();
            if x > 0 && x + word_width > width {
                lines.push(std::mem::take(&mut line));
                x = 0;
            }
            for &(character, start, end) in &characters[index..word_end] {
                let step = advance(&style, character);
                if x > 0 && x + step > width {
                    lines.push(std::mem::take(&mut line));
                    x = 0;
                }
                line.push(Placed {
                    start,
                    end,
                    x,
                    advance: step,
                    space: false,
                    rtl: false,
                });
                x += step;
            }
            index = word_end;
        }
        if !line.is_empty() || lines.is_empty() {
            lines.push(line);
        }
        Self {
            style,
            left,
            top,
            width,
            lines,
        }
    }

    /// The offsets at which the lines start, from 0 (spec §14.7).
    pub(crate) fn line_starts(&self) -> Vec<u32> {
        let mut starts: Vec<u32> = self
            .lines
            .iter()
            .filter_map(|line| line.first().map(|placed| placed.start))
            .collect();
        if starts.first() != Some(&0) {
            starts.insert(0, 0);
        }
        starts
    }

    /// The lines as hit-testing sees them, in page coordinates (spec §14.4).
    pub(crate) fn hit_lines(&self) -> Vec<HitLine> {
        self.lines()
            .map(|(top, line)| {
                let mut items: Vec<HitItem> = line
                    .iter()
                    .map(|placed| HitItem {
                        left: self.left + placed.x,
                        right: self.left + placed.x + placed.advance,
                        start: placed.start,
                        end: placed.end,
                        rtl: placed.rtl,
                    })
                    .collect();
                items.sort_by_key(|item| item.left);
                HitLine {
                    top,
                    bottom: top + self.style.line_pitch,
                    start: line.first().map_or(0, |placed| placed.start),
                    empty_x: self.left,
                    items,
                }
            })
            .collect()
    }

    /// The bottom edge of the last line.
    pub(crate) fn bottom(&self) -> i64 {
        self.top + self.line_count() * self.style.line_pitch
    }

    fn line_count(&self) -> i64 {
        i64::try_from(self.lines.len()).unwrap_or(i64::MAX)
    }

    /// The box around all lines, as wide as the column.
    pub(crate) fn bounds(&self) -> Rect {
        Rect {
            x: self.left,
            y: self.top,
            width: self.width,
            height: self.bottom() - self.top,
        }
    }

    /// The top edge of each line, with its characters.
    fn lines(&self) -> impl Iterator<Item = (i64, &Vec<Placed>)> {
        self.lines.iter().enumerate().map(|(index, line)| {
            let index = i64::try_from(index).unwrap_or(i64::MAX);
            (self.top + index * self.style.line_pitch, line)
        })
    }

    /// One bar for every word, centred on its line; a word whose direction changes inside it gets one bar for each direction.
    pub(crate) fn shapes(&self) -> Vec<Shape> {
        let mut shapes = Vec::new();
        let bar_top = (self.style.line_pitch - self.style.bar_height) / 2;
        for (top, line) in self.lines() {
            for word in line.split(|placed| placed.space) {
                for part in word.chunk_by(|one, other| one.rtl == other.rtl) {
                    let left = part.iter().map(|placed| placed.x).min();
                    let right = part.iter().map(|placed| placed.x + placed.advance).max();
                    if let (Some(left), Some(right)) = (left, right) {
                        shapes.push(Shape {
                            rect: Rect {
                                x: self.left + left,
                                y: top + bar_top,
                                width: right - left,
                                height: self.style.bar_height,
                            },
                            color: self.style.color,
                        });
                    }
                }
            }
        }
        shapes
    }

    /// The box of every character: its advance wide and the line's pitch high.
    pub(crate) fn char_boxes(&self) -> Vec<CharBox> {
        let mut boxes = Vec::new();
        for (top, line) in self.lines() {
            for placed in line {
                boxes.push(CharBox {
                    start: placed.start,
                    end: placed.end,
                    rect: Rect {
                        x: self.left + placed.x,
                        y: top,
                        width: placed.advance,
                        height: self.style.line_pitch,
                    },
                });
            }
        }
        boxes
    }
}

/// Lays one line out right to left in a column `width` wide: each character is mirrored from its left-to-right place, then every unbroken run of characters inside `ltr` is put back in reading order from the run's left edge, except the white space at the line's end, which runs right to left whatever it lies in (UAX #9 rule L1).
fn mirror(line: &mut [Placed], width: i64, ltr: &[(u32, u32)]) {
    let trailing = line.iter().rev().take_while(|placed| placed.space).count();
    let kept = line.len() - trailing;
    for (index, placed) in line.iter_mut().enumerate() {
        placed.x = width - placed.x - placed.advance;
        placed.rtl = index >= kept || !inside(ltr, placed.start, placed.end);
    }
    let mut index = 0;
    while index < kept {
        if line[index].rtl {
            index += 1;
            continue;
        }
        let end = (index..kept).find(|&next| line[next].rtl).unwrap_or(kept);
        let mut x = line[index..end]
            .iter()
            .map(|placed| placed.x)
            .min()
            .unwrap_or(0);
        for placed in &mut line[index..end] {
            placed.x = x;
            x += placed.advance;
        }
        index = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lays_right_to_left_text_out_from_the_right_with_left_to_right_parts_in_reading_order() {
        // Ten narrow characters fit on a line; "AB" (offsets 4 to 6) runs left to right inside right-to-left text. The first line is "abc AB ", the second "defg hij".
        let a = BODY.advance;
        let bidi = Bidi {
            dir: TextDirection::Rtl,
            ltr: &[(4, 6)],
        };
        let block = TextBlock::directed("abc AB defg hij", BODY, 0, 0, 10 * a, bidi);
        let lefts = |line: usize| -> Vec<(u32, i64, bool)> {
            block.lines[line]
                .iter()
                .map(|placed| (placed.start, placed.x, placed.rtl))
                .collect()
        };
        // "abc " from the right edge leftwards, then "AB" left to right in the next two places; the space at the line's end goes to its left end and runs right to left.
        assert_eq!(
            lefts(0),
            [
                (0, 9 * a, true),
                (1, 8 * a, true),
                (2, 7 * a, true),
                (3, 6 * a, true),
                (4, 4 * a, false),
                (5, 5 * a, false),
                (6, 3 * a, true),
            ]
        );
        assert_eq!(lefts(1)[0], (7, 9 * a, true));
        assert_eq!(lefts(1)[7], (14, 2 * a, true));
        assert_eq!(block.line_starts(), [0, 7]);
        // Left-to-right text is laid out as before.
        let plain = TextBlock::new("abc AB defg hij", BODY, 0, 0, 10 * a);
        assert_eq!(plain.lines[0][1].x, a);
        assert!(plain.lines.iter().flatten().all(|placed| !placed.rtl));
    }

    #[test]
    fn hit_lines_list_characters_left_to_right_with_their_direction() {
        let width = 10 * BODY.advance;
        let bidi = Bidi {
            dir: TextDirection::Rtl,
            ltr: &[(3, 5)],
        };
        let block = TextBlock::directed("ab CD", BODY, 100, 50, width, bidi);
        let lines = block.hit_lines();
        assert_eq!(lines.len(), 1);
        let order: Vec<(u32, bool)> = lines[0]
            .items
            .iter()
            .map(|item| (item.start, item.rtl))
            .collect();
        // Visually from the left: "CD" left to right, the space, then "ba".
        assert_eq!(
            order,
            [(3, false), (4, false), (2, true), (1, true), (0, true)]
        );
        assert_eq!(lines[0].items[4].right, 100 + width);
        assert_eq!((lines[0].top, lines[0].bottom), (50, 50 + BODY.line_pitch));
        // A word that changes direction inside it gets a bar for each direction: "ab" and "CD".
        assert_eq!(block.shapes().len(), 2);
    }

    #[test]
    fn breaks_lines_greedily_at_spaces() {
        // Ten narrow characters fit on a line.
        let width = 10 * BODY.advance;
        let block = TextBlock::new("aaaa bbbb cccc", BODY, 0, 0, width);
        assert_eq!(block.lines.len(), 2);
        // "aaaa bbbb " on the first line, "cccc" on the second.
        assert_eq!(block.lines[0].len(), 10);
        assert_eq!(block.lines[1].len(), 4);
        assert_eq!(block.lines[1][0].x, 0);
        assert_eq!(block.bottom(), 2 * BODY.line_pitch);
        assert_eq!(block.shapes().len(), 3);
    }

    #[test]
    fn breaks_words_longer_than_the_column() {
        let block = TextBlock::new("abcdefghij", BODY, 0, 0, 4 * BODY.advance);
        let lengths: Vec<usize> = block.lines.iter().map(Vec::len).collect();
        assert_eq!(lengths, [4, 4, 2]);
    }

    #[test]
    fn counts_utf16_code_units_and_widens_east_asian_characters() {
        let block = TextBlock::new("a日😀", BODY, 0, 0, 100 * BODY.advance);
        let boxes = block.char_boxes();
        let offsets: Vec<(u32, u32)> = boxes.iter().map(|b| (b.start, b.end)).collect();
        assert_eq!(offsets, [(0, 1), (1, 2), (2, 4)]);
        assert_eq!(boxes[1].rect.width, 2 * BODY.advance);
        assert_eq!(boxes[2].rect.x, 3 * BODY.advance);
    }

    #[test]
    fn empty_text_has_one_empty_line() {
        let block = TextBlock::new("", BODY, 5, 7, 100);
        assert_eq!(block.lines.len(), 1);
        assert!(block.shapes().is_empty());
        assert_eq!(block.bounds().height, BODY.line_pitch);
        assert_eq!(block.line_starts(), [0]);
        let lines = block.hit_lines();
        assert_eq!((lines.len(), lines[0].start, lines[0].empty_x), (1, 0, 5));
        assert!(lines[0].items.is_empty());
    }
}
