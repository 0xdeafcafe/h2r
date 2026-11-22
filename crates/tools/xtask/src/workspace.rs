//! The workspace's packages from `cargo metadata --no-deps`, and which of
//! them a set of changed files can affect (docs/dev/build-speed.md, "Test
//! only what changed").

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

/// Files whose change can change every package's build or test run.
const GLOBAL_FILES: &[&str] =
    &["Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml", ".config/nextest.toml"];
/// Directories of the same kind.
const GLOBAL_DIRS: &[&str] = &[".cargo/"];

/// Whether a change to `path` (relative to the root) affects every package.
pub fn is_global(path: &str) -> bool {
    GLOBAL_FILES.contains(&path) || GLOBAL_DIRS.iter().any(|d| path.starts_with(d))
}

#[derive(Debug, Clone, Default)]
pub struct Package {
    /// The package's directory relative to the workspace root, `/`-separated.
    pub dir: String,
    /// Workspace packages it builds against (normal and build dependencies).
    pub deps: BTreeSet<String>,
    /// Workspace packages only its tests, benches and examples build against.
    pub dev_deps: BTreeSet<String>,
    /// Whether it has a library target with doctests.
    pub doctests: bool,
    /// Its library target's crate name (`live_server` for `live-server`).
    pub lib_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    #[cfg(test)]
    target_dir: PathBuf,
    pub packages: BTreeMap<String, Package>,
    /// (directory, package), longest directory first, so a package nested
    /// in another's directory claims its own files.
    dirs: Vec<(String, String)>,
}

/// Which packages a change affects, and why.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Affected {
    /// The packages to test.
    pub test: BTreeSet<String>,
    /// Each package a changed file belongs to or is named by: (file, the
    /// file is built only for the package's tests).
    pub why: BTreeMap<String, Vec<(String, bool)>>,
    /// Changed files that belong to no package and that no package names.
    pub unmapped: Vec<String>,
    /// A workspace-wide input changed, so every package is in `test`.
    pub global: Option<String>,
}

/// Files outside a package that its sources name: path (relative to the
/// root, a file or a directory) -> {package: named only from its tests}.
pub type OutsideInputs = BTreeMap<String, BTreeMap<String, bool>>;

