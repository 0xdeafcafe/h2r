use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_proto::Client;
use agent_proto::host_lobby::{OpenParams, OpenResult, Phase, StateResult};
use agent_proto::method::{
    HOST_LOBBY_CLOSE as CLOSE, HOST_LOBBY_OPEN as OPEN, HOST_LOBBY_START as START,
    HOST_LOBBY_STATE as STATE, STATUS,
};
use anyhow::{Context, Result, bail, ensure};
use live_proto::hosts::{EndReason, HostLease, Transport};
use serde_json::{Value, json};

use super::USAGE;
use super::lifecycle::{Action, Lifecycle, Observation, exit_code};
use super::live::Live;
use super::process::{self, Game, stopping};
use super::sim_run;

const BOOT_BUDGET: Duration = Duration::from_secs(60);
const BOOT_POLL: Duration = Duration::from_millis(200);
const POLL: Duration = Duration::from_secs(1);
const AGENT_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) struct Options {
    lease: String,
    live: String,
    pub token: String,
    data_url: String,
    data_sha: String,
    region: Option<String>,
    base_port: u16,
    pub tunnel_listen: SocketAddr,
    pub out: PathBuf,
    pub data: Option<PathBuf>,
    pub film: Option<PathBuf>,
    cache: PathBuf,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
        Self::parse_with(args, |name| std::env::var(name).ok())
    }

    fn parse_with(
        mut args: impl Iterator<Item = String>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let mut values = BTreeMap::new();
        while let Some(flag) = args.next() {
            ensure!(
                matches!(
                    flag.as_str(),
                    "--lease"
                        | "--live"
                        | "--token"
                        | "--data-url"
                        | "--data-sha"
                        | "--region"
                        | "--base-port"
                        | "--tunnel-listen"
                        | "--out"
                        | "--data"
                        | "--film"
                ),
                "{USAGE}"
            );
            let value = args
                .next()
                .filter(|v| !v.starts_with("--") && !v.trim().is_empty())
                .with_context(|| format!("{flag} needs a value"))?;
            ensure!(!values.contains_key(&flag), "{flag} was given twice");
            values.insert(flag, value);
        }
        let value = |flag: &str, name: &str| {
            values
                .get(flag)
                .cloned()
                .or_else(|| env(name).filter(|v| !v.trim().is_empty()))
        };
        let required = |flag, name| {
            value(flag, name).with_context(|| format!("set {flag} or {name}\n\n{USAGE}"))
        };
        let lease = required("--lease", "H2_LEASE")?;
        ensure!(
            lease
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "invalid host lease ID"
        );
        let token = required("--token", "H2_TOKEN")?;
        ensure!(
            token.bytes().all(|b| b.is_ascii_graphic()),
            "invalid host token"
        );
        let base_port = value("--base-port", "H2_BASE_PORT")
            .as_deref()
            .unwrap_or("31000")
            .parse()
            .context("--base-port must be an integer in 1..=65529")?;
        ensure!(
            (1..=65529).contains(&base_port),
            "--base-port must be in 1..=65529"
        );
        let tunnel_listen: SocketAddr = value("--tunnel-listen", "H2_TUNNEL_LISTEN")
            .as_deref()
            .unwrap_or("0.0.0.0:31100")
            .parse()
            .context("--tunnel-listen must be an IP address and port")?;
        ensure!(
            tunnel_listen.port() != 0,
            "--tunnel-listen needs a nonzero port"
        );
        Ok(Self {
            lease,
            live: required("--live", "H2_LIVE")?,
            token,
            data_url: required("--data-url", "H2_DATA_URL")?,
            data_sha: required("--data-sha", "H2_DATA_SHA")?,
            region: value("--region", "H2_REGION"),
            base_port,
            tunnel_listen,
            out: value("--out", "H2_IMAGE")
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::temp_dir().join("h2boot")),
            data: value("--data", "H2_DATA").map(PathBuf::from),
            film: value("--film", "H2_FILM_OUT").map(PathBuf::from),
            cache: env("H2_HOST_CACHE")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| agent_proto::cache_dir().join("host-data")),
        })
    }
}

