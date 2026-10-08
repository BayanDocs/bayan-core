#!/usr/bin/env bash
# Installs the tools that bayan-core needs besides cargo-deny (see AGENTS.md), at pinned versions, and says what it did. It
# runs on developer machines (Ubuntu 24.04 and macOS) and in BayanDocs cloud sessions, and is idempotent: a tool that is
# already installed at its pinned version is left alone.
#
#   - The Rust toolchain pinned in rust-toolchain.toml, with the components and targets it lists, among them wasm32-wasip1
#     for the tests in WebAssembly. rustup checks every download against the checksums of the Rust release, and never
#     updates itself here (`--no-self-update`).
#   - Node.js 24.21.0 (NODE_VERSION below), which runs those tests in the gate's `test` and `determinism` steps. A node of
#     exactly that version on PATH is used as it is, then the one the BayanDocs cloud environment installs; otherwise the
#     script downloads it from nodejs.org, checks its SHA-256 before unpacking it, and installs it in
#     ~/.local/share/bayandocs/core-tools (or $BAYAN_TOOLS_DIR). A script cannot change the PATH of the shell that started
#     it, so it prints the line to add. On Windows, install Node.js 24.21.0 from nodejs.org yourself; CI does that in
#     .github/workflows/verify.yml.
#   - wasm-bindgen 0.2.129 (the command-line tool of the wasm-bindgen-cli crate, released 2026-09-25; MIT OR
#     Apache-2.0), which turns the engine's WebAssembly module into the package for the web shell (`cargo xtask
#     wasm-package`). It must equal the version of the wasm-bindgen crate in Cargo.lock; an xtask test checks that. A
#     wasm-bindgen of that version already on PATH, such as the BayanDocs cloud environment's, is used as it is.
#     Otherwise the script installs the project's release archive for this platform, checked against the SHA-256 pinned
#     below, into $CARGO_HOME/bin (default ~/.cargo/bin), which rustup puts on PATH. Only where no archive is pinned, or
#     the download fails, does it build the tool from crates.io instead (`cargo install --locked --no-default-features`:
#     149 crates and about a minute; Cargo checks every crate against the SHA-256 in the crates.io index). That build
#     compiles no C code, but it builds the crates of wasm-bindgen's test runner too, among them four that RustSec lists
#     as unmaintained (buf_redux, multipart, safemem, twoway) and one that was yanked (yoke-derive 0.8.3, which
#     `--locked` builds anyway); the release archives avoid all of that.
#   - Miri and the standard library's source for the pinned nightly toolchain (the BayanDocs cloud environment's
#     RUST_NIGHTLY), which `cargo xtask miri` uses to check bayan-ffi's pointer handling for undefined behaviour.
#     rustup checks every component against the SHA-256 in the toolchain's release manifest. `cargo miri setup` then
#     builds Miri's copy of the standard library, downloading the standard library's own dependencies from crates.io
#     at the versions of the toolchain's lockfile (outside bayan-core's Cargo.lock and cargo-deny).
#
# Usage: scripts/dev-setup.sh [--no-node] [--no-wasm-bindgen] [--no-miri]
# It must keep working with bash 3.2, the version macOS ships: no associative arrays, mapfile or case-changing expansions.
# The pins change only in the monthly dependency session (docs/plan/06-agent-workflow.md); xtask tests keep them equal to
# their other copies.
set -euo pipefail

NODE_VERSION=24.21.0 # Active LTS, released 2026-09-07; must equal NODE_VERSION in xtask/src/verify.rs and .github/workflows/verify.yml
WASM_BINDGEN_VERSION=0.2.129 # must equal the wasm-bindgen crate in Cargo.lock (xtask test: dev_setup_pins_the_locked_wasm_bindgen)
MIRI_TOOLCHAIN=nightly-2026-10-02 # must equal MIRI_TOOLCHAIN in xtask/src/artifacts.rs (xtask test: dev_setup_pins_the_miri_toolchain)

# The SHA-256 of each Node.js download, from nodejs.org's SHASUMS256.txt for v24.21.0, whose signature by the Node.js
# release key 5BE8 A3F6 C8A5 C01D 106C 0AD8 20B1 A390 B168 D356 was checked.
node_sha256() {
  case "$1" in
    linux-x64) echo fd8e59d5a511510f6a298afb548f18c7d2b1be404d8b4a27d94fbe49f56cb2d6 ;;
    darwin-arm64) echo 6239d4cf92d864487ec8cd3615038f7b67e7f58b77b21cd2f09ea9fbd68065fe ;;
    darwin-x64) echo 0ae5a24c24bb7d015cd816c5036b3f90f2945aa872fcf54e58da054753b3a299 ;;
    *) return 1 ;;
  esac
}

