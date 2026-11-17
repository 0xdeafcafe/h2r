#!/bin/sh
# Drives the Xbox app from the Mac (docs/runtime/xbox.md, "Remote control"
# and "Profiling on the console"): the Device Portal for files and
# launching, the app's agent API (tcp://<console>:4720) for everything
# while it runs.
#
#   H2_XBOX=10.0.3.29 platform/xbox/xbox.sh <command> [args]
#
#   log                       print LocalState\h2uwp.log
#   get NAME [OUT]            fetch LocalState\NAME (to OUT, else stdout)
#   put FILE [NAME]           upload FILE as LocalState\NAME
#   rm NAME                   delete LocalState\NAME
#   env [KEY=VALUE ...]       write LocalState\h2env (no pairs: delete it);
#                             the app reads it at launch
#   level MAP [DIFFICULTY]    boot straight into MAP next time (H2_LEVEL), and
#                             relaunch; `level menu` goes back to the menu
#   launch | stop | relaunch  the app
#   agent ARGS...             h2 agent --xbox <console> ARGS (status, press a,
#                             stick left 0 1 2000, hostperf, ...)
#   profile [SECONDS] [HZ]    sample the game thread (default 20 s, 1000 Hz),
#                             fetch LocalState\profile.h2prof and symbolise it
#   symbolize DUMP            symbolise a dump fetched earlier
#
# H2_XBOX is the console (as for deploy.sh); the portal user is
# H2_XBOX_USER (default afr), its password the keychain item
# h2-xbox-device-portal, kept in a mode-600 netrc file deleted on exit.
# Symbolising needs build/symbols/ from the build that is installed
# (build.sh fills it) and reads disc/ghidra-functions.tsv for names.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
. "$here/portal.sh"

usage() {
  sed -n '2,/^set -eu/p' "$0" | sed -e '/^set -eu/d' -e 's/^# \{0,1\}//' >&2
  exit 2
}

cmd=${1:-}
[ -n "$cmd" ] || usage
shift
name=${H2_XBOX_PACKAGE_NAME:-h2recomp.Halo2}
user=${H2_XBOX_USER:-afr}
symbols=${H2_XBOX_SYMBOLS:-$here/build/symbols}
# `h2 agent` and the symboliser; tests stand them in.
agent_cmd=${H2_XBOX_AGENT_CMD:-cargo run -q --release -p cli -- agent}
symbolize_cmd=${H2_XBOX_SYMBOLIZE_CMD:-cargo run -q --release -p perf --example remote_profile --}

agent() {
  # shellcheck disable=SC2086 # the command is words on purpose
  (cd "$root" && $agent_cmd --xbox "$H2_XBOX" "$@")
}

symbolize() {
  dump=$1
  set --
  for f in h2game.dll h2uwp.exe; do
    if [ -f "$symbols/$f" ]; then set -- "$@" --image "$symbols/$f"; fi
  done
  [ -f "$symbols/h2uwp.map" ] && set -- "$@" --map "$symbols/h2uwp.map"
  [ -f "$root/disc/ghidra-functions.tsv" ] && set -- "$@" --names "$root/disc/ghidra-functions.tsv"
  [ $# -gt 0 ] || echo "xbox.sh: no $symbols (run build.sh): frames stay module+offset" >&2
  # shellcheck disable=SC2086
  (cd "$root" && $symbolize_cmd "$dump" "$@")
}

case "$cmd" in
  symbolize) symbolize "${1:?symbolize needs a dump}"; exit $? ;;
  -h | --help | help) usage ;;
esac
: "${H2_XBOX:?set H2_XBOX to the console address, e.g. H2_XBOX=10.0.3.29}"
case "$cmd" in
  agent) agent "$@"; exit $? ;;
esac

