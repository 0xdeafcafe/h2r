//! `h2 bots test`: Clankers in a Live custom game, end to end, with no one
//! at the controller (docs/game/bots.md, "Testing"). A Live server in this
//! process with a throwaway store, a signed-in test account, one `h2 play`
//! game with Clankers On at the count asked for (`--set`, so no menu work
//! for that), the menus pressed through to a hosted custom game and Start
//! Game ([`crate::bots_drive`]), and a verdict from the game's lines, the
//! agent API's phase and the server's session and game record
//! ([`crate::bots_verdict`]).
//!
//! `h2 bots test --clankermatch [--playlist slayer|teams|doubles]` runs
//! Clankermatch instead (docs/game/bots.md §8 and §13): Clankers off, the
//! Live menu's Clankermatch row, a clanker playlist, the empty search, the
//! lobby the local bots fill, the game, and out again
//! ([`crate::bots_drive::clankermatch`],
//! [`crate::bots_verdict::clankermatch`]).
//!
//! `h2 bots verdict <log>` prints the verdict of a log again.

use std::fs;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::bots_drive::Driver;
use crate::bots_progress;
use crate::bots_verdict::clankermatch::{self as cm_verdict, CmPlan};
use crate::bots_verdict::{self, Plan};
use crate::coop_test_cmd::{self, Game, Supervisor};
use crate::coop_verdict::Roles;
use crate::live_cmd::{Client, LIVE_DIR_ENV};

const USAGE: &str = "usage:
  h2 bots test [--disc ISO] [--count N] [--map NAME] [--keep] [--timeout SECS]
               [--wait-clang] [--settings-from DIR] [--root DIR] [--out DIR]
               [--server-port N] [--no-drive] [--dry-run]
               [-- <more h2 play arguments>]
  h2 bots test --clankermatch [--playlist slayer|teams|doubles] [the same,
               but --count and --map]
  h2 bots verdict <bots.log>

Clankers in a Live custom game on this Mac, with no one at the controller
(docs/game/bots.md, \"Testing\"). Starts a Live server in this process on
127.0.0.1:--server-port (8077) with a throwaway store, makes the account
Tester on it, and opens one `h2 play` window signed in as Tester with
Clankers On and Count --count (5 by default: 3 local bots on ports 1 to 3,
the rest phantoms), set for this run with `--set`. The window builds the
game into --out first if --out has none (as `h2 play` does).
--wait-clang runs clang's full build (`h2 play --tier clang`): it waits
for a background clang build of --out that is still writing its log,
then builds clang's module in the foreground if it is not current, never
the Cranelift fast tier. The game takes the [graphics] settings (video,
frame rate, render scale) of --settings-from's settings.toml (the data
root `h2 play` uses by default), so it renders as your own game does.
--timeout counts from the game's agent socket coming up, so a build
before that is not counted. It is 360 s, or 900 s with --no-drive: a
drive by hand (or by a model) needs longer.

--no-drive: launches and watches as above but presses nothing; the
scripted driver is off. It prints the agent socket and a hint at the
start, and you (or a model over `h2 agent --socket <sock> ui`, `frame
snap 1`, `press <button>`) drive the menus. The verdict is the same.

Either way <logs>/progress.json is rewritten whenever it changes: the
checks as pass, pending (fail once the run has ended), the phase, mode
and map, the last `ui:` line and the last `bots:` or `clankers:` line.
Poll that one file to know what to do next.

Without --no-drive nobody presses anything: the game prints its menu screen (H2_UI_WATCH=1)
and this process presses over its agent socket, each press on a screen or
line it waited for: A through the title screen and the new profile's
dialogs, Xbox Live from the main menu's row (A on the sign-in's dialogs),
Custom Game, then, once the local bots are on Xbox Live (or after 90 s),
Start Game. Every press is a `bots-drive:`
line.

It watches the game's lines, the agent API's phase and map, and the Live
server's session and game record, and stops once every check has passed
or after --timeout (360 s). The verdict lists each check, passed or
failed, with the log lines that explain it, and names the first failure:
signed in; Clankers on; on Xbox Live (not Split Screen or System Link);
hosting a Live custom game (the session advertised); the Live roster; each local
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
command line and starts nothing.

