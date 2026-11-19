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
  h2 agent <call> [args] [--socket PATH|tcp://HOST:PORT] [--xbox [HOST]] [--timeout SECONDS]
  h2 agent - [--socket PATH]          calls from stdin, one per line, on one connection
  h2 agent watch [phase map bsp pause] print events until the game exits
  h2 agent run <script|-> [--socket PATH]  a `-` script with `expect <pointer> <op>
                                      [JSON]` and `keep` lines, answered as one
                                      JSON verdict; exits 1 unless it passed

A call is a method and JSON params (`session.wait '{\"until\":{\"phase\":\"gameplay\"}}'`)
or a short form:
  status                 session.status
  methods                session.methods
  count [kind|team]      world.count, grouped
  query [JSON]           world.query, e.g. '{\"kind\":[\"actor\"],\"near\":{\"player\":0,\"radius\":20}}'
                         (a unit's forward is where it aims, with yaw and
                         pitch in degrees; body_forward is its body's)
  object <id>            world.object, e.g. actor.e1230045
  counters               profile.counters (frame-stats window)
  capture <N|Nms> [sample]  profile.capture over N game ticks or N ms: frame
                         percentiles, phase totals, spikes over 33.3 ms, a CSV
                         of every frame; `sample` also runs the sampler
                         (profile.sample.*) over it
  features               features.list
  claim | release        take or give up control
  pause [max_ms]         session.pause (needs claim, so use it in a `-` script)
  resume                 session.resume
  step <ticks>           session.step
  wait <JSON until> [timeout_ms]
  record <name> [replace]  input.record: log every controller read to the
                         cache directory's inputs/<name>.input
  replay <name>          input.replay: answer controller reads from that log
  stop                   input.stop: end the recording (writing it) or replay
  sleep <ms>             (scripts only) wait before the next line
  press <b[+b...]> [ms]  input.pad: hold buttons (a b x y black white lt rt up
                         down left right start back ls rs) for ms (default 150)
  stick <left|right> <x> <y> [ms]  input.pad: a stick, -1..1, +y up (no ms: until changed)
  pad <JSON>             input.pad, e.g. '{\"steps\":[{\"buttons\":[\"start\"],\"ms\":100},{\"left\":[0,1],\"ms\":2000}]}'
                         or '{\"player\":1,\"at_tick\":900,\"steps\":[{\"buttons\":[\"rt\"],\"ticks\":30}]}'
  unpress                input.release: let go of everything
  padstate               input.state: every player's pad, recording or replay
  sample-start [hz]      profile.sample.start (the game thread; the Xbox app)
  sample-stop            profile.sample.stop: writes LocalState\\profile.h2prof
  sample-status          profile.sample.status
  hostperf               host.perf: the newest host perf line
  ui                     ui.screen: the menu screen player one's input goes to (H2_UI_WATCH=1)
  frame [draws] [snap N[,M...]]  render.frame: record the next whole frame and
                         print it: passes (target, clears, draws by outcome,
                         primitive, entry, texture formats), copies and reads,
                         native render paths; `draws` lists each draw; `snap`
                         reads passes N... back as PNGs under the cache
                         directory's frames/ (waits up to 10 s for the frame)
  loads [N]              load.timeline: the last N (8) level loads, BSP
                         switches and stalls: stall and settle times, and the
                         bytes read, uploads and compiles in each; a stall
                         going on now as `current`
  fallbacks [N]          profile.fallbacks: the N (20) fallback interpreter
                         entry addresses that interpreted most, with their
                         function, hits and instructions
  render                 render.stats: in-between frames and why they held,
                         the pipelined render, native render path calls
  state save <name> [replace]  state.save: the whole machine at the next tick
                         boundary where nothing is live, under the cache
                         directory's snapshots/ (retries for 90 ticks, then
                         answers BUSY with what stayed live)
  state load <name>      state.load: back to that moment; says whether guest
                         RAM hashes as it did at the save
  state list             state.list: this build's snapshots
  state drop <name>      state.drop
  state fork <from> <name> [replace]  state.fork: load <from>, save it as <name>
{HOT}
{SIM}
{FLIGHT}

Input calls need no claim while nobody holds control; once a connection
does, only it may send them. Queued steps outlive the connection and play out.

The socket is --socket (a path, or tcp://HOST:PORT), else --xbox HOST
(tcp://HOST:4720, the Xbox app; HOST defaults to $H2_XBOX), else $H2_AGENT,
else the cache directory's h2recomp/agent.sock, where `h2 serve` listens
by default. Results print as
JSON, one per line in scripts; an error prints to stderr and exits 1 (a
script stops at its first error).";

/// [`USAGE`] with the hot-iteration forms (`agent_hot_cmd`) in place.
fn usage() -> String {
    USAGE
        .replace("{HOT}", crate::agent_hot_cmd::USAGE)
        .replace("{SIM}", crate::agent_sim::USAGE)
        .replace("{FLIGHT}", crate::agent_flight::USAGE)
}

pub fn run(args: Vec<String>) -> Result<()> {
    let mut socket = None;
    let mut timeout = None;
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--socket" => socket = Some(PathBuf::from(it.next().context("--socket needs a path")?)),
            "--xbox" => {
                // A host may follow; a call never looks like one.
                let host = match it.as_slice().first() {
                    Some(h) if looks_like_host(h) => it.next(),
                    _ => std::env::var("H2_XBOX").ok().filter(|h| !h.is_empty()),
                }
                .context("--xbox needs the console's address (or set H2_XBOX)")?;
                socket = Some(PathBuf::from(xbox_endpoint(&host)));
            }
            "--timeout" => {
                let s: f64 = it
                    .next()
                    .context("--timeout needs seconds")?
                    .parse()
                    .context("--timeout needs seconds")?;
                timeout = Some(
                    Duration::try_from_secs_f64(s)
                        .ok()
                        .filter(|d| !d.is_zero())
                        .context("--timeout needs a positive number of seconds")?,
                );
            }
            "-h" | "--help" => bail!("{}", usage()),
            _ => rest.push(a),
        }
    }
    let Some(first) = rest.first().cloned() else {
        bail!("{}", usage())
    };
    // Reads a file; no game needed.
    if first == "flight" && rest.get(1).is_some_and(|w| w == "show") {
        return crate::agent_flight::show(&rest[2..]);
    }
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
        "run" => run_script(&mut c, &rest[1..], false),
        "sim" if rest.get(1).is_some_and(|w| w == "run") => run_script(&mut c, &rest[2..], true),
        "sim" => {
            let v = sim(&mut c, &rest[1..])?;
            println!("{}", serde_json::to_string_pretty(&v)?);
            Ok(())
        }
        "frame" => {
            let v = frame(&mut c, &rest)?;
            println!("{}", serde_json::to_string_pretty(&v)?);
            Ok(())
        }
        _ => {
            let (m, p) = parse_call(&rest)?;
            let v = call(&mut c, &m, p)?;
            println!("{}", serde_json::to_string_pretty(&v)?);
            Ok(())
        }
    }
}

