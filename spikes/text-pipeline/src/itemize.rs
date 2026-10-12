//! Itemization: cutting a paragraph into runs that each have one bidi embedding level, one script and one font, the units that are shaped.
//!
//! - **Bidi levels** come from the Unicode Bidirectional Algorithm (unicode-bidi with ICU4X's Bidi_Class data; ADR-0009 §3), for the paragraph direction the document gives.
//! - **Scripts** come from ICU4X's Script property. Characters of the Common and Inherited scripts (spaces, digits, punctuation, combining marks) take the script of the text around them: a combining mark always takes its base's; another character takes the script of the nearest letter before it, if its Script_Extensions allow that script or name none, otherwise the first script its Script_Extensions name; characters before the first letter take the first letter's script, and a text without letters is Latin.
//! - **Fonts** are chosen per grapheme cluster, so that a base and its marks always share a font: the first font of the paragraph's list that has a glyph for every character of the cluster except default-ignorable ones (which the shaper hides), or the first font if none has.

use icu_properties::CodePointSetData;
use icu_properties::props::{DefaultIgnorableCodePoint, Script};
use icu_properties::script::ScriptWithExtensions;
use unicode_bidi::Level;

/// A run: a stretch of text with one bidi level, one script and one font.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    /// The byte offset of the run's first character in the paragraph.
    pub start: usize,
    /// The byte offset after the run's last character.
    pub end: usize,
    /// The bidi embedding level: even is left to right, odd right to left.
    pub level: u8,
    /// The script, after Common and Inherited characters were resolved.
    pub script: Script,
    /// The index of the font in the paragraph's font list.
    pub font: usize,
}

/// The resolved script of every character of `text`, in order.
#[must_use]
pub fn resolve_scripts(text: &str) -> Vec<Script> {
    let data = ScriptWithExtensions::new();
    let mut resolved: Vec<Option<Script>> = Vec::new();
    let mut last_letter: Option<Script> = None;
    for c in text.chars() {
        let script = data.get_script_val(c);
        let choice = if script == Script::Inherited {
            // A combining mark belongs to its base; at the very start of the text it waits like a leading Common character.
            resolved.last().copied().flatten()
        } else if matches!(script, Script::Common | Script::Unknown) {
            let mut extensions = data
                .get_script_extensions_val(c)
                .iter()
                .filter(|&s| !matches!(s, Script::Common | Script::Inherited | Script::Unknown));
            let first = extensions.next();
            match last_letter {
                Some(last)
                    if first.is_none() || first == Some(last) || extensions.any(|s| s == last) =>
                {
                    Some(last)
                }
                _ => first,
            }
        } else {
            last_letter = Some(script);
            Some(script)
        };
        resolved.push(choice);
    }
    // What is still open (only characters before the first letter can be) takes the script of the next resolved character.
    let mut next: Option<Script> = None;
    for slot in resolved.iter_mut().rev() {
        match slot {
            Some(script) => next = Some(*script),
            None => *slot = next,
        }
    }
    resolved
        .into_iter()
        .map(|script| script.unwrap_or(Script::Latin))
        .collect()
}