# The SHA-256 of wasm-bindgen's release archive for each platform, equal to the checksum the project publishes next to
# it (<archive>.sha256sum); every archive was downloaded and checked on 2026-10-08. The archive names carry the version,
# and the xtask test dev_setup_pins_the_locked_wasm_bindgen keeps it equal to WASM_BINDGEN_VERSION.
wasm_bindgen_sha256() {
  case "$1" in
    wasm-bindgen-0.2.129-x86_64-unknown-linux-musl.tar.gz) echo 82d12bb940e2d4e72e0d5605387fc1b8ca179044e012b620f0ce4e7440e8320e ;;
    wasm-bindgen-0.2.129-aarch64-unknown-linux-musl.tar.gz) echo 2ed4351c35dd9440308bbb02767d47ea278efe851a52465300f3c94f5b6c2a87 ;;
    wasm-bindgen-0.2.129-x86_64-apple-darwin.tar.gz) echo 7e028879a68ec53dae14048b55cc5215f6df75b17a9c1426fac8fa086d3202fd ;;
    wasm-bindgen-0.2.129-aarch64-apple-darwin.tar.gz) echo 81d4a23d56b3c3eb8187658329116d50e0b228a93b343825fb71f70179051cd1 ;;
    *) return 1 ;;
  esac
}

# Where the BayanDocs cloud environment's setup script (docs/scripts/cloud-environment-setup.sh) installs the same Node.js.
CLOUD_NODE_DIR=/opt/bayandocs/node/bin

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TOOLS_DIR="${BAYAN_TOOLS_DIR:-$HOME/.local/share/bayandocs/core-tools}"
want_node=true
want_wasm_bindgen=true
want_miri=true
work=""
node_work=""
node_path_hint=""

log() { printf '[dev-setup] %s\n' "$*"; }
die() {
  printf '[dev-setup] error: %s\n' "$*" >&2
  exit 1
}
cleanup() {
  if [ -n "$work" ]; then
    rm -rf "$work"
  fi
  if [ -n "$node_work" ]; then
    rm -rf "$node_work"
  fi
}
trap cleanup EXIT

while [ $# -gt 0 ]; do
  case "$1" in
    --no-node) want_node=false ;;
    --no-wasm-bindgen) want_wasm_bindgen=false ;;
    --no-miri) want_miri=false ;;
    -h | --help)
      awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "${BASH_SOURCE[0]}"
      exit 0
      ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
  shift
done

command -v rustup >/dev/null 2>&1 || die "rustup is not installed; see https://rustup.rs"
# Run cargo from the repository, so rustup uses the toolchain pinned in rust-toolchain.toml.
cd "$REPO_ROOT"

grep -q "^pub const NODE_VERSION: &str = \"$NODE_VERSION\";" "$REPO_ROOT/xtask/src/verify.rs" ||
  die "xtask/src/verify.rs does not pin Node.js $NODE_VERSION like this script; update both together"

# The version a program reports with --version, or nothing if it cannot run.
version_of() {
  "$1" --version 2>/dev/null || true
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

# Inside the repository, rustup reads rust-toolchain.toml and installs whatever of that toolchain is missing, the targets
# included. --no-self-update: by default this command also updates rustup itself to its newest release, which can be less
# than 24 hours old (ADR-0017: no silent upgrades). The flag applies to this command only and leaves the machine's rustup
# settings alone.
rustup toolchain install --no-self-update
log "Rust: $(rustc --version), with the components and targets of rust-toolchain.toml"

# The name nodejs.org gives this machine's build, or nothing for an unsupported machine.
node_target() {
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) echo linux-x64 ;;
    Darwin-arm64) echo darwin-arm64 ;;
    Darwin-x86_64) echo darwin-x64 ;;
    *) echo "" ;;
  esac
}

# Makes Node.js NODE_VERSION available, and sets node_path_hint when it is not on PATH yet.
install_node() {
  local target sha dir actual
  if [ "$(version_of node)" = "v$NODE_VERSION" ]; then
    log "Node.js $NODE_VERSION is already on PATH"
    return
  fi
  if [ "$(version_of "$CLOUD_NODE_DIR/node")" = "v$NODE_VERSION" ]; then
    log "Node.js $NODE_VERSION is installed by the BayanDocs cloud environment in $CLOUD_NODE_DIR"
    node_path_hint="$CLOUD_NODE_DIR"
    return
  fi
  target="$(node_target)"
  sha="$(node_sha256 "$target")" ||
    die "no pinned Node.js build for this machine; install Node.js $NODE_VERSION yourself (for example with nvm or fnm), then run this script again"
  dir="$TOOLS_DIR/node-v$NODE_VERSION-$target"
  if [ "$(version_of "$dir/bin/node")" != "v$NODE_VERSION" ]; then
    log "Node.js $NODE_VERSION: downloading and verifying the $target build"
    node_work="$(mktemp -d)"
    curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location --retry 3 --output "$node_work/node.tar.xz" \
      "https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-$target.tar.xz" ||
      die "could not download Node.js $NODE_VERSION"
    actual="$(sha256_of "$node_work/node.tar.xz")"
    [ "$actual" = "$sha" ] ||
      die "node-v$NODE_VERSION-$target.tar.xz has SHA-256 $actual, but $sha is pinned; nothing was installed"
    mkdir -p "$TOOLS_DIR"
    rm -rf "$dir"
    tar -xJf "$node_work/node.tar.xz" -C "$TOOLS_DIR" --no-same-owner
  fi
  log "Node.js $NODE_VERSION is installed in $dir"
  node_path_hint="$dir/bin"
}

