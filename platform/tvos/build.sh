#!/bin/sh
# platform/tvos/build.sh /abs/path/to/disc/halo2.iso [--no-module] [--maps demo|all] [--mappack ISO]
# Fills platform/tvos/build/ for H2TV.xcodeproj; see platform/apple/build.sh.
exec "$(cd "$(dirname "$0")" && pwd)/../apple/build.sh" tvos "$@"
