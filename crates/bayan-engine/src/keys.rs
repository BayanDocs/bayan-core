//! Which key presses reach the engine ([engine protocol specification][spec] §14.3).
//!
//! The UI manifest publishes the keys the engine handles as data resolved for the session's platform and runtime, so that a shell can decide at once, without asking the engine, whether a press goes to the engine as `input.key` or stays with the platform:
//!
//! - **bindings** of the stub manifest's commands, which follow Microsoft's current public list of Word's shortcuts: Word for Windows on every platform but macOS and iOS, where the manifest's "Ctrl" is Command (the DOM's `meta`) as in Word for Mac. Each binding gives all four modifiers and a label as the platform writes it ("Ctrl+B", "⌘B").
//! - **document keys**, the twelve keys that the engine handles in the document area with any modifiers.
//! - **reserved keys**, the presses that the shell keeps for itself, the operating system or the browser even where a binding or a document key matches them: the table of spec §14.3, row by row in [`reserved`].
//!
//! A press goes to the engine if and only if it matches a binding of a command without `shell_action` or a document key, matches no reserved key, and no input method is composing with it (spec §14.3 item 2). [`meaning`] applies that rule on the engine's side, with the same data and the same matching rule ([`matches`]), so the engine runs a binding's command for exactly the presses that a shell sends. The rule needs no Unicode data about scripts: ASCII letters are compared without regard to case and every other character exactly, with a fall-back to the key's position for presses that type no ASCII letter or digit.
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use crate::protocol::{DocumentKey, KeyBinding, Platform, ReservedKey, Runtime, UiKeys};

/// The `shell_action` of the clipboard's commands (spec §14.3, §14.5).
pub const SHELL_ACTION_COPY: &str = "copy";
/// The `shell_action` of `edit.cut`.
pub const SHELL_ACTION_CUT: &str = "cut";
/// The `shell_action` of `edit.paste`.
pub const SHELL_ACTION_PASTE: &str = "paste";

/// The keys that the engine handles in the document area with any modifiers (spec §14.3): in v1, the twelve editing and navigation keys.
pub const DOCUMENT_KEYS: [&str; 12] = [
    "Backspace",
    "Delete",
    "Enter",
    "Escape",
    "ArrowLeft",
    "ArrowRight",
    "ArrowUp",
    "ArrowDown",
    "Home",
    "End",
    "PageUp",
    "PageDown",
];

/// A command of the stub manifest with a key binding: Mod (Ctrl, or Command on macOS and iOS) with a letter, as Word binds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StubCommand {
    /// The command's identifier.
    pub id: &'static str,
    /// Its label.
    pub label: &'static str,
    /// The letter that Mod goes with, lower case.
    pub key: &'static str,
    /// The shell's own action for the command, if the shell carries it out itself.
    pub shell_action: Option<&'static str>,
}

/// The stub manifest's commands, in the manifest's order, with their bindings as Word for Windows and Word for Mac have them (Mod+O, Mod+S, Mod+Z, Mod+Y, Mod+B, Mod+I, Mod+C, Mod+X and Mod+V).
pub const STUB_COMMANDS: [StubCommand; 9] = [
    StubCommand {
        id: "file.open",
        label: "Open",
        key: "o",
        shell_action: None,
    },
    StubCommand {
        id: "file.save",
        label: "Save",
        key: "s",
        shell_action: None,
    },
    StubCommand {
        id: "edit.undo",
        label: "Undo",
        key: "z",
        shell_action: None,
    },
    StubCommand {
        id: "edit.redo",
        label: "Redo",
        key: "y",
        shell_action: None,
    },
    StubCommand {
        id: "edit.cut",
        label: "Cut",
        key: "x",
        shell_action: Some(SHELL_ACTION_CUT),
    },
    StubCommand {
        id: "edit.copy",
        label: "Copy",
        key: "c",
        shell_action: Some(SHELL_ACTION_COPY),
    },
    StubCommand {
        id: "edit.paste",
        label: "Paste",
        key: "v",
        shell_action: Some(SHELL_ACTION_PASTE),
    },
    StubCommand {
        id: "format.bold.toggle",
        label: "Bold",
        key: "b",
        shell_action: None,
    },
    StubCommand {
        id: "format.italic.toggle",
        label: "Italic",
        key: "i",
        shell_action: None,
    },
];

