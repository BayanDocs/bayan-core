//! The measurements of the mock document, in BLU (ADR-0005).

use crate::raster::Rgb;

/// BLU in an inch (ADR-0005 §1).
pub(crate) const BLU_PER_INCH: i64 = 1_828_800;

/// BLU in a device-independent pixel at zoom 1: 1/96 inch (spec §7).
pub(crate) const BLU_PER_DIP: i64 = 19_050;

/// `numerator / denominator` inches in BLU; the callers use fractions for which this is exact.
pub(crate) const fn inches(numerator: i64, denominator: i64) -> i64 {
    BLU_PER_INCH * numerator / denominator
}

/// The page: US Letter, 8.5 × 11 inches.
pub(crate) const PAGE_WIDTH: i64 = inches(17, 2);
/// The page's height.
pub(crate) const PAGE_HEIGHT: i64 = inches(11, 1);
/// The margins on all four sides: one inch.
pub(crate) const MARGIN: i64 = inches(1, 1);
/// The width between the margins.
pub(crate) const CONTENT_WIDTH: i64 = PAGE_WIDTH - 2 * MARGIN;
/// The gap between two pages in view space (spec §7).
pub(crate) const PAGE_GAP: i64 = inches(1, 4);

/// Space after a paragraph, a list or a table.
pub(crate) const PARAGRAPH_SPACING: i64 = inches(1, 10);
/// Space before a heading that does not start a page.
pub(crate) const HEADING_SPACING_BEFORE: i64 = inches(15, 100);
/// Space after a heading.
pub(crate) const HEADING_SPACING_AFTER: i64 = inches(5, 100);

/// Where a list item's text starts, from the left margin.
pub(crate) const LIST_TEXT_INDENT: i64 = inches(4, 10);
/// Where a bullet starts, from the left margin.
pub(crate) const BULLET_INDENT: i64 = inches(15, 100);
/// A bullet's width and height.
pub(crate) const BULLET_SIZE: i64 = inches(6, 100);
/// Space between list items.
pub(crate) const LIST_ITEM_SPACING: i64 = inches(25, 1000);

/// A table row's height.
pub(crate) const TABLE_ROW_HEIGHT: i64 = inches(3, 10);
/// The thickness of a table's rules: one device-independent pixel at zoom 1.
pub(crate) const TABLE_RULE: i64 = BLU_PER_DIP;
/// The space between a cell's edge and its text.
pub(crate) const CELL_PADDING: i64 = inches(8, 100);

/// The editable line's height.
pub(crate) const EDITABLE_HEIGHT: i64 = inches(36, 100);
/// The space between the editable line's edge and its text.
pub(crate) const EDITABLE_PADDING: i64 = inches(8, 100);
/// The thickness of the editable line's border.
pub(crate) const EDITABLE_BORDER: i64 = BLU_PER_DIP;
/// The space on each side of a character's box in the editable line.
pub(crate) const CHAR_GAP: i64 = BLU_PER_DIP / 2;
/// The height of a character's box in the editable line.
pub(crate) const CHAR_HEIGHT: i64 = inches(11, 100);
/// The thickness of the underline under a composition; twice that under its selected part.
pub(crate) const UNDERLINE: i64 = BLU_PER_DIP;
/// The caret's width.
pub(crate) const CARET_WIDTH: i64 = BLU_PER_DIP;

/// The page.
pub(crate) const PAPER: Rgb = Rgb {
    r: 255,
    g: 255,
    b: 255,
};
/// Bullets, table rules and page numbers.
pub(crate) const MUTED: Rgb = Rgb {
    r: 154,
    g: 160,
    b: 166,
};
/// The table's header row.
pub(crate) const HEADER_FILL: Rgb = Rgb {
    r: 232,
    g: 234,
    b: 237,
};
/// The editable line's background.
pub(crate) const EDITABLE_FILL: Rgb = Rgb {
    r: 255,
    g: 248,
    b: 220,
};
/// The editable line's border, and composition text and its underline.
pub(crate) const ACCENT: Rgb = Rgb {
    r: 26,
    g: 115,
    b: 232,
};
/// Committed text in the editable line.
pub(crate) const INK: Rgb = Rgb {
    r: 32,
    g: 33,
    b: 36,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measurements_are_exact_in_blu() {
        // Each fraction used above is an exact number of BLU.
        for (numerator, denominator) in [
            (17, 2),
            (1, 4),
            (1, 10),
            (15, 100),
            (5, 100),
            (4, 10),
            (6, 100),
            (25, 1000),
            (3, 10),
            (8, 100),
            (36, 100),
            (11, 100),
            (75, 1000),
            (2, 10),
            (7, 100),
            (35, 100),
            (14, 100),
            (25, 100),
            (9, 100),
        ] {
            assert_eq!(
                inches(numerator, denominator) * denominator,
                BLU_PER_INCH * numerator,
                "{numerator}/{denominator} inch"
            );
        }
        assert_eq!(PAGE_WIDTH, 15_544_800);
        assert_eq!(BLU_PER_DIP * 96, BLU_PER_INCH);
    }
}
