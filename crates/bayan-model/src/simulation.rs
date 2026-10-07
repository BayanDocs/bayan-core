//! Seeded random editing by several replicas, for the convergence tests and the spike's long runs.

/// A seeded pseudo-random number generator (SplitMix64): fast, reproducible and identical on every platform. Not cryptographic.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// A generator that always produces the same sequence for the same seed.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next 64 random bits.
    pub const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n`, or 0 when `n` is 0.
    pub fn below(&mut self, n: usize) -> usize {
        match u64::try_from(n) {
            Ok(0) | Err(_) => 0,
            Ok(n) => usize::try_from(self.next_u64() % n).unwrap_or(0),
        }
    }

    /// True with probability `numerator / denominator`.
    pub fn chance(&mut self, numerator: u64, denominator: u64) -> bool {
        denominator != 0 && self.next_u64() % denominator < numerator
    }

    /// A random element of a slice, or `None` when it is empty.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        items.get(self.below(items.len()))
    }
}
