//! Public match ABI over local advertisements or asynchronous server discovery.
//! Exchange keys remain local; remote search records carry a zero HLE key.
use crate::{
    client::{MatchRequest, Pending, Reply, Service, Session},
    matchmaking::DedicatedMatch,
    state::{Error, Storage},
    status::*,
};
use runtime::{Guest, GuestFault, guest::Access};
use std::collections::{BTreeMap, BTreeSet};

const MAX_ATTRIBUTES: u32 = 64;
const MAX_SESSIONS: usize = 32;
const MAX_RESULTS: u32 = 1000;
const MAX_RESULTS_BYTES: u32 = 16 * 1024 * 1024;
const TYPE_MASK: u32 = 0x00f00000;
const STRING: u32 = 0x00100000;
const BLOB: u32 = 0x00200000;
const NULL: u32 = 0x00f00000;

pub use live_proto::session::{
    AttributeValue as SessionAttributeValue, Slots as SessionSlots, Visibility,
};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAttribute {
    pub id: u32,
    pub value: SessionAttributeValue,
}
fn slots(a: &[u32]) -> SessionSlots {
    SessionSlots {
        public_filled: a[0],
        public_open: a[1],
        private_filled: a[2],
        private_open: a[3],
    }
}
/// Snapshot of an advertised local session. Exchange keys stay private to the
/// backend/task and are never included in debug output or future event data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSession {
    pub id: [u8; 8],
    pub host_address: [u8; 36],
    pub host_users: Vec<u64>,
    pub slots: SessionSlots,
    pub attributes: BTreeMap<u32, SessionAttributeValue>,
    pub revision: u64,
    /// Who finds it in the server's lists (and so in the custom games
    /// browser): the host's setting when it was last advertised.
    pub visibility: Visibility,
    /// The host's loaded map, exactly (docs/game/maps.md §5), when a package
    /// serves it; None for the disc's own map or when nothing says.
    pub map_id: Option<live_proto::maps::MapId>,
    /// The skulls the host has on for its next game (docs/game/skulls.md "In
    /// multiplayer"), when any.
    pub skulls: Option<Vec<u8>>,
    /// A co-op campaign lobby's level and difficulty (docs/live/live-coop.md,
    /// "Co-op over Live"): advertised with `flags: "coop"`.
    pub campaign: Option<live_proto::session::Campaign>,
    /// A co-op lobby's measured round trip to its guest, in ms.
    pub ping_ms: Option<u16>,
    /// The game variant's h2recomp options for the host's next game
    /// (docs/game/game-options.md section 3), when the host set them.
    pub variant_ext: Option<live_proto::session::VariantExt>,
}
struct Record {
    // A title may advertise no capacity at all, notably in campaign co-op.
    needs_squad_slots: bool,
    session: LocalSession,
    key: [u8; 16],
}
/// A guest task. `HeldEmpty` is a clanker hopper's search
/// (docs/game/bots.md §8.4): no sessions, answered at `until` (running till
/// then).
enum Task {
    Session { id: [u8; 8], key: [u8; 16] },
    Done,
    EmptySearch,
    HeldEmpty { until: std::time::Instant },
    Search { array: u32, count: u32 },
    Pending { request: Pending, intent: Intent },
    Queue {
        search: Box<crate::matchmaking::Search>,
        intent: Intent,
    },
    Failed(u32),
}
enum Intent {
    Put {
        session: Box<LocalSession>,
        key: [u8; 16],
    },
    Delete([u8; 8]),
    Search {
        limit: u32,
        bytes: u32,
        retail: bool,
        /// FindFromID's session: the first step of a join, logged.
        find: Option<[u8; 8]>,
    },
}
pub(crate) struct Context<'a> {
    pub hle: &'a crate::Hle,
    pub storage: &'a mut Storage,
    pub host: [u8; 36],
    pub users: &'a [u64; 4],
    pub service: Option<&'a Service>,
    pub session: Option<&'a Session>,
    /// The server's clanker hoppers: their searches find nothing.
    pub clankers: &'a live_proto::hopper::clankers::ClankerHoppers,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Create,
    Update,
    Info,
    Delete,
    Find,
    Search,
    Results,
    Parse,
    ResultsLen,
}
impl Op {
    pub fn starts_task(self) -> bool {
        matches!(
            self,
            Self::Create | Self::Update | Self::Delete | Self::Find | Self::Search
        )
    }
}
#[derive(Default)]
pub(crate) struct Sessions {
    records: BTreeMap<[u8; 8], Record>,
    slot_update: Option<([u8; 8], SessionSlots, Pending)>,
    results: crate::qos_memory::QosMemory,
    tasks: BTreeMap<u32, Task>,
    /// The elected host gets an empty search, then publishes using this lease.
    matchmaking: Option<(live_proto::matchmaking::Assignment, std::time::Instant)>,
    /// The selected queue result. Keep the lease route for FindFromID and
    /// the frame policy for this published session alone.
    match_join: Option<MatchJoin>,
    /// The host's visibility setting, for new and updated advertisements.
    visibility: Visibility,
    /// Visibility updates sent outside a guest task; dropped once answered.
    background: Vec<Pending>,
    /// Reads the title's own party privacy, when the embedder knows where it
    /// is; it wins over the setting when a lobby is advertised.
    probe: Option<VisibilityProbe>,
    /// Reads the host's loaded map's `MapId` (boot: the cache header's map
    /// name through the VFS).
    map_probe: Option<MapProbe>,
    /// The host's multiplayer skulls, for new and updated advertisements.
    skulls: Option<Vec<u8>>,
    /// The host's variant options for its next game, for new and updated
    /// advertisements.
    variant_ext: Option<live_proto::session::VariantExt>,
    /// Reads them afresh (boot: the signed-in profiles' choice) each time
    /// the game advertises or updates a lobby.
    skulls_probe: Option<SkullsProbe>,
    /// The co-op campaign the host set up on its co-op screen, for new and
    /// updated advertisements.
    coop: Option<live_proto::session::Campaign>,
    /// The host's campaign skulls for that campaign; they replace the
    /// multiplayer ones while a co-op campaign is armed.
    coop_skulls: Option<Vec<u8>>,
    /// A co-op host's last measured round trip ([`Sessions::set_coop_ping`]).
    coop_ping: Option<u16>,
    /// The session advertised last: the lobby's current one.
    current: Option<[u8; 8]>,
    /// The sessions advertised, oldest first (at most [`MAX_SESSIONS`]
    /// kept): what [`Sessions::lobby_id`] picks from.
    advertised: Vec<[u8; 8]>,
    /// The sessions the title looked up (FindFromID): joins it started.
    looked_up: BTreeSet<[u8; 8]>,
    /// The last find line said and when ([`say_once_per`]).
    said_find: Option<(String, std::time::Instant)>,
    /// The last OptiMatch search line said and when.
    said_search: Option<(String, std::time::Instant)>,
    /// The players each hosted session was told of ([`Sessions::set_players`]).
    reported: BTreeMap<[u8; 8], BTreeSet<live_proto::Xuid>>,
    /// Clanker hoppers' search runs ([`crate::clankermatch::Hold`]).
    clanker_hold: crate::clankermatch::Hold,
}

