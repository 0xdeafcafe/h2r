//! `h2 agent`: talk to a running game's agent API (docs/runtime/agent-api.md).
//!
//! One call per run, or a script of calls on one connection (`-`), which is
//! what pausing and stepping need: control ends when the connection does.

use std::io::BufRead;
use std::path::PathBuf;
use std::time::Duration;

use agent_proto::{CallError, Client, method};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

const USAGE: &str = "usage:
  h2 agent <call> [args] [--socket PATH] [--timeout SECONDS]
  h2 agent - [--socket PATH]          calls from stdin, one per line, on one connection
  h2 agent watch [phase map bsp pause] print events until the game exits

A call is a method and JSON params (`session.wait '{\"until\":{\"phase\":\"gameplay\"}}'`)
or a short form:
  status                 session.status
  methods                session.methods
  count [kind|team]      world.count, grouped
  query [JSON]           world.query, e.g. '{\"kind\":[\"actor\"],\"near\":{\"player\":0,\"radius\":20}}'
  object <id>            world.object, e.g. actor.e1230045
  counters               profile.counters (frame-stats window)
  features               features.list
  claim | release        take or give up control
  pause [max_ms]         session.pause (needs claim, so use it in a `-` script)
  resume                 session.resume
  step <ticks>           session.step
  wait <JSON until> [timeout_ms]
  sleep <ms>             (scripts only) wait before the next line

The socket is --socket, else $H2_AGENT, else the cache directory's
h2recomp/agent.sock, where `h2 serve` listens by default. Results print as
JSON, one per line in scripts; an error prints to stderr and exits 1 (a
script stops at its first error).";

pub fn run(args: Vec<String>) -> Result<()> {
    let mut socket = None;
    let mut timeout = None;
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--socket" => socket = Some(PathBuf::from(it.next().context("--socket needs a path")?)),
            "--timeout" => {
                let s: f64 = it
                    .next()
                    .context("--timeout needs seconds")?
                    .parse()
                    .context("--timeout needs seconds")?;
                timeout = Some(Duration::from_secs_f64(s));
            }
            "-h" | "--help" => bail!("{USAGE}"),
            _ => rest.push(a),
        }
    }
    let Some(first) = rest.first().cloned() else {
        bail!("{USAGE}")
    };
    let path = socket.unwrap_or_else(agent_proto::default_socket);
    let mut c = Client::connect(&path).with_context(|| {
        format!(
            "connecting to {}: is `h2 serve` (or a game with H2_AGENT) running?",
            path.display()
        )
    })?;
    c.set_timeout(timeout)?;
    match first.as_str() {
        "-" => script(&mut c),
        "watch" => watch(&mut c, &rest[1..]),
        _ => {
            let (m, p) = parse_call(&rest)?;
            let v = call(&mut c, &m, p)?;
            println!("{}", serde_json::to_string_pretty(&v)?);
            Ok(())
        }
    }
}

fn call(c: &mut Client, m: &str, p: Value) -> Result<Value> {
    match c.call(m, p) {
        Ok(v) => Ok(v),
        Err(CallError::Rpc(e)) => {
            eprintln!("{}", serde_json::to_string_pretty(&e)?);
            std::process::exit(1);
        }
        Err(e) => Err(anyhow::anyhow!("{m}: {e}")),
    }
}

fn script(c: &mut Client) -> Result<()> {
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let words = split(line)?;
        if words[0] == "sleep" {
            let ms: u64 = words.get(1).context("sleep needs milliseconds")?.parse()?;
            std::thread::sleep(Duration::from_millis(ms));
            continue;
        }
        let (m, p) = parse_call(&words)?;
        let v = call(c, &m, p)?;
        println!("{}", serde_json::to_string(&v)?);
    }
    Ok(())
}

