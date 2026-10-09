//! # crdt-workload
//!
//! The benchmark workload that work package CORE-004 runs on Loro (through bayan-crdt) and on Automerge, so that both libraries are measured on exactly the same document and the same edits ([CORE-004]).
//!
//! Everything here is deterministic: the same seed always produces the same document and the same edits, on every platform. The crate has no dependencies, so it adds nothing to either measurement.
//!
//! - [`Rng`]: a small seeded random number generator (SplitMix64).
//! - [`paragraphs`] and [`comments`]: the synthetic 500-page document of the brief, as plain data (text, formatting ranges, comment ranges).
//! - [`edit_script`]: a session of edits (typing, deleting, formatting, splitting paragraphs) to apply to the loaded document.
//! - [`resident_bytes`] and [`wasm_linear_memory_bytes`]: memory probes for native runs and WebAssembly runs.
//! - [`metric`]: prints one measurement as a line `metric <name> <value> <unit>`, which the report's tables are built from.
//!
//! **Positions.** Both libraries store the document as one sequence in which every paragraph contributes its characters plus one boundary element: the BayanDocs model ends each paragraph with a paragraph-end atom, and Automerge starts each paragraph with a block marker. A *sequence position* counts one per character (Unicode scalar value) and one per boundary element, so a document with `n` paragraphs has `characters + n` positions in both libraries, and the edit script resolves its positions against that length.
//!
//! **Status:** spike code (CORE-004). Nothing under `crates/` may depend on it.
//!
//! [CORE-004]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-004-spike-crdt-document-model.md

#![forbid(unsafe_code)]

use std::fmt::Display;
use std::ops::Range;
use std::time::Duration;

