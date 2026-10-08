#!/usr/bin/env bash
# Sets up the tools that bayan-core's verification gate (`cargo xtask verify`) needs besides Cargo itself, at pinned versions with verified checksums (ADR-0017):
#   1. The Rust toolchain pinned in rust-toolchain.toml, with the components and targets it lists, among them wasm32-wasip1 for the tests in
#      WebAssembly. rustup checks every download against the checksums of the Rust release.
#   2. Node.js, which runs those tests (NODE_VERSION below). Uses a node of exactly that version if one is on PATH, then the one the BayanDocs
#      cloud environment installs, and otherwise downloads it from nodejs.org and checks its SHA-256 before unpacking it.
# cargo-deny, the gate's other tool, is installed as AGENTS.md describes.
#
# It is idempotent: each step checks whether its result is already in place and skips the work if so. A script cannot change the PATH of the
# shell that started it, so when Node.js is not on PATH yet, it prints the line to add.
#
# Supported platforms: Ubuntu 24.04 on x86-64, and macOS 14 or later on Apple Silicon or Intel. On Windows, install Node.js NODE_VERSION from
# nodejs.org yourself; CI does that in .github/workflows/verify.yml.
# Written for bash 3.2, the version macOS ships: no associative arrays, mapfile or case-changing expansions, and no arrays at all.
#
# Usage: scripts/dev-setup.sh
#
# The pins change only in the monthly dependency session (docs/plan/06-agent-workflow.md), together with NODE_VERSION in xtask/src/verify.rs
# and .github/workflows/verify.yml; a test of xtask keeps the three versions equal.
set -euo pipefail

NODE_VERSION=24.21.0 # Active LTS, released 2026-09-07; must equal NODE_VERSION in xtask/src/verify.rs and .github/workflows/verify.yml

# SHA-256 of each Node.js download, from nodejs.org's SHASUMS256.txt for v24.21.0, whose signature by the Node.js release key
# 5BE8 A3F6 C8A5 C01D 106C 0AD8 20B1 A390 B168 D356 was checked.
node_sha256() {
  case "$1" in
    linux-x64) echo fd8e59d5a511510f6a298afb548f18c7d2b1be404d8b4a27d94fbe49f56cb2d6 ;;
    darwin-arm64) echo 6239d4cf92d864487ec8cd3615038f7b67e7f58b77b21cd2f09ea9fbd68065fe ;;
    darwin-x64) echo 0ae5a24c24bb7d015cd816c5036b3f90f2945aa872fcf54e58da054753b3a299 ;;
    *) return 1 ;;
  esac
}

# Where the BayanDocs cloud environment's setup script (docs/scripts/cloud-environment-setup.sh) installs the same Node.js.
CLOUD_NODE_DIR=/opt/bayandocs/node/bin

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TOOLS_DIR="${BAYAN_TOOLS_DIR:-$HOME/.local/share/bayandocs/core-tools}"
TMP_ROOT=""
PATH_HINT=""

log() { printf 'dev-setup: %s\n' "$*"; }
die() {
  printf 'dev-setup: error: %s\n' "$*" >&2
  exit 1
}
cleanup() { if [ -n "$TMP_ROOT" ]; then rm -rf "$TMP_ROOT"; fi; }
trap cleanup EXIT

# The name nodejs.org gives this machine's build, or nothing for an unsupported machine.
host_target() {
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) echo linux-x64 ;;
    Darwin-arm64) echo darwin-arm64 ;;
    Darwin-x86_64) echo darwin-x64 ;;
    *) echo "" ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

# Downloads the URL $1 to the file $3 and checks that its SHA-256 is $2.
fetch_verified() {
  curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location --retry 3 --output "$3" "$1" ||
    die "could not download $1"
  local actual
  actual="$(sha256_of "$3")"
  [ "$actual" = "$2" ] || die "checksum mismatch for $1: expected $2, got $actual"
}

# The version a node program reports, or nothing if it cannot run.
version_of() {
  "$1" --version 2>/dev/null || true
}

check_pins_match_repository() {
  grep -q "^pub const NODE_VERSION: &str = \"$NODE_VERSION\";" "$REPO_ROOT/xtask/src/verify.rs" ||
    die "xtask/src/verify.rs does not pin Node.js $NODE_VERSION like this script; update both together"
}

setup_rust() {
  command -v rustup >/dev/null 2>&1 || die "rustup is not installed; install it from https://rustup.rs, then run this script again"
  # Inside the repository, rustup reads rust-toolchain.toml and installs whatever of that toolchain is missing.
  # --no-self-update: by default this command also updates rustup itself to its newest release, which can be less than 24 hours old (ADR-0017: no silent upgrades). The flag applies to this command only and leaves the machine's rustup settings alone.
  (cd "$REPO_ROOT" && rustup toolchain install --no-self-update)
  log "Rust: $(cd "$REPO_ROOT" && rustc --version), with the components and targets of rust-toolchain.toml"
}

setup_node() {
  if [ "$(version_of node)" = "v$NODE_VERSION" ]; then
    log "Node.js $NODE_VERSION: already on PATH"
    return
  fi
  if [ "$(version_of "$CLOUD_NODE_DIR/node")" = "v$NODE_VERSION" ]; then
    log "Node.js $NODE_VERSION: installed by the BayanDocs cloud environment in $CLOUD_NODE_DIR"
    PATH_HINT="$CLOUD_NODE_DIR"
    return
  fi
  local target sha dir
  target="$(host_target)"
  sha="$(node_sha256 "$target")" ||
    die "no pinned Node.js build for this machine; install Node.js $NODE_VERSION yourself (for example with nvm or fnm), then run this script again"
  dir="$TOOLS_DIR/node-v$NODE_VERSION-$target"
  if [ "$(version_of "$dir/bin/node")" != "v$NODE_VERSION" ]; then
    log "Node.js $NODE_VERSION: downloading and verifying the $target build"
    TMP_ROOT="$(mktemp -d)"
    fetch_verified "https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-$target.tar.xz" "$sha" "$TMP_ROOT/node.tar.xz"
    mkdir -p "$TOOLS_DIR"
    rm -rf "$dir"
    tar -xJf "$TMP_ROOT/node.tar.xz" -C "$TOOLS_DIR" --no-same-owner
  fi
  log "Node.js $NODE_VERSION: installed in $dir"
  PATH_HINT="$dir/bin"
}

main() {
  [ "$#" -eq 0 ] || die "usage: scripts/dev-setup.sh (it takes no arguments)"
  check_pins_match_repository
  setup_rust
  setup_node
  if [ -n "$PATH_HINT" ]; then
    log "put Node.js $NODE_VERSION first on PATH before running the gate, for example: export PATH=\"$PATH_HINT:\$PATH\""
  fi
  log "done. Run 'cargo xtask verify' to check everything."
}

main "$@"
