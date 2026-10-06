#!/usr/bin/env bash
# Prints the section of CHANGELOG.md for one version, without its heading: what the GitHub
# release of that version says. Fails when the changelog has no section for the version, or
# an empty one.
#
#   tools/release/notes.sh 0.1.14      (or v0.1.14)
#
# GitHub breaks the lines of a release where its text does, so the lines of a paragraph or of
# a list item, which the changelog wraps, are joined into one.
set -euo pipefail
cd "$(dirname "$0")/../.."

if [ $# -ne 1 ]; then
  echo "usage: tools/release/notes.sh <version>" >&2
  exit 2
fi
version="${1#v}"

# A section starts at `## <version>` and ends at the next `## `. The blank lines around it
# are left out. A heading, a list item and a table row start a line; a code block is kept as
# written.
if ! awk -v version="$version" '
  function flush() {
    if (line == "") return
    printf "%s%s\n", blank, line
    line = ""; blank = ""; started = 1
  }
  /^## / { if (inside) exit; inside = ($2 == version); next }
  !inside { next }
  fenced || /^[ \t]*```/ {
    flush()
    printf "%s%s\n", blank, $0
    blank = ""; started = 1
    if ($0 ~ /^[ \t]*```/) fenced = !fenced
    next
  }
  /^[ \t]*$/ { flush(); if (started) blank = "\n"; next }
  /^(#|[|]|[ \t]*([-*+]|[0-9]+[.]) )/ { flush(); line = $0; next }
  line == "" { line = $0; next }
  { sub(/^[ \t]+/, ""); line = line " " $0 }
  END { flush(); exit !started }
' CHANGELOG.md; then
  echo "CHANGELOG.md has no section for $version: add \"## $version - $(date +%F)\"" >&2
  exit 1
fi
