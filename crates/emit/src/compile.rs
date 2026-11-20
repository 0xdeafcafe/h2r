//! Build glue: generated C to objects and static libraries with clang.
//!
//! Objects are cached by a content hash of everything that affects them
//! (source, `h2gen.h`, `h2abi.h`, flags, target, and the toolchain: see
//! [`toolchain_identity`]), so regenerating an image where only some
//! shards changed recompiles only those, and switching compiler or SDK
//! recompiles everything rather than mixing objects. Everything lives
//! under the output directory the caller chooses, under a temporary or user
//! cache directory: it is derived from the disc and never committed. The objects
//! can instead go to a cache shared between checkouts (`H2_OBJ_CACHE`, see
//! [`obj_dir`]), kept under a size cap (`H2_OBJ_CACHE_MAX`, see
//! [`prune_obj_cache`]).

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use crate::fnv64;
use crate::output::{self, Temporary};

mod dependencies;
pub mod pgo;
mod tiers;

pub use pgo::Pgo;
pub use tiers::{Tiers, hot_addresses};

/// Cranelift's optimisation level for the fast tier: `H2_CLIF_OPT`
/// (`none`, `speed`), default `speed`.
pub fn clif_opt_level() -> Result<&'static str, String> {
    match std::env::var("H2_CLIF_OPT").ok().as_deref() {
        None | Some("") | Some("speed") => Ok("speed"),
        Some("none") => Ok("none"),
        Some(v) => Err(format!("H2_CLIF_OPT={v}: expected none or speed")),
    }
}

/// Flags for every generated object that do not depend on the build
/// profile (LIFTER.md §1): no fast-math or FP contraction, no
/// strict-aliasing assumptions. The optimisation level and debug-info
/// level come from [`profile_flags`].
pub const CFLAGS: &[&str] = &[
    "-std=c11",
    "-fno-strict-aliasing",
    "-fno-fast-math",
    "-ffp-contract=off",
    "-Wall",
    "-Wno-unused-label",
    "-Wno-unused-variable",
    "-Wno-unused-but-set-variable",
    "-Werror=implicit-function-declaration",
    "-Werror=int-conversion",
    // Clang records its working directory in the driver's expanded command
    // line and in debug info. Pin it, so the same source and flags key and
    // compile identically from any directory. Without these, running the
    // build from another checkout missed every cached object.
    "-fdebug-compilation-dir=.",
    "-fcoverage-compilation-dir=.",
];

/// The `-O` levels `H2_OPT` accepts.
pub const OPT_LEVELS: &[&str] = &["0", "1", "g", "s", "2", "3"];

/// The optimisation and debug-info flags for `opt` (`H2_OPT`: `0` `1` `g`
/// `s` `2` `3`, default `1`) and `debug` (`H2_DEBUG`: `lines`, `full`, `none`,
/// default `lines`). Both are part of every object's cache key. `-O1` is
/// the release level (LIFTER.md §1); `lines` keeps guest-address line
/// tables for backtraces and stepping at about two thirds of the compile
/// time of `full` (`-g`), whose variable-level info is the bulk of the
/// object size. [`profile_flags_for`] takes a target's own default level.
pub fn profile_flags(opt: Option<&str>, debug: Option<&str>) -> Result<Vec<String>, Error> {
    let opt = opt.unwrap_or("1");
    if !OPT_LEVELS.contains(&opt) {
        return Err(err(format!("H2_OPT={opt}: expected 0, 1, g, s, 2 or 3")));
    }
    let debug = match debug.unwrap_or("lines") {
        "lines" => Some("-gline-tables-only"),
        "full" => Some("-g"),
        "none" => None,
        d => return Err(err(format!("H2_DEBUG={d}: expected lines, full or none"))),
    };
    Ok([Some(format!("-O{opt}")), debug.map(str::to_owned)].into_iter().flatten().collect())
}

/// Every flag for a generated object: [`CFLAGS`] plus [`profile_flags`]
/// from the environment, plus the optional paths. `H2_PERF` is off unless
/// set to `1`; the inline RAM path (`H2_FASTMEM`), native x87
/// (`H2_X87_NATIVE`) and SSE (`H2_SSE_NATIVE`), and direct calls between
/// generated handlers (`H2_DIRECT_CALLS`) are on unless set to `0`; store
/// sites testing the dirty bit before marking a page (`H2_DIRTY_INLINE`,
/// `h2fastmem.h`) is off unless set to `1`; so is counting cold guest
/// accesses by cause (`H2_COLD_STATS`, printed to stderr), and lending the
/// RAM gate across direct calls (`H2_DIRECT_GATE`, `h2gen.h`), carrying the
/// access count across direct calls (`H2_DIRECT_COUNT`) and taking the RAM
/// gate at a handler's start (`H2_ENTRY_GATE`, `h2fastmem.h`). Charging
/// accesses in the RAM view (`H2_VIEW_CHARGE`) defaults on.
/// Profile-guided optimisation (`H2_PGO`) and the target CPU (`H2_CPU`,
/// arm64 only: see [`pgo::for_arch`]) are off unless set ([`pgo`]).
///
/// These are the flags for `arch`: [`Arch::codegen_flags`] after
/// [`CFLAGS`], and its default level ([`Arch::default_opt`]) when `H2_OPT`
/// is unset. A macOS, tvOS or iOS object's flags are the same as before
/// targets had flags of their own. The stamp ([`boot`'s `build_stamp`])
/// takes them for the target it builds for, so a module cross-built for
/// Windows matches what the app computes on the device.
///
/// H2_FLAT_RAM, H2_RAM_PAGE_CACHE and H2_X87_TOP are opt-in until
/// measured in full images: both trade fewer hot operations for more
/// live state or larger generated code.
pub fn cflags_for(arch: Arch) -> Result<Vec<String>, Error> {
    let env = |k| std::env::var(k).ok();
    let mut v: Vec<String> = CFLAGS.iter().map(|s| s.to_string()).collect();
    v.extend(arch.codegen_flags().iter().map(|s| s.to_string()));
    v.extend(profile_flags_for(arch, env("H2_OPT").as_deref(), env("H2_DEBUG").as_deref())?);
    v.extend(Pgo::from_env()?.flags()?);
    v.extend(pgo::cpu_flags(env("H2_CPU").as_deref())?);
    for (name, default) in [
        ("H2_PERF", false),
        ("H2_FASTMEM", true),
        ("H2_X87_NATIVE", true),
        ("H2_X87_TOP", false),
        ("H2_SSE_NATIVE", true),
        ("H2_RAM_PAGE_CACHE", false),
        ("H2_FLAT_RAM", false),
        ("H2_DIRECT_CALLS", true),
        ("H2_DIRTY_INLINE", false),
        ("H2_COLD_STATS", false),
        ("H2_DIRECT_GATE", false),
        ("H2_DIRECT_COUNT", false),
        ("H2_ENTRY_GATE", false),
        ("H2_VIEW_CHARGE", true),
    ] {
        if flag(name, env(name).as_deref(), default)? {
            v.push(format!("-D{name}=1"));
        }
    }
    if let Some(owner) = env("H2_FLAT_STATS_OWNER").filter(|s| !s.is_empty()) {
        let pc = u32::from_str_radix(owner.strip_prefix("0x").unwrap_or(&owner), 16)
            .map_err(|_| err(format!("H2_FLAT_STATS_OWNER={owner}: expected a hexadecimal guest owner address")))?;
        v.push(format!("-DH2_FLAT_STATS_OWNER=0x{pc:08x}u"));
    }
    Ok(pgo::for_arch(v, arch))
}

/// [`cflags_for`] the host: what a test compiling generated C for this
/// process uses.
pub fn cflags() -> Result<Vec<String>, Error> {
    cflags_for(Arch::host())
}

/// [`profile_flags`] with `arch`'s default level ([`Arch::default_opt`])
/// when `opt` is unset.
pub fn profile_flags_for(arch: Arch, opt: Option<&str>, debug: Option<&str>) -> Result<Vec<String>, Error> {
    profile_flags(Some(opt.unwrap_or(arch.default_opt())), debug)
}

/// A `0`/`1` switch: unset or empty takes `default`.
fn flag(name: &str, value: Option<&str>, default: bool) -> Result<bool, Error> {
    match value {
        None | Some("") => Ok(default),
        Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(value) => Err(err(format!("{name}={value}: expected 0 or 1"))),
    }
}

/// A job count that uses every core.
pub fn default_jobs() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

/// What generated code is compiled for: a CPU and the platform it runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// Apple silicon macOS.
    Arm64,
    /// Intel macOS.
    X86_64,
    /// tvOS on an Apple TV (docs/runtime/tvos.md). Cross-compiled on a Mac against
    /// the appletvos SDK; never the host there.
    Arm64Tvos,
    /// iOS and iPadOS on an iPhone or iPad (docs/runtime/ios.md). Cross-compiled on
    /// a Mac against the iphoneos SDK; never the host there.
    Arm64Ios,
    /// 64-bit Windows (docs/runtime/windows.md), the MSVC ABI: a DLL for desktop
    /// Windows and, later, the UWP app on an Xbox in Dev Mode.
    /// Cross-compiled on a Mac with clang against the MSVC CRT and Windows
    /// SDK headers an xwin splat provides ([`XWIN_ENV`]), linked by
    /// `lld-link` ([`windows_tool`]).
    X86_64Windows,
    /// x86-64 Linux with glibc, using the System V ABI.
    X86_64Linux,
}

/// The environment variable naming the xwin splat (the MSVC CRT and Windows
/// SDK, `xwin splat --output <dir>`) a Windows module compiles and links
/// against: `<dir>/crt/include`, `<dir>/sdk/include/ucrt`, and the
/// matching `lib/` directories. `platform/windows/env.sh` sets it.
pub const XWIN_ENV: &str = "H2_XWIN";

/// The oldest tvOS the generated module (and the app around it) supports.
/// The Rust side's `TVOS_DEPLOYMENT_TARGET` must match (platform/tvos).
pub const TVOS_DEPLOYMENT_TARGET: &str = "17.0";

/// The oldest iOS the generated module (and the app around it) supports.
/// The Rust side's `IPHONEOS_DEPLOYMENT_TARGET` must match (platform/ios).
pub const IOS_DEPLOYMENT_TARGET: &str = "17.0";

impl Arch {
    /// The macOS release targets (`--arch all`).
    pub const ALL: [Arch; 2] = [Arch::Arm64, Arch::X86_64];

