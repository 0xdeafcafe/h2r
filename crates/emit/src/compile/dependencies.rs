//! Ask the actual preprocessor for every input, including system headers.
//! Rescan on cache lookup so a newly added include-path shadow is noticed.
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use super::{Arch, Error, err};
use crate::fnv64;

/// One clang probe's result, shared by every translation unit that can use
/// it. The first thread to need it runs the probe; the others wait.
type Shared<T> = std::sync::Arc<std::sync::OnceLock<Result<T, String>>>;
/// Probe results by key, each computed once.
type Probes<K, T> = Mutex<HashMap<K, Shared<T>>>;

#[derive(Default)]
pub(super) struct Headers {
    // Many translation units include the same SDK/support headers. Read and
    // hash each resolved dependency only once in this build.
    digests: Mutex<HashMap<PathBuf, (PathBuf, u64)>>,
    // Driver traces (`-###`) by source directory and extension, with the
    // source's names abstracted (see `driver_fingerprint`).
    drivers: Probes<(PathBuf, OsString), Option<u64>>,
    // `-M` dependency lists by preprocessor skeleton (see `skeleton`),
    // without the source that was scanned.
    scans: Probes<(PathBuf, u64), Vec<PathBuf>>,
    // Whether the build's headers can change between translation units
    // with the same skeleton (they use `__BASE_FILE__` or save and restore
    // macros); then every unit is scanned on its own.
    unshareable: std::sync::atomic::AtomicBool,
}

/// Run `probe` once per `key` in `map`; every caller gets its result.
fn once<K: std::hash::Hash + Eq, T: Clone>(
    map: &Probes<K, T>,
    key: K,
    probe: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    let cell = map.lock().unwrap().entry(key).or_default().clone();
    cell.get_or_init(|| probe().map_err(|e| e.0)).clone().map_err(Error)
}

/// What decides which files preprocessing a source reads, as far as the
/// source's own text goes: every line that holds a directive or could
/// (anything with `#`, or the `%:` digraph, or the `??=` trigraph), with its
/// line number, since `__LINE__` is visible to `#if`. `None` when the text
/// can change preprocessor state outside directives (`_Pragma`,
/// `__COUNTER__`) or names itself (`__FILE__` and relatives): such a source
/// is always scanned on its own.
///
/// Lines joined to their neighbours count too: a line spliced to the next
/// by a final backslash (or `??/`), the line after it, and every line that
/// starts or ends inside a block comment. Those can carry a directive's
/// text, or decide whether a `#` line is a directive at all. A comment that
/// opens and closes on one line changes neither.
///
/// Two sources in one directory with the same skeleton, under the same
/// flags, include exactly the same files: the text between those lines is
/// ordinary tokens, which can't define macros or include anything.
pub(super) fn skeleton(text: &[u8]) -> Option<u64> {
    const SELF_AWARE: &[&[u8]] = &[b"_Pragma", b"__COUNTER__", b"__FILE__", b"__FILE_NAME__", b"__BASE_FILE__"];
    if SELF_AWARE.iter().any(|n| text.windows(n.len()).any(|w| w == *n)) {
        return None;
    }
    let mut s = b"h2-skeleton-v2\0".to_vec();
    // Comment state ignores string literals: a `/*` inside one only makes
    // more lines count, which is the safe direction.
    let mut in_comment = false;
    let mut spliced = false;
    for (n, line) in text.split(|b| *b == b'\n').enumerate() {
        let starts_in_comment = in_comment;
        let mut i = 0;
        while i + 1 < line.len() {
            match (in_comment, &line[i..i + 2]) {
                (false, b"/*") | (true, b"*/") => {
                    in_comment = !in_comment;
                    i += 2;
                }
                _ => i += 1,
            }
        }
        let body = line.strip_suffix(b"\r").unwrap_or(line);
        let splices = body.ends_with(b"\\") || body.ends_with(b"??/");
        let joined = std::mem::replace(&mut spliced, splices) || splices || starts_in_comment || in_comment;
        let directive = line.contains(&b'#') || line.windows(2).any(|w| w == b"%:") || line.windows(3).any(|w| w == b"??=");
        if directive || joined {
            s.extend((n as u64).to_le_bytes());
            s.extend((line.len() as u64).to_le_bytes());
            s.extend(line);
        }
    }
    Some(fnv64(&s))
}

/// Header text that makes what a source includes depend on more than its
/// skeleton.
fn defeats_sharing(header: &[u8]) -> bool {
    [&b"__BASE_FILE__"[..], b"push_macro", b"pop_macro", b"include_alias"]
        .iter()
        .any(|n| header.windows(n.len()).any(|w| w == *n))
}

