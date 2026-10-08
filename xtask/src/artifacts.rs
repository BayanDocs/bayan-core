//! `cargo xtask sdk`, `cargo xtask wasm-package` and `cargo xtask c-driver`: the engine's artifacts for the shells, and the C test driver (work package CORE-007).
//!
//! - `sdk` builds the C SDK for one target, the host by default: the engine as a static and a dynamic library, its C header and the protocol's JSON Schema, in the layout that bayan-desktop expects (its `src/engine/README.md`).
//! - `wasm-package` builds the WebAssembly package for the web shell: the engine module with wasm-bindgen's JavaScript glue and TypeScript declarations, the worker host `bayan-worker.js`, the protocol's JSON Schema and TypeScript declarations, and a test page.
//! - `c-driver` compiles the C test driver, `crates/bayan-ffi/c-driver/driver.c`, against an SDK twice, linked with the static and with the dynamic library, and runs both.
//! - `miri` runs bayan-ffi's unit tests under Miri, which detects undefined behaviour in its pointer handling (ADR-0006 §2).
//!
//! Each artifact carries the licence texts its binaries need: BayanDocs' own, the third-party crates' notices (`notices.rs`) and the Rust standard library's. CI adds SHA-256 checksums and uploads the artifacts.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use crate::process::{self, cargo};
use crate::{json, notices};

/// The WebAssembly target of the web shell's engine (ADR-0014).
const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// The nightly toolchain that runs Miri: the BayanDocs cloud environment's nightly (`RUST_NIGHTLY` in the docs repository's `scripts/cloud-environment-setup.sh`). `scripts/dev-setup.sh` installs it with Miri, and a test keeps the two equal; change them in the monthly dependency session.
pub const MIRI_TOOLCHAIN: &str = "nightly-2026-10-02";

/// The licence of the artifacts, as an SPDX expression (ADR-0003), and the files under `LICENSES/` that hold it.
const LICENSE: &str = "GPL-3.0-or-later WITH LicenseRef-BayanDocs-App-Store-Permission";
const LICENSE_FILES: [&str; 2] = [
    "GPL-3.0-or-later.txt",
    "LicenseRef-BayanDocs-App-Store-Permission.txt",
];

/// The JavaScript files of the WebAssembly package that live in `crates/bayan-wasm/js`.
const WASM_PACKAGE_SCRIPTS: [&str; 4] = [
    "bayan-worker.js",
    "test-page.html",
    "test-page.css",
    "test-page.js",
];

/// `cargo xtask sdk [--target <triple>] [--out <folder>]`.
pub fn sdk(args: &[&str]) -> ExitCode {
    finish("sdk", build_sdk(args))
}

/// `cargo xtask wasm-package [--out <folder>]`.
pub fn wasm_package(args: &[&str]) -> ExitCode {
    finish("wasm-package", build_wasm_package(args))
}

/// `cargo xtask c-driver [--sdk <folder>]`.
pub fn c_driver(args: &[&str]) -> ExitCode {
    finish("c-driver", run_c_driver(args))
}

/// `cargo xtask miri`.
pub fn miri(args: &[&str]) -> ExitCode {
    finish("miri", run_miri(args))
}

fn run_miri(args: &[&str]) -> Result<(), String> {
    Options::parse(args, &[])?;
    let root = crate::workspace_root();
    // Through rustup, because the Cargo that runs xtask is the pinned stable one.
    let mut command = Command::new("rustup");
    command
        .args(["run", MIRI_TOOLCHAIN, "cargo", "miri", "test"])
        .args(["--package", "bayan-ffi", "--lib", "--locked"])
        .current_dir(&root);
    process::run(&mut command).map_err(|problem| {
        format!(
            "{problem}\nMiri and the {MIRI_TOOLCHAIN} toolchain come from scripts/dev-setup.sh."
        )
    })
}

fn finish(command: &str, result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("\ncargo xtask {command} FAILED:\n{problem}");
            ExitCode::FAILURE
        }
    }
}

/// Options of the form `--name value`.
struct Options<'a>(Vec<(&'a str, &'a str)>);

