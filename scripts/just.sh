# Helpers for the root justfile (docs/runtime/building.md). Sourced by its
# recipes, from the repository root; not run on its own.
#
# Recipes take key=value arguments (`just tvos maps=all module=skip`).
# h2_args reads them into shell variables named after the key, refusing
# keys the recipe does not take:
#
#   h2_args "maps module" "$@"
#
# The inputs (`iso`, `mappack`) come from, in order: the recipe's argument,
# the justfile variable (`just iso=PATH tvos`, exported as $iso/$mappack,
# set from H2_ISO, else H2_DISC, and H2_MAPPACK), then disc/halo2.iso and
# disc/mappack.iso when they exist.

h2_die() {
  echo "just: $*" >&2
  exit 2
}

h2_args() {
  local allowed=" $1 " a key
  shift
  for a in "$@"; do
    case "$a" in
      *=*) key=${a%%=*} ;;
      *) h2_die "expected key=value, not '$a' (this recipe takes:$allowed)" ;;
    esac
    case "$allowed" in
      *" $key "*) ;;
      *) h2_die "unknown argument '$key' (this recipe takes:$allowed)" ;;
    esac
    printf -v "$key" '%s' "${a#*=}"
  done
}

# An absolute path for $1, which must exist.
h2_abs() {
  (cd "$(dirname "$1")" && printf '%s/%s\n' "$(pwd -P)" "$(basename "$1")")
}

# Sets $iso to the Halo 2 disc image's absolute path, or stops with how to
# give one.
h2_iso() {
  if [ -z "${iso:-}" ] && [ -f disc/halo2.iso ]; then
    iso=disc/halo2.iso
  fi
  if [ -z "${iso:-}" ]; then
    h2_die "no Halo 2 disc image: put yours at disc/halo2.iso (cp -c ~/Downloads/your.iso disc/halo2.iso), or pass iso=PATH, or set H2_ISO"
  fi
  [ -f "$iso" ] || h2_die "no disc image at $iso (from iso=, H2_ISO or H2_DISC)"
  iso=$(h2_abs "$iso")
}

# Sets $mappack to the Multiplayer Map Pack disc's absolute path, or to
# nothing when there is none. H2_MAPPACK=0/false/no/off means none, as it
# does for `h2 play`.
h2_mappack() {
  case "$(printf '%s' "${mappack:-}" | tr '[:upper:]' '[:lower:]')" in
    0 | false | no | off)
      mappack=
      return ;;
  esac
  if [ -z "${mappack:-}" ] && [ -f disc/mappack.iso ]; then
    mappack=disc/mappack.iso
  fi
  if [ -n "${mappack:-}" ]; then
    [ -f "$mappack" ] || h2_die "no map pack disc at $mappack (from mappack= or H2_MAPPACK)"
    mappack=$(h2_abs "$mappack")
  fi
}

# Checks maps= and module=, and fills $build_args for platform/*/build.sh.
h2_build_args() {
  build_args=()
  case "${module:-build}" in
    build) ;;
    skip) build_args+=(--no-module) ;;
    *) h2_die "module must be build or skip, not '$module'" ;;
  esac
  case "${maps:-demo}" in
    demo) ;;
    all) build_args+=(--maps all) ;;
    *) h2_die "maps must be demo or all, not '$maps'" ;;
  esac
}

# The paired physical devices for platform $1 (tvOS or iOS) that are not
# unavailable, one name per line.
h2_devices() {
  local json
  json=$(mktemp "${TMPDIR:-/tmp}/h2devices.XXXXXX")
  if xcrun devicectl list devices --quiet --json-output "$json" >/dev/null 2>&1; then
    /usr/bin/python3 -I - "$json" "$1" <<'EOF'
import json, sys
for d in json.load(open(sys.argv[1]))["result"]["devices"]:
    hw, conn = d.get("hardwareProperties", {}), d.get("connectionProperties", {})
    if hw.get("platform") != sys.argv[2] or hw.get("reality") == "simulated":
        continue
    if conn.get("pairingState") != "paired" or conn.get("tunnelState") == "unavailable":
        continue
    print(d["deviceProperties"]["name"])
EOF
  fi
  rm -f "$json"
}

# Sets $device for platform $1 (tvOS or iOS): device= if given, else the one
# paired device, else stops with the list.
h2_device() {
  [ -n "${device:-}" ] && return
  local found
  found=$(h2_devices "$1")
  case "$(printf '%s' "$found" | grep -c .)" in
    1) device=$found ;;
    0) h2_die "no paired $1 device is available; pair one (docs/runtime/$(echo "$1" | tr '[:upper:]' '[:lower:]').md) and check \`xcrun devicectl list devices\`" ;;
    *) h2_die "more than one $1 device; pick one with device=NAME:
$found" ;;
  esac
}

# The app's bundle id: PRODUCT_BUNDLE_IDENTIFIER from $1/Local.xcconfig when
# set there, else $2.
h2_bundle_id() {
  local id
  id=$(sed -n 's/^[[:space:]]*PRODUCT_BUNDLE_IDENTIFIER[[:space:]]*=[[:space:]]*\([^[:space:]]*\).*/\1/p' "$1/Local.xcconfig" 2>/dev/null | tail -1)
  printf '%s\n' "${id:-$2}"
}

# Stops unless platform/xbox is on this checkout.
h2_xbox() {
  [ -d platform/xbox ] || h2_die "platform/xbox is not on this checkout yet (the Xbox app is still on its branch)"
}

h2_size_warning() {
  local target=$1
  [ "${maps:-demo}" = all ] || return 0
  echo "just: maps=all bundles the whole disc${mappack:+ and the map pack}: 7 GB or more in platform/$target/build, the app and on the device."
  if [ "$target" = tvos ]; then
    echo "just: on tvOS the map pack is copied into Caches, which tvOS may purge when short of space; the app copies it back at the next launch."
  fi
  if [ "$target" = xbox ]; then
    echo "just: on Xbox the map pack is copied into LocalState at the first launch (NTFS does not clone), so that launch takes longer."
  fi
  if [ "${maps:-demo}" = all ] && [ -z "${mappack:-}" ]; then
    echo "just: no map pack disc (disc/mappack.iso, mappack= or H2_MAPPACK): the disc's own maps only."
  fi
}
