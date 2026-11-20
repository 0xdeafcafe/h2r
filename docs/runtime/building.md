# Building every target with `just`

Status: Built. A guide to the root `justfile`.
Code: `justfile`, `scripts/doctor.sh`. Tests: `scripts/tests/just_test.sh`.
Switch: `H2_ISO`, `H2_DISC`, `H2_MAPPACK`, `H2_OPT` and the toolchain variables, see [switches.md](switches.md).

The root `justfile` is one front door for building each target from your own
disc. It is a thin layer: each recipe resolves the disc image and the
arguments, then calls the command the platform's own doc describes
(`cargo h2`, `platform/*/build.sh`, `xcodebuild`, `devicectl`). Those docs
stay the reference for what each piece is: [app.md](app.md) for the Mac,
[tvos.md](tvos.md), [ios.md](ios.md), [windows.md](windows.md) and
[xbox.md](xbox.md).

```sh
brew install just
just              # the recipes, grouped, one line each
just doctor       # what is installed and what is missing, with the install command
```

## Inputs

| Input | From, in order | Default |
| --- | --- | --- |
| the Halo 2 disc | `iso=PATH` on the recipe, `just iso=PATH <recipe>`, `H2_ISO`, `H2_DISC` | `disc/halo2.iso` |
| the Multiplayer Map Pack disc | `mappack=PATH`, `just mappack=PATH <recipe>`, `H2_MAPPACK` | `disc/mappack.iso`, when it is there |

`H2_MAPPACK=0` (or `false`, `no`, `off`) means no map pack, as it does for
`h2 play`. A path that does not exist stops the recipe with the variable it
came from. `just inputs` prints what the recipes would use.

Recipes take `key=value` arguments after their name, in any order:
`just tvos maps=all module=skip`. An unknown key or a bare word stops the
recipe and lists the keys it takes.

## Recipes

| Recipe | What it runs | Needs (`just doctor` checks) |
| --- | --- | --- |
| `doctor` | `scripts/doctor.sh`: each target's prerequisites, ok or MISSING with the install command; free disk | nothing |
| `inputs` | prints the disc and map pack the recipes would use | |
| `mac [iso=]` | `cargo h2 build`: the game module for this Mac, in `$TMPDIR/h2boot` | Rust (the pinned version), clang |
| `mac-play [iso= mappack=]` | `cargo h2 play`; a map pack kept somewhere other than beside the disc is imported first (`h2 import-mappack`) | as `mac` |
| `tvos [maps= module= iso= mappack=]` | `platform/tvos/build.sh`: runtime, module, disc and boot cache in `platform/tvos/build/` | Xcode, the appletvos SDK, rust-src |
| `tvos-install [device=]` | `xcodebuild`, `devicectl device install app`, then `devicectl device process launch --console` | a signing team in `platform/tvos/Local.xcconfig`, a paired Apple TV |
| `ios [maps= module= iso= mappack=]` | `platform/ios/build.sh`, as `tvos` | Xcode, the iphoneos SDK, rust-src |
| `ios-install [device=]` | as `tvos-install`, for `H2iOS` | a signing team in `platform/ios/Local.xcconfig`, a paired iPhone or iPad |
| `windows-dll [iso= out=]` | `platform/windows/env.sh`, then `H2_EMIT_TARGET=windows cargo h2 build` into `out` (default `$TMPDIR/h2windows-gen`): `h2game.dll` and its stamp | llvm@22, lld@22, xwin and its splat |
| `xbox [module= iso=]` | `platform/xbox/build.sh`: the signed `h2recomp.msix` | as `windows-dll`, plus makemsix, osslsigncode, openssl |
| `xbox-deploy [ip=]` | `platform/xbox/deploy.sh` with `H2_XBOX=ip` (default: `H2_XBOX`) | a console in Dev Mode, its Device Portal password in the keychain |
| `test [cargo args]` | `cargo test --workspace` | Rust |
| `check [--fix \| --crate NAME]` | `scripts/check.sh`: fmt, clippy, tests | Rust |
| `check-windows` | `cargo check -Zbuild-std --target x86_64-pc-windows-msvc --workspace` under `env.sh` | as `windows-dll` |
| `clean-device-builds` | removes the `build` directories under `platform/tvos`, `platform/ios` and `platform/xbox`, and the `DerivedData` of tvOS and iOS | |

### Arguments

- `module=build|skip` (default `build`): `skip` keeps the existing game
  module (`build.sh --no-module`). The module build is the slow part, a
  full-image lift, emit and clang; rebuild it whenever the generator, the
  runtime ABI or `Cargo.lock` changes, or the app refuses the stamp.
