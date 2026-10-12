//! SHA-256 digests and their hexadecimal spelling, the identity of every font file (ADR-0010 §3), layout and raster.

use sha2::{Digest, Sha256};

/// The SHA-256 digest of `data`.
#[must_use]
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// `bytes` as lowercase hexadecimal digits.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(2 * bytes.len());
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

/// A SHA-256 digest written as 64 lowercase hexadecimal digits, or `None` for anything else.
#[must_use]
pub fn parse_sha256(text: &str) -> Option<[u8; 32]> {
    let digits = text.as_bytes();
    if digits.len() != 64 {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (byte, pair) in digest.iter_mut().zip(digits.as_chunks::<2>().0) {
        *byte = (digit(pair[0])? << 4) | digit(pair[1])?;
    }
    Some(digest)
}

/// The value of one lowercase hexadecimal digit.
fn digit(ascii: u8) -> Option<u8> {
    match ascii {
        b'0'..=b'9' => Some(ascii - b'0'),
        b'a'..=b'f' => Some(ascii - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_the_test_vector_of_fips_180_2() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn reads_back_what_it_writes() {
        let digest = sha256(b"BayanDocs");
        assert_eq!(parse_sha256(&hex(&digest)), Some(digest));
    }

    #[test]
    fn refuses_anything_but_64_lowercase_hexadecimal_digits() {
        let good = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(parse_sha256(good).is_some());
        assert_eq!(parse_sha256(&good.to_uppercase()), None);
        assert_eq!(parse_sha256(&good[1..]), None);
        assert_eq!(parse_sha256(&format!("{good}0")), None);
        assert_eq!(parse_sha256(&good.replacen('b', "g", 1)), None);
        assert_eq!(parse_sha256(""), None);
        // A multi-byte character must not make the parser split a character.
        assert_eq!(parse_sha256(&format!("é{}", &good[2..])), None);
    }
}
