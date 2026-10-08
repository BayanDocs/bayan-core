//! Affine transforms in fixed point, for rotating, scaling, flipping and moving drawing objects.

use crate::rounding::div_round;
use crate::{Angle, Blu, Fixed, Point, Rounding};

/// An affine transform of the page: it maps a point (x, y) to
///
/// ```text
/// x′ = xx·x + xy·y + tx
/// y′ = yx·x + yy·y + ty
/// ```
///
/// The four coefficients are [`Fixed`] numbers and the translation is in BLU. Mapping a point computes this sum exactly and rounds once, as the caller says, so the result is the same on every platform. Composing two transforms rounds each new coefficient to a multiple of 2⁻³² and the new translation to a whole BLU, as the caller says. Compared with the exact composition, that moves the image of a point at most 22 inches from the origin on each axis by at most about 0.51 BLU when rounding to the nearest (0.01 BLU from the coefficients, 0.5 from the translation), or by less than 1.02 BLU when rounding down, up or toward zero.
///
/// With the page's y axis pointing down, [`Transform::rotation`] turns clockwise for positive angles, as DrawingML's `rot` does. A drawing object is rotated about its centre by moving the centre to the origin, rotating, and moving it back:
///
/// ```
/// use bayan_units::{Angle, Blu, Point, Rounding, Transform};
///
/// // A 200 × 100 shape whose centre is (100, 50), turned 90° clockwise.
/// let rounding = Rounding::HalfEven;
/// let turn = Transform::translation(Blu(-100), Blu(-50))
///     .then(Transform::rotation(Angle::from_degrees(90)), rounding)
///     .and_then(|t| t.then(Transform::translation(Blu(100), Blu(50)), rounding))
///     .unwrap();
/// // The top-left corner swings to the top right of the turned shape.
/// assert_eq!(turn.map_point(Point::ORIGIN, rounding), Some(Point::new(Blu(150), Blu(-50))));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Transform {
    /// How much x′ grows with x.
    pub xx: Fixed,
    /// How much y′ grows with x.
    pub yx: Fixed,
    /// How much x′ grows with y.
    pub xy: Fixed,
    /// How much y′ grows with y.
    pub yy: Fixed,
    /// Added to x′.
    pub tx: Blu,
    /// Added to y′.
    pub ty: Blu,
}

impl Transform {
    /// The transform that leaves every point where it is.
    pub const IDENTITY: Transform = Transform {
        xx: Fixed::ONE,
        yx: Fixed::ZERO,
        xy: Fixed::ZERO,
        yy: Fixed::ONE,
        tx: Blu::ZERO,
        ty: Blu::ZERO,
    };

    /// Moves every point by `dx` to the right and `dy` down.
    #[must_use]
    pub const fn translation(dx: Blu, dy: Blu) -> Transform {
        Transform {
            tx: dx,
            ty: dy,
            ..Transform::IDENTITY
        }
    }

    /// Scales x by `sx` and y by `sy`, from the origin. A negative factor flips: `scaling(-Fixed::ONE, Fixed::ONE)` is DrawingML's `flipH` about the origin.
    #[must_use]
    pub const fn scaling(sx: Fixed, sy: Fixed) -> Transform {
        Transform {
            xx: sx,
            yy: sy,
            ..Transform::IDENTITY
        }
    }

    /// Turns about the origin by `angle`, clockwise on the page for a positive angle.
    #[must_use]
    pub fn rotation(angle: Angle) -> Transform {
        let (sin, cos) = angle.sin_cos();
        Transform {
            xx: cos,
            yx: sin,
            xy: -sin,
            yy: cos,
            tx: Blu::ZERO,
            ty: Blu::ZERO,
        }
    }