/// A seeded pseudo-random number generator (SplitMix64). Not cryptographic; it only has to be fast, reproducible and identical on every platform.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// A generator that always produces the same sequence for the same seed.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next 64 random bits.
    pub const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n`, or 0 when `n` is 0. (The tiny bias of the modulo does not matter for a benchmark or a test.)
    pub fn below(&mut self, n: usize) -> usize {
        match u64::try_from(n) {
            Ok(0) | Err(_) => 0,
            Ok(n) => usize::try_from(self.next_u64() % n).unwrap_or(0),
        }
    }

    /// A number in `low..=high` (or `low` when `high < low`).
    pub fn between(&mut self, low: usize, high: usize) -> usize {
        if high < low {
            low
        } else {
            low + self.below(high - low + 1)
        }
    }

    /// True with probability `numerator / denominator`.
    pub fn chance(&mut self, numerator: u64, denominator: u64) -> bool {
        denominator != 0 && self.next_u64() % denominator < numerator
    }

    /// A random element of a slice, or `None` when the slice is empty.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        items.get(self.below(items.len()))
    }

    /// A fresh 128-bit identifier written as 32 lowercase hexadecimal digits, as the BayanDocs model writes entity identifiers.
    pub fn hex_id(&mut self) -> String {
        format!("{:016x}{:016x}", self.next_u64(), self.next_u64())
    }
}

/// The size of a synthetic document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    /// Number of paragraphs in the main text.
    pub paragraphs: usize,
    /// Shortest paragraph, in characters.
    pub min_paragraph_chars: usize,
    /// Longest paragraph, in characters.
    pub max_paragraph_chars: usize,
    /// Number of comments.
    pub comments: usize,
    /// Number of tables (used by the full BayanDocs model only; the shared subset has none).
    pub tables: usize,
}

/// The 500-page document of the brief: about 1.5 million characters in 15,000 paragraphs (100 characters on average), 2,000 comments and 200 tables.
pub const FIVE_HUNDRED_PAGES: Shape = Shape {
    paragraphs: 15_000,
    min_paragraph_chars: 60,
    max_paragraph_chars: 140,
    comments: 2_000,
    tables: 200,
};

/// A small document with the same proportions, for quick runs and tests.
pub const TEN_PAGES: Shape = Shape {
    paragraphs: 300,
    min_paragraph_chars: 60,
    max_paragraph_chars: 140,
    comments: 40,
    tables: 4,
};

/// The seed of the synthetic document in the report.
pub const DOCUMENT_SEED: u64 = 0xC0DE_0004_D0C0_0001;

/// The seed of the edit script in the report.
pub const EDIT_SEED: u64 = 0xC0DE_0004_0ED1_7001;

/// Words the synthetic text is made of: mostly Latin, with Arabic words (two bytes per letter in UTF-8) so that positions are not byte offsets by accident.
const WORDS: [&str; 40] = [
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
    "sed",
    "do",
    "eiusmod",
    "tempor",
    "incididunt",
    "ut",
    "labore",
    "et",
    "dolore",
    "magna",
    "aliqua",
    "enim",
    "ad",
    "minim",
    "veniam",
    "quis",
    "nostrud",
    "exercitation",
    "ullamco",
    "laboris",
    "nisi",
    "aliquip",
    "\u{0643}\u{062A}\u{0627}\u{0628}",
    "\u{0645}\u{062F}\u{064A}\u{0646}\u{0629}",
    "\u{0628}\u{064A}\u{062A}",
    "\u{0642}\u{0644}\u{0645}",
    "\u{0633}\u{0644}\u{0627}\u{0645}",
    "\u{0646}\u{0648}\u{0631}",
    "document",
    "table",
    "comment",
    "paragraph",
];

/// Characters that typing inserts in the edit script.
const TYPED: [char; 12] = [
    'a', 'e', 'i', 'o', 'n', 's', 't', ' ', '\u{0627}', '\u{0644}', '\u{0645}', '.',
];

/// One paragraph of the synthetic document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paragraph {
    /// The paragraph's text (no control characters).
    pub text: String,
    /// A bold range, in characters within the paragraph.
    pub bold: Option<Range<usize>>,
    /// An italic range, in characters within the paragraph.
    pub italic: Option<Range<usize>>,
}

impl Paragraph {
    /// The length in characters (Unicode scalar values).
    #[must_use]
    pub fn chars(&self) -> usize {
        self.text.chars().count()
    }
}

/// The paragraphs of a synthetic document: words up to a random length, every tenth paragraph with a bold start (characters 0–20) and every seventh, from the fourth on, with an italic range (characters 10–40).
#[must_use]
pub fn paragraphs(rng: &mut Rng, shape: &Shape) -> Vec<Paragraph> {
    (0..shape.paragraphs)
        .map(|index| {
            let target = rng.between(shape.min_paragraph_chars, shape.max_paragraph_chars);
            let mut text = String::new();
            let mut chars = 0;
            while chars < target {
                if chars > 0 {
                    text.push(' ');
                    chars += 1;
                }
                let word = rng.pick(&WORDS).copied().unwrap_or("x");
                for character in word.chars() {
                    if chars == target {
                        break;
                    }
                    text.push(character);
                    chars += 1;
                }
            }
            let bold = (index % 10 == 0).then(|| 0..chars.min(20));
            let italic = (index % 7 == 3 && chars > 10).then(|| 10..chars.min(40));
            Paragraph { text, bold, italic }
        })
        .collect()
}

/// The number of sequence positions of a list of paragraphs: their characters plus one boundary element each.
#[must_use]
pub fn sequence_len(paragraphs: &[Paragraph]) -> usize {
    paragraphs
        .iter()
        .map(|paragraph| paragraph.chars() + 1)
        .sum()
}

/// The canonical plain text of a list of paragraphs: their texts joined by line feeds. Both benchmarks hash it to prove that they built the same document.
#[must_use]
pub fn canonical_text(paragraphs: &[Paragraph]) -> String {
    let texts: Vec<&str> = paragraphs
        .iter()
        .map(|paragraph| paragraph.text.as_str())
        .collect();
    texts.join("\n")
}

/// A comment over a range of sequence positions (which may cross paragraph boundaries).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comment {
    /// The first sequence position the comment covers.
    pub start: usize,
    /// How many positions it covers (20 to 200).
    pub len: usize,
}

/// `count` comments at random places in a sequence of `sequence_len` positions; they overlap freely.
#[must_use]
pub fn comments(rng: &mut Rng, sequence_len: usize, count: usize) -> Vec<Comment> {
    (0..count)
        .map(|_| {
            let len = rng.between(20, 200).min(sequence_len.saturating_sub(1));
            let start = rng.below(sequence_len.saturating_sub(len + 1).max(1));
            Comment { start, len }
        })
        .collect()
}

/// One edit of the edit script. Its numbers are raw random values that [`Edit::resolve`] turns into positions against the current sequence length, so that the same script applies to any document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Type `text` before a position.
    Insert {
        /// Raw random value for the position.
        at: u64,
        /// One to five typed characters.
        text: String,
    },
    /// Delete up to `len` positions.
    Delete {
        /// Raw random value for the position.
        at: u64,
        /// One to five positions.
        len: usize,
    },
    /// Make up to `len` positions bold.
    Bold {
        /// Raw random value for the position.
        at: u64,
        /// Five to fifty positions.
        len: usize,
    },
    /// Split a paragraph: insert a boundary element before a position.
    Split {
        /// Raw random value for the position.
        at: u64,
    },
}

/// An edit resolved against the current sequence length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved<'a> {
    /// Insert `text` at `at`.
    Insert {
        /// Position.
        at: usize,
        /// Text.
        text: &'a str,
    },
    /// Delete `range`.
    Delete {
        /// Positions.
        range: Range<usize>,
    },
    /// Make `range` bold.
    Bold {
        /// Positions.
        range: Range<usize>,
    },
    /// Insert a boundary element at `at`.
    Split {
        /// Position.
        at: usize,
    },
}

impl Edit {
    /// The concrete edit for a sequence of `len` positions (at least 1). Nothing is ever inserted after, or deleted at, the last position, because the BayanDocs model never lets the final paragraph end of a story go; Automerge runs the same edits so that the documents stay the same size.
    #[must_use]
    pub fn resolve(&self, len: usize) -> Resolved<'_> {
        let below = |at: u64, n: usize| match u64::try_from(n) {
            Ok(0) | Err(_) => 0,
            Ok(n) => usize::try_from(at % n).unwrap_or(0),
        };
        match self {
            Self::Insert { at, text } => Resolved::Insert {
                at: below(*at, len),
                text,
            },
            Self::Delete { at, len: count } => {
                let start = below(*at, len.saturating_sub(1));
                let end = (start + count).min(len.saturating_sub(1)).max(start);
                Resolved::Delete { range: start..end }
            }
            Self::Bold { at, len: count } => {
                let start = below(*at, len);
                Resolved::Bold {
                    range: start..(start + count).min(len),
                }
            }
            Self::Split { at } => Resolved::Split {
                at: below(*at, len),
            },
        }
    }

    /// How the sequence length changes when the resolved edit is applied.
    #[must_use]
    pub fn new_len(resolved: &Resolved<'_>, len: usize) -> usize {
        match resolved {
            Resolved::Insert { text, .. } => len + text.chars().count(),
            Resolved::Delete { range } => len - range.len(),
            Resolved::Bold { .. } => len,
            Resolved::Split { .. } => len + 1,
        }
    }
}

/// An editing session of `count` edits: 70 % typing (one to five characters), 15 % deleting (one to five positions), 10 % bolding (five to fifty positions), 5 % paragraph splits.
#[must_use]
pub fn edit_script(seed: u64, count: usize) -> Vec<Edit> {
    let mut rng = Rng::new(seed);
    (0..count)
        .map(|_| {
            let kind = rng.below(100);
            let at = rng.next_u64();
            if kind < 70 {
                let n = rng.between(1, 5);
                let text = (0..n)
                    .map(|_| rng.pick(&TYPED).copied().unwrap_or('a'))
                    .collect();
                Edit::Insert { at, text }
            } else if kind < 85 {
                Edit::Delete {
                    at,
                    len: rng.between(1, 5),
                }
            } else if kind < 95 {
                Edit::Bold {
                    at,
                    len: rng.between(5, 50),
                }
            } else {
                Edit::Split { at }
            }
        })
        .collect()
}

/// The 64-bit FNV-1a hash of some bytes (not cryptographic; it only shows that two benchmarks built the same text).
#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xCBF2_9CE4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01B3)
    })
}

/// The memory the process holds in RAM right now (its resident set size), read from `/proc/self/status` on Linux; `None` elsewhere, including WebAssembly.
#[must_use]
pub fn resident_bytes() -> Option<u64> {
    proc_status_bytes("VmRSS:")
}

/// The most memory the process has held in RAM so far (its peak resident set size), on Linux; `None` elsewhere.
#[must_use]
pub fn peak_resident_bytes() -> Option<u64> {
    proc_status_bytes("VmHWM:")
}

fn proc_status_bytes(field: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with(field))?;
    let kibibytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kibibytes.checked_mul(1024)
}

/// The size of the WebAssembly linear memory (all the memory a WebAssembly program has, which only ever grows), or `None` in a native build.
#[must_use]
pub fn wasm_linear_memory_bytes() -> Option<u64> {
    #[cfg(target_arch = "wasm32")]
    {
        let pages = u64::try_from(core::arch::wasm32::memory_size::<0>()).ok()?;
        pages.checked_mul(65_536)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// A duration in milliseconds with three decimals, computed with integers (`12.345`).
#[must_use]
pub fn millis(duration: Duration) -> String {
    let micros = duration.as_micros();
    format!("{}.{:03}", micros / 1_000, micros % 1_000)
}

/// Prints one measurement as the line `metric <name> <value> <unit>`.
#[expect(
    clippy::print_stdout,
    reason = "the benchmarks report their measurements on standard output"
)]
pub fn metric(name: &str, value: impl Display, unit: &str) {
    println!("metric {name} {value} {unit}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_is_reproducible() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        let first: Vec<u64> = (0..5).map(|_| a.next_u64()).collect();
        let second: Vec<u64> = (0..5).map(|_| b.next_u64()).collect();
        assert_eq!(first, second);
        // The first SplitMix64 output for seed 0 is a published test vector.
        assert_eq!(Rng::new(0).next_u64(), 0xE220_A839_7B1D_CDAF);
    }

    #[test]
    fn the_small_document_has_the_requested_shape() {
        let mut rng = Rng::new(DOCUMENT_SEED);
        let paragraphs = paragraphs(&mut rng, &TEN_PAGES);
        assert_eq!(paragraphs.len(), TEN_PAGES.paragraphs);
        for paragraph in &paragraphs {
            let chars = paragraph.chars();
            assert!((60..=140).contains(&chars), "{chars}");
            assert!(!paragraph.text.chars().any(char::is_control));
        }
        assert_eq!(paragraphs[0].bold, Some(0..20));
        assert!(paragraphs[3].italic.is_some());
        let len = sequence_len(&paragraphs);
        for comment in comments(&mut rng, len, 100) {
            assert!(comment.start + comment.len < len);
            assert!((20..=200).contains(&comment.len));
        }
    }

    #[test]
    fn edits_stay_inside_the_sequence_and_never_touch_the_last_position() {
        let mut len = 10;
        for edit in edit_script(EDIT_SEED, 2_000) {
            let resolved = edit.resolve(len);
            match &resolved {
                Resolved::Insert { at, text } => {
                    assert!(*at < len);
                    assert!((1..=5).contains(&text.chars().count()));
                }
                Resolved::Delete { range } => assert!(range.end < len),
                Resolved::Bold { range } => assert!(range.end <= len && range.start < len),
                Resolved::Split { at } => assert!(*at < len),
            }
            len = Edit::new_len(&resolved, len);
            // The last position (the final paragraph end) is never deleted.
            assert!(len >= 1);
        }
    }

    #[test]
    fn formats_milliseconds_with_integers() {
        assert_eq!(millis(Duration::from_micros(12_345)), "12.345");
        assert_eq!(millis(Duration::from_micros(7)), "0.007");
    }

    #[test]
    fn fnv_matches_the_published_test_vector() {
        assert_eq!(fnv1a64(b""), 0xCBF2_9CE4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xAF63_DC4C_8601_EC8C);
    }
}
