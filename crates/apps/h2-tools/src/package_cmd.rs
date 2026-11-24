//! `h2 package check` and `h2 package build` (docs/editor/studio-redesign.md,
//! "Packages"): the package build of `crates/tools/workspace` from the
//! command line, for the coordinator and CI.
//!
//! `check` reads package folders, orders them as a game would
//! (`content::package::Set`) and reports what is wrong without a map: a
//! stale `package.lock`, files a package may not hold, an item with no
//! builder or a source its builder refuses, and two packages adding the
//! same tag. `build` runs the same checks, then applies the packages to a
//! base map the user names and writes the result.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use content::Sha256;
use content::package::{LOCK_FILE, MANIFEST_FILE, MapScope, Package, Set, SetFile};
use map::build::TagSet;
use map::{CacheType, Map};
use workspace::{
    Loaded, MapUnpin, Target, build, cache_key, check_package, check_set, forbidden_files,
    lock_findings, read_lock,
};

const USAGE: &str = "usage: h2 package check <dir>...
       h2 package build <dir>... --base <map> -o <out.map> [--map <name>] [--scope multiplayer|campaign|mainmenu|shared] [--no-lock]

A <dir> is a package folder (it holds package.toml), or a folder of them
(content/). Packages load in the order of their requires and after lists.
`build` applies them to --base, a cache map the user owns, for the map named
--map (default: the file's name) and writes the result to -o; it also
writes each package's package.lock unless --no-lock.";

pub fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("check") => check(parse(args.collect(), false)?),
        Some("build") => build_cmd(parse(args.collect(), true)?),
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => bail!("unknown package command {other:?}\n{USAGE}"),
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    dirs: Vec<PathBuf>,
    base: Option<PathBuf>,
    out: Option<PathBuf>,
    map: Option<String>,
    scope: Option<String>,
    no_lock: bool,
}

fn parse(args: Vec<String>, building: bool) -> Result<Args> {
    let mut out = Args::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = |what: &str| {
            it.next()
                .filter(|v| !v.starts_with("--"))
                .with_context(|| format!("{what} needs a value\n{USAGE}"))
        };
        match a.as_str() {
            "--base" if building => out.base = Some(value("--base")?.into()),
            "-o" | "--out" if building => out.out = Some(value("-o")?.into()),
            "--map" if building => out.map = Some(value("--map")?),
            "--scope" if building => out.scope = Some(value("--scope")?),
            "--no-lock" if building => out.no_lock = true,
            s if s.starts_with('-') => bail!("unexpected argument {s:?}\n{USAGE}"),
            _ => out.dirs.push(a.into()),
        }
    }
    if out.dirs.is_empty() {
        bail!("no package folder given\n{USAGE}");
    }
    if building && (out.base.is_none() || out.out.is_none()) {
        bail!("build needs --base and -o\n{USAGE}");
    }
    Ok(out)
}