/// Whether `--xbox`'s next word is an address rather than a call.
fn looks_like_host(w: &str) -> bool {
    !w.starts_with('-') && (w.contains('.') || w.contains(':')) && !w.contains("..") && {
        let head = w.split(':').next().unwrap_or(w);
        head.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            && !agent_proto::method::ALL.contains(&w)
            && !w.starts_with("input.")
            && !w.starts_with("profile.")
            && !w.starts_with("host.")
            && !w.starts_with("world.")
            && !w.starts_with("session.")
            && !w.starts_with("features.")
            && !w.starts_with("load.")
            && !w.starts_with("edit.")
            && !w.starts_with("render.")
            && !w.starts_with("sim.")
            && !w.starts_with("trace.")
            && !w.starts_with("flight.")
    }
}

/// `tcp://HOST:4720` for a console address (a port given wins; an
/// `https://...:11443` Device Portal URL gives its host).
fn xbox_endpoint(host: &str) -> String {
    let h = host
        .split("://")
        .last()
        .unwrap_or(host)
        .trim_end_matches('/');
    let h = h.split('/').next().unwrap_or(h);
    match h.rsplit_once(':') {
        Some((name, port)) if port != "11443" && port.parse::<u16>().is_ok() => {
            format!("tcp://{name}:{port}")
        }
        Some((name, _)) => format!("tcp://{name}:{}", agent_proto::XBOX_PORT),
        None => format!("tcp://{h}:{}", agent_proto::XBOX_PORT),
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
        let v = if words[0] == "frame" {
            frame(c, &words)?
        } else if words[0] == "sim" {
            sim(c, &words[1..])?
        } else {
            let (m, p) = parse_call(&words)?;
            call(c, &m, p)?
        };
        println!("{}", serde_json::to_string(&v)?);
    }
    Ok(())
}

