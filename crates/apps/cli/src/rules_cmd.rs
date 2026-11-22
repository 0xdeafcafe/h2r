//! `h2 rules check|build|run|sim|caps|builtin|lsp`: the rules language's tooling
//! (docs/game/rules-engine.md, "Security and usability"). Every command is pure over its
//! inputs: no game, no disc. The `*_text` functions build what a command
//! prints, so tests read it without a process.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rules::caps::{self, Kind, Status};
use rules::ir::Scope;
use rules::scenario::{self, Scenario};
use rules::sim;
use rules::vm::TICK_BUDGET;
use rules::{BuildError, Ir, Report, builtin};

const USAGE: &str = "usage: h2 rules check <file.rules>
       h2 rules build <file.rules> -o <out.rir>
       h2 rules run <file.rules> --scenario <scenario.json> [--trace]
       h2 rules sim <file.rules> [--players N] [--teams N] [--seed S]
                    [--matches N] [--length SECONDS] [--score-limit N]
                    [--json] [-o <scenario.json>]
       h2 rules caps
       h2 rules builtin [name]
       h2 rules lsp
  check compiles and verifies; errors print as file:line:col: message and
  the exit status is 1. The report gives the worst-tick estimate against
  the budget, the IR size, the slots, what the script can do, and any
  entries a host cannot make; a script over the budget or the IR cap
  prints the report with a refusal and exits 1 too.
  build writes the verified IR blob and prints its SHA-256. It writes
  nothing for a script check refuses.
  run steps the script over a JSON scenario on a fake game and prints the
  calls made each tick and the final scores (--trace adds each event and
  call).
  sim plays a generated match of the script on the fake game until the
  game ends: made-up players, the events the script handles, the same
  match for the same seed. --matches plays that many seeds and sums them
  up (who wins, how games end, the worst tick, warnings). -o writes the
  match as a scenario file for run.
  caps lists the capabilities and the engine API entries with their status.
  builtin lists the built-in scripts (the engine ports) and the example
  scripts, or prints one by name, engine index or example key.
  lsp serves the Language Server Protocol on stdin and stdout, for an
  editor: diagnostics, completion, hover, signatures, definitions,
  references, semantic tokens, the outline and formatting.";

pub fn run(words: Vec<String>) -> Result<()> {
    let mut it = words.into_iter();
    let Some(sub) = it.next() else {
        bail!("{USAGE}")
    };
    let rest: Vec<String> = it.collect();
    match sub.as_str() {
        "-h" | "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        "check" => check(&rest),
        "build" => build(&rest),
        "run" => run_scenario(&rest),
        "sim" => simulate(&rest),
        "lsp" => crate::rules_lsp::run(),
        "caps" => {
            print!("{}", caps_text());
            Ok(())
        }
        "builtin" => match builtin_text(rest.first().map(String::as_str)) {
            Ok(text) => {
                print!("{text}");
                Ok(())
            }
            Err(message) => bail!("{message}"),
        },
        _ => bail!("{USAGE}"),
    }
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

/// One positional path, and the flags the command takes.
struct Parsed {
    file: PathBuf,
    out: Option<PathBuf>,
    scenario: Option<PathBuf>,
    trace: bool,
}

fn parse(words: &[String]) -> Result<Parsed> {
    let mut file = None;
    let mut out = None;
    let mut scenario = None;
    let mut trace = false;
    let mut it = words.iter();
    while let Some(w) = it.next() {
        match w.as_str() {
            "-o" | "--out" => {
                out = Some(PathBuf::from(it.next().context("-o requires a path")?));
            }
            "--scenario" => {
                scenario = Some(PathBuf::from(
                    it.next().context("--scenario requires a path")?,
                ));
            }
            "--trace" => trace = true,
            f if !f.starts_with('-') && file.is_none() => file = Some(PathBuf::from(f)),
            _ => bail!("{USAGE}"),
        }
    }
    Ok(Parsed {
        file: file.context(USAGE)?,
        out,
        scenario,
        trace,
    })
}

fn check(words: &[String]) -> Result<()> {
    let p = parse(words)?;
    let label = p.file.display().to_string();
    match check_text(&label, &read(&p.file)?) {
        Ok(text) => {
            print!("{text}");
            Ok(())
        }
        Err(text) => {
            eprint!("{text}");
            std::process::exit(1);
        }
    }
}

fn build(words: &[String]) -> Result<()> {
    let p = parse(words)?;
    let out = p.out.clone().context("build requires -o <out.rir>")?;
    if out == p.file {
        bail!("the output {} is the source file", out.display());
    }
    let label = p.file.display().to_string();
    match build_blob(&label, &read(&p.file)?) {
        Ok((bytes, hash)) => {
            std::fs::write(&out, &bytes).with_context(|| format!("writing {}", out.display()))?;
            println!("{hash}  {}  ({} bytes)", out.display(), bytes.len());
            Ok(())
        }
        Err(text) => {
            eprint!("{text}");
            std::process::exit(1);
        }
    }
}

fn run_scenario(words: &[String]) -> Result<()> {
    let p = parse(words)?;
    let path = p.scenario.clone().context("run requires --scenario <file>")?;
    let label = p.file.display().to_string();
    match run_text(&label, &read(&p.file)?, &read(&path)?, p.trace) {
        Ok((text, ok)) => {
            print!("{text}");
            if !ok {
                std::process::exit(1);
            }
            Ok(())
        }
        Err(text) => {
            eprint!("{text}");
            std::process::exit(1);
        }
    }
}

/// What `h2 rules sim` was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimArgs {
    pub setup: sim::Setup,
    pub matches: u32,
    pub json: bool,
}

