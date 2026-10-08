//! Reference implementations for the property tests, written straight from the mathematical definitions with arbitrary-precision integers (num-bigint). They share no code with bayan-units, so an agreement between the two is evidence, not an echo.

use bayan_units::Rounding;
use num_bigint::BigInt;

/// Rounds the exact rational number `numerator ÷ denominator` as `rounding` says. `denominator` must not be zero.
///
/// The floor is the largest integer k with k ≤ n ÷ d, and the ceiling the smallest with k ≥ n ÷ d; both are found among the three integers around the truncated quotient by exact comparisons. The nearest of the two is then the one whose distance |n − k·d| is smaller.
pub fn round(numerator: &BigInt, denominator: &BigInt, rounding: Rounding) -> BigInt {
    let zero = BigInt::from(0);
    let truncated = numerator / denominator;
    let candidates = [&truncated - 1, truncated.clone(), &truncated + 1];
    // k ≤ n ÷ d exactly when (n − k·d)·d ≥ 0, because multiplying by d² > 0 keeps the order.
    let at_most = |k: &BigInt| (numerator - k * denominator) * denominator >= zero;
    let at_least = |k: &BigInt| (k * denominator - numerator) * denominator >= zero;
    let floor = candidates
        .iter()
        .filter(|k| at_most(k))
        .max()
        .expect("the floor is a candidate")
        .clone();
    let ceiling = candidates
        .iter()
        .filter(|k| at_least(k))
        .min()
        .expect("the ceiling is a candidate")
        .clone();
    if floor == ceiling {
        return floor;
    }
    let positive = numerator * denominator > zero;
    let below = (numerator - &floor * denominator).magnitude().clone();
    let above = (&ceiling * denominator - numerator).magnitude().clone();
    let tie = |floor: BigInt, ceiling: BigInt| match rounding {
        Rounding::HalfUp => ceiling,
        Rounding::HalfEven => {
            if &floor % 2 == BigInt::from(0) {
                floor
            } else {
                ceiling
            }
        }
        Rounding::HalfAwayFromZero => {
            if positive {
                ceiling
            } else {
                floor
            }
        }
        Rounding::Floor | Rounding::Ceiling | Rounding::TowardZero => {
            unreachable!("only the half modes break ties")
        }
    };
    match rounding {
        Rounding::Floor => floor,
        Rounding::Ceiling => ceiling,
        Rounding::TowardZero => {
            if positive {
                floor
            } else {
                ceiling
            }
        }
        Rounding::HalfUp | Rounding::HalfEven | Rounding::HalfAwayFromZero => {
            match below.cmp(&above) {
                core::cmp::Ordering::Less => floor,
                core::cmp::Ordering::Greater => ceiling,
                core::cmp::Ordering::Equal => tie(floor, ceiling),
            }
        }
    }
}

/// The reference for `bayan_units::scale`: `value × numerator ÷ denominator`, rounded once, or `None` if the denominator is zero or the result does not fit in an `i64`.
pub fn scale(value: i64, numerator: i64, denominator: i64, rounding: Rounding) -> Option<i64> {
    if denominator == 0 {
        return None;
    }
    let product = BigInt::from(value) * BigInt::from(numerator);
    i64::try_from(&round(&product, &BigInt::from(denominator), rounding)).ok()
}

/// π × 2²⁸⁸, truncated: π written with 72 hexadecimal fraction digits (computed for this test with Machin's formula; they are also the well-known digits that initialize Blowfish).
const PI_SHL_288: &[u8] =
    b"3243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89452821E6";

/// The number of binary fraction digits the trigonometry reference works with.
const BITS: usize = 192;

/// The sine and cosine of `units` sixty-thousandths of a degree, each times 2³² and rounded to the nearest integer.
///
/// Written independently of bayan-units' own method: no reduction to the first octant, but the Taylor series of the angle itself (from 0 up to 2π) with 192 fraction digits, where the accumulated error is below 2⁻¹⁸⁰. The exact sine or cosine of these angles is never exactly halfway between two multiples of 2⁻³² (by Niven's theorem the only rational values are 0, ±½ and ±1), so rounding this result gives the correctly rounded value.
pub fn sin_cos_q32(units: i64) -> (i64, i64) {
    let pi = BigInt::parse_bytes(PI_SHL_288, 16).expect("hexadecimal digits") >> (288 - BITS);
    let turn = units.rem_euclid(21_600_000);
    // The angle in radians with 192 fraction digits, from 0 up to 2π.
    let x = BigInt::from(turn) * &pi / BigInt::from(10_800_000);
    let x_squared = (&x * &x) >> BITS;
    let series = |first: BigInt, mut k: u32| {
        let mut term = first.clone();
        let mut sum = first;
        let mut subtract = true;
        loop {
            term = ((&term * &x_squared) >> BITS) / BigInt::from((k + 1) * (k + 2));
            if term == BigInt::from(0) {
                return sum;
            }
            if subtract {
                sum -= &term;
            } else {
                sum += &term;
            }
            subtract = !subtract;
            k += 2;
        }
    };
    let sin = series(x.clone(), 1);
    let cos = series(BigInt::from(1) << BITS, 0);
    // Adding a half and shifting right (which rounds toward negative infinity) rounds to the nearest multiple of 2^-32.
    let to_q32 = |value: BigInt| {
        let rounded = (value + (BigInt::from(1) << (BITS - 33))) >> (BITS - 32);
        i64::try_from(&rounded).expect("at most 2^32 in magnitude")
    };
    (to_q32(sin), to_q32(cos))
}
