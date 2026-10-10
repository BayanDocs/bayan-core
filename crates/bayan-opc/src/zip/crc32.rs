//! CRC-32 as ZIP uses it (the IEEE 802.3 polynomial, bit-reflected), computed byte by byte from a table built at compile time.

/// The table of CRC-32 remainders for every byte value.
const TABLE: [u32; 256] = table();

const fn table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    // The position in the table and the byte value it stands for, counted side by side so that no conversion is needed.
    let mut index = 0;
    let mut byte = 0_u32;
    while index < 256 {
        let mut remainder = byte;
        let mut bit = 0;
        while bit < 8 {
            remainder = if remainder & 1 == 1 {
                (remainder >> 1) ^ 0xEDB8_8320
            } else {
                remainder >> 1
            };
            bit += 1;
        }
        table[index] = remainder;
        index += 1;
        byte += 1;
    }
    table
}

/// A running CRC-32 checksum, for data that arrives in pieces.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Crc32(u32);

impl Crc32 {
    /// The checksum of no data yet.
    pub(crate) fn new() -> Self {
        Crc32(0xFFFF_FFFF)
    }

    /// Adds `data` to the checksum.
    pub(crate) fn update(&mut self, data: &[u8]) {
        let mut crc = self.0;
        for &byte in data {
            let [low, ..] = crc.to_le_bytes();
            crc = TABLE[usize::from(low ^ byte)] ^ (crc >> 8);
        }
        self.0 = crc;
    }

    /// The checksum of everything added so far.
    pub(crate) fn finish(self) -> u32 {
        !self.0
    }
}

/// The CRC-32 checksum of `data`.
pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(data);
    crc.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_standard_check_values() {
        // The check value of CRC-32/ISO-HDLC (the ZIP checksum) is the checksum of "123456789".
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn gives_the_same_result_in_pieces() {
        let data = b"The quick brown fox jumps over the lazy dog";
        for split in 0..data.len() {
            let mut crc = Crc32::new();
            crc.update(&data[..split]);
            crc.update(&data[split..]);
            assert_eq!(crc.finish(), crc32(data));
        }
    }
}