pub(super) fn run(args: impl Iterator<Item = String>) -> Result<()> {
    let mut args = args.peekable();
    if matches!(args.peek().map(String::as_str), Some("-h" | "--help")) {
        println!("{USAGE}");
        return Ok(());
    }
    let opts = Options::parse(args)?;
    let lease_json = std::env::var("H2_HOST_LEASE_JSON")
        .context("h2 host run requires H2_HOST_LEASE_JSON from the provider")?;
    let lease = decode_lease(&opts.lease, &lease_json)?;
    let live = Live::new(&opts.live, &opts.lease, &opts.token)?;
    process::watch_stop_requests()?;
    let region = opts.region.as_deref().or(lease.region.as_deref());
    eprintln!(
        "h2 host {}: provisioning for {} ({})",
        lease.id,
        lease.map,
        region.unwrap_or("no region")
    );

    let mut lifecycle = Lifecycle::new(lease.idle_timeout_s, lease.max_match_s, lease.roster.len());
    let mut game = None;
    let simulation = sim_run::assigned(&lease.assignment_id, &lease.bots);
    let outcome = drive(&opts, &live, &lease, simulation, &mut game, &mut lifecycle);
    let (reason, error) = match outcome {
        Ok(reason) => (reason, None),
        Err(error) if stopping() => (EndReason::Stopped, Some(error)),
        Err(error) => (EndReason::Crashed, Some(error)),
    };
    let finish_film = !simulation && reason == EndReason::MatchOver;
    let report_end = || live.ended(&reason, lifecycle.players_peak);
    let reported = (!finish_film).then(&report_end);
    if !simulation
        && let Some(game) = game.as_ref()
        && let Ok(mut agent) = game.connect(AGENT_TIMEOUT)
        && let Err(error) = agent.call(CLOSE, json!({}))
    {
        eprintln!("h2 host {}: closing lobby: {error}", lease.id);
    }
    if finish_film && let Some(game) = game.as_mut() {
        // A normal end lets the child join its film upload before the lease
        // report makes the process eligible for provider cleanup.
        game.shutdown(film::UPLOAD_TIMEOUT + Duration::from_secs(5));
    }
    let reported = reported.unwrap_or_else(report_end);
    drop(game);

    if exit_code(&reason) != 0 {
        if let Err(report_error) = reported {
            eprintln!("h2 host {}: {report_error:#}", lease.id);
        }
        let message = format!("h2 host {} ended with {reason:?}", lease.id);
        return Err(match error {
            Some(error) => error.context(message),
            None => anyhow::anyhow!("{message}"),
        });
    }
    reported?;
    eprintln!("h2 host {}: ended with {reason:?}", lease.id);
    Ok(())
}

fn decode_lease(id: &str, json: &str) -> Result<HostLease> {
    let lease: HostLease = serde_json::from_str(json).context("decoding H2_HOST_LEASE_JSON")?;
    ensure!(
        lease.id == id,
        "H2_HOST_LEASE_JSON id does not match the requested lease"
    );
    Ok(lease)
}

