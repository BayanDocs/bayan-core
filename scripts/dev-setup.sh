#!/usr/bin/env bash
# Installs the tools that bayan-core needs besides the Rust toolchain (rustup installs it from rust-toolchain.toml) and
# cargo-deny (see AGENTS.md), at pinned versions, and says what it did. It runs on developer machines (Ubuntu 24.04 and
# macOS), in BayanDocs cloud sessions and in CI, and is idempotent: a tool that is already installed at its pinned
# version is left alone.
#
#   - wasm-bindgen-cli 0.2.129 (published 2026-09-25; MIT OR Apache-2.0), which turns the engine's WebAssembly module into
#     the package for the web shell (`cargo xtask wasm-package`). It must equal the version of the wasm-bindgen crate in
#     Cargo.lock; an xtask test checks that. It is built from crates.io with `cargo install --locked`: Cargo checks every
#     crate it downloads against the SHA-256 in the crates.io index, and --locked builds exactly the dependency versions
#     the release was published with. It goes to $CARGO_HOME/bin (default ~/.cargo/bin), which rustup puts on PATH.
#   - Miri and the standard library's source for the pinned nightly toolchain (the BayanDocs cloud environment's
#     RUST_NIGHTLY), which `cargo xtask miri` uses to check bayan-ffi's pointer handling for undefined behaviour.
#     rustup checks every component against the SHA-256 in the toolchain's release manifest.
#
# Usage: scripts/dev-setup.sh [--no-wasm-bindgen] [--no-miri]
# It must keep working with bash 3.2, the version macOS ships.
set -euo pipefail

WASM_BINDGEN_VERSION=0.2.129 # must equal the wasm-bindgen crate in Cargo.lock (xtask test: dev_setup_pins_the_locked_wasm_bindgen)
MIRI_TOOLCHAIN=nightly-2026-10-02 # must equal MIRI_TOOLCHAIN in xtask/src/artifacts.rs (xtask test: dev_setup_pins_the_miri_toolchain)

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
want_wasm_bindgen=true
want_miri=true

log() { printf '[dev-setup] %s\n' "$*"; }
die() {
  printf '[dev-setup] error: %s\n' "$*" >&2
  exit 1
}

while [ $# -gt 0 ]; do
  case "$1" in
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

if [ "$want_wasm_bindgen" = true ]; then
  bin_dir="${CARGO_HOME:-$HOME/.cargo}/bin"
  found=""
  if [ -x "$bin_dir/wasm-bindgen" ]; then
    found="$("$bin_dir/wasm-bindgen" --version 2>/dev/null || true)"
  fi
  if [ "$found" = "wasm-bindgen $WASM_BINDGEN_VERSION" ]; then
    log "wasm-bindgen $WASM_BINDGEN_VERSION is already installed in $bin_dir"
  elif [ -n "$found" ]; then
    # Another project may need the version that is there, so it is not replaced without asking.
    die "$bin_dir/wasm-bindgen is ${found#wasm-bindgen }, but bayan-core needs $WASM_BINDGEN_VERSION. Replace it with: cargo install --locked --force --version $WASM_BINDGEN_VERSION wasm-bindgen-cli"
  else
    log "building wasm-bindgen-cli $WASM_BINDGEN_VERSION from crates.io (takes a few minutes)"
    cargo install --locked --version "$WASM_BINDGEN_VERSION" wasm-bindgen-cli
    found="$("$bin_dir/wasm-bindgen" --version)"
    [ "$found" = "wasm-bindgen $WASM_BINDGEN_VERSION" ] || die "the installed wasm-bindgen reports '$found'"
    log "installed wasm-bindgen $WASM_BINDGEN_VERSION in $bin_dir"
  fi
  case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) log "note: put $bin_dir on PATH, so that cargo xtask wasm-package finds wasm-bindgen" ;;
  esac
fi

if [ "$want_miri" = true ]; then
  log "installing Miri for $MIRI_TOOLCHAIN (rustup skips what is already there)"
  rustup toolchain install "$MIRI_TOOLCHAIN" --profile minimal --component miri --component rust-src --no-self-update
  # Builds Miri's own copy of the standard library once, so the first test run does not have to.
  cargo "+$MIRI_TOOLCHAIN" miri setup
  log "Miri is ready: cargo xtask miri"
fi

log "done"
