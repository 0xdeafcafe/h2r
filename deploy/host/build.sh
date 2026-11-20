#!/usr/bin/env bash
# Run on macOS from the repository root. The build uses the owner's disc.
set -euo pipefail

die() {
  printf 'build.sh: %s\n' "$*" >&2
  exit 1
}

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"

disc=
out="$here/image-out"
jobs=
coverage=
while [ "$#" -gt 0 ]; do
  case "$1" in
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
      printf '%s\n' 'Usage: deploy/host/build.sh [disc.iso|default.xbe] [--out DIR] [--jobs N] [--coverage FILE]' \
        'Set H2_LINUX_SYSROOT to an absolute Debian bookworm amd64 sysroot path.' \
        'Set H2_DISC if omitting the disc and H2_BIN to use a Mac h2 from this revision.'
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
disc=${disc:-${H2_DISC:-}}
[ -n "$disc" ] || die "pass a disc path or set H2_DISC"
# Resolve paths before Cargo switches to the repository.
case "$disc" in /*) ;; *) disc="$PWD/$disc" ;; esac
case "$out" in /*) ;; *) out="$PWD/$out" ;; esac
[ -f "$disc" ] || die "no disc file at $disc"
if [ -n "$coverage" ]; then
  case "$coverage" in /*) ;; *) coverage="$PWD/$coverage" ;; esac
  [ -s "$coverage" ] || die "no host coverage list at $coverage"
fi
stage="$here/stage"

triple=x86_64-unknown-linux-gnu
command -v cargo >/dev/null 2>&1 || die "install Rust and Cargo with rustup"
command -v cargo-zigbuild >/dev/null 2>&1 || die "run: cargo install cargo-zigbuild --locked"
command -v zig >/dev/null 2>&1 || die "run: brew install zig"
if command -v rustup >/dev/null 2>&1 &&
  ! rustup target list --installed | grep -Fx "$triple" >/dev/null; then
  die "run: rustup target add $triple"
fi
for tool in clang ld.lld llvm-ar llvm-strip; do
  command -v "$tool" >/dev/null 2>&1 ||
    die "$tool is missing from PATH. Install with brew install llvm@22 lld@22 and add their bin directories to PATH."
done

sysroot_help="set H2_LINUX_SYSROOT to an absolute Debian bookworm amd64 sysroot path (install libc6-dev, linux-libc-dev and gcc in the sysroot)"
sysroot=${H2_LINUX_SYSROOT:-}
case "$sysroot" in /*) ;; *) die "$sysroot_help" ;; esac
for file in \
  usr/include/features.h \
  usr/include/linux/types.h \
  usr/lib/x86_64-linux-gnu/crti.o \
  usr/lib/x86_64-linux-gnu/crtn.o \
  usr/lib/x86_64-linux-gnu/libc.so \
  usr/lib/x86_64-linux-gnu/libm.so; do
  [ -f "$sysroot/$file" ] || die "sysroot is missing $file. $sysroot_help"
done
gcc_found=false
for crt in "$sysroot"/usr/lib/gcc/x86_64-linux-gnu/*/crtbeginS.o; do
  if [ -f "$crt" ] && [ -f "${crt%/*}/libgcc.a" ]; then
    gcc_found=true
    break
  fi
done
[ "$gcc_found" = true ] || die "sysroot is missing the amd64 GCC start files or libgcc. $sysroot_help"
export H2_LINUX_SYSROOT="$sysroot"

h2_bin=${H2_BIN:-}
if [ -n "$h2_bin" ]; then
  case "$h2_bin" in /*) ;; *) h2_bin="$PWD/$h2_bin" ;; esac
  [ -x "$h2_bin" ] || die "no runnable macOS h2 at $h2_bin"
fi

cd "$root"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}"
cargo zigbuild --release -p cli --no-default-features --features host \
  --target x86_64-unknown-linux-gnu.2.36
linux_h2="$CARGO_TARGET_DIR/$triple/release/h2"
[ -x "$linux_h2" ] || die "no Linux h2 at $linux_h2"

if [ -z "$h2_bin" ]; then
  cargo build --release --locked -p cli --bin h2
  h2_bin="$CARGO_TARGET_DIR/release/h2"
fi

module_cmd=("$h2_bin" build "$disc" --arch x86_64-linux --tier clang --out "$out")
if [ -n "$jobs" ]; then
  module_cmd+=(--jobs "$jobs")
fi
if [ -n "$coverage" ]; then
  module_cmd+=(--profile host --coverage "$coverage")
fi
if "${module_cmd[@]}" --check; then
  printf 'Reusing the current module in %s\n' "$out"
else
  status=$?
  [ "$status" -eq 3 ] || exit "$status"
  "${module_cmd[@]}"
fi

module="$out/libh2game.so"
[ -s "$module" ] || die "no Linux module at $module"
[ -s "$module.stamp" ] || die "no Linux module stamp at $module.stamp"

mkdir -p "$stage/image"
[[ ! "$out" -ef "$stage/image" ]] || die "--out must be separate from the staged image"
# Publish the stamp last so an interrupted copy can't leave a valid stage.
rm -f "$stage/image/libh2game.so.stamp"
install -m 0755 "$linux_h2" "$stage/h2"
install -m 0644 "$module" "$stage/image/libh2game.so"
llvm-strip --strip-unneeded "$stage/h2" "$stage/image/libh2game.so"
install -m 0644 "$module.stamp" "$stage/image/libh2game.so.stamp"
wc -c "$stage/h2" "$stage/image/libh2game.so"
printf 'Staged the Linux host and module in %s\n' "$stage"
