//! The tests of the targets' harness that need a process of their own (AC-2 of CORE-010): a panic that the engine catches still fails an engine target, and in `crdt_import` a panic in BayanDocs code fails the run while a panic of Loro's does not.
//!
//! A run fails when the target's process aborts: libFuzzer then keeps the input as `crash-…`. Each case therefore runs in a child process, this test program started again with `BAYAN_FUZZ_CHILD` naming the case, with the panic hook that libfuzzer-sys 0.4.13's `initialize` installs in a target (it prints the panic with Rust's default hook, then aborts), and, for `crdt_import`, the target's own hook on top, as its `init` installs it. The parent checks how the child ended.
//!
//! No input may make Loro panic, so the case for a panic of Loro's hands the target's hook serde_json in place of Loro's packages and raises a panic that serde_json's documentation promises, in serde_json's own code. The hook decides whose a panic is by the package whose code raised it, read from the real backtrace, as it decides for Loro's packages; a second case shows that with Loro's own packages the same panic fails the run.

use std::collections::BTreeSet;
use std::os::unix::process::ExitStatusExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::Command;

use bayan_engine::config::Config;
use serde_json::{Value, json};

use crate::loro_panics::{Lockfile, Package};
use crate::{crdt, engine};

/// The signal with which `std::process::abort` ends a process on Linux.
const SIGABRT: i32 = 6;

/// A message that makes an engine panic when its test switch allows it, and is refused otherwise.
const DIAG_PANIC: &[u8] = br#"{"v":0,"id":2,"type":"diag.panic"}"#;

/// The hook that libfuzzer-sys 0.4.13 installs in `initialize` before a target's own `init` runs (its `src/lib.rs`).
fn libfuzzer_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        std::process::abort();
    }));
}

/// A recording of the engine's tests with one more entry, `diag.panic`.
fn recording_with_a_panic() -> Vec<u8> {
    let mut recording: Value = serde_json::from_slice(engine::SESSION_RECORDING).unwrap();
    recording["entries"].as_array_mut().unwrap().push(json!({ "t_ms": 99, "kind": "message", "text": String::from_utf8(DIAG_PANIC.to_vec()).unwrap(), "digest": "fnv1a64:0000000000000000" }));
    serde_json::to_vec(&recording).unwrap()
}

/// Runs one case in the child process. Not a test of its own: [`child_process`] runs it when `BAYAN_FUZZ_CHILD` names it.
fn run_case(case: &str) {
    libfuzzer_hook();
    match case {
        "engine_messages, a panic the engine catches" => {
            engine::messages_with(Config { allow_panic: true }, DIAG_PANIC)
        }
        "engine_messages, diag.panic refused" => engine::messages(DIAG_PANIC),
        "engine_replay, a panic the replaying engine catches" => {
            engine::replay_with(Config { allow_panic: true }, &recording_with_a_panic())
        }
        "engine_replay, diag.panic refused" => engine::replay(&recording_with_a_panic()),
        "crdt_import, a panic of a package that stands in for Loro's" => {
            let lockfile = Lockfile::parse(crate::LOCKFILE).unwrap();
            let stand_in: BTreeSet<Package> = lockfile
                .packages()
                .filter(|package| package.name == "serde_json")
                .cloned()
                .collect();
            assert_eq!(stand_in.len(), 1);
            crdt::install_panic_hook_for(lockfile, stand_in);
            panic_in_serde_json();
            assert_eq!(crdt::loro_panics(), 1);
        }
        "crdt_import, the same panic with Loro's own packages" => {
            crdt::install_panic_hook().unwrap();
            panic_in_serde_json();
        }
        "crdt_import, a panic in BayanDocs code" => {
            crdt::install_panic_hook().unwrap();
            // As a property of the harness that does not hold, or an `unwrap` in the adapter: std raises the panic, and the first frame outside std is BayanDocs code.
            let nothing: Option<u8> = std::hint::black_box(None);
            let _ = nothing.unwrap();
        }
        other => panic!("no such case: {other}"),
    }
}

/// Raises a panic in serde_json's own code, which its documentation of `IndexMut` promises when a string indexes a JSON value that is neither an object nor null, and catches it, as `Doc::import` catches a panic of Loro's: the panic must unwind to here.
fn panic_in_serde_json() {
    let caught = catch_unwind(|| {
        let mut number = json!(0);
        number["key"] = json!(1);
    });
    assert!(caught.is_err());
}

/// Runs a case in a child process and returns how it ended: `Ok(())` for a normal end, `Err(signal)` for an abort, and the child's output for the messages of failing assertions.
fn in_child(case: &str) -> (Result<(), Option<i32>>, String) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::child_process",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("BAYAN_FUZZ_CHILD", case)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let ended = if output.status.success() {
        Ok(())
    } else {
        Err(output.status.signal())
    };
    (ended, text)
}

/// The entry point of the child processes; does nothing in a normal test run.
#[test]
fn child_process() {
    if let Ok(case) = std::env::var("BAYAN_FUZZ_CHILD") {
        run_case(&case);
    }
}

#[test]
fn a_panic_the_engine_catches_fails_engine_messages() {
    let (ended, output) = in_child("engine_messages, a panic the engine catches");
    assert_eq!(ended, Err(Some(SIGABRT)), "{output}");
    // The engine's own message, printed by the default hook before the abort: the engine had not caught it yet.
    assert!(
        output.contains("diag.panic: a panic that a test asked for"),
        "{output}"
    );
    let (ended, output) = in_child("engine_messages, diag.panic refused");
    assert_eq!(ended, Ok(()), "{output}");
}

#[test]
fn a_panic_the_replaying_engine_catches_fails_engine_replay() {
    let (ended, output) = in_child("engine_replay, a panic the replaying engine catches");
    assert_eq!(ended, Err(Some(SIGABRT)), "{output}");
    let (ended, output) = in_child("engine_replay, diag.panic refused");
    assert_eq!(ended, Ok(()), "{output}");
}

#[test]
fn without_the_hook_the_harness_still_fails_on_a_panic_the_engine_caught() {
    // Without libfuzzer-sys's hook the engine catches the panic and answers with the error `panic`, which the harness's own check refuses.
    let caught = catch_unwind(AssertUnwindSafe(|| {
        engine::messages_with(Config { allow_panic: true }, DIAG_PANIC)
    }));
    let message = caught.unwrap_err();
    let message = message
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| message.downcast_ref::<&str>().copied())
        .unwrap_or_default();
    assert!(message.contains("the engine caught a panic"), "{message}");
}

#[test]
fn a_panic_of_loro_s_does_not_fail_crdt_import() {
    let (ended, output) = in_child("crdt_import, a panic of a package that stands in for Loro's");
    assert_eq!(ended, Ok(()), "{output}");
    // The hook printed nothing for it.
    assert!(!output.contains("crdt_import: a panic"), "{output}");
    // With Loro's own packages the same panic fails the run: what let it through was the package that stood in for them.
    let (ended, output) = in_child("crdt_import, the same panic with Loro's own packages");
    assert_eq!(ended, Err(Some(SIGABRT)), "{output}");
    assert!(
        output.contains(
            "crdt_import: a panic that is not Loro's fails the run (raised in serde_json "
        ),
        "{output}"
    );
}

#[test]
fn a_panic_in_bayandocs_code_fails_crdt_import() {
    let (ended, output) = in_child("crdt_import, a panic in BayanDocs code");
    assert_eq!(ended, Err(Some(SIGABRT)), "{output}");
    assert!(
        output.contains(
            "crdt_import: a panic that is not Loro's fails the run (raised in ./src/tests.rs)"
        ),
        "{output}"
    );
}
