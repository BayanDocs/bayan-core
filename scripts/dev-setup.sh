#!/usr/bin/env bash
# Installs the tools that bayan-core needs besides the Rust toolchain (rustup installs it from rust-toolchain.toml) and
# cargo-deny (see AGENTS.md), at pinned versions, and says what it did. It runs on developer machines (Ubuntu 24.04 and
# macOS) and in BayanDocs cloud sessions, and is idempotent: a tool that is already installed at its pinned version is
# left alone.
#
#   - wasm-bindgen 0.2.129 (the command-line tool of the wasm-bindgen-cli crate, released 2026-09-25; MIT OR
#     Apache-2.0), which turns the engine's WebAssembly module into the package for the web shell (`cargo xtask
#     wasm-package`). It must equal the version of the wasm-bindgen crate in Cargo.lock; an xtask test checks that. A
#     wasm-bindgen of that version already on PATH, such as the BayanDocs cloud environment's, is used as it is.
#     Otherwise the script installs the project's release archive for this platform, checked against the SHA-256 pinned
#     below, into $CARGO_HOME/bin (default ~/.cargo/bin), which rustup puts on PATH. Only where no archive is pinned, or
#     the download fails, does it build the tool from crates.io instead (`cargo install --locked --no-default-features`:
#     149 crates and about a minute; Cargo checks every crate against the SHA-256 in the crates.io index). That build
#     compiles no C code, but four of its crates, which only wasm-bindgen's test runner uses, are listed by RustSec as
#     unmaintained (buf_redux, multipart, safemem, twoway); the release archives avoid it.
#   - Miri and the standard library's source for the pinned nightly toolchain (the BayanDocs cloud environment's
#     RUST_NIGHTLY), which `cargo xtask miri` uses to check bayan-ffi's pointer handling for undefined behaviour.
#     rustup checks every component against the SHA-256 in the toolchain's release manifest. `cargo miri setup` then
#     builds Miri's copy of the standard library, downloading the standard library's own dependencies from crates.io
#     at the versions of the toolchain's lockfile (outside bayan-core's Cargo.lock and cargo-deny).
#
# Usage: scripts/dev-setup.sh [--no-wasm-bindgen] [--no-miri]
# It must keep working with bash 3.2, the version macOS ships.
set -euo pipefail

WASM_BINDGEN_VERSION=0.2.129 # must equal the wasm-bindgen crate in Cargo.lock (xtask test: dev_setup_pins_the_locked_wasm_bindgen)
MIRI_TOOLCHAIN=nightly-2026-10-02 # must equal MIRI_TOOLCHAIN in xtask/src/artifacts.rs (xtask test: dev_setup_pins_the_miri_toolchain)

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

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
want_wasm_bindgen=true
want_miri=true
work=""

log() { printf '[dev-setup] %s\n' "$*"; }
die() {
  printf '[dev-setup] error: %s\n' "$*" >&2
  exit 1
}
cleanup() {
  if [ -n "$work" ]; then
    rm -rf "$work"
  fi
}
trap cleanup EXIT

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

# The version a wasm-bindgen program reports, or nothing.
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

log "done"
