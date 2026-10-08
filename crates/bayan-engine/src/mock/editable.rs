//! The mock document's editable line: committed text, a caret, and an optional input method composition.
//!
//! Positions are counted in characters (Unicode scalar values) inside the engine and converted to UTF-16 code units at the protocol boundary (spec §7). A composition is shown at the caret, between the committed text before and after it, until it ends or committed text replaces it.

use serde::{Deserialize, Serialize};

use crate::limits::{MAX_COMPOSITION_UNITS, MAX_EDITABLE_TEXT_UNITS};
use crate::protocol::TextRange;

/// An edit would make the text longer than its limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TooLong;

/// The state of the editable line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EditableLine {
    /// The committed text.
    text: String,
    /// The caret, in characters into `text`.
    caret: u32,
    /// The composition, if one is active.
    composition: Option<Composition>,
}

/// An input method composition (pre-edit text).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Composition {
    /// The composition's text.
    text: String,
    /// The start of the input method's selection, in characters into `text`.
    selection_start: u32,
    /// Its end: where the input method's caret is.
    selection_end: u32,
}

/// One character as the line shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shown {
    /// The character.
    pub(crate) character: char,
    /// Whether it belongs to the composition.
    pub(crate) composed: bool,
    /// Whether it is inside the composition's selection.
    pub(crate) selected: bool,
}

/// Removes characters a single line cannot hold: control characters and the line and paragraph separators.
fn clean(text: &str) -> String {
    text.chars()
        .filter(|&character| {
            !character.is_control() && !matches!(character, '\u{2028}' | '\u{2029}')
        })
        .collect()
}

/// The length of a string in UTF-16 code units.
fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The character at or before a UTF-16 offset into `text`: an offset inside a surrogate pair, or beyond the end, moves back to the nearest character boundary.
fn char_index_at(text: &str, units: u32) -> u32 {
    let mut seen = 0_u32;
    let mut index = 0_u32;
    for character in text.chars() {
        let next = seen + u32::try_from(character.len_utf16()).unwrap_or(1);
        if next > units {
            break;
        }
        seen = next;
        index += 1;
    }
    index
}

/// The number of characters in a string.
fn char_count(text: &str) -> u32 {
    u32::try_from(text.chars().count()).unwrap_or(u32::MAX)
}

/// The byte position of character `index` in `text` (the end if it is past the last character).
fn byte_at(text: &str, index: u32) -> usize {
    text.char_indices()
        .nth(usize::try_from(index).unwrap_or(usize::MAX))
        .map_or(text.len(), |(position, _)| position)
}

impl EditableLine {
    /// Whether a composition is active.
    pub(crate) fn composing(&self) -> bool {
        self.composition.is_some()
    }

    /// Inserts committed text at the caret. An active composition is replaced by it and ends.
    pub(crate) fn insert(&mut self, text: &str) -> Result<bool, TooLong> {
        let text = clean(text);
        if utf16_len(&self.text) + utf16_len(&text) > MAX_EDITABLE_TEXT_UNITS {
            return Err(TooLong);
        }
        let had_composition = self.composition.take().is_some();
        if text.is_empty() {
            return Ok(had_composition);
        }
        let at = byte_at(&self.text, self.caret);
        self.text.insert_str(at, &text);
        self.caret += char_count(&text);
        Ok(true)
    }

    /// Applies an editing key, unless a composition is active. Returns whether anything changed.
    pub(crate) fn key(&mut self, key: &str) -> bool {
        if self.composing() {
            return false;
        }
        let length = char_count(&self.text);
        match key {
            "Backspace" if self.caret > 0 => {
                let at = byte_at(&self.text, self.caret - 1);
                self.text.remove(at);
                self.caret -= 1;
            }
            "Delete" if self.caret < length => {
                let at = byte_at(&self.text, self.caret);
                self.text.remove(at);
            }
            "ArrowLeft" if self.caret > 0 => self.caret -= 1,
            "ArrowRight" if self.caret < length => self.caret += 1,
            "Home" if self.caret > 0 => self.caret = 0,
            "End" if self.caret < length => self.caret = length,
            _ => return false,
        }
        true
    }

