//! Points, sizes, rectangles and insets in BLU.
//!
//! Coordinates follow the page: x grows to the right and y grows downward, as in OOXML. Like [`Blu`]'s operators, the methods here panic on overflow in builds with overflow checks and wrap otherwise; lengths a document can hold (at most about 757 km, the largest DrawingML coordinate) never come near that.

use crate::Blu;

/// A position on the page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Point {
    /// The distance to the right of the origin.
    pub x: Blu,
    /// The distance below the origin.
    pub y: Blu,
}

impl Point {
    /// The origin, (0, 0).
    pub const ORIGIN: Point = Point::new(Blu::ZERO, Blu::ZERO);

    /// The point (`x`, `y`).
    #[must_use]
    pub const fn new(x: Blu, y: Blu) -> Point {
        Point { x, y }
    }

    /// This point moved `dx` to the right and `dy` down.
    #[must_use]
    pub fn translated(self, dx: Blu, dy: Blu) -> Point {
        Point::new(self.x + dx, self.y + dy)
    }
}

/// A width and a height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Size {
    /// The extent from left to right.
    pub width: Blu,
    /// The extent from top to bottom.
    pub height: Blu,
}

impl Size {
    /// No extent at all.
    pub const ZERO: Size = Size::new(Blu::ZERO, Blu::ZERO);

    /// The size `width` × `height`.
    #[must_use]
    pub const fn new(width: Blu, height: Blu) -> Size {
        Size { width, height }
    }

    /// Whether the size covers no area: its width or its height is zero or negative.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.width.0 <= 0 || self.height.0 <= 0
    }
}

/// A rectangle with sides parallel to the page's edges: its top-left corner and its size.
///
/// It contains the points from its left edge up to, but not including, its right edge, and from its top edge up to, but not including, its bottom edge, so two rectangles that share an edge do not overlap. A rectangle whose width or height is zero or negative is empty: it contains no point.
///
/// ```
/// use bayan_units::{Blu, Insets, Point, Rect, Size};
///
/// // A US Letter page with 1-inch margins.
/// let page = Rect::new(Point::ORIGIN, Size::new(Blu::from_twips(12_240), Blu::from_twips(15_840)));
/// let body = page.inset(Insets::uniform(Blu::INCH));
/// assert_eq!(body.left(), Blu::INCH);
/// assert_eq!(body.size.width, Blu::from_twips(9_360)); // 6.5 inches
/// assert!(body.contains(Point::new(Blu::INCH, Blu::INCH)));
/// assert!(!body.contains(Point::new(body.right(), Blu::INCH)));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    /// The top-left corner.
    pub origin: Point,
    /// The width and height.
    pub size: Size,
}

impl Rect {
    /// The rectangle with top-left corner `origin` and size `size`.
    #[must_use]
    pub const fn new(origin: Point, size: Size) -> Rect {
        Rect { origin, size }
    }

    /// The rectangle between the given edges.
    #[must_use]
    pub fn from_edges(left: Blu, top: Blu, right: Blu, bottom: Blu) -> Rect {
        Rect::new(Point::new(left, top), Size::new(right - left, bottom - top))
    }

    /// The x of the left edge.
    #[must_use]
    pub const fn left(self) -> Blu {
        self.origin.x
    }

    /// The y of the top edge.
    #[must_use]
    pub const fn top(self) -> Blu {
        self.origin.y
    }

    /// The x of the right edge: the left edge plus the width.
    #[must_use]
    pub fn right(self) -> Blu {
        self.origin.x + self.size.width
    }

    /// The y of the bottom edge: the top edge plus the height.
    #[must_use]
    pub fn bottom(self) -> Blu {
        self.origin.y + self.size.height
    }

    /// Whether the rectangle covers no area.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.size.is_empty()
    }

    /// Whether `point` lies inside: on or right of the left edge and left of the right edge, on or below the top edge and above the bottom edge.
    #[must_use]
    pub fn contains(self, point: Point) -> bool {
        self.left() <= point.x
            && point.x < self.right()
            && self.top() <= point.y
            && point.y < self.bottom()
    }

    /// The area both rectangles cover, or `None` if they do not overlap.
    #[must_use]
    pub fn intersection(self, other: Rect) -> Option<Rect> {
        let overlap = Rect::from_edges(
            self.left().max(other.left()),
            self.top().max(other.top()),
            self.right().min(other.right()),
            self.bottom().min(other.bottom()),
        );
        if overlap.is_empty() {
            None
        } else {
            Some(overlap)
        }
    }

    /// The smallest rectangle that covers both. An empty rectangle covers nothing, so the union with an empty rectangle is the other rectangle.
    #[must_use]
    pub fn union(self, other: Rect) -> Rect {
        if self.is_empty() {
            other
        } else if other.is_empty() {
            self
        } else {
            Rect::from_edges(
                self.left().min(other.left()),
                self.top().min(other.top()),
                self.right().max(other.right()),
                self.bottom().max(other.bottom()),
            )
        }
    }

    /// The rectangle with each edge moved inward by the matching inset, as page margins shrink a page to its text area.
    #[must_use]
    pub fn inset(self, insets: Insets) -> Rect {
        Rect::from_edges(
            self.left() + insets.left,
            self.top() + insets.top,
            self.right() - insets.right,
            self.bottom() - insets.bottom,
        )
    }

    /// The rectangle with each edge moved outward by the matching inset.
    #[must_use]
    pub fn outset(self, insets: Insets) -> Rect {
        Rect::from_edges(
            self.left() - insets.left,
            self.top() - insets.top,
            self.right() + insets.right,
            self.bottom() + insets.bottom,
        )
    }

    /// The rectangle moved `dx` to the right and `dy` down.
    #[must_use]
    pub fn translated(self, dx: Blu, dy: Blu) -> Rect {
        Rect::new(self.origin.translated(dx, dy), self.size)
    }
}