if [ "$want_node" = true ]; then
  install_node
fi

# Installs wasm-bindgen into the folder $1: the pinned release archive for this platform, or else a build from crates.io.
install_wasm_bindgen() {
  local bin_dir=$1 target="" archive sha256 actual
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) target=x86_64-unknown-linux-musl ;;
    Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
    Darwin-x86_64) target=x86_64-apple-darwin ;;
    Darwin-arm64) target=aarch64-apple-darwin ;;
  esac
  archive="wasm-bindgen-$WASM_BINDGEN_VERSION-$target.tar.gz"
  if [ -n "$target" ] && sha256="$(wasm_bindgen_sha256 "$archive")"; then
    work="$(mktemp -d)"
    log "downloading $archive"
    if curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location --retry 3 --output "$work/$archive" \
      "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$WASM_BINDGEN_VERSION/$archive"; then
      actual="$(sha256_of "$work/$archive")"
      # A wrong checksum is never answered with a build from source: the download may have been tampered with.
      [ "$actual" = "$sha256" ] || die "$archive has SHA-256 $actual, but $sha256 is pinned; nothing was installed"
      # Only the one program is taken out of the archive.
      tar -xzf "$work/$archive" -C "$work" "${archive%.tar.gz}/wasm-bindgen"
      mkdir -p "$bin_dir"
      cp "$work/${archive%.tar.gz}/wasm-bindgen" "$bin_dir/wasm-bindgen"
      chmod 755 "$bin_dir/wasm-bindgen"
      return
    fi
    log "the download failed, so wasm-bindgen is built from crates.io instead"
  else
    log "no release archive is pinned for $(uname -s) $(uname -m), so wasm-bindgen is built from crates.io"
  fi
  # Without default features: they only add the TLS stack of wasm-bindgen-test-runner, which needs ring's C code.
  cargo install --locked --no-default-features --version "$WASM_BINDGEN_VERSION" --root "${bin_dir%/bin}" wasm-bindgen-cli
}

if [ "$want_wasm_bindgen" = true ]; then
  bin_dir="${CARGO_HOME:-$HOME/.cargo}/bin"
  wanted="wasm-bindgen $WASM_BINDGEN_VERSION"
  on_path="$(command -v wasm-bindgen 2>/dev/null || true)"
  if [ -n "$on_path" ] && [ "$(version_of "$on_path")" = "$wanted" ]; then
    log "$wanted is already installed: $on_path"
  else
    found=""
    if [ -x "$bin_dir/wasm-bindgen" ]; then
      found="$(version_of "$bin_dir/wasm-bindgen")"
    fi
    if [ -n "$found" ] && [ "$found" != "$wanted" ]; then
      # Another project may need the version that is there, so it is not replaced without asking.
      die "$bin_dir/wasm-bindgen is ${found#wasm-bindgen }, but bayan-core needs $WASM_BINDGEN_VERSION. Remove it and run this script again."
    fi
    if [ -z "$found" ]; then
      install_wasm_bindgen "$bin_dir"
      found="$(version_of "$bin_dir/wasm-bindgen")"
      [ "$found" = "$wanted" ] || die "the installed wasm-bindgen reports '$found'"
      log "installed $wanted in $bin_dir"
    else
      log "$wanted is already installed in $bin_dir"
    fi
    # cargo xtask wasm-package runs the first wasm-bindgen on PATH.
    on_path="$(command -v wasm-bindgen 2>/dev/null || true)"
    if [ -z "$on_path" ]; then
      log "note: put $bin_dir on PATH, so that cargo xtask wasm-package finds wasm-bindgen"
    elif [ "$on_path" != "$bin_dir/wasm-bindgen" ]; then
      log "note: $on_path comes first on PATH and is $(version_of "$on_path"); put $bin_dir before it"
    fi
  fi
fi

if [ "$want_miri" = true ]; then
  log "installing Miri for $MIRI_TOOLCHAIN (rustup skips what is already there)"
  rustup toolchain install "$MIRI_TOOLCHAIN" --profile minimal --component miri --component rust-src --no-self-update
  # Builds Miri's own copy of the standard library once, so the first test run does not have to.
  cargo "+$MIRI_TOOLCHAIN" miri setup
  log "Miri is ready: cargo xtask miri"
fi

if [ -n "$node_path_hint" ]; then
  log "put Node.js $NODE_VERSION first on PATH before running the gate, for example: export PATH=\"$node_path_hint:\$PATH\""
fi
log "done. Run 'cargo xtask verify' to check everything."