--clankermatch (or --playlist NAME, which implies it): Clankermatch
instead of a custom game. Clankers is Off (`--set clankers.enabled=false`);
the server's built-in lineup has the clanker playlists (hoppers 8 Clanker
Slayer, 9 Clanker Teams, 10 Clanker Doubles; --playlist, slayer by
default), and the run stops at once if the server does not mark the one
asked for. The drive goes Xbox Live, row 2 (Clankermatch), the
playlist's row, A, then waits: the search, the lobby this console hosts,
the local bots seated in it, the countdown, the game (to its end: 6 to 8
minutes) and back out, B until no session is hosted. The checks: signed
in; the clanker hoppers (server and game); the Clankermatch row's
playlists; the clanker search; the search ending with this console
hosting; the local bots planned; each seated and signed in, in the lobby;
each on Xbox Live as a bot account; the lobby at the playlist's fewest
players; the countdown and start; gameplay; the phantoms joining after
the start; the game record (kind clankermatch, bot: true); the local
bots leaving after the game. --timeout is 900 s (1200 with --no-drive).";

pub const TAG: &str = "Tester";
const DEFAULT_COUNT: usize = 5;
const DEFAULT_SERVER_PORT: u16 = 8077;
const DEFAULT_TIMEOUT_SECS: u64 = 360;
/// `--no-drive`'s default: a manual drive takes longer.
const NO_DRIVE_TIMEOUT_SECS: u64 = 900;
/// The Clankers page's count range (`clankers.count`).
const MAX_COUNT: usize = 15;
/// Clankermatch's defaults: the search, the fill, a whole game (6 to 8
/// minutes) and 30 s after it.
const CLANKERMATCH_TIMEOUT_SECS: u64 = 900;
const CLANKERMATCH_NO_DRIVE_TIMEOUT_SECS: u64 = 1200;

/// A clanker playlist of the built-in lineup (`--playlist`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmPlaylist {
    Slayer,
    Teams,
    Doubles,
}

