#!/usr/bin/env bash
# Installs Ghidra, the XboxDev XBE loader and ghidra-cli into .tools/
# (git-ignored). Use Ghidra through scripts/ghidra afterwards.
# The loader only works on the exact Ghidra version it was built for, so
# both are pinned here. Re-running is a no-op once installed.
set -euo pipefail

GHIDRA_VERSION=12.1.4
GHIDRA_URL=https://github.com/NationalSecurityAgency/ghidra/releases/download/Ghidra_12.1.4_build/ghidra_12.1.4_PUBLIC_20260921.zip
GHIDRA_SHA256=ddac49f903da9d5bac833e5cc79395098b9c33cfd3279be5f31bd00387d2d4db
XBE_URL=https://github.com/XboxDev/ghidra-xbe/releases/download/build-202610161023/ghidra_12.1.4_PUBLIC_20261016_ghidra-xbe.zip
XBE_SHA256=339b2f67563bbb8b5217740319b7f3eae9e9bbacf1f3a8aa29151f7ecb7d5152
GHIDRA_CLI_TAG=v0.2.2

root="$(cd "$(dirname "$0")/.." && pwd)"
tools="$root/.tools"
ghidra="$tools/ghidra_${GHIDRA_VERSION}_PUBLIC"
mkdir -p "$tools/downloads"

fetch() { # url sha256 dest
  if [[ ! -f "$3" ]]; then
    echo "downloading $(basename "$3")"
    # -C - resumes a partial download from an earlier run.
    curl -fL -C - --retry 5 --progress-bar -o "$3.part" "$1"
    mv "$3.part" "$3"
  fi
  echo "$2  $3" | shasum -a 256 -c - >/dev/null || { echo "checksum mismatch: $3" >&2; rm -f "$3"; exit 1; }
}

fetch "$GHIDRA_URL" "$GHIDRA_SHA256" "$tools/downloads/ghidra.zip"
fetch "$XBE_URL" "$XBE_SHA256" "$tools/downloads/ghidra-xbe.zip"

if [[ ! -d "$ghidra" ]]; then
  echo "unpacking ghidra"
  unzip -q "$tools/downloads/ghidra.zip" -d "$tools"
fi
if [[ ! -d "$ghidra/Ghidra/Extensions/ghidra-xbe" ]]; then
  echo "installing XBE loader"
  unzip -q "$tools/downloads/ghidra-xbe.zip" -d "$ghidra/Ghidra/Extensions"
fi

# The release zip has no macOS decompiler binary; build it once.
if [[ "$(uname)" == Darwin ]] && ! find "$ghidra/Ghidra/Features/Decompiler" -name decompile -path "*os/mac*" 2>/dev/null | grep -q .; then
  echo "building decompiler natives (one-off, a few minutes)"
  (cd "$ghidra/support/gradle" && ./gradlew buildNatives -q)
fi

if [[ ! -x "$tools/ghidra-cli/bin/ghidra" ]]; then
  echo "building ghidra-cli $GHIDRA_CLI_TAG"
  # The system git copes with insteadOf/credential setups that cargo's doesn't.
  CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install -q --locked \
    --git https://github.com/akiselev/ghidra-cli --tag "$GHIDRA_CLI_TAG" --root "$tools/ghidra-cli"
fi

echo "ghidra ready: $ghidra"
echo "GUI: $ghidra/ghidraRun"
echo "CLI: scripts/ghidra doctor"