    /// Starts an empty composition at the caret, replacing one that is active.
    pub(crate) fn start_composition(&mut self) -> bool {
        let changed = self
            .composition
            .as_ref()
            .is_none_or(|composition| !composition.text.is_empty());
        self.composition = Some(Composition::default());
        changed
    }

    /// Replaces the composition's text and selection, starting a composition if none is active. Without a selection, the input method's caret is at the end of the text.
    pub(crate) fn update_composition(
        &mut self,
        text: &str,
        selection: Option<TextRange>,
    ) -> Result<bool, TooLong> {
        let text = clean(text);
        if utf16_len(&text) > MAX_COMPOSITION_UNITS {
            return Err(TooLong);
        }
        let length = char_count(&text);
        let (start, end) = selection.map_or((length, length), |range| {
            let start = char_index_at(&text, range.start);
            let end = char_index_at(&text, range.end);
            if start <= end {
                (start, end)
            } else {
                (end, start)
            }
        });
        let composition = Composition {
            text,
            selection_start: start,
            selection_end: end,
        };
        let changed = self.composition.as_ref() != Some(&composition);
        self.composition = Some(composition);
        Ok(changed)
    }

    /// Ends the composition and discards its text.
    pub(crate) fn end_composition(&mut self) -> bool {
        self.composition
            .take()
            .is_some_and(|composition| !composition.text.is_empty())
    }

    /// Moves the caret to character `index` of the committed text, unless a composition is active.
    pub(crate) fn set_caret(&mut self, index: u32) -> bool {
        let index = index.min(char_count(&self.text));
        if self.composing() || index == self.caret {
            return false;
        }
        self.caret = index;
        true
    }

    /// The characters as the line shows them: the committed text with the composition at the caret.
    pub(crate) fn shown(&self) -> Vec<Shown> {
        let committed = |character| Shown {
            character,
            composed: false,
            selected: false,
        };
        let split = byte_at(&self.text, self.caret);
        let mut shown: Vec<Shown> = self.text[..split].chars().map(committed).collect();
        if let Some(composition) = &self.composition {
            for (index, character) in composition.text.chars().enumerate() {
                let index = u32::try_from(index).unwrap_or(u32::MAX);
                shown.push(Shown {
                    character,
                    composed: true,
                    selected: index >= composition.selection_start
                        && index < composition.selection_end,
                });
            }
        }
        shown.extend(self.text[split..].chars().map(committed));
        shown
    }

    /// The caret's position in the shown characters: inside an active composition, the input method's caret.
    pub(crate) fn shown_caret(&self) -> u32 {
        self.caret
            + self
                .composition
                .as_ref()
                .map_or(0, |composition| composition.selection_end)
    }

    /// The shown text, as the accessibility tree reports it.
    pub(crate) fn shown_text(&self) -> String {
        self.shown().iter().map(|shown| shown.character).collect()
    }

