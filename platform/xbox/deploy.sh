#!/bin/sh
# Installs and launches the Xbox package through the console's Device
# Portal (docs/runtime/xbox.md):
#
#   H2_XBOX=192.168.1.50 platform/xbox/deploy.sh [build/h2recomp.msix] [--no-launch]
#
# H2_XBOX is the console's address: a host or IP (https and port 11443 are
# assumed) or a full URL. The user is H2_XBOX_USER (default afr); the
# password comes from the macOS keychain item `h2-xbox-device-portal`
# (`security add-generic-password -s h2-xbox-device-portal -a afr -w`),
# and only ever goes into a mode-600 netrc file deleted on exit.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
. "$here/portal.sh"

pkg=$here/build/h2recomp.msix
launch=1
for a in "$@"; do
  case "$a" in
    --no-launch) launch=0 ;;
    -*) echo "deploy.sh: unknown option $a" >&2; exit 2 ;;
    *) pkg=$a ;;
  esac
done
: "${H2_XBOX:?set H2_XBOX to the console address, e.g. H2_XBOX=192.168.1.50}"
[ -f "$pkg" ] || { echo "deploy.sh: no package at $pkg; run platform/xbox/build.sh" >&2; exit 1; }
cer=${H2_XBOX_CER:-${pkg%.msix}.cer}
[ -f "$cer" ] || cer=
name=${H2_XBOX_PACKAGE_NAME:-h2recomp.Halo2}
user=${H2_XBOX_USER:-afr}

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

echo "deploy.sh: $PORTAL_BASE: checking the portal"
portal_curl GET /api/os/info >/dev/null
case $(portal_status) in
  200) ;;
  401) echo "deploy.sh: the portal refused user $user (check the keychain password)" >&2; exit 1 ;;
  *) echo "deploy.sh: the portal answered $(portal_status): $(cat "$PORTAL_BODY")" >&2; exit 1 ;;
esac

echo "deploy.sh: uploading $(basename "$pkg") ($(du -h "$pkg" | cut -f1))${cer:+ with $(basename "$cer")}"
portal_install "$pkg" "$cer" >/dev/null
case $(portal_status) in
  200|202|204) ;;
  *) echo "deploy.sh: install refused ($(portal_status)): $(cat "$PORTAL_BODY")" >&2; exit 1 ;;
esac

# 204 while it installs; then 200 with the result (or an error status).
while :; do
  portal_curl GET /api/app/packagemanager/state >/dev/null
  s=$(portal_status)
  [ "$s" = 204 ] || break
  sleep 2
done
if [ "$s" != 200 ] || jq -e '.Success == false' "$PORTAL_BODY" >/dev/null 2>&1; then
  echo "deploy.sh: install failed ($s): $(cat "$PORTAL_BODY")" >&2
  exit 1
fi
echo "deploy.sh: installed"

[ "$launch" = 1 ] || exit 0
portal_curl GET /api/app/packagemanager/packages >"$work/packages.json"
found=$(portal_find_package "$work/packages.json" "$name")
[ -n "$found" ] || { echo "deploy.sh: $name is not in the console's package list" >&2; exit 1; }
full=${found% *}
aumid=${found#* }
echo "deploy.sh: launching $aumid"
portal_curl POST "/api/taskmanager/app?appid=$(portal_b64 "$aumid")&package=$(portal_b64 "$full")" >/dev/null
case $(portal_status) in
  200|204) echo "deploy.sh: started; the log is LocalState/h2uwp.log in the portal's file explorer" ;;
  *) echo "deploy.sh: launch refused ($(portal_status)): $(cat "$PORTAL_BODY")" >&2; exit 1 ;;
esac