struct MatchJoin {
    id: [u8; 8],
    frames_from_host: bool,
    dedicated: DedicatedMatch,
}
/// The players hosted session `id` holds: this console's signed-in
/// `users`, and `bots` when `id` is the session they were rostered for
/// (`bots_in`). The server leases a roster's bots to its session alone
/// and refuses them in any other (`unknown player`, a 400): the user's
/// Clankermatch run told the party's lobby session (advertised before the
/// search) of the match's bots, and every `PlayerJoined` and `PlayerLeft`
/// there was refused. Zero and host-made bot xuids are never reported.
pub(crate) fn session_players(
    id: [u8; 8],
    users: &[u64],
    bots_in: Option<[u8; 8]>,
    bots: &[live_proto::Xuid],
) -> BTreeSet<live_proto::Xuid> {
    let bots = if bots_in == Some(id) { bots } else { &[] };
    users
        .iter()
        .map(|u| live_proto::Xuid(*u))
        .chain(bots.iter().copied())
        .filter(|x| x.0 != 0 && !live_proto::bots::is_local_xuid(*x))
        .collect()
}
/// What a hosted session must be told to hold `want` when it was told of
/// `had`: (joined, left). Zero and host-made bot xuids are never reported:
/// the server knows neither.
pub(crate) fn players_diff(
    had: &BTreeSet<live_proto::Xuid>,
    want: &BTreeSet<live_proto::Xuid>,
) -> (Vec<live_proto::Xuid>, Vec<live_proto::Xuid>) {
    let real = |x: &&live_proto::Xuid| x.0 != 0 && !live_proto::bots::is_local_xuid(**x);
    (
        want.difference(had).filter(real).copied().collect(),
        had.difference(want).filter(real).copied().collect(),
    )
}
/// The host's multiplayer skulls as of now (None for none), read with guest
/// memory held: it must take no lock that guest memory may come after.
pub type SkullsProbe = std::sync::Arc<dyn Fn(&Guest) -> Option<Vec<u8>> + Send + Sync>;
/// Reads the `MapId` of the map the host has loaded, when a package serves
/// it; None for the disc's own map.
pub type MapProbe = std::sync::Arc<dyn Fn(&Guest) -> Option<live_proto::maps::MapId> + Send + Sync>;
/// Reads the host's visibility out of guest memory (boot: the squad
/// session's privacy), or None when there is none to read.
pub type VisibilityProbe = std::sync::Arc<dyn Fn(&Guest) -> Option<Visibility> + Send + Sync>;
enum Failure {
    Fault(GuestFault),
    Hr(u32),
}
impl From<GuestFault> for Failure {
    fn from(f: GuestFault) -> Self {
        Self::Fault(f)
    }
}
type Result<T> = std::result::Result<T, Failure>;
fn hr<T>(value: u32) -> Result<T> {
    Err(Failure::Hr(value))
}
impl Sessions {
    /// Backfill a zero-count advertisement from the actual hosted squad.
    /// An explicit slot advertisement (including reserved seats) wins.
    pub fn sync_squad_slots(
        &mut self,
        slots: SessionSlots,
        service: Option<&Service>,
        connection: Option<&Session>,
    ) {
        if let Some((id, sent, pending)) = &self.slot_update {
            let Some(reply) = pending.take() else { return };
            if reply.is_ok()
                && let Some(record) = self.records.get_mut(id)
                && record.needs_squad_slots
            {
                record.session.slots = *sent;
            }
            self.slot_update = None;
        }
        let Some(id) = self.current_id() else { return };
        let record = &self.records[&id];
        if !record.needs_squad_slots || record.session.slots == slots || !slots.validate() {
            return;
        }
        // An explicit update queued by the title comes first. Sending guessed
        // slots after it could override a newly reserved seat on the server.
        if self.tasks.values().any(|task| {
            matches!(
                task, Task::Pending { intent: Intent::Put { session, .. }, .. } if session.id == id
            )
        }) {
            return;
        }
        let (Some(service), Some(connection)) = (service, connection) else {
            return;
        };
        let request = MatchRequest::Update {
            id: live_proto::sign::hex(&id),
            update: live_proto::session::Update {
                slots: Some(slots),
                ..Default::default()
            },
        };
        if let Ok(pending) = service.match_request(connection, request) {
            self.slot_update = Some((id, slots, pending));
        }
    }

