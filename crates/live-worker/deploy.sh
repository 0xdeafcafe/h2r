#!/bin/sh
# Builds and deploys the Live Worker (docs/live/live-worker.md, "Deploy").
#
#   crates/live-worker/deploy.sh staging
#   crates/live-worker/deploy.sh cutover --yes-production
#   crates/live-worker/deploy.sh production --yes-production
#
# staging     h2-live-staging on workers.dev: its own id, key, bucket and log.
# cutover     h2-live with live.forbes.red's id and key, on workers.dev only
#             (no route): the Worker the log is copied into before the switch.
# production  h2-live with the live.forbes.red route: the switch.
#
# In order: the toolchain (a rustup of its own, so the machine's is left
# alone), the web build, the wasm build, the Cloudflare sign-in, the R2
# bucket, the server key (asked for only when the Worker has none), the
# deploy, a health check. Each step says what it does first. The key is
# never printed, logged or put on a command line.
#
# Options:
#   --yes-production   required for cutover and production
#   --skip-build       deploy what build/ and web/dist already hold
#   --dry-run          build and bundle, deploy nothing (no sign-in needed)
#   --no-hosts         deploy without the dedicated host containers: a copy of
#                      wrangler.toml with no [[containers]] and LIVE_MAX_HOSTS
#                      "0" (Live refuses host launches). Needed until the host
#                      image is staged (deploy/host/build.sh fills deploy/host/stage)
#
# Environment:
#   H2_LIVE_KEY_FILE      cutover/production: read the key from this file
#                         (the machine server's server.key) instead of asking
#   H2_LIVE_PUBLIC_URL    staging: the workers.dev address, for discovery
#   H2_LIVE_STAGING_DIR   staging: where its identity is kept, server.key and
#                         server.json, for `h2 live store copy --operator`
#                         (default ~/.config/h2-live-worker/staging)
#   H2_WORKER_HOME        the toolchain's home (default ~/.cache/h2-live-worker)
#   WRANGLER              how to run wrangler (default: bunx wrangler@4)
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)

usage() {
    sed -n '2,36p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

env_name=""
yes_production=0
skip_build=0
dry_run=0
no_hosts=0
for arg in "$@"; do
    case "$arg" in
        staging | cutover | production) env_name=$arg ;;
        --yes-production) yes_production=1 ;;
        --skip-build) skip_build=1 ;;
        --dry-run) dry_run=1 ;;
        --no-hosts) no_hosts=1 ;;
        -h | --help) usage ;;
        *) echo "deploy.sh: unknown argument $arg" >&2; usage ;;
    esac
done
[ -n "$env_name" ] || usage

step() { printf '\n==> %s\n' "$*"; }
die() { printf 'deploy.sh: %s\n' "$*" >&2; exit 1; }

case "$env_name" in
    staging) ;;
    *)
        [ "$yes_production" = 1 ] || [ "$dry_run" = 1 ] ||
            die "$env_name is live.forbes.red's server; run again with --yes-production"
        ;;
esac

