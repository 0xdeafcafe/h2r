//! `h2 host pack`: several leases' matches in one container
//! (docs/live/dedicated-host.md#packed-hosts).
//!
//! The pack starts with no match. Live adds and removes matches over a
//! private HTTP control port. Each match is its own `h2 host run` child in
//! its own process group, with its own slot of ports and writable data, so
//! guest state stays isolated while the module and maps stay shared through
//! the page cache. A new match is admitted only when the measured cost of
//! the running ones leaves room ([`live_proto::host_pack::admit`]). The pack
//! exits after it has run nothing for `--linger-s`.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use live_proto::host_pack::{
    self, AddMatch, Added, Limits, MatchLoad, MatchStatus, PackStatus, Refusal, SlotPorts, http,
};
use serde_json::json;

use super::process::stopping;

#[path = "pack_measure.rs"]
mod measure;
use measure::Meter;

pub(super) const USAGE: &str = "usage:
  h2 host pack [--control-listen 127.0.0.1:31099] [--max-matches 5]
               [--vcpus N] [--memory-mib MIB] [--linger-s 60]
               [--base-port 31000] [--tunnel-listen 127.0.0.1:31100]
               [--agent-port 14720] [--data DIR]";

const SAMPLE: Duration = Duration::from_secs(2);
const STOP_GRACE: Duration = Duration::from_secs(15);
const IO_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct Options {
    control: SocketAddr,
    max_matches: usize,
    vcpus: Option<f64>,
    memory_mib: Option<u64>,
    linger: Duration,
    base_port: u16,
    tunnel: SocketAddr,
    agent_port: u16,
    data: Option<PathBuf>,
}

