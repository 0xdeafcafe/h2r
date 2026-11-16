#!/bin/sh
# Fills platform/tvos/build/ for the Xcode project (docs/tvos.md):
#
#   platform/tvos/build.sh /abs/path/to/disc/halo2.iso [--no-module]
#
#   build/libtvos.a          the runtime (crates/tvos), cross-compiled for tvOS
#   build/h2game.framework   the game, lifted, emitted and compiled for tvOS
#   build/disc/              the disc files the app ships (about 1 GB)
#   build/bootcache/         the XBE's analysis, so the first launch skips it
#
# --no-module keeps an existing h2game.framework (the slow part: about
# 400 MB of C and several minutes of clang; rebuild it whenever the
# generator or the runtime ABI changes, or the app refuses the stamp).
#
# aarch64-apple-tvos is a tier 3 Rust target: std is built from source
# (-Zbuild-std) with the pinned compiler and RUSTC_BOOTSTRAP=1, which needs
# the rust-src component (`rustup component add rust-src`; Homebrew's rust
# ships it). The Mac build is unaffected.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
iso=${1:?usage: build.sh /abs/path/to/halo2.iso [--no-module]}
shift
pack_args=
for a in "$@"; do
  case "$a" in
    --no-module) pack_args=--no-module ;;
    *) echo "build.sh: unknown argument $a" >&2; exit 2 ;;
  esac
done
out=$here/build
mkdir -p "$out"
# Separate target directories: the cross build's RUSTC_BOOTSTRAP would
# otherwise invalidate the host build's artifacts, and back.
tvos_target=${H2_TVOS_TARGET_DIR:-$root/target/tvos-cross}
host_target=${H2_TVOS_HOST_TARGET_DIR:-$root/target/tvos-host}
# Keep in step with emit::compile::TVOS_DEPLOYMENT_TARGET and project.yml.
export TVOS_DEPLOYMENT_TARGET=17.0

echo "build.sh: runtime for aarch64-apple-tvos"
(cd "$root" && RUSTC_BOOTSTRAP=1 CARGO_TARGET_DIR="$tvos_target" \
  cargo build -Zbuild-std=std,panic_abort --target aarch64-apple-tvos --release -p tvos)
cp "$tvos_target/aarch64-apple-tvos/release/libtvos.a" "$out/libtvos.a"

echo "build.sh: disc subset and game module"
(cd "$root" && CARGO_TARGET_DIR="$host_target" \
  cargo run --release -p tvos --example pack -- "$iso" "$out" $pack_args)

if [ ! -f "$out/h2game.framework/h2game" ]; then
  echo "build.sh: no build/h2game.framework yet; run without --no-module once" >&2
  exit 1
fi
du -sh "$out"/* | sed 's/^/build.sh: /'
echo "build.sh: done; open platform/tvos/H2TV.xcodeproj, or see docs/tvos.md for xcodebuild"
