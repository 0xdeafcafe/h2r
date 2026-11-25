//! Consent to use the playing Mac (docs/runtime/lab.md). A job holds the
//! lab activity lock while waiting. The game stays under the player's
//! control: only their Close and start lets the lab close it, through the
//! game's own `host.quit`; Start alongside leaves it open.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_proto::{Client, discover, host_quit, takeover};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

const SCAN_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const QUIT_TIMEOUT: Duration = Duration::from_secs(60);
const DECLINED: &str = "declined by the player";
const UNAVAILABLE: &str = "player consent unavailable while a game window is open";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub timeout_seconds: u32,
    pub cooldown_seconds: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout_seconds: 60,
            cooldown_seconds: 30 * 60,
        }
    }
}

pub(super) enum Outcome {
    Proceed,
    /// The player chose Start alongside for these games: the job starts
    /// with them open, and none of them is asked again.
    Alongside(Vec<GameKey>),
    Skipped(&'static str),
    Stopped,
    /// The player allowed the job, but their game is still open: the lab
    /// couldn't close it. The job waits for `game` to close, and `reason`
    /// says why and what the player can do.
    Held { reason: String, game: GameKey },
}

pub(super) struct Gate {
    config: Config,
    /// The player's published game sockets.
    agents: PathBuf,
    lab_sockets: PathBuf,
    current_h2: PathBuf,
    declined_until_file: PathBuf,
    declined_until: Mutex<u64>,
    prompt: Mutex<()>,
}

impl Gate {
    pub(super) fn new(config: Config, root: &Path, current_h2: &Path, agents: &Path) -> Self {
        let declined_until_file = root.join("takeover-declined-until");
        let declined_until = fs::read_to_string(&declined_until_file)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        Self {
            config,
            agents: agents.to_owned(),
            lab_sockets: root.join("a"),
            current_h2: current_h2.to_owned(),
            declined_until_file,
            declined_until: Mutex::new(declined_until),
            prompt: Mutex::new(()),
        }
    }

    /// `kept`: games the player already let this job run alongside; they
    /// aren't asked again.
    pub(super) fn admit(
        &self,
        mut request: takeover::Request,
        kept: &[GameKey],
        phase: impl Fn(&str),
        stopped: impl Fn() -> bool,
        owns_game: impl Fn(Option<&Path>, Option<u32>) -> bool,
    ) -> Outcome {
        // The game shows one prompt at a time. A queued job can still stop.
        let _prompt = loop {
            if stopped() {
                return Outcome::Stopped;
            }
            match self.prompt.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(POLL_INTERVAL),
                Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
            }
        };
        let timeout = Duration::from_secs(self.config.timeout_seconds.into());
        // Multiple windows may need consent. Bound the whole handoff even
        // if the player opens a different window after agreeing.
        let deadline = Instant::now() + timeout + QUIT_TIMEOUT;
        let mut alongside = kept.to_vec();
        loop {
            if stopped() {
                return Outcome::Stopped;
            }
            let mut games = match self.windows(&owns_game) {
                Ok(games) => games,
                Err(error) => {
                    eprintln!("lab: checking for game windows: {error:#}");
                    return Outcome::Skipped("could not check for an active game window");
                }
            };
            games.retain(|game| !alongside.contains(&game.key()));
            let Some(game) = games.first() else {
                return if alongside.is_empty() {
                    Outcome::Proceed
                } else {
                    Outcome::Alongside(alongside)
                };
            };
            if Instant::now() >= deadline || self.cooling_down() {
                return Outcome::Skipped(DECLINED);
            }
            if games.iter().any(|game| !game.can_ask) {
                return Outcome::Skipped(UNAVAILABLE);
            }
            let Some(path) = &game.socket else {
                return Outcome::Skipped(UNAVAILABLE);
            };
            phase("awaiting_consent");
            let answer_by = (Instant::now() + timeout).min(deadline);
            let remaining = answer_by.saturating_duration_since(Instant::now());
            request.timeout_seconds = remaining
                .as_secs()
                .saturating_add(u64::from(remaining.subsec_nanos() != 0))
                .clamp(1, u64::from(self.config.timeout_seconds))
                as u32;
            let Ok(reply) = call::<takeover::Requested>(
                path,
                takeover::REQUEST,
                json!(request),
                discover::PROBE_TIMEOUT.min(remaining),
            ) else {
                return Outcome::Skipped(UNAVAILABLE);
            };
            match answer(path, reply.id, answer_by, &stopped) {
                Some(takeover::Decision::Allowed) => {}
                // Not a decline: no cool-down. The game stays open.
                Some(takeover::Decision::Alongside) if request.alongside => {
                    alongside.push(game.key());
                    continue;
                }
                Some(takeover::Decision::Unavailable) => return Outcome::Skipped(UNAVAILABLE),
                None => return Outcome::Stopped,
                Some(_) => {
                    self.remember_decline();
                    return Outcome::Skipped(DECLINED);
                }
            }
            // Allow closes the game: ask it to, then wait for it to go.
            phase("awaiting_quit");
            let closed = close_allowed(
                || quit(path),
                || match self.windows(&owns_game) {
                    Ok(games) => Ok(games.iter().any(|candidate| game.same_as(candidate))),
                    Err(error) => Err(format!("{error:#}")),
                },
                &stopped,
                Instant::now() + QUIT_TIMEOUT,
                Instant::now,
                std::thread::sleep,
            );
            match closed {
                Closed::Gone => {}
                Closed::Stopped => return Outcome::Stopped,
                Closed::StillOpen(reason) => {
                    return Outcome::Held {
                        reason,
                        game: game.key(),
                    };
                }
            }
        }
    }

