//! `h2 agent trace ...` and `h2 agent flight ...`: per-tick rows taken in
//! the game (docs/runtime/agent-api.md, "Tick traces and the flight
//! recorder"), and `flight show`, which prints rows from a local file or
//! pages them from a running game.

use std::io::{ErrorKind, Write};

use agent_proto::Client;
use agent_proto::trace::{
    ShowFilter, TraceKind, TraceReadPage, TraceReadParams, method, show as show_rows, valid_file_name,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

pub const USAGE: &str = "  trace start <id[,id...]> [field[,field...]] [path]
                         trace.start: one JSONL row per game tick of those
                         objects (fields from world.object), written in the
                         game to traces/ under the agent output directory
  trace stop | status    trace.stop (flushes; counts) or trace.status
  flight start <id[,id...]> [field[,field...]] [seconds]
                         flight.start: keep the last seconds (30) of the same
                         rows, with each tick's controller reads, in memory
  flight dump [path]     flight.dump: write the ring to flights/ (or path)
  flight stop | status   flight.stop or flight.status
  flight show <file> [--remote] [--kind trace|flight]
                    [--from TICK] [--to TICK] [--field PATH]...
                         print a trace or flight file's rows. A missing local
                         file, --remote or --kind selects the game's socket
                         and needs a bare file name.
                         --field prints dotted paths per object (e.g.
                         physics.crouch) or `input` for controller reads";

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
            let ids = w
                .get(1)
                .with_context(|| format!("{domain} start needs object ids"))?;
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
        _ => bail!(
            "{domain}: start, stop, status{}",
            match domain {
                "flight" => ", dump or show",
                _ => "",
            }
        ),
    };
    if p.is_null() {
        most(1)?;
    }
    Ok((m.into(), p))
}

struct ShowArgs {
    file: String,
    filter: ShowFilter,
    remote: bool,
    kind: Option<TraceKind>,
}

impl ShowArgs {
    fn check_remote_name(&self) -> Result<()> {
        ensure!(
            valid_file_name(&self.file),
            "flight show: {:?} must be a bare file name for a remote read",
            self.file
        );
        Ok(())
    }
}

fn show_args(w: &[String]) -> Result<ShowArgs> {
    let mut file = None;
    let mut filter = ShowFilter::default();
    let mut remote = false;
    let mut kind = None;
    let mut it = w.iter();
    let tick = |v: Option<&String>, flag: &str| -> Result<u64> {
        v.with_context(|| format!("{flag} needs a tick"))?
            .parse()
            .with_context(|| format!("{flag} needs a tick"))
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--from" => filter.from = Some(tick(it.next(), "--from")?),
            "--to" => filter.to = Some(tick(it.next(), "--to")?),
            "--field" => filter
                .fields
                .push(it.next().context("--field needs a path")?.clone()),
            "--remote" => remote = true,
            "--kind" => {
                kind = Some(match it.next().map(String::as_str) {
                    Some("trace") => TraceKind::Trace,
                    Some("flight") => TraceKind::Flight,
                    _ => bail!("--kind needs trace or flight"),
                });
                remote = true;
            }
            x if x.starts_with("--") => bail!("flight show: no option {x}"),
            x if file.is_none() => file = Some(x.to_owned()),
            x => bail!("flight show: unexpected {x}"),
        }
    }
    Ok(ShowArgs {
        file: file.context("flight show needs a file")?,
        filter,
        remote,
        kind,
    })
}

/// Lab requests always read from the run's game, so paths are bare names.
#[cfg(all(unix, feature = "desktop"))]
pub(crate) fn check_remote_show_args(w: &[String]) -> Result<()> {
    show_args(w)?.check_remote_name()
}

/// `h2 agent flight show <file> ...`: opens the socket only for remote reads.
pub fn show(w: &[String], connect: impl FnOnce() -> Result<Client>) -> Result<()> {
    let args = show_args(w)?;
    let mut out = std::io::stdout().lock();
    if !args.remote {
        match std::fs::read_to_string(&args.file) {
            Ok(text) => return write_rows(&args, &text, &mut out),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("reading {}", args.file)),
        }
    }
    args.check_remote_name()?;
    let mut client = connect()?;
    show_remote(
        &args,
        |params| Ok(client.call(method::TRACE_READ, params)?),
        &mut out,
    )
}

