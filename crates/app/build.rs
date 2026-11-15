//! Bakes the commit, branch and build time into the watermark
//! (`app::watermark::BuildInfo`). Without git (a source tarball) they read
//! `unknown`. `H2_BUILD_SHA`, `H2_BUILD_BRANCH` and `SOURCE_DATE_EPOCH`
//! override them, for reproducible builds.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn main() {
    let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    for v in ["H2_BUILD_SHA", "H2_BUILD_BRANCH", "SOURCE_DATE_EPOCH"] {
        println!("cargo:rerun-if-env-changed={v}");
    }
    // Rerun when HEAD moves (a commit, a checkout) and when the crate's
    // sources change, so the build time follows a rebuild of the shell.
    println!("cargo:rerun-if-changed=src");
    if let Some(git_dir) = git(&dir, &["rev-parse", "--absolute-git-dir"]) {
        let git_dir = PathBuf::from(git_dir);
        println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
        let common = git(&dir, &["rev-parse", "--git-common-dir"])
            .map(|c| {
                let c = PathBuf::from(c);
                if c.is_absolute() { c } else { dir.join(c) }
            })
            .unwrap_or_else(|| git_dir.clone());
        println!(
            "cargo:rerun-if-changed={}",
            common.join("packed-refs").display()
        );
        if let Some(r) = git(&dir, &["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={}", common.join(r).display());
        }
    }
    let sha = std::env::var("H2_BUILD_SHA")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| git(&dir, &["rev-parse", "--short=7", "HEAD"]))
        .unwrap_or_else(|| "unknown".into());
    let branch = std::env::var("H2_BUILD_BRANCH")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| git(&dir, &["symbolic-ref", "-q", "--short", "HEAD"]))
        .unwrap_or_else(|| {
            if sha == "unknown" {
                "unknown".into()
            } else {
                "detached".into()
            }
        });
    let time = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        });
    println!("cargo:rustc-env=H2_BUILD_SHA={sha}");
    println!("cargo:rustc-env=H2_BUILD_BRANCH={branch}");
    println!("cargo:rustc-env=H2_BUILD_UNIX={time}");
}
