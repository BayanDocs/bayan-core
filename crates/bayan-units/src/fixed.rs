//! A dimensionless fixed-point number for scale factors, transforms and the sine and cosine.

use core::ops::{Add, Neg, Sub};

use crate::rounding::{div_round, div_round_by_positive};
use crate::{Blu, Rounding};

/// A dimensionless number in fixed point with 32 binary fraction digits: `Fixed(n)` means n ÷ 2³², so `Fixed(1 << 32)` is 1 and `Fixed(1 << 31)` is ½.
///
/// It holds scale factors, the coefficients of a [`Transform`](crate::Transform) and the results of [`Angle::sin`](crate::Angle::sin) and [`Angle::cos`](crate::Angle::cos). Its range is about ±2.1 billion and its precision 2⁻³² (about 2.3 × 10⁻¹⁰), so multiplying a length of 22 inches (the largest page Word allows) by a factor is off by at most 0.01 BLU before the final rounding.
///
/// All arithmetic on it is integer arithmetic, so it gives the same bits on every platform. The operators (`+`, `-`, unary `-`) overflow like those of [`Blu`]; multiplication rounds, so it is a method that takes a [`Rounding`].
///
/// ```
/// use bayan_units::{Blu, Fixed, Rounding};
///
/// // A group shape drawn at 2 ÷ 3 of its child size.
/// let factor = Fixed::from_ratio(2, 3, Rounding::HalfEven).unwrap();
/// assert_eq!(factor.mul_blu(Blu::from_points(9), Rounding::HalfEven), Some(Blu::from_points(6)));
/// assert_eq!(Fixed::ONE + Fixed::ONE, Fixed::from_integer(2));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fixed(pub i64);

impl Fixed {
    /// The number of binary fraction digits: a `Fixed(n)` means n ÷ 2^`FRACTIONAL_BITS`.
    pub const FRACTIONAL_BITS: u32 = 32;
    /// Zero.
    pub const ZERO: Fixed = Fixed(0);
    /// One.
    pub const ONE: Fixed = Fixed(1 << Fixed::FRACTIONAL_BITS);

    /// `integer`, exactly.
    #[must_use]
    pub const fn from_integer(integer: i32) -> Fixed {
        // |integer| ≤ 2^31, so shifting it by 32 places stays within an i64. `as` widens losslessly; `i64::from` is not available in a `const fn`.
        Fixed((integer as i64) << Fixed::FRACTIONAL_BITS)
    }

    /// `numerator ÷ denominator`, rounded to a multiple of 2⁻³² as `rounding` says, or `None` if `denominator` is zero or the result is out of range.
    #[must_use]
    pub fn from_ratio(numerator: i64, denominator: i64, rounding: Rounding) -> Option<Fixed> {
        // |numerator| ≤ 2^63, so shifting it by 32 places stays below 2^95.
        let scaled = i128::from(numerator) << Fixed::FRACTIONAL_BITS;
        let raw = div_round(scaled, i128::from(denominator), rounding)?;
        i64::try_from(raw).ok().map(Fixed)
    }

    /// `self × rhs`, rounded to a multiple of 2⁻³² as `rounding` says, or `None` if the result is out of range.
    #[must_use]
    pub fn checked_mul(self, rhs: Fixed, rounding: Rounding) -> Option<Fixed> {
        // The product of two i64 values always fits in an i128; it has 64 fraction digits, and dividing by 2^32 leaves 32.
        let product = i128::from(self.0) * i128::from(rhs.0);
        let raw = div_round(product, 1 << Fixed::FRACTIONAL_BITS, rounding)?;
        i64::try_from(raw).ok().map(Fixed)
    }

    /// `self × length`, rounded to a whole number of BLU as `rounding` says, or `None` if the result does not fit.
    #[must_use]
    pub fn mul_blu(self, length: Blu, rounding: Rounding) -> Option<Blu> {
        let product = i128::from(self.0) * i128::from(length.0);
        let blu = div_round(product, 1 << Fixed::FRACTIONAL_BITS, rounding)?;
        i64::try_from(blu).ok().map(Blu)
    }

    /// This number rounded to an integer as `rounding` says.
    #[must_use]
    pub const fn round(self, rounding: Rounding) -> i64 {
        div_round_by_positive(self.0, 1 << Fixed::FRACTIONAL_BITS, rounding)
    }
}

impl Add for Fixed {
    type Output = Fixed;

    fn add(self, rhs: Fixed) -> Fixed {
        Fixed(self.0 + rhs.0)
    }
}

impl Sub for Fixed {
    type Output = Fixed;

    fn sub(self, rhs: Fixed) -> Fixed {
        Fixed(self.0 - rhs.0)
    }
}

impl Neg for Fixed {
    type Output = Fixed;

