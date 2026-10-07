//! Story sequences: rich text in which special atoms are placeholder characters bound to entities by a mark.

use std::collections::BTreeMap;
use std::ops::Range;

use loro::cursor::PosType;
use loro::{LoroText, TextDelta};

use crate::{CrdtError, Value};

/// The mark key that binds a placeholder character to the entity it stands for (document model §16: "placeholder character plus a mark `atom` = { kind, id } with no expansion"). The adapter always configures it not to expand, so text typed next to an atom never inherits its binding.
pub const ATOM_KEY: &str = "atom";

/// How a mark grows when text is inserted exactly at one of its boundaries (document model §5). Text inserted strictly inside a marked range always takes the mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expand {
    /// Never grows: text typed at either boundary stays outside.
    None,
    /// Grows at its start: text typed just before the range takes the mark.
    Before,
    /// Grows at its end: text typed just after the range takes the mark (how Word continues a run's formatting).
    After,
    /// Grows at both boundaries.
    Both,
}

/// A family of mark keys and how its marks expand.
///
/// A family named `r` covers the key `r` itself and every key `r:<anything>`: the CRDT library chooses the expansion of a key by the text before its first colon, so dynamically named keys such as one per comment (`cmt:<CommentId>`) need no configuration of their own. A family name therefore must not contain a colon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkFamily {
    /// The family name: the whole key, or the part before its first colon.
    pub name: &'static str,
    /// How its marks expand.
    pub expand: Expand,
}

/// The family of [`ATOM_KEY`], which the adapter configures itself.
pub(crate) const ATOM_FAMILY: MarkFamily = MarkFamily {
    name: ATOM_KEY,
    expand: Expand::None,
};

/// Consecutive characters of a story that carry the same marks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// The characters, placeholders included.
    pub text: String,
    /// The marks on every character of the run, by key. Keys whose value is null (cleared marks) are left out.
    pub marks: BTreeMap<String, Value>,
}

impl Run {
    /// The number of characters (Unicode scalar values) in the run.
    #[must_use]
    pub fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// Whether the run has no characters.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// One story: a sequence of atoms stored as rich text, with positions counted in Unicode scalar values (one per atom).
///
/// Cloning a `Story` gives another handle to the same sequence.
#[derive(Debug, Clone)]
pub struct Story {
    text: LoroText,
}

impl Story {
    pub(crate) const fn new(text: LoroText) -> Self {
        Self { text }
    }

    /// The number of atoms.
    #[must_use]
    pub fn len(&self) -> usize {
        self.text.len_unicode()
    }

    /// Whether the story has no atoms at all (a well-formed story always has at least its final paragraph end).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The characters of the story, placeholders included.
    #[must_use]
    pub fn text(&self) -> String {
        self.text.to_string()
    }

    /// The character at `pos`, if there is one.
    #[must_use]
    pub fn char_at(&self, pos: usize) -> Option<char> {
        self.text.char_at(pos).ok()
    }

    /// Inserts `text` before position `pos` (`pos == len()` appends).
    ///
    /// The adapter stores whatever characters it is given; keeping placeholder code points out of user text is the model's job.
    ///
    /// # Errors
    ///
    /// [`CrdtError::OutOfRange`] if `pos > len()`, or [`CrdtError::Library`] if the CRDT library refuses.
    pub fn insert(&self, pos: usize, text: &str) -> Result<(), CrdtError> {
        self.check_position(pos)?;
        if text.is_empty() {
            return Ok(());
        }
        self.text.insert(pos, text).map_err(CrdtError::library)
    }

    /// Deletes the atoms in `range`.
    ///
    /// # Errors
    ///
    /// [`CrdtError::OutOfRange`] if the range is reversed or reaches past the end, or [`CrdtError::Library`].
    pub fn delete(&self, range: Range<usize>) -> Result<(), CrdtError> {
        self.check_range(&range)?;
        if range.is_empty() {
            return Ok(());
        }
        self.text
            .delete(range.start, range.len())
            .map_err(CrdtError::library)
    }

