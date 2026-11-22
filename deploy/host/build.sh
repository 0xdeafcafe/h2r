#!/usr/bin/env bash
# Run from the repository root. Stages the Linux host binary (public code;
# CI builds it on every deploy) and the game module compiled from the
# owner's disc (macOS only; it goes into the private host-data bundle,
# never into the image). docs/live/dedicated-host-build.md
set -euo pipefail

die() {
  printf 'build.sh: %s\n' "$*" >&2
  exit 1
}

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"

want_h2=true
want_module=true
disc=
out="$here/image-out"
jobs=
coverage=${H2_HOST_COVERAGE:-}
while [ "$#" -gt 0 ]; do
  case "$1" in
    --h2)
      want_module=false
      shift
      ;;
    --module)
      want_h2=false
      shift
      ;;
    --out)
      [ "$#" -ge 2 ] && [ -n "$2" ] || die "--out needs a directory"
      out=$2
      shift 2
      ;;
    --jobs)
      [ "$#" -ge 2 ] || die "--jobs needs a positive count"
      [[ "$2" =~ ^[1-9][0-9]*$ ]] || die "--jobs needs a positive count"
      jobs=$2
      shift 2
      ;;
    --coverage)
      [ "$#" -ge 2 ] && [ -n "$2" ] || die "--coverage needs an address list"
      coverage=$2
      shift 2
      ;;
    -h|--help)
      printf '%s\n' 'Usage: deploy/host/build.sh [--h2 | --module] [disc.iso|default.xbe] [--out DIR] [--jobs N] [--coverage FILE]' \
        '--h2 stages only the Linux h2 (deploy/host/stage/h2), from public code: no disc.' \
        '--module stages only the module (deploy/host/stage/image/) for the host-data bundle.' \
        'With neither it stages both.' \
        'The module needs H2_LINUX_SYSROOT, an absolute Debian bookworm amd64 sysroot path.' \
        'Set H2_DISC if omitting the disc and H2_BIN to use a Mac h2 from this revision.' \
        'The module is slim when a coverage list is given: --coverage, H2_HOST_COVERAGE,' \
        'or deploy/host/host-coverage.txt (private, ignored by Git) when it exists.'
      exit 0
      ;;
    -*) die "unknown option: $1" ;;
    *)
      [ -z "$disc" ] || die "pass one disc path"
      disc=$1
      shift
      ;;
  esac
