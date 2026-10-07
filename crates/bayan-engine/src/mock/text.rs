//! A stand-in for text layout until the text pipeline exists (CORE-003): every character has a fixed advance, wide characters twice that, and lines break greedily at spaces.
//!
//! The words are drawn as grey bars ("greeked" text), and every character gets a box for the accessibility tree, so screen readers can tell where each character is.

use crate::protocol::{CharBox, Rect};
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
}

/// Text laid out into lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextBlock {
    style: TextStyle,
    left: i64,
    top: i64,
    width: i64,
    /// The characters of each line.
    lines: Vec<Vec<Placed>>,
}

impl TextBlock {
    /// Lays out text in a column `width` wide whose top-left corner is at `left`, `top`. A word wider than the column is broken between characters.
    pub(crate) fn new(text: &str, style: TextStyle, left: i64, top: i64, width: i64) -> Self {
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

    /// One bar for every word, centred on its line.
    pub(crate) fn shapes(&self) -> Vec<Shape> {
        let mut shapes = Vec::new();
        let bar_top = (self.style.line_pitch - self.style.bar_height) / 2;
        for (top, line) in self.lines() {
            for word in line.split(|placed| placed.space) {
                if let (Some(first), Some(last)) = (word.first(), word.last()) {
                    shapes.push(Shape {
                        rect: Rect {
                            x: self.left + first.x,
                            y: top + bar_top,
                            width: last.x + last.advance - first.x,
                            height: self.style.bar_height,
                        },
                        color: self.style.color,
                    });
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

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