impl<'a> Options<'a> {
    fn parse(args: &[&'a str], allowed: &[&str]) -> Result<Self, String> {
        let mut values = Vec::new();
        let mut rest = args.iter();
        while let Some(&name) = rest.next() {
            if !allowed.contains(&name) {
                return Err(format!(
                    "unknown option `{name}`; the options are {}",
                    allowed.join(", ")
                ));
            }
            let value = rest
                .next()
                .ok_or_else(|| format!("`{name}` needs a value"))?;
            values.push((name, *value));
        }
        Ok(Self(values))
    }

    fn get(&self, name: &str) -> Option<&'a str> {
        self.0
            .iter()
            .rev()
            .find(|(option, _)| *option == name)
            .map(|(_, value)| *value)
    }
}

/// The operating system family of a target triple, which decides the names and places of its library files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Linux,
    MacOs,
    Windows,
}

impl Family {
    fn of(target: &str) -> Result<Self, String> {
        if target.contains("-windows-msvc") {
            Ok(Self::Windows)
        } else if target.contains("-apple-darwin") {
            Ok(Self::MacOs)
        } else if target.contains("-linux-") {
            Ok(Self::Linux)
        } else {
            Err(format!(
                "no C SDK layout for {target}: the SDK is built for Linux, macOS and Windows with MSVC"
            ))
        }
    }

    /// The linker arguments that give the dynamic library the name programs record (bayan-desktop's `src/engine/README.md`): a SONAME on Linux, an install name relative to the run path on macOS.
    fn dynamic_link_args(self) -> &'static [&'static str] {
        match self {
            Self::Linux => &["-C", "link-arg=-Wl,-soname,libbayan_ffi.so"],
            Self::MacOs => &["-C", "link-arg=-Wl,-install_name,@rpath/libbayan_ffi.dylib"],
            Self::Windows => &[],
        }
    }

    /// The files Cargo writes and where they go in the SDK: (file in Cargo's output folder, path in the SDK, required).
    fn sdk_files(self) -> &'static [(&'static str, &'static str, bool)] {
        match self {
            Self::Linux => &[
                ("libbayan_ffi.so", "lib/libbayan_ffi.so", true),
                ("libbayan_ffi.a", "lib/static/libbayan_ffi.a", true),
            ],
            Self::MacOs => &[
                ("libbayan_ffi.dylib", "lib/libbayan_ffi.dylib", true),
                ("libbayan_ffi.a", "lib/static/libbayan_ffi.a", true),
            ],
            // The import library keeps the name Rust gives it, so it cannot be mistaken for the static library, which has its own folder.
            Self::Windows => &[
                ("bayan_ffi.dll", "bin/bayan_ffi.dll", true),
                ("bayan_ffi.pdb", "bin/bayan_ffi.pdb", false),
                ("bayan_ffi.dll.lib", "lib/bayan_ffi.dll.lib", true),
                ("bayan_ffi.lib", "lib/static/bayan_ffi.lib", true),
            ],
        }
    }

    fn dynamic_library(self) -> &'static str {
        match self {
            Self::Linux => {
                "`lib/libbayan_ffi.so`: the engine as a shared library, with the SONAME `libbayan_ffi.so`."
            }
            Self::MacOs => {
                "`lib/libbayan_ffi.dylib`: the engine as a dynamic library, with the install name `@rpath/libbayan_ffi.dylib`."
            }
            Self::Windows => {
                "`bin/bayan_ffi.dll` and its import library `lib/bayan_ffi.dll.lib`: the engine as a DLL (`bin/bayan_ffi.pdb` holds its debug symbols)."
            }
        }
    }

    fn static_library(self) -> &'static str {
        match self {
            Self::Linux | Self::MacOs => "lib/static/libbayan_ffi.a",
            Self::Windows => "lib/static/bayan_ffi.lib",
        }
    }
}

