//! Drops the test executables a nextest run has made stale.
//!
//! Cargo names a test executable `deps/<stem>-<hash>`, and the hash changes
//! with the crate's features, its dependencies and the profile. A merge of
//! master or a flag change builds a new one and never deletes the old one,
//! so a worktree's `deps` collects a copy per round (docs/dev/build-speed.md,
//! "Stale test executables"). After a run, nextest's list names the
//! executables that are current; every other `<stem>-<hash>` executable
//! with the same stem in the same `deps` directory goes, with its `.d`
//! dep-info and `.dSYM` bundle.
//!
//! Libraries are never touched: an rlib is `lib<stem>-<hash>.rlib`, and
//! kache's unit clean-up already drops the unused ones. A stale executable
//! modified within the last hour is kept, so a build running in the same
//! target at the same time keeps what it just linked.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Only executables older than this are deleted.
const MIN_AGE: Duration = Duration::from_secs(3600);

/// `name` split as `<stem>-<16 hex digits>`, the form cargo gives a unit's
/// output in `deps`.
fn stem_hash(name: &str) -> Option<(&str, &str)> {
    let (stem, hash) = name.rsplit_once('-')?;
    (hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()) && !stem.is_empty()).then_some((stem, hash))
}

/// The current executables, grouped by `deps` directory: the stems and
/// the hashes of each stem that are current.
pub fn current(binaries: &[PathBuf]) -> BTreeMap<PathBuf, BTreeMap<String, BTreeSet<String>>> {
    let mut out: BTreeMap<PathBuf, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    for b in binaries {
        let (Some(dir), Some(name)) = (b.parent(), b.file_name().and_then(|n| n.to_str())) else { continue };
        if let Some((stem, hash)) = stem_hash(name) {
            out.entry(dir.to_owned()).or_default().entry(stem.to_owned()).or_default().insert(hash.to_owned());
        }
    }
    out
}

/// Of the entry names in one `deps` directory, the stale executables:
/// a `<stem>-<hash>` with no extension, of a current stem, whose hash is
/// not current. `is_old` says whether an entry is old enough to go.
pub fn stale<'a>(
    names: impl IntoIterator<Item = &'a str>,
    current: &BTreeMap<String, BTreeSet<String>>,
    is_old: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut out: Vec<String> = names
        .into_iter()
        .filter(|n| !n.contains('.'))
        .filter(|n| match stem_hash(n) {
            Some((stem, hash)) => current.get(stem).is_some_and(|hashes| !hashes.contains(hash)),
            None => false,
        })
        .filter(|n| is_old(n))
        .map(str::to_owned)
        .collect();
    out.sort();
    out
}

/// Deletes the stale test executables beside `binaries`, the current ones
/// nextest listed. Returns how many executables went and the bytes freed.
/// Errors are skipped: pruning never fails a test run.
pub fn prune(binaries: &[PathBuf]) -> (usize, u64) {
    let now = SystemTime::now();
    let (mut count, mut bytes) = (0, 0);
    for (dir, stems) in current(binaries) {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let names: Vec<String> = entries.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
        let is_old = |n: &str| {
            std::fs::symlink_metadata(dir.join(n))
                .and_then(|m| m.modified())
                .is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age >= MIN_AGE))
        };
        for name in stale(names.iter().map(String::as_str), &stems, is_old) {
            let exe = dir.join(&name);
            let size = std::fs::symlink_metadata(&exe).map(|m| m.len()).unwrap_or(0);
            if std::fs::remove_file(&exe).is_err() {
                continue;
            }
            count += 1;
            bytes += size;
            let _ = std::fs::remove_file(dir.join(format!("{name}.d")));
            bytes += remove_tree(&dir.join(format!("{name}.dSYM")));
        }
    }
    (count, bytes)
}

/// Removes a directory tree, returning the bytes its files held.
fn remove_tree(p: &Path) -> u64 {
    fn size(p: &Path) -> u64 {
        match std::fs::symlink_metadata(p) {
            Ok(m) if m.is_dir() => std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| size(&e.path())).sum(),
            Ok(m) => m.len(),
            Err(_) => 0,
        }
    }
    let n = size(p);
    if std::fs::remove_dir_all(p).is_ok() { n } else { 0 }
}

/// The executables nextest's `list --message-format json` names.
pub fn binaries_from_list(list: &serde_json::Value) -> Vec<PathBuf> {
    list["rust-suites"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(_, s)| s["binary-path"].as_str().map(PathBuf::from))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0123456789abcdef";
    const B: &str = "fedcba9876543210";
    const C: &str = "00112233aabbccdd";

    fn cur() -> BTreeMap<String, BTreeSet<String>> {
        current(&[PathBuf::from(format!("/t/debug/deps/engine-{A}")), PathBuf::from(format!("/t/debug/deps/live_server-{C}"))])
            .remove(Path::new("/t/debug/deps"))
            .unwrap()
    }

    #[test]
    fn only_other_hashes_of_current_stems_go() {
        let names = [
            format!("engine-{A}"),        // current
            format!("engine-{B}"),        // stale
            format!("engine-{B}.d"),      // goes with it, not on its own
            format!("libengine-{B}.rlib"), // a library: never
            format!("tags-{B}"),          // a stem nextest didn't list
            format!("live_server-{A}"),   // stale, other stem
            "engine-notahash".to_owned(),
        ];
        let got = stale(names.iter().map(String::as_str), &cur(), |_| true);
        assert_eq!(got, vec![format!("engine-{B}"), format!("live_server-{A}")]);
    }

    #[test]
    fn young_executables_stay() {
        let names = [format!("engine-{B}")];
        assert!(stale(names.iter().map(String::as_str), &cur(), |_| false).is_empty());
    }

    #[test]
    fn hyphenated_stems_split_at_the_hash() {
        assert_eq!(stem_hash(&format!("sim-objects-{A}")), Some(("sim-objects", A)));
        assert_eq!(stem_hash("sim-objects"), None);
    }

    #[test]
    fn prunes_on_disk() {
        let dir = std::env::temp_dir().join(format!("xtask-prune-{}", std::process::id()));
        let deps = dir.join("debug/deps");
        std::fs::create_dir_all(deps.join(format!("engine-{B}.dSYM/Contents"))).unwrap();
        for n in [format!("engine-{A}"), format!("engine-{B}"), format!("engine-{B}.d"), format!("libengine-{B}.rlib")] {
            std::fs::write(deps.join(n), b"x").unwrap();
        }
        std::fs::write(deps.join(format!("engine-{B}.dSYM/Contents/dwarf")), b"yy").unwrap();
        let old = SystemTime::now() - Duration::from_secs(7200);
        std::fs::File::options().write(true).open(deps.join(format!("engine-{B}"))).unwrap().set_modified(old).unwrap();

        let (n, bytes) = prune(&[deps.join(format!("engine-{A}"))]);
        assert_eq!((n, bytes), (1, 3));
        assert!(deps.join(format!("engine-{A}")).exists());
        assert!(deps.join(format!("libengine-{B}.rlib")).exists());
        assert!(!deps.join(format!("engine-{B}")).exists());
        assert!(!deps.join(format!("engine-{B}.d")).exists());
        assert!(!deps.join(format!("engine-{B}.dSYM")).exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