fn rel(root: &Path, path: &str) -> Result<String> {
    let p = Path::new(path);
    let r = p.strip_prefix(root).with_context(|| format!("{path} is outside the workspace root {}", root.display()))?;
    Ok(r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
}

impl Workspace {
    /// Reads `cargo metadata --no-deps --format-version 1` output.
    pub fn from_metadata(meta: &Value) -> Result<Self> {
        let root = PathBuf::from(meta["workspace_root"].as_str().ok_or_else(|| anyhow!("metadata has no workspace_root"))?);
        #[cfg(test)]
        let target_dir = meta["target_directory"].as_str().map(PathBuf::from).unwrap_or_else(|| root.join("target"));
        let list = meta["packages"].as_array().ok_or_else(|| anyhow!("metadata has no packages"))?;
        let mut packages = BTreeMap::new();
        let mut by_dir = BTreeMap::new();
        for p in list {
            let name = p["name"].as_str().ok_or_else(|| anyhow!("a package has no name"))?.to_owned();
            let manifest = p["manifest_path"].as_str().ok_or_else(|| anyhow!("{name} has no manifest_path"))?;
            let dir = rel(&root, Path::new(manifest).parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default().as_str())?;
            let mut pkg = Package { dir: dir.clone(), ..Package::default() };
            for t in p["targets"].as_array().into_iter().flatten() {
                let kinds: Vec<&str> = t["kind"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                if kinds.iter().any(|k| matches!(*k, "lib" | "rlib" | "dylib" | "proc-macro")) {
                    pkg.lib_name = t["name"].as_str().map(|n| n.replace('-', "_"));
                    pkg.doctests = t["doctest"].as_bool().unwrap_or(true);
                }
            }
            by_dir.insert(dir, name.clone());
            packages.insert(name, pkg);
        }
        for p in list {
            let name = p["name"].as_str().unwrap_or_default().to_owned();
            for dep in p["dependencies"].as_array().into_iter().flatten() {
                let Some(path) = dep["path"].as_str() else { continue };
                let Ok(dir) = rel(&root, path) else { continue };
                let Some(target) = by_dir.get(&dir) else { continue };
                if *target == name {
                    continue;
                }
                let pkg = packages.get_mut(&name).expect("listed above");
                if dep["kind"].as_str() == Some("dev") {
                    pkg.dev_deps.insert(target.clone());
                } else {
                    pkg.deps.insert(target.clone());
                }
            }
        }
        let mut dirs: Vec<(String, String)> = by_dir.into_iter().collect();
        dirs.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(&b.0)));
        Ok(Workspace {
            root,
            #[cfg(test)]
            target_dir,
            packages,
            dirs,
        })
    }

    /// The package whose directory holds `path` (relative to the root).
    pub fn package_of(&self, path: &str) -> Option<&str> {
        self.dirs
            .iter()
            .find(|(d, _)| d.is_empty() || path == d || path.starts_with(&format!("{d}/")))
            .map(|(_, n)| n.as_str())
    }

    /// Whether `path`, inside `package`, is built only for its tests.
    pub fn test_only(&self, package: &str, path: &str) -> bool {
        let dir = &self.packages[package].dir;
        let inner = path.strip_prefix(dir.as_str()).unwrap_or(path).trim_start_matches('/');
        matches!(inner.split('/').next(), Some("tests" | "benches" | "examples"))
    }

    /// `seeds`, every package that builds against one of them in turn, and
    /// the packages whose tests alone build against one of those.
    pub fn dependents(&self, seeds: &BTreeSet<String>) -> BTreeSet<String> {
        let mut rev: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (name, p) in &self.packages {
            for d in &p.deps {
                rev.entry(d.as_str()).or_default().push(name);
            }
        }
        let mut seen = BTreeSet::new();
        let mut stack: Vec<&str> = seeds.iter().map(String::as_str).collect();
        while let Some(c) = stack.pop() {
            if seen.insert(c.to_owned()) {
                stack.extend(rev.get(c).into_iter().flatten().copied());
            }
        }
        let tests: Vec<String> = self
            .packages
            .iter()
            .filter(|(_, p)| p.dev_deps.iter().any(|d| seen.contains(d)))
            .map(|(n, _)| n.clone())
            .collect();
        seen.extend(tests);
        seen
    }

    /// `package`, what its tests build against, and all those build
    /// against in turn: what a test run of `package` compiles of ours.
    pub fn closure(&self, package: &str) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![package.to_owned()];
        stack.extend(self.packages[package].dev_deps.iter().cloned());
        while let Some(c) = stack.pop() {
            if seen.insert(c.clone()) {
                stack.extend(self.packages[&c].deps.iter().cloned());
            }
        }
        seen
    }

    /// The packages a change to `files` can affect. `outside` is
    /// [`Workspace::outside_inputs`].
    pub fn affected(&self, files: &[String], outside: &OutsideInputs) -> Affected {
        let mut out = Affected::default();
        for f in files {
            if is_global(f) {
                out.global.get_or_insert_with(|| f.clone());
                continue;
            }
            let owner = self.package_of(f);
            if let Some(owner) = owner {
                out.why.entry(owner.to_owned()).or_default().push((f.clone(), self.test_only(owner, f)));
            }
            let mut readers: BTreeMap<&str, bool> = BTreeMap::new();
            for (target, pkgs) in outside {
                if f == target || f.starts_with(&format!("{target}/")) {
                    for (p, tests) in pkgs {
                        let r = readers.entry(p.as_str()).or_insert(true);
                        *r = *r && *tests;
                    }
                }
            }
            for (p, tests) in &readers {
                out.why.entry((*p).to_owned()).or_default().push((f.clone(), *tests));
            }
            if owner.is_none() && readers.is_empty() {
                out.unmapped.push(f.clone());
            }
        }
        if out.global.is_some() {
            out.test = self.packages.keys().cloned().collect();
            return out;
        }
        let built: BTreeSet<String> =
            out.why.iter().filter(|(_, fs)| !fs.iter().all(|(_, t)| *t)).map(|(p, _)| p.clone()).collect();
        out.test = self.dependents(&built);
        out.test.extend(out.why.keys().cloned());
        out
    }

    /// Files outside a package that its sources name, from the lines of
    /// `git grep -n -I -E -e 'include(_str|_bytes)?!\(|#\[path|CARGO_MANIFEST_DIR' -- '*.rs'`:
    /// `include!` forms, `#[path]`, and `../` strings on a line that
    /// joins them to `CARGO_MANIFEST_DIR`.
    pub fn outside_inputs<'a>(&self, grep_lines: impl IntoIterator<Item = &'a str>) -> OutsideInputs {
        let mut found = OutsideInputs::new();
        for line in grep_lines {
            let mut parts = line.splitn(3, ':');
            let (Some(source), Some(_), Some(text)) = (parts.next(), parts.next(), parts.next()) else { continue };
            let Some(owner) = self.package_of(source) else { continue };
            let owner_dir = &self.packages[owner].dir;
            let source_dir = source.rsplit_once('/').map_or("", |(d, _)| d);
            let mut named: Vec<(String, String)> = Vec::new();
            for opener in ["include!(", "include_str!(", "include_bytes!("] {
                for s in quoted_after(text, opener, true) {
                    named.push((source_dir.to_owned(), s));
                }
            }
            for s in quoted_after(text, "#[path", true) {
                named.push((source_dir.to_owned(), s));
            }
            for s in quoted_after(text, "CARGO_MANIFEST_DIR", false) {
                if s.starts_with("../") || s.starts_with("/../") {
                    named.push((owner_dir.clone(), s.trim_start_matches('/').to_owned()));
                }
            }
            for (base, path) in named {
                if !path.contains("..") {
                    continue;
                }
                let Some(target) = normalize(&format!("{base}/{path}")) else { continue };
                // Not the package's own files, and not a directory above it
                // (code that only finds the checkout's root).
                if self.package_of(&target) == Some(owner) {
                    continue;
                }
                if target.is_empty() || format!("{owner_dir}/").starts_with(&format!("{target}/")) {
                    continue;
                }
                let tests = self.test_only(owner, source);
                let r = found.entry(target).or_default().entry(owner.to_owned()).or_insert(true);
                *r = *r && tests;
            }
        }
        found
    }
}

