//! Packed local hosts (docs/live/dedicated-host.md#packed-hosts): the same
//! [`Pool`] routing and `h2 host pack` supervisor as a Cloudflare container,
//! with one local supervisor process per pack. On when `LIVE_HOST_PACK_MATCHES`
//! is set.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use live_proto::host_pack::{
    self, AddMatch, Added, Limits, MatchStatus, Pool, Refusal, WarmDemand, UDP_WIDTH, http, valid_pack,
};
use live_proto::hosts::{HostClass, Transport};

use super::{HostBoot, HostError, HostLease, stop_child};
use crate::hosts::now_ms;

const REF_PREFIX: &str = "pack:";
const READY: Duration = Duration::from_secs(15);
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const PLACE_TRIES: usize = 3;

/// `pack:<pack>/<lease>`.
pub(super) fn provider_ref(pack: &str, lease: &str) -> String {
    format!("{REF_PREFIX}{pack}/{lease}")
}

pub(super) fn parse_ref(reference: &str) -> Option<(&str, &str)> {
    let (pack, lease) = reference.strip_prefix(REF_PREFIX)?.split_once('/')?;
    (valid_pack(pack) && !lease.is_empty()).then_some((pack, lease))
}

/// Matches per local pack, from `LIVE_HOST_PACK_MATCHES`. Unset: no packing.
pub(super) fn pack_matches(setting: Option<&str>) -> Option<usize> {
    match setting?.trim() {
        "auto" => Some(host_pack::MAX_PACK_MATCHES),
        s => Some(s.parse::<usize>().ok()?.clamp(1, host_pack::MAX_PACK_MATCHES)),
    }
}

/// Port bases for pack window `window`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Window {
    pub control: u16,
    pub base_port: u16,
    pub tunnel_port: u16,
    pub agent_port: u16,
}

pub(super) fn window(
    first: u16,
    tunnel: u16,
    control: u16,
    window: u16,
) -> Result<Window, HostError> {
    let width = host_pack::MAX_PACK_MATCHES as u32;
    let w = u32::from(window);
    let base = u32::from(first) + w * width * u32::from(UDP_WIDTH);
    let tunnel = u32::from(tunnel) + w * width;
    let agent = u32::from(host_pack::AGENT_PORT) + w * width;
    let control = u32::from(control) + w;
    let top = [
        base + width * u32::from(UDP_WIDTH) - 1,
        tunnel + width - 1,
        agent + width - 1,
        control,
    ];
    if first == 0 || tunnel == 0 || control == 0 || top.iter().any(|port| *port > u32::from(u16::MAX)) {
        return Err(HostError::new("local host port range exhausted"));
    }
    Ok(Window {
        control: control as u16,
        base_port: base as u16,
        tunnel_port: tunnel as u16,
        agent_port: agent as u16,
    })
}

/// Large local deployments can reach the next TCP range (for example,
/// tunnel window 90 starts at the default control port). Skip those
/// collisions as well as windows already claimed by another group.
fn next_window(
    first: u16,
    tunnel: u16,
    control: u16,
    used: &[u16],
) -> Result<(u16, Window), HostError> {
    let occupied = used
        .iter()
        .map(|&n| window(first, tunnel, control, n))
        .collect::<Result<Vec<_>, _>>()?;
    let ranges = |ports: Window| {
        let last = host_pack::MAX_PACK_MATCHES as u16 - 1;
        [
            (ports.control, ports.control),
            (ports.tunnel_port, ports.tunnel_port + last),
            (ports.agent_port, ports.agent_port + last),
        ]
    };
    let overlaps = |(a, b): (u16, u16), (c, d): (u16, u16)| a <= d && c <= b;
    for n in 0..=u16::MAX {
        let ports = window(first, tunnel, control, n)?;
        let tcp = ranges(ports);
        if tcp.iter().enumerate().any(|(i, &r)| tcp[..i].iter().any(|&s| overlaps(r, s))) {
            continue;
        }
        if !occupied.iter().any(|p| {
            ranges(*p).into_iter().any(|r| tcp.iter().any(|&s| overlaps(r, s)))
        }) {
            return Ok((n, ports));
        }
    }
    Err(HostError::new("no free local pack windows"))
}