fn build_sdk(args: &[&str]) -> Result<(), String> {
    let root = crate::workspace_root();
    let options = Options::parse(args, &["--target", "--out"])?;
    let target = match options.get("--target") {
        Some(target) => target.to_owned(),
        None => host_target(&root)?,
    };
    let family = Family::of(&target)?;
    let metadata = full_metadata(&root, &target)?;
    let target_dir = target_directory(&metadata)?;
    let out = options.get("--out").map_or_else(
        || {
            target_dir
                .join("artifacts")
                .join(format!("bayan-core-sdk-{target}"))
        },
        PathBuf::from,
    );
    println!(
        "cargo xtask sdk: the C SDK for {target} in {}",
        out.display()
    );

    // The static library, and the system libraries that a program linking it needs, which rustc names on request.
    let mut command = cargo(&root);
    command.args(library_build_args("bayan-ffi", &target, "staticlib"));
    // Plain text, because the note is read from Cargo's output, which CI colours (CARGO_TERM_COLOR=always).
    command.args(["--color", "never", "--", "--print", "native-static-libs"]);
    // Cargo replays the note from its cache when the library is already built.
    let (_, messages) = run_and_capture_both(&mut command)?;
    let native_libs = native_static_libs(&messages).ok_or_else(|| {
        format!(
            "`{}` did not report the native libraries",
            process::display(&command)
        )
    })?;
    // The dynamic library.
    let mut command = cargo(&root);
    command.args(library_build_args("bayan-ffi", &target, "cdylib"));
    if !family.dynamic_link_args().is_empty() {
        command.arg("--").args(family.dynamic_link_args());
    }
    process::run(&mut command)?;

    fresh_folder(&out, &target_dir)?;
    let built = target_dir.join(&target).join("release");
    for (file, destination, required) in family.sdk_files() {
        let source = built.join(file);
        if source.is_file() {
            copy(&source, &out.join(destination))?;
        } else if *required {
            return Err(format!("Cargo did not write {}", source.display()));
        }
    }
    copy(
        &root
            .join("crates")
            .join("bayan-ffi")
            .join("include")
            .join("bayan_ffi.h"),
        &out.join("include").join("bayan_ffi.h"),
    )?;
    write(
        &out.join("lib").join("static").join("native-libs.txt"),
        &format!("{native_libs}\n"),
    )?;
    write(
        &out.join("schema").join("engine-protocol.schema.json"),
        &protocol_file(&root, "schema")?,
    )?;
    let version = engine_version(&metadata)?;
    write_licenses(
        &root,
        &out,
        &metadata,
        "bayan-ffi",
        "the BayanDocs engine C SDK",
    )?;
    write(
        &out.join("README.md"),
        &sdk_readme(&version, &target, family, &native_libs),
    )?;
    println!("cargo xtask sdk: done");
    Ok(())
}

/// The arguments after `cargo` that build the library of `package` in release mode as `crate_type`.
fn library_build_args(package: &str, target: &str, crate_type: &str) -> Vec<String> {
    [
        "rustc",
        "--package",
        package,
        "--lib",
        "--release",
        "--locked",
        "--target",
        target,
        "--crate-type",
        crate_type,
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Reads the note `native-static-libs: …` that rustc prints for a static library.
fn native_static_libs(output: &str) -> Option<String> {
    without_colours(output)
        .lines()
        .find_map(|line| line.split_once("native-static-libs:"))
        .map(|(_, libs)| libs.trim().to_owned())
        .filter(|libs| !libs.is_empty())
}

/// Removes the escape sequences that colour terminal output (`ESC [ … letter`), in case a coloured note slips through.
fn without_colours(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(character);
        }
    }
    plain
}

fn sdk_readme(version: &str, target: &str, family: Family, native_libs: &str) -> String {
    let static_library = family.static_library();
    format!(
        "# BayanDocs engine C SDK\n\n\
         The BayanDocs engine {version} for `{target}`, with engine protocol version 0. Built by `cargo xtask sdk` in bayan-core (work package CORE-007).\n\n\
         - `include/bayan_ffi.h`: the C interface (engine protocol specification §3.1). Read the rules for callers at its top.\n\
         - {}\n\
         - `{static_library}`: the engine as a static library. A program that links it also links the system libraries in `lib/static/native-libs.txt`: `{native_libs}`.\n\
         - `schema/engine-protocol.schema.json`: the JSON Schema of the protocol's messages.\n\
         - `LICENSES/`, `THIRD-PARTY-LICENSES.txt` and `RUST-STANDARD-LIBRARY-COPYRIGHT.html`: BayanDocs' licence ({LICENSE}) and the notices of the code compiled into the libraries. Distribute them with the libraries.\n\n\
         The protocol: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md\n",
        family.dynamic_library()
    )
}

