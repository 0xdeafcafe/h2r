//! Builds the live web app (`web/`) with bun when its build is missing or
//! older than the sources, so `h2 play --live` (and `--follow`) serve it
//! without a separate step.
//!
//! Freshness: `web/dist/.h2-web-tree` holds the git tree id of `web/` it was
//! built from. With uncommitted changes under `web/` there is no tree id to
//! trust, so an existing build is kept (use `bun run dev` while editing).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const STAMP: &str = ".h2-web-tree";

/// The web sources this binary was built from.
pub fn source_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web")
}

/// bun on PATH, else where its installer puts it.
pub fn find_bun() -> Option<PathBuf> {
    let on_path = Command::new("bun")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if on_path {
        return Some(PathBuf::from("bun"));
    }
    let home = std::env::var_os("HOME")?;
    let bun = PathBuf::from(home).join(".bun/bin/bun");
    bun.is_file().then_some(bun)
}

/// The git tree id of `web` at HEAD, or None when it isn't in git or has
/// uncommitted changes.
pub fn clean_tree(web: &Path) -> Option<String> {
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(web)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let dirty = git(&["status", "--porcelain", "--", "."])?;
    if !dirty.is_empty() {
        return None;
    }
    git(&["rev-parse", "HEAD:./"]).filter(|t| !t.is_empty())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    UpToDate,
    Built,
}

/// Builds `web` if needed. `run` runs one command (label, command) and
/// says why it failed.
pub fn ensure(
    web: &Path,
    bun: Option<&Path>,
    tree: Option<&str>,
    run: &mut dyn FnMut(&str, &mut Command) -> Result<(), String>,
) -> Result<Outcome, String> {
    let dist = web.join("dist");
    let built = dist.join("index.html").is_file();
    let stamp = fs::read_to_string(dist.join(STAMP)).ok();
    let fresh = match tree {
        Some(t) => built && stamp.as_deref().map(str::trim) == Some(t),
        None => built,
    };
    if fresh {
        return Ok(Outcome::UpToDate);
    }
    let bun = bun.ok_or_else(|| {
        "bun is not installed; install it (https://bun.sh, or `brew install oven-sh/bun/bun`) \
         and the web app builds on the next run"
            .to_string()
    })?;
    let mut install = Command::new(bun);
    install
        .args(["install", "--frozen-lockfile"])
        .current_dir(web);
    run("bun install", &mut install)?;
    let mut build = Command::new(bun);
    build.args(["run", "build"]).current_dir(web);
    run("bun run build", &mut build)?;
    if let Some(t) = tree {
        fs::write(dist.join(STAMP), format!("{t}\n"))
            .map_err(|e| format!("writing {}: {e}", dist.join(STAMP).display()))?;
    }
    Ok(Outcome::Built)
}

/// Runs a command with its output on this process's stderr.
pub fn run_inherit(what: &str, cmd: &mut Command) -> Result<(), String> {
    let status = cmd
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| format!("{what}: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("{what} failed ({status})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "h2-web-build-{name}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    type Ran = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

    /// A fake bun: records the commands and makes `dist/index.html`.
    fn fake(web: &Path, ran: &Ran) -> impl FnMut(&str, &mut Command) -> Result<(), String> {
        let (web, ran) = (web.to_path_buf(), ran.clone());
        move |what, _| {
            ran.borrow_mut().push(what.to_string());
            if what == "bun run build" {
                fs::create_dir_all(web.join("dist")).unwrap();
                fs::write(web.join("dist/index.html"), "<html>").unwrap();
            }
            Ok(())
        }
    }

    #[test]
    fn builds_when_missing_and_stamps_the_tree() {
        let web = scratch("missing");
        let ran = Ran::default();
        let out = ensure(
            &web,
            Some(Path::new("bun")),
            Some("abc"),
            &mut fake(&web, &ran),
        );
        assert_eq!(out, Ok(Outcome::Built));
        assert_eq!(*ran.borrow(), ["bun install", "bun run build"]);
        assert_eq!(
            fs::read_to_string(web.join("dist").join(STAMP)).unwrap(),
            "abc\n"
        );
    }

    #[test]
    fn same_tree_is_up_to_date_and_a_new_tree_rebuilds() {
        let web = scratch("stamp");
        let ran = Ran::default();
        ensure(
            &web,
            Some(Path::new("bun")),
            Some("abc"),
            &mut fake(&web, &ran),
        )
        .unwrap();
        ran.borrow_mut().clear();
        let out = ensure(
            &web,
            Some(Path::new("bun")),
            Some("abc"),
            &mut fake(&web, &ran),
        );
        assert_eq!((out, ran.borrow().len()), (Ok(Outcome::UpToDate), 0));
        let out = ensure(
            &web,
            Some(Path::new("bun")),
            Some("def"),
            &mut fake(&web, &ran),
        );
        assert_eq!(out, Ok(Outcome::Built));
    }

    #[test]
    fn uncommitted_changes_keep_an_existing_build() {
        let web = scratch("dirty");
        fs::create_dir_all(web.join("dist")).unwrap();
        fs::write(web.join("dist/index.html"), "<html>").unwrap();
        let ran = Ran::default();
        let out = ensure(&web, Some(Path::new("bun")), None, &mut fake(&web, &ran));
        assert_eq!((out, ran.borrow().len()), (Ok(Outcome::UpToDate), 0));
    }

    #[test]
    fn no_bun_says_how_to_get_it() {
        let web = scratch("nobun");
        let ran = Ran::default();
        let err = ensure(&web, None, Some("abc"), &mut fake(&web, &ran)).unwrap_err();
        assert!(err.contains("bun is not installed"), "{err}");
        assert!(ran.borrow().is_empty());
    }
}
