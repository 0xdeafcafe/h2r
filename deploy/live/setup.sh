#!/usr/bin/env bash
# One-time setup for the Live server behind a named Cloudflare tunnel
# (docs/live/live-hosting.md, "Docker with a named tunnel"). Asks for what it
# needs, writes deploy/live/.env, extracts the game art if it is missing or stale,
# starts the containers, waits for the public URL and, for an invite-only
# server, makes a first invite.
#
#   deploy/live/setup.sh
#
# Safe to run again: it keeps an existing .env unless you say otherwise and
# skips the art when deploy/live/art holds art from this h2's extractor
# (`h2 live assets check`), extracting it again when it is older.
set -euo pipefail

start="$PWD"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
cd "$here"

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
ask() { # ask VAR "prompt" [default]
  local v
  read -r -p "$2${3:+ [$3]}: " v
  printf -v "$1" '%s' "${v:-${3:-}}"
}

# `type -P` looks only on PATH (`command -v` would find this function).
h2() {
  if type -P h2 >/dev/null; then
    command h2 "$@"
  else
    (cd "$root" && cargo run -q --release -p cli --bin h2 -- "$@")
  fi
}

say "Checking tools"
command -v docker >/dev/null || die "docker not found (install Docker Desktop)"
docker compose version >/dev/null 2>&1 || die "docker compose not found"
docker info >/dev/null 2>&1 || die "Docker isn't running: start Docker Desktop and run this again"
command -v curl >/dev/null || die "curl not found"
if ! type -P h2 >/dev/null; then
  command -v cargo >/dev/null || die "neither h2 nor cargo found (needed to extract art and make invites)"
  ask inst "h2 isn't on your PATH. Install it with cargo install now? (y/n)" y
  if [[ $inst == [yY]* ]]; then
    cargo install --locked --path "$root/crates/apps/cli" --bin h2
    hash -r
  else
    echo "fine: using cargo run from the repo instead"
  fi
fi
echo "ok"

write_env=yes
if [[ -f .env ]]; then
  ask keep ".env already exists. Keep it? (y/n)" y
  [[ $keep == [yY]* ]] && write_env=no
fi

if [[ $write_env == yes ]]; then
  say "Cloudflare tunnel"
  cat <<'TXT'
In the Cloudflare dashboard: Zero Trust > Networks > Tunnels > Create a tunnel
(type Cloudflared), name it (say h2live). On the install step, copy the long
eyJ... token from the command shown. Then on Public Hostname add:
  subdomain: live   domain: forbes.red   service: HTTP  127.0.0.1:8074
