use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_proto::Client;
use agent_proto::host_lobby::{OpenParams, OpenResult, Phase, Player, StateResult};
use agent_proto::method::{
    HOST_LOBBY_CLOSE as CLOSE, HOST_LOBBY_OPEN as OPEN, HOST_LOBBY_START as START,
    HOST_LOBBY_STATE as STATE, STATUS,
};
use agent_proto::sim::method::PLAYER_ADD;
use agent_proto::sim::{AddParams, Identity, PlayerKind, SourceKind};
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
    pack_control: Option<SocketAddr>,
    pub tunnel_listen: SocketAddr,
    /// The operator's own module directory (`--out`, `H2_IMAGE`); `None`
    /// runs the module the verified bundle carries ([`image_dir`]).
    pub out: Option<PathBuf>,
    pub data: Option<PathBuf>,
    pub film: Option<PathBuf>,
    pub archive: live_proto::hosts::HostArchive,
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
                        | "--pack-control"
                        | "--tunnel-listen"
                        | "--out"
                        | "--data"
                        | "--film"
                        | "--archive"
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
            .unwrap_or("127.0.0.1:31100")
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
            pack_control: values.get("--pack-control").map(|s| s.parse()).transpose()
                .context("--pack-control must be an IP address and port")?,
            tunnel_listen,
            out: value("--out", "H2_IMAGE").map(PathBuf::from),
            data: value("--data", "H2_DATA").map(PathBuf::from),
            film: value("--film", "H2_FILM_OUT").map(PathBuf::from),
            archive: live_proto::hosts::HostArchive::parse(
                value("--archive", "H2_HOST_ARCHIVE").as_deref().unwrap_or("dual")
            ).map_err(anyhow::Error::msg)?,
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
    let (mut reason, mut error) = match outcome {
        Ok(reason) => (reason, None),
        Err(error) if stopping() => (EndReason::Stopped, Some(error)),
        Err(error) => (EndReason::Crashed, Some(error)),
    };
    let finish_film = !simulation && reason == EndReason::MatchOver;
    let reported = (!finish_film).then(|| live.ended(&reason, lifecycle.players_peak, opts.pack_control));
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
        if opts.archive == live_proto::hosts::HostArchive::Film {
            let result = game.exited().and_then(|status| {
                let status = status.context("host still running after film upload deadline")?;
                ensure!(status.success(), "film-only host exited with {status}");
                Ok(())
            });
            if let Err(failure) = result {
                reason = EndReason::Crashed;
                error = Some(failure);
            }
        }
    }
    let reported = reported.unwrap_or_else(|| live.ended(&reason, lifecycle.players_peak, opts.pack_control));
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
    let image = image_dir(opts.out.as_deref(), &disc, &opts.data_sha)?;
    if stopping() {
        return Ok(EndReason::Stopped);
    }

    if simulation {
        let players = u8::try_from(lease.bots.len()).context("invalid leased sim bot count")?;
        let endpoint = live.endpoint(lease.transport.clone(), bind, None, None);
        live.register(&endpoint)?;
        let game = game.insert(Game::spawn_sim(opts, live, lease, &disc, &image)?);
        return sim_run::wait(game, live, players, &mut lifecycle.players_peak);
    }

    let public = match lease.transport {
        Transport::Udp => process::public_address(opts.base_port)?,
        Transport::Tunnel => None,
    };
    let game = game.insert(Game::spawn(opts, live, lease, &disc, &image, bind)?);
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
        synchronous: false,
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
    if !lease.bots.is_empty() && !stopping() {
        let failures = add_phantoms(&lease.bots, |params| {
            agent.call(PLAYER_ADD, params).map_err(anyhow::Error::from)
        });
        for failure in &failures {
            eprintln!("h2 host {}: {failure:#}", lease.id);
        }
        eprintln!(
            "h2 host {}: {} of {} leased bots added as phantoms",
            lease.id,
            lease.bots.len() - failures.len(),
            lease.bots.len()
        );
    }
    play(
        game,
        &mut agent,
        live,
        lease_age(lease.created_ms),
        lifecycle,
    )
}

