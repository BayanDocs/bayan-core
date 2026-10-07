#!/usr/bin/env bash
# Installs the extra tools that bayan-core's benchmarks need beyond the verification gate, and checks the ones it expects to find.
#
# The gate itself (`cargo xtask verify`) needs only rustup, which installs the toolchain pinned in rust-toolchain.toml by itself, and cargo-deny (see AGENTS.md). The CRDT benchmarks of work package CORE-004 (spikes/crdt-model/REPORT.md) also run the engine as WebAssembly under Node.js, through Node.js's built-in WASI support, so they need:
#   - the Rust standard library for the wasm32-wasip1 target, for the toolchain pinned in rust-toolchain.toml. rustup downloads it from static.rust-lang.org and checks it against the SHA-256 hash in the signed release manifest of that exact Rust version (ADR-0017), the same way it installs the toolchain itself;
#   - Node.js 20 or newer (for `node:wasi` with `version: "preview1"`). This script does not install Node.js: BayanDocs cloud sessions already have the pinned version (docs/scripts/cloud-environment-setup.sh), and on other machines use the one you have, as long as it is 20 or newer.
# It is idempotent: rustup does nothing when the target is already installed.
#
# Usage: scripts/dev-setup.sh
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

die() { echo "dev-setup: $*" >&2; exit 1; }
say() { echo "dev-setup: $*"; }

case "${1:-}" in
  "") ;;
  -h|--help) awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "${BASH_SOURCE[0]}"; exit 0 ;;
  *) die "unknown option: $1 (see --help)" ;;
esac

command -v rustup >/dev/null 2>&1 || die "rustup is not installed (https://rustup.rs); it installs the pinned Rust toolchain"

# The pinned toolchain: the `channel` line of rust-toolchain.toml, for example `channel = "1.99.0"`.
toolchain=$(sed -n 's/^channel *= *"\([^"]*\)".*/\1/p' "$root/rust-toolchain.toml")
[ -n "$toolchain" ] || die "cannot read the toolchain channel from rust-toolchain.toml"

# Never let rustup update itself as a side effect (ADR-0017: no silent upgrades).
rustup set auto-self-update disable >/dev/null
rustup toolchain install "$toolchain" --profile minimal --no-self-update >/dev/null
rustup target add --toolchain "$toolchain" wasm32-wasip1
say "Rust $toolchain has the wasm32-wasip1 target (for the WebAssembly benchmarks)"

if command -v node >/dev/null 2>&1; then
  node_version=$(node --version)
  node_major=${node_version#v}
  node_major=${node_major%%.*}
  if [ "$node_major" -ge 20 ] 2>/dev/null; then
    say "Node.js $node_version runs the WebAssembly benchmarks"
  else
    die "Node.js $node_version is too old for the WebAssembly benchmarks; they need Node.js 20 or newer"
  fi
else
  die "Node.js is not installed; the WebAssembly benchmarks need Node.js 20 or newer"
fi
