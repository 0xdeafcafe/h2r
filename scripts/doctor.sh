#!/usr/bin/env bash
# `just doctor`: what each build target needs, what is missing, and the
# command that installs it (docs/runtime/building.md). Reads only; changes
# nothing. Exits 0 even when something is missing: a target you don't
# build doesn't need its tools.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# The checkout you are in (its disc/ and Local.xcconfig files), else this
# script's own.
cd "$(git rev-parse --show-toplevel 2>/dev/null || echo "$here/..")" || exit 1
. "$here/just.sh"
iso=${iso:-${H2_ISO:-${H2_DISC:-}}}
mappack=${mappack:-${H2_MAPPACK:-}}

missing=0
ok() { printf '  ok       %s\n' "$1"; }
miss() {
  printf '  MISSING  %s\n' "$1"
  [ -n "${2:-}" ] && printf '           %s\n' "$2"
  missing=$((missing + 1))
}
note() { printf '  note     %s\n' "$1"; }
have() { command -v "$1" >/dev/null 2>&1; }
section() { printf '\n%s\n' "$1"; }

# Free space, in GB, on the volume holding $1.
free_gb() { df -Pk "$1" 2>/dev/null | awk 'NR==2 { printf "%d", $4 / 1048576 }'; }

section "Inputs"
(
  h2_iso 2>/dev/null && echo "  ok       disc: $iso"
) || miss "the Halo 2 disc image" "cp -c ~/Downloads/your.iso disc/halo2.iso (or set H2_ISO)"
(
  h2_mappack 2>/dev/null
  if [ -n "$mappack" ]; then echo "  ok       map pack: $mappack"; else
    echo "  note     no map pack disc (optional, for maps=all): cp -c ~/Downloads/mappack.iso disc/mappack.iso"
  fi
) || miss "the map pack image named by H2_MAPPACK" "check the path in H2_MAPPACK"

section "Mac (just mac, mac-play)"
want_rust=$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)
if have cargo && have rustc; then
  got_rust=$(rustc -V | awk '{print $2}')
  if [ "$(printf '%s\n%s\n' "$want_rust" "$got_rust" | sort -V | head -1)" = "$want_rust" ]; then
    ok "rustc $got_rust (wants $want_rust or newer)"
  else
    miss "rustc $want_rust or newer (have $got_rust)" "brew upgrade rust, or rustup update"
  fi
else
  miss "Rust (cargo, rustc)" "brew install rust, or https://rustup.rs"
fi
if have clang; then ok "clang ($(command -v clang))"; else miss "clang" "xcode-select --install"; fi

section "Apple TV and iPhone/iPad (just tvos, ios, tvos-install, ios-install)"
dev=$(xcode-select -p 2>/dev/null || true)
case "$dev" in
  */Xcode*.app/*) ok "Xcode ($dev)" ;;
  "") miss "Xcode" "install Xcode from the App Store, then: sudo xcode-select -s /Applications/Xcode.app" ;;
  *) miss "Xcode is not selected (xcode-select points at $dev)" "sudo xcode-select -s /Applications/Xcode.app" ;;
esac
for sdk in appletvos:tvOS iphoneos:iOS; do
  name=${sdk#*:} sdk=${sdk%%:*}
  if v=$(xcrun --sdk "$sdk" --show-sdk-version 2>/dev/null); then
    ok "$sdk SDK $v"
  else
    miss "the $sdk SDK" "xcodebuild -downloadPlatform $name (or Xcode > Settings > Components)"
  fi
done
if have rustc && [ -d "$(rustc --print sysroot)/lib/rustlib/src/rust/library" ]; then
  ok "rust-src (for -Zbuild-std)"
else
  miss "rust-src (std from source for the device targets)" "rustup component add rust-src (Homebrew's rust ships it)"
fi
for p in tvos ios; do
  team=$(sed -n 's/^[[:space:]]*DEVELOPMENT_TEAM[[:space:]]*=[[:space:]]*\([A-Z0-9]*\).*/\1/p' "platform/$p/Local.xcconfig" 2>/dev/null | tail -1)
  if [ -n "$team" ]; then
    ok "signing team $team (platform/$p/Local.xcconfig)"
  else
    miss "a signing team for $p" "echo 'DEVELOPMENT_TEAM = <your team id>' > platform/$p/Local.xcconfig (team id: security find-identity -v -p codesigning)"
  fi