    pub fn snapshot(&self) -> Vec<LocalSession> {
        self.records.values().map(|r| r.session.clone()).collect()
    }
    /// The host's visibility setting changed: new advertisements carry it,
    /// and open ones are updated now (a signed `SessionUpdated`), so a lobby
    /// can be opened to the browser or closed to it while it waits.
    pub fn set_visibility(
        &mut self,
        v: Visibility,
        service: Option<&Service>,
        connection: Option<&Session>,
    ) {
        self.visibility = v;
        self.background.retain(|p| p.take().is_none());
        for (id, record) in &mut self.records {
            let v = coop_visibility(v, record.session.campaign.is_some());
            if record.session.visibility == v {
                continue;
            }
            if let (Some(service), Some(connection)) = (service, connection) {
                let request = MatchRequest::Update {
                    id: live_proto::sign::hex(id),
                    update: live_proto::session::Update {
                        visibility: Some(v),
                        ..Default::default()
                    },
                };
                match service.match_request(connection, request) {
                    Ok(p) => self.background.push(p),
                    Err(_) => continue,
                }
            }
            record.session.visibility = v;
        }
    }
    pub fn visibility(&self) -> Visibility {
        self.visibility
    }
    /// The host's multiplayer skulls changed (boot, at the lobby's Skulls
    /// screen and at each game start): new advertisements carry them, and
    /// open ones are updated now, so guests that look again see them.
    pub fn set_skulls(
        &mut self,
        skulls: Option<Vec<u8>>,
        service: Option<&Service>,
        connection: Option<&Session>,
    ) {
        self.skulls = skulls;
        self.background.retain(|p| p.take().is_none());
        for (id, record) in &mut self.records {
            if record.session.skulls == self.skulls {
                continue;
            }
            if let (Some(service), Some(connection)) = (service, connection) {
                let request = MatchRequest::Update {
                    id: live_proto::sign::hex(id),
                    update: live_proto::session::Update {
                        skulls: Some(self.skulls.clone().unwrap_or_default()),
                        ..Default::default()
                    },
                };
                match service.match_request(connection, request) {
                    Ok(p) => self.background.push(p),
                    Err(_) => continue,
                }
            }
            record.session.skulls = self.skulls.clone();
        }
    }
    /// The host's variant options for the game it is starting (boot, at
    /// each multiplayer start as a Live host): new advertisements carry
    /// them, and open ones are updated now, so guests that look again find
    /// them before their own start.
    pub fn set_variant_ext(
        &mut self,
        ext: Option<live_proto::session::VariantExt>,
        service: Option<&Service>,
        connection: Option<&Session>,
    ) {
        self.variant_ext = ext;
        self.background.retain(|p| p.take().is_none());
        let Some(ext) = self.variant_ext.clone() else {
            return;
        };
        for (id, record) in &mut self.records {
            if record.session.variant_ext.as_ref() == Some(&ext) {
                continue;
            }
            if let (Some(service), Some(connection)) = (service, connection) {
                let request = MatchRequest::Update {
                    id: live_proto::sign::hex(id),
                    update: live_proto::session::Update {
                        variant_ext: Some(ext.clone()),
                        ..Default::default()
                    },
                };
                match service.match_request(connection, request) {
                    Ok(p) => self.background.push(p),
                    Err(_) => continue,
                }
            }
            record.session.variant_ext = Some(ext.clone());
        }
    }
    /// The host's game holds `bots` beside its signed-in users (boot, from
    /// the bot driver while a roster is in force): each hosted session is
    /// told of its players with `PlayerJoined` and `PlayerLeft`, so the
    /// Live session counts them (docs/game/bots.md section 9.5). The bots
    /// are `session`'s alone, the one their roster leased them to
    /// ([`session_players`]); every other session this console advertises
    /// (a Clankermatch's party lobby beside its match) gets its users only.
    pub fn set_players(
        &mut self,
        session: Option<[u8; 8]>,
        bots: &[live_proto::Xuid],
        service: Option<&Service>,
        connection: Option<&Session>,
    ) {
        let records = &self.records;
        self.reported.retain(|id, _| records.contains_key(id));
        self.background.retain(|p| p.take().is_none());
        let (Some(service), Some(connection)) = (service, connection) else {
            return;
        };
        for (id, record) in &self.records {
            let want = session_players(*id, &record.session.host_users, session, bots);
            let had = self.reported.entry(*id).or_default();
            let (joined, left) = players_diff(had, &want);
            if joined.is_empty() && left.is_empty() {
                continue;
            }
            let request = MatchRequest::Players {
                id: live_proto::sign::hex(id),
                joined,
                left,
            };
            match service.match_request(connection, request) {
                Ok(p) => self.background.push(p),
                Err(_) => continue,
            }
            *had = want;
        }
    }
    /// The host armed (Some) or left (None) a co-op campaign on its co-op
    /// screen: new advertisements are `coop` sessions with the campaign, and
    /// open ones are updated now (a signed `SessionUpdated`), so the lobby
    /// the guest is invited to says what it plays.
    pub fn set_coop(
        &mut self,
        campaign: Option<live_proto::session::Campaign>,
        skulls: Option<Vec<u8>>,
        service: Option<&Service>,
        connection: Option<&Session>,
    ) {
        self.coop_skulls = campaign.as_ref().and(skulls);
        if campaign.is_none() {
            self.coop_ping = None;
        }
        self.coop = campaign;
        self.background.retain(|p| p.take().is_none());
        let visibility = self.visibility;
        let coop = self.coop.clone();
        let skulls = self.advertised_skulls();
        for (id, record) in &mut self.records {
            let want = coop_visibility(visibility, coop.is_some());
            let drop_ping = coop.is_none() && record.session.ping_ms.is_some();
            if record.session.campaign == coop
                && record.session.visibility == want
                && record.session.skulls == skulls
                && !drop_ping
            {
                continue;
            }
            if let (Some(service), Some(connection)) = (service, connection) {
                let request = MatchRequest::Update {
                    id: live_proto::sign::hex(id),
                    update: live_proto::session::Update {
                        flags: coop.is_none().then(|| "custom".to_owned()),
                        campaign: coop.clone(),
                        visibility: (record.session.visibility != want).then_some(want),
                        skulls: (record.session.skulls != skulls)
                            .then(|| skulls.clone().unwrap_or_default()),
                        ping_ms: drop_ping.then_some(0),
                        ..Default::default()
                    },
                };
                match service.match_request(connection, request) {
                    Ok(p) => self.background.push(p),
                    Err(_) => continue,
                }
            }
            record.session.campaign = coop.clone();
            if drop_ping {
                record.session.ping_ms = None;
            }
            record.session.visibility = want;
            record.session.skulls = skulls.clone();
        }
    }
    /// A co-op host's measured round trip to its guest (None: no guest or
    /// no answer): open co-op records are updated when it moved enough
    /// ([`ping_moved`]).
    pub fn set_coop_ping(
        &mut self,
        ms: Option<u16>,
        service: Option<&Service>,
        connection: Option<&Session>,
    ) {
        self.coop_ping = ms;
        self.background.retain(|p| p.take().is_none());
        for (id, record) in &mut self.records {
            let want = record.session.campaign.as_ref().and(ms);
            if !ping_moved(record.session.ping_ms, want) {
                continue;
            }
            if let (Some(service), Some(connection)) = (service, connection) {
                let request = MatchRequest::Update {
                    id: live_proto::sign::hex(id),
                    update: live_proto::session::Update {
                        ping_ms: Some(want.unwrap_or(0)),
                        ..Default::default()
                    },
                };
                match service.match_request(connection, request) {
                    Ok(p) => self.background.push(p),
                    Err(_) => continue,
                }
            }
            record.session.ping_ms = want;
        }
    }
    /// The skulls an advertisement carries: the co-op campaign's while one
    /// is armed, else the multiplayer ones.
    fn advertised_skulls(&self) -> Option<Vec<u8>> {
        if self.coop.is_some() {
            self.coop_skulls.clone()
        } else {
            self.skulls.clone()
        }
    }
    pub fn coop(&self) -> Option<&live_proto::session::Campaign> {
        self.coop.as_ref()
    }
    /// The ids of the sessions this console advertises.
    pub fn own_ids(&self) -> Vec<[u8; 8]> {
        self.records.keys().copied().collect()
    }
    /// The session this console advertised last, while it still does: the
    /// lobby's current one (a lobby that re-advertises gets a new id, and
    /// ids are random, so the smallest is no guide).
    pub fn current_id(&self) -> Option<[u8; 8]> {
        self.current.filter(|id| self.records.contains_key(id))
    }
    /// The playlist (hopper index) of the session this console advertised
    /// last, while it still does; `None` for a custom game's.
    pub fn current_playlist(&self) -> Option<u32> {
        let id = self.current_id()?;
        live_proto::optimatch::playlist(&self.records.get(&id)?.session.attributes)
    }
    fn advertised(&mut self, id: [u8; 8]) {
        self.current = Some(id);
        self.advertised
            .retain(|a| *a != id && self.records.contains_key(a));
        self.advertised.push(id);
    }
    /// The co-op lobby to invite to: the newest advertisement that carries
    /// a co-op campaign, so a session the console opened on its way (the
    /// Xbox Live menu's, a custom game's) is never taken for it; without
    /// one, the newest.
    pub fn lobby_id(&self) -> Option<[u8; 8]> {
        self.advertised
            .iter()
            .rev()
            .find(|id| {
                self.records
                    .get(*id)
                    .is_some_and(|r| r.session.campaign.is_some())
            })
            .copied()
            .or_else(|| self.current_id())
    }
    /// Whether the title looked `id` up (FindFromID) since sign-in.
    pub fn looked_up(&self, id: &[u8; 8]) -> bool {
        self.looked_up.contains(id)
    }
    pub fn set_probe(&mut self, probe: Option<VisibilityProbe>) {
        self.probe = probe;
    }
    pub fn set_map_probe(&mut self, probe: Option<MapProbe>) {
        self.map_probe = probe;
    }
    /// Publish a lobby selection even if the title's other advertisement
    /// attributes have stayed the same.
    pub fn lobby_map_changed(
        &mut self,
        guest: &Guest,
        service: Option<&Service>,
        connection: Option<&Session>,
    ) {
        let Some(probe) = &self.map_probe else { return };
        let Some(id) = self.current_id() else { return };
        let map_id = probe(guest);
        let Some(record) = self.records.get_mut(&id) else {
            return;
        };
        if record.session.map_id == map_id || record.session.campaign.is_some() {
            return;
        }
        if let (Some(service), Some(connection)) = (service, connection) {
            let request = MatchRequest::Update {
                id: live_proto::sign::hex(&id),
                update: live_proto::session::Update {
                    map_id,
                    clear_map_id: map_id.is_none(),
                    ..Default::default()
                },
            };
            let Ok(pending) = service.match_request(connection, request) else {
                return;
            };
            self.background.retain(|p| p.take().is_none());
            self.background.push(pending);
        }
        record.session.map_id = map_id;
        record.session.revision = record.session.revision.saturating_add(1);
    }

