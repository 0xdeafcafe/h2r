//! `h2 coop test`: co-op over Live on one Mac with one command
//! (docs/live/live-coop.md section 12). A Live server in this process, an
//! account per game (friends with the host), and two (or more) `h2 play`
//! games with their own data, Live directory, XNet ports and agent socket.
//! Their co-op lines go into one timestamped log, and the checklist of
//! section 11.3 is printed from it when the games close.
//!
//! `h2 coop verdict <log>` prints the checklist of a log again.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::coop_verdict::{self, Roles};
use crate::live_accounts::{self, LocalAccount};
use crate::live_cmd::{Client, FriendAction, LIVE_DIR_ENV, friend_run, status_of};

const USAGE: &str = "usage:
  h2 coop test [--disc ISO] [--players N] [--auto] [--root DIR] [--out DIR]
               [--ip ADDR] [--port N] [--server-port N] [--pad host|guest]
               [--extras] [--fresh] [--dry-run] [-- <more h2 play arguments>]
  h2 coop verdict <coop.log> [--auto]

Co-op over Live on this Mac (docs/live/live-coop.md section 12). Starts a
Live server in this process on 127.0.0.1:--server-port (8075), keeping its
data under --root; makes an account per game (Alpha hosts; Bravo, then
Charlie and Delta, join) that are friends with the host; and opens one
`h2 play` window per game (--players, 2 by default, at most 4), each with
its own data and Live directory under --root, its own XNet ports (--port,
31000, then 10 up per game, on --ip, this Mac's LAN address by default) and
its own agent socket (`h2 agent --socket <path> status`). The host's window
opens first; the others once it is signed in (its first start may build
the game into --out; the others never build at the same time).

Input: each window takes the keyboard while it has focus. The pads go to
the host (--pad guest gives them to the first guest; the others have
none). The guests play without sound.

--auto: the host invites the guests to its lobby by itself, and each guest
accepts by itself, as if pressed through the friends list. You still sign
each game in to Xbox Live, pick Co-op Campaign on the host, and start.

Every game's whole stderr goes to <root>/logs/<time>/<gamertag>.log, and
their co-op lines (coop-live, coop-probe, coop-auto, skulls, session) to
coop.log there, with the time since the start. When the last window
closes (or after Ctrl-C), the checklist prints: each step of section 11.3
as seen or not seen, where it stopped, and lines that look like trouble.
`h2 coop verdict` prints it again from a coop.log.

--root defaults to ~/Library/Application Support/h2recomp/coop-test, so
profiles and accounts last between runs; --fresh forgets the server and the
accounts (not the game data). --extras installs the map pack and Halo 1
into each game's data as `h2 play` does (off: co-op needs neither).
--dry-run prints what would run and starts nothing.";

/// Gamertags in game order: the host first.
pub const TAGS: [&str; 4] = ["Alpha", "Bravo", "Charlie", "Delta"];
const DEFAULT_PORT: u16 = 31000;
const PORT_STEP: u16 = 10;
const DEFAULT_SERVER_PORT: u16 = 8075;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Test(Opts),
    Verdict { log: PathBuf, auto: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opts {
    pub disc: PathBuf,
    pub players: usize,
    pub auto: bool,
    pub root: PathBuf,
    pub out: Option<PathBuf>,
    pub ip: Option<Ipv4Addr>,
    pub port: u16,
    pub server_port: u16,
    pub pad_on_guest: bool,
    pub extras: bool,
    pub fresh: bool,
    pub dry_run: bool,
    pub play_args: Vec<String>,
}

/// Parses `h2 coop ...`. `env` answers `H2_DISC` and `HOME`.
pub fn parse(args: Vec<String>, env: impl Fn(&str) -> Option<String>) -> Result<Cmd> {
    let mut it = args.into_iter();
    match it.next().as_deref() {
        Some("test") => {}
        Some("verdict") => {
            let mut log = None;
            let mut auto = false;
            for a in it {
                match a.as_str() {
                    "--auto" => auto = true,
                    _ if log.is_none() && !a.starts_with("--") => log = Some(PathBuf::from(a)),
                    _ => bail!("unexpected {a:?}\n\n{USAGE}"),
                }
            }
            let log = log.with_context(|| format!("verdict takes a coop.log\n\n{USAGE}"))?;
            return Ok(Cmd::Verdict { log, auto });
        }
        Some("-h" | "--help") | None => bail!("{USAGE}"),
        Some(other) => bail!("unknown `h2 coop {other}`\n\n{USAGE}"),
    }
    let mut disc = env("H2_DISC").map(PathBuf::from);
    let mut players = 2;
    let mut auto = false;
    let mut root = None;
    let mut out = None;
    let mut ip = None;
    let mut port = DEFAULT_PORT;
    let mut server_port = DEFAULT_SERVER_PORT;
    let mut pad_on_guest = false;
    let mut extras = false;
    let mut fresh = false;
    let mut dry_run = false;
    let mut play_args = Vec::new();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().with_context(|| format!("{name} needs a value"));
        match a.as_str() {
            "--disc" => disc = Some(PathBuf::from(value("--disc")?)),
            "--players" => {
                let v = value("--players")?;
                players = v
                    .parse()
                    .ok()
                    .filter(|n| (2..=TAGS.len()).contains(n))
                    .with_context(|| format!("--players {v}: 2 to {}", TAGS.len()))?;
            }
            "--auto" => auto = true,
            "--root" => root = Some(PathBuf::from(value("--root")?)),
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--ip" => {
                let v = value("--ip")?;
                ip = Some(
                    v.parse()
                        .with_context(|| format!("--ip {v}: an IPv4 address"))?,
                );
            }
            "--port" => {
                let v = value("--port")?;
                port = v.parse().with_context(|| format!("--port {v}: a port"))?;
            }
            "--server-port" => {
                let v = value("--server-port")?;
                server_port = v
                    .parse()
                    .with_context(|| format!("--server-port {v}: a port"))?;
            }
            "--pad" => match value("--pad")?.as_str() {
                "host" => pad_on_guest = false,
                "guest" => pad_on_guest = true,
                v => bail!("--pad {v}: host or guest"),
            },
            "--extras" => extras = true,
            "--fresh" => fresh = true,
            "--dry-run" => dry_run = true,
            "--" => {
                play_args.extend(it.by_ref());
            }
            "-h" | "--help" => bail!("{USAGE}"),
            _ => bail!("unexpected {a:?}\n\n{USAGE}"),
        }
    }
    let disc = disc.with_context(|| format!("no disc: --disc ISO or H2_DISC\n\n{USAGE}"))?;
    let last = u32::from(port) + u32::from(PORT_STEP) * (players as u32 - 1) + 6;
    if port == 0 || last > 65535 {
        bail!("--port {port}: the games need {port} to {last}");
    }
    let root = match root {
        Some(r) => r,
        None => default_root(env("HOME"))?,
    };
    Ok(Cmd::Test(Opts {
        disc,
        players,
        auto,
        root,
        out,
        ip,
        port,
        server_port,
        pad_on_guest,
        extras,
        fresh,
        dry_run,
        play_args,
    }))
}