    fn cooling_down(&self) -> bool {
        self.config.cooldown_seconds != 0 && unix_seconds() < *self.declined_until.lock().unwrap()
    }

    fn remember_decline(&self) {
        if self.config.cooldown_seconds == 0 {
            return;
        }
        let until = unix_seconds() + u64::from(self.config.cooldown_seconds);
        *self.declined_until.lock().unwrap() = until;
        let temp = self.declined_until_file.with_extension("new");
        let saved = fs::write(&temp, format!("{until}\n"))
            .and_then(|()| fs::rename(&temp, &self.declined_until_file));
        if let Err(error) = saved {
            eprintln!("lab: saving takeover cool-down: {error}");
        }
    }

    /// The player's open games: the game windows on this Mac that
    /// `owns_game` doesn't claim, as the start consent finds them.
    pub(super) fn player_games(
        &self,
        owns_game: &impl Fn(Option<&Path>, Option<u32>) -> bool,
    ) -> Result<Vec<Seen>> {
        Ok(self
            .windows(owns_game)?
            .into_iter()
            .filter_map(|game| {
                if game.pid.is_none() && game.socket.is_none() {
                    return None;
                }
                Some(Seen {
                    key: game.key(),
                    socket: game.socket.filter(|_| game.can_ask),
                })
            })
            .collect())
    }

    /// Asks the game at `path` what the running job should do; the
    /// request's id, or None when the game didn't take it.
    pub(super) fn ask_running(&self, path: &Path, mut request: takeover::Request) -> Option<u64> {
        request.ask = takeover::Ask::Running;
        request.timeout_seconds = self.config.timeout_seconds.max(1);
        call::<takeover::Requested>(
            path,
            takeover::REQUEST,
            json!(request),
            discover::PROBE_TIMEOUT,
        )
        .ok()
        .map(|reply| reply.id)
    }

    /// How long the player has to answer, the game's dialog timeout.
    pub(super) fn answer_timeout(&self) -> Duration {
        Duration::from_secs(self.config.timeout_seconds.max(1).into())
    }

    fn windows(
        &self,
        owns_game: &impl Fn(Option<&Path>, Option<u32>) -> bool,
    ) -> Result<Vec<Game>> {
        let agents = &self.agents;
        let mut games = Vec::new();
        let mut known = BTreeSet::new();
        let deadline = Instant::now() + SCAN_TIMEOUT;
        for (dir, published) in [(agents, true), (&self.lab_sockets, false)] {
            let paths = match fs::read_dir(dir) {
                Ok(entries) => entries
                    .collect::<std::io::Result<Vec<_>>>()?
                    .into_iter()
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.extension().is_some_and(|ext| ext == "sock")
                            && discover::is_socket(path)
                    })
                    .collect::<BTreeSet<_>>(),
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error).with_context(|| format!("listing {}", dir.display()));
                }
            };
            for path in paths {
                let published_pid = published.then(|| discover::pid_of(&path)).flatten();
                if owns_game(Some(&path), published_pid) {
                    continue;
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                ensure!(!remaining.is_zero(), "game agents did not answer in time");
                match probe(&path, remaining.min(discover::PROBE_TIMEOUT)) {
                    Probe::Active(info) => {
                        let pid = info.pid.or(published_pid);
                        if !owns_game(Some(&path), pid)
                            && pid.is_none_or(|pid| known.insert(pid))
                            && info.window
                        {
                            games.push(Game {
                                socket: Some(path),
                                pid,
                                can_ask: true,
                            });
                        }
                    }
                    Probe::Unavailable => games.push(Game {
                        socket: Some(path),
                        pid: published_pid,
                        can_ask: false,
                    }),
                    Probe::Stale => {}
                }
            }
        }
        let processes = processes(&self.current_h2)?;
        games.retain(|game| {
            game.can_ask
                || !game.pid.is_some_and(|pid| {
                    known.contains(&pid) || processes.get(&pid) == Some(&Process::Headless)
                })
        });
        // This also covers startup before a socket is listening and players
        // with the agent switched off. A known headless process is harmless.
        for (pid, kind) in processes {
            if kind == Process::Windowed
                && !owns_game(None, Some(pid))
                && !known.contains(&pid)
                && !games.iter().any(|game| game.pid == Some(pid))
            {
                games.push(Game {
                    socket: None,
                    pid: Some(pid),
                    can_ask: false,
                });
            }
        }
        Ok(games)
    }
}

