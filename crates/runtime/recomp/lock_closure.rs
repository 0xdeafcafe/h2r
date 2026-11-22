//! The part of `Cargo.lock` a build stamp depends on: the packages
//! reachable from a set of root packages through `dependencies`.
//!
//! Included by `build.rs` and, for its tests, by `src/lock_closure_tests.rs`
//! (both via `#[path]`), so this file has no dependencies of its own. The
//! lockfile's package tables are regular enough to read line by line:
//! `[[package]]`, then `name`, `version`, `source`, `checksum` and a
//! `dependencies` array whose entries are `"name"`, `"name version"` or
//! `"name version (source)"`.

#[derive(Default)]
struct Pkg {
    name: String,
    version: String,
    source: String,
    checksum: String,
    deps: Vec<String>,
}

fn quoted(s: &str) -> Option<&str> {
    let s = s.trim().trim_end_matches(',').trim();
    s.strip_prefix('"')?.strip_suffix('"')
}

fn parse(lock: &str) -> Vec<Pkg> {
    let mut pkgs: Vec<Pkg> = Vec::new();
    let mut in_pkg = false;
    let mut in_deps = false;
    for line in lock.lines() {
        let t = line.trim();
        if in_deps {
            if t.starts_with(']') {
                in_deps = false;
            } else if let (true, Some(d)) = (in_pkg, quoted(t)) {
                pkgs.last_mut().unwrap().deps.push(d.to_owned());
            }
            continue;
        }
        if t.starts_with('[') {
            in_pkg = t == "[[package]]";
            if in_pkg {
                pkgs.push(Pkg::default());
            }
            continue;
        }
        if !in_pkg {
            continue;
        }
        let Some((key, val)) = t.split_once('=') else { continue };
        let (key, val) = (key.trim(), val.trim());
        let p = pkgs.last_mut().unwrap();
        match key {
            "name" => p.name = quoted(val).unwrap_or_default().to_owned(),
            "version" => p.version = quoted(val).unwrap_or_default().to_owned(),
            "source" => p.source = quoted(val).unwrap_or_default().to_owned(),
            "checksum" => p.checksum = quoted(val).unwrap_or_default().to_owned(),
            "dependencies" => {
                // `[]` on one line is an empty list; otherwise entries follow.
                in_deps = !val.contains(']');
            }
            _ => {}
        }
    }
    pkgs
}

/// The canonical text of every package reachable from `roots` (names):
/// one `name version source checksum` line each, sorted, so it does not
/// depend on the lockfile's order and changes only when a reached package
/// is added, removed, or changes version, source or checksum. Packages
/// nothing reaches do not appear. Err when a root is not in the lockfile.
pub fn closure_text(lock: &str, roots: &[&str]) -> Result<String, String> {
    let pkgs = parse(lock);
    let mut seen = vec![false; pkgs.len()];
    let mut work: Vec<usize> = Vec::new();
    for r in roots {
        let hits: Vec<usize> = (0..pkgs.len()).filter(|&i| pkgs[i].name == *r).collect();
        if hits.is_empty() {
            return Err(format!("Cargo.lock has no package {r}"));
        }
        work.extend(hits);
    }
    while let Some(i) = work.pop() {
        if std::mem::replace(&mut seen[i], true) {
            continue;
        }
        for d in &pkgs[i].deps {
            let mut parts = d.split_whitespace();
            let name = parts.next().unwrap_or_default();
            let version = parts.next();
            for (j, p) in pkgs.iter().enumerate() {
                if p.name == name && version.is_none_or(|v| p.version == v) {
                    work.push(j);
                }
            }
        }
    }
    let mut lines: Vec<String> = (0..pkgs.len())
        .filter(|&i| seen[i])
        .map(|i| {
            let p = &pkgs[i];
            format!("{} {} {} {}\n", p.name, p.version, p.source, p.checksum)
        })
        .collect();
    lines.sort();
    Ok(lines.concat())
}

/// FNV-1a, continuing from `h`.
pub fn fnv_extend(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// The closure's digest for `roots` in the lockfile at `path`, continuing
/// the running hash `h`. Panics (a build script's way to fail) when the
/// lockfile is unreadable or lacks a root.
#[allow(dead_code)] // the tests include this file without calling it
pub fn fold_closure(h: u64, path: &str, roots: &[&str]) -> u64 {
    let lock = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let text = closure_text(&lock, roots).unwrap_or_else(|e| panic!("{path}: {e}"));
    fnv_extend(h, text.as_bytes())
}