impl Default for SimArgs {
    fn default() -> SimArgs {
        SimArgs {
            setup: sim::Setup::default(),
            matches: 1,
            json: false,
        }
    }
}

/// The file, the arguments and where the scenario goes.
fn parse_sim(words: &[String]) -> Result<(PathBuf, SimArgs, Option<PathBuf>)> {
    let mut file = None;
    let mut out = None;
    let mut args = SimArgs::default();
    let mut it = words.iter();
    fn number<T: std::str::FromStr>(flag: &str, v: Option<&String>) -> Result<T> {
        let v = v.with_context(|| format!("{flag} requires a number"))?;
        v.parse()
            .map_err(|_| anyhow::anyhow!("{flag} takes a number, not {v}"))
    }
    while let Some(w) = it.next() {
        match w.as_str() {
            "--players" => args.setup.players = number(w, it.next())?,
            "--teams" => args.setup.teams = number(w, it.next())?,
            "--seed" => args.setup.seed = number(w, it.next())?,
            "--matches" => args.matches = number(w, it.next())?,
            "--length" => args.setup.length = Some(number(w, it.next())?),
            "--score-limit" => args.setup.score_limit = Some(number(w, it.next())?),
            "--json" => args.json = true,
            "-o" | "--out" => {
                out = Some(PathBuf::from(it.next().context("-o requires a path")?));
            }
            f if !f.starts_with('-') && file.is_none() => file = Some(PathBuf::from(f)),
            _ => bail!("{USAGE}"),
        }
    }
    Ok((file.context(USAGE)?, args, out))
}

fn simulate(words: &[String]) -> Result<()> {
    let (file, args, out) = parse_sim(words)?;
    if out.as_ref() == Some(&file) {
        bail!("the output {} is the source file", file.display());
    }
    let label = file.display().to_string();
    match sim_text(&label, &read(&file)?, &args) {
        Ok(done) => {
            print!("{}", done.text);
            if let Some(path) = out {
                std::fs::write(&path, &done.scenario)
                    .with_context(|| format!("writing {}", path.display()))?;
                eprintln!("wrote the match's scenario to {}", path.display());
                if !done.left_out.is_empty() {
                    eprintln!("the scenario file has no place for: {}", done.left_out.join(", "));
                }
            }
            if !done.ok {
                std::process::exit(1);
            }
            Ok(())
        }
        Err(text) => {
            eprint!("{text}");
            std::process::exit(1);
        }
    }
}