/// The decision on the request `id` the game at `path` holds; None when the
/// game didn't answer.
pub(super) fn poll(path: &Path, id: u64) -> Option<takeover::Decision> {
    call::<takeover::Status>(
        path,
        takeover::STATUS,
        json!(takeover::StatusParams { id }),
        discover::PROBE_TIMEOUT,
    )
    .ok()
    .map(|status| status.decision)
}

/// A game of the player's, known by its process when it has one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum GameKey {
    Pid(u32),
    Socket(PathBuf),
}

/// One of the player's games in a look at the Mac.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Seen {
    pub key: GameKey,
    /// Where to ask it; None while it can't be asked (starting up, its
    /// agent switched off, not answering).
    pub socket: Option<PathBuf>,
}

/// What the player chose for a running job when they opened a game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Choice {
    /// Stop the job now, put it back at the front of the queue and hold
    /// the queue while the game is open.
    StopAndPause,
    /// The job and the queue carry on alongside the game.
    KeepRunning,
    /// Let the job finish, then hold the rest of the queue while the game
    /// is open. What no answer picks.
    FinishThis,
}

/// A choice settled in one look: the player's own, or the default when they
/// gave none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Settled {
    pub choice: Choice,
    pub answered: bool,
}

impl Settled {
    /// The line the job's log and summary get.
    pub(super) fn note(self) -> String {
        let what = match (self.choice, self.answered) {
            (Choice::StopAndPause, _) => {
                "chose Stop and pause: the job goes back to the front of the queue, \
                 held until their game closes"
            }
            (Choice::KeepRunning, _) => "chose Keep running: the job and the queue carry on",
            (Choice::FinishThis, true) => {
                "chose Finish this one: the rest of the queue waits until their game closes"
            }
            (Choice::FinishThis, false) => {
                "gave no answer, so the job finishes and the rest of the queue waits until \
                 their game closes"
            }
        };
        format!("player: opened a game and {what}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    /// Seen, not asked yet: retried each look until `until`.
    Unasked {
        until: Instant,
    },
    /// The question is open in the game.
    Asking {
        id: u64,
        until: Instant,
    },
    Chosen(Choice),
}

/// Slack past the dialog's own timeout before the host stops waiting for a
/// game that doesn't report one.
const ANSWER_SLACK: Duration = Duration::from_secs(10);

/// The player's games and what each answered, asked once per game. Pure:
/// the caller looks at the Mac and passes the asking and polling in.
#[derive(Debug, Default)]
pub(super) struct PlayerGames {
    games: BTreeMap<GameKey, Answer>,
}

impl PlayerGames {
    /// One look while a job runs. Forgets games that closed, asks each new
    /// game once (`ask`: the game's socket to the request id), polls open
    /// questions (`poll`), and returns what this look settled. A game that
    /// can't be asked, or doesn't answer, within `timeout` gets
    /// [`Choice::FinishThis`].
    pub(super) fn observe(
        &mut self,
        seen: &[Seen],
        now: Instant,
        timeout: Duration,
        ask: impl FnMut(&Path) -> Option<u64>,
        poll: impl FnMut(&Path, u64) -> Option<takeover::Decision>,
    ) -> Vec<Settled> {
        self.look(seen, now, timeout, true, ask, poll)
    }

    /// One look between jobs: forgets games that closed and polls open
    /// questions, but asks nothing new, since no job runs.
    pub(super) fn refresh(
        &mut self,
        seen: &[Seen],
        now: Instant,
        timeout: Duration,
        poll: impl FnMut(&Path, u64) -> Option<takeover::Decision>,
    ) -> Vec<Settled> {
        self.look(seen, now, timeout, false, |_| None, poll)
    }

    /// Holds the queue for `game` until it closes: the player allowed a job
    /// but the lab couldn't close their game ([`Outcome::Held`]).
    pub(super) fn hold(&mut self, game: GameKey) {
        self.games.insert(game, Answer::Chosen(Choice::FinishThis));
    }

    /// The player started a job alongside `game`: it counts as asked, with
    /// the answer Keep running.
    pub(super) fn keep_running(&mut self, game: GameKey) {
        self.games.insert(game, Answer::Chosen(Choice::KeepRunning));
    }

    /// Whether the queue waits: a game of the player's is open that hasn't
    /// answered Keep running.
    pub(super) fn holds(&self) -> bool {
        self.games
            .values()
            .any(|answer| *answer != Answer::Chosen(Choice::KeepRunning))
    }

    fn look(
        &mut self,
        seen: &[Seen],
        now: Instant,
        timeout: Duration,
        new_games: bool,
        mut ask: impl FnMut(&Path) -> Option<u64>,
        mut poll: impl FnMut(&Path, u64) -> Option<takeover::Decision>,
    ) -> Vec<Settled> {
        self.games
            .retain(|key, _| seen.iter().any(|game| &game.key == key));
        let mut settled = Vec::new();
        for game in seen {
            let answer = match self.games.get(&game.key) {
                Some(answer) => *answer,
                None if new_games => Answer::Unasked {
                    until: now + timeout,
                },
                None => continue,
            };
            let socket = game.socket.as_deref();
            let settle =
                |choice, answered| (Answer::Chosen(choice), Some(Settled { choice, answered }));
            let (next, settles) = match answer {
                Answer::Chosen(_) => continue,
                Answer::Unasked { until } => match socket.and_then(&mut ask) {
                    Some(id) => (
                        Answer::Asking {
                            id,
                            until: now + timeout + ANSWER_SLACK,
                        },
                        None,
                    ),
                    None if now >= until => settle(Choice::FinishThis, false),
                    None => (answer, None),
                },
                Answer::Asking { id, until } => match socket.and_then(|path| poll(path, id)) {
                    Some(takeover::Decision::StopAndPause) => settle(Choice::StopAndPause, true),
                    Some(takeover::Decision::KeepRunning) => settle(Choice::KeepRunning, true),
                    Some(takeover::Decision::FinishThis) => settle(Choice::FinishThis, true),
                    Some(takeover::Decision::Pending) | None if now < until => (answer, None),
                    // Timed out, dismissed, or a game that can't show the
                    // question.
                    _ => settle(Choice::FinishThis, false),
                },
            };
            settled.extend(settles);
            self.games.insert(game.key.clone(), next);
        }
        settled
    }
}

/// How closing a game the player allowed the lab to take ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Closed {
    /// The game exited: the job starts.
    Gone,
    /// The game is still open: why, and what the player can do. The job
    /// waits for it to close.
    StillOpen(String),
    /// The job was stopped meanwhile.
    Stopped,
}

/// What the player can do when the lab couldn't close their game.
const CLOSE_IT_YOURSELF: &str =
    "quit it from its menu or close its window, and the job starts once it has closed";

/// After the player's Allow: asks the game to close (`quit`), then waits
/// until `open` no longer finds it, until `quit_by`. A game that refused
/// or didn't take the request but has gone anyway counts as gone. Pure:
/// the caller passes the asking, the looking, the clock and the sleep in.
pub(super) fn close_allowed(
    quit: impl FnOnce() -> Result<(), String>,
    mut open: impl FnMut() -> Result<bool, String>,
    stopped: &impl Fn() -> bool,
    quit_by: Instant,
    mut now: impl FnMut() -> Instant,
    mut sleep: impl FnMut(Duration),
) -> Closed {
    let asked = quit();
    loop {
        if stopped() {
            return Closed::Stopped;
        }
        match open() {
            Ok(false) => return Closed::Gone,
            Ok(true) => {}
            Err(error) => {
                eprintln!("lab: checking if the game closed: {error}");
                return Closed::StillOpen(format!(
                    "allowed the job, but the lab could not check whether their game \
                     closed ({error}); {CLOSE_IT_YOURSELF}"
                ));
            }
        }
        if let Err(error) = &asked {
            return Closed::StillOpen(format!(
                "allowed the job, but the lab could not close their game ({error}); \
                 {CLOSE_IT_YOURSELF}"
            ));
        }
        let remaining = quit_by.saturating_duration_since(now());
        if remaining.is_zero() {
            return Closed::StillOpen(format!(
                "allowed the job, but their game did not close within {} seconds of \
                 being asked; {CLOSE_IT_YOURSELF}",
                QUIT_TIMEOUT.as_secs()
            ));
        }
        sleep(POLL_INTERVAL.min(remaining));
    }
}

/// Asks the game at `path` to close its window (`host.quit`).
fn quit(path: &Path) -> Result<(), String> {
    let reply = call::<host_quit::QuitReply>(
        path,
        host_quit::method::QUIT,
        json!({}),
        discover::PROBE_TIMEOUT,
    )
    .map_err(|error| format!("{error:#}"))?;
    if reply.closing {
        Ok(())
    } else {
        Err("the game would not close".to_owned())
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

struct Game {
    socket: Option<PathBuf>,
    pid: Option<u32>,
    can_ask: bool,
}

impl Game {
    /// Known by its process when it has one, else by its socket. Every
    /// game the scan finds has one of the two.
    fn key(&self) -> GameKey {
        match (self.pid, &self.socket) {
            (Some(pid), _) => GameKey::Pid(pid),
            (None, Some(path)) => GameKey::Socket(path.clone()),
            (None, None) => GameKey::Socket(PathBuf::new()),
        }
    }

    fn same_as(&self, other: &Self) -> bool {
        self.pid.is_some_and(|pid| other.pid == Some(pid))
            || self
                .socket
                .as_ref()
                .is_some_and(|path| other.socket.as_ref() == Some(path))
    }
}

enum Probe {
    Active(takeover::Info),
    Unavailable,
    Stale,
}

fn probe(path: &Path, timeout: Duration) -> Probe {
    let mut client = match Client::connect(path) {
        Ok(client) => client,
        Err(error) => {
            return match error.kind() {
                ErrorKind::ConnectionRefused | ErrorKind::NotFound => Probe::Stale,
                _ => Probe::Unavailable,
            };
        }
    };
    if client.set_timeout(Some(timeout)).is_err() {
        return Probe::Unavailable;
    }
    client
        .call(takeover::INFO, Value::Null)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
        .map_or(Probe::Unavailable, Probe::Active)
}

fn call<T: serde::de::DeserializeOwned>(
    path: &Path,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<T> {
    ensure!(!timeout.is_zero(), "the takeover request expired");
    let mut client = Client::connect(path)?;
    client.set_timeout(Some(timeout))?;
    Ok(serde_json::from_value(client.call(method, params)?)?)
}

fn answer(
    path: &Path,
    id: u64,
    deadline: Instant,
    stopped: &impl Fn() -> bool,
) -> Option<takeover::Decision> {
    loop {
        if stopped() {
            return None;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Some(takeover::Decision::TimedOut);
        }
        if let Ok(reply) = call::<takeover::Status>(
            path,
            takeover::STATUS,
            json!(takeover::StatusParams { id }),
            remaining.min(discover::PROBE_TIMEOUT),
        ) && reply.decision != takeover::Decision::Pending
        {
            return Some(reply.decision);
        }
        std::thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

/// The same user's recognizable game processes, including a game the
/// lab launched in another root. Inspect process arguments only for
/// discovery; the gate never signals a process or reads its memory.
fn processes(current_h2: &Path) -> Result<BTreeMap<u32, Process>> {
    let output = Command::new("/bin/ps")
        .args(["-axww", "-o", "pid=", "-o", "uid=", "-o", "comm="])
        .output()
        .context("listing processes with ps")?;
    ensure!(output.status.success(), "ps exited with {}", output.status);
    // SAFETY: getuid reads the real user id and takes no arguments.
    let uid = unsafe { libc::getuid() };
    // Identify the executable independently of argv. A shell or editor can
    // have the text of an h2 invocation in its arguments.
    let candidates: BTreeSet<u32> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (pid, rest) = line.trim_start().split_once(char::is_whitespace)?;
            let (owner, executable) = rest.trim_start().split_once(char::is_whitespace)?;
            let executable = Path::new(executable.trim());
            if owner.parse::<u32>().ok()? != uid
                || !(is_cli(executable, current_h2)
                    || executable.file_name().is_some_and(|name| name == "Halo2"))
            {
                return None;
            }
            pid.parse().ok()
        })
        .collect();
    if candidates.is_empty() {
        return Ok(BTreeMap::new());
    }
    let output = Command::new("/bin/ps")
        .args(["-axww", "-o", "pid=", "-o", "command="])
        .output()
        .context("listing game process arguments with ps")?;
    ensure!(output.status.success(), "ps exited with {}", output.status);
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (pid, command) = line.trim_start().split_once(char::is_whitespace)?;
            let pid = pid.parse().ok()?;
            if !candidates.contains(&pid) {
                return None;
            }
            Some((pid, process_role(command.trim_start(), current_h2)?))
        })
        .collect())
}

fn is_cli(executable: &Path, current_h2: &Path) -> bool {
    executable.file_name().is_some_and(|name| {
        Some(name) == current_h2.file_name()
            || name
                .to_str()
                .is_some_and(|name| name == "h2" || name.starts_with("h2-"))
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Process {
    Windowed,
    Headless,
}

fn process_role(command: &str, current_h2: &Path) -> Option<Process> {
    if let Some((bundle, args)) = command.split_once("/Contents/MacOS/Halo2")
        && bundle.ends_with(".app")
        && (args.is_empty() || args.starts_with(" -psn_"))
    {
        return Some(Process::Windowed);
    }
    // The executable path may contain spaces. Find the subcommand by
    // matching its immediately preceding executable rather than splitting
    // the path at the first space.
    for (index, _) in command.match_indices(' ') {
        let executable = Path::new(&command[..index]);
        if !is_cli(executable, current_h2) {
            continue;
        }
        match command[index..].split_whitespace().next()? {
            "play" => return Some(Process::Windowed),
            "serve" | "run" | "sim" => return Some(Process::Headless),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_roles_distinguish_play_from_headless_commands() {
        let current = Path::new("/my tools/custom");
        for command in [
            "/Applications/Halo 2.app/Contents/MacOS/Halo2",
            "/Applications/Halo 2.app/Contents/MacOS/Halo2 -psn_1",
            "/usr/local/bin/h2 play --level mainmenu",
            "/path with spaces/h2-dev play iso",
            "/path/h2-source folder/target/release/h2 play iso",
            "/my tools/custom play iso",
        ] {
            assert_eq!(
                process_role(command, current),
                Some(Process::Windowed),
                "{command}"
            );
        }
        for command in [
            "h2 serve --socket /path",
            "/usr/local/bin/h2 sim",
            "/my tools/custom run iso",
        ] {
            assert_eq!(
                process_role(command, current),
                Some(Process::Headless),
                "{command}"
            );
        }
        for command in ["h2 lab run --current", "h2 agent a", "cargo build -p cli"] {
            assert_eq!(process_role(command, current), None, "{command}");
        }
    }

    use std::cell::Cell;
    use takeover::Decision;

    const TIMEOUT: Duration = Duration::from_secs(60);

    fn game(pid: u32) -> Seen {
        Seen {
            key: GameKey::Pid(pid),
            socket: Some(PathBuf::from(format!("/a/{pid}.sock"))),
        }
    }

    /// One look that asks (request id 7) and answers every poll with
    /// `decision`; also counts the asks and polls.
    fn look(
        games: &mut PlayerGames,
        seen: &[Seen],
        now: Instant,
        decision: Option<Decision>,
        asks: &Cell<u32>,
        polls: &Cell<u32>,
    ) -> Vec<Settled> {
        games.observe(
            seen,
            now,
            TIMEOUT,
            |_| {
                asks.set(asks.get() + 1);
                Some(7)
            },
            |_, id| {
                assert_eq!(id, 7);
                polls.set(polls.get() + 1);
                decision
            },
        )
    }

    #[test]
    fn each_answer_settles_once_and_only_keep_running_frees_the_queue() {
        for (decision, choice, holds) in [
            (Decision::StopAndPause, Choice::StopAndPause, true),
            (Decision::KeepRunning, Choice::KeepRunning, false),
            (Decision::FinishThis, Choice::FinishThis, true),
        ] {
            let (asks, polls) = (Cell::new(0), Cell::new(0));
            let mut games = PlayerGames::default();
            let now = Instant::now();
            let seen = [game(42)];
            // The first look asks; the question is open and holds the queue.
            assert!(look(&mut games, &seen, now, Some(decision), &asks, &polls).is_empty());
            assert_eq!((asks.get(), polls.get()), (1, 0));
            assert!(games.holds());
            // The next look reads the answer.
            assert_eq!(
                look(&mut games, &seen, now, Some(decision), &asks, &polls),
                [Settled {
                    choice,
                    answered: true
                }]
            );
            assert_eq!(games.holds(), holds, "{choice:?}");
            // Asked once per game: later looks neither ask nor poll.
            for _ in 0..3 {
                assert!(look(&mut games, &seen, now, Some(decision), &asks, &polls).is_empty());
            }
            assert_eq!((asks.get(), polls.get()), (1, 1));
            // The game closes: forgotten, and the queue is free.
            assert!(look(&mut games, &[], now, None, &asks, &polls).is_empty());
            assert!(!games.holds());
        }
    }

    #[test]
    fn a_game_the_job_started_alongside_is_already_asked_and_frees_the_queue() {
        let (asks, polls) = (Cell::new(0), Cell::new(0));
        let mut games = PlayerGames::default();
        let now = Instant::now();
        let seen = [game(42)];
        games.keep_running(GameKey::Pid(42));
        assert!(!games.holds());
        // The job's monitor never asks it again.
        for _ in 0..3 {
            assert!(look(&mut games, &seen, now, Some(Decision::FinishThis), &asks, &polls)
                .is_empty());
        }
        assert_eq!((asks.get(), polls.get()), (0, 0));
        assert!(!games.holds());
        // When it closes it is forgotten, like any other.
        assert!(look(&mut games, &[], now, None, &asks, &polls).is_empty());
        assert!(!games.holds());
    }

    #[test]
    fn no_answer_in_time_picks_finish_this_one() {
        let (asks, polls) = (Cell::new(0), Cell::new(0));
        let mut games = PlayerGames::default();
        let now = Instant::now();
        let seen = [game(42)];
        look(&mut games, &seen, now, None, &asks, &polls);
        // Pending, or a game that doesn't answer the poll, waits out the
        // dialog's timeout and the slack.
        for decision in [Some(Decision::Pending), None] {
            let later = now + TIMEOUT;
            assert!(look(&mut games, &seen, later, decision, &asks, &polls).is_empty());
        }
        let expired = now + TIMEOUT + ANSWER_SLACK;
        let settled = look(
            &mut games,
            &seen,
            expired,
            Some(Decision::Pending),
            &asks,
            &polls,
        );
        let unanswered = Settled {
            choice: Choice::FinishThis,
            answered: false,
        };
        assert_eq!(settled, [unanswered]);
        assert!(games.holds());

        // The game's own timeout, a dismissal it couldn't map, or a dialog
        // it couldn't show settle at once.
        for decision in [
            Decision::TimedOut,
            Decision::Declined,
            Decision::Unavailable,
            Decision::Allowed,
        ] {
            let mut games = PlayerGames::default();
            look(&mut games, &seen, now, None, &asks, &polls);
            assert_eq!(
                look(&mut games, &seen, now, Some(decision), &asks, &polls),
                [unanswered],
                "{decision:?}"
            );
        }
    }

    #[test]
    fn a_game_that_cant_be_asked_yet_is_retried_until_the_timeout() {
        let mut games = PlayerGames::default();
        let now = Instant::now();
        let starting = [Seen {
            key: GameKey::Pid(42),
            socket: None,
        }];
        let never = |_: &Path| -> Option<u64> { panic!("no socket to ask") };
        let no_poll = |_: &Path, _| -> Option<Decision> { panic!("nothing asked") };
        assert!(
            games
                .observe(&starting, now, TIMEOUT, never, no_poll)
                .is_empty()
        );
        assert!(games.holds());
        // Its socket appears before the timeout: asked then.
        let asked = Cell::new(0);
        let ask = |path: &Path| {
            assert_eq!(path, Path::new("/a/42.sock"));
            asked.set(asked.get() + 1);
            Some(1)
        };
        games.observe(&[game(42)], now + TIMEOUT / 2, TIMEOUT, ask, no_poll);
        assert_eq!(asked.get(), 1);

        // One that never can be asked (or refuses the question) gets the
        // default once the timeout passes.
        let mut games = PlayerGames::default();
        let refused = |_: &Path| None;
        assert!(
            games
                .observe(&[game(42)], now, TIMEOUT, refused, no_poll)
                .is_empty()
        );
        assert!(
            games
                .observe(&starting, now + TIMEOUT / 2, TIMEOUT, never, no_poll)
                .is_empty()
        );
        assert_eq!(
            games.observe(&starting, now + TIMEOUT, TIMEOUT, never, no_poll),
            [Settled {
                choice: Choice::FinishThis,
                answered: false
            }]
        );
        assert!(games.holds());
    }

    #[test]
    fn between_jobs_a_look_asks_nothing_new_but_reads_open_questions() {
        let (asks, polls) = (Cell::new(0), Cell::new(0));
        let mut games = PlayerGames::default();
        let now = Instant::now();
        // A game opened between jobs is the start consent's to ask.
        let no_poll = |_: &Path, _| -> Option<Decision> { panic!("nothing asked") };
        assert!(games.refresh(&[game(1)], now, TIMEOUT, no_poll).is_empty());
        assert!(!games.holds());
        // A question asked during the job is still read after it ends.
        look(&mut games, &[game(2)], now, None, &asks, &polls);
        let settled = games.refresh(&[game(1), game(2)], now, TIMEOUT, |_, _| {
            Some(Decision::KeepRunning)
        });
        assert_eq!(
            settled,
            [Settled {
                choice: Choice::KeepRunning,
                answered: true
            }]
        );
        assert!(!games.holds());
        assert_eq!(asks.get(), 1);
    }

    #[test]
    fn the_notes_say_what_the_player_chose() {
        let note = |choice, answered| Settled { choice, answered }.note();
        assert!(note(Choice::StopAndPause, true).contains("chose Stop and pause"));
        assert!(note(Choice::KeepRunning, true).contains("chose Keep running"));
        assert!(note(Choice::FinishThis, true).contains("chose Finish this one"));
        assert!(note(Choice::FinishThis, false).contains("gave no answer"));
        assert!(note(Choice::KeepRunning, true).starts_with("player: opened a game and "));
    }

    /// `close_allowed` on a fake clock: `open` answers from `looks` in
    /// turn (the last one repeats), and every call is logged in order.
    fn close_with(
        quit: Result<(), String>,
        looks: &[Result<bool, String>],
        stop_after: Option<usize>,
    ) -> (Closed, Vec<String>, Duration) {
        use std::cell::RefCell;
        let log = RefCell::new(Vec::new());
        let start = Instant::now();
        let slept = Cell::new(Duration::ZERO);
        let looked = Cell::new(0usize);
        let closed = close_allowed(
            || {
                log.borrow_mut().push("quit".to_owned());
                quit
            },
            || {
                let i = looked.get();
                looked.set(i + 1);
                log.borrow_mut().push("look".to_owned());
                looks[i.min(looks.len() - 1)].clone()
            },
            &|| stop_after.is_some_and(|n| looked.get() >= n),
            start + QUIT_TIMEOUT,
            || start + slept.get(),
            |d| {
                log.borrow_mut().push("sleep".to_owned());
                slept.set(slept.get() + d);
            },
        );
        (closed, log.into_inner(), slept.get())
    }

    #[test]
    fn allowed_with_the_game_open_closes_it_then_starts() {
        let (closed, log, slept) = close_with(Ok(()), &[Ok(true), Ok(true), Ok(false)], None);
        assert_eq!(closed, Closed::Gone);
        // Asked once, first; then watched until it went.
        assert_eq!(log, ["quit", "look", "sleep", "look", "sleep", "look"]);
        assert_eq!(slept, POLL_INTERVAL * 2);
    }

    #[test]
    fn a_game_that_cant_be_closed_holds_the_job_with_the_reason() {
        // The game didn't take the request (no host.quit, agent gone).
        let (closed, log, slept) = close_with(Err("method not found".into()), &[Ok(true)], None);
        let Closed::StillOpen(reason) = closed else {
            panic!("{closed:?}");
        };
        assert!(reason.starts_with("allowed the job, but the lab could not close"), "{reason}");
        assert!(reason.contains("method not found"), "{reason}");
        assert!(reason.ends_with(CLOSE_IT_YOURSELF), "{reason}");
        assert_eq!(log, ["quit", "look"]);
        assert_eq!(slept, Duration::ZERO);

        // It took the request but is still open when the time is up.
        let (closed, _, slept) = close_with(Ok(()), &[Ok(true)], None);
        let Closed::StillOpen(reason) = closed else {
            panic!("{closed:?}");
        };
        assert!(reason.contains("did not close within 60 seconds"), "{reason}");
        assert!(reason.ends_with(CLOSE_IT_YOURSELF), "{reason}");
        assert_eq!(slept, QUIT_TIMEOUT, "bounded by the quit timeout");

        // The Mac can't be looked at.
        let (closed, _, _) = close_with(Ok(()), &[Err("ps failed".into())], None);
        let Closed::StillOpen(reason) = closed else {
            panic!("{closed:?}");
        };
        assert!(reason.contains("ps failed"), "{reason}");
    }

    #[test]
    fn a_game_gone_anyway_starts_and_a_stop_ends_the_wait() {
        // Refused, but the player closed it themselves meanwhile.
        let (closed, _, _) = close_with(Err("busy".into()), &[Ok(false)], None);
        assert_eq!(closed, Closed::Gone);
        let (closed, log, _) = close_with(Ok(()), &[Ok(true)], Some(2));
        assert_eq!(closed, Closed::Stopped);
        assert_eq!(log, ["quit", "look", "sleep", "look", "sleep"]);
    }

    #[test]
    fn a_held_game_holds_the_queue_until_it_closes() {
        let mut games = PlayerGames::default();
        let now = Instant::now();
        let no_poll = |_: &Path, _| -> Option<Decision> { panic!("nothing asked") };
        games.hold(GameKey::Pid(42));
        assert!(games.holds());
        // Still open: nothing to settle, nothing asked, still held.
        assert!(games.refresh(&[game(42)], now, TIMEOUT, no_poll).is_empty());
        assert!(games.holds());
        // Closed: the queue goes on.
        assert!(games.refresh(&[], now, TIMEOUT, no_poll).is_empty());
        assert!(!games.holds());
    }

    /// A gate looks only at the agents directory it was given: a socket in
    /// the real one (a game the user is playing) is never discovered.
    #[cfg(unix)]
    #[test]
    fn gate_scans_only_its_own_agents_directory() {
        use std::os::unix::net::UnixListener;

        let base = std::env::temp_dir().join(format!("h2lab-gate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (root, mine, other) = (base.join("root"), base.join("mine"), base.join("other"));
        for dir in [&root, &mine, &other] {
            fs::create_dir_all(dir).unwrap();
        }
        // Stands for the real agents directory, holding a live socket.
        let other_sock = other.join("player.sock");
        let _held = UnixListener::bind(&other_sock).unwrap();

        // Claims every process, so only socket discovery can find a game.
        let owns = |socket: Option<&Path>, _: Option<u32>| socket.is_none();
        let h2 = Path::new("/bin/true");

        let isolated = Gate::new(Config::default(), &root, h2, &mine);
        let seen = isolated.player_games(&owns).unwrap();
        assert!(
            seen.iter().all(|game| game.key != GameKey::Socket(other_sock.clone())),
            "{seen:?}"
        );
        assert!(seen.is_empty(), "{seen:?}");

        // Control: pointed at the directory, the same gate does see it.
        let pointed = Gate::new(Config::default(), &root, h2, &other);
        let seen = pointed.player_games(&owns).unwrap();
        assert!(
            seen.iter().any(|game| game.key == GameKey::Socket(other_sock.clone())),
            "{seen:?}"
        );
        let _ = fs::remove_dir_all(&base);
    }
}
