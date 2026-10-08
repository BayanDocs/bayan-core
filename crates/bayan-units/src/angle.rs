//! Angles in DrawingML's unit, and their sine and cosine computed deterministically.

use core::ops::{Add, Neg, Sub};

use crate::Fixed;

/// An angle in sixty-thousandths of a degree, the unit of DrawingML (`ST_Angle`): `Angle(60_000)` is one degree. Positive angles turn clockwise on the page, as in DrawingML, where the y axis points down.
///
/// The operators (`+`, `-`, unary `-`) overflow like those of [`Blu`](crate::Blu). DrawingML's angles are 32-bit integers, far from the limits of an `i64`, but [`parse_angle`](crate::ooxml::parse_angle) accepts any value that fits in an `i64`, so check an angle from a document against its schema type's range before computing with it.
///
/// [`Angle::sin`], [`Angle::cos`] and [`Angle::sin_cos`] replace the platform's floating-point `sin` and `cos`, which give different results on different platforms and are forbidden in bayan-core (ADR-0005 §4). They use integer arithmetic only, so they return the same bits everywhere, and each result is the exact sine or cosine rounded to the nearest multiple of 2⁻³². Special angles come out exact: the sine of 30° is exactly ½, and the sine and cosine of every multiple of 90° are exactly 0, 1 or −1.
///
/// ```
/// use bayan_units::{Angle, Fixed};
///
/// assert_eq!(Angle::from_degrees(30).sin(), Fixed(1 << 31)); // exactly ½
/// assert_eq!(Angle::from_degrees(-90).sin(), -Fixed::ONE);
/// assert_eq!(Angle::from_degrees(45).sin(), Angle::from_degrees(45).cos());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Angle(pub i64);

impl Angle {
    /// No turn.
    pub const ZERO: Angle = Angle(0);
    /// One degree: 60,000.
    pub const DEGREE: Angle = Angle(60_000);
    /// A right angle, 90°: 5,400,000.
    pub const RIGHT_ANGLE: Angle = Angle(5_400_000);
    /// Half a turn, 180°: 10,800,000.
    pub const HALF_TURN: Angle = Angle(10_800_000);
    /// A full turn, 360°: 21,600,000.
    pub const FULL_TURN: Angle = Angle(21_600_000);

    /// `degrees` degrees, exactly.
    #[must_use]
    pub const fn from_degrees(degrees: i32) -> Angle {
        // |degrees| ≤ 2^31, so the product stays below 2^47. `as` widens losslessly; `i64::from` is not available in a `const fn`.
        Angle(degrees as i64 * Angle::DEGREE.0)
    }

    /// The same direction as an angle from 0° up to, but not including, 360°.
    #[must_use]
    pub const fn normalized(self) -> Angle {
        Angle(self.0.rem_euclid(Angle::FULL_TURN.0))
    }

    /// The sine, rounded to the nearest multiple of 2⁻³².
    #[must_use]
    pub fn sin(self) -> Fixed {
        self.sin_cos().0
    }

    /// The cosine, rounded to the nearest multiple of 2⁻³².
    #[must_use]
    pub fn cos(self) -> Fixed {
        self.sin_cos().1
    }

    /// The sine and the cosine, each rounded to the nearest multiple of 2⁻³².
    #[must_use]
    pub fn sin_cos(self) -> (Fixed, Fixed) {
        crate::trig::sin_cos(self)
    }
}

impl Add for Angle {
    type Output = Angle;

    fn add(self, rhs: Angle) -> Angle {
        Angle(self.0 + rhs.0)
    }
}

impl Sub for Angle {
    type Output = Angle;

    fn sub(self, rhs: Angle) -> Angle {
        Angle(self.0 - rhs.0)
    }
}

impl Neg for Angle {
    type Output = Angle;

