#!/bin/sh
# The Halo 1 conversion gate: imports the whole disc into a fresh data root
# and fails unless every map converted (docs/halo1/parity.md "Boot sweep").
set -eu
: "${H1_ISO:?path to the Halo 1 disc}" "${H2_DISC:?path to the Halo 2 disc}"
h2bin=${H2_BIN:-h2}
# Ten campaign levels, 13 multiplayer maps in two sandboxes, ten Halo 2-assets variants.
want=46
root=$(mktemp -d "${TMPDIR:?}/h1-convert-gate.XXXXXX")
trap 'rm -rf "$root"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
log="$root/import.log"
status=0
"$h2bin" import halo1 "$H1_ISO" --disc "$H2_DISC" --data "$root/data" --force >"$log" 2>&1 || status=$?
cat "$log"
summary=$(grep -E '^[0-9]+ converted, [0-9]+ failed' "$log" | tail -n 1 || :)
if [ -z "$summary" ]; then
    printf 'Conversion gate: no summary line from h2 import halo1 (exit %s).\n' "$status" >&2
    exit 1
fi
converted=${summary%% converted*}
case $summary in
    *" 0 failed"*) ;;
    *) printf 'Conversion gate: maps failed: %s\n' "$summary" >&2; exit 1 ;;
esac
if [ "$status" -ne 0 ]; then
    printf 'Conversion gate: h2 import halo1 exited %s.\n' "$status" >&2
    exit 1
fi
if [ "$converted" -ne "$want" ]; then
    printf 'Conversion gate: %s maps converted, want %s.\n' "$converted" "$want" >&2
    exit 1
fi
printf 'Conversion gate: %s maps converted, 0 failed.\n' "$converted"
