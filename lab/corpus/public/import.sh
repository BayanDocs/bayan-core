#!/usr/bin/env bash
# Builds the public corpus (tier T1) from its sources, reproducibly: clones each source at its pinned revision, registers
# it with its license and notice files, and adds every .docx file of the source that the exclusion list (excluded.tsv)
# does not name. The result is lab/corpus/public/manifest.json and lab/corpus/public/frequency-report.md.
#
# Usage: lab/corpus/public/import.sh WORK_DIR STORE
#   WORK_DIR  a folder for the sources' checkouts, outside this repository (about 120 MB; created if missing, and
#             reused by later runs)
#   STORE     where the documents and license texts go: a local folder, or an S3 address as bayan-lab's --store takes
#             it (s3://BUCKET/PREFIX?endpoint=https://HOST&region=REGION, with the credentials in AWS_ACCESS_KEY_ID and
#             AWS_SECRET_ACCESS_KEY)
#
# Run against the committed manifest, the script changes nothing in it as long as the sources, the exclusion list and
# the tagger are unchanged, but it stores every document and license text the store lacks: that is how a new store,
# such as the public bucket, is filled. `git diff lab/corpus/public` afterwards shows what changed. To build the corpus
# from nothing, delete manifest.json first.
#
# Set BAYAN_LAB to the path of a bayan-lab binary to use it; otherwise the script builds one with Cargo. The sources are
# fetched from GitHub with git (a shallow, blob-less, sparse clone: only the .docx files and the license files are
# downloaded). Every source revision was at least 24 hours old when it was pinned, like every dependency (ADR-0017).
# git checks out no symbolic links here, only regular files are added, and bayan-lab refuses any path through a link,
# so a source cannot lead the import to a file outside its checkout.
#
# It must keep working with bash 3.2, the version macOS ships: no associative arrays, mapfile or case-changing
# expansions.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
MANIFEST="$HERE/manifest.json"
EXCLUSIONS="$HERE/excluded.tsv"
REPORT="$HERE/frequency-report.md"
# The report's title; tests/public_corpus.rs uses the same one to check that the committed report is current.
TITLE="Public corpus v1: frequency report"

log() { printf '[import] %s\n' "$*"; }
die() {
  printf '[import] error: %s\n' "$*" >&2
  exit 1
}

case "${1:-}" in
  -h | --help)
    awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "${BASH_SOURCE[0]}"
    exit 0
    ;;
esac
[ $# -eq 2 ] || die "usage: $0 WORK_DIR STORE (see --help)"
WORK_DIR="$1"
STORE="$2"
mkdir -p "$WORK_DIR"
WORK_DIR="$(cd "$WORK_DIR" && pwd)"
case "$WORK_DIR/" in
  "$REPO_ROOT/"*) die "WORK_DIR must be outside the repository" ;;
esac

command -v git >/dev/null 2>&1 || die "git is not installed"
if [ -n "${BAYAN_LAB:-}" ]; then
  LAB="$BAYAN_LAB"
else
  log "building bayan-lab"
  (cd "$REPO_ROOT" && cargo build --quiet --release --locked -p bayan-lab)
  LAB="${CARGO_TARGET_DIR:-$REPO_ROOT/target}/release/bayan-lab"
fi
[ -x "$LAB" ] || die "no bayan-lab binary at $LAB; set BAYAN_LAB to its path"

# fetch NAME URL REVISION LICENSE_FILE... checks out the source's .docx files and license files at REVISION in
# WORK_DIR/NAME.
fetch() {
  local name="$1" url="$2" revision="$3"
  shift 3
  local dir="$WORK_DIR/$name"
  if [ ! -d "$dir/.git" ]; then
    git init --quiet "$dir"
    git -C "$dir" remote add origin "$url"
  fi
  [ "$(git -C "$dir" remote get-url origin)" = "$url" ] || die "$dir is a clone of another repository"
  # No symbolic links (a link is checked out as a small text file instead), and no line-ending conversion, so the
  # files are exactly the bytes in Git.
  git -C "$dir" config core.symlinks false
  git -C "$dir" config core.autocrlf false
  local patterns=('*.[Dd][Oo][Cc][Xx]')
  local file
  for file in "$@"; do
    patterns+=("/$file")
  done
  git -C "$dir" sparse-checkout set --no-cone "${patterns[@]}"
  git -C "$dir" fetch --quiet --depth 1 --filter=blob:none origin "$revision"
  git -C "$dir" -c advice.detachedHead=false checkout --quiet --force --detach "$revision"
  [ "$(git -C "$dir" rev-parse HEAD)" = "$revision" ] || die "$name is not at revision $revision"
}