impl CmPlaylist {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "slayer" => Some(Self::Slayer),
            "teams" => Some(Self::Teams),
            "doubles" => Some(Self::Doubles),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Slayer => "slayer",
            Self::Teams => "teams",
            Self::Doubles => "doubles",
        }
    }

    /// Its playlist id in the built-in lineup
    /// (`live_proto::hopper::defaults`).
    pub fn playlist_id(self) -> u32 {
        match self {
            Self::Slayer => 9,
            Self::Teams => 10,
            Self::Doubles => 11,
        }
    }
}

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
    /// The scripted driver is off: someone else presses.
    pub no_drive: bool,
    pub root: PathBuf,
    pub out: Option<PathBuf>,
    pub server_port: u16,
    pub dry_run: bool,
    /// clang's full build (`--tier clang`), never the fast tier.
    pub wait_clang: bool,
    /// The data root whose settings.toml's [graphics] the game takes.
    pub settings_from: PathBuf,
    pub play_args: Vec<String>,
    /// `--clankermatch`: Clankermatch on this playlist, not a custom game.
    pub clankermatch: Option<CmPlaylist>,
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
    let mut count = None;
    let mut map = None;
    let mut clankermatch = None;
    let mut keep = false;
    let mut timeout = None;
    let mut no_drive = false;
    let mut root = None;
    let mut out = None;
    let mut server_port = DEFAULT_SERVER_PORT;
    let mut dry_run = false;
    let mut wait_clang = false;
    let mut settings_from = None;
    let mut play_args = Vec::new();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().with_context(|| format!("{name} needs a value"));
        match a.as_str() {
            "--disc" => disc = Some(PathBuf::from(value("--disc")?)),
            "--count" => {
                let v = value("--count")?;
                count = Some(
                    v.parse()
                        .ok()
                        .filter(|n| (1..=MAX_COUNT).contains(n))
                        .with_context(|| format!("--count {v}: 1 to {MAX_COUNT}"))?,
                );
            }
            "--map" => {
                let v = value("--map")?;
                if v.is_empty() || v.contains(char::is_whitespace) {
                    bail!("--map {v:?}: a map name (lockout)");
                }
                map = Some(v);
            }
            "--keep" => keep = true,
            "--clankermatch" => {
                clankermatch.get_or_insert(CmPlaylist::Slayer);
            }
            "--playlist" => {
                let v = value("--playlist")?;
                clankermatch = Some(
                    CmPlaylist::parse(&v)
                        .with_context(|| format!("--playlist {v:?}: slayer, teams or doubles"))?,
                );
            }
            "--timeout" => {
                let v = value("--timeout")?;
                let secs: u64 = v
                    .parse()
                    .ok()
                    .filter(|s| *s > 0)
                    .with_context(|| format!("--timeout {v}: seconds, more than 0"))?;
                timeout = Some(Duration::from_secs(secs));
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
            "--no-drive" => no_drive = true,
            "--dry-run" => dry_run = true,
            "--wait-clang" => wait_clang = true,
            "--settings-from" => settings_from = Some(PathBuf::from(value("--settings-from")?)),
            "--" => play_args.extend(it.by_ref()),
            "-h" | "--help" => bail!("{USAGE}"),
            _ => bail!("unexpected {a:?}\n\n{USAGE}"),
        }
    }
    if clankermatch.is_some() && (count.is_some() || map.is_some()) {
        bail!(
            "--count and --map are the custom game's: Clankermatch's playlist sets the bots and \
             the map\n\n{USAGE}"
        );
    }
    let count = count.unwrap_or(DEFAULT_COUNT);
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
        timeout: timeout.unwrap_or_else(|| {
            Duration::from_secs(match (clankermatch.is_some(), no_drive) {
                (false, false) => DEFAULT_TIMEOUT_SECS,
                (false, true) => NO_DRIVE_TIMEOUT_SECS,
                (true, false) => CLANKERMATCH_TIMEOUT_SECS,
                (true, true) => CLANKERMATCH_NO_DRIVE_TIMEOUT_SECS,
            })
        }),
        no_drive,
        root,
        out,
        server_port,
        dry_run,
        wait_clang,
        settings_from: settings_from.unwrap_or_else(|| {
            env("TMPDIR")
                .filter(|t| !t.is_empty())
                .map_or_else(std::env::temp_dir, PathBuf::from)
                .join("h2boot-data")
        }),
        play_args,
        clankermatch,
    }))
}

/// A Clankermatch run's plan for `playlist`, from the built-in lineup (the
/// lineup a fresh server serves, `live_server::default_hopper`): its
/// hopper (descriptor index), its row on the Clankermatch list (the clanker
/// hoppers in index order), its fewest and most players and teams, and the
/// bots the fill seats, local first (`boot::clankers::max_local`).
pub fn cm_plan(playlist: CmPlaylist, tag: &str) -> Result<CmPlan> {
    use live_proto::hopper::clankers::size_of;
    let lineup = live_proto::hopper::defaults::playlists();
    let marked: Vec<(usize, &live_proto::hopper::defaults::Playlist)> = lineup
        .iter()
        .enumerate()
        .filter(|(_, p)| p.clankers.is_some())
        .collect();
    let (row, &(hopper, p)) = marked
        .iter()
        .enumerate()
        .find(|(_, (_, p))| p.playlist_id == playlist.playlist_id())
        .with_context(|| {
            format!(
                "the built-in lineup has no clanker playlist {} (id {})",
                playlist.name(),
                playlist.playlist_id()
            )
        })?;
    let min = p
        .descriptor()
        .min_players()
        .with_context(|| format!("{}: no fewest players", p.title))?;
    let (max, teams) = size_of(&p.rules);
    let bots = max.saturating_sub(1) as usize;
    Ok(CmPlan {
        playlist: playlist.name().into(),
        title: p.title.into(),
        hopper: hopper as u32,
        row: row as u32,
        min,
        max,
        teams,
        bots,
        local: boot::clankers::max_local(bots, max, teams),
        tag: tag.into(),
    })
}

