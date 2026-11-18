//! `h2 bots test`: Clankers in a Live custom game, end to end, with no one
//! at the controller (docs/game/bots.md, "Testing"). A Live server in this
//! process with a throwaway store, a signed-in test account, one `h2 play`
//! game with Clankers On at the count asked for (`--set`, so no menu work
//! for that), the menus pressed through to a hosted custom game and Start
//! Game ([`crate::bots_drive`]), and a verdict from the game's lines, the
//! agent API's phase and the server's session and game record
//! ([`crate::bots_verdict`]).
//!
//! `h2 bots verdict <log>` prints the verdict of a log again.

use std::fs;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::bots_drive::Driver;
use crate::bots_verdict::{self, Plan};
use crate::coop_test_cmd::{self, Game, Supervisor};
use crate::coop_verdict::Roles;
use crate::live_cmd::{Client, LIVE_DIR_ENV};

const USAGE: &str = "usage:
  h2 bots test [--disc ISO] [--count N] [--map NAME] [--keep] [--timeout SECS]
               [--root DIR] [--out DIR] [--server-port N] [--dry-run]
               [-- <more h2 play arguments>]
  h2 bots verdict <bots.log>

Clankers in a Live custom game on this Mac, with no one at the controller
(docs/game/bots.md, \"Testing\"). Starts a Live server in this process on
127.0.0.1:--server-port (8077) with a throwaway store, makes the account
Tester on it, and opens one `h2 play` window signed in as Tester with
Clankers On and Count --count (5 by default: 3 local bots on ports 1 to 3,
the rest phantoms), set for this run with `--set`. The window builds the
game into --out first if --out has none (as `h2 play` does).

Nobody presses anything: the game prints its menu screen (H2_UI_WATCH=1)
and this process presses over its agent socket, each press on a screen or
line it waited for: Xbox Live, Custom Game, then, once the local bots are
on Xbox Live (or after 90 s), Start Game. Every press is a `bots-drive:`
line.

It watches the game's lines, the agent API's phase and map, and the Live
server's session and game record, and stops once every check has passed
or after --timeout (360 s). The verdict lists each check, passed or
failed, with the log lines that explain it, and names the first failure:
signed in; Clankers on; hosting a custom game; the Live roster; each local
bot through the join gate; each local bot on Xbox Live as a bot account
(000b...); the start state; gameplay on a map (--map NAME: on that map;
the map is the lobby's, this does not pick it); the phantoms joined; the
session holding 1 + count players; the game record's bots (bot: true).
It exits non-zero when a check failed.

The merged log (bots.log) and the game's whole stderr (Tester.log) go to
<root>/logs/<time>/, printed at the start and the end. The game's data,
Live directory and the server's store are under <root>/run-<time>/ and
are deleted at the end unless --keep, which also leaves the game open
(and this process with it, for the Live server) until its window closes.
--root defaults to $TMPDIR/h2-bots-test. --dry-run prints the game's
command line and starts nothing.";

pub const TAG: &str = "Tester";
const DEFAULT_COUNT: usize = 5;
const DEFAULT_SERVER_PORT: u16 = 8077;
const DEFAULT_TIMEOUT_SECS: u64 = 360;
/// The Clankers page's count range (`clankers.count`).
const MAX_COUNT: usize = 15;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Test(Opts),
    Verdict(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opts {
    pub disc: PathBuf,
    pub count: usize,
    pub map: Option<String>,
    pub keep: bool,
    pub timeout: Duration,
    pub root: PathBuf,
    pub out: Option<PathBuf>,
    pub server_port: u16,
    pub dry_run: bool,
    pub play_args: Vec<String>,
}

/// Parses `h2 bots test ...` / `h2 bots verdict ...` (the arguments after
/// `bots`). `env` answers `H2_DISC` and `TMPDIR`.
pub fn parse(args: Vec<String>, env: impl Fn(&str) -> Option<String>) -> Result<Cmd> {
    let mut it = args.into_iter();
    match it.next().as_deref() {
        Some("test") => {}
        Some("verdict") => {
            let log = it
                .next()
                .filter(|a| !a.starts_with('-'))
                .with_context(|| format!("verdict takes a bots.log\n\n{USAGE}"))?;
            if let Some(a) = it.next() {
                bail!("unexpected {a:?}\n\n{USAGE}");
            }
            return Ok(Cmd::Verdict(PathBuf::from(log)));
        }
        Some(other) => bail!("unknown `h2 bots {other}`\n\n{USAGE}"),
        None => bail!("{USAGE}"),
    }
    let mut disc = env("H2_DISC").map(PathBuf::from);
    let mut count = DEFAULT_COUNT;
    let mut map = None;
    let mut keep = false;
    let mut timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);
    let mut root = None;
    let mut out = None;
    let mut server_port = DEFAULT_SERVER_PORT;
    let mut dry_run = false;
    let mut play_args = Vec::new();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().with_context(|| format!("{name} needs a value"));
        match a.as_str() {
            "--disc" => disc = Some(PathBuf::from(value("--disc")?)),
            "--count" => {
                let v = value("--count")?;
                count = v
                    .parse()
                    .ok()
                    .filter(|n| (1..=MAX_COUNT).contains(n))
                    .with_context(|| format!("--count {v}: 1 to {MAX_COUNT}"))?;
            }
            "--map" => {
                let v = value("--map")?;
                if v.is_empty() || v.contains(char::is_whitespace) {
                    bail!("--map {v:?}: a map name (lockout)");
                }
                map = Some(v);
            }
            "--keep" => keep = true,
            "--timeout" => {
                let v = value("--timeout")?;
                let secs: u64 = v
                    .parse()
                    .ok()
                    .filter(|s| *s > 0)
                    .with_context(|| format!("--timeout {v}: seconds, more than 0"))?;
                timeout = Duration::from_secs(secs);
            }
            "--root" => root = Some(PathBuf::from(value("--root")?)),
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--server-port" => {
                let v = value("--server-port")?;
                server_port = v
                    .parse()
                    .ok()
                    .filter(|p| *p != 0)
                    .with_context(|| format!("--server-port {v}: a port"))?;
            }
            "--dry-run" => dry_run = true,
            "--" => play_args.extend(it.by_ref()),
            "-h" | "--help" => bail!("{USAGE}"),
            _ => bail!("unexpected {a:?}\n\n{USAGE}"),
        }
    }
    let disc = disc.with_context(|| format!("no disc: --disc ISO or H2_DISC\n\n{USAGE}"))?;
    let root = root.unwrap_or_else(|| {
        env("TMPDIR")
            .filter(|t| !t.is_empty())
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join("h2-bots-test")
    });
    Ok(Cmd::Test(Opts {
        disc,
        count,
        map,
        keep,
        timeout,
        root,
        out,
        server_port,
        dry_run,
        play_args,
    }))
}

