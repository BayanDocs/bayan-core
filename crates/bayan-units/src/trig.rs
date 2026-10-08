//! The sine and cosine in integer arithmetic.
//!
//! The platform's floating-point `sin` and `cos` differ between platforms and are forbidden in bayan-core (ADR-0005 §4); these replace them. Only integer operations are used, so the results are the same bits on every platform by construction, and `tests/determinism.rs` checks that on each of them.
//!
//! An angle is first reduced to the first octant, 0° to 45°, with the symmetries of the circle, which are exact: whole turns are removed, each quarter turn swaps the sine and cosine and changes a sign, and an angle above 45° becomes its complement with the sine and cosine swapped. Within the octant, both are computed from their Taylor series in 128-bit fixed point with 64 binary fraction digits: each term is rounded to 2⁻⁶⁴, and the series stops when the next term rounds to zero, after at most 13 terms. The accumulated error stays below 2⁻⁵⁹, far below the 2⁻³³ that the final rounding to a multiple of 2⁻³² allows. A test compares the results for each of the 2,700,001 angles of the first octant with a 192-bit reference and finds every one correctly rounded (`every_angle_of_the_first_octant_is_correctly_rounded` in `tests/properties/trig.rs`). It takes about 20 seconds in a release build, so it runs only on request: run it after any change to this file, with `cargo test -p bayan-units --release --test properties -- --ignored`, and report the result in the pull request.

use crate::{Angle, Fixed};

/// A full turn, a quarter turn and an eighth of a turn in sixty-thousandths of a degree.
const FULL_TURN: i64 = 21_600_000;
const QUARTER_TURN: u64 = 5_400_000;
const EIGHTH_TURN: u64 = 2_700_000;

/// Half a turn in sixty-thousandths of a degree: π radians.
const HALF_TURN: u128 = 10_800_000;

/// π × 2¹⁰⁴, rounded to the nearest integer (the next binary digits are 0.439…).
const PI_SHL_104: u128 = 0x324_3f6a_8885_a308_d313_198a_2e03;

/// One, with 64 binary fraction digits.
const ONE_Q64: u128 = 1 << 64;

/// The sine and cosine of `angle`, each rounded to the nearest multiple of 2⁻³².
pub(crate) fn sin_cos(angle: Angle) -> (Fixed, Fixed) {
    // 0 ≤ turn < 21,600,000
    let turn = angle.0.rem_euclid(FULL_TURN).unsigned_abs();
    let quadrant = turn / QUARTER_TURN;
    let within = turn % QUARTER_TURN;
    let (sin, cos) = if within <= EIGHTH_TURN {
        first_octant(within)
    } else {
        let (sin, cos) = first_octant(QUARTER_TURN - within);
        (cos, sin)
    };
    let (sin, cos) = match quadrant {
        0 => (sin, cos),
        1 => (cos, -sin),
        2 => (-sin, -cos),
        _ => (-cos, sin),
    };
    (Fixed(sin), Fixed(cos))
}

/// The sine and cosine of `units` sixty-thousandths of a degree, where 0 ≤ `units` ≤ 2,700,000 (0° to 45°), each times 2³² and rounded to the nearest integer.
fn first_octant(units: u64) -> (i64, i64) {
    // The angle in radians, x = units × π ÷ 10,800,000, with 64 fraction digits: units < 2^22 and π × 2^104 < 2^106, so the product stays below 2^128, and x < π/4 < 1 fits in 64 bits.
    let x = round_div(u128::from(units) * PI_SHL_104, HALF_TURN << 40);
    let x_squared = round_div(x * x, ONE_Q64);
    let sin = alternating_series(x, x_squared, 1);
    let cos = alternating_series(ONE_Q64, x_squared, 0);
    (to_q32(sin), to_q32(cos))
}

/// The sum t₀ − t₁ + t₂ − … where t₀ = `first` and tᵢ₊₁ = tᵢ × x² ÷ ((k + 1)(k + 2)) with k = `k0` + 2i, all with 64 fraction digits: the Taylor series of the sine for (x, k0 = 1) and of the cosine for (1, k0 = 0).
fn alternating_series(first: u128, x_squared: u128, k0: u128) -> u128 {
    let mut term = first;
    let mut k = k0;
    let mut added = first;
    let mut subtracted = 0;
    let mut subtract = true;
    loop {
        // term ≤ 1 and x² < 0.62, both with 64 fraction digits, so their product is below 2^128; dividing by (k + 1)(k + 2) × 2^64 brings it back to 64 fraction digits.
        term = round_div(term * x_squared, ((k + 1) * (k + 2)) << 64);
        if term == 0 {
            break;
        }
        if subtract {
            subtracted += term;
        } else {
            added += term;
        }
        subtract = !subtract;
        k += 2;
    }
    // The terms shrink, so each partial sum stays between 0 and `first`.
    added - subtracted
}

/// `numerator ÷ denominator`, rounded to the nearest integer (halves upward), for a positive denominator.
fn round_div(numerator: u128, denominator: u128) -> u128 {
    (numerator + denominator / 2) / denominator
}

/// A value with 64 fraction digits, at most 1, rounded to the nearest multiple of 2⁻³².
#[expect(
    clippy::cast_possible_truncation,
    reason = "the value is at most 1, that is 2^64 with 64 fraction digits, so the result is at most 2^32 and fits in an i64"
)]
fn to_q32(value: u128) -> i64 {
    ((value + (1 << 31)) >> 32) as i64
}