fn build_wasm_package(args: &[&str]) -> Result<(), String> {
    let root = crate::workspace_root();
    let options = Options::parse(args, &["--out"])?;
    let metadata = full_metadata(&root, WASM_TARGET)?;
    let target_dir = target_directory(&metadata)?;
    let out = options.get("--out").map_or_else(
        || target_dir.join("artifacts").join("bayan-core-wasm"),
        PathBuf::from,
    );
    println!(
        "cargo xtask wasm-package: the WebAssembly package in {}",
        out.display()
    );

    // wasm-bindgen's command-line tool must be the version of the wasm-bindgen crate in Cargo.lock, because the two halves of its glue must match.
    let expected = locked_version(&root, "wasm-bindgen")?;
    let mut command = Command::new("wasm-bindgen");
    command.arg("--version");
    let found = run_and_capture(&mut command).map_err(|_| {
        format!(
            "wasm-bindgen is not installed. Install version {expected}: `cargo install --locked --version {expected} wasm-bindgen-cli`, or run scripts/dev-setup.sh."
        )
    })?;
    if found.split_whitespace().nth(1) != Some(expected.as_str()) {
        return Err(format!(
            "the wasm-bindgen crate in Cargo.lock is {expected}, so its command-line tool must be too, but found `{}`. Install it with `cargo install --locked --version {expected} wasm-bindgen-cli`, or run scripts/dev-setup.sh.",
            found.trim()
        ));
    }

    let mut command = cargo(&root);
    command.args(library_build_args("bayan-wasm", WASM_TARGET, "cdylib"));
    process::run(&mut command)?;

    fresh_folder(&out, &target_dir)?;
    let module = target_dir
        .join(WASM_TARGET)
        .join("release")
        .join("bayan_wasm.wasm");
    // `--experimental-reset-state-function` adds `__wbg_reset_state`, with which the worker host replaces an instance that panicked (engine protocol §3.2).
    let mut command = Command::new("wasm-bindgen");
    command
        .args(["--target", "web", "--experimental-reset-state-function"])
        .arg("--out-dir")
        .arg(&out)
        .arg(&module);
    process::run(&mut command)?;

    let scripts = root.join("crates").join("bayan-wasm").join("js");
    for file in WASM_PACKAGE_SCRIPTS {
        copy(&scripts.join(file), &out.join(file))?;
    }
    write(
        &out.join("engine-protocol.schema.json"),
        &protocol_file(&root, "schema")?,
    )?;
    write(
        &out.join("engine-protocol.d.ts"),
        &protocol_file(&root, "typescript")?,
    )?;
    let version = engine_version(&metadata)?;
    write(&out.join("package.json"), &package_json(&version))?;
    write_licenses(
        &root,
        &out,
        &metadata,
        "bayan-wasm",
        "the BayanDocs engine WebAssembly package",
    )?;
    write(&out.join("README.md"), &wasm_readme(&version))?;
    println!("cargo xtask wasm-package: done");
    Ok(())
}

/// A `package.json` that names the package and its licence. It is private: the package is a build artifact, never published to a registry.
fn package_json(version: &str) -> String {
    format!(
        "{{\n  \"name\": \"@bayandocs/engine-wasm\",\n  \"version\": \"{version}\",\n  \"description\": \"The BayanDocs engine as WebAssembly, with its Web Worker host (engine protocol v0)\",\n  \"license\": \"{LICENSE}\",\n  \"private\": true,\n  \"type\": \"module\"\n}}\n"
    )
}

fn wasm_readme(version: &str) -> String {
    format!(
        "# BayanDocs engine WebAssembly package\n\n\
         The BayanDocs engine {version} as WebAssembly, with engine protocol version 0. Built by `cargo xtask wasm-package` in bayan-core (work package CORE-007).\n\n\
         - `bayan-worker.js`: the worker host. Start it with `new Worker(new URL(\"bayan-worker.js\", base), {{ type: \"module\" }})` and exchange protocol messages with it (engine protocol specification §3.2). It has no dependencies.\n\
         - `bayan_wasm.js`, `bayan_wasm_bg.wasm` and their `.d.ts` files: the engine module and wasm-bindgen's glue, which the worker host loads. The web shell never calls them directly.\n\
         - `engine-protocol.d.ts` and `engine-protocol.schema.json`: the protocol's TypeScript declarations and JSON Schema.\n\
         - `test-page.html`, `test-page.css` and `test-page.js`: a page for trying the engine by hand. Serve this folder over HTTP, for example with `python3 -m http.server`, and open `test-page.html`.\n\
         - `LICENSES/`, `THIRD-PARTY-LICENSES.txt` and `RUST-STANDARD-LIBRARY-COPYRIGHT.html`: BayanDocs' licence ({LICENSE}) and the notices of the code compiled into the module. Distribute them with it.\n\n\
         The page that runs the worker needs a Content Security Policy that allows `'wasm-unsafe-eval'` in `script-src` and `'self'` in `worker-src`. Under Trusted Types, creating the worker needs a policy for its script address.\n\n\
         The protocol: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md\n"
    )
}