/// The game: the account Tester on `server`, its data and Live directory
/// under `run`, its agent socket at `agent`.
pub fn plan(opts: &Opts, server: &str, run: &Path, agent: &Path) -> Game {
    let data = run.join("data");
    let live = run.join("live");
    let mut args = vec![
        "play".to_string(),
        opts.disc.display().to_string(),
        "--data".into(),
        data.display().to_string(),
        "--live-server".into(),
        server.into(),
        "--gamertag".into(),
        TAG.into(),
        "--title".into(),
        "Halo 2 — bots test".into(),
        "--no-mappack".into(),
        "--no-halo1".into(),
        "--set".into(),
        "clankers.enabled=true".into(),
        "--set".into(),
        format!("clankers.count={}", opts.count),
    ];
    if let Some(out) = &opts.out {
        args.extend(["--out".into(), out.display().to_string()]);
    }
    args.extend(opts.play_args.iter().cloned());
    let env = vec![
        (LIVE_DIR_ENV.to_string(), live.display().to_string()),
        (agent_proto::ENV.into(), agent.display().to_string()),
        ("H2_UI_WATCH".into(), "1".into()),
    ];
    Game {
        tag: TAG.into(),
        host: true,
        data,
        live,
        xnet: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0),
        agent: agent.to_path_buf(),
        args,
        env,
    }
}

pub fn run(args: Vec<String>) -> Result<()> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    match parse(args, env)? {
        Cmd::Verdict(log) => {
            let text =
                fs::read_to_string(&log).with_context(|| format!("reading {}", log.display()))?;
            let v = bots_verdict::verdict(&text)
                .with_context(|| format!("{} has no `bots-test: plan:` line", log.display()))?;
            print!("{}", v.render());
            if !v.passed() {
                bail!("the bots test failed");
            }
            Ok(())
        }
        Cmd::Test(opts) => test(opts),
    }
}

