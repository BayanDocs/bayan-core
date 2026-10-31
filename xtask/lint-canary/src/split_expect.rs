// Form 2 of work package CORE-009: the same as form 1 with an `#[expect(…, reason = "…")]`, which is the sanctioned exception only where it is written out in full. Clippy and the compiler accept this module; the lint audit must reject the hash map it silences.

macro_rules! exempt_map {
    ($hash:tt $decoy:tt, $bracket:tt) => {
        $hash $bracket
        /// The number of entries of an empty map.
        #[must_use]
        pub fn entries() -> usize {
            let map: std::collections::HashMap<u8, u8> = std::collections::HashMap::new();
            map.len()
        }
    };
}

exempt_map!(
    #[doc = "Entries."],
    [expect(clippy::disallowed_types, reason = "canary: an exception that a macro assembles")]
);