struct LocalPack {
    window: u16,
    control: SocketAddr,
    child: Child,
    idle_task: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for LocalPack {
    fn drop(&mut self) {
        if let Some(task) = self.idle_task.take() {
            task.abort();
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = stop_child(&mut self.child);
        }
    }
}

// Each class in each region has its own placement history. All local packs
// share this machine's port space.
type Group = (Option<String>, HostClass);
type PackKey = (Group, String);

#[derive(Default)]
struct State {
    pools: HashMap<Group, Pool>,
    packs: HashMap<PackKey, LocalPack>,
}

impl State {
    fn pool(&mut self, group: &Group) -> &mut Pool {
        self.pools.entry(group.clone()).or_default()
    }

    fn key(&self, pack: &str, lease: &str) -> Option<PackKey> {
        self.pools.iter()
            .find(|(_, pool)| pool.pack_of(lease) == Some(pack))
            .map(|(group, _)| (group.clone(), pack.to_owned()))
    }
}

type Spawn = dyn Fn(&Path, Limits, &[u16]) -> Result<LocalPack, HostError> + Send + Sync;

pub(super) struct LocalPacks {
    max_matches: usize,
    launch: Box<Spawn>,
    state: Arc<tokio::sync::Mutex<State>>,
    /// Lease → its match's loopback tunnel listener.
    tunnels: std::sync::Mutex<HashMap<String, SocketAddr>>,
}

impl LocalPacks {
    pub(super) fn new(max_matches: usize, root: PathBuf) -> Self {
        Self {
            max_matches,
            launch: Box::new(move |bin, limits, used| Self::spawn(&root, bin, limits, used)),
            state: Arc::default(),
            tunnels: std::sync::Mutex::default(),
        }
    }

    fn spawn(root: &Path, bin: &Path, limits: Limits, used: &[u16]) -> Result<LocalPack, HostError> {
        let env = |name: &str| std::env::var(name).ok();
        let first: u16 = env("H2_BASE_PORT")
            .unwrap_or_else(|| "31000".into())
            .parse()
            .map_err(|_| HostError::new("invalid H2_BASE_PORT"))?;
        let mut tunnel: SocketAddr = env("H2_TUNNEL_LISTEN")
            .unwrap_or_else(|| "127.0.0.1:31100".into())
            .parse()
            .map_err(|_| HostError::new("invalid H2_TUNNEL_LISTEN"))?;
        // The tunnel listener trusts its relay; keep it on loopback.
        if tunnel.ip().is_unspecified() {
            tunnel.set_ip(Ipv4Addr::LOCALHOST.into());
        }
        if !tunnel.ip().is_loopback() {
            return Err(HostError::new("local H2_TUNNEL_LISTEN must use a loopback address"));
        }
        let control: u16 = env("LIVE_HOST_PACK_CONTROL_PORT")
            .unwrap_or_else(|| "32000".into())
            .parse()
            .map_err(|_| HostError::new("invalid LIVE_HOST_PACK_CONTROL_PORT"))?;
        let (window, ports) = next_window(first, tunnel.port(), control, used)?;
        let control = SocketAddr::from((Ipv4Addr::LOCALHOST, ports.control));
        let data = root.join(format!("window-{window}"));
        std::fs::create_dir_all(&data)?;
        let mut cmd = Command::new(bin);
        cmd.args(["host", "pack", "--control-listen"])
            .arg(control.to_string())
            .arg("--max-matches")
            .arg(limits.max_matches.to_string())
            .arg("--vcpus")
            .arg(limits.vcpus.to_string())
            .arg("--memory-mib")
            .arg(limits.memory_mib.to_string())
            .arg("--base-port")
            .arg(ports.base_port.to_string())
            .arg("--tunnel-listen")
            .arg(SocketAddr::new(tunnel.ip(), ports.tunnel_port).to_string())
            .arg("--agent-port")
            .arg(ports.agent_port.to_string())
            .arg("--data")
            .arg(&data)
            .env_remove("H2_AGENT")
            // The pack watches this pipe and ends its matches if Live goes away.
            .env("H2_HOST_PARENT_PIPE", "1")
            .stdin(Stdio::piped());
        if let Some(public) = env("H2_PUBLIC_ADDR")
            && let Ok(mut address) = public.parse::<SocketAddrV4>()
        {
            let shifted = u32::from(address.port()) + u32::from(ports.base_port - first);
            address.set_port(
                u16::try_from(shifted)
                    .map_err(|_| HostError::new("local host port range exhausted"))?,
            );
            cmd.env("H2_PUBLIC_ADDR", address.to_string());
        }
        #[cfg(unix)]
        cmd.process_group(0);
        Ok(LocalPack {
            window,
            control,
            child: cmd.spawn()?,
            idle_task: None,
        })
    }

