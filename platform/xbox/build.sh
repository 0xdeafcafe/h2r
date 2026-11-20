#!/bin/sh
# Builds the Xbox app's signed package, platform/xbox/build/h2recomp.msix
# (docs/runtime/xbox.md):
#
#   platform/xbox/build.sh /abs/path/to/disc/halo2.iso [--no-module]
#       [--maps demo|all] [--mappack /abs/path/to/mappack.iso]
#
#   build/package/           the package's contents: h2uwp.exe, h2game.dll
#                            (+ .stamp), disc/, bootcache/, AppxManifest.xml, Assets/
#   build/package/bootcache/h2data/
#                            with --maps all and --mappack: the map pack as
#                            nine 1.0 content packages (h2 import-mappack),
#                            copied into LocalState\h2data at the first launch
#   build/h2recomp.msix      the signed package
#   build/h2recomp.cer       its certificate (the console must trust it;
#                            deploy.sh sends it with the package)
#   build/h2recomp.aumid     the app's AUMID, for launching by hand
#   build/symbols/           this build's h2uwp.exe, its linker map
#                            (h2uwp.map) and h2game.dll, for symbolising
#                            console profiles (xbox.sh profile)
#
# --maps demo (the default) packs the menu, The Heretic and Zanzibar
# (h2host::layout::DISC_FILES, about 1 GB); --maps all packs the whole disc
# and, with --mappack, the Multiplayer Map Pack (7 GB or more), as
# platform/apple/build.sh does for tvOS and iOS. Going back to demo removes
# what all added.
#
# --no-module keeps an existing build/package/h2game.dll (the slow part:
# the whole-XBE lift, emit and about 16 minutes of clang). Rebuild it
# whenever the generator, the runtime ABI or Cargo.lock changes, or the app
# refuses the stamp at start-up (its log says so).
#
# Needs: platform/windows/env.sh's toolchain (llvm@22, lld@22, an xwin
# splat in H2_XWIN); makemsix from Microsoft's msix-packaging built with
# pack support (H2_MAKEMSIX, default makemsix on PATH, then
# $TMPDIR/msix-tools/bin/makemsix); osslsigncode and openssl (Homebrew).
#
# Signing: a self-signed code-signing certificate whose subject is the
# manifest's Publisher (CN=h2recomp-dev), made once and kept in
# H2_XBOX_KEYS (default $RUSH_KEEP/xbox-signing, else
# ~/Library/Application Support/h2recomp/xbox-signing). Never commit it.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
usage="usage: build.sh /abs/path/to/halo2.iso [--no-module] [--maps demo|all] [--mappack ISO]"
iso=${1:?$usage}
shift
pack_args=
maps=demo
mappack=
while [ $# -gt 0 ]; do
  case "$1" in
    --no-module) pack_args=--no-module ;;
    --maps)
      maps=${2:?build.sh: --maps needs demo or all}
      shift ;;
    --mappack)
      mappack=${2:?build.sh: --mappack needs a path}
      shift ;;
    *) echo "build.sh: unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done
case "$maps" in
  demo | all) ;;
  *) echo "build.sh: --maps must be demo or all, not $maps" >&2; exit 2 ;;
esac
if [ -n "$mappack" ]; then
  [ "$maps" = all ] || { echo "build.sh: --mappack needs --maps all" >&2; exit 2; }
  [ -f "$mappack" ] || { echo "build.sh: no map pack disc at $mappack" >&2; exit 2; }
fi
[ "${H2_XBOX_BUILD_DRY_RUN:-}" = 1 ] && { echo "build.sh: maps=$maps mappack=${mappack:-none} pack_args=${pack_args:-none}"; exit 0; }
publisher=CN=h2recomp-dev
out=${H2_XBOX_OUT:-$here/build}
stage=$out/package
keys=${H2_XBOX_KEYS:-${RUSH_KEEP:+$RUSH_KEEP/xbox-signing}}
keys=${keys:-$HOME/Library/Application Support/h2recomp/xbox-signing}
makemsix=${H2_MAKEMSIX:-$(command -v makemsix || echo "${TMPDIR%/}/msix-tools/bin/makemsix")}
[ -x "$makemsix" ] || { echo "build.sh: no makemsix (set H2_MAKEMSIX; docs/runtime/xbox.md)" >&2; exit 1; }
command -v osslsigncode >/dev/null || { echo "build.sh: no osslsigncode (brew install osslsigncode)" >&2; exit 1; }
# Each part increases, so an install over an earlier build is an update:
# 1.0.<yyddd>.<minute of the day>.
version=1.0.$(date -u +%y%j | sed 's/^0*//').$(( $(date -u +%H | sed 's/^0//') * 60 + $(date -u +%M | sed 's/^0//') ))
mkdir -p "$stage" "$out/symbols"

# The toolchain for the cross build and the module (H2_XWIN, H2_LLVM, ...).
. "$root/platform/windows/env.sh"
xbox_target=${H2_XBOX_TARGET_DIR:-$root/target/xbox}
host_target=${H2_XBOX_HOST_TARGET_DIR:-$root/target/xbox-host}