    pub fn set_skulls_probe(&mut self, probe: Option<SkullsProbe>) {
        self.skulls_probe = probe;
    }
    pub fn clear(&mut self) {
        self.records.clear();
        self.slot_update = None;
        self.looked_up.clear();
        self.tasks.clear();
        self.matchmaking = None;
        self.match_join = None;
        self.results.clear();
    }
    pub fn matchmaking_status(&self) -> Option<&str> {
        self.tasks
            .values()
            .rev()
            .find_map(|task| match task {
                Task::Queue { search, .. } => Some(search.status()),
                _ => None,
            })
            .flatten()
    }
    pub fn frames_from_host(&self, session_id: &str) -> bool {
        self.match_join.as_ref().is_some_and(|joined| {
            joined.frames_from_host && wire_id(session_id) == Some(joined.id)
        })
    }
    pub fn dedicated_match(&self, keys: &[[u8; 8]]) -> Option<(u32, String)> {
        let joined = self.match_join.as_ref().filter(|j| keys.contains(&j.id))?;
        Some((joined.dedicated.hopper, live_proto::sign::hex(&joined.id)))
    }
    pub fn end_match_join(&mut self, id: [u8; 8]) -> bool {
        if self.match_join.as_ref().is_some_and(|joined| joined.id == id) {
            let had_tunnel = self.has_match_tunnel();
            self.match_join = None;
            return had_tunnel;
        }
        false
    }
    pub fn has_match_tunnel(&self) -> bool {
        self.match_join
            .as_ref()
            .is_some_and(|joined| joined.dedicated.route.tunnel_url.is_some())
    }
    pub fn poll(
        &mut self,
        h: u32,
        m: &mut Guest,
        _storage: &mut Storage,
        hle: &crate::Hle,
    ) -> std::result::Result<Option<u32>, GuestFault> {
        let Some(task) = self.tasks.remove(&h) else {
            return Ok(None);
        };
        let task = match task {
            Task::HeldEmpty { until } if std::time::Instant::now() >= until => Task::EmptySearch,
            task => task,
        };
        let task = if let Task::Pending { request, intent } = task {
            match request.take() {
                None => Task::Pending { request, intent },
                Some(Err(e)) => {
                    if let Intent::Search { find: Some(id), .. } = &intent {
                        eprintln!(
                            "live: join: find session {}: the server failed it ({e:#x})",
                            live_proto::sign::hex(id)
                        );
                    }
                    Task::Failed(e)
                }
                Some(Ok(Reply::Matches(rows))) => match self.complete(m, intent, rows) {
                    Ok(v) => v,
                    Err(Failure::Hr(e)) => Task::Failed(e),
                    Err(Failure::Fault(f)) => return Err(f),
                },
                _ => Task::Failed(INVALID_ARGUMENT),
            }
        } else {
            task
        };
        let task = if let Task::Queue { mut search, intent } = task {
            use crate::matchmaking::Outcome;
            match search.poll() {
                Ok(None) => {
                    if search.status().is_some() {
                        self.matchmaking = None;
                    }
                    Task::Queue { search, intent }
                }
                Ok(Some(Outcome::Host(assignment))) => {
                    self.match_join = None;
                    self.matchmaking = Some((assignment, std::time::Instant::now()));
                    Task::EmptySearch
                }
                Ok(Some(Outcome::Join {
                    rows,
                    frames_from_host,
                    dedicated,
                    tunnel,
                })) => {
                    let id = rows.first().and_then(|row| wire_id(&row.session_id));
                    let installed = tunnel.is_some();
                    if let Some((address, handle)) = tunnel
                        && let Err(why) = hle.install_tunnel(address, handle)
                    {
                        eprintln!("live: could not install dedicated host tunnel: {why}");
                        search.retry_dedicated();
                        self.tasks.insert(h, Task::Queue { search, intent });
                        return Ok(Some(0));
                    }
                    self.match_join = None;
                    self.matchmaking = None;
                    match self.complete(m, intent, rows) {
                        Ok(v @ Task::Search { .. }) => {
                            self.match_join =
                                id.zip(dedicated).map(|(id, dedicated)| MatchJoin {
                                    id,
                                    frames_from_host,
                                    dedicated,
                                });
                            v
                        }
                        result => {
                            if installed {
                                hle.clear_tunnel();
                            }
                            match result {
                                Ok(v) => v,
                                Err(Failure::Hr(e)) => Task::Failed(e),
                                Err(Failure::Fault(f)) => return Err(f),
                            }
                        }
                    }
                }
                Err(e) => Task::Failed(e),
            }
        } else {
            task
        };
        let status = match task {
            Task::Pending { .. } | Task::HeldEmpty { .. } | Task::Queue { .. } => 0,
            Task::Failed(e) => e,
            _ => TASK_SUCCESS,
        };
        self.tasks.insert(h, task);
        Ok(Some(status))
    }
    pub fn close(&mut self, h: u32) -> bool {
        match self.tasks.remove(&h) {
            Some(Task::Search { array, .. }) => {
                self.results.release(array);
                true
            }
            Some(_) => true,
            None => false,
        }
    }
    pub fn invoke(
        &mut self,
        op: Op,
        a: &[u32],
        m: &mut Guest,
        ctx: Context<'_>,
    ) -> std::result::Result<u32, Error> {
        match self.checked(op, a, m, ctx) {
            Ok(v) => Ok(v),
            Err(Failure::Hr(v)) => Ok(v),
            Err(Failure::Fault(f)) => Err(Error::Fault(f)),
        }
    }
    fn checked(&mut self, op: Op, a: &[u32], m: &mut Guest, ctx: Context<'_>) -> Result<u32> {
        let Context {
            hle,
            storage,
            host,
            users,
            service,
            session: connection,
            clankers: ctx_clankers,
        } = ctx;
        if matches!(op, Op::Info | Op::Results) {
            let status = self
                .poll(a[0], m, storage, hle)?
                .ok_or(Failure::Hr(INVALID_ARGUMENT))?;
            if status == 0 {
                return hr(TASK_BUSY);
            }
            if status & 0x80000000 != 0 {
                return hr(status);
            }
        }
        match op {
            Op::Create | Op::Update => {
                let offset = usize::from(op == Op::Update) * 2;
                let p = &a[offset..];
                work_event(p[6])?;
                m.check(p[7], 4, Access::Write, 1)?;
                // Marshall everything before publishing a task or changing state.
                let attributes = attributes(m, p[5], p[4], op == Op::Create)?;
                for i in 0..p[4] {
                    m.check(p[5] + i * 16 + 4, 4, Access::Write, 1)?;
                }
                let slots = slots(p);
                if let Some(v) = self.probe.as_ref().and_then(|probe| probe(m)) {
                    self.visibility = v;
                }
                let map_id = self.map_probe.as_ref().and_then(|probe| probe(m));
                if let Some(probe) = &self.skulls_probe {
                    self.skulls = probe(m);
                }
                let (id, key, mut session) = if op == Op::Create {
                    if self.records.len() >= MAX_SESSIONS {
                        return hr(OUT_OF_MEMORY);
                    }
                    let (id, key) = self.identity()?;
                    (
                        id,
                        key,
                        LocalSession {
                            id,
                            host_address: host,
                            host_users: users.iter().copied().filter(|u| *u != 0).collect(),
                            slots,
                            attributes: BTreeMap::new(),
                            revision: 0,
                            visibility: self.visibility,
                            map_id,
                            skulls: self.advertised_skulls(),
                            campaign: self.coop.clone(),
                            ping_ms: self.coop.as_ref().and(self.coop_ping),
                            variant_ext: self.variant_ext.clone(),
                        },
                    )
                } else {
                    let id = id(a[0], a[1]);
                    let Some(record) = self.records.get(&id) else {
                        return hr(MATCH_INVALID_SESSION);
                    };
                    (id, record.key, record.session.clone())
                };
                session.slots = slots;
                session.visibility = coop_visibility(self.visibility, self.coop.is_some());
                session.skulls = self.advertised_skulls();
                if self.variant_ext.is_some() {
                    session.variant_ext = self.variant_ext.clone();
                }
                let was_coop = session.campaign.clone();
                session.campaign = self.coop.clone();
                session.map_id = map_id;
                for attr in attributes {
                    // Attribute identity excludes its datatype. A changed NULL
                    // clears the value; unchanged entries never dereference data.
                    session
                        .attributes
                        .retain(|old, _| old & !TYPE_MASK != attr.id & !TYPE_MASK);
                    if attr.value != SessionAttributeValue::Null {
                        session.attributes.insert(attr.id, attr.value);
                    }
                }
                if session.attributes.len() > MAX_ATTRIBUTES as usize {
                    return hr(BUFFER_TOO_SMALL);
                }
                let changed = self
                    .records
                    .get(&id)
                    .is_none_or(|old| old.session != session);
                if changed {
                    session.revision = session.revision.saturating_add(1);
                }
                let h = storage.alloc(m, 8).ok_or(Failure::Hr(OUT_OF_MEMORY))?;
                if let (Some(service), Some(connection)) = (service, connection) {
                    let data = advertisement(&session);
                    let request = if op == Op::Create {
                        let publish_window = std::time::Duration::from_secs(
                            live_proto::matchmaking::ASSIGNMENT_SECONDS as u64,
                        );
                        let assignment = self
                            .matchmaking
                            .as_ref()
                            .filter(|(lease, elected)| {
                                elected.elapsed() < publish_window
                                    && !data.is_coop()
                                    && live_proto::optimatch::playlist(&data.attributes)
                                        == Some(lease.hopper)
                            })
                            .map(|(lease, _)| lease.id.clone());
                        MatchRequest::Create {
                            session: data,
                            assignment,
                        }
                    } else {
                        MatchRequest::Update {
                            id: live_proto::sign::hex(&id),
                            update: live_proto::session::Update {
                                // Repeated zero-count updates leave capacity with the
                                // squad publisher. A pending capacity change may already
                                // be on the wire ahead of this update.
                                slots: (data.slots != SessionSlots::default()
                                    || self.records.get(&id).is_none_or(|r| !r.needs_squad_slots))
                                .then_some(data.slots),
                                attributes: Some(data.attributes),
                                // Only a change is sent, so a server that
                                // predates visibility sees the same updates.
                                visibility: self
                                    .records
                                    .get(&id)
                                    .is_none_or(|r| r.session.visibility != session.visibility)
                                    .then_some(session.visibility),
                                // Only a change, as for visibility.
                                flags: (was_coop.is_some() && session.campaign.is_none())
                                    .then(|| "custom".to_owned()),
                                campaign: session
                                    .campaign
                                    .clone()
                                    .filter(|c| was_coop.as_ref() != Some(c)),
                                map_id: session.map_id.filter(|m| {
                                    self.records
                                        .get(&id)
                                        .is_none_or(|r| r.session.map_id != Some(*m))
                                }),
                                clear_map_id: session.map_id.is_none()
                                    && self
                                        .records
                                        .get(&id)
                                        .is_some_and(|r| r.session.map_id.is_some()),
                                // Only a change, as for visibility; empty
                                // says the host turned them all off.
                                skulls: self
                                    .records
                                    .get(&id)
                                    .is_none_or(|r| r.session.skulls != session.skulls)
                                    .then(|| session.skulls.clone().unwrap_or_default()),
                                // Only a change, as for the skulls.
                                variant_ext: session.variant_ext.clone().filter(|v| {
                                    self.records
                                        .get(&id)
                                        .is_none_or(|r| r.session.variant_ext.as_ref() != Some(v))
                                }),
                                ..Default::default()
                            },
                        }
                    };
                    let pending = service
                        .match_request(connection, request)
                        .map_err(Failure::Hr)?;
                    self.tasks.insert(
                        h,
                        Task::Pending {
                            request: pending,
                            intent: Intent::Put {
                                session: Box::new(session),
                                key,
                            },
                        },
                    );
                } else {
                    if changed {
                        debug("advertise", &session);
                    }
                    if op == Op::Create {
                        self.advertised(id);
                    }
                    self.records.insert(
                        id,
                        Record {
                            needs_squad_slots: slots == SessionSlots::default(),
                            session,
                            key,
                        },
                    );
                    self.tasks.insert(h, Task::Session { id, key });
                }
                // All guest ranges were checked before submitting. Queue
                // failures leave output and dirty flags untouched.
                for i in 0..p[4] {
                    m.set_u32(p[5] + i * 16 + 4, 0)?;
                }
                m.set_u32(p[7], h)?;
                Ok(0)
            }
            Op::Info => {
                let Some(Task::Session { id, key }) = self.tasks.get(&a[0]) else {
                    return hr(INVALID_ARGUMENT);
                };
                m.check(a[1], 8, Access::Write, 1)?;
                m.check(a[2], 16, Access::Write, 1)?;
                m.write_bytes(a[1], id, 1)?;
                m.write_bytes(a[2], key, 1)?;
                Ok(0)
            }
            Op::Delete => {
                work_event(a[2])?;
                m.check(a[3], 4, Access::Write, 1)?;
                let id = id(a[0], a[1]);
                if !self.records.contains_key(&id) {
                    return hr(MATCH_INVALID_SESSION);
                }
                let h = storage.alloc(m, 8).ok_or(Failure::Hr(OUT_OF_MEMORY))?;
                if let (Some(service), Some(connection)) = (service, connection) {
                    let pending = service
                        .match_request(connection, MatchRequest::Delete(live_proto::sign::hex(&id)))
                        .map_err(Failure::Hr)?;
                    self.tasks.insert(
                        h,
                        Task::Pending {
                            request: pending,
                            intent: Intent::Delete(id),
                        },
                    );
                } else {
                    let record = self.records.remove(&id).unwrap();
                    debug("delete", &record.session);
                    self.tasks.insert(h, Task::Done);
                }
                m.set_u32(a[3], h)?;
                Ok(0)
            }
            Op::Search => {
                work_event(a[5])?;
                m.check(a[6], 4, Access::Write, 1)?;
                if a[1] > MAX_RESULTS || a[4] < 0x5c || a[4] > MAX_RESULTS_BYTES {
                    return hr(MATCH_INVALID_SEARCH);
                }
                let params = attributes(m, a[3], a[2], true)?;
                if std::env::var_os("H2_BOOT_VERBOSE").is_some() {
                    eprintln!(
                        "live match search debug: procedure={} limit={} params={params:?}",
                        a[0], a[1]
                    );
                }
                // A clanker hopper (docs/game/bots.md §8.4): nobody to
                // find. The run's searches answer with no sessions once its
                // delay is over, and the game hosts.
                let clanker = (a[0] == live_proto::optimatch::PROCEDURE)
                    .then(|| {
                        let values: Vec<_> = params.iter().map(|v| v.value.clone()).collect();
                        live_proto::optimatch::Query::from_params(&values)
                    })
                    .flatten()
                    .and_then(|q| crate::clankermatch::clanker_search(ctx_clankers, q.hopper));
                if let Some(hopper) = clanker {
                    let now = std::time::Instant::now();
                    let until = self.clanker_hold.answer_at(hopper, now, || {
                        let r = crate::network::random(4)
                            .ok()
                            .and_then(|b| b.get(..4)?.try_into().ok())
                            .map_or(0, u32::from_le_bytes);
                        let d = crate::clankermatch::search_delay(r);
                        eprintln!(
                            "live: clankermatch: searching clanker playlist {hopper}: no one to find; the console hosts in {:.1} s",
                            d.as_secs_f32()
                        );
                        d
                    });
                    let h = storage.alloc(m, 8).ok_or(Failure::Hr(OUT_OF_MEMORY))?;
                    m.set_u32(a[6], h)?;
                    self.tasks.insert(h, Task::HeldEmpty { until });
                    return Ok(0);
                }
                if let (Some(service), Some(connection)) = (service, connection) {
                    // Procedure 1 is the retail stored procedure: anonymous
                    // parameters are positional, not attribute IDs. Its return
                    // projection is the seven public title attributes. The
                    // twelve-parameter shape is OptiMatch or the custom games
                    // browser (live_proto::optimatch); the server filters it.
                    // Other procedures' named filters round-trip.
                    let optimatch = (a[0] == live_proto::optimatch::PROCEDURE)
                        .then(|| {
                            let values: Vec<_> = params.iter().map(|v| v.value.clone()).collect();
                            live_proto::optimatch::Query::from_params(&values)
                        })
                        .flatten();
                    if let Some(q) = &optimatch {
                        let line = optimatch_line(q);
                        if say_once_per(&mut self.said_search, &line, FIND_REPEAT) {
                            eprintln!("{line}");
                        }
                    }
                    if a[1] > 0
                        && let Some(query) = optimatch
                        && let Some(hopper) = crate::matchmaking::solo_hopper(query, users)
                        && let Some(connectivity) = crate::matchmaking::config()
                    {
                        let h = storage.alloc(m, 8).ok_or(Failure::Hr(OUT_OF_MEMORY))?;
                        let ticket = live_proto::matchmaking::Ticket {
                            hopper,
                            port: u16::from_be_bytes([host[8], host[9]]),
                            dedicated: false,
                            connectivity,
                            samples: Vec::new(),
                        };
                        let search =
                            crate::matchmaking::Search::new(service, connection, ticket, query, a[1])
                                .map_err(Failure::Hr)?;
                        m.set_u32(a[6], h)?;
                        self.tasks.insert(
                            h,
                            Task::Queue {
                                search: Box::new(search),
                                intent: Intent::Search {
                                    limit: a[1],
                                    bytes: a[4],
                                    retail: true,
                                    find: None,
                                },
                            },
                        );
                        return Ok(0);
                    }
                    let filters = if a[0] == 1 {
                        BTreeMap::new()
                    } else {
                        params
                            .into_iter()
                            .filter(|v| v.value != SessionAttributeValue::Null)
                            .map(|v| (v.id, v.value))
                            .collect()
                    };
                    self.remote_search(
                        m,
                        storage,
                        service,
                        connection,
                        MatchRequest::Search {
                            limit: a[1],
                            attributes: filters,
                            optimatch,
                        },
                        (
                            a[6],
                            Intent::Search {
                                limit: a[1],
                                bytes: a[4],
                                retail: a[0] == 1,
                                find: None,
                            },
                        ),
                    )
                } else {
                    self.empty_search(m, storage, a[6])
                }
            }
            Op::Find => {
                work_event(a[2])?;
                m.check(a[3], 4, Access::Write, 1)?;
                if self.looked_up.len() >= 64 {
                    self.looked_up.clear();
                }
                self.looked_up.insert(id(a[0], a[1]));
                if let (Some(service), Some(connection)) = (service, connection) {
                    self.remote_search(
                        m,
                        storage,
                        service,
                        connection,
                        MatchRequest::Find(live_proto::sign::hex(&id(a[0], a[1]))),
                        (
                            a[3],
                            Intent::Search {
                                limit: 1,
                                bytes: MAX_RESULTS_BYTES,
                                retail: true,
                                find: Some(id(a[0], a[1])),
                            },
                        ),
                    )
                } else {
                    eprintln!(
                        "live: join: find session {}: not signed in to a Live server",
                        live_proto::sign::hex(&id(a[0], a[1]))
                    );
                    self.empty_search(m, storage, a[3])
                }
            }
            Op::Results => {
                let (array, count) = match self.tasks.get(&a[0]) {
                    Some(Task::EmptySearch | Task::HeldEmpty { .. }) => (0, 0),
                    Some(Task::Search { array, count }) => (*array, *count),
                    _ => return hr(INVALID_ARGUMENT),
                };
                m.check(a[1], 4, Access::Write, 1)?;
                m.check(a[2], 4, Access::Write, 1)?;
                m.set_u32(a[1], array)?;
                m.set_u32(a[2], count)?;
                Ok(0)
            }
            Op::ResultsLen => results_len(m, a),
            Op::Parse => parse_result(m, a),
        }
    }
    fn remote_search(
        &mut self,
        m: &mut Guest,
        storage: &mut Storage,
        service: &Service,
        connection: &Session,
        request: MatchRequest,
        (out, intent): (u32, Intent),
    ) -> Result<u32> {
        let h = storage.alloc(m, 8).ok_or(Failure::Hr(OUT_OF_MEMORY))?;
        let pending = service
            .match_request(connection, request)
            .map_err(Failure::Hr)?;
        m.set_u32(out, h)?;
        self.tasks.insert(
            h,
            Task::Pending {
                request: pending,
                intent,
            },
        );
        Ok(0)
    }
    fn complete(
        &mut self,
        m: &mut Guest,
        intent: Intent,
        rows: Vec<live_proto::session::Session>,
    ) -> Result<Task> {
        match intent {
            Intent::Put { mut session, key } => {
                let row = rows
                    .into_iter()
                    .next()
                    .ok_or(Failure::Hr(MATCH_INVALID_SESSION))?;
                let id = wire_id(&row.session_id).ok_or(Failure::Hr(MATCH_INVALID_SESSION))?;
                if let Some(lease) = &row.matchmaking
                    && self.matchmaking.as_ref().is_some_and(|(held, _)| held.id == lease.id)
                {
                    self.matchmaking = None;
                }
                session.id = id;
                let needs_squad_slots = session.slots == SessionSlots::default();
                session.slots = row.advertisement.slots;
                session.attributes = row.advertisement.attributes;
                debug("server advertise", &session);
                if !self.records.contains_key(&id) {
                    eprintln!(
                        "live: session {} advertised for {}:{}",
                        live_proto::sign::hex(&id),
                        row.endpoint.addr,
                        row.endpoint.port
                    );
                    self.advertised(id);
                }
                self.records.insert(
                    id,
                    Record {
                        needs_squad_slots,
                        session: *session,
                        key,
                    },
                );
                Ok(Task::Session { id, key })
            }
            Intent::Delete(id) => {
                self.records.remove(&id);
                eprintln!("live: session {} deleted", live_proto::sign::hex(&id));
                Ok(Task::Done)
            }
            Intent::Search {
                limit,
                bytes,
                retail,
                find,
            } => {
                let mut rows: Vec<_> = rows
                    .into_iter()
                    .filter(|v| v.state == "open")
                    .take(limit as usize)
                    .collect();
                if let Some(joined) = self.match_join.as_ref().filter(|j| find == Some(j.id)) {
                    for row in &mut rows {
                        if wire_id(&row.session_id) == Some(joined.id)
                            && joined.dedicated.route.still_assigned(row)
                        {
                            joined.dedicated.route.apply(row);
                        }
                    }
                }
                if let Some(id) = find {
                    let line = find_line(&id, &rows);
                    if say_once_per(&mut self.said_find, &line, FIND_REPEAT) {
                        eprintln!("{line}");
                    }
                }
                if rows.is_empty() {
                    return Ok(Task::EmptySearch);
                }
                let encoded: Vec<_> = rows
                    .iter()
                    .map(|v| encode_result(v, retail))
                    .collect::<Result<_>>()?;
                let data_len: usize = encoded.iter().map(Vec::len).sum();
                if data_len + 0x5c > bytes as usize {
                    return hr(BUFFER_TOO_SMALL);
                }
                let total = data_len + rows.len() * 4;
                let array = self
                    .results
                    .alloc(m, total as u32)
                    .ok_or(Failure::Hr(OUT_OF_MEMORY))?;
                let mut ptr = array + rows.len() as u32 * 4;
                for (i, row) in encoded.iter().enumerate() {
                    m.write_bytes(ptr, row, 1)?;
                    m.set_u32(array + i as u32 * 4, ptr)?;
                    ptr += row.len() as u32;
                }
                Ok(Task::Search {
                    array,
                    count: rows.len() as u32,
                })
            }
        }
    }
    fn empty_search(&mut self, m: &mut Guest, storage: &mut Storage, out: u32) -> Result<u32> {
        let h = storage.alloc(m, 8).ok_or(Failure::Hr(OUT_OF_MEMORY))?;
        m.set_u32(out, h)?;
        self.tasks.insert(h, Task::EmptySearch);
        Ok(0)
    }
    fn identity(&self) -> Result<([u8; 8], [u8; 16])> {
        loop {
            let mut data = crate::network::random(24).map_err(|_| Failure::Hr(OUT_OF_MEMORY))?;
            data[0] = (data[0] & 0x0f) | 0x80; // XNET_XNKID_ONLINE_PEER
            let id: [u8; 8] = data[..8].try_into().unwrap();
            if !self.records.contains_key(&id) {
                return Ok((id, data[8..].try_into().unwrap()));
            }
        }
    }
}
/// A find line repeated within this is not said again. The main menu's
/// friend view looks a friend's session up every frame until the title is
/// signed in to Live (`0x002b6690`; the result is dropped at `0x000901c1`
/// until the logon task's result is 1), sixty lines a second.
const FIND_REPEAT: std::time::Duration = std::time::Duration::from_secs(30);
/// Whether to say `line`: not if it was the last said, within `within`.
fn say_once_per(
    last: &mut Option<(String, std::time::Instant)>,
    line: &str,
    within: std::time::Duration,
) -> bool {
    if let Some((said, at)) = last
        && said == line
        && at.elapsed() < within
    {
        return false;
    }
    *last = Some((line.to_owned(), std::time::Instant::now()));
    true
}
/// What FindFromID found: the host's endpoint (QoS answers on its port
/// + 2), or why the join stops here.
fn find_line(id: &[u8; 8], rows: &[live_proto::session::Session]) -> String {
    let id = live_proto::sign::hex(id);
    match rows.first() {
        None => format!("live: join: find session {id}: no such session on the server"),
        Some(r) if r.state != "open" => {
            format!("live: join: find session {id}: the session is {}", r.state)
        }
        Some(r) => {
            let s = r.advertisement.slots;
            format!(
                "live: join: find session {id}: {} hosts it at {}:{} ({}), slots open: {} of {} \
                 public, {} of {} private",
                r.host_gamertag,
                r.endpoint.addr,
                r.endpoint.port,
                r.endpoint.source,
                s.public_open,
                s.public_open + s.public_filled,
                s.private_open,
                s.private_open + s.private_filled,
            )
        }
    }
}
fn debug(action: &str, session: &LocalSession) {
    if std::env::var_os("H2_BOOT_VERBOSE").is_some() {
        eprintln!("live session {action} debug: {session:?}");
    }
}
fn id(lo: u32, hi: u32) -> [u8; 8] {
    (u64::from(lo) | u64::from(hi) << 32).to_le_bytes()
}
fn work_event(event: u32) -> Result<()> {
    if event == 0 {
        Ok(())
    } else {
        hr(INVALID_ARGUMENT)
    }
}
fn attributes(m: &Guest, p: u32, count: u32, all: bool) -> Result<Vec<SessionAttribute>> {
    if count > MAX_ATTRIBUTES {
        return hr(BUFFER_TOO_SMALL);
    }
    m.check(p, count as usize * 16, Access::Read, 1)?;
    let mut output = Vec::new();
    for i in 0..count {
        let at = p + i * 16;
        if !all && m.u32(at + 4)? == 0 {
            continue;
        }
        let id = m.u32(at)?;
        let value = match id & TYPE_MASK {
            0 => SessionAttributeValue::Integer(m.u64(at + 8)?),
            STRING => SessionAttributeValue::String(string(m, m.u32(at + 8)?)?),
            BLOB => {
                let n = m.u32(at + 12)?;
                if n > 800 {
                    return hr(MATCH_BLOB_TOO_LONG);
                }
                let mut bytes = vec![0; n as usize];
                m.read_bytes(m.u32(at + 8)?, &mut bytes, 1)?;
                SessionAttributeValue::Blob(bytes)
            }
            NULL => SessionAttributeValue::Null,
            _ => return hr(MATCH_INVALID_DATA_TYPE),
        };
        output.push(SessionAttribute { id, value });
    }
    Ok(output)
}
fn string(m: &Guest, p: u32) -> Result<String> {
    let mut units = Vec::new();
    for i in 0..=400 {
        let value = m.u16(p.wrapping_add(i * 2))?;
        if value == 0 {
            return String::from_utf16(&units).map_err(|_| Failure::Hr(MATCH_INVALID_STRING));
        }
        units.push(value);
    }
    hr(MATCH_STRING_TOO_LONG)
}
fn results_len(m: &Guest, a: &[u32]) -> Result<u32> {
    // DWORD return: refuse unsafe/overflowing capacities with zero rather than
    // turning a validation HRESULT into a giant guest allocation.
    if a[0] > MAX_RESULTS || a[1] > MAX_ATTRIBUTES {
        return Ok(0);
    }
    m.check(a[2], a[1] as usize * 8, Access::Read, 1)?;
    let mut per = 0x54u64;
    for i in 0..a[1] {
        let at = a[2] + i * 8;
        let ty = m.u32(at)?;
        let len = m.u32(at + 4)?;
        let extra = match ty {
            0 => 8,
            STRING if len <= 400 => u64::from(len) * 2 + 2,
            BLOB if len <= 800 => u64::from(len) + 2,
            NULL => 0,
            _ => return Ok(0),
        };
        per += 8 + extra;
    }
    let total = 0x5c + u64::from(a[0]) * per;
    Ok(if total > u64::from(MAX_RESULTS_BYTES) {
        0
    } else {
        total as u32
    })
}