/// The double-quoted strings that follow each `opener` in `text`. With
/// `first_only`, only the first string after each opener (the argument of
/// `include_str!(...)`); otherwise every string up to the next `;`.
fn quoted_after(text: &str, opener: &str, first_only: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(opener) {
        rest = &rest[at + opener.len()..];
        if !first_only {
            // `env!("CARGO_MANIFEST_DIR")`: the opener's own closing quote.
            rest = rest.strip_prefix('"').unwrap_or(rest);
        }
        let end = rest.find(';').unwrap_or(rest.len());
        let mut seg = &rest[..end];
        while let Some(q) = seg.find('"') {
            let body = &seg[q + 1..];
            let mut s = String::new();
            let mut chars = body.char_indices();
            let mut close = None;
            while let Some((i, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, n)) = chars.next() {
                            s.push(n);
                        }
                    }
                    '"' => {
                        close = Some(i);
                        break;
                    }
                    c => s.push(c),
                }
            }
            let Some(close) = close else { break };
            out.push(s);
            if first_only {
                break;
            }
            seg = &body[close + 1..];
        }
    }
    out
}

/// `a/b/../c` -> `a/c`; None when the path climbs above the root.
pub fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            c => parts.push(c),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dep(root: &str, dir: &str, kind: Option<&str>) -> Value {
        json!({ "name": dir, "path": format!("{root}/{dir}"), "kind": kind })
    }

    /// formats/a <- runtime/b <- apps/c; tools/d dev-depends on b; apps/e
    /// depends on d; runtime/b/probe is nested in b's directory.
    fn sample() -> Workspace {
        let root = "/w";
        let pkg = |name: &str, dir: &str, deps: Vec<Value>| {
            json!({
                "name": name,
                "manifest_path": format!("{root}/{dir}/Cargo.toml"),
                "dependencies": deps,
                "targets": [{ "kind": ["lib"], "name": name, "doctest": true }],
            })
        };
        let meta = json!({
            "workspace_root": root,
            "target_directory": "/w/target",
            "packages": [
                pkg("a", "crates/formats/a", vec![]),
                pkg("b", "crates/runtime/b", vec![dep(root, "crates/formats/a", None)]),
                pkg("probe", "crates/runtime/b/probe", vec![]),
                pkg("c", "crates/apps/c", vec![dep(root, "crates/runtime/b", Some("build"))]),
                pkg("d", "crates/tools/d", vec![dep(root, "crates/runtime/b", Some("dev"))]),
                pkg("e", "crates/apps/e", vec![dep(root, "crates/tools/d", None)]),
            ],
        });
        Workspace::from_metadata(&meta).unwrap()
    }

    fn files(f: &[&str]) -> Vec<String> {
        f.iter().map(|s| s.to_string()).collect()
    }

    fn set(s: &[&str]) -> BTreeSet<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reads_the_metadata() {
        let ws = sample();
        assert_eq!(ws.packages["b"].dir, "crates/runtime/b");
        assert_eq!(ws.packages["b"].deps, set(&["a"]));
        assert_eq!(ws.packages["c"].deps, set(&["b"]));
        assert_eq!(ws.packages["d"].dev_deps, set(&["b"]));
        assert_eq!(ws.packages["a"].lib_name.as_deref(), Some("a"));
        assert_eq!(ws.target_dir, PathBuf::from("/w/target"));
    }

    #[test]
    fn maps_paths_to_packages_nested_first() {
        let ws = sample();
        assert_eq!(ws.package_of("crates/runtime/b/src/lib.rs"), Some("b"));
        assert_eq!(ws.package_of("crates/runtime/b/probe/src/lib.rs"), Some("probe"));
        assert_eq!(ws.package_of("crates/runtime/bb/src/lib.rs"), None);
        assert_eq!(ws.package_of("docs/x.md"), None);
        assert!(ws.test_only("b", "crates/runtime/b/tests/it.rs"));
        assert!(!ws.test_only("b", "crates/runtime/b/src/tests.rs"));
    }

    #[test]
    fn a_source_change_takes_every_reverse_dependency() {
        let ws = sample();
        let got = ws.affected(&files(&["crates/formats/a/src/lib.rs"]), &OutsideInputs::new());
        // d only dev-depends on b, so e (which depends on d) is not affected.
        assert_eq!(got.test, set(&["a", "b", "c", "d"]));
        assert!(got.unmapped.is_empty());
        assert_eq!(got.global, None);
    }

    #[test]
    fn a_test_change_takes_its_package_alone() {
        let ws = sample();
        let got = ws.affected(&files(&["crates/runtime/b/tests/it.rs"]), &OutsideInputs::new());
        assert_eq!(got.test, set(&["b"]));
        assert_eq!(got.why["b"], vec![("crates/runtime/b/tests/it.rs".to_string(), true)]);
    }

    #[test]
    fn a_root_file_change_takes_everything() {
        let ws = sample();
        for f in ["Cargo.lock", "Cargo.toml", ".cargo/config.toml", ".config/nextest.toml", "rust-toolchain.toml"] {
            let got = ws.affected(&files(&["docs/x.md", f]), &OutsideInputs::new());
            assert_eq!(got.test.len(), 6, "{f}");
            assert_eq!(got.global.as_deref(), Some(f));
        }
        assert!(!is_global("crates/formats/a/Cargo.toml"));
        assert!(!is_global(".config/other.toml"));
    }

    #[test]
    fn unmapped_files_are_reported() {
        let ws = sample();
        let got = ws.affected(&files(&["docs/x.md", "web/src/a.ts"]), &OutsideInputs::new());
        assert!(got.test.is_empty());
        assert_eq!(got.unmapped, files(&["docs/x.md", "web/src/a.ts"]));
    }

    #[test]
    fn files_named_from_outside_count_for_their_readers() {
        let ws = sample();
        let grep = [
            r#"crates/apps/c/src/lib.rs:3:const X: &str = include_str!("../../../../docs/x.md");"#,
            r#"crates/formats/a/tests/it.rs:9:#[path = "../../../runtime/b/tests/common/mod.rs"]"#,
            r#"crates/apps/e/src/lib.rs:4:let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../disc/a.iso");"#,
            r#"crates/apps/e/src/lib.rs:5:let own = include_bytes!("../assets/icon.png");"#,
            r#"crates/apps/e/src/lib.rs:6:let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");"#,
        ];
        let outside = ws.outside_inputs(grep.iter().copied());
        assert_eq!(outside["docs/x.md"], BTreeMap::from([("c".to_string(), false)]));
        assert_eq!(outside["crates/runtime/b/tests/common/mod.rs"], BTreeMap::from([("a".to_string(), true)]));
        assert_eq!(outside["disc/a.iso"], BTreeMap::from([("e".to_string(), false)]));
        // Its own files and the checkout root are not outside inputs.
        assert_eq!(outside.len(), 3, "{outside:?}");

        let got = ws.affected(&files(&["docs/x.md"]), &outside);
        assert_eq!(got.test, set(&["c"]));
        assert!(got.unmapped.is_empty());
        // Named only from a's tests: a is tested, its dependents are not
        // (b is there because b's own test file changed).
        let got = ws.affected(&files(&["crates/runtime/b/tests/common/mod.rs"]), &outside);
        assert_eq!(got.test, set(&["a", "b"]));
    }

    #[test]
    fn closure_is_what_a_test_build_compiles() {
        let ws = sample();
        assert_eq!(ws.closure("d"), set(&["a", "b", "d"]));
        assert_eq!(ws.closure("e"), set(&["d", "e"]));
    }

    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize("crates/apps/c/../../../docs/x.md").as_deref(), Some("docs/x.md"));
        assert_eq!(normalize("a/./b//c").as_deref(), Some("a/b/c"));
        assert_eq!(normalize("a/../.."), None);
    }
}