/// How often the harness asks the game and the server.
const STATUS_EVERY: Duration = Duration::from_secs(2);
const SERVER_EVERY: Duration = Duration::from_secs(3);
/// Once every check passed, this long more for the last lines.
const SETTLE: Duration = Duration::from_secs(3);
/// How long the game may take to close.
const CLOSE_WAIT: Duration = Duration::from_secs(15);

fn test(opts: Opts) -> Result<()> {
    let exe = std::env::current_exe().context("finding this h2")?;
    let stamp = coop_test_cmd::stamp(crate::live_cmd::now());
    let run_dir = opts.root.join(format!("run-{stamp}"));
    let logs = opts.root.join("logs").join(&stamp);
    let server = format!("http://127.0.0.1:{}", opts.server_port);
    let agent = std::env::temp_dir().join(format!("h2-bots-test-{}.sock", std::process::id()));
    let game = plan(&opts, &server, &run_dir, &agent);
    let plan = Plan {
        count: opts.count,
        tag: TAG.into(),
        map: opts.map.clone(),
    };
    if opts.dry_run {
        println!(
            "server: {server} (data in {})",
            run_dir.join("server").display()
        );
        println!(
            "{}: {}",
            game.tag,
            coop_test_cmd::shell_line(&exe.display().to_string(), &game)
        );
        println!("{}", plan.line());
        return Ok(());
    }
    if !opts.disc.is_file() {
        bail!("{} is not a file", opts.disc.display());
    }
    let interrupted = coop_test_cmd::watch_interrupts();
    let server_data = run_dir.join("server");
    fs::create_dir_all(&server_data)?;
    fs::create_dir_all(&logs)?;
    let listen = SocketAddrV4::new(Ipv4Addr::LOCALHOST, opts.server_port);
    let embedded = live_server::start_embedded(
        live_server::FILE_DB,
        listen.into(),
        &server_data,
        live_server::http::Site {
            web: live_server::web_dir(),
            game_art: None,
        },
    )
    .with_context(|| {
        format!("starting the Live server on {listen}; another on that port? --server-port N")
    })?;
    let client = Client::at(&embedded.url)?;
    coop_test_cmd::accounts(
        &client,
        std::slice::from_ref(&game),
        &run_dir.join("accounts.json"),
    )?;
    let merged = logs.join("bots.log");
    eprintln!(
        "bots test: Live server {} ({} store); {} Clankers ({} local, {} phantoms)",
        embedded.url,
        embedded.store,
        plan.count,
        plan.local(),
        plan.phantoms()
    );
    eprintln!(
        "bots test: logs in {} (bots.log, {TAG}.log); agent `h2 agent --socket {} status`",
        logs.display(),
        agent.display()
    );
    let roles = Roles {
        host: TAG.into(),
        ..Roles::default()
    };
    let mut sup = Supervisor::new(&merged, &logs, &roles)?.with_filter(bots_verdict::wanted);
    sup.keep_lines();
    sup.spawn(&exe, &game)?;
    sup.note(0, &plan.line())?;
    eprintln!("bots test: the game opens (it builds into --out first if that has none)");

    let mut watch = Watch::new(&plan, &client, &agent);
    let start = Instant::now();
    let mut passed_at: Option<Instant> = None;
    let mut ended = None;
    loop {
        sup.pump(Duration::from_millis(200))?;
        watch.step(&mut sup)?;
        if interrupted.load(std::sync::atomic::Ordering::Relaxed) {
            ended = Some("interrupted");
            sup.note(0, "bots-test: interrupted")?;
            break;
        }
        if watch.crashed {
            ended = Some("the game stopped (its guest thread exited)");
            break;
        }
        if sup.exited(0) {
            ended = Some("the game closed");
            sup.note(0, "bots-test: the game closed")?;
            break;
        }
        if start.elapsed() >= opts.timeout {
            sup.note(
                0,
                &format!("bots-test: timed out after {} s", opts.timeout.as_secs()),
            )?;
            break;
        }
        let all = bots_verdict::verdict(&fs::read_to_string(&merged)?).is_some_and(|v| v.passed());
        match (all, passed_at) {
            (true, None) => passed_at = Some(Instant::now()),
            (true, Some(t)) if t.elapsed() >= SETTLE => break,
            (false, _) => passed_at = None,
            _ => {}
        }
    }
    sup.pump(Duration::from_millis(300))?;
    let text = fs::read_to_string(&merged)?;
    let verdict = bots_verdict::verdict(&text).context("the merged log lost its plan line")?;
    print!("{}", verdict.render());
    if let Some(why) = ended {
        println!("ended early: {why}");
    }
    println!("log: {}", merged.display());
    println!("game log: {}", logs.join(format!("{TAG}.log")).display());

    if opts.keep && !sup.all_exited() {
        eprintln!(
            "bots test: --keep: the game stays open, and this process with it for the Live \
             server at {}; close the window (or Ctrl-C) to end. Data in {}",
            embedded.url,
            run_dir.display()
        );
        while !sup.all_exited() {
            sup.pump(Duration::from_millis(500))?;
            let _ = sup.take_lines();
            if interrupted.load(std::sync::atomic::Ordering::Relaxed) {
                sup.close_all();
                break;
            }
        }
    }
    sup.close_all();
    let closing = Instant::now();
    while !sup.all_exited() && closing.elapsed() < CLOSE_WAIT {
        sup.pump(Duration::from_millis(200))?;
    }
    if !sup.all_exited() {
        sup.kill_all();
    }
    drop(embedded);
    let _ = fs::remove_file(&agent);
    if !opts.keep {
        let _ = fs::remove_dir_all(&run_dir);
    }
    if !verdict.passed() {
        let (n, c) = verdict.first_failure().expect("not passed");
        bail!(
            "the bots test failed at step {n} ({}); log {}",
            c.name,
            merged.display()
        );
    }
    Ok(())
}