fn drive(
    opts: &Options,
    live: &Live<'_>,
    lease: &HostLease,
    simulation: bool,
    game: &mut Option<Game>,
    lifecycle: &mut Lifecycle,
) -> Result<EndReason> {
    if stopping() {
        return Ok(EndReason::Stopped);
    }
    let bind = process::bind_address(opts.base_port)?;
    let disc = host_data::provision(&opts.data_url, &opts.data_sha, &opts.cache).map_err(|e| {
        let detail = format!("{e:#}").replace(&opts.data_url, "<host data URL>");
        anyhow::anyhow!("provisioning verified host data: {detail}")
    })?;
    if stopping() {
        return Ok(EndReason::Stopped);
    }

    if simulation {
        let players = u8::try_from(lease.bots.len()).context("invalid leased sim bot count")?;
        let endpoint = live.endpoint(lease.transport.clone(), bind, None, None);
        live.register(&endpoint)?;
        let game = game.insert(Game::spawn_sim(opts, live, lease, &disc)?);
        return sim_run::wait(game, live, players, &mut lifecycle.players_peak);
    }

    let public = match lease.transport {
        Transport::Udp => process::public_address(opts.base_port)?,
        Transport::Tunnel => None,
    };
    let game = game.insert(Game::spawn(opts, live, lease, &disc, bind)?);
    let deadline = Instant::now() + BOOT_BUDGET;
    let Some(mut agent) = wait_for_menu(game, deadline)? else {
        return Ok(boot_end());
    };
    if stopping() || Instant::now() >= deadline {
        return Ok(boot_end());
    }
    agent.set_timeout(Some(
        deadline
            .saturating_duration_since(Instant::now())
            .max(BOOT_POLL),
    ))?;
    let params = OpenParams {
        map: lease.map.clone(),
        variant: lease.variant.clone(),
        max_players: lease.max_players,
        live: true,
    };
    let opened = agent.call(OPEN, serde_json::to_value(params)?);
    check_running(game)?;
    if stopping() || Instant::now() >= deadline {
        return Ok(boot_end());
    }
    let opened: OpenResult =
        serde_json::from_value(opened.context(OPEN)?).context("decoding host.lobby.open")?;
    if !wait_for_lobby(game, &mut agent, deadline)? || stopping() {
        return Ok(boot_end());
    }
    let endpoint = live.endpoint(lease.transport.clone(), bind, public, opened.session_id);
    live.register(&endpoint)?;
    eprintln!("h2 host {}: joinable", lease.id);
    agent.set_timeout(Some(AGENT_TIMEOUT))?;
    play(
        game,
        &mut agent,
        live,
        lease_age(lease.created_ms),
        lifecycle,
    )
}

fn boot_end() -> EndReason {
    if stopping() {
        EndReason::Stopped
    } else {
        EndReason::BootTimeout
    }
}

fn check_running(game: &mut Game) -> Result<()> {
    if let Some(status) = game.exited()? {
        bail!("light game host exited during boot ({status})");
    }
    Ok(())
}

fn wait_for_menu(game: &mut Game, deadline: Instant) -> Result<Option<Client>> {
    while !stopping() && Instant::now() < deadline {
        check_running(game)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if let Ok(mut agent) = game.connect(remaining.min(AGENT_TIMEOUT))
            && let Ok(value) = agent.call(STATUS, Value::Null)
        {
            let status: agent_proto::Status =
                serde_json::from_value(value).context("decoding session.status")?;
            if status.at.phase == agent_proto::Phase::Menu {
                return Ok(Some(agent));
            }
        }
        thread::sleep(BOOT_POLL);
    }
    if !stopping() {
        check_running(game)?;
    }
    Ok(None)
}

fn wait_for_lobby(game: &mut Game, agent: &mut Client, deadline: Instant) -> Result<bool> {
    while !stopping() && Instant::now() < deadline {
        check_running(game)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        agent.set_timeout(Some(remaining.max(Duration::from_millis(1))))?;
        let state = agent.call(STATE, json!({}));
        check_running(game)?;
        if Instant::now() >= deadline {
            return Ok(false);
        }
        if lobby_state(state.context(STATE)?)?.phase == Phase::Lobby {
            return Ok(true);
        }
        thread::sleep(BOOT_POLL);
    }
    if !stopping() {
        check_running(game)?;
    }
    Ok(false)
}

fn lease_age(created_ms: i64) -> Duration {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let created_ms = u128::try_from(created_ms).unwrap_or(now_ms);
    Duration::from_millis(u64::try_from(now_ms.saturating_sub(created_ms)).unwrap_or(u64::MAX))
}

