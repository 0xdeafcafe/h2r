//! A receipt for a completed analyze/lift/emit run. Validate file contents,
//! not mtimes: a missing, edited or extra source must trigger regeneration.
//! The caller holds emit_output::OutputLock across validation, writes, receipt
//! publication, compilation and final use of the generated module.
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

const RECEIPT: &str = "emit-cache-v1";

fn digest_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// Hash the entire executable as well as the XBE. A package version alone
/// cannot distinguish edits in the analyzer, lifter, IR passes or emitter.
pub fn key(image: &[u8], executable: &Path) -> io::Result<String> {
    Ok(format!(
        "{RECEIPT}\n{}\n{:x}\n{}\n",
        env!("CARGO_PKG_VERSION"),
        Sha256::digest(image),
        digest_file(executable)?
    ))
}

fn snapshot(src: &Path) -> io::Result<String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| io::Error::other("non-UTF8 generated file name"))?;
        // The ABI is installed separately on every invocation; object keys
        // cover its contents. A different ABI need not repeat lifting.
        if name != "h2abi.h"
            && (name.ends_with(".c") || name.ends_with(".h") || name == "manifest.tsv")
        {
            files.push((name.to_owned(), entry.path()));
        }
    }
    files.sort();
    let mut result = String::new();
    for (name, path) in files {
        result.push_str(&format!("{name}\t{}\n", digest_file(&path)?));
    }
    Ok(result)
}

pub fn valid(out: &Path, key: &str) -> bool {
    let Ok(receipt) = fs::read_to_string(out.join(RECEIPT)) else {
        return false;
    };
    let Some(expected) = receipt.strip_prefix(key) else {
        return false;
    };
    snapshot(&out.join("src")).is_ok_and(|actual| actual == expected)
}

/// Invalidate before regeneration, so an interrupted write cannot leave a
/// receipt that describes a partially replaced source tree.
pub fn invalidate(out: &Path) -> io::Result<()> {
    match fs::remove_file(out.join(RECEIPT)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Publish only after every generated file is written successfully.
pub fn record(out: &Path, key: &str) -> io::Result<()> {
    let text = format!("{key}{}", snapshot(&out.join("src"))?);
    let tmp = out.join(format!("{RECEIPT}.{}.tmp", std::process::id()));
    fs::write(&tmp, text)?;
    fs::rename(&tmp, out.join(RECEIPT)).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Tree(PathBuf);
    impl Tree {
        fn new(name: &str) -> Self {
            let out =
                std::env::temp_dir().join(format!("h2-emit-cache-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&out);
            fs::create_dir_all(out.join("src")).unwrap();
            for name in [
                "shard_00001.c",
                "h2gen.h",
                "h2simd.h",
                "h2x87.h",
                "h2gen_registry.c",
                "manifest.tsv",
            ] {
                fs::write(out.join("src").join(name), "synthetic\n").unwrap();
            }
            Self(out)
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn input_or_generator_edits_invalidate_the_key() {
        let tree = Tree::new("key");
        let exe = tree.0.join("generator");
        fs::write(&exe, "generator version one").unwrap();
        let first = key(b"synthetic XBE one", &exe).unwrap();
        assert_eq!(first, key(b"synthetic XBE one", &exe).unwrap());
        assert_ne!(first, key(b"synthetic XBE two", &exe).unwrap());
        fs::write(&exe, "generator version two").unwrap();
        assert_ne!(first, key(b"synthetic XBE one", &exe).unwrap());
    }

    #[test]
    fn receipt_checks_contents_even_when_size_and_mtime_match() {
        let tree = Tree::new("contents");
        let out = &tree.0;
        assert!(!valid(out, "key\n"));
        record(out, "key\n").unwrap();
        assert!(valid(out, "key\n"));
        assert!(!valid(out, "another key\n"));
        let path = out.join("src/shard_00001.c");
        let mtime = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&path, "different\n").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        assert!(!valid(out, "key\n"));
    }

    #[test]
    fn missing_extra_and_changed_support_files_are_cache_misses() {
        let tree = Tree::new("files");
        let out = &tree.0;
        record(out, "key\n").unwrap();
        for name in [
            "shard_00001.c",
            "h2gen.h",
            "h2simd.h",
            "h2x87.h",
            "manifest.tsv",
        ] {
            let path = out.join("src").join(name);
            fs::remove_file(&path).unwrap();
            assert!(!valid(out, "key\n"), "missing {name}");
            fs::write(&path, "changed support\n").unwrap();
            assert!(!valid(out, "key\n"), "changed {name}");
            fs::write(&path, "synthetic\n").unwrap();
            assert!(valid(out, "key\n"));
        }
        fs::write(out.join("src/extra.c"), "extra source\n").unwrap();
        assert!(!valid(out, "key\n"));
    }

    #[test]
    fn abi_refresh_does_not_repeat_lifting() {
        let tree = Tree::new("abi");
        record(&tree.0, "key\n").unwrap();
        fs::write(tree.0.join("src/h2abi.h"), "new ABI\n").unwrap();
        assert!(valid(&tree.0, "key\n"));
    }

    #[test]
    fn an_interrupted_generation_or_corrupt_receipt_is_a_miss() {
        let tree = Tree::new("interrupted");
        record(&tree.0, "key\n").unwrap();
        invalidate(&tree.0).unwrap();
        assert!(!valid(&tree.0, "key\n"));
        invalidate(&tree.0).unwrap();
        fs::write(tree.0.join(RECEIPT), "key\npartial").unwrap();
        assert!(!valid(&tree.0, "key\n"));
    }
}
