//! Compiler-level switches for the generated code that are off by default:
//! profile-guided optimisation (`H2_PGO`) and the target CPU (`H2_CPU`).
//! Each adds flags to [`super::cflags`], so each is in every object's
//! cache key and in the boot stamp: `h2 run` rebuilds when one changes, and
//! needs the same values the build had (docs/lifter/BUILD-CACHE.md).
//!
//! `H2_PGO=gen` compiles the objects with clang's instrumentation and
//! [`super::host_dylib`] links clang's profile runtime (compiler-rt's
//! `libclang_rt.profile_*.a`, found by the same clang driver) and [`SHIM`]
//! into the module. The raw profile goes to `H2_PGO_OUT` (default
//! `$TMPDIR/h2.profraw`) at process exit, or when the host calls
//! [`write_profile`]. `H2_PGO=use:<file.profdata>` compiles with that
//! profile; its contents are hashed into the flags, so a new profile at
//! the same path recompiles.

use std::path::PathBuf;

use super::{Arch, Error, err};
use crate::fnv64;

/// `H2_PGO`: off, instrumenting, or using a merged profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pgo {
    Off,
    /// `gen`: `-fprofile-instr-generate`.
    Gen,
    /// `use:<path>`: `-fprofile-instr-use=<path>`, the path made absolute.
    Use(PathBuf),
}

/// The instrumentation flag; also what [`super::host_dylib`] passes the
/// link so the driver adds the profile runtime.
pub const GEN_FLAG: &str = "-fprofile-instr-generate";

impl Pgo {
    pub fn from_env() -> Result<Pgo, Error> {
        Pgo::parse(std::env::var("H2_PGO").ok().as_deref())
    }

    /// `None`, empty or `0` is off; `gen`; `use:<path>`.
    pub fn parse(value: Option<&str>) -> Result<Pgo, Error> {
        match value.unwrap_or("") {
            "" | "0" => Ok(Pgo::Off),
            "gen" => Ok(Pgo::Gen),
            v => match v.strip_prefix("use:") {
                Some("") | None => Err(err(format!(
                    "H2_PGO={v}: expected gen or use:<file.profdata>"
                ))),
                Some(p) => {
                    let p = std::path::absolute(p).map_err(|e| err(format!("H2_PGO={v}: {e}")))?;
                    Ok(Pgo::Use(p))
                }
            },
        }
    }

    /// The compiler flags. For [`Pgo::Use`] this reads the profile: it must
    /// exist and not be empty, and its content hash is part of the flags
    /// (`-DH2_PGO_PROFILE=<hash>`, which no source reads) so it reaches
    /// every object's key and the boot stamp.
    pub fn flags(&self) -> Result<Vec<String>, Error> {
        match self {
            Pgo::Off => Ok(vec![]),
            Pgo::Gen => Ok(vec![GEN_FLAG.to_owned()]),
            Pgo::Use(path) => Ok(vec![
                format!("-fprofile-instr-use={}", path.display()),
                "-Wno-profile-instr-out-of-date".to_owned(),
                "-Wno-profile-instr-unprofiled".to_owned(),
                format!("-DH2_PGO_PROFILE=0x{:016x}", profile_hash(path)?),
            ]),
        }
    }
}

/// The content hash of the profile at `path`. A build asks for the flags
/// several times (objects, link, stamp) and a game profile is large, so
/// the hash is remembered for the file's size and modification time.
fn profile_hash(path: &std::path::Path) -> Result<u64, Error> {
    type Seen = (PathBuf, u64, Option<std::time::SystemTime>, u64);
    static SEEN: std::sync::Mutex<Option<Seen>> = std::sync::Mutex::new(None);
    let fail = |e: &dyn std::fmt::Display| err(format!("H2_PGO=use:{}: {e}", path.display()));
    let meta = std::fs::metadata(path).map_err(|e| fail(&e))?;
    let (len, modified) = (meta.len(), meta.modified().ok());
    let mut seen = SEEN.lock().unwrap();
    if let Some((p, l, m, hash)) = seen.as_ref()
        && (p.as_path(), *l, *m) == (path, len, modified)
    {
        return Ok(*hash);
    }
    let bytes = std::fs::read(path).map_err(|e| fail(&e))?;
    if bytes.is_empty() {
        return Err(fail(&"empty profile"));
    }
    let hash = fnv64(&bytes);
    *seen = Some((path.to_path_buf(), len, modified, hash));
    Ok(hash)
}