    /// The transform that applies `self` first and then `next`, with its coefficients and translation rounded as `rounding` says, or `None` if a value does not fit.
    #[must_use]
    pub fn then(self, next: Transform, rounding: Rounding) -> Option<Transform> {
        // Each new coefficient is a sum of two products of coefficients, with 64 fraction digits; dividing by 2^32 leaves 32.
        let coefficient = |a: Fixed, b: Fixed, c: Fixed, d: Fixed| {
            let sum = wide(a.0)
                .checked_mul(wide(b.0))?
                .checked_add(wide(c.0).checked_mul(wide(d.0))?)?;
            let raw = div_round(sum, ONE, rounding)?;
            i64::try_from(raw).ok().map(Fixed)
        };
        Some(Transform {
            xx: coefficient(next.xx, self.xx, next.xy, self.yx)?,
            yx: coefficient(next.yx, self.xx, next.yy, self.yx)?,
            xy: coefficient(next.xx, self.xy, next.xy, self.yy)?,
            yy: coefficient(next.yx, self.xy, next.yy, self.yy)?,
            // `next` applied to the point that `self` moves the origin to.
            tx: affine(next.xx, self.tx, next.xy, self.ty, next.tx, rounding)?,
            ty: affine(next.yx, self.tx, next.yy, self.ty, next.ty, rounding)?,
        })
    }

    /// Where the transform maps `point`, each coordinate computed exactly and rounded once to a whole number of BLU as `rounding` says, or `None` if a coordinate does not fit.
    #[must_use]
    pub fn map_point(self, point: Point, rounding: Rounding) -> Option<Point> {
        Some(Point::new(
            affine(self.xx, point.x, self.xy, point.y, self.tx, rounding)?,
            affine(self.yx, point.x, self.yy, point.y, self.ty, rounding)?,
        ))
    }
}

/// 1 with 32 fraction digits.
const ONE: i128 = 1 << Fixed::FRACTIONAL_BITS;

fn wide(value: i64) -> i128 {
    i128::from(value)
}

