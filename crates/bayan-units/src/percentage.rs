//! Percentages in the units OOXML writes them in.

use crate::rounding::div_round_by_positive;
use crate::{Blu, Rounding};

/// One percent and one fiftieth of a percent, in thousandths of a percent.
const THOUSANDTHS_PER_PERCENT: i64 = 1_000;
const THOUSANDTHS_PER_FIFTIETH: i64 = 20;

/// A percentage in thousandths of a percent: `Percentage(100_000)` is 100 %.
///
/// That one unit holds both forms OOXML writes exactly: WordprocessingML's fiftieths of a percent (for example a table width of type `pct`, where 5,000 is 100 %), each 20 thousandths, and DrawingML's thousandths of a percent (`ST_Percentage`, where 100,000 is 100 %). The operations round only where a result is not a whole number, and only as the caller says.
///
/// ```
/// use bayan_units::{Blu, Percentage, Rounding};
///
/// let half = Percentage::from_fiftieths_of_a_percent(2_500);
/// assert_eq!(half, Percentage::from_percent(50));
/// assert_eq!(half.of(Blu::from_points(7), Rounding::HalfEven), Some(Blu::from_twips(70)));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Percentage(pub i64);

impl Percentage {
    /// 0 %.
    pub const ZERO: Percentage = Percentage(0);
    /// 100 %.
    pub const HUNDRED: Percentage = Percentage(100_000);

    /// `percent` percent, exactly.
    #[must_use]
    pub const fn from_percent(percent: i32) -> Percentage {
        // `as` widens losslessly; `i64::from` is not available in a `const fn`.
        Percentage(percent as i64 * THOUSANDTHS_PER_PERCENT)
    }

    /// `fiftieths` fiftieths of a percent, exactly.
    #[must_use]
    pub const fn from_fiftieths_of_a_percent(fiftieths: i32) -> Percentage {
        Percentage(fiftieths as i64 * THOUSANDTHS_PER_FIFTIETH)
    }

    /// This percentage as a whole number of fiftieths of a percent, rounded as `rounding` says.
    #[must_use]
    pub const fn to_fiftieths_of_a_percent(self, rounding: Rounding) -> i64 {
        div_round_by_positive(self.0, THOUSANDTHS_PER_FIFTIETH, rounding)
    }

    /// This percentage as a whole number of fiftieths of a percent, or `None` if it is not one.
    #[must_use]
    pub const fn to_fiftieths_of_a_percent_exact(self) -> Option<i64> {
        if self.0 % THOUSANDTHS_PER_FIFTIETH == 0 {
            Some(self.0 / THOUSANDTHS_PER_FIFTIETH)
        } else {
            None
        }
    }

    /// This percentage of `length`, rounded to a whole number of BLU as `rounding` says, or `None` if the result does not fit.
    #[must_use]
    pub fn of(self, length: Blu, rounding: Rounding) -> Option<Blu> {
        length.scale(self.0, Percentage::HUNDRED.0, rounding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_both_ooxml_forms_exactly() {
        assert_eq!(Percentage::HUNDRED, Percentage(100_000));
        assert_eq!(Percentage::from_percent(100), Percentage::HUNDRED);
        assert_eq!(Percentage::from_percent(-7), Percentage(-7_000));
        assert_eq!(
            Percentage::from_fiftieths_of_a_percent(5_000),
            Percentage::HUNDRED
        );
        assert_eq!(Percentage::from_fiftieths_of_a_percent(1), Percentage(20));
        assert_eq!(
            Percentage::from_fiftieths_of_a_percent(i32::MIN),
            Percentage(-42_949_672_960)
        );
    }

    #[test]
    fn converts_back_to_fiftieths() {
        assert_eq!(
            Percentage::HUNDRED.to_fiftieths_of_a_percent_exact(),
            Some(5_000)
        );
        assert_eq!(Percentage(-40).to_fiftieths_of_a_percent_exact(), Some(-2));
        assert_eq!(Percentage(30).to_fiftieths_of_a_percent_exact(), None); // 1.5 fiftieths
        assert_eq!(
            Percentage(30).to_fiftieths_of_a_percent(Rounding::HalfEven),
            2
        );
        assert_eq!(Percentage(30).to_fiftieths_of_a_percent(Rounding::Floor), 1);
        assert_eq!(
            Percentage(-30).to_fiftieths_of_a_percent(Rounding::HalfAwayFromZero),
            -2
        );
        assert_eq!(
            Percentage(-30).to_fiftieths_of_a_percent(Rounding::HalfUp),
            -1
        );
    }

    #[test]
    fn takes_a_percentage_of_a_length() {
        // 50 % of 7 BLU is 3.5 BLU.
        let half = Percentage(50_000);
        assert_eq!(half.of(Blu(7), Rounding::HalfEven), Some(Blu(4)));
        assert_eq!(half.of(Blu(7), Rounding::Floor), Some(Blu(3)));
        assert_eq!(half.of(Blu(-7), Rounding::TowardZero), Some(Blu(-3)));
        // 33.333 % of 300,000 BLU is exactly 99,999 BLU.
        assert_eq!(
            Percentage(33_333).of(Blu(300_000), Rounding::Floor),
            Some(Blu(99_999))
        );
        assert_eq!(
            Percentage::HUNDRED.of(Blu::MAX, Rounding::Floor),
            Some(Blu::MAX)
        );
        assert_eq!(Percentage(200_000).of(Blu::MAX, Rounding::Floor), None);
    }
}
