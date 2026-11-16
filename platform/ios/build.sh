#!/bin/sh
# platform/ios/build.sh /abs/path/to/disc/halo2.iso [--no-module] [--maps demo|all] [--mappack ISO]
# Fills platform/ios/build/ for H2iOS.xcodeproj; see platform/apple/build.sh.
exec "$(cd "$(dirname "$0")" && pwd)/../apple/build.sh" ios "$@"