/// Whether the server's marks (`GET /v1/hoppers/clankers`) agree with
/// `plan`: its hopper marked, on the same row of the list, the same size.
pub fn check_marks(
    plan: &CmPlan,
    marks: &live_proto::hopper::clankers::ClankerHoppers,
) -> Result<()> {
    let mut ids: Vec<u32> = marks.hoppers.iter().map(|h| h.hopper_id).collect();
    ids.sort_unstable();
    let Some(mark) = marks.get(plan.hopper) else {
        bail!(
            "the server marks clanker hoppers {ids:?}, not {} ({}): its hopper file is not the \
             built-in lineup",
            plan.hopper,
            plan.title
        );
    };
    let row = ids.iter().position(|h| *h == plan.hopper).unwrap_or(0) as u32;
    if row != plan.row || mark.players != plan.max || mark.teams != plan.teams {
        bail!(
            "the server's hopper {} is on row {row} with {} players and {} teams; the plan \
             (the built-in lineup) has row {}, {} and {}",
            plan.hopper,
            mark.players,
            mark.teams,
            plan.row,
            plan.max,
            plan.teams
        );
    }
    Ok(())
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
        if opts.clankermatch.is_some() {
            "Halo 2 — clankermatch test".into()
        } else {
            "Halo 2 — bots test".into()
        },
        "--no-mappack".into(),
        "--no-halo1".into(),
    ];
    if opts.clankermatch.is_some() {
        // The fill seats Clankermatch's bots; the page's own stay off.
        args.extend(["--set".into(), "clankers.enabled=false".into()]);
    } else {
        args.extend([
            "--set".into(),
            "clankers.enabled=true".into(),
            "--set".into(),
            format!("clankers.count={}", opts.count),
        ]);
    }
    if let Some(out) = &opts.out {
        args.extend(["--out".into(), out.display().to_string()]);
    }
    if opts.wait_clang {
        args.extend(["--tier".into(), "clang".into()]);
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
/// How long the game may take to come up (a build) before the timeout's
/// clock starts.
const BUILD_LIMIT: Duration = Duration::from_secs(60 * 60);
/// A background clang build whose log changed this recently still runs.
const CLANG_QUIET: Duration = Duration::from_secs(120);
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
    let cm = opts.clankermatch.map(|p| cm_plan(p, TAG)).transpose()?;
    let plan = Plan {
        count: opts.count,
        tag: TAG.into(),
        map: opts.map.clone(),
    };
    let plan_line = cm.as_ref().map_or_else(|| plan.line(), CmPlan::line);
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
        println!("{plan_line}");
        return Ok(());
    }
    if !opts.disc.is_file() {
        bail!("{} is not a file", opts.disc.display());
    }
    let interrupted = coop_test_cmd::watch_interrupts();
    let server_data = run_dir.join("server");
    fs::create_dir_all(&server_data)?;
    fs::create_dir_all(&logs)?;
    fs::create_dir_all(&game.data)?;
    match copy_graphics(&opts.settings_from, &game.data) {
        Ok(Some(n)) => eprintln!(
            "bots test: the game takes your [graphics] settings ({n} keys) from {}",
            settings::path_in(&opts.settings_from).display()
        ),
        Ok(None) => eprintln!(
            "bots test: no [graphics] in {}; the game's default video settings",
            settings::path_in(&opts.settings_from).display()
        ),
        Err(e) => eprintln!("bots test: video settings not copied: {e:#}"),
    }
    if opts.wait_clang {
        let out = opts
            .out
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("h2boot"));
        wait_for_background_clang(&out, &interrupted);
    }
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
    // Clankermatch needs the server's clanker hoppers: a fresh store
    // serves the built-in lineup, which has them.
    let marks_line = match &cm {
        Some(cm) => {
            let marks: live_proto::hopper::clankers::ClankerHoppers = client
                .get(live_proto::hopper::clankers::PATH, &[])
                .context("reading the server's clanker hoppers (GET /v1/hoppers/clankers)")?;
            check_marks(cm, &marks)?;
            let ids: Vec<u32> = marks.hoppers.iter().map(|h| h.hopper_id).collect();
            Some(format!("{}{ids:?}", cm_verdict::SERVER_MARKS))
        }
        None => None,
    };
    let merged = logs.join("bots.log");
    match &cm {
        Some(cm) => eprintln!(
            "bots test: Live server {} ({} store); Clankermatch {} (hopper {}, row {}): {} bots \
             ({} local, {} phantoms), {} to {} players",
            embedded.url,
            embedded.store,
            cm.title,
            cm.hopper,
            cm.row,
            cm.bots,
            cm.local,
            cm.phantoms(),
            cm.min,
            cm.max
        ),
        None => eprintln!(
            "bots test: Live server {} ({} store); {} Clankers ({} local, {} phantoms)",
            embedded.url,
            embedded.store,
            plan.count,
            plan.local(),
            plan.phantoms()
        ),
    }
    eprintln!(
        "bots test: logs in {} (bots.log, {TAG}.log); agent `h2 agent --socket {} status`",
        logs.display(),
        agent.display()
    );
    if opts.no_drive {
        eprintln!(
            "bots test: --no-drive: nothing is pressed. Agent socket: {}",
            agent.display()
        );
        eprintln!(
            "bots test: drive it yourself: poll {} and use `h2 agent --socket {} ui | frame snap 1 | press <button>`; {} s from the agent coming up",
            logs.join(bots_progress::FILE).display(),
            agent.display(),
            opts.timeout.as_secs()
        );
    }
    let roles = Roles {
        host: TAG.into(),
        ..Roles::default()
    };
    let mut sup = Supervisor::new(&merged, &logs, &roles)?.with_filter(bots_verdict::wanted);
    sup.keep_lines();
    sup.spawn(&exe, &game)?;
    sup.note(0, &plan_line)?;
    if let Some(line) = &marks_line {
        sup.note(0, line)?;
    }
    eprintln!("bots test: the game opens (it builds into --out first if that has none)");

    let driver = match &cm {
        Some(cm) => Driver::clankermatch(cm.local, cm.row),
        None => Driver::new(plan.local()),
    };
    let mut watch = Watch::new(TAG, &client, &agent, !opts.no_drive, driver, cm.is_some());
    let mut progress = String::new();
    let spawned = Instant::now();
    // The timeout counts from the agent socket: a build before is not run
    // time (at most BUILD_LIMIT of it).
    let mut start: Option<Instant> = None;
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
        if start.is_none() && watch.agent_up {
            start = Some(Instant::now());
        }
        if start.is_none() && spawned.elapsed() >= BUILD_LIMIT {
            sup.note(
                0,
                &format!(
                    "bots-test: timed out: no agent socket {} min after the start (a build?)",
                    BUILD_LIMIT.as_secs() / 60
                ),
            )?;
            break;
        }
        if start.is_some_and(|s| s.elapsed() >= opts.timeout) {
            sup.note(
                0,
                &format!("bots-test: timed out after {} s", opts.timeout.as_secs()),
            )?;
            break;
        }
        let text = fs::read_to_string(&merged)?;
        write_progress(&logs, &text, false, &mut progress);
        let all = bots_verdict::verdict(&text).is_some_and(|v| v.passed());
        match (all, passed_at) {
            (true, None) => passed_at = Some(Instant::now()),
            (true, Some(t)) if t.elapsed() >= SETTLE => break,
            (false, _) => passed_at = None,
            _ => {}
        }
    }
    sup.pump(Duration::from_millis(300))?;
    let text = fs::read_to_string(&merged)?;
    write_progress(&logs, &text, true, &mut progress);
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

