//! Several dedicated-host matches in one container
//! (docs/live/dedicated-host.md#packed-hosts).
//!
//! A pack is one container (or one local supervisor process) running up to
//! [`Limits::max_matches`] matches, each a separate `h2 host run` child with
//! its own guest state. The module and maps are memory-mapped and shared.
//!
//! Three pure pieces live here so the Worker, native Live and the in-container
//! supervisor agree on them:
//!
//! - [`admit`]: the supervisor's admission check from measured load.
//! - [`Pool`]: which pack a new lease goes to, and which pack stops when its
//!   last lease ends.
//! - The supervisor's control wire ([`AddMatch`], [`Added`], [`PackStatus`])
//!   and its minimal HTTP/1.1 framing ([`http`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One game tick at 30 Hz, in milliseconds.
pub const TICK_MS: f64 = 1000.0 / 30.0;
/// Matches per container unless configured.
pub const DEFAULT_MAX_MATCHES: usize = 3;
/// Share of each vCPU's tick the summed matches may use.
pub const DEFAULT_CPU_BUDGET: f64 = 0.70;
/// Assumed cost of a match with no measurement yet: the top of the
/// observed 6-10 ms per tick for a light dedicated host.
pub const DEFAULT_MS_PER_TICK: f64 = 10.0;
/// Assumed memory of a match with no measurement yet (observed ~340 MB RSS).
pub const DEFAULT_RSS_MIB: u64 = 340;
/// Kept free for the supervisor, the kernel and short spikes.
pub const DEFAULT_RESERVE_MIB: u64 = 256;

/// The supervisor's private control port inside a container.
pub const CONTROL_PORT: u16 = 31099;
/// Slot 0's first XNet UDP port. Each slot takes [`UDP_WIDTH`] ports.
pub const BASE_PORT: u16 = 31000;
pub const UDP_WIDTH: u16 = 7;
/// Slot 0's WebSocket tunnel port. Slot n listens on `TUNNEL_PORT + n`.
pub const TUNNEL_PORT: u16 = 31100;
/// Slot 0's private diagnostic agent port. Slot n uses `AGENT_PORT + n`.
pub const AGENT_PORT: u16 = 14720;

pub const STATUS_PATH: &str = "/v1/pack";
pub const MATCHES_PATH: &str = "/v1/pack/matches";

/// What one pack may run.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    pub max_matches: usize,
    pub vcpus: f64,
    /// The container's memory limit.
    pub memory_mib: u64,
    /// Fraction of `TICK_MS × vcpus` the summed match cost may reach.
    pub cpu_budget: f64,
    pub reserve_mib: u64,
    /// Cost assumed for the match being admitted and for unmeasured ones.
    pub new_ms_per_tick: f64,
    pub new_rss_mib: u64,
}

impl Limits {
    pub fn new(max_matches: usize, vcpus: f64, memory_mib: u64) -> Self {
        Self {
            max_matches,
            vcpus,
            memory_mib,
            cpu_budget: DEFAULT_CPU_BUDGET,
            reserve_mib: DEFAULT_RESERVE_MIB,
            new_ms_per_tick: DEFAULT_MS_PER_TICK,
            new_rss_mib: DEFAULT_RSS_MIB,
        }
    }

    /// Summed per-tick CPU the pack may commit, in milliseconds.
    pub fn tick_budget_ms(&self) -> f64 {
        TICK_MS * self.cpu_budget * self.vcpus.max(0.0)
    }

    /// Memory the matches may use, in MiB.
    pub fn memory_budget_mib(&self) -> u64 {
        self.memory_mib.saturating_sub(self.reserve_mib)
    }
}

/// One running match's measured cost. `None` until the supervisor has
/// enough samples; admission then assumes the configured default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MatchLoad {
    /// CPU time of the match's processes per 33 ms of wall time.
    pub ms_per_tick: Option<f64>,
    /// Proportional resident memory of the match's processes.
    pub rss_mib: Option<u64>,
}

/// Why a pack refused a match. The router tries another pack.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "refused", rename_all = "snake_case")]
pub enum Refusal {
    #[error("the pack already runs {running} of {max} matches")]
    Full { running: usize, max: usize },
    #[error("CPU: {projected_ms:.1} ms per tick would exceed the {budget_ms:.1} ms budget")]
    Cpu { projected_ms: f64, budget_ms: f64 },
    #[error("memory: {projected_mib} MiB would exceed the {budget_mib} MiB budget")]
    Memory { projected_mib: u64, budget_mib: u64 },
    #[error("the pack is shutting down")]
    Draining,
}