done
if have security && security find-identity -v -p codesigning 2>/dev/null | grep -q 'Apple Development'; then
  ok "an Apple Development signing identity"
else
  miss "an Apple Development signing identity" "Xcode > Settings > Accounts: sign in and add your team"
fi
for plat in tvOS iOS; do
  found=$(h2_devices "$plat" | paste -sd, - | sed 's/,/, /g')
  if [ -n "$found" ]; then
    ok "paired $plat device(s): $found"
  else
    miss "a paired $plat device" "pair it in Xcode > Window > Devices and Simulators; xcrun devicectl list devices should show it (docs/runtime/$(echo $plat | tr '[:upper:]' '[:lower:]').md)"
  fi
done
if have xcodegen; then ok "xcodegen (only to regenerate the Xcode projects)"; else
  note "xcodegen is not installed; only needed to regenerate the Xcode projects (brew install xcodegen)"
fi

section "Windows (just windows-dll, check-windows)"
llvm=${H2_LLVM:-/opt/homebrew/opt/llvm@22/bin}
lld=${H2_LLD:-/opt/homebrew/opt/lld@22/bin}
xwin_dir=${H2_XWIN:-${TMPDIR%/}/xwin}
if [ -x "$llvm/clang-cl" ]; then ok "llvm@22 ($llvm)"; else miss "llvm@22 (clang-cl, llvm-lib)" "brew install llvm@22"; fi
if [ -x "$lld/lld-link" ]; then ok "lld@22 ($lld)"; else miss "lld@22 (lld-link)" "brew install lld@22"; fi
if have xwin; then ok "xwin"; else miss "xwin" "brew install xwin"; fi
if [ -d "$xwin_dir/crt/include" ] && [ -d "$xwin_dir/sdk/include" ]; then
  ok "the xwin splat ($xwin_dir)"
else
  miss "the MSVC CRT and Windows SDK splat in $xwin_dir" "xwin --accept-license --arch x86_64 --cache-dir \"\$TMPDIR/xwin-cache\" splat --output \"\$TMPDIR/xwin\" (about 650 MB; or set H2_XWIN)"
fi

section "Xbox (just xbox, xbox-deploy)"
if [ -d platform/xbox ]; then ok "platform/xbox"; else note "platform/xbox is not on this checkout yet; the recipes say so until it is merged"; fi
makemsix=${H2_MAKEMSIX:-$(command -v makemsix || echo "${TMPDIR%/}/msix-tools/bin/makemsix")}
if [ -x "$makemsix" ]; then ok "makemsix ($makemsix)"; else
  miss "makemsix" "build microsoft/msix-packaging with pack support and set H2_MAKEMSIX (docs/runtime/building.md)"
fi
if have osslsigncode; then ok "osslsigncode"; else miss "osslsigncode" "brew install osslsigncode"; fi
if have openssl; then ok "openssl"; else miss "openssl" "brew install openssl"; fi
if [ -n "${H2_XBOX:-}" ]; then ok "console address H2_XBOX=$H2_XBOX"; else
  note "no H2_XBOX; pass ip=ADDRESS to just xbox-deploy"
fi

section "Disk"
# Rough needs: h2 build about 3 GB in $TMPDIR; a device build about 2.5 GB
# (maps=demo) or 9 GB (maps=all) in platform/<target>/build, plus about
# 1.2 GB of C and objects in $TMPDIR/h2<target>-gen.
here_gb=$(free_gb .)
tmp_gb=$(free_gb "${TMPDIR:-/tmp}")
for pair in "repository:$here_gb:12" "TMPDIR:$tmp_gb:5"; do
  IFS=: read -r name gb need <<<"$pair"
  if [ -z "$gb" ]; then
    note "could not read the free space for $name"
  elif [ "$gb" -ge "$need" ]; then
    ok "$gb GB free for $name"
  else
    miss "$need GB or more free for $name (have $gb GB)" "just clean-device-builds, and delete \$TMPDIR/h2*-gen"
  fi
done

printf '\n'
if [ "$missing" -eq 0 ]; then
  echo "doctor: everything is in place"
else
  echo "doctor: $missing missing; each line above says how to install it"
fi
