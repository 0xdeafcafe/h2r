#!/usr/bin/env bash
# Tests the root justfile's argument handling (scripts/just.sh) end to end,
# without building anything: the justfile and its helper run in a scratch
# copy of the repository whose platform scripts, cargo, xcodebuild and xcrun
# only print what they were asked to do. No game data: the "discs" are
# empty files.
#
#   scripts/tests/just_test.sh
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
command -v just >/dev/null || { echo "just_test: needs just (brew install just)" >&2; exit 2; }
work=$(mktemp -d "${TMPDIR:-/tmp}/h2just.XXXXXX")
trap 'rm -rf "$work"' EXIT INT TERM

repo=$work/repo
mkdir -p "$repo/platform/macos" "$repo/scripts" "$repo/platform/tvos" "$repo/platform/ios" "$repo/platform/windows" "$repo/disc" "$work/bin"
cp "$root/justfile" "$repo/justfile"
cp "$root/scripts/just.sh" "$repo/scripts/just.sh"
stub() {
  printf '#!/bin/sh\necho "%s $*"\n' "$2" >"$1"
  chmod +x "$1"
}
stub "$repo/platform/macos/install.sh" "macos/install.sh"
stub "$repo/platform/tvos/build.sh" "tvos/build.sh"
stub "$repo/platform/ios/build.sh" "ios/build.sh"
stub "$work/bin/cargo" "cargo"
stub "$work/bin/xcodebuild" "xcodebuild"
# xcrun: devicectl's device list (one paired Apple TV, one iPhone that is
# not reachable, one simulator), else print.
cat >"$work/bin/xcrun" <<'EOF'
#!/bin/sh
if [ "$1 $2 $3" = "devicectl list devices" ]; then
  while [ $# -gt 0 ]; do [ "$1" = --json-output ] && out=$2; shift; done
  cat >"$out" <<'JSON'
{"result": {"devices": [
 {"deviceProperties": {"name": "Living Room"}, "hardwareProperties": {"platform": "tvOS", "reality": "physical"},
  "connectionProperties": {"pairingState": "paired", "tunnelState": "disconnected"}},
 {"deviceProperties": {"name": "Sim TV"}, "hardwareProperties": {"platform": "tvOS", "reality": "simulated"},
  "connectionProperties": {"pairingState": "paired", "tunnelState": "disconnected"}},
 {"deviceProperties": {"name": "Old iPad"}, "hardwareProperties": {"platform": "iOS", "reality": "physical"},
  "connectionProperties": {"pairingState": "paired", "tunnelState": "unavailable"}}
]}}
JSON
  exit 0
fi
echo "xcrun $*"
EOF
chmod +x "$work/bin/xcrun"
: >"$repo/disc/halo2.iso"
mkdir -p "$work/elsewhere"
: >"$work/elsewhere/pack.iso"

fails=0
# expect NAME WANT-SUBSTRING -- just ARGS...: runs just in the scratch repo
# and checks its combined output contains WANT.
expect() {
  local name=$1 want=$2 got
  shift 3
  got=$(cd "$repo" && env -u H2_ISO -u H2_DISC -u H2_MAPPACK PATH="$work/bin:$PATH" just "$@" 2>&1) || true
  if [[ "$got" == *"$want"* ]]; then
    echo "ok   $name"
  else
    echo "FAIL $name: wanted '$want' in:"
    printf '%s\n' "$got" | sed 's/^/     /'
    fails=$((fails + 1))
  fi
}
disc=$(cd "$repo/disc" && pwd -P)

expect "tvos defaults to the demo maps and disc/halo2.iso" "tvos/build.sh $disc/halo2.iso" -- tvos
expect "module=skip keeps the framework" "tvos/build.sh $disc/halo2.iso --no-module" -- tvos module=skip
expect "maps=all without a map pack is the disc alone" "tvos/build.sh $disc/halo2.iso --maps all" -- tvos maps=all
expect "maps=all warns about size" "7 GB or more" -- tvos maps=all
expect "tvOS warns that Caches can be purged" "Caches, which tvOS may purge" -- tvos maps=all
: >"$repo/disc/mappack.iso"
expect "maps=all takes disc/mappack.iso" "tvos/build.sh $disc/halo2.iso --maps all --mappack $disc/mappack.iso" -- tvos maps=all
expect "maps=demo leaves the map pack out" "ios/build.sh $disc/halo2.iso" -- ios maps=demo
expect "mappack= names another disc" "--mappack $(cd "$work/elsewhere" && pwd -P)/pack.iso" -- ios maps=all mappack="$work/elsewhere/pack.iso"
expect "mappack=off turns the map pack off" "no map pack disc" -- mappack=off ios maps=all
rm "$repo/disc/mappack.iso"
expect "iso= as a just variable" "no disc image at /none" -- iso=/none tvos
mv "$repo/disc/halo2.iso" "$work/halo2.iso"
expect "a missing disc says where to put it" "put yours at disc/halo2.iso" -- ios
mv "$work/halo2.iso" "$repo/disc/halo2.iso"
expect "bad maps value" "maps must be demo or all, not 'some'" -- tvos maps=some
expect "bad module value" "module must be build or skip" -- ios module=no
expect "bare word" "expected key=value, not 'all'" -- tvos all
expect "unknown key" "unknown argument 'device'" -- tvos device=x
expect "install needs a filled build" "run \`just tvos\` first" -- tvos-install
mkdir -p "$repo/platform/tvos/build/h2game.framework" "$repo/platform/ios/build/h2game.framework"
: >"$repo/platform/tvos/build/h2game.framework/h2game"
: >"$repo/platform/ios/build/h2game.framework/h2game"
expect "the one paired Apple TV is picked" "devicectl device install app --device Living Room" -- tvos-install
expect "device= is passed through" "platform=tvOS,name=Den" -- tvos-install device=Den
expect "an unreachable iPhone is not a device" "no paired iOS device is available" -- ios-install
expect "the launch uses the default bundle id" "launch --console --device Living Room red.forbes.h2recomp.tv" -- tvos-install
printf 'DEVELOPMENT_TEAM = X\nPRODUCT_BUNDLE_IDENTIFIER = com.example.tv // mine\n' >"$repo/platform/tvos/Local.xcconfig"
expect "Local.xcconfig's bundle id wins" "Living Room com.example.tv" -- tvos-install
expect "xbox is guarded until merged" "platform/xbox is not on this checkout yet" -- xbox
expect "xbox-deploy is guarded until merged" "platform/xbox is not on this checkout yet" -- xbox-deploy ip=10.0.0.2
mkdir -p "$repo/platform/xbox"
stub "$repo/platform/xbox/build.sh" "xbox/build.sh"
# shellcheck disable=SC2016 # expanded by the stub
printf '#!/bin/sh\necho "deploy.sh to $H2_XBOX $*"\n' >"$repo/platform/xbox/deploy.sh"
chmod +x "$repo/platform/xbox/deploy.sh"
expect "xbox runs build.sh once merged" "xbox/build.sh $disc/halo2.iso --no-module" -- xbox module=skip
expect "xbox maps=all without a map pack is the disc alone" "xbox/build.sh $disc/halo2.iso --maps all" -- xbox maps=all
: >"$repo/disc/mappack.iso"
expect "xbox maps=all takes disc/mappack.iso" "xbox/build.sh $disc/halo2.iso --maps all --mappack $disc/mappack.iso" -- xbox maps=all
expect "xbox warns that the map pack is copied" "copied into LocalState" -- xbox maps=all
expect "xbox maps=demo leaves the map pack out" "xbox/build.sh $disc/halo2.iso" -- xbox maps=demo
rm "$repo/disc/mappack.iso"
expect "xbox-deploy passes ip= as H2_XBOX" "deploy.sh to 10.0.0.2" -- xbox-deploy ip=10.0.0.2
expect "xbox-deploy needs an address" "no console address" -- xbox-deploy
expect "mac-install packages a reference to the selected disc" "macos/install.sh $disc/halo2.iso" -- mac-install
expect "mac builds with h2 build" "cargo h2 build $disc/halo2.iso" -- mac
expect "mac-play imports a map pack kept elsewhere" "cargo h2 import-mappack $(cd "$work/elsewhere" && pwd -P)/pack.iso --disc $disc/halo2.iso" -- mac-play mappack="$work/elsewhere/pack.iso"

if [ "$fails" -ne 0 ]; then
  echo "just_test: $fails failed"
  exit 1
fi
echo "just_test: all passed"
