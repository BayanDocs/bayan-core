//! The Bayan Layout Unit.

use core::fmt;
use core::iter::Sum;
use core::ops::{Add, AddAssign, Mul, MulAssign, Neg, Sub, SubAssign};

use crate::rounding::div_round_by_positive;
use crate::{Dpi, LengthUnit, Rounding};

/// A length in Bayan Layout Units (BLU): 1 BLU = 1/1,828,800 inch = 1/25,400 point (ADR-0005).
///
/// Every unit OOXML and Word use is a whole number of BLU (see [`LengthUnit`]), so a length read from a document is stored exactly, and layout arithmetic is exact integer arithmetic. The range of an `i64` is about ±128 million kilometres.
///
/// The field is the number of BLU; `Blu(25_400)` is one point.
///
/// # Overflow
///
/// The operators (`+`, `-`, unary `-`, and `*` with an `i64`) behave exactly like the same operators on `i64`: when the result does not fit, they panic in builds with overflow checks (debug builds, by default) and wrap around otherwise. Realistic documents stay far from the limits, but a value read from a file can be anything that fits (the schema sets no upper limit for WordprocessingML's measurements in twips), so code that handles such values uses the `checked_` or `saturating_` methods, or first brings them into the range Word itself accepts.
///
/// There is deliberately no `/` operator: dividing a length almost never gives a whole number of BLU, and integer division would round toward zero without saying so. Use [`Blu::scale`] with an explicit [`Rounding`] instead, as in `length.scale(1, 2, Rounding::HalfEven)` for half a length (ADR-0005 §2).
///
/// ```
/// use bayan_units::{Blu, LengthUnit, Rounding};
///
/// let margin = Blu::from_twips(1_440); // 1 inch
/// assert_eq!(margin, Blu::INCH);
/// assert_eq!(margin + Blu::from_points(36), Blu::from_points(108)); // 1 inch is 72 points
/// assert_eq!(margin.to_units_exact(LengthUnit::Emu), Some(914_400));
/// assert_eq!(Blu(1_000).to_units(LengthUnit::Twip, Rounding::HalfEven), 1); // 0.787… twip
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Blu(pub i64);

impl Blu {
    /// Zero.
    pub const ZERO: Blu = Blu(0);
    /// The smallest length an `i64` can hold.
    pub const MIN: Blu = Blu(i64::MIN);
    /// The largest length an `i64` can hold.
    pub const MAX: Blu = Blu(i64::MAX);
    /// One inch: 1,828,800 BLU.
    pub const INCH: Blu = Blu(crate::unit::BLU_PER_INCH);
    /// One pica, 12 points: 304,800 BLU.
    pub const PICA: Blu = Blu(304_800);
    /// One point, 1/72 inch: 25,400 BLU.
    pub const POINT: Blu = Blu(25_400);
    /// Half a point: 12,700 BLU.
    pub const HALF_POINT: Blu = Blu(12_700);
    /// An eighth of a point: 3,175 BLU.
    pub const EIGHTH_POINT: Blu = Blu(3_175);
    /// One twip, 1/20 point: 1,270 BLU.
    pub const TWIP: Blu = Blu(1_270);
    /// One English Metric Unit, 1/914,400 inch: 2 BLU.
    pub const EMU: Blu = Blu(2);
    /// One centimetre: 720,000 BLU.
    pub const CENTIMETRE: Blu = Blu(720_000);
    /// One millimetre: 72,000 BLU.
    pub const MILLIMETRE: Blu = Blu(72_000);

    /// `count` of `unit`, exactly. Every unit is at most 1,828,800 BLU, so no `i32` count can overflow.
    #[must_use]
    pub const fn from_units(count: i32, unit: LengthUnit) -> Blu {
        // |count| ≤ 2^31 and a unit is at most 1,828,800 < 2^21 BLU, so the product stays below 2^52. `as` widens losslessly; `i64::from` is not available in a `const fn`.
        Blu(count as i64 * unit.blu().0)
    }