/// Rewrites progress.json when it differs from `last` (`finished`: the run
/// is over). A failed write is said once per change, never fatal.
fn write_progress(logs: &Path, log: &str, finished: bool, last: &mut String) {
    let Some(text) = bots_progress::render(log, finished) else {
        return;
    };
    if text == *last {
        return;
    }
    if let Err(e) = bots_progress::write(logs, &text) {
        eprintln!("bots test: progress.json not written: {e:#}");
    }
    *last = text;
}

/// What the harness watches besides the game's lines: the driver, the
/// agent API's phase and map, and the Live server.
struct Watch<'a> {
    tag: &'a str,
    client: &'a Client,
    agent: &'a Path,
    drive: Driver,
    /// Whether the scripted driver presses (off with `--no-drive`).
    driving: bool,
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
    /// A Clankermatch run: the game line carries the record's kind and
    /// playlist.
    clankermatch: bool,
}

impl<'a> Watch<'a> {
    fn new(
        tag: &'a str,
        client: &'a Client,
        agent: &'a Path,
        driving: bool,
        drive: Driver,
        clankermatch: bool,
    ) -> Self {
        Watch {
            tag,
            client,
            agent,
            drive,
            driving,
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
            clankermatch,
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
            if crate::bots_drive::advertised(&line) {
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
        if self.agent_up && self.driving {
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
            .get("/sessions", &[("host", self.tag.to_string())])
            .ok()?;
        session_from(&page)
    }

    /// The newest game record, as a `bots-test: game` line.
    fn game(&self) -> Option<String> {
        let page: serde_json::Value = self.client.get("/games", &[]).ok()?;
        if self.clankermatch {
            cm_game_from(&page)
        } else {
            game_from(&page)
        }
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

/// The newest game of a `GET /v1/games` page as a Clankermatch run's
/// `bots-test: game` line: [`game_from`]'s, with the record's kind and
/// playlist.
pub fn cm_game_from(page: &serde_json::Value) -> Option<String> {
    let g = page["items"].as_array()?.first()?;
    let players = g["players"].as_array()?;
    let bots: Vec<&serde_json::Value> = players
        .iter()
        .filter(|p| p["bot"].as_bool() == Some(true))
        .collect();
    let accounts = bots.iter().filter(|p| p["xuid"].is_string()).count();
    Some(cm_verdict::game_line(
        g["game_id"].as_str().unwrap_or("?"),
        g["map"].as_str().unwrap_or("?"),
        (players.len(), bots.len(), accounts),
        g["kind"].as_str().unwrap_or("?"),
        g["playlist"].as_str().unwrap_or("-"),
    ))
}

/// `<from>/settings.toml`'s `[graphics]` table as a settings file of its
/// own, or `None` when it has none.
pub fn graphics_only(text: &str) -> Result<Option<(String, usize)>> {
    let table: toml::Table = text.parse().context("not TOML")?;
    let Some(toml::Value::Table(g)) = table.get("graphics") else {
        return Ok(None);
    };
    let n = g.len();
    let mut out = toml::Table::new();
    out.insert("graphics".into(), toml::Value::Table(g.clone()));
    Ok(Some((toml::to_string(&out)?, n)))
}

/// Writes `from`'s `[graphics]` into `data`'s settings.toml (which a run
/// starts without); the number of keys, or `None` without any.
fn copy_graphics(from: &Path, data: &Path) -> Result<Option<usize>> {
    let path = settings::path_in(from);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let Some((graphics, n)) =
        graphics_only(&text).with_context(|| format!("reading {}", path.display()))?
    else {
        return Ok(None);
    };
    fs::write(settings::path_in(data), graphics)?;
    Ok(Some(n))
}

/// Whether a log last written at `modified` is still being written at
/// `now`.
pub fn still_writing(modified: std::time::SystemTime, now: std::time::SystemTime) -> bool {
    now.duration_since(modified).is_ok_and(|d| d < CLANG_QUIET) || now < modified
}

/// `--wait-clang`: waits while a background clang build of `out`
/// (`boot::tier::background_build`) is still writing its log, so the
/// game's own `--tier clang` doesn't build alongside it.
fn wait_for_background_clang(out: &Path, interrupted: &std::sync::atomic::AtomicBool) {
    let log = boot::tier::background_log(out);
    let modified = || fs::metadata(&log).and_then(|m| m.modified()).ok();
    let mut said = false;
    while let Some(m) = modified() {
        if !still_writing(m, std::time::SystemTime::now())
            || interrupted.load(std::sync::atomic::Ordering::Relaxed)
        {
            break;
        }
        if !said {
            eprintln!(
                "bots test: --wait-clang: clang's background build is still writing {}; waiting",
                log.display()
            );
            said = true;
        }
        std::thread::sleep(Duration::from_secs(10));
    }
}

#[cfg(test)]
mod tests;