Cloudflare creates the DNS record for you (delete any old "live" record first).
TXT
  token=""
  while [[ -z $token ]]; do
    read -r -s -p "Tunnel token (hidden): " token; echo
    token="${token#*--token }"; token="${token// /}"
    [[ $token == eyJ* ]] || { echo "that doesn't look like a token (it starts with eyJ)"; token=""; }
  done

  ask url "Public URL" "https://live.forbes.red"
  [[ $url == https://* ]] || die "the public URL must start with https://"
  ask admins "Admin gamertag(s), comma-separated"
  [[ -n $admins ]] || die "name at least one admin gamertag"
  ask invite_only "Invite-only sign-up (accounts need an invite code)? (y/n)" n
  [[ $invite_only == [yY]* ]] && invite_only=1 || invite_only=""

  say "Cloudflare R2 (optional)"
  cat <<'TXT'
The event log is SQLite in ./data. Content and cloud saves can live in an R2
bucket instead of ./data, shared game art can be served straight from a
public one, and live.db can be backed up to R2 continuously (free tier:
10 GB, no egress fees; docs/live/live-hosting.md, "Free hosting").
Leave the bucket blank to keep everything on this machine.

In the Cloudflare dashboard: R2 > Create bucket (say h2live; and h2live-public
for the art, with a custom domain or r2.dev under Settings > Public access).
R2 > Manage API tokens > Create API token: Object Read & Write, those buckets.
The S3 endpoint is on the R2 overview: https://<account id>.r2.cloudflarestorage.com
TXT
  ask bucket "Private bucket for content, saves and backups (blank: none)"
  r2=""
  if [[ -n $bucket ]]; then
    ask endpoint "S3 endpoint (https://<account id>.r2.cloudflarestorage.com)"
    [[ $endpoint == https://* ]] || die "the endpoint starts with https://"
    ask key_id "Access key ID"
    secret=""
    while [[ -z $secret ]]; do read -r -s -p "Secret access key (hidden): " secret; echo; done
    ask public_bucket "Public bucket for the game art (blank: serve it from here)"
    public_url=""
    if [[ -n $public_bucket ]]; then
      ask public_url "Its public URL (custom domain or https://pub-....r2.dev)"
      [[ $public_url == https://* ]] || die "the public URL starts with https://"
    fi
    ask backup "Back up live.db to $bucket continuously with Litestream? (y/n)" y
    r2="H2_LIVE_S3_ENDPOINT=$endpoint
H2_LIVE_S3_ACCESS_KEY_ID=$key_id
H2_LIVE_S3_SECRET_ACCESS_KEY=$secret
H2_LIVE_BLOBS=s3://$bucket"
    if [[ -n $public_bucket ]]; then
      r2+="
H2_LIVE_PUBLIC_BLOBS=s3://$public_bucket
H2_LIVE_PUBLIC_BLOBS_URL=$public_url"
    fi
    if [[ $backup == [yY]* ]]; then
      r2+="
COMPOSE_PROFILES=backup
LITESTREAM_BUCKET=$bucket"
    fi
  fi

  umask 077
  cat > .env <<ENV
TUNNEL_TOKEN=$token
H2_LIVE_ADMINS=$admins
H2_LIVE_PUBLIC_URL=$url
H2_LIVE_INVITE_ONLY=$invite_only
H2_LIVE_DATA_DIR=./data
H2_LIVE_ART_DIR=./art
$r2
ENV
  umask 022
  echo "wrote $here/.env (only you can read it)"
fi

url="$(sed -n 's/^H2_LIVE_PUBLIC_URL=//p' .env)"; url="${url:-https://live.forbes.red}"
mkdir -p data art

# An h2 installed before `assets check` existed is older than this checkout,
# and art it extracts would be stale too.
if type -P h2 >/dev/null && [[ "$(h2 live --help 2>&1 || true)" != *"assets check"* ]]; then
  die "the h2 on your PATH is older than this checkout; update it: cargo install --locked --force --path $root/crates/apps/cli --bin h2"
fi

if ! h2 live assets check --out "$here/art"; then
  say "Game art"
  if [[ -f art/manifest.json ]]; then
    echo "The art in $here/art is from an older extractor: extracting it again"
    echo "(the 3D viewer's animated, textured models need the current one)."
  else
    echo "The server needs art extracted from your disc once (it is never committed)."
  fi
  ask iso "Path to your Halo 2 disc image (.iso)"
  iso="${iso/#\~/$HOME}"
  [[ $iso == /* ]] || iso="$start/$iso"   # relative to where you ran this
  [[ -f $iso ]] || die "no file at $iso"
  h2 live assets extract --disc "$iso" --out "$here/art"
  [[ -f art/manifest.json ]] || die "extraction finished without art/manifest.json"
fi

say "Building and starting (the first build takes a while)"
docker compose up -d --build

say "Waiting for $url"
for _ in $(seq 1 60); do
  if curl -fsS -m 5 "$url/healthz" >/dev/null 2>&1; then echo "up"; up=1; break; fi
  sleep 3
done
if [[ -z ${up:-} ]]; then
  docker compose ps
  docker compose logs --tail 40 server cloudflared
  die "$url didn't answer in 3 minutes (logs above)"
fi

invite_only="$(sed -n 's/^H2_LIVE_INVITE_ONLY=//p' .env)"
if [[ -n $invite_only ]]; then
  say "First invite"
  h2 live invite create --server "$url" --data "$here/data" \
    || echo "couldn't make an invite; run: h2 live invite create --server $url --data $here/data"
  first="sign up with the invite code above"
else
  first="sign up"
fi

say "Done"
cat <<TXT
Open $url, $first using one of your admin gamertags, and you're the
admin. Sign up as your admins soon: until then anyone could take those
names. Invites: the Server screen, or h2 live invite create.

Logs:     cd $here && docker compose logs -f server
Update:   git pull && cd $here && docker compose up -d --build
Backup:   tar czf h2live-data.tgz -C $here data   (the key; live.db too unless
          Litestream backs it up: COMPOSE_PROFILES=backup)
TXT
