//! CRC-32 (ISO 3309, the polynomial ZIP uses), to check every decompressed part against its archive entry.
//!
//! A checksum against accidental corruption, not a cryptographic hash: a mismatch means the package is damaged, as Word would also report.

/// The reflected form of the CRC-32 polynomial 0x04C11DB7.
const POLYNOMIAL: u32 = 0xEDB8_8320;

/// The remainder of every byte value, computed once at compile time.
const TABLE: [u32; 256] = table();

const fn table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut index = 0;
    // The byte value of `index`, counted alongside it so that no conversion is needed.
    let mut value: u32 = 0;
    while index < 256 {
        let mut remainder = value;
        let mut bit = 0;
        while bit < 8 {
            remainder = if remainder & 1 == 1 {
                (remainder >> 1) ^ POLYNOMIAL
            } else {
                remainder >> 1
            };
            bit += 1;
        }
        table[index] = remainder;
        index += 1;
        value += 1;
    }
    table
}

/// The CRC-32 of `bytes`.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        let index = usize::from(crc.to_le_bytes()[0] ^ byte);
        crc = (crc >> 8) ^ TABLE[index];
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::crc32;

    #[test]
    fn matches_the_standard_check_values() {
        // The check value of CRC-32/ISO-HDLC, the variant ZIP uses.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }
}