/// Admit one more match next to `running`, or say why not.
///
/// `container_mib` is the container's measured non-reclaimable memory
/// (cgroup `memory.current` less inactive file pages). Without it, the
/// matches' own measurements (or the default for unmeasured ones) are
/// summed; per-process RSS counts shared pages in each, so that sum
/// overstates use and errs towards refusing.
pub fn admit(
    limits: &Limits,
    running: &[MatchLoad],
    container_mib: Option<u64>,
) -> Result<(), Refusal> {
    if running.len() >= limits.max_matches {
        return Err(Refusal::Full {
            running: running.len(),
            max: limits.max_matches,
        });
    }
    let budget_ms = limits.tick_budget_ms();
    let projected_ms = running
        .iter()
        .map(|m| m.ms_per_tick.unwrap_or(limits.new_ms_per_tick).max(0.0))
        .sum::<f64>()
        + limits.new_ms_per_tick;
    if projected_ms > budget_ms {
        return Err(Refusal::Cpu {
            projected_ms,
            budget_ms,
        });
    }
    let used = container_mib.unwrap_or_else(|| {
        running
            .iter()
            .map(|m| m.rss_mib.unwrap_or(limits.new_rss_mib))
            .sum()
    });
    let projected_mib = used.saturating_add(limits.new_rss_mib);
    let budget_mib = limits.memory_budget_mib();
    if projected_mib > budget_mib {
        return Err(Refusal::Memory {
            projected_mib,
            budget_mib,
        });
    }
    Ok(())
}

/// Ports one slot of a pack uses, from the pack's bases.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotPorts {
    pub base_port: u16,
    pub tunnel_port: u16,
    pub agent_port: u16,
}

impl SlotPorts {
    /// `None` when a port would pass 65535 or a base is zero.
    pub fn for_slot(base_port: u16, tunnel_port: u16, agent_port: u16, slot: u16) -> Option<Self> {
        if base_port == 0 || tunnel_port == 0 || agent_port == 0 {
            return None;
        }
        let base = u32::from(base_port) + u32::from(slot) * u32::from(UDP_WIDTH);
        let last = base + u32::from(UDP_WIDTH) - 1;
        let tunnel = u32::from(tunnel_port) + u32::from(slot);
        let agent = u32::from(agent_port) + u32::from(slot);
        (last <= 65535 && tunnel <= 65535 && agent <= 65535).then(|| Self {
            base_port: base as u16,
            tunnel_port: tunnel as u16,
            agent_port: agent as u16,
        })
    }
}

// -- Routing and teardown ----------------------------------------------------

/// Where a lease should go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placement {
    /// A running pack with room, or the pack already holding this lease.
    Existing(String),
    /// No pack has room: start this new one.
    New(String),
}

