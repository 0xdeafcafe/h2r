//! `h2 render gate`: the zero gate of docs/graphics/retire-d3d.md (R9).
//! Runs the census corpus through `render.residual` on the serve it finds,
//! prints one table of the D3D entries still asked for by producer and
//! caller in each state, and exits 1 while any entry outside
//! `outside_render` remains.
//!
//! The pure parts (the corpus, the verdict and the table) are tested on
//! invented captures; the driver is thin and only calls the agent API.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use agent_proto::{CallError, Client, ResidualReply, method};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub const USAGE: &str = "usage: h2 render gate [--frames N] [--only NAME[,NAME]] [--list] [--socket PATH|tcp://HOST:PORT] [--timeout SECONDS]
  Runs the census corpus through render.residual on a running `h2 serve`
  started with H2_FEATURES=frame-scene.census: boot, menu, movie, lobby,
  loading, a cutscene, then every campaign level and multiplayer map, each
  over N presents (3000 for a map, 300 for the others; 1 to 3600). Prints the
  residual D3D entries by state, producer and caller, and exits 1 while any
  entry outside `outside_render` remains.
  Start the serve fresh: `boot` is the window right after the first present.";

/// Presents per map window (framescene.md 9.1).
pub const MAP_FRAMES: u32 = 3000;
/// Presents per other state.
pub const STATE_FRAMES: u32 = 300;
/// The producer whose work is outside any render frame: allowed.
pub const OUTSIDE: &str = "outside_render";

/// How a state is reached before its window is armed.
#[derive(Debug, Clone, PartialEq)]
pub enum Setup {
    /// Nothing: the serve is where it started.
    Boot,
    /// `game.menu` unless already there, then the menu's map.
    Menu,
    /// The menu's idle attract movie playing (`ui.attract`).
    Movie,
    /// `host.lobby.open` on a multiplayer map, closed after.
    Lobby { map: String },
    /// The window is armed before `game.load`, so it spans the load.
    Loading { scenario: String },
    /// A campaign cutscene: `game.load`, then `phase: cinematic`.
    Cutscene { scenario: String },
    /// `game.load`, then gameplay for 30 ticks.
    Level { scenario: String, campaign: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub name: String,
    pub setup: Setup,
    pub frames: u32,
}

/// The census corpus from `game.levels`' answer (`{campaign: [{name,
/// scenario}], multiplayer: [...]}`): the fixed states, then every level.
pub fn corpus(levels: &Value) -> Result<Vec<Step>> {
    let list = |k: &str| -> Vec<String> {
        levels[k]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|l| l["scenario"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (campaign, multi) = (list("campaign"), list("multiplayer"));
    let (Some(first_campaign), Some(first_multi)) = (campaign.first(), multi.first()) else {
        bail!("game.levels lists no campaign or no multiplayer level");
    };
    // The opening mission starts with a cutscene.
    let intro = campaign
        .iter()
        .find(|s| stem(s).starts_with("00a"))
        .unwrap_or(first_campaign)
        .clone();
    let mut steps = vec![
        Step { name: "boot".into(), setup: Setup::Boot, frames: STATE_FRAMES },
        Step { name: "menu".into(), setup: Setup::Menu, frames: STATE_FRAMES },
        Step { name: "movie".into(), setup: Setup::Movie, frames: STATE_FRAMES },
        Step { name: "lobby".into(), setup: Setup::Lobby { map: stem(first_multi) }, frames: STATE_FRAMES },
        Step { name: "loading".into(), setup: Setup::Loading { scenario: first_multi.clone() }, frames: STATE_FRAMES },
        Step { name: "cutscene".into(), setup: Setup::Cutscene { scenario: intro }, frames: STATE_FRAMES },
    ];
    for (scenarios, campaign) in [(&campaign, true), (&multi, false)] {
        for s in scenarios {
            steps.push(Step {
                name: stem(s),
                setup: Setup::Level { scenario: s.clone(), campaign },
                frames: MAP_FRAMES,
            });
        }
    }
    Ok(steps)
}

/// A scenario path's last component: the map's short name.
pub fn stem(scenario: &str) -> String {
    scenario.rsplit(['\\', '/']).next().unwrap_or(scenario).to_owned()
}

/// One state's verdict.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Verdict {
    pub state: String,
    /// Producer rows with work left, outside_render left out.
    pub producers: Vec<Row>,
    /// Caller rows (not split by producer, so they include outside_render),
    /// kept only for a failing state.
    pub callers: Vec<Row>,
    /// Why the capture can not be judged.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub name: String,
    pub entries: u64,
    pub residual: u64,
    pub push_words: u64,
}

impl Verdict {
    pub fn passed(&self) -> bool {
        self.problem.is_none() && self.producers.is_empty()
    }
}

/// Judges one window: any producer but `outside_render` with entries,
/// residual draws or push words fails the state. A capture without
/// producers (no `frame-scene.census`) or without presents is a problem,
/// not a pass.
pub fn judge(state: &str, r: &ResidualReply) -> Verdict {
    let mut v = Verdict { state: state.into(), ..Default::default() };
    let Some(window) = &r.window else {
        v.problem = Some("no finished window".into());
        return v;
    };
    if window.frames == 0 {
        v.problem = Some("the window counted no presents".into());
        return v;
    }
    let Some(producers) = &r.producers else {
        v.problem = Some("no producers: start the serve with H2_FEATURES=frame-scene.census".into());
        return v;
    };
    v.producers = producers
        .iter()
        .filter(|p| p.producer != OUTSIDE)
        .filter(|p| p.entries + p.residual + p.push_words > 0)
        .map(|p| Row {
            name: p.producer.clone(),
            entries: p.entries,
            residual: p.residual,
            push_words: p.push_words,
        })
        .collect();
    v.producers
        .sort_by(|a, b| (b.entries, &a.name).cmp(&(a.entries, &b.name)));
    if !v.producers.is_empty() {
        v.callers = r
            .callers
            .iter()
            .flatten()
            .filter(|c| c.calls() + c.residual > 0)
            .map(|c| Row { name: c.function.clone(), entries: c.calls(), residual: c.residual, push_words: 0 })
            .collect();
    }
    v
}

/// The one table: a summary row per state, then the producer and caller
/// rows of each failing state.
pub fn table(verdicts: &[Verdict]) -> String {
    let mut rows: Vec<[String; 6]> = vec![[
        "state".into(), "kind".into(), "name".into(), "entries".into(), "residual".into(), "push_words".into(),
    ]];
    for v in verdicts {
        let status = match (&v.problem, v.producers.is_empty()) {
            (Some(p), _) => format!("problem: {p}"),
            (None, true) => "zero".into(),
            (None, false) => "residual".into(),
        };
        rows.push([v.state.clone(), "state".into(), status, String::new(), String::new(), String::new()]);
        for (kind, list) in [("producer", &v.producers), ("caller", &v.callers)] {
            for r in list {
                rows.push([
                    v.state.clone(), kind.into(), r.name.clone(),
                    r.entries.to_string(), r.residual.to_string(), r.push_words.to_string(),
                ]);
            }
        }
    }
    let widths: Vec<usize> = (0..6)
        .map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0))
        .collect();
    let mut out = String::new();
    for r in &rows {
        let line: Vec<String> = r
            .iter()
            .enumerate()
            .map(|(i, c)| if i >= 3 { format!("{c:>w$}", w = widths[i]) } else { format!("{c:<w$}", w = widths[i]) })
            .collect();
        out.push_str(line.join("  ").trim_end());
        out.push('\n');
    }
    let failed = verdicts.iter().filter(|v| !v.passed()).count();
    out.push_str(&format!("{} states, {} not at zero\n", verdicts.len(), failed));
    out
}