    /// Sets the mark `key` to `value` on every atom in `range`. How the mark expands later depends on the family of `key` (see [`MarkFamily`]).
    ///
    /// # Errors
    ///
    /// [`CrdtError::OutOfRange`], [`CrdtError::InvalidValue`] for a value that cannot be stored, or [`CrdtError::Library`].
    pub fn mark(&self, range: Range<usize>, key: &str, value: &Value) -> Result<(), CrdtError> {
        self.check_range(&range)?;
        if range.is_empty() {
            return Ok(());
        }
        let value = value.to_loro()?;
        self.text
            .mark(range, key, value)
            .map_err(CrdtError::library)
    }

    /// Removes the mark `key` from every atom in `range`.
    ///
    /// # Errors
    ///
    /// [`CrdtError::OutOfRange`] or [`CrdtError::Library`].
    pub fn unmark(&self, range: Range<usize>, key: &str) -> Result<(), CrdtError> {
        self.check_range(&range)?;
        if range.is_empty() {
            return Ok(());
        }
        self.text.unmark(range, key).map_err(CrdtError::library)
    }

    /// Inserts one placeholder character at `pos` and binds it to an entity: the mark [`ATOM_KEY`] with the value `binding` on exactly that character. Both happen in the same transaction, so no replica ever sees the placeholder without its binding.
    ///
    /// # Errors
    ///
    /// As for [`Story::insert`] and [`Story::mark`].
    pub fn insert_atom(
        &self,
        pos: usize,
        placeholder: char,
        binding: &str,
    ) -> Result<(), CrdtError> {
        self.check_position(pos)?;
        let mut buffer = [0; 4];
        self.text
            .insert(pos, placeholder.encode_utf8(&mut buffer))
            .map_err(CrdtError::library)?;
        self.text
            .mark(pos..pos + 1, ATOM_KEY, binding)
            .map_err(CrdtError::library)
    }

    /// The whole story as runs, in order; adjacent runs always differ in their marks.
    #[must_use]
    pub fn runs(&self) -> Vec<Run> {
        runs_from_delta(self.text.to_delta())
    }

    /// The runs covering `range`, as for [`Story::runs`].
    ///
    /// # Errors
    ///
    /// [`CrdtError::OutOfRange`] or [`CrdtError::Library`].
    pub fn runs_in(&self, range: Range<usize>) -> Result<Vec<Run>, CrdtError> {
        self.check_range(&range)?;
        if range.is_empty() {
            return Ok(Vec::new());
        }
        let delta = self
            .text
            .slice_delta(range.start, range.end, PosType::Unicode)
            .map_err(CrdtError::library)?;
        Ok(runs_from_delta(delta))
    }

    fn check_position(&self, pos: usize) -> Result<(), CrdtError> {
        let len = self.len();
        if pos > len {
            return Err(CrdtError::OutOfRange { pos, len });
        }
        Ok(())
    }

    fn check_range(&self, range: &Range<usize>) -> Result<(), CrdtError> {
        let len = self.len();
        if range.start > range.end || range.end > len {
            return Err(CrdtError::OutOfRange {
                pos: range.end.max(range.start),
                len,
            });
        }
        Ok(())
    }
}

/// Converts the library's delta into runs: marks into ordered maps without null values (the order of the library's hash maps cannot leak out), and adjacent runs with equal marks merged.
fn runs_from_delta(delta: Vec<TextDelta>) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::with_capacity(delta.len());
    for item in delta {
        let TextDelta::Insert { insert, attributes } = item else {
            continue;
        };
        if insert.is_empty() {
            continue;
        }
        let marks: BTreeMap<String, Value> = attributes
            .map(|attributes| {
                attributes
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::from_loro(value)))
                    .filter(|(_, value)| !value.is_null())
                    .collect()
            })
            .unwrap_or_default();
        match runs.last_mut() {
            Some(last) if last.marks == marks => last.text.push_str(&insert),
            _ => runs.push(Run {
                text: insert,
                marks,
            }),
        }
    }
    runs
}