impl Placement {
    pub fn pack(&self) -> &str {
        match self {
            Self::Existing(pack) | Self::New(pack) => pack,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct PackEntry {
    /// Lease ID → its hard deadline (Unix ms).
    leases: BTreeMap<String, i64>,
    /// The lease count when the pack last refused. It has no room until
    /// it drops below this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    full_at: Option<usize>,
}

impl PackEntry {
    fn has_room(&self, max_matches: usize) -> bool {
        let n = self.leases.len();
        n < max_matches && self.full_at.is_none_or(|full| n < full)
    }
}

/// Every pack of one class and region and the leases each runs. The Worker
/// keeps this in a Durable Object's storage; native Live keeps it in memory.
///
/// The pool is the optimistic router. Each pack's supervisor has the final
/// word from measured load: a refusal marks the pack full until one of its
/// leases ends.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pool {
    packs: BTreeMap<String, PackEntry>,
    /// Pack names are never reused, so a late call to a stopped pack
    /// cannot land on a new one.
    next: u64,
}

impl Pool {
    /// Choose a pack for `lease`. A lease already placed keeps its pack.
    /// Otherwise the fullest pack with room wins, so lightly used packs
    /// drain and stop; ties go to the oldest name. With no room anywhere,
    /// name a new pack. Nothing is recorded until [`Pool::admitted`].
    pub fn place(&mut self, lease: &str, max_matches: usize) -> Placement {
        if let Some(pack) = self.pack_of(lease) {
            return Placement::Existing(pack.to_owned());
        }
        let best = self
            .packs
            .iter()
            .filter(|(_, p)| p.has_room(max_matches))
            .max_by(|(a_name, a), (b_name, b)| {
                a.leases
                    .len()
                    .cmp(&b.leases.len())
                    // Prefer the older (smaller-numbered) pack on a tie.
                    .then_with(|| pack_order(b_name).cmp(&pack_order(a_name)))
            })
            .map(|(name, _)| name.clone());
        match best {
            Some(pack) => Placement::Existing(pack),
            None => {
                let name = format!("pack-{}", self.next);
                self.next += 1;
                Placement::New(name)
            }
        }
    }

    /// Record that `pack` now runs `lease` until `deadline_ms`.
    pub fn admitted(&mut self, pack: &str, lease: &str, deadline_ms: i64) {
        // A pack named elsewhere (restored state, another router) still
        // claims its number, so a later new pack never reuses it.
        if let (n, _) = pack_order(pack)
            && n != u64::MAX
        {
            self.next = self.next.max(n + 1);
        }
        self.packs
            .entry(pack.to_owned())
            .or_default()
            .leases
            .insert(lease.to_owned(), deadline_ms);
    }

    /// `pack` refused a lease. Returns true when the pack runs nothing and
    /// should stop (a new pack whose first match failed to start).
    pub fn refused(&mut self, pack: &str) -> bool {
        let Some(entry) = self.packs.get_mut(pack) else {
            return true;
        };
        if !entry.leases.is_empty() {
            entry.full_at = Some(entry.leases.len());
            return false;
        }
        self.packs.remove(pack);
        true
    }

    /// `lease` ended. Returns its pack when that was the pack's last lease;
    /// the pack is forgotten and the caller stops it.
    pub fn release(&mut self, lease: &str) -> Option<String> {
        let pack = self.pack_of(lease)?.to_owned();
        let entry = self.packs.get_mut(&pack)?;
        entry.leases.remove(lease);
        if entry.full_at.is_some_and(|full| entry.leases.len() < full) {
            entry.full_at = None;
        }
        if entry.leases.is_empty() {
            self.packs.remove(&pack);
            return Some(pack);
        }
        None
    }

    /// Drop leases past their hard deadline (a lost stop). Returns the
    /// packs left empty, which the caller stops.
    pub fn expire(&mut self, now_ms: i64) -> Vec<String> {
        let expired: Vec<String> = self
            .packs
            .values()
            .flat_map(|p| p.leases.iter())
            .filter(|(_, deadline)| **deadline <= now_ms)
            .map(|(lease, _)| lease.clone())
            .collect();
        expired.iter().filter_map(|lease| self.release(lease)).collect()
    }

    /// Forget a pack whose container is gone, with all its leases.
    pub fn forget(&mut self, pack: &str) -> Vec<String> {
        self.packs
            .remove(pack)
            .map(|p| p.leases.into_keys().collect())
            .unwrap_or_default()
    }

    pub fn pack_of(&self, lease: &str) -> Option<&str> {
        self.packs
            .iter()
            .find(|(_, p)| p.leases.contains_key(lease))
            .map(|(name, _)| name.as_str())
    }

    /// Leases on `pack`.
    pub fn leases(&self, pack: &str) -> usize {
        self.packs.get(pack).map_or(0, |p| p.leases.len())
    }

    /// Running packs.
    pub fn packs(&self) -> impl Iterator<Item = &str> {
        self.packs.keys().map(String::as_str)
    }

    /// The latest lease deadline on `pack`, for its inactivity timeout and alarm.
    pub fn deadline(&self, pack: &str) -> Option<i64> {
        self.packs.get(pack)?.leases.values().copied().max()
    }

    /// The earliest lease deadline in the pool, for its expiry alarm.
    pub fn next_deadline(&self) -> Option<i64> {
        self.packs
            .values()
            .flat_map(|p| p.leases.values().copied())
            .min()
    }

    pub fn is_empty(&self) -> bool {
        self.packs.is_empty()
    }
}

fn pack_order(name: &str) -> (u64, &str) {
    let n = name
        .strip_prefix("pack-")
        .and_then(|n| n.parse().ok())
        .unwrap_or(u64::MAX);
    (n, name)
}

/// A valid pack name from [`Pool::place`].
pub fn valid_pack(name: &str) -> bool {
    name.strip_prefix("pack-")
        .is_some_and(|n| !n.is_empty() && n.len() <= 20 && n.bytes().all(|b| b.is_ascii_digit()))
}

// -- Supervisor control wire --------------------------------------------------

/// Start one match on a pack. Carries the lease credential and the signed
/// bundle URL, so it has no Debug and is never stored.
#[derive(Clone, Serialize, Deserialize)]
pub struct AddMatch {
    pub lease: String,
    /// The complete lease, as `H2_HOST_LEASE_JSON`.
    pub lease_json: String,
    pub live: String,
    pub token: String,
    pub data_url: String,
    pub data_sha: String,
    /// Each lease selects its own recording and stats upload mode.
    #[serde(default)]
    pub archive: crate::hosts::HostArchive,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

/// A match the pack accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Added {
    pub slot: u16,
    pub ports: SlotPorts,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MatchStatus {
    pub lease: String,
    pub slot: u16,
    /// False once its `h2 host run` exited or is being stopped.
    pub alive: bool,
    pub load: MatchLoad,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PackStatus {
    pub limits: Limits,
    pub matches: Vec<MatchStatus>,
    pub container_mib: Option<u64>,
    pub draining: bool,
}

/// The few HTTP/1.1 bytes the supervisor and native Live exchange over a
/// private loopback or container port. One request per connection, JSON
/// bodies with a Content-Length, no chunking.
pub mod http {
    pub const MAX_HEAD: usize = 16 * 1024;
    pub const MAX_BODY: usize = 64 * 1024;

    #[derive(Debug, PartialEq, Eq)]
    pub struct Request {
        pub method: String,
        pub path: String,
        pub body: Vec<u8>,
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum Parse<T> {
        /// Read more bytes.
        Partial,
        Done(T),
        Invalid(&'static str),
    }

    fn split_head(buf: &[u8]) -> Option<(&str, &[u8])> {
        let end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
        let head = std::str::from_utf8(&buf[..end]).ok()?;
        Some((head, &buf[end + 4..]))
    }

    fn content_length(head: &str) -> Result<usize, &'static str> {
        let mut length = None;
        for line in head.split("\r\n").skip(1) {
            let Some((name, value)) = line.split_once(':') else {
                return Err("malformed header");
            };
            if name.trim().eq_ignore_ascii_case("transfer-encoding") {
                return Err("chunked bodies are not supported");
            }
            if name.trim().eq_ignore_ascii_case("content-length") {
                if length.is_some() {
                    return Err("duplicate content-length");
                }
                length = Some(value.trim().parse().map_err(|_| "bad content-length")?);
            }
        }
        let length = length.unwrap_or(0);
        if length > MAX_BODY {
            return Err("body too large");
        }
        Ok(length)
    }

    /// Parse a request from everything read so far.
    pub fn parse_request(buf: &[u8]) -> Parse<Request> {
        let Some((head, rest)) = split_head(buf) else {
            return if buf.len() > MAX_HEAD {
                Parse::Invalid("head too large")
            } else {
                Parse::Partial
            };
        };
        let mut words = head.split("\r\n").next().unwrap_or("").split(' ');
        let (Some(method), Some(path), Some(version), None) =
            (words.next(), words.next(), words.next(), words.next())
        else {
            return Parse::Invalid("malformed request line");
        };
        if !version.starts_with("HTTP/1.") || !path.starts_with('/') {
            return Parse::Invalid("malformed request line");
        }
        let length = match content_length(head) {
            Ok(n) => n,
            Err(e) => return Parse::Invalid(e),
        };
        if rest.len() < length {
            return Parse::Partial;
        }
        Parse::Done(Request {
            method: method.to_owned(),
            path: path.to_owned(),
            body: rest[..length].to_vec(),
        })
    }

    /// Parse a response: its status and body.
    pub fn parse_response(buf: &[u8]) -> Parse<(u16, Vec<u8>)> {
        let Some((head, rest)) = split_head(buf) else {
            return if buf.len() > MAX_HEAD {
                Parse::Invalid("head too large")
            } else {
                Parse::Partial
            };
        };
        let status = head
            .split("\r\n")
            .next()
            .and_then(|line| line.split(' ').nth(1))
            .and_then(|code| code.parse().ok());
        let Some(status) = status else {
            return Parse::Invalid("malformed status line");
        };
        let length = match content_length(head) {
            Ok(n) => n,
            Err(e) => return Parse::Invalid(e),
        };
        if rest.len() < length {
            return Parse::Partial;
        }
        Parse::Done((status, rest[..length].to_vec()))
    }

    pub fn request(method: &str, path: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "{method} {path} HTTP/1.1\r\nhost: pack\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    pub fn response(status: u16, body: &[u8]) -> Vec<u8> {
        let reason = match status {
            200 => "OK",
            204 => "No Content",
            400 => "Bad Request",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            _ => "Error",
        };
        let mut out = format!(
            "HTTP/1.1 {status} {reason}\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }
}

#[cfg(test)]
#[path = "host_pack_tests.rs"]
mod tests;