/// Whether `flags` (from [`super::cflags`]) instrument.
pub fn generates(flags: &[String]) -> bool {
    flags.iter().any(|f| f == GEN_FLAG)
}

/// `H2_CPU=<name>`: `-mcpu=<name>` for arm64 targets (`apple-m1`,
/// `apple-m3`, ...). Unset or empty adds nothing. Clang checks the name;
/// this only keeps it to one plain argument.
pub fn cpu_flags(value: Option<&str>) -> Result<Vec<String>, Error> {
    match value.unwrap_or("") {
        "" => Ok(vec![]),
        v if v
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.+".contains(&b)) =>
        {
            Ok(vec![format!("-mcpu={v}")])
        }
        v => Err(err(format!(
            "H2_CPU={v}: expected a CPU name such as apple-m3"
        ))),
    }
}

/// `flags` for `arch`: `-mcpu` is an arm64 CPU, so an x86_64 target (Intel
/// macOS in `--arch all`, Windows) compiles without it.
pub fn for_arch(flags: Vec<String>, arch: Arch) -> Vec<String> {
    match arch {
        Arch::Arm64 | Arch::Arm64Tvos | Arch::Arm64Ios => flags,
        Arch::X86_64 | Arch::X86_64Windows => flags
            .into_iter()
            .filter(|f| !f.starts_with("-mcpu="))
            .collect(),
    }
}

/// Linked into an instrumented module (`H2_PGO=gen`) by
/// [`super::host_dylib`]. Its constructor names the raw profile file:
/// `H2_PGO_OUT`, else `$TMPDIR/h2.profraw` (`/tmp` without `TMPDIR`). The
/// name is set through the runtime API, which outranks `LLVM_PROFILE_FILE`
/// and the runtime's own default whichever initialiser runs first, and the
/// runtime keeps the pointer, hence the static buffer. The profile
/// runtime writes the file at exit (or when the module is unloaded);
/// `h2pgo_write_v1` writes it now, for a host stopping a run on its own
/// terms (`H2_BOOT_STOP_AT_PRESENT`).
pub const SHIM: &str = r#"/* H2_PGO=gen glue: names the raw profile and writes it on request. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

void __llvm_profile_set_filename(const char *);
int __llvm_profile_write_file(void);

static char h2pgo_path[4096];

__attribute__((constructor)) static void h2pgo_init(void) {
    const char *out = getenv("H2_PGO_OUT");
    if (out && *out) {
        snprintf(h2pgo_path, sizeof h2pgo_path, "%s", out);
    } else {
        const char *t = getenv("TMPDIR");
        if (!t || !*t) t = "/tmp";
        size_t n = strlen(t);
        snprintf(h2pgo_path, sizeof h2pgo_path, "%s%sh2.profraw", t, t[n - 1] == '/' ? "" : "/");
    }
    __llvm_profile_set_filename(h2pgo_path);
}

const char *h2pgo_file_v1(void) { return h2pgo_path; }

int h2pgo_write_v1(void) { return __llvm_profile_write_file(); }
"#;

/// The name [`super::host_dylib`] gives [`SHIM`] in the source directory
/// (removed again once compiled).
pub const SHIM_NAME: &str = "h2pgo_shim.c";