    fn neg(self) -> Angle {
        Angle(-self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: i64 = 4_294_967_296; // 2^32

    #[test]
    fn has_the_units_of_drawingml() {
        assert_eq!(Angle::DEGREE, Angle(60_000));
        assert_eq!(Angle::RIGHT_ANGLE, Angle(5_400_000));
        assert_eq!(Angle::HALF_TURN, Angle(10_800_000));
        assert_eq!(Angle::FULL_TURN, Angle(21_600_000));
        assert_eq!(Angle::from_degrees(90), Angle::RIGHT_ANGLE);
        assert_eq!(Angle::from_degrees(-45), Angle(-2_700_000));
        assert_eq!(
            Angle::from_degrees(i32::MIN),
            Angle(-2_147_483_648 * 60_000)
        );
    }

    #[test]
    fn normalizes_to_one_turn() {
        assert_eq!(Angle(-1).normalized(), Angle(21_599_999));
        assert_eq!(Angle::FULL_TURN.normalized(), Angle::ZERO);
        assert_eq!(Angle::from_degrees(-360).normalized(), Angle::ZERO);
        assert_eq!(Angle::from_degrees(450).normalized(), Angle::RIGHT_ANGLE);
        assert_eq!(Angle(i64::MAX).normalized(), Angle(4_375_807)); // (2^63 − 1) mod 21,600,000
        assert_eq!(Angle(i64::MIN).normalized(), Angle(17_224_192)); // −2^63 mod 21,600,000
    }

    #[test]
    fn adds_and_subtracts_angles() {
        assert_eq!(Angle::RIGHT_ANGLE + Angle::DEGREE, Angle(5_460_000));
        assert_eq!(Angle::ZERO - Angle::DEGREE, Angle(-60_000));
        assert_eq!(-Angle::HALF_TURN, Angle(-10_800_000));
    }

    /// Expected values from an 80-digit decimal computation in Python (Machin's formula for π, Taylor series), independent of this crate: the exact value times 2^32, rounded to the nearest integer.
    #[test]
    fn matches_high_precision_values() {
        let table = [
            (Angle(0), 0, ONE),
            (Angle(1), 1_249, ONE),
            (Angle(30_000), 37_480_185, 4_294_803_757),
            (Angle(60_000), 74_957_515, 4_294_313_152),
            (Angle(740_700), 918_254_088, 4_195_658_888),
            (Angle(1_800_000), 2_147_483_648, 3_719_550_787),
            (Angle(2_699_999), 3_036_999_617, 3_037_001_383),
            (Angle(2_700_000), 3_037_000_500, 3_037_000_500),
            (Angle(3_600_000), 3_719_550_787, 2_147_483_648),
            (Angle(5_340_000), 4_294_313_152, 74_957_515),
            (Angle(5_400_000), ONE, 0),
        ];
        for (angle, sin, cos) in table {
            assert_eq!(angle.sin_cos(), (Fixed(sin), Fixed(cos)), "{angle:?}");
            assert_eq!(angle.sin(), Fixed(sin), "{angle:?}");
            assert_eq!(angle.cos(), Fixed(cos), "{angle:?}");
        }
    }

    #[test]
    fn multiples_of_a_right_angle_are_exact() {
        for (quarter_turns, sin, cos) in [
            (0, 0, ONE),
            (1, ONE, 0),
            (2, 0, -ONE),
            (3, -ONE, 0),
            (4, 0, ONE),
            (-1, -ONE, 0),
            (-2, 0, -ONE),
            (7, -ONE, 0),
        ] {
            let angle = Angle(quarter_turns * 5_400_000);
            assert_eq!(
                angle.sin_cos(),
                (Fixed(sin), Fixed(cos)),
                "{quarter_turns} quarter turns"
            );
        }
    }

    #[test]
    fn follows_the_symmetries_of_the_circle_exactly() {
        for units in [
            1, 59_999, 740_700, 1_800_000, 2_699_999, 2_700_001, 4_000_000, 5_399_999,
        ] {
            let angle = Angle(units);
            let (sin, cos) = angle.sin_cos();
            assert_eq!((-angle).sin_cos(), (-sin, cos), "−{units}");
            assert_eq!(
                (Angle::RIGHT_ANGLE - angle).sin_cos(),
                (cos, sin),
                "90° − {units}"
            );
            assert_eq!(
                (Angle::RIGHT_ANGLE + angle).sin_cos(),
                (cos, -sin),
                "90° + {units}"
            );
            assert_eq!(
                (Angle::HALF_TURN - angle).sin_cos(),
                (sin, -cos),
                "180° − {units}"
            );
            assert_eq!(
                (Angle::HALF_TURN + angle).sin_cos(),
                (-sin, -cos),
                "180° + {units}"
            );
            assert_eq!(
                Angle(units + 3 * 21_600_000).sin_cos(),
                (sin, cos),
                "{units} + 3 turns"
            );
            assert_eq!(
                Angle(units - 5 * 21_600_000).sin_cos(),
                (sin, cos),
                "{units} − 5 turns"
            );
        }
    }

    #[test]
    fn works_for_every_angle_an_i64_can_hold() {
        assert_eq!(
            Angle(i64::MAX).sin_cos(),
            Angle(i64::MAX).normalized().sin_cos()
        );
        assert_eq!(
            Angle(i64::MIN).sin_cos(),
            Angle(i64::MIN).normalized().sin_cos()
        );
    }
}
