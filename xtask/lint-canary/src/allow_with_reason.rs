/// A hash map with an exception written as `#[allow]`, which is never flagged when it becomes unnecessary.
#[must_use]
#[allow(
    clippy::disallowed_types,
    reason = "canary: a reason alone is not enough; exceptions must use #[expect]"
)]
pub fn allowed_hash_map() -> usize {
    let map: std::collections::HashMap<u8, u8> = std::collections::HashMap::new();
    map.len()
}
