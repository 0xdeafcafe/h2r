//! The co-op link that offers the Forge lanes (docs/game/forge-design.md,
//! F5; docs/live/live-coop.md, section 8): one [`coop::Peer`] per other
//! machine of a synchronous network game, over the co-op channel, pumped
//! each tick and offered to the session as it opens.
//!
//! The session opens only in a synchronous network game whose lobby is in
//! Forge mode ([`crate::forge_map::admitted_shared`],
//! [`crate::lobby_start::forge_session`]); the lane is the co-op
//! datagram's Forge section, a stream of its own beside everything else
//! the channel carries. A seat that joins later is added
//! ([`crate::forge_glue::net::add_seat`]), one that leaves is dropped, and
//! the link closing ends the session ([`crate::forge_glue::net::end`]).
//! The channel is the direct UDP one, with the Live session's relay
//! beside it ([`coop::link::Fallback`]) for two machines that cannot
//! reach each other: the host's advertisements say the session runs
//! (`flags: "forge"`, [`Transport::forge`]) while it does, which is what
//! the relay admits.

use std::collections::BTreeSet;
use std::net::SocketAddrV4;
use std::sync::{Arc, Mutex};

use agent_proto::sim::SessionKind;
use guest_layout::memory::GuestMemory;
use forge::net::Seat;

use super::net::{self, CoopLane};
use crate::coop_probe;
use crate::layout_glue::ReadGm;

/// The input delay the Forge-only peers run with. The pads never cross
/// them (the retail synchronous path carries the game), so the number
/// only has to be the same on both machines.
const DELAY: u32 = 2;

/// The datagram path of the co-op channel: this machine's end and the
/// other machines' (the game's Live transport supplies both from XNet,
/// where the channel is the base port plus `coop::link::COOP_PORT_OFFSET`),
/// with the Live server's relay beside the direct path for two machines
/// that cannot reach each other.
pub trait Transport: Send + Sync {
    /// This machine's own co-op channel endpoint, which a link binds.
    fn local(&self) -> Option<SocketAddrV4>;
    /// The other machines of the co-op session, as co-op channel
    /// endpoints: every guest for a host, the host for a guest.
    fn remotes(&self, host: bool) -> Vec<SocketAddrV4>;
    /// The datagram path to one of them, or none while it cannot be had:
    /// the direct channel, with the session's relay as the fallback a
    /// silent direct path falls back to.
    fn link(&self, remote: SocketAddrV4, host: bool) -> Option<Box<dyn coop::link::Link + Send>>;
    /// A shared Forge session began (or ended) on this machine: the
    /// advertisements that let the session's machines use the relay follow.
    fn forge(&self, _on: bool) {}
}

/// How long a direct path stays quiet before the relay carries the
/// datagrams too (microseconds, as [`coop::link::Fallback`] wants).
const RELAY_SILENCE: u64 = 3_000_000;

/// The game's transport: the Live HLE's XNet, whose peers are the session's
/// machines. Every machine serves its lanes from one socket bound at its
/// own channel (`coop::link::UdpPort`): the host's to each guest, a
/// guest's to the host. Beside the direct path, the Live session's relay
/// (`coop::relay::RelayLink`, `Hle::coop_relay`) serves one lane — a relay room
/// holds two machines — connecting on a thread of its own so the game
/// never waits on it.
pub struct HleTransport {
    hle: Option<Arc<live_hle::Hle>>,
    /// The one bound socket, and whether binding it has failed already.
    port: Mutex<Bound>,
    /// The session's relay: the shared slot a background connect fills,
    /// and the one lane it serves. None until a lane asks for it.
    relay: Mutex<Option<Relay>>,
    /// Whether the advertisements last said this console runs a shared
    /// Forge session, so the HLE is told only on change.
    forge: Mutex<Option<bool>>,
}

enum Bound {
    Untried,
    Failed,
    Port(Arc<coop::link::UdpPort>),
}

/// The session's relay, as a lane's fallback path.
struct Relay {
    /// The Live session whose room this is.
    session: String,
    /// The connected relay, once the background connect lands.
    link: Arc<Mutex<Option<coop::relay::RelayLink>>>,
    /// The lane the relay serves: a room holds two machines, so the first
    /// lane to ask takes it.
    serves: Option<SocketAddrV4>,
}

impl HleTransport {
    pub fn new(hle: Option<Arc<live_hle::Hle>>) -> HleTransport {
        HleTransport {
            hle,
            port: Mutex::new(Bound::Untried),
            relay: Mutex::new(None),
            forge: Mutex::new(None),
        }
    }