fn advertisement(s: &LocalSession) -> live_proto::session::NewSession {
    live_proto::session::NewSession {
        addr: Some(std::net::Ipv4Addr::new(
            s.host_address[0],
            s.host_address[1],
            s.host_address[2],
            s.host_address[3],
        )),
        port: u16::from_be_bytes([s.host_address[8], s.host_address[9]]),
        host_address: s.host_address.to_vec(),
        slots: s.slots,
        attributes: s.attributes.clone(),
        flags: if s.campaign.is_some() {
            live_proto::session::COOP
        } else {
            "custom"
        }
        .into(),
        hopper_id: None,
        map: None,
        name: None,
        language: None,
        visibility: s.visibility,
        map_id: s.map_id,
        skulls: s.skulls.clone(),
        campaign: s.campaign.clone(),
        ping_ms: s.ping_ms,
        variant_ext: s.variant_ext.clone(),
    }
}
/// Whether a new round trip is worth a session update: it appeared, went,
/// or moved by at least 5 ms and a tenth (a lobby pings every two seconds;
/// jitter alone sends nothing).
pub fn ping_moved(old: Option<u16>, new: Option<u16>) -> bool {
    match (old, new) {
        (Some(a), Some(b)) => {
            let d = a.abs_diff(b);
            d >= 5 && u32::from(d) * 10 >= u32::from(a.min(b))
        }
        (a, b) => a.is_some() != b.is_some(),
    }
}
/// A co-op lobby is friends-only unless the host made it invite-only.
pub fn coop_visibility(v: Visibility, coop: bool) -> Visibility {
    if coop && v == Visibility::Public {
        Visibility::Friends
    } else {
        v
    }
}
/// One line per distinct OptiMatch search, so a run log shows the window
/// widen as the game re-searches.
fn optimatch_line(q: &live_proto::optimatch::Query) -> String {
    if q.hopper == -1 {
        return "live: browser: searching for custom games".into();
    }
    let window = match (q.lowest, q.highest) {
        (None, None) => "any level".to_string(),
        (lo, hi) => format!(
            "levels {}..{}",
            lo.map_or("*".into(), |v| v.to_string()),
            hi.map_or("*".into(), |v| v.to_string())
        ),
    };
    let party = q.party.map_or("?".into(), |n| n.to_string());
    format!(
        "live: optimatch: searching playlist {} for a party of {party}, mean level {}, {window}",
        q.hopper, q.average
    )
}
pub(crate) fn wire_id(s: &str) -> Option<[u8; 8]> {
    if !live_proto::session::valid_id(s) {
        return None;
    }
    Some(u64::from_str_radix(s, 16).ok()?.to_be_bytes())
}
fn encode_result(s: &live_proto::session::Session, retail: bool) -> Result<Vec<u8>> {
    let id = wire_id(&s.session_id).ok_or(Failure::Hr(MATCH_INVALID_SESSION))?;
    let values: Vec<_> = if retail {
        // The title's procedure 1 projects IDs 1,2,3,5,6,7,8, in that order
        // (0xb4b80 / 0xb4a40). The other attributes are search-only flags.
        [1, 2, 3, 5, 6, 7, 8]
            .into_iter()
            .map(|id| {
                s.advertisement
                    .attributes
                    .get(&id)
                    .cloned()
                    .unwrap_or(SessionAttributeValue::Integer(0))
            })
            .collect()
    } else {
        s.advertisement.attributes.values().cloned().collect()
    };
    let mut bytes = vec![0; 0x54 + values.len() * 4];
    bytes[4..12].copy_from_slice(&id);
    if s.advertisement.host_address.len() != 36 {
        return hr(MATCH_INVALID_SESSION);
    }
    bytes[12..48].copy_from_slice(&s.advertisement.host_address);
    bytes[12..16].copy_from_slice(&s.endpoint.addr.octets());
    bytes[16..20].copy_from_slice(&s.endpoint.addr.octets());
    bytes[20..22].copy_from_slice(&s.endpoint.port.to_be_bytes());
    // XNKEY +0x30 stays zero. HLE sockets use no encryption; the host key
    // never appears in a public event or read response.
    let slots = s.advertisement.slots;
    for (offset, v) in [
        (0x40, slots.public_open),
        (0x44, slots.private_open),
        (0x48, slots.public_filled),
        (0x4c, slots.private_filled),
        (0x50, values.len() as u32),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
    }
    for (i, value) in values.into_iter().enumerate() {
        let offset = bytes.len() as u32;
        bytes[0x54 + i * 4..0x58 + i * 4].copy_from_slice(&offset.to_le_bytes());
        match value {
            SessionAttributeValue::Integer(n) => {
                bytes.extend_from_slice(&0u32.to_le_bytes());
                bytes.extend_from_slice(&n.to_le_bytes());
            }
            SessionAttributeValue::String(s) => {
                let mut utf8 = s.into_bytes();
                utf8.push(0);
                bytes.extend_from_slice(&STRING.to_le_bytes());
                bytes.extend_from_slice(&(utf8.len() as u16).to_le_bytes());
                bytes.extend(utf8);
            }
            SessionAttributeValue::Blob(b) => {
                bytes.extend_from_slice(&BLOB.to_le_bytes());
                bytes.extend_from_slice(&(b.len() as u16).to_le_bytes());
                bytes.extend(b);
            }
            SessionAttributeValue::Null => bytes.extend_from_slice(&NULL.to_le_bytes()),
        }
    }
    let size = bytes.len() as u32;
    bytes[..4].copy_from_slice(&size.to_le_bytes());
    Ok(bytes)
}
fn parse_result(m: &mut Guest, a: &[u32]) -> Result<u32> {
    if a[0] == 0 {
        return hr(INVALID_ARGUMENT);
    }
    m.check(a[0], 0x54, Access::Read, 1)?;
    let count = m.u32(a[0] + 0x50)?;
    if count == 0 {
        return Ok(0);
    }
    if count > MAX_ATTRIBUTES || count > a[1] {
        return hr(MATCH_INVALID_RESULT_COLUMN);
    }
    let size = m.u32(a[0])?;
    if size < 0x54 + count * 4 || size > MAX_RESULTS_BYTES {
        return hr(MATCH_INVALID_RESULT_COLUMN);
    }
    m.check(a[0], size as usize, Access::Read, 1)?;
    m.check(a[2], count as usize * 8, Access::Read, 1)?;
    let mut output = Vec::new();
    for i in 0..count {
        let offset = m.u32(a[0] + 0x54 + i * 4)?;
        if offset < 0x54 + count * 4 || offset.checked_add(4).is_none_or(|v| v > size) {
            return hr(MATCH_INVALID_RESULT_COLUMN);
        }
        let at = a[0] + offset;
        let ty = m.u32(at)?;
        let spec = m.u32(a[2] + i * 8)?;
        let cap = m.u32(a[2] + i * 8 + 4)?;
        if ty != spec && ty != NULL {
            return hr(MATCH_INVALID_DATA_TYPE);
        }
        match spec {
            0 => {
                if ty == NULL {
                    output.extend_from_slice(&0u64.to_le_bytes());
                } else {
                    if offset + 12 > size {
                        return hr(MATCH_INVALID_RESULT_COLUMN);
                    }
                    output.extend_from_slice(&m.u64(at + 4)?.to_le_bytes());
                }
            }
            STRING | BLOB => {
                if cap > 800 {
                    return hr(BUFFER_TOO_SMALL);
                }
                let data = if ty == NULL {
                    Vec::new()
                } else {
                    if offset + 6 > size {
                        return hr(MATCH_INVALID_RESULT_COLUMN);
                    }
                    let len = u32::from(m.u16(at + 4)?);
                    if offset + 6 + len > size {
                        return hr(MATCH_INVALID_RESULT_COLUMN);
                    }
                    let mut data = vec![0; len as usize];
                    m.read_bytes(at + 6, &mut data, 1)?;
                    data
                };
                if spec == BLOB {
                    if data.len() > cap as usize {
                        return hr(BUFFER_TOO_SMALL);
                    }
                    output.extend_from_slice(&(data.len() as u16).to_le_bytes());
                    let start = output.len();
                    output.resize(start + cap as usize, 0);
                    output[start..start + data.len()].copy_from_slice(&data);
                } else {
                    let text = std::str::from_utf8(&data)
                        .map_err(|_| Failure::Hr(MATCH_INVALID_STRING))?;
                    let text = text.trim_end_matches('\0');
                    let encoded: Vec<_> = text
                        .encode_utf16()
                        .chain([0])
                        .flat_map(u16::to_le_bytes)
                        .collect();
                    if encoded.len() > cap as usize {
                        return hr(BUFFER_TOO_SMALL);
                    }
                    let start = output.len();
                    output.resize(start + cap as usize, 0);
                    output[start..start + encoded.len()].copy_from_slice(&encoded);
                }
            }
            _ => return hr(MATCH_INVALID_DATA_TYPE),
        }
    }
    m.write_bytes(a[3], &output, 1)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ping_update_waits_for_a_real_change() {
        assert!(ping_moved(None, Some(30)), "appeared");
        assert!(ping_moved(Some(30), None), "went");
        assert!(!ping_moved(None, None));
        assert!(!ping_moved(Some(30), Some(33)), "jitter");
        assert!(ping_moved(Some(30), Some(36)));
        assert!(!ping_moved(Some(200), Some(215)), "under a tenth");
        assert!(ping_moved(Some(200), Some(225)));
    }

    #[test]
    fn dedicated_match_uses_its_published_id_hopper_and_frame_policy() {
        use crate::matchmaking::Route;
        use live_proto::hosts::{HostEndpoint, Transport};

        let id = "8000000000000002";
        let key = wire_id(id).unwrap();
        let party = wire_id("8000000000000001").unwrap();
        let endpoint = HostEndpoint {
            transport: Transport::Udp,
            udp: Some("192.0.2.10:31000".into()),
            tunnel_url: None,
            xnaddr: "00".repeat(36),
            session_id: Some(id.into()),
        };
        let route = Route::from_endpoint(&endpoint, "https://live.example/v1", "hl-test").unwrap();
        let mut sessions = Sessions {
            match_join: Some(MatchJoin {
                id: key,
                frames_from_host: true,
                dedicated: DedicatedMatch { route, hopper: 3 },
            }),
            ..Default::default()
        };

        assert_eq!(sessions.dedicated_match(&[party, key]), Some((3, id.into())));
        assert_eq!(sessions.dedicated_match(&[party]), None);
        assert!(!sessions.frames_from_host("8000000000000001"));
        sessions.end_match_join(party);
        assert!(sessions.frames_from_host(id));
        sessions.end_match_join(key);
        assert!(!sessions.frames_from_host(id));
    }
}