/// Cuts `text` into runs, given the bidi level of each of its bytes (as unicode-bidi computes them) and the fonts the paragraph may use: `covers(font, character)` says whether font number `font` (of `font_count`) has a glyph for the character.
///
/// # Panics
///
/// If `levels` does not hold one level per byte of `text`, or `font_count` is zero.
#[must_use]
pub fn itemize(
    text: &str,
    levels: &[Level],
    font_count: usize,
    covers: &dyn Fn(usize, char) -> bool,
) -> Vec<Run> {
    assert_eq!(levels.len(), text.len(), "one bidi level per byte");
    assert!(font_count > 0, "a paragraph needs a font");
    let scripts = resolve_scripts(text);
    let ignorable = CodePointSetData::new::<DefaultIgnorableCodePoint>();
    let mut runs: Vec<Run> = Vec::new();
    let mut script_of = scripts.into_iter();
    for pair in crate::segment::grapheme_boundaries(text).windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let cluster = &text[start..end];
        let font = (0..font_count)
            .find(|&font| {
                cluster
                    .chars()
                    .all(|c| ignorable.contains(c) || covers(font, c))
            })
            .unwrap_or(0);
        for (offset, c) in cluster.char_indices() {
            let at = start + offset;
            let level = levels[at].number();
            let script = script_of.next().unwrap_or(Script::Latin);
            match runs.last_mut() {
                Some(run)
                    if run.end == at
                        && run.level == level
                        && run.script == script
                        && run.font == font =>
                {
                    run.end = at + c.len_utf8();
                }
                _ => runs.push(Run {
                    start: at,
                    end: at + c.len_utf8(),
                    level,
                    script,
                    font,
                }),
            }
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use icu_properties::CodePointMapData;
    use icu_properties::props::BidiClass;
    use unicode_bidi::BidiInfo;

    fn levels(text: &str, rtl: bool) -> Vec<Level> {
        let data = CodePointMapData::<BidiClass>::new();
        let level = if rtl { Level::rtl() } else { Level::ltr() };
        BidiInfo::new_with_data_source(&data, text, Some(level)).levels
    }

    fn run(start: usize, end: usize, level: u8, script: Script, font: usize) -> Run {
        Run {
            start,
            end,
            level,
            script,
            font,
        }
    }

    #[test]
    fn a_latin_line_is_one_run() {
        let text = "Plain text, 2026.";
        let runs = itemize(text, &levels(text, false), 1, &|_, _| true);
        assert_eq!(runs, [run(0, text.len(), 0, Script::Latin, 0)]);
    }

    #[test]
    fn splits_hebrew_numbers_and_latin_by_level_script_and_font() {
        // Font 0 has Hebrew, digits and spaces; font 1 has ASCII.
        let covers = |font: usize, c: char| match font {
            0 => ('\u{0590}'..='\u{05ff}').contains(&c) || c.is_ascii_digit() || c == ' ',
            _ => c.is_ascii(),
        };
        let text = "שלום 123 abc";
        let runs = itemize(text, &levels(text, true), 2, &covers);
        assert_eq!(
            runs,
            [
                run(0, 9, 1, Script::Hebrew, 0),
                run(9, 12, 2, Script::Hebrew, 0),
                run(12, 13, 1, Script::Hebrew, 0),
                run(13, 16, 2, Script::Latin, 1),
            ]
        );
    }

    #[test]
    fn characters_before_the_first_letter_take_its_script() {
        assert_eq!(resolve_scripts("«مرحبا»"), [Script::Arabic; 7]);
    }

    #[test]
    fn a_combining_mark_takes_its_base_script() {
        // U+0301 COMBINING ACUTE ACCENT is Inherited.
        assert_eq!(
            resolve_scripts("\u{3b1}\u{301}a\u{301}"),
            [Script::Greek, Script::Greek, Script::Latin, Script::Latin]
        );
    }

    #[test]
    fn a_text_without_letters_is_latin() {
        assert_eq!(resolve_scripts("123 ."), [Script::Latin; 5]);
    }

    #[test]
    fn uses_script_extensions_to_choose_between_neighbours() {
        // U+30FC KATAKANA-HIRAGANA PROLONGED SOUND MARK is Common, with the extensions Hiragana and Katakana: after katakana it is Katakana, after hiragana Hiragana.
        assert_eq!(resolve_scripts("カー"), [Script::Katakana; 2]);
        assert_eq!(resolve_scripts("ひー"), [Script::Hiragana; 2]);
        // After a Han ideograph it takes one of its own extensions, never Han.
        let scripts = resolve_scripts("漢ー");
        assert_eq!(scripts[0], Script::Han);
        assert!(
            matches!(scripts[1], Script::Hiragana | Script::Katakana),
            "{scripts:?}"
        );
        // A full stop, Common without extensions, takes the script before it.
        assert_eq!(resolve_scripts("ひ。"), [Script::Hiragana; 2]);
    }

    #[test]
    fn keeps_a_cluster_in_one_font() {
        // Font 0 has `e` but not the combining accent; font 1 has both, so the whole cluster goes to font 1.
        let covers = |font: usize, c: char| font == 1 || c == 'e';
        let text = "e\u{301}e";
        let runs = itemize(text, &levels(text, false), 2, &covers);
        assert_eq!(
            runs,
            [
                run(0, 3, 0, Script::Latin, 1),
                run(3, 4, 0, Script::Latin, 0)
            ]
        );
    }

    #[test]
    fn falls_back_to_the_first_font_when_no_font_has_a_character() {
        let text = "a\u{2603}";
        let runs = itemize(text, &levels(text, false), 2, &|_, c: char| c == 'a');
        assert_eq!(runs, [run(0, text.len(), 0, Script::Latin, 0)]);
    }

    #[test]
    fn ignores_default_ignorable_characters_when_choosing_a_font() {
        // A zero-width joiner does not send `a` to another font.
        let text = "a\u{200d}a";
        let runs = itemize(text, &levels(text, false), 2, &|font: usize, c: char| {
            font == 1 || c == 'a'
        });
        assert_eq!(runs, [run(0, text.len(), 0, Script::Latin, 0)]);
    }
}