/// The four modifiers of a press, or of an entry where `None` means "either state".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Mods {
    ctrl: Option<bool>,
    shift: Option<bool>,
    alt: Option<bool>,
    meta: Option<bool>,
}

impl Mods {
    /// Exactly these modifiers held and no other.
    const fn exactly(ctrl: bool, shift: bool, alt: bool, meta: bool) -> Self {
        Self {
            ctrl: Some(ctrl),
            shift: Some(shift),
            alt: Some(alt),
            meta: Some(meta),
        }
    }
}

/// Whether the platform's primary shortcut modifier, Mod, is Command (the DOM's `meta`) rather than Ctrl: on macOS and iOS (ADR-0014's amendment of 2026-10-12).
#[must_use]
pub const fn mod_is_meta(platform: Platform) -> bool {
    matches!(platform, Platform::Macos | Platform::Ios)
}

/// Whether the platform follows Windows' conventions for Alt, which types nothing and opens the menu and its KeyTips: Windows and Linux (spec §14.3).
const fn alt_opens_menus(platform: Platform) -> bool {
    matches!(platform, Platform::Windows | Platform::Linux)
}

/// Mod with a key, and Shift as given, without Alt and without the other of Ctrl and Command.
const fn with_mod(platform: Platform, shift: bool) -> Mods {
    let meta = mod_is_meta(platform);
    Mods::exactly(!meta, shift, false, meta)
}