    /// The timer owns no provider reference. A placement cancels it and each
    /// search pulse replaces it. This also reaps the last idle child when
    /// matchmaking goes quiet and nothing calls the provider again.
    fn linger(&self, state: &mut State, key: &PackKey) {
        let local = state.packs.get_mut(key).expect("pack is ready");
        if let Some(task) = local.idle_task.take() {
            task.abort();
        }
        let weak = Arc::downgrade(&self.state);
        let key = key.clone();
        let until = tokio::time::Instant::now() + Duration::from_millis(host_pack::LINGER_MS as u64);
        local.idle_task = Some(tokio::spawn(async move {
            tokio::time::sleep_until(until).await;
            let Some(state) = weak.upgrade() else { return };
            let mut state = state.lock().await;
            if state.pool(&key.0).leases(&key.1) == 0 {
                state.pool(&key.0).forget(&key.1);
                state.packs.remove(&key);
            }
        }));
    }

    fn forget(&self, state: &mut State, key: &PackKey) {
        state.packs.remove(key);
        for lease in state.pool(&key.0).forget(&key.1) {
            self.tunnels.lock().unwrap().remove(&lease);
        }
    }

    fn reap(&self, state: &mut State) -> Result<(), HostError> {
        let mut exited = Vec::new();
        for (key, local) in &mut state.packs {
            if local.child.try_wait()?.is_some() {
                exited.push(key.clone());
            }
        }
        for key in exited {
            self.forget(state, &key);
        }
        Ok(())
    }

    async fn ensure_pack(
        &self,
        state: &mut State,
        key: &PackKey,
        bin: &Path,
        limits: Limits,
    ) -> Result<(SocketAddr, Option<u64>), HostError> {
        if let Some(local) = state.packs.get(key) {
            return Ok((local.control, None));
        }
        let used: Vec<_> = state.packs.values().map(|pack| pack.window).collect();
        let start = Instant::now();
        let mut local = (self.launch)(bin, limits, &used)?;
        let control = local.control;
        wait_ready(&mut local).await?;
        state.packs.insert(key.clone(), local);
        self.linger(state, key);
        Ok((control, Some(start.elapsed().as_millis() as u64)))
    }