/// The words of a script line (for `agent_run`).
pub(crate) fn split_words(line: &str) -> Result<Vec<String>> {
    split(line)
}

/// A `sim` verb (`agent_sim`); an error prints as `call`'s do.
fn sim(c: &mut Client, words: &[String]) -> Result<Value> {
    match crate::agent_sim::call(c, words) {
        Ok(v) => Ok(v),
        Err(e) => {
            eprintln!("{}", serde_json::to_string_pretty(&e)?);
            std::process::exit(1);
        }
    }
}

/// `h2 agent run`: the script, then its verdict. `sim run` (`sim`) also
/// removes the simulated players once a script has added any.
fn run_script(c: &mut Client, args: &[String], sim: bool) -> Result<()> {
    let [path] = args else {
        bail!("run needs one script file (or - for stdin)")
    };
    let text = if path == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else {
        std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?
    };
    let lines = crate::agent_run::parse(&text, split)?;
    let mut added = false;
    let mut verdict = crate::agent_run::execute(
        &lines,
        |words| {
            if words[0] == "frame" {
                return frame(c, words).map_err(|e| format!("{e:#}"));
            }
            if words[0] == "sim" {
                added |= words.get(1).is_some_and(|w| w == "add");
                return crate::agent_sim::call(c, &words[1..])
                    .map_err(|e| serde_json::to_string(&e).unwrap_or_default());
            }
            let (m, p) = parse_call(words).map_err(|e| format!("{e:#}"))?;
            c.call(&m, p).map_err(|e| match e {
                CallError::Rpc(e) => serde_json::to_string(&e).unwrap_or_default(),
                e => format!("{m}: {e}"),
            })
        },
        |ms| std::thread::sleep(Duration::from_millis(ms)),
    );
    if sim && added {
        verdict["cleanup"] = crate::agent_sim::cleanup(c);
    }
    println!("{}", serde_json::to_string_pretty(&verdict)?);
    if verdict["pass"] != true {
        std::process::exit(1);
    }
    Ok(())
}

/// How long `frame` waits for the frame it armed.
const FRAME_WAIT: Duration = Duration::from_secs(10);

/// `frame`'s arming params from its words.
fn frame_params(words: &[String]) -> Result<Value> {
    let mut p = json!({ "arm": true });
    let mut it = words[1..].iter();
    while let Some(w) = it.next() {
        match w.as_str() {
            "draws" => p["draws"] = json!(true),
            "snap" => {
                let list = it
                    .next()
                    .context("snap needs pass numbers, e.g. snap 0,3")?;
                let passes: Vec<u64> = list
                    .split(',')
                    .map(|n| n.trim().parse::<u64>())
                    .collect::<Result<_, _>>()
                    .with_context(|| format!("snap: pass numbers, not {list}"))?;
                p["snap"] = json!(passes);
            }
            other => bail!("frame: unexpected {other} (draws, snap N[,M...])"),
        }
    }
    Ok(p)
}