fn run_c_driver(args: &[&str]) -> Result<(), String> {
    let root = crate::workspace_root();
    let options = Options::parse(args, &["--sdk"])?;
    let target = host_target(&root)?;
    let family = Family::of(&target)?;
    let metadata = full_metadata(&root, &target)?;
    let target_dir = target_directory(&metadata)?;
    let sdk = options.get("--sdk").map_or_else(
        || {
            target_dir
                .join("artifacts")
                .join(format!("bayan-core-sdk-{target}"))
        },
        PathBuf::from,
    );
    // Absolute, but not canonicalized: on Windows that would give a `\\?\` path, which the MSVC tools do not all accept.
    let sdk = std::path::absolute(&sdk)
        .map_err(|error| format!("no C SDK at {}: {error}", sdk.display()))?;
    if !sdk.join("include").join("bayan_ffi.h").is_file() {
        return Err(format!(
            "no C SDK at {}. Build it first with `cargo xtask sdk`.",
            sdk.display()
        ));
    }
    let source = root
        .join("crates")
        .join("bayan-ffi")
        .join("c-driver")
        .join("driver.c");
    let out = target_dir.join("c-driver");
    std::fs::create_dir_all(&out)
        .map_err(|error| format!("cannot create {}: {error}", out.display()))?;
    let native_libs = read(&sdk.join("lib").join("static").join("native-libs.txt"))?;
    let native_libs: Vec<&str> = native_libs.split_whitespace().collect();

    let mut outputs = Vec::new();
    for linking in ["static", "dynamic"] {
        let program = out.join(format!(
            "c-driver-{linking}{}",
            if family == Family::Windows {
                ".exe"
            } else {
                ""
            }
        ));
        let mut compile = c_compile(family, &sdk, &source, &program, linking, &native_libs);
        process::run(&mut compile)?;
        if family == Family::Windows && linking == "dynamic" {
            // Windows looks for a DLL next to the program first.
            copy(
                &sdk.join("bin").join("bayan_ffi.dll"),
                &out.join("bayan_ffi.dll"),
            )?;
        }
        println!("    running the C driver, {linking}ally linked:");
        let mut run = Command::new(&program);
        let (output, errors) = run_and_capture_both(&mut run)?;
        for line in output.lines() {
            println!("      {line}");
        }
        // The driver writes only to standard output, and it makes the engine panic on purpose: the engine must not print that panic, because a panic message could quote document content (spec §3.1). So anything on standard error fails the run.
        if !errors.is_empty() {
            return Err(format!(
                "the C driver, {linking}ally linked, wrote to standard error:\n{errors}"
            ));
        }
        outputs.push(output);
    }
    let tiles: Vec<Option<&str>> = outputs
        .iter()
        .map(|output| output.lines().find(|line| line.starts_with("tile ")))
        .collect();
    match tiles.as_slice() {
        [Some(first), Some(second)] if first == second => {
            println!("cargo xtask c-driver: both programs passed and rendered the same {first}");
            Ok(())
        }
        _ => Err(format!(
            "the statically and the dynamically linked driver rendered different tiles: {tiles:?}"
        )),
    }
}

/// The command that compiles the C driver against the SDK, warnings as errors, with the platform's C compiler: MSVC's `cl` on Windows (run it from a Visual Studio developer environment), otherwise `cc`, or the compiler in `CC`.
fn c_compile(
    family: Family,
    sdk: &Path,
    source: &Path,
    program: &Path,
    linking: &str,
    native_libs: &[&str],
) -> Command {
    let headers = sdk.join("include");
    if family == Family::Windows {
        let mut command = Command::new("cl");
        // /MD: the dynamic C runtime, which Rust's MSVC libraries are built for.
        command
            .args(["/nologo", "/std:c11", "/W4", "/WX", "/O2", "/MD"])
            .arg(format!("/I{}", headers.display()))
            .arg(source)
            .arg(format!("/Fe{}", program.display()))
            .arg(format!(
                "/Fo{}\\",
                program.parent().unwrap_or(sdk).display()
            ))
            .arg("/link");
        if linking == "static" {
            command
                .arg(sdk.join("lib").join("static").join("bayan_ffi.lib"))
                .args(native_libs);
        } else {
            command.arg(sdk.join("lib").join("bayan_ffi.dll.lib"));
        }
        return command;
    }
    let compiler = std::env::var_os("CC").unwrap_or_else(|| "cc".into());
    let mut command = Command::new(compiler);
    command
        .args([
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Wpedantic",
            "-Werror",
            "-O2",
        ])
        .arg("-I")
        .arg(&headers)
        .arg(source)
        .arg("-o")
        .arg(program);
    if linking == "static" {
        command
            .arg(sdk.join("lib").join("static").join("libbayan_ffi.a"))
            .args(native_libs);
    } else {
        let lib = sdk.join("lib");
        command
            .arg("-L")
            .arg(&lib)
            .arg("-lbayan_ffi")
            .arg(format!("-Wl,-rpath,{}", lib.display()));
    }
    command.arg("-pthread");
    command
}

