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

#[derive(Default)]
pub(super) struct Headers {
    // Many translation units include the same SDK/support headers. Read and
    // hash each resolved dependency only once in this build.
    digests: Mutex<HashMap<PathBuf, (PathBuf, u64)>>,
}

impl Headers {
    pub(super) fn fingerprint(
        &self,
        cc: &Path,
        flags: &[String],
        arch: Arch,
        src: &Path,
        root: &Path,
    ) -> Result<u64, Error> {
        let driver = self.driver_fingerprint(cc, flags, arch, src)?;
        let output = Command::new(cc)
            .args(flags)
            .args(["-target", arch.triple(), "-M", "-MT", "h2"])
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
        let source = std::fs::canonicalize(src).map_err(err)?;
        let mut dependencies = Vec::new();
        let mut digests = self.digests.lock().unwrap();
        for path in parse(&output.stdout)? {
            let (resolved, digest) = match digests.get(&path) {
                Some(value) => value.clone(),
                None => {
                    let resolved = std::fs::canonicalize(&path).map_err(err)?;
                    // The caller already hashes the source in parallel.
                    if resolved == source {
                        continue;
                    }
                    let digest = fnv64(&std::fs::read(&resolved).map_err(err)?);
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
    fn driver_fingerprint(
        &self,
        cc: &Path,
        flags: &[String],
        arch: Arch,
        src: &Path,
    ) -> Result<u64, Error> {
        let output = Command::new(cc)
            .args(flags)
            .args(["-target", arch.triple(), "-###", "-c"])
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
        Ok(fnv64(&identity))
    }
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