    pub fn name(self) -> &'static str {
        match self {
            Arch::Arm64 => "arm64",
            Arch::X86_64 => "x86_64",
            Arch::Arm64Tvos => "arm64-tvos",
            Arch::Arm64Ios => "arm64-ios",
            Arch::X86_64Windows => "x86_64-windows",
            Arch::X86_64Linux => "x86_64-linux",
        }
    }

    pub fn triple(self) -> &'static str {
        match self {
            Arch::Arm64 => "arm64-apple-macos11",
            Arch::X86_64 => "x86_64-apple-macos11",
            // Keep in step with TVOS_DEPLOYMENT_TARGET.
            Arch::Arm64Tvos => "arm64-apple-tvos17.0",
            // Keep in step with IOS_DEPLOYMENT_TARGET.
            Arch::Arm64Ios => "arm64-apple-ios17.0",
            Arch::X86_64Windows => "x86_64-pc-windows-msvc",
            Arch::X86_64Linux => "x86_64-unknown-linux-gnu",
        }
    }

    /// The Xcode SDK to compile and link against, when it is not the
    /// default (macOS) one.
    pub fn sdk(self) -> Option<&'static str> {
        match self {
            Arch::Arm64Tvos => Some("appletvos"),
            Arch::Arm64Ios => Some("iphoneos"),
            Arch::Arm64 | Arch::X86_64 | Arch::X86_64Windows | Arch::X86_64Linux => None,
        }
    }

    /// The clang arguments that select this target: `-target` and, for a
    /// non-macOS SDK, `-isysroot` (found once with `xcrun`); for Windows,
    /// the DLL C runtime and the xwin splat's headers ([`XWIN_ENV`]).
    /// Linux uses the native toolchain or the selected Debian sysroot.
    pub fn target_args(self) -> Result<Vec<String>, Error> {
        if self == Arch::X86_64Linux {
            let root = std::env::var_os("H2_LINUX_SYSROOT");
            let root = root.as_deref().map(|r| r.to_str().ok_or_else(|| err("H2_LINUX_SYSROOT must be UTF-8"))).transpose()?;
            return Ok(linux_target_args(root));
        }
        let mut v = vec!["-target".to_owned(), self.triple().to_owned()];
        if let Some(sdk) = self.sdk() {
            v.push("-isysroot".into());
            v.push(sdk_path(sdk)?);
        }
        if self == Arch::X86_64Windows {
            v.extend(windows_target_args(&xwin_dir()?));
        }
        Ok(v)
    }

    /// Whether this is the Windows target: a DLL linked by `lld-link`,
    /// archived by `llvm-ar`.
    pub fn is_windows(self) -> bool {
        self == Arch::X86_64Windows
    }

    /// Name used to publish and locate the game module on this platform.
    pub const fn module_file_name(self) -> &'static str {
        match self {
            Arch::X86_64Windows => "h2game.dll",
            Arch::X86_64Linux => "libh2game.so",
            Arch::Arm64 | Arch::X86_64 | Arch::Arm64Ios | Arch::Arm64Tvos => "libh2game.dylib",
        }
    }

    /// The architecture this process runs on.
    pub const fn host() -> Arch {
        if cfg!(target_os = "tvos") {
            Arch::Arm64Tvos
        } else if cfg!(target_os = "ios") {
            Arch::Arm64Ios
        } else if cfg!(windows) {
            Arch::X86_64Windows
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Arch::X86_64Linux
        } else if cfg!(target_arch = "x86_64") {
            Arch::X86_64
        } else {
            Arch::Arm64
        }
    }

    /// What `h2 build` (`boot::build`) compiles for: `H2_EMIT_TARGET`
    /// (`tvos`, `ios`, `windows`, `x86_64-linux`, or `host`/unset for the host), see
    /// [`Arch::target_from`].
    pub fn target() -> Result<Arch, Error> {
        Arch::target_from(std::env::var("H2_EMIT_TARGET").ok().as_deref())
    }

    /// [`Arch::target`] for a given `H2_EMIT_TARGET` value. `macos` is the
    /// host only on a Mac: a tvOS, iOS or Windows host has no macOS to build for.
    pub fn target_from(value: Option<&str>) -> Result<Arch, Error> {
        match value.unwrap_or("") {
            "" | "host" => Ok(Arch::host()),
            "tvos" => Ok(Arch::Arm64Tvos),
            "ios" => Ok(Arch::Arm64Ios),
            "windows" => Ok(Arch::X86_64Windows),
            "linux" | "x86_64-linux" => Ok(Arch::X86_64Linux),
            "macos" if Arch::host().is_macos() => Ok(Arch::host()),
            v => Err(err(format!("H2_EMIT_TARGET={v}: expected host, macos, tvos, ios, windows or x86_64-linux"))),
        }
    }

    /// The ELF SONAME is the published basename. For Mach-O, the install name is its own path for macOS
    /// (loaded by path from the output directory), and the framework the
    /// tvOS and iOS apps embed it in (platform/tvos, platform/ios), which
    /// it is loaded from.
    pub fn install_name(self, out: &Path) -> OsString {
        match self {
            Arch::Arm64Tvos | Arch::Arm64Ios => "@rpath/h2game.framework/h2game".into(),
            Arch::X86_64Linux => out.file_name().unwrap_or_default().to_owned(),
            Arch::Arm64 | Arch::X86_64 | Arch::X86_64Windows => out.as_os_str().to_owned(),
        }
    }

    /// A macOS target (the ones `h2 build` runs on).
    pub fn is_macos(self) -> bool {
        matches!(self, Arch::Arm64 | Arch::X86_64)
    }

    /// The optimisation level (`H2_OPT`) a generated object of this target
    /// compiles at when `H2_OPT` is unset. `1` (LIFTER.md §1) except on
    /// Windows, which is `2`: the Xbox package's stamp is computed on the
    /// console with `H2_OPT` unset (`pack` refuses it), so its level has to
    /// be the target's default, and on the console's Zen 2 the generated
    /// code is CPU-bound where -O1 leaves about a sixth more instructions
    /// than -O2 (docs/runtime/xbox.md).
    pub fn default_opt(self) -> &'static str {
        match self {
            Arch::X86_64Windows => "2",
            Arch::Arm64 | Arch::X86_64 | Arch::Arm64Tvos | Arch::Arm64Ios | Arch::X86_64Linux => "1",
        }
    }

    /// Code generation flags of this target's own, after [`CFLAGS`] and
    /// part of every object's key and the module stamp. None for the Apple
    /// targets.
    ///
    /// Windows: clang's x86-64 baseline has no FMA, so the native x87
    /// path's exactness check (`__builtin_fma`, h2x87_native.h) was a call
    /// to the static UCRT's `fma`, a software routine with its own stack
    /// cookie, on every native FMUL, FDIV and FSQRT. `x86-64-v3` (AVX2,
    /// FMA, BMI1/2, LZCNT, MOVBE: Haswell and every Zen, so the Series
    /// consoles' Zen 2 and any desktop since 2013) makes it one
    /// `vfmadd`; `-mtune=znver2` schedules for the console. The ISA does
    /// not change a result: contraction stays off (`-ffp-contract=off`).
    /// MSVC targets keep `errno` for math by default, which turns each
    /// `sqrt` into an inline `sqrtsd` plus a call for a negative operand;
    /// generated code never reads `errno`, and the Apple targets already
    /// default to `-fno-math-errno`.
    pub fn codegen_flags(self) -> &'static [&'static str] {
        match self {
            Arch::X86_64Linux => {
                // Embed the config so the installed host can compute a module stamp.
                // The file contains one literal compiler option per line.
                static FLAGS: std::sync::LazyLock<Vec<&str>> = std::sync::LazyLock::new(|| {
                    include_str!("../configs/x86_64-unknown-linux-gnu-clang.cfg").split_whitespace().collect()
                });
                &FLAGS
            }
            Arch::X86_64Windows => &["-march=x86-64-v3", "-mtune=znver2", "-fno-math-errno"],
            Arch::Arm64 | Arch::X86_64 | Arch::Arm64Tvos | Arch::Arm64Ios => &[],
        }
    }
}

fn linux_target_args(sysroot: Option<&str>) -> Vec<String> {
    let mut args = vec!["-target".into(), Arch::X86_64Linux.triple().into()];
    if let Some(root) = sysroot.filter(|root| !root.is_empty()) {
        args.push(format!("--sysroot={root}"));
        args.push(format!("--gcc-toolchain={root}/usr"));
    }
    args
}

/// The xwin splat [`XWIN_ENV`] names, checked to hold the CRT headers.
pub fn xwin_dir() -> Result<PathBuf, Error> {
    let dir = std::env::var_os(XWIN_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| err(format!("{XWIN_ENV} is not set: a Windows module needs an xwin splat of the MSVC CRT and Windows SDK (docs/runtime/windows.md)")))?;
    if !dir.join("crt/include").is_dir() || !dir.join("sdk/include/ucrt").is_dir() {
        return Err(err(format!("{XWIN_ENV}={}: no crt/include or sdk/include/ucrt (an xwin splat?)", dir.display())));
    }
    Ok(dir)
}

/// The Windows target's clang arguments past `-target`, for the splat at
/// `xwin`: the static C runtime (libcmt, libvcruntime, libucrt) and the
/// CRT and UCRT headers as system headers.
///
/// Static, because the Xbox (a UWP app container) has no desktop
/// `vcruntime140.dll`, only the Store framework package's
/// `vcruntime140_app.dll`, which xwin does not splat
/// (docs/runtime/xbox.md). The module keeps a CRT of its own: the
/// generated C uses only the memory and math functions and never passes
/// CRT state (heap blocks, `FILE*`, errno) across the host boundary.
pub fn windows_target_args(xwin: &Path) -> Vec<String> {
    let include = |p: &str| xwin.join(p).to_string_lossy().into_owned();
    vec![
        "-fms-runtime-lib=static".into(),
        "-isystem".into(),
        include("crt/include"),
        "-isystem".into(),
        include("sdk/include/ucrt"),
    ]
}

/// `lld-link`'s arguments for linking `objects` into the DLL `out` against
/// the splat at `xwin`. Exports are the definitions the sources mark
/// `H2_EXPORT` (h2abi.h); no import library is written.
pub fn windows_link_args(xwin: &Path, out: &Path, objects: &[PathBuf]) -> Vec<OsString> {
    let mut v: Vec<OsString> = ["/dll", "/nologo", "/machine:x64", "/noimplib", "/incremental:no"]
        .into_iter()
        .map(OsString::from)
        .collect();
    for lib in ["crt/lib/x86_64", "sdk/lib/ucrt/x86_64", "sdk/lib/um/x86_64"] {
        let mut a = OsString::from("/libpath:");
        a.push(xwin.join(lib));
        v.push(a);
    }
    let mut o = OsString::from("/out:");
    o.push(out);
    v.push(o);
    v.extend(objects.iter().map(|p| p.as_os_str().to_owned()));
    v
}

/// Where Homebrew keeps the LLVM tools a Windows build uses, when neither
/// the environment nor `PATH` names them.
const LLVM_TOOL_DIRS: &[&str] = &[
    "/opt/homebrew/opt/lld@22/bin",
    "/opt/homebrew/opt/llvm@22/bin",
    "/opt/homebrew/opt/lld/bin",
    "/opt/homebrew/opt/llvm/bin",
    "/usr/local/opt/lld/bin",
    "/usr/local/opt/llvm/bin",
];

/// An LLVM tool for the Windows target (`lld-link`, `llvm-ar`): from the
/// directories in `H2_LLD` and `H2_LLVM` (platform/windows/env.sh), then
/// `PATH`, then Homebrew's [`LLVM_TOOL_DIRS`].
pub fn windows_tool(name: &str) -> Result<PathBuf, Error> {
    llvm_tool(name)
}

fn llvm_tool(name: &str) -> Result<PathBuf, Error> {
    let env: Vec<OsString> = ["H2_LLD", "H2_LLVM"].iter().filter_map(std::env::var_os).collect();
    let homebrew = std::env::join_paths(LLVM_TOOL_DIRS).ok();
    env.into_iter()
        .chain(std::env::var_os("PATH"))
        .chain(homebrew)
        .find_map(|dirs| resolve_on_path(name, Some(dirs)))
        .ok_or_else(|| err(format!("{name} not found (set H2_LLD/H2_LLVM, or `brew install lld@22 llvm@22`)")))
}