#[cfg(test)]
mod say_tests {
    use super::*;

    #[test]
    fn a_repeated_find_line_is_said_once_per_window() {
        let mut last = None;
        let window = std::time::Duration::from_secs(30);
        assert!(say_once_per(&mut last, "a", window));
        assert!(!say_once_per(&mut last, "a", window));
        assert!(say_once_per(&mut last, "b", window));
        assert!(say_once_per(&mut last, "a", window));
        assert!(say_once_per(&mut last, "a", std::time::Duration::ZERO));
    }
}

#[cfg(test)]
mod players_tests {
    use super::{players_diff, session_players};
    use live_proto::Xuid;
    use std::collections::BTreeSet;

    fn set(x: &[u64]) -> BTreeSet<Xuid> {
        x.iter().map(|x| Xuid(*x)).collect()
    }

    #[test]
    fn a_session_is_told_only_of_the_players_that_changed() {
        let host = 0x0009_0000_0000_0001;
        let bot = 0x000b_0000_0000_0002;
        let other = 0x000b_0000_0000_0003;
        assert_eq!(
            players_diff(&set(&[]), &set(&[host, bot])),
            (vec![Xuid(host), Xuid(bot)], vec![])
        );
        assert_eq!(
            players_diff(&set(&[host, bot]), &set(&[host, bot])),
            (vec![], vec![])
        );
        assert_eq!(
            players_diff(&set(&[host, bot]), &set(&[host, other])),
            (vec![Xuid(other)], vec![Xuid(bot)])
        );
    }

