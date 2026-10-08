//! `scale` against the big-integer reference, for every rounding mode.

use bayan_units::{Rounding, scale};
use proptest::prelude::*;

use crate::reference;

/// Values that often meet the edges of the `i64` range.
fn edgy() -> impl Strategy<Value = i64> {
    prop_oneof![
        Just(i64::MIN),
        Just(i64::MIN + 1),
        Just(i64::MAX),
        Just(i64::MAX - 1),
        Just(0),
        Just(1),
        Just(-1),
        any::<i64>(),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn matches_the_reference_over_the_whole_range(value in any::<i64>(), numerator in any::<i64>(), denominator in any::<i64>()) {
        for rounding in Rounding::ALL {
            prop_assert_eq!(
                scale(value, numerator, denominator, rounding),
                reference::scale(value, numerator, denominator, rounding),
                "{} × {} ÷ {} with {:?}", value, numerator, denominator, rounding
            );
        }
    }

    // Small operands make exact results and halfway cases common, which random 64-bit operands almost never produce.
    #[test]
    fn matches_the_reference_on_halfway_and_exact_cases(value in -300_i64..300, numerator in -30_i64..30, denominator in -12_i64..12) {
        for rounding in Rounding::ALL {
            prop_assert_eq!(
                scale(value, numerator, denominator, rounding),
                reference::scale(value, numerator, denominator, rounding),
                "{} × {} ÷ {} with {:?}", value, numerator, denominator, rounding
            );
        }
    }

    #[test]
    fn matches_the_reference_at_the_edges_of_the_range(value in edgy(), numerator in edgy(), denominator in prop_oneof![edgy(), -4_i64..=4]) {
        for rounding in Rounding::ALL {
            prop_assert_eq!(
                scale(value, numerator, denominator, rounding),
                reference::scale(value, numerator, denominator, rounding),
                "{} × {} ÷ {} with {:?}", value, numerator, denominator, rounding
            );
        }
    }

    #[test]
    fn rounding_modes_keep_their_order_and_stay_within_one(value in any::<i64>(), denominator in 1_i64..1_000_000) {
        let at = |rounding| scale(value, 1, denominator, rounding).expect("dividing by a positive number never overflows");
        let (floor, ceiling) = (at(Rounding::Floor), at(Rounding::Ceiling));
        prop_assert!(ceiling - floor <= 1);
        for rounding in Rounding::ALL {
            let result = at(rounding);
            prop_assert!(floor <= result && result <= ceiling, "{:?} gave {} outside [{}, {}]", rounding, result, floor, ceiling);
        }
        // Negating the input mirrors the directed modes into each other.
        if value != i64::MIN {
            prop_assert_eq!(scale(-value, 1, denominator, Rounding::Floor), Some(-ceiling));
            prop_assert_eq!(scale(-value, 1, denominator, Rounding::HalfAwayFromZero), at(Rounding::HalfAwayFromZero).checked_neg());
            prop_assert_eq!(scale(-value, 1, denominator, Rounding::HalfEven), at(Rounding::HalfEven).checked_neg());
        }
    }
}
