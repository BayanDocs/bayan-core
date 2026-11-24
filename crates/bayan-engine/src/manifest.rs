//! The stub UI manifest that `ui.manifest` returns ([engine protocol specification][spec] §6.8, §14.3), so the shells can try rendering a ribbon from data and know which key presses reach the engine. The real manifest of ADR-0019 (bayan-ui, Phase 2) replaces it.
//!
//! Its commands' bindings, the document keys and the reserved keys are resolved for the session's platform and runtime by [`crate::keys`].
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use crate::keys::{self, STUB_COMMANDS};
use crate::protocol::{Platform, Runtime, UiCommand, UiGroup, UiManifest, UiRibbon, UiTab};

fn group(id: &str, label: &str, commands: &[&str]) -> UiGroup {
    UiGroup {
        id: id.to_owned(),
        label: label.to_owned(),
        commands: commands.iter().map(|&command| command.to_owned()).collect(),
    }
}

/// The stub manifest for a session's platform and runtime, in English whatever locale is asked for.
pub(crate) fn stub(platform: Platform, runtime: Runtime) -> UiManifest {
    UiManifest {
        manifest_version: 0,
        locale: "en-US".to_owned(),
        commands: STUB_COMMANDS
            .iter()
            .map(|command| UiCommand {
                id: command.id.to_owned(),
                label: command.label.to_owned(),
                bindings: vec![keys::binding(command, platform)],
                shell_action: command.shell_action.map(str::to_owned),
            })
            .collect(),
        keys: keys::ui_keys(platform, runtime),
        ribbon: UiRibbon {
            tabs: vec![
                UiTab {
                    id: "file".to_owned(),
                    label: "File".to_owned(),
                    groups: vec![group("document", "Document", &["file.open", "file.save"])],
                },
                UiTab {
                    id: "home".to_owned(),
                    label: "Home".to_owned(),
                    groups: vec![
                        group("history", "Undo", &["edit.undo", "edit.redo"]),
                        group(
                            "clipboard",
                            "Clipboard",
                            &["edit.paste", "edit.cut", "edit.copy"],
                        ),
                        group(
                            "font",
                            "Font",
                            &["format.bold.toggle", "format.italic.toggle"],
                        ),
                    ],
                },
            ],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ribbon_command_is_in_the_command_list() {
        for platform in Platform::ALL {
            for runtime in Runtime::ALL {
                let manifest = stub(platform, runtime);
                for tab in &manifest.ribbon.tabs {
                    for group in &tab.groups {
                        for id in &group.commands {
                            assert!(
                                manifest.commands.iter().any(|command| &command.id == id),
                                "{id}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn only_the_clipboard_commands_are_carried_out_by_the_shell() {
        let manifest = stub(Platform::Windows, Runtime::Native);
        let actions: Vec<(&str, &str)> = manifest
            .commands
            .iter()
            .filter_map(|command| Some((command.id.as_str(), command.shell_action.as_deref()?)))
            .collect();
        assert_eq!(
            actions,
            [
                ("edit.cut", "cut"),
                ("edit.copy", "copy"),
                ("edit.paste", "paste")
            ]
        );
    }
}