# import_source NAME URL REVISION LICENSE COPYRIGHT NOTES LICENSE_FILE... registers the source and adds its documents.
import_source() {
  local name="$1" url="$2" revision="$3" license="$4" copyright="$5" notes="$6"
  shift 6
  log "$name: fetching $url at $revision"
  fetch "$name" "$url" "$revision" "$@"
  local dir="$WORK_DIR/$name"
  local license_args=()
  local file
  for file in "$@"; do
    if [ ! -f "$dir/$file" ] || [ -L "$dir/$file" ]; then
      die "$name has no license file $file"
    fi
    license_args+=(--license-file "$file=$dir/$file")
  done
  "$LAB" corpus source add "$name" --manifest "$MANIFEST" --store "$STORE" \
    --url "$url" --revision "$revision" --license "$license" --copyright "$copyright" --notes "$notes" \
    "${license_args[@]}"
  # Every .docx file that Git records as a regular file (mode 100644 or 100755), in Git's order.
  local list="$WORK_DIR/$name.files"
  local tab
  tab="$(printf '\t')"
  git -C "$dir" ls-files -z --stage -- ':(icase)*.docx' |
    while IFS= read -r -d '' entry; do
      case "$entry" in
        100644\ * | 100755\ *) printf '%s\0' "${entry#*"$tab"}" ;;
        *) log "$name: skipping ${entry#*"$tab"}: not a regular file in Git" >&2 ;;
      esac
    done >"$list"
  [ -s "$list" ] || die "$name has no .docx files"
  log "$name: adding documents"
  # xargs hands the paths over in batches that fit the system's command-line limit, and fails if any batch fails.
  xargs -0 "$LAB" corpus add --manifest "$MANIFEST" --store "$STORE" --source "$name" --root "$dir" \
    --exclude "$EXCLUSIONS" -- <"$list"
}

REVIEW="Every file was reviewed for content from third parties that this license may not cover (LAB-001); lab/corpus/public/excluded.tsv lists the files kept out and why."

# The sources, in the order in which they are added: a document found in several sources takes the license of the
# first. Changing a revision makes a new source (register it under a new name, such as python-docx-2): sources never
# change.
import_source python-docx https://github.com/python-openxml/python-docx \
  e45454602b53e8e572b179ccf1c91093ec9f4ed7 MIT \
  "Copyright (c) 2013 Steve Canny, https://github.com/scanny" \
  "Test and example documents of python-docx, under the repository's MIT license. $REVIEW" \
  LICENSE
import_source mammoth-js https://github.com/mwilliamson/mammoth.js \
  791a189230977143cde64dbf9f1d24dce96ac8a1 BSD-2-Clause \
  "Copyright (c) 2013, Michael Williamson" \
  "Test documents of mammoth.js, under the repository's BSD-2-Clause license. $REVIEW" \
  LICENSE
import_source open-xml-sdk https://github.com/dotnet/Open-XML-SDK \
  431ab05cf160248cc3885a4a766026d4f8243792 MIT \
  "Copyright (c) .NET Foundation and Contributors" \
  "Test documents of the Open XML SDK, under the repository's MIT license. $REVIEW" \
  LICENSE NOTICE
import_source open-xml-powertools https://github.com/OfficeDev/Open-Xml-PowerTools \
  5881422a881f6ccefce2b9801b5dc6a753670d6e MIT \
  "Copyright (c) Microsoft Corporation" \
  "Test and example documents of Open-Xml-PowerTools (archived by Microsoft in 2019), under the repository's MIT license. $REVIEW" \
  LICENSE
import_source docx4j https://github.com/plutext/docx4j \
  0e8e7633ef46012e0d3603339728b83ed21d5045 Apache-2.0 \
  "Copyright 2007-2025 Plutext Pty Ltd" \
  "Test and sample documents of docx4j, under the repository's Apache-2.0 license, whose NOTICE file is kept with it. $REVIEW" \
  legals/LICENSE legals/NOTICE
import_source apache-poi https://github.com/apache/poi \
  ae62bb5116b9aee19ebd5834e3a82066132c9f7f Apache-2.0 \
  "Copyright 2003-2026 The Apache Software Foundation" \
  "Test documents of Apache POI, under the repository's Apache-2.0 license, whose NOTICE file is kept with it. $REVIEW" \
  legal/LICENSE legal/NOTICE
import_source apache-tika https://github.com/apache/tika \
  63284c17c0ee28223c4e8b736b49188fb2585ec7 Apache-2.0 \
  "Copyright 2007-2026 The Apache Software Foundation" \
  "Test documents of Apache Tika, under the repository's Apache-2.0 license, whose NOTICE file is kept with it. $REVIEW" \
  LICENSE.txt NOTICE.txt

log "verifying the corpus"
"$LAB" corpus verify --manifest "$MANIFEST" --store "$STORE" --exclude "$EXCLUSIONS"
"$LAB" corpus stats --manifest "$MANIFEST" --title "$TITLE" >"$REPORT.partial"
mv "$REPORT.partial" "$REPORT"
log "done: $MANIFEST and $REPORT are up to date"
