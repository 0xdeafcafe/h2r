#!/bin/sh
# Fills platform/<tvos|ios>/build/ for that Xcode project (docs/runtime/tvos.md,
# docs/runtime/ios.md):
#
#   platform/apple/build.sh tvos|ios /abs/path/to/disc/halo2.iso [--no-module]
#
#   build/libh2host.a        the runtime (crates/apple-host), cross-compiled
#   build/h2game.framework   the game, lifted, emitted and compiled for the device
#   build/disc/              the disc files the app ships (about 1 GB)
#   build/bootcache/         the XBE's analysis, so the first launch skips it
#
# --no-module keeps an existing h2game.framework (the slow part: about
# 400 MB of C and several minutes of clang; rebuild it whenever the
# generator or the runtime ABI changes, or the app refuses the stamp).
#
# std for aarch64-apple-tvos (tier 3) and aarch64-apple-ios is built from
# source (-Zbuild-std) with the pinned compiler and RUSTC_BOOTSTRAP=1, which
# needs the rust-src component (`rustup component add rust-src`; Homebrew's
# rust ships it, but no prebuilt std for either). The Mac build is unaffected.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
platform=${1:?usage: build.sh tvos|ios /abs/path/to/halo2.iso [--no-module]}
shift
iso=${1:?usage: build.sh tvos|ios /abs/path/to/halo2.iso [--no-module]}
shift
case "$platform" in
  # Keep the versions in step with emit::compile's *_DEPLOYMENT_TARGET and
  # each project.yml.
  tvos) triple=aarch64-apple-tvos; export TVOS_DEPLOYMENT_TARGET=17.0 ;;
  ios) triple=aarch64-apple-ios; export IPHONEOS_DEPLOYMENT_TARGET=17.0 ;;
  *) echo "build.sh: platform must be tvos or ios, not $platform" >&2; exit 2 ;;
esac
pack_args=
for a in "$@"; do
  case "$a" in
    --no-module) pack_args=--no-module ;;
    *) echo "build.sh: unknown argument $a" >&2; exit 2 ;;
  esac
done
out=$root/platform/$platform/build
mkdir -p "$out"
# Separate target directories: the cross build's RUSTC_BOOTSTRAP would
# otherwise invalidate the host build's artifacts, and back.
cross_target=${H2_APPLE_TARGET_DIR:-$root/target/$platform-cross}
host_target=${H2_APPLE_HOST_TARGET_DIR:-$root/target/apple-host-pack}

echo "build.sh: runtime for $triple"
(cd "$root" && RUSTC_BOOTSTRAP=1 CARGO_TARGET_DIR="$cross_target" \
  cargo build -Zbuild-std=std,panic_abort --target "$triple" --release -p apple-host)
cp "$cross_target/$triple/release/libh2host.a" "$out/libh2host.a"

echo "build.sh: disc subset and game module"
(cd "$root" && CARGO_TARGET_DIR="$host_target" \
  cargo run --release -p apple-host --example pack -- "$iso" "$out" --platform "$platform" $pack_args)

if [ ! -f "$out/h2game.framework/h2game" ]; then
  echo "build.sh: no build/h2game.framework yet; run without --no-module once" >&2
  exit 1
fi
du -sh "$out"/* | sed 's/^/build.sh: /'
echo "build.sh: done; open the platform/$platform Xcode project, or see docs/$platform.md for xcodebuild"
