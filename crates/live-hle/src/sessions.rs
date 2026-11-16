//! Public match ABI over local advertisements or asynchronous server discovery.
//! Exchange keys remain local; remote search records carry a zero HLE key.
use crate::{
    client::{MatchRequest, Pending, Reply, Service, Session},
    state::{Error, Storage},
    status::*,
};
use runtime::{Guest, GuestFault, guest::Access};
use std::collections::BTreeMap;

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
}
struct Record {
    session: LocalSession,
    key: [u8; 16],
}
enum Task {
    Session { id: [u8; 8], key: [u8; 16] },
    Done,
    EmptySearch,
    Search { array: u32, count: u32 },
    Pending { request: Pending, intent: Intent },
    Failed(u32),
}
enum Intent {
    Put {
        session: LocalSession,
        key: [u8; 16],
    },
    Delete([u8; 8]),
    Search {
        limit: u32,
        bytes: u32,
        retail: bool,
    },
}
pub(crate) struct Context<'a> {
    pub storage: &'a mut Storage,
    pub host: [u8; 36],
    pub users: &'a [u64; 4],
    pub service: Option<&'a Service>,
    pub session: Option<&'a Session>,
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
    results: crate::qos_memory::QosMemory,
    tasks: BTreeMap<u32, Task>,
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
    /// Reads them afresh (boot: the signed-in profiles' choice) each time
    /// the game advertises or updates a lobby.
    skulls_probe: Option<SkullsProbe>,
}
/// The host's multiplayer skulls as of now (None for none), read with guest
/// memory held: it must take no lock that guest memory may come after.
pub type SkullsProbe = std::sync::Arc<dyn Fn(&Guest) -> Option<Vec<u8>> + Send + Sync>;
/// Reads the `MapId` of the map the host has loaded, when a package serves
/// it; None for the disc's own map.
pub type MapProbe =
    std::sync::Arc<dyn Fn(&Guest) -> Option<live_proto::maps::MapId> + Send + Sync>;
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
    /// The ids of the sessions this console advertises.
    pub fn own_ids(&self) -> Vec<[u8; 8]> {
        self.records.keys().copied().collect()
    }
    pub fn set_probe(&mut self, probe: Option<VisibilityProbe>) {
        self.probe = probe;
    }
    pub fn set_map_probe(&mut self, probe: Option<MapProbe>) {
        self.map_probe = probe;
    }
    pub fn set_skulls_probe(&mut self, probe: Option<SkullsProbe>) {
        self.skulls_probe = probe;
    }
    pub fn clear(&mut self) {
        self.records.clear();
        self.tasks.clear();
        self.results.clear();
    }
    pub fn poll(
        &mut self,
        h: u32,
        m: &mut Guest,
        _storage: &mut Storage,
    ) -> std::result::Result<Option<u32>, GuestFault> {
        let Some(task) = self.tasks.remove(&h) else {
            return Ok(None);
        };
        let task = if let Task::Pending { request, intent } = task {
            match request.take() {
                None => Task::Pending { request, intent },
                Some(Err(e)) => Task::Failed(e),
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
        let status = match task {
            Task::Pending { .. } => 0,
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
            storage,
            host,
            users,
            service,
            session: connection,
        } = ctx;
        if matches!(op, Op::Info | Op::Results) {
            let status = self
                .poll(a[0], m, storage)?
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
                            skulls: self.skulls.clone(),
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
                session.visibility = self.visibility;
                session.skulls = self.skulls.clone();
                // A session keeps its map id once it has one (the wire has
                // no way back to none).
                if map_id.is_some() {
                    session.map_id = map_id;
                }
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
                        MatchRequest::Create(data)
                    } else {
                        MatchRequest::Update {
                            id: live_proto::sign::hex(&id),
                            update: live_proto::session::Update {
                                slots: Some(data.slots),
                                attributes: Some(data.attributes),
                                // Only a change is sent, so a server that
                                // predates visibility sees the same updates.
                                visibility: self
                                    .records
                                    .get(&id)
                                    .is_none_or(|r| r.session.visibility != self.visibility)
                                    .then_some(self.visibility),
                                map_id: session.map_id.filter(|m| {
                                    self.records
                                        .get(&id)
                                        .is_none_or(|r| r.session.map_id != Some(*m))
                                }),
                                // Only a change, as for visibility; empty
                                // says the host turned them all off.
                                skulls: self
                                    .records
                                    .get(&id)
                                    .is_none_or(|r| r.session.skulls != session.skulls)
                                    .then(|| session.skulls.clone().unwrap_or_default()),
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
                            intent: Intent::Put { session, key },
                        },
                    );
                } else {
                    if changed {
                        debug("advertise", &session);
                    }
                    self.records.insert(id, Record { session, key });
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
                if let (Some(service), Some(connection)) = (service, connection) {
                    // Procedure 1 is the retail stored procedure: anonymous
                    // parameters are positional, not attribute IDs. Its return
                    // projection is the seven public title attributes. Until
                    // their filter meanings are pinned, do not invent equality
                    // tests for anonymous params. Named API filters round-trip.
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
                        },
                        (
                            a[6],
                            Intent::Search {
                                limit: a[1],
                                bytes: a[4],
                                retail: a[0] == 1,
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
                            },
                        ),
                    )
                } else {
                    self.empty_search(m, storage, a[3])
                }
            }
            Op::Results => {
                let (array, count) = match self.tasks.get(&a[0]) {
                    Some(Task::EmptySearch) => (0, 0),
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
                session.id = id;
                session.slots = row.advertisement.slots;
                session.attributes = row.advertisement.attributes;
                debug("server advertise", &session);
                self.records.insert(id, Record { session, key });
                Ok(Task::Session { id, key })
            }
            Intent::Delete(id) => {
                self.records.remove(&id);
                Ok(Task::Done)
            }
            Intent::Search {
                limit,
                bytes,
                retail,
            } => {
                let rows: Vec<_> = rows
                    .into_iter()
                    .filter(|v| v.state == "open")
                    .take(limit as usize)
                    .collect();
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
        flags: "custom".into(),
        hopper_id: None,
        map: None,
        name: None,
        language: None,
        visibility: s.visibility,
        map_id: s.map_id,
        skulls: s.skulls.clone(),
    }
}
fn wire_id(s: &str) -> Option<[u8; 8]> {
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
