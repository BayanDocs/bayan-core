//! The stub UI manifest that `ui.manifest` returns in v0 ([engine protocol specification][spec] §6.8), so the shells can try rendering a ribbon from data. The real manifest of ADR-0019 (bayan-ui, Phase 2) replaces it.
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use crate::protocol::{UiCommand, UiGroup, UiManifest, UiRibbon, UiTab};

fn command(id: &str, label: &str, shortcut: &str) -> UiCommand {
    UiCommand {
        id: id.to_owned(),
        label: label.to_owned(),
        shortcut: Some(shortcut.to_owned()),
    }
}

fn group(id: &str, label: &str, commands: &[&str]) -> UiGroup {
    UiGroup {
        id: id.to_owned(),
        label: label.to_owned(),
        commands: commands.iter().map(|&command| command.to_owned()).collect(),
    }
}

/// The stub manifest, in English whatever locale is asked for.
pub(crate) fn stub() -> UiManifest {
    UiManifest {
        manifest_version: 0,
        locale: "en-US".to_owned(),
        commands: vec![
            command("file.open", "Open", "Ctrl+O"),
            command("file.save", "Save", "Ctrl+S"),
            command("edit.undo", "Undo", "Ctrl+Z"),
            command("edit.redo", "Redo", "Ctrl+Y"),
            command("format.bold.toggle", "Bold", "Ctrl+B"),
            command("format.italic.toggle", "Italic", "Ctrl+I"),
        ],
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
        let manifest = stub();
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
