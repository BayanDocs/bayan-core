//! Rounding modes and exact rational scaling.

/// How a result that falls between two integers is rounded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rounding {
    /// Toward negative infinity: 2.5 becomes 2, −2.5 becomes −3.
    Floor,
    /// Toward positive infinity: 2.5 becomes 3, −2.5 becomes −2.
    Ceiling,
    /// Toward zero, also called truncation: 2.7 becomes 2, −2.7 becomes −2.
    TowardZero,
    /// To the nearest integer, and halfway cases toward positive infinity: 2.5 becomes 3, −2.5 becomes −2.
    HalfUp,
    /// To the nearest integer, and halfway cases to the even neighbour ("banker's rounding"): 2.5 becomes 2, 3.5 becomes 4, −2.5 becomes −2.
    HalfEven,
    /// To the nearest integer, and halfway cases away from zero: 2.5 becomes 3, −2.5 becomes −3.
    HalfAwayFromZero,
}

impl Rounding {
    /// Every rounding mode, for code and tests that need to try them all.
    pub const ALL: [Rounding; 6] = [
        Rounding::Floor,
        Rounding::Ceiling,
        Rounding::TowardZero,
        Rounding::HalfUp,
        Rounding::HalfEven,
        Rounding::HalfAwayFromZero,
    ];
}

/// Computes `value × numerator ÷ denominator` exactly and rounds the result once, as `rounding` says.
///
/// The product is formed in 128-bit integers, where it always fits, so no precision is lost before the single rounding step. This is the conversion that turns font units into BLU, for example an advance width of 1,229 units in a 2,048-unit em at 11 points (279,400 BLU): `scale(1229, 279_400, 2048, Rounding::HalfEven)` is 167,667, because the exact result is 167,667.285….
///
/// Returns `None` if `denominator` is zero or the rounded result does not fit in an `i64`.
///
/// ```
/// use bayan_units::{scale, Rounding};
///
/// assert_eq!(scale(5, 1, 2, Rounding::HalfEven), Some(2)); // 2.5 → 2
/// assert_eq!(scale(5, 1, 2, Rounding::HalfAwayFromZero), Some(3)); // 2.5 → 3
/// assert_eq!(scale(-7, 1, 3, Rounding::Floor), Some(-3)); // −2.33… → −3
/// assert_eq!(scale(1, 1, 0, Rounding::Floor), None); // division by zero
/// ```
#[must_use]
pub fn scale(value: i64, numerator: i64, denominator: i64, rounding: Rounding) -> Option<i64> {
    // |value × numerator| ≤ 2^126, so the product always fits in an i128.
    let product = i128::from(value) * i128::from(numerator);
    let quotient = div_round(product, i128::from(denominator), rounding)?;
    i64::try_from(quotient).ok()
}

/// Divides `numerator` by a `divisor` of at least 1 and rounds the exact quotient once.
///
/// It cannot overflow: dividing by at least 1 never makes a value larger, and the rounding step happens only when the divisor is at least 2, which leaves room for it. The caller guarantees `divisor ≥ 1` (unit sizes, which are positive constants or checked by [`Dpi::new`](crate::Dpi::new)); a smaller divisor is a bug, caught by the `debug_assert!` in debug builds.
pub(crate) const fn div_round_by_positive(numerator: i64, divisor: i64, rounding: Rounding) -> i64 {
    debug_assert!(divisor >= 1, "the divisor must be at least 1");
    let floor = numerator.div_euclid(divisor);
    // 0 ≤ remainder < divisor
    let remainder = numerator.rem_euclid(divisor);
    if remainder == 0 {
        return floor;
    }
    let ceiling = floor + 1;
    let rest = divisor - remainder;
    match rounding {
        Rounding::Floor => floor,
        Rounding::Ceiling => ceiling,
        Rounding::TowardZero => {
            if numerator < 0 {
                ceiling
            } else {
                floor
            }
        }
        Rounding::HalfUp | Rounding::HalfEven | Rounding::HalfAwayFromZero => {
            if remainder < rest {
                floor
            } else if remainder > rest {
                ceiling
            } else {
                match rounding {
                    Rounding::HalfEven if floor % 2 == 0 => floor,
                    Rounding::HalfAwayFromZero if numerator < 0 => floor,
                    _ => ceiling,
                }
            }
        }
    }
}