    /// Whether a state read from a recording keeps every limit and points inside its text (spec §10: recordings are untrusted).
    pub(crate) fn is_valid(&self) -> bool {
        let text_ok = utf16_len(&self.text) <= MAX_EDITABLE_TEXT_UNITS
            && self.text == clean(&self.text)
            && self.caret <= char_count(&self.text);
        let composition_ok = self.composition.as_ref().is_none_or(|composition| {
            utf16_len(&composition.text) <= MAX_COMPOSITION_UNITS
                && composition.text == clean(&composition.text)
                && composition.selection_start <= composition.selection_end
                && composition.selection_end <= char_count(&composition.text)
        });
        text_ok && composition_ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown_string(line: &EditableLine) -> String {
        line.shown_text()
    }

    #[test]
    fn inserts_at_the_caret_and_edits_with_keys() {
        let mut line = EditableLine::default();
        assert_eq!(line.insert("Hllo"), Ok(true));
        assert!(line.key("Home"));
        assert!(line.key("ArrowRight"));
        assert_eq!(line.insert("e"), Ok(true));
        assert_eq!(shown_string(&line), "Hello");
        assert!(line.key("End"));
        assert!(line.key("Backspace"));
        assert!(line.key("Home"));
        assert!(line.key("Delete"));
        assert_eq!(shown_string(&line), "ell");
        assert!(!line.key("Home"));
        assert!(!line.key("Tab"));
    }

    #[test]
    fn removes_control_characters_and_line_breaks() {
        let mut line = EditableLine::default();
        assert_eq!(line.insert("a\nb\tc\u{2028}d\u{7f}"), Ok(true));
        assert_eq!(shown_string(&line), "abcd");
        assert_eq!(line.insert("\n"), Ok(false));
    }

    #[test]
    fn shows_a_composition_at_the_caret_until_text_replaces_it() {
        let mut line = EditableLine::default();
        line.insert("ab").unwrap();
        line.key("ArrowLeft");
        assert!(line.start_composition());
        // The input method composes two characters and selects the second one.
        let selection = Some(TextRange { start: 1, end: 2 });
        assert_eq!(line.update_composition("にほ", selection), Ok(true));
        assert_eq!(shown_string(&line), "aにほb");
        let flags: Vec<(bool, bool)> = line
            .shown()
            .iter()
            .map(|s| (s.composed, s.selected))
            .collect();
        assert_eq!(
            flags,
            [(false, false), (true, false), (true, true), (false, false)]
        );
        assert_eq!(line.shown_caret(), 3);
        // Keys and clicks belong to the input method while it composes.
        assert!(!line.key("Backspace"));
        assert!(!line.set_caret(0));
        // Committing replaces the composition.
        assert_eq!(line.insert("日本"), Ok(true));
        assert!(!line.composing());
        assert_eq!(shown_string(&line), "a日本b");
        assert_eq!(line.shown_caret(), 3);
    }

    #[test]
    fn ending_a_composition_discards_its_text() {
        let mut line = EditableLine::default();
        line.update_composition("한", None).unwrap();
        assert_eq!(line.shown_caret(), 1);
        assert!(line.end_composition());
        assert_eq!(shown_string(&line), "");
        assert!(!line.end_composition());
    }

    #[test]
    fn maps_utf16_selections_to_characters() {
        let mut line = EditableLine::default();
        // "😀" is two UTF-16 code units; an offset inside it moves back to its start, so 1 to 2 selects the emoji.
        let selection = Some(TextRange { start: 1, end: 2 });
        line.update_composition("😀x", selection).unwrap();
        let flags: Vec<bool> = line.shown().iter().map(|s| s.selected).collect();
        assert_eq!(flags, [true, false]);
        assert_eq!(line.shown_caret(), 1);
    }

    #[test]
    fn enforces_the_limits() {
        let mut line = EditableLine::default();
        assert_eq!(line.insert(&"a".repeat(MAX_EDITABLE_TEXT_UNITS)), Ok(true));
        assert_eq!(line.insert("b"), Err(TooLong));
        assert_eq!(
            line.update_composition(&"c".repeat(MAX_COMPOSITION_UNITS + 1), None),
            Err(TooLong)
        );
        assert!(line.is_valid());
    }

    #[test]
    fn notices_invalid_states_from_recordings() {
        let valid = EditableLine {
            text: "ab".to_owned(),
            caret: 2,
            composition: None,
        };
        assert!(valid.is_valid());
        let beyond = EditableLine {
            caret: 3,
            ..valid.clone()
        };
        assert!(!beyond.is_valid());
        let control = EditableLine {
            text: "a\n".to_owned(),
            ..valid.clone()
        };
        assert!(!control.is_valid());
        let selection = EditableLine {
            composition: Some(Composition {
                text: "x".to_owned(),
                selection_start: 1,
                selection_end: 0,
            }),
            ..valid
        };
        assert!(!selection.is_valid());
    }
}
