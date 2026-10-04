/// Reads a value through a raw pointer, which needs an `unsafe` block.
#[must_use]
pub fn read_through_pointer(value: &u8) -> u8 {
    let pointer: *const u8 = value;
    // SAFETY: `pointer` was made from a reference, so it is valid, aligned and initialized.
    unsafe { pointer.read() }
}
