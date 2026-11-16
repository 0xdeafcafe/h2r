# h2recomp: one front door for building every target from your own disc
# (docs/runtime/building.md). Install just with `brew install just`, then
# `just` lists the recipes and `just doctor` says what is missing.
#
# Recipes take key=value arguments: `just tvos maps=all module=skip`,
# `just tvos-install device="Living Room"`. The disc comes from iso=PATH,
# `just iso=PATH <recipe>`, H2_ISO (or H2_DISC), else disc/halo2.iso; the
# map pack from mappack=PATH, H2_MAPPACK, else disc/mappack.iso.

set positional-arguments
set shell := ["bash", "-euo", "pipefail", "-c"]

# The Halo 2 disc image (default: disc/halo2.iso).
export iso := env("H2_ISO", env("H2_DISC", ""))
# The Multiplayer Map Pack disc image (default: disc/mappack.iso, if there).
export mappack := env("H2_MAPPACK", "")

# List the recipes
default:
    @just --list --unsorted

# Check the prerequisites for every target and print what to install
[group('setup')]
doctor:
    scripts/doctor.sh

# Show which disc and map pack images the recipes would use
[group('setup')]
inputs *args:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/just.sh
    h2_args "iso mappack" "$@"
    h2_iso
    h2_mappack
    echo "iso:     $iso"
    echo "mappack: ${mappack:-none}"

# Build the game module for this Mac (h2 build) [iso=]
[group('mac')]
mac *args:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/just.sh
    h2_args "iso" "$@"
    h2_iso
    cargo h2 build "$iso"

# Play on this Mac (h2 play), installing the map pack if given [iso= mappack=]
[group('mac')]
mac-play *args:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/just.sh
    h2_args "iso mappack" "$@"
    h2_iso
    h2_mappack
    # h2 play installs mappack.iso from beside the disc by itself; one kept
    # elsewhere is imported into the same data root first.
    if [ -n "$mappack" ] && [ "$(dirname "$mappack")" != "$(dirname "$iso")" ]; then
      cargo h2 import-mappack "$mappack" --disc "$iso"
    fi
    cargo h2 play "$iso"

# Build the Apple TV app's inputs: runtime, module, disc [maps=demo|all module=build|skip iso= mappack=]
[group('apple')]
tvos *args:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/just.sh
    h2_args "maps module iso mappack" "$@"
    h2_build_args
    h2_iso
    if [ "${maps:-demo}" = all ]; then h2_mappack; else mappack=; fi
    h2_size_warning tvos
    platform/tvos/build.sh "$iso" ${build_args[@]+"${build_args[@]}"} ${mappack:+--mappack "$mappack"}

# Build, install and launch the Apple TV app with its console [device=NAME]
[group('apple')]
tvos-install *args:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/just.sh
    h2_args "device" "$@"
    h2_device tvOS
    [ -f platform/tvos/build/h2game.framework/h2game ] || h2_die "platform/tvos/build is not filled; run \`just tvos\` first"
    cd platform/tvos
    xcodebuild -project H2TV.xcodeproj -scheme H2TV -configuration Release \
      -destination "platform=tvOS,name=$device" \
      -allowProvisioningUpdates -derivedDataPath DerivedData build
    xcrun devicectl device install app --device "$device" \
      DerivedData/Build/Products/Release-appletvos/H2TV.app
    xcrun devicectl device process launch --console --device "$device" \
      "$(h2_bundle_id . red.forbes.h2recomp.tv)"

# Build the iPhone and iPad app's inputs: runtime, module, disc [maps=demo|all module=build|skip iso= mappack=]
[group('apple')]
ios *args:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/just.sh
    h2_args "maps module iso mappack" "$@"
    h2_build_args
    h2_iso
    if [ "${maps:-demo}" = all ]; then h2_mappack; else mappack=; fi
    h2_size_warning ios
    platform/ios/build.sh "$iso" ${build_args[@]+"${build_args[@]}"} ${mappack:+--mappack "$mappack"}

# Build, install and launch the iPhone or iPad app with its console [device=NAME]
[group('apple')]
ios-install *args:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/just.sh
    h2_args "device" "$@"
    h2_device iOS
    [ -f platform/ios/build/h2game.framework/h2game ] || h2_die "platform/ios/build is not filled; run \`just ios\` first"
    cd platform/ios
    xcodebuild -project H2iOS.xcodeproj -scheme H2iOS -configuration Release \
      -destination 'generic/platform=iOS' \
      -allowProvisioningUpdates -derivedDataPath DerivedData build
    xcrun devicectl device install app --device "$device" \
      DerivedData/Build/Products/Release-iphoneos/H2iOS.app
    xcrun devicectl device process launch --console --device "$device" \
      "$(h2_bundle_id . red.forbes.h2recomp.ios)"

# Cross-build the game module as a Windows DLL (H2_EMIT_TARGET=windows) [iso= out=DIR]
[group('windows')]
windows-dll *args:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/just.sh
    out=${TMPDIR%/}/h2windows-gen
    h2_args "iso out" "$@"
    h2_iso
    host_target=${CARGO_TARGET_DIR:-$PWD/target}
    . platform/windows/env.sh
    # The h2 tool itself is a host build: without the cross build's
    # bootstrap flag, in the usual target directory.
    env -u RUSTC_BOOTSTRAP CARGO_TARGET_DIR="$host_target" H2_EMIT_TARGET=windows \
      cargo h2 build "$iso" --out "$out"
    echo "windows-dll: h2game.dll and its stamp are in $out"

# Build the Xbox app's signed package (platform/xbox/build.sh) [module=build|skip iso=]
[group('xbox')]
xbox *args:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/just.sh
    h2_args "module iso" "$@"
    h2_xbox
    maps=demo
    h2_build_args
    h2_iso
    platform/xbox/build.sh "$iso" ${build_args[@]+"${build_args[@]}"}

# Install and launch the Xbox package through Device Portal [ip=ADDRESS]
[group('xbox')]
xbox-deploy *args:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/just.sh
    ip=${H2_XBOX:-}
    h2_args "ip" "$@"
    h2_xbox
    [ -n "$ip" ] || h2_die "no console address: pass ip=ADDRESS or set H2_XBOX"
    H2_XBOX=$ip platform/xbox/deploy.sh

# Run the workspace tests
[group('checks')]
test *args:
    cargo test --workspace "$@"

# Format, lint and test, as before a push (scripts/check.sh)
[group('checks')]
check *args:
    scripts/check.sh "$@"

# Check the workspace cross-compiles for Windows (platform/windows/env.sh)
[group('checks')]
check-windows:
    . platform/windows/env.sh && cargo check -Zbuild-std --target x86_64-pc-windows-msvc --workspace

# Delete the device build outputs (platform/*/build, DerivedData), keeping the code
[group('clean')]
clean-device-builds:
    #!/usr/bin/env bash
    set -euo pipefail
    for d in platform/tvos/build platform/tvos/DerivedData platform/ios/build platform/ios/DerivedData platform/xbox/build; do
      if [ -e "$d" ]; then
        echo "removing $d ($(du -sh "$d" | cut -f1))"
        rm -rf "$d"
      fi
    done
