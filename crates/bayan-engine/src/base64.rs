//! Base64 (RFC 4648 §4, the standard alphabet with padding), for the blobs inside recordings ([engine protocol specification][spec] §10).
//!
//! Recordings are JSON, which cannot hold raw bytes. The decoder is strict: it rejects any character outside the alphabet, whitespace, missing or misplaced padding, and non-zero padding bits, so that every byte string has exactly one encoding and a damaged recording is refused instead of misread.
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes bytes.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk.first().copied().unwrap_or(0);
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        let group = u32::from(first) << 16 | u32::from(second) << 8 | u32::from(third);
        let symbols = [group >> 18, group >> 12, group >> 6, group];
        for (index, &bits) in symbols.iter().enumerate() {
            if index <= chunk.len() {
                out.push(symbol(bits));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The symbol for the lowest six bits of `bits`.
fn symbol(bits: u32) -> char {
    let index = usize::try_from(bits & 0x3f).unwrap_or_default();
    ALPHABET.get(index).map_or('=', |&byte| char::from(byte))
}

/// The value of one symbol of the alphabet.
fn value(symbol: u8) -> Option<u32> {
    let value = match symbol {
        b'A'..=b'Z' => symbol - b'A',
        b'a'..=b'z' => symbol - b'a' + 26,
        b'0'..=b'9' => symbol - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(u32::from(value))
}

/// Decodes text, or returns `None` if it is not exactly the encoding of some bytes.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let symbols = text.as_bytes();
    if !symbols.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(symbols.len() / 4 * 3);
    let groups = symbols.len() / 4;
    for (index, group) in symbols.chunks(4).enumerate() {
        let last = index + 1 == groups;
        let padding = group
            .iter()
            .rev()
            .take_while(|&&symbol| symbol == b'=')
            .count();
        if padding > 2 || (padding > 0 && !last) {
            return None;
        }
        let mut bits = 0_u32;
        for &symbol in &group[..4 - padding] {
            bits = bits << 6 | value(symbol)?;
        }
        bits <<= 6 * padding;
        let bytes = bits.to_be_bytes();
        // The padding bits must be zero, so that each byte string has one encoding.
        let kept = 3 - padding;
        if bytes[1 + kept..].iter().any(|&byte| byte != 0) {
            return None;
        }
        out.extend_from_slice(&bytes[1..=kept]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_rfc_4648_test_vectors() {
        let vectors = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, encoded) in vectors {
            assert_eq!(encode(plain.as_bytes()), encoded);
            assert_eq!(decode(encoded).as_deref(), Some(plain.as_bytes()));
        }
    }

    #[test]
    fn round_trips_every_byte_value() {
        let bytes: Vec<u8> = (0..=255).collect();
        for length in 0..bytes.len() {
            let slice = &bytes[..length];
            assert_eq!(decode(&encode(slice)).as_deref(), Some(slice));
        }
    }

    #[test]
    fn rejects_anything_that_is_not_an_exact_encoding() {
        for bad in [
            "Zg", "Zg=", "Zm9v\n", " Zm9v", "Zm9v====", "Zg==Zg==", "Zh==", "Zm9=", "Zm-v", "Zm_v",
            "=Zm9", "Z===",
        ] {
            assert_eq!(decode(bad), None, "{bad:?}");
        }
    }
}
