/// A hash map with an exception that gives no reason.
#[must_use]
#[expect(clippy::disallowed_types)]
pub fn unjustified_hash_map() -> usize {
    let map: std::collections::HashMap<u8, u8> = std::collections::HashMap::new();
    map.len()
}