impl Headers {
    pub(super) fn fingerprint(
        &self,
        cc: &Path,
        flags: &[String],
        arch: Arch,
        src: &Path,
        text: &[u8],
        root: &Path,
    ) -> Result<u64, Error> {
        let share = skeleton(text);
        self.fingerprint_with(cc, flags, arch, src, share, root)
    }

    fn fingerprint_with(
        &self,
        cc: &Path,
        flags: &[String],
        arch: Arch,
        src: &Path,
        skeleton: Option<u64>,
        root: &Path,
    ) -> Result<u64, Error> {
        let driver = self.driver_fingerprint(cc, flags, arch, src)?;
        let source = std::fs::canonicalize(src).map_err(err)?;
        let dir = source.parent().unwrap_or(Path::new("/")).to_path_buf();
        let target = arch.target_args()?;
        let scan = |src: &Path| -> Result<Vec<PathBuf>, Error> {
            let output = Command::new(cc)
                .args(flags)
                .args(&target)
                .args(["-M", "-MT", "h2"])
                .arg(src)
                .output()
                .map_err(err)?;
            if !output.status.success() {
                return Err(err(format!(
                    "dependencies for {}: {}",
                    src.display(),
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
            parse(&output.stdout)
        };
        let shared = skeleton.filter(|_| !self.unshareable.load(std::sync::atomic::Ordering::Relaxed));
        let paths = match shared {
            // The scanned source itself is dropped: each user hashes its own.
            Some(k) => once(&self.scans, (dir, k), || {
                let found = scan(src)?;
                let mut kept = Vec::new();
                for p in found {
                    if std::fs::canonicalize(&p).map_err(err)? != source {
                        kept.push(p);
                    }
                }
                Ok(kept)
            })?,
            None => scan(src)?,
        };
        let mut dependencies = Vec::new();
        let mut digests = self.digests.lock().unwrap();
        for path in paths {
            let (resolved, digest) = match digests.get(&path) {
                Some(value) => value.clone(),
                None => {
                    let resolved = std::fs::canonicalize(&path).map_err(err)?;
                    // The caller already hashes the source in parallel.
                    if resolved == source {
                        continue;
                    }
                    let bytes = std::fs::read(&resolved).map_err(err)?;
                    if defeats_sharing(&bytes) {
                        self.unshareable.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    let digest = fnv64(&bytes);
                    digests.insert(path, (resolved.clone(), digest));
                    (resolved, digest)
                }
            };
            if resolved != source {
                // Output-local headers have the same identity across output
                // directories; external include locations remain distinct.
                let name = resolved
                    .strip_prefix(root)
                    .unwrap_or(&resolved)
                    .to_path_buf();
                dependencies.push((name, digest));
            }
        }
        drop(digests);
        if shared.is_some() && self.unshareable.load(std::sync::atomic::Ordering::Relaxed) {
            // A header seen just now rules sharing out: scan this source
            // on its own after all (later ones won't share either).
            return self.fingerprint_with(cc, flags, arch, src, None, root);
        }
        dependencies.sort();
        dependencies.dedup();
        let mut identity = b"h2-dependencies-v2\0".to_vec();
        identity.extend(driver.to_le_bytes());
        for (path, digest) in dependencies {
            let path = path.as_os_str().as_bytes();
            identity.extend((path.len() as u64).to_le_bytes());
            identity.extend(path);
            identity.extend(digest.to_le_bytes());
        }
        Ok(fnv64(&identity))
    }

    /// -M lists C includes, not driver configurations. The dry run exposes
    /// expanded options, including configuration/response-file arguments,
    /// and identifies explicit and default configuration files that were read.
    ///
    /// The driver sees a source only as its path, so one dry run serves a
    /// directory of sources with one extension: the trace is kept with the
    /// path and `-main-file-name` abstracted, and each source's own name is
    /// added back. If the probe's name shows up anywhere else in its trace,
    /// each source is probed on its own.
    fn driver_fingerprint(&self, cc: &Path, flags: &[String], arch: Arch, src: &Path) -> Result<u64, Error> {
        let name = src.file_name().unwrap_or_default().to_owned();
        let dir = std::fs::canonicalize(src.parent().unwrap_or(Path::new("."))).map_err(err)?;
        let ext = src.extension().unwrap_or_default().to_owned();
        let shared = once(&self.drivers, (dir, ext), || {
            let (trace, abstracted) = self.driver_trace(cc, flags, arch, src)?;
            Ok(abstracted.then(|| fnv64(&trace)))
        })?;
        let trace = match shared {
            Some(h) => h.to_le_bytes().to_vec(),
            None => self.driver_trace(cc, flags, arch, src)?.0,
        };
        let mut identity = b"h2-driver-v2\0".to_vec();
        identity.extend(trace);
        identity.extend(name.as_bytes());
        Ok(fnv64(&identity))
    }

    /// `src`'s normalized driver trace, and whether its name was fully
    /// abstracted (appears nowhere else in it).
    fn driver_trace(&self, cc: &Path, flags: &[String], arch: Arch, src: &Path) -> Result<(Vec<u8>, bool), Error> {
        let output = Command::new(cc)
            .args(flags)
            .args(arch.target_args()?)
            .args(["-###", "-c"])
            .arg(src)
            .args(["-o", "/dev/null"])
            .output()
            .map_err(err)?;
        if !output.status.success() {
            return Err(err(format!(
                "driver configuration for {}: {}",
                src.display(),
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let trace = [output.stdout, output.stderr].concat();
        let mut identity = normalize(&trace, src);
        let name = src.file_name().unwrap_or_default().as_bytes();
        let mut main = b"\"-main-file-name\" \"".to_vec();
        main.extend(name);
        main.push(b'"');
        identity = replace(&identity, &main, b"\"-main-file-name\" \"<h2-name>\"");
        let abstracted = !name.is_empty() && !identity.windows(name.len()).any(|w| w == name);
        let mut digests = self.digests.lock().unwrap();
        for line in trace.split(|b| *b == b'\n') {
            if let Some(name) = line.strip_prefix(b"Configuration file: ") {
                let path = PathBuf::from(OsString::from_vec(name.to_vec()));
                let digest = match digests.get(&path) {
                    Some((_, digest)) => *digest,
                    None => {
                        let resolved = std::fs::canonicalize(&path).map_err(err)?;
                        let digest = fnv64(&std::fs::read(&resolved).map_err(err)?);
                        digests.insert(path, (resolved, digest));
                        digest
                    }
                };
                identity.extend(digest.to_le_bytes());
            }
        }
        Ok((identity, abstracted))
    }
}

/// `text` with every `from` replaced by `to`.
fn replace(text: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with(from) {
            result.extend(to);
            i += from.len();
        } else {
            result.push(text[i]);
            i += 1;
        }
    }
    result
}

/// Normalize only the quoted translation-unit argument, preserving every
/// expanded configuration option (including paths embedded in -D values).
/// The basename is separately present in clang's -main-file-name option.
fn normalize(trace: &[u8], source: &Path) -> Vec<u8> {
    let mut quoted = b"\"-x\" \"c\" \"".to_vec();
    for byte in source.as_os_str().as_bytes() {
        if matches!(byte, b'\\' | b'"') {
            quoted.push(b'\\');
        }
        quoted.push(*byte);
    }
    quoted.push(b'"');
    let mut result = Vec::new();
    let mut i = 0;
    while i < trace.len() {
        if trace[i..].starts_with(&quoted) {
            result.extend(b"\"-x\" \"c\" \"<h2-source>\"");
            i += quoted.len();
        } else {
            result.push(trace[i]);
            i += 1;
        }
    }
    result
}

/// The one `-MT h2` make rule produced by clang. Preserve escaped spaces,
/// hashes, backslashes and dollars, and do not depend on paths being UTF-8.
fn parse(text: &[u8]) -> Result<Vec<PathBuf>, Error> {
    let text = text
        .strip_prefix(b"h2:")
        .ok_or_else(|| err("invalid clang dependency rule"))?;
    let mut result = Vec::new();
    let mut path = Vec::new();
    let mut i = 0;
    while i < text.len() {
        match text[i] {
            b'\\' => {
                i += 1;
                let next = *text
                    .get(i)
                    .ok_or_else(|| err("incomplete clang dependency escape"))?;
                if next == b'\r' && text.get(i + 1) == Some(&b'\n') {
                    i += 1;
                } else if next != b'\n' {
                    path.push(next);
                }
            }
            b'$' if text.get(i + 1) == Some(&b'$') => {
                path.push(b'$');
                i += 1;
            }
            b' ' | b'\t' | b'\r' | b'\n' => {
                if !path.is_empty() {
                    result.push(PathBuf::from(OsString::from_vec(std::mem::take(&mut path))));
                }
            }
            byte => path.push(byte),
        }
        i += 1;
    }
    if !path.is_empty() {
        result.push(PathBuf::from(OsString::from_vec(path)));
    }
    if result.is_empty() {
        return Err(err("empty clang dependency rule"));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skeleton_is_the_directive_lines_and_where_they_are() {
        let a = skeleton(b"/* banner */\n#include \"h2gen.h\"\n\nint f(void) { return 1; }\n").unwrap();
        let b = skeleton(b"/* banner */\n#include \"h2gen.h\"\n\nint g(void) { return 2 + 3; }\nint h;\n").unwrap();
        assert_eq!(a, b, "only the text between directives differs");
        let moved = skeleton(b"\n/* banner */\n#include \"h2gen.h\"\n\nint f(void) { return 1; }\n").unwrap();
        assert_ne!(a, moved, "__LINE__ can see where a directive is");
        let other = skeleton(b"/* banner */\n#include \"h2abi.h\"\n\nint f(void) { return 1; }\n").unwrap();
        assert_ne!(a, other);
        // Directives hiding behind a comment, a digraph or a trigraph count.
        for hidden in [&b"/* x */ #include \"y.h\"\n"[..], b"%:include \"y.h\"\n", b"??=include \"y.h\"\n"] {
            let text = [&b"/* banner */\n#include \"h2gen.h\"\n\nint f(void) { return 1; }\n"[..], hidden].concat();
            assert_ne!(Some(a), skeleton(&text), "{}", String::from_utf8_lossy(hidden));
        }
        // Lines joined to a directive, or that decide whether a `#` line is
        // one, count.
        let base = b"/* banner */\n#include \"h2gen.h\"\n".to_vec();
        for (x, y) in [
            (&b"#define X \\\n1\n"[..], &b"#define X \\\n0\n"[..]),
            (b"#define X ??/\n1\n", b"#define X ??/\n0\n"),
            (b"#define X \\\r\n1\n", b"#define X \\\r\n0\n"),
            (b"#define X /*\n*/ 1\n", b"#define X /*\n*/ 0\n"),
            (b"#define X /*\n\n*/ 1\n", b"#define X /*\n\n*/ 0\n"),
            (b"int a; \\\n#define X 1\n", b"int a;\n#define X 1\n"),
            (b"int a; /*\n*/ #define X 1\n", b"int a;\n */ #define X 1\n"),
            (b"int a; /*\nb */ #define X 1\n", b"int a; /*\nc */ #define X 1\n"),
        ] {
            assert_ne!(
                skeleton(&[&base[..], x].concat()),
                skeleton(&[&base[..], y].concat()),
                "{}",
                String::from_utf8_lossy(x)
            );
        }
        // A comment within one line joins nothing: generated code is full of them.
        assert_eq!(
            skeleton(&[&base[..], b"int f(void) { /* 0x10 */ return 1; }\n"].concat()),
            skeleton(&[&base[..], b"int f(void) { /* 0x20 */ return 2; }\n"].concat())
        );
        // Text that can change the preprocessor's state or names the file
        // is never shared.
        for own in ["_Pragma(\"x\")", "__COUNTER__", "__FILE__", "__FILE_NAME__", "__BASE_FILE__"] {
            assert_eq!(skeleton(format!("#include \"h2gen.h\"\nint x = {own};\n").as_bytes()), None, "{own}");
        }
    }

    #[test]
    fn driver_normalization_preserves_paths_in_configuration_options() {
        let trace = br#"clang "-include" "/tmp/source/one.c" "-D" "LOCATION=/tmp/source/one.c" "-x" "c" "/tmp/source/one.c""#;
        assert_eq!(
            normalize(trace, Path::new("/tmp/source/one.c")),
            br#"clang "-include" "/tmp/source/one.c" "-D" "LOCATION=/tmp/source/one.c" "-x" "c" "<h2-source>""#
        );
        let quoted = br#"clang "-x" "c" "/tmp/source \"quoted\"/one.c""#;
        assert_eq!(
            normalize(quoted, Path::new("/tmp/source \"quoted\"/one.c")),
            br#"clang "-x" "c" "<h2-source>""#
        );
    }

    #[test]
    fn make_dependency_paths_preserve_escapes_and_non_utf8() {
        let mut rule =
            b"h2: /src/one.c \\\n /SDK\\ space/hash\\#cash$$.h /back\\\\slash.h /non-utf8-"
                .to_vec();
        rule.extend([0xff, b'.', b'h', b'\n']);
        let paths = parse(&rule).unwrap();
        assert_eq!(paths[0], Path::new("/src/one.c"));
        assert_eq!(paths[1], Path::new("/SDK space/hash#cash$.h"));
        assert_eq!(paths[2], Path::new("/back\\slash.h"));
        assert_eq!(paths[3].as_os_str().as_bytes(), b"/non-utf8-\xff.h");
        assert!(parse(b"wrong: source.c").is_err());
        assert!(parse(b"h2:").is_err());
        assert!(parse(b"h2: broken\\").is_err());
    }
}