fn watch(c: &mut Client, kinds: &[String]) -> Result<()> {
    let kinds: Vec<Value> = if kinds.is_empty() {
        ["phase", "map", "bsp", "pause"]
            .into_iter()
            .map(Value::from)
            .collect()
    } else {
        kinds.iter().map(|k| Value::from(k.as_str())).collect()
    };
    call(c, method::SUBSCRIBE, json!({ "events": kinds }))?;
    c.set_timeout(None)?;
    loop {
        match c.next_event() {
            Ok(e) => println!("{}", serde_json::to_string(&e)?),
            Err(CallError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => bail!("watch: {e}"),
        }
    }
}

/// Splits a script line into words; a JSON value (`{…}` or `[…]`) is one
/// word whatever spaces it holds.
fn split(line: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut rest = line.trim_start();
    while !rest.is_empty() {
        if rest.starts_with('{') || rest.starts_with('[') {
            let mut de = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
            de.next()
                .context("empty JSON")?
                .with_context(|| format!("bad JSON in {line}"))?;
            let n = de.byte_offset();
            out.push(rest[..n].to_owned());
            rest = rest[n..].trim_start();
        } else {
            let n = rest.find(char::is_whitespace).unwrap_or(rest.len());
            out.push(rest[..n].to_owned());
            rest = rest[n..].trim_start();
        }
    }
    Ok(out)
}

fn json_arg(s: Option<&String>) -> Result<Value> {
    match s {
        None => Ok(Value::Null),
        Some(s) => serde_json::from_str(s).with_context(|| format!("not JSON: {s}")),
    }
}

fn number(s: Option<&String>, what: &str) -> Result<u64> {
    s.with_context(|| format!("{what} needs a number"))?
        .parse()
        .with_context(|| format!("{what} needs a number"))
}

/// A call's method and params from its words.
fn parse_call(words: &[String]) -> Result<(String, Value)> {
    let name = words[0].as_str();
    let arg = words.get(1);
    let extra = |n: usize| -> Result<()> {
        if words.len() > n {
            bail!("{name}: unexpected {}", words[n]);
        }
        Ok(())
    };
    let bare = match name {
        "status" => Some(method::STATUS),
        "methods" => Some(method::METHODS),
        "counters" => Some(method::PROFILE_COUNTERS),
        "features" => Some(method::FEATURES),
        "claim" => Some(method::CLAIM),
        "release" => Some(method::RELEASE),
        "resume" => Some(method::RESUME),
        _ => None,
    };
    if let Some(m) = bare {
        extra(1)?;
        return Ok((m.into(), Value::Null));
    }
    let (m, p) = match name {
        "count" => (
            method::WORLD_COUNT,
            match arg {
                None => Value::Null,
                Some(g) => json!({ "group_by": g }),
            },
        ),
        "query" => (method::WORLD_QUERY, json_arg(arg)?),
        "object" => (
            method::WORLD_OBJECT,
            json!({ "id": arg.context("object needs an id, e.g. actor.e1230045")? }),
        ),
        "pause" => (
            method::PAUSE,
            match arg {
                None => Value::Null,
                Some(_) => json!({ "max_ms": number(arg, "pause")? }),
            },
        ),
        "step" => (method::STEP, json!({ "ticks": number(arg, "step")? })),
        "wait" => {
            let until = json_arg(arg)?;
            if until.is_null() {
                bail!("wait needs a JSON condition, e.g. '{{\"phase\":\"gameplay\"}}'");
            }
            let mut p = json!({ "until": until });
            if words.len() > 2 {
                p["timeout_ms"] = json!(number(words.get(2), "wait's timeout")?);
            }
            extra(3)?;
            return Ok((method::WAIT.into(), p));
        }
        m if m.contains('.') => {
            extra(2)?;
            return Ok((m.into(), json_arg(arg)?));
        }
        other => bail!("{other}: not a method or short form\n\n{USAGE}"),
    };
    extra(2)?;
    Ok((m.into(), p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        split(s).unwrap()
    }

    #[test]
    fn json_stays_one_word() {
        assert_eq!(
            words(r#"query {"kind": ["actor"], "limit": 5}"#),
            ["query", r#"{"kind": ["actor"], "limit": 5}"#]
        );
        assert_eq!(words("  step   3 "), ["step", "3"]);
        assert!(split("query {oops").is_err());
    }

    #[test]
    fn short_forms_become_methods() {
        let c = |s: &str| parse_call(&words(s)).unwrap();
        assert_eq!(c("status"), ("session.status".into(), Value::Null));
        assert_eq!(
            c("count team"),
            ("world.count".into(), json!({"group_by": "team"}))
        );
        assert_eq!(
            c("object actor.e1230045"),
            ("world.object".into(), json!({"id": "actor.e1230045"}))
        );
        assert_eq!(c("step 30"), ("session.step".into(), json!({"ticks": 30})));
        assert_eq!(
            c("pause 5000"),
            ("session.pause".into(), json!({"max_ms": 5000}))
        );
        assert_eq!(
            c(r#"wait {"phase": "gameplay"} 9000"#),
            (
                "session.wait".into(),
                json!({"until": {"phase": "gameplay"}, "timeout_ms": 9000})
            )
        );
        assert_eq!(
            c(r#"world.query {"limit": 1}"#),
            ("world.query".into(), json!({"limit": 1}))
        );
    }

    #[test]
    fn mistakes_are_refused() {
        for s in [
            "step",
            "step x",
            "wait",
            "object",
            "status now",
            "dance",
            "query {}  extra",
        ] {
            assert!(parse_call(&words(s)).is_err(), "{s}");
        }
    }
}
