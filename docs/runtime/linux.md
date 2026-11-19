# Linux x86-64 game module

The dedicated host runs an ELF64 shared object, `libh2game.so`, built for
`x86_64-unknown-linux-gnu`. The native handlers and the Rust host use the
System V AMD64 C ABI. The image uses baseline x86-64 instructions. The
`--arch x86_64-linux` selector chooses this target even on a Mac. On a
64-bit Intel or AMD Linux machine, `--arch host` selects it automatically.

## Cross-build on macOS

Use LLVM clang, ld.lld and llvm-ar on the Mac, with an **amd64 Debian
sysroot from the release used by the host container**. The emitter already
uses clang's driver and dependency scans for its object cache. Using the
container's headers, startup objects and libraries also pins the glibc ABI
used at link time. A usable sysroot contains glibc development files
(including libm), Linux headers and the target GCC startup and support
files under `usr/lib/gcc/x86_64-linux-gnu/<version>/`. Preserve Debian's
multiarch directory layout and ensure library symlinks resolve inside the
sysroot.

With clang, ld.lld and llvm-ar on PATH, and `H2_LINUX_SYSROOT` set to the
absolute sysroot directory, run the following with the Mac build of `h2`.

```sh
h2 build disc/default.xbe --arch x86_64-linux --tier clang --out "$TMPDIR/h2-linux"
h2 build disc/default.xbe --arch x86_64-linux --tier clang --out "$TMPDIR/h2-linux" --check
```

`H2_EMIT_TARGET=x86_64-linux` selects the same target for `h2 build`.
Clang receives `-target x86_64-unknown-linux-gnu`, `--sysroot=<dir>` and
`--gcc-toolchain=<dir>/usr` for compilation and linking. A native Linux
build can leave the variable unset and use its installed development
toolchain. The fast tier uses the running process's architecture. Use the
clang tier for a cross-build.

Ship `libh2game.so` and its adjacent `libh2game.so.stamp` together. Build
the host and the module from the same generator revision and with matching
code-generation settings, including `H2_OPT`. The stamp includes the
target triple. The sysroot path is specific to the build machine.

## Linking and loading

The Linux clang config selects `-fPIC -fvisibility=hidden`. Generated
handlers are reachable through the exported `h2gen_module` registry.
The boot host also looks up `h2gen_module_abi_version`,
`h2boot_install_v3` and `h2x87bridge_install`. LLVM's archiver writes a
GNU archive of the shards. For the loadable image clang invokes lld with
`-shared`, sets the SONAME to the published basename, and links libm.

The boot and x87 bridges are linked into the module. The host installs
their Rust callbacks through the bridge tables. The shared link also
permits undefined host imports. ld.lld's shared-output defaults permit
references from the module's objects, and `--allow-shlib-undefined`
permits unresolved references in input shared libraries. At load time
libloading requests `RTLD_NOW | RTLD_LOCAL`. A missing import therefore
fails the load immediately, and module symbols stay local to its handle.

The shipped bridge tables work without exporting symbols from the
executable. To support modules that import a host function **by name**,
the Linux **h2 executable** must export that function from its dynamic
symbol table. Pass `-rdynamic` to its final C linker driver to export the
executable's visible globals, or export the individual symbol. With a
clang or GCC linker driver, the CLI build script
can emit `cargo:rustc-link-arg-bin=h2=-rdynamic` for the Linux target.
A directly imported Rust function needs an unmangled C ABI name and
default visibility, and its defining object must be linked into the
executable. Check the deployed binary's defined dynamic symbols with
`nm -D --defined-only h2`. Bridge callbacks passed by address continue
to use the installed tables.

## Checks still needed

This lane has not built or run the Linux image. On the coordinator's
cross-build, inspect the ELF header, SONAME, dependencies and exported
bridge entry points with `readelf -h -d -Ws libh2game.so`. On the target
Debian image, check the stamp, load with the release headless `h2`, and
exercise the bridge, x87 helpers and generated dispatch by booting to a
joinable lobby. Use the game agent API to observe the lobby on that run.
