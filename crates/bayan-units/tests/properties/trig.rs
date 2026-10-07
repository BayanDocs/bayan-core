//! `Angle::sin_cos` against the 192-bit reference, and the identities of the circle.

use bayan_units::{Angle, Fixed};
use proptest::prelude::*;

use crate::reference;

fn raw(values: (Fixed, Fixed)) -> (i64, i64) {
    (values.0.0, values.1.0)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn matches_the_reference_for_any_angle(units in any::<i64>()) {
        prop_assert_eq!(raw(Angle(units).sin_cos()), reference::sin_cos_q32(units), "angle {}", units);
    }

    // Where the reduction to the first octant switches from one case to the next: every multiple of 45°, from −360° to 360°.
    #[test]
    fn matches_the_reference_around_every_multiple_of_45_degrees(eighths in -8_i64..=8, offset in -2_000_i64..=2_000) {
        let units = eighths * 2_700_000 + offset;
        prop_assert_eq!(raw(Angle(units).sin_cos()), reference::sin_cos_q32(units), "angle {}", units);
    }

    #[test]
    fn sine_squared_plus_cosine_squared_is_one(units in any::<i64>()) {
        let (sin, cos) = Angle(units).sin_cos();
        // Each value is within 2^-33 of the exact one, so the sum of their squares is within about 2^-31 of 1, which is 2^33 in units of 2^-64.
        let sum = i128::from(sin.0) * i128::from(sin.0) + i128::from(cos.0) * i128::from(cos.0);
        prop_assert!((sum - (1_i128 << 64)).abs() <= 1 << 33, "angle {}: {}", units, sum);
    }

    #[test]
    fn the_symmetries_of_the_circle_hold_exactly(units in -100_000_000_i64..100_000_000) {
        let (sin, cos) = Angle(units).sin_cos();
        prop_assert_eq!(Angle(-units).sin_cos(), (-sin, cos));
        prop_assert_eq!(Angle(5_400_000 - units).sin_cos(), (cos, sin));
        prop_assert_eq!(Angle(units + 10_800_000).sin_cos(), (-sin, -cos));
        prop_assert_eq!(Angle(units + 21_600_000).sin_cos(), (sin, cos));
    }

    #[test]
    fn sine_rises_and_cosine_falls_through_the_first_quadrant(units in 0_i64..5_400_000) {
        let (sin, cos) = Angle(units).sin_cos();
        let (next_sin, next_cos) = Angle(units + 1).sin_cos();
        prop_assert!(sin <= next_sin && next_cos <= cos, "angle {}", units);
    }
}

/// Every 2,700th angle of the first octant, 1,001 in all, on every run of the tests.
#[test]
fn a_sample_of_the_first_octant_is_correctly_rounded() {
    for units in (0..=2_700_000).step_by(2_700) {
        assert_eq!(
            raw(Angle(units).sin_cos()),
            reference::sin_cos_q32(units),
            "angle {units}"
        );
    }
}

/// Every one of the 2,700,001 angles of the first octant, from which every other angle's sine and cosine are taken. It takes about 20 seconds in a release build (several minutes in a debug build), so it runs on request: `cargo test -p bayan-units --release --test properties -- --ignored`.
#[test]
#[ignore = "exhaustive; run on request with --release -- --ignored"]
fn every_angle_of_the_first_octant_is_correctly_rounded() {
    for units in 0..=2_700_000 {
        assert_eq!(
            raw(Angle(units).sin_cos()),
            reference::sin_cos_q32(units),
            "angle {units}"
        );
    }
}