    pub(super) async fn warm(
        &self,
        bin: PathBuf,
        region: Option<&str>,
        class: &HostClass,
        demand: WarmDemand,
    ) -> Result<usize, HostError> {
        let group = (region.map(str::to_owned), class.clone());
        let limits = host_pack::class_limits(class, Some(self.max_matches));
        let mut state = self.state.lock().await;
        self.reap(&mut state)?;
        let plan = state.pool(&group).warm_plan(demand, limits.max_matches, now_ms());
        for name in plan.refresh.into_iter().chain(plan.start) {
            let key = (group.clone(), name);
            let (control, elapsed) = self.ensure_pack(&mut state, &key, &bin, limits).await?;
            match exchange(control, "POST", host_pack::WARM_PATH, Vec::new()).await {
                Ok((204, _)) => {
                    state.pool(&group).warmed(&key.1, now_ms(), elapsed);
                    self.linger(&mut state, &key);
                }
                failed => {
                    // The plan only refreshes empty packs.
                    self.forget(&mut state, &key);
                    return Err(match failed {
                        Ok((status, _)) => HostError::new(format!("local pack warm returned HTTP {status}")),
                        Err(e) => e,
                    });
                }
            }
        }
        Ok(state.pool(&group).free_slots(limits.max_matches))
    }

    pub(super) async fn start(
        &self,
        bin: PathBuf,
        lease: &HostLease,
        boot: &HostBoot,
    ) -> Result<String, HostError> {
        let add = AddMatch {
            lease: lease.id.clone(),
            lease_json: serde_json::to_string(lease)
                .map_err(|_| HostError::new("could not encode local host lease"))?,
            live: boot.live_url.clone(),
            token: boot.token.clone(),
            data_url: boot.data_url.clone(),
            data_sha: boot.data_sha256.clone(),
            archive: boot.archive,
            region: lease.region.clone(),
        };
        let body = serde_json::to_vec(&add).map_err(|e| HostError::new(e.to_string()))?;
        let group = (lease.region.clone(), lease.class.clone());
        let limits = host_pack::class_limits(&lease.class, Some(self.max_matches));
        let mut state = self.state.lock().await;
        self.reap(&mut state)?;
        for _ in 0..PLACE_TRIES {
            let placement = state.pool(&group).place(&lease.id, limits.max_matches);
            let pack = placement.pack().to_owned();
            let key = (group.clone(), pack.clone());
            let (control, cold) = self.ensure_pack(&mut state, &key, &bin, limits).await?;
            if cold.is_some() {
                state.pool(&group).warmed(&pack, now_ms(), cold);
            }
            match exchange(control, "POST", host_pack::MATCHES_PATH, body.clone()).await {
                Ok((200, answer)) => {
                    let added: Added = serde_json::from_slice(&answer)
                        .map_err(|e| HostError::new(format!("pack answer: {e}")))?;
                    state.pool(&group).admitted_at(
                        &pack, &lease.id, lease.deadline_ms(), now_ms(),
                    );
                    if let Some(task) = state.packs.get_mut(&key).and_then(|p| p.idle_task.take()) {
                        task.abort();
                    }
                    if lease.transport == Transport::Tunnel {
                        self.tunnels.lock().unwrap().insert(
                            lease.id.clone(),
                            SocketAddr::from((Ipv4Addr::LOCALHOST, added.ports.tunnel_port)),
                        );
                    }
                    return Ok(provider_ref(&pack, &lease.id));
                }
                Ok((409, answer)) => {
                    let why = serde_json::from_slice::<Refusal>(&answer)
                        .map_or_else(|_| "refused".to_owned(), |r| r.to_string());
                    eprintln!("live: local {pack} refused {}: {why}", lease.id);
                    if state.pool(&group).refused(&pack) {
                        self.forget(&mut state, &key);
                    }
                }
                failed => {
                    // A lingering supervisor can exit between placement and the
                    // request. Only retry a transport failure after reaping it.
                    if failed.is_err()
                        && state.packs.get_mut(&key).unwrap().child.try_wait()?.is_some()
                    {
                        self.forget(&mut state, &key);
                        continue;
                    }
                    if state.pool(&group).refused(&pack) {
                        self.forget(&mut state, &key);
                    }
                    return Err(match failed {
                        Ok((status, _)) => {
                            HostError::new(format!("local pack returned HTTP {status}"))
                        }
                        Err(error) => error,
                    });
                }
            }
        }
        Err(HostError::new("no local host pack has room for the match"))
    }

