//! Hit-testing ([engine protocol specification][spec] §14.4): the text position at a point of a page.
//!
//! The answer comes from the same lines and character boxes that the accessibility tree reports (`chars`, spec §6.6), so that a shell's platform interface that must answer at once from those boxes, such as Qt's `offsetAtPoint`, gives the character that `query.hit` gives.
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use crate::protocol::Hit;

use super::text::HitLine;

/// The distance from `value` to the values from `low` up to `high`, excluded: 0 for a value among them.
fn distance(value: i64, low: i64, high: i64) -> i64 {
    if value < low {
        low - value
    } else if value >= high {
        value - high + 1
    } else {
        0
    }
}

/// The horizontal extent of a line: from its leftmost character's left edge to its rightmost character's right edge, or the place of the caret of a line without characters.
fn extent(line: &HitLine) -> (i64, i64) {
    match (line.items.first(), line.items.last()) {
        (Some(first), Some(last)) => (first.left, last.right),
        _ => (line.empty_x, line.empty_x + 1),
    }
}

/// The text position at the point `x`, `y` (page coordinates) among `lines`, each with the node whose text it shows, in reading order: in the line whose vertical extent is nearest to the point, and among lines equally near the one nearest horizontally, and the first in reading order on a tie, the caret position at the nearer edge of the character under or beside the point, and that character if its box holds the point. `None` if there are no lines.
pub(crate) fn hit<'a>(
    lines: impl IntoIterator<Item = (u64, &'a HitLine)>,
    x: i64,
    y: i64,
) -> Option<Hit> {
    let mut best: Option<((i64, i64), u64, &HitLine)> = None;
    for (node, line) in lines {
        let (left, right) = extent(line);
        let key = (distance(y, line.top, line.bottom), distance(x, left, right));
        if best.as_ref().is_none_or(|(nearest, _, _)| key < *nearest) {
            best = Some((key, node, line));
        }
    }
    let (_, node, line) = best?;
    Some(within(node, line, x, y))
}

/// The text position at the point in a line (spec §14.4): the offset at the nearer edge, in the line's visual order, of the character under or beside the point (the one nearest horizontally, the leftmost on a tie), so that in right-to-left text the right half of a character gives the offset before it; a point at a character's exact middle counts as its right half. The character itself if its box, half-open like every box, holds the point.
fn within(node: u64, line: &HitLine, x: i64, y: i64) -> Hit {
    let Some(item) = line
        .items
        .iter()
        .min_by_key(|item| distance(x, item.left, item.right))
    else {
        return Hit {
            node,
            caret: line.start,
            character: None,
        };
    };
    let left_half = x.saturating_mul(2) < item.left.saturating_add(item.right);
    let caret = if left_half == item.rtl {
        item.end
    } else {
        item.start
    };
    let inside = (line.top..line.bottom).contains(&y) && (item.left..item.right).contains(&x);
    Hit {
        node,
        caret,
        character: inside.then_some(item.start),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::text::HitItem;

    /// A line from 100 to 200, with "ab" left to right from 0 to 20, and "CD" right to left from 20 to 40 (D left of C).
    fn line() -> HitLine {
        let item = |left, start, rtl| HitItem {
            left,
            right: left + 10,
            start,
            end: start + 1,
            rtl,
        };
        HitLine {
            top: 100,
            bottom: 200,
            start: 0,
            empty_x: 0,
            items: vec![
                item(0, 0, false),
                item(10, 1, false),
                item(20, 3, true),
                item(30, 2, true),
            ],
        }
    }

    fn at(x: i64, y: i64) -> (u32, Option<u32>) {
        let line = line();
        let hit = hit([(7, &line)], x, y).unwrap();
        assert_eq!(hit.node, 7);
        (hit.caret, hit.character)
    }

    #[test]
    fn both_halves_of_a_left_to_right_character() {
        // "b" spans 10 to 20: its left half gives the offset before it, its right half the one after; the middle counts as the right half.
        assert_eq!(at(12, 150), (1, Some(1)));
        assert_eq!(at(15, 150), (2, Some(1)));
        assert_eq!(at(19, 150), (2, Some(1)));
    }

    #[test]
    fn both_halves_of_a_right_to_left_character() {
        // "C" (offset 2) spans 30 to 40: its right half gives the offset before it, its left half the one after.
        assert_eq!(at(38, 150), (2, Some(2)));
        assert_eq!(at(31, 150), (3, Some(2)));
        // "D" (offset 3) spans 20 to 30.
        assert_eq!(at(28, 150), (3, Some(3)));
        assert_eq!(at(21, 150), (4, Some(3)));
    }

    #[test]
    fn beside_a_line_and_between_lines_gives_a_caret_and_no_character() {
        // Left of the line: the left edge of its leftmost character; right of it: the right edge of its rightmost, which runs right to left.
        assert_eq!(at(-50, 150), (0, None));
        assert_eq!(at(90, 150), (2, None));
        // Above and below the line, over a character: the caret as at the line's height, but no character.
        assert_eq!(at(12, 99), (1, None));
        assert_eq!(at(12, 200), (1, None));
    }

    #[test]
    fn chooses_the_nearest_line_then_the_nearest_horizontally_then_reading_order() {
        let upper = line();
        let mut lower = line();
        lower.top = 261;
        lower.bottom = 361;
        // Nearer the upper line, whose last row is 199, than the lower line, whose first row is 261.
        let found = hit([(1, &upper), (2, &lower)], 5, 220).unwrap();
        assert_eq!((found.node, found.character), (1, None));
        // 31 from both: the first in reading order.
        assert_eq!(hit([(1, &upper), (2, &lower)], 5, 230).unwrap().node, 1);
        assert_eq!(hit([(1, &upper), (2, &lower)], 5, 231).unwrap().node, 2);
        // Lines side by side, such as cells of a table row: the one nearest horizontally.
        let mut right = line();
        for item in &mut right.items {
            item.left += 1000;
            item.right += 1000;
        }
        assert_eq!(hit([(1, &upper), (3, &right)], 900, 150).unwrap().node, 3);
        // A line without characters gives its start.
        let empty = HitLine {
            top: 0,
            bottom: 10,
            start: 0,
            empty_x: 50,
            items: Vec::new(),
        };
        assert_eq!(
            hit([(4, &empty)], 0, 5),
            Some(Hit {
                node: 4,
                caret: 0,
                character: None
            })
        );
        assert_eq!(hit(std::iter::empty(), 0, 0), None);
    }
}