    /// `count` of `unit`, exactly, or `None` if the result does not fit in an `i64`.
    #[must_use]
    pub const fn checked_from_units(count: i64, unit: LengthUnit) -> Option<Blu> {
        match count.checked_mul(unit.blu().0) {
            Some(blu) => Some(Blu(blu)),
            None => None,
        }
    }

    /// `inches` inches, exactly.
    #[must_use]
    pub const fn from_inches(inches: i32) -> Blu {
        Blu::from_units(inches, LengthUnit::Inch)
    }

    /// `points` points, exactly.
    #[must_use]
    pub const fn from_points(points: i32) -> Blu {
        Blu::from_units(points, LengthUnit::Point)
    }

    /// `half_points` half-points (WordprocessingML font sizes), exactly.
    #[must_use]
    pub const fn from_half_points(half_points: i32) -> Blu {
        Blu::from_units(half_points, LengthUnit::HalfPoint)
    }

    /// `eighth_points` eighths of a point (WordprocessingML border widths), exactly.
    #[must_use]
    pub const fn from_eighth_points(eighth_points: i32) -> Blu {
        Blu::from_units(eighth_points, LengthUnit::EighthPoint)
    }

    /// `twips` twips, exactly.
    #[must_use]
    pub const fn from_twips(twips: i32) -> Blu {
        Blu::from_units(twips, LengthUnit::Twip)
    }

    /// `emus` English Metric Units (DrawingML), exactly. DrawingML coordinates can exceed the `i32` range; use [`Blu::checked_from_units`] for those.
    #[must_use]
    pub const fn from_emus(emus: i32) -> Blu {
        Blu::from_units(emus, LengthUnit::Emu)
    }

    /// `centimetres` centimetres, exactly.
    #[must_use]
    pub const fn from_centimetres(centimetres: i32) -> Blu {
        Blu::from_units(centimetres, LengthUnit::Centimetre)
    }

    /// `millimetres` millimetres, exactly.
    #[must_use]
    pub const fn from_millimetres(millimetres: i32) -> Blu {
        Blu::from_units(millimetres, LengthUnit::Millimetre)
    }

    /// `dots` pixels or printer dots at the resolution `dpi`, exactly.
    #[must_use]
    pub const fn from_dots(dots: i32, dpi: Dpi) -> Blu {
        Blu::from_units(dots, LengthUnit::Dot(dpi))
    }

    /// This length as a whole number of `unit`, rounded as `rounding` says. It cannot overflow: no unit is smaller than one BLU.
    #[must_use]
    pub const fn to_units(self, unit: LengthUnit, rounding: Rounding) -> i64 {
        div_round_by_positive(self.0, unit.blu().0, rounding)
    }

    /// This length as a whole number of `unit`, or `None` if it is not one.
    #[must_use]
    pub const fn to_units_exact(self, unit: LengthUnit) -> Option<i64> {
        let size = unit.blu().0;
        if self.0 % size == 0 {
            Some(self.0 / size)
        } else {
            None
        }
    }

    /// `self × numerator ÷ denominator`, computed exactly and rounded once (see [`scale`](crate::scale)), or `None` if `denominator` is zero or the result does not fit.
    #[must_use]
    pub fn scale(self, numerator: i64, denominator: i64, rounding: Rounding) -> Option<Blu> {
        crate::scale(self.0, numerator, denominator, rounding).map(Blu)
    }

    /// `self + rhs`, or `None` on overflow.
    #[must_use]
    pub const fn checked_add(self, rhs: Blu) -> Option<Blu> {
        match self.0.checked_add(rhs.0) {
            Some(sum) => Some(Blu(sum)),
            None => None,
        }
    }

    /// `self - rhs`, or `None` on overflow.
    #[must_use]
    pub const fn checked_sub(self, rhs: Blu) -> Option<Blu> {
        match self.0.checked_sub(rhs.0) {
            Some(difference) => Some(Blu(difference)),
            None => None,
        }
    }

