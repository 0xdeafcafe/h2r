//! `h2 coop-probe diff A B`: compare the per-pool simulation hash logs two
//! instances wrote with `H2_COOP_PROBE=1` (docs/live/live-coop.md section 9).

use anyhow::{Context, Result, bail};

const USAGE: &str = "usage: h2 coop-probe diff <host.hashes> <guest.hashes>

Compares two hash logs written by H2_COOP_PROBE=1 runs (one line per game
tick and game-state pool, in <data>/coop-probe/). Prints the ticks both
logs cover, the first tick where they differ and the pools that differ
there, and how many ticks each pool differs on. Exits 0 when every common
tick agrees, 1 when they differ or share no tick.";

/// The process exit code.
pub fn run(args: Vec<String>) -> Result<i32> {
    let [verb, a, b] = args.as_slice() else {
        bail!("{USAGE}")
    };
    if verb != "diff" {
        bail!("{USAGE}")
    }
    let (la, lb) = (read(a)?, read(b)?);
    let report = lockstep::pools::diff(&la, &lb);
    println!("a: {a}");
    println!("b: {b}");
    print!("{report}");
    Ok(if report.identical() { 0 } else { 1 })
}

fn read(path: &str) -> Result<lockstep::pools::Log> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
    lockstep::pools::Log::parse(&text).map_err(|e| anyhow::anyhow!("{path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, name: &str, body: &str) -> String {
        let p = dir.join(name);
        std::fs::write(&p, format!("{}\n{body}", lockstep::pools::LOG_HEADER)).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn diff_exits_by_verdict_and_refuses_bad_input() {
        let dir = std::env::temp_dir().join(format!("h2-coop-probe-cmd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = write(&dir, "a", "1 * 0000000000000001\n2 * 0000000000000002\n");
        let same = write(&dir, "same", "1 * 0000000000000001\n2 * 0000000000000002\n");
        let other = write(
            &dir,
            "other",
            "1 * 0000000000000001\n2 * 00000000000000ff\n",
        );
        let bad = dir.join("bad");
        std::fs::write(&bad, "not a log\n").unwrap();
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(run(s(&["diff", &a, &same])).unwrap(), 0);
        assert_eq!(run(s(&["diff", &a, &other])).unwrap(), 1);
        assert!(
            run(s(&["diff", &a, bad.to_str().unwrap()]))
                .unwrap_err()
                .to_string()
                .contains("not a coop-probe")
        );
        assert!(run(s(&["diff", &a])).is_err());
        assert!(run(s(&["merge", &a, &same])).is_err());
        assert!(run(s(&["diff", &a, "/nonexistent/h2-coop-probe"])).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
