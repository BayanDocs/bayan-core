//! The only code in bayan-ffi that reads or writes through pointers from C, kept small so Miri can check every line of it (`cargo +nightly miri test -p bayan-ffi`).
//!
//! Every function checks what it can check (null pointers, lengths, sizes that would overflow) and documents what only the caller can guarantee: that a pointer really points to that many bytes, for the duration of the call (engine protocol §3).

use std::ptr;

/// Bytes C passes in: `length` bytes at `bytes`, borrowed for the duration of one call. Returns `None` if `bytes` is null while `length` is not 0, or if `length` exceeds `limit`.
///
/// # Safety
///
/// Unless `bytes` is null or `length` is 0, `bytes` must point to `length` readable bytes that nothing writes to until the returned slice is dropped.
#[expect(
    unsafe_code,
    reason = "reads or writes memory that C passes in, as the C interface requires (ADR-0006 §2)"
)]
pub(crate) unsafe fn borrowed<'a>(
    bytes: *const u8,
    length: usize,
    limit: usize,
) -> Option<&'a [u8]> {
    if length > limit {
        return None;
    }
    if length == 0 {
        return Some(&[]);
    }
    if bytes.is_null() {
        return None;
    }
    // SAFETY: `bytes` is not null and, as the caller guarantees, points to `length` readable bytes that stay unchanged while the slice lives. A `u8` needs no alignment, and `length` is at most `limit`, which the callers keep far below `isize::MAX`.
    Some(unsafe { std::slice::from_raw_parts(bytes, length) })
}

/// Copies `source` to the start of the `capacity` bytes at `out`. Returns false, without writing, if `out` is null or too small.
///
/// # Safety
///
/// Unless `out` is null, it must point to `capacity` writable bytes that nothing else reads or writes during the call.
#[expect(
    unsafe_code,
    reason = "reads or writes memory that C passes in, as the C interface requires (ADR-0006 §2)"
)]
pub(crate) unsafe fn copy_out(source: &[u8], out: *mut u8, capacity: usize) -> bool {
    if out.is_null() || capacity < source.len() {
        return false;
    }
    // SAFETY: `out` is not null and, as the caller guarantees, points to at least `capacity` writable bytes, of which we write `source.len()` (no more than `capacity`). `source` is Rust memory, so the two cannot overlap.
    unsafe { ptr::copy_nonoverlapping(source.as_ptr(), out, source.len()) };
    true
}

/// Writes a size through a pointer C passed in. Returns false if the pointer is null. The pointer need not be aligned.
///
/// # Safety
///
/// Unless `out` is null, it must point to a writable `size_t`.
#[expect(
    unsafe_code,
    reason = "reads or writes memory that C passes in, as the C interface requires (ADR-0006 §2)"
)]
pub(crate) unsafe fn write_size(out: *mut usize, value: usize) -> bool {
    if out.is_null() {
        return false;
    }
    // SAFETY: `out` is not null and, as the caller guarantees, points to a writable `size_t`; `write_unaligned` does not need it to be aligned.
    unsafe { out.write_unaligned(value) };
    true
}

/// The number of bytes a tile of `width` × `height` pixels needs in a buffer whose rows start `stride` bytes apart: `stride × (height − 1) + 4 × width`. Returns `None` if a size is 0, the stride is shorter than a row, or the size does not fit in `usize`.
pub(crate) fn tile_buffer_len(width: u32, height: u32, stride: usize) -> Option<usize> {
    let row = usize::try_from(width).ok()?.checked_mul(4)?;
    let rows = usize::try_from(height).ok()?;
    if row == 0 || rows == 0 || stride < row {
        return None;
    }
    stride.checked_mul(rows - 1)?.checked_add(row)
}

/// Writes tightly packed rows of `row` bytes each into a buffer whose rows start `stride` bytes apart, leaving the bytes between rows untouched. Returns false, without writing, if `out` is null, `pixels` is not a whole number of rows, or the buffer is too small.
///
/// # Safety
///
/// Unless `out` is null, it must point to `capacity` writable bytes that nothing else reads or writes during the call; they need not be initialized.
#[expect(
    unsafe_code,
    reason = "reads or writes memory that C passes in, as the C interface requires (ADR-0006 §2)"
)]
pub(crate) unsafe fn write_rows(
    pixels: &[u8],
    row: usize,
    out: *mut u8,
    capacity: usize,
    stride: usize,
) -> bool {
    if out.is_null() || row == 0 || stride < row || !pixels.len().is_multiple_of(row) {
        return false;
    }
    let rows = pixels.len() / row;
    let needed = if rows == 0 {
        Some(0)
    } else {
        stride
            .checked_mul(rows - 1)
            .and_then(|start| start.checked_add(row))
    };
    if needed.is_none_or(|needed| needed > capacity) {
        return false;
    }
    // The caller's buffer may be uninitialized (a new `QImage`'s pixels are), so no Rust reference to it is ever made: each row is copied through raw pointers.
    for (index, source) in pixels.chunks_exact(row).enumerate() {
        // SAFETY: `index` is less than `rows`, so `index × stride` is at most `stride × (rows − 1)`, which the check above showed to fit in `capacity` together with a row; the offset therefore stays inside the caller's buffer, and it cannot overflow.
        let target = unsafe { out.add(index * stride) };
        // SAFETY: `target` points to at least `row` writable bytes inside the caller's buffer (see above), which nothing else uses during the call; `source` is Rust memory, so the two cannot overlap, and a `u8` needs no alignment.
        unsafe { ptr::copy_nonoverlapping(source.as_ptr(), target, row) };
    }
    true
}

#[cfg(test)]
#[expect(
    unsafe_code,
    reason = "the tests call the pointer helpers with valid Rust buffers, the way C would"
)]
mod tests {
    use super::*;

