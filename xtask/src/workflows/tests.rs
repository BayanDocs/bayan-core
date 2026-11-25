use super::*;

/// Whether some command of a line of a script builds.
fn builds_line(line: &str) -> bool {
    commands(line).iter().any(|words| builds(words))
}

#[test]
fn finds_a_compiler_in_every_form_of_a_command() {
    for line in [
        "cargo build --locked",
        r#"RUSTFLAGS="-C debuginfo=0" cargo build --locked"#,
        "cd fuzz && cargo fuzz build",
        "test -d target || cargo build",
        "cd fuzz; cargo fuzz build",
        "echo y | cargo build",
        "(cd fuzz && cargo build)",
        "{ cargo build; }",
        "echo $(cargo metadata --format-version 1)",
        "echo `rustc -vV`",
        "env CARGO_TERM_COLOR=always cargo build",
        "! cargo build",
        "/home/runner/.cargo/bin/cargo build",
        r"C:\Users\runner\.cargo\bin\cargo.exe build",
        "& cargo build",
        "cargo +nightly-2026-11-16 fuzz build",
        "rustc src/main.rs",
        r#"bash -c "cargo build --locked""#,
        "sh -c 'cd fuzz && cargo fuzz build'",
        "cargo \\\n  build --locked",
        "cargo xtask sdk --target x86_64-unknown-linux-gnu",
        "RUST_BACKTRACE=1 cargo xtask wasm-package",
        "cargo xtask verify --release",
        "cargo xtask check-exact-pins && cargo build",
        "scripts/dev-setup.sh",
        "./scripts/dev-setup.sh --no-node",
        "bash scripts/dev-setup.sh",
        r#""$GITHUB_WORKSPACE/scripts/dev-setup.sh" --no-miri"#,
        "cd scripts && ./dev-setup.sh",
        "command cargo build",
        "which cargo && cargo build",
    ] {
        assert!(builds_line(line), "{line}");
    }
    for line in [
        "cargo xtask check-exact-pins",
        "cargo xtask check-lockfile-age",
        "cargo xtask verify",
        "RUST_BACKTRACE=1 cargo xtask verify",
        "cargo --version",
        "rustc --version",
        r#"rustc "+$FUZZ_TOOLCHAIN" --version"#,
        "cargo deny --version",
        "cargo-deny --version",
        "rustup toolchain install",
        r#"echo "$HOME/.cargo/bin" >> "$GITHUB_PATH""#,
        "# cargo build",
        "echo done # cargo build",
        "command -v cargo",
        "which rustc",
        r#"cp "$name/cargo-deny$exe" "$(dirname "$(command -v cargo)")/""#,
    ] {
        assert!(!builds_line(line), "{line}");
    }
}

/// A workflow indented with four spaces, whose jobs show every rule.
const WORKFLOW: &str = r#"name: scratch

on:
    pull_request:
    merge_group:
        types: [checks_requested]

jobs:
    good:
        runs-on: ubuntu-24.04
        steps:
            - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
              with:
                  # The age check needs the full history.
                  fetch-depth: 0
            - run: cargo xtask check-exact-pins
            - name: The age check
              run: cargo xtask check-lockfile-age
            - name: Build
              run: |
                  cargo build --locked
            - name: Build again, after a step that failed
              if: ${{ !cancelled() }}
              run: cargo build --release --locked
    early:
        runs-on: ubuntu-24.04
        steps:
            - run: |
                  RUSTFLAGS="-C debuginfo=0" cargo build --locked
            - run: cargo xtask check-exact-pins
            - run: cargo xtask check-lockfile-age
    shallow:
        runs-on: ubuntu-24.04
        steps:
            - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
            - run: cargo xtask check-exact-pins
              continue-on-error: true
            - run: cargo xtask check-lockfile-age
              if: github.event_name == 'push'
            - run: ./scripts/dev-setup.sh --no-node
    codeql:
        strategy:
            matrix:
                language: [python, rust]
        runs-on: ubuntu-24.04
        steps:
            - uses: github/codeql-action/init@33d7b85da1a04171e211befe2c739303b9a4c066 # v4.39.3
              with:
                  languages: ${{ matrix.language }}
    local:
        runs-on: ubuntu-24.04
        steps:
            - uses: ./.github/actions/build
    quiet:
        runs-on: ubuntu-24.04
        steps:
            - run: echo "nothing to build"
"#;

