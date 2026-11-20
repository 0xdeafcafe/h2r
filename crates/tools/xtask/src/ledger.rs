//! The test ledger (docs/dev/build-speed.md, "The test ledger"): a package
//! whose tests passed with the same inputs, in any checkout on this
//! machine, is not run again.
//!
//! A package's key covers its own git tree, the trees of every workspace
//! package its tests build against (without their `tests/`, `benches/`
//! and `examples/`), the outside files those name, the workspace-wide
//! inputs, the toolchain, the machine, the environment that changes builds
//! or tests, and the command. A package with an uncommitted change in any
//! of that has no key and always runs. Only passes are recorded.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::git;
use crate::workspace::{OutsideInputs, Workspace, is_global};

/// Environment variables that change what cargo builds or what tests do.
pub fn keyed_env(name: &str) -> bool {
    matches!(name, "RUSTFLAGS" | "RUSTDOCFLAGS" | "CARGO_ENCODED_RUSTFLAGS")
        || name.starts_with("CARGO_PROFILE_")
        || name.starts_with("CARGO_BUILD_")
        || name.starts_with("H2_")
        || name.starts_with("H1_")
}

/// 128-bit FNV-1a, as 32 hex digits. A key names a pass; it is not a
/// defence against anyone.
pub fn hash(text: &str) -> String {
    let mut h: u128 = 0x6c62272e07bb014262b821756295c58d;
    for b in text.as_bytes() {
        h ^= u128::from(*b);
        h = h.wrapping_mul(0x0000000001000000000000000000013b);
    }
    format!("{h:032x}")
}

/// The inputs of every key, read once.
pub struct Inputs {
    /// package -> its whole tree's id at HEAD.
    pub tree: BTreeMap<String, String>,
    /// package -> what a dependent sees of it: the ids of its top-level
    /// entries, without the test-only directories.
    pub lib: BTreeMap<String, String>,
    /// outside path -> its id at HEAD ("absent" when HEAD has none).
    pub outside_ids: BTreeMap<String, String>,
    /// The workspace-wide inputs, the toolchain, the machine, the
    /// environment and the command, as one block of text.
    pub common: String,
    /// Packages with uncommitted changes in their own files or in the
    /// outside files they name; with `global_dirty`, every package.
    pub dirty: BTreeSet<String>,
    pub global_dirty: bool,
}

/// The key text of `package`; None when its inputs are not all committed.
pub fn key_text(ws: &Workspace, outside: &OutsideInputs, inputs: &Inputs, package: &str) -> Option<String> {
    let members = ws.closure(package);
    if inputs.global_dirty || members.iter().any(|m| inputs.dirty.contains(m)) {
        return None;
    }
    let mut text = inputs.common.clone();
    for m in &members {
        let id = if m == package { inputs.tree.get(m) } else { inputs.lib.get(m) };
        text.push_str(&format!("{m}={}\n", id?));
    }
    for (target, readers) in outside {
        if readers.contains_key(package) || readers.iter().any(|(r, tests)| members.contains(r) && !tests) {
            text.push_str(&format!("{target}={}\n", inputs.outside_ids.get(target).map_or("absent", String::as_str)));
        }
    }
    Some(text)
}

/// Reads the inputs for `packages` from git and the environment.
pub fn read_inputs(ws: &Workspace, outside: &OutsideInputs, packages: &BTreeSet<String>, command: &str) -> Result<Inputs> {
    let root = &ws.root;
    let mut members = BTreeSet::new();
    for p in packages {
        members.extend(ws.closure(p));
    }
    let names: Vec<String> = members.iter().cloned().collect();
    let dirs: Vec<String> = names.iter().map(|n| ws.packages[n].dir.clone()).collect();
    let mut tree = BTreeMap::new();
    for (n, id) in names.iter().zip(git::head_ids(root, &dirs)?) {
        if let Some(id) = id {
            tree.insert(n.clone(), id);
        }
    }
    let mut lib = BTreeMap::new();
    for n in &names {
        let mut text = String::new();
        for (id, path) in git::ls_tree(root, &ws.packages[n].dir)? {
            if ws.package_of(&path) == Some(n.as_str()) && !ws.test_only(n, &path) {
                text.push_str(&format!("{id} {path}\n"));
            }
        }
        lib.insert(n.clone(), hash(&text));
    }
    let targets: Vec<String> = outside.keys().cloned().collect();
    let outside_ids = targets
        .iter()
        .cloned()
        .zip(git::head_ids(root, &targets)?)
        .map(|(t, id)| (t, id.unwrap_or_else(|| "absent".into())))
        .collect();

    let mut dirty = BTreeSet::new();
    let mut global_dirty = false;
    for f in git::dirty_files(root)? {
        global_dirty |= is_global(&f);
        if let Some(p) = ws.package_of(&f) {
            dirty.insert(p.to_owned());
        }
        for (target, readers) in outside {
            if f == *target || f.starts_with(&format!("{target}/")) {
                dirty.extend(readers.keys().cloned());
            }
        }
    }

    let mut common = format!("command {command}\n");
    let globals = ["Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml", ".config/nextest.toml", ".cargo"];
    let globals: Vec<String> = globals.iter().map(|s| s.to_string()).collect();
    for (g, id) in globals.iter().zip(git::head_ids(root, &globals)?) {
        common.push_str(&format!("{g}={}\n", id.as_deref().unwrap_or("absent")));
    }
    let rustc = std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .arg("-vV")
        .current_dir(root)
        .output()?;
    common.push_str(&String::from_utf8_lossy(&rustc.stdout));
    common.push_str(&format!("machine {} {}\n", std::env::consts::OS, std::env::consts::ARCH));
    let mut env: Vec<(String, String)> = std::env::vars().filter(|(k, _)| keyed_env(k)).collect();
    env.sort();
    for (k, v) in env {
        common.push_str(&format!("{k}={v}\n"));
    }
    Ok(Inputs { tree, lib, outside_ids, common, dirty, global_dirty })
}