    /// The user's Clankermatch run: the party's lobby session
    /// (84ad6bf112ae91df, advertised before the search) and the match the
    /// search hosted (893954eb98f2e2fe) were both this console's, and the
    /// match's bots went to both; the party's refused each one (400).
    #[test]
    fn bots_go_only_to_the_session_they_were_rostered_for() {
        let host = 0x0009_0000_0000_0001;
        let bot = Xuid(0x000b_0000_0000_0002);
        let party = 0x84ad_6bf1_12ae_91dfu64.to_be_bytes();
        let game = 0x8939_54eb_98f2_e2feu64.to_be_bytes();
        assert_eq!(
            session_players(game, &[host], Some(game), &[bot]),
            set(&[host, bot.0])
        );
        assert_eq!(
            session_players(party, &[host], Some(game), &[bot]),
            set(&[host]),
            "the party's lobby gets its users only"
        );
        // No roster in force: users only, so bots told before leave.
        assert_eq!(session_players(game, &[host], None, &[]), set(&[host]));
        assert_eq!(session_players(game, &[host], None, &[bot]), set(&[host]));
        // The game moved (a re-host): the bots follow the new session.
        let next = 0x80f4_7828_6be3_a884u64.to_be_bytes();
        assert_eq!(
            session_players(game, &[host], Some(next), &[bot]),
            set(&[host])
        );
        assert_eq!(
            session_players(next, &[host], Some(next), &[bot]),
            set(&[host, bot.0])
        );
    }

    #[test]
    fn zero_and_host_made_bots_are_never_reported() {
        let local = live_proto::bots::local_xuid(4).0;
        assert_eq!(players_diff(&set(&[]), &set(&[0, local])), (vec![], vec![]));
        assert_eq!(players_diff(&set(&[0, local]), &set(&[])), (vec![], vec![]));
    }
}
