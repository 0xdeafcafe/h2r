//! Offline memory investigation. This command also handles the local
//! `h2 agent postmortem` shortcut, before any socket connection.
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const USAGE: &str = "usage: h2 mem postmortem [--pid N | FILE]";

#[derive(Debug, PartialEq)]
enum Selection {
    Process(Option<NonZeroU32>),
    File(PathBuf),
}

fn parse(args: &[String]) -> Result<Selection> {
    match args {
        [] => Ok(Selection::Process(None)),
        [flag, value] if flag == "--pid" => Ok(Selection::Process(Some(
            value.parse().context("--pid needs a positive process id")?,
        ))),
        [file] if !file.is_empty() && !file.starts_with('-') => {
            Ok(Selection::File(PathBuf::from(file)))
        }
        _ => bail!("{USAGE}"),
    }
}

pub fn run(args: &[String]) -> Result<()> {
    let Some((op, rest)) = args.split_first() else {
        bail!("{USAGE}")
    };
    if op != "postmortem" {
        bail!("{USAGE}");
    }
    let report = match parse(rest)? {
        Selection::File(file) => mem_watch::recorder::postmortem_file(Path::new(&file))
            .with_context(|| format!("reading {}", file.display()))?,
        Selection::Process(pid) => {
            let dir = mem_watch::report_dir(&content::store::default_data_root());
            mem_watch::recorder::postmortem(&dir, pid.map(NonZeroU32::get))
                .with_context(|| format!("finding a memory recording in {}", dir.display()))?
        }
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn postmortem_selects_one_pid_or_file() {
        assert_eq!(parse(&[]).unwrap(), Selection::Process(None));
        assert_eq!(
            parse(&words("--pid 34")).unwrap(),
            Selection::Process(NonZeroU32::new(34))
        );
        assert_eq!(
            parse(&words("saved.ndjson")).unwrap(),
            Selection::File("saved.ndjson".into())
        );
        for s in [
            "--pid",
            "--pid 0",
            "--pid -1",
            "--pid 34 a",
            "a b",
            "--unknown",
        ] {
            assert!(parse(&words(s)).is_err(), "{s}");
        }
    }
}