/// Where passes are recorded: one empty-ish file per key.
pub fn dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let cache = if cfg!(target_os = "macos") {
        home.join("Library/Caches")
    } else {
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".cache"))
    };
    cache.join("h2recomp/xtask-ledger")
}

pub fn passed(dir: &Path, key: &str) -> bool {
    dir.join(key).is_file()
}

pub fn record(dir: &Path, key: &str, note: &str) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(key), note)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ws() -> Workspace {
        let pkg = |name: &str, deps: Vec<&str>, dev: Vec<&str>| {
            let mut d: Vec<_> = deps.iter().map(|n| json!({ "name": n, "path": format!("/w/c/{n}"), "kind": null })).collect();
            d.extend(dev.iter().map(|n| json!({ "name": n, "path": format!("/w/c/{n}"), "kind": "dev" })));
            json!({ "name": name, "manifest_path": format!("/w/c/{name}/Cargo.toml"), "dependencies": d, "targets": [] })
        };
        let meta = json!({ "workspace_root": "/w", "packages": [
            pkg("a", vec![], vec![]), pkg("b", vec!["a"], vec![]), pkg("t", vec![], vec!["b"]), pkg("x", vec![], vec![]),
        ]});
        Workspace::from_metadata(&meta).unwrap()
    }

    fn inputs() -> Inputs {
        let ids = |s: &str| ["a", "b", "t", "x"].iter().map(|n| (n.to_string(), format!("{s}-{n}"))).collect();
        Inputs {
            tree: ids("tree"),
            lib: ids("lib"),
            outside_ids: BTreeMap::from([("docs/a.md".to_string(), "doc1".to_string())]),
            common: "common\n".into(),
            dirty: BTreeSet::new(),
            global_dirty: false,
        }
    }

    #[test]
    fn a_key_covers_what_the_tests_build_against() {
        let ws = ws();
        let outside = OutsideInputs::from([("docs/a.md".to_string(), BTreeMap::from([("a".to_string(), false)]))]);
        let mut i = inputs();
        let t = key_text(&ws, &outside, &i, "t").unwrap();
        assert_eq!(t, "common\na=lib-a\nb=lib-b\nt=tree-t\ndocs/a.md=doc1\n");
        let before = hash(&t);
        // x is outside t's closure: changing it keeps t's key.
        i.lib.insert("x".into(), "lib-x2".into());
        assert_eq!(hash(&key_text(&ws, &outside, &i, "t").unwrap()), before);
        // a is inside: changing it changes the key.
        i.lib.insert("a".into(), "lib-a2".into());
        assert_ne!(hash(&key_text(&ws, &outside, &i, "t").unwrap()), before);
        // An uncommitted change anywhere inside: no key.
        i.dirty.insert("a".into());
        assert_eq!(key_text(&ws, &outside, &i, "t"), None);
        assert!(key_text(&ws, &outside, &i, "x").is_some());
        i.global_dirty = true;
        assert_eq!(key_text(&ws, &outside, &i, "x"), None);
    }

    #[test]
    fn hashes_are_stable_and_distinct() {
        assert_eq!(hash(""), "6c62272e07bb014262b821756295c58d");
        assert_ne!(hash("a"), hash("b"));
        assert_eq!(hash("a").len(), 32);
        assert!(keyed_env("H2_XBE") && keyed_env("CARGO_PROFILE_DEV_DEBUG") && !keyed_env("PATH"));
    }
}