PORTAL_BASE=$(portal_base "$H2_XBOX")
work=$(mktemp -d "${TMPDIR:-/tmp}/h2xbox.XXXXXX")
trap 'rm -rf "$work"' EXIT INT TERM
PORTAL_NETRC=$work/netrc
PORTAL_JAR=$work/cookies
PORTAL_BODY=$work/body
PORTAL_STATUS_FILE=$work/status
: >"$PORTAL_JAR"
security find-generic-password -s h2-xbox-device-portal -a "$user" -w |
  portal_netrc "$PORTAL_NETRC" "$(portal_host "$PORTAL_BASE")" "$user"

# The installed package: sets $full and $aumid.
package() {
  portal_curl GET /api/app/packagemanager/packages >/dev/null
  [ "$(portal_status)" = 200 ] || { echo "xbox.sh: the portal answered $(portal_status): $(cat "$PORTAL_BODY")" >&2; exit 1; }
  cp "$PORTAL_BODY" "$work/packages.json"
  found=$(portal_find_package "$work/packages.json" "$name")
  [ -n "$found" ] || { echo "xbox.sh: $name is not installed (deploy.sh)" >&2; exit 1; }
  full=${found% *}
  aumid=${found#* }
}

ok() {
  case $(portal_status) in
    200 | 202 | 204) ;;
    *) echo "xbox.sh: $1 refused ($(portal_status)): $(cat "$PORTAL_BODY")" >&2; exit 1 ;;
  esac
}

get() {
  portal_localstate_get "$full" "$1" >/dev/null
  ok "reading LocalState\\$1"
  if [ -n "${2:-}" ]; then cp "$PORTAL_BODY" "$2"; else cat "$PORTAL_BODY"; fi
}

put() {
  portal_localstate_put "$full" "$1" "$2" >/dev/null
  ok "writing LocalState\\$2"
}

remove() {
  portal_localstate_rm "$full" "$1" >/dev/null
  case $(portal_status) in
    200 | 204 | 404) ;;
    *) ok "deleting LocalState\\$1" ;;
  esac
}

stop_app() {
  portal_stop_app "$full" >/dev/null
  case $(portal_status) in
    200 | 204 | 404) ;;
    *) ok "stopping the app" ;;
  esac
}

launch_app() {
  portal_launch "$aumid" "$full" >/dev/null
  ok "launching the app"
  echo "xbox.sh: launched $aumid"
}

write_env() {
  if [ $# -eq 0 ]; then
    remove h2env
    echo "xbox.sh: removed LocalState\\h2env"
    return
  fi
  : >"$work/h2env"
  for kv in "$@"; do
    case "$kv" in
      H2_*=*) printf '%s\n' "$kv" >>"$work/h2env" ;;
      *) echo "xbox.sh: $kv is not H2_NAME=VALUE" >&2; exit 2 ;;
    esac
  done
  put "$work/h2env" h2env
  echo "xbox.sh: LocalState\\h2env: $*"
}

package
case "$cmd" in
  log) get h2uwp.log ;;
  get) get "${1:?get needs a file name}" "${2:-}" ;;
  put)
    f=${1:?put needs a file}
    put "$f" "${2:-$(basename "$f")}" ;;
  rm) remove "${1:?rm needs a file name}" ;;
  env) write_env "$@" ;;
  level)
    map=${1:?level needs a map name (03a_oldmombasa, ascension, menu)}
    if [ "$map" = menu ]; then
      write_env
    else
      write_env "H2_LEVEL=$map" ${2:+"H2_DIFFICULTY=$2"}
    fi
    stop_app
    launch_app ;;
  launch) launch_app ;;
  stop) stop_app; echo "xbox.sh: stopped" ;;
  relaunch) stop_app; launch_app ;;
  profile)
    secs=${1:-20}
    hz=${2:-1000}
    agent sample-start "$hz"
    echo "xbox.sh: sampling for $secs s"
    sleep "$secs"
    agent sample-stop
    out=${H2_XBOX_PROFILE_DIR:-${TMPDIR:-/tmp}}/xbox-$(date +%Y%m%d-%H%M%S).h2prof
    get profile.h2prof "$out"
    echo "xbox.sh: $out"
    symbolize "$out" ;;
  *) echo "xbox.sh: unknown command $cmd" >&2; usage ;;
esac
