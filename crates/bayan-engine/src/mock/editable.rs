//! The mock document's editable line: committed text, a caret with an optional selection, and an optional input method composition.
//!
//! Positions are counted in characters (Unicode scalar values) inside the engine and converted to UTF-16 code units at the protocol boundary (spec §7). A composition is shown at the caret, between the committed text before and after it, until it ends or committed text replaces it. Positions that the shell sends (`input.text` with `replace`, `input.select`) count the text as the line shows it, the composition included, as the accessibility tree reports it (spec §14.4).
//!
//! The selection runs from an anchor, which stays, to the caret, which moves (spec §14.4). Typing and pasting replace it, Backspace and Delete delete it, the arrows without Shift collapse it, and Shift with ArrowLeft, ArrowRight, Home and End extends it. A composition replaces it when it starts, and while one is active nothing is selected: the input method owns the caret.

use serde::{Deserialize, Serialize};

use crate::limits::{MAX_COMPOSITION_UNITS, MAX_EDITABLE_TEXT_UNITS};
use crate::protocol::TextRange;

/// An edit would make the text longer than its limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TooLong;

/// Why the line refuses a range or a position that the shell sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditError {
    /// Beyond the shown text, backwards, or inside a surrogate pair (`invalid_request`).
    Invalid,
    /// The text would be longer than its limit (`limit_exceeded`).
    TooLong,
}

/// The state of the editable line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EditableLine {
    /// The committed text.
    text: String,
    /// The caret, in characters into `text`: the selection's focus.
    caret: u32,
    /// The selection's anchor, in characters into `text`, when something is selected: never equal to the caret, and absent while a composition is active. Recordings of engines before CORE-014 have none.
    #[serde(default)]
    anchor: Option<u32>,
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

/// Whether a single line can hold a character: not a control character, nor the line or paragraph separator.
fn holds(character: char) -> bool {
    !character.is_control() && !matches!(character, '\u{2028}' | '\u{2029}')
}

/// Removes characters a single line cannot hold: control characters and the line and paragraph separators.
fn clean(text: &str) -> String {
    text.chars().filter(|&character| holds(character)).collect()
}