done
[ "$want_h2" = true ] || [ "$want_module" = true ] || die "--h2 and --module exclude each other; pass neither to stage both"
case "$out" in /*) ;; *) out="$PWD/$out" ;; esac
stage="$here/stage"
triple=x86_64-unknown-linux-gnu

command -v cargo >/dev/null 2>&1 || die "install Rust and Cargo with rustup"
start=$PWD
cd "$root"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}"

# The Linux h2, linked against bookworm's glibc 2.36 with cargo-zigbuild on
# macOS and Linux alike, so the image's Debian base runs it.
stage_h2() {
  command -v cargo-zigbuild >/dev/null 2>&1 || die "run: cargo install cargo-zigbuild --locked (or pip install cargo-zigbuild)"
  command -v zig >/dev/null 2>&1 || die "install Zig (brew install zig, or pip install ziglang with a zig wrapper on PATH)"
  if command -v rustup >/dev/null 2>&1 &&
    ! rustup target list --installed | grep -Fx "$triple" >/dev/null; then
    die "run: rustup target add $triple"
  fi
  local strip
  strip=$(command -v llvm-strip || true)
  if [ -z "$strip" ] && [ "$(uname -s)" = Linux ]; then
    strip=$(command -v strip || true)
  fi
  [ -n "$strip" ] || die "llvm-strip is missing from PATH (brew install llvm@22)"
  cargo zigbuild --release --locked -p cli --no-default-features --features host \
    --target "$triple.2.36"
  local linux_h2="$CARGO_TARGET_DIR/$triple/release/h2"
  [ -x "$linux_h2" ] || die "no Linux h2 at $linux_h2"
  mkdir -p "$stage"
  install -m 0755 "$linux_h2" "$stage/h2.tmp"
  "$strip" --strip-unneeded "$stage/h2.tmp"
  mv -f "$stage/h2.tmp" "$stage/h2"
  wc -c "$stage/h2"
  printf 'Staged the Linux host in %s\n' "$stage/h2"
}

# The module from the owner's disc, emitted by a Mac h2 from this checkout.
# It must come from the same revision as the h2 that runs it: the host
# checks its stamp before it starts a game.
stage_module() {
  disc=${disc:-${H2_DISC:-}}
  if [ -z "$coverage" ] && [ -s "$here/host-coverage.txt" ]; then
    coverage="$here/host-coverage.txt"
  fi
  [ -n "$disc" ] || die "pass a disc path or set H2_DISC"
  case "$disc" in /*) ;; *) disc="$start/$disc" ;; esac
  [ -f "$disc" ] || die "no disc file at $disc"
  if [ -n "$coverage" ]; then
    case "$coverage" in /*) ;; *) coverage="$start/$coverage" ;; esac
    [ -s "$coverage" ] || die "no host coverage list at $coverage"
  fi
  [ "$(uname -s)" = Darwin ] || die "the module is built on macOS with the owner's disc"
  for tool in clang ld.lld llvm-ar llvm-strip; do
    command -v "$tool" >/dev/null 2>&1 ||
      die "$tool is missing from PATH. Install with brew install llvm@22 lld@22 and add their bin directories to PATH."
  done

  local sysroot_help="set H2_LINUX_SYSROOT to an absolute Debian bookworm amd64 sysroot path (install libc6-dev, linux-libc-dev and gcc in the sysroot)"
  local sysroot=${H2_LINUX_SYSROOT:-}
  case "$sysroot" in /*) ;; *) die "$sysroot_help" ;; esac
  local file
  for file in \
    usr/include/features.h \
    usr/include/linux/types.h \
    usr/lib/x86_64-linux-gnu/crti.o \
    usr/lib/x86_64-linux-gnu/crtn.o \
    usr/lib/x86_64-linux-gnu/libc.so \
    usr/lib/x86_64-linux-gnu/libm.so; do
    [ -f "$sysroot/$file" ] || die "sysroot is missing $file. $sysroot_help"
  done
  local gcc_found=false crt
  for crt in "$sysroot"/usr/lib/gcc/x86_64-linux-gnu/*/crtbeginS.o; do
    if [ -f "$crt" ] && [ -f "${crt%/*}/libgcc.a" ]; then
      gcc_found=true
      break
    fi
  done
  [ "$gcc_found" = true ] || die "sysroot is missing the amd64 GCC start files or libgcc. $sysroot_help"
  export H2_LINUX_SYSROOT="$sysroot"

  local h2_bin=${H2_BIN:-}
  if [ -n "$h2_bin" ]; then
    case "$h2_bin" in /*) ;; *) h2_bin="$start/$h2_bin" ;; esac
    [ -x "$h2_bin" ] || die "no runnable macOS h2 at $h2_bin"
  else
    cargo build --release --locked -p cli --bin h2
    h2_bin="$CARGO_TARGET_DIR/release/h2"
  fi

  local module_cmd=("$h2_bin" build "$disc" --arch x86_64-linux --tier clang --out "$out")
  if [ -n "$jobs" ]; then
    module_cmd+=(--jobs "$jobs")
  fi
  if [ -n "$coverage" ]; then
    printf 'Slim module from %s\n' "$coverage"
    module_cmd+=(--profile host --coverage "$coverage")
  fi
  local status=0
  "${module_cmd[@]}" --check || status=$?
  if [ "$status" -eq 0 ]; then
    printf 'Reusing the current module in %s\n' "$out"
  else
    [ "$status" -eq 3 ] || exit "$status"
    "${module_cmd[@]}"
  fi

  local module="$out/libh2game.so"
  [ -s "$module" ] || die "no Linux module at $module"
  [ -s "$module.stamp" ] || die "no Linux module stamp at $module.stamp"

  mkdir -p "$stage/image"
  [[ ! "$out" -ef "$stage/image" ]] || die "--out must be separate from the staged image"
  # Publish the stamp last so an interrupted copy can't leave a valid stage.
  rm -f "$stage/image/libh2game.so.stamp"
  install -m 0644 "$module" "$stage/image/libh2game.so"
  llvm-strip --strip-unneeded "$stage/image/libh2game.so"
  install -m 0644 "$module.stamp" "$stage/image/libh2game.so.stamp"
  wc -c "$stage/image/libh2game.so"
  printf 'Staged the module in %s; bundle it with h2 host bundle --image %s\n' "$stage/image" "$stage/image"
}

if [ "$want_h2" = true ]; then
  stage_h2
fi
if [ "$want_module" = true ]; then
  stage_module
fi
