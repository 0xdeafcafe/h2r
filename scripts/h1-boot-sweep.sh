#!/bin/sh
# Boots every level `h2 import halo1` stored, one headless serve each on the
# fast tier, and prints one row per level. Run by the coordinator after the
# import (docs/halo1/parity.md "Boot sweep"). Lanes do not run it.
#
#   H2_DISC=halo2.iso H2_IMAGE=<image dir> H2_DATA=<data root> scripts/h1-boot-sweep.sh [stem...]
#
# With no stem it takes every `*.converted` marker under $H2_DATA/tags/maps
# that is a Halo 1 level. Row: stem, state (ok, boot, panic), the sim ledger's loop count, the sim arms, the first panic line.
set -eu
: "${H2_DISC:?path to the Halo 2 disc}" "${H2_IMAGE:?prebuilt image directory}"
: "${H2_DATA:?data root with the imported tag store}"
h2bin=${H2_BIN:-h2}
ticks=${SWEEP_TICKS:-600}
work=$(mktemp -d "${TMPDIR:?}/boot-sweep.XXXXXX")
pid=
stop() {
    if [ -n "$pid" ]; then
        kill "$pid" 2>/dev/null || :
        wait "$pid" 2>/dev/null || :
        pid=
    fi
}
trap 'stop' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir "$work/settings"
export H2_TIER=fast H2_OPT=${H2_OPT:-2} H2_DIFFICULTY=normal
unset H2_SIM H2_BOTS H2_MAP_DIR
if [ $# -gt 0 ]; then
    stems=$*
else
    stems=
    for f in "$H2_DATA"/tags/maps/*.converted; do
        s=$(basename "$f" .converted)
        case $s in
            (halo1_campaign_*_h[12] | h1_*_h[12]) stems="$stems $s" ;;
        esac
    done
fi
[ -n "$stems" ] || { printf 'No stored Halo 1 levels under %s/tags/maps: run h2 import halo1\n' "$H2_DATA" >&2; exit 1; }
printf 'Boot sweep logs: %s\n' "$work"
printf 'stem\tstate\tledger_loops\tarms\tfirst_panic\n'
failed=0
for stem in $stems; do
    variant=${stem##*_}
    case $stem in
        halo1_campaign_*)
            level=${stem#halo1_campaign_}
            level=${level%_*}
            assets=halo1
            [ "$variant" = h2 ] && assets=halo2
            env_set="H2_LEVEL=$level H2_HALO1_ASSETS=$assets"
            ;;
        *)
            level=${stem%_*}
            env_set="H2_LEVEL=$level H2_HALO1_SANDBOX=$variant"
            ;;
    esac
    log="$work/$stem.log"
    socket="$work/$stem.sock"
    # shellcheck disable=SC2086
    env $env_set "$h2bin" serve "$H2_DISC" --out "$H2_IMAGE" --data "$H2_DATA" --settings "$work/settings" \
        --no-halo1 --no-mappack --socket "$socket" >"$log" 2>&1 &
    pid=$!
    state=ok loops=- arms=-
    ask() { "$h2bin" agent "$@" --socket "$socket" 2>&1; }
    # Wait for the socket, then for the level and its first ticks.
    n=0
    while [ ! -S "$socket" ] && kill -0 "$pid" 2>/dev/null && [ $n -lt 600 ]; do
        sleep 0.2
        n=$((n + 1))
    done
    if ask session.wait "{\"until\":{\"phase\":\"gameplay\",\"map_ticks\":$ticks},\"timeout_ms\":300000}" >"$work/$stem.wait" 2>&1; then
        loops=$(ask sim.ledger '{}' | sed -n 's/.*"loops": *\([0-9]*\).*/\1/p' | head -1)
        arms=$(ask sim.arms '{}' | tr -d '\n' | cut -c1-120)
    else
        state=boot
    fi
    panic=$(grep -m1 -i 'panicked at\|unimplemented\|not implemented' "$log" | cut -c1-200 || :)
    if [ -n "$panic" ]; then
        [ "$state" = ok ] && state=panic
    fi
    [ "$state" = ok ] || failed=1
    printf '%s\t%s\t%s\t%s\t%s\n' "$stem" "$state" "${loops:--}" "$arms" "${panic:--}"
    stop
    rm -f "$socket"
done
exit "$failed"