fn default_root(home: Option<String>) -> Result<PathBuf> {
    let home = home
        .filter(|h| !h.is_empty())
        .context("HOME is not set; pass --root")?;
    Ok(PathBuf::from(home).join("Library/Application Support/h2recomp/coop-test"))
}

/// One game: what it is called, where it keeps things, and how it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    pub tag: String,
    pub host: bool,
    pub data: PathBuf,
    pub live: PathBuf,
    pub xnet: SocketAddrV4,
    pub agent: PathBuf,
    /// After `h2`.
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// The games of `opts` against the server at `server`, the XNet address
/// `ip`, with agent sockets in `sockets`.
pub fn plan(opts: &Opts, server: &str, ip: Ipv4Addr, sockets: &Path) -> Vec<Game> {
    let tags = &TAGS[..opts.players];
    let xnet: Vec<SocketAddrV4> = (0..opts.players)
        .map(|i| SocketAddrV4::new(ip, opts.port + PORT_STEP * i as u16))
        .collect();
    tags.iter()
        .enumerate()
        .map(|(i, tag)| {
            let host = i == 0;
            let dir = opts.root.join(tag.to_ascii_lowercase());
            let data = dir.join("data");
            let live = dir.join("live");
            let agent = sockets.join(format!("coop-{}.sock", tag.to_ascii_lowercase()));
            let mut args = vec![
                "play".to_string(),
                opts.disc.display().to_string(),
                "--data".into(),
                data.display().to_string(),
                "--live-server".into(),
                server.into(),
                "--gamertag".into(),
                tag.to_string(),
            ];
            if let Some(out) = &opts.out {
                args.extend(["--out".into(), out.display().to_string()]);
            }
            if !host {
                args.push("--no-audio".into());
            }
            let pads = if opts.pad_on_guest { i == 1 } else { host };
            if !pads {
                args.push("--no-gamepads".into());
            }
            if !opts.extras {
                args.extend(["--no-mappack".into(), "--no-halo1".into()]);
            }
            args.extend(opts.play_args.iter().cloned());
            let peers: Vec<String> = xnet
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, a)| a.to_string())
                .collect();
            let mut env = vec![
                (LIVE_DIR_ENV.to_string(), live.display().to_string()),
                ("H2_XNET_ADDR".into(), xnet[i].to_string()),
                ("H2_XNET_LOOPBACK_PEERS".into(), peers.join(",")),
                (agent_proto::ENV.into(), agent.display().to_string()),
            ];
            if opts.auto {
                if host {
                    env.push(("H2_COOP_AUTO_INVITE".into(), tags[1..].join(",")));
                } else {
                    env.push(("H2_COOP_AUTO_ACCEPT".into(), tags[0].into()));
                }
            }
            Game {
                tag: tag.to_string(),
                host,
                data,
                live,
                xnet: xnet[i],
                agent,
                args,
                env,
            }
        })
        .collect()
}

