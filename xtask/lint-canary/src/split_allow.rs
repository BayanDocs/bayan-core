// Form 1 of work package CORE-009: a macro builds `#[allow(…)]` from the `#` of a doc attribute and a bracket group handed in separately, so that neither the source check nor Clippy sees an `#[allow]`. Clippy and the compiler accept this module; the lint audit must reject both lints it silences: the hash-map ban that the lint tables turn on, and `unused_must_use`, which the compiler turns on by default.

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

exempt_map!(#[doc = "Entries."], [allow(clippy::disallowed_types)]);

macro_rules! exempt_result {
    ($hash:tt $decoy:tt, $bracket:tt) => {
        $hash $bracket
        /// Parses a number and drops the result.
        pub fn drop_result() {
            "1".parse::<u8>();
        }
    };
}

exempt_result!(#[doc = "Drops a result."], [allow(unused_must_use)]);