    /// The Live session whose relay this machine uses: the one it hosts,
    /// or the one it joined.
    fn live_session(&self, host: bool) -> Option<String> {
        let hle = self.hle.as_ref()?;
        if host {
            return hle.hosted_session();
        }
        match hle.joined(std::time::Duration::ZERO) {
            live_hle::Joined::Known { session, .. } => Some(session.session_id.clone()),
            live_hle::Joined::Unknown { session } => Some(session),
            live_hle::Joined::NotJoined => None,
        }
    }

    /// The relay of this machine's Live session, for `remote`'s lane: the
    /// shared slot a background connect fills. None when no session names
    /// one, or another lane already has the room (a relay holds two
    /// machines). A new Live session starts a fresh one.
    fn relay_for(
        &self,
        host: bool,
        remote: SocketAddrV4,
    ) -> Option<Arc<Mutex<Option<coop::relay::RelayLink>>>> {
        let hle = self.hle.as_ref()?;
        let session = self.live_session(host)?;
        let mut relay = self.relay.lock().unwrap_or_else(|e| e.into_inner());
        if relay.as_ref().is_some_and(|r| r.session != session) {
            *relay = None;
        }
        let relay = relay.get_or_insert_with(|| Relay {
            session: session.clone(),
            link: Arc::new(Mutex::new(None)),
            serves: None,
        });
        if relay.serves.is_some_and(|served| served != remote) {
            return None;
        }
        relay.serves = Some(remote);
        if relay.link.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
            let hle = Arc::clone(hle);
            let slot = Arc::clone(&relay.link);
            let asked = session;
            let _ = background::spawn("forge-relay", move || {
                connect_relay(hle, asked, slot);
            });
        }
        Some(Arc::clone(&relay.link))
    }
}