/// This Mac's LAN address (the one a packet to the internet would leave
/// from), else loopback.
fn lan_ip() -> Ipv4Addr {
    (|| {
        let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
        s.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
        match s.local_addr().ok()? {
            std::net::SocketAddr::V4(a) => Some(*a.ip()),
            _ => None,
        }
    })()
    .unwrap_or(Ipv4Addr::LOCALHOST)
}

/// A shell line for `game` (for the plan and the user to rerun one game).
pub fn shell_line(exe: &str, game: &Game) -> String {
    let quote = |s: &str| {
        if !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:,=@".contains(c))
        {
            s.to_string()
        } else {
            format!("'{}'", s.replace('\'', r"'\''"))
        }
    };
    let mut parts: Vec<String> = game
        .env
        .iter()
        .map(|(k, v)| format!("{k}={}", quote(v)))
        .collect();
    parts.push(quote(exe));
    parts.extend(game.args.iter().map(|a| quote(a)));
    parts.join(" ")
}

/// Makes sure each game has an account on the server, kept in its own
/// Live directory, and that every guest is the host's friend. Keeps what a
/// past run made.
pub fn accounts(client: &Client, games: &[Game], all: &Path) -> Result<Vec<LocalAccount>> {
    let mut made = Vec::new();
    for g in games {
        let path = g.live.join("accounts.json");
        let mut mine = live_accounts::load(&path)?;
        let found = mine
            .iter()
            .find(|a| {
                a.gamertag.eq_ignore_ascii_case(&g.tag)
                    && a.is_on(&client.base, &client.discovery.id)
            })
            .cloned();
        let account = match found {
            Some(a) => a,
            None => {
                let (created, key) = client.create_account(&g.tag, None).map_err(|e| {
                    match status_of(&e) {
                        Some(409) => anyhow::anyhow!(
                            "the server has {} but {} has no key for it; run again with --fresh",
                            g.tag,
                            path.display()
                        ),
                        _ => e,
                    }
                })?;
                let a = LocalAccount::new(
                    created.xuid,
                    created.gamertag.as_str(),
                    &client.base,
                    &client.discovery,
                    &key,
                );
                eprintln!("coop test: made {} ({})", a.gamertag, a.xuid);
                live_accounts::upsert(&mut mine, a.clone());
                fs::create_dir_all(&g.live)?;
                live_accounts::save(&path, &mine)?;
                a
            }
        };
        made.push(account);
    }
    live_accounts::save(all, &made)?;
    let host = &games[0].tag;
    for g in &games[1..] {
        let friends: live_proto::Friends = client.get(&format!("/accounts/{host}/friends"), &[])?;
        if friends
            .friends
            .iter()
            .any(|f| f.gamertag.as_str().eq_ignore_ascii_case(&g.tag))
        {
            continue;
        }
        if !friends
            .requests_out
            .iter()
            .any(|r| r.gamertag.as_str().eq_ignore_ascii_case(&g.tag))
        {
            friend_run(client, all, Some(host), FriendAction::Add, Some(&g.tag))?;
        }
        friend_run(client, all, Some(&g.tag), FriendAction::Accept, Some(host))?;
    }
    Ok(made)
}

