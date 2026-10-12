//! Greedy (first-fit) line breaking at UAX #14 opportunities, with no Word tailoring yet (ADR-0009 §4; Word's own rules are LAB-005's to find).
//!
//! The paragraph is cut at every break opportunity into segments, each measured in BLU. Lines are filled with whole segments, first fit: a line ends before the first segment that would make it wider than the measure. Whitespace at the end of a line hangs: it does not count when deciding whether the line fits (UAX #14 §LB7 keeps spaces before a break, and a space that would end a line is not measured). A segment that is wider than the measure on its own gets a line of its own and overflows it; there is no emergency breaking inside a segment yet.

/// The stretch of a paragraph between two consecutive break opportunities, measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    /// The byte offset in the paragraph where the segment ends, a break opportunity.
    pub end: usize,
    /// The advance of the whole segment, in BLU, including the whitespace at its end.
    pub width: i64,
    /// The advance of the whitespace at the segment's end, in BLU, which hangs when a line ends after it.
    pub hang: i64,
    /// Whether the break at its end is mandatory, as after a line separator.
    pub mandatory: bool,
}

/// Breaks a paragraph into lines no wider than `measure` BLU, where possible, and returns the byte offset at which each line ends, in order. No segments, no lines.
#[must_use]
pub fn break_lines(segments: &[Segment], measure: i64) -> Vec<usize> {
    let mut ends = Vec::new();
    // The advance of the segments on the current line so far, whitespace included, and where the last of them ends.
    let mut width = 0_i64;
    let mut line_end: Option<usize> = None;
    for segment in segments {
        if let Some(end) = line_end {
            // Saturating, so that a hostile width cannot wrap around into "fits".
            let needed = width
                .saturating_add(segment.width)
                .saturating_sub(segment.hang);
            if needed > measure {
                ends.push(end);
                width = 0;
            }
        }
        width = width.saturating_add(segment.width);
        line_end = Some(segment.end);
        if segment.mandatory {
            ends.push(segment.end);
            width = 0;
            line_end = None;
        }
    }
    if let Some(end) = line_end {
        ends.push(end);
    }
    ends
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Segments of the given (width, hang) pairs, each 10 bytes long, none mandatory.
    fn segments(sizes: &[(i64, i64)]) -> Vec<Segment> {
        sizes
            .iter()
            .enumerate()
            .map(|(index, &(width, hang))| Segment {
                end: 10 * (index + 1),
                width,
                hang,
                mandatory: false,
            })
            .collect()
    }

    #[test]
    fn keeps_everything_that_fits_on_one_line() {
        assert_eq!(
            break_lines(&segments(&[(30, 5), (30, 5), (20, 0)]), 100),
            [30]
        );
    }

    #[test]
    fn breaks_before_the_first_segment_that_does_not_fit() {
        // 30 + 30 + 30 fits in 100 (the last space hangs); a fourth segment does not.
        let lines = break_lines(&segments(&[(35, 5), (35, 5), (35, 5), (20, 0)]), 100);
        assert_eq!(lines, [30, 40]);
    }

    #[test]
    fn lets_the_whitespace_at_the_end_of_a_line_hang() {
        // The second segment's word ends exactly at the measure; its space would not fit, but hangs.
        assert_eq!(
            break_lines(&segments(&[(50, 10), (60, 10), (5, 0)]), 100),
            [20, 30]
        );
    }

    #[test]
    fn counts_the_whitespace_inside_a_line() {
        // Inside the line the first space counts: 50 + 51 is wider than 100.
        assert_eq!(break_lines(&segments(&[(50, 10), (51, 0)]), 100), [10, 20]);
    }

    #[test]
    fn a_line_exactly_as_wide_as_the_measure_fits() {
        assert_eq!(break_lines(&segments(&[(40, 0), (60, 0)]), 100), [20]);
    }

    #[test]
    fn gives_a_segment_wider_than_the_measure_a_line_of_its_own() {
        let lines = break_lines(&segments(&[(20, 5), (300, 5), (20, 0)]), 100);
        assert_eq!(lines, [10, 20, 30]);
    }

    #[test]
    fn ends_a_line_at_a_mandatory_break() {
        let mut segments = segments(&[(10, 0), (10, 0), (10, 0)]);
        segments[0].mandatory = true;
        assert_eq!(break_lines(&segments, 100), [10, 30]);
    }

    #[test]
    fn a_paragraph_without_segments_has_no_lines() {
        assert!(break_lines(&[], 100).is_empty());
    }
}