#[test]
fn checks_the_order_of_every_job_at_any_indentation() {
    let (building, problems) = supply_chain_order("scratch.yml", WORKFLOW).unwrap();
    assert_eq!(
        building,
        [
            "scratch.yml good",
            "scratch.yml early",
            "scratch.yml shallow",
            "scratch.yml codeql",
            "scratch.yml local",
        ]
    );
    assert_eq!(
        problems,
        [
            "scratch.yml, job `good`, line 23: `cargo build --release --locked` runs `if: ${{ !cancelled() }}`, also when an earlier step, such as a supply-chain check, failed",
            "scratch.yml, job `early`, line 28: `RUSTFLAGS=-C debuginfo=0 cargo build --locked` runs before `cargo xtask check-exact-pins` and `cargo xtask check-lockfile-age`",
            "scratch.yml, job `early`, line 31: `cargo xtask check-lockfile-age` runs before a checkout with `fetch-depth: 0`",
            "scratch.yml, job `shallow`, line 37: `cargo xtask check-exact-pins` may fail without stopping the job (`continue-on-error: true`)",
            "scratch.yml, job `shallow`, line 39: `cargo xtask check-lockfile-age` runs only `if: github.event_name == 'push'`, so a job that builds can run without it",
            "scratch.yml, job `shallow`, line 38: `cargo xtask check-lockfile-age` runs before a checkout with `fetch-depth: 0`",
            "scratch.yml, job `codeql`, line 47: `CodeQL's analysis of Rust` runs before `cargo xtask check-exact-pins` and `cargo xtask check-lockfile-age`",
            "scratch.yml, job `local`, line 53: `uses: ./.github/actions/build` runs before `cargo xtask check-exact-pins` and `cargo xtask check-lockfile-age`",
        ]
    );
    assert_eq!(
        on_every_pull_request("scratch.yml", WORKFLOW),
        Ok(Some(true))
    );
}

#[test]
fn finds_the_workflows_that_must_run_in_the_merge_queue_at_any_indentation() {
    let on = |events: &str| {
        on_every_pull_request(
            "scratch.yml",
            &format!("on:\n{events}\njobs:\n    quiet:\n        runs-on: ubuntu-24.04\n"),
        )
    };
    assert_eq!(on("    pull_request:\n    push:"), Ok(Some(false)));
    assert_eq!(
        on("    \"pull_request\":\n        branches: [main]"),
        Ok(Some(false))
    );
    assert_eq!(
        on("    pull_request:\n    merge_group:\n        types:\n            - checks_requested"),
        Ok(Some(true))
    );
    assert_eq!(
        on("    pull_request:\n    merge_group:\n        types: [destroyed]"),
        Ok(Some(false))
    );
    assert_eq!(
        on("    pull_request:\n        paths:\n            - lab/**"),
        Ok(None)
    );
    assert_eq!(on("    push:\n    workflow_dispatch:"), Ok(None));
}

#[test]
fn refuses_what_it_cannot_read() {
    let refused = |workflow: &str, expected: &str| {
        let order = supply_chain_order("scratch.yml", workflow);
        let events = on_every_pull_request("scratch.yml", workflow);
        let problem = order
            .err()
            .or(events.err())
            .unwrap_or_else(|| panic!("read:\n{workflow}"));
        assert!(problem.contains(expected), "{expected}\n{problem}");
    };
    let on = "on:\n  pull_request:\n";
    refused(&format!("{on}name: scratch\n"), "has no `jobs:`");
    refused(
        &format!("{on}jobs:\n"),
        "`jobs:` must open a block below it",
    );
    refused(
        &format!("{on}jobs:\n  build: {{runs-on: ubuntu-24.04, steps: [{{run: cargo build}}]}}\n"),
        "job `build`: a job must be a block of keys",
    );
    refused(
        &format!(
            "{on}jobs:\n    first:\n      runs-on: ubuntu-24.04\n  second:\n    runs-on: ubuntu-24.04\n"
        ),
        "a key at another indentation than the first",
    );
    refused(
        &format!("{on}jobs:\n  build:\n    steps: [{{run: cargo build}}]\n"),
        "`steps:` must open a block of items below it",
    );
    refused(
        &format!("{on}jobs:\n  build:\n    steps:\n    - run: cargo build\n"),
        "`- run: cargo build` is not a key of a mapping",
    );
    refused(
        &format!("{on}jobs:\n  build:\n    steps:\n      run: cargo build\n"),
        "`run: cargo build` is not an item of the sequence",
    );
    refused(
        &format!("{on}jobs:\n  build:\n    steps: *steps\n"),
        "an anchor, an alias or a merge key",
    );
    refused(
        &format!("{on}jobs:\n  build:\n    <<: *defaults\n"),
        "an anchor, an alias or a merge key",
    );
    refused(
        &format!("{on}jobs:\n\tbuild:\n"),
        "a tab in the indentation",
    );
    refused(
        "on: [push, pull_request]\njobs:\n  quiet:\n    runs-on: ubuntu-24.04\n",
        "`on:` must open a block below it",
    );
    refused(
        "on:\n  pull_request: {}\njobs:\n  quiet:\n    runs-on: ubuntu-24.04\n",
        "`pull_request:` with a value on its line",
    );
    refused(
        "on:\n  - pull_request\njobs:\n  quiet:\n    runs-on: ubuntu-24.04\n",
        "`- pull_request` is not a key of a mapping",
    );
    refused(
        &format!(
            "{on}jobs:\n  build:\n    runs-on: ubuntu-24.04\n    steps:\n      - uses: github/codeql-action/init@33d7b85da1a04171e211befe2c739303b9a4c066\n        with:\n          languages: ${{{{ fromJSON(inputs.languages) }}}}\n"
        ),
        "languages from an expression other than one value of the job's matrix",
    );
}
