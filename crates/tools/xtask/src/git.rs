//! The git questions xtask asks. Paths are relative to the root.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

pub fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .context("could not run git")?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Files that differ between the merge base of `base` and HEAD, and the
/// working tree: committed, staged, unstaged and untracked changes.
pub fn changed_files(root: &Path, base: &str) -> Result<Vec<String>> {
    let merge_base = git(root, &["merge-base", base, "HEAD"])
        .with_context(|| format!("no merge base with {base}; pass --base <rev>"))?;
    let merge_base = merge_base.trim();
    let mut files = BTreeSet::new();
    for f in git(root, &["diff", "--name-only", "--no-renames", "-z", merge_base])?.split('\0') {
        files.insert(f.to_owned());
    }
    for f in git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?.split('\0') {
        files.insert(f.to_owned());
    }
    files.remove("");
    Ok(files.into_iter().collect())
}

/// Uncommitted files: staged, unstaged and untracked.
pub fn dirty_files(root: &Path) -> Result<Vec<String>> {
    let mut files = BTreeSet::new();
    for f in git(root, &["diff", "--name-only", "--no-renames", "-z", "HEAD"])?.split('\0') {
        files.insert(f.to_owned());
    }
    for f in git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?.split('\0') {
        files.insert(f.to_owned());
    }
    files.remove("");
    Ok(files.into_iter().collect())
}

/// The lines naming files outside a crate, for
/// [`crate::workspace::Workspace::outside_inputs`].
pub fn outside_grep(root: &Path) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["grep", "-n", "-I", "-E", "-e", r"include(_str|_bytes)?!\(|#\[path|CARGO_MANIFEST_DIR", "--", "*.rs"])
        .output()
        .context("could not run git grep")?;
    // 1 is "no match".
    if !matches!(out.status.code(), Some(0 | 1)) {
        bail!("git grep failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The object id of `HEAD:<path>`, or None when HEAD has no such path.
pub fn head_ids(root: &Path, paths: &[String]) -> Result<Vec<Option<String>>> {
    // One process for all of them: rev-parse prints the name back for one
    // it cannot resolve, so resolve each with --verify only when needed.
    let mut out = Vec::with_capacity(paths.len());
    let listed = git(root, &["ls-tree", "-d", "-r", "--full-tree", "HEAD"]).unwrap_or_default();
    let mut dirs = std::collections::BTreeMap::new();
    for line in listed.lines() {
        if let Some((info, path)) = line.split_once('\t')
            && let Some(id) = info.split_whitespace().nth(2)
        {
            dirs.insert(path.to_owned(), id.to_owned());
        }
    }
    for p in paths {
        if let Some(id) = dirs.get(p) {
            out.push(Some(id.clone()));
            continue;
        }
        let spec = format!("HEAD:{p}");
        let got = Command::new("git").arg("-C").arg(root).args(["rev-parse", "--verify", "--quiet", &spec]).output()?;
        let id = String::from_utf8_lossy(&got.stdout).trim().to_owned();
        out.push((got.status.success() && !id.is_empty()).then_some(id));
    }
    Ok(out)
}

/// `git ls-tree HEAD -- <dir>/`: (object id, path) of each entry directly
/// inside `dir`.
pub fn ls_tree(root: &Path, dir: &str) -> Result<Vec<(String, String)>> {
    let listed = git(root, &["ls-tree", "--full-tree", "HEAD", "--", &format!("{dir}/")])?;
    Ok(listed
        .lines()
        .filter_map(|l| {
            let (info, path) = l.split_once('\t')?;
            Some((info.split_whitespace().nth(2)?.to_owned(), path.to_owned()))
        })
        .collect())
}

/// The directory holding the disc files: `disc/` beside the main
/// checkout (worktrees share it), else in this one.
pub fn disc_dir(root: &Path) -> Option<PathBuf> {
    let common = git(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).ok()?;
    let main = Path::new(common.trim()).parent()?.join("disc");
    [main, root.join("disc")].into_iter().find(|d| d.is_dir())
}