pub fn run(args: Vec<String>) -> Result<()> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    match parse(args, env)? {
        Cmd::Verdict { log, auto } => {
            let text =
                fs::read_to_string(&log).with_context(|| format!("reading {}", log.display()))?;
            let v = coop_verdict::verdict(&text, auto)
                .with_context(|| format!("{} has no `# host` line", log.display()))?;
            print!("{}", v.render());
            Ok(())
        }
        Cmd::Test(opts) => test(opts),
    }
}

fn test(opts: Opts) -> Result<()> {
    let exe = std::env::current_exe().context("finding this h2")?;
    let ip = opts.ip.unwrap_or_else(lan_ip);
    let server = format!("http://127.0.0.1:{}", opts.server_port);
    let sockets = agent_proto::default_socket()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let games = plan(&opts, &server, ip, &sockets);
    let exe_s = exe.display().to_string();
    if opts.dry_run {
        println!(
            "server: {server} (data in {})",
            opts.root.join("server").display()
        );
        for g in &games {
            println!("{}: {}", g.tag, shell_line(&exe_s, g));
        }
        return Ok(());
    }
    if !opts.disc.is_file() {
        bail!("{} is not a file", opts.disc.display());
    }
    // Before any other thread starts (the server's), so none of them
    // takes a Ctrl-C meant for the games.
    let interrupted = watch_interrupts();
    let server_data = opts.root.join("server");
    if opts.fresh {
        let _ = fs::remove_dir_all(&server_data);
        for g in &games {
            let _ = fs::remove_dir_all(&g.live);
        }
    }
    fs::create_dir_all(&server_data)?;
    fs::create_dir_all(&sockets)?;
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
    accounts(&client, &games, &opts.root.join("accounts.json"))?;
    let stamp = stamp(crate::live_cmd::now());
    let logs = opts.root.join("logs").join(stamp);
    fs::create_dir_all(&logs)?;
    let roles = Roles {
        host: games[0].tag.clone(),
        guests: games[1..].iter().map(|g| g.tag.clone()).collect(),
    };
    eprintln!(
        "coop test: Live server {} ({} store), web app at {}/",
        embedded.url, embedded.store, embedded.url
    );
    for g in &games {
        eprintln!(
            "coop test: {}{}: data {}, XNet {}, agent `h2 agent --socket {} status`",
            g.tag,
            if g.host { " (host)" } else { "" },
            g.data.display(),
            g.xnet,
            g.agent.display()
        );
    }
    eprintln!("coop test: logs in {}", logs.display());
    let merged = logs.join("coop.log");
    let mut sup = Supervisor::new(&merged, &logs, &roles)?;
    sup.spawn(&exe, &games[0])?;
    eprintln!(
        "coop test: {} opening first (it builds the game if --out has none)",
        games[0].tag
    );
    let mut waiting: Vec<&Game> = games[1..].iter().collect();
    loop {
        let ready = sup.pump(Duration::from_millis(200))?;
        if ready.contains(&0) && !waiting.is_empty() {
            for g in waiting.drain(..) {
                sup.spawn(&exe, g)?;
            }
        }
        if sup.all_exited() {
            break;
        }
        if sup.exited(0) && !waiting.is_empty() {
            eprintln!(
                "coop test: {} closed before it signed in; not opening the others",
                games[0].tag
            );
            waiting.clear();
        }
        if interrupted.load(std::sync::atomic::Ordering::Relaxed) {
            sup.note_interrupt();
        }
    }
    drop(embedded);
    let text = fs::read_to_string(&merged)?;
    if let Some(v) = coop_verdict::verdict(&text, opts.auto) {
        print!("{}", v.render());
    }
    println!("log: {}", merged.display());
    Ok(())
}

/// A UTC `YYYYMMDD-HHMMSS` for unix time `secs`, naming a run's logs.
pub fn stamp(secs: i64) -> String {
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Blocks SIGINT and SIGTERM here and notes them on a thread, so Ctrl-C
/// stops the games (they get it too, from the terminal) and this process
/// still prints the checklist. The games start with the mask reset.
#[cfg(unix)]
fn watch_interrupts() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let flag = std::sync::Arc::new(AtomicBool::new(false));
    // SAFETY: plain libc signal-mask calls on a local, initialised set.
    let set = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    };
    let f = flag.clone();
    let _ = std::thread::Builder::new()
        .name("coop-signals".into())
        .spawn(move || {
            loop {
                let mut sig = 0;
                // SAFETY: `set` is a valid set; sigwait only writes `sig`.
                if unsafe { libc::sigwait(&set, &mut sig) } != 0 {
                    return;
                }
                if f.swap(true, Ordering::Relaxed) {
                    eprintln!("coop test: interrupted twice; exiting");
                    std::process::exit(130);
                }
            }
        });
    flag
}

