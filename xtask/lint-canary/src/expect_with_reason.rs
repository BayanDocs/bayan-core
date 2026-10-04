/// A hash map with the sanctioned, justified exception.
#[must_use]
#[expect(
    clippy::disallowed_types,
    reason = "canary: the map is only counted, never iterated, so its order cannot affect output"
)]
pub fn justified_hash_map() -> usize {
    let map: std::collections::HashMap<u8, u8> = std::collections::HashMap::new();
    map.len()
}
