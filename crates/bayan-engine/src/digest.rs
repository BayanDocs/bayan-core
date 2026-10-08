//! Digests of tiles and of the engine's output, for record and replay ([engine protocol specification][spec] §10).
//!
//! A digest is written `fnv1a64:` followed by 16 lower-case hexadecimal digits: the 64-bit FNV-1a hash of the bytes. FNV-1a is a simple, well-known, non-cryptographic hash, so the C and Node test drivers can compute exactly the same digest of the pixels they receive in a few lines of their own. It detects accidental differences, such as a replay that renders one pixel differently; it does not protect recordings against deliberate tampering, and nothing relies on it for security.
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

/// The prefix of every digest, which names the algorithm so a later version can switch to another one.
pub const PREFIX: &str = "fnv1a64:";

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0100_0000_01b3;

/// An FNV-1a hash being computed, for bytes that arrive in pieces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fnv1a64(u64);

impl Fnv1a64 {
    /// Starts a hash.
    #[must_use]
    pub const fn new() -> Self {
        Self(OFFSET_BASIS)
    }

    /// Adds bytes.
    pub fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(PRIME);
        }
    }

    /// The hash of the bytes added so far.
    #[must_use]
    pub const fn value(&self) -> u64 {
        self.0
    }

    /// The digest of the bytes added so far, as text.
    #[must_use]
    pub fn digest(&self) -> String {
        format!("{PREFIX}{:016x}", self.0)
    }
}

impl Default for Fnv1a64 {
    fn default() -> Self {
        Self::new()
    }
}

/// The digest of some bytes, such as a tile's pixels.
#[must_use]
pub fn of_bytes(bytes: &[u8]) -> String {
    let mut hash = Fnv1a64::new();
    hash.update(bytes);
    hash.digest()
}

/// The digest of the messages the engine sent while handling one entry of a recording: each message's bytes followed by a line feed, in order.
#[must_use]
pub fn of_messages(messages: &[String]) -> String {
    let mut hash = Fnv1a64::new();
    for message in messages {
        hash.update(message.as_bytes());
        hash.update(b"\n");
    }
    hash.digest()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_published_fnv_1a_test_vectors() {
        // From the FNV reference (Fowler, Noll and Vo): the empty string, "a" and "foobar".
        assert_eq!(of_bytes(b""), "fnv1a64:cbf29ce484222325");
        assert_eq!(of_bytes(b"a"), "fnv1a64:af63dc4c8601ec8c");
        assert_eq!(of_bytes(b"foobar"), "fnv1a64:85944171f73967e8");
    }

    #[test]
    fn hashing_in_pieces_equals_hashing_at_once() {
        let mut hash = Fnv1a64::new();
        hash.update(b"foo");
        hash.update(b"bar");
        assert_eq!(hash.digest(), of_bytes(b"foobar"));
        assert_eq!(hash.value(), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn message_digests_keep_message_boundaries() {
        let together = of_messages(&["ab".to_owned()]);
        let apart = of_messages(&["a".to_owned(), "b".to_owned()]);
        assert_ne!(together, apart);
        assert_eq!(of_messages(&[]), of_bytes(b""));
    }
}
