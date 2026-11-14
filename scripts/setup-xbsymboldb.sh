#!/usr/bin/env bash
# Fetches Cxbx-Reloaded's XbSymbolDatabase (pinned commit) into
# .tools/XbSymbolDatabase (git-ignored). `h2 symbols` reads its OOVPA
# signature sources at run time.
#
# Why this is not vendored: the signature data under src/OOVPADatabase is
# ODbL-1.0 (per-file SPDX headers; the library code around it is MIT). ODbL
# share-alike does not fit this repo's MIT OR Apache-2.0, so we fetch the
# data on the user's machine instead of redistributing it. Re-running is a
# no-op once the pinned commit is in place.
set -euo pipefail

COMMIT=20eced544726f5558c5a408458f38a086cc4e543
URL="https://codeload.github.com/Cxbx-Reloaded/XbSymbolDatabase/tar.gz/$COMMIT"

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/.tools/XbSymbolDatabase"

if [[ "$(cat "$dest/.commit" 2>/dev/null || true)" == "$COMMIT" ]]; then
  echo "XbSymbolDatabase $COMMIT already in place"
  exit 0
fi

mkdir -p "$root/.tools"
tmp="$(mktemp -d "$root/.tools/xsd.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
curl -fL --retry 5 --progress-bar -o "$tmp/src.tgz" "$URL"
mkdir "$tmp/out"
tar xzf "$tmp/src.tgz" -C "$tmp/out" --strip-components=1
echo "$COMMIT" > "$tmp/out/.commit"
rm -rf "$dest"
mv "$tmp/out" "$dest"
echo "installed XbSymbolDatabase $COMMIT in $dest"
