# Windows (and the road to Xbox Dev Mode)

The goal is an Xbox Series X in Developer Mode, which runs UWP apps. This
first step makes the runtime and the generated game module build for
`x86_64-pc-windows-msvc`, cross-compiled from a Mac. There is no Windows
host app yet: no UWP shell, no packaging. Nothing here has run on Windows.
It is all compile- and link-checked on a Mac.

## Toolchain (on the Mac)

```sh
brew install llvm@22 lld@22 xwin
xwin --accept-license --arch x86_64 --cache-dir "$TMPDIR/xwin-cache" \
  splat --output "$TMPDIR/xwin"          # ~650 MB, plus ~1 GB of downloads
. platform/windows/env.sh                # run from the repository root
cargo check -Zbuild-std --target x86_64-pc-windows-msvc --workspace
```

- **Headers and import libraries** come from an `xwin splat` of the MSVC CRT
  and Windows SDK. cargo-xwin is the same tool behind a cargo wrapper. Doing
  the splat ourselves keeps one copy in a known place, the emitter can point
  clang at it (`H2_XWIN`), and cargo stays plain. A MinGW-free clang alone
  is not enough: the generated C includes `<math.h>` and `<string.h>`, and
  build scripts (ring, zstd) compile C, so the CRT headers are needed even
  for `cargo check`.
- **std**: Homebrew's Rust ships only the host std, so `env.sh` sets
  `RUSTC_BOOTSTRAP=1` and the commands pass `-Zbuild-std`, as for tvOS.
  With rustup, `rustup target add x86_64-pc-windows-msvc` replaces both.
- **C compiler for build scripts**: `clang-cl` from llvm@22 with `/imsvc`
  paths into the splat (`CC_x86_64_pc_windows_msvc`, `CFLAGS_...`).
- **Linker**: `lld-link` from lld@22 (`CARGO_TARGET_..._LINKER`, plus
  `-Lnative` paths into the splat). It is only needed for a real
  `cargo build`, not for `check`.
- `CARGO_TARGET_DIR` is `target/windows-cross`, so the bootstrap flags never
  invalidate the host build.

## The generated module

`H2_EMIT_TARGET=windows` (or `h2 emit --arch windows`) selects
`emit::compile::Arch::X86_64Windows`:

| | macOS | Windows |
| --- | --- | --- |
| compile | clang `-target arm64-apple-macos11` | the same clang, `-target x86_64-pc-windows-msvc -fms-runtime-lib=dll -isystem $H2_XWIN/crt/include -isystem $H2_XWIN/sdk/include/ucrt` |
| objects | `shard.arm64.<key>.o` | `shard.x86_64-windows.<key>.o` (keyed apart) |
| archive | libtool | `llvm-lib` (Apple's libtool takes only Mach-O) |
| link | `clang -dynamiclib` | `lld-link /dll /noimplib`, against the splat's CRT, UCRT and um libs |
| module | `libh2game.dylib` | `h2game.dll` |
| exports | every global | only what is marked `H2_EXPORT` (see below) |
| stamp | unchanged | ends `target x86_64-pc-windows-msvc` |

`lld-link` and `llvm-lib` are found through `H2_LLD` and `H2_LLVM` (set by
`env.sh`), then `PATH`, then Homebrew's usual prefixes
(`emit::compile::windows_tool`).

**Exports.** A DLL exports only what it marks, and at most 65,535 symbols.
The retail image has about 18,700 handlers, and they are reached through the
`h2gen_module` table anyway. `h2abi.h` defines
`H2_EXPORT` as `__declspec(dllexport)` on `_WIN32` and as nothing elsewhere.
It marks `h2gen_module`, `h2gen_module_abi_version`, `h2boot_install_v2`,
`h2x87bridge_install` and the test bridge's `h2bridge_*`. Mac objects come
out the same as before.

**What was checked in the emitted C**:

- No `long` (32 bits under LLP64): the support headers use `<stdint.h>`
  types and the `ll` builtins. There is no `long double` (h2abi.h already
  forbids it).
- No sections, `visibility`, `weak` or `alias`.
- `__int128` (`h2u128`) is used only for bit operations inside C, never
  across the host boundary. It needs no compiler-rt helpers.
- `preserve_most` (h2fastmem.h's cold helpers), the `stmxcsr` and `"+x"`
  inline asm, and the native x87 and SSE paths all compile for
  x86_64-windows-msvc.

**Calling convention.** The host ABI is now Microsoft x64 on both sides.
The Rust side declares every bridge and handler type `extern "C"`. Clang
compiles the C for the same triple. So both follow the platform C ABI. A
`TransferAbi` (28 bytes) is returned through a hidden pointer in RCX, and
`CpuAbi*` and the context arrive in RDX and R8. Nothing uses `sysv64`,
naked functions or hand-written trampolines. `__int128` and structs are
never passed by value across the boundary.

**Float ABI** (docs/FLOAT-ABI.md). This is unaffected. Guest x87 state is
`X87Abi` bytes, native x87 and SSE use SSE2 and MXCSR as on Intel macOS,
and Windows' default MXCSR (0x1F80) is the same.

**The C runtime.** The module uses the DLL CRT (vcruntime140 and the
api-ms-win-crt UCRT), which is the one Rust's msvc target links. The
process therefore has one CRT. UWP needs the app-container flavour
(`vcruntime140_app`); see the step 2 plan.

## What Windows gets in the runtime

| Area | macOS / Unix | Windows |
| --- | --- | --- |
| guest memory, fastmem, threads, preemption | portable Rust (no mmap or signals on the game path) | as is |
| rendering | wgpu on Metal | wgpu on DX12 (wgpu's default features) |
| host-owned surface (`app::embedded`) | `CAMetalLayer` | XAML `SwapChainPanel` (`app::embedded::host_surface`), the UWP seam |
| windowed shell, keyboard, mouse | winit | winit |
| gamepads | GameController, IOKit HID | gilrs, whose Windows backend is Windows.Gaming.Input, which UWP also has |
| audio | cpal on CoreAudio | cpal on WASAPI |
| data root | `~/Library/Application Support/h2recomp` | `%LOCALAPPDATA%\h2recomp` (`vfs::default_data_root`) |
| pipeline cache | `~/Library/Caches/h2recomp` | `%LOCALAPPDATA%\h2recomp\cache` |
| utility-drive format | handle-anchored walk (openat, unlinkat) | by path, never following links or junctions (`vfs` `empty_by_path`) |
| Live sockets (`--live`) | BSD sockets | stand-in: every socket fails with WSASYSNOTREADY (Winsock port to do) |
| movies (Bink, AVFoundation) | AVFoundation | compiled out: movies are skipped |
| `h2 emit` output lock | flock | std `File::lock` (LockFileEx) |
| perf sampler, fastmem-proto, HID | Mach and IOKit | compiled out (they were already gated) |
| Ctrl-C in `h2 play` | signal watcher | none yet |
| live-server identity files | mode 0600 | the profile directory's ACL |

## Next: the UWP app on Xbox (not started)

1. **Host crate** (`crates/uwp`, like `crates/tvos`). It is a `cdylib` or
   `staticlib` for `x86_64-uwp-windows-msvc`, a tier 3 Rust target built
   with `-Zbuild-std`. It exports `h2uwp_start(panel, w, h)` and
   `h2uwp_redraw()` around `app::embedded::Embedded`, plus suspend and
   resume hooks. The XAML shell can be C++/WinRT, or Rust with the
   `windows` crate. It owns a `SwapChainPanel` and a `CompositionTarget`
   rendering tick. wgpu 30 has no `CoreWindow` surface, only
   `SwapChainPanel`, `CompositionVisual` and `SurfaceHandle`. The game
   module stays a DLL loaded with `LoadPackagedLibrary` (libloading's
   `LoadLibraryExW` has to be checked inside the app container), with the
   UWP CRT (`vcruntime140_app`; xwin can splat the store libs).
2. **Paths.** `ApplicationData.Current.LocalFolder` is the data root
   (`vfs::Config::new`). The disc is in the package, or in a LocalState
   folder filled over the Device Portal file API.
3. **Packaging from macOS.** Write the AppxManifest and assets, then `makeappx`
   and `signtool` from the Windows SDK (Windows only), or
   msix-packaging's `makemsix` and osslsigncode on macOS, signed with a
   self-signed certificate whose subject matches the manifest's Publisher.
4. **Deploy.** Use the Xbox Device Portal REST API over HTTPS with the
   console's credentials: `POST /api/app/packagemanager/package?package=...`
   (multipart: the msix and its dependencies), poll
   `/api/app/packagemanager/state`, then launch with
   `POST /api/taskmanager/app?appid=<base64 AUMID>`. Set the content type to
   Game, either in Dev Home or through the portal's app details, to get game
   resources.