    /// `self × rhs`, or `None` on overflow.
    #[must_use]
    pub const fn checked_mul(self, rhs: i64) -> Option<Blu> {
        match self.0.checked_mul(rhs) {
            Some(product) => Some(Blu(product)),
            None => None,
        }
    }

    /// `-self`, or `None` for [`Blu::MIN`], whose negation does not fit.
    #[must_use]
    pub const fn checked_neg(self) -> Option<Blu> {
        match self.0.checked_neg() {
            Some(negated) => Some(Blu(negated)),
            None => None,
        }
    }

    /// The absolute value, or `None` for [`Blu::MIN`], whose absolute value does not fit.
    #[must_use]
    pub const fn checked_abs(self) -> Option<Blu> {
        match self.0.checked_abs() {
            Some(magnitude) => Some(Blu(magnitude)),
            None => None,
        }
    }

    /// `self + rhs`, or the nearest limit ([`Blu::MIN`] or [`Blu::MAX`]) on overflow.
    #[must_use]
    pub const fn saturating_add(self, rhs: Blu) -> Blu {
        Blu(self.0.saturating_add(rhs.0))
    }

    /// `self - rhs`, or the nearest limit on overflow.
    #[must_use]
    pub const fn saturating_sub(self, rhs: Blu) -> Blu {
        Blu(self.0.saturating_sub(rhs.0))
    }

    /// `self × rhs`, or the nearest limit on overflow.
    #[must_use]
    pub const fn saturating_mul(self, rhs: i64) -> Blu {
        Blu(self.0.saturating_mul(rhs))
    }

    /// The absolute value. Like `i64::abs`, it overflows for [`Blu::MIN`] (see "Overflow" above).
    #[must_use]
    pub const fn abs(self) -> Blu {
        Blu(self.0.abs())
    }
}

impl Add for Blu {
    type Output = Blu;

    fn add(self, rhs: Blu) -> Blu {
        Blu(self.0 + rhs.0)
    }
}

impl Sub for Blu {
    type Output = Blu;

    fn sub(self, rhs: Blu) -> Blu {
        Blu(self.0 - rhs.0)
    }
}

impl Neg for Blu {
    type Output = Blu;

    fn neg(self) -> Blu {
        Blu(-self.0)
    }
}

impl Mul<i64> for Blu {
    type Output = Blu;

    fn mul(self, rhs: i64) -> Blu {
        Blu(self.0 * rhs)
    }
}

impl Mul<Blu> for i64 {
    type Output = Blu;

    fn mul(self, rhs: Blu) -> Blu {
        Blu(self * rhs.0)
    }
}

impl AddAssign for Blu {
    fn add_assign(&mut self, rhs: Blu) {
        *self = *self + rhs;
    }
}

impl SubAssign for Blu {
    fn sub_assign(&mut self, rhs: Blu) {
        *self = *self - rhs;
    }
}

impl MulAssign<i64> for Blu {
    fn mul_assign(&mut self, rhs: i64) {
        *self = *self * rhs;
    }
}

impl Sum for Blu {
    fn sum<I: Iterator<Item = Blu>>(iter: I) -> Blu {
        iter.fold(Blu::ZERO, Add::add)
    }
}

impl<'a> Sum<&'a Blu> for Blu {
    fn sum<I: Iterator<Item = &'a Blu>>(iter: I) -> Blu {
        iter.copied().sum()
    }
}

/// Writes the exact number of BLU followed by the unit, as in `25400 BLU`.
impl fmt::Display for Blu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} BLU", self.0)
    }
}

/// Serializes as the number of BLU, an `i64`.
impl serde_core::Serialize for Blu {
    fn serialize<S: serde_core::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(self.0)
    }
}

