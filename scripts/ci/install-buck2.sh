#!/bin/sh
# Installs the pinned buck2 release onto PATH for a CI job
# (.github/workflows/buck.yml): BUCK2_TAG and BUCK2_SHA256 name the release
# and the checksum of its x86_64 Linux archive.
set -eu
: "${BUCK2_TAG:?}" "${BUCK2_SHA256:?}"
dir=${RUNNER_TEMP:-/tmp}/buck2-download
bin=$HOME/.local/bin
mkdir -p "$dir" "$bin"
curl -fLsS --retry 3 -o "$dir/buck2.zst" \
  "https://github.com/facebook/buck2/releases/download/$BUCK2_TAG/buck2-x86_64-unknown-linux-gnu.zst"
echo "$BUCK2_SHA256  $dir/buck2.zst" | sha256sum -c -
zstd -d -q "$dir/buck2.zst" -o "$bin/buck2"
chmod +x "$bin/buck2"
echo "$bin" >> "${GITHUB_PATH:-/dev/null}"
"$bin/buck2" --version