/// What `h2 rules sim` made.
#[derive(Clone, Debug)]
pub struct SimOut {
    /// What it prints: text, or JSON with `--json`.
    pub text: String,
    /// The shown match as a scenario file (`rules::scenario`).
    pub scenario: String,
    /// What the scenario file has no place for.
    pub left_out: Vec<String>,
    /// No match stopped on a script error.
    pub ok: bool,
}

/// A tick as match time: `3:12.4`.
fn clock(tick: u32) -> String {
    let tenths = u64::from(tick) * 10 / caps::TICKS_PER_SECOND as u64;
    format!("{}:{:02}.{}", tenths / 600, tenths / 10 % 60, tenths % 10)
}

fn times<T>(list: &[T], words: impl Fn(&T) -> (String, u32)) -> String {
    list.iter()
        .map(|x| {
            let (name, n) = words(x);
            format!("{name} x{n}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Plays the match (and the batch, for more than one) `h2 rules sim`
/// prints.
pub fn sim_text(label: &str, src: &str, args: &SimArgs) -> Result<SimOut, String> {
    let (ir, _) = compile(label, src)?;
    let refuse = |e: rules::workbench::RunError| match e {
        rules::workbench::RunError::Scenario(m) => format!("{label}: {m}\n"),
        rules::workbench::RunError::Script(problems) => problems
            .iter()
            .map(|p| format!("{label}: {}\n", p.message))
            .collect(),
    };
    let m = sim::simulate(src, &args.setup).map_err(refuse)?;
    let batch = if args.matches == 1 {
        None
    } else {
        Some(sim::batch(src, &args.setup, args.matches).map_err(refuse)?)
    };
    let (file, left_out) = sim::to_file(&m.scenario);
    let scenario = serde_json::to_string(&file).map_err(|e| format!("{label}: {e}\n"))?;
    let ok = m.end != sim::End::Error && batch.as_ref().is_none_or(|b| b.summary.errors.is_empty());

    if args.json {
        let scores: Vec<_> = m
            .trace
            .scores
            .iter()
            .map(|(slot, score)| serde_json::json!({"slot": slot, "score": score}))
            .collect();
        let value = serde_json::json!({
            "name": ir.header.name,
            "seed": m.seed,
            "players": args.setup.players,
            "teams": m.teams,
            "end": m.end,
            "why": m.why,
            "winner": m.winner,
            "top_score": m.top_score,
            "ticks": m.trace.last_tick,
            "ops": m.trace.ops,
            "worst_tick_ops": m.trace.worst_tick_ops,
            "handlers_ran": m.trace.handlers_ran,
            "scores": scores,
            "summary": batch.as_ref().map(|b| &b.summary),
            "rows": batch.as_ref().map(|b| &b.rows),
            "scenario": file,
        });
        let mut text = serde_json::to_string_pretty(&value).map_err(|e| format!("{label}: {e}\n"))?;
        text.push('\n');
        return Ok(SimOut {
            text,
            scenario,
            left_out,
            ok,
        });
    }

    let mut s = String::new();
    let _ = writeln!(
        s,
        "{label}: \"{}\", {} {}, {}, seed {}",
        ir.header.name,
        args.setup.players,
        if args.setup.players == 1 { "player" } else { "players" },
        match m.teams {
            0 => "free for all".to_string(),
            n => format!("{n} teams"),
        },
        m.seed
    );
    let _ = writeln!(
        s,
        "ended at {} (tick {}): {}{}",
        clock(m.trace.last_tick),
        m.trace.last_tick,
        m.end.words(),
        m.why.as_ref().map(|w| format!(" ({w})")).unwrap_or_default()
    );
    let _ = writeln!(s, "winner: {} (top score {})", m.winner.words(), m.top_score);
    let team_of: std::collections::BTreeMap<u8, u8> =
        m.scenario.players.iter().map(|p| (p.slot, p.team)).collect();
    for (slot, score) in &m.trace.scores {
        let _ = writeln!(
            s,
            "player {slot}: team {}, score {score}",
            team_of.get(slot).copied().unwrap_or(0)
        );
    }
    let _ = writeln!(
        s,
        "handlers run: {}",
        if m.trace.handlers_ran.is_empty() {
            "none".to_string()
        } else {
            m.trace.handlers_ran.join(", ")
        }
    );
    let _ = writeln!(
        s,
        "worst tick: {} ops of {} budget; {} events, {} ops in all",
        m.trace.worst_tick_ops,
        TICK_BUDGET,
        m.scenario.steps.len(),
        m.trace.ops
    );

    if let Some(b) = &batch {
        let sum = &b.summary;
        let _ = writeln!(
            s,
            "\n{} matches, seeds {} to {}",
            sum.ran,
            args.setup.seed,
            args.setup.seed.wrapping_add(u64::from(sum.ran.max(1)) - 1)
        );
        if let Some(why) = &sum.stopped {
            let _ = writeln!(s, "stopped early: {why}");
        }
        let _ = writeln!(s, "winners: {}", times(&sum.wins, |w| (w.winner.words(), w.matches)));
        let _ = writeln!(
            s,
            "ended: {}",
            times(&sum.ends, |e| (e.end.words().to_string(), e.matches))
        );
        let _ = writeln!(
            s,
            "winning score: average {:.1}, highest {}",
            sum.avg_score, sum.max_score
        );
        let _ = writeln!(
            s,
            "length: average {}, longest {}",
            clock(sum.avg_ticks),
            clock(sum.max_ticks)
        );
        let _ = writeln!(
            s,
            "worst tick: {} ops of {} budget (seed {})",
            sum.worst_tick_ops, sum.budget, sum.worst_tick_seed
        );
        if sum.flags.is_empty() {
            let _ = writeln!(s, "warnings: none");
        } else {
            let _ = writeln!(s, "warnings:");
            for f in &sum.flags {
                let _ = writeln!(s, "  {} (seed {})", f.text, f.seed);
            }
        }
        if !sum.errors.is_empty() {
            let _ = writeln!(s, "errors:");
            for e in &sum.errors {
                let _ = writeln!(s, "  seed {}: {}", e.seed, e.message);
            }
        }
    }
    Ok(SimOut {
        text: s,
        scenario,
        left_out,
        ok,
    })
}

/// Compiles and verifies source. Errors are `label:line:col: message`
/// lines.
fn compile(label: &str, src: &str) -> Result<(Ir, Report), String> {
    rules::build(src).map_err(|e| match e {
        BuildError::Compile(c) => format!("{label}:{}:{}: {}\n", c.line, c.col, c.message),
        BuildError::Refused(r) => format!("{label}: refused: {r}\n"),
        other => format!("{label}: refused: {other}\n"),
    })
}

/// The report `h2 rules check` prints for source that builds.
pub fn check_text(label: &str, src: &str) -> Result<String, String> {
    let (ir, report) = compile(label, src)?;
    let blob = ir.encode();
    let mut s = String::new();
    let h = &ir.header;
    let _ = writeln!(
        s,
        "{label}: \"{}\" version {}, carrier engine {}, {}",
        h.name,
        h.version,
        h.carrier,
        match h.teams {
            0 => "any number of teams".to_string(),
            n => format!("{n} teams"),
        }
    );
    let _ = writeln!(
        s,
        "worst tick: {} ops of {} budget ({})",
        report.worst_tick,
        TICK_BUDGET,
        if report.fits() { "fits" } else { "OVER BUDGET" }
    );
    if let Some(i) = report.costliest
        && let Some(hd) = ir.handlers.get(i)
    {
        let _ = writeln!(
            s,
            "costliest handler: on {} ({} ops a run)",
            hd.event.name(),
            report.handler_ops.get(i).copied().unwrap_or(0)
        );
    }
    let _ = writeln!(
        s,
        "IR: {} bytes of {}",
        blob.len(),
        rules::ir::MAX_IR_BYTES
    );
    let count = |scope: Scope| ir.slots.iter().filter(|x| x.scope == scope).count();
    let _ = writeln!(
        s,
        "slots: {} global, {} per player, {} per team, {} per object",
        count(Scope::Global),
        count(Scope::Player),
        count(Scope::Team),
        count(Scope::Object)
    );
    let _ = writeln!(s, "id: {}", ir.id());
    if h.requires.is_empty() {
        let _ = writeln!(
            s,
            "can do: core only (scores, reads of the game; no powers)"
        );
    } else {
        let _ = writeln!(s, "can do:");
        for name in &h.requires {
            match caps::capability(name, h.api_version) {
                Some(c) => {
                    let _ = writeln!(s, "  {} - {}", c.name, c.words);
                }
                None => {
                    let _ = writeln!(s, "  {name} - unknown");
                }
            }
        }
    }
    let bad = unhostable(&ir);
    if bad.is_empty() {
        let _ = writeln!(s, "unhostable entries: none");
    } else {
        let _ = writeln!(s, "unhostable entries (a host refuses this script):");
        for e in bad {
            let _ = writeln!(s, "  {} ({})", caps::spelled(e), status_words(e.status));
        }
    }
    // Over the budget or the IR cap: the report, then the refusal.
    if let Err(e) = rules::fits(&ir, &report) {
        let _ = writeln!(s, "{label}: refused: {e}");
        return Err(s);
    }
    Ok(s)
}

/// Every entry the blob calls that a host cannot make, once each, in
/// entry order.
fn unhostable(ir: &Ir) -> Vec<&'static caps::Entry> {
    let mut out: Vec<&'static caps::Entry> = Vec::new();
    for hd in &ir.handlers {
        for op in &hd.code {
            if let rules::ir::Op::Call { entry, .. } = op
                && let Some(e) = caps::entry(*entry, ir.header.api_version)
                && !e.status.hostable()
                && !out.iter().any(|o| o.id == e.id)
            {
                out.push(e);
            }
        }
    }
    out.sort_by_key(|e| e.id);
    out
}

fn status_words(s: Status) -> &'static str {
    match s {
        Status::Named => "named in the engine",
        Status::Traced => "traced, no named point yet",
        Status::Untraced => "untraced",
        Status::NotBuilt => "game side not built",
        Status::VmOnly => "answered by the VM",
    }
}

/// The blob and its SHA-256 for source that builds.
pub fn build_blob(label: &str, src: &str) -> Result<(Vec<u8>, String), String> {
    let (ir, report) = compile(label, src)?;
    rules::fits(&ir, &report).map_err(|e| format!("{label}: refused: {e}\n"))?;
    let bytes = ir.encode();
    let hash = rules::Sha256::of(&bytes).to_string();
    Ok((bytes, hash))
}

/// What `h2 rules run` prints, and whether the run ended without a
/// script error.
pub fn run_text(
    label: &str,
    src: &str,
    scenario_json: &str,
    trace: bool,
) -> Result<(String, bool), String> {
    let (ir, _) = compile(label, src)?;
    let scenario: Scenario =
        serde_json::from_str(scenario_json).map_err(|e| format!("scenario: {e}\n"))?;
    let result = scenario::run(ir, &scenario).map_err(|e| format!("{label}: refused: {e}\n"))?;
    let mut s = String::new();
    for t in &result.ticks {
        let _ = writeln!(s, "tick {}: {} calls", t.tick, t.calls.len());
        if trace {
            for (event, fired) in &t.fired {
                let _ = writeln!(s, "  on {event}: {fired:?}");
            }
            for c in &t.calls {
                let _ = writeln!(s, "  {}({})", c.entry, args_text(&c.args));
            }
        }
    }
    let _ = writeln!(s, "total: {} calls over {} ticks", result.total_calls(), result.ticks.len());
    for (slot, p) in &result.game.players {
        let _ = writeln!(
            s,
            "player {slot}: team {}, score {}, lives {}, {}",
            p.team,
            p.score,
            p.lives,
            if p.alive { "alive" } else { "dead" }
        );
    }
    let mut teams = result.game.teams.clone();
    teams.sort_unstable();
    for t in teams {
        let total: i32 = result
            .game
            .players
            .values()
            .filter(|p| p.team == t)
            .map(|p| p.score)
            .sum();
        let _ = writeln!(s, "team {t}: score {total}");
    }
    match (result.script_error, result.ended_at) {
        (Some(e), _) => {
            let _ = writeln!(
                s,
                "result: script error {:?} at tick {}, handler {}, op {}",
                e.kind, e.tick, e.handler, e.pc
            );
        }
        (None, Some(t)) => {
            let _ = writeln!(s, "result: the script ended the round at tick {t}");
        }
        (None, None) => {
            let _ = writeln!(s, "result: the round was still running");
        }
    }
    Ok((s, result.script_error.is_none()))
}

fn args_text(args: &[rules::vm::Arg]) -> String {
    args.iter()
        .map(|a| format!("{a:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The capability list and the API entry table.
pub fn caps_text() -> String {
    let mut s = String::new();
    let _ = writeln!(s, "API version {}", caps::API_VERSION);
    let _ = writeln!(s, "\ncapabilities (a script declares them in `requires`):");
    for c in caps::CAPABILITIES {
        let _ = writeln!(
            s,
            "  {:<16} {}{}",
            c.name,
            c.words,
            if c.grantable { " (grantable)" } else { "" }
        );
    }
    let _ = writeln!(s, "\nAPI entries:");
    for e in caps::ENTRIES {
        let args = e.args.iter().map(|t| t.name()).collect::<Vec<_>>().join(", ");
        let result = e.result.map_or(String::new(), |t| format!(" -> {}", t.name()));
        let kind = match e.kind {
            Kind::Read => "read",
            Kind::Action => "action",
            Kind::VmOnly => "vm",
        };
        let _ = writeln!(
            s,
            "  {:>2} {}({args}){result}  [{kind}, {}, {}{}]",
            e.id,
            caps::spelled(e),
            e.cap.unwrap_or("core"),
            status_words(e.status),
            if e.status.hostable() { "" } else { ", not hostable" }
        );
    }
    s
}

/// The built-in scripts: the engine ports by engine index, then the
/// example scripts by key; or one source by name, engine index or key.
pub fn builtin_text(which: Option<&str>) -> Result<String, String> {
    let mut found = Vec::new();
    for index in 0..=u8::MAX {
        let Some(src) = builtin::builtin(index) else {
            continue;
        };
        let name = rules::compile(src)
            .map(|ir| ir.header.name)
            .map_err(|e| format!("built-in {index}: {e}\n"))?;
        found.push((index.to_string(), name, src, false));
    }
    for &(key, src) in builtin::examples() {
        let name = rules::compile(src)
            .map(|ir| ir.header.name)
            .map_err(|e| format!("example {key}: {e}\n"))?;
        found.push((key.to_string(), name, src, true));
    }
    let mut s = String::new();
    match which {
        None => {
            for (key, name, _, example) in &found {
                if *example {
                    let _ = writeln!(s, "example {key}: {name}");
                } else {
                    let _ = writeln!(s, "engine {key}: {name}");
                }
            }
        }
        Some(w) => {
            let Some((_, _, src, _)) = found
                .iter()
                .find(|(k, n, _, _)| n.eq_ignore_ascii_case(w) || k == w)
            else {
                return Err(format!("no built-in script {w:?}; `h2 rules builtin` lists them\n"));
            };
            s.push_str(src);
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
rules "Count" 1
carrier slayer

on kill(victim, killer) {
    if killer == none or killer == victim { score(victim, -1) } else { score(killer, 1) }
}
"#;

    const SCENARIO: &str = r#"{
        "players": [{"slot": 0, "team": 0}, {"slot": 1, "team": 1}],
        "ticks": [{}, {"events": [{"kill": {"victim": 1, "killer": 0}}]}]
    }"#;

    #[test]
    fn a_compile_error_prints_as_file_line_col_message() {
        let err = check_text("x.rules", "rules \"A\" 1\ncarrier slayer\non kill(a, b) { nope(a) }\n")
            .unwrap_err();
        let first = err.lines().next().unwrap();
        let mut parts = first.splitn(4, ':');
        assert_eq!(parts.next(), Some("x.rules"));
        assert!(parts.next().unwrap().parse::<u32>().is_ok(), "{first}");
        assert!(parts.next().unwrap().parse::<u32>().is_ok(), "{first}");
        assert!(!parts.next().unwrap().trim().is_empty());
    }

    #[test]
    fn the_report_names_budget_size_and_powers() {
        let text = check_text("x.rules", GOOD).unwrap();
        assert!(text.contains("worst tick:"), "{text}");
        assert!(text.contains("budget (fits)"), "{text}");
        assert!(text.contains("IR: "), "{text}");
        assert!(text.contains("can do: core only"), "{text}");
        assert!(text.contains("unhostable entries: none"), "{text}");
    }

    #[test]
    fn the_hash_is_stable_and_names_the_bytes() {
        let (a, ha) = build_blob("x", GOOD).unwrap();
        let (b, hb) = build_blob("x", GOOD).unwrap();
        assert_eq!(a, b);
        assert_eq!(ha, hb);
        assert_eq!(ha.len(), 64);
        assert_eq!(ha, rules::Sha256::of(&a).to_string());
        assert!(rules::load(&a).is_ok());
    }

    #[test]
    fn a_run_counts_calls_per_tick_and_scores() {
        let (text, ok) = run_text("x", GOOD, SCENARIO, true).unwrap();
        assert!(ok);
        assert!(text.contains("tick 0: 0 calls"), "{text}");
        assert!(text.contains("tick 1: 1 calls"), "{text}");
        assert!(text.contains("score("), "{text}");
        assert!(text.contains("player 0: team 0, score 1"), "{text}");
        assert!(text.contains("team 0: score 1"), "{text}");
        assert!(text.contains("round was still running"), "{text}");
    }

    #[test]
    fn sim_plays_a_match_to_its_end_and_the_same_one_again() {
        let args = SimArgs {
            setup: sim::Setup { players: 6, teams: 2, seed: 3, ..sim::Setup::default() },
            ..SimArgs::default()
        };
        let a = sim_text("x.rules", rules::builtin::SLAYER, &args).unwrap();
        assert!(a.ok);
        assert!(a.text.starts_with("x.rules: \"Slayer\", 6 players, 2 teams, seed 3\n"), "{}", a.text);
        assert!(a.text.contains("\nended at "), "{}", a.text);
        assert!(a.text.contains("\nwinner: "), "{}", a.text);
        assert!(a.text.contains("\nplayer 5: team 1, score "), "{}", a.text);
        assert!(a.text.contains("\nhandlers run: kill\n"), "{}", a.text);
        assert!(!a.text.contains("matches, seeds"), "{}", a.text);
        let b = sim_text("x.rules", rules::builtin::SLAYER, &args).unwrap();
        assert_eq!((a.text.as_str(), a.scenario.as_str()), (b.text.as_str(), b.scenario.as_str()));

        // The scenario it writes is one `run` reads.
        let file: Scenario = serde_json::from_str(&a.scenario).unwrap();
        assert_eq!(file.seed, 3);
        assert_eq!(file.players.len(), 6);
        let (text, ok) = run_text("x.rules", rules::builtin::SLAYER, &a.scenario, false).unwrap();
        assert!(ok, "{text}");
        assert!(a.left_out.is_empty(), "{:?}", a.left_out);
        assert_eq!(file.variant["score_limit"], 25);
        assert!(file.players.iter().all(|p| p.unit.is_some()));
    }

    #[test]
    fn sim_sums_up_a_batch() {
        let args = SimArgs {
            setup: sim::Setup { seed: 10, ..sim::Setup::default() },
            matches: 5,
            json: false,
        };
        let out = sim_text("x.rules", rules::builtin::SLAYER, &args).unwrap();
        assert!(out.text.contains("\n5 matches, seeds 10 to 14\n"), "{}", out.text);
        assert!(out.text.contains("\nwinners: "), "{}", out.text);
        assert!(out.text.contains("\nended: "), "{}", out.text);
        assert!(out.text.contains("\nwinning score: average "), "{}", out.text);
        assert!(out.text.contains(" ops of 20000 budget (seed 1"), "{}", out.text);
        assert!(out.text.contains("\nwarnings:"), "{}", out.text);

        let json = sim_text("x.rules", rules::builtin::SLAYER, &SimArgs { json: true, ..args }).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json.text).unwrap();
        assert_eq!(v["name"], "Slayer");
        assert_eq!(v["seed"], 10);
        assert_eq!(v["summary"]["ran"], 5);
        assert_eq!(v["rows"].as_array().unwrap().len(), 5);
        assert_eq!(v["rows"][0]["winner"], v["winner"]);
        assert!(v["scenario"]["ticks"].as_array().unwrap().len() > 1);
        assert!(v["scores"].as_array().unwrap().len() == 8);
    }

    #[test]
    fn sim_reads_its_flags_and_names_what_is_wrong() {
        let words = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let (file, args, out) =
            parse_sim(&words("m.rules --players 4 --teams 2 --seed 9 --matches 25 --length 60 --score-limit 10 --json -o m.json")).unwrap();
        assert_eq!(file, PathBuf::from("m.rules"));
        assert_eq!(out, Some(PathBuf::from("m.json")));
        assert_eq!(
            args,
            SimArgs {
                setup: sim::Setup { players: 4, teams: 2, seed: 9, length: Some(60), score_limit: Some(10) },
                matches: 25,
                json: true,
            }
        );
        assert_eq!(parse_sim(&words("m.rules")).unwrap().1, SimArgs::default());
        assert!(parse_sim(&words("m.rules --players many")).is_err());
        assert!(parse_sim(&words("m.rules --seed")).is_err());
        assert!(parse_sim(&words("--players 4")).is_err());
        assert!(parse_sim(&words("m.rules --fast")).is_err());

        let err = sim_text("x.rules", "rules \"A\" 1\ncarrier slayer\non kill(a, b) { nope(a) }\n", &SimArgs::default())
            .unwrap_err();
        assert!(err.starts_with("x.rules:3:"), "{err}");
        let many = SimArgs { setup: sim::Setup { players: 17, ..sim::Setup::default() }, ..SimArgs::default() };
        let err = sim_text("x.rules", rules::builtin::SLAYER, &many).unwrap_err();
        assert_eq!(err, "x.rules: a match seats 1 to 16 players\n");
        let err = sim_text("x.rules", rules::builtin::SLAYER, &SimArgs { matches: 101, ..SimArgs::default() }).unwrap_err();
        assert!(err.contains("1 to 100 matches"), "{err}");
        assert_eq!(clock(5772), "3:12.4");
        assert_eq!(clock(0), "0:00.0");
    }

    #[test]
    fn a_bad_scenario_is_named() {
        let err = run_text("x", GOOD, r#"{"nope": 1}"#, false).unwrap_err();
        assert!(err.starts_with("scenario:"), "{err}");
    }

    #[test]
    fn caps_and_builtins_list() {
        let c = caps_text();
        assert!(c.contains("score("), "{c}");
        assert!(c.contains("forge.move"), "{c}");
        let list = builtin_text(None).unwrap();
        assert!(list.contains("Slayer"), "{list}");
        assert!(builtin_text(Some("slayer")).unwrap().starts_with("rules \"Slayer\""));
        assert!(builtin_text(Some("nothing")).is_err());
        // The examples follow the ports, marked, and print by key.
        assert!(list.contains("engine 2: Slayer"), "{list}");
        for key in ["infection", "gun_game", "vip", "siege", "race", "bounce_practice"] {
            assert!(list.contains(&format!("example {key}: ")), "{list}");
            assert_eq!(builtin_text(Some(key)).unwrap(), rules::builtin::examples().iter().find(|(k, _)| *k == key).unwrap().1);
        }
        assert!(list.find("engine ").unwrap() < list.find("example ").unwrap());
        // The three message forms list under one name.
        assert!(c.contains("message(team, text)"), "{c}");
    }

    /// 16 players times 16 times 16, once per kill: over the budget.
    const HEAVY: &str = "rules \"Heavy\" 1\ncarrier slayer\nglobal var n: int = 0\n\
                         on kill(v, k) { for each players a { for each players b { for each players c { n = n + 1 } } } }\n";

    #[test]
    fn check_and_build_refuse_a_script_over_the_budget() {
        let err = check_text("x.rules", HEAVY).unwrap_err();
        assert!(err.contains("OVER BUDGET"), "{err}");
        assert!(err.contains("x.rules: refused: the worst tick costs"), "{err}");
        let err = build_blob("x.rules", HEAVY).unwrap_err();
        assert!(err.starts_with("x.rules: refused: the worst tick costs"), "{err}");
    }
}
