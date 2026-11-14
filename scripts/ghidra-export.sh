#!/usr/bin/env bash
# Imports disc/default.xbe into the Ghidra project (first run only; analysis
# takes a while), then writes Ghidra's function list to
# disc/ghidra-functions.tsv for `cargo h2 analyze --against`.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
ghidra="$root/scripts/ghidra"
out="$root/disc/ghidra-functions.tsv"

[[ -f "$root/disc/default.xbe" ]] || (cd "$root" && cargo h2 extract-xbe)

if [[ ! -d "$root/disc/ghidra/h2.rep" ]]; then
  "$ghidra" import "$root/disc/default.xbe"
fi
"$ghidra" script run "$root/scripts/ghidra-scripts/ExportFunctions.java" \
  --expect "$out" -- "$out"
