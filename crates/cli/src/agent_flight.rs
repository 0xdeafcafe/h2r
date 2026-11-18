//! `h2 agent trace ...` and `h2 agent flight ...`: per-tick rows taken in
//! the game (docs/runtime/agent-api.md, "Tick traces and the flight
//! recorder"), and `flight show`, which prints a trace or flight file's
//! rows without a game.

use agent_proto::trace::{ShowFilter, method, show as show_rows};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub const USAGE: &str = "  trace start <id[,id...]> [field[,field...]] [path]
                         trace.start: one JSONL row per game tick of those
                         objects (fields from world.object), written in the
                         game to the cache directory's traces/ (or path)
  trace stop | status    trace.stop (flushes; counts) or trace.status
  flight start <id[,id...]> [field[,field...]] [seconds]
                         flight.start: keep the last seconds (30) of the same
                         rows, with each tick's controller reads, in memory
  flight dump [path]     flight.dump: write the ring to flights/ (or path)
  flight stop | status   flight.stop or flight.status
  flight show <file> [--from TICK] [--to TICK] [--field PATH]...
                         print a trace or flight file's rows (no game needed):
                         whole rows, or per object the dotted paths asked for
                         (physics.crouch, pos) and `input` for the reads";

/// `trace ...` and `flight ...` (but `flight show`) as a call.
pub fn parse(words: &[String]) -> Option<Result<(String, Value)>> {
    let first = words.first()?.as_str();
    if first != "trace" && first != "flight" {
        return None;
    }
    Some(parse_verb(first, &words[1..]))
}

fn list(w: &str) -> Vec<String> {
    w.split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_verb(domain: &str, w: &[String]) -> Result<(String, Value)> {
    let verb = w.first().map(String::as_str).unwrap_or("");
    let most = |n: usize| -> Result<()> {
        if w.len() > n {
            bail!("{domain} {verb}: unexpected {}", w[n]);
        }
        Ok(())
    };
    let (m, p) = match (domain, verb) {
        (_, "start") => {
            most(4)?;
            let ids = w.get(1).with_context(|| format!("{domain} start needs object ids"))?;
            let mut p = json!({ "objects": list(ids) });
            if let Some(f) = w.get(2) {
                p["fields"] = json!(list(f));
            }
            if let Some(x) = w.get(3) {
                if domain == "trace" {
                    p["path"] = json!(x);
                } else {
                    p["seconds"] = json!(x.parse::<u32>().context("flight start: seconds")?);
                }
            }
            let m = if domain == "trace" {
                method::TRACE_START
            } else {
                method::FLIGHT_START
            };
            (m, p)
        }
        ("trace", "stop") => (method::TRACE_STOP, Value::Null),
        ("trace", "status") => (method::TRACE_STATUS, Value::Null),
        ("flight", "stop") => (method::FLIGHT_STOP, Value::Null),
        ("flight", "status") => (method::FLIGHT_STATUS, Value::Null),
        ("flight", "dump") => {
            most(2)?;
            (
                method::FLIGHT_DUMP,
                w.get(1).map_or(Value::Null, |p| json!({ "path": p })),
            )
        }
        _ => bail!("{domain}: start, stop, status{}", match domain {
            "flight" => ", dump or show",
            _ => "",
        }),
    };
    if p.is_null() {
        most(1)?;
    }
    Ok((m.into(), p))
}

/// `flight show`'s arguments: the file and the filter.
pub fn show_args(w: &[String]) -> Result<(String, ShowFilter)> {
    let mut file = None;
    let mut f = ShowFilter::default();
    let mut it = w.iter();
    let tick = |v: Option<&String>, flag: &str| -> Result<u64> {
        v.with_context(|| format!("{flag} needs a tick"))?
            .parse()
            .with_context(|| format!("{flag} needs a tick"))
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--from" => f.from = Some(tick(it.next(), "--from")?),
            "--to" => f.to = Some(tick(it.next(), "--to")?),
            "--field" => f
                .fields
                .push(it.next().context("--field needs a path")?.clone()),
            x if x.starts_with("--") => bail!("flight show: no option {x}"),
            x if file.is_none() => file = Some(x.to_owned()),
            x => bail!("flight show: unexpected {x}"),
        }
    }
    Ok((file.context("flight show needs a file")?, f))
}

/// `h2 agent flight show <file> ...`: prints the rows.
pub fn show(w: &[String]) -> Result<()> {
    let (file, f) = show_args(w)?;
    let text = std::fs::read_to_string(&file).with_context(|| format!("reading {file}"))?;
    for line in show_rows(&text, &f).map_err(|e| anyhow::anyhow!("{file}: {e}"))? {
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn short_forms_become_calls() {
        let c = |s: &str| parse(&w(s)).unwrap().unwrap();
        assert_eq!(
            c("trace start player.0,actor.e0010001 physics,pos walk.jsonl"),
            (
                "trace.start".into(),
                json!({"objects": ["player.0", "actor.e0010001"],
                       "fields": ["physics", "pos"], "path": "walk.jsonl"})
            )
        );
        assert_eq!(
            c("trace start player.0"),
            ("trace.start".into(), json!({"objects": ["player.0"]}))
        );
        assert_eq!(c("trace stop"), ("trace.stop".into(), Value::Null));
        assert_eq!(
            c("flight start player.0 physics 10"),
            (
                "flight.start".into(),
                json!({"objects": ["player.0"], "fields": ["physics"], "seconds": 10})
            )
        );
        assert_eq!(
            c("flight dump /tmp/x.jsonl"),
            ("flight.dump".into(), json!({"path": "/tmp/x.jsonl"}))
        );
        assert_eq!(c("flight status"), ("flight.status".into(), Value::Null));
        assert!(parse(&w("status")).is_none());
        for bad in [
            "trace",
            "trace start",
            "trace stop now",
            "flight start player.0 physics soon",
            "trace dump",
        ] {
            assert!(parse(&w(bad)).unwrap().is_err(), "{bad}");
        }
    }

    #[test]
    fn show_takes_a_file_and_filters() {
        let (file, f) =
            show_args(&w("f.jsonl --from 10 --to 20 --field physics.crouch --field input"))
                .unwrap();
        assert_eq!(file, "f.jsonl");
        assert_eq!(
            f,
            ShowFilter {
                from: Some(10),
                to: Some(20),
                fields: vec!["physics.crouch".into(), "input".into()],
            }
        );
        for bad in ["", "--from 3", "f --from x", "f g", "f --loud"] {
            assert!(show_args(&w(bad)).is_err(), "{bad}");
        }
    }
}
