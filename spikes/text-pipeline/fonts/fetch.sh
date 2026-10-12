#!/usr/bin/env bash
# Puts the fonts of the CORE-003 probe set into one folder, each checked against the SHA-256 in manifest.txt next to this
# script. It downloads the fonts that are not committed (about 15 MB), copies the committed ones, and leaves alone a font
# that is already there with the right checksum, so it is safe to run again. Nothing is moved into the folder before its
# checksum is checked: a download is checked before anything is read from it, and a font taken out of an archive is
# checked again. A font that is there with another checksum is replaced.
#
# Usage: spikes/text-pipeline/fonts/fetch.sh [<folder>]   (default: target/text-pipeline/fonts in the repository)
# Needs bash (3.2 or later, the version macOS ships), curl, tar and sha256sum or shasum; Git Bash on Windows has them all.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
manifest="$here/manifest.txt"
dest="${1:-$here/../../../target/text-pipeline/fonts}"
work=""

log() { printf '[fetch-fonts] %s\n' "$*"; }
die() {
  printf '[fetch-fonts] error: %s\n' "$*" >&2
  exit 1
}
cleanup() {
  if [ -n "$work" ]; then
    rm -rf "$work"
  fi
}
trap cleanup EXIT

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

# The size of a file in bytes, without stat, whose options differ between Linux, macOS and Git Bash.
size_of() {
  wc -c <"$1" | tr -d ' '
}

# Checks that the file $1 has the SHA-256 $2 and the size $3; $4 names it in the message.
check() {
  local actual
  actual="$(sha256_of "$1")"
  [ "$actual" = "$2" ] || die "$4 has SHA-256 $actual, but manifest.txt pins $2"
  [ "$(size_of "$1")" = "$3" ] || die "$4 has $(size_of "$1") bytes, but manifest.txt pins $3"
}

# Downloads into the file $1 from the first of the URLs $3… that works, and checks it against the SHA-256 $2 and the
# size in bytes $4 (given before the URLs). A download with another checksum is never answered with the next mirror: it
# may have been tampered with.
download() {
  local out=$1 sha=$2 bytes=$3 url
  shift 3
  for url in "$@"; do
    log "downloading $url"
    if curl --proto '=https' --proto-redir '=https' --tlsv1.2 --fail --silent --show-error --location --retry 3 \
      --output "$out" "$url"; then
      check "$out" "$sha" "$bytes" "$(basename "$out") from $url"
      return 0
    fi
    log "could not download $url"
  done
  die "could not download $(basename "$out") from any of its URLs"
}

[ -f "$manifest" ] || die "cannot find $manifest"
mkdir -p "$dest"
dest="$(cd "$dest" && pwd)"
# Inside the destination, so that moving a checked file into place is a rename that cannot leave half a file behind.
work="$(mktemp -d "$dest/.fetch.XXXXXX")"

# The archives' lines, kept until a font needs one of them.
archives=""
while read -r kind file sha bytes source rest; do
  if [ "$kind" = archive ]; then
    archives="$archives$file $sha $bytes $source $rest
"
  fi
done <"$manifest"

# Makes sure that the archive $1 has been downloaded into the work folder and checked.
need_archive() {
  local name=$1 file sha bytes urls
  [ -f "$work/$name" ] && return 0
  while read -r file sha bytes urls; do
    if [ "$file" = "$name" ]; then
      # shellcheck disable=SC2086 # the URLs are separate words on purpose
      download "$work/$name" "$sha" "$bytes" $urls
      return 0
    fi
  done <<EOF
$archives
EOF
  die "manifest.txt names the archive $name, but has no archive line for it"
}

count=0
while read -r kind file sha bytes source rest; do
  [ "$kind" = font ] || continue
  count=$((count + 1))
  case "$file" in
    */* | .* | "") die "manifest.txt names a font file '$file', which is not a plain file name" ;;
  esac
  if [ -f "$dest/$file" ] && [ "$(sha256_of "$dest/$file")" = "$sha" ]; then
    log "$file is already there"
    continue
  fi
  case "$source" in
    committed)
      cp "$here/$file" "$work/$file"
      ;;
    https://*)
      download "$work/$file" "$sha" "$bytes" "$source"
      ;;
    *:*)
      archive="${source%%:*}"
      member="${source#*:}"
      need_archive "$archive"
      mkdir -p "$work/extract"
      tar -xzf "$work/$archive" -C "$work/extract" --no-same-owner "$member"
      mv "$work/extract/$member" "$work/$file"
      ;;
    *) die "manifest.txt gives $file the source '$source', which is not a URL, an archive member or 'committed'" ;;
  esac
  check "$work/$file" "$sha" "$bytes" "$file"
  mv -f "$work/$file" "$dest/$file"
  log "$file: checked and in place"
done <"$manifest"

[ "$count" -gt 0 ] || die "manifest.txt lists no fonts"
log "all $count fonts are in $dest"
