#!/usr/bin/env bash
# Writes, checks and summarizes the SHA-256 checksums of an artifact folder, for .github/workflows/artifacts.yml (CORE-007).
#
#   scripts/ci-checksums.sh <folder>                              writes <folder>/SHA256SUMS: one line per file, sorted
#   scripts/ci-checksums.sh --check <folder>                      checks every file against <folder>/SHA256SUMS
#   scripts/ci-checksums.sh --summary <name> <digest> <folder>    prints a Markdown summary for the job summary
#
# Linux and Git Bash on Windows have sha256sum; macOS has shasum instead. It must keep working with bash 3.2.
set -euo pipefail

die() {
  printf 'ci-checksums: %s\n' "$*" >&2
  exit 1
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi
}

case "${1:-}" in
  --check)
    [ $# -eq 2 ] || die "usage: $0 --check <folder>"
    cd "$2"
    [ -f SHA256SUMS ] || die "$2 has no SHA256SUMS"
    sha256 -c SHA256SUMS
    ;;
  --summary)
    [ $# -eq 4 ] || die "usage: $0 --summary <name> <digest> <folder>"
    tick='`'
    printf '### %s\n\nSHA-256 of the artifact as GitHub stores it: %s%s%s\n\n' "$2" "$tick" "${3:-(not reported)}" "$tick"
    printf '<details><summary>SHA-256 of each file (SHA256SUMS, inside the artifact)</summary>\n\n```\n'
    cat "$4/SHA256SUMS"
    printf '```\n\n</details>\n'
    ;;
  -* | '')
    die "usage: $0 <folder> | --check <folder> | --summary <name> <digest> <folder>"
    ;;
  *)
    [ $# -eq 1 ] || die "usage: $0 <folder>"
    cd "$1"
    list="$(mktemp)"
    trap 'rm -f "$list"' EXIT
    # Binary mode (-b) gives the same line format, "<hash> *<file>", on every platform.
    find . -type f ! -name SHA256SUMS | sed 's|^\./||' | LC_ALL=C sort | while IFS= read -r file; do
      sha256 -b "$file"
    done >"$list"
    [ -s "$list" ] || die "$1 has no files"
    mv "$list" SHA256SUMS
    trap - EXIT
    printf 'ci-checksums: wrote %s/SHA256SUMS (%s files)\n' "$1" "$(wc -l <SHA256SUMS | tr -d ' ')"
    ;;
esac