/// `xcrun --sdk <sdk> --show-sdk-path`, once per process.
fn sdk_path(sdk: &str) -> Result<String, Error> {
    static PATHS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    let mut paths = PATHS.lock().unwrap();
    if let Some((_, p)) = paths.iter().find(|(s, _)| s == sdk) {
        return Ok(p.clone());
    }
    let out = Command::new("xcrun")
        .args(["--sdk", sdk, "--show-sdk-path"])
        .output()
        .map_err(|e| err(format!("xcrun --sdk {sdk}: {e}")))?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !out.status.success() || path.is_empty() {
        return Err(err(format!(
            "the {sdk} SDK is not installed (xcrun: {})",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    paths.push((sdk.to_owned(), path.clone()));
    Ok(path)
}

/// Architectures selected by the CLI. Iteration builds the host by default;
/// `all` explicitly requests both release targets.
pub fn architectures(selection: Option<&str>) -> Result<Vec<Arch>, Error> {
    match selection.unwrap_or("host") {
        "host" => Ok(vec![Arch::host()]),
        "all" => Ok(Arch::ALL.to_vec()),
        "arm64" => Ok(vec![Arch::Arm64]),
        "x86_64" => Ok(vec![Arch::X86_64]),
        "tvos" => Ok(vec![Arch::Arm64Tvos]),
        "ios" => Ok(vec![Arch::Arm64Ios]),
        "windows" => Ok(vec![Arch::X86_64Windows]),
        "linux" | "x86_64-linux" => Ok(vec![Arch::X86_64Linux]),
        value => Err(err(format!("--arch {value}: expected host, all, arm64, x86_64, tvos, ios, windows or x86_64-linux"))),
    }
}

/// Result of building one architecture.
#[derive(Debug, Clone)]
pub struct Build {
    pub arch: Arch,
    /// The build's pins of its objects (`out_dir/pin/<arch>`): what the
    /// archive and [`host_dylib`] read. They stay valid until the next
    /// build of this architecture into the same `out_dir`.
    pub objects: Vec<PathBuf>,
    /// The same objects in the object directory ([`obj_dir`]), for
    /// [`prune_obj_cache`]'s `keep`.
    pub cache_objects: Vec<PathBuf>,
    pub compiled: usize,
    pub reused: usize,
    /// Sources in the cold tier ([`Tiers`]); 0 without tiers.
    pub cold: usize,
    /// Wall time for compiling (not archiving).
    pub compile_secs: f64,
    /// Toolchain probes, source hashing and pin management.
    pub cache_secs: f64,
    /// Archive validation or creation.
    pub archive_secs: f64,
    /// Sum of object file sizes (debug info included).
    pub object_bytes: u64,
    pub library: PathBuf,
    pub library_bytes: u64,
}

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn err(e: impl std::fmt::Display) -> Error {
    Error(e.to_string())
}

/// Environment variables that change what the same clang command line
/// produces or which clang/SDK runs: SDK and developer-directory
/// selection, deployment target, implicit include paths, driver overrides.
pub const TOOLCHAIN_ENV: &[&str] = &[
    "SDKROOT",
    "DEVELOPER_DIR",
    "MACOSX_DEPLOYMENT_TARGET",
    "CPATH",
    "C_INCLUDE_PATH",
    "CCC_OVERRIDE_OPTIONS",
    "CLANG_CONFIG_FILE_SYSTEM_DIR",
    "CLANG_CONFIG_FILE_USER_DIR",
];

/// The probes [`toolchain_identity`] runs: each output (or its absence) is
/// part of the identity. `clang` here means the resolved compiler the build
/// runs ([`compiler`]), invoked by its path; the macOS `/usr/bin/clang` shim
/// follows `DEVELOPER_DIR`/`xcode-select`, so the real binary, the developer
/// directory and the SDK are probed too.
pub const TOOLCHAIN_PROBES: &[(&str, &[&str])] = &[
    ("clang", &["--version"]),
    ("clang", &["-print-resource-dir"]),
    ("xcrun", &["--find", "clang"]),
    ("xcrun", &["--show-sdk-path"]),
    ("xcrun", &["--show-sdk-version"]),
    ("xcrun", &["--show-sdk-build-version"]),
    ("xcode-select", &["-p"]),
];

/// `name` resolved on a `PATH`-style list: the first entry holding an
/// executable file of that name, as `Command` would find it.
pub fn resolve_on_path(name: &str, path: Option<OsString>) -> Option<PathBuf> {
    std::env::split_paths(&path?).map(|d| d.join(name)).find(|p| std::fs::metadata(p).is_ok_and(|m| executable(&m)))
}

/// Whether a file's metadata says it can be run: an executable bit on Unix.
/// Elsewhere (Windows, where the emitter is only compile-checked) any file.
fn executable(m: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.is_file() && m.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        m.is_file()
    }
}

/// The C compiler: `clang` resolved on `PATH` once. The build invokes this
/// exact path and [`toolchain_identity`] fingerprints it, so the compiler
/// identified is the compiler run.
pub fn compiler() -> Result<PathBuf, Error> {
    resolve_on_path("clang", std::env::var_os("PATH")).ok_or_else(|| err("clang not found on PATH"))
}

/// Executables up to this size are fingerprinted by content (a wrapper
/// script's flags are its content); larger ones by size and mtime.
const HASHED_EXECUTABLE: u64 = 64 << 20;

/// The resolved compiler executable itself: its path, where its symlinks
/// lead, and its content (or size and modification time when large).
pub fn executable_fingerprint(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let what = match std::fs::metadata(&canonical) {
        Ok(m) if m.len() <= HASHED_EXECUTABLE => match std::fs::read(&canonical) {
            Ok(bytes) => format!("content {:016x}", fnv64(&bytes)),
            Err(e) => format!("unreadable: {e}"),
        },
        Ok(m) => format!("size {} mtime {:?}", m.len(), m.modified().ok()),
        Err(e) => format!("missing: {e}"),
    };
    format!("{} -> {}: {what}", path.display(), canonical.display())
}

/// Everything about the compiler `cc` and the SDK that the object cache key
/// must cover, as text: `cc`'s [`executable_fingerprint`], each
/// [`TOOLCHAIN_PROBES`] output (with `clang` run as `cc`) and each
/// [`TOOLCHAIN_ENV`] value.
pub fn toolchain_identity(cc: &Path) -> String {
    let probe = |cmd: &str, args: &[&str]| {
        let mut c = if cmd == "clang" { Command::new(cc) } else { Command::new(cmd) };
        let o = c.args(args).output().ok()?;
        Some(format!("{}{}{}", o.status, String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
    };
    toolchain_identity_with(&executable_fingerprint(cc), &|k| std::env::var(k).ok(), &probe)
}

/// [`toolchain_identity`] over a given compiler fingerprint, environment
/// and probe functions.
pub fn toolchain_identity_with(
    compiler: &str,
    env: &dyn Fn(&str) -> Option<String>,
    probe: &dyn Fn(&str, &[&str]) -> Option<String>,
) -> String {
    let mut s = format!("compiler {compiler}\n");
    for (cmd, args) in TOOLCHAIN_PROBES {
        s.push_str(&format!("$ {cmd} {}\n{}\n", args.join(" "), probe(cmd, args).as_deref().unwrap_or("<failed>")));
    }
    for k in TOOLCHAIN_ENV {
        s.push_str(&format!("{k}={}\n", env(k).as_deref().unwrap_or("<unset>")));
    }
    s
}

/// How [`build_in`] names sources in objects' debug info. Part of every
/// key, so objects written under an older scheme are never reused. 2:
/// relative to the source directory ([`source_prefix_map`]); before, the
/// generating output's absolute path, which a shared cache handed to every
/// other output.
const DEBUG_PATHS: &[u8] = b"debug paths 2: source-relative";

/// The part of every object's cache key shared by all sources of one
/// build: support headers, flags, target, toolchain.
pub fn common_key(h2gen: &[u8], h2abi: &[u8], arch: Arch, flags: &[String], toolchain: &str) -> u64 {
    let mut common = DEBUG_PATHS.to_vec();
    common.extend(h2gen);
    common.extend(h2abi);
    common.extend(flags.join(" ").as_bytes());
    common.extend(arch.triple().as_bytes());
    common.extend(toolchain.as_bytes());
    fnv64(&common)
}

/// Where [`build`] keeps objects: `H2_OBJ_CACHE` when it is set and not
/// empty, otherwise `out_dir/obj`. Object names are content keys
/// (`{stem}.{arch}.{key}.o`) and each is written to a per-process
/// temporary and renamed into place, so one cache can serve several
/// checkouts and concurrent builds.
pub fn obj_dir(out_dir: &Path) -> PathBuf {
    obj_dir_with(out_dir, std::env::var_os("H2_OBJ_CACHE"))
}

/// [`obj_dir`] with the value of `H2_OBJ_CACHE` given.
pub fn obj_dir_with(out_dir: &Path, cache: Option<OsString>) -> PathBuf {
    match cache {
        Some(c) if !c.is_empty() => PathBuf::from(c),
        _ => out_dir.join("obj"),
    }
}

/// A byte count such as `20G`, `512M`, `64K` or `1000000` (binary units,
/// case-insensitive, an optional trailing `B` or `iB`).
pub fn parse_size(s: &str) -> Result<u64, Error> {
    let t = s.trim().to_ascii_uppercase();
    let t = t.strip_suffix("IB").or_else(|| t.strip_suffix('B')).unwrap_or(&t);
    let (num, shift) = match t.as_bytes().last() {
        Some(b'K') => (&t[..t.len() - 1], 10),
        Some(b'M') => (&t[..t.len() - 1], 20),
        Some(b'G') => (&t[..t.len() - 1], 30),
        Some(b'T') => (&t[..t.len() - 1], 40),
        _ => (t, 0),
    };
    let n: u64 = num.trim().parse().map_err(|_| err(format!("{s:?}: expected a size such as 20G, 512M or 1000000")))?;
    n.checked_mul(1 << shift).ok_or_else(|| err(format!("{s:?}: too large")))
}

/// The cap from `H2_OBJ_CACHE_MAX`, if set.
pub fn obj_cache_max() -> Result<Option<u64>, Error> {
    match std::env::var("H2_OBJ_CACHE_MAX") {
        Ok(v) if !v.trim().is_empty() => parse_size(&v).map(Some).map_err(|e| err(format!("H2_OBJ_CACHE_MAX: {e}"))),
        _ => Ok(None),
    }
}

/// What [`prune_obj_cache`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Prune {
    pub removed: usize,
    pub freed: u64,
    /// Bytes of objects left.
    pub kept: u64,
}

/// Delete the least recently used objects (`*.o`, by modification time;
/// [`build`] touches every object it reuses) in `dir`, oldest first, until
/// the rest fit in `max_bytes`. Objects in `keep` (the build that just ran)
/// are never deleted, even if they alone exceed the cap. Temporaries left
/// by a killed compile are deleted once they are an hour old.
///
/// Another checkout's build is never broken by this: [`build`] archives
/// and links its own hard links (or copies) of the objects, not the cache
/// entries, so deleting an entry only costs a recompile later.
pub fn prune_obj_cache(dir: &Path, max_bytes: u64, keep: &HashSet<PathBuf>) -> Result<Prune, Error> {
    let now = std::time::SystemTime::now();
    let mut objs = Vec::new();
    let mut p = Prune::default();
    for e in std::fs::read_dir(dir).map_err(|e| err(format!("{}: {e}", dir.display())))?.flatten() {
        let path = e.path();
        let Ok(m) = e.metadata() else { continue };
        if !m.is_file() {
            continue;
        }
        let mtime = m.modified().unwrap_or(std::time::UNIX_EPOCH);
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".tmp") {
            if now.duration_since(mtime).is_ok_and(|d| d.as_secs() > 3600) && std::fs::remove_file(&path).is_ok() {
                p.removed += 1;
                p.freed += m.len();
            }
        } else if name.ends_with(".o") {
            p.kept += m.len();
            if !keep.contains(&path) {
                objs.push((mtime, m.len(), path));
            }
        }
    }
    objs.sort_by_key(|o| o.0);
    for (_, len, path) in objs {
        if p.kept <= max_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            p.kept -= len;
            p.removed += 1;
            p.freed += len;
        }
    }
    Ok(p)
}

/// Copy the runtime's ABI header next to the generated sources, so the
/// build only reads files under the output directory.
pub fn install_header(src_dir: &Path, h2abi: &Path) -> Result<(), Error> {
    let text = std::fs::read(h2abi).map_err(|e| err(format!("reading {}: {e}", h2abi.display())))?;
    let dst = src_dir.join("h2abi.h");
    if std::fs::read(&dst).ok().as_deref() != Some(&text[..]) {
        output::replace(&dst, &text).map_err(err)?;
    }
    Ok(())
}

/// Compile every `.c` in `src_dir` (which must hold `h2gen.h`, `h2simd.h`,
/// `h2x87.h` and `h2abi.h`) for `arch`, and archive them into `out_dir/libh2gen-<arch>.a`.
/// Objects go to [`obj_dir`] and are pinned under `out_dir` (see
/// [`build_in`]).
pub fn build(src_dir: &Path, out_dir: &Path, arch: Arch, jobs: usize) -> Result<Build, Error> {
    build_with(src_dir, out_dir, arch, jobs, &compiler()?)
}

/// [`build`] with the compiler executable `cc`.
pub fn build_with(src_dir: &Path, out_dir: &Path, arch: Arch, jobs: usize, cc: &Path) -> Result<Build, Error> {
    build_in(src_dir, out_dir, &obj_dir(out_dir), arch, jobs, cc)
}

/// A source's cache object, its pin, and whether it needs compiling.
type Keyed = Result<(PathBuf, PathBuf, bool), Error>;

/// Make `pin` a hard link to `obj` (a copy when they are on different
/// file systems), replacing nothing: an existing pin has the same content
/// key, so the same content. Fails with `NotFound` when `obj` is gone.
fn pin_object(obj: &Path, pin: &Path) -> std::io::Result<()> {
    match std::fs::hard_link(obj, pin) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(e),
        Err(_) => {
            let tmp = Temporary::new(pin)?;
            std::fs::copy(obj, tmp.path())?;
            tmp.publish(pin)
        }
    }
}

/// Set a file's modification time to now, so pruning by modification time
/// drops the least recently used.
fn touch(path: &Path) -> std::io::Result<()> {
    std::fs::File::options().append(true).open(path)?.set_modified(std::time::SystemTime::now())
}

/// The member names of the static library `lib`, from `ar -t`.
pub fn archive_members(lib: &Path) -> Result<Vec<String>, Error> {
    archive_members_with(Path::new("ar"), lib)
}

/// [`archive_members`] for `arch`'s archives: `llvm-ar t` for Windows,
/// whose `llvm-lib` archives name members by the path they were given.
pub fn archive_members_for(arch: Arch, lib: &Path) -> Result<Vec<String>, Error> {
    if matches!(arch, Arch::X86_64Windows | Arch::X86_64Linux) {
        archive_members_with(&llvm_tool("llvm-ar")?, lib)
    } else {
        archive_members(lib)
    }
}

fn archive_members_with(ar: &Path, lib: &Path) -> Result<Vec<String>, Error> {
    let o = Command::new(ar).arg("t").arg(lib).output().map_err(|e| err(format!("running {}: {e}", ar.display())))?;
    if !o.status.success() {
        return Err(Error(format!("{} t {}: {}", ar.display(), lib.display(), String::from_utf8_lossy(&o.stderr))));
    }
    Ok(String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with("__.SYMDEF"))
        .map(|l| Path::new(l).file_name().map_or_else(|| l.to_owned(), |n| n.to_string_lossy().into_owned()))
        .collect())
}

/// Whether `lib` holds exactly the objects `names` (sorted).
fn archive_matches(arch: Arch, lib: &Path, names: &[String]) -> bool {
    lib.exists()
        && archive_members_for(arch, lib).is_ok_and(|mut m| {
            m.sort();
            m == names
        })
}

/// Archive `objects` into the static library `library`: written to a
/// temporary, checked to hold exactly `objects`, then renamed into place.
/// Apple's libtool warns about a missing input and still exits 0, so any
/// diagnostic from it is a failure too. On failure `library` is untouched.
pub fn archive(library: &Path, objects: &[PathBuf]) -> Result<(), Error> {
    archive_for(Arch::host(), library, objects)
}

/// [`archive`] for `arch`'s objects: `llvm-lib` for Windows (Apple's
/// libtool takes only Mach-O), GNU-format llvm-ar for ELF, libtool on Apple targets.
pub fn archive_for(arch: Arch, library: &Path, objects: &[PathBuf]) -> Result<(), Error> {
    let mut names: Vec<String> = objects.iter().map(|o| o.file_name().unwrap().to_string_lossy().into_owned()).collect();
    names.sort();
    if let Some(o) = objects.iter().find(|o| !o.is_file()) {
        return Err(Error(format!("archiving {}: {} is missing", library.display(), o.display())));
    }
    let tmp = Temporary::new(library).map_err(err)?;
    let out = if arch.is_windows() {
        let mut out = OsString::from("/out:");
        out.push(tmp.path());
        Command::new(windows_tool("llvm-lib")?).arg("/nologo").arg(out).args(objects).output()
    } else if arch == Arch::X86_64Linux {
        // llvm-ar updates existing archives. Temporary reserves an empty file.
        std::fs::write(tmp.path(), b"!<arch>\n").map_err(err)?;
        Command::new(llvm_tool("llvm-ar")?).args(["--format=gnu", "rcs"]).arg(tmp.path()).args(objects).output()
    } else {
        Command::new("libtool").args(["-static", "-no_warning_for_no_symbols", "-o"]).arg(tmp.path()).args(objects).output()
    }
    .map_err(|e| err(format!("running the archiver: {e}")))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() || !stderr.trim().is_empty() {
        return Err(err(format!("archiving: {stderr}")));
    }
    if !archive_matches(arch, tmp.path(), &names) {
        return Err(err(format!("archive {} does not hold exactly the {} objects given", tmp.path().display(), names.len())));
    }
    tmp.publish(library).map_err(err)
}

/// The absolute source directory and the clang flag that names it `.` in
/// debug info. An object's line tables then name `./shard_NNNNN.c`, the
/// same from every output directory, so an object a shared `H2_OBJ_CACHE`
/// hands to another output does not point at the output that compiled it
/// (which may since be deleted or regenerated). Consumers resolve the name
/// against the source directory they built from: `perf::symbolize` uses
/// `src/` beside the dylib.
fn source_prefix_map(src_dir: &Path) -> Result<(PathBuf, OsString), Error> {
    let abs = std::path::absolute(src_dir).map_err(|e| err(format!("{}: {e}", src_dir.display())))?;
    let mut flag = OsString::from("-fdebug-prefix-map=");
    flag.push(abs.as_os_str());
    flag.push("=.");
    Ok((abs, flag))
}

/// [`build_with`] keeping objects in `obj_dir`.
///
/// Every object of the build is pinned in `out_dir/pin/<arch>` (a hard
/// link to the cache entry, or a copy across file systems) before it is
/// used, and the archive and [`host_dylib`] read the pins. A shared cache
/// pruned by another checkout mid-build can then only make this build
/// recompile, never lose an input. The archive is written by [`archive`];
/// an existing one is reused only if it holds exactly the build's objects.
pub fn build_in(src_dir: &Path, out_dir: &Path, obj_dir: &Path, arch: Arch, jobs: usize, cc: &Path) -> Result<Build, Error> {
    build_tiered(src_dir, out_dir, obj_dir, arch, jobs, cc, Tiers::from_env()?.as_ref())
}