    #[test]
    fn borrows_bytes_and_refuses_null_or_oversized_input() {
        let data = [1_u8, 2, 3];
        // SAFETY: `data` is three readable bytes that live for the whole test.
        let whole = unsafe { borrowed(data.as_ptr(), 3, 16) };
        assert_eq!(whole, Some(&data[..]));
        // SAFETY: a null pointer with length 0 is allowed.
        let empty = unsafe { borrowed(ptr::null(), 0, 16) };
        assert_eq!(empty, Some(&[][..]));
        // SAFETY: a null pointer is refused before it is used.
        let null = unsafe { borrowed(ptr::null(), 3, 16) };
        assert_eq!(null, None);
        // SAFETY: the length is refused before the pointer is used.
        let oversized = unsafe { borrowed(data.as_ptr(), 3, 2) };
        assert_eq!(oversized, None);
    }

    #[test]
    fn copies_out_only_into_buffers_that_are_large_enough() {
        let mut out = [0_u8; 4];
        // SAFETY: `out` is four writable bytes.
        let copied = unsafe { copy_out(&[7, 8, 9], out.as_mut_ptr(), out.len()) };
        assert!(copied);
        assert_eq!(out, [7, 8, 9, 0]);
        // SAFETY: the buffer is refused as too small before it is used.
        let too_small = unsafe { copy_out(&[1; 5], out.as_mut_ptr(), out.len()) };
        assert!(!too_small);
        // SAFETY: a null buffer is refused before it is used.
        let null_out = unsafe { copy_out(&[1], ptr::null_mut(), 1) };
        assert!(!null_out);
        assert_eq!(out, [7, 8, 9, 0]);
    }

    #[test]
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "the test makes an unaligned pointer on purpose; write_size writes through it with write_unaligned"
    )]
    fn writes_sizes_through_unaligned_pointers() {
        let mut bytes = [0_u8; 1 + size_of::<usize>()];
        let unaligned = bytes[1..].as_mut_ptr().cast::<usize>();
        // SAFETY: `unaligned` points to `size_of::<usize>()` writable bytes inside `bytes`; `write_size` does not need alignment.
        let written = unsafe { write_size(unaligned, 0x0102) };
        assert!(written);
        let mut written = [0_u8; size_of::<usize>()];
        written.copy_from_slice(&bytes[1..]);
        assert_eq!(usize::from_ne_bytes(written), 0x0102);
        // SAFETY: a null pointer is refused before it is used.
        let null_written = unsafe { write_size(ptr::null_mut(), 1) };
        assert!(!null_written);
    }

    #[test]
    fn computes_tile_buffer_sizes_without_overflow() {
        assert_eq!(tile_buffer_len(2, 3, 8), Some(8 * 2 + 8));
        assert_eq!(tile_buffer_len(2, 3, 12), Some(12 * 2 + 8));
        assert_eq!(tile_buffer_len(2, 3, 7), None);
        assert_eq!(tile_buffer_len(0, 3, 8), None);
        assert_eq!(tile_buffer_len(2, 0, 8), None);
        assert_eq!(tile_buffer_len(4096, 4096, usize::MAX), None);
    }

    #[test]
    fn writes_rows_into_uninitialized_buffers() {
        // C may pass memory that was never written, such as a new QImage's pixels; only the rows may be written and read back.
        let pixels = [1_u8, 2, 3, 4, 5, 6, 7, 8];
        let mut out = Box::<[u8]>::new_uninit_slice(10);
        // SAFETY: `out` is 10 writable bytes, enough for two rows of four bytes six bytes apart; they need not be initialized.
        let rows_written = unsafe { write_rows(&pixels, 4, out.as_mut_ptr().cast::<u8>(), 10, 6) };
        assert!(rows_written);
        for (index, start) in [0, 1, 2, 3, 6, 7, 8, 9].into_iter().enumerate() {
            // SAFETY: byte `start` belongs to one of the two rows that `write_rows` wrote.
            let byte = unsafe { out[start].assume_init() };
            assert_eq!(byte, pixels[index]);
        }
    }

    #[test]
    fn writes_rows_into_padded_buffers_and_leaves_the_padding_alone() {
        let pixels = [1_u8, 2, 3, 4, 5, 6, 7, 8];
        let mut out = [0xAA_u8; 14];
        // Two rows of four bytes, six bytes apart; the last row needs no padding after it.
        // SAFETY: `out` is 14 writable bytes, enough for two rows six bytes apart.
        let rows_written = unsafe { write_rows(&pixels, 4, out.as_mut_ptr(), 10, 6) };
        assert!(rows_written);
        assert_eq!(
            out,
            [1, 2, 3, 4, 0xAA, 0xAA, 5, 6, 7, 8, 0xAA, 0xAA, 0xAA, 0xAA]
        );
        // SAFETY: the buffer is refused as too small before it is used.
        let rows_too_small = unsafe { write_rows(&pixels, 4, out.as_mut_ptr(), 9, 6) };
        assert!(!rows_too_small);
        // SAFETY: a stride shorter than a row is refused before the buffer is used.
        let short_stride = unsafe { write_rows(&pixels, 4, out.as_mut_ptr(), 14, 3) };
        assert!(!short_stride);
        // SAFETY: pixels that are not whole rows are refused before the buffer is used.
        let partial_rows = unsafe { write_rows(&pixels[..7], 4, out.as_mut_ptr(), 14, 6) };
        assert!(!partial_rows);
        // SAFETY: a null buffer is refused before it is used.
        let null_rows = unsafe { write_rows(&pixels, 4, ptr::null_mut(), 14, 6) };
        assert!(!null_rows);
    }
}
