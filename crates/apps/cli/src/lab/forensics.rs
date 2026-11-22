//! A compact postmortem for job metadata and the driver's lab mem output.
use std::path::{Path, PathBuf};

use anyhow::{Result, bail, ensure};
use mem_watch::recorder::{Diff, postmortem};
use serde::{Deserialize, Serialize};

use super::proto;

const MAX_GROWERS: usize = 20;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum Summary {
    Ready {
        pid: u32,
        #[serde(flatten)]
        diff: Diff,
    },
    Unavailable {
        reason: String,
    },
}

impl Summary {
    /// A missing recorder (for example, a game that died during startup) is
    /// recorded explicitly. Reading a large report never holds the jobs mutex.
    pub(super) fn read(dir: &Path) -> Self {
        match postmortem(dir, None) {
            Ok(mut evidence) => {
                evidence.diff.growers.truncate(MAX_GROWERS);
                Self::Ready {
                    pid: evidence.pid,
                    diff: evidence.diff,
                }
            }
            Err(error) => Self::Unavailable {
                reason: error.to_string(),
            },
        }
    }

    pub(super) fn print(&self) -> Result<()> {
        match self {
            Self::Unavailable { reason } => bail!("postmortem unavailable: {reason}"),
            Self::Ready { pid, diff } => {
                println!(
                    "PID {pid}   {}–{} ms   footprint {:+} bytes",
                    diff.from_ms, diff.to_ms, diff.footprint_delta
                );
                if diff.growers.is_empty() {
                    println!("No measured category grew during the recorded interval.");
                }
                for change in &diff.growers {
                    println!(
                        "{:+} {}  {}  {} ({} to {})",
                        change.delta,
                        change.unit,
                        change.category,
                        change.name,
                        change.before,
                        change.after
                    );
                }
                Ok(())
            }
        }
    }
}

/// Both bulk-download commands take a validated numeric job ID and destination.
pub(super) fn fetch_args(cmd: &str, args: Vec<String>) -> Result<(String, Option<PathBuf>)> {
    let mut id = None;
    let mut to = None;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--to" if to.is_none() => {
                let Some(path) = it.next() else {
                    bail!("--to needs a directory")
                };
                ensure!(!path.is_empty(), "--to needs a directory");
                to = Some(PathBuf::from(path));
            }
            value if id.is_none() && !value.starts_with('-') => {
                proto::check_job_id(value)?;
                id = Some(arg);
            }
            _ => bail!("usage: h2 lab {cmd} <id> [--to DIR]"),
        }
    }
    let Some(id) = id else {
        bail!("usage: h2 lab {cmd} <id> [--to DIR]")
    };
    Ok((id, to))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_accepts_one_numeric_id_and_optional_destination() {
        let parse =
            |args: &[&str]| fetch_args("mem", args.iter().map(|s| (*s).to_owned()).collect());
        assert_eq!(parse(&["0007"]).unwrap(), ("0007".into(), None));
        assert_eq!(
            parse(&["--to", "captures", "0007"]).unwrap(),
            ("0007".into(), Some("captures".into()))
        );
        for args in [
            vec![],
            vec!["abc"],
            vec!["0001", "0002"],
            vec!["../0007"],
            vec!["--all"],
            vec!["0007", "--to"],
            vec!["--to", "a", "--to", "b", "0007"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn a_killed_job_records_its_largest_growth_or_why_there_is_none() {
        let dir = std::env::temp_dir().join(format!(
            "h2-lab-postmortem-{}",
            live_proto::sign::random_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = Summary::read(&dir);
        assert!(matches!(missing, Summary::Unavailable { .. }));
        assert!(missing.print().is_err());
        // The lab host's own observation: footprint samples for the game.
        let observation = serde_json::json!({
            "job_id": "0007", "reason": "memory limit: 30 over 20 bytes", "game_pid": 321,
            "samples": [[0, 10], [2000, 30]],
        });
        std::fs::write(dir.join("lab-host.json"), observation.to_string()).unwrap();
        let summary = Summary::read(&dir);
        let Summary::Ready { pid, diff } = &summary else {
            panic!("{summary:?}")
        };
        assert_eq!((*pid, diff.footprint_delta), (321, 20));
        // The job record keeps it beside the job's other fields.
        let wire = serde_json::to_value(&summary).unwrap();
        assert_eq!(wire["status"], "ready");
        assert_eq!(wire["footprint_delta"], 20);
        let back: Summary = serde_json::from_value(wire).unwrap();
        assert!(matches!(back, Summary::Ready { pid: 321, .. }));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
