//! A minimal PNG writer for the rasters: 8-bit RGBA, no filtering, zlib from miniz_oxide (the DEFLATE that bayan-opc uses). The spike hashes raw pixels, never PNG files; the PNGs are for people to look at.

/// Encodes an image of `width` × `height` pixels, given as RGBA bytes row by row, as a PNG file.
///
/// # Errors
///
/// A message if a side is zero or larger than PNG allows, or if `rgba` does not hold exactly `width × height × 4` bytes.
pub fn encode_rgba8(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    // PNG limits each side to 2^31 − 1 pixels.
    const MAX_SIDE: u32 = (1 << 31) - 1;
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(format!("a PNG image cannot be {width} × {height} pixels"));
    }
    let row = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or("the image is too wide for this platform")?;
    let rows = usize::try_from(height).map_err(|_| "the image is too tall for this platform")?;
    if Some(rgba.len()) != row.checked_mul(rows) {
        return Err(format!(
            "{} bytes of pixels do not make an image of {width} × {height} RGBA pixels",
            rgba.len()
        ));
    }
    // Every row starts with its filter type; 0 (None) keeps the writer trivial.
    let mut raw = Vec::with_capacity(rgba.len() + rows);
    for line in rgba.chunks_exact(row) {
        raw.push(0);
        raw.extend_from_slice(line);
    }
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    // 8 bits per channel, colour type 6 (RGBA), compression method 0, filter method 0, no interlacing.
    header.extend_from_slice(&[8, 6, 0, 0, 0]);

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, *b"IHDR", &header)?;
    chunk(
        &mut png,
        *b"IDAT",
        &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6),
    )?;
    chunk(&mut png, *b"IEND", &[])?;
    Ok(png)
}

/// Appends one chunk: its length, type, data and the CRC of type and data.
fn chunk(png: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) -> Result<(), String> {
    let length = u32::try_from(data.len()).map_err(|_| "a PNG chunk cannot hold 4 GiB")?;
    png.extend_from_slice(&length.to_be_bytes());
    let start = png.len();
    png.extend_from_slice(&kind);
    png.extend_from_slice(data);
    let crc = crc32(&png[start..]);
    png.extend_from_slice(&crc.to_be_bytes());
    Ok(())
}

/// The CRC-32 of ISO 3309 and ITU-T V.42 that PNG chunks carry.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    // Bit by bit, with the reflected polynomial: slow, but the images are small and the code is short.
    let mut crc = 0xffff_ffff_u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chunks of a PNG file, checking the signature and every CRC on the way.
    fn chunks(png: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let mut chunks = Vec::new();
        let mut rest = &png[8..];
        while !rest.is_empty() {
            let length =
                usize::try_from(u32::from_be_bytes(rest[..4].try_into().unwrap())).unwrap();
            let kind: [u8; 4] = rest[4..8].try_into().unwrap();
            let data = rest[8..8 + length].to_vec();
            let crc = u32::from_be_bytes(rest[8 + length..12 + length].try_into().unwrap());
            assert_eq!(crc, crc32(&rest[4..8 + length]), "CRC of {kind:?}");
            chunks.push((kind, data));
            rest = &rest[12 + length..];
        }
        chunks
    }

    #[test]
    fn computes_the_check_value_of_crc_32() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn writes_a_png_that_holds_exactly_the_pixels() {
        let pixels: Vec<u8> = (0..2 * 3 * 4)
            .map(|value| u8::try_from(value * 10).unwrap())
            .collect();
        let png = encode_rgba8(2, 3, &pixels).unwrap();
        let chunks = chunks(&png);
        let kinds: Vec<&[u8; 4]> = chunks.iter().map(|(kind, _)| kind).collect();
        assert_eq!(kinds, [b"IHDR", b"IDAT", b"IEND"]);
        // Width 2, height 3, 8 bits per channel, colour type 6 (RGBA), compression 0, filter 0, no interlacing.
        assert_eq!(chunks[0].1, [0, 0, 0, 2, 0, 0, 0, 3, 8, 6, 0, 0, 0]);
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&chunks[1].1).unwrap();
        let mut expected = Vec::new();
        for row in pixels.chunks(2 * 4) {
            expected.push(0); // filter type None
            expected.extend_from_slice(row);
        }
        assert_eq!(raw, expected);
        assert!(chunks[2].1.is_empty());
    }

    #[test]
    fn writes_the_same_bytes_every_time() {
        let pixels = vec![0x7f; 16 * 16 * 4];
        assert_eq!(
            encode_rgba8(16, 16, &pixels).unwrap(),
            encode_rgba8(16, 16, &pixels).unwrap()
        );
    }

    #[test]
    fn refuses_inconsistent_sizes() {
        assert!(encode_rgba8(0, 1, &[]).is_err());
        assert!(encode_rgba8(1, 0, &[]).is_err());
        assert!(encode_rgba8(2, 2, &[0; 15]).is_err());
        assert!(encode_rgba8(1 << 31, 1, &[]).is_err());
    }
}
