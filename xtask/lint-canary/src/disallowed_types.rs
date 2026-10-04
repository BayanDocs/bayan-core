/// Uses both disallowed collection types.
#[must_use]
pub fn hash_collections() -> usize {
    let map: std::collections::HashMap<u8, u8> = std::collections::HashMap::new();
    let set: std::collections::HashSet<u8> = std::collections::HashSet::new();
    map.len() + set.len()
}
