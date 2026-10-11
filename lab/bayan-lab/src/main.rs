//! The `bayan-lab` command-line tool of the Fidelity Lab. See [`bayan_lab::cli`] for the commands, or run `bayan-lab help`.

#![forbid(unsafe_code)]

use std::io::Write as _;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    let Ok(args) = std::env::args_os()
        .skip(1)
        .map(std::ffi::OsString::into_string)
        .collect::<Result<Vec<String>, _>>()
    else {
        let _ = writeln!(err, "bayan-lab: arguments must be valid Unicode");
        return ExitCode::from(bayan_lab::cli::Outcome::Error.code());
    };
    ExitCode::from(bayan_lab::cli::run(&args, &mut out, &mut err).code())
}