/// The text of clipboard bytes in `text/plain` (spec §14.5), as the line would insert it: UTF-8 with every invalid sequence replaced by U+FFFD, as `String::from_utf8_lossy` replaces it, and without the characters the line cannot hold. The bytes are untrusted and may fill a blob of 64 MiB, so they are decoded only as far as the line's limit: text longer than the line can ever hold is refused before more of it is kept.
pub(crate) fn clipboard_text(bytes: &[u8]) -> Result<String, TooLong> {
    let mut text = String::new();
    let mut units = 0;
    for chunk in bytes.utf8_chunks() {
        let replacement = (!chunk.invalid().is_empty()).then_some('\u{FFFD}');
        for character in chunk.valid().chars().chain(replacement) {
            if holds(character) {
                units += character.len_utf16();
                if units > MAX_EDITABLE_TEXT_UNITS {
                    return Err(TooLong);
                }
                text.push(character);
            }
        }
    }
    Ok(text)
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

/// The character whose start is at UTF-16 offset `units` of characters `shown`, or `None` if the offset is beyond them or inside a surrogate pair (spec §7).
fn boundary(shown: &[Shown], units: u32) -> Option<usize> {
    let mut seen = 0_u32;
    for (index, character) in shown.iter().enumerate() {
        if seen == units {
            return Some(index);
        }
        if seen > units {
            return None;
        }
        seen += u32::try_from(character.character.len_utf16()).unwrap_or(1);
    }
    (seen == units).then_some(shown.len())
}

/// Characters as a string.
fn collect(characters: impl Iterator<Item = char>) -> String {
    characters.collect()
}

impl EditableLine {
    /// Whether a composition is active.
    pub(crate) fn composing(&self) -> bool {
        self.composition.is_some()
    }

    /// The selection's anchor, in characters into the committed text: the caret when nothing is selected.
    fn anchor(&self) -> u32 {
        self.anchor.unwrap_or(self.caret)
    }

    /// The selection as the range of the committed text it covers, in characters; empty when nothing is selected.
    fn selected(&self) -> (u32, u32) {
        let anchor = self.anchor();
        (anchor.min(self.caret), anchor.max(self.caret))
    }

    /// Moves the caret, keeping the anchor where it was if `extend`, so that the selection grows or shrinks, and otherwise collapsing the selection.
    fn move_caret(&mut self, to: u32, extend: bool) {
        let anchor = self.anchor();
        self.caret = to;
        self.anchor = (extend && anchor != to).then_some(anchor);
    }

    /// Deletes the selected text, leaving the caret where it was. Returns whether anything was selected.
    fn delete_selection(&mut self) -> bool {
        let (start, end) = self.selected();
        if start == end {
            return false;
        }
        let range = byte_at(&self.text, start)..byte_at(&self.text, end);
        self.text.replace_range(range, "");
        self.caret = start;
        self.anchor = None;
        true
    }

    /// The selected text; empty when nothing is selected.
    pub(crate) fn selected_text(&self) -> String {
        let (start, end) = self.selected();
        self.text[byte_at(&self.text, start)..byte_at(&self.text, end)].to_owned()
    }

    /// Deletes the selected text, for `clipboard.copy` with `cut` (spec §14.5). Returns whether anything changed.
    pub(crate) fn cut(&mut self) -> bool {
        !self.composing() && self.delete_selection()
    }

    /// Inserts committed text: in place of an active composition, which it ends, or of the selection, or at the caret (spec §6.5, §14.4). Text that is empty once control characters are removed changes nothing but ending a composition.
    pub(crate) fn insert(&mut self, text: &str) -> Result<bool, TooLong> {
        let text = clean(text);
        let (start, end) = self.selected();
        let selected = utf16_len(&self.text[byte_at(&self.text, start)..byte_at(&self.text, end)]);
        if utf16_len(&self.text) - selected + utf16_len(&text) > MAX_EDITABLE_TEXT_UNITS {
            return Err(TooLong);
        }
        let had_composition = self.composition.take().is_some();
        if text.is_empty() {
            return Ok(had_composition);
        }
        self.delete_selection();
        let at = byte_at(&self.text, self.caret);
        self.text.insert_str(at, &text);
        self.caret += char_count(&text);
        Ok(true)
    }

    /// Replaces the UTF-16 range `start` to `end` of the shown text with committed text, which ends an active composition: what of the composition's text lies outside the range is discarded with it. The caret ends after the new text, and nothing is selected (spec §14.4). Returns whether anything changed.
    pub(crate) fn replace(&mut self, start: u32, end: u32, text: &str) -> Result<bool, EditError> {
        let shown = self.shown();
        let (Some(from), Some(to)) = (boundary(&shown, start), boundary(&shown, end)) else {
            return Err(EditError::Invalid);
        };
        if from > to {
            return Err(EditError::Invalid);
        }
        let text = clean(text);
        let committed = |part: &[Shown]| {
            collect(
                part.iter()
                    .filter(|character| !character.composed)
                    .map(|character| character.character),
            )
        };
        let before = committed(&shown[..from]);
        let mut new_text = before.clone();
        new_text.push_str(&text);
        new_text.push_str(&committed(&shown[to..]));
        if utf16_len(&new_text) > MAX_EDITABLE_TEXT_UNITS {
            return Err(EditError::TooLong);
        }
        let old = self.clone();
        self.text = new_text;
        self.caret = char_count(&before) + char_count(&text);
        self.anchor = None;
        self.composition = None;
        Ok(*self != old)
    }

    /// Sets the selection from the UTF-16 offsets `anchor` and `focus` of the shown text (spec §14.4), unless a composition is active, whose input method owns the caret; an offset beyond the text or inside a surrogate pair is refused either way. Returns whether the selection changed.
    pub(crate) fn select(&mut self, anchor: u32, focus: u32) -> Result<bool, EditError> {
        let shown = self.shown();
        let (Some(anchor), Some(focus)) = (boundary(&shown, anchor), boundary(&shown, focus))
        else {
            return Err(EditError::Invalid);
        };
        if self.composing() {
            return Ok(false);
        }
        let (anchor, focus) = (
            u32::try_from(anchor).unwrap_or(u32::MAX),
            u32::try_from(focus).unwrap_or(u32::MAX),
        );
        let old = (self.caret, self.anchor);
        self.caret = focus;
        self.anchor = (anchor != focus).then_some(anchor);
        Ok((self.caret, self.anchor) != old)
    }

    /// Applies an editing key, unless a composition is active; `shift` extends the selection with ArrowLeft, ArrowRight, Home and End. Returns whether anything changed.
    pub(crate) fn key(&mut self, key: &str, shift: bool) -> bool {
        if self.composing() {
            return false;
        }
        let old = self.clone();
        let length = char_count(&self.text);
        let (start, end) = self.selected();
        match key {
            "Backspace" | "Delete" if start != end => {
                self.delete_selection();
            }
            "Backspace" if self.caret > 0 => {
                let at = byte_at(&self.text, self.caret - 1);
                self.text.remove(at);
                self.caret -= 1;
            }
            "Delete" if self.caret < length => {
                let at = byte_at(&self.text, self.caret);
                self.text.remove(at);
            }
            "ArrowLeft" if shift => self.move_caret(self.caret.saturating_sub(1), true),
            "ArrowRight" if shift => self.move_caret((self.caret + 1).min(length), true),
            // Without Shift, an arrow collapses a selection to the end it points to.
            "ArrowLeft" if start != end => self.move_caret(start, false),
            "ArrowRight" if start != end => self.move_caret(end, false),
            "ArrowLeft" => self.move_caret(self.caret.saturating_sub(1), false),
            "ArrowRight" => self.move_caret((self.caret + 1).min(length), false),
            "Home" => self.move_caret(0, shift),
            "End" => self.move_caret(length, shift),
            _ => return false,
        }
        *self != old
    }

    /// Starts an empty composition at the caret, replacing one that is active; it replaces the selection, which it deletes (spec §14.4).
    pub(crate) fn start_composition(&mut self) -> bool {
        let deleted = self.delete_selection();
        let changed = self
            .composition
            .as_ref()
            .is_none_or(|composition| !composition.text.is_empty());
        self.composition = Some(Composition::default());
        changed || deleted
    }

    /// Replaces the composition's text and selection, starting a composition if none is active, which replaces the selection. Without a selection, the input method's caret is at the end of the text.
    pub(crate) fn update_composition(
        &mut self,
        text: &str,
        selection: Option<TextRange>,
    ) -> Result<bool, TooLong> {
        let text = clean(text);
        if utf16_len(&text) > MAX_COMPOSITION_UNITS {
            return Err(TooLong);
        }
        let deleted = !self.composing() && self.delete_selection();
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
        Ok(changed || deleted)
    }

    /// Ends the composition and discards its text.
    pub(crate) fn end_composition(&mut self) -> bool {
        self.composition
            .take()
            .is_some_and(|composition| !composition.text.is_empty())
    }

    /// Moves the caret to character `index` of the committed text and collapses the selection there, unless a composition is active. Returns whether anything changed.
    pub(crate) fn set_caret(&mut self, index: u32) -> bool {
        let index = index.min(char_count(&self.text));
        if self.composing() || (index == self.caret && self.anchor.is_none()) {
            return false;
        }
        self.caret = index;
        self.anchor = None;
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

    /// The selection's anchor in the shown characters: the caret's position when nothing is selected, which is always the case while a composition is active.
    pub(crate) fn shown_anchor(&self) -> u32 {
        self.anchor.unwrap_or_else(|| self.shown_caret())
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
        let anchor_ok = self.anchor.is_none_or(|anchor| {
            anchor != self.caret && anchor <= char_count(&self.text) && self.composition.is_none()
        });
        let composition_ok = self.composition.as_ref().is_none_or(|composition| {
            utf16_len(&composition.text) <= MAX_COMPOSITION_UNITS
                && composition.text == clean(&composition.text)
                && composition.selection_start <= composition.selection_end
                && composition.selection_end <= char_count(&composition.text)
        });
        text_ok && anchor_ok && composition_ok
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
        assert!(line.key("Home", false));
        assert!(line.key("ArrowRight", false));
        assert_eq!(line.insert("e"), Ok(true));
        assert_eq!(shown_string(&line), "Hello");
        assert!(line.key("End", false));
        assert!(line.key("Backspace", false));
        assert!(line.key("Home", false));
        assert!(line.key("Delete", false));
        assert_eq!(shown_string(&line), "ell");
        assert!(!line.key("Home", false));
        assert!(!line.key("Tab", false));
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
        line.key("ArrowLeft", false);
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
        assert!(!line.key("Backspace", false));
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

    /// A line holding `text`, with the selection from character `anchor` to character `focus`.
    fn selecting(text: &str, anchor: u32, focus: u32) -> EditableLine {
        let mut line = EditableLine::default();
        line.insert(text).unwrap();
        line.caret = focus;
        line.anchor = (anchor != focus).then_some(anchor);
        line
    }

    /// The shown text, and the anchor and the caret in its characters.
    fn state(line: &EditableLine) -> (String, u32, u32) {
        (line.shown_text(), line.shown_anchor(), line.shown_caret())
    }

    #[test]
    fn shift_with_the_arrows_home_and_end_extends_the_selection() {
        let mut line = selecting("hello", 2, 2);
        assert!(line.key("ArrowRight", true));
        assert!(line.key("ArrowRight", true));
        assert_eq!(state(&line), ("hello".to_owned(), 2, 4));
        assert_eq!(line.selected_text(), "ll");
        // Back past the anchor: the selection turns around.
        assert!(line.key("ArrowLeft", true));
        assert!(line.key("ArrowLeft", true));
        assert!(line.key("ArrowLeft", true));
        assert_eq!(state(&line), ("hello".to_owned(), 2, 1));
        assert_eq!(line.selected_text(), "e");
        assert!(line.key("Home", true));
        assert_eq!(state(&line), ("hello".to_owned(), 2, 0));
        assert!(line.key("End", true));
        assert_eq!(state(&line), ("hello".to_owned(), 2, 5));
        // Nothing to extend at the end.
        assert!(!line.key("ArrowRight", true));
        // Shrinking back to the anchor leaves nothing selected.
        let mut line = selecting("hello", 2, 3);
        assert!(line.key("ArrowLeft", true));
        assert_eq!(line.anchor, None);
    }

    #[test]
    fn arrows_without_shift_collapse_the_selection_to_the_end_they_point_to() {
        let mut line = selecting("hello", 1, 4);
        assert!(line.key("ArrowLeft", false));
        assert_eq!(state(&line), ("hello".to_owned(), 1, 1));
        let mut line = selecting("hello", 4, 1);
        assert!(line.key("ArrowRight", false));
        assert_eq!(state(&line), ("hello".to_owned(), 4, 4));
        let mut line = selecting("hello", 1, 4);
        assert!(line.key("Home", false));
        assert_eq!(state(&line), ("hello".to_owned(), 0, 0));
    }

    #[test]
    fn typing_backspace_and_delete_replace_the_selection() {
        let mut line = selecting("hello", 1, 4);
        assert_eq!(line.insert("ipp"), Ok(true));
        assert_eq!(state(&line), ("hippo".to_owned(), 4, 4));
        let mut line = selecting("hello", 4, 1);
        assert!(line.key("Backspace", false));
        assert_eq!(state(&line), ("ho".to_owned(), 1, 1));
        let mut line = selecting("hello", 0, 5);
        assert!(line.key("Delete", true));
        assert_eq!(state(&line), (String::new(), 0, 0));
        // Text that is empty once cleaned keeps the selection.
        let mut line = selecting("hello", 0, 5);
        assert_eq!(line.insert("\n"), Ok(false));
        assert_eq!(line.selected_text(), "hello");
        // The limit counts the selected text as gone.
        let mut line = selecting(&"a".repeat(MAX_EDITABLE_TEXT_UNITS), 0, 3);
        assert_eq!(line.insert("bcd"), Ok(true));
        assert_eq!(line.insert("e"), Err(TooLong));
    }

    #[test]
    fn replaces_a_range_of_the_shown_text_and_refuses_ranges_it_cannot_take() {
        // "a😀b": the emoji is two UTF-16 code units, 1 to 3.
        let mut line = selecting("a😀b", 4, 4);
        assert_eq!(line.replace(1, 3, "x"), Ok(true));
        assert_eq!(state(&line), ("axb".to_owned(), 2, 2));
        let mut line = selecting("a😀b", 0, 0);
        for (start, end) in [(2, 3), (1, 2), (0, 5), (3, 1)] {
            assert_eq!(line.replace(start, end, "x"), Err(EditError::Invalid));
        }
        // An insertion is a range with equal ends; the caret ends after the new text, at character 4 (UTF-16 offset 5).
        assert_eq!(line.replace(4, 4, "c"), Ok(true));
        assert_eq!(state(&line), ("a😀bc".to_owned(), 4, 4));
        // A replacement that changes nothing reports no change.
        assert_eq!(line.replace(4, 5, "c"), Ok(false));
        let mut long = selecting(&"a".repeat(MAX_EDITABLE_TEXT_UNITS), 0, 0);
        assert_eq!(long.replace(0, 1, "bc"), Err(EditError::TooLong));
        assert_eq!(long.replace(0, 2, "bc"), Ok(true));
    }

    #[test]
    fn a_replacement_ends_a_composition_and_discards_what_of_it_lies_outside_the_range() {
        let mut line = selecting("abc", 2, 2);
        line.update_composition("xy", None).unwrap();
        assert_eq!(line.shown_text(), "abxyc");
        // The range covers the composition exactly: the text takes its place.
        let mut committed = line.clone();
        assert_eq!(committed.replace(2, 4, "XY"), Ok(true));
        assert_eq!(state(&committed), ("abXYc".to_owned(), 4, 4));
        // The range lies before the composition, which is discarded.
        let mut before = line.clone();
        assert_eq!(before.replace(0, 1, "A"), Ok(true));
        assert_eq!(state(&before), ("Abc".to_owned(), 1, 1));
        // The range takes part of the composition: the rest goes too.
        assert_eq!(line.replace(1, 3, "Z"), Ok(true));
        assert_eq!(state(&line), ("aZc".to_owned(), 2, 2));
        assert!(!line.composing());
    }

    #[test]
    fn selects_by_utf16_offsets_of_the_shown_text_except_while_composing() {
        let mut line = selecting("a😀b", 0, 0);
        assert_eq!(line.select(4, 1), Ok(true));
        assert_eq!(state(&line), ("a😀b".to_owned(), 3, 1));
        assert_eq!(line.selected_text(), "😀b");
        assert_eq!(line.select(4, 1), Ok(false));
        assert_eq!(line.select(2, 2), Err(EditError::Invalid));
        assert_eq!(line.select(0, 5), Err(EditError::Invalid));
        // A caret is a selection with equal ends.
        assert_eq!(line.select(1, 1), Ok(true));
        assert_eq!(line.anchor, None);
        // While composing, the input method owns the caret: a valid selection changes nothing, an invalid one is still refused.
        line.update_composition("にほ", None).unwrap();
        assert_eq!(line.select(0, 0), Ok(false));
        assert_eq!(line.select(9, 0), Err(EditError::Invalid));
        assert_eq!(line.shown_caret(), 3);
    }

    #[test]
    fn a_composition_replaces_the_selection_and_a_click_collapses_it() {
        let mut line = selecting("hello", 1, 4);
        assert!(line.start_composition());
        assert_eq!(state(&line), ("ho".to_owned(), 1, 1));
        // The composition is still empty, so ending it changes nothing shown; the selected text stays deleted.
        assert!(!line.end_composition());
        assert_eq!(line.shown_text(), "ho");
        let mut line = selecting("hello", 1, 4);
        assert_eq!(line.update_composition("か", None), Ok(true));
        assert_eq!(line.shown_text(), "hかo");
        let mut line = selecting("hello", 1, 4);
        assert!(line.set_caret(4));
        assert_eq!(state(&line), ("hello".to_owned(), 4, 4));
        assert!(!line.set_caret(4));
    }

    #[test]
    fn decodes_clipboard_text_as_utf8_lossily_within_the_limit() {
        // Invalid bytes become U+FFFD as String::from_utf8_lossy makes them; control characters go as for typed text.
        let bytes = b"a\xffb\r\n\xf0\x9f\x98\x80\xe2\x82";
        assert_eq!(
            clipboard_text(bytes),
            Ok(clean(&String::from_utf8_lossy(bytes)))
        );
        assert_eq!(clipboard_text(bytes), Ok("a\u{FFFD}b😀\u{FFFD}".to_owned()));
        // Characters the line cannot hold do not count towards the limit; the rest stops at it.
        let mut fits = vec![b'\n'; 4 * MAX_EDITABLE_TEXT_UNITS];
        fits.extend(std::iter::repeat_n(b'x', MAX_EDITABLE_TEXT_UNITS));
        assert_eq!(
            clipboard_text(&fits).map(|text| text.len()),
            Ok(MAX_EDITABLE_TEXT_UNITS)
        );
        fits.push(b'y');
        assert_eq!(clipboard_text(&fits), Err(TooLong));
        assert_eq!(clipboard_text(&vec![0xff; 64 << 10]), Err(TooLong));
    }

    #[test]
    fn copies_and_cuts_the_selection() {
        let mut line = selecting("hello", 4, 1);
        assert_eq!(line.selected_text(), "ell");
        assert!(line.cut());
        assert_eq!(state(&line), ("ho".to_owned(), 1, 1));
        assert_eq!(line.selected_text(), "");
        assert!(!line.cut());
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
            anchor: None,
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
        // An anchor is valid only apart from the caret, inside the text, and without a composition.
        let selected = EditableLine {
            anchor: Some(0),
            ..valid.clone()
        };
        assert!(selected.is_valid());
        for anchor in [2, 3] {
            let wrong = EditableLine {
                anchor: Some(anchor),
                ..valid.clone()
            };
            assert!(!wrong.is_valid(), "{anchor}");
        }
        let composing = EditableLine {
            composition: Some(Composition::default()),
            ..selected
        };
        assert!(!composing.is_valid());
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