fn play(
    game: &mut Game,
    agent: &mut Client,
    live: &Live<'_>,
    lease_age_at_registration: Duration,
    lifecycle: &mut Lifecycle,
) -> Result<EndReason> {
    let began = Instant::now();
    loop {
        let next_poll = Instant::now() + POLL;
        let observation = if stopping() {
            Observation::Stop
        } else if let Some(status) = game.exited()? {
            eprintln!("h2 host: light game process exited ({status})");
            Observation::ChildExited
        } else {
            let state = agent.call(STATE, json!({}));
            if stopping() {
                Observation::Stop
            } else if let Some(status) = game.exited()? {
                eprintln!("h2 host: light game process exited ({status})");
                Observation::ChildExited
            } else {
                let state = lobby_state(state.context(STATE)?)?;
                let players =
                    u8::try_from(state.players.len()).context("invalid host player count")?;
                Observation::Lobby {
                    phase: state.phase,
                    players,
                    ready: state.ready,
                }
            }
        };
        let joinable_for = began.elapsed();
        let lease_age = lease_age_at_registration.saturating_add(joinable_for);
        match lifecycle.observe(joinable_for, lease_age, observation) {
            Action::Wait => {}
            Action::Start => {
                agent.call(START, json!({})).context(START)?;
            }
            Action::Playing(players) => live.playing(players)?,
            Action::End(reason) => return Ok(reason),
        }
        thread::sleep(next_poll.saturating_duration_since(Instant::now()));
    }
}

fn lobby_state(value: Value) -> Result<StateResult> {
    serde_json::from_value(value).context("decoding host.lobby.state")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_provider_lease_json_only_for_the_requested_id() {
        let lease_json = json!({
            "id": "hl-a",
            "assignment_id": "custom-test",
            "provider": "local",
            "region": null,
            "class": "small",
            "map": "lockout",
            "map_id": null,
            "variant": "Slayer",
            "hopper": null,
            "max_players": 8,
            "roster": [],
            "bots": [],
            "transport": "udp",
            "idle_timeout_s": 120,
            "max_match_s": 1800,
            "created_ms": 1
        })
        .to_string();
        let lease = decode_lease("hl-a", &lease_json).unwrap();
        assert_eq!(lease.map, "lockout");
        assert_eq!(lease.variant.as_deref(), Some("Slayer"));
        let error = decode_lease("hl-b", &lease_json).unwrap_err();
        assert_eq!(
            error.to_string(),
            "H2_HOST_LEASE_JSON id does not match the requested lease"
        );
        assert!(decode_lease("hl-a", "{").is_err());
    }

    #[test]
    fn reads_provider_environment_and_prefers_flags() {
        let env = |key: &str| {
            Some(
                match key {
                    "H2_LEASE" => "hl-env",
                    "H2_LIVE" => "https://live.example",
                    "H2_TOKEN" => "token",
                    "H2_DATA_URL" => "https://data.example/data?sig=a&expires=b",
                    "H2_DATA_SHA" => "sha",
                    "H2_REGION" => "lhr",
                    "H2_BASE_PORT" => "31020",
                    "H2_TUNNEL_LISTEN" => "0.0.0.0:31120",
                    "H2_IMAGE" => "modules",
                    "H2_DATA" => "container-data",
                    "H2_FILM_OUT" => "container-film.h2film",
                    "H2_HOST_CACHE" => "cache",
                    _ => return None,
                }
                .to_owned(),
            )
        };
        let args = [
            "--lease",
            "hl-cli",
            "--base-port",
            "31030",
            "--data",
            "state",
            "--film",
            "retained.h2film",
        ];
        let opts = Options::parse_with(args.into_iter().map(str::to_owned), env).unwrap();
        assert_eq!(opts.lease, "hl-cli");
        assert_eq!(opts.live, "https://live.example");
        assert_eq!(opts.token, "token");
        assert_eq!(opts.data_url, "https://data.example/data?sig=a&expires=b");
        assert_eq!(opts.data_sha, "sha");
        assert_eq!(opts.region.as_deref(), Some("lhr"));
        assert_eq!(opts.base_port, 31030);
        assert_eq!(opts.tunnel_listen.port(), 31120);
        assert_eq!(opts.out, PathBuf::from("modules"));
        assert_eq!(opts.data, Some(PathBuf::from("state")));
        assert_eq!(opts.film, Some(PathBuf::from("retained.h2film")));
        assert_eq!(opts.cache, PathBuf::from("cache"));
    }

    #[test]
    fn counts_players_from_lobby_state() {
        let state = lobby_state(json!({
            "phase": "playing",
            "players": [{"slot": 0, "peer": 1, "name": "Guest"}],
            "ready": false
        }))
        .unwrap();
        assert_eq!(state.phase, Phase::Playing);
        assert_eq!(state.players.len(), 1);
        assert!(!state.ready);
    }
}
