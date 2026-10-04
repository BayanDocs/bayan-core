/// Tries to allow unsafe code for one function, which `forbid` does not permit.
#[must_use]
#[expect(
    unsafe_code,
    reason = "canary: a crate-wide forbid cannot be lifted locally"
)]
pub fn read_through_pointer(value: &u8) -> u8 {
    let pointer: *const u8 = value;
    // SAFETY: `pointer` was made from a reference, so it is valid, aligned and initialized.
    unsafe { pointer.read() }
}