/// `a·x + b·y + t`, computed exactly and rounded once to a whole number of BLU, or `None` if it does not fit.
fn affine(a: Fixed, x: Blu, b: Fixed, y: Blu, t: Blu, rounding: Rounding) -> Option<Blu> {
    // Each product of an i64 coefficient and an i64 length has 32 fraction digits and is below 2^126 in magnitude; checked additions catch the rare sums that would not fit in an i128.
    let sum = (wide(a.0) * wide(x.0))
        .checked_add(wide(b.0) * wide(y.0))?
        .checked_add(wide(t.0) * ONE)?;
    let blu = div_round(sum, ONE, rounding)?;
    i64::try_from(blu).ok().map(Blu)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: i64 = 4_294_967_296; // 2^32

    fn point(x: i64, y: i64) -> Point {
        Point::new(Blu(x), Blu(y))
    }

    fn map(transform: Transform, x: i64, y: i64) -> Option<Point> {
        transform.map_point(point(x, y), Rounding::HalfEven)
    }

    #[test]
    fn the_identity_leaves_points_alone() {
        assert_eq!(Transform::IDENTITY.xx, Fixed(ONE));
        assert_eq!(Transform::IDENTITY.yy, Fixed(ONE));
        for (x, y) in [(0, 0), (1, -1), (i64::MAX, i64::MIN), (1_828_800, 914_400)] {
            assert_eq!(map(Transform::IDENTITY, x, y), Some(point(x, y)));
        }
    }

    #[test]
    fn translations_move_points_exactly() {
        assert_eq!(
            map(Transform::translation(Blu(10), Blu(-5)), 1, 2),
            Some(point(11, -3))
        );
        assert_eq!(map(Transform::translation(Blu::MAX, Blu::ZERO), 1, 0), None);
    }

    #[test]
    fn scalings_round_once() {
        let half = Fixed(ONE / 2);
        let scale = Transform::scaling(Fixed(2 * ONE), half);
        assert_eq!(map(scale, 3, 7), Some(point(6, 4))); // 3.5 → 4 (half to even)
        assert_eq!(
            scale.map_point(point(3, 7), Rounding::Floor),
            Some(point(6, 3))
        );
        assert_eq!(
            scale.map_point(point(3, -7), Rounding::TowardZero),
            Some(point(6, -3))
        );
        let flip = Transform::scaling(-Fixed::ONE, Fixed::ONE);
        assert_eq!(map(flip, 5, 6), Some(point(-5, 6)));
    }

    #[test]
    fn rotations_turn_clockwise_on_the_page() {
        let quarter = Transform::rotation(Angle::from_degrees(90));
        assert_eq!(map(quarter, 1_000, 0), Some(point(0, 1_000))); // right → down
        assert_eq!(map(quarter, 0, 1_000), Some(point(-1_000, 0))); // down → left
        assert_eq!(
            map(Transform::rotation(Angle::from_degrees(180)), 3, -4),
            Some(point(-3, 4))
        );
        assert_eq!(
            map(Transform::rotation(Angle::from_degrees(-90)), 1_000, 0),
            Some(point(0, -1_000))
        );
        assert_eq!(
            map(Transform::rotation(Angle::ZERO), 7, 8),
            Some(point(7, 8))
        );
    }

    #[test]
    fn rotations_by_other_angles_round_once() {
        // cos 30° × 1,000,000 = 866,025.403…, sin 30° × 1,000,000 = 500,000 exactly.
        let thirty = Transform::rotation(Angle::from_degrees(30));
        assert_eq!(map(thirty, 1_000_000, 0), Some(point(866_025, 500_000)));
        assert_eq!(
            thirty.map_point(point(1_000_000, 0), Rounding::Ceiling),
            Some(point(866_026, 500_000))
        );
        // (0, 1,000,000) → (−500,000, 866,025.403…)
        assert_eq!(map(thirty, 0, 1_000_000), Some(point(-500_000, 866_025)));
    }

    #[test]
    fn composing_applies_the_first_transform_first() {
        let rounding = Rounding::HalfEven;
        let move_then_turn = Transform::translation(Blu(10), Blu(0))
            .then(Transform::rotation(Angle::from_degrees(90)), rounding)
            .unwrap();
        // (0, 0) → (10, 0) → (0, 10)
        assert_eq!(map(move_then_turn, 0, 0), Some(point(0, 10)));
        let turn_then_move = Transform::rotation(Angle::from_degrees(90))
            .then(Transform::translation(Blu(10), Blu(0)), rounding)
            .unwrap();
        // (0, 0) → (0, 0) → (10, 0)
        assert_eq!(map(turn_then_move, 0, 0), Some(point(10, 0)));
        // Two quarter turns make a half turn, exactly.
        let quarter = Transform::rotation(Angle::from_degrees(90));
        assert_eq!(
            quarter.then(quarter, rounding),
            Some(Transform::rotation(Angle::from_degrees(180)))
        );
        assert_eq!(Transform::IDENTITY.then(quarter, rounding), Some(quarter));
        assert_eq!(quarter.then(Transform::IDENTITY, rounding), Some(quarter));
    }

    #[test]
    fn composition_rounds_the_new_coefficients() {
        let third = Fixed(1_431_655_765); // 1/3, rounded down
        let scale = Transform::scaling(third, third);
        let nine_times = Transform::scaling(Fixed(9 * ONE), Fixed(9 * ONE));
        let composed = scale.then(scale, Rounding::Floor).unwrap();
        // (1,431,655,765 ÷ 2^32)² × 2^32 = 477,218,588.22…
        assert_eq!(composed.xx, Fixed(477_218_588));
        let composed = scale.then(scale, Rounding::Ceiling).unwrap();
        assert_eq!(composed.xx, Fixed(477_218_589));
        assert_eq!(
            Transform::scaling(third, third)
                .then(nine_times, Rounding::HalfEven)
                .unwrap()
                .xx,
            Fixed(12_884_901_885)
        );
    }

    #[test]
    fn refuses_results_that_do_not_fit() {
        let rounding = Rounding::HalfEven;
        let huge = Transform::scaling(Fixed::from_integer(1 << 30), Fixed::from_integer(1 << 30));
        assert_eq!(huge.then(huge, rounding), None);
        assert_eq!(huge.map_point(point(i64::MAX, 0), rounding), None);
        let far = Transform::translation(Blu::MAX, Blu::MAX);
        assert_eq!(far.then(far, rounding), None);
        assert_eq!(
            Transform::scaling(Fixed::from_integer(2), Fixed::ONE)
                .then(far, rounding)
                .map(|t| t.tx),
            Some(Blu::MAX)
        );
    }
}