fn write_rows(args: &ShowArgs, text: &str, out: &mut impl Write) -> Result<()> {
    let lines = show_rows(text, &args.filter).map_err(|e| anyhow::anyhow!("{}: {e}", args.file))?;
    for line in lines {
        writeln!(out, "{line}")?;
    }
    Ok(())
}

fn show_remote(
    args: &ShowArgs,
    mut read: impl FnMut(Value) -> Result<Value>,
    out: &mut impl Write,
) -> Result<()> {
    let mut params = TraceReadParams {
        path: args.file.clone(),
        from_tick: args.filter.from,
        to_tick: args.filter.to,
        kind: args.kind,
        ..Default::default()
    };
    loop {
        let page = read(serde_json::to_value(&params)?)
            .with_context(|| format!("reading {} with {}", args.file, method::TRACE_READ))?;
        let page: TraceReadPage = serde_json::from_value(page).context("invalid trace.read page")?;
        if let Some(next) = page.next {
            ensure!(
                next > params.offset.unwrap_or(0),
                "trace.read did not advance the cursor"
            );
        }
        write_rows(args, &page.rows.join("\n"), out)?;
        let Some(next) = page.next else {
            return Ok(());
        };
        params.offset = Some(next);
    }
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
        let args = show_args(&w(
            "f.jsonl --from 10 --to 20 --field physics.crouch --field input",
        ))
        .unwrap();
        assert_eq!(args.file, "f.jsonl");
        assert!(!args.remote);
        assert_eq!(
            args.filter,
            ShowFilter {
                from: Some(10),
                to: Some(20),
                fields: vec!["physics.crouch".into(), "input".into()],
            }
        );
        let args = show_args(&w("--remote f.jsonl --kind flight")).unwrap();
        assert!(args.remote);
        assert!(matches!(args.kind, Some(TraceKind::Flight)));
        assert!(show_args(&w("f.jsonl --kind trace")).unwrap().remote);
        for bad in ["", "--from 3", "f --from x", "f g", "f --loud", "f --kind x"] {
            assert!(show_args(&w(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn remote_show_keeps_filters_and_uses_each_cursor_until_eof() {
        let args = show_args(&w(
            "walk.jsonl --from 12 --to 14 --kind flight --field physics.crouch",
        ))
        .unwrap();
        let row = |tick| {
            json!({"tick": tick, "ticks": tick, "objects": {
                "player.0": {"physics": {"crouch": true}}
            }})
            .to_string()
        };
        let mut steps = [
            (None, json!({"rows": [row(12)], "next": 300})),
            (Some(300), json!({"rows": [], "next": 515})),
            (Some(515), json!({"rows": [row(14)], "next": null})),
        ]
        .into_iter();
        let mut out = Vec::new();
        show_remote(
            &args,
            |params| {
                let (offset, page) = steps.next().expect("one request per page");
                assert_eq!(params.get("offset").and_then(Value::as_u64), offset);
                assert_eq!(params["path"], "walk.jsonl");
                assert_eq!(params["from_tick"], 12);
                assert_eq!(params["to_tick"], 14);
                assert_eq!(params["kind"], "flight");
                Ok(page)
            },
            &mut out,
        )
        .unwrap();
        assert!(steps.next().is_none());
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "tick 12  player.0 physics.crouch=true\ntick 14  player.0 physics.crouch=true\n"
        );
    }

    #[test]
    fn remote_show_rejects_an_unmoving_cursor() {
        let mut calls = 0;
        let error = show_remote(
            &show_args(&w("walk.jsonl --remote")).unwrap(),
            |_| {
                calls += 1;
                assert!(calls <= 2, "a repeated cursor must stop paging");
                Ok(json!({"rows": [], "next": 10}))
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(calls, 2);
        assert!(error.to_string().contains("did not advance"));
    }
}