// ---- the driver ----

fn call(c: &mut Client, m: &str, p: Value) -> Result<Value> {
    c.call(m, p).map_err(|e| match e {
        CallError::Rpc(e) => anyhow::anyhow!("{m}: {}", serde_json::to_string(&e).unwrap_or(e.message.clone())),
        e => anyhow::anyhow!("{m}: {e}"),
    })
}

fn wait(c: &mut Client, until: Value, ms: u64) -> Result<()> {
    call(c, method::WAIT, json!({"until": until, "timeout_ms": ms})).map(|_| ())
}

fn attract_playing(c: &mut Client, within: Duration) -> Result<()> {
    let end = Instant::now() + within;
    while Instant::now() < end {
        let v = call(c, method::UI_ATTRACT, json!({}))?;
        if v["state"].as_str().is_some_and(|s| s.to_lowercase().contains("playing")) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    bail!("the menu's attract movie did not start within {} s", within.as_secs())
}

fn load(c: &mut Client, scenario: &str, campaign: bool) -> Result<()> {
    let p = if campaign {
        json!({"scenario": scenario, "difficulty": "normal"})
    } else {
        json!({"scenario": scenario, "variant": "slayer"})
    };
    call(c, "game.load", p).map(|_| ())
}

fn to_menu(c: &mut Client) -> Result<()> {
    // Refused at the menu already; that is where we want to be.
    let _ = c.call("game.menu", json!({}));
    wait(c, json!({"map": "mainmenu"}), 120_000)
}

/// Reaches the step's state; `Loading` arms first, so its window spans the load.
fn reach(c: &mut Client, s: &Step) -> Result<()> {
    match &s.setup {
        Setup::Boot => {}
        Setup::Menu => to_menu(c)?,
        Setup::Movie => {
            to_menu(c)?;
            attract_playing(c, Duration::from_secs(180))?;
        }
        Setup::Lobby { map } => {
            to_menu(c)?;
            call(c, method::HOST_LOBBY_OPEN, json!({"map": map, "variant": "slayer", "max_players": 8, "live": false}))?;
        }
        Setup::Loading { .. } => {}
        Setup::Cutscene { scenario } => {
            load(c, scenario, true)?;
            wait(c, json!({"phase": "cinematic"}), 180_000)?;
        }
        Setup::Level { scenario, campaign } => {
            load(c, scenario, *campaign)?;
            wait(c, json!({"map": stem(scenario), "phase": "gameplay", "map_ticks": 30}), 180_000)?;
        }
    }
    Ok(())
}

/// Arms a window and reads it once it is a new one.
fn window(c: &mut Client, frames: u32, during: impl FnOnce(&mut Client) -> Result<()>) -> Result<ResidualReply> {
    let before = call(c, method::RENDER_RESIDUAL, json!({}))?["window"].clone();
    call(c, method::RENDER_RESIDUAL, json!({"arm": {"frames": frames}}))?;
    during(c)?;
    let end = Instant::now() + Duration::from_secs(60) + Duration::from_millis(100 * u64::from(frames));
    loop {
        let r = call(c, method::RENDER_RESIDUAL, json!({"top": 500}))?;
        if r["state"] == "done" && r["window"] != before {
            return serde_json::from_value(r).context("render.residual's answer");
        }
        if Instant::now() > end {
            bail!("the window of {frames} presents did not finish (a paused or loading game presents nothing)");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn run_step(c: &mut Client, s: &Step) -> Result<ResidualReply> {
    let r = match &s.setup {
        Setup::Loading { scenario } => {
            to_menu(c)?;
            window(c, s.frames, |c| {
                load(c, scenario, false)?;
                wait(c, json!({"map": stem(scenario), "phase": "gameplay", "map_ticks": 30}), 180_000)
            })
        }
        _ => {
            reach(c, s)?;
            window(c, s.frames, |_| Ok(()))
        }
    };
    if matches!(s.setup, Setup::Lobby { .. }) {
        let _ = c.call(method::HOST_LOBBY_CLOSE, json!({}));
    }
    r
}

/// The command's flags.
#[derive(Debug, PartialEq, Default)]
pub struct Args {
    pub frames: Option<u32>,
    pub only: Option<Vec<String>>,
    pub list: bool,
    pub socket: Option<PathBuf>,
    pub timeout: Option<Duration>,
}

pub fn parse(words: &[String]) -> Result<Args> {
    let mut a = Args::default();
    let mut it = words.iter();
    while let Some(w) = it.next() {
        let mut val = |what: &str| it.next().with_context(|| format!("{what} needs a value\n{USAGE}"));
        match w.as_str() {
            "--frames" => {
                let n: u32 = val("--frames")?.parse().context("--frames needs a number")?;
                if !(1..=3600).contains(&n) {
                    bail!("--frames: 1 to 3600");
                }
                a.frames = Some(n);
            }
            "--only" => a.only = Some(val("--only")?.split(',').map(str::to_owned).collect()),
            "--list" => a.list = true,
            "--socket" => a.socket = Some(PathBuf::from(val("--socket")?)),
            "--timeout" => {
                let s: f64 = val("--timeout")?.parse().context("--timeout needs seconds")?;
                a.timeout = Some(Duration::try_from_secs_f64(s).ok().filter(|d| !d.is_zero()).context("--timeout needs a positive number of seconds")?);
            }
            _ => bail!("{w}: not a flag\n{USAGE}"),
        }
    }
    Ok(a)
}

/// `h2 render gate ...`.
pub fn run(args: Vec<String>) -> Result<()> {
    if args.first().map(String::as_str) != Some("gate") {
        bail!("{USAGE}");
    }
    let a = parse(&args[1..])?;
    let mut c = crate::agent_cmd::connect(a.socket, a.timeout)?;
    let levels = call(&mut c, "game.levels", json!({}))?;
    let mut steps = corpus(&levels)?;
    if let Some(only) = &a.only {
        steps.retain(|s| only.contains(&s.name));
        if steps.is_empty() {
            bail!("--only matched no state (--list names them)");
        }
    }
    if let Some(n) = a.frames {
        steps.iter_mut().for_each(|s| s.frames = n);
    }
    if a.list {
        steps.iter().for_each(|s| println!("{}\t{} presents", s.name, s.frames));
        return Ok(());
    }
    let mut verdicts = Vec::new();
    for s in &steps {
        eprintln!("gate: {} ({} presents)", s.name, s.frames);
        verdicts.push(match run_step(&mut c, s) {
            Ok(r) => judge(&s.name, &r),
            Err(e) => Verdict { state: s.name.clone(), problem: Some(format!("{e:#}")), ..Default::default() },
        });
    }
    print!("{}", table(&verdicts));
    if verdicts.iter().any(|v| !v.passed()) {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::{ResidualCaller, ResidualProducer, ResidualWindow};

    fn prod(name: &str, entries: u64, residual: u64) -> ResidualProducer {
        ResidualProducer { producer: name.into(), entries, residual, ..Default::default() }
    }

    fn reply(producers: Option<Vec<ResidualProducer>>) -> ResidualReply {
        ResidualReply {
            state: "done".into(),
            window: Some(ResidualWindow { frames: 300, ..Default::default() }),
            producers,
            ..Default::default()
        }
    }

    #[test]
    fn outside_render_work_is_not_a_failure() {
        let v = judge("menu", &reply(Some(vec![prod("outside_render", 900, 12), prod("hud", 0, 0)])));
        assert!(v.passed());
        assert!(v.producers.is_empty() && v.callers.is_empty());
    }

    #[test]
    fn any_other_producer_with_work_fails_and_lists_callers() {
        let mut r = reply(Some(vec![prod("outside_render", 5, 0), prod("water_teardown", 4, 2), prod("unknown", 9, 0)]));
        r.callers = Some(vec![ResidualCaller {
            function: "0x1234".into(),
            entries: [("DrawIndexed".to_owned(), 6)].into(),
            residual: 2,
            ..Default::default()
        }]);
        let v = judge("lockout", &r);
        assert!(!v.passed());
        let names: Vec<_> = v.producers.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["unknown", "water_teardown"], "by entries");
        assert_eq!(v.callers, [Row { name: "0x1234".into(), entries: 6, residual: 2, push_words: 0 }]);
    }

    #[test]
    fn push_words_alone_fail_a_state() {
        let mut p = prod("ui", 0, 0);
        p.push_words = 3;
        assert!(!judge("menu", &reply(Some(vec![p]))).passed());
    }

    #[test]
    fn a_capture_that_cannot_be_judged_is_not_a_pass() {
        assert!(judge("boot", &reply(None)).problem.unwrap().contains("frame-scene.census"));
        assert!(judge("boot", &ResidualReply::default()).problem.is_some());
        let mut r = reply(Some(vec![]));
        r.window.as_mut().unwrap().frames = 0;
        assert!(!judge("boot", &r).passed());
    }

    #[test]
    fn the_table_shows_states_then_failing_rows() {
        let ok = judge("menu", &reply(Some(vec![])));
        let bad = judge("lockout", &reply(Some(vec![prod("sky", 7, 3)])));
        let t = table(&[ok, bad]);
        let lines: Vec<&str> = t.lines().collect();
        assert!(lines[0].starts_with("state"));
        assert!(lines.iter().any(|l| l.starts_with("menu") && l.ends_with("zero")));
        assert!(lines.iter().any(|l| l.starts_with("lockout") && l.contains("producer") && l.contains("sky")));
        assert_eq!(*lines.last().unwrap(), "2 states, 1 not at zero");
    }

    #[test]
    fn the_corpus_is_the_states_then_every_level() {
        let levels = json!({
            "campaign": [{"name": "a", "scenario": "scenarios\\solo\\01a_tutorial\\01a_tutorial"}, {"name": "b", "scenario": "scenarios\\solo\\00a_introduction\\00a_introduction"}],
            "multiplayer": [{"name": "c", "scenario": "scenarios\\multi\\lockout\\lockout"}],
        });
        let c = corpus(&levels).unwrap();
        let names: Vec<_> = c.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["boot", "menu", "movie", "lobby", "loading", "cutscene", "01a_tutorial", "00a_introduction", "lockout"]);
        assert_eq!(c[5].setup, Setup::Cutscene { scenario: "scenarios\\solo\\00a_introduction\\00a_introduction".into() });
        assert!(c[6..].iter().all(|s| s.frames == MAP_FRAMES));
        assert!(corpus(&json!({"campaign": [], "multiplayer": []})).is_err());
    }

    #[test]
    fn flags_parse() {
        let w = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        let a = parse(&w("--frames 600 --only menu,lockout --list")).unwrap();
        assert_eq!((a.frames, a.list), (Some(600), true));
        assert_eq!(a.only.unwrap(), ["menu", "lockout"]);
        assert!(parse(&w("--frames 0")).is_err());
        assert!(parse(&w("--frames")).is_err());
        assert!(parse(&w("--what")).is_err());
    }
}