echo "build.sh: h2uwp.exe for x86_64-pc-windows-msvc (static CRT, app container)"
# The static CRT: an Xbox has no desktop vcruntime140.dll, and the Store
# one (vcruntime140_app, a framework package) is not in the xwin splat.
# /APPCONTAINER marks the executable for the UWP app container. wgpu's GL
# backend (never used: app::embedded::host_surface::backends) imports
# opengl32.dll and two gdi32 pixel-format calls, which the console may not
# have; delay-loading them keeps the loader from looking. /map writes the
# linker map that names its functions for console profiles (nothing in
# the executable changes).
(cd "$root" && CARGO_TARGET_DIR="$xbox_target" \
  CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="$CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS -C target-feature=+crt-static -C link-arg=/APPCONTAINER" \
  cargo rustc -Zbuild-std=std,panic_abort --target x86_64-pc-windows-msvc --release -p uwp-host --bin h2uwp -- \
  -C link-arg=/DELAYLOAD:opengl32.dll -C link-arg=/DELAYLOAD:gdi32.dll -C link-arg=delayimp.lib \
  "-C" "link-arg=/map:$out/symbols/h2uwp.map")
cp "$xbox_target/x86_64-pc-windows-msvc/release/h2uwp.exe" "$stage/h2uwp.exe"
cp "$stage/h2uwp.exe" "$out/symbols/h2uwp.exe"

echo "build.sh: disc subset, game module and boot cache"
(cd "$root" && env -u RUSTC_BOOTSTRAP CARGO_TARGET_DIR="$host_target" \
  cargo run --release -p h2host --example pack -- "$iso" "$stage" --platform xbox --maps "$maps" $pack_args)
[ -f "$stage/h2game.dll" ] || { echo "build.sh: no h2game.dll yet; run without --no-module once" >&2; exit 1; }
cp "$stage/h2game.dll" "$out/symbols/h2game.dll"

# The map pack lives on the hard disk, not the disc: the same import as
# `h2 play` does on the Mac, into the package's seed, which the app copies
# into LocalState\h2data at launch (h2host::maps, Layout::prepare).
seed=$stage/bootcache/h2data
rm -rf "$seed"
if [ -n "$mappack" ]; then
  echo "build.sh: map pack packages from $mappack"
  (cd "$root" && env -u RUSTC_BOOTSTRAP CARGO_TARGET_DIR="$host_target" \
    cargo run --release -q -p cli -- import-mappack "$mappack" --data "$seed" --no-update --force)
fi

echo "build.sh: manifest $version"
ids=$(cd "$root" && env -u RUSTC_BOOTSTRAP CARGO_TARGET_DIR="$host_target" \
  cargo run -q --release -p uwp-host --example package -- "$stage" --version "$version" --publisher "$publisher")
printf '%s\n' "$ids" | sed -n 's/^aumid=//p' >"$out/h2recomp.aumid"

echo "build.sh: app-container API check"
(cd "$root" && env -u RUSTC_BOOTSTRAP CARGO_TARGET_DIR="$host_target" \
  cargo run -q --release -p uwp-host --example apicheck -- \
  "$H2_XWIN/sdk/lib/um/x86_64/OneCoreUAP.Lib" -- "$stage/h2uwp.exe" "$stage/h2game.dll") ||
  { echo "build.sh: imports outside OneCoreUAP (above) may not exist on the console, and then the app cannot start; H2_XBOX_API_CHECK=warn to package anyway" >&2
    [ "${H2_XBOX_API_CHECK:-fail}" = warn ] || exit 1; }

if [ ! -f "$keys/h2recomp.pfx" ]; then
  echo "build.sh: making the signing certificate in $keys ($publisher)"
  mkdir -p "$keys"
  chmod 700 "$keys"
  printf '[req]\ndistinguished_name=dn\nprompt=no\nx509_extensions=v3\n[dn]\nCN=%s\n[v3]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=codeSigning\nsubjectKeyIdentifier=hash\n' \
    "${publisher#CN=}" >"$keys/cert.cnf"
  (umask 077 && openssl req -x509 -newkey rsa:2048 -sha256 -nodes -days 3650 -config "$keys/cert.cnf" \
    -keyout "$keys/key.pem" -out "$keys/cert.pem" 2>/dev/null)
  (umask 077 && openssl pkcs12 -export -inkey "$keys/key.pem" -in "$keys/cert.pem" -out "$keys/h2recomp.pfx" -passout pass:)
  openssl x509 -in "$keys/cert.pem" -outform der -out "$keys/h2recomp.cer"
fi

echo "build.sh: packing and signing"
rm -f "$out/h2recomp.msix" "$out/unsigned.msix"
"$makemsix" pack -d "$stage" -p "$out/unsigned.msix"
osslsigncode sign -pkcs12 "$keys/h2recomp.pfx" -pass "" -h sha256 -in "$out/unsigned.msix" -out "$out/h2recomp.msix" >/dev/null
rm -f "$out/unsigned.msix"
osslsigncode verify -CAfile "$keys/cert.pem" -in "$out/h2recomp.msix" >/dev/null
cp "$keys/h2recomp.cer" "$out/h2recomp.cer"
du -sh "$stage"/h2uwp.exe "$stage"/h2game.dll "$stage"/disc "$out/h2recomp.msix" | sed 's/^/build.sh: /'
echo "build.sh: done; AUMID $(cat "$out/h2recomp.aumid")"
echo "build.sh: deploy with H2_XBOX=<console address> platform/xbox/deploy.sh"
