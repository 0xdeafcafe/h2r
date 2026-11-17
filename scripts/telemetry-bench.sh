#!/usr/bin/env bash
# Runs every live telemetry benchmark (docs/live/live-telemetry.md,
# "Performance") and collects their JSON in one directory:
#
#   guest.json   the game thread's cost per tick, allocations per tick, the
#                sink worker's packed encode and the wire size
#                (crates/live-telemetry/src/bench.rs)
#   server.json  ingest, signed event batches and stream fan-out
#                (crates/live-server/benches/telemetry.rs)
#   web.json     packed-frame decode, the frame hub and repaint sampling
#                (web/bench/telemetry.bench.ts)
#
#   scripts/telemetry-bench.sh [out dir]   default: $TMPDIR/telemetry-bench
#
# Set CARGO_LOCK to a lock file to run cargo under `lockf -k` (machines that
# share one compiler between jobs). No game data and no game run is needed.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
out="${1:-${TMPDIR:-/tmp}/telemetry-bench}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
export TELEMETRY_BENCH_OUT="$out"

cargo_run() {
  if [[ -n "${CARGO_LOCK:-}" ]]; then
    lockf -k "$CARGO_LOCK" cargo "$@"
  else
    cargo "$@"
  fi
}

echo "== guest (live-telemetry)"
cargo_run test --release -p live-telemetry --lib bench -- \
  --ignored --nocapture --test-threads=1
echo "== server (live-server)"
cargo_run bench -p live-server --bench telemetry
echo "== web"
(cd web && bun run bench:telemetry)
echo "results in $out"
