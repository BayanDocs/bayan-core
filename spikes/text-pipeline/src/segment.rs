//! Segmentation with ICU4X's compiled data: line break opportunities (UAX #14), with the dictionary models for Thai, Lao, Khmer and Burmese and never the LSTM models, whose floating-point inference ADR-0009 §2 rules out (the crate is built without them); and grapheme clusters (UAX #29), the units fonts are chosen for.

use icu_properties::CodePointMapData;
use icu_properties::props::LineBreak;
use icu_segmenter::options::LineBreakOptions;
use icu_segmenter::{GraphemeClusterSegmenter, LineSegmenter};

/// A line break opportunity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Break {
    /// The byte offset in the text before which a line may (or must) start.
    pub offset: usize,
    /// Whether a line must end here, as after a line feed or a line separator.
    pub mandatory: bool,
}

/// The line break opportunities of `text`, in order, without the start of the text and with its end.
#[must_use]
pub fn line_breaks(text: &str) -> Vec<Break> {
    // The default options: strict line breaking, no word-level tailoring and no content locale (UAX #14 as published).
    let segmenter = LineSegmenter::new_dictionary(LineBreakOptions::default());
    let classes = CodePointMapData::<LineBreak>::new();
    segmenter
        .segment_str(text)
        .filter(|&offset| offset > 0)
        .map(|offset| Break {
            offset,
            // ICU4X does not say which breaks are mandatory (outside its unstable interface), so this is the test its documentation gives: a break after a hard line break character (UAX #14 classes BK, CR, LF and NL), and the end of the text (rule LB3).
            mandatory: offset == text.len()
                || text[..offset].chars().next_back().is_some_and(|c| {
                    matches!(
                        classes.get(c),
                        LineBreak::MandatoryBreak
                            | LineBreak::CarriageReturn
                            | LineBreak::LineFeed
                            | LineBreak::NextLine
                    )
                }),
        })
        .collect()
}

/// The grapheme cluster boundaries of `text`, in order, from 0 to `text.len()`.
#[must_use]
pub fn grapheme_boundaries(text: &str) -> Vec<usize> {
    let mut boundaries: Vec<usize> = GraphemeClusterSegmenter::new().segment_str(text).collect();
    // ICU4X reports no boundary at all for an empty text.
    if boundaries.is_empty() {
        boundaries.push(0);
    }
    boundaries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offsets(text: &str) -> Vec<usize> {
        line_breaks(text).iter().map(|b| b.offset).collect()
    }

    #[test]
    fn breaks_latin_text_after_spaces() {
        assert_eq!(offsets("Hello World"), [6, 11]);
    }

    #[test]
    fn breaks_thai_between_words_with_the_dictionary() {
        // ภาษา | ไทย | …: Thai has no spaces between words, so only a dictionary finds these (3 bytes per character).
        let breaks = offsets("ภาษาไทยง่ายนิดเดียว");
        assert_eq!(&breaks[..2], [12, 21]);
        assert_eq!(breaks.last(), Some(&57));
        let boundaries = grapheme_boundaries("ภาษาไทยง่ายนิดเดียว");
        assert!(breaks.iter().all(|offset| boundaries.contains(offset)));
    }

    #[test]
    fn breaks_between_ideographs() {
        assert_eq!(offsets("汉字排版"), [3, 6, 9, 12]);
    }

    #[test]
    fn marks_the_break_after_a_line_feed_as_mandatory() {
        let breaks = line_breaks("one two\nthree");
        assert_eq!(
            breaks[..2],
            [
                Break {
                    offset: 4,
                    mandatory: false
                },
                Break {
                    offset: 8,
                    mandatory: true
                },
            ]
        );
    }

    #[test]
    fn keeps_a_combining_mark_with_its_base() {
        assert_eq!(grapheme_boundaries("e\u{301}x"), [0, 3, 4]);
    }

    #[test]
    fn keeps_a_devanagari_conjunct_in_one_cluster() {
        // क्ष (ka, virama, ssa) is one extended grapheme cluster since Unicode 15.1 (rule GB9c).
        assert_eq!(grapheme_boundaries("क्षa"), [0, 9, 10]);
    }

    #[test]
    fn an_empty_text_has_no_breaks_and_one_boundary() {
        assert!(line_breaks("").is_empty());
        assert_eq!(grapheme_boundaries(""), [0]);
    }
}
