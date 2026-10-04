//! Starting the programs that the verification gate runs.

use std::path::Path;
use std::process::{Command, Output, Stdio};

/// A command that runs the same Cargo as the one running xtask (`cargo run` sets `CARGO`), in the workspace root.
pub fn cargo(root: &Path) -> Command {
    let program = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(program);
    command.current_dir(root);
    command
}

/// Runs `command` with its output going straight to the terminal, after printing it so it can be re-run by hand.
pub fn run(command: &mut Command) -> Result<(), String> {
    let shown = display(command);
    println!("    $ {shown}");
    let status = command
        .status()
        .map_err(|error| format!("could not start `{shown}`: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{shown}` failed ({status})"))
    }
}

/// Runs `command` and returns its exit status and captured output.
pub fn capture(command: &mut Command) -> Result<Output, String> {
    command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not start `{}`: {error}", display(command)))
}

/// Shows a command the way a person would type it, for example `RUSTDOCFLAGS="-D warnings" cargo doc --workspace`.
pub fn display(command: &Command) -> String {
    let mut words = Vec::new();
    for (name, value) in command.get_envs() {
        if let Some(value) = value {
            words.push(format!(
                "{}={}",
                name.to_string_lossy(),
                quote(&value.to_string_lossy())
            ));
        }
    }
    let program = Path::new(command.get_program());
    words.push(
        program
            .file_stem()
            .unwrap_or(program.as_os_str())
            .to_string_lossy()
            .into_owned(),
    );
    words.extend(command.get_args().map(|arg| quote(&arg.to_string_lossy())));
    words.join(" ")
}

fn quote(word: &str) -> String {
    if !word.is_empty() && !word.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'') {
        return word.to_owned();
    }
    format!("\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_a_command_like_a_shell_line() {
        let mut command = Command::new("/home/me/.rustup/toolchains/1.99.0/bin/cargo");
        command
            .args(["doc", "--workspace"])
            .env("RUSTDOCFLAGS", "-D warnings")
            .env_remove("CARGO_ENCODED_RUSTDOCFLAGS");
        assert_eq!(
            display(&command),
            r#"RUSTDOCFLAGS="-D warnings" cargo doc --workspace"#
        );
    }

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(quote("--locked"), "--locked");
        assert_eq!(quote(""), r#""""#);
        assert_eq!(quote(r#"a "b""#), r#""a \"b\"""#);
    }
}