/// [`build_in`] with the optimisation tiers given instead of read from
/// `H2_OPT_COLD` and `H2_HOT`: with `tiers`, a source holding none of its
/// hot addresses compiles at its cold level ([`Tiers`]). Each tier has its
/// own flags in its objects' keys and its own dependency and driver probes.
pub fn build_tiered(
    src_dir: &Path,
    out_dir: &Path,
    obj_dir: &Path,
    arch: Arch,
    jobs: usize,
    cc: &Path,
    tiers: Option<&Tiers>,
) -> Result<Build, Error> {
    let cache_start = Instant::now();
    let pin_dir = out_dir.join("pin").join(arch.name());
    for d in [obj_dir, out_dir, &pin_dir] {
        std::fs::create_dir_all(d).map_err(err)?;
    }
    let read = |name: &str| std::fs::read(src_dir.join(name)).map_err(|e| err(format!("{name}: {e}")));
    // h2gen.h includes h2simd.h, h2x87.h and h2fastmem.h: all are support
    // headers' identity.
    let support = [read("h2gen.h")?, read("h2simd.h")?, read("h2x87.h")?, read("h2fastmem.h")?].concat();
    let flags = cflags_for(arch)?;
    let toolchain = toolchain_identity(cc);
    let abi = read("h2abi.h")?;
    let common = common_key(&support, &abi, arch, &flags, &toolchain);
    // The cold tier: its flags, key and probes. Unused without tiers.
    let cold_flags = tiers.map_or_else(|| flags.clone(), |t| t.cold_flags(&flags));
    let cold_common = common_key(&support, &abi, arch, &cold_flags, &toolchain);
    let (src_abs, prefix_map) = source_prefix_map(src_dir)?;

    let mut sources: Vec<PathBuf> = std::fs::read_dir(src_dir)
        .map_err(err)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "c"))
        .collect();
    sources.sort();
    let source_root = std::fs::canonicalize(src_dir).map_err(err)?;
    let headers = dependencies::Headers::default();
    let cold_headers = dependencies::Headers::default();

    // (source, object, pin, needs compiling). Hashing ~400 MB of sources is
    // serial work worth spreading over the cores on a no-op rebuild.
    let keyed: Mutex<Vec<(usize, bool, Keyed)>> = Mutex::new(Vec::new());
    let cursor = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..jobs.clamp(1, 16) {
            s.spawn(|| {
                loop {
                    let i = cursor.fetch_add(1, Ordering::Relaxed);
                    let Some(src) = sources.get(i) else { break };
                    let mut cold = false;
                    let r = std::fs::read(src).map_err(err).and_then(|text| {
                        cold = tiers.is_some_and(|t| !t.is_hot(&text));
                        let (flags, headers, common) =
                            if cold { (&cold_flags, &cold_headers, cold_common) } else { (&flags, &headers, common) };
                        let dependencies = headers.fingerprint(cc, flags, arch, src, &text, &source_root)?;
                        let key = fnv64(&[&common.to_le_bytes()[..], &dependencies.to_le_bytes()[..], &text].concat());
                        let stem = src.file_stem().unwrap().to_string_lossy();
                        let name = format!("{stem}.{}.{key:016x}.o", arch.name());
                        let (obj, pin) = (obj_dir.join(&name), pin_dir.join(&name));
                        // An object is reused only once pinned: the link
                        // either holds it or finds it already pruned.
                        let fresh = if pin.exists() {
                            // Restore a pruned cache entry from the pin.
                            if !obj.exists() {
                                let _ = pin_object(&pin, &obj);
                            }
                            false
                        } else {
                            match pin_object(&obj, &pin) {
                                Ok(()) => false,
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
                                Err(e) => {
                                    return Err(err(format!("pinning {}: {e}", obj.display())));
                                }
                            }
                        };
                        if !fresh {
                            let _ = touch(&pin);
                            let _ = touch(&obj);
                        }
                        Ok((obj, pin, fresh))
                    });
                    keyed.lock().unwrap().push((i, cold, r));
                }
            });
        }
    });
    let mut keyed = keyed.into_inner().unwrap();
    keyed.sort_by_key(|k| k.0);
    let mut jobs_list = Vec::new();
    for (i, cold, r) in keyed {
        let (obj, pin, fresh) = r?;
        jobs_list.push((sources[i].clone(), obj, pin, fresh, cold));
    }
    // Pins of earlier builds' objects are not inputs any more.
    let current: HashSet<&PathBuf> = jobs_list.iter().map(|j| &j.2).collect();
    for e in std::fs::read_dir(&pin_dir).map_err(err)?.flatten() {
        if !current.contains(&e.path()) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let cache_secs = cache_start.elapsed().as_secs_f64();
    let todo: Vec<_> = jobs_list.iter().filter(|j| j.3).collect();
    let next = AtomicUsize::new(0);
    let failures = Mutex::new(Vec::new());
    let start = Instant::now();
    // Largest first, so a big shard doesn't start last.
    let mut order: Vec<usize> = (0..todo.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(std::fs::metadata(&todo[i].0).map(|m| m.len()).unwrap_or(0)));
    let target = arch.target_args()?;
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&j) = order.get(i) else { break };
                    let (src, obj, pin, _, cold) = todo[j];
                    let flags = if *cold { &cold_flags } else { &flags };
                    let tmp = match Temporary::new(obj) {
                        Ok(tmp) => tmp,
                        Err(e) => {
                            failures.lock().unwrap().push(format!("{}: {e}", obj.display()));
                            continue;
                        }
                    };
                    // Not in the key: every tree maps to the same `.`.
                    let out = Command::new(cc)
                        .args(flags)
                        .arg(&prefix_map)
                        .args(&target)
                        .arg("-c")
                        .arg(src_abs.join(src.file_name().unwrap()))
                        .arg("-o")
                        .arg(tmp.path())
                        .output();
                    match out {
                        Ok(o) if o.status.success() => {
                            // Pin before publishing, so a pruner never sees
                            // the object unpinned.
                            if let Err(e) = tmp.require_nonempty().and_then(|()| pin_object(tmp.path(), pin)).and_then(|()| tmp.publish(obj)) {
                                failures.lock().unwrap().push(format!("{}: {e}", obj.display()));
                            }
                        }
                        Ok(o) => failures.lock().unwrap().push(format!(
                            "{}: {}",
                            src.display(),
                            String::from_utf8_lossy(&o.stderr).lines().take(20).collect::<Vec<_>>().join("\n")
                        )),
                        Err(e) => failures.lock().unwrap().push(format!("running clang: {e}")),
                    }
                }
            });
        }
    });
    let compile_secs = start.elapsed().as_secs_f64();
    let failures = failures.into_inner().unwrap();
    if !failures.is_empty() {
        return Err(Error(format!("{} files failed to compile:\n{}", failures.len(), failures.join("\n"))));
    }

    let objects: Vec<PathBuf> = jobs_list.iter().map(|j| j.2.clone()).collect();
    let cache_objects: Vec<PathBuf> = jobs_list.iter().map(|j| j.1.clone()).collect();
    let mut object_bytes = 0;
    for o in &objects {
        let m = std::fs::metadata(o).map_err(|e| err(format!("pinned object {}: {e}", o.display())))?;
        object_bytes += m.len();
    }
    let archive_start = Instant::now();
    let library = out_dir.join(format!("libh2gen-{}.a", arch.name()));
    // Object names embed their content keys, so the same member names mean
    // the same archive: skip archiving when it already holds exactly these.
    let mut names: Vec<String> = objects.iter().map(|o| o.file_name().unwrap().to_string_lossy().into_owned()).collect();
    names.sort();
    // Written by earlier versions, which trusted it over the archive.
    let _ = std::fs::remove_file(library.with_extension("a.members"));
    if !archive_matches(arch, &library, &names) {
        archive_for(arch, &library, &objects)?;
    }
    let library_bytes = std::fs::metadata(&library).map(|m| m.len()).unwrap_or(0);
    Ok(Build {
        arch,
        compiled: todo.len(),
        reused: jobs_list.len() - todo.len(),
        cold: jobs_list.iter().filter(|j| j.4).count(),
        objects,
        cache_objects,
        compile_secs,
        cache_secs,
        archive_secs: archive_start.elapsed().as_secs_f64(),
        object_bytes,
        library,
        library_bytes,
    })
}