/// A package folder, or the package folders in a folder.
fn expand(dir: &Path) -> Result<Vec<PathBuf>> {
    if dir.join(MANIFEST_FILE).is_file() {
        return Ok(vec![dir.to_owned()]);
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("{}: not a folder", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(MANIFEST_FILE).is_file())
        .collect();
    found.sort();
    if found.is_empty() {
        bail!(
            "{}: no {MANIFEST_FILE} here or in a folder below",
            dir.display()
        );
    }
    Ok(found)
}

struct Read {
    /// In load order.
    loaded: Vec<(PathBuf, Loaded)>,
    findings: Vec<String>,
}

/// Read the folders, order them, and collect what `check` reports.
fn read(dirs: &[PathBuf]) -> Result<Read> {
    let mut all = Vec::new();
    for d in dirs {
        all.extend(expand(d)?);
    }
    let mut found = Vec::new();
    let mut by_id: Vec<(PathBuf, Loaded)> = Vec::new();
    for dir in all {
        match Loaded::from_dir(&dir) {
            Ok(l) => by_id.push((dir, l)),
            Err(e) => found.push(e.to_string()),
        }
    }
    let packages: Vec<Package> = by_id.iter().map(|(_, l)| l.package.clone()).collect();
    let set = Set::new(packages, &SetFile::default()).map_err(|e| anyhow::anyhow!("{e}"))?;
    for p in &set.problems {
        found.push(format!("{p:?}"));
    }
    let mut loaded = Vec::new();
    for p in &set.loaded {
        let at = by_id
            .iter()
            .position(|(_, l)| l.package.id == p.id)
            .expect("the set holds what was read");
        loaded.push(by_id.swap_remove(at));
    }
    for (dir, l) in &loaded {
        let id = &l.package.manifest.package.id;
        match read_lock(dir) {
            Ok(Some(on_disc)) => {
                for f in lock_findings(&on_disc, &l.package.lock) {
                    found.push(format!("{id}: {f}"));
                }
            }
            Ok(None) => found.push(format!(
                "{id}: no {LOCK_FILE} (`h2 package build` writes it)"
            )),
            Err(e) => found.push(e.to_string()),
        }
        for f in forbidden_files(dir) {
            found.push(format!("{id}: {f}"));
        }
        found.extend(check_package(l).into_iter().map(|f| f.to_string()));
    }
    let ordered: Vec<Loaded> = loaded.iter().map(|(_, l)| l.clone()).collect();
    found.extend(check_set(&ordered).into_iter().map(|f| f.to_string()));
    Ok(Read {
        loaded,
        findings: found,
    })
}

fn check(args: Args) -> Result<()> {
    let r = read(&args.dirs)?;
    for f in &r.findings {
        println!("{f}");
    }
    println!("{} packages, {} problems", r.loaded.len(), r.findings.len());
    if r.findings.is_empty() {
        Ok(())
    } else {
        bail!("package check found problems")
    }
}

fn scope_of(word: &str) -> Result<MapScope> {
    Ok(match word {
        "multiplayer" => MapScope::Multiplayer,
        "campaign" => MapScope::Campaign,
        "mainmenu" => MapScope::Mainmenu,
        "shared" => MapScope::Shared,
        _ => bail!("--scope is multiplayer, campaign, mainmenu or shared, not {word:?}"),
    })
}

fn build_cmd(args: Args) -> Result<()> {
    let r = read(&args.dirs)?;
    // A missing or stale lock is what this command writes; the rest stops it.
    let blocking: Vec<&String> = r
        .findings
        .iter()
        .filter(|f| !f.contains(LOCK_FILE))
        .collect();
    if !blocking.is_empty() {
        for f in blocking {
            println!("{f}");
        }
        bail!("fix the problems `h2 package check` reports first");
    }
    let base_path = args.base.as_deref().expect("parsed");
    let out_path = args.out.as_deref().expect("parsed");
    if out_path == base_path {
        bail!("-o is the base map: write elsewhere");
    }
    let bytes = std::fs::read(base_path).with_context(|| base_path.display().to_string())?;
    let base_hash = Sha256::of(&bytes);
    let mut map = Map::from_reader(Cursor::new(bytes.as_slice()))
        .map_err(|e| anyhow::anyhow!("{}: not a map: {e}", base_path.display()))?;
    let scope = match &args.scope {
        Some(s) => scope_of(s)?,
        None => match map.header().cache_type {
            CacheType::Multiplayer => MapScope::Multiplayer,
            CacheType::Campaign => MapScope::Campaign,
            CacheType::MainMenu => MapScope::Mainmenu,
            _ => MapScope::Shared,
        },
    };
    let name = args.map.clone().unwrap_or_else(|| {
        base_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let target = Target { name: &name, scope };

    let unpin = MapUnpin::new(&mut map).map_err(|e| anyhow::anyhow!(e))?;
    let mut set =
        TagSet::from_map(&mut map).map_err(|e| anyhow::anyhow!("decomposing the map: {e}"))?;
    // A tag laid out anew can land past the old metadata's end; the same
    // lift `ui_kit::retail_edits` makes, and the size check below still runs.
    set.check_tag_memory = false;
    if let Some(p) = set.pinned.as_mut() {
        // Room in the index for the tags the packages add.
        p.clear_index_tail();
    }
    let ordered: Vec<Loaded> = r.loaded.iter().map(|(_, l)| l.clone()).collect();
    let unpin: &dyn workspace::Unpin = &unpin;
    let report =
        build(&mut set, &ordered, &target, Some(unpin)).map_err(|e| anyhow::anyhow!("{e}"))?;
    let (out, _) = set.build().context("building the map")?;
    let key = cache_key(&base_hash, &ordered, &target);
    std::fs::write(out_path, &out).with_context(|| out_path.display().to_string())?;

    if !args.no_lock {
        for (dir, l) in &r.loaded {
            std::fs::write(dir.join(LOCK_FILE), l.package.lock.to_toml())
                .with_context(|| format!("{}/{LOCK_FILE}", dir.display()))?;
        }
    }
    for (id, n) in &report.packages {
        println!("{id}: {n} items");
    }
    for k in &report.added {
        println!("added {k}");
    }
    for k in &report.changed {
        println!("changed {k}");
    }
    for o in &report.overrides {
        println!("override {o}");
    }
    println!(
        "{} for {} ({}): {} bytes, key {key}",
        out_path.display(),
        name,
        scope.word(),
        out.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn arguments_parse() {
        let a = parse(
            words("a b --base m.map -o out.map --map lockout --no-lock"),
            true,
        )
        .unwrap();
        assert_eq!(a.dirs, vec![PathBuf::from("a"), PathBuf::from("b")]);
        assert_eq!(a.base, Some("m.map".into()));
        assert_eq!(a.out, Some("out.map".into()));
        assert_eq!(a.map.as_deref(), Some("lockout"));
        assert!(a.no_lock);
        assert!(parse(words("a"), true).is_err());
        assert!(parse(words("--base m.map -o o"), true).is_err());
        assert!(parse(words("a --base m.map"), false).is_err());
        assert!(parse(words("a"), false).is_ok());
        assert_eq!(scope_of("campaign").unwrap(), MapScope::Campaign);
        assert!(scope_of("all").is_err());
    }

    fn write(dir: &Path, name: &str, text: &str) {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// A folder with two invented packages: one adds a tag, one is wrong.
    fn tree(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("h2-package-cmd-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(
            &root,
            "good/package.toml",
            "[package]\nformat = 1\nid = \"good\"\nname = \"Good\"\n\n[[item]]\nkind = \"tag\"\nsource = \"dmg.jpt!.yaml\"\ntag = 'invented\\dmg\\c'\n",
        );
        write(
            &root,
            "good/dmg.jpt!.yaml",
            "fields:\n  damage_lower_bound: 1.0\n",
        );
        write(
            &root,
            "bad/package.toml",
            "[package]\nformat = 1\nid = \"bad\"\nname = \"Bad\"\nafter = [\"good\"]\n\n[[item]]\nkind = \"model\"\nsource = \"m.yaml\"\ntag = 'invented\\m'\n\n[[item]]\nkind = \"tag\"\nsource = \"dmg.jpt!.yaml\"\ntag = 'invented\\dmg\\c'\n",
        );
        write(&root, "bad/m.yaml", "");
        write(&root, "bad/dmg.jpt!.yaml", "fields: {}\n");
        write(&root, "bad/leftover.map", "");
        root
    }

    #[test]
    fn check_reads_a_folder_of_packages_in_order_and_reports() {
        let root = tree("check");
        let r = read(&[root.clone()]).unwrap();
        let ids: Vec<&str> = r
            .loaded
            .iter()
            .map(|(_, l)| l.package.manifest.package.id.as_str())
            .collect();
        assert_eq!(ids, vec!["good", "bad"]);
        let all = r.findings.join("\n");
        assert!(all.contains("good: no package.lock"), "{all}");
        assert!(all.contains("a cache map"), "{all}");
        assert!(all.contains("no builder"), "{all}");
        assert!(all.contains("adds tag"), "{all}");
        assert!(
            check(Args {
                dirs: vec![root.clone()],
                ..Args::default()
            })
            .is_err()
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
