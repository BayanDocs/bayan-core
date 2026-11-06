//! Histograms of glyph position errors, in whole thousandths of a point, from which the median, the 95th percentile and the maximum of a document or of any group of documents are computed exactly as integers (M4).
//!
//! Values below 2,048 thousandths of a point (about 2 pt) each have a bin of their own, so their percentiles are exact. Larger values share bins of 1/1,024 of their power of two, so a percentile there is the bin's lower end, less than 0.1 % below the value. The maximum is always exact. Merging the histograms of documents gives the histogram of their group, so the report's percentiles are those of all the glyphs of a group, not averages of documents' percentiles.

use std::collections::BTreeMap;

/// Values below this have a bin each.
const EXACT: u64 = 2048;

/// Bins per power of two above [`EXACT`].
const SUB_BINS: u64 = 1024;

/// A histogram of non-negative whole numbers (see the module's documentation).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Histogram {
    bins: BTreeMap<u64, u64>,
    count: u64,
    max: Option<u64>,
}

impl Histogram {
    /// An empty histogram.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bins: BTreeMap::new(),
            count: 0,
            max: None,
        }
    }

    /// Adds `value`.
    pub fn add(&mut self, value: u64) {
        *self.bins.entry(bin(value)).or_insert(0) += 1;
        self.count += 1;
        self.max = Some(self.max.map_or(value, |max| max.max(value)));
    }

    /// Adds every value of `other`.
    pub fn merge(&mut self, other: &Self) {
        for (&bin, &count) in &other.bins {
            *self.bins.entry(bin).or_insert(0) += count;
        }
        self.count += other.count;
        if let Some(value) = other.max {
            self.max = Some(self.max.map_or(value, |max| max.max(value)));
        }
    }

    /// How many values it holds.
    #[must_use]
    pub const fn count(&self) -> u64 {
        self.count
    }

    /// The largest value, exactly.
    #[must_use]
    pub const fn max(&self) -> Option<u64> {
        self.max
    }

    /// The `per_mille`-th percentile by nearest rank: the value at rank ⌈`per_mille` × count / 1000⌉ (at least 1) in ascending order, so the median of an even count is the lower middle value. Exact below 2,048, otherwise the lower end of its bin.
    #[must_use]
    pub fn percentile(&self, per_mille: u64) -> Option<u64> {
        if self.count == 0 {
            return None;
        }
        let rank = u128::from(per_mille.min(1000)) * u128::from(self.count);
        let rank = u64::try_from(rank.div_ceil(1000))
            .unwrap_or(u64::MAX)
            .max(1);
        let mut seen = 0_u64;
        for (&bin, &count) in &self.bins {
            seen += count;
            if seen >= rank {
                return Some(lower_end(bin));
            }
        }
        self.max
    }
}

/// The bin of `value`.
fn bin(value: u64) -> u64 {
    if value < EXACT {
        return value;
    }
    // The power of two below value, at least 2^11, and the next ten bits.
    let exponent = 63 - u64::from(value.leading_zeros());
    let mantissa = (value >> (exponent - 10)) - SUB_BINS;
    EXACT + (exponent - 11) * SUB_BINS + mantissa
}

/// The smallest value of bin `bin`.
const fn lower_end(bin: u64) -> u64 {
    if bin < EXACT {
        return bin;
    }
    let exponent = (bin - EXACT) / SUB_BINS + 11;
    let mantissa = (bin - EXACT) % SUB_BINS + SUB_BINS;
    mantissa << (exponent - 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_values_are_exact_and_large_ones_within_a_thousandth() {
        for value in [
            0,
            1,
            999,
            2047,
            2048,
            2049,
            4095,
            4096,
            1_000_000,
            u64::MAX >> 2,
        ] {
            let low = lower_end(bin(value));
            assert!(low <= value, "{value}");
            if value < EXACT {
                assert_eq!(low, value);
            } else {
                assert!((value - low) * 1024 <= value, "{value}");
            }
        }
        assert_eq!(lower_end(bin(4096)), 4096);
        assert_eq!(lower_end(bin(4095)), 4094);
    }

    #[test]
    fn percentiles_by_nearest_rank() {
        let mut histogram = Histogram::new();
        assert_eq!(histogram.percentile(500), None);
        for value in [5, 1, 4, 2, 3, 100] {
            histogram.add(value);
        }
        // Sorted: 1 2 3 4 5 100.
        assert_eq!(histogram.percentile(500), Some(3));
        assert_eq!(histogram.percentile(950), Some(100));
        assert_eq!(histogram.percentile(0), Some(1));
        assert_eq!(histogram.max(), Some(100));
        let mut other = Histogram::new();
        other.add(7);
        histogram.merge(&other);
        assert_eq!(histogram.count(), 7);
        assert_eq!(histogram.percentile(500), Some(4));
    }
}