/// Write an instrumented module's raw profile now. `None` when `lib` was
/// not built with `H2_PGO=gen`; otherwise the file written, or why not.
///
/// # Safety
/// `lib` must be a module built by [`super::host_dylib`].
pub unsafe fn write_profile(lib: &libloading::Library) -> Option<Result<PathBuf, String>> {
    unsafe {
        let write: libloading::Symbol<unsafe extern "C" fn() -> i32> =
            lib.get(b"h2pgo_write_v1").ok()?;
        let file: libloading::Symbol<unsafe extern "C" fn() -> *const std::ffi::c_char> =
            lib.get(b"h2pgo_file_v1").ok()?;
        let path = PathBuf::from(
            std::ffi::CStr::from_ptr(file())
                .to_string_lossy()
                .into_owned(),
        );
        Some(match write() {
            0 => Ok(path),
            r => Err(format!(
                "__llvm_profile_write_file returned {r} for {}",
                path.display()
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pgo_values_parse() {
        assert_eq!(Pgo::parse(None).unwrap(), Pgo::Off);
        assert_eq!(Pgo::parse(Some("")).unwrap(), Pgo::Off);
        assert_eq!(Pgo::parse(Some("0")).unwrap(), Pgo::Off);
        assert_eq!(Pgo::parse(Some("gen")).unwrap(), Pgo::Gen);
        assert_eq!(
            Pgo::parse(Some("use:/p/h2.profdata")).unwrap(),
            Pgo::Use("/p/h2.profdata".into())
        );
        let Pgo::Use(rel) = Pgo::parse(Some("use:h2.profdata")).unwrap() else {
            panic!()
        };
        assert!(rel.is_absolute() && rel.ends_with("h2.profdata"), "{rel:?}");
        for bad in ["1", "use:", "generate", "use"] {
            assert!(Pgo::parse(Some(bad)).is_err(), "{bad}");
        }
        assert!(Pgo::Off.flags().unwrap().is_empty());
        assert_eq!(Pgo::Gen.flags().unwrap(), [GEN_FLAG]);
        assert!(generates(&Pgo::Gen.flags().unwrap()));
        assert!(Pgo::Use("/nonexistent/h2.profdata".into()).flags().is_err());
    }

    #[test]
    fn a_profile_is_keyed_by_its_contents() {
        let dir = std::env::temp_dir().join(format!("h2-pgo-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("h2.profdata");
        std::fs::write(&p, b"").unwrap();
        assert!(Pgo::Use(p.clone()).flags().is_err(), "an empty profile");
        std::fs::write(&p, b"one").unwrap();
        let a = Pgo::Use(p.clone()).flags().unwrap();
        assert_eq!(a[0], format!("-fprofile-instr-use={}", p.display()));
        assert!(a.contains(&"-Wno-profile-instr-out-of-date".to_owned()));
        assert!(a.contains(&"-Wno-profile-instr-unprofiled".to_owned()));
        assert_eq!(Pgo::Use(p.clone()).flags().unwrap(), a, "stable");
        std::fs::write(&p, b"three").unwrap();
        let b = Pgo::Use(p.clone()).flags().unwrap();
        assert_ne!(a, b, "same path, new contents");
        assert!(!generates(&b));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cpu_is_arm64_only() {
        assert!(cpu_flags(None).unwrap().is_empty());
        assert!(cpu_flags(Some("")).unwrap().is_empty());
        assert_eq!(cpu_flags(Some("apple-m3")).unwrap(), ["-mcpu=apple-m3"]);
        assert!(cpu_flags(Some("apple-m3 -O0")).is_err());
        let f = || vec!["-O2".to_owned(), "-mcpu=apple-m3".to_owned()];
        for a in [Arch::Arm64, Arch::Arm64Tvos, Arch::Arm64Ios] {
            assert_eq!(for_arch(f(), a), f(), "{a:?}");
        }
        for a in [Arch::X86_64, Arch::X86_64Windows] {
            assert_eq!(for_arch(f(), a), ["-O2"], "{a:?}");
        }
    }
}