/// A binding's label as the platform writes it: "⌘B" on macOS and iOS, "Ctrl+B" elsewhere.
fn label(platform: Platform, key: &str) -> String {
    let key = key.to_ascii_uppercase();
    if mod_is_meta(platform) {
        format!("\u{2318}{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// The binding of a stub command, resolved for a platform. The runtime does not change it: no stub binding is among the browser's reserved keys.
#[must_use]
pub fn binding(command: &StubCommand, platform: Platform) -> KeyBinding {
    let meta = mod_is_meta(platform);
    KeyBinding {
        key: Some(command.key.to_owned()),
        code: None,
        ctrl: !meta,
        shift: false,
        alt: false,
        meta,
        label: label(platform, command.key),
    }
}

/// A reserved entry for a key named by `key`.
fn by_key(key: &str, mods: Mods) -> ReservedKey {
    ReservedKey {
        key: Some(key.to_owned()),
        code: None,
        ctrl: mods.ctrl,
        shift: mods.shift,
        alt: mods.alt,
        meta: mods.meta,
    }
}

/// A reserved entry for a physical key named by `code`.
fn by_code(code: String, mods: Mods) -> ReservedKey {
    ReservedKey {
        key: None,
        code: Some(code),
        ctrl: mods.ctrl,
        shift: mods.shift,
        alt: mods.alt,
        meta: mods.meta,
    }
}

/// The ASCII letters and digits, in the order of their codes `KeyA` to `KeyZ` and `Digit0` to `Digit9`.
fn letters_and_digits() -> impl Iterator<Item = char> {
    ('a'..='z').chain('0'..='9')
}

/// The DOM `code` of the key that types an ASCII letter or digit on a US layout, such as `KeyA` or `Digit0`.
fn code_of(character: char) -> String {
    if character.is_ascii_digit() {
        format!("Digit{character}")
    } else {
        format!("Key{}", character.to_ascii_uppercase())
    }
}

/// The reserved keys of a platform and runtime: the table of spec §14.3, row by row, one entry per key, each with only the modifiers that matter.
#[must_use]
pub fn reserved(platform: Platform, runtime: Runtime) -> Vec<ReservedKey> {
    let windows_or_linux = alt_opens_menus(platform);
    let none_but_shift_either = Mods {
        ctrl: Some(false),
        shift: None,
        alt: Some(false),
        meta: Some(false),
    };
    let none = Mods::exactly(false, false, false, false);
    let ctrl = Mods::exactly(true, false, false, false);
    let shift = Mods::exactly(false, true, false, false);
    let alt = Mods::exactly(false, false, true, false);
    let mut keys = vec![
        // Every shell: Tab and Shift+Tab, so that the keyboard never gets stuck in the document (ADR-0020 decision 5).
        by_key("Tab", none_but_shift_either),
        // Every shell: the shell's own interface. F6, Shift+F6, F10, Shift+F10 and ContextMenu; on Windows and Linux also Alt pressed alone, and Alt with a letter or digit and no other modifier (KeyTips, ADR-0019 decision 3).
        by_key("F6", none_but_shift_either),
        by_key("F10", none_but_shift_either),
        by_key("ContextMenu", Mods::default()),
    ];
    if windows_or_linux {
        keys.push(by_key(
            "Alt",
            Mods {
                ctrl: Some(false),
                shift: Some(false),
                alt: None,
                meta: Some(false),
            },
        ));
        keys.extend(letters_and_digits().map(|character| by_code(code_of(character), alt)));
    }

    // Every shell: the platform's clipboard actions, which the shell carries out (spec §14.5). Mod+C, Mod+X and Mod+V; on Windows and Linux also Ctrl+Insert, Shift+Insert and Shift+Delete.
    for key in ["c", "x", "v"] {
        keys.push(by_key(key, with_mod(platform, false)));
    }
    if windows_or_linux {
        keys.push(by_key("Insert", ctrl));
        keys.push(by_key("Insert", shift));
        keys.push(by_key("Delete", shift));
    }

    // Every shell: the operating system. On macOS, Command+H, Command+M and Command+Q.
    if platform == Platform::Macos {
        for key in ["h", "m", "q"] {
            keys.push(by_key(key, Mods::exactly(false, false, false, true)));
        }
    }

    if runtime == Runtime::Browser {
        // The browser's page zoom, on which people with low vision rely: Mod with `=`, `+`, `-` or `0`, without Shift or Alt.
        for key in ["=", "+", "-", "0"] {
            keys.push(by_key(key, with_mod(platform, false)));
        }

        // Reloading, caret browsing, full screen, the developer tools and the address bar: F5, F7, F11 and F12; Ctrl+Shift+I, J and C on Windows and Linux; Command+Alt+I, J and C on macOS; Alt+D on Windows and Linux.
        for key in ["F5", "F7", "F11", "F12"] {
            keys.push(by_key(key, none));
        }
        if windows_or_linux {
            for key in ["i", "j", "c"] {
                keys.push(by_key(key, Mods::exactly(true, true, false, false)));
            }
        }
        if platform == Platform::Macos {
            for key in ["i", "j", "c"] {
                keys.push(by_key(key, Mods::exactly(false, false, true, true)));
            }
        }
        if windows_or_linux {
            keys.push(by_key("d", alt));
        }

        // Windows, tabs and history: Mod+N, Mod+T, Mod+W, Mod+Shift+N, Mod+Shift+T, Mod+Shift+W, Mod+1 to Mod+9, Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+PageUp and Ctrl+PageDown; Alt+ArrowLeft and Alt+ArrowRight on Windows and Linux.
        for shifted in [false, true] {
            for key in ["n", "t", "w"] {
                keys.push(by_key(key, with_mod(platform, shifted)));
            }
        }
        for digit in ["1", "2", "3", "4", "5", "6", "7", "8", "9"] {
            keys.push(by_key(digit, with_mod(platform, false)));
        }
        keys.push(by_key("Tab", ctrl));
        keys.push(by_key("Tab", Mods::exactly(true, true, false, false)));
        keys.push(by_key("PageUp", ctrl));
        keys.push(by_key("PageDown", ctrl));
        if windows_or_linux {
            keys.push(by_key("ArrowLeft", alt));
            keys.push(by_key("ArrowRight", alt));
        }
    }
    keys
}

/// The manifest's key data for a platform and runtime: the document keys and the reserved keys.
#[must_use]
pub fn ui_keys(platform: Platform, runtime: Runtime) -> UiKeys {
    UiKeys {
        document: DOCUMENT_KEYS
            .iter()
            .map(|&key| DocumentKey {
                key: key.to_owned(),
            })
            .collect(),
        reserved: reserved(platform, runtime),
    }
}

/// A key press, as `input.key` carries it (spec §6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Press<'a> {
    /// The key's meaning, as the DOM's `KeyboardEvent.key`.
    pub key: &'a str,
    /// The physical key, as the DOM's `KeyboardEvent.code`; empty if the shell does not know it.
    pub code: &'a str,
    /// Control.
    pub ctrl: bool,
    /// Shift.
    pub shift: bool,
    /// Alt.
    pub alt: bool,
    /// Meta.
    pub meta: bool,
}

/// Whether a key name is a single ASCII letter or digit.
fn is_ascii_letter_or_digit(key: &str) -> bool {
    key.len() == 1 && key.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// The ASCII letter (lower case) or digit that a DOM `code` from `KeyA` to `KeyZ` or `Digit0` to `Digit9` names.
fn letter_or_digit_of_code(code: &str) -> Option<char> {
    let named = code
        .strip_prefix("Key")
        .filter(|rest| rest.len() == 1 && rest.bytes().all(|byte| byte.is_ascii_uppercase()))
        .or_else(|| {
            code.strip_prefix("Digit")
                .filter(|rest| rest.len() == 1 && rest.bytes().all(|byte| byte.is_ascii_digit()))
        })?;
    named
        .chars()
        .next()
        .map(|character| character.to_ascii_lowercase())
}

/// The one way to match (spec §14.3 item 3), for bindings, document keys and reserved keys alike: every modifier that the entry gives equals the press's, and its key or code matches the press's. An entry's `key` matches the press's `key` with ASCII letters compared without regard to case and every other character exactly; if the two differ, the entry's key is an ASCII letter or digit, the press's is not, and the press does not hold Ctrl and Alt together (how Windows reports AltGr), the letter or digit that the press's `code` names is compared instead.
fn matches(key: Option<&str>, code: Option<&str>, mods: Mods, press: &Press<'_>) -> bool {
    let modifier_ok = |given: Option<bool>, held: bool| given.is_none_or(|given| given == held);
    if !(modifier_ok(mods.ctrl, press.ctrl)
        && modifier_ok(mods.shift, press.shift)
        && modifier_ok(mods.alt, press.alt)
        && modifier_ok(mods.meta, press.meta))
    {
        return false;
    }
    match (key, code) {
        (_, Some(code)) => press.code == code,
        (Some(key), None) => {
            if key.eq_ignore_ascii_case(press.key) {
                return true;
            }
            let falls_back = is_ascii_letter_or_digit(key)
                && !is_ascii_letter_or_digit(press.key)
                && !(press.ctrl && press.alt);
            falls_back
                && letter_or_digit_of_code(press.code)
                    .is_some_and(|named| key.eq_ignore_ascii_case(named.encode_utf8(&mut [0; 4])))
        }
        (None, None) => false,
    }
}

/// Whether a press matches a binding.
#[must_use]
pub fn matches_binding(binding: &KeyBinding, press: &Press<'_>) -> bool {
    let mods = Mods::exactly(binding.ctrl, binding.shift, binding.alt, binding.meta);
    matches(binding.key.as_deref(), binding.code.as_deref(), mods, press)
}

/// Whether a press matches a reserved key.
#[must_use]
pub fn matches_reserved(reserved: &ReservedKey, press: &Press<'_>) -> bool {
    let mods = Mods {
        ctrl: reserved.ctrl,
        shift: reserved.shift,
        alt: reserved.alt,
        meta: reserved.meta,
    };
    matches(
        reserved.key.as_deref(),
        reserved.code.as_deref(),
        mods,
        press,
    )
}

/// What a press means to the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Meaning {
    /// It runs a command of the manifest.
    Command(&'static str),
    /// It is a document key, named as [`DOCUMENT_KEYS`] names it.
    DocumentKey(&'static str),
    /// Nothing: a shell does not send it, because it matches no binding of a command without `shell_action` and no document key, or matches a reserved key, or an input method is composing with it.
    Nothing,
}

/// What a press means for a session of a platform and runtime (spec §14.3 item 2): the command of the first binding it matches, among commands without `shell_action`; otherwise the document key it matches; and nothing if it matches a reserved key or an input method is composing with it. A shell sends a press as `input.key` exactly when this is not [`Meaning::Nothing`].
#[must_use]
pub fn meaning(
    press: &Press<'_>,
    composing: bool,
    platform: Platform,
    runtime: Runtime,
) -> Meaning {
    if composing
        || reserved(platform, runtime)
            .iter()
            .any(|reserved| matches_reserved(reserved, press))
    {
        return Meaning::Nothing;
    }
    let command = STUB_COMMANDS.iter().find(|command| {
        command.shell_action.is_none() && matches_binding(&binding(command, platform), press)
    });
    if let Some(command) = command {
        return Meaning::Command(command.id);
    }
    DOCUMENT_KEYS
        .iter()
        .find(|&&key| matches(Some(key), None, Mods::default(), press))
        .map_or(Meaning::Nothing, |&key| Meaning::DocumentKey(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press<'a>(
        key: &'a str,
        code: &'a str,
        ctrl: bool,
        shift: bool,
        alt: bool,
        meta: bool,
    ) -> Press<'a> {
        Press {
            key,
            code,
            ctrl,
            shift,
            alt,
            meta,
        }
    }

    #[test]
    fn mod_is_command_only_on_apple_systems() {
        let bold = &STUB_COMMANDS[7];
        assert_eq!(bold.id, "format.bold.toggle");
        for platform in Platform::ALL {
            let binding = binding(bold, platform);
            let apple = matches!(platform, Platform::Macos | Platform::Ios);
            assert_eq!(
                (binding.ctrl, binding.meta),
                (!apple, apple),
                "{platform:?}"
            );
            assert_eq!(binding.label, if apple { "\u{2318}B" } else { "Ctrl+B" });
        }
    }

    #[test]
    fn letters_compare_without_case_and_fall_back_to_the_code() {
        let bold = Press {
            key: "b",
            code: "KeyB",
            ..press("", "", true, false, false, false)
        };
        let windows = |press: &Press<'_>| meaning(press, false, Platform::Windows, Runtime::Native);
        assert_eq!(windows(&bold), Meaning::Command("format.bold.toggle"));
        assert_eq!(
            windows(&Press { key: "B", ..bold }),
            Meaning::Command("format.bold.toggle")
        );
        // Russian: Ctrl with the key in B's position types "и"; the code decides.
        assert_eq!(
            windows(&Press { key: "и", ..bold }),
            Meaning::Command("format.bold.toggle")
        );
        // A letter that the press types itself is compared, not the position: German Ctrl+Z is on KeyY.
        let undo = Press {
            key: "z",
            code: "KeyY",
            ..bold
        };
        assert_eq!(windows(&undo), Meaning::Command("edit.undo"));
        // AltGr (Ctrl and Alt together on Windows) never falls back, and a binding needs exactly its modifiers.
        assert_eq!(
            windows(&Press {
                key: "€",
                code: "KeyE",
                alt: true,
                ..bold
            }),
            Meaning::Nothing
        );
        assert_eq!(
            windows(&Press {
                shift: true,
                ..bold
            }),
            Meaning::Nothing
        );
        // An unknown code does not fall back.
        assert_eq!(
            windows(&Press {
                key: "и",
                code: "",
                ..bold
            }),
            Meaning::Nothing
        );
    }

    #[test]
    fn document_keys_match_with_any_modifiers_unless_reserved() {
        let left = press("ArrowLeft", "ArrowLeft", false, true, false, false);
        assert_eq!(
            meaning(&left, false, Platform::Linux, Runtime::Native),
            Meaning::DocumentKey("ArrowLeft")
        );
        // Alt+ArrowLeft is the browser's "back" on Windows and Linux, but not in a native shell or on macOS.
        let back = press("ArrowLeft", "ArrowLeft", false, false, true, false);
        assert_eq!(
            meaning(&back, false, Platform::Linux, Runtime::Browser),
            Meaning::Nothing
        );
        assert_eq!(
            meaning(&back, false, Platform::Linux, Runtime::Native),
            Meaning::DocumentKey("ArrowLeft")
        );
        assert_eq!(
            meaning(&back, false, Platform::Macos, Runtime::Browser),
            Meaning::DocumentKey("ArrowLeft")
        );
        // Shift+Delete is the clipboard's cut on Windows and Linux.
        let cut = press("Delete", "Delete", false, true, false, false);
        assert_eq!(
            meaning(&cut, false, Platform::Windows, Runtime::Native),
            Meaning::Nothing
        );
        assert_eq!(
            meaning(&cut, false, Platform::Macos, Runtime::Native),
            Meaning::DocumentKey("Delete")
        );
        // While an input method composes, nothing reaches the engine.
        assert_eq!(
            meaning(&left, true, Platform::Linux, Runtime::Native),
            Meaning::Nothing
        );
    }

    #[test]
    fn the_clipboard_commands_are_the_shells() {
        let copy = press("c", "KeyC", false, false, false, true);
        assert_eq!(
            meaning(&copy, false, Platform::Macos, Runtime::Native),
            Meaning::Nothing
        );
        for platform in Platform::ALL {
            for runtime in Runtime::ALL {
                for command in STUB_COMMANDS
                    .iter()
                    .filter(|command| command.shell_action.is_some())
                {
                    let binding = binding(command, platform);
                    let code = code_of(command.key.chars().next().unwrap());
                    let typed = press(command.key, &code, binding.ctrl, false, false, binding.meta);
                    assert!(
                        reserved(platform, runtime)
                            .iter()
                            .any(|reserved| matches_reserved(reserved, &typed)),
                        "{} on {platform:?} {runtime:?}",
                        command.id
                    );
                }
            }
        }
    }

    #[test]
    fn every_reserved_entry_names_a_key_or_a_code() {
        for platform in Platform::ALL {
            for runtime in Runtime::ALL {
                for entry in reserved(platform, runtime) {
                    assert!(entry.key.is_some() != entry.code.is_some(), "{entry:?}");
                }
            }
        }
        // Alt with a letter or digit is 36 entries on Windows and Linux.
        let alt_codes = |platform| {
            reserved(platform, Runtime::Native)
                .iter()
                .filter(|entry| entry.code.is_some())
                .count()
        };
        assert_eq!(alt_codes(Platform::Windows), 36);
        assert_eq!(alt_codes(Platform::Macos), 0);
    }

    #[test]
    fn codes_name_letters_and_digits_only() {
        assert_eq!(letter_or_digit_of_code("KeyQ"), Some('q'));
        assert_eq!(letter_or_digit_of_code("Digit7"), Some('7'));
        for other in [
            "Key",
            "Keyq",
            "KeyAB",
            "Digit",
            "Digit10",
            "Numpad1",
            "Semicolon",
            "",
        ] {
            assert_eq!(letter_or_digit_of_code(other), None, "{other}");
        }
    }
}