# The environment's bucket and id, from wrangler.toml.
toml_value() { # SECTION KEY: the first KEY = "..." after the [SECTION] header
    awk -v sec="$1" -v key="$2" '
        /^\[/ { inside = ($0 == sec) }
        inside && $1 == key { sub(/^[^"]*"/, ""); sub(/".*$/, ""); print; exit }
    ' "$here/wrangler.toml"
}
bucket=$(toml_value "[[env.$env_name.r2_buckets]]" bucket_name)
server_id=$(toml_value "[env.$env_name.vars]" H2_LIVE_ID)
[ -n "$bucket" ] && [ -n "$server_id" ] || die "wrangler.toml has no bucket or id for env.$env_name"
echo "deploy.sh: $env_name: server id $server_id, bucket $bucket"

# The host containers build deploy/host/Dockerfile from the staged image.
# Without it (or with --no-hosts) deploy from a copy without them; it sits
# next to wrangler.toml so its relative paths still resolve.
config="$here/wrangler.toml"
if [ "$no_hosts" = 1 ]; then
    config="$here/wrangler.nohosts.toml"
    awk '
        /^\[/ { skip = ($0 ~ /^\[\[(env\.[a-z]+\.)?containers\]\]$/) }
        skip { next }
        /^LIVE_MAX_HOSTS = / { print "LIVE_MAX_HOSTS = \"0\""; next }
        { print }
    ' "$here/wrangler.toml" > "$config"
    echo "deploy.sh: --no-hosts: no host containers, LIVE_MAX_HOSTS 0"
elif [ "$dry_run" = 0 ] && ! [ -d "$repo/deploy/host/stage" ]; then
    die "the host containers need the staged image (deploy/host/build.sh); run again with --no-hosts to deploy without them"
fi

wrangler() {
    # shellcheck disable=SC2086
    ${WRANGLER:-bunx wrangler@4} "$@" --config "$config"
}
# The terminal echoes again however this ends (the key is read unechoed).
trap 'stty echo 2>/dev/null || true' EXIT INT TERM

# ---- build ----
home=${H2_WORKER_HOME:-${XDG_CACHE_HOME:-$HOME/.cache}/h2-live-worker}
channel=$(awk -F'"' '/^channel/ { print $2 }' "$repo/rust-toolchain.toml")

if [ "$skip_build" = 0 ]; then
    step "toolchain: Rust $channel with wasm32 and worker-build in $home"
    export RUSTUP_HOME="$home/rustup"
    if [ ! -x "$home/cargo/bin/rustup" ]; then
        mkdir -p "$home"
        echo "installing rustup into $home (once; ~/.rustup and ~/.cargo are not touched)"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "$home/rustup-init.sh"
        CARGO_HOME="$home/cargo" sh "$home/rustup-init.sh" -y --no-modify-path \
            --profile minimal --default-toolchain none
    fi
    PATH="$home/cargo/bin:$PATH"
    export PATH
    CARGO_HOME="$home/cargo" rustup toolchain install "$channel" --profile minimal \
        --target wasm32-unknown-unknown
    if [ ! -x "$home/cargo/bin/worker-build" ]; then
        CARGO_HOME="$home/cargo" cargo install worker-build --version 0.9.0 --locked
    fi

    step "web: bun install and vite build into web/dist"
    (cd "$repo/web" && bun install --frozen-lockfile && bun run build)

    step "wasm: worker-build --release into crates/live-worker/build"
    (cd "$here" && RUSTC_WRAPPER='' CARGO_TARGET_DIR="$home/target" worker-build --release)
fi
[ -f "$here/build/index.js" ] || die "no build/index.js: run without --skip-build"
[ -f "$repo/web/dist/index.html" ] || die "no web/dist: run without --skip-build"

vars=""
if [ "$env_name" = staging ] && [ -n "${H2_LIVE_PUBLIC_URL:-}" ]; then
    vars="--var H2_LIVE_PUBLIC_URL:$H2_LIVE_PUBLIC_URL"
fi

if [ "$dry_run" = 1 ]; then
    step "dry run: wrangler deploy --env $env_name --dry-run (nothing is uploaded)"
    # shellcheck disable=SC2086
    wrangler deploy --env "$env_name" --dry-run --outdir "$here/build/dry-run" $vars
    exit 0
fi

# ---- Cloudflare ----
step "sign-in: wrangler whoami"
who=$(wrangler whoami 2>&1) || true
case "$who" in
    *"not authenticated"* | *"Not logged in"* | *"wrangler login"*)
        die "wrangler is not signed in: run \`bunx wrangler@4 login\` and try again" ;;
esac
printf '%s\n' "$who" | grep -E "logged in|Account Name|Account ID|email" || true

step "R2: bucket $bucket"
if wrangler r2 bucket list 2>/dev/null | grep -qE "name:[[:space:]]+$bucket\$"; then
    echo "$bucket exists"
elif ! made=$(wrangler r2 bucket create "$bucket" 2>&1); then
    case "$made" in
        *"already exists"* | *"already own"*) echo "$bucket exists" ;;
        *) printf '%s\n' "$made" >&2; die "could not create the bucket $bucket" ;;
    esac
else
    printf '%s\n' "$made"
fi

# The key: the Worker's secret. Read now, put after the deploy (a Worker
# has to exist to hold a secret).
step "server key: is H2_LIVE_SERVER_KEY set on the Worker?"
key=""
staging_dir=${H2_LIVE_STAGING_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/h2-live-worker/staging}
if wrangler secret list --env "$env_name" 2>/dev/null | grep -q '"H2_LIVE_SERVER_KEY"'; then
    echo "set; leaving it as it is"
    if [ "$env_name" = staging ] && [ ! -f "$staging_dir/server.key" ]; then
        echo "warning: $staging_dir has no server.key, so nothing here can sign as staging's"
        echo "operator (h2 live store copy --operator). To give staging a key kept here:"
        echo "  bunx wrangler@4 secret delete H2_LIVE_SERVER_KEY --env staging, then deploy again"
    fi
else
    if [ "$env_name" = staging ]; then
        echo "not set: making a fresh key for staging (it is no other server's),"
        echo "kept as $staging_dir/server.key (mode 0600) for the operator's copy"
        if [ -f "$staging_dir/server.key" ]; then
            key=$(tr -d ' \r\n' < "$staging_dir/server.key")
        else
            key=$(openssl rand 32 | base64 | tr '+/' '-_' | tr -d '=\n')
            (umask 077 && mkdir -p "$staging_dir" && printf '%s\n' "$key" > "$staging_dir/server.key")
        fi
        printf '{"id":"%s"}\n' "$server_id" > "$staging_dir/server.json"
    elif [ -n "${H2_LIVE_KEY_FILE:-}" ]; then
        echo "not set: reading it from \$H2_LIVE_KEY_FILE"
        key=$(tr -d ' \r\n' < "$H2_LIVE_KEY_FILE")
    else
        echo "not set: paste the contents of the machine server's server.key"
        echo "(~/Library/Application Support/h2recomp/live/server/server.key); it is not echoed"
        stty -echo 2>/dev/null || true
        IFS= read -r key || true
        stty echo 2>/dev/null || true
        echo
        key=$(printf '%s' "$key" | tr -d ' \r\n')
    fi
    [ ${#key} = 43 ] || die "that is not a server key (43 characters of base64url)"
    if [ "$env_name" != staging ]; then
        # The key must be the one live.forbes.red signs with: compare its
        # public half with the discovery document's.
        step "server key: checking it is $server_id's"
        mine=$(printf '%s' "$key" | tr -- '-_' '+/' | sed 's/$/=/' | base64 -d 2>/dev/null |
            { printf '\060\056\002\001\000\060\005\006\003\053\145\160\004\042\004\040'; cat; } |
            openssl pkey -inform DER -pubout -outform DER 2>/dev/null | tail -c 32 |
            base64 | tr '+/' '-_' | tr -d '=\n')
        theirs=$(curl -fsS "https://$server_id/.well-known/h2live.json" 2>/dev/null |
            sed -n 's/.*"x":"\([^"]*\)".*/\1/p' | head -1)
        if [ -z "$theirs" ]; then
            echo "warning: could not fetch https://$server_id/.well-known/h2live.json to compare"
        elif [ "$mine" != "$theirs" ]; then
            die "that key is not $server_id's (its public key differs from discovery's)"
        else
            echo "it is: the public key matches discovery's"
        fi
    fi
fi

# ---- deploy ----
step "deploy: wrangler deploy --env $env_name"
log=$(mktemp "${TMPDIR:-/tmp}/h2-live-deploy.XXXXXX")
# shellcheck disable=SC2086
wrangler deploy --env "$env_name" $vars 2>&1 | tee "$log"
url=$(grep -oE 'https://[a-z0-9.-]+\.workers\.dev' "$log" | head -1 || true)
rm -f "$log"

if [ -n "$key" ]; then
    step "server key: wrangler secret put H2_LIVE_SERVER_KEY --env $env_name"
    printf '%s' "$key" | wrangler secret put H2_LIVE_SERVER_KEY --env "$env_name"
    key=""
fi

step "health"
base=$url
[ "$env_name" = production ] && base="https://$server_id"
if [ -n "$base" ]; then
    echo "$base/healthz: $(curl -fsS "$base/healthz" 2>&1 || true)"
    echo "$base/.well-known/h2live.json id: $(curl -fsS "$base/.well-known/h2live.json" 2>/dev/null |
        sed -n 's/.*"id":"\([^"]*\)".*/\1/p')"
    if [ "$env_name" = staging ] && [ -z "${H2_LIVE_PUBLIC_URL:-}" ]; then
        echo "discovery names https://$server_id; to name $base, deploy again with H2_LIVE_PUBLIC_URL=$base"
    fi
else
    echo "no workers.dev address in wrangler's output; check the dashboard"
fi
echo "deployed $env_name"