    fn neg(self) -> Fixed {
        Fixed(-self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: i64 = 4_294_967_296; // 2^32

    #[test]
    fn one_and_the_integers_are_exact() {
        assert_eq!(Fixed::ONE, Fixed(ONE));
        assert_eq!(Fixed::from_integer(3), Fixed(3 * ONE));
        assert_eq!(Fixed::from_integer(-1), Fixed(-ONE));
        assert_eq!(Fixed::from_integer(i32::MAX), Fixed(2_147_483_647 * ONE));
        assert_eq!(Fixed::from_integer(i32::MIN), Fixed(i64::MIN));
    }

    #[test]
    fn ratios_are_rounded_once() {
        // 1/3 × 2^32 = 1,431,655,765.33…
        assert_eq!(
            Fixed::from_ratio(1, 3, Rounding::HalfEven),
            Some(Fixed(1_431_655_765))
        );
        assert_eq!(
            Fixed::from_ratio(1, 3, Rounding::Ceiling),
            Some(Fixed(1_431_655_766))
        );
        // 2/3 × 2^32 = 2,863,311,530.66…
        assert_eq!(
            Fixed::from_ratio(2, 3, Rounding::HalfEven),
            Some(Fixed(2_863_311_531))
        );
        assert_eq!(
            Fixed::from_ratio(2, 3, Rounding::Floor),
            Some(Fixed(2_863_311_530))
        );
        assert_eq!(
            Fixed::from_ratio(-2, 3, Rounding::Floor),
            Some(Fixed(-2_863_311_531))
        );
        assert_eq!(
            Fixed::from_ratio(-1, 2, Rounding::HalfEven),
            Some(Fixed(-ONE / 2))
        );
        assert_eq!(
            Fixed::from_ratio(7, -7, Rounding::HalfEven),
            Some(Fixed(-ONE))
        );
    }

    #[test]
    fn ratios_out_of_range_are_refused() {
        assert_eq!(Fixed::from_ratio(1, 0, Rounding::HalfEven), None);
        assert_eq!(Fixed::from_ratio(1 << 31, 1, Rounding::HalfEven), None); // 2^31 itself is out of range
        assert_eq!(
            Fixed::from_ratio(-(1 << 31), 1, Rounding::HalfEven),
            Some(Fixed(i64::MIN))
        );
        assert_eq!(Fixed::from_ratio(i64::MAX, 1, Rounding::HalfEven), None);
    }

    #[test]
    fn multiplication_is_rounded_once() {
        let one_and_a_half = Fixed(3 * ONE / 2);
        assert_eq!(
            one_and_a_half.checked_mul(one_and_a_half, Rounding::Floor),
            Some(Fixed(9 * ONE / 4))
        );
        // The smallest positive value squared is 2^-64: below half of 2^-32.
        assert_eq!(
            Fixed(1).checked_mul(Fixed(1), Rounding::HalfEven),
            Some(Fixed(0))
        );
        assert_eq!(
            Fixed(1).checked_mul(Fixed(1), Rounding::Ceiling),
            Some(Fixed(1))
        );
        assert_eq!(
            Fixed(-1).checked_mul(Fixed(1), Rounding::Floor),
            Some(Fixed(-1))
        );
        // One third, rounded, times three is just below one.
        let third = Fixed(1_431_655_765);
        assert_eq!(
            third.checked_mul(Fixed::from_integer(3), Rounding::HalfEven),
            Some(Fixed(ONE - 1))
        );
    }

    #[test]
    fn multiplication_out_of_range_is_refused() {
        let big = Fixed::from_integer(1 << 30);
        assert_eq!(big.checked_mul(big, Rounding::HalfEven), None);
        assert_eq!(
            Fixed(i64::MIN).checked_mul(Fixed(-ONE), Rounding::HalfEven),
            None
        );
        assert_eq!(
            Fixed(i64::MIN).checked_mul(Fixed(ONE), Rounding::HalfEven),
            Some(Fixed(i64::MIN))
        );
    }

    #[test]
    fn lengths_are_scaled_and_rounded_to_whole_blu() {
        let third = Fixed(1_431_655_765);
        // 0.333… × 3 BLU = 0.99999999977 BLU
        assert_eq!(third.mul_blu(Blu(3), Rounding::HalfEven), Some(Blu(1)));
        assert_eq!(third.mul_blu(Blu(3), Rounding::Floor), Some(Blu(0)));
        assert_eq!(
            Fixed(-ONE / 2).mul_blu(Blu(3), Rounding::HalfEven),
            Some(Blu(-2))
        );
        assert_eq!(
            Fixed(-ONE / 2).mul_blu(Blu(3), Rounding::HalfUp),
            Some(Blu(-1))
        );
        assert_eq!(
            Fixed::ONE.mul_blu(Blu::MAX, Rounding::HalfEven),
            Some(Blu::MAX)
        );
        assert_eq!(
            Fixed::from_integer(2).mul_blu(Blu::MAX, Rounding::HalfEven),
            None
        );
    }

    #[test]
    fn rounds_to_integers() {
        let one_and_a_half = Fixed(3 * ONE / 2);
        assert_eq!(one_and_a_half.round(Rounding::Floor), 1);
        assert_eq!(one_and_a_half.round(Rounding::HalfEven), 2);
        assert_eq!((-one_and_a_half).round(Rounding::HalfEven), -2);
        assert_eq!((-one_and_a_half).round(Rounding::TowardZero), -1);
        assert_eq!(Fixed(i64::MAX).round(Rounding::Ceiling), 1 << 31);
        assert_eq!(Fixed(i64::MIN).round(Rounding::Floor), -(1 << 31));
    }

    #[test]
    fn adds_and_subtracts_exactly() {
        assert_eq!(Fixed::ONE + Fixed(ONE / 2), Fixed(3 * ONE / 2));
        assert_eq!(Fixed::ONE - Fixed(3 * ONE), Fixed(-2 * ONE));
        assert_eq!(-Fixed::ONE, Fixed(-ONE));
    }
}
