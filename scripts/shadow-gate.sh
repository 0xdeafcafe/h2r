#!/usr/bin/env bash
# The shadow-execution disc gate (docs/engine/shadow-exec.md): a headless
# `h2 sim run` on MAP with the named native sim ports installed and their
# shadow checks on (`shadow.enable` through the agent API) for TICKS game
# ticks of gameplay. Fails on any mismatch, when nothing was checked, or
# when a named port is not installed.
#
#   scripts/shadow-gate.sh [MAP] [TICKS] [PORT|GROUP|all ...]
#
#   MAP     a multiplayer map (default lockout)
#   TICKS   game ticks of gameplay to check (default 900: 30 s)
#   PORTS   port names (sim.port.unit_pose), groups (sim.native,
#           sim.physics, sim.native.objects), object point names,
#           or all (the default standalone port set)
#
# Environment:
#   H2            the h2 binary (default h2; reinstall it from this tree)
#   H2_DISC       the disc image (default disc/halo2.iso in the checkout)
#   H2_OPT        as for the build: the image must already be built
#   H2_FEATURES   passed through (every port is always installed)
#   SHADOW_EVERY  check one call in this many (default 1)
#   SHADOW_OUT    where the match, log, report and verdict go
#                 (default a new directory under $TMPDIR)
#
# The map needs its nav mesh (`h2 bots nav MAP`). The verdict (with the
# full shadow.status) is SHADOW_OUT/verdict.json; mismatches carry the
# port, function, tick, entry registers and the diff.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
h2="${H2:-h2}"
map="${1:-lockout}"
ticks="${2:-900}"
shift $(($# < 2 ? $# : 2))
selected=("$@")
[[ ${#selected[@]} -eq 0 ]] && selected=(all)
every="${SHADOW_EVERY:-1}"
disc="${H2_DISC:-$root/disc/halo2.iso}"

# Every port by group. `feature`'s shadow tests check that each registered
# port is listed here, one per line.
sim_native=(
  sim.port.transform_compose
  sim.port.pose_matrix
  sim.port.unit_pose
  sim.port.marker_transforms
)
sim_physics=(
  sim.port.unit_aim
  sim.port.first_person_pose
  sim.port.first_person_filters
)
# The object bodies replace some standalone ports (including unit aim).
# Select this group separately from the standalone "all" profile.
sim_objects=(
  OBJECTS_PRE_PHYSICS
  OBJECTS_POST_PHYSICS
  OBJECT_FUNCTION_VALUE
  LIGHT_RECORD_UPDATE
  UNIT_CLAMP_DIRECTION
  UNIT_ACTIONS_UPDATE
)

ports=()
for name in "${selected[@]}"; do
  case "$name" in
    all) ports+=("${sim_native[@]}" "${sim_physics[@]}") ;;
    sim.native) ports+=("${sim_native[@]}") ;;
    sim.physics) ports+=("${sim_physics[@]}") ;;
    sim.native.objects) ports+=("${sim_objects[@]}") ;;
    *)
      if [[ " ${sim_native[*]} ${sim_physics[*]} ${sim_objects[*]} " != *" $name "* ]]; then
        echo "shadow-gate: no port or group named $name" >&2
        exit 2
      fi
      ports+=("$name")
      ;;
  esac
done

features="${H2_FEATURES:-}"
json_ports=""
for p in "${ports[@]}"; do
  json_ports="${json_ports:+$json_ports,}\"$p\""
done

out="${SHADOW_OUT:-$(mktemp -d "${TMPDIR:-/tmp}/shadow-gate.XXXXXX")}"
mkdir -p "$out"
sock="$out/agent.sock"
rm -f "$sock"

# Two phantoms, no time limit, a score nobody reaches: the match outlasts
# the check, which stops it.
cat > "$out/match.toml" <<EOF
map = "$map"
game_type = "slayer"
time_limit_s = 0
score_limit = 250
seed = 7
max_ticks = $((ticks + 18000))
max_wall_s = 7200
render_every = 0
pace = "unpaced"

[[bot]]
name = "Red"
skill = "legendary"
team = 0

[[bot]]
name = "Blue"
skill = "legendary"
team = 1
EOF

# The agent script: gameplay, checks on, TICKS ticks (in waits of at most
# 300 ticks, each under the 600 s wait cap), the status, the verdict.
{
  echo 'session.wait {"until":{"phase":"gameplay"},"timeout_ms":600000}'
  echo "shadow.enable {\"ports\":[$json_ports],\"every\":$every}"
  echo 'keep'
  echo 'expect /not_installed == []'
  left=$ticks
  while ((left > 0)); do
    n=$((left < 300 ? left : 300))
    echo "session.wait {\"until\":{\"ticks\":$n},\"timeout_ms\":600000}"
    left=$((left - n))
  done
  echo 'shadow.status'
  echo 'keep'
  echo 'expect /mismatched == 0'
  echo 'expect /checked > 0'
  echo 'match.stop'
} > "$out/gate.script"

echo "shadow-gate: $map, $ticks ticks, ${#ports[@]} ports, every $every; output in $out"
H2_FEATURES="$features" "$h2" sim run "$out/match.toml" --disc "$disc" \
  --socket "$sock" --report "$out/report.json" > "$out/sim.log" 2>&1 &
sim=$!
trap 'kill "$sim" 2>/dev/null || true' EXIT

for _ in $(seq 1 600); do
  [[ -S "$sock" ]] && break
  if ! kill -0 "$sim" 2>/dev/null; then
    echo "shadow-gate: h2 sim run exited before its agent socket opened" >&2
    tail -n 40 "$out/sim.log" >&2
    exit 1
  fi
  sleep 1
done
if [[ ! -S "$sock" ]]; then
  echo "shadow-gate: no agent socket after 600 s" >&2
  exit 1
fi

verdict=0
"$h2" agent run "$out/gate.script" --socket "$sock" --timeout 700 > "$out/verdict.json" || verdict=$?

# match.stop ends the run; give it a minute to write its report.
for _ in $(seq 1 60); do
  kill -0 "$sim" 2>/dev/null || break
  sleep 1
done
kill "$sim" 2>/dev/null || true
trap - EXIT

if command -v jq > /dev/null; then
  jq '{pass, failed, stopped,
       shadow: (.kept[-1].result // null
         | if . then {calls, checked, matched, mismatched, skipped,
                      ports: [.ports[] | select(.on) | {name, calls, checked, matched, mismatched, skipped}],
                      mismatches: [.mismatches[] | {port, function, pc, tick, kind, summary}]}
           else null end)}' "$out/verdict.json" || cat "$out/verdict.json"
else
  cat "$out/verdict.json"
fi

if [[ $verdict -ne 0 ]]; then
  echo "shadow-gate: FAIL ($out/verdict.json)" >&2
  exit 1
fi
echo "shadow-gate: pass"
