//! SHA-256 digests, the identity of every corpus document and license text.

use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256 as Sha256Hasher};

/// A SHA-256 digest, written as 64 lower-case hexadecimal digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256([u8; 32]);

impl Sha256 {
    /// The digest of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256Hasher::digest(bytes).into())
    }

    /// The digest's 32 bytes.
    #[must_use]
    pub const fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The first 12 hexadecimal digits, enough to tell documents apart in messages.
    #[must_use]
    pub fn short(&self) -> String {
        let mut text = self.to_string();
        text.truncate(12);
        text
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Why text is not a SHA-256 digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotADigest;

impl fmt::Display for NotADigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("not a SHA-256 digest: expected 64 lower-case hexadecimal digits")
    }
}

impl std::error::Error for NotADigest {}

impl FromStr for Sha256 {
    type Err = NotADigest;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let digits = text.as_bytes();
        if digits.len() != 64 {
            return Err(NotADigest);
        }
        let mut bytes = [0_u8; 32];
        let (pairs, _) = digits.as_chunks::<2>();
        for (byte, &[high, low]) in bytes.iter_mut().zip(pairs) {
            let (Some(high), Some(low)) = (hex_digit(high), hex_digit(low)) else {
                return Err(NotADigest);
            };
            *byte = (high << 4) | low;
        }
        Ok(Self(bytes))
    }
}

/// The value of a lower-case hexadecimal digit. Upper-case digits are refused, so every digest has exactly one spelling.
const fn hex_digit(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}

impl Serialize for Sha256 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Sha256 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Sha256 {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Sha256".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A SHA-256 digest: 64 lower-case hexadecimal digits.",
            "type": "string",
            "pattern": "^[0-9a-f]{64}$"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_match_the_published_test_vectors() {
        // FIPS 180-2, appendix B.1 and the empty message.
        assert_eq!(
            Sha256::of(b"abc").to_string(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            Sha256::of(b"").to_string(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn text_round_trips_and_only_one_spelling_is_accepted() {
        let digest = Sha256::of(b"abc");
        let text = digest.to_string();
        assert_eq!(text.parse::<Sha256>(), Ok(digest));
        assert_eq!(digest.short(), "ba7816bf8f01");
        assert_eq!(text.to_uppercase().parse::<Sha256>(), Err(NotADigest));
        assert_eq!(text[1..].parse::<Sha256>(), Err(NotADigest));
        assert_eq!(format!("{text}0").parse::<Sha256>(), Err(NotADigest));
        assert_eq!(
            format!("{}g", &text[..63]).parse::<Sha256>(),
            Err(NotADigest)
        );
    }

    #[test]
    fn serializes_as_a_string() {
        let digest = Sha256::of(b"abc");
        let json = serde_json::to_string(&digest).unwrap();
        assert_eq!(json, format!("\"{digest}\""));
        assert_eq!(serde_json::from_str::<Sha256>(&json).unwrap(), digest);
        assert!(serde_json::from_str::<Sha256>("\"abc\"").is_err());
    }
}
