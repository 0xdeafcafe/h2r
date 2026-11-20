//! Packed local hosts (docs/live/dedicated-host.md#packed-hosts): the same
//! [`Pool`] routing and `h2 host pack` supervisor as a Cloudflare container,
//! with one local supervisor process per pack. On when `LIVE_HOST_PACK_MATCHES`
//! is set.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use live_proto::host_pack::{
    self, AddMatch, Added, MatchStatus, Pool, Refusal, UDP_WIDTH, http, valid_pack,
};
use live_proto::hosts::Transport;

use super::{HostBoot, HostError, HostLease, stop_child};

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
    let n: usize = setting?.trim().parse().ok()?;
    Some(n.clamp(1, 8))
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
    max_matches: usize,
    first: u16,
    tunnel: u16,
    control: u16,
    window: u16,
) -> Result<Window, HostError> {
    let width = max_matches as u32;
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
    if first == 0 || top.iter().any(|port| *port > u32::from(u16::MAX)) {
        return Err(HostError::new("local host port range exhausted"));
    }
    Ok(Window {
        control: control as u16,
        base_port: base as u16,
        tunnel_port: tunnel as u16,
        agent_port: agent as u16,
    })
}

struct LocalPack {
    window: u16,
    control: SocketAddr,
    child: Child,
}

#[derive(Default)]
struct State {
    pool: Pool,
    packs: HashMap<String, LocalPack>,
}

pub(super) struct LocalPacks {
    max_matches: usize,
    root: PathBuf,
    state: tokio::sync::Mutex<State>,
    /// Lease → its match's loopback tunnel listener.
    tunnels: std::sync::Mutex<HashMap<String, SocketAddr>>,
}

impl LocalPacks {
    pub(super) fn new(max_matches: usize, root: PathBuf) -> Self {
        Self {
            max_matches,
            root,
            state: tokio::sync::Mutex::default(),
            tunnels: std::sync::Mutex::default(),
        }
    }

    fn spawn(&self, bin: PathBuf, name: &str, window: u16) -> Result<LocalPack, HostError> {
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
        let ports = self::window(self.max_matches, first, tunnel.port(), control, window)?;
        let control = SocketAddr::from((Ipv4Addr::LOCALHOST, ports.control));
        let data = self.root.join(name);
        std::fs::create_dir_all(&data)?;
        let mut cmd = Command::new(bin);
        cmd.args(["host", "pack", "--control-listen"])
            .arg(control.to_string())
            .arg("--max-matches")
            .arg(self.max_matches.to_string())
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
        })
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
        let mut state = self.state.lock().await;
        for _ in 0..PLACE_TRIES {
            let placement = state.pool.place(&lease.id, self.max_matches);
            let pack = placement.pack().to_owned();
            if !state.packs.contains_key(&pack) {
                let window = (0..=u16::MAX)
                    .find(|w| state.packs.values().all(|p| p.window != *w))
                    .ok_or_else(|| HostError::new("no free local pack windows"))?;
                let local = self.spawn(bin.clone(), &pack, window)?;
                let control = local.control;
                state.packs.insert(pack.clone(), local);
                if let Err(error) = wait_ready(control).await {
                    self.drop_pack(&mut state, &pack);
                    return Err(error);
                }
            }
            let control = state.packs[&pack].control;
            match exchange(control, "POST", host_pack::MATCHES_PATH, body.clone()).await {
                Ok((200, answer)) => {
                    let added: Added = serde_json::from_slice(&answer)
                        .map_err(|e| HostError::new(format!("pack answer: {e}")))?;
                    state.pool.admitted(&pack, &lease.id, lease.deadline_ms());
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
                    if state.pool.refused(&pack) {
                        self.drop_pack(&mut state, &pack);
                    }
                }
                failed => {
                    if state.pool.refused(&pack) {
                        self.drop_pack(&mut state, &pack);
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

    fn drop_pack(&self, state: &mut State, pack: &str) {
        if let Some(mut local) = state.packs.remove(pack) {
            let _ = stop_child(&mut local.child);
        }
        for lease in state.pool.forget(pack) {
            self.tunnels.lock().unwrap().remove(&lease);
        }
    }

    pub(super) async fn stop(&self, lease_id: &str, reference: &str) -> Result<(), HostError> {
        let Some((pack, lease)) = parse_ref(reference).filter(|(_, lease)| *lease == lease_id)
        else {
            return Ok(());
        };
        let mut state = self.state.lock().await;
        if state.pool.pack_of(lease) != Some(pack) {
            return Ok(());
        }
        if state.pool.leases(pack) <= 1 {
            // Its last match: stop the whole pack.
            self.drop_pack(&mut state, pack);
            return Ok(());
        }
        if let Some(local) = state.packs.get(pack) {
            let path = format!("{}/{lease}", host_pack::MATCHES_PATH);
            match exchange(local.control, "DELETE", &path, Vec::new()).await {
                Ok((204, _)) => {}
                Ok((status, _)) => {
                    return Err(HostError::new(format!("local pack returned HTTP {status}")));
                }
                Err(error) => return Err(error),
            }
        }
        state.pool.release(lease);
        self.tunnels.lock().unwrap().remove(lease);
        Ok(())
    }

    pub(super) async fn alive(&self, reference: &str) -> Result<bool, HostError> {
        let Some((pack, lease)) = parse_ref(reference) else {
            return Ok(false);
        };
        let mut state = self.state.lock().await;
        if state.pool.pack_of(lease) != Some(pack) {
            return Ok(false);
        }
        let Some(local) = state.packs.get_mut(pack) else {
            return Ok(false);
        };
        if local.child.try_wait()?.is_some() {
            self.drop_pack(&mut state, pack);
            return Ok(false);
        }
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

    /// Stop every pack. Called when the provider drops.
    pub(super) fn stop_all(&mut self) {
        for local in self.state.get_mut().packs.values_mut() {
            let _ = stop_child(&mut local.child);
        }
    }
}

async fn wait_ready(control: SocketAddr) -> Result<(), HostError> {
    let deadline = Instant::now() + READY;
    loop {
        match exchange(control, "GET", host_pack::STATUS_PATH, Vec::new()).await {
            Ok((200, _)) => return Ok(()),
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