/// Writes the licence texts into an artifact: BayanDocs' licence, the notices of the third-party crates in `package`'s library, and the Rust standard library's notices.
fn write_licenses(
    root: &Path,
    out: &Path,
    metadata: &json::Value,
    package: &str,
    artifact: &str,
) -> Result<(), String> {
    for file in LICENSE_FILES {
        copy(
            &root.join("LICENSES").join(file),
            &out.join("LICENSES").join(file),
        )?;
    }
    let crates = notices::bundled_crates(metadata, package)?;
    write(
        &out.join("THIRD-PARTY-LICENSES.txt"),
        &notices::notices(artifact, &crates)?,
    )?;
    let mut rustc = Command::new("rustc");
    rustc.args(["--print", "sysroot"]).current_dir(root);
    let sysroot = run_and_capture(&mut rustc)?;
    copy(
        &Path::new(sysroot.trim())
            .join("share")
            .join("doc")
            .join("rust")
            .join("COPYRIGHT-library.html"),
        &out.join("RUST-STANDARD-LIBRARY-COPYRIGHT.html"),
    )
}

/// The protocol's JSON Schema (`schema`) or TypeScript declarations (`typescript`), generated from the Rust types by bayan-cli.
fn protocol_file(root: &Path, kind: &str) -> Result<String, String> {
    let mut command = cargo(root);
    command.args([
        "run",
        "--quiet",
        "--package",
        "bayan-cli",
        "--release",
        "--locked",
        "--",
        "protocol",
        kind,
    ]);
    run_and_capture(&mut command)
}

/// `cargo metadata` with the dependency graph, for one target.
fn full_metadata(root: &Path, target: &str) -> Result<json::Value, String> {
    let mut command = cargo(root);
    command.args([
        "metadata",
        "--format-version",
        "1",
        "--locked",
        "--filter-platform",
        target,
    ]);
    json::parse(&run_and_capture(&mut command)?)
}

fn target_directory(metadata: &json::Value) -> Result<PathBuf, String> {
    metadata
        .get("target_directory")
        .and_then(json::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "`cargo metadata` did not name the target directory".to_owned())
}

/// The engine's version: the version of the workspace's bayan-engine package.
fn engine_version(metadata: &json::Value) -> Result<String, String> {
    metadata
        .get("packages")
        .and_then(json::Value::as_array)
        .unwrap_or_default()
        .iter()
        .find(|package| {
            package.get("name").and_then(json::Value::as_str) == Some("bayan-engine")
                && package.get("source") == Some(&json::Value::Null)
        })
        .and_then(|package| package.get("version"))
        .and_then(json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "`cargo metadata` did not list bayan-engine".to_owned())
}

/// The version of a package in `Cargo.lock`, which must appear exactly once.
fn locked_version(root: &Path, package: &str) -> Result<String, String> {
    let lock = read(&root.join("Cargo.lock"))?;
    let versions = lock_versions(&lock, package);
    match versions.as_slice() {
        [version] => Ok(version.clone()),
        _ => Err(format!(
            "Cargo.lock must hold exactly one version of {package}, but holds {versions:?}"
        )),
    }
}

/// The versions of `package` in the text of a `Cargo.lock`.
fn lock_versions(lock: &str, package: &str) -> Vec<String> {
    let name = format!("name = \"{package}\"");
    let mut versions = Vec::new();
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim() == name
            && let Some(version) = lines
                .next()
                .and_then(|next| next.trim().strip_prefix("version = \""))
                .and_then(|rest| rest.strip_suffix('"'))
        {
            versions.push(version.to_owned());
        }
    }
    versions
}