/// Distances from each of the four edges of a rectangle, such as page margins or cell padding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Insets {
    /// From the top edge.
    pub top: Blu,
    /// From the right edge.
    pub right: Blu,
    /// From the bottom edge.
    pub bottom: Blu,
    /// From the left edge.
    pub left: Blu,
}

impl Insets {
    /// No inset on any side.
    pub const ZERO: Insets = Insets::uniform(Blu::ZERO);

    /// The same inset on all four sides.
    #[must_use]
    pub const fn uniform(inset: Blu) -> Insets {
        Insets {
            top: inset,
            right: inset,
            bottom: inset,
            left: inset,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i64, top: i64, right: i64, bottom: i64) -> Rect {
        Rect::new(
            Point::new(Blu(left), Blu(top)),
            Size::new(Blu(right - left), Blu(bottom - top)),
        )
    }

    #[test]
    fn points_move() {
        assert_eq!(
            Point::new(Blu(1), Blu(2)).translated(Blu(10), Blu(-20)),
            Point::new(Blu(11), Blu(-18))
        );
        assert_eq!(Point::ORIGIN, Point::new(Blu(0), Blu(0)));
    }

    #[test]
    fn sizes_without_area_are_empty() {
        assert!(!Size::new(Blu(1), Blu(1)).is_empty());
        assert!(Size::ZERO.is_empty());
        assert!(Size::new(Blu(5), Blu(0)).is_empty());
        assert!(Size::new(Blu(-5), Blu(5)).is_empty());
    }

    #[test]
    fn rectangles_have_edges() {
        let r = rect(10, 20, 110, 70);
        assert_eq!(
            (r.left(), r.top(), r.right(), r.bottom()),
            (Blu(10), Blu(20), Blu(110), Blu(70))
        );
        assert_eq!(Rect::from_edges(Blu(10), Blu(20), Blu(110), Blu(70)), r);
        assert_eq!(r.size, Size::new(Blu(100), Blu(50)));
        assert!(!r.is_empty());
        assert!(rect(10, 20, 10, 70).is_empty());
        assert!(Rect::from_edges(Blu(5), Blu(5), Blu(4), Blu(6)).is_empty());
    }

    #[test]
    fn contains_its_top_left_but_not_its_bottom_right_edges() {
        let r = rect(0, 0, 10, 10);
        assert!(r.contains(Point::new(Blu(0), Blu(0))));
        assert!(r.contains(Point::new(Blu(9), Blu(9))));
        assert!(!r.contains(Point::new(Blu(10), Blu(5))));
        assert!(!r.contains(Point::new(Blu(5), Blu(10))));
        assert!(!r.contains(Point::new(Blu(-1), Blu(5))));
        assert!(!rect(0, 0, 0, 10).contains(Point::new(Blu(0), Blu(0))));
    }

    #[test]
    fn intersects_overlapping_rectangles_only() {
        let a = rect(0, 0, 10, 10);
        assert_eq!(a.intersection(rect(5, -5, 20, 5)), Some(rect(5, 0, 10, 5)));
        assert_eq!(a.intersection(a), Some(a));
        assert_eq!(a.intersection(rect(10, 0, 20, 10)), None); // sharing an edge is not overlapping
        assert_eq!(a.intersection(rect(20, 20, 30, 30)), None);
        assert_eq!(a.intersection(rect(2, 2, 2, 8)), None); // an empty rectangle
    }

    #[test]
    fn unites_into_the_bounding_box() {
        let a = rect(0, 0, 10, 10);
        assert_eq!(a.union(rect(20, -5, 30, 5)), rect(0, -5, 30, 10));
        assert_eq!(a.union(rect(100, 100, 100, 200)), a); // empty: covers nothing
        assert_eq!(Rect::default().union(a), a);
    }

    #[test]
    fn insets_and_outsets_move_each_edge() {
        let page = rect(0, 0, 12_240, 15_840);
        let margins = Insets {
            top: Blu(1_440),
            right: Blu(1_800),
            bottom: Blu(720),
            left: Blu(1_080),
        };
        let body = page.inset(margins);
        assert_eq!(body, rect(1_080, 1_440, 10_440, 15_120));
        assert_eq!(body.outset(margins), page);
        assert_eq!(
            page.inset(Insets::uniform(Blu(10))),
            rect(10, 10, 12_230, 15_830)
        );
        assert_eq!(
            Insets::uniform(Blu(3)),
            Insets {
                top: Blu(3),
                right: Blu(3),
                bottom: Blu(3),
                left: Blu(3)
            }
        );
        assert_eq!(page.inset(Insets::ZERO), page);
        // Margins wider than the page leave an empty rectangle, never a reflected one.
        assert!(
            rect(0, 0, 100, 100)
                .inset(Insets::uniform(Blu(60)))
                .is_empty()
        );
    }

    #[test]
    fn rectangles_move() {
        assert_eq!(
            rect(0, 0, 10, 10).translated(Blu(5), Blu(-5)),
            rect(5, -5, 15, 5)
        );
    }
}