/// Asks the Live worker for the session's relay URL and token until it
/// answers, connects, and puts the link in `slot`. Gives up after
/// [`RELAY_TRIES`] answers that refuse, leaving the direct path alone.
fn connect_relay(
    hle: Arc<live_hle::Hle>,
    session: String,
    slot: Arc<Mutex<Option<coop::relay::RelayLink>>>,
) {
    for _ in 0..RELAY_TRIES {
        if let Some(Ok((url, token))) = hle.coop_relay(&session, std::time::Duration::from_secs(2)) {
            match coop::relay::RelayLink::connect(&url, &token) {
                Ok(link) => {
                    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(link);
                    eprintln!("forge: the co-op relay of session {session} is up");
                    return;
                }
                Err(e) => eprintln!("forge: the co-op relay of session {session}: {e}"),
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// The relay's answers that refuse before the connector gives up: the
/// flag the host sets may still be on its way to the server.
const RELAY_TRIES: usize = 20;

/// The session's relay while it connects: a lane's fallback path that
/// drops everything until the connect lands, then carries the datagrams.
struct RelayWhenUp(Arc<Mutex<Option<coop::relay::RelayLink>>>);

impl coop::link::Link for RelayWhenUp {
    fn send(&mut self, datagram: &[u8]) {
        if let Some(relay) = self.0.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            relay.send(datagram);
        }
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .and_then(coop::link::Link::recv)
    }
}

impl Transport for HleTransport {
    fn local(&self) -> Option<SocketAddrV4> {
        self.hle.as_ref().map(|hle| hle.coop_endpoint())
    }

    fn remotes(&self, host: bool) -> Vec<SocketAddrV4> {
        let Some(hle) = self.hle.as_ref() else {
            return Vec::new();
        };
        let mut endpoints = hle.coop_endpoints();
        if !host {
            // A guest's lane goes to the host: the endpoint its joined
            // session names, else the one machine its XNet knows (a
            // synchronous guest exchanges game traffic with the host only).
            match hle.joined(std::time::Duration::ZERO) {
                live_hle::Joined::Known { session, .. } => {
                    // Its public endpoint, or its LAN one when this console
                    // is behind the host's NAT (`Session::reach_order`).
                    let hosts: Vec<_> = session
                        .reach_order()
                        .into_iter()
                        .map(|base| {
                            SocketAddrV4::new(
                                *base.ip(),
                                base.port().saturating_add(coop::link::COOP_PORT_OFFSET),
                            )
                        })
                        .collect();
                    endpoints.retain(|endpoint| hosts.contains(endpoint));
                }
                _ if endpoints.len() == 1 => {}
                _ => endpoints.clear(),
            }
        }
        endpoints.sort_unstable();
        endpoints.dedup();
        endpoints
    }

    fn link(&self, remote: SocketAddrV4, host: bool) -> Option<Box<dyn coop::link::Link + Send>> {
        let local = self.local()?;
        let mut bound = self.port.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*bound, Bound::Untried) {
            *bound = match coop::link::UdpPort::bind(std::net::SocketAddr::V4(local)) {
                Ok(port) => Bound::Port(Arc::new(port)),
                Err(e) => {
                    eprintln!("forge: the co-op channel at {local} did not bind: {e}");
                    Bound::Failed
                }
            };
        }
        let direct = match &*bound {
            Bound::Port(port) => port.link_to(std::net::SocketAddr::V4(remote)),
            _ => return None,
        };
        drop(bound);
        // The relay serves one lane beside the direct path: while the
        // direct path carries the datagrams it stays quiet, and a silent
        // one falls back to it ([`coop::link::Fallback`]). Until the
        // background connect lands it carries nothing.
        match self.relay_for(host, remote) {
            Some(slot) => Some(Box::new(coop::link::Fallback::new(
                direct,
                RelayWhenUp(slot),
                coop::link::wall_clock(),
                RELAY_SILENCE,
            ))),
            None => Some(Box::new(direct)),
        }
    }

    fn forge(&self, on: bool) {
        let mut said = self.forge.lock().unwrap_or_else(|e| e.into_inner());
        if *said == Some(on) {
            return;
        }
        *said = Some(on);
        if let Some(hle) = self.hle.as_ref() {
            hle.set_forge(on);
        }
    }
}

/// What one machine knows of the co-op session it is in, read from the
/// game: its role, and a number every machine of the session derives the
/// same, for the co-op datagram's session tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coop {
    pub host: bool,
    /// The retail session's address in guest memory, which every machine
    /// of one build reads the same; 0 when the session cannot be read.
    pub session: u32,
}

impl Coop {
    /// The synchronous network game the simulation world names, if it is
    /// one: a co-op host or guest.
    pub fn read(m: &dyn GuestMemory) -> Option<Coop> {
        let sim = coop_probe::read_sim(&ReadGm(m))?;
        let host = match SessionKind::from_sim_mode(sim.mode) {
            SessionKind::CoopHost => true,
            SessionKind::CoopGuest => false,
            _ => return None,
        };
        Some(Coop {
            host,
            session: coop_probe::read_session(&ReadGm(m)).map_or(0, |s| s.addr),
        })
    }
}

type Wire = coop::Peer<Box<dyn coop::link::Link + Send>>;

/// One other machine of the session: its co-op channel endpoint and the
/// peer the Forge lane runs over.
struct Remote {
    endpoint: SocketAddrV4,
    peer: Arc<Mutex<Wire>>,
    /// The seat the host gave this lane; a guest's own lane has none.
    seat: Option<Seat>,
}

/// The co-op link: the peers the Forge lanes run over, kept current with
/// the session's machines and offered to the Forge session.
pub struct CoopLink {
    transport: Box<dyn Transport>,
    remotes: Vec<Remote>,
    session: Option<Coop>,
    /// The advertisements say this console runs a shared Forge session.
    was_forge: bool,
}

impl CoopLink {
    pub fn new(transport: Box<dyn Transport>) -> CoopLink {
        CoopLink {
            transport,
            remotes: Vec::new(),
            session: None,
            was_forge: false,
        }
    }

    /// One game tick: read the co-op session, add and drop lanes as its
    /// machines come and go, offer the lanes, and pump the peers so the
    /// Forge stream flows. Call before the session opens.
    pub fn tick(&mut self, m: &dyn GuestMemory) {
        let coop = if crate::lobby_start::forge_session() {
            Coop::read(m)
        } else {
            None
        };
        self.step(coop);
    }

    /// One tick with the co-op session given: [`Coop::read`] of the game,
    /// or none while there is no synchronous game to link.
    fn step(&mut self, coop: Option<Coop>) {
        let Some(coop) = coop else {
            self.close("the co-op link closed");
            return;
        };
        let before = self.session;
        if before.is_some_and(|s| s != coop) {
            self.close("a new co-op session began");
        }
        if before.is_none() && coop.host && !self.was_forge {
            // The host's advertisements say a shared Forge session runs,
            // so the session's machines may use the relay.
            self.transport.forge(true);
            self.was_forge = true;
        }
        self.session = Some(coop);
        let endpoints = self.transport.remotes(coop.host);
        // A guest's one lane goes to the host; anything else its
        // transport might name is not this session's host.
        let endpoints: Vec<_> = if coop.host {
            endpoints
        } else {
            endpoints.into_iter().take(1).collect()
        };
        let had = !self.remotes.is_empty();
        self.remotes.retain(|remote| {
            let keep = endpoints.contains(&remote.endpoint);
            if !keep && let Some(seat) = remote.seat {
                net::drop_seat(seat);
            }
            keep
        });
        for endpoint in endpoints {
            if self.remotes.iter().any(|r| r.endpoint == endpoint) {
                continue;
            }
            let Some(peer) = self.peer(&coop, endpoint) else {
                continue;
            };
            // The host names each lane's seat; a guest's one lane needs
            // none (the host's welcome tells it its seat).
            let seat = coop.host.then(|| self.free_seat());
            if let Some(seat) = seat {
                net::add_seat(
                    seat,
                    Box::new(CoopLane(Arc::clone(&peer))) as Box<dyn forge::net::Lane + Send>,
                );
            }
            self.remotes.push(Remote {
                endpoint,
                peer,
                seat,
            });
        }
        if had && self.remotes.is_empty() {
            self.close("the last machine left the co-op link");
            return;
        }
        // The offer stands for the session that opens next: re-made every
        // tick, so a session that ended (a map change) finds lanes again.
        if coop.host && !self.remotes.is_empty() {
            net::offer_host(self.lanes());
        } else if !coop.host
            && let Some(remote) = self.remotes.first()
        {
            let lane: Box<dyn forge::net::Lane + Send> =
                Box::new(CoopLane(Arc::clone(&remote.peer)));
            net::offer_guest(lane);
        }
        self.pump();
    }

    /// The peers of the lanes as they stand, for tests.
    #[cfg(test)]
    fn peers(&self) -> Vec<(SocketAddrV4, Option<Seat>)> {
        self.remotes
            .iter()
            .map(|remote| (remote.endpoint, remote.seat))
            .collect()
    }

    /// The lanes as they stand: one per remote, by its seat.
    fn lanes(&self) -> Vec<(Seat, Box<dyn forge::net::Lane + Send>)> {
        self.remotes
            .iter()
            .filter_map(|remote| {
                remote.seat.map(|seat| {
                    (
                        seat,
                        Box::new(CoopLane(Arc::clone(&remote.peer)))
                            as Box<dyn forge::net::Lane + Send>,
                    )
                })
            })
            .collect()
    }

    /// The lowest seat no lane holds. The host's own is 0.
    fn free_seat(&self) -> Seat {
        let taken: BTreeSet<u8> = self
            .remotes
            .iter()
            .filter_map(|remote| remote.seat.map(|seat| seat.0))
            .collect();
        // A session holds at most SEATS machines; if more lanes were made
        // anyway, the last seat's lane replaces its own in the offer.
        Seat(
            (1..forge::net::SEATS)
                .find(|seat| !taken.contains(seat))
                .unwrap_or(forge::net::SEATS - 1),
        )
    }

    fn peer(&self, coop: &Coop, endpoint: SocketAddrV4) -> Option<Arc<Mutex<Wire>>> {
        let link = self.transport.link(endpoint, coop.host)?;
        // The co-op datagram's session tag must match on both machines;
        // the host is port 0 and a guest port 1.
        let port = usize::from(!coop.host);
        Some(Arc::new(Mutex::new(coop::Peer::new(
            link,
            coop.session,
            port,
            DELAY,
        ))))
    }

    /// The link is gone: the lanes go and the session on them ends.
    fn close(&mut self, why: &'static str) {
        if self.session.take().is_some() || !self.remotes.is_empty() {
            self.remotes.clear();
            if self.was_forge {
                self.transport.forge(false);
                self.was_forge = false;
            }
            net::end(why);
        }
    }

    /// The game ended or changed its map: drop the lanes without ending
    /// anything on the network (the game's own end clears the offer, and
    /// the session goes on to the next map).
    pub fn reset(&mut self) {
        self.remotes.clear();
        self.session = None;
    }

    fn pump(&mut self) {
        for remote in &self.remotes {
            remote.peer.lock().unwrap_or_else(|e| e.into_inner()).pump();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge_glue::net::{self as glue, Change, Offer};
    use forge::look::session::CanDraw;
    use guest_layout::points;
    use guest_layout::testing::FakeGuest;
    use forge::net::{Node, Role};
    use forge::{PaletteItem, Session, World};
    use forge_doc::{BuilderVersions, ForgeDoc, Limits, MapRef, Sha256, WorldBounds};
    use std::collections::{HashMap, VecDeque};

    // ---- a fake co-op link ----

    /// One end of an in-memory datagram path: delivery at once, in order.
    struct FakeLink {
        out: Arc<Mutex<VecDeque<Vec<u8>>>>,
        inn: Arc<Mutex<VecDeque<Vec<u8>>>>,
    }

    fn fake_pair() -> (FakeLink, FakeLink) {
        let (a, b) = (
            Arc::new(Mutex::new(VecDeque::new())),
            Arc::new(Mutex::new(VecDeque::new())),
        );
        (
            FakeLink {
                out: Arc::clone(&b),
                inn: Arc::clone(&a),
            },
            FakeLink { out: a, inn: b },
        )
    }

    impl coop::link::Link for FakeLink {
        fn send(&mut self, datagram: &[u8]) {
            self.out.lock().unwrap().push_back(datagram.to_vec());
        }
        fn recv(&mut self) -> Option<Vec<u8>> {
            self.inn.lock().unwrap().pop_front()
        }
    }

    /// The wires between the fake transports: a `link` call takes the half
    /// its peer left for this machine, or makes a pair and parks one.
    #[derive(Default)]
    struct FakeNet {
        waiting: Mutex<HashMap<SocketAddrV4, VecDeque<FakeLink>>>,
    }

    impl FakeNet {
        fn connect(&self, local: SocketAddrV4, remote: SocketAddrV4) -> FakeLink {
            let mut waiting = self.waiting.lock().unwrap();
            if let Some(half) = waiting.get_mut(&local).and_then(VecDeque::pop_front) {
                return half;
            }
            let (mine, theirs) = fake_pair();
            waiting.entry(remote).or_default().push_back(theirs);
            mine
        }
    }

    struct FakeTransport {
        local: SocketAddrV4,
        net: Arc<FakeNet>,
        remotes: Mutex<Vec<SocketAddrV4>>,
        /// Whether the direct path works; a false one leaves the relay
        /// (`net`) as the only path, as two machines behind NATs are.
        direct: Mutex<bool>,
        /// What `Transport::forge` last said.
        forge: Mutex<Option<bool>>,
    }

    impl FakeTransport {
        fn new(local: SocketAddrV4, net: &Arc<FakeNet>) -> FakeTransport {
            FakeTransport {
                local,
                net: Arc::clone(net),
                remotes: Mutex::new(Vec::new()),
                direct: Mutex::new(true),
                forge: Mutex::new(None),
            }
        }

        /// What the transport's XNet would say: the machines it knows.
        fn set_remotes(&self, remotes: &[SocketAddrV4]) {
            *self.remotes.lock().unwrap() = remotes.to_vec();
        }

        /// Takes the direct path down (or brings it back).
        fn set_direct(&self, up: bool) {
            *self.direct.lock().unwrap() = up;
        }

        fn forge(&self) -> Option<bool> {
            *self.forge.lock().unwrap()
        }
    }

    impl Transport for FakeTransport {
        fn local(&self) -> Option<SocketAddrV4> {
            Some(self.local)
        }
        fn remotes(&self, host: bool) -> Vec<SocketAddrV4> {
            let _ = host;
            self.remotes.lock().unwrap().clone()
        }
        fn link(&self, remote: SocketAddrV4, _host: bool) -> Option<Box<dyn coop::link::Link + Send>> {
            let link = self.net.connect(self.local, remote);
            if *self.direct.lock().unwrap() {
                Some(Box::new(link))
            } else {
                // The direct path is dead: the relay carries the datagrams
                // (from the start, as `Fallback` does until direct speaks).
                Some(Box::new(coop::link::Fallback::new(
                    Dead,
                    link,
                    coop::link::wall_clock(),
                    3_000_000,
                )))
            }
        }
        fn forge(&self, on: bool) {
            *self.forge.lock().unwrap() = Some(on);
        }
    }

    /// A direct path that carries nothing.
    struct Dead;

    impl coop::link::Link for Dead {
        fn send(&mut self, _datagram: &[u8]) {}
        fn recv(&mut self) -> Option<Vec<u8>> {
            None
        }
    }

    fn addr(n: u16) -> SocketAddrV4 {
        SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, n)
    }

    /// The shared fake transport, handed to the link.
    struct Shared(Arc<FakeTransport>);

    impl Transport for Shared {
        fn local(&self) -> Option<SocketAddrV4> {
            self.0.local()
        }
        fn remotes(&self, host: bool) -> Vec<SocketAddrV4> {
            self.0.remotes(host)
        }
        fn link(&self, remote: SocketAddrV4, host: bool) -> Option<Box<dyn coop::link::Link + Send>> {
            self.0.link(remote, host)
        }
        fn forge(&self, on: bool) {
            Transport::forge(&*self.0, on);
        }
    }

    /// One machine of the story: its co-op link, its fake transport and
    /// its co-op state.
    struct Machine {
        link: CoopLink,
        transport: Arc<FakeTransport>,
        coop: Option<Coop>,
    }

    fn machine(local: SocketAddrV4, net: &Arc<FakeNet>, host: bool) -> Machine {
        let transport = Arc::new(FakeTransport::new(local, net));
        Machine {
            link: CoopLink::new(Box::new(Shared(transport.clone()))),
            transport,
            coop: Some(Coop {
                host,
                session: 0x5e55,
            }),
        }
    }

    // ---- a session to run over the lanes ----

    const BLOCK: forge_doc::BlockKind = forge_doc::BlockKind::Crates;

    fn doc() -> ForgeDoc {
        ForgeDoc {
            format: forge_doc::FORMAT,
            builder: BuilderVersions {
                forge_build: 1,
                pathfind: 1,
                bots: 1,
            },
            base: MapRef {
                map: "linkroom".into(),
                sha256: Sha256::of(b"an invented room"),
            },
            sources: vec![],
            title: "Room".into(),
            author: "Builder".into(),
            description: String::new(),
            recipes: vec![],
            imports: vec![],
            ops: vec![],
            expect: None,
            waves: None,
        }
    }

    fn limits() -> Limits {
        Limits {
            base_counts: std::collections::BTreeMap::from([(BLOCK, 7)]),
            world: Some(WorldBounds {
                min: [-20.; 3],
                max: [20.; 3],
            }),
            ..Default::default()
        }
    }

    fn session() -> Session {
        Session::new(
            doc(),
            limits(),
            vec![forge::PaletteItem {
                block: BLOCK,
                tag: forge_doc::TagKey {
                    group: "bloc".into(),
                    path: r"fixture\cube".into(),
                },
            }],
            10,
        )
        .unwrap()
    }

    #[derive(Default)]
    struct FakeWorld {
        objects: usize,
    }

    impl World for FakeWorld {
        fn place(&mut self, _item: &PaletteItem, _frame: forge::Frame) -> Result<u32, String> {
            self.objects += 1;
            Ok(self.objects as u32)
        }
        fn respawn(&mut self, item: &PaletteItem, f: forge::Frame) -> Result<u32, String> {
            self.place(item, f)
        }
        fn move_object(&mut self, _h: u32, _f: forge::Frame) -> Result<(), String> {
            Ok(())
        }
        fn delete(&mut self, _h: u32) -> Result<(), String> {
            Ok(())
        }
    }

    /// The host's session and node, as the editor opens them from an
    /// offered link.
    struct Hosted {
        node: Node<forge::SessionAdmit, Box<dyn forge::net::Lane + Send>>,
        session: Session,
        world: FakeWorld,
    }

    /// The guest's, likewise.
    struct Joined {
        node: Node<forge::net::DocOnly, Box<dyn forge::net::Lane + Send>>,
        session: Session,
        world: FakeWorld,
    }

    impl Hosted {
        fn step(&mut self, tick: u32) {
            let outgoing = self.session.take_outgoing();
            if !outgoing.is_empty() {
                self.node.submit(outgoing).unwrap();
            }
            let step = self.node.tick(tick).unwrap();
            for item in step.items {
                self.session.apply_item(&mut self.world, &item).unwrap();
            }
        }
    }

    impl Joined {
        fn step(&mut self, tick: u32) {
            let outgoing = self.session.take_outgoing();
            if !outgoing.is_empty() {
                self.node.submit(outgoing).unwrap();
            }
            let step = self.node.tick(tick).unwrap();
            if let Some(state) = &step.joined {
                let seat = self.node.seat().unwrap();
                self.session = self.session.clone().share(seat, "Guest");
                self.session.sync_to(&mut self.world, state).unwrap();
            }
            for item in step.items {
                self.session.apply_item(&mut self.world, &item).unwrap();
            }
        }
    }

    /// The offer and its changes are one global (`forge_glue::net`), so
    /// the tests that touch them take this and run one at a time.
    static NET: Mutex<()> = Mutex::new(());

    /// The host opens its session over the offered lanes, as
    /// [`crate::forge_glue::editing::Editor::open_shared`] does.
    fn open_host() -> Hosted {
        let Some(Offer::Host(lanes)) = glue::take() else {
            panic!("the host's link offered no lanes");
        };
        let session = session().share(forge::net::HOST_SEAT, "Host");
        let mut node = Node::host(
            session.document().clone(),
            session.limits().clone(),
            session.admit(),
            "Host",
            0,
            forge::net::APPLY_DELAY,
        )
        .unwrap();
        for (seat, lane) in lanes {
            node.accept(seat, lane, Role::Editor).unwrap();
        }
        Hosted {
            node,
            session,
            world: FakeWorld::default(),
        }
    }

    /// A guest joins over its offered lane.
    fn open_guest(name: &str) -> Joined {
        let Some(Offer::Guest(lane)) = glue::take() else {
            panic!("the guest's link offered no lane");
        };
        Joined {
            node: Node::guest(limits(), name, CanDraw::default(), lane).unwrap(),
            session: session(),
            world: FakeWorld::default(),
        }
    }

    /// Which lane change it was, once its lane has gone to the node.
    #[derive(Debug, PartialEq, Eq)]
    enum Seen {
        Add(u8),
        Drop(u8),
        End,
    }

    /// What the editor's net tick does with the link's lane changes.
    fn apply_changes(host: &mut Hosted) -> Vec<Seen> {
        glue::take_changes()
            .into_iter()
            .map(|change| match change {
                Change::Add(seat, lane) => {
                    host.node.accept(seat, lane, Role::Editor).unwrap();
                    Seen::Add(seat.0)
                }
                Change::Drop(seat) => {
                    let _ = host.node.disconnect(seat);
                    Seen::Drop(seat.0)
                }
                Change::End(_) => Seen::End,
            })
            .collect()
    }

    #[test]
    fn a_coop_link_opens_the_session_and_a_placement_reaches_the_guest() {
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let net = Arc::new(FakeNet::default());
        let mut host = machine(addr(31004), &net, true);
        let mut guest = machine(addr(31014), &net, false);
        host.transport.set_remotes(&[addr(31014)]);
        guest.transport.set_remotes(&[addr(31004)]);
        host.link.step(host.coop);
        guest.link.step(guest.coop);
        // The offers stand on both sides: the host's by seat, the guest's one.
        host.link.step(host.coop);
        let mut hosted = open_host();
        guest.link.step(guest.coop);
        let mut joined = open_guest("guest");
        assert_eq!(
            host.link.peers(),
            vec![(addr(31014), Some(Seat(1)))],
            "the guest's lane is seat 1"
        );
        // Run the session: the welcome crosses and the guest joins.
        for tick in 1..=200 {
            host.link.step(host.coop);
            guest.link.step(guest.coop);
            hosted.step(tick);
            joined.step(tick);
            if joined.node.state().is_some() {
                break;
            }
        }
        assert!(joined.node.state().is_some(), "the guest never joined");
        assert!(glue::take_changes().is_empty(), "nothing changed yet");
        // A placement on the host reaches the guest.
        hosted
            .session
            .place(
                &mut hosted.world,
                200,
                0,
                forge::Frame {
                    position: [1., 2., 3.],
                    rotation: [0.; 3],
                },
            )
            .unwrap();
        for tick in 201..=400 {
            host.link.step(host.coop);
            guest.link.step(guest.coop);
            hosted.step(tick);
            joined.step(tick);
            if joined.session.objects().len() == 1 {
                break;
            }
        }
        assert_eq!(hosted.session.objects().len(), 1);
        assert_eq!(
            joined.session.objects().len(),
            1,
            "the placement reached the guest"
        );
        assert_eq!(
            joined.session.objects()[0].id,
            hosted.session.objects()[0].id,
            "the guest's object is the host's placement"
        );
        // The game ends: the offer and its claim on the network go.
        glue::clear();
    }

    #[test]
    fn a_guest_arriving_late_gets_a_lane_and_one_leaving_loses_it() {
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let net = Arc::new(FakeNet::default());
        let mut host = machine(addr(31004), &net, true);
        let mut first = machine(addr(31014), &net, false);
        host.transport.set_remotes(&[addr(31014)]);
        first.transport.set_remotes(&[addr(31004)]);
        host.link.step(host.coop);
        let mut hosted = open_host();
        first.link.step(first.coop);
        let mut joined = open_guest("guest");
        for tick in 1..=200 {
            host.link.step(host.coop);
            first.link.step(first.coop);
            hosted.step(tick);
            joined.step(tick);
            if joined.node.state().is_some() {
                break;
            }
        }
        assert!(joined.node.state().is_some());
        // A second machine joins the co-op link: its lane is added.
        let mut second = machine(addr(31024), &net, false);
        second.transport.set_remotes(&[addr(31004)]);
        host.transport.set_remotes(&[addr(31014), addr(31024)]);
        host.link.step(host.coop);
        assert_eq!(apply_changes(&mut hosted), vec![Seen::Add(2)]);
        assert_eq!(
            host.link.peers(),
            vec![(addr(31014), Some(Seat(1))), (addr(31024), Some(Seat(2)))]
        );
        // The second guest joins the running session over its lane.
        second.link.step(second.coop);
        let mut late = open_guest("late");
        for tick in 201..=400 {
            host.link.step(host.coop);
            first.link.step(first.coop);
            second.link.step(second.coop);
            hosted.step(tick);
            joined.step(tick);
            late.step(tick);
            if late.node.state().is_some() {
                break;
            }
        }
        assert!(late.node.state().is_some(), "the late guest never joined");
        // The first guest leaves: its lane is dropped, the other stays.
        host.transport.set_remotes(&[addr(31024)]);
        host.link.step(host.coop);
        assert_eq!(apply_changes(&mut hosted), vec![Seen::Drop(1)]);
        assert_eq!(host.link.peers(), vec![(addr(31024), Some(Seat(2)))]);
        // The link closes: the offer goes and the session on it ends.
        host.transport.set_remotes(&[]);
        host.link.step(host.coop);
        assert_eq!(
            apply_changes(&mut hosted),
            vec![Seen::Drop(2), Seen::End],
            "the last lane goes and the link's close ends the session"
        );
        assert!(!glue::offered(), "no lane is offered any more");
        glue::clear();
    }

    #[test]
    fn a_dead_direct_path_opens_the_session_over_the_relay() {
        let _net = NET.lock().unwrap_or_else(|e| e.into_inner());
        let net = Arc::new(FakeNet::default());
        let mut host = machine(addr(31004), &net, true);
        let mut guest = machine(addr(31014), &net, false);
        host.transport.set_remotes(&[addr(31014)]);
        guest.transport.set_remotes(&[addr(31004)]);
        // The two machines cannot reach each other directly: the relay is
        // the only path, as `Fallback` makes it until direct speaks.
        host.transport.set_direct(false);
        guest.transport.set_direct(false);
        host.link.step(host.coop);
        assert_eq!(
            host.transport.forge(),
            Some(true),
            "the host's advertisements say a shared Forge session runs"
        );
        assert_eq!(guest.transport.forge(), None, "a guest advertises nothing");
        host.link.step(host.coop);
        let mut hosted = open_host();
        guest.link.step(guest.coop);
        let mut joined = open_guest("guest");
        for tick in 1..=400 {
            host.link.step(host.coop);
            guest.link.step(guest.coop);
            hosted.step(tick);
            joined.step(tick);
            if joined.node.state().is_some() {
                break;
            }
        }
        assert!(
            joined.node.state().is_some(),
            "the guest never joined over the relay"
        );
        // A placement on the host reaches the guest over the relay too.
        hosted
            .session
            .place(&mut hosted.world, 400, 0, forge::Frame {
                position: [4., 5., 6.],
                rotation: [0.; 3],
            })
            .unwrap();
        for tick in 401..=800 {
            host.link.step(host.coop);
            guest.link.step(guest.coop);
            hosted.step(tick);
            joined.step(tick);
            if joined.session.objects().len() == 1 {
                break;
            }
        }
        assert_eq!(joined.session.objects().len(), 1);
        // The link closing takes the Forge flag with it.
        host.transport.set_remotes(&[]);
        guest.transport.set_remotes(&[]);
        host.link.step(host.coop);
        guest.link.step(guest.coop);
        assert_eq!(host.transport.forge(), Some(false));
        glue::clear();
    }

    #[test]
    fn the_coop_session_is_read_from_the_simulation_world() {
        let mut m = FakeGuest::new();
        assert_eq!(Coop::read(&m), None);
        m.put_u32(points::SIM_GLOBALS.base, 0x4000);
        assert_eq!(Coop::read(&m), None, "no simulation world yet");
        m.put_u32(0x4000 + coop_probe::SIM_MODE, 2);
        assert_eq!(
            Coop::read(&m),
            Some(Coop {
                host: true,
                session: 0,
            })
        );
        m.put_u32(0x4000 + coop_probe::SIM_MODE, 3);
        assert_eq!(
            Coop::read(&m),
            Some(Coop {
                host: false,
                session: 0,
            })
        );
        m.put_u32(0x4000 + coop_probe::SIM_MODE, 1);
        assert_eq!(Coop::read(&m), None, "a local game is no co-op link");
    }
}
