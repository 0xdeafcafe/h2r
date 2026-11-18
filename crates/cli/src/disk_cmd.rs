//! `h2 disk clean`: removes merged idle worktrees, idle Cargo targets and
//! stale build artifacts (docs/dev/disk.md). Shows the plan unless `--yes`.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use diskclean::{Action, Options, apply, free_bytes, gib, plan};

const USAGE: &str = "usage: h2 disk clean [--yes] [--repo DIR] [--base BRANCH] [--scan DIR]...
                     [--if-below GB] [--kache PATH] [--empty-kache-below GB]

  --yes                 do it; without it, print what would go
  --repo DIR            a checkout of the repository (default: this directory)
  --base BRANCH         worktrees must be merged into it (default master)
  --scan DIR            also look for Cargo targets here, three levels deep
  --if-below GB         do nothing while more than GB are free
  --kache PATH          kache binary; with --empty-kache-below, empty its store
  --empty-kache-below GB  when still under GB free after cleaning (default 20)";

pub fn run(args: Vec<String>) -> Result<()> {
    let mut it = args.into_iter();
    match it.next().as_deref() {
        Some("clean") => {}
        _ => bail!("{USAGE}"),
    }
    let mut o = Options::new(std::env::current_dir()?);
    let (mut yes, mut if_below, mut kache, mut kache_below) = (false, None, None, 20.0);
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value\n{USAGE}"));
        match a.as_str() {
            "--yes" => yes = true,
            "--repo" => o.repo = PathBuf::from(val()?),
            "--base" => o.base = val()?,
            "--scan" => o.scan.push(PathBuf::from(val()?)),
            "--if-below" => if_below = Some(val()?.parse::<f64>()?),
            "--kache" => kache = Some(PathBuf::from(val()?)),
            "--empty-kache-below" => kache_below = val()?.parse::<f64>()?,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => bail!("unknown option {a}\n{USAGE}"),
        }
    }

    let free = gib(free_bytes(&o.repo)?);
    if let Some(limit) = if_below
        && free >= limit
    {
        println!("{free:.1} GB free, over {limit} GB: nothing to do");
        return Ok(());
    }
    println!("{free:.1} GB free");

    let p = plan(&o)?;
    for (path, why) in &p.kept {
        println!("keep    {}  ({why})", path.display());
    }
    let mut freed = 0u64;
    for a in &p.actions {
        let label = match a {
            Action::RemoveWorktree { path, branch } => format!("worktree {} ({branch})", path.display()),
            Action::RemoveTarget { path } => format!("target  {}", path.display()),
            Action::PruneArtifacts { target, paths } => {
                format!("prune   {} ({} old artifacts)", target.display(), paths.len())
            }
        };
        if !yes {
            println!("would remove {label}");
            continue;
        }
        match apply(&o, a) {
            Ok(b) => {
                freed += b;
                println!("removed {label}: {:.1} GB", gib(b));
            }
            Err(why) => println!("skipped {label}: {why}"),
        }
    }
    if !yes {
        println!("dry run: pass --yes to remove");
        return Ok(());
    }

    let free = gib(free_bytes(&o.repo)?);
    if let Some(k) = &kache
        && free < kache_below
    {
        println!("{free:.1} GB free, under {kache_below} GB: emptying the kache store");
        let st = Command::new(k).args(["clean", "--cache", "--yes"]).status();
        if !st.as_ref().is_ok_and(|s| s.success()) {
            println!("kache clean --cache failed: {st:?}");
        }
    }
    println!("freed {:.1} GB; {:.1} GB free", gib(freed), gib(free_bytes(&o.repo)?));
    Ok(())
}
