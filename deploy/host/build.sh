#!/usr/bin/env bash
# Cross-builds the dedicated host for Linux x86-64 from a Mac (arm64 or x86_64)
# and stages everything the container image needs into deploy/host/stage/
# (docs/live/dedicated-host.md, "Container image"):
#
#   stage/h2            the Linux h2, host profile (--no-default-features
#                       --features host: no window, Metal, AVFoundation,
#                       input devices or app icons)
#   stage/image/        the game module this checkout's macOS h2 compiles from
#                       YOUR disc (libh2game.so or libh2game.dylib, whichever
#                       the Linux emit target names, plus its .stamp), and
#                       default.xbe (the runtime needs the image the module was
#                       built from to boot and check the stamp)
#
# Nothing else goes in the image: maps and shared disc data are fetched at
# start from the owner's private bundle (H2_DATA_URL/H2_DATA_SHA).
#
# Usage:
#   deploy/host/build.sh [<disc.iso|default.xbe>] [--out DIR] [--jobs N]
#                        [--skip-zigbuild] [-- <h2 build args>...]
#
#   disc       the disc to compile the module from; else $H2_DISC
#   --out      where `h2 build` writes (default deploy/host/image-out,
#              gitignored; kept between runs so the stamp skips a rebuild)
#   --jobs     clang parallelism for the module
#   --skip-zigbuild  stage only the module (the Linux h2 is already current)
#
# Environment:
#   H2_BIN             use this macOS h2 for `h2 build` instead of building
#                      ./target/release/h2
#   H2_ZIGBUILD_TARGET cargo target for the Linux h2
#                      (default x86_64-unknown-linux-gnu.2.36; the .2.36 pins
#                      glibc to debian bookworm's, the Dockerfile's base)
#   H2_MODULE_TARGET   how `h2 build` picks the Linux module target.
#                      GUESS (dh-linux-emit, contract 10): the repo's existing
#                      seam is H2_EMIT_TARGET — windows, tvos, ios
#                      (crates/emit/src/compile.rs, target_from) — so "linux"
#                      is the expected value and the default here. If
#                      dh-linux-emit instead ships a --arch flag on
#                      `h2 build`, set H2_BUILD_ARCH (below) and leave this
#                      alone.
#   H2_BUILD_ARCH      when set, passed to `h2 build` as --arch (e.g.
#                      x86_64-linux) instead of the H2_EMIT_TARGET env
#
# Prerequisites this script checks but never installs (it says what's missing):
#   cargo install cargo-zigbuild
#   brew install zig          (or: python3 -m pip install ziglang)
#   rustup target add x86_64-unknown-linux-gnu
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
cd "$root"

die() { printf 'build.sh: error: %s\n' "$*" >&2; exit 1; }
note() { printf 'build.sh: %s\n' "$*"; }

# --- arguments -------------------------------------------------------------

disc=${H2_DISC:-}
out="$here/image-out"
jobs=
skip_zigbuild=false
extra=()

while [ $# -gt 0 ]; do
  case $1 in
    --out) [ $# -ge 2 ] || die "--out needs a directory"; out=$2; shift 2 ;;
    --jobs) [ $# -ge 2 ] || die "--jobs needs a count"; jobs=$2; shift 2 ;;
    --skip-zigbuild) skip_zigbuild=true; shift ;;
    --) shift; extra=("$@"); break ;;
    -*) die "unknown option $1" ;;
    *) [ -z "$disc" ] || die "pass one disc (or set H2_DISC), not: $1"
       disc=$1; shift ;;
  esac
done
[ -n "$disc" ] || die "pass the disc (halo2.iso or a default.xbe) or set H2_DISC"
[ -f "$disc" ] || die "no such file: $disc"

target=${H2_ZIGBUILD_TARGET:-x86_64-unknown-linux-gnu.2.36}
# The plain triple names the artifact dir; cargo-zigbuild parses the .glibc
# suffix off the target it is given.
triple=${target%%.*}

# --- prerequisites (checked, never installed) -------------------------------

if ! cargo zigbuild --version >/dev/null 2>&1; then
  cat >&2 <<'EOF'
build.sh: cargo zigbuild is missing. Install it with:

    cargo install cargo-zigbuild

EOF
  exit 1
fi

# zig comes either as a binary on PATH or as the ziglang Python package
# cargo-zigbuild falls back to.
if ! zig version >/dev/null 2>&1 && ! python3 -c 'import ziglang' >/dev/null 2>&1; then
  cat >&2 <<'EOF'
build.sh: zig is missing (cargo-zigbuild drives it for the cross C toolchain).
Install one of:

    brew install zig
    python3 -m pip install ziglang

