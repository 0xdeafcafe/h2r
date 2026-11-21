//! Consent to use the playing Mac (docs/runtime/lab.md). A job holds the
//! lab activity lock while waiting. The game stays under the player's control.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_proto::{Client, discover, takeover};
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
    Skipped(&'static str),
    Stopped,
}

pub(super) struct Gate {
    config: Config,
    lab_sockets: PathBuf,
    current_h2: PathBuf,
    declined_until_file: PathBuf,
    declined_until: Mutex<u64>,
}

impl Gate {
    pub(super) fn new(config: Config, root: &Path, current_h2: &Path) -> Self {
        let declined_until_file = root.join("takeover-declined-until");
        let declined_until = fs::read_to_string(&declined_until_file)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        Self {
            config,
            lab_sockets: root.join("a"),
            current_h2: current_h2.to_owned(),
            declined_until_file,
            declined_until: Mutex::new(declined_until),
        }
    }

    pub(super) fn admit(
        &self,
        mut request: takeover::Request,
        phase: impl Fn(&str),
        stopped: impl Fn() -> bool,
    ) -> Outcome {
        let timeout = Duration::from_secs(self.config.timeout_seconds.into());
        // Multiple windows may need consent. Bound the whole handoff even
        // if the player opens a different window after agreeing.
        let deadline = Instant::now() + timeout + QUIT_TIMEOUT;
        loop {
            if stopped() {
                return Outcome::Stopped;
            }
            let games = match self.windows() {
                Ok(games) => games,
                Err(error) => {
                    eprintln!("lab: checking for game windows: {error:#}");
                    return Outcome::Skipped("could not check for an active game window");
                }
            };
            let Some(game) = games.first() else {
                return Outcome::Proceed;
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
                Some(takeover::Decision::Unavailable) => return Outcome::Skipped(UNAVAILABLE),
                None => return Outcome::Stopped,
                Some(_) => {
                    self.remember_decline();
                    return Outcome::Skipped(DECLINED);
                }
            }
            phase("awaiting_quit");
            let quit_by = (Instant::now() + QUIT_TIMEOUT).min(deadline);
            loop {
                if stopped() {
                    return Outcome::Stopped;
                }
                match self.windows() {
                    Ok(games) if !games.iter().any(|candidate| game.same_as(candidate)) => break,
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("lab: checking if the game quit: {error:#}");
                        return Outcome::Skipped("could not check whether the game quit");
                    }
                }
                let remaining = quit_by.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Outcome::Skipped("player allowed takeover, but the game is still open");
                }
                std::thread::sleep(POLL_INTERVAL.min(remaining));
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

    fn windows(&self) -> Result<Vec<Game>> {
        let agents = discover::dir();
        let mut games = Vec::new();
        let mut known = BTreeSet::new();
        let deadline = Instant::now() + SCAN_TIMEOUT;
        for (dir, published) in [(&agents, true), (&self.lab_sockets, false)] {
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
                let remaining = deadline.saturating_duration_since(Instant::now());
                ensure!(!remaining.is_zero(), "game agents did not answer in time");
                let published_pid = published.then(|| discover::pid_of(&path)).flatten();
                match probe(&path, remaining.min(discover::PROBE_TIMEOUT)) {
                    Probe::Active(info) => {
                        let pid = info.pid.or(published_pid);
                        if pid.is_none_or(|pid| known.insert(pid)) && info.window {
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
}
