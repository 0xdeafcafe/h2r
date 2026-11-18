#!/bin/sh
# Runs the Worker under local `wrangler dev` (workerd and miniflare on this
# machine: no Cloudflare account, nothing deployed) and smoke-tests it
# (smoke.ts). The cold start is measured twice: on an empty log, and after
# a restart with the log `--seed` grew.
#
#   crates/live-worker/smoke/run.sh [SEED_PAIRS] [OUT_DIR]
#
# Needs build/ from `worker-build --release` in crates/live-worker. Keeps
# its state (the hub's storage, a dev server key) under
# $TMPDIR/live-worker-dev; remove that directory to start over.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
repo=$(cd "$here/../.." && pwd)
seed=${1:-500}
out=${2:-$here/smoke/out}
work=${TMPDIR:?}/live-worker-dev
port=${PORT:-8787}
mkdir -p "$work" "$out"
[ -f "$here/build/index.js" ] || { echo "no build: run worker-build --release in $here" >&2; exit 1; }

# A dev identity, made once: the server key is a secret (.dev.vars).
if [ ! -f "$work/.dev.vars" ]; then
    key=$(openssl rand 32 | base64 | tr '+/' '-_' | tr -d '=')
    printf 'H2_LIVE_SERVER_KEY=%s\n' "$key" > "$work/.dev.vars"
    chmod 600 "$work/.dev.vars"
fi
cat > "$work/wrangler.toml" <<EOF
name = "h2-live-dev"
main = "$here/build/index.js"
compatibility_date = "2026-11-01"

[vars]
H2_LIVE_ID = "live.test"
H2_LIVE_PUBLIC_URL = "http://127.0.0.1:$port"
H2_LIVE_ADMINS = "SmokeAdmin"

[[r2_buckets]]
binding = "BLOBS"
bucket_name = "h2-live-dev"

[durable_objects]
bindings = [{ name = "LIVE_HUB", class_name = "LiveHub" }]

[[migrations]]
tag = "v1"
new_sqlite_classes = ["LiveHub"]
EOF

start() {
    (cd "$work" && exec bunx wrangler@4 dev --local --port "$port" --persist-to "$work/state" \
        --show-interactive-dev-session=false) > "$work/wrangler-$1.log" 2>&1 &
    pid=$!
    # Ready when wrangler says so. Every request goes to the hub, so no
    # probe is sent: the smoke test's first /healthz is the cold start.
    i=0
    until grep -q "Ready on" "$work/wrangler-$1.log" 2>/dev/null; do
        i=$((i + 1))
        [ $i -lt 600 ] || { echo "wrangler dev did not start" >&2; cat "$work/wrangler-$1.log" >&2; exit 1; }
        sleep 0.2
    done
}
stop() {
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    # wrangler leaves workerd behind if it is killed mid-start.
    pkill -f "workerd.*$work" 2>/dev/null || true
}
trap stop EXIT

start first
bun "$here/smoke/smoke.ts" "http://127.0.0.1:$port" --seed "$seed" | tee "$out/first.json"
stop
start restart
bun "$here/smoke/smoke.ts" "http://127.0.0.1:$port" --cold-only | tee "$out/restart.json"
stop
cp "$work/wrangler-first.log" "$work/wrangler-restart.log" "$out/"
echo "results in $out" >&2