/// What the harness watches besides the game's lines: the driver, the
/// agent API's phase and map, and the Live server.
struct Watch<'a> {
    plan: &'a Plan,
    client: &'a Client,
    agent: &'a Path,
    drive: Driver,
    agent_up: bool,
    #[cfg(unix)]
    conn: Option<agent_proto::Client>,
    status_at: Option<Instant>,
    server_at: Option<Instant>,
    last_status: String,
    last_session: String,
    last_game: String,
    hosting: bool,
    playing: bool,
    crashed: bool,
    start: Instant,
}

impl<'a> Watch<'a> {
    fn new(plan: &'a Plan, client: &'a Client, agent: &'a Path) -> Self {
        Watch {
            plan,
            client,
            agent,
            drive: Driver::new(plan.local()),
            agent_up: false,
            #[cfg(unix)]
            conn: None,
            status_at: None,
            server_at: None,
            last_status: String::new(),
            last_session: String::new(),
            last_game: String::new(),
            hosting: false,
            playing: false,
            crashed: false,
            start: Instant::now(),
        }
    }

    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn step(&mut self, sup: &mut Supervisor) -> Result<()> {
        for (_, _, line) in sup.take_lines() {
            if line.starts_with("agent: listening") {
                self.agent_up = true;
            }
            if line.contains("guest thread exited") {
                self.crashed = true;
            }
            if line.starts_with("coop-live: lobby mode 2")
                || (line.starts_with("live: session ") && line.contains(" advertised"))
            {
                self.hosting = true;
            }
            let now = self.now_ms();
            self.drive.line(now, &line);
        }
        if self.agent_up && self.status_at.is_none_or(|t| t.elapsed() >= STATUS_EVERY) {
            self.status_at = Some(Instant::now());
            if let Some((phase, mode, map)) = self.status() {
                let line = bots_verdict::status_line(&phase, &mode, &map);
                if line != self.last_status {
                    sup.note(0, &line)?;
                    self.last_status = line;
                }
                if phase == "gameplay" {
                    self.playing = true;
                }
                let now = self.now_ms();
                self.drive.status(now, &phase);
            }
        }
        if self.agent_up {
            let now = self.now_ms();
            if let Some(p) = self.drive.poll(now) {
                sup.note(0, &self.drive.press_line(&p))?;
                if let Err(e) = self.press(&p) {
                    sup.note(0, &format!("bots-drive: the press failed: {e:#}"))?;
                }
            }
            for said in self.drive.take_said() {
                sup.note(0, &said)?;
            }
        }
        if self.hosting && self.server_at.is_none_or(|t| t.elapsed() >= SERVER_EVERY) {
            self.server_at = Some(Instant::now());
            if let Some(line) = self.session()
                && line != self.last_session
            {
                sup.note(0, &line)?;
                self.last_session = line;
            }
            if self.playing
                && let Some(line) = self.game()
                && line != self.last_game
            {
                sup.note(0, &line)?;
                self.last_game = line;
            }
        }
        Ok(())
    }

