#!/usr/bin/env bash
# The render-blind gate (docs/engine/sim-render-split.md, "The proof"; step
# 4b of the verification ladder in docs/engine/sim-rewrite.md): one input
# tape, run from one snapshot in several arms, leaves the same
# `state_hashes_v1` series, every row and arena hash, tick for tick.
#
#   scripts/render-blind-gate.sh OUTDIR SNAPSHOT TAPE TICKS NAME=SOCKET...
#
#   OUTDIR    where each arm's `sim.hashes` reply and the verdict go
#   SNAPSHOT  a `state.save` name of the map's starting state
#   TAPE      an `input.record` name made from that snapshot (TICKS long)
#   TICKS     ticks to step (the gate: 1800)
#   NAME=SOCKET  one arm per game, already running with `impl.sim.arena-hash`
#             on, the first the reference (render skipped: a headless
#             `h2 sim run --socket`, or a game with `sim.render_every` 0),
#             the rest drawn as the arm table in the page says (640x480;
#             32:9 window at scale 4; Theater's free camera; 30 and 120
#             display frames a second).
#
# It loads the snapshot, replays the tape for TICKS ticks, reads
# `sim.hashes` and, once every arm has run, `h2 sim blind` compares them.
# Fails on the first differing tick (named with its arm), on a tape that ran
# out or read no inputs, and on an arm with too few ticks or no arena hash.
# Run it once per map: Lockout, Waterworks and one campaign level.
set -euo pipefail

h2="${H2:-h2}"
if [[ $# -lt 6 ]]; then
  sed -n '2,/^set -/p' "$0" | sed -e '$d' -e 's/^# \{0,1\}//' >&2
  exit 2
fi
out="$1" snapshot="$2" tape="$3" ticks="$4"
shift 4
mkdir -p "$out"

files=()
for arm in "$@"; do
  name="${arm%%=*}" socket="${arm#*=}"
  [[ "$arm" == *=* && -n "$name" && -n "$socket" ]] || { echo "render-blind-gate: want NAME=SOCKET, got $arm" >&2; exit 2; }
  echo "render-blind-gate: arm $name" >&2
  # One connection holds the claim: load, replay, step, stop.
  result="$(printf '%s\n' \
    'session.claim' \
    'session.pause {"max_ms":600000}' \
    "state.load {\"name\":\"$snapshot\"}" \
    "input.replay {\"name\":\"$tape\"}" \
    "session.step {\"ticks\":$ticks}" \
    'input.stop' \
    'session.release' | "$h2" agent - --socket "$socket")"
  # The tape must have run to its end on the same pads it was recorded on.
  python3 -I -c '
import json, sys
stop = None
for line in sys.stdin.read().splitlines():
    try:
        v = json.loads(line)
    except ValueError:
        continue
    if isinstance(v, dict) and "replayed" in v and "matched" in v:
        stop = v
if stop is None:
    sys.exit("no input.stop counts in the arm\x27s replies")
if not (stop["matched"] == stop["replayed"] > 0 and stop.get("past_end", 0) == 0):
    sys.exit("the tape ran out, read no inputs, or saw other handles: %s" % stop)
' <<<"$result" || { echo "render-blind-gate: arm $name: bad replay" >&2; exit 1; }
  "$h2" agent sim.hashes "{\"max\":$ticks}" --socket "$socket" >"$out/$name.json"
  files+=("$name=$out/$name.json")
done

"$h2" sim blind --ticks "$ticks" "${files[@]}" | tee "$out/verdict.txt"