    pub(super) async fn stop(&self, lease_id: &str, reference: &str) -> Result<(), HostError> {
        let Some((pack, lease)) = parse_ref(reference).filter(|(_, lease)| *lease == lease_id)
        else {
            return Ok(());
        };
        let mut state = self.state.lock().await;
        self.reap(&mut state)?;
        let Some(key) = state.key(pack, lease) else { return Ok(()) };
        if let Some(local) = state.packs.get(&key) {
            let path = format!("{}/{lease}", host_pack::MATCHES_PATH);
            match exchange(local.control, "DELETE", &path, Vec::new()).await {
                Ok((204, _)) => {}
                Ok((status, _)) => {
                    return Err(HostError::new(format!("local pack returned HTTP {status}")));
                }
                Err(error) => return Err(error),
            }
        }
        state.pool(&key.0).release_at(lease, now_ms());
        if state.pool(&key.0).leases(pack) == 0 {
            self.linger(&mut state, &key);
        }
        self.tunnels.lock().unwrap().remove(lease);
        Ok(())
    }

    pub(super) async fn alive(&self, reference: &str) -> Result<bool, HostError> {
        let Some((pack, lease)) = parse_ref(reference) else {
            return Ok(false);
        };
        let mut state = self.state.lock().await;
        self.reap(&mut state)?;
        let Some(key) = state.key(pack, lease) else { return Ok(false) };
        let Some(local) = state.packs.get(&key) else { return Ok(false) };
        let path = format!("{}/{lease}", host_pack::MATCHES_PATH);
        Ok(match exchange(local.control, "GET", &path, Vec::new()).await? {
            (200, body) => serde_json::from_slice::<MatchStatus>(&body).is_ok_and(|m| m.alive),
            _ => false,
        })
    }

    #[cfg_attr(not(feature = "native"), allow(dead_code))]
    pub(super) fn tunnel_addr(&self, lease_id: &str, reference: &str) -> Option<SocketAddr> {
        let (_, lease) = parse_ref(reference).filter(|(_, lease)| *lease == lease_id)?;
        self.tunnels.lock().unwrap().get(lease).copied()
    }
}

async fn wait_ready(local: &mut LocalPack) -> Result<(), HostError> {
    let deadline = Instant::now() + READY;
    loop {
        if let Some(status) = local.child.try_wait()? {
            return Err(HostError::new(format!("local pack exited before control readiness ({status})")));
        }
        match exchange(local.control, "GET", host_pack::STATUS_PATH, Vec::new()).await {
            Ok((200, body)) if serde_json::from_slice::<host_pack::PackStatus>(&body).is_ok() => {
                return Ok(());
            }
            Ok((200, _)) => return Err(HostError::new("unexpected service on the pack control port")),
            _ if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Ok((status, _)) => {
                return Err(HostError::new(format!("local pack returned HTTP {status}")));
            }
            Err(error) => return Err(error),
        }
    }
}

/// One request to a pack's loopback control port.
async fn exchange(
    control: SocketAddr,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> Result<(u16, Vec<u8>), HostError> {
    let request = http::request(method, path, &body);
    tokio::task::spawn_blocking(move || exchange_blocking(control, &request))
        .await
        .map_err(|e| HostError::new(e.to_string()))?
        .map_err(HostError::from)
}

fn exchange_blocking(control: SocketAddr, request: &[u8]) -> io::Result<(u16, Vec<u8>)> {
    let mut stream = TcpStream::connect_timeout(&control, IO_TIMEOUT)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.write_all(request)?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match http::parse_response(&buf) {
            http::Parse::Done(answer) => return Ok(answer),
            http::Parse::Invalid(why) => return Err(io::Error::other(why)),
            http::Parse::Partial => {}
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

#[cfg(test)]
#[path = "local_pack_tests.rs"]
mod tests;