/// `render.frame`: arms, then reads until the frame it armed is in.
fn frame(c: &mut Client, words: &[String]) -> Result<Value> {
    let armed = call(c, method::RENDER_FRAME, frame_params(words)?)?;
    let seq = armed["seq"].clone();
    let deadline = std::time::Instant::now() + FRAME_WAIT;
    loop {
        let r = call(c, method::RENDER_FRAME, Value::Null)?;
        if !r["frame"].is_null()
            && r["frame"]["seq"] == seq
            && !r["armed"].as_bool().unwrap_or(false)
        {
            return Ok(r["frame"].clone());
        }
        if std::time::Instant::now() > deadline {
            bail!(
                "frame: no whole frame within {} s (armed at present {}; a paused or loading game presents nothing)",
                FRAME_WAIT.as_secs(),
                armed["armed_at_present"]
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
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

/// `state <save|load|list|drop|fork> ...`.
fn state_call(words: &[String]) -> Result<(String, Value)> {
    use agent_proto::state::method as state;
    const HOW: &str = "state save|load|drop <name>, state fork <from> <name>, or state list";
    let w = |i: usize| words.get(i).map(String::as_str);
    let replace = |i: usize| -> Result<bool> {
        match w(i) {
            None => Ok(false),
            Some("replace") => Ok(true),
            Some(other) => bail!("state: unexpected {other} (only `replace`)"),
        }
    };
    let name = |i: usize| w(i).with_context(|| format!("{HOW}: a name is missing"));
    let (m, p, n) = match w(0) {
        Some("list") => (state::LIST, Value::Null, 1),
        Some("save") => (
            state::SAVE,
            json!({ "name": name(1)?, "replace": replace(2)? }),
            3,
        ),
        Some("load") => (state::LOAD, json!({ "name": name(1)? }), 2),
        Some("drop") => (state::DROP, json!({ "name": name(1)? }), 2),
        Some("fork") => (
            state::FORK,
            json!({ "from": name(1)?, "name": name(2)?, "replace": replace(3)? }),
            4,
        ),
        _ => bail!("{HOW}"),
    };
    if let Some(extra) = words.get(n) {
        bail!("state: unexpected {extra}");
    }
    Ok((m.into(), p))
}

/// A call's method and params from its words.
fn parse_call(words: &[String]) -> Result<(String, Value)> {
    if let Some(hot) = crate::agent_hot_cmd::parse(words) {
        return hot;
    }
    if let Some(r) = crate::agent_flight::parse(words) {
        return r;
    }
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
        "stop" => Some(method::INPUT_STOP),
        "unpress" => Some(method::INPUT_RELEASE),
        "padstate" => Some(method::INPUT_STATE),
        "sample-stop" => Some(method::SAMPLE_STOP),
        "sample-status" => Some(method::SAMPLE_STATUS),
        "hostperf" => Some(method::HOST_PERF),
        "render" => Some(method::RENDER_STATS),
        "ui" => Some(method::UI_SCREEN),
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
        "loads" => (
            method::LOAD_TIMELINE,
            match arg {
                None => Value::Null,
                Some(_) => json!({ "last": number(arg, "loads")? }),
            },
        ),
        "fallbacks" => (
            method::PROFILE_FALLBACKS,
            match arg {
                None => Value::Null,
                Some(_) => json!({ "top": number(arg, "fallbacks")? }),
            },
        ),
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
        "press" => {
            let buttons: Vec<&str> = arg
                .context("press needs buttons, e.g. a or start+a")?
                .split('+')
                .filter(|b| !b.is_empty())
                .collect();
            let ms = match words.get(2) {
                Some(_) => number(words.get(2), "press's ms")?,
                None => 150,
            };
            extra(3)?;
            return Ok((
                method::INPUT_PAD.into(),
                json!({ "steps": [{ "buttons": buttons, "ms": ms }] }),
            ));
        }
        "stick" => {
            let side = arg.context("stick needs left or right")?;
            if side != "left" && side != "right" {
                bail!("stick: left or right, not {side}");
            }
            let axis = |i: usize| -> Result<f64> {
                words
                    .get(i)
                    .context("stick needs x and y, -1 to 1")?
                    .parse::<f64>()
                    .context("stick needs x and y, -1 to 1")
            };
            let mut step = json!({});
            step[side.as_str()] = json!([axis(2)?, axis(3)?]);
            if words.len() > 4 {
                step["ms"] = json!(number(words.get(4), "stick's ms")?);
            }
            extra(5)?;
            return Ok((method::INPUT_PAD.into(), json!({ "steps": [step] })));
        }
        "pad" => {
            let p = json_arg(arg)?;
            if !p.is_object() {
                bail!(
                    "pad needs JSON steps, e.g. '{{\"steps\":[{{\"buttons\":[\"a\"],\"ms\":100}}]}}'"
                );
            }
            (method::INPUT_PAD, p)
        }
        "record" => {
            let name = arg.context("record needs a name")?;
            let p = match words.get(2).map(String::as_str) {
                None => json!({ "name": name }),
                Some("replace") => json!({ "name": name, "replace": true }),
                Some(other) => bail!("record: unexpected {other} (only `replace`)"),
            };
            extra(3)?;
            return Ok((method::INPUT_RECORD.into(), p));
        }
        "replay" => (
            method::INPUT_REPLAY,
            json!({ "name": arg.context("replay needs a name")? }),
        ),
        "capture" => {
            let n = arg.context("capture needs ticks (300) or milliseconds (5000ms)")?;
            let mut p = match n.strip_suffix("ms") {
                Some(ms) => json!({ "ms": number(Some(&ms.to_owned()), "capture")? }),
                None => json!({ "ticks": number(Some(n), "capture")? }),
            };
            match words.get(2).map(String::as_str) {
                None => {}
                Some("sample") => p["sample"] = json!(true),
                Some(other) => bail!("capture: unexpected {other} (only `sample`)"),
            }
            extra(3)?;
            return Ok((method::PROFILE_CAPTURE.into(), p));
        }
        "sample-start" => (
            method::SAMPLE_START,
            match arg {
                None => Value::Null,
                Some(_) => json!({ "hz": number(arg, "sample-start")? }),
            },
        ),
        "state" => return state_call(&words[1..]),
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
        other => bail!("{other}: not a method or short form\n\n{}", usage()),
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
        assert_eq!(
            c(r#"pad {"player": 1, "steps": [{"buttons": ["a"], "ticks": 4}]}"#),
            (
                "input.pad".into(),
                json!({"player": 1, "steps": [{"buttons": ["a"], "ticks": 4}]})
            )
        );
        assert_eq!(c("padstate"), ("input.state".into(), Value::Null));
        assert_eq!(
            c("record fight replace"),
            (
                "input.record".into(),
                json!({"name": "fight", "replace": true})
            )
        );
        assert_eq!(
            c("replay fight"),
            ("input.replay".into(), json!({"name": "fight"}))
        );
        assert_eq!(c("stop"), ("input.stop".into(), Value::Null));
        assert_eq!(
            c("capture 300"),
            ("profile.capture".into(), json!({"ticks": 300}))
        );
        assert_eq!(
            c("capture 5000ms sample"),
            (
                "profile.capture".into(),
                json!({"ms": 5000, "sample": true})
            )
        );

        assert_eq!(
            c("press start+a"),
            (
                "input.pad".into(),
                json!({"steps": [{"buttons": ["start", "a"], "ms": 150}]})
            )
        );
        assert_eq!(
            c("press rt 400"),
            (
                "input.pad".into(),
                json!({"steps": [{"buttons": ["rt"], "ms": 400}]})
            )
        );
        assert_eq!(
            c("stick left 0 1 2000"),
            (
                "input.pad".into(),
                json!({"steps": [{"left": [0.0, 1.0], "ms": 2000}]})
            )
        );
        assert_eq!(
            c("stick right -0.5 0"),
            (
                "input.pad".into(),
                json!({"steps": [{"right": [-0.5, 0.0]}]})
            )
        );
        assert_eq!(c("unpress"), ("input.release".into(), Value::Null));
        assert_eq!(
            c("sample-start 2000"),
            ("profile.sample.start".into(), json!({"hz": 2000}))
        );
        assert_eq!(
            c("sample-stop"),
            ("profile.sample.stop".into(), Value::Null)
        );
        assert_eq!(c("hostperf"), ("host.perf".into(), Value::Null));
        assert_eq!(c("ui"), ("ui.screen".into(), Value::Null));
        assert_eq!(c("render"), ("render.stats".into(), Value::Null));
        assert_eq!(
            c("fallbacks 5"),
            ("profile.fallbacks".into(), json!({"top": 5}))
        );
        assert_eq!(c("fallbacks"), ("profile.fallbacks".into(), Value::Null));
        assert_eq!(c("loads 3"), ("load.timeline".into(), json!({"last": 3})));
        assert_eq!(
            c("state save fight"),
            (
                "state.save".into(),
                json!({"name": "fight", "replace": false})
            )
        );
        assert_eq!(
            c("state save fight replace"),
            (
                "state.save".into(),
                json!({"name": "fight", "replace": true})
            )
        );
        assert_eq!(
            c("state load fight"),
            ("state.load".into(), json!({"name": "fight"}))
        );
        assert_eq!(c("state list"), ("state.list".into(), Value::Null));
        assert_eq!(
            c("state drop fight"),
            ("state.drop".into(), json!({"name": "fight"}))
        );
        assert_eq!(
            c("state fork fight fight.b"),
            (
                "state.fork".into(),
                json!({"from": "fight", "name": "fight.b", "replace": false})
            )
        );
        assert_eq!(frame_params(&words("frame")).unwrap(), json!({"arm": true}));
        assert_eq!(
            frame_params(&words("frame draws snap 0,3")).unwrap(),
            json!({"arm": true, "draws": true, "snap": [0, 3]})
        );
        for bad in ["frame loud", "frame snap", "frame snap x"] {
            assert!(frame_params(&words(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn xbox_addresses_become_tcp_endpoints() {
        assert_eq!(xbox_endpoint("10.0.3.29"), "tcp://10.0.3.29:4720");
        assert_eq!(
            xbox_endpoint("https://10.0.3.29:11443/"),
            "tcp://10.0.3.29:4720"
        );
        assert_eq!(xbox_endpoint("10.0.3.29:5000"), "tcp://10.0.3.29:5000");
        assert!(looks_like_host("10.0.3.29") && looks_like_host("xbox.lan"));
        for call in [
            "status",
            "press",
            "session.status",
            "input.pad",
            "-",
            "watch",
        ] {
            assert!(!looks_like_host(call), "{call}");
        }
    }

    #[test]
    fn mistakes_are_refused() {
        for s in [
            "press",
            "press a x",
            "stick up 0 1",
            "stick left 0",
            "step",
            "step x",
            "wait",
            "object",
            "status now",
            "dance",
            "query {}  extra",
            "pad",
            "pad [1]",
            "record",
            "record a twice",
            "replay",
            "capture",
            "capture soon",
            "capture 30 loud",
            "fallbacks many",
            "state",
            "state save",
            "state load",
            "state list now",
            "state save a twice",
            "state fork a",
            "state nap a",
        ] {
            assert!(parse_call(&words(s)).is_err(), "{s}");
        }
    }
}
