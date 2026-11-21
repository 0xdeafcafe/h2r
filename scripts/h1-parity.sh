#!/bin/sh
# Run by the coordinator after h2, the game image and the maps are ready.
set -eu
: "${H1_ISO:?path to the Halo 1 disc}" "${H2_DISC:?path to the Halo 2 disc}"
: "${H2_IMAGE:?prebuilt image directory}" "${H2_DATA:?installed converted maps}"
h2bin=${H2_BIN:-h2}
work=$(mktemp -d "${TMPDIR:?}/parity.XXXXXX")
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
mkdir "$work/maps" "$work/settings"
cat > "$work/round.toml" <<'TOML'
map = "coagulation"
game_type = "slayer"
pace = "realtime"
score_limit = 50
seed = 7
[[bot]]
name = "Reference seat"
[[bot]]
name = "Opponent"
TOML
export H2_OPT=2 H2_DIFFICULTY=normal H2_SNAPSHOTS="$work/snapshots"
unset H2_SIM H2_BOTS
failed=0
trial() {
    case_name=$1
    converted=$2
    phase=$3
    socket="$work/game.sock"
    log="$work/$case_name$phase.log"
    if [ "$phase" = "--round-only" ]; then
        export H2_SIM="$work/round.toml"
    else
        unset H2_SIM
    fi
    "$h2bin" serve "$H2_DISC" --out "$H2_IMAGE" --data "$H2_DATA" --settings "$work/settings" \
        --no-halo1 --no-mappack --socket "$socket" >"$log" 2>&1 &
    pid=$!
    if ! "$h2bin" parity "$H1_ISO" "$converted" "$case_name" "$socket" "$phase"; then
        printf 'Failed %s %s. Serve log: %s\n' "$case_name" "$phase" "$log" >&2
        failed=1
    fi
    stop
}
printf 'Parity serve logs: %s\n' "$work"
for sandbox in h1 h2; do
    file="$H2_DATA/maps/h1_bloodgulch/$sandbox/h1_bloodgulch.map"
    [ -r "$file" ] || { printf 'Missing %s\n' "$file" >&2; exit 1; }
    # Give this disc slot the exact variant, even when both use one cache name.
    ln -sf "$(cd "$(dirname "$file")" && pwd)/$(basename "$file")" "$work/maps/coagulation.map"
    export H2_MAP_DIR="$work/maps" H2_LEVEL=coagulation
    trial "bloodgulch-$sandbox" "$file" --round-only
    trial "bloodgulch-$sandbox" "$file" --objects-only
done
unset H2_MAP_DIR
export H2_LEVEL=a30
trial a30 "$H2_DATA/campaigns/halo1/a30.map" --objects-only
exit "$failed"
