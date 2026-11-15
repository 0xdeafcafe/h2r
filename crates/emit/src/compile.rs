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
];

/// The optimisation and debug-info flags for `opt` (`H2_OPT`: `0` `1` `g`
/// `s` `2`, default `1`) and `debug` (`H2_DEBUG`: `lines`, `full`, `none`,
/// default `lines`). Both are part of every object's cache key. `-O1` is
/// the release level (LIFTER.md §1); `lines` keeps guest-address line
/// tables for backtraces and stepping at about two thirds of the compile
/// time of `full` (`-g`), whose variable-level info is the bulk of the
/// object size.
pub fn profile_flags(opt: Option<&str>, debug: Option<&str>) -> Result<Vec<String>, Error> {
    let opt = opt.unwrap_or("1");
    if !["0", "1", "g", "s", "2"].contains(&opt) {
        return Err(err(format!("H2_OPT={opt}: expected 0, 1, g, s or 2")));
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
/// from the environment.
pub fn cflags() -> Result<Vec<String>, Error> {
    let env = |k| std::env::var(k).ok();
    let mut v: Vec<String> = CFLAGS.iter().map(|s| s.to_string()).collect();
    v.extend(profile_flags(env("H2_OPT").as_deref(), env("H2_DEBUG").as_deref())?);
    Ok(v)
}

/// A job count that uses every core.
pub fn default_jobs() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    Arm64,
    X86_64,
}

impl Arch {
    pub const ALL: [Arch; 2] = [Arch::Arm64, Arch::X86_64];

    pub fn name(self) -> &'static str {
        match self {
            Arch::Arm64 => "arm64",
            Arch::X86_64 => "x86_64",
        }
    }

    pub fn triple(self) -> &'static str {
        match self {
            Arch::Arm64 => "arm64-apple-macos11",
            Arch::X86_64 => "x86_64-apple-macos11",
        }
    }

    /// The architecture this process runs on.
    pub fn host() -> Arch {
        if cfg!(target_arch = "x86_64") { Arch::X86_64 } else { Arch::Arm64 }
    }
}

/// Architectures selected by the CLI. Iteration builds the host by default;
/// `all` explicitly requests both release targets.
pub fn architectures(selection: Option<&str>) -> Result<Vec<Arch>, Error> {
    match selection.unwrap_or("host") {
        "host" => Ok(vec![Arch::host()]),
        "all" => Ok(Arch::ALL.to_vec()),
        "arm64" => Ok(vec![Arch::Arm64]),
        "x86_64" => Ok(vec![Arch::X86_64]),
        value => Err(err(format!("--arch {value}: expected host, all, arm64 or x86_64"))),
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
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(&path?)
        .map(|d| d.join(name))
        .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
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

/// The part of every object's cache key shared by all sources of one
/// build: support headers, flags, target, toolchain.
pub fn common_key(h2gen: &[u8], h2abi: &[u8], arch: Arch, flags: &[String], toolchain: &str) -> u64 {
    let mut common = h2gen.to_vec();
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
        std::fs::write(&dst, text).map_err(err)?;
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
            let tmp = pin.with_extension(format!("o.{}.tmp", std::process::id()));
            std::fs::copy(obj, &tmp).and_then(|_| std::fs::rename(&tmp, pin)).inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })
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
    let o = Command::new("ar").arg("-t").arg(lib).output().map_err(|e| err(format!("running ar: {e}")))?;
    if !o.status.success() {
        return Err(Error(format!("ar -t {}: {}", lib.display(), String::from_utf8_lossy(&o.stderr))));
    }
    Ok(String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with("__.SYMDEF"))
        .map(str::to_owned)
        .collect())
}

/// Whether `lib` holds exactly the objects `names` (sorted).
fn archive_matches(lib: &Path, names: &[String]) -> bool {
    lib.exists()
        && archive_members(lib).is_ok_and(|mut m| {
            m.sort();
            m == names
        })
}