- `maps=demo|all` (default `demo`), for `tvos` and `ios`; see below.
- `device=NAME`: the name `xcrun devicectl list devices` shows. Without it,
  the one paired, reachable device of the right kind is used; with none or
  several, the recipe says so and lists them.
- `ip=ADDRESS`: the Xbox's address, as `H2_XBOX` takes it.

## Maps on Apple TV and iPhone: `maps=demo|all`

`maps=demo` is what the apps have always carried: the menu, the shared maps,
The Heretic and Zanzibar (`h2host::layout::DISC_FILES`, about 1 GB).
Choosing another level fails to load it.

`maps=all` carries every map:

- **The disc**: the packer (`crates/apps/h2host/examples/pack.rs --maps all`)
  copies the whole disc into `build/disc/`, which the app serves as D:
  (`vfs::DirDiscBackend`), as the Mac serves the image itself.
- **The map pack**: it is not disc content but hard-disk content. On the Mac,
  `h2 play` imports `mappack.iso` into the data root's emulated hard disk as
  nine 1.0 content packages, one per map, each with its own `patch.lvl`
  ([../game/map-pack.md](../game/map-pack.md) section 8):
  `<data>/disk/Partition1/TDATA/4D530064/$c/<offer id>/`. `build.sh` runs the
  same import (`h2 import-mappack --no-update`) into
  `build/bootcache/h2data/`, which the bundle carries. At launch the app
  copies that tree into its data root (`h2host::maps::seed`, from
  `Layout::prepare`): Caches on tvOS, Documents on iOS. Files already there
  with the right size are kept, and the game's own writes are never touched.
  On APFS the copy is a clone, so it takes no extra space until one side
  changes.

Size: the whole disc is about 4.3 GB as an XISO, and the map pack's maps come
on top. Expect 7 GB or more in `platform/<target>/build`, in the app and on
the device, and a long install over the network. On tvOS the map pack is in
Caches, which tvOS may purge when it is short of space; the app copies it
back from the bundle at the next launch (saves in Caches are not restored).
On iOS it is in Documents, so it counts towards device backups.

Going back to `maps=demo` removes what `all` added: the packer deletes the
disc files it does not want (`h2host::maps::prune`) and `build.sh` removes
the seed. A device that already has the map pack in its data root keeps it.

## Prerequisites by target

`just doctor` checks all of these and prints the install command for each
one that is missing:

- **Inputs**: the disc image; the map pack image (optional).
- **Mac**: `rustc` at the version in `rust-toolchain.toml` or newer; clang.
- **Apple TV, iPhone and iPad**: Xcode selected (`xcode-select -p`), the
  appletvos and iphoneos SDKs, rust-src (`-Zbuild-std`), a
  `DEVELOPMENT_TEAM` in each `platform/<target>/Local.xcconfig`, an Apple
  Development signing identity, and a paired device of each kind
  (`devicectl`). xcodegen only matters to regenerate the Xcode projects.
- **Windows**: llvm@22 (`clang-cl`, `llvm-lib`), lld@22 (`lld-link`), xwin,
  and the CRT and SDK splat in `H2_XWIN` (default `$TMPDIR/xwin`).
- **Xbox**: `platform/xbox` on the checkout, makemsix (`H2_MAKEMSIX`, `PATH`,
  then `$TMPDIR/msix-tools/bin`), osslsigncode, openssl.
- **Disk**: 12 GB free for the repository (a `maps=all` device build) and
  5 GB in `$TMPDIR` (the generated C and objects). `just clean-device-builds`
  and deleting `$TMPDIR/h2*-gen` free most of it.

## Target directories

The cross builds use cargo target directories of their own, because
`RUSTC_BOOTSTRAP` and `-Zbuild-std` would otherwise invalidate the host
build's artifacts, and back. Each can be moved (to another volume, say):

| Variable | What it holds | Default |
| --- | --- | --- |
| `H2_APPLE_TARGET_DIR` | the `h2host` runtime cross-built for tvOS or iOS (`platform/apple/build.sh`) | `target/<tvos\|ios>-cross` |
| `H2_APPLE_HOST_TARGET_DIR` | the host-side `h2` and packer that `build.sh` runs for the disc subset and game module | `target/apple-host-pack` |
| `H2_WINDOWS_TARGET_DIR` | the Windows cross build (`platform/windows/env.sh`, so `windows-dll` and `check-windows`) | `target/windows-cross` |

## Tests

`scripts/tests/just_test.sh` runs the justfile in a scratch copy of the
repository whose build scripts, cargo, xcodebuild and xcrun only print what
they were asked to do, and checks the arguments each recipe passes on. The
packer's `maps` logic (subset, seed, prune) is unit-tested in
`crates/apps/h2host/src/maps.rs`.