/// The host's target triple, from `rustc -vV`.
fn host_target(root: &Path) -> Result<String, String> {
    let mut rustc = Command::new("rustc");
    rustc.arg("-vV").current_dir(root);
    let output = run_and_capture(&mut rustc)?;
    output
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(|host| host.trim().to_owned())
        .ok_or_else(|| "`rustc -vV` did not name the host".to_owned())
}

/// Runs a command, printing it first, and returns what it wrote to standard output, or an error with all its output if it failed.
fn run_and_capture(command: &mut Command) -> Result<String, String> {
    run_and_capture_both(command).map(|(stdout, _)| stdout)
}

/// Runs a command, printing it first, and returns what it wrote to standard output and to standard error, or an error with that text if it failed.
fn run_and_capture_both(command: &mut Command) -> Result<(String, String), String> {
    println!("    $ {}", process::display(command));
    let output = process::capture(command)?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.status.success() {
        Ok((stdout, stderr))
    } else {
        Err(format!(
            "`{}` failed ({}):\n{stdout}{stderr}",
            process::display(command),
            output.status
        ))
    }
}

/// Empties the output folder, or creates it. An existing folder that is not empty is emptied only if it lies inside `target/artifacts/` (Cargo's target directory), where these commands write by default, so a mistyped `--out` cannot delete anything else.
fn fresh_folder(out: &Path, target_dir: &Path) -> Result<(), String> {
    if out.exists() {
        let artifacts = target_dir.join("artifacts");
        let empty = std::fs::read_dir(out)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if !empty && !strictly_inside(out, &artifacts) {
            return Err(format!(
                "{} exists and is not empty; choose a new or empty folder (only folders inside {} are emptied)",
                out.display(),
                artifacts.display()
            ));
        }
        std::fs::remove_dir_all(out)
            .map_err(|error| format!("cannot empty {}: {error}", out.display()))?;
    }
    std::fs::create_dir_all(out)
        .map_err(|error| format!("cannot create {}: {error}", out.display()))
}