/// The game module directory this host runs, checked before any child
/// starts: the operator's `--out`/`H2_IMAGE` when given, else the verified
/// bundle's `image/`. A bundle without a module, or a module whose stamp
/// is not the one this h2 runs for the bundle's XBE (the check `h2 serve`
/// makes, `boot::tier::plan_light`), stops the host with a named error;
/// there is no other module to fall back to.
fn image_dir(explicit: Option<&Path>, disc: &Path, data_sha: &str) -> Result<PathBuf> {
    let (image, source) = match explicit {
        Some(dir) => (dir.to_path_buf(), "--out/H2_IMAGE"),
        None => {
            let dir = host_data::bundled_image_dir(disc)
                .context("reading the host data's game module")?
                .with_context(|| {
                    format!(
                        "the host data bundle {data_sha} carries no game module (image/{}); \
                         rebuild it with `h2 host bundle --image DIR` \
                         (docs/live/dedicated-host-build.md#the-game-module) or set H2_IMAGE",
                        host_data::IMAGE_FILES[0]
                    )
                })?;
            (dir, "the host data bundle")
        }
    };
    let xbe = boot::read_xbe(disc).context("reading the host data's default.xbe")?;
    check_module(&xbe, &image, source)?;
    Ok(image)
}

/// The child's own start check, made first: `h2 serve` runs clang's module
/// unless `H2_TIER` says otherwise, and a light host also takes a slim one.
fn check_module(xbe: &[u8], image: &Path, source: &str) -> Result<()> {
    let choice = if std::env::var_os("H2_TIER").is_some_and(|v| !v.is_empty()) {
        boot::tier::Choice::from_env()?
    } else {
        boot::tier::Choice::Clang
    };
    let plan = boot::tier::plan_light(xbe, image, choice);
    if !plan.build {
        return Ok(());
    }
    ensure!(
        plan.dylib.is_file(),
        "{source} has no game module at {}; build it with deploy/host/build.sh --module \
         and bundle it (docs/live/dedicated-host-build.md#the-game-module)",
        plan.dylib.display()
    );
    bail!(
        "the game module from {source} ({}) was not built for this h2 and this disc: \
         its stamp {} is not the one this h2 runs; rebuild the module with this revision \
         (deploy/host/build.sh --module) and bundle it again \
         (docs/live/dedicated-host-build.md#the-game-module)",
        plan.dylib.display(),
        boot::stamp_path(&plan.dylib).display()
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
                Observation::Lobby {
                    phase: state.phase,
                    players: humans(&state.players),
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

/// The lobby's people: every player but the phantoms, whose session name
/// carries the bot mark no person can type (`live_proto::bots::BOT_MARK`).
/// The start rule, idle and empty ends and the reported counts are people.
fn humans(players: &[Player]) -> u8 {
    let people = players
        .iter()
        .filter(|p| !p.name.starts_with(live_proto::bots::BOT_MARK))
        .count();
    u8::try_from(people).unwrap_or(u8::MAX)
}

/// The `sim.player.add` request for leased bot `index` (from 0): a
/// brain-driven phantom named as the lease names it, with the host-made
/// xuid a System Link host gives its own bots
/// (`live_proto::bots::local_xuid`, seat numbers from 1).
fn phantom_params(index: usize, name: &str) -> Value {
    let seat = u32::try_from(index + 1).unwrap_or(u32::MAX);
    let params = AddParams {
        kind: PlayerKind::Phantom,
        source: SourceKind::Brain,
        team: None,
        steps: Vec::new(),
        recording: None,
        identity: Some(Identity {
            xuid: live_proto::bots::local_xuid(seat).to_string(),
            gamertag: name.to_owned(),
        }),
    };
    serde_json::to_value(params).expect("sim.player.add params serialize")
}

/// Adds each leased bot through the child's `sim.player.add`. A refused
/// add does not stop the others or the host: the lobby still takes
/// people. Each failure comes back named, for the host log.
fn add_phantoms(
    bots: &[String],
    mut add: impl FnMut(Value) -> Result<Value>,
) -> Vec<anyhow::Error> {
    bots.iter()
        .enumerate()
        .filter_map(|(index, name)| {
            add(phantom_params(index, name))
                .with_context(|| format!("adding leased bot {name:?} as a phantom ({PLAYER_ADD})"))
                .err()
        })
        .collect()
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
            "--archive",
            "film",
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
        assert_eq!(opts.out, Some(PathBuf::from("modules")));
        assert_eq!(opts.data, Some(PathBuf::from("state")));
        assert_eq!(opts.film, Some(PathBuf::from("retained.h2film")));
        assert_eq!(opts.archive, live_proto::hosts::HostArchive::Film);
        assert_eq!(opts.cache, PathBuf::from("cache"));
    }

    #[test]
    fn without_out_or_h2_image_the_bundle_supplies_the_module() {
        let env = |key: &str| {
            Some(
                match key {
                    "H2_LEASE" => "hl-env",
                    "H2_LIVE" => "https://live.example",
                    "H2_TOKEN" => "token",
                    "H2_DATA_URL" => "https://data.example/data",
                    "H2_DATA_SHA" => "sha",
                    _ => return None,
                }
                .to_owned(),
            )
        };
        let opts = Options::parse_with(std::iter::empty(), env).unwrap();
        assert_eq!(opts.out, None);
    }

    /// A built, verified bundle (the host-data crate's own builder and
    /// provisioner), optionally carrying a module whose stamp is `stamp`.
    fn provisioned(root: &Path, stamp: Option<&[u8]>) -> (PathBuf, String) {
        let disc = root.join("disc");
        std::fs::create_dir_all(disc.join("maps")).unwrap();
        std::fs::write(disc.join("default.xbe"), b"XBEH synthetic").unwrap();
        for (map, kind) in [("mainmenu", 2u32), ("shared", 3), ("single_player_shared", 4)] {
            let mut bytes = vec![0u8; 0x800];
            bytes[..4].copy_from_slice(b"daeh");
            bytes[4..8].copy_from_slice(&8u32.to_le_bytes());
            bytes[0x140..0x144].copy_from_slice(&kind.to_le_bytes());
            bytes[0x198..0x198 + map.len()].copy_from_slice(map.as_bytes());
            bytes[0x7fc..0x800].copy_from_slice(b"toof");
            std::fs::write(disc.join(format!("maps/{map}.map")), bytes).unwrap();
        }
        let image = stamp.map(|stamp| {
            let dir = root.join("image-out");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("libh2game.so"), b"synthetic module").unwrap();
            std::fs::write(dir.join("libh2game.so.stamp"), stamp).unwrap();
            dir
        });
        let out = root.join("bundle");
        let built = host_data::build(&host_data::BundleSpec {
            disc,
            out: out.clone(),
            stripped: None,
            sim_stores: None,
            bots: None,
            image,
            force: true,
            zstd_level: 1,
        })
        .unwrap();
        let sha = built.manifest.sha256;
        let url = format!("file://{}", out.join(host_data::ARCHIVE_NAME).display());
        let dir = host_data::provision(&url, &sha, &root.join("cache")).unwrap();
        (dir, sha)
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("h2-host-image-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_bundle_without_a_module_is_a_named_error() {
        let root = scratch("none");
        let (disc, sha) = provisioned(&root, None);
        let text = format!("{:#}", image_dir(None, &disc, &sha).unwrap_err());
        assert!(text.contains(&format!("bundle {sha} carries no game module")), "{text}");
        assert!(text.contains("--image"), "{text}");
        let _ = std::fs::remove_dir_all(root);
    }

    /// A module directory for this platform's module name, stamped `stamp`.
    fn module_dir(root: &Path, name: &str, stamp: &[u8]) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(boot::DYLIB), b"synthetic module").unwrap();
        std::fs::write(boot::stamp_path(&dir.join(boot::DYLIB)), stamp).unwrap();
        dir
    }

    const OTHER_STAMP: &[u8] = b"xbe 0000000000000000 generator other\nhost coverage 0000000000000000\n";

    #[test]
    fn a_module_with_another_builds_stamp_is_refused() {
        let root = scratch("stale");
        let (disc, sha) = provisioned(&root, None);
        let image = module_dir(&root, "stale-image", OTHER_STAMP);
        let text = format!("{:#}", image_dir(Some(&image), &disc, &sha).unwrap_err());
        assert!(text.contains("from --out/H2_IMAGE"), "{text}");
        assert!(text.contains("not built for this h2 and this disc"), "{text}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_explicit_directory_without_a_module_is_refused() {
        let root = scratch("explicit");
        let (disc, sha) = provisioned(&root, None);
        let empty = root.join("empty-image");
        std::fs::create_dir_all(&empty).unwrap();
        let text = format!("{:#}", image_dir(Some(&empty), &disc, &sha).unwrap_err());
        assert!(text.contains("--out/H2_IMAGE has no game module"), "{text}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_module_stamped_for_this_h2_and_this_disc_is_used() {
        let root = scratch("current");
        let (disc, sha) = provisioned(&root, None);
        let xbe = boot::read_xbe(&disc).unwrap();
        let image = module_dir(&root, "current-image", boot::build_stamp(&xbe).as_bytes());
        assert_eq!(image_dir(Some(&image), &disc, &sha).unwrap(), image);
        let _ = std::fs::remove_dir_all(root);
    }

    /// The bundle's names are the Linux module's, so the bundled module
    /// itself runs only on Linux.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_bundled_module_is_used_when_its_stamp_matches_and_refused_when_not() {
        let root = scratch("bundled");
        let xbe = b"XBEH synthetic".to_vec();
        let (disc, sha) = provisioned(&root, Some(boot::build_stamp(&xbe).as_bytes()));
        assert_eq!(image_dir(None, &disc, &sha).unwrap(), disc.join("image"));

        let stale_root = scratch("bundled-stale");
        let (disc, sha) = provisioned(&stale_root, Some(OTHER_STAMP));
        let text = format!("{:#}", image_dir(None, &disc, &sha).unwrap_err());
        assert!(text.contains("from the host data bundle"), "{text}");
        assert!(text.contains("not built for this h2 and this disc"), "{text}");
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(stale_root);
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

    #[test]
    fn phantoms_are_not_people() {
        let state = lobby_state(json!({
            "phase": "lobby",
            "players": [
                {"slot": 0, "peer": 1, "name": "\u{2699} Sprocket Cog"},
                {"slot": 1, "peer": 2, "name": "Guest"},
                {"slot": 2, "peer": 3, "name": "\u{2699} Rivet Shim"}
            ],
            "ready": true
        }))
        .unwrap();
        assert_eq!(humans(&state.players), 1);
        assert_eq!(humans(&state.players[..1]), 0);
        assert_eq!(humans(&[]), 0);

        // Phantoms alone never start an open lobby; one person does, even
        // with the bots filling every other seat.
        let mut host = Lifecycle::new(120, 1800, 0);
        let lobby = |players: &[Player]| Observation::Lobby {
            phase: Phase::Lobby,
            players: humans(players),
            ready: true,
        };
        let secs = Duration::from_secs;
        let bots_only = [state.players[0].clone(), state.players[2].clone()];
        assert_eq!(host.observe(secs(1), secs(1), lobby(&bots_only)), Action::Wait);
        assert_eq!(host.observe(secs(2), secs(2), lobby(&state.players)), Action::Start);
        // Idle with only phantoms in it, a lobby ends as one with nobody.
        let mut idle = Lifecycle::new(120, 1800, 0);
        assert_eq!(
            idle.observe(secs(121), secs(121), lobby(&bots_only)),
            Action::End(EndReason::Idle)
        );
    }

    #[test]
    fn leased_bots_are_added_as_brain_phantoms_and_failures_are_named() {
        let bots = ["Sprocket Cog".to_owned(), "Rivet Shim".to_owned(), "Gear Stud".to_owned()];
        let mut sent = Vec::new();
        let failures = add_phantoms(&bots, |params| {
            sent.push(params.clone());
            if params["identity"]["gamertag"] == "Rivet Shim" {
                bail!("light host bots are off; start with --host-bots");
            }
            Ok(json!({"id": format!("sim.{}", sent.len())}))
        });
        assert_eq!(sent.len(), 3, "a refusal does not stop the next add");
        assert_eq!(
            sent[0],
            json!({
                "kind": "phantom",
                "source": "brain",
                "identity": {"xuid": "000e000000000001", "gamertag": "Sprocket Cog"}
            })
        );
        for (index, params) in sent.iter().enumerate() {
            let add: AddParams = serde_json::from_value(params.clone()).unwrap();
            let identity = add.identity.unwrap();
            assert_eq!(identity.gamertag, bots[index]);
            let xuid = u64::from_str_radix(&identity.xuid, 16).unwrap();
            assert_eq!(
                live_proto::Xuid(xuid),
                live_proto::bots::local_xuid(index as u32 + 1)
            );
            assert!(live_proto::bots::is_local_xuid(live_proto::Xuid(xuid)));
        }
        assert_eq!(failures.len(), 1);
        let text = format!("{:#}", failures[0]);
        assert!(text.contains("\"Rivet Shim\""), "{text}");
        assert!(text.contains("sim.player.add"), "{text}");
        assert!(text.contains("--host-bots"), "{text}");

        assert!(add_phantoms(&[], |_| unreachable!()).is_empty());
    }
}