/// Link with the published SONAME even though the output is a temporary sibling.
/// libm supplies fma/sqrt on baseline x86-64.
fn linux_link_args(out: &Path, temporary: &Path, objects: &[PathBuf]) -> Vec<OsString> {
    // Direct host imports are bound by dlopen against the executable's dynamic
    // symbol table. Keep shared-output defaults for objects and permit undefined
    // symbols in input shared libraries as well.
    let mut args: Vec<OsString> = [
        "-fuse-ld=lld", "-shared", "-Wl,--allow-shlib-undefined", "-Wl,-soname", "-Xlinker",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(Arch::X86_64Linux.install_name(out));
    args.push("-o".into());
    args.push(temporary.as_os_str().to_owned());
    args.extend(objects.iter().map(|p| p.as_os_str().to_owned()));
    args.push("-lm".into());
    args
}

/// Link a build's objects plus extra C sources (a test bridge providing the
/// ABI symbols) into a Mach-O dylib, an ELF shared object, or a Windows DLL
/// (`lld-link`, exporting what the sources mark `H2_EXPORT`).
pub fn host_dylib(build: &Build, src_dir: &Path, extra: &[(&str, &str)], out: &Path) -> Result<(), Error> {
    let cc = compiler()?;
    let flags = cflags_for(build.arch)?;
    let target = build.arch.target_args()?;
    let mut args: Vec<PathBuf> = build.objects.clone();
    // H2_PGO=gen: the driver links the profile runtime when the link names
    // the instrumentation flag; the shim names the profile and exports the
    // write.
    let generate = pgo::generates(&flags);
    if generate && build.arch.is_windows() {
        return Err(err("H2_PGO=gen: a Windows module cannot link the profile runtime yet"));
    }
    let shim = [(pgo::SHIM_NAME, pgo::SHIM)];
    let extra = extra.iter().chain(if generate { &shim[..] } else { &[] });
    for (name, text) in extra {
        let src = src_dir.join(name);
        output::replace(&src, text.as_bytes()).map_err(err)?;
        let obj = src.with_extension("o");
        let temporary = Temporary::new(&obj).map_err(err)?;
        let o = Command::new(&cc)
            .args(&flags)
            .args(&target)
            .arg("-c")
            .arg(&src)
            .arg("-o")
            .arg(temporary.path())
            .output();
        // The bridge is not a generated shard: keep it out of later builds.
        let _ = std::fs::remove_file(&src);
        let o = o.map_err(err)?;
        if !o.status.success() {
            return Err(Error(format!("{name}: {}", String::from_utf8_lossy(&o.stderr))));
        }
        temporary.require_nonempty().map_err(err)?;
        temporary.publish(&obj).map_err(err)?;
        args.push(obj);
    }
    let temporary = Temporary::new(out).map_err(err)?;
    let o = if build.arch.is_windows() {
        Command::new(windows_tool("lld-link")?).args(windows_link_args(&xwin_dir()?, temporary.path(), &args)).output()
    } else if build.arch == Arch::X86_64Linux {
        Command::new(&cc)
            .args(&target)
            .args(generate.then_some(pgo::GEN_FLAG))
            .args(linux_link_args(out, temporary.path(), &args))
            .output()
    } else {
        Command::new(&cc)
            .args(&target)
            .args(generate.then_some(pgo::GEN_FLAG))
            // The instrumented image is too big for compact unwind: ld64
            // asserts that a function's offset fits in 24 bits. DWARF
            // unwind info serves the training run.
            .args(generate.then_some(["-Xlinker", "-no_compact_unwind"]).into_iter().flatten())
            .args(["-dynamiclib", "-Xlinker", "-install_name", "-Xlinker"])
            .arg(build.arch.install_name(out))
            .arg("-o")
            .arg(temporary.path())
            .args(&args)
            .output()
    }
    .map_err(err)?;
    if !o.status.success() {
        return Err(Error(format!("linking dylib: {}", String::from_utf8_lossy(&o.stderr))));
    }
    temporary.require_nonempty().map_err(err)?;
    temporary.publish(out).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(env: &[(&str, &str)], version: &str) -> String {
        let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let version = version.to_string();
        toolchain_identity_with(
            "/usr/bin/clang: content 1",
            &move |k| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()),
            &move |cmd, args| Some(if args == ["--version"] { version.clone() } else { format!("{cmd} ok") }),
        )
    }

    #[test]
    fn the_emit_target_defaults_to_the_host_and_accepts_apple_devices() {
        assert_eq!(Arch::target_from(None).unwrap(), Arch::host());
        assert_eq!(Arch::target_from(Some("")).unwrap(), Arch::host());
        assert_eq!(Arch::target_from(Some("host")).unwrap(), Arch::host());
        assert_eq!(Arch::target_from(Some("macos")).ok(), Arch::host().is_macos().then_some(Arch::host()));
        assert_eq!(Arch::target_from(Some("tvos")).unwrap(), Arch::Arm64Tvos);
        assert_eq!(Arch::target_from(Some("ios")).unwrap(), Arch::Arm64Ios);
        assert!(Arch::target_from(Some("iphoneos")).is_err());
        assert_eq!(architectures(Some("tvos")).unwrap(), vec![Arch::Arm64Tvos]);
        assert_eq!(architectures(Some("ios")).unwrap(), vec![Arch::Arm64Ios]);
        for a in [Arch::Arm64Tvos, Arch::Arm64Ios] {
            assert!(!Arch::ALL.contains(&a), "--arch all stays the macOS release pair");
            assert!(!a.is_macos());
        }
        assert!(Arch::Arm64.is_macos() && Arch::X86_64.is_macos());
    }

    #[test]
    fn windows_is_asked_for_by_name_and_never_the_mac_host() {
        assert_eq!(Arch::target_from(Some("windows")).unwrap(), Arch::X86_64Windows);
        assert_eq!(architectures(Some("windows")).unwrap(), vec![Arch::X86_64Windows]);
        assert!(!Arch::ALL.contains(&Arch::X86_64Windows), "--arch all stays the macOS release pair");
        assert!(Arch::target_from(Some("win64")).is_err());
        if cfg!(target_vendor = "apple") {
            assert_ne!(Arch::host(), Arch::X86_64Windows);
            assert!(!Arch::host().is_windows());
        }
        assert_eq!(Arch::host() == Arch::X86_64Windows, cfg!(windows));
    }

    #[test]
    fn windows_objects_are_named_and_keyed_apart_and_link_as_a_dll() {
        assert_eq!(Arch::X86_64Windows.triple(), "x86_64-pc-windows-msvc");
        assert_eq!(Arch::X86_64Windows.sdk(), None);
        for other in [Arch::Arm64, Arch::X86_64, Arch::Arm64Tvos] {
            assert_ne!(Arch::X86_64Windows.name(), other.name(), "object and pin names");
            let key = |a| common_key(b"h2gen", b"h2abi", a, &[], "toolchain");
            assert_ne!(key(Arch::X86_64Windows), key(other));
            assert!(!other.is_windows());
            assert_eq!(other.module_file_name(), "libh2game.dylib");
        }
        assert_eq!(Arch::X86_64Windows.module_file_name(), "h2game.dll");
        let xwin = Path::new("/x");
        assert_eq!(
            windows_target_args(xwin),
            ["-fms-runtime-lib=static", "-isystem", "/x/crt/include", "-isystem", "/x/sdk/include/ucrt"]
        );
        let link = windows_link_args(xwin, Path::new("/o/h2game.dll"), &[PathBuf::from("/o/a.o"), PathBuf::from("/o/b.o")]);
        let link: Vec<&str> = link.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            link,
            [
                "/dll",
                "/nologo",
                "/machine:x64",
                "/noimplib",
                "/incremental:no",
                "/libpath:/x/crt/lib/x86_64",
                "/libpath:/x/sdk/lib/ucrt/x86_64",
                "/libpath:/x/sdk/lib/um/x86_64",
                "/out:/o/h2game.dll",
                "/o/a.o",
                "/o/b.o",
            ]
        );
    }

    #[test]
    fn linux_selects_the_gnu_target_and_uses_its_own_object_keys() {
        let linux = Arch::X86_64Linux;
        assert_eq!(Arch::target_from(Some("x86_64-linux")).unwrap(), linux);
        assert_eq!(architectures(Some("x86_64-linux")).unwrap(), [linux]);
        assert_eq!(Arch::host() == linux, cfg!(all(target_os = "linux", target_arch = "x86_64")));
        assert!(!Arch::ALL.contains(&linux));
        assert_eq!(linux.name(), "x86_64-linux");
        assert_eq!(linux.triple(), "x86_64-unknown-linux-gnu");
        assert_eq!(linux.sdk(), None);
        let key = |a| common_key(b"h2gen", b"h2abi", a, &[], "toolchain");
        for other in [Arch::X86_64, Arch::X86_64Windows] {
            assert_ne!(linux.name(), other.name());
            assert_ne!(key(linux), key(other));
        }
        assert_eq!(linux.module_file_name(), "libh2game.so");
        assert_eq!(linux.default_opt(), "1");
    }

    #[test]
    fn linux_clang_args_select_pic_and_an_optional_debian_sysroot() {
        let linux = Arch::X86_64Linux;
        assert_eq!(linux.codegen_flags(), ["-fPIC", "-fvisibility=hidden"]);
        let flags = cflags_for(linux).unwrap();
        assert!(flags.windows(2).any(|f| f == linux.codegen_flags()));
        assert_eq!(linux_target_args(None), ["-target", linux.triple()]);
        assert_eq!(linux_target_args(Some("")), linux_target_args(None));
        assert_eq!(linux_target_args(Some("/Debian SDK")),
            ["-target", linux.triple(), "--sysroot=/Debian SDK", "--gcc-toolchain=/Debian SDK/usr"]);
        assert_eq!(linux.target_args().unwrap(), linux_target_args(std::env::var("H2_LINUX_SYSROOT").ok().as_deref()));
    }

    #[test]
    fn linux_link_uses_the_published_soname_and_links_math_after_objects() {
        let out = Path::new("/release/libh2game.so");
        let args = linux_link_args(out, Path::new("/release/.temporary"), &[PathBuf::from("shard.o")]);
        assert!(args.contains(&OsString::from("-shared")));
        assert!(args.contains(&OsString::from("-fuse-ld=lld")));
        assert!(args.contains(&OsString::from("-Wl,--allow-shlib-undefined")));
        let args: Vec<_> = args.iter().map(|a| a.to_str().unwrap()).collect();
        assert!(args.windows(3).any(|a| a == ["-Wl,-soname", "-Xlinker", "libh2game.so"]));
        assert!(args.windows(2).any(|a| a == ["-o", "/release/.temporary"]));
        assert!(args.ends_with(&["shard.o", "-lm"]));
    }

    #[test]
    fn tvos_objects_are_named_and_keyed_apart_from_macos() {
        assert_eq!(Arch::Arm64Tvos.triple(), format!("arm64-apple-tvos{TVOS_DEPLOYMENT_TARGET}"));
        assert_ne!(Arch::Arm64Tvos.name(), Arch::Arm64.name(), "object and pin names");
        let key = |a| common_key(b"h2gen", b"h2abi", a, &[], "toolchain");
        assert_ne!(key(Arch::Arm64Tvos), key(Arch::Arm64));
        assert_eq!(Arch::Arm64.sdk(), None);
        assert_eq!(Arch::Arm64.target_args().unwrap(), ["-target", "arm64-apple-macos11"]);
        let out = Path::new("/o/libh2game.dylib");
        assert_eq!(Arch::Arm64.install_name(out), out.as_os_str());
        assert_eq!(Arch::Arm64Tvos.install_name(out), "@rpath/h2game.framework/h2game");
    }

    #[test]
    fn ios_objects_are_named_and_keyed_apart_from_macos_and_tvos() {
        assert_eq!(Arch::Arm64Ios.triple(), format!("arm64-apple-ios{IOS_DEPLOYMENT_TARGET}"));
        let names = [Arch::Arm64, Arch::X86_64, Arch::Arm64Tvos, Arch::Arm64Ios].map(Arch::name);
        for (i, a) in names.iter().enumerate() {
            assert!(!names[i + 1..].contains(a), "object and pin names: {names:?}");
        }
        let key = |a| common_key(b"h2gen", b"h2abi", a, &[], "toolchain");
        assert_ne!(key(Arch::Arm64Ios), key(Arch::Arm64));
        assert_ne!(key(Arch::Arm64Ios), key(Arch::Arm64Tvos));
        assert_eq!(Arch::Arm64Ios.sdk(), Some("iphoneos"));
        assert_eq!(Arch::Arm64Ios.install_name(Path::new("/o/x.dylib")), "@rpath/h2game.framework/h2game");
    }

    /// The tvOS and iOS arguments compile and link a dylib whose load
    /// commands name that platform, with the SDK's headers (generated code
    /// includes math.h).
    fn device_target_args_build_a_dylib(arch: Arch, sysroot: &str, platform: &str) {
        let Ok(args) = arch.target_args() else {
            eprintln!("SKIPPED {arch:?} dylib: no {:?} SDK", arch.sdk());
            return;
        };
        assert_eq!(&args[..3], ["-target", arch.triple(), "-isysroot"]);
        assert!(args[3].contains(sysroot), "{args:?}");
        let Ok(cc) = compiler() else {
            eprintln!("SKIPPED {arch:?} dylib: no clang");
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-emit-{}-{}", arch.name(), std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("t.c");
        std::fs::write(&src, "#include <math.h>\n#include <string.h>\ndouble h2_t(double x) { return sqrt(x) + (double)strlen(\"ab\"); }\n").unwrap();
        let lib = dir.join("libt.dylib");
        let o = Command::new(&cc)
            .args(&args)
            .args(["-dynamiclib", "-Xlinker", "-install_name", "-Xlinker"])
            .arg(arch.install_name(&lib))
            .arg(&src)
            .arg("-o")
            .arg(&lib)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let show = Command::new("xcrun").args(["vtool", "-show-build"]).arg(&lib).output().unwrap();
        let text = String::from_utf8_lossy(&show.stdout);
        assert!(text.lines().any(|l| l.split_whitespace().eq(["platform", platform])), "{text}");
        assert!(!text.contains("MACOS"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tvos_target_args_build_a_tvos_dylib() {
        device_target_args_build_a_dylib(Arch::Arm64Tvos, "AppleTVOS", "TVOS");
    }

    #[test]
    fn ios_target_args_build_an_ios_dylib() {
        device_target_args_build_a_dylib(Arch::Arm64Ios, "iPhoneOS", "IOS");
    }

    #[test]
    fn architecture_selection_defaults_to_the_host() {
        assert_eq!(architectures(None).unwrap(), vec![Arch::host()]);
        assert_eq!(architectures(Some("host")).unwrap(), vec![Arch::host()]);
        assert_eq!(architectures(Some("all")).unwrap(), Arch::ALL);
        assert_eq!(architectures(Some("arm64")).unwrap(), vec![Arch::Arm64]);
        assert_eq!(architectures(Some("x86_64")).unwrap(), vec![Arch::X86_64]);
        assert!(architectures(Some("aarch64")).is_err());
        assert!(architectures(Some("")).is_err());
    }

    #[test]
    fn the_object_key_covers_compiler_sdk_and_environment() {
        let flags = cflags().unwrap();
        let key = |id: &str| common_key(b"h2gen", b"h2abi", Arch::Arm64, &flags, id);
        let base = identity(&[], "clang 17");
        assert_eq!(key(&base), key(&identity(&[], "clang 17")), "deterministic");
        assert_ne!(key(&base), key(&identity(&[], "clang 18")), "compiler version");
        // Independent of the production list, so dropping an entry from it
        // fails here.
        const REQUIRED_ENV: [&str; 8] = [
            "SDKROOT",
            "DEVELOPER_DIR",
            "MACOSX_DEPLOYMENT_TARGET",
            "CPATH",
            "C_INCLUDE_PATH",
            "CCC_OVERRIDE_OPTIONS",
            "CLANG_CONFIG_FILE_SYSTEM_DIR",
            "CLANG_CONFIG_FILE_USER_DIR",
        ];
        for var in REQUIRED_ENV {
            assert_ne!(key(&base), key(&identity(&[(var, "/x")], "clang 17")), "{var}");
        }
        // Another executable with the same probe output is another compiler.
        let other_exe = toolchain_identity_with("/opt/bin/clang: content 2", &|_| None, &|cmd, args| {
            Some(if args == ["--version"] { "clang 17".into() } else { format!("{cmd} ok") })
        });
        assert_ne!(key(&base), key(&other_exe), "compiler executable");
        assert_ne!(key(&base), common_key(b"h2gen", b"h2abi", Arch::X86_64, &flags, &base), "target");
        // A probe that changes output (another SDK) changes the key.
        let other_sdk = toolchain_identity_with("cc", &|_| None, &|cmd, args| {
            Some(if args == ["--show-sdk-path"] { "/SDKs/Other.sdk".into() } else { format!("{cmd} ok") })
        });
        let same_sdk = toolchain_identity_with("cc", &|_| None, &|cmd, _| Some(format!("{cmd} ok")));
        assert_ne!(key(&other_sdk), key(&same_sdk));
    }

    #[test]
    fn fast_paths_default_on_and_zero_turns_them_off() {
        assert!(flag("H2_FASTMEM", None, true).unwrap());
        assert!(flag("H2_FASTMEM", Some(""), true).unwrap());
        assert!(!flag("H2_FASTMEM", Some("0"), true).unwrap());
        assert!(!flag("H2_PERF", None, false).unwrap());
        assert!(flag("H2_PERF", Some("1"), false).unwrap());
        assert!(flag("H2_X87_NATIVE", Some("yes"), true).is_err());
        let flags = cflags().unwrap();
        for name in [
            "H2_FASTMEM",
            "H2_X87_NATIVE",
            "H2_SSE_NATIVE",
            "H2_DIRECT_CALLS",
            "H2_VIEW_CHARGE",
        ] {
            let on = std::env::var(name).as_deref() != Ok("0");
            assert_eq!(flags.contains(&format!("-D{name}=1")), on, "{name}");
        }
        let inline = std::env::var("H2_DIRTY_INLINE").as_deref() == Ok("1");
        assert_eq!(flags.contains(&"-DH2_DIRTY_INLINE=1".to_string()), inline);
        let stats = std::env::var("H2_COLD_STATS").as_deref() == Ok("1");
        assert_eq!(flags.contains(&"-DH2_COLD_STATS=1".to_string()), stats);
        let gate = std::env::var("H2_DIRECT_GATE").as_deref() == Ok("1");
        assert_eq!(flags.contains(&"-DH2_DIRECT_GATE=1".to_string()), gate);
        for name in ["H2_DIRECT_COUNT", "H2_ENTRY_GATE"] {
            let on = std::env::var(name).as_deref() == Ok("1");
            assert_eq!(flags.contains(&format!("-D{name}=1")), on, "{name}");
        }
    }

    #[test]
    fn profile_flags_are_validated_and_keyed() {
        let f = |o, d| profile_flags(o, d).unwrap();
        assert_eq!(f(None, None), ["-O1", "-gline-tables-only"]);
        assert_eq!(f(Some("0"), Some("full")), ["-O0", "-g"]);
        assert_eq!(f(Some("2"), Some("none")), ["-O2"]);
        assert_eq!(f(Some("3"), None), ["-O3", "-gline-tables-only"]);
        assert!(profile_flags(Some("4"), None).is_err());
        assert!(profile_flags(Some("fast"), None).is_err());
        assert!(profile_flags(None, Some("dwarf")).is_err());
        let key = |o, d| common_key(b"a", b"b", Arch::Arm64, &f(o, d), "t");
        assert_ne!(key(None, None), key(Some("0"), None), "opt level");
        assert_ne!(key(Some("2"), None), key(Some("3"), None), "-O3");
        assert_ne!(key(None, None), key(None, Some("full")), "debug level");
    }

    #[test]
    fn windows_defaults_to_o2_and_the_apple_targets_to_o1() {
        assert_eq!(Arch::X86_64Windows.default_opt(), "2");
        for arch in [Arch::Arm64, Arch::X86_64, Arch::Arm64Tvos, Arch::Arm64Ios] {
            assert_eq!(arch.default_opt(), "1", "{arch:?}");
            assert_eq!(profile_flags_for(arch, None, None).unwrap(), profile_flags(None, None).unwrap(), "{arch:?}");
        }
        let f = |o| profile_flags_for(Arch::X86_64Windows, o, Some("none")).unwrap();
        assert_eq!(f(None), ["-O2"]);
        assert_eq!(f(Some("1")), ["-O1"], "H2_OPT still wins");
        assert!(profile_flags_for(Arch::X86_64Windows, Some("4"), None).is_err());
        // What the build compiles with and the stamp records: with H2_OPT
        // unset (as `pack` requires), Windows objects are -O2.
        if std::env::var_os("H2_OPT").is_none() {
            let win = cflags_for(Arch::X86_64Windows).unwrap();
            assert!(win.iter().any(|f| f == "-O2") && !win.iter().any(|f| f == "-O1"), "{win:?}");
            let mac = cflags_for(Arch::Arm64).unwrap();
            assert!(mac.iter().any(|f| f == "-O1") && !mac.iter().any(|f| f == "-O2"), "{mac:?}");
        }
    }

    #[test]
    fn windows_codegen_flags_follow_cflags_and_apple_targets_use_the_defaults() {
        assert_eq!(Arch::X86_64Windows.codegen_flags(), ["-march=x86-64-v3", "-mtune=znver2", "-fno-math-errno"]);
        for arch in [Arch::Arm64, Arch::X86_64, Arch::Arm64Tvos, Arch::Arm64Ios] {
            assert!(arch.codegen_flags().is_empty(), "{arch:?}");
            let flags = cflags_for(arch).unwrap();
            assert!(!flags.iter().any(|f| f.starts_with("-march") || f.starts_with("-mtune") || f == "-fno-math-errno"), "{flags:?}");
        }
        assert_eq!(cflags().unwrap(), cflags_for(Arch::host()).unwrap());
        let win = cflags_for(Arch::X86_64Windows).unwrap();
        assert_eq!(&win[..CFLAGS.len()], CFLAGS);
        // After -fno-fast-math, which would otherwise restore MSVC's
        // math-errno default.
        let at = |f: &str| win.iter().position(|x| x == f).unwrap();
        assert!(at("-fno-fast-math") < at("-fno-math-errno"));
        let mac = cflags_for(Arch::Arm64).unwrap();
        assert_ne!(
            common_key(b"a", b"b", Arch::X86_64Windows, &win, "t"),
            common_key(b"a", b"b", Arch::X86_64Windows, &mac, "t"),
            "the flags are in the object key"
        );
        // A cold tier changes only the level.
        let tiers = Tiers::new("1", "0x00100000").unwrap();
        let cold = tiers.cold_flags(&win);
        assert!(cold.iter().any(|f| f == "-march=x86-64-v3") && cold.iter().any(|f| f == "-fno-math-errno"));
    }

    /// The point of the Windows flags, on the compiler: the native x87
    /// path's `__builtin_fma` becomes an instruction rather than a call to
    /// the UCRT's software `fma`, and `sqrt` needs no errno call.
    #[test]
    fn windows_flags_make_fma_and_sqrt_instructions() {
        let Ok(cc) = compiler() else {
            eprintln!("SKIPPED windows_flags_make_fma_and_sqrt_instructions: no clang");
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-win-flags-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("probe.c");
        std::fs::write(
            &src,
            "double probe_fma(double a, double b, double c) { return __builtin_fma(a, b, c); }\n\
             double probe_sqrt(double x) { return __builtin_sqrt(x); }\n",
        )
        .unwrap();
        let object = |name: &str, flags: &[String]| {
            let out = dir.join(name);
            let o = Command::new(&cc)
                .args(flags)
                .args(["-target", Arch::X86_64Windows.triple(), "-c"])
                .arg(&src)
                .arg("-o")
                .arg(&out)
                .output()
                .unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
            std::fs::read(out).unwrap()
        };
        // COFF keeps a symbol name of up to 8 bytes in place, NUL-padded.
        let calls = |object: &[u8], name: &[u8]| {
            let mut needle = name.to_vec();
            needle.resize(8, 0);
            object.windows(8).any(|w| w == needle)
        };
        let mut baseline: Vec<String> = CFLAGS.iter().map(|s| s.to_string()).collect();
        baseline.push("-O1".into());
        let before = object("baseline.o", &baseline);
        if !calls(&before, b"fma") || !calls(&before, b"sqrt") {
            eprintln!("SKIPPED windows_flags_make_fma_and_sqrt_instructions: this clang inlines both already");
        } else {
            let after = object("windows.o", &cflags_for(Arch::X86_64Windows).unwrap());
            assert!(!calls(&after, b"fma"), "fma is still a call");
            assert!(!calls(&after, b"sqrt"), "sqrt still calls for errno");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn obj_cache_dir_and_sizes() {
        let out = Path::new("/o");
        assert_eq!(obj_dir_with(out, None), Path::new("/o/obj"));
        assert_eq!(obj_dir_with(out, Some("".into())), Path::new("/o/obj"));
        assert_eq!(obj_dir_with(out, Some("/shared/h2obj".into())), Path::new("/shared/h2obj"));
        assert_eq!(parse_size("1000").unwrap(), 1000);
        assert_eq!(parse_size("64k").unwrap(), 64 << 10);
        assert_eq!(parse_size("512MiB").unwrap(), 512 << 20);
        assert_eq!(parse_size(" 20G ").unwrap(), 20 << 30);
        assert!(parse_size("lots").is_err());
        assert!(parse_size("99999999999T").is_err());
    }

    #[test]
    fn pruning_drops_least_recently_used_and_spares_the_current_build() {
        use std::time::{Duration, SystemTime};
        let dir = std::env::temp_dir().join(format!("h2-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let t0 = SystemTime::now() - Duration::from_secs(10_000);
        let make = |name: &str, len: usize, age: u64| {
            let p = dir.join(name);
            std::fs::write(&p, vec![0u8; len]).unwrap();
            std::fs::File::options().append(true).open(&p).unwrap().set_modified(t0 + Duration::from_secs(age)).unwrap();
            p
        };
        let oldest = make("a.arm64.1.o", 100, 0);
        let old = make("b.arm64.2.o", 100, 10);
        let new = make("c.arm64.3.o", 100, 20);
        let pinned = make("d.arm64.4.o", 100, 1); // old, but in the current build
        let stale_tmp = make("e.arm64.5.o.1.tmp", 7, 0);
        let other = make("notes.txt", 5, 0);
        let keep: HashSet<PathBuf> = [pinned.clone()].into();
        let p = prune_obj_cache(&dir, 250, &keep).unwrap();
        assert_eq!(p, Prune { removed: 3, freed: 207, kept: 200 });
        assert!(pinned.exists() && new.exists() && other.exists());
        assert!(!old.exists() && !oldest.exists() && !stale_tmp.exists());
        // The current build is kept even when it alone exceeds the cap.
        let p = prune_obj_cache(&dir, 0, &keep).unwrap();
        assert_eq!(p, Prune { removed: 1, freed: 100, kept: 100 });
        assert!(pinned.exists() && !new.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pruning_evicts_oldest_first() {
        use std::time::{Duration, SystemTime};
        let dir = std::env::temp_dir().join(format!("h2-prune-lru-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let t0 = SystemTime::now() - Duration::from_secs(10_000);
        let make = |name: &str, len: usize, age: u64| {
            let p = dir.join(name);
            std::fs::write(&p, vec![0u8; len]).unwrap();
            std::fs::File::options().append(true).open(&p).unwrap().set_modified(t0 + Duration::from_secs(age)).unwrap();
            p
        };
        let newest = make("n.arm64.1.o", 200, 20);
        let middle = make("m.arm64.2.o", 100, 10);
        let oldest = make("o.arm64.3.o", 40, 0);
        // Keeping the 40-byte oldest would fit, but the middle one is more
        // recently used: both go.
        let p = prune_obj_cache(&dir, 250, &HashSet::new()).unwrap();
        assert_eq!(p, Prune { removed: 2, freed: 140, kept: 200 });
        assert!(newest.exists() && !middle.exists() && !oldest.exists());
        // Eviction stops as soon as the rest fit.
        let a = make("a.arm64.4.o", 50, 0);
        let b = make("b.arm64.5.o", 50, 5);
        let p = prune_obj_cache(&dir, 260, &HashSet::new()).unwrap();
        assert_eq!(p, Prune { removed: 1, freed: 50, kept: 250 });
        assert!(!a.exists() && b.exists() && newest.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A one-function source tree under a fresh `dir`.
    fn tiny_tree(dir: &Path) -> PathBuf {
        let _ = std::fs::remove_dir_all(dir);
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        for h in ["h2gen.h", "h2simd.h", "h2x87.h", "h2fastmem.h", "h2abi.h"] {
            std::fs::write(src.join(h), "/* support */\n").unwrap();
        }
        std::fs::write(src.join("one.c"), "__attribute__((visibility(\"default\"))) int h2_one(void) { return 1; }\n").unwrap();
        src
    }

    fn member_names(b: &Build) -> Vec<String> {
        b.objects.iter().map(|o| o.file_name().unwrap().to_string_lossy().into_owned()).collect()
    }

    /// Sources with the same preprocessor skeleton share one dependency
    /// scan, and still each get exactly their own includes: a header only
    /// one includes, a conditional include, a directive moved to another
    /// line or a header that saves and restores macros each key
    /// correctly, and the scanned source never becomes a dependency of the
    /// others.
    #[test]
    fn shared_dependency_scans_key_each_source_by_its_own_includes() {
        let Ok(cc) = compiler() else {
            eprintln!("SKIPPED shared_dependency_scans_key_each_source_by_its_own_includes: no clang");
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-shared-scan-{}", std::process::id()));
        let src = tiny_tree(&dir);
        let out = dir.join("out");
        let common = "#include \"h2abi.h\"\n";
        std::fs::write(src.join("only.h"), "#define ONLY 1\n").unwrap();
        std::fs::write(src.join("other.h"), "#define OTHER 1\n").unwrap();
        // Same skeleton: a, b and c. d includes only.h too; e includes it
        // under a condition another header controls; f has its include on a
        // later line, so `__LINE__` could tell them apart.
        for (name, text) in [
            ("a.c", format!("{common}int h2_a(void) {{ return 1; }}\n")),
            ("b.c", format!("{common}int h2_b(void) {{ return 2; }}\n")),
            ("c.c", format!("{common}int h2_c(void) {{ return 3; }}\n")),
            ("d.c", format!("{common}#include \"only.h\"\nint h2_d(void) {{ return ONLY; }}\n")),
            ("e.c", format!("{common}#ifdef WANT_OTHER\n#include \"other.h\"\n#endif\nint h2_e(void) {{ return 5; }}\n")),
            ("f.c", format!("\n{common}int h2_f(void) {{ return 6; }}\n")),
        ] {
            std::fs::write(src.join(name), text).unwrap();
        }
        let names = |b: &Build| -> Vec<String> { member_names(b) };
        let first = build_in(&src, &out, &dir.join("cache"), Arch::host(), 4, &cc).unwrap();
        assert_eq!((first.compiled, first.reused), (7, 0));
        // The scanned source (whichever of a, b, c) is no dependency of the
        // others: changing one recompiles only it.
        std::fs::write(src.join("b.c"), format!("{common}int h2_b(void) {{ return 22; }}\n")).unwrap();
        let b = build_in(&src, &out, &dir.join("cache"), Arch::host(), 4, &cc).unwrap();
        assert_eq!((b.compiled, b.reused), (1, 6));
        // A header only d includes recompiles only d.
        std::fs::write(src.join("only.h"), "#define ONLY 2\n").unwrap();
        let only = build_in(&src, &out, &dir.join("cache"), Arch::host(), 4, &cc).unwrap();
        assert_eq!((only.compiled, only.reused), (1, 6));
        let changed: Vec<_> = names(&only).into_iter().filter(|n| !names(&b).contains(n)).collect();
        assert!(changed[0].starts_with("d."), "{changed:?}");
        // Turning e's conditional include on (from a header everyone
        // includes) recompiles everyone; then other.h is e's alone.
        std::fs::write(src.join("h2abi.h"), "#define WANT_OTHER 1\n").unwrap();
        let on = build_in(&src, &out, &dir.join("cache"), Arch::host(), 4, &cc).unwrap();
        assert_eq!(on.reused, 0, "h2abi.h is in every key");
        std::fs::write(src.join("other.h"), "#define OTHER 2\n").unwrap();
        let other = build_in(&src, &out, &dir.join("cache"), Arch::host(), 4, &cc).unwrap();
        assert_eq!((other.compiled, other.reused), (1, 6));
        let changed: Vec<_> = names(&other).into_iter().filter(|n| !names(&on).contains(n)).collect();
        assert!(changed[0].starts_with("e."), "{changed:?}");
        // A header that saves and restores macros turns sharing off; every
        // source still keys (and builds) correctly.
        std::fs::write(src.join("h2abi.h"), "#pragma push_macro(\"X\")\n#pragma pop_macro(\"X\")\n").unwrap();
        let unshared = build_in(&src, &out, &dir.join("cache"), Arch::host(), 4, &cc).unwrap();
        assert_eq!(unshared.reused, 0);
        std::fs::write(src.join("only.h"), "#define ONLY 3\n").unwrap();
        let again = build_in(&src, &out, &dir.join("cache"), Arch::host(), 4, &cc).unwrap();
        assert_eq!((again.compiled, again.reused), (1, 6));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With tiers, a source holding no hot instruction compiles at the cold
    /// level and one holding one at the usual level; each tier keys its
    /// own objects, so moving a source between tiers recompiles exactly
    /// that source, and objects are reused across tier changes when their
    /// tier and text are the same.
    #[test]
    fn tiers_compile_cold_sources_at_their_level_and_key_them_apart() {
        let Ok(cc) = compiler() else {
            eprintln!("SKIPPED tiers_compile_cold_sources_at_their_level_and_key_them_apart: no clang");
            return;
        };
        if cflags().unwrap().iter().any(|f| f == "-O0") {
            eprintln!("SKIPPED tiers_compile_cold_sources_at_their_level_and_key_them_apart: H2_OPT=0");
            return;
        }
        let dir = std::env::temp_dir().join(format!("h2-tiers-{}", std::process::id()));
        let src = tiny_tree(&dir);
        for (name, pc) in [("a", "0x00001000"), ("b", "0x00002000")] {
            let text = format!(
                "int h2_{name}(int x) {{\n    /* {pc} */ {{ x += 1; }}\n    return x;\n}}\n\
                 #ifdef __OPTIMIZE__\nint h2_{name}_optimised;\n#else\nint h2_{name}_unoptimised;\n#endif\n"
            );
            std::fs::write(src.join(format!("{name}.c")), text).unwrap();
        }
        let out = dir.join("out");
        let cache = dir.join("cache");
        let build = |t: Option<&Tiers>| build_tiered(&src, &out, &cache, Arch::host(), 2, &cc, t).unwrap();
        let symbols = |b: &Build, stem: &str| -> String {
            let o = b.objects.iter().find(|o| o.file_name().unwrap().to_string_lossy().starts_with(&format!("{stem}."))).unwrap();
            String::from_utf8_lossy(&Command::new("nm").arg(o).output().unwrap().stdout).into_owned()
        };
        let first_a = Tiers::new("0", "guest 0x00001000 12").unwrap();
        let first = build(Some(&first_a));
        assert_eq!((first.compiled, first.reused, first.cold), (3, 0, 2));
        assert!(symbols(&first, "a").contains("h2_a_optimised"), "{}", symbols(&first, "a"));
        assert!(symbols(&first, "b").contains("h2_b_unoptimised"), "{}", symbols(&first, "b"));
        let warm = build(Some(&first_a));
        assert_eq!((warm.compiled, warm.reused), (0, 3));
        // b becomes hot and a cold: exactly those two recompile.
        let then_b = Tiers::new("0", "0x2000").unwrap();
        let moved = build(Some(&then_b));
        assert_eq!((moved.compiled, moved.reused, moved.cold), (2, 1, 2));
        assert!(symbols(&moved, "a").contains("h2_a_unoptimised"));
        assert!(symbols(&moved, "b").contains("h2_b_optimised"));
        // Without tiers everything is hot: a and b were built hot before,
        // one.c never was.
        let off = build(None);
        assert_eq!((off.compiled, off.reused, off.cold), (1, 2, 0));
        // Another cold level is another key (`-Og` is never the usual
        // level); a cold level equal to the usual one is the same object.
        let og = build(Some(&Tiers::new("g", "0x2000").unwrap()));
        assert_eq!((og.compiled, og.reused), (2, 1));
        let usual = cflags().unwrap().into_iter().find_map(|f| f.strip_prefix("-O").map(str::to_owned)).unwrap();
        let same = build(Some(&Tiers::new(&usual, "0x2000").unwrap()));
        assert_eq!((same.compiled, same.reused), (0, 3));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Changing one function recompiles only its source; changing any
    /// included support header invalidates both objects.
    #[test]
    fn one_changed_source_reuses_the_other_objects() {
        let Ok(cc) = compiler() else {
            eprintln!("SKIPPED one_changed_source_reuses_the_other_objects: no clang");
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-incremental-{}", std::process::id()));
        let src = tiny_tree(&dir);
        std::fs::write(src.join("two.c"), "int h2_two(void) { return 2; }\n").unwrap();
        let out = dir.join("out");
        // The first build must be cold even when a lane uses H2_OBJ_CACHE
        // and an earlier test process already cached these synthetic files.
        let cache = dir.join("cache");
        let build = || build_in(&src, &out, &cache, Arch::host(), 2, &cc).unwrap();
        let first = build();
        assert_eq!((first.compiled, first.reused), (2, 0));
        let warm = build();
        assert_eq!((warm.compiled, warm.reused), (0, 2));
        std::fs::write(src.join("one.c"), "int h2_one(void) { return 3; }\n").unwrap();
        let changed = build();
        assert_eq!((changed.compiled, changed.reused), (1, 1));
        assert_ne!(first.objects[0], changed.objects[0]);
        assert_eq!(first.objects[1], changed.objects[1]);
        assert_eq!(archive_members(&changed.library).unwrap(), member_names(&changed));
        for name in ["h2gen.h", "h2simd.h", "h2x87.h", "h2abi.h"] {
            std::fs::write(src.join(name), "/* changed support */\n").unwrap();
            let changed = build();
            assert_eq!((changed.compiled, changed.reused), (2, 0), "{name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An included constant must match a clean build, including when it
    /// comes from a system include directory or a newly shadowing header.
    #[test]
    fn transitive_headers_and_new_include_shadows_invalidate_objects() {
        use std::os::unix::fs::PermissionsExt;
        let Ok(cc) = compiler() else {
            eprintln!("SKIPPED transitive_headers_and_new_include_shadows_invalidate_objects: no clang");
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-transitive-{}", std::process::id()));
        let src = tiny_tree(&dir);
        std::fs::create_dir(src.join("nested")).unwrap();
        let external = dir.join("system headers # $ with spaces");
        std::fs::create_dir(&external).unwrap();
        let leaf = external.join("selected.h");
        std::fs::write(&leaf, "#define VALUE 1\n").unwrap();
        std::fs::write(src.join("nested/outer.h"), "#include <selected.h>\n").unwrap();
        std::fs::write(src.join("h2abi.h"), "#include \"nested/outer.h\"\n").unwrap();
        std::fs::write(src.join("one.c"), "#include \"h2abi.h\"\n__attribute__((visibility(\"default\"))) int h2_value(void) { return VALUE; }\n").unwrap();
        std::fs::write(src.join("two.c"), "int h2_two(void) { return 0; }\n").unwrap();
        let wrapper = dir.join("clang");
        std::fs::write(&wrapper, format!("#!/bin/sh\nexec '{}' -I '{}' -isystem '{}' \"$@\"\n", cc.display(), src.display(), external.display())).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = dir.join("out");
        let value = |build: &Build, name: &str| {
            let dylib = dir.join(name);
            host_dylib(build, &src, &[], &dylib).unwrap();
            // SAFETY: this synthetic library defines h2_value with exactly
            // this signature, and the symbol is used before unloading it.
            unsafe {
                let library = libloading::Library::new(dylib).unwrap();
                let function: libloading::Symbol<unsafe extern "C" fn() -> i32> = library.get(b"h2_value").unwrap();
                function()
            }
        };
        let first = build_with(&src, &out, Arch::host(), 2, &wrapper).unwrap();
        assert_eq!((first.compiled, first.reused), (2, 0));
        assert_eq!(value(&first, "first.dylib"), 1);
        let warm = build_with(&src, &out, Arch::host(), 2, &wrapper).unwrap();
        assert_eq!((warm.compiled, warm.reused), (0, 2));
        let mtime = std::fs::metadata(&leaf).unwrap().modified().unwrap();
        std::fs::write(&leaf, "#define VALUE 2\n").unwrap();
        std::fs::File::options().write(true).open(&leaf).unwrap().set_modified(mtime).unwrap();
        let changed = build_with(&src, &out, Arch::host(), 2, &wrapper).unwrap();
        assert_eq!((changed.compiled, changed.reused), (1, 1));
        assert_ne!(first.objects[0], changed.objects[0]);
        assert_eq!(first.objects[1], changed.objects[1]);
        assert_eq!(value(&changed, "changed.dylib"), 2);
        let clean = build_in(&src, &dir.join("clean-out"), &dir.join("clean-cache"), Arch::host(), 2, &wrapper).unwrap();
        assert_eq!(value(&clean, "clean.dylib"), value(&changed, "cached.dylib"));
        std::fs::write(src.join("selected.h"), "#define VALUE 3\n").unwrap();
        let shadowed = build_with(&src, &out, Arch::host(), 2, &wrapper).unwrap();
        assert_eq!((shadowed.compiled, shadowed.reused), (1, 1));
        assert_eq!(value(&shadowed, "shadowed.dylib"), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dependency_keys_reuse_identical_headers_across_source_directories() {
        let Ok(cc) = compiler() else {
            eprintln!("SKIPPED dependency_keys_reuse_identical_headers_across_source_directories: no clang");
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-dependency-shared-{}", std::process::id()));
        let a = tiny_tree(&dir.join("a"));
        let b = tiny_tree(&dir.join("b"));
        for src in [&a, &b] {
            std::fs::create_dir(src.join("nested")).unwrap();
            std::fs::write(src.join("h2abi.h"), "#include \"nested/config.h\"\n").unwrap();
            std::fs::write(src.join("nested/config.h"), "#define VALUE 7\n").unwrap();
            std::fs::write(src.join("one.c"), "#include \"h2abi.h\"\nint h2_one(void) { return VALUE; }\n").unwrap();
        }
        let cache = dir.join("shared-cache");
        let first = build_in(&a, &dir.join("out-a"), &cache, Arch::host(), 1, &cc).unwrap();
        assert_eq!((first.compiled, first.reused), (1, 0));
        let second = build_in(&b, &dir.join("out-b"), &cache, Arch::host(), 1, &cc).unwrap();
        assert_eq!((second.compiled, second.reused), (0, 1));
        assert_eq!(first.cache_objects, second.cache_objects);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The compiler's working directory must not reach the object key: the
    /// same generated C built from another checkout or shell directory
    /// (clang embeds its cwd as the compilation directory) reuses every
    /// object.
    #[test]
    fn the_compilers_working_directory_is_not_part_of_the_object_key() {
        use std::os::unix::fs::PermissionsExt;
        let Ok(cc) = compiler() else {
            eprintln!("SKIPPED the_compilers_working_directory_is_not_part_of_the_object_key: no clang");
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-cwd-key-{}", std::process::id()));
        let a = tiny_tree(&dir.join("a"));
        let b = tiny_tree(&dir.join("b"));
        for src in [&a, &b] {
            std::fs::write(src.join("one.c"), "int h2_one(void) { return 1; }\n").unwrap();
        }
        let (cwd_a, cwd_b) = (dir.join("cwd-a"), dir.join("cwd-b"));
        std::fs::create_dir_all(&cwd_a).unwrap();
        std::fs::create_dir_all(&cwd_b).unwrap();
        // One wrapper (so one compiler identity) that runs clang in the
        // directory named by a file the test rewrites between builds.
        let wrapper = dir.join("clang");
        std::fs::write(&wrapper, format!("#!/bin/sh\ncd \"$(cat '{}')\"\nexec '{}' \"$@\"\n", dir.join("cwd").display(), cc.display())).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cache = dir.join("cache");
        std::fs::write(dir.join("cwd"), cwd_a.to_str().unwrap()).unwrap();
        let first = build_in(&a, &dir.join("out-a"), &cache, Arch::host(), 1, &wrapper).unwrap();
        assert_eq!((first.compiled, first.reused), (1, 0));
        std::fs::write(dir.join("cwd"), cwd_b.to_str().unwrap()).unwrap();
        let second = build_in(&b, &dir.join("out-b"), &cache, Arch::host(), 1, &wrapper).unwrap();
        assert_eq!((second.compiled, second.reused), (0, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A shared cache hands one output's objects to another, so an object
    /// must not name the directory it was compiled from: its line tables
    /// name sources relative to the source directory. Otherwise the second
    /// output's profiler reads the first output's sources, or none once
    /// they are deleted.
    #[test]
    fn objects_do_not_name_the_source_directory_they_were_built_from() {
        let Ok(cc) = compiler() else {
            eprintln!("SKIPPED objects_do_not_name_the_source_directory_they_were_built_from: no clang");
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-debug-paths-{}", std::process::id()));
        let src = tiny_tree(&dir.join("first-output"));
        let b = build_in(&src, &dir.join("out"), &dir.join("cache"), Arch::host(), 1, &cc).unwrap();
        let object = std::fs::read(&b.objects[0]).unwrap();
        let has = |needle: &[u8]| object.windows(needle.len()).any(|w| w == needle);
        if !has(b"one.c") {
            eprintln!("SKIPPED objects_do_not_name_the_source_directory_they_were_built_from: no debug info (H2_DEBUG=none?)");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        for d in [src.clone(), std::fs::canonicalize(&src).unwrap()] {
            let d = d.to_string_lossy().into_owned();
            assert!(!has(d.as_bytes()), "{} names {d}", b.objects[0].display());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn driver_configuration_contents_and_expanded_response_options_are_keyed() {
        use std::os::unix::fs::PermissionsExt;
        let Ok(cc) = compiler() else {
            eprintln!("SKIPPED driver_configuration_contents_and_expanded_response_options_are_keyed: no clang");
            return;
        };
        for mode in ["ccc", "explicit", "default"] {
            let dir = std::env::temp_dir().join(format!("h2-driver-config-{mode}-{}", std::process::id()));
            let src = tiny_tree(&dir.join("source \"quoted\""));
            std::fs::create_dir_all(dir.join("configs")).unwrap();
            let config = if mode == "default" {
                dir.join("configs").join(format!("{}-clang.cfg", Arch::host().triple()))
            } else {
                dir.join("configs/flags.cfg")
            };
            std::fs::write(&config, "-DVALUE=1\n").unwrap();
            std::fs::write(src.join("one.c"), "__attribute__((visibility(\"default\"))) int h2_value(void) { return VALUE; }\n").unwrap();
            let options = match mode {
                "ccc" => format!("export CCC_OVERRIDE_OPTIONS='^--config={}'\nexec '{}' \"$@\"\n", config.display(), cc.display()),
                "explicit" => format!("unset CCC_OVERRIDE_OPTIONS\nexec '{}' --no-default-config --config '{}' \"$@\"\n", cc.display(), config.display()),
                _ => format!("unset CCC_OVERRIDE_OPTIONS\nexec '{}' --config-system-dir='{}' --config-user-dir='{}' \"$@\"\n", cc.display(), dir.join("configs").display(), dir.join("no-user-config").display()),
            };
            let wrapper = dir.join("clang");
            std::fs::write(&wrapper, format!("#!/bin/sh\n{options}")).unwrap();
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
            let out = dir.join("out");
            let build = || build_with(&src, &out, Arch::host(), 1, &wrapper).unwrap();
            let value = |build: &Build, name: &str| {
                let dylib = dir.join(name);
                host_dylib(build, &src, &[], &dylib).unwrap();
                // SAFETY: the synthetic function and its signature are known,
                // and the library remains loaded until after the call.
                unsafe {
                    let library = libloading::Library::new(dylib).unwrap();
                    let function: libloading::Symbol<unsafe extern "C" fn() -> i32> = library.get(b"h2_value").unwrap();
                    function()
                }
            };
            let first = build();
            assert_eq!((first.compiled, first.reused), (1, 0), "{mode}");
            assert_eq!(value(&first, "first.dylib"), 1);
            let warm = build();
            assert_eq!((warm.compiled, warm.reused), (0, 1), "{mode}");
            let identity = toolchain_identity(&wrapper);
            std::fs::write(&config, "-DVALUE=2\n").unwrap();
            assert_eq!(identity, toolchain_identity(&wrapper), "old probes miss {mode} config edits");
            let changed = build();
            assert_eq!((changed.compiled, changed.reused), (1, 0), "{mode}");
            assert_ne!(first.objects, changed.objects);
            assert_eq!(value(&changed, "changed.dylib"), 2);
            let clean = build_in(&src, &dir.join("clean-out"), &dir.join("clean-cache"), Arch::host(), 1, &wrapper).unwrap();
            assert_eq!(value(&clean, "clean.dylib"), 2);
            // Even a configuration comment changes the configuration content
            // key, while the expanded cc1 arguments remain the same.
            std::fs::write(&config, "-DVALUE=2\n# new config comment\n").unwrap();
            assert_eq!(build().compiled, 1, "{mode} config contents");
            if mode == "ccc" {
                let response = dir.join("response.rsp");
                std::fs::write(&response, "-DVALUE=3\n").unwrap();
                std::fs::write(&config, format!("@{}\n", response.display())).unwrap();
                assert_eq!(value(&build(), "response-three.dylib"), 3);
                std::fs::write(&response, "-DVALUE=4\n").unwrap();
                let changed_response = build();
                assert_eq!(changed_response.compiled, 1);
                assert_eq!(value(&changed_response, "response-four.dylib"), 4);
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn mutable_abi_bridge_and_libraries_detach_links_but_cached_objects_keep_them() {
        use std::os::unix::fs::MetadataExt;
        let Ok(cc) = compiler() else {
            eprintln!("SKIPPED mutable_abi_bridge_and_libraries_detach_links_but_cached_objects_keep_them: no clang");
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-hard-linked-native-{}", std::process::id()));
        let src = tiny_tree(&dir);
        let alias = |name: &str, destination: &Path| {
            let sentinel = dir.join(format!("sentinel-{name}"));
            std::fs::write(&sentinel, "sentinel\n").unwrap();
            let _ = std::fs::remove_file(destination);
            std::fs::hard_link(&sentinel, destination).unwrap();
            sentinel
        };
        let intact = |sentinel: &Path, destination: &Path| {
            assert_eq!(std::fs::read(sentinel).unwrap(), b"sentinel\n");
            assert_ne!(std::fs::metadata(sentinel).unwrap().ino(), std::fs::metadata(destination).unwrap().ino());
        };
        let abi = alias("abi", &src.join("h2abi.h"));
        let input = dir.join("input-abi.h");
        std::fs::write(&input, "/* installed ABI */\n").unwrap();
        install_header(&src, &input).unwrap();
        intact(&abi, &src.join("h2abi.h"));
        let out = dir.join("out");
        std::fs::create_dir(&out).unwrap();
        let archive = out.join(format!("libh2gen-{}.a", Arch::host().name()));
        let archive_sentinel = alias("archive", &archive);
        let first = build_with(&src, &out, Arch::host(), 1, &cc).unwrap();
        intact(&archive_sentinel, &archive);
        let object = std::fs::metadata(&first.objects[0]).unwrap();
        let cached = std::fs::metadata(&first.cache_objects[0]).unwrap();
        assert_eq!((object.dev(), object.ino()), (cached.dev(), cached.ino()));
        assert!(object.nlink() >= 2, "object pins must remain hard links");
        let source_sentinel = alias("bridge-source", &src.join("bridge.c"));
        let object_sentinel = alias("bridge-object", &src.join("bridge.o"));
        let dylib = out.join("host.dylib");
        let dylib_sentinel = alias("dylib", &dylib);
        host_dylib(&first, &src, &[("bridge.c", "__attribute__((visibility(\"default\"))) int bridge_marker(void) { return 23; }\n")], &dylib).unwrap();
        assert_eq!(std::fs::read(&source_sentinel).unwrap(), b"sentinel\n");
        assert!(!src.join("bridge.c").exists());
        intact(&object_sentinel, &src.join("bridge.o"));
        intact(&dylib_sentinel, &dylib);
        if Arch::host().is_macos() {
            let identity = Command::new("otool").arg("-D").arg(&dylib).output().unwrap();
            assert!(identity.status.success());
            assert!(String::from_utf8_lossy(&identity.stdout).lines().any(|line| line == dylib.to_string_lossy()), "dylib install name must remain the final output path");
        }
        // SAFETY: both signatures and the linked synthetic definitions are
        // known; all calls finish before the library is unloaded.
        unsafe {
            let library = libloading::Library::new(&dylib).unwrap();
            let function: libloading::Symbol<unsafe extern "C" fn() -> i32> = library.get(b"h2_one").unwrap();
            assert_eq!(function(), 1);
            let bridge: libloading::Symbol<unsafe extern "C" fn() -> i32> = library.get(b"bridge_marker").unwrap();
            assert_eq!(bridge(), 23);
        }
        let warm = build_with(&src, &out, Arch::host(), 1, &cc).unwrap();
        assert_eq!((warm.compiled, warm.reused), (0, 1));
        assert_eq!(warm.objects, first.objects);
        assert_eq!(std::fs::metadata(&warm.cache_objects[0]).unwrap().ino(), object.ino());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Another checkout pruning a shared cache to nothing between this
    /// build and its archive/link: the pins keep every input, the archive
    /// is complete, and the next build restores the cache from the pins.
    #[test]
    fn a_concurrent_prune_never_loses_an_input() {
        let Ok(cc) = compiler() else {
            eprintln!("SKIPPED a_concurrent_prune_never_loses_an_input: no clang");
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-concurrent-prune-{}", std::process::id()));
        let src = tiny_tree(&dir);
        let (cache, out) = (dir.join("cache"), dir.join("a"));
        let a = build_in(&src, &out, &cache, Arch::host(), 1, &cc).unwrap();
        assert_eq!((a.compiled, a.reused), (1, 0));
        assert_ne!(a.objects, a.cache_objects, "the build reads its pins, not the cache");
        // The other checkout's prune protects only its own build.
        let p = prune_obj_cache(&cache, 0, &HashSet::new()).unwrap();
        assert_eq!(p.removed, 1);
        assert!(!a.cache_objects[0].exists());
        // This build's archive and objects are intact: archive them again
        // (as a build that lost the race would) and link them.
        assert!(a.objects[0].exists());
        assert_eq!(archive_members(&a.library).unwrap(), member_names(&a));
        archive(&a.library, &a.objects).unwrap();
        host_dylib(&a, &src, &[], &dir.join("a.dylib")).unwrap();
        // Rebuilding reuses the pin and puts the cache entry back.
        let again = build_in(&src, &out, &cache, Arch::host(), 1, &cc).unwrap();
        assert_eq!((again.compiled, again.reused), (0, 1));
        assert!(again.cache_objects[0].exists());
        // A build with no pin of its own after the prune recompiles.
        prune_obj_cache(&cache, 0, &HashSet::new()).unwrap();
        let b = build_in(&src, &dir.join("b"), &cache, Arch::host(), 1, &cc).unwrap();
        assert_eq!((b.compiled, b.reused), (1, 0));
        assert_eq!(archive_members(&b.library).unwrap(), member_names(&b));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An archive missing an object is never produced, and an existing
    /// incomplete one is never trusted.
    #[test]
    fn an_incomplete_archive_is_never_accepted() {
        let Ok(cc) = compiler() else {
            eprintln!("SKIPPED an_incomplete_archive_is_never_accepted: no clang");
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-incomplete-archive-{}", std::process::id()));
        let src = tiny_tree(&dir);
        let out = dir.join("out");
        let a = build_in(&src, &out, &dir.join("cache"), Arch::host(), 1, &cc).unwrap();
        let before = std::fs::read(&a.library).unwrap();
        // A missing input fails without touching the library.
        let missing = vec![a.objects[0].clone(), dir.join("gone.o")];
        assert!(archive(&a.library, &missing).is_err());
        assert_eq!(std::fs::read(&a.library).unwrap(), before);
        // libtool given a missing input warns and exits 0; that is a failure.
        if Arch::host().is_macos() {
            let lt = Command::new("libtool")
                .args(["-static", "-no_warning_for_no_symbols", "-o"])
                .arg(dir.join("probe.a"))
                .arg(&a.objects[0])
                .arg(dir.join("gone.o"))
                .output()
                .unwrap();
            if lt.status.success() {
                assert!(!lt.stderr.is_empty(), "libtool reports the missing input");
            }
        }
        // An empty archive left in place (and the old stamp claiming it
        // is complete) is rebuilt.
        std::fs::write(&a.library, "!<arch>\n").unwrap();
        std::fs::write(a.library.with_extension("a.members"), member_names(&a).join("\n") + "\n").unwrap();
        let again = build_in(&src, &out, &dir.join("cache"), Arch::host(), 1, &cc).unwrap();
        assert_eq!(archive_members(&again.library).unwrap(), member_names(&again));
        assert!(!again.library.with_extension("a.members").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two output directories sharing one object directory: the second build
    /// reuses every object and refreshes their modification times.
    #[test]
    fn a_shared_obj_dir_serves_two_outputs() {
        let Ok(cc) = compiler() else {
            eprintln!("SKIPPED a_shared_obj_dir_serves_two_outputs: no clang");
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-shared-obj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        for h in ["h2gen.h", "h2simd.h", "h2x87.h", "h2fastmem.h", "h2abi.h"] {
            std::fs::write(src.join(h), "/* support */\n").unwrap();
        }
        std::fs::write(src.join("one.c"), "int h2_one(void) { return 1; }\n").unwrap();
        let cache = dir.join("cache");
        let a = build_in(&src, &dir.join("a"), &cache, Arch::host(), 1, &cc).unwrap();
        assert_eq!((a.compiled, a.reused), (1, 0));
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options().append(true).open(&a.cache_objects[0]).unwrap().set_modified(old).unwrap();
        let b = build_in(&src, &dir.join("b"), &cache, Arch::host(), 1, &cc).unwrap();
        assert_eq!((b.compiled, b.reused), (0, 1));
        assert_eq!(a.cache_objects, b.cache_objects);
        assert!(b.library.exists() && b.library != a.library);
        let mtime = std::fs::metadata(&b.cache_objects[0]).unwrap().modified().unwrap();
        assert!(mtime > old, "reuse refreshes the LRU time");
        assert!(!dir.join("a/obj").exists(), "nothing under out_dir/obj");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolution_follows_path_order_and_skips_non_executables() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("h2-resolve-{}", std::process::id()));
        let (a, b) = (dir.join("a"), dir.join("b"));
        for d in [&a, &b] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(a.join("cc"), "not executable").unwrap();
        std::fs::write(b.join("cc"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(b.join("cc"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([&a, &b]).unwrap();
        assert_eq!(resolve_on_path("cc", Some(path)), Some(b.join("cc")));
        assert_eq!(resolve_on_path("cc", Some(std::env::join_paths([&a]).unwrap())), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two `clang` wrappers that answer every probe like the real compiler
    /// but compile differently must not share objects.
    #[test]
    fn a_different_compiler_on_path_never_reuses_an_object() {
        use std::os::unix::fs::PermissionsExt;
        let Ok(real) = compiler() else {
            eprintln!("SKIPPED a_different_compiler_on_path_never_reuses_an_object: no clang");
            return;
        };
        let dir = std::env::temp_dir().join(format!("h2-wrappers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        for h in ["h2gen.h", "h2simd.h", "h2x87.h", "h2fastmem.h", "h2abi.h"] {
            std::fs::write(src.join(h), "/* support */\n").unwrap();
        }
        std::fs::write(src.join("mark.c"), "int h2_mark(void) { return TOOLCHAIN_MARK; }\n").unwrap();
        let wrapper = |n: u32| {
            let d = dir.join(format!("compiler-{n}"));
            std::fs::create_dir_all(&d).unwrap();
            let p = d.join("clang");
            std::fs::write(&p, format!("#!/bin/sh\nexec '{}' -DTOOLCHAIN_MARK={n} \"$@\"\n", real.display())).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            resolve_on_path("clang", Some(std::env::join_paths([&d]).unwrap())).unwrap()
        };
        let (one, two) = (wrapper(1), wrapper(2));
        let out = dir.join("out");
        let first = build_with(&src, &out, Arch::host(), 1, &one).unwrap();
        assert_eq!((first.compiled, first.reused), (1, 0));
        let again = build_with(&src, &out, Arch::host(), 1, &one).unwrap();
        assert_eq!((again.compiled, again.reused), (0, 1), "same compiler reuses");
        let second = build_with(&src, &out, Arch::host(), 1, &two).unwrap();
        assert_eq!((second.compiled, second.reused), (1, 0), "another compiler recompiles");
        assert_ne!(first.objects, second.objects);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