/// Deserializes from any integer that fits in an `i64`, read as a number of BLU.
impl<'de> serde_core::Deserialize<'de> for Blu {
    fn deserialize<D: serde_core::Deserializer<'de>>(deserializer: D) -> Result<Blu, D::Error> {
        <i64 as serde_core::Deserialize>::deserialize(deserializer).map(Blu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_test::{Token, assert_de_tokens, assert_tokens};

    #[test]
    fn has_a_constant_for_every_unit_of_the_table() {
        assert_eq!(Blu::INCH, Blu(1_828_800));
        assert_eq!(Blu::PICA, Blu(304_800));
        assert_eq!(Blu::POINT, Blu(25_400));
        assert_eq!(Blu::HALF_POINT, Blu(12_700));
        assert_eq!(Blu::EIGHTH_POINT, Blu(3_175));
        assert_eq!(Blu::TWIP, Blu(1_270));
        assert_eq!(Blu::EMU, Blu(2));
        assert_eq!(Blu::CENTIMETRE, Blu(720_000));
        assert_eq!(Blu::MILLIMETRE, Blu(72_000));
    }

    #[test]
    fn converts_counts_of_every_unit_exactly() {
        assert_eq!(Blu::from_inches(1), Blu(1_828_800));
        assert_eq!(Blu::from_points(12), Blu(304_800));
        assert_eq!(Blu::from_half_points(22), Blu(279_400)); // 11 pt
        assert_eq!(Blu::from_eighth_points(4), Blu(12_700)); // ½ pt
        assert_eq!(Blu::from_twips(1_440), Blu(1_828_800));
        assert_eq!(Blu::from_emus(914_400), Blu(1_828_800));
        assert_eq!(Blu::from_centimetres(-3), Blu(-2_160_000));
        assert_eq!(Blu::from_millimetres(254), Blu(18_288_000)); // 10 inches
        assert_eq!(Blu::from_dots(96, Dpi::DPI_96), Blu(1_828_800));
        assert_eq!(Blu::from_dots(-1, Dpi::DPI_300), Blu(-6_096));
        assert_eq!(Blu::from_units(5, LengthUnit::Pica), Blu(1_524_000));
        // The largest i32 count of the largest unit still fits.
        assert_eq!(
            Blu::from_units(i32::MAX, LengthUnit::Inch),
            Blu(2_147_483_647 * 1_828_800)
        );
        assert_eq!(
            Blu::from_units(i32::MIN, LengthUnit::Inch),
            Blu(-2_147_483_648 * 1_828_800)
        );
    }

    #[test]
    fn converts_i64_counts_until_they_overflow() {
        assert_eq!(
            Blu::checked_from_units(27_273_042_329_600, LengthUnit::Emu),
            Some(Blu(54_546_084_659_200))
        );
        assert_eq!(
            Blu::checked_from_units(i64::MAX / 2, LengthUnit::Emu),
            Some(Blu(i64::MAX - 1))
        );
        assert_eq!(
            Blu::checked_from_units(i64::MAX / 2 + 1, LengthUnit::Emu),
            None
        );
        assert_eq!(
            Blu::checked_from_units(i64::MIN / 2, LengthUnit::Emu),
            Some(Blu::MIN)
        );
        assert_eq!(
            Blu::checked_from_units(i64::MIN / 2 - 1, LengthUnit::Emu),
            None
        );
        assert_eq!(Blu::checked_from_units(i64::MIN, LengthUnit::Twip), None);
    }

    #[test]
    fn converts_between_units_through_blu_exactly() {
        assert_eq!(
            Blu::from_inches(1).to_units_exact(LengthUnit::Twip),
            Some(1_440)
        );
        assert_eq!(
            Blu::from_inches(1).to_units_exact(LengthUnit::Emu),
            Some(914_400)
        );
        assert_eq!(
            Blu::from_points(1).to_units_exact(LengthUnit::Emu),
            Some(12_700)
        );
        assert_eq!(
            Blu::from_twips(1).to_units_exact(LengthUnit::Emu),
            Some(635)
        );
        assert_eq!(
            Blu::from_eighth_points(1).to_units_exact(LengthUnit::Emu),
            None // 1,587.5 EMU: why EMU could not be the base unit
        );
        assert_eq!(
            Blu::from_millimetres(127).to_units_exact(LengthUnit::Inch),
            Some(5)
        );
        assert_eq!(
            Blu::from_points(3).to_units_exact(LengthUnit::Dot(Dpi::DPI_96)),
            Some(4)
        );
        assert_eq!(
            Blu::from_points(1).to_units_exact(LengthUnit::Dot(Dpi::DPI_96)),
            None
        );
        assert_eq!(Blu::from_points(-9).to_units_exact(LengthUnit::Pica), None);
        assert_eq!(
            Blu::from_points(-24).to_units_exact(LengthUnit::Pica),
            Some(-2)
        );
        assert_eq!(Blu::ZERO.to_units_exact(LengthUnit::Centimetre), Some(0));
    }

    #[test]
    fn rounds_lengths_that_are_not_whole_units_as_asked() {
        // In the order of Rounding::ALL: floor, ceiling, toward zero, half up, half even, half away from zero.
        let cases = [
            (Blu(1_000), [0, 1, 0, 1, 1, 1]),        // 0.787… twip
            (Blu(635), [0, 1, 0, 1, 0, 1]),          // exactly half a twip
            (Blu(1_905), [1, 2, 1, 2, 2, 2]),        // 1.5 twips
            (Blu(-635), [-1, 0, 0, 0, 0, -1]),       // −0.5 twip
            (Blu(-1_000), [-1, 0, 0, -1, -1, -1]),   // −0.787… twip
            (Blu(-1_905), [-2, -1, -1, -1, -2, -2]), // −1.5 twips
            (Blu(2_540), [2; 6]),                    // exactly 2 twips
        ];
        for (length, expected) in cases {
            for (rounding, want) in Rounding::ALL.into_iter().zip(expected) {
                assert_eq!(
                    length.to_units(LengthUnit::Twip, rounding),
                    want,
                    "{length:?} in twips with {rounding:?}"
                );
            }
        }
    }

    #[test]
    fn rounds_the_extreme_lengths_without_overflowing() {
        assert_eq!(
            Blu::MAX.to_units(LengthUnit::Emu, Rounding::Ceiling),
            1 << 62
        );
        assert_eq!(
            Blu::MAX.to_units(LengthUnit::Emu, Rounding::Floor),
            (1 << 62) - 1
        );
        assert_eq!(
            Blu::MIN.to_units(LengthUnit::Emu, Rounding::Floor),
            -(1 << 62)
        );
        assert_eq!(
            Blu::MIN.to_units(LengthUnit::Emu, Rounding::HalfAwayFromZero),
            -(1 << 62)
        );
        let one_blu_dot = LengthUnit::Dot(Dpi::new(1_828_800).expect("divides an inch"));
        assert_eq!(Blu::MAX.to_units(one_blu_dot, Rounding::HalfEven), i64::MAX);
        assert_eq!(Blu::MIN.to_units(one_blu_dot, Rounding::Ceiling), i64::MIN);
    }

    #[test]
    fn scales_with_one_rounding_step() {
        // 14 pt at 150 %: exact.
        assert_eq!(
            Blu::from_points(14).scale(3, 2, Rounding::Floor),
            Some(Blu::from_points(21))
        );
        // A third of 1 BLU.
        assert_eq!(Blu(1).scale(1, 3, Rounding::Ceiling), Some(Blu(1)));
        assert_eq!(Blu(1).scale(1, 3, Rounding::HalfEven), Some(Blu(0)));
        assert_eq!(Blu(1).scale(1, 0, Rounding::HalfEven), None);
        assert_eq!(Blu::MAX.scale(2, 1, Rounding::HalfEven), None);
    }

    #[test]
    fn checked_arithmetic_reports_overflow() {
        assert_eq!(Blu(2).checked_add(Blu(3)), Some(Blu(5)));
        assert_eq!(Blu::MAX.checked_add(Blu(1)), None);
        assert_eq!(Blu::MIN.checked_add(Blu(-1)), None);
        assert_eq!(Blu(2).checked_sub(Blu(3)), Some(Blu(-1)));
        assert_eq!(Blu::MIN.checked_sub(Blu(1)), None);
        assert_eq!(Blu(-7).checked_mul(6), Some(Blu(-42)));
        assert_eq!(Blu::MAX.checked_mul(2), None);
        assert_eq!(Blu::MIN.checked_mul(-1), None);
        assert_eq!(Blu(9).checked_neg(), Some(Blu(-9)));
        assert_eq!(Blu::MIN.checked_neg(), None);
        assert_eq!(Blu(-9).checked_abs(), Some(Blu(9)));
        assert_eq!(Blu::MIN.checked_abs(), None);
    }

    #[test]
    fn saturating_arithmetic_stops_at_the_limits() {
        assert_eq!(Blu(2).saturating_add(Blu(3)), Blu(5));
        assert_eq!(Blu::MAX.saturating_add(Blu(1)), Blu::MAX);
        assert_eq!(Blu::MIN.saturating_add(Blu(-1)), Blu::MIN);
        assert_eq!(Blu(2).saturating_sub(Blu(3)), Blu(-1));
        assert_eq!(Blu::MIN.saturating_sub(Blu(1)), Blu::MIN);
        assert_eq!(Blu::MAX.saturating_sub(Blu(-1)), Blu::MAX);
        assert_eq!(Blu(-7).saturating_mul(6), Blu(-42));
        assert_eq!(Blu::MAX.saturating_mul(-2), Blu::MIN);
        assert_eq!(Blu::MIN.saturating_mul(-1), Blu::MAX);
    }

    #[test]
    fn operators_compute_like_integers() {
        let mut length = Blu(10);
        assert_eq!(length + Blu(5), Blu(15));
        assert_eq!(length - Blu(15), Blu(-5));
        assert_eq!(-length, Blu(-10));
        assert_eq!(length * 3, Blu(30));
        assert_eq!(-4 * length, Blu(-40));
        assert_eq!(Blu(-12).abs(), Blu(12));
        length += Blu(1);
        assert_eq!(length, Blu(11));
        length -= Blu(2);
        assert_eq!(length, Blu(9));
        length *= -2;
        assert_eq!(length, Blu(-18));
        assert_eq!([Blu(1), Blu(2), Blu(-4)].into_iter().sum::<Blu>(), Blu(-1));
        assert_eq!([Blu(1), Blu(2), Blu(-4)].iter().sum::<Blu>(), Blu(-1));
        assert!(Blu(-1) < Blu::ZERO && Blu::ZERO < Blu::TWIP);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic = "overflow"]
    fn addition_panics_on_overflow_in_debug_builds() {
        let _ = Blu::MAX + Blu(1);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic = "overflow"]
    fn multiplication_panics_on_overflow_in_debug_builds() {
        let _ = Blu::MIN * -1;
    }

    #[test]
    fn displays_the_exact_number_of_blu() {
        assert_eq!(Blu(25_400).to_string(), "25400 BLU");
        assert_eq!(Blu(-1_270).to_string(), "-1270 BLU");
        assert_eq!(Blu::ZERO.to_string(), "0 BLU");
    }

    #[test]
    fn serializes_as_an_integer_number_of_blu() {
        assert_tokens(&Blu(12_700), &[Token::I64(12_700)]);
        assert_tokens(&Blu::MIN, &[Token::I64(i64::MIN)]);
        // Smaller integer types and unsigned values that fit are accepted too.
        assert_de_tokens(&Blu(5), &[Token::U8(5)]);
        assert_de_tokens(&Blu(-3), &[Token::I32(-3)]);
    }
}
