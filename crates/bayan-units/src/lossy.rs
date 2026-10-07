//! The crate's only conversion to floating point, for display and debugging (CORE-002, acceptance criterion 3).
//!
//! Everything else in bayan-units is integer arithmetic. The test `tests/floating_point.rs` fails if a floating-point type is named in any other source file of the crate, so its public interface cannot grow another one unnoticed.

use crate::{Blu, LengthUnit};

impl Blu {
    /// This length as a floating-point number of `unit`, **for display and debugging only**, for example to show "1.25 in" on a ruler.
    ///
    /// The result is lossy: most lengths are not a finite binary fraction of a unit (1 BLU is 1/25,400 of a point), and lengths beyond 2⁵³ BLU lose whole BLU. Never use it in layout, which must stay exact (ADR-0005 §3). The computation itself is deterministic: two conversions to floating point and one division, all exactly specified by IEEE 754.
    ///
    /// ```
    /// use bayan_units::{Blu, LengthUnit};
    ///
    /// assert_eq!(Blu::from_points(90).to_f64_lossy(LengthUnit::Inch), 1.25);
    /// ```
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "a lossy conversion for display is this function's purpose; lengths beyond 2^53 BLU lose precision, as documented"
    )]
    pub fn to_f64_lossy(self, unit: LengthUnit) -> f64 {
        self.0 as f64 / unit.blu().0 as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_to_floating_point_units() {
        assert_eq!(Blu::from_points(18).to_f64_lossy(LengthUnit::Inch), 0.25);
        assert_eq!(Blu::from_twips(30).to_f64_lossy(LengthUnit::Point), 1.5);
        assert_eq!(Blu(-1_828_800).to_f64_lossy(LengthUnit::Inch), -1.0);
        assert_eq!(Blu(1).to_f64_lossy(LengthUnit::Emu), 0.5);
        // One BLU is not a finite binary fraction of a point; the result is the nearest double.
        assert_eq!(
            Blu(1).to_f64_lossy(LengthUnit::Point),
            3.937_007_874_015_748e-5
        );
    }
}
