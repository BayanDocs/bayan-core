//! `bayan-cli protocol schema` and `bayan-cli protocol typescript` write exactly the files that bayan-engine's tests keep current in `crates/bayan-engine/protocol/`, which the artifacts of CORE-007 ship.

#![cfg(test)]

use std::path::Path;
use std::process::{Command, Output};

fn bayan_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bayan-cli"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn writes_the_committed_protocol_files() {
    let protocol = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("bayan-engine")
        .join("protocol");
    for (kind, file) in [
        ("schema", "engine-protocol.schema.json"),
        ("typescript", "engine-protocol.d.ts"),
    ] {
        let output = bayan_cli(&["protocol", kind]);
        assert!(output.status.success(), "protocol {kind} failed");
        let committed = std::fs::read_to_string(protocol.join(file))
            .unwrap()
            .replace("\r\n", "\n");
        assert!(
            String::from_utf8(output.stdout).unwrap() == committed,
            "`bayan-cli protocol {kind}` differs from {file}"
        );
    }
}

#[test]
fn refuses_unknown_commands() {
    let output = bayan_cli(&["protocol", "yaml"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}