/// Archive `objects` into the static library `library`: written to a
/// temporary, checked to hold exactly `objects`, then renamed into place.
/// Apple's libtool warns about a missing input and still exits 0, so any
/// diagnostic from it is a failure too. On failure `library` is untouched.
pub fn archive(library: &Path, objects: &[PathBuf]) -> Result<(), Error> {
    let mut names: Vec<String> = objects.iter().map(|o| o.file_name().unwrap().to_string_lossy().into_owned()).collect();
    names.sort();
    if let Some(o) = objects.iter().find(|o| !o.is_file()) {
        return Err(Error(format!("archiving {}: {} is missing", library.display(), o.display())));
    }
    let tmp = library.with_extension(format!("a.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let out = Command::new("libtool")
        .args(["-static", "-no_warning_for_no_symbols", "-o"])
        .arg(&tmp)
        .args(objects)
        .output()
        .map_err(|e| err(format!("running libtool: {e}")))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let fail = if !out.status.success() || !stderr.trim().is_empty() {
        Some(format!("libtool: {stderr}"))
    } else if !archive_matches(&tmp, &names) {
        Some(format!("libtool: {} does not hold exactly the {} objects given", tmp.display(), names.len()))
    } else {
        std::fs::rename(&tmp, library).err().map(|e| format!("{}: {e}", library.display()))
    };
    match fail {
        Some(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(Error(e))
        }
        None => Ok(()),
    }
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
    let cache_start = Instant::now();
    let pin_dir = out_dir.join("pin").join(arch.name());
    for d in [obj_dir, out_dir, &pin_dir] {
        std::fs::create_dir_all(d).map_err(err)?;
    }
    let read = |name: &str| std::fs::read(src_dir.join(name)).map_err(|e| err(format!("{name}: {e}")));
    // h2gen.h includes h2simd.h and h2x87.h: all three are the support
    // headers' identity.
    let support = [read("h2gen.h")?, read("h2simd.h")?, read("h2x87.h")?].concat();
    let flags = cflags()?;
    let common = common_key(&support, &read("h2abi.h")?, arch, &flags, &toolchain_identity(cc));

    let mut sources: Vec<PathBuf> = std::fs::read_dir(src_dir)
        .map_err(err)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "c"))
        .collect();
    sources.sort();

    // (source, object, pin, needs compiling). Hashing ~400 MB of sources is
    // serial work worth spreading over the cores on a no-op rebuild.
    let keyed: Mutex<Vec<(usize, Keyed)>> = Mutex::new(Vec::new());
    let cursor = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..jobs.clamp(1, 16) {
            s.spawn(|| {
                loop {
                    let i = cursor.fetch_add(1, Ordering::Relaxed);
                    let Some(src) = sources.get(i) else { break };
                    let r = std::fs::read(src).map_err(err).and_then(|text| {
                        let key = fnv64(&[&common.to_le_bytes()[..], &text].concat());
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
                    keyed.lock().unwrap().push((i, r));
                }
            });
        }
    });
    let mut keyed = keyed.into_inner().unwrap();
    keyed.sort_by_key(|k| k.0);
    let mut jobs_list = Vec::new();
    for (i, r) in keyed {
        let (obj, pin, fresh) = r?;
        jobs_list.push((sources[i].clone(), obj, pin, fresh));
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
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&j) = order.get(i) else { break };
                    let (src, obj, pin, _) = todo[j];
                    // Per process: another checkout sharing the cache may be
                    // compiling the same object.
                    let tmp = obj.with_extension(format!("o.{}.tmp", std::process::id()));
                    let out = Command::new(cc)
                        .args(&flags)
                        .args(["-target", arch.triple(), "-c"])
                        .arg(src)
                        .arg("-o")
                        .arg(&tmp)
                        .output();
                    match out {
                        Ok(o) if o.status.success() => {
                            // Pin before publishing, so a pruner never sees
                            // the object unpinned.
                            if let Err(e) = pin_object(&tmp, pin).and_then(|()| std::fs::rename(&tmp, obj)) {
                                let _ = std::fs::remove_file(&tmp);
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
    if !archive_matches(&library, &names) {
        archive(&library, &objects)?;
    }
    let library_bytes = std::fs::metadata(&library).map(|m| m.len()).unwrap_or(0);
    Ok(Build {
        arch,
        compiled: todo.len(),
        reused: jobs_list.len() - todo.len(),
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

/// Link a build's objects plus extra C sources (a test bridge providing the
/// ABI symbols) into a loadable dylib for the host.
pub fn host_dylib(build: &Build, src_dir: &Path, extra: &[(&str, &str)], out: &Path) -> Result<(), Error> {
    let cc = compiler()?;
    let flags = cflags()?;
    let mut args: Vec<PathBuf> = build.objects.clone();
    for (name, text) in extra {
        let src = src_dir.join(name);
        std::fs::write(&src, text).map_err(err)?;
        let obj = src.with_extension("o");
        let o = Command::new(&cc)
            .args(&flags)
            .args(["-target", build.arch.triple(), "-c"])
            .arg(&src)
            .arg("-o")
            .arg(&obj)
            .output()
            .map_err(err)?;
        // The bridge is not a generated shard: keep it out of later builds.
        let _ = std::fs::remove_file(&src);
        if !o.status.success() {
            return Err(Error(format!("{name}: {}", String::from_utf8_lossy(&o.stderr))));
        }
        args.push(obj);
    }
    let o = Command::new(&cc)
        .args(["-target", build.arch.triple(), "-dynamiclib", "-o"])
        .arg(out)
        .args(&args)
        .output()
        .map_err(err)?;
    if !o.status.success() {
        return Err(Error(format!("linking dylib: {}", String::from_utf8_lossy(&o.stderr))));
    }
    Ok(())
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
    fn profile_flags_are_validated_and_keyed() {
        let f = |o, d| profile_flags(o, d).unwrap();
        assert_eq!(f(None, None), ["-O1", "-gline-tables-only"]);
        assert_eq!(f(Some("0"), Some("full")), ["-O0", "-g"]);
        assert_eq!(f(Some("2"), Some("none")), ["-O2"]);
        assert!(profile_flags(Some("3"), None).is_err());
        assert!(profile_flags(None, Some("dwarf")).is_err());
        let key = |o, d| common_key(b"a", b"b", Arch::Arm64, &f(o, d), "t");
        assert_ne!(key(None, None), key(Some("0"), None), "opt level");
        assert_ne!(key(None, None), key(None, Some("full")), "debug level");
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
        for h in ["h2gen.h", "h2simd.h", "h2x87.h", "h2abi.h"] {
            std::fs::write(src.join(h), "/* support */\n").unwrap();
        }
        std::fs::write(src.join("one.c"), "int h2_one(void) { return 1; }\n").unwrap();
        src
    }

    fn member_names(b: &Build) -> Vec<String> {
        b.objects.iter().map(|o| o.file_name().unwrap().to_string_lossy().into_owned()).collect()
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
        let first = build_with(&src, &out, Arch::host(), 2, &cc).unwrap();
        assert_eq!((first.compiled, first.reused), (2, 0));
        let warm = build_with(&src, &out, Arch::host(), 2, &cc).unwrap();
        assert_eq!((warm.compiled, warm.reused), (0, 2));
        std::fs::write(src.join("one.c"), "int h2_one(void) { return 3; }\n").unwrap();
        let changed = build_with(&src, &out, Arch::host(), 2, &cc).unwrap();
        assert_eq!((changed.compiled, changed.reused), (1, 1));
        assert_ne!(first.objects[0], changed.objects[0]);
        assert_eq!(first.objects[1], changed.objects[1]);
        assert_eq!(archive_members(&changed.library).unwrap(), member_names(&changed));
        for name in ["h2gen.h", "h2simd.h", "h2x87.h", "h2abi.h"] {
            std::fs::write(src.join(name), "/* changed support */\n").unwrap();
            let changed = build_with(&src, &out, Arch::host(), 2, &cc).unwrap();
            assert_eq!((changed.compiled, changed.reused), (2, 0), "{name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
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
        for h in ["h2gen.h", "h2simd.h", "h2x87.h", "h2abi.h"] {
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
        for h in ["h2gen.h", "h2simd.h", "h2x87.h", "h2abi.h"] {
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