/// Divides `numerator` by `denominator` and rounds the exact quotient once. Returns `None` if `denominator` is zero or the quotient does not fit in an `i128`.
pub(crate) fn div_round(numerator: i128, denominator: i128, rounding: Rounding) -> Option<i128> {
    // Rust's division truncates toward zero, and the remainder takes the sign of the numerator. `checked_` refuses a zero denominator and i128::MIN ÷ −1, whose quotient does not fit.
    let truncated = numerator.checked_div(denominator)?;
    let remainder = numerator.checked_rem(denominator)?;
    if remainder == 0 {
        return Some(truncated);
    }
    // The exact quotient lies strictly between `truncated` and its neighbour one step further from zero. A non-zero remainder means |denominator| ≥ 2, so |truncated| ≤ 2^126 and that step cannot overflow.
    let negative = (numerator < 0) != (denominator < 0);
    let away_from_zero = if negative {
        truncated - 1
    } else {
        truncated + 1
    };
    let (floor, ceiling) = if negative {
        (away_from_zero, truncated)
    } else {
        (truncated, away_from_zero)
    };
    // Compare the fraction |remainder| ÷ |denominator| with one half, without doubling anything that could overflow.
    let remainder = remainder.unsigned_abs();
    let rest = denominator.unsigned_abs() - remainder;
    let nearest = match remainder.cmp(&rest) {
        core::cmp::Ordering::Less => Some(truncated),
        core::cmp::Ordering::Greater => Some(away_from_zero),
        core::cmp::Ordering::Equal => None,
    };
    Some(match rounding {
        Rounding::Floor => floor,
        Rounding::Ceiling => ceiling,
        Rounding::TowardZero => truncated,
        Rounding::HalfUp => nearest.unwrap_or(ceiling),
        Rounding::HalfEven => nearest.unwrap_or(if floor % 2 == 0 { floor } else { ceiling }),
        Rounding::HalfAwayFromZero => nearest.unwrap_or(away_from_zero),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The expected results in the order of [`Rounding::ALL`]: floor, ceiling, toward zero, half up, half even, half away from zero.
    fn check(value: i64, numerator: i64, denominator: i64, expected: [i64; 6]) {
        for (rounding, want) in Rounding::ALL.into_iter().zip(expected) {
            assert_eq!(
                scale(value, numerator, denominator, rounding),
                Some(want),
                "{value} × {numerator} ÷ {denominator} with {rounding:?}"
            );
        }
    }

    #[test]
    fn rounds_halfway_cases_as_each_mode_says() {
        check(5, 1, 2, [2, 3, 2, 3, 2, 3]); // 2.5
        check(7, 1, 2, [3, 4, 3, 4, 4, 4]); // 3.5
        check(-5, 1, 2, [-3, -2, -2, -2, -2, -3]); // −2.5
        check(-7, 1, 2, [-4, -3, -3, -3, -4, -4]); // −3.5
        check(1, 1, 2, [0, 1, 0, 1, 0, 1]); // 0.5
        check(-1, 1, 2, [-1, 0, 0, 0, 0, -1]); // −0.5
        check(10, 3, 4, [7, 8, 7, 8, 8, 8]); // 7.5
    }

    #[test]
    fn rounds_other_fractions_to_the_nearest_or_as_directed() {
        check(7, 1, 3, [2, 3, 2, 2, 2, 2]); // 2.33…
        check(8, 1, 3, [2, 3, 2, 3, 3, 3]); // 2.66…
        check(-7, 1, 3, [-3, -2, -2, -2, -2, -2]); // −2.33…
        check(-8, 1, 3, [-3, -2, -2, -3, -3, -3]); // −2.66…
    }

    #[test]
    fn leaves_exact_results_alone() {
        check(6, 1, 3, [2; 6]);
        check(-6, 1, 3, [-2; 6]);
        check(0, 5, 7, [0; 6]);
        check(12, 7, 7, [12; 6]);
    }

    #[test]
    fn handles_negative_numerators_and_denominators() {
        check(5, 1, -2, [-3, -2, -2, -2, -2, -3]); // −2.5
        check(-5, 1, -2, [2, 3, 2, 3, 2, 3]); // 2.5
        check(5, -1, 2, [-3, -2, -2, -2, -2, -3]); // −2.5
        check(-5, -1, -2, [-3, -2, -2, -2, -2, -3]); // −2.5
    }

    #[test]
    fn converts_font_units_to_blu() {
        // 1,229 × 279,400 ÷ 2,048 = 343,382,600 ÷ 2,048 = 167,667.28515625
        check(
            1229,
            279_400,
            2048,
            [167_667, 167_668, 167_667, 167_667, 167_667, 167_667],
        );
    }

    #[test]
    fn keeps_the_full_product_before_dividing() {
        // The product i64::MAX × i64::MAX overflows 64 bits but not 128.
        check(i64::MAX, i64::MAX, i64::MAX, [i64::MAX; 6]);
        check(i64::MIN, i64::MAX, i64::MAX, [i64::MIN; 6]);
        // (2^62 × 3) ÷ 4 = 3 × 2^60, exactly.
        check(1 << 62, 3, 4, [3 << 60; 6]);
    }

    #[test]
    fn refuses_division_by_zero() {
        for rounding in Rounding::ALL {
            assert_eq!(scale(1, 1, 0, rounding), None);
            assert_eq!(scale(0, 0, 0, rounding), None);
        }
    }

    #[test]
    fn refuses_results_that_do_not_fit() {
        for rounding in Rounding::ALL {
            assert_eq!(scale(i64::MAX, 2, 1, rounding), None);
            assert_eq!(scale(i64::MIN, -1, 1, rounding), None);
            assert_eq!(scale(i64::MIN, 1, -1, rounding), None);
            assert_eq!(scale(i64::MIN, 1, 1, rounding), Some(i64::MIN));
        }
        // (2^63 − 1) × 3 ÷ 2 = 1.5 × MAX, which is out of range in every mode.
        for rounding in Rounding::ALL {
            assert_eq!(scale(i64::MAX, 3, 2, rounding), None);
        }
    }

    #[test]
    fn rounds_just_below_the_limit_into_range_and_just_above_out_of_it() {
        // (2 × MAX + 1) ÷ 2 = MAX + 0.5: floor, toward zero and half even (MAX is odd, so the even neighbour is MAX + 1) differ.
        let numerator = i128::from(i64::MAX) * 2 + 1;
        let expected = |rounding| match rounding {
            Rounding::Floor | Rounding::TowardZero => Some(i64::MAX),
            Rounding::Ceiling
            | Rounding::HalfUp
            | Rounding::HalfEven
            | Rounding::HalfAwayFromZero => None,
        };
        for rounding in Rounding::ALL {
            assert_eq!(
                div_round(numerator, 2, rounding).and_then(|q| i64::try_from(q).ok()),
                expected(rounding),
                "{rounding:?}"
            );
        }
    }
}
