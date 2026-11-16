# Cross-compilation environment for x86_64-pc-windows-msvc from a Mac
# (docs/windows.md). Source it:
#
#   . platform/windows/env.sh
#
# Needs Homebrew's llvm@22 (clang, clang-cl, llvm-lib), lld@22 (lld-link) and
# the MSVC CRT and Windows SDK splatted by xwin into $H2_XWIN (default
# $TMPDIR/xwin; about 650 MB, plus 1 GB of downloads in $H2_XWIN-cache):
#
#   brew install llvm@22 lld@22 xwin
#   xwin --accept-license --arch x86_64 --cache-dir "$TMPDIR/xwin-cache" \
#     splat --output "$TMPDIR/xwin"
#
# x86_64-pc-windows-msvc is a tier 1 target, but Homebrew's Rust ships only
# the host's std, so std is built from source (-Zbuild-std, RUSTC_BOOTSTRAP=1)
# as for tvOS; with rustup, `rustup target add x86_64-pc-windows-msvc` makes
# that unnecessary.
llvm=${H2_LLVM:-/opt/homebrew/opt/llvm@22/bin}
lld=${H2_LLD:-/opt/homebrew/opt/lld@22/bin}
xwin=${H2_XWIN:-${TMPDIR%/}/xwin}
if [ ! -d "$xwin/crt/include" ] || [ ! -x "$llvm/clang-cl" ] || [ ! -x "$lld/lld-link" ]; then
  echo "platform/windows/env.sh: need $llvm/clang-cl, $lld/lld-link and an xwin splat in $xwin" >&2
  return 1 2>/dev/null || exit 1
fi
export H2_XWIN="$xwin" H2_LLVM="$llvm" H2_LLD="$lld"
# cc-rs (build scripts' C: ring, zstd, ...) through clang-cl with the
# splat's headers.
inc="/imsvc$xwin/crt/include /imsvc$xwin/sdk/include/ucrt /imsvc$xwin/sdk/include/um /imsvc$xwin/sdk/include/shared"
export CC_x86_64_pc_windows_msvc="$llvm/clang-cl"
export CXX_x86_64_pc_windows_msvc="$llvm/clang-cl"
export AR_x86_64_pc_windows_msvc="$llvm/llvm-lib"
export CFLAGS_x86_64_pc_windows_msvc="--target=x86_64-pc-windows-msvc -Wno-unused-command-line-argument $inc"
export CXXFLAGS_x86_64_pc_windows_msvc="$CFLAGS_x86_64_pc_windows_msvc"
# rustc's link step (only for a real build, not `cargo check`).
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER="$lld/lld-link"
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-Lnative=$xwin/crt/lib/x86_64 -Lnative=$xwin/sdk/lib/um/x86_64 -Lnative=$xwin/sdk/lib/ucrt/x86_64"
export RUSTC_BOOTSTRAP=1
# A target directory of its own: RUSTC_BOOTSTRAP and -Zbuild-std would
# otherwise invalidate the host build's artifacts, and back.
export CARGO_TARGET_DIR="${H2_WINDOWS_TARGET_DIR:-$(git rev-parse --show-toplevel)/target/windows-cross}"