/// Whether `path` lies inside `folder`, and is not `folder` itself, after resolving links and `..`.
fn strictly_inside(path: &Path, folder: &Path) -> bool {
    std::fs::canonicalize(path)
        .ok()
        .zip(std::fs::canonicalize(folder).ok())
        .is_some_and(|(path, folder)| path != folder && path.starts_with(folder))
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    std::fs::copy(from, to).map(|_| ()).map_err(|error| {
        format!(
            "cannot copy {} to {}: {error}",
            from.display(),
            to.display()
        )
    })
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    std::fs::write(path, text).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_come_in_pairs_and_unknown_ones_are_refused() {
        let options = Options::parse(
            &["--out", "a", "--target", "b", "--out", "c"],
            &["--out", "--target"],
        )
        .unwrap();
        assert_eq!(options.get("--out"), Some("c"));
        assert_eq!(options.get("--target"), Some("b"));
        assert_eq!(options.get("--sdk"), None);
        assert!(Options::parse(&["--out"], &["--out"]).is_err());
        assert!(Options::parse(&["--force", "yes"], &["--out"]).is_err());
    }

    #[test]
    fn each_target_has_the_layout_bayan_desktop_expects() {
        assert_eq!(Family::of("x86_64-unknown-linux-gnu"), Ok(Family::Linux));
        assert_eq!(Family::of("aarch64-apple-darwin"), Ok(Family::MacOs));
        assert_eq!(Family::of("x86_64-pc-windows-msvc"), Ok(Family::Windows));
        assert!(Family::of("x86_64-pc-windows-gnu").is_err());
        assert!(Family::of(WASM_TARGET).is_err());
        let destinations = |family: Family| -> Vec<&str> {
            family
                .sdk_files()
                .iter()
                .map(|(_, destination, _)| *destination)
                .collect()
        };
        assert!(destinations(Family::Linux).contains(&"lib/libbayan_ffi.so"));
        assert!(destinations(Family::MacOs).contains(&"lib/libbayan_ffi.dylib"));
        let windows = destinations(Family::Windows);
        assert!(windows.contains(&"bin/bayan_ffi.dll"));
        assert!(windows.contains(&"lib/bayan_ffi.dll.lib"));
        // The static library may not take the import library's place, where bayan-desktop looks for `bayan_ffi.lib`.
        assert!(!windows.contains(&"lib/bayan_ffi.lib"));
    }

    #[test]
    fn only_folders_inside_target_artifacts_are_emptied() {
        let scratch =
            std::env::temp_dir().join(format!("bayan-xtask-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let target = scratch.join("target");
        let artifact = target.join("artifacts").join("sdk");
        let precious = scratch.join("precious");
        for folder in [&artifact, &precious] {
            std::fs::create_dir_all(folder).unwrap();
            std::fs::write(folder.join("file"), "x").unwrap();
        }
        // Not inside target/artifacts: refused, and nothing is deleted.
        assert!(fresh_folder(&precious, &target).is_err());
        assert!(precious.join("file").is_file());
        // target/artifacts itself, and the target directory, are not emptied either.
        assert!(fresh_folder(&target.join("artifacts"), &target).is_err());
        assert!(fresh_folder(&target, &target).is_err());
        // A folder inside target/artifacts is emptied.
        fresh_folder(&artifact, &target).unwrap();
        assert!(artifact.is_dir() && std::fs::read_dir(&artifact).unwrap().next().is_none());
        // An empty folder anywhere may be used.
        std::fs::remove_file(precious.join("file")).unwrap();
        fresh_folder(&precious, &target).unwrap();
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn reads_the_native_libraries_note() {
        let output =
            "   Compiling bayan-ffi\nnote: native-static-libs: -lgcc_s -lutil -lc\n\n    Finished";
        assert_eq!(
            native_static_libs(output).as_deref(),
            Some("-lgcc_s -lutil -lc")
        );
        assert_eq!(native_static_libs("note: native-static-libs: \n"), None);
        // As Cargo prints it when it colours its output.
        let coloured = "\u{1b}[0m\u{1b}[1m\u{1b}[92mnote\u{1b}[0m: native-static-libs: -lSystem -lc -lm\u{1b}[0m\n";
        assert_eq!(
            native_static_libs(coloured).as_deref(),
            Some("-lSystem -lc -lm")
        );
        assert_eq!(native_static_libs("Finished"), None);
    }

    #[test]
    fn reads_package_versions_from_the_lockfile() {
        let lock = "[[package]]\nname = \"wasm-bindgen\"\nversion = \"0.2.129\"\n\n[[package]]\nname = \"wasm-bindgen-macro\"\nversion = \"0.2.129\"\n\n[[package]]\nname = \"syn\"\nversion = \"2.0.1\"\n\n[[package]]\nname = \"syn\"\nversion = \"3.0.0\"\n";
        assert_eq!(lock_versions(lock, "wasm-bindgen"), ["0.2.129"]);
        assert_eq!(lock_versions(lock, "syn"), ["2.0.1", "3.0.0"]);
        assert!(lock_versions(lock, "serde").is_empty());
    }

    /// The value of `NAME=` at the start of a line of `scripts/dev-setup.sh`, up to the first space.
    fn dev_setup_pin(name: &str) -> String {
        let script = read(&crate::workspace_root().join("scripts").join("dev-setup.sh")).unwrap();
        let prefix = format!("{name}=");
        script
            .lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or_else(|| panic!("scripts/dev-setup.sh does not set {name}"))
            .to_owned()
    }

    #[test]
    fn dev_setup_pins_the_locked_wasm_bindgen() {
        let locked = locked_version(&crate::workspace_root(), "wasm-bindgen").unwrap();
        assert_eq!(dev_setup_pin("WASM_BINDGEN_VERSION"), locked);
    }

    #[test]
    fn dev_setup_pins_the_miri_toolchain() {
        assert_eq!(dev_setup_pin("MIRI_TOOLCHAIN"), MIRI_TOOLCHAIN);
    }

    #[test]
    fn the_package_json_is_private_and_names_the_licence() {
        let text = package_json("1.2.3");
        let value = json::parse(&text).unwrap();
        assert_eq!(value.get("private"), Some(&json::Value::Bool(true)));
        assert_eq!(
            value.get("version").and_then(json::Value::as_str),
            Some("1.2.3")
        );
        assert_eq!(
            value.get("license").and_then(json::Value::as_str),
            Some(LICENSE)
        );
    }

    #[test]
    fn the_artifacts_ship_the_files_of_their_licence() {
        let root = crate::workspace_root();
        for file in LICENSE_FILES {
            assert!(root.join("LICENSES").join(file).is_file(), "{file}");
        }
        for file in WASM_PACKAGE_SCRIPTS {
            assert!(
                root.join("crates")
                    .join("bayan-wasm")
                    .join("js")
                    .join(file)
                    .is_file(),
                "{file}"
            );
        }
    }
}
