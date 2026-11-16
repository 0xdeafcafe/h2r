#!/usr/bin/env bash
# One-time setup for the Live server behind a named Cloudflare tunnel
# (docs/live-hosting.md, "Docker with a named tunnel"). Asks for what it
# needs, writes deploy/live/.env, extracts the game art if it is missing,
# starts the containers, waits for the public URL and makes a first invite.
#
#   deploy/live/setup.sh
#
# Safe to run again: it keeps an existing .env unless you say otherwise and
# skips the art when deploy/live/art/manifest.json is already there.
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

h2() {
  if command -v h2 >/dev/null 2>&1; then
    command h2 "$@"
  else
    (cd "$root" && cargo run -q --release -p cli --bin h2 -- "$@")
  fi
}

say "Checking tools"
command -v docker >/dev/null || die "docker not found (install Docker Desktop)"
docker compose version >/dev/null 2>&1 || die "docker compose not found"
docker info >/dev/null 2>&1 || die "Docker isn't running: start Docker Desktop and run this again"
command -v openssl >/dev/null || die "openssl not found"
command -v curl >/dev/null || die "curl not found"
command -v h2 >/dev/null || command -v cargo >/dev/null \
  || die "neither h2 nor cargo found (needed to extract art and make invites)"
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

  umask 077
  cat > .env <<ENV
TUNNEL_TOKEN=$token
POSTGRES_PASSWORD=$(openssl rand -hex 24)
H2_LIVE_ADMINS=$admins
H2_LIVE_PUBLIC_URL=$url
H2_LIVE_DATA_DIR=./data
H2_LIVE_ART_DIR=./art
ENV
  umask 022
  echo "wrote $here/.env (only you can read it)"
fi

url="$(sed -n 's/^H2_LIVE_PUBLIC_URL=//p' .env)"; url="${url:-https://live.forbes.red}"
mkdir -p data art

if [[ ! -f art/manifest.json ]]; then
  say "Game art"
  echo "The server needs art extracted from your disc once (it is never committed)."
  ask iso "Path to your Halo 2 disc image (.iso)"
  iso="${iso/#\~/$HOME}"
  [[ $iso == /* ]] || iso="$start/$iso"   # relative to where you ran this
  [[ -f $iso ]] || die "no file at $iso"
  h2 live assets extract --disc "$iso" --out "$here/art"
  [[ -f art/manifest.json ]] || die "extraction finished without art/manifest.json"
else
  echo "art: already extracted"
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

say "First invite"
h2 live invite create --server "$url" --data "$here/data" \
  || echo "couldn't make an invite; run: h2 live invite create --server $url --data $here/data"

say "Done"
cat <<TXT
Open $url, sign up with the invite code above using one of your admin
gamertags, and you're the admin.

Logs:     cd $here && docker compose logs -f server
Update:   git pull && cd $here && docker compose up -d --build
Backup:   tar czf h2live-data.tgz -C $here data
          docker compose exec -T db pg_dump -U h2live h2live > h2live.sql
TXT
