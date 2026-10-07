//! # crdt-model
//!
//! The command-line driver of the CORE-004 spike (the document model on Loro): benchmarks, the long convergence run and the import fuzzer. Results and how to reproduce them are in `spikes/crdt-model/REPORT.md`.
//!
//! ```text
//! crdt-model generate --mode full|subset --out FILE [--small]
//! crdt-model bench    --mode full|subset --snapshot FILE [--edits N]
//! crdt-model converge [--runs N] [--operations N] [--threads N] [--seed N] [--check-every N]
//! crdt-model fuzz     [--seconds N] [--threads N] [--seed N] [--out DIR]
//! ```
//!
//! `bench` also runs as WebAssembly: build with `--target wasm32-wasip1` and run under Node.js with `run-wasi.mjs` (the snapshot directory appears as `/data`).
//!
//! **Status:** spike code. Nothing under `crates/` may depend on it.

#![forbid(unsafe_code)]

mod bench;
mod converge;
mod fuzz;
mod synthetic;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use bayan_model::simulation::Config;
use bench::Mode;

const USAGE: &str = "usage: crdt-model generate --mode full|subset --out FILE [--small]
       crdt-model bench --mode full|subset --snapshot FILE [--edits N]
       crdt-model converge [--runs N] [--operations N] [--threads N] [--seed N] [--check-every N]
       crdt-model fuzz [--seconds N] [--threads N] [--seed N] [--out DIR]";

/// `--name value` options.
struct Options(BTreeMap<String, String>);

impl Options {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = BTreeMap::new();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let name = arg
                .strip_prefix("--")
                .ok_or_else(|| format!("unexpected argument `{arg}`"))?;
            let value = if name == "small" {
                "true".to_owned()
            } else {
                iter.next()
                    .ok_or_else(|| format!("--{name} needs a value"))?
                    .clone()
            };
            options.insert(name.to_owned(), value);
        }
        Ok(Self(options))
    }

    fn text(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    fn number(&self, name: &str, default: u64) -> Result<u64, String> {
        self.text(name).map_or(Ok(default), |value| {
            let value = value.replace('_', "");
            match value.strip_prefix("0x") {
                Some(hex) => u64::from_str_radix(hex, 16),
                None => value.parse(),
            }
            .map_err(|_| format!("--{name} needs a number"))
        })
    }

    fn count(&self, name: &str, default: usize) -> Result<usize, String> {
        let value = self.number(name, u64::try_from(default).unwrap_or(u64::MAX))?;
        usize::try_from(value).map_err(|_| format!("--{name} is too large"))
    }

    fn mode(&self) -> Result<Mode, String> {
        match self.text("mode") {
            Some("full") | None => Ok(Mode::Full),
            Some("subset") => Ok(Mode::Subset),
            Some(other) => Err(format!("unknown mode `{other}`")),
        }
    }

    fn path(&self, name: &str) -> Result<PathBuf, String> {
        self.text(name)
            .map(PathBuf::from)
            .ok_or_else(|| format!("--{name} is required"))
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let (command, rest) = args.split_first().ok_or_else(|| USAGE.to_owned())?;
    let options = Options::parse(rest)?;
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    match command.as_str() {
        "generate" => {
            let shape = if options.text("small").is_some() {
                crdt_workload::TEN_PAGES
            } else {
                crdt_workload::FIVE_HUNDRED_PAGES
            };
            bench::generate(options.mode()?, &shape, &options.path("out")?)
        }
        "bench" => bench::bench(
            options.mode()?,
            &options.path("snapshot")?,
            options.count("edits", 1_000)?,
        ),
        "converge" => {
            let config = Config {
                replicas: 3,
                operations: options.count("operations", Config::BRIEF.operations)?,
                check_every: options.count("check-every", Config::BRIEF.check_every)?,
            };
            converge::converge(
                options.count("runs", 1_000)?,
                options.count("threads", threads)?,
                options.number("seed", 0xC0DE_0004_0001_0000)?,
                &config,
            )
        }
        "fuzz" => fuzz::fuzz(
            options.number("seconds", 60)?,
            options.count("threads", threads)?,
            options.number("seed", 0xC0DE_0004_00F2_2000)?,
            &options
                .text("out")
                .map_or_else(fuzz::default_output, PathBuf::from),
        ),
        _ => Err(USAGE.to_owned()),
    }
}

#[expect(
    clippy::print_stderr,
    reason = "a command-line tool reports errors to the person running it"
)]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("crdt-model: {problem}");
            ExitCode::FAILURE
        }
    }
}