#[cfg(not(unix))]
fn watch_interrupts() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    Default::default()
}

enum Event {
    Line(usize, String),
    Closed(usize),
}

/// The games' processes and their logs: every line of game `i` into its
/// own log, the wanted ones into the merged log (and the terminal).
pub struct Supervisor {
    start: Instant,
    merged: File,
    logs: PathBuf,
    games: Vec<(String, Child, File, usize)>,
    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
    ready: HashMap<usize, bool>,
    interrupted: bool,
}

/// Game `tag`'s line saying it is signed in (after any build).
fn signed_in(tag: &str, line: &str) -> bool {
    line.starts_with("live: playing as ") && line["live: playing as ".len()..].starts_with(tag)
}

impl Supervisor {
    pub fn new(merged: &Path, logs: &Path, roles: &Roles) -> Result<Self> {
        let mut file = File::create(merged)?;
        file.write_all(roles.header().as_bytes())?;
        let (tx, rx) = mpsc::channel();
        Ok(Supervisor {
            start: Instant::now(),
            merged: file,
            logs: logs.into(),
            games: Vec::new(),
            tx,
            rx,
            ready: HashMap::new(),
            interrupted: false,
        })
    }

    /// Starts `game` with `program` (the `h2` binary; tests pass a shell).
    pub fn spawn(&mut self, program: &Path, game: &Game) -> Result<()> {
        let mut cmd = Command::new(program);
        cmd.args(&game.args)
            .envs(game.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        self.spawn_command(cmd, &game.tag)
    }

    pub fn spawn_command(&mut self, mut cmd: Command, tag: &str) -> Result<()> {
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting {tag}'s game"))?;
        let i = self.games.len();
        let streams = [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        ];
        let mut open = 0;
        for s in streams.into_iter().flatten() {
            open += 1;
            let tx = self.tx.clone();
            std::thread::Builder::new()
                .name(format!("coop-{tag}"))
                .spawn(move || {
                    for line in BufReader::new(s).lines() {
                        let Ok(line) = line else { break };
                        if tx.send(Event::Line(i, line)).is_err() {
                            return;
                        }
                    }
                    let _ = tx.send(Event::Closed(i));
                })?;
        }
        let log = File::create(self.logs.join(format!("{tag}.log")))?;
        self.games.push((tag.to_string(), child, log, open));
        Ok(())
    }

    /// Handles what came in for up to `wait`; the games that signed in
    /// during it.
    pub fn pump(&mut self, wait: Duration) -> Result<Vec<usize>> {
        let mut ready = Vec::new();
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let ev = match self.rx.recv_timeout(left) {
                Ok(ev) => ev,
                Err(_) => break,
            };
            match ev {
                Event::Line(i, line) => {
                    let secs = self.start.elapsed().as_secs_f64();
                    let (tag, _, log, _) = &mut self.games[i];
                    writeln!(log, "{line}")?;
                    if coop_verdict::wanted(&line) {
                        let merged = coop_verdict::format_line(secs, tag, &line);
                        writeln!(self.merged, "{merged}")?;
                        eprintln!("{merged}");
                    }
                    if signed_in(tag, &line) && self.ready.insert(i, true).is_none() {
                        ready.push(i);
                    }
                }
                Event::Closed(i) => {
                    let g = &mut self.games[i];
                    g.3 = g.3.saturating_sub(1);
                }
            }
        }
        self.merged.flush()?;
        Ok(ready)
    }

    pub fn exited(&mut self, i: usize) -> bool {
        let Some((_, child, _, open)) = self.games.get_mut(i) else {
            return false;
        };
        *open == 0 && !matches!(child.try_wait(), Ok(None))
    }

    pub fn all_exited(&mut self) -> bool {
        (0..self.games.len()).all(|i| self.exited(i))
    }

    fn note_interrupt(&mut self) {
        if !self.interrupted {
            self.interrupted = true;
            eprintln!("coop test: interrupted; waiting for the games to close (again to stop now)");
        }
    }
}

#[cfg(test)]
mod tests;