impl Options {
    fn parse_with(
        mut args: impl Iterator<Item = String>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let mut values = BTreeMap::new();
        while let Some(flag) = args.next() {
            ensure!(
                matches!(
                    flag.as_str(),
                    "--control-listen"
                        | "--max-matches"
                        | "--vcpus"
                        | "--memory-mib"
                        | "--linger-s"
                        | "--base-port"
                        | "--tunnel-listen"
                        | "--agent-port"
                        | "--data"
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
        let control: SocketAddr = value("--control-listen", "H2_PACK_CONTROL")
            .unwrap_or_else(|| format!("127.0.0.1:{}", host_pack::CONTROL_PORT))
            .parse()
            .context("--control-listen must be an IP address and port")?;
        let max_matches: usize = value("--max-matches", "H2_PACK_MAX_MATCHES")
            .map_or(Ok(host_pack::DEFAULT_MAX_MATCHES), |v| v.parse())
            .context("--max-matches must be an integer")?;
        ensure!(
            (1..=host_pack::MAX_PACK_MATCHES).contains(&max_matches),
            "--max-matches must be in 1..=10"
        );
        let vcpus = value("--vcpus", "H2_PACK_VCPUS")
            .map(|v| v.parse::<f64>())
            .transpose()
            .context("--vcpus must be a number")?;
        ensure!(
            vcpus.is_none_or(|v| v.is_finite() && v > 0.0),
            "--vcpus must be positive"
        );
        let memory_mib = value("--memory-mib", "H2_PACK_MEMORY_MIB")
            .map(|v| v.parse::<u64>())
            .transpose()
            .context("--memory-mib must be an integer")?;
        let linger = value("--linger-s", "H2_PACK_LINGER_S")
            .map_or(Ok(60), |v| v.parse::<u64>())
            .context("--linger-s must be an integer")?;
        let base_port = value("--base-port", "H2_BASE_PORT")
            .map_or(Ok(host_pack::BASE_PORT), |v| v.parse())
            .context("--base-port must be a port")?;
        let tunnel: SocketAddr = value("--tunnel-listen", "H2_TUNNEL_LISTEN")
            .unwrap_or_else(|| format!("127.0.0.1:{}", host_pack::TUNNEL_PORT))
            .parse()
            .context("--tunnel-listen must be an IP address and port")?;
        let agent_port = value("--agent-port", "H2_PACK_AGENT_PORT")
            .map_or(Ok(host_pack::AGENT_PORT), |v| v.parse())
            .context("--agent-port must be a port")?;
        let last = u16::try_from(max_matches - 1).unwrap_or(u16::MAX);
        ensure!(
            SlotPorts::for_slot(base_port, tunnel.port(), agent_port, last).is_some(),
            "the port bases leave no room for {max_matches} matches"
        );
        Ok(Self {
            control,
            max_matches,
            vcpus,
            memory_mib,
            linger: Duration::from_secs(linger),
            base_port,
            tunnel,
            agent_port,
            data: value("--data", "H2_PACK_DATA").map(PathBuf::from),
        })
    }

    /// Flags win, then the container's cgroup, then this machine.
    fn limits(&self) -> Limits {
        let vcpus = self
            .vcpus
            .or_else(measure::cgroup_vcpus)
            .unwrap_or_else(|| thread::available_parallelism().map_or(1.0, |n| n.get() as f64));
        let memory = self.memory_mib.or_else(measure::cgroup_memory_max_mib).unwrap_or(
            // No known limit: let the match cap decide.
            self.max_matches as u64 * host_pack::DEFAULT_RSS_MIB * 4 + host_pack::DEFAULT_RESERVE_MIB,
        );
        Limits::new(self.max_matches, vcpus, memory)
    }
}

/// Starts one match's `h2 host run`.
type Launch = Box<dyn Fn(&AddMatch, u16, SlotPorts) -> io::Result<Child> + Send>;

/// Each occupied slot accumulates the same fraction of each time interval.
struct Occupancy {
    changed: Instant,
    per_slot: Duration,
}

impl Occupancy {
    fn at(&self, now: Instant, slots: usize) -> Duration {
        let elapsed = now.saturating_duration_since(self.changed);
        self.per_slot + elapsed.checked_div(slots as u32).unwrap_or_default()
    }

    fn advance(&mut self, now: Instant, slots: usize) {
        self.per_slot = self.at(now, slots);
        self.changed = self.changed.max(now);
    }
}

struct Slot {
    started: Instant,
    share_at_start: Duration,
    charge_start: bool,
    lease: String,
    ports: SlotPorts,
    map: Option<String>,
    child: Child,
    stopping_since: Option<Instant>,
    meter: Meter,
}

impl Slot {
    fn alive(&self) -> bool {
        self.stopping_since.is_none()
    }
}

#[derive(Debug)]
enum AddError {
    Refused(Refusal),
    Invalid(String),
    Spawn(io::Error),
}

/// Recent trusted observations across this pack. A single bounded history
/// also bounds the number of distinct mod maps a long-lived pack retains.
#[derive(Default)]
struct MapCosts(VecDeque<(String, f64, Option<u64>)>);

impl MapCosts {
    fn observe(&mut self, map: &str, load: MatchLoad) {
        // A low-CPU menu or a busy boot is not a sample of this map.
        if load.wall_ms_per_tick_p99.is_some()
            && let Some(ms) = load.ms_per_tick.filter(|ms| ms.is_finite() && *ms >= 0.0)
        {
            if self.0.len() == 256 {
                self.0.pop_front();
            }
            self.0.push_back((map.to_owned(), ms, load.rss_mib.filter(|m| *m > 0)));
        }
    }

    fn estimate(&self, map: Option<&str>, mut limits: Limits) -> Limits {
        let Some(map) = map else { return limits };
        let mut times = Vec::new();
        let mut memory = Vec::new();
        for (_, ms, mib) in self.0.iter().filter(|(key, _, _)| key == map) {
            times.push(*ms);
            memory.extend(*mib);
        }
        if !times.is_empty() {
            times.sort_by(f64::total_cmp);
            limits.new_ms_per_tick = (times[(times.len() - 1) / 2] + times[times.len() / 2]) / 2.0;
        }
        if !memory.is_empty() {
            memory.sort_unstable();
            limits.new_rss_mib = memory[(memory.len() - 1) / 2].midpoint(memory[memory.len() / 2]);
        }
        limits
    }
}

pub(super) struct Pack {
    limits: Limits,
    history: MapCosts,
    /// UDP, tunnel and agent port bases for slot 0.
    bases: (u16, u16, u16),
    slots: Vec<Option<Slot>>,
    container_mib: Option<u64>,
    idle_since: Option<Instant>,
    linger: Duration,
    draining: bool,
    launch: Launch,
    occupancy: Occupancy,
    start_claimed: bool,
}

impl Pack {
    fn new(
        limits: Limits,
        bases: (u16, u16, u16),
        linger: Duration,
        launch: Launch,
        now: Instant,
    ) -> Self {
        Self {
            bases,
            history: MapCosts::default(),
            slots: (0..limits.max_matches).map(|_| None).collect(),
            limits,
            container_mib: None,
            idle_since: Some(now),
            linger,
            draining: false,
            launch,
            occupancy: Occupancy { changed: now, per_slot: Duration::ZERO },
            start_claimed: false,
        }
    }

    fn find(&self, lease: &str) -> Option<(usize, &Slot)> {
        self.slots
            .iter()
            .enumerate()
            .find_map(|(i, s)| s.as_ref().filter(|s| s.lease == lease).map(|s| (i, s)))
    }

    fn add(&mut self, add: &AddMatch) -> Result<Added, AddError> {
        if !valid_lease(&add.lease) {
            return Err(AddError::Invalid("invalid lease ID".into()));
        }
        let lease = serde_json::from_str::<serde_json::Value>(&add.lease_json).unwrap_or_default();
        if lease["id"].as_str() != Some(add.lease.as_str()) {
            return Err(AddError::Invalid("lease_json does not name this lease".into()));
        }
        if let Some((slot, existing)) = self.find(&add.lease) {
            return if existing.alive() {
                Ok(Added {
                    slot: slot as u16,
                    ports: existing.ports,
                })
            } else {
                Err(AddError::Invalid("this lease is stopping".into()))
            };
        }
        if self.draining {
            return Err(AddError::Refused(Refusal::Draining));
        }
        // Stopping matches still hold their slot and CPU until reaped.
        let mut running = Vec::new();
        let mut reserved_mib = 0u64;
        for slot in self.slots.iter().flatten() {
            let mut load = slot.meter.load();
            if load.wall_ms_per_tick_p99.is_none() || load.ms_per_tick.is_none() || load.rss_mib.is_none() {
                let estimate = self.history.estimate(slot.map.as_deref(), self.limits);
                load.ms_per_tick = Some(load.ms_per_tick.unwrap_or(0.0).max(estimate.new_ms_per_tick));
                let observed = load.rss_mib.unwrap_or(0);
                // A just-started match may not have faulted its RAM yet.
                // Add only its outstanding reservation to the cgroup sample.
                let outstanding = estimate.new_rss_mib.saturating_sub(observed);
                reserved_mib = reserved_mib.saturating_add(outstanding);
                load.rss_mib = Some(observed.max(estimate.new_rss_mib));
            }
            running.push(load);
        }
        // Packaged maps are identified by content even if the scenario name matches.
        let map = lease["map_id"].as_str().or_else(|| lease["map"].as_str());
        let estimate = self.history.estimate(map, self.limits);
        let committed_mib = self.container_mib.map(|used| used.saturating_add(reserved_mib));
        host_pack::admit(&estimate, &running, committed_mib).map_err(AddError::Refused)?;
        let slot = self
            .slots
            .iter()
            .position(Option::is_none)
            .expect("admit checked the match cap");
        let (base, tunnel, agent) = self.bases;
        let ports = u16::try_from(slot)
            .ok()
            .and_then(|slot| SlotPorts::for_slot(base, tunnel, agent, slot))
            .ok_or_else(|| AddError::Invalid("no ports for this slot".into()))?;
        let child = (self.launch)(add, slot as u16, ports).map_err(AddError::Spawn)?;
        eprintln!(
            "h2 host pack: slot {slot} runs {} (UDP {}, tunnel {})",
            add.lease, ports.base_port, ports.tunnel_port
        );
        let started = Instant::now();
        self.occupancy.advance(started, self.slots.iter().flatten().count());
        self.slots[slot] = Some(Slot {
            started,
            share_at_start: self.occupancy.per_slot,
            charge_start: !self.start_claimed,
            lease: add.lease.clone(),
            map: map.map(str::to_owned),
            ports,
            child,
            stopping_since: None,
            meter: Meter::default(),
        });
        self.start_claimed = true;
        self.idle_since = None;
        Ok(Added {
            slot: slot as u16,
            ports,
        })
    }

    /// Ask a match to end. Idempotent; the slot frees once it is reaped.
    fn remove(&mut self, lease: &str, now: Instant) {
        if let Some(slot) = self
            .slots
            .iter_mut()
            .flatten()
            .find(|s| s.lease == lease && s.stopping_since.is_none())
        {
            signal_group(&slot.child, libc::SIGTERM);
            slot.stopping_since = Some(now);
        }
    }

    /// Reap exited matches, escalate slow stops, refresh measurements and
    /// the idle clock.
    fn tick(
        &mut self,
        now: Instant,
        sample: &dyn Fn(u32) -> Option<measure::Usage>,
        container_mib: Option<u64>,
    ) {
        for index in 0..self.slots.len() {
            let Some(slot) = &mut self.slots[index] else { continue };
            if let Ok(Some(status)) = slot.child.try_wait() {
                eprintln!("h2 host pack: {} exited ({status})", slot.lease);
                self.release_slot(index, now);
                continue;
            }
            if slot
                .stopping_since
                .is_some_and(|since| now.duration_since(since) >= STOP_GRACE)
            {
                signal_group(&slot.child, libc::SIGKILL);
                if slot.child.wait().is_ok() {
                    self.release_slot(index, now);
                }
                continue;
            }
            if let Some(usage) = sample(slot.child.id()) {
                slot.meter.observe(now, usage);
            }
            if let Some(map) = &slot.map {
                self.history.observe(map, slot.meter.load());
            }
        }
        self.container_mib = container_mib;
        if self.slots.iter().any(Option::is_some) {
            self.idle_since = None;
        } else if self.idle_since.is_none() {
            self.idle_since = Some(now);
        }
    }

    /// Settle the interval at its old occupancy before the reaped slot disappears.
    fn release_slot(&mut self, index: usize, now: Instant) {
        self.occupancy.advance(now, self.slots.iter().flatten().count());
        self.slots[index] = None;
    }

    fn idle_for(&self, now: Instant) -> Option<Duration> {
        self.idle_since.map(|since| now.duration_since(since))
    }

    fn should_exit(&self, now: Instant) -> bool {
        self.draining || self.idle_for(now).is_some_and(|idle| idle >= self.linger)
    }

    fn usage(&self, lease: &str, now: Instant) -> Option<live_proto::hosts::PackUsage> {
        let (_, slot) = self.find(lease)?;
        let shared = self.occupancy.at(now, self.slots.iter().flatten().count());
        // Linux includes already reaped children in the supervisor's CPU
        // counters. Take the last reading after any film upload completes.
        let mut meter = slot.meter.clone();
        if let Some(usage) = measure::group_usage(slot.child.id()) {
            meter.observe(now, usage);
        }
        let load = meter.load();
        Some(live_proto::hosts::PackUsage {
            slot_us: now.saturating_duration_since(slot.started).as_micros() as u64,
            container_share_us: shared.saturating_sub(slot.share_at_start).as_micros() as u64,
            charge_start: slot.charge_start,
            cpu_us: load.cpu_seconds.map(|s| (s * 1_000_000.0).round() as u64),
            peak_pss_bytes: load.peak_pss_bytes,
        })
    }

    fn status(&self) -> PackStatus {
        PackStatus {
            limits: self.limits,
            matches: self
                .slots
                .iter()
                .enumerate()
                .filter_map(|(i, s)| Some(match_status(i, s.as_ref()?)))
                .collect(),
            container_mib: self.container_mib,
            draining: self.draining,
        }
    }

    /// End every match: TERM, wait, then KILL.
    fn shutdown(&mut self) {
        self.draining = true;
        for slot in self.slots.iter().flatten() {
            signal_group(&slot.child, libc::SIGTERM);
        }
        let deadline = Instant::now() + STOP_GRACE;
        while Instant::now() < deadline && self.slots.iter().any(Option::is_some) {
            for index in 0..self.slots.len() {
                if let Some(slot) = &mut self.slots[index]
                    && matches!(slot.child.try_wait(), Ok(Some(_)))
                {
                    self.release_slot(index, Instant::now());
                }
            }
            if self.slots.iter().any(Option::is_some) {
                thread::sleep(Duration::from_millis(100));
            }
        }
        for index in 0..self.slots.len() {
            if let Some(slot) = &mut self.slots[index] {
                signal_group(&slot.child, libc::SIGKILL);
                if slot.child.wait().is_ok() {
                    self.release_slot(index, Instant::now());
                }
            }
        }
    }
}

fn match_status(slot: usize, s: &Slot) -> MatchStatus {
    MatchStatus {
        lease: s.lease.clone(),
        slot: slot as u16,
        alive: s.alive(),
        load: s.meter.load(),
    }
}

fn valid_lease(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn signal_group(child: &Child, signal: libc::c_int) {
    // SAFETY: the child is its own group leader and not yet reaped, so its
    // PID (and group ID) cannot have been reused.
    unsafe { libc::killpg(child.id() as libc::pid_t, signal) };
}

fn error(status: u16, message: &str) -> (u16, Vec<u8>) {
    (status, json!({ "error": message }).to_string().into_bytes())
}

fn ok_json(value: &impl serde::Serialize) -> (u16, Vec<u8>) {
    (200, serde_json::to_vec(value).unwrap_or_default())
}

/// One control request.
fn handle(pack: &mut Pack, req: &http::Request, now: Instant) -> (u16, Vec<u8>) {
    if req.method == "GET"
        && let Some(lease) = req.path.strip_prefix(host_pack::MATCHES_PATH)
            .and_then(|path| path.strip_prefix('/'))
            .and_then(|path| path.strip_suffix("/usage"))
            .filter(|id| valid_lease(id))
    {
        return match pack.usage(lease, now) {
            Some(usage) => ok_json(&usage),
            None => error(404, "no such match"),
        };
    }
    let lease = req
        .path
        .strip_prefix(host_pack::MATCHES_PATH)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|id| valid_lease(id));
    match (req.method.as_str(), req.path.as_str(), lease) {
        ("GET", host_pack::STATUS_PATH, _) => ok_json(&pack.status()),
        ("POST", host_pack::WARM_PATH, _) if !pack.draining => {
            pack.start_claimed = true; // The warm reservation paid the pack start.
            if pack.idle_since.is_some() {
                pack.idle_since = Some(now);
            }
            (204, Vec::new())
        }
        ("POST", host_pack::MATCHES_PATH, _) => {
            let Ok(add) = serde_json::from_slice::<AddMatch>(&req.body) else {
                return error(400, "expected an AddMatch body");
            };
            match pack.add(&add) {
                Ok(added) => ok_json(&added),
                Err(AddError::Refused(refusal)) => {
                    eprintln!("h2 host pack: refused {}: {refusal}", add.lease);
                    (409, serde_json::to_vec(&refusal).unwrap_or_default())
                }
                Err(AddError::Invalid(message)) => error(400, &message),
                Err(AddError::Spawn(e)) => error(500, &format!("could not start the match: {e}")),
            }
        }
        ("GET", _, Some(lease)) => match pack.find(lease) {
            Some((slot, s)) => ok_json(&match_status(slot, s)),
            None => error(404, "no such match"),
        },
        ("DELETE", _, Some(lease)) => {
            pack.remove(lease, now);
            (204, Vec::new())
        }
        _ => error(404, "not found"),
    }
}

fn serve_one(pack: &Mutex<Pack>, mut stream: TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let request = loop {
        match http::parse_request(&buf) {
            http::Parse::Done(req) => break Ok(req),
            http::Parse::Invalid(why) => break Err(why),
            http::Parse::Partial => {}
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let (status, body) = match request {
        Ok(req) => handle(&mut pack.lock().unwrap(), &req, Instant::now()),
        Err(why) => error(400, why),
    };
    stream.write_all(&http::response(status, &body))
}

fn launcher(opts: &Options, mut control: SocketAddr) -> Result<Launch> {
    if control.ip().is_unspecified() {
        control.set_ip(if control.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    let exe = std::env::current_exe().context("finding the h2 binary")?;
    let tunnel_ip = opts.tunnel.ip();
    let data = opts.data.clone();
    let public = std::env::var("H2_PUBLIC_ADDR").ok();
    let pack_base = opts.base_port;
    let slots = opts.max_matches as u64;
    Ok(Box::new(move |add: &AddMatch, slot: u16, ports: SlotPorts| {
        let mut cmd = Command::new(&exe);
        cmd.args([
            "host",
            "run",
            "--lease",
            &add.lease,
            "--pack-control",
            &control.to_string(),
            "--live",
            &add.live,
            "--data-sha",
            &add.data_sha,
            "--archive",
            add.archive.as_str(),
            "--base-port",
            &ports.base_port.to_string(),
            "--tunnel-listen",
            &SocketAddr::new(tunnel_ip, ports.tunnel_port).to_string(),
        ]);
        if let Some(region) = &add.region {
            cmd.args(["--region", region]);
        }
        if let Some(root) = &data {
            let dir = root.join(&add.lease);
            std::fs::create_dir_all(&dir)?;
            cmd.arg("--data").arg(dir);
        }
        for name in [
            "H2_LEASE",
            "H2_DATA_SHA",
            "H2_REGION",
            "H2_BASE_PORT",
            "H2_TUNNEL_LISTEN",
            "H2_DATA",
            // Each match owns its film and can finish independently.
            "H2_FILM_OUT",
        ] {
            cmd.env_remove(name);
        }
        if let Some(public) = slot_public_addr(public.as_deref(), pack_base, ports.base_port)? {
            cmd.env("H2_PUBLIC_ADDR", public);
        }
        let phase_ns = (1_000_000_000 / 30) * u64::from(slot) / slots;
        cmd.env("H2_HOST_TICK_PHASE_NS", phase_ns.to_string());
        cmd.env("H2_TOKEN", &add.token)
            .env("H2_DATA_URL", &add.data_url)
            .env("H2_HOST_LEASE_JSON", &add.lease_json)
            // Each match gets a private diagnostic agent of its own.
            .env(
                agent_proto::ENV,
                format!("tcp://127.0.0.1:{}", ports.agent_port),
            )
            // The pack holds the write end; if it dies, the match stops.
            .env("H2_HOST_PARENT_PIPE", "1")
            .stdin(Stdio::piped())
            .process_group(0);
        #[cfg(target_os = "linux")]
        super::process::stop_on_parent_death(&mut cmd);
        cmd.spawn()
    }))
}

/// An explicit `IPv4:port` public address names the forwarded base for
/// slot 0; shift it with the slot. A bare IP already follows `--base-port`.
fn slot_public_addr(
    public: Option<&str>,
    pack_base: u16,
    slot_base: u16,
) -> io::Result<Option<String>> {
    let Some(public) = public else {
        return Ok(None);
    };
    if let Ok(mut addr) = public.parse::<SocketAddrV4>() {
        let offset = slot_base.saturating_sub(pack_base);
        let port = addr
            .port()
            .checked_add(offset)
            .ok_or_else(|| io::Error::other("H2_PUBLIC_ADDR port range exhausted"))?;
        addr.set_port(port);
        return Ok(Some(addr.to_string()));
    }
    Ok(Some(public.to_owned()))
}

pub(super) fn run(args: impl Iterator<Item = String>) -> Result<()> {
    let mut args = args.peekable();
    if matches!(args.peek().map(String::as_str), Some("-h" | "--help")) {
        println!("{USAGE}");
        return Ok(());
    }
    let opts = Options::parse_with(args, |name| std::env::var(name).ok())?;
    super::process::watch_stop_requests()?;
    let limits = opts.limits();
    let listener = TcpListener::bind(opts.control)
        .with_context(|| format!("binding the pack control port {}", opts.control))?;
    eprintln!(
        "h2 host pack: up to {} matches, {:.2} vCPU ({:.1} ms/tick budget), {} MiB; control on {}",
        limits.max_matches,
        limits.vcpus,
        limits.tick_budget_ms(),
        limits.memory_mib,
        opts.control
    );
    let pack = Arc::new(Mutex::new(Pack::new(
        limits,
        (opts.base_port, opts.tunnel.port(), opts.agent_port),
        opts.linger,
        launcher(&opts, listener.local_addr()?)?,
        Instant::now(),
    )));
    {
        let pack = Arc::clone(&pack);
        thread::Builder::new()
            .name("pack-control".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(stream) => {
                            if let Err(e) = serve_one(&pack, stream) {
                                eprintln!("h2 host pack: control connection: {e}");
                            }
                        }
                        Err(e) => eprintln!("h2 host pack: accept: {e}"),
                    }
                }
            })
            .context("starting the pack control thread")?;
    }
    loop {
        thread::sleep(SAMPLE);
        // The control thread may add or stop a lease while the agent replies.
        // Never hold its lock across an RPC, or apply a sample to a reused slot.
        let slots: Vec<_> = pack.lock().unwrap().slots.iter().flatten()
            .map(|s| (s.child.id(), s.ports.agent_port)).collect();
        let readings: Vec<_> = slots.into_iter().map(|(pid, port)| {
            (pid, measure::agent_reading(port))
        }).collect();
        let mut pack = pack.lock().unwrap();
        let now = Instant::now();
        for (pid, reading) in readings {
            if let Some(slot) = pack.slots.iter_mut().flatten().find(|s| s.child.id() == pid) {
                slot.meter.observe_agent(now, reading);
            }
        }
        pack.tick(now, &measure::group_usage, measure::cgroup_used_mib());
        if stopping() || pack.should_exit(now) {
            eprintln!("h2 host pack: stopping");
            pack.shutdown();
            return Ok(());
        }
    }
}

#[cfg(test)]
#[path = "pack_tests.rs"]
mod tests;