    /// The agent API's phase, mode and map (`session.status`).
    #[cfg(unix)]
    fn status(&mut self) -> Option<(String, String, String)> {
        let v = self
            .call(agent_proto::method::STATUS, serde_json::Value::Null)
            .ok()?;
        Some(read_status(&v))
    }

    #[cfg(not(unix))]
    fn status(&mut self) -> Option<(String, String, String)> {
        None
    }

    #[cfg(unix)]
    fn press(&mut self, p: &crate::coop_drive::Press) -> Result<()> {
        self.call(
            agent_proto::method::INPUT_PAD,
            crate::coop_drive::pad_params(p),
        )
        .map(drop)
    }

    #[cfg(not(unix))]
    fn press(&mut self, _: &crate::coop_drive::Press) -> Result<()> {
        bail!("the agent API needs Unix sockets")
    }

    #[cfg(unix)]
    fn call(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        if self.conn.is_none() {
            let c = agent_proto::Client::connect(self.agent)
                .with_context(|| format!("connecting to {}", self.agent.display()))?;
            c.set_timeout(Some(Duration::from_secs(3)))?;
            self.conn = Some(c);
        }
        let c = self.conn.as_mut().expect("connected above");
        match c.call(method, params) {
            Ok(v) => Ok(v),
            Err(e) => {
                self.conn = None;
                bail!("{method}: {e}")
            }
        }
    }

    /// The host's session on the server, as a `bots-test: session` line.
    fn session(&self) -> Option<String> {
        let page: serde_json::Value = self
            .client
            .get("/sessions", &[("host", self.plan.tag.clone())])
            .ok()?;
        session_from(&page)
    }

    /// The newest game record, as a `bots-test: game` line.
    fn game(&self) -> Option<String> {
        let page: serde_json::Value = self.client.get("/games", &[]).ok()?;
        game_from(&page)
    }
}

/// `session.status`'s phase, mode and map (`-` when absent).
pub fn read_status(v: &serde_json::Value) -> (String, String, String) {
    let at = &v["at"];
    let s = |k: &str| at[k].as_str().unwrap_or("-").to_string();
    (s("phase"), s("mode"), s("map"))
}

/// The first session of a `GET /v1/sessions` page, as a `bots-test:
/// session` line.
pub fn session_from(page: &serde_json::Value) -> Option<String> {
    let s = page["items"].as_array()?.first()?;
    let slots = &s["advertisement"]["slots"];
    let n = |k: &str| slots[k].as_u64().unwrap_or(0);
    let joined = s["players"].as_object().map_or(0, |p| p.len());
    Some(bots_verdict::session_line(
        s["session_id"].as_str().unwrap_or("?"),
        n("public_filled"),
        n("private_filled"),
        joined,
    ))
}

/// The newest game of a `GET /v1/games` page, as a `bots-test: game`
/// line: its players, those with `bot: true`, and those of them with an
/// xuid (a bot account).
pub fn game_from(page: &serde_json::Value) -> Option<String> {
    let g = page["items"].as_array()?.first()?;
    let players = g["players"].as_array()?;
    let bots: Vec<&serde_json::Value> = players
        .iter()
        .filter(|p| p["bot"].as_bool() == Some(true))
        .collect();
    let accounts = bots.iter().filter(|p| p["xuid"].is_string()).count();
    Some(bots_verdict::game_line(
        g["game_id"].as_str().unwrap_or("?"),
        g["map"].as_str().unwrap_or("?"),
        players.len(),
        bots.len(),
        accounts,
    ))
}

#[cfg(test)]
mod tests;