EOF
  exit 1
fi

if command -v rustup >/dev/null 2>&1 \
  && ! rustup target list --installed | grep -qx "$triple"; then
  cat >&2 <<EOF
build.sh: the rust standard library for $triple is missing. Install it with:

    rustup target add $triple

EOF
  exit 1
fi

# --- the Linux h2 ------------------------------------------------------------

linux_h2="${CARGO_TARGET_DIR:-$root/target}/$triple/release/h2"
if [ "$skip_zigbuild" = false ]; then
  note "cross-building h2 for $target (host profile)"
  cargo zigbuild --release --locked -p cli \
    --no-default-features --features host --target "$target"
  [ -f "$linux_h2" ] || die "zigbuild finished but $linux_h2 is not there"
fi
[ -f "$linux_h2" ] || die "$linux_h2 is missing (run without --skip-zigbuild)"
magic="$(head -c 4 "$linux_h2" | od -An -tx1 | tr -d ' \n')"
[ "$magic" = "7f454c46" ] || die "$linux_h2 is not an ELF binary (magic $magic)"

# --- the module, from this checkout's macOS h2 --------------------------------
#
# The tool that runs `h2 build` is a host build (as the Windows DLL cross in
# the justfile): only the C it drives targets Linux.

if [ -n "${H2_BIN:-}" ]; then
  h2_bin=$H2_BIN
else
  note "building the macOS h2 that compiles the module"
  cargo build --release --locked -p cli
  h2_bin="${CARGO_TARGET_DIR:-$root/target}/release/h2"
fi
[ -x "$h2_bin" ] || die "no runnable h2 at $h2_bin"

jobs_args=()
if [ -n "$jobs" ]; then
  jobs_args=(--jobs "$jobs")
fi

# How `h2 build` selects the Linux module target. The default follows the
# repo's H2_EMIT_TARGET seam (see the environment section above): the guess is
# that dh-linux-emit maps H2_EMIT_TARGET=linux like today's =windows. The
# H2_BUILD_ARCH knob covers a `--arch x86_64-linux` flag landing instead.
module_target_args=()
module_env=()
if [ -n "${H2_BUILD_ARCH:-}" ]; then
  module_target_args=(--arch "$H2_BUILD_ARCH")
else
  module_env=(H2_EMIT_TARGET="${H2_MODULE_TARGET:-linux}")
fi

# `h2 build` always builds; --check (exit 0 when the stamped module is
# current for this disc and this h2) keeps re-runs of this script cheap.
module_cmd=(
  "$h2_bin" build "$disc" --out "$out" --tier clang
  ${jobs_args[@]+"${jobs_args[@]}"}
  ${module_target_args[@]+"${module_target_args[@]}"}
  ${extra[@]+"${extra[@]}"}
)
if [ ${#module_env[@]} -gt 0 ]; then
  if env "${module_env[@]}" "${module_cmd[@]}" --check >/dev/null 2>&1; then
    note "module in $out is current (h2 build --check)"
  else
    note "compiling the Linux module from $disc into $out (the long step)"
    env "${module_env[@]}" "${module_cmd[@]}"
  fi
else
  if "${module_cmd[@]}" --check >/dev/null 2>&1; then
    note "module in $out is current (h2 build --check)"
  else
    note "compiling the Linux module from $disc into $out (the long step)"
    "${module_cmd[@]}"
  fi
fi

# --- stage ---------------------------------------------------------------------
#
# stage/ is this script's output and is gitignored; it is regenerated
# wholesale on every run.

stage="$here/stage"
rm -rf "$stage"
mkdir -p "$stage/image"

# The linked module's name is the emit target's choice (Arch::module_file_name):
# ELF .so on Linux is expected, the macOS dylib name is the fallback if
# dh-linux-emit kept it.
module=
for name in libh2game.so libh2game.dylib h2game.dll; do
  if [ -f "$out/$name" ]; then
    module=$name
    break
  fi
done
[ -n "$module" ] || die "no module (libh2game.so|libh2game.dylib|h2game.dll) in $out"

install -m 0755 "$linux_h2" "$stage/h2"
cp "$out/$module" "$stage/image/$module"
cp "$out/$module.stamp" "$stage/image/$module.stamp"
"$h2_bin" extract-xbe "$disc" "$stage/image/default.xbe" --force

note "staged $(du -sh "$stage/h2" | cut -f1) h2 and $(du -sh "$stage/image/$module" | cut -f1) $module into $stage"
note "next: cd $here && docker build -t h2host:local ."
